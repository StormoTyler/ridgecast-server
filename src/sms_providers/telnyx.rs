// the Telnyx provider. so far its configuration (the API key messages are sent
// with, and the public key webhook signatures are checked against), its
// sender, its webhook signature check, and its webhook route, which leaves
// HELP and START to Telnyx and passes STOP on as an opt-out. it is added to
// sms_providers in its own step, so SMS_PROVIDER can never pick a Telnyx
// provider that answers a STOP with the help message or replies twice.
// nothing here is used until then, so dead code is allowed for the whole file;
// remove this when the provider is added
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// local modules
use crate::messaging;
use crate::sms::inbound;
use crate::state;

// the Telnyx API. TelnyxSender takes its base URL, so tests can point it at a mock
pub const BASE_URL: &str = "https://api.telnyx.com";

// the Telnyx error code for a recipient who replied STOP
const OPTED_OUT_CODE: &str = "40300";

// the DER prefix of an Ed25519 public key in X.509 SubjectPublicKeyInfo form,
// before the 32 key bytes. Telnyx's SDKs disagree on whether the portal's key
// is the raw 32 bytes or this form, so both are accepted
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

// TELNYX_API_KEY and TELNYX_PUBLIC_KEY, read at startup
pub struct Config {
    api_key: String,
    public_key: ed25519_dalek::VerifyingKey,
}

// the API key is left out, so it can't end up in a log
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("telnyx::Config(..)")
    }
}

// the messages leave the values out, even the public key, so every
// configuration error reads the same way
#[derive(Debug, Error, PartialEq)]
pub enum ConfigError {
    // the variable's name
    #[error("{0} must be set, to the key from the Telnyx portal")]
    Missing(&'static str),
    #[error("TELNYX_PUBLIC_KEY must be the base64 Ed25519 public key from the Telnyx portal")]
    InvalidPublicKey,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_values(
            std::env::var("TELNYX_API_KEY").ok().as_deref(),
            std::env::var("TELNYX_PUBLIC_KEY").ok().as_deref(),
        )
    }

    // the values of TELNYX_API_KEY and TELNYX_PUBLIC_KEY, None when unset.
    // an empty value counts as unset. the API key can only be checked by
    // sending, so only the public key is checked here
    pub fn from_values(
        api_key: Option<&str>,
        public_key: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let api_key = set_value(api_key).ok_or(ConfigError::Missing("TELNYX_API_KEY"))?;
        let public_key = set_value(public_key).ok_or(ConfigError::Missing("TELNYX_PUBLIC_KEY"))?;
        Ok(Config {
            api_key: api_key.to_string(),
            public_key: decode_public_key(public_key).ok_or(ConfigError::InvalidPublicKey)?,
        })
    }
}

// a base64 Ed25519 public key, either the raw 32 bytes or in X.509 SPKI form.
// None when it isn't base64, is another length, or isn't a valid key
fn decode_public_key(value: &str) -> Option<ed25519_dalek::VerifyingKey> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    let key = match bytes.strip_prefix(&SPKI_PREFIX) {
        Some(key) => key,
        None => &bytes,
    };
    ed25519_dalek::VerifyingKey::from_bytes(key.try_into().ok()?).ok()
}

// the trimmed value, None for an unset or empty variable
fn set_value(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

// ---- webhook signatures ----

// how far a webhook's timestamp may be from the server's clock, either way, so a
// captured webhook can't be replayed later
pub const MAX_TIMESTAMP_SKEW_SECONDS: i64 = 5 * 60;

// why a webhook was rejected. none of them holds the body, which can name a number
#[derive(Debug, Error, PartialEq)]
pub enum SignatureError {
    #[error("missing the telnyx-signature-ed25519 or telnyx-timestamp header")]
    MissingHeader,
    #[error("the signature is not a base64 Ed25519 signature")]
    MalformedSignature,
    #[error("the signature does not match")]
    BadSignature,
    // signed by Telnyx, so the timestamp is the one Telnyx sent
    #[error("telnyx-timestamp is not a Unix timestamp")]
    InvalidTimestamp,
    // signed by Telnyx, but sent too long ago: seconds before the server's
    // clock, negative for a timestamp ahead of it
    #[error(
        "the webhook is {age_seconds} s from the server's clock, over the {MAX_TIMESTAMP_SKEW_SECONDS} s limit"
    )]
    StaleTimestamp { age_seconds: i64 },
}

