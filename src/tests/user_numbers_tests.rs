// tests for sms::user_numbers' key and record ids. reading and writing records
// needs MongoDB, so it isn't tested
use crate::messaging;
use crate::sms::user_numbers;

const KEY: &str = "0123456789abcdef0123456789abcdef";
const USER: &str = "+15415550123";

fn key(value: &str) -> user_numbers::NumberKey {
    user_numbers::NumberKey::from_value(Some(value)).unwrap()
}

fn number(value: &str) -> messaging::PhoneNumber {
    messaging::PhoneNumber::new(value).unwrap()
}

#[test]
fn a_missing_or_empty_key_is_rejected() {
    for value in [None, Some(""), Some("   ")] {
        assert_eq!(
            user_numbers::NumberKey::from_value(value).unwrap_err(),
            user_numbers::KeyError::Missing
        );
    }
}

#[test]
fn a_short_key_is_rejected_without_its_value() {
    let error = user_numbers::NumberKey::from_value(Some("short-secret")).unwrap_err();
    assert_eq!(error, user_numbers::KeyError::TooShort);
    assert!(!error.to_string().contains("short-secret"));
}

#[test]
fn the_key_is_not_shown_by_debug() {
    assert!(!format!("{:?}", key(KEY)).contains(KEY));
}

// the same number always finds its record again
#[test]
fn a_number_id_is_stable_hex_without_the_number() {
    let id = key(KEY).number_id(&number(USER));
    assert_eq!(id, key(KEY).number_id(&number(USER)));
    assert_eq!(id.len(), 64);
    assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(!id.contains("5415550123"));
}

#[test]
fn number_ids_differ_by_number_and_key() {
    let id = key(KEY).number_id(&number(USER));
    assert_ne!(id, key(KEY).number_id(&number("+15415550124")));
    assert_ne!(
        id,
        key("fedcba9876543210fedcba9876543210").number_id(&number(USER))
    );
}
