// the Telnyx provider. so far only its configuration: the API key messages are
// sent with, and the public key webhook signatures are checked against. it is
// added to sms_providers once it can send and receive, so SMS_PROVIDER can never
// pick a Telnyx provider that can't answer texts

use base64::Engine;
use thiserror::Error;

// the DER prefix of an Ed25519 public key in X.509 SubjectPublicKeyInfo form,
// before the 32 key bytes. Telnyx's SDKs disagree on whether the portal's key
// is the raw 32 bytes or this form, so both are accepted
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

// TELNYX_API_KEY and TELNYX_PUBLIC_KEY, read at startup
// unused until the provider is added to sms_providers
#[allow(dead_code)]
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

#[allow(dead_code)]
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
