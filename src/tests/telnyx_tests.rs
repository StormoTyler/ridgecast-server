// tests for the Telnyx provider: its configuration, and its sender against a
// local mock of the Telnyx API
use base64::Engine;
use wiremock::matchers::{bearer_token, body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::messaging;
use crate::sms_providers::telnyx;

const API_KEY: &str = "KEY0123456789ABCDEF";

// the public key of a fixed signing key, so the tests need no random numbers
fn public_key_bytes() -> [u8; 32] {
    ed25519_dalek::SigningKey::from_bytes(&[7; 32])
        .verifying_key()
        .to_bytes()
}

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// the same key in X.509 SubjectPublicKeyInfo form
fn spki(key: &[u8; 32]) -> Vec<u8> {
    let mut bytes = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    bytes.extend_from_slice(key);
    bytes
}

fn config_error(api_key: Option<&str>, public_key: Option<&str>) -> telnyx::ConfigError {
    telnyx::Config::from_values(api_key, public_key).expect_err("should be rejected")
}

#[test]
fn a_raw_public_key_is_accepted() {
    let public_key = base64(&public_key_bytes());
    assert!(telnyx::Config::from_values(Some(API_KEY), Some(&public_key)).is_ok());
}

#[test]
fn an_spki_public_key_is_accepted() {
    let public_key = base64(&spki(&public_key_bytes()));
    assert!(telnyx::Config::from_values(Some(API_KEY), Some(&public_key)).is_ok());
}

// a value pasted into .env with spaces or a trailing newline still works
#[test]
fn surrounding_whitespace_is_ignored() {
    let public_key = format!(" {}\n", base64(&public_key_bytes()));
    assert!(telnyx::Config::from_values(Some(" KEY0123 "), Some(&public_key)).is_ok());
}

#[test]
fn the_api_key_is_required() {
    let public_key = base64(&public_key_bytes());
    for unset in [None, Some(""), Some("  ")] {
        assert_eq!(
            config_error(unset, Some(&public_key)),
            telnyx::ConfigError::Missing("TELNYX_API_KEY")
        );
    }
}

#[test]
fn the_public_key_is_required() {
    for unset in [None, Some(""), Some("  ")] {
        assert_eq!(
            config_error(Some(API_KEY), unset),
            telnyx::ConfigError::Missing("TELNYX_PUBLIC_KEY")
        );
    }
}

#[test]
fn a_public_key_that_isnt_base64_is_rejected() {
    assert_eq!(
        config_error(Some(API_KEY), Some("not base64!")),
        telnyx::ConfigError::InvalidPublicKey
    );
}

// such as a key copied short, or another kind of key
#[test]
fn a_public_key_of_another_length_is_rejected() {
    let key = public_key_bytes();
    let mut long = key.to_vec();
    long.push(0);
    for bytes in [&key[..31], &long[..], &spki(&key)[1..]] {
        assert_eq!(
            config_error(Some(API_KEY), Some(&base64(bytes))),
            telnyx::ConfigError::InvalidPublicKey
        );
    }
}

// an X25519 key in SPKI form is also 44 bytes, differing only in the
// algorithm id, so it must not be read as an Ed25519 key
#[test]
fn a_44_byte_key_of_another_algorithm_is_rejected() {
    let mut bytes = spki(&public_key_bytes());
    // X25519's algorithm id, 1.3.101.110, in place of Ed25519's 1.3.101.112
    bytes[8] = 0x6e;
    assert_eq!(
        config_error(Some(API_KEY), Some(&base64(&bytes))),
        telnyx::ConfigError::InvalidPublicKey
    );
}

#[test]
fn the_api_key_is_left_out_of_debug_output() {
    let public_key = base64(&public_key_bytes());
    let config = telnyx::Config::from_values(Some(API_KEY), Some(&public_key)).unwrap();
    let debug = format!("{config:?}");
    assert!(!debug.contains(API_KEY), "{debug}");
}

// a configuration error never repeats the value it rejected
#[test]
fn errors_leave_the_values_out() {
    let error = config_error(Some(API_KEY), Some("not base64!"));
    let message = error.to_string();
    assert!(!message.contains(API_KEY) && !message.contains("not base64!"));
}

// ---- the sender ----

const FROM: &str = "+18005550100";
const TO: &str = "+15415550123";

async fn send_to(server: &MockServer) -> Result<messaging::SendReceipt, messaging::SendError> {
    let sender = telnyx::TelnyxSender::new(server.uri(), API_KEY);
    messaging::MessageSender::send(
        &sender,
        &messaging::PhoneNumber::new(FROM).unwrap(),
        &messaging::PhoneNumber::new(TO).unwrap(),
        "(1/1) RidgeCast\nhello",
    )
    .await
}

// a server answering every send with this status and JSON body
async fn server_answering(status: u16, body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/messages"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    server
}

fn telnyx_error(code: &str, title: &str) -> serde_json::Value {
    serde_json::json!({
        "errors": [{
            "code": code,
            "title": title,
            "detail": format!("The destination number {TO} is not reachable"),
        }]
    })
}

// the request Telnyx documents: the API key as a bearer token, and the numbers
// and text as JSON. only a request matching all of it gets the receipt
#[tokio::test]
async fn a_sent_message_returns_its_telnyx_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/messages"))
        .and(bearer_token(API_KEY))
        .and(body_json(serde_json::json!({
            "from": FROM,
            "to": TO,
            "text": "(1/1) RidgeCast\nhello",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": { "id": "40385f64-5717-4562-b3fc-2c963f66afa6", "type": "message" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let receipt = send_to(&server).await.unwrap();
    assert_eq!(
        receipt.provider_message_id,
        "40385f64-5717-4562-b3fc-2c963f66afa6"
    );
}

// Telnyx accepted the message, so it was sent: an error would stop the rest
// of the reply or resend it
#[tokio::test]
async fn an_accepted_message_with_an_unreadable_body_is_sent() {
    let server = server_answering(200, serde_json::json!({ "unexpected": true })).await;
    let receipt = send_to(&server).await.unwrap();
    assert_eq!(receipt.provider_message_id, "");
}

#[tokio::test]
async fn server_errors_and_rate_limits_are_transient() {
    for status in [500, 502, 503, 429, 408] {
        let server = server_answering(status, telnyx_error("10007", "Unexpected error")).await;
        let error = send_to(&server).await.unwrap_err();
        assert!(
            matches!(error, messaging::SendError::Transient(_)),
            "{status}: {error:?}"
        );
    }
}

#[tokio::test]
async fn a_server_that_cant_be_reached_is_transient() {
    // nothing listens on port 1
    let sender = telnyx::TelnyxSender::new("http://127.0.0.1:1", API_KEY);
    let error = messaging::MessageSender::send(
        &sender,
        &messaging::PhoneNumber::new(FROM).unwrap(),
        &messaging::PhoneNumber::new(TO).unwrap(),
        "hello",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, messaging::SendError::Transient(_)),
        "{error:?}"
    );
}

// whatever the status, so the core stops sending to a user who replied STOP
#[tokio::test]
async fn a_stop_block_is_opted_out() {
    for status in [400, 403] {
        let server =
            server_answering(status, telnyx_error("40300", "Blocked due to STOP message")).await;
        let error = send_to(&server).await.unwrap_err();
        assert!(
            matches!(error, messaging::SendError::OptedOut),
            "{status}: {error:?}"
        );
    }
}

// an invalid number, an unverified sending number, a bad API key, or no balance
#[tokio::test]
async fn other_rejections_are_permanent_and_name_the_code() {
    for (status, code) in [
        (400, "40310"),
        (403, "40329"),
        (401, "10009"),
        (402, "10038"),
    ] {
        let server = server_answering(status, telnyx_error(code, "Rejected")).await;
        let error = send_to(&server).await.unwrap_err();
        let messaging::SendError::Permanent(message) = error else {
            panic!("{status} {code}: {error:?}");
        };
        assert!(message.contains(code), "{message}");
        assert!(message.contains(&status.to_string()), "{message}");
    }
}

#[tokio::test]
async fn a_rejection_without_a_json_body_is_permanent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string("Bad Request"))
        .mount(&server)
        .await;
    let error = send_to(&server).await.unwrap_err();
    let messaging::SendError::Permanent(message) = error else {
        panic!("{error:?}");
    };
    assert_eq!(message, "telnyx 400 Bad Request");
}

// Telnyx's detail can name the number, so the error message leaves it out:
// the core logs it
#[tokio::test]
async fn an_error_never_names_the_numbers() {
    for status in [400, 500] {
        let server = server_answering(status, telnyx_error("40310", "Invalid 'to' address")).await;
        let message = send_to(&server).await.unwrap_err().to_string();
        for number in [FROM, TO, &TO[2..], &FROM[2..]] {
            assert!(!message.contains(number), "{message}");
        }
    }
}
