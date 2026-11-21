// tests for shared::validity, shared::NoticeFlag, and DateTimeTypes::to_utc
use chrono::{DateTime, FixedOffset, NaiveDate, TimeDelta, TimeZone, Utc};

use crate::models::DateTimeTypes;
use crate::tracking::shared;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap()
}

// a period from start to end hours away from now
fn validity_at(start_hours: i64, end_hours: i64) -> shared::Validity {
    shared::validity(
        Some(now() + TimeDelta::hours(start_hours)),
        Some(now() + TimeDelta::hours(end_hours)),
        now(),
    )
}

#[test]
fn validity_is_current_between_start_and_end() {
    assert_eq!(validity_at(-1, 1), shared::Validity::Current);
}

#[test]
fn validity_is_current_at_start() {
    assert_eq!(validity_at(0, 1), shared::Validity::Current);
}

#[test]
fn validity_is_expired_at_end() {
    assert_eq!(validity_at(-2, 0), shared::Validity::Expired);
}

#[test]
fn validity_is_expired_after_end() {
    assert_eq!(validity_at(-2, -1), shared::Validity::Expired);
}

#[test]
fn validity_is_not_started_before_start() {
    assert_eq!(validity_at(1, 2), shared::Validity::NotStarted);
}

#[test]
fn validity_reports_missing_start_or_end() {
    let end = Some(now() + TimeDelta::hours(1));
    assert_eq!(
        shared::validity(None, end, now()),
        shared::Validity::NoStart
    );
    assert_eq!(
        shared::validity(Some(now()), None, now()),
        shared::Validity::NoEnd
    );
    // a missing start is reported first
    assert_eq!(
        shared::validity(None, None, now()),
        shared::Validity::NoStart
    );
}

#[test]
fn notice_flag_raises_once() {
    let mut flag = shared::NoticeFlag::default();
    assert!(!flag.is_raised());
    assert!(flag.raise(), "the first raise should send the notice");
    assert!(!flag.raise(), "later raises should not send it again");
    assert!(flag.is_raised());
}

#[test]
fn notice_flag_raises_again_after_clear() {
    let mut flag = shared::NoticeFlag::default();
    flag.raise();
    flag.clear();
    assert!(!flag.is_raised());
    assert!(
        flag.raise(),
        "a condition that returns should be notified again"
    );
}

#[test]
fn notice_flag_is_stored_as_a_bool() {
    let mut flag = shared::NoticeFlag::default();
    flag.raise();
    let stored = bson::to_bson(&flag).expect("flag should serialize");
    assert_eq!(stored, bson::Bson::Boolean(true));
    let read: shared::NoticeFlag = bson::from_bson(stored).expect("flag should deserialize");
    assert!(read.is_raised());
}

#[test]
fn to_utc_converts_aware_dates() {
    let mountain = FixedOffset::west_opt(7 * 3600).unwrap();
    let aware = mountain.with_ymd_and_hms(2026, 1, 15, 5, 0, 0).unwrap();
    assert_eq!(DateTimeTypes::Aware(aware).to_utc(), now());
}

#[test]
fn to_utc_reads_naive_dates_as_utc() {
    let naive = NaiveDate::from_ymd_opt(2026, 1, 15)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    assert_eq!(DateTimeTypes::Naive(naive).to_utc(), now());
}
