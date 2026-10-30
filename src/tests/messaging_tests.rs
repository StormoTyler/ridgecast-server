// tests for messaging::PhoneNumber validation, masking, and serde
use crate::messaging;

const NUMBER: &str = "+15415550123";

#[test]
fn accepts_a_us_number() {
    let number = messaging::PhoneNumber::new(NUMBER).unwrap();
    assert_eq!(number.expose(), NUMBER);
}

#[test]
fn accepts_the_shortest_and_longest_numbers() {
    assert!(messaging::PhoneNumber::new("+1").is_ok());
    assert!(messaging::PhoneNumber::new("+123456789012345").is_ok());
}

#[test]
fn rejects_a_number_without_a_plus() {
    assert_eq!(
        messaging::PhoneNumber::new("15415550123"),
        Err(messaging::InvalidPhoneNumber)
    );
}

#[test]
fn rejects_a_leading_zero() {
    assert!(messaging::PhoneNumber::new("+05415550123").is_err());
}

#[test]
fn rejects_a_number_over_15_digits() {
    assert!(messaging::PhoneNumber::new("+1234567890123456").is_err());
}

#[test]
fn rejects_a_plus_alone() {
    assert!(messaging::PhoneNumber::new("+").is_err());
}

#[test]
fn rejects_non_digits() {
    for number in ["+1541555O123", "+1 541 555 0123", "+1-541-555-0123", ""] {
        assert!(
            messaging::PhoneNumber::new(number).is_err(),
            "accepted {number:?}"
        );
    }
}

#[test]
fn display_masks_all_but_the_first_and_last_four_digits() {
    let number = messaging::PhoneNumber::new(NUMBER).unwrap();
    assert_eq!(number.to_string(), "+1******0123");
}

#[test]
fn debug_masks_the_number() {
    let number = messaging::PhoneNumber::new(NUMBER).unwrap();
    let debug = format!("{number:?}");
    assert_eq!(debug, "PhoneNumber(+1******0123)");
    assert!(!debug.contains("5415550123"));
}

#[test]
fn short_numbers_are_masked_completely() {
    let number = messaging::PhoneNumber::new("+123456789").unwrap();
    assert_eq!(number.to_string(), "+*********");
}

// a struct holding a number, such as a message, mustn't print it either
#[test]
fn derived_debug_of_a_message_masks_the_number() {
    let message = messaging::InboundMessage {
        from: messaging::PhoneNumber::new(NUMBER).unwrap(),
        to: messaging::PhoneNumber::new("+18005550100").unwrap(),
        body: "RC1".to_string(),
        received_at: chrono::Utc::now(),
        provider_message_id: "id".to_string(),
    };
    let debug = format!("{message:?}");
    assert!(!debug.contains("5415550123"));
    assert!(!debug.contains("8005550100"));
}

#[test]
fn serializes_as_a_plain_string() {
    let number = messaging::PhoneNumber::new(NUMBER).unwrap();
    assert_eq!(
        serde_json::to_string(&number).unwrap(),
        format!("\"{NUMBER}\"")
    );
}

#[test]
fn deserializing_validates_the_number() {
    let number: messaging::PhoneNumber = serde_json::from_str(&format!("\"{NUMBER}\"")).unwrap();
    assert_eq!(number.expose(), NUMBER);
    assert!(serde_json::from_str::<messaging::PhoneNumber>("\"5415550123\"").is_err());
}
