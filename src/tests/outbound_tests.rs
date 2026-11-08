// tests for sms::outbound: sending responses in order with the retry policy,
// with tokio time paused so the retry delays run instantly
use crate::messaging;
use crate::sms::outbound;
use crate::tests::fake_sender::{self, FakeSender};

const USER: &str = "+15415550123";
const TOLL_FREE: &str = "+18005550100";
const LONG_CODE: &str = "+15415550100";

fn number(value: &str) -> messaging::PhoneNumber {
    messaging::PhoneNumber::new(value).unwrap()
}

// one response of the given number of messages
fn one_response(count: usize) -> Vec<Vec<String>> {
    vec![fake_sender::messages(count)]
}

// a reply with more alerts than fit in MAX_MESSAGES continues in another
// response, numbered from (1/n) again, and is sent right after the first
#[tokio::test(start_paused = true)]
async fn responses_are_sent_one_after_another() {
    let sender = FakeSender::default();
    let responses = vec![fake_sender::messages(2), fake_sender::messages(1)];
    let deliveries =
        outbound::send_responses(&sender, &number(TOLL_FREE), &number(USER), &responses).await;
    assert_eq!(sender.attempted(), ["(1/2)", "(2/2)", "(1/1)"]);
    assert_eq!(deliveries.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn messages_are_sent_in_order_from_the_number_texted() {
    let sender = FakeSender::default();
    let deliveries =
        outbound::send_responses(&sender, &number(LONG_CODE), &number(USER), &one_response(3))
            .await;
    assert_eq!(sender.attempted(), fake_sender::messages(3));
    assert!(deliveries.iter().all(|delivery| delivery.result.is_ok()));
    let attempts = sender.attempts.lock().unwrap();
    assert!(
        attempts
            .iter()
            .all(|(from, to, _)| *from == number(LONG_CODE) && *to == number(USER))
    );
}

// the message is retried before the next is sent, so they arrive in order
#[tokio::test(start_paused = true)]
async fn a_transient_failure_is_retried_before_the_next_message() {
    let sender = FakeSender::scripted(vec![fake_sender::transient(), fake_sender::transient()]);
    let started = tokio::time::Instant::now();
    let deliveries =
        outbound::send_responses(&sender, &number(TOLL_FREE), &number(USER), &one_response(2))
            .await;
    assert_eq!(sender.attempted(), ["(1/2)", "(1/2)", "(1/2)", "(2/2)"]);
    assert!(deliveries.iter().all(|delivery| delivery.result.is_ok()));
    // about 2 then 8 seconds, plus up to half a second of jitter each
    let waited = started.elapsed();
    assert!(
        waited >= std::time::Duration::from_secs(10)
            && waited <= std::time::Duration::from_secs(11),
        "waited {waited:?}"
    );
}

// the rest may hold alerts, and the gap in the numbering shows something is missing
#[tokio::test(start_paused = true)]
async fn a_message_that_keeps_failing_is_skipped_and_the_rest_are_sent() {
    let sender = FakeSender::scripted(vec![
        fake_sender::transient(),
        fake_sender::transient(),
        fake_sender::transient(),
    ]);
    let deliveries =
        outbound::send_responses(&sender, &number(TOLL_FREE), &number(USER), &one_response(2))
            .await;
    assert_eq!(sender.attempted(), ["(1/2)", "(1/2)", "(1/2)", "(2/2)"]);
    assert!(matches!(
        deliveries[0].result,
        Err(messaging::SendError::Transient(_))
    ));
    assert!(deliveries[1].result.is_ok());
}

// every message would fail the same way, so the rest aren't attempted
#[tokio::test(start_paused = true)]
async fn a_permanent_failure_or_an_opt_out_stops_the_reply() {
    for error in [
        messaging::SendError::Permanent("invalid number".to_string()),
        messaging::SendError::OptedOut,
    ] {
        let sender = FakeSender::scripted(vec![fake_sender::sent(), Err(error)]);
        let deliveries =
            outbound::send_responses(&sender, &number(TOLL_FREE), &number(USER), &one_response(3))
                .await;
        // not retried, and (3/3) is never attempted
        assert_eq!(sender.attempted(), ["(1/3)", "(2/3)"]);
        assert_eq!(deliveries.len(), 2);
        assert!(deliveries[1].result.is_err());
    }
}
