// tests for the Telnyx provider: its configuration, its sender against a local
// mock of the Telnyx API, its webhook signature check, and its webhook route.
// the route's end-to-end test needs MongoDB, so it is ignored by default (see
// test_db)
use base64::Engine;
use wiremock::matchers::{bearer_token, body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use mongodb::bson::{self, doc};

use crate::database;
use crate::messaging;
use crate::sms::{reply, user_numbers};
use crate::sms_providers::telnyx;
use crate::state;
use crate::tests;
use crate::tests::{fake_sender, models_tests, test_db};

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

// SMS_PROVIDER=telnyx builds the provider from its configuration. its routes
// are tested below with the webhook
#[test]
fn a_provider_is_built_from_its_configuration() {
    let public_key = base64(&public_key_bytes());
    let config = telnyx::Config::from_values(Some(API_KEY), Some(&public_key)).unwrap();
    let _provider = telnyx::provider(config);
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

// ---- webhook signatures ----

const BODY: &[u8] = br#"{"data":{"event_type":"message.received"}}"#;

fn signing_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[7; 32])
}

// 2026-11-23 12:00:00 UTC
fn webhook_now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(1_795_435_200, 0).unwrap()
}

// the telnyx-signature-ed25519 header Telnyx would send for this timestamp and body
fn sign(key: &ed25519_dalek::SigningKey, timestamp: &str, body: &[u8]) -> String {
    let mut signed = format!("{timestamp}|").into_bytes();
    signed.extend_from_slice(body);
    base64(&ed25519_dalek::Signer::sign(key, &signed).to_bytes())
}

// a webhook signed with the test key, sent this many seconds before webhook_now
fn check_webhook_sent(seconds_ago: i64) -> Result<(), telnyx::SignatureError> {
    let timestamp = (webhook_now().timestamp() - seconds_ago).to_string();
    let signature = sign(&signing_key(), &timestamp, BODY);
    telnyx::verify_signature(
        &signing_key().verifying_key(),
        Some(&timestamp),
        Some(&signature),
        BODY,
        webhook_now(),
    )
}

#[test]
fn a_signed_webhook_is_accepted() {
    assert_eq!(check_webhook_sent(2), Ok(()));
}

// more than 5 minutes either way is rejected, exactly 5 minutes is not
#[test]
fn a_webhook_over_5_minutes_from_the_clock_is_rejected() {
    assert_eq!(check_webhook_sent(300), Ok(()));
    assert_eq!(check_webhook_sent(-300), Ok(()));
    assert_eq!(
        check_webhook_sent(301),
        Err(telnyx::SignatureError::StaleTimestamp { age_seconds: 301 })
    );
    // the server's clock is behind Telnyx's
    assert_eq!(
        check_webhook_sent(-301),
        Err(telnyx::SignatureError::StaleTimestamp { age_seconds: -301 })
    );
}

// a captured webhook's body can't be swapped for another
#[test]
fn a_changed_body_is_rejected() {
    let timestamp = webhook_now().timestamp().to_string();
    let signature = sign(&signing_key(), &timestamp, BODY);
    let error = telnyx::verify_signature(
        &signing_key().verifying_key(),
        Some(&timestamp),
        Some(&signature),
        br#"{"data":{"event_type":"message.received","forged":true}}"#,
        webhook_now(),
    );
    assert_eq!(error, Err(telnyx::SignatureError::BadSignature));
}

// a captured webhook can't be given a new timestamp to replay it
#[test]
fn a_changed_timestamp_is_rejected() {
    let old = (webhook_now().timestamp() - 3600).to_string();
    let signature = sign(&signing_key(), &old, BODY);
    let now = webhook_now().timestamp().to_string();
    let error = telnyx::verify_signature(
        &signing_key().verifying_key(),
        Some(&now),
        Some(&signature),
        BODY,
        webhook_now(),
    );
    assert_eq!(error, Err(telnyx::SignatureError::BadSignature));
}

