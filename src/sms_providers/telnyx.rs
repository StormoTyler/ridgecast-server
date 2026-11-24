// the Telnyx provider. so far its configuration (the API key messages are sent
// with, and the public key webhook signatures are checked against) and its
// sender. it is added to sms_providers once it can also receive, so
// SMS_PROVIDER can never pick a Telnyx provider that can't answer texts.
// nothing here is used until then, so dead code is allowed for the whole file;
// remove this when the provider is added
#![allow(dead_code)]

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// local modules
use crate::messaging;

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
