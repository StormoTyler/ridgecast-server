// tests for choosing the SMS provider and for the local provider. the dev route
// needs an AppState, which needs MongoDB, so it is checked by running the server
use crate::messaging;
use crate::sms::outbound;
use crate::sms_providers::{self, local};

#[test]
fn local_is_a_provider() {
    assert!(sms_providers::from_value(Some("local")).is_ok());
}

// there is no default, so a server can't fall back to printing its replies
#[test]
fn sms_provider_is_required() {
    for unset in [None, Some(""), Some("  ")] {
        let error = sms_providers::from_value(unset)
            .err()
            .expect("should be rejected");
        assert_eq!(error, "SMS_PROVIDER must be set, to one of: local");
    }
}

// a typo stops the server rather than falling through to some provider
#[test]
fn an_unknown_provider_is_rejected_with_the_choices() {
    let error = sms_providers::from_value(Some("telnix"))
        .err()
        .expect("should be rejected");
    assert_eq!(
        error,
        "SMS_PROVIDER \"telnix\" is not a provider, use one of: local"
    );
}

#[tokio::test]
async fn the_local_sender_never_fails_and_numbers_its_receipts() {
    let sender = local::LocalSender::default();
    let from = messaging::PhoneNumber::new("+18005550100").unwrap();
    let to = messaging::PhoneNumber::new("+15415550123").unwrap();
    for expected in ["local-1", "local-2"] {
        let receipt = messaging::MessageSender::send(&sender, &from, &to, "(1/1)\nhello")
            .await
            .unwrap();
        assert_eq!(receipt.provider_message_id, expected);
    }
}

fn delivery(body: &str, result: Result<(), messaging::SendError>) -> outbound::Delivery {
    outbound::Delivery {
        body: body.to_string(),
        result: result.map(|()| messaging::SendReceipt {
            provider_message_id: "local-1".to_string(),
        }),
    }
}

#[test]
fn the_dev_route_shows_messages_as_a_phone_would() {
    let deliveries = [
        delivery("(1/2)\nAV3 Considerable", Ok(())),
        delivery("(2/2)\nTu 34/22 P90", Ok(())),
    ];
    assert_eq!(
        local::format_deliveries(&deliveries),
        "(1/2)\nAV3 Considerable\n\n(2/2)\nTu 34/22 P90\n"
    );
}

#[test]
fn the_dev_route_marks_a_message_that_failed() {
    let deliveries = [delivery(
        "(1/1)\nhello",
        Err(messaging::SendError::Permanent(
            "invalid number".to_string(),
        )),
    )];
    assert_eq!(
        local::format_deliveries(&deliveries),
        "(not sent: message rejected: invalid number)\n(1/1)\nhello\n"
    );
}

// the core dropped the text, so the route says why instead of returning nothing
#[test]
fn the_dev_route_explains_a_dropped_text() {
    assert_eq!(
        local::format_deliveries(&[]),
        "(no reply: not sent to one of this server's numbers, dropped)\n"
    );
}