#[test]
fn a_webhook_signed_with_another_key_is_rejected() {
    let other = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
    let timestamp = webhook_now().timestamp().to_string();
    let signature = sign(&other, &timestamp, BODY);
    let error = telnyx::verify_signature(
        &signing_key().verifying_key(),
        Some(&timestamp),
        Some(&signature),
        BODY,
        webhook_now(),
    );
    assert_eq!(error, Err(telnyx::SignatureError::BadSignature));
}

#[test]
fn a_missing_header_is_rejected() {
    let timestamp = webhook_now().timestamp().to_string();
    let signature = sign(&signing_key(), &timestamp, BODY);
    let key = signing_key().verifying_key();
    for (timestamp, signature) in [
        (None, Some(signature.as_str())),
        (Some(timestamp.as_str()), None),
        (None, None),
    ] {
        assert_eq!(
            telnyx::verify_signature(&key, timestamp, signature, BODY, webhook_now()),
            Err(telnyx::SignatureError::MissingHeader)
        );
    }
}

// not base64, or not the 64 bytes of an Ed25519 signature
#[test]
fn a_malformed_signature_is_rejected() {
    let timestamp = webhook_now().timestamp().to_string();
    let key = signing_key().verifying_key();
    for signature in [
        "not base64!".to_string(),
        base64(&[0; 63]),
        base64(&[0; 65]),
    ] {
        assert_eq!(
            telnyx::verify_signature(
                &key,
                Some(&timestamp),
                Some(&signature),
                BODY,
                webhook_now()
            ),
            Err(telnyx::SignatureError::MalformedSignature)
        );
    }
}

// a timestamp Telnyx signed but that isn't a number is rejected, not read as 0
#[test]
fn a_signed_timestamp_that_isnt_a_number_is_rejected() {
    let signature = sign(&signing_key(), "yesterday", BODY);
    let error = telnyx::verify_signature(
        &signing_key().verifying_key(),
        Some("yesterday"),
        Some(&signature),
        BODY,
        webhook_now(),
    );
    assert_eq!(error, Err(telnyx::SignatureError::InvalidTimestamp));
}

// a timestamp at the far end of the range is rejected rather than overflowing
#[test]
fn an_extreme_timestamp_is_rejected_without_overflow() {
    for timestamp in [i64::MIN, i64::MAX] {
        let timestamp = timestamp.to_string();
        let signature = sign(&signing_key(), &timestamp, BODY);
        let error = telnyx::verify_signature(
            &signing_key().verifying_key(),
            Some(&timestamp),
            Some(&signature),
            BODY,
            webhook_now(),
        );
        assert!(
            matches!(error, Err(telnyx::SignatureError::StaleTimestamp { .. })),
            "{error:?}"
        );
    }
}

// ---- the webhook ----

// webhooks shaped like the examples in Telnyx's docs, with placeholder numbers
// and ids: an inbound text, and a delivery report for a message sent
const MESSAGE_RECEIVED: &str = include_str!("test_data/telnyx_message_received.json");
const MESSAGE_FINALIZED: &str = include_str!("test_data/telnyx_message_finalized.json");

// the fixtures use the sender tests' numbers: a text comes from the user (TO)
// to the server's number (FROM)

// the inbound text's message id, not the event id beside it
const MESSAGE_ID: &str = "4f1b2c3d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

// the headers Telnyx would send with this body, signed at this time
fn signed_headers(body: &[u8], at: chrono::DateTime<chrono::Utc>) -> axum::http::HeaderMap {
    let timestamp = at.timestamp().to_string();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "telnyx-signature-ed25519",
        sign(&signing_key(), &timestamp, body).parse().unwrap(),
    );
    headers.insert("telnyx-timestamp", timestamp.parse().unwrap());
    headers
}

// a fixture with a change made, as the bytes Telnyx would send
fn edited(fixture: &str, edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut webhook: serde_json::Value = serde_json::from_str(fixture).unwrap();
    edit(&mut webhook);
    serde_json::to_vec(&webhook).unwrap()
}

