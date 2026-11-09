// the port between the core and an SMS provider. the core sends and receives
// messages only through these types, so a provider is an adapter that can be
// swapped without touching sms or the scheduler.
// nothing provider specific belongs here, this module never uses reqwest or axum

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// a phone number in E.164 format, such as +15415550123.
// Debug and Display mask it, so a number can't end up in a log by accident.
// expose() gives the full number to the adapter that sends to it
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
// stored as a plain string, and validated when read back
#[serde(try_from = "String")]
pub struct PhoneNumber(String);

// the number itself is left out of the message, since it may be a real number
#[derive(Debug, Error, PartialEq)]
#[error("not an E.164 phone number")]
pub struct InvalidPhoneNumber;

impl PhoneNumber {
    // E.164: a +, then 1 to 15 digits, the first not 0. checked by hand,
    // matching ^\+[1-9]\d{0,14}$
    pub fn new(number: impl Into<String>) -> Result<Self, InvalidPhoneNumber> {
        let number = number.into();
        let Some(digits) = number.strip_prefix('+') else {
            return Err(InvalidPhoneNumber);
        };
        let valid = (1..=15).contains(&digits.len())
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && !digits.starts_with('0');
        if valid {
            Ok(PhoneNumber(number))
        } else {
            Err(InvalidPhoneNumber)
        }
    }

    // the full number, for the adapter sending to it. never log the result.
    // unused until an adapter sends to a real number: the local provider only
    // prints the masked number
    #[allow(dead_code)]
    pub fn expose(&self) -> &str {
        &self.0
    }

    // keeps the first digit and the last 4, such as +1******0123.
    // a short number is masked completely, since its last 4 digits
    // would give away most of it
    fn masked(&self) -> String {
        let digits = &self.0[1..];
        if digits.len() < 10 {
            return format!("+{}", "*".repeat(digits.len()));
        }
        let (first, rest) = digits.split_at(1);
        let (hidden, last) = rest.split_at(rest.len() - 4);
        format!("+{first}{}{last}", "*".repeat(hidden.len()))
    }
}

impl TryFrom<String> for PhoneNumber {
    type Error = InvalidPhoneNumber;

    fn try_from(number: String) -> Result<Self, Self::Error> {
        PhoneNumber::new(number)
    }
}

impl std::fmt::Display for PhoneNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.masked())
    }
}

impl std::fmt::Debug for PhoneNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PhoneNumber({})", self.masked())
    }
}

// a message received by the server, built by an adapter from its provider's webhook
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub from: PhoneNumber,
    // the server's number the user texted, which the reply is sent from.
    // a server can have two numbers, a toll-free one and one for international users
    pub to: PhoneNumber,
    pub body: String,
    // when the provider received the text. the core reads its own clock instead,
    // since a provider's timestamp can be skewed; kept for logging and the Telnyx
    // adapter, unused until then
    #[allow(dead_code)]
    pub received_at: DateTime<Utc>,
    // opaque to the core, used only to drop duplicate webhook deliveries
    pub provider_message_id: String,
}

// a message the provider accepted for sending
#[derive(Debug, Clone)]
pub struct SendReceipt {
    // the provider's id, to match delivery status updates to the message later.
    // unused until delivery status updates are handled
    #[allow(dead_code)]
    pub provider_message_id: String,
}

// the core branches on the variant only, the strings are for logs,
// so provider error codes never decide what the core does. only the tests
// construct the variants until an adapter that can fail exists (local never fails)
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum SendError {
    // may succeed if retried: a timeout, a 5xx response, or a rate limit.
    // a timeout may mean the message was sent, but it is still retried,
    // since a duplicate warning is better than a missing one
    #[error("transient send failure: {0}")]
    Transient(String),
    // retrying won't help, such as an invalid or unreachable number
    #[error("message rejected: {0}")]
    Permanent(String),
    // the recipient opted out by replying STOP, so nothing more can be sent to them
    #[error("recipient has opted out")]
    OptedOut,
}

// sends messages through an SMS provider. used as Arc<dyn MessageSender>, since
// the provider is chosen at runtime. native async fn in traits can't be used
// as dyn, so async_trait boxes the returned future
#[async_trait]
pub trait MessageSender: Send + Sync {
    // from is one of the server's numbers: the one the user texted, so the reply
    // stays in the same conversation. a satellite messenger's gateway may not
    // deliver a reply from a different number
    // an adapter makes one attempt per call and doesn't retry: the core retries
    // a SendError::Transient itself (sms::outbound), so retries
    // in an adapter would multiply them
    async fn send(
        &self,
        from: &PhoneNumber,
        to: &PhoneNumber,
        body: &str,
    ) -> Result<SendReceipt, SendError>;
}