// checks a webhook's telnyx-signature-ed25519 header (base64) against the public
// key: Telnyx signs "{telnyx-timestamp}|{raw body}". the signature is checked
// before the timestamp's age, so a StaleTimestamp is always a real Telnyx webhook
// that arrived late, and its age is worth logging. body must be the bytes as
// received, before any parsing
pub fn verify_signature(
    public_key: &ed25519_dalek::VerifyingKey,
    timestamp: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
    now: DateTime<Utc>,
) -> Result<(), SignatureError> {
    let (Some(timestamp), Some(signature)) = (timestamp, signature) else {
        return Err(SignatureError::MissingHeader);
    };
    let signature = base64::engine::general_purpose::STANDARD
        .decode(signature.trim())
        .ok()
        .and_then(|bytes| ed25519_dalek::Signature::from_slice(&bytes).ok())
        .ok_or(SignatureError::MalformedSignature)?;

    // the header exactly as received, since those are the bytes Telnyx signed
    let mut signed = Vec::with_capacity(timestamp.len() + 1 + body.len());
    signed.extend_from_slice(timestamp.as_bytes());
    signed.push(b'|');
    signed.extend_from_slice(body);
    // strict rejects weak keys and malleable signatures, which Telnyx never sends
    public_key
        .verify_strict(&signed, &signature)
        .map_err(|_| SignatureError::BadSignature)?;

    let sent: i64 = timestamp
        .trim()
        .parse()
        .map_err(|_| SignatureError::InvalidTimestamp)?;
    // saturating, so a timestamp far from now can't overflow
    let age_seconds = now.timestamp().saturating_sub(sent);
    if age_seconds.unsigned_abs() > MAX_TIMESTAMP_SKEW_SECONDS.unsigned_abs() {
        return Err(SignatureError::StaleTimestamp { age_seconds });
    }
    Ok(())
}

// sends messages with the Telnyx API, one attempt per message: the core
// retries a SendError::Transient itself (sms::outbound)
pub struct TelnyxSender {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl TelnyxSender {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            // fail fast if Telnyx is unreachable
            .connect_timeout(Duration::from_secs(10))
            // cap the whole request. a timeout is retried by the core, up to
            // 3 attempts, so a message waits at most about a minute
            .timeout(Duration::from_secs(15))
            .build()
            .expect("Failed to build client");
        TelnyxSender {
            client,
            base_url: base_url.into(),
            api_key: api_key.into(),
        }
    }
}

// the body of POST /v2/messages. from is a specific number, so no messaging
// profile id is needed
#[derive(Serialize)]
struct SendRequest<'a> {
    from: &'a str,
    to: &'a str,
    text: &'a str,
}

// a sent message, of which only the id is read
#[derive(Deserialize)]
struct SendResponse {
    data: SentMessage,
}

#[derive(Deserialize)]
struct SentMessage {
    id: String,
}

// an error response: {"errors": [{"code": "40300", "title": ..., "detail": ...}]}
#[derive(Deserialize)]
struct ErrorResponse {
    #[serde(default)]
    errors: Vec<ApiError>,
}

// detail is left out on purpose: it can name the phone number, which must
// never reach a log
#[derive(Deserialize)]
struct ApiError {
    code: Option<String>,
    title: Option<String>,
}

