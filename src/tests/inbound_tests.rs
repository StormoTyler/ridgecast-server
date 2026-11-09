// tests for sms::inbound: what is done with an inbound message. sending is in
// outbound_tests, and the forecast path needs MongoDB, so it isn't tested
use crate::messaging;
use crate::sms::{inbound, reply, request};
use crate::state;

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
    let plan = inbound::plan(&text(LONG_CODE, "hello"), &numbers());
    let inbound::Plan::Reply {
        reply_from,
        message,
    } = plan
    else {
        panic!("a message that isn't a request should get the help message");
    };
    assert_eq!(reply_from, number(LONG_CODE));
    assert!(message.contains("Help: support@ridgecast.app"));
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