// reads a body as the route does, signed 1 second before webhook_now
fn read(body: &[u8]) -> telnyx::Webhook {
    let sent = webhook_now() - chrono::Duration::seconds(1);
    telnyx::read_webhook(
        &signing_key().verifying_key(),
        &signed_headers(body, sent),
        body,
        webhook_now(),
    )
}

fn read_text(body: &[u8]) -> messaging::InboundMessage {
    match read(body) {
        telnyx::Webhook::Inbound(message) => message,
        other => panic!("should be an inbound text, was {other:?}"),
    }
}

fn assert_dropped(body: &[u8]) {
    let webhook = read(body);
    assert!(
        matches!(webhook, telnyx::Webhook::Dropped(_)),
        "should be dropped, was {webhook:?}"
    );
}

fn payload(webhook: &mut serde_json::Value) -> &mut serde_json::Value {
    &mut webhook["data"]["payload"]
}

#[test]
fn an_inbound_text_is_read() {
    let message = read_text(MESSAGE_RECEIVED.as_bytes());
    assert_eq!(message.from, messaging::PhoneNumber::new(TO).unwrap());
    assert_eq!(message.to, messaging::PhoneNumber::new(FROM).unwrap());
    assert_eq!(message.body, "RC1 45.3735 -121.6959 0000 0 2");
    assert_eq!(
        message.received_at,
        webhook_now() - chrono::Duration::seconds(1)
    );
    // the same in every delivery of the webhook, so duplicates can be dropped
    assert_eq!(message.provider_message_id, MESSAGE_ID);
}

// nothing is parsed before the signature is checked: a body that can't be
// parsed is rejected, not dropped as unreadable
#[test]
fn a_webhook_with_a_bad_signature_is_rejected() {
    let body = b"not json";
    let mut headers = signed_headers(body, webhook_now());
    headers.insert(
        "telnyx-signature-ed25519",
        sign(&signing_key(), "1", body).parse().unwrap(),
    );
    let webhook = telnyx::read_webhook(
        &signing_key().verifying_key(),
        &headers,
        body,
        webhook_now(),
    );
    assert!(matches!(
        webhook,
        telnyx::Webhook::Rejected(telnyx::SignatureError::BadSignature)
    ));
}

#[test]
fn a_webhook_without_headers_is_rejected() {
    let webhook = telnyx::read_webhook(
        &signing_key().verifying_key(),
        &axum::http::HeaderMap::new(),
        MESSAGE_RECEIVED.as_bytes(),
        webhook_now(),
    );
    assert!(matches!(
        webhook,
        telnyx::Webhook::Rejected(telnyx::SignatureError::MissingHeader)
    ));
}

#[test]
fn a_delivery_report_is_ignored_by_its_event_type() {
    let webhook = read(MESSAGE_FINALIZED.as_bytes());
    assert!(
        matches!(&webhook, telnyx::Webhook::Ignored(event_type) if event_type == "message.finalized"),
        "should be ignored, was {webhook:?}"
    );
}

