// the users' phone numbers: the server's only record of which numbers have used
// it, and whether each has been sent the opt-in message (the help message, with
// the contact address and how to opt out), so that is sent once per number
// rather than with every reply. a number is stored only as an HMAC under
// USER_NUMBERS_KEY, so a leaked database doesn't reveal which numbers used the
// service. each text from a number refreshes its record, which MongoDB deletes
// database::USER_NUMBER_TTL after the number last texted

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use mongodb::bson::doc;
use sha2::Sha256;
use thiserror::Error;

// local modules
use crate::database;
use crate::messaging;

// a 32 byte key, such as 64 hex digits from `openssl rand -hex 32`, is the
// HMAC-SHA256 block size's recommended minimum
const MIN_KEY_LENGTH: usize = 32;

// the secret the records' ids are made with. changing it only means every number
// is sent the opt-in message once more
#[derive(Clone)]
pub struct NumberKey(Vec<u8>);

// the key is left out, so it can't end up in a log
impl std::fmt::Debug for NumberKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NumberKey(..)")
    }
}

// the messages leave the value out
#[derive(Debug, Error, PartialEq)]
pub enum KeyError {
    #[error("USER_NUMBERS_KEY must be set, generate one with: openssl rand -hex 32")]
    Missing,
    #[error(
        "USER_NUMBERS_KEY must be at least {MIN_KEY_LENGTH} characters, \
         generate one with: openssl rand -hex 32"
    )]
    TooShort,
}

impl NumberKey {
    pub fn from_env() -> Result<Self, KeyError> {
        Self::from_value(std::env::var("USER_NUMBERS_KEY").ok().as_deref())
    }

    // the value of USER_NUMBERS_KEY, None when unset. an empty value counts as unset
    pub fn from_value(value: Option<&str>) -> Result<Self, KeyError> {
        let value = value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(KeyError::Missing)?;
        if value.len() < MIN_KEY_LENGTH {
            return Err(KeyError::TooShort);
        }
        Ok(NumberKey(value.as_bytes().to_vec()))
    }

    // the id of a number's record: its HMAC-SHA256 in hex. a plain hash would not do,
    // since every phone number can be hashed and looked up in minutes
    pub fn number_id(&self, number: &messaging::PhoneNumber) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC takes a key of any length");
        mac.update(number.expose().as_bytes());
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

// whether a number still needs the opt-in message. a number whose record can't
// be read is sent it, since a repeat costs one message and a missing one breaks
// the opt-in the verification relies on
pub async fn needs_opt_in_message(
    db: &database::DB,
    key: &NumberKey,
    number: &messaging::PhoneNumber,
) -> bool {
    match db
        .user_numbers_collection
        .find_one(doc! { "_id": key.number_id(number) })
        .await
    {
        Ok(record) => record.is_none(),
        Err(e) => {
            tracing::warn!(
                "user_numbers: failed to read a record, sending the opt-in message: {e}"
            );
            true
        }
    }
}

// records that a number texted the server at now, so its record is kept
// another database::USER_NUMBER_TTL. sent_opt_in says the opt-in message
// was delivered in this reply, which adds a record for the number if it has
// none. failures are logged, not returned: the reply has already been sent, and
// the cost is the opt-in message being sent again or the record deleted early
pub async fn record_use(
    db: &database::DB,
    key: &NumberKey,
    number: &messaging::PhoneNumber,
    sent_opt_in: bool,
    now: DateTime<Utc>,
) {
    let filter = doc! { "_id": key.number_id(number) };
    let last_seen = bson::DateTime::from_chrono(now);
    let record = if sent_opt_in {
        db.user_numbers_collection
            .update_one(
                filter,
                doc! {
                    "$set": { "last_seen": last_seen },
                    "$setOnInsert": { "opt_in_sent": last_seen },
                },
            )
            .upsert(true)
            .await
    } else {
        // a number that wasn't sent the message gets no record
        db.user_numbers_collection
            .update_one(filter, doc! { "$set": { "last_seen": last_seen } })
            .await
    };
    if let Err(e) = record {
        tracing::warn!("user_numbers: failed to record a number's use: {e}");
    }
}
