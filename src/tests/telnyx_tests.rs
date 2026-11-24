// tests for the Telnyx provider's configuration
use base64::Engine;

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

// 44 bytes that aren't the SPKI form are not read as a key with a prefix
#[test]
fn a_44_byte_value_without_the_spki_prefix_is_rejected() {
    let mut bytes = spki(&public_key_bytes());
    bytes[0] = 0x31;
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