// signed, so it came from Telnyx, but it can never be read
#[test]
fn a_signed_body_that_isnt_a_webhook_is_dropped() {
    assert_dropped(b"not json");
    assert_dropped(br#"{"data": {}}"#);
    assert_dropped(&edited(MESSAGE_RECEIVED, |webhook| {
        payload(webhook)["from"] = serde_json::json!(TO);
    }));
}

// a short code or an alphanumeric sender can't be replied to
#[test]
fn a_sender_that_isnt_a_phone_number_is_dropped() {
    for sender in ["12345", "RidgeCast"] {
        assert_dropped(&edited(MESSAGE_RECEIVED, |webhook| {
            payload(webhook)["from"]["phone_number"] = serde_json::json!(sender);
        }));
    }
}

// a text is answered only when it was sent to exactly one number, the one the
// reply comes from. a text sent to no number, or a group text, which lists
// every recipient, is dropped and nobody is replied to
#[test]
fn a_text_not_sent_to_exactly_one_number_is_dropped() {
    for to in [
        serde_json::json!([]),
        serde_json::json!([{"phone_number": FROM}, {"phone_number": "+15415550124"}]),
    ] {
        assert_dropped(&edited(MESSAGE_RECEIVED, |webhook| {
            payload(webhook)["to"] = to;
        }));
    }
}

// messages with bodies that aren't text are rejected
#[test]
fn a_text_with_no_text_has_an_empty_body() {
    let body = edited(MESSAGE_RECEIVED, |webhook| {
        payload(webhook)["text"] = serde_json::Value::Null;
    });
    assert_eq!(read_text(&body).body, "");
}

// the core reads its own clock, so a missing or unreadable time isn't an error
#[test]
fn received_at_falls_back_to_the_event_time_then_the_clock() {
    // occurred_at differs from the clock, so the time can only have come from it
    let unreadable_received_at = edited(MESSAGE_RECEIVED, |webhook| {
        payload(webhook)["received_at"] = serde_json::json!("yesterday");
        webhook["data"]["occurred_at"] = serde_json::json!("2026-11-23T11:59:58.000+00:00");
    });
    assert_eq!(
        read_text(&unreadable_received_at).received_at,
        webhook_now() - chrono::Duration::seconds(2)
    );

    let missing_both = edited(MESSAGE_RECEIVED, |webhook| {
        payload(webhook)
            .as_object_mut()
            .unwrap()
            .remove("received_at");
        webhook["data"]["occurred_at"] = serde_json::Value::Null;
    });
    assert_eq!(read_text(&missing_both).received_at, webhook_now());
}

// serves the webhook route on a local port, with FROM as the server's number,
// and returns its URL
async fn serve_route(db: &database::DB, sender: std::sync::Arc<fake_sender::FakeSender>) -> String {
    let state = state::AppState {
        db: db.clone(),
        numbers: state::Numbers::from_values(Some(FROM), None).unwrap(),
        number_key: user_numbers::NumberKey::from_value(Some("0123456789abcdef0123456789abcdef"))
            .unwrap(),
        sender,
    };
    let app = telnyx::routes(signing_key().verifying_key()).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}{}",
        listener.local_addr().unwrap(),
        telnyx::WEBHOOK_PATH
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

// posts a body signed now, since the route reads the real clock
async fn post_signed(url: &str, body: Vec<u8>) -> reqwest::StatusCode {
    reqwest::Client::new()
        .post(url)
        .headers(signed_headers(&body, chrono::Utc::now()))
        .body(body)
        .send()
        .await
        .unwrap()
        .status()
}

// the route end to end: a signed text is answered 200 and handled in the
// background, and an unsigned one is 401 and never handled. the text isn't a
// request, so the reply is the help message and nothing calls NWS
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn the_route_answers_and_then_handles_a_text() {
    tests::set_support_email();
    let db = test_db::fresh("telnyx_route").await;
    let sender = std::sync::Arc::new(fake_sender::FakeSender::default());
    let url = serve_route(&db, sender.clone()).await;

    let body = edited(MESSAGE_RECEIVED, |webhook| {
        payload(webhook)["text"] = serde_json::json!("hello");
    });
    let unsigned = reqwest::Client::new()
        .post(&url)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(unsigned.status(), reqwest::StatusCode::UNAUTHORIZED);

    // another message id, so if the unsigned text had been handled, the repeat
    // check couldn't drop this one, and a second reply would be sent
    let signed = edited(MESSAGE_RECEIVED, |webhook| {
        payload(webhook)["text"] = serde_json::json!("hello");
        payload(webhook)["id"] = serde_json::json!("another-message-id");
    });
    assert_eq!(post_signed(&url, signed).await, reqwest::StatusCode::OK);

    // the reply is sent after the answer, so wait for it
    for _ in 0..50 {
        if !sender.attempted().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // a reply to the unsigned text would also come in the background, so give
    // one time to arrive
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(sender.attempted(), vec![reply::help_message()]);
    let attempts = sender.attempts.lock().unwrap().clone();
    assert_eq!(attempts[0].0, messaging::PhoneNumber::new(FROM).unwrap());
    assert_eq!(attempts[0].1, messaging::PhoneNumber::new(TO).unwrap());
    test_db::remove(db).await;
}

fn with_autoresponse(autoresponse_type: &str) -> Vec<u8> {
    edited(MESSAGE_RECEIVED, |webhook| {
        payload(webhook)["autoresponse_type"] = serde_json::json!(autoresponse_type);
    })
}

// a STOP is read as an opt-out by the user who sent it, not passed on as a
// text: Telnyx has answered it and blocks the user, so the route only deletes
// their trackers
#[test]
fn stop_is_an_opt_out_by_the_sender() {
    match read(&with_autoresponse("STOP")) {
        telnyx::Webhook::OptOut(user) => {
            assert_eq!(user, messaging::PhoneNumber::new(TO).unwrap())
        }
        other => panic!("should be an opt-out, was {other:?}"),
    }
}

// Telnyx's opt-outs cover every number on the messaging profile, so the
// number texted doesn't matter
#[test]
fn stop_is_an_opt_out_whatever_number_was_texted() {
    let body = edited(MESSAGE_RECEIVED, |webhook| {
        payload(webhook)["autoresponse_type"] = serde_json::json!("STOP");
        payload(webhook)["to"] = serde_json::json!([]);
    });
    assert!(matches!(read(&body), telnyx::Webhook::OptOut(_)));
}

// Telnyx has answered HELP and START itself, so the core would reply twice
#[test]
fn help_and_start_are_left_to_telnyx() {
    for (autoresponse_type, keyword) in [("HELP", "HELP"), ("START", "START"), ("help", "HELP")] {
        let webhook = read(&with_autoresponse(autoresponse_type));
        assert!(
            matches!(webhook, telnyx::Webhook::AutoAnswered(answered) if answered == keyword),
            "{autoresponse_type} should be left to Telnyx, was {webhook:?}"
        );
    }
}

// a type Telnyx's docs don't give is answered as a normal text, since a second
// reply is better than none
#[test]
fn an_unknown_autoresponse_type_is_passed_on() {
    let message = read_text(&with_autoresponse("SOMETHING_NEW"));
    assert_eq!(message.body, "RC1 45.3735 -121.6959 0000 0 2");
}

// the route end to end for a STOP: answered 200, then every tracker of the user
// who sent it is deleted in the background, another user's is kept, and
// nothing is sent, since Telnyx has answered the STOP itself
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn the_route_deletes_the_trackers_of_a_user_who_replied_stop() {
    let db = test_db::fresh("telnyx_route_stop").await;
    let sender = std::sync::Arc::new(fake_sender::FakeSender::default());
    let url = serve_route(&db, sender.clone()).await;
    let user = messaging::PhoneNumber::new(TO).unwrap();
    let other = messaging::PhoneNumber::new("+15415550199").unwrap();
    for number in [&user, &user, &other] {
        let mut tracker = models_tests::tracker();
        tracker.user = number.clone();
        database::add_tracker(&tracker, &db).await.unwrap();
    }
    let trackers_of = |number: &messaging::PhoneNumber| {
        let filter = doc! { "user": bson::to_bson(number).unwrap() };
        let collection = db.points_collection.clone();
        async move { collection.count_documents(filter).await.unwrap() }
    };

    assert_eq!(
        post_signed(&url, with_autoresponse("STOP")).await,
        reqwest::StatusCode::OK
    );

    // the trackers are deleted after the answer, so wait for it
    for _ in 0..50 {
        if trackers_of(&user).await == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(trackers_of(&user).await, 0);
    assert_eq!(trackers_of(&other).await, 1);
    // a reply would be sent in the background too, so give one time to arrive
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(sender.attempted().is_empty());
    test_db::remove(db).await;
}
