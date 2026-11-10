// tests for sms::inbound: what is done with an inbound message. sending is in
// outbound_tests, and the forecast path needs MongoDB, so it isn't tested
use crate::messaging;
use crate::sms::{inbound, reply, request};
use crate::state;
use crate::tests;

const USER: &str = "+15415550123";
const TOLL_FREE: &str = "+18005550100";
const LONG_CODE: &str = "+15415550100";

fn number(value: &str) -> messaging::PhoneNumber {
    messaging::PhoneNumber::new(value).unwrap()
}

fn numbers() -> state::Numbers {
    state::Numbers::from_values(Some(TOLL_FREE), Some(LONG_CODE)).unwrap()
}

fn text(to: &str, body: &str) -> messaging::InboundMessage {
    messaging::InboundMessage {
        from: number(USER),
        to: number(to),
        body: body.to_string(),
        received_at: chrono::Utc::now(),
        provider_message_id: "test".to_string(),
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
    tests::set_support_email();
    let plan = inbound::plan(&text(LONG_CODE, "hello"), &numbers());
    let inbound::Plan::Reply {
        reply_from,
        message,
    } = plan
    else {
        panic!("a message that isn't a request should get the help message");
    };
    assert_eq!(reply_from, number(LONG_CODE));
    assert!(message.contains(&format!("Help: {}", tests::SUPPORT_EMAIL)));
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
    tests::set_support_email();
    let responses = inbound::with_opt_in_message(true, forecast_reply());
    assert_eq!(responses[0], vec![reply::help_message()]);
    assert_eq!(responses[1..], forecast_reply()[..]);
}

#[test]
fn the_opt_in_message_is_left_out_once_sent() {
    tests::set_support_email();
    assert_eq!(
        inbound::with_opt_in_message(false, forecast_reply()),
        forecast_reply()
    );
}

// a first text that isn't a request gets the help message, which is the
// opt-in message, so it isn't sent twice
#[test]
fn the_help_reply_is_not_sent_twice() {
    tests::set_support_email();
    let help = vec![vec![reply::help_message()]];
    assert_eq!(inbound::with_opt_in_message(true, help.clone()), help);
}
