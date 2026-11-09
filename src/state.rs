// the state shared with the route handlers, and the server configuration it
// holds, read from the environment at startup. provider settings (the sender,
// API keys) are not here: src/sms_providers builds those, so nothing in the
// server's state names an SMS provider

use std::sync::Arc;

use axum::extract::FromRef;
use thiserror::Error;

// local modules
use crate::database;
use crate::messaging;

#[derive(Clone)]
pub struct AppState {
    pub db: database::DB,
    pub numbers: Numbers,
    // the provider's sender, built by sms_providers. the core sees only the port
    pub sender: Arc<dyn messaging::MessageSender>,
}

// lets a handler take State<database::DB> rather than the whole AppState, so
// handlers that only need the database are unchanged
impl FromRef<AppState> for database::DB {
    fn from_ref(state: &AppState) -> Self {
        state.db.clone()
    }
}

// the phone numbers users text. a reply is always sent from the number the
// user texted, so an inbound message must be addressed to one of these
#[derive(Debug, Clone, PartialEq)]
pub struct Numbers {
    // SMS_NUMBER, toll-free or a regular number
    primary: messaging::PhoneNumber,
    // SMS_INTERNATIONAL_NUMBER, for users outside the US and Canada, who
    // can't text toll-free numbers. None when it isn't set
    international: Option<messaging::PhoneNumber>,
}

// a configuration value that stops the server at startup. the messages leave
// the values out, so a phone number is never logged
#[derive(Debug, Error, PartialEq)]
pub enum ConfigError {
    #[error("SMS_NUMBER must be set")]
    MissingNumber,
    #[error("{0} must be an E.164 phone number, such as +18005550100")]
    InvalidNumber(&'static str),
    #[error("SMS_NUMBER and SMS_INTERNATIONAL_NUMBER must be different numbers")]
    SameNumbers,
}

impl Numbers {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_values(
            std::env::var("SMS_NUMBER").ok().as_deref(),
            std::env::var("SMS_INTERNATIONAL_NUMBER").ok().as_deref(),
        )
    }

    // the values of SMS_NUMBER and SMS_INTERNATIONAL_NUMBER, None when unset.
    // an empty value counts as unset, so a variable left blank in .env is
    // the same as leaving it out
    pub fn from_values(
        primary: Option<&str>,
        international: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let parse = |name, value: &str| {
            messaging::PhoneNumber::new(value).map_err(|_| ConfigError::InvalidNumber(name))
        };

        let primary = parse(
            "SMS_NUMBER",
            set_value(primary).ok_or(ConfigError::MissingNumber)?,
        )?;
        let international = set_value(international)
            .map(|value| parse("SMS_INTERNATIONAL_NUMBER", value))
            .transpose()?;
        if international.as_ref() == Some(&primary) {
            return Err(ConfigError::SameNumbers);
        }
        Ok(Numbers {
            primary,
            international,
        })
    }

    // the server number an inbound message was sent to, which the reply is
    // sent from. None for any other number, such as another number on the
    // same provider account, so the message is dropped rather than answered
    // from a number this server doesn't use
    pub fn matching(&self, to: &messaging::PhoneNumber) -> Option<&messaging::PhoneNumber> {
        std::iter::once(&self.primary)
            .chain(&self.international)
            .find(|number| *number == to)
    }
}

impl Numbers {
    // SMS_NUMBER, the number a text is assumed to be sent to when no other is given
    pub fn primary(&self) -> &messaging::PhoneNumber {
        &self.primary
    }
}

// None for an unset or empty variable
fn set_value(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}
