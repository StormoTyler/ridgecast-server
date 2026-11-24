// tests for sms::inbound: what is done with an inbound message. sending is in
// outbound_tests. inbound::handle needs MongoDB, so its tests are ignored by
// default (see test_db), and its forecast path calls NWS, so it isn't tested
use crate::messaging;
use crate::sms::{inbound, reply, request, user_numbers};
use crate::state;
use crate::tests;
use crate::tests::{fake_sender, models_tests, test_db};

const USER: &str = "+15415550123";
const TOLL_FREE: &str = "+18005550100";
const LONG_CODE: &str = "+15415550100";

fn number(value: &str) -> messaging::PhoneNumber {
    messaging::PhoneNumber::new(value).unwrap()
}

fn numbers() -> state::Numbers {
    state::Numbers::from_values(Some(TOLL_FREE), Some(LONG_CODE)).unwrap()
}

// a new text each call, with its own provider id, so handle doesn't drop it as
// a repeated delivery
fn text(to: &str, body: &str) -> messaging::InboundMessage {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    messaging::InboundMessage {
        from: number(USER),
        to: number(to),
        body: body.to_string(),
        received_at: chrono::Utc::now(),
        provider_message_id: format!("test-{id}"),
    }
}

// another number on the same provider account, for example
#[test]
fn a_message_to_another_number_is_dropped() {
    let plan = inbound::plan(
        &text("+15035550199", "RC1 45.3735 -121.6959 0000 0 1"),
        &numbers(),
    );
    assert!(matches!(plan, inbound::Plan::Drop));
}

#[test]
fn a_request_is_answered_from_the_number_it_was_sent_to() {
    for to in [TOLL_FREE, LONG_CODE] {
        let plan = inbound::plan(&text(to, "RC1 45.3735 -121.6959 0000 0 1"), &numbers());
        let inbound::Plan::Forecast {
            reply_from,
            request,
        } = plan
        else {
            panic!("a valid request to {to} should get a forecast");
        };
        assert_eq!(reply_from, number(to));
        assert_eq!(request.forecast_length, 1);
    }
}

#[test]
fn a_message_that_isnt_a_request_gets_the_help_message() {
    for body in ["HELP", "hello"] {
        let plan = inbound::plan(&text(LONG_CODE, body), &numbers());
        let inbound::Plan::Help { reply_from } = plan else {
            panic!("{body} isn't a request, so it should get the help message");
        };
        assert_eq!(reply_from, number(LONG_CODE));
    }
}

#[test]
fn an_invalid_request_gets_its_parse_error() {
    let plan = inbound::plan(&text(TOLL_FREE, "RC1 95.1 -121.6959 0000 0 1"), &numbers());
    let inbound::Plan::Reply { message, .. } = plan else {
        panic!("an invalid request should get its parse error");
    };
    let error = "RC1 95.1 -121.6959 0000 0 1"
        .parse::<request::SmsRequest>()
        .unwrap_err();
    assert_eq!(message, reply::format_parse_error(&error));
}

fn forecast_reply() -> Vec<Vec<String>> {
    vec![vec![
        "(1/2) RidgeCast\nforecast".to_string(),
        "(2/2) RidgeCast\nmore".to_string(),
    ]]
}

// a number with no record in sms::user_numbers gets the opt-in message as its own message,
// before the reply
#[test]
fn the_opt_in_message_comes_first_when_needed() {
    let responses = inbound::with_opt_in_message(true, forecast_reply());
    assert_eq!(responses[0], vec![reply::opt_in_message()]);
    assert_eq!(responses[1..], forecast_reply()[..]);
}

#[test]
fn the_opt_in_message_is_left_out_once_sent() {
    assert_eq!(
        inbound::with_opt_in_message(false, forecast_reply()),
        forecast_reply()
    );
}

fn number_key() -> user_numbers::NumberKey {
    user_numbers::NumberKey::from_value(Some("0123456789abcdef0123456789abcdef")).unwrap()
}

