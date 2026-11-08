// tests for reading the server's phone numbers from the environment
use crate::messaging;
use crate::state;

const TOLL_FREE: &str = "+18005550100";
const LONG_CODE: &str = "+15415550100";

fn number(value: &str) -> messaging::PhoneNumber {
    messaging::PhoneNumber::new(value).unwrap()
}

#[test]
fn sms_number_alone_is_enough() {
    let numbers = state::Numbers::from_values(Some(TOLL_FREE), None).unwrap();
    assert_eq!(
        numbers.matching(&number(TOLL_FREE)),
        Some(&number(TOLL_FREE))
    );
    assert_eq!(numbers.matching(&number(LONG_CODE)), None);
}

#[test]
fn both_numbers_match_their_own_number() {
    let numbers = state::Numbers::from_values(Some(TOLL_FREE), Some(LONG_CODE)).unwrap();
    assert_eq!(
        numbers.matching(&number(TOLL_FREE)),
        Some(&number(TOLL_FREE))
    );
    assert_eq!(
        numbers.matching(&number(LONG_CODE)),
        Some(&number(LONG_CODE))
    );
}

// another number on the same provider account, for example
#[test]
fn any_other_number_does_not_match() {
    let numbers = state::Numbers::from_values(Some(TOLL_FREE), Some(LONG_CODE)).unwrap();
    assert_eq!(numbers.matching(&number("+15035550199")), None);
}

#[test]
fn sms_number_is_required() {
    for unset in [None, Some(""), Some("   ")] {
        assert_eq!(
            state::Numbers::from_values(unset, Some(LONG_CODE)),
            Err(state::ConfigError::MissingNumber),
            "SMS_NUMBER {unset:?}"
        );
    }
}

// a variable left blank in .env is the same as leaving it out
#[test]
fn an_empty_international_number_is_unset() {
    for unset in [None, Some(""), Some("   ")] {
        assert_eq!(
            state::Numbers::from_values(Some(TOLL_FREE), unset),
            state::Numbers::from_values(Some(TOLL_FREE), None),
            "SMS_INTERNATIONAL_NUMBER {unset:?}"
        );
    }
}

// the error says which variable holds the invalid number. the value itself is
// left out so a mistyped real number is never logged, so the variable's name is
// all a host has to find the line to fix. the second case sets a valid
// SMS_NUMBER, so the error can only come from SMS_INTERNATIONAL_NUMBER
#[test]
fn an_invalid_number_names_the_variable() {
    assert_eq!(
        state::Numbers::from_values(Some("18005550100"), None),
        Err(state::ConfigError::InvalidNumber("SMS_NUMBER"))
    );
    assert_eq!(
        state::Numbers::from_values(Some(TOLL_FREE), Some("555-0100")),
        Err(state::ConfigError::InvalidNumber(
            "SMS_INTERNATIONAL_NUMBER"
        ))
    );
}

#[test]
fn the_two_numbers_must_differ() {
    assert_eq!(
        state::Numbers::from_values(Some(TOLL_FREE), Some(TOLL_FREE)),
        Err(state::ConfigError::SameNumbers)
    );
}

// the error is printed when the server stops at startup, so it must not
// contain the rejected value, which may be a real number with a typo
#[test]
fn errors_leave_the_number_out() {
    let error = state::Numbers::from_values(Some(TOLL_FREE), Some("+1999888777x")).unwrap_err();
    assert_eq!(
        error,
        state::ConfigError::InvalidNumber("SMS_INTERNATIONAL_NUMBER")
    );
    assert!(!error.to_string().contains("999888777"));
}