#[async_trait]
impl messaging::MessageSender for TelnyxSender {
    async fn send(
        &self,
        from: &messaging::PhoneNumber,
        to: &messaging::PhoneNumber,
        body: &str,
    ) -> Result<messaging::SendReceipt, messaging::SendError> {
        let request = SendRequest {
            from: from.expose(),
            to: to.expose(),
            text: body,
        };
        let response = self
            .client
            .post(format!("{}/v2/messages", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            // a timeout or connection failure. a timeout may mean the message
            // was sent, but a duplicate warning is better than a missing one
            .map_err(|e| messaging::SendError::Transient(format!("telnyx request failed: {e}")))?;

        let status = response.status();
        // the body as text first, so an unreadable error body still gives the status
        let text = response.text().await.unwrap_or_default();
        if status.is_success() {
            return Ok(receipt(&text));
        }
        Err(send_error(status, &text))
    }
}

// Telnyx accepted the message, so it is sent even if the id can't be read.
// an error here would stop the rest of the reply, or resend the message
fn receipt(body: &str) -> messaging::SendReceipt {
    let provider_message_id = match serde_json::from_str::<SendResponse>(body) {
        Ok(response) => response.data.id,
        Err(e) => {
            tracing::warn!("telnyx: message accepted but its id could not be read: {e}");
            String::new()
        }
    };
    messaging::SendReceipt {
        provider_message_id,
    }
}

// a rejected send: a 429 or 5xx may succeed if retried, a STOP is OptedOut,
// and anything else is Permanent, such as an invalid number or a problem with
// the account or sending number. the message names the status and Telnyx's
// codes and titles, never the numbers
fn send_error(status: reqwest::StatusCode, body: &str) -> messaging::SendError {
    let errors = serde_json::from_str::<ErrorResponse>(body)
        .map(|response| response.errors)
        .unwrap_or_default();
    if errors
        .iter()
        .any(|error| error.code.as_deref() == Some(OPTED_OUT_CODE))
    {
        return messaging::SendError::OptedOut;
    }

    let described: Vec<String> = errors
        .iter()
        .map(|error| {
            format!(
                "{} {}",
                error.code.as_deref().unwrap_or("no code"),
                error.title.as_deref().unwrap_or("no title")
            )
        })
        .collect();
    let message = if described.is_empty() {
        format!("telnyx {status}")
    } else {
        format!("telnyx {status}: {}", described.join("; "))
    };

    let retryable = status.is_server_error()
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT;
    if retryable {
        messaging::SendError::Transient(message)
    } else {
        messaging::SendError::Permanent(message)
    }
}

// ---- the webhook ----

// where Telnyx posts webhooks, set as the messaging profile's webhook URL
pub const WEBHOOK_PATH: &str = "/webhooks/telnyx";

const SIGNATURE_HEADER: &str = "telnyx-signature-ed25519";
const TIMESTAMP_HEADER: &str = "telnyx-timestamp";

// the route Telnyx posts webhooks to. the public key is held by the route, so
// the server's state never holds anything of a provider's
pub fn routes(public_key: ed25519_dalek::VerifyingKey) -> Router<state::AppState> {
    let public_key = Arc::new(public_key);
    Router::new().route(
        WEBHOOK_PATH,
        post(
            move |State(state): State<state::AppState>, headers: HeaderMap, body: Bytes| {
                let public_key = Arc::clone(&public_key);
                async move { webhook(state, &public_key, &headers, &body) }
            },
        ),
    )
}

// answers Telnyx at once and handles a text in the background: Telnyx wants an
// answer within about 2 seconds, and a forecast reply can take longer. a
// rejected webhook is 401, anything signed is 200, so Telnyx doesn't retry
// what would never succeed
fn webhook(
    state: state::AppState,
    public_key: &ed25519_dalek::VerifyingKey,
    headers: &HeaderMap,
    body: &[u8],
) -> StatusCode {
    match read_webhook(public_key, headers, body, Utc::now()) {
        Webhook::Rejected(error) => {
            tracing::warn!("telnyx: webhook rejected: {error}");
            StatusCode::UNAUTHORIZED
        }
        Webhook::Ignored(event_type) => {
            tracing::debug!("telnyx: ignored a {event_type} webhook");
            StatusCode::OK
        }
        Webhook::Dropped(reason) => {
            tracing::warn!("telnyx: inbound text dropped: {reason}");
            StatusCode::OK
        }
        Webhook::AutoAnswered(keyword) => {
            tracing::debug!("telnyx: {keyword} was answered by Telnyx, not passed on");
            StatusCode::OK
        }
        Webhook::OptOut(user) => {
            tokio::spawn(async move { inbound::delete_trackers(&user, &state.db).await });
            StatusCode::OK
        }
        Webhook::Inbound(message) => {
            tokio::spawn(async move {
                // inbound::handle results in either a message being dropped
                // or message(s) being sent with TelnyxSender::send
                inbound::handle(
                    &message,
                    &state.numbers,
                    &state.number_key,
                    &state.db,
                    state.sender.as_ref(),
                )
                .await;
            });
            StatusCode::OK
        }
    }
}

// what a webhook is, once read. none of them holds the body or a number in the
// clear, so each can be logged
#[derive(Debug)]
pub enum Webhook {
    // not from Telnyx, or too late: answered 401
    Rejected(SignatureError),
    // from Telnyx but not an inbound text, such as a delivery report, by its
    // event type
    Ignored(String),
    // an inbound text, or a body, that can't be answered: a sender that isn't
    // a phone number, a group text, or a payload that can't be read
    Dropped(&'static str),
    // HELP or START, which Telnyx has already answered from the messaging
    // profile's settings, so the core isn't given it: it would reply a second
    // time. by the keyword
    AutoAnswered(&'static str),
    // STOP, which Telnyx has already answered and now blocks every message to
    // the user: the core deletes their trackers and sends nothing
    OptOut(messaging::PhoneNumber),
    Inbound(messaging::InboundMessage),
}

// a webhook: {"data": {"event_type", "occurred_at", "payload"}}. the payload's
// shape depends on the event type, so it is read only for an inbound text
#[derive(Deserialize)]
struct Envelope {
    data: Event,
}

#[derive(Deserialize)]
struct Event {
    event_type: String,
    occurred_at: Option<String>,
    #[serde(default)]
    payload: serde_json::Value,
}

// a message.received payload, of which only these fields are read. from is an
// object and to an array, since a group text has several recipients
#[derive(Deserialize)]
struct InboundPayload {
    // the message's id, the same in every delivery of its webhook
    id: String,
    from: Endpoint,
    to: Vec<Endpoint>,
    // null for a picture with no text
    text: Option<String>,
    received_at: Option<String>,
    // set when the text was one of the messaging profile's keywords, which
    // Telnyx answers itself: "STOP", "START", or "HELP". Telnyx's docs give
    // only these values, and not what a normal text carries (null or no field)
    autoresponse_type: Option<String>,
}

#[derive(Deserialize)]
struct Endpoint {
    phone_number: String,
}

// reads a webhook's headers and raw body. the signature is checked before the
// body is parsed, so nothing unsigned is read. parse errors aren't kept, since
// serde's messages can quote a value, such as a number
pub fn read_webhook(
    public_key: &ed25519_dalek::VerifyingKey,
    headers: &HeaderMap,
    body: &[u8],
    now: DateTime<Utc>,
) -> Webhook {
    let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
    if let Err(error) = verify_signature(
        public_key,
        header(TIMESTAMP_HEADER),
        header(SIGNATURE_HEADER),
        body,
        now,
    ) {
        return Webhook::Rejected(error);
    }

    let Ok(envelope) = serde_json::from_slice::<Envelope>(body) else {
        return Webhook::Dropped("the body is not a Telnyx webhook");
    };
    let event = envelope.data;
    if event.event_type != "message.received" {
        return Webhook::Ignored(event.event_type);
    }
    let Ok(payload) = serde_json::from_value::<InboundPayload>(event.payload) else {
        return Webhook::Dropped("the text's payload could not be read");
    };

    // a short code or an alphanumeric sender can't be replied to
    let Ok(from) = messaging::PhoneNumber::new(payload.from.phone_number) else {
        return Webhook::Dropped("the sender is not an E.164 number");
    };
    // before the number texted is checked: Telnyx's opt-outs cover every
    // number on the messaging profile, so a STOP to any of them counts
    match payload
        .autoresponse_type
        .as_deref()
        .map(str::to_ascii_uppercase)
        .as_deref()
    {
        Some("STOP") => return Webhook::OptOut(from),
        Some("HELP") => return Webhook::AutoAnswered("HELP"),
        Some("START") => return Webhook::AutoAnswered("START"),
        // a type Telnyx's docs don't give: answered as a normal text, since a
        // second reply is better than none
        Some(other) => tracing::warn!("telnyx: unknown autoresponse_type {other:?}, passed on"),
        None => {}
    }
    // whether it was sent to one of the server's numbers is the core's decision
    let [to] = payload.to.as_slice() else {
        return Webhook::Dropped("not sent to exactly one number, such as a group text");
    };
    let Ok(to) = messaging::PhoneNumber::new(to.phone_number.as_str()) else {
        return Webhook::Dropped("the number texted is not an E.164 number");
    };

    let received_at = parse_time(payload.received_at.as_deref())
        .or_else(|| parse_time(event.occurred_at.as_deref()))
        .unwrap_or(now);
    Webhook::Inbound(messaging::InboundMessage {
        from,
        to,
        body: payload.text.unwrap_or_default(),
        received_at,
        provider_message_id: payload.id,
    })
}

// Telnyx's times are RFC 3339, such as 2026-11-23T12:00:00.000+00:00
fn parse_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}