const INVALID_REQUEST: &str = "RC1 95.1 -121.6959 0000 0 1";

fn parse_error() -> String {
    let error = INVALID_REQUEST.parse::<request::SmsRequest>().unwrap_err();
    reply::format_parse_error(&error)
}

// the bodies of the messages handle sent, in order
fn bodies(deliveries: &[crate::sms::outbound::Delivery]) -> Vec<String> {
    deliveries
        .iter()
        .map(|delivery| delivery.body.clone())
        .collect()
}

// the opt-in message is its own message before the reply, sent from the number
// the user texted, and only with a number's first text
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_numbers_first_text_gets_the_opt_in_message_once() {
    tests::set_support_email();
    let db = test_db::fresh("first_text").await;
    let sender = fake_sender::FakeSender::default();
    let message = || text(LONG_CODE, INVALID_REQUEST);

    let first = inbound::handle(&message(), &numbers(), &number_key(), &db, &sender).await;
    assert_eq!(bodies(&first), vec![reply::opt_in_message(), parse_error()]);

    let second = inbound::handle(&message(), &numbers(), &number_key(), &db, &sender).await;
    assert_eq!(bodies(&second), vec![parse_error()]);

    for (from, to, _) in sender.attempts.lock().unwrap().iter() {
        assert_eq!(from, &number(LONG_CODE));
        assert_eq!(to, &number(USER));
    }
    test_db::remove(db).await;
}

// a first text that isn't a request, such as HELP, gets only the help message,
// since asking for help isn't asking for forecasts or alerts. it doesn't count
// as the opt-in, so the number's first request still gets the opt-in message
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn help_is_sent_without_the_opt_in_message() {
    tests::set_support_email();
    let db = test_db::fresh("help_reply").await;
    let sender = fake_sender::FakeSender::default();

    for _ in 0..2 {
        let help = inbound::handle(
            &text(TOLL_FREE, "HELP"),
            &numbers(),
            &number_key(),
            &db,
            &sender,
        )
        .await;
        assert_eq!(bodies(&help), vec![reply::help_message()]);
        assert!(user_numbers::needs_opt_in_message(&db, &number_key(), &number(USER)).await);
    }

    let request = inbound::handle(
        &text(TOLL_FREE, INVALID_REQUEST),
        &numbers(),
        &number_key(),
        &db,
        &sender,
    )
    .await;
    assert_eq!(
        bodies(&request),
        vec![reply::opt_in_message(), parse_error()]
    );
    test_db::remove(db).await;
}

// an opt-in message that wasn't delivered isn't recorded, so the number's next
// text is sent it again
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn an_undelivered_opt_in_message_is_sent_again() {
    tests::set_support_email();
    let db = test_db::fresh("undelivered_opt_in").await;
    let rejected = Err(messaging::SendError::Permanent("unreachable".to_string()));
    let sender = fake_sender::FakeSender::scripted(vec![rejected]);
    let message = || text(TOLL_FREE, INVALID_REQUEST);

    // a permanent failure stops the rest of the reply
    let first = inbound::handle(&message(), &numbers(), &number_key(), &db, &sender).await;
    assert_eq!(bodies(&first), vec![reply::opt_in_message()]);
    assert!(first[0].result.is_err());

    let second = inbound::handle(&message(), &numbers(), &number_key(), &db, &sender).await;
    assert_eq!(
        bodies(&second),
        vec![reply::opt_in_message(), parse_error()]
    );
    test_db::remove(db).await;
}

// a text to a number that isn't the server's sends nothing and records nothing
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_dropped_text_sends_and_records_nothing() {
    tests::set_support_email();
    let db = test_db::fresh("dropped_text").await;
    let sender = fake_sender::FakeSender::default();

    let deliveries = inbound::handle(
        &text("+15035550199", "hello"),
        &numbers(),
        &number_key(),
        &db,
        &sender,
    )
    .await;

    assert!(deliveries.is_empty());
    assert!(sender.attempted().is_empty());
    assert!(user_numbers::needs_opt_in_message(&db, &number_key(), &number(USER)).await);
    test_db::remove(db).await;
}

