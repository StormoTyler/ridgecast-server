// tests for sms::user_numbers: its key and record ids, and reading and writing
// records, which needs MongoDB and is ignored by default (see test_db)
use std::sync::LazyLock;

use chrono::{DateTime, TimeDelta, Utc};
use mongodb::bson::{Document, doc};

use crate::database;
use crate::messaging;
use crate::models::UserNumber;
use crate::sms::user_numbers;
use crate::tests::test_db;

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

// when the tests started, read once so every call to at() counts from the same
// time. it's recent so the records aren't already past the TTL index's
// USER_NUMBER_TTL, which MongoDB could delete mid-test, and MongoDB keeps
// milliseconds, so it has nothing smaller
static START: LazyLock<DateTime<Utc>> =
    LazyLock::new(|| DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).unwrap());

fn at(minutes: i64) -> DateTime<Utc> {
    *START + TimeDelta::minutes(minutes)
}

async fn record(db: &database::DB) -> Option<UserNumber> {
    db.user_numbers_collection
        .find_one(doc! {})
        .await
        .expect("the record should be read")
}

#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_number_without_a_record_needs_the_opt_in_message() {
    let db = test_db::fresh("number_without_record").await;
    assert!(user_numbers::needs_opt_in_message(&db, &key(KEY), &number(USER)).await);
    test_db::remove(db).await;
}

// a text whose opt-in message wasn't delivered leaves no record, so the next
// text is sent the message again
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_use_without_the_opt_in_message_adds_no_record() {
    let db = test_db::fresh("use_without_opt_in").await;

    user_numbers::record_use(&db, &key(KEY), &number(USER), false, at(0)).await;

    assert!(record(&db).await.is_none());
    assert!(user_numbers::needs_opt_in_message(&db, &key(KEY), &number(USER)).await);
    test_db::remove(db).await;
}

#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_delivered_opt_in_message_is_recorded_without_the_number() {
    let db = test_db::fresh("delivered_opt_in").await;

    user_numbers::record_use(&db, &key(KEY), &number(USER), true, at(0)).await;

    assert!(!user_numbers::needs_opt_in_message(&db, &key(KEY), &number(USER)).await);
    // another number still needs it
    assert!(user_numbers::needs_opt_in_message(&db, &key(KEY), &number("+15415550124")).await);

    let record = record(&db).await.expect("the number should have a record");
    assert_eq!(record.id, key(KEY).number_id(&number(USER)));
    assert_eq!(record.opt_in_sent, at(0));
    assert_eq!(record.last_seen, at(0));

    // nothing stored holds the number, and last_seen is a date for the TTL index
    let stored = db
        .user_numbers_collection
        .clone_with_type::<Document>()
        .find_one(doc! {})
        .await
        .expect("the record should be read")
        .expect("the number should have a record");
    assert!(!stored.to_string().contains("5415550123"));
    assert!(stored.get_datetime("last_seen").is_ok());
    test_db::remove(db).await;
}

// each text keeps the record another USER_NUMBER_TTL, and the opt-in time is
// only ever the first
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_later_use_updates_last_seen_only() {
    let db = test_db::fresh("later_use").await;
    user_numbers::record_use(&db, &key(KEY), &number(USER), true, at(0)).await;

    user_numbers::record_use(&db, &key(KEY), &number(USER), false, at(10)).await;
    let later = record(&db).await.expect("the number should have a record");
    assert_eq!(later.opt_in_sent, at(0));
    assert_eq!(later.last_seen, at(10));

    // the help message delivered again, such as a reply to HELP
    user_numbers::record_use(&db, &key(KEY), &number(USER), true, at(20)).await;
    let again = record(&db).await.expect("the number should have a record");
    assert_eq!(again.opt_in_sent, at(0));
    assert_eq!(again.last_seen, at(20));

    let count = db.user_numbers_collection.count_documents(doc! {}).await;
    assert_eq!(count.expect("records should count"), 1);
    test_db::remove(db).await;
}

// changing USER_NUMBERS_KEY means every number gets the opt-in message once more
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_changed_key_needs_the_opt_in_message_again() {
    let db = test_db::fresh("changed_key").await;
    user_numbers::record_use(&db, &key(KEY), &number(USER), true, at(0)).await;

    let new_key = key("fedcba9876543210fedcba9876543210");
    assert!(user_numbers::needs_opt_in_message(&db, &new_key, &number(USER)).await);
    test_db::remove(db).await;
}