// a provider can deliver the same text again, such as a webhook retried after a
// slow answer: it is answered once, and the repeat sends and records nothing
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_repeated_delivery_is_answered_once() {
    tests::set_support_email();
    let db = test_db::fresh("repeated_delivery").await;
    let sender = fake_sender::FakeSender::default();
    let message = text(TOLL_FREE, "HELP");

    let first = inbound::handle(&message, &numbers(), &number_key(), &db, &sender).await;
    assert_eq!(bodies(&first), vec![reply::help_message()]);

    let repeat = inbound::handle(&message, &numbers(), &number_key(), &db, &sender).await;
    assert!(repeat.is_empty());
    assert_eq!(sender.attempted(), vec![reply::help_message()]);

    // another text is still answered
    let next = inbound::handle(
        &text(TOLL_FREE, "HELP"),
        &numbers(),
        &number_key(),
        &db,
        &sender,
    )
    .await;
    assert_eq!(bodies(&next), vec![reply::help_message()]);
    test_db::remove(db).await;
}

async fn add_trackers(users: &[&str], db: &crate::database::DB) {
    for user in users {
        let mut tracker = models_tests::tracker();
        tracker.user = number(user);
        crate::database::add_tracker(&tracker, db).await.unwrap();
    }
}

async fn trackers_of(user: &str, db: &crate::database::DB) -> u64 {
    db.points_collection
        .count_documents(mongodb::bson::doc! { "user": user })
        .await
        .unwrap()
}

const OTHER_USER: &str = "+15415550199";

// a STOP the provider reported deletes every tracker of the user and no one
// else's, and sends nothing
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn an_opt_out_deletes_the_users_trackers() {
    let db = test_db::fresh("delete_trackers").await;
    add_trackers(&[USER, USER, OTHER_USER], &db).await;

    inbound::delete_trackers(&number(USER), &db).await;

    assert_eq!(trackers_of(USER, &db).await, 0);
    assert_eq!(trackers_of(OTHER_USER, &db).await, 1);
    test_db::remove(db).await;
}

// a reply the provider refuses because the user replied STOP earlier deletes
// their trackers, so a tracker their request created doesn't wait for its
// first check to be deleted
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_reply_refused_as_opted_out_deletes_the_users_trackers() {
    tests::set_support_email();
    let db = test_db::fresh("reply_opted_out").await;
    add_trackers(&[USER, OTHER_USER], &db).await;
    let sender = fake_sender::FakeSender::scripted(vec![Err(messaging::SendError::OptedOut)]);

    inbound::handle(
        &text(TOLL_FREE, "HELP"),
        &numbers(),
        &number_key(),
        &db,
        &sender,
    )
    .await;

    assert_eq!(trackers_of(USER, &db).await, 0);
    assert_eq!(trackers_of(OTHER_USER, &db).await, 1);
    test_db::remove(db).await;
}

// a permanent failure may be the server's, such as an unverified number, so it
// deletes nothing
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_reply_refused_for_another_reason_keeps_the_trackers() {
    tests::set_support_email();
    let db = test_db::fresh("reply_permanent").await;
    add_trackers(&[USER], &db).await;
    let rejected = Err(messaging::SendError::Permanent("unreachable".to_string()));
    let sender = fake_sender::FakeSender::scripted(vec![rejected]);

    inbound::handle(
        &text(TOLL_FREE, "HELP"),
        &numbers(),
        &number_key(),
        &db,
        &sender,
    )
    .await;

    assert_eq!(trackers_of(USER, &db).await, 1);
    test_db::remove(db).await;
}
