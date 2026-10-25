// tests for snapshots::is_stale and the fetched_at default of stored snapshots
use chrono::{DateTime, TimeDelta, Utc};

use crate::models::{AlertsSorted, AvalancheData};
use crate::snapshots::{self, ALERTS_MAX_AGE, AVY_MAX_AGE};

#[test]
fn test_snapshot_within_max_age_is_fresh()
{
    let now = Utc::now();
    assert!(!snapshots::is_stale(now - TimeDelta::minutes(14), ALERTS_MAX_AGE, now));
    assert!(!snapshots::is_stale(now - TimeDelta::minutes(119), AVY_MAX_AGE, now));
}

#[test]
fn test_snapshot_at_max_age_is_fresh()
{
    // stale only once it is older than the max age
    let now = Utc::now();
    assert!(!snapshots::is_stale(now - ALERTS_MAX_AGE, ALERTS_MAX_AGE, now));
}

#[test]
fn test_snapshot_past_max_age_is_stale()
{
    let now = Utc::now();
    assert!(snapshots::is_stale(now - TimeDelta::minutes(16), ALERTS_MAX_AGE, now));
    assert!(snapshots::is_stale(now - TimeDelta::minutes(121), AVY_MAX_AGE, now));
}

#[test]
fn test_snapshot_stored_without_fetched_at_is_stale()
{
    // a document stored before fetched_at was added must be refreshed
    let alerts: AlertsSorted = bson::from_document(bson::doc! {
        "with_geometry": [],
        "by_same_code": {},
    }).expect("an alerts document without fetched_at should still deserialize");
    let avy: AvalancheData = bson::from_document(bson::doc! { "features": [] })
        .expect("an avalanche document without fetched_at should still deserialize");

    assert_eq!(alerts.fetched_at, DateTime::<Utc>::UNIX_EPOCH);
    assert!(snapshots::is_stale(alerts.fetched_at, ALERTS_MAX_AGE, Utc::now()));
    assert!(snapshots::is_stale(avy.fetched_at, AVY_MAX_AGE, Utc::now()));
}

#[test]
fn test_fetched_at_round_trips_through_bson()
{
    // fetched_at is stored as a bson datetime, which keeps millisecond precision
    let fetched_at = DateTime::from_timestamp_millis(1_760_000_000_123)
        .expect("timestamp should be valid");
    let avy = AvalancheData { features: Vec::new(), fetched_at };

    let stored = bson::to_document(&avy).expect("avalanche data should serialize");
    let read: AvalancheData = bson::from_document(stored).expect("avalanche data should deserialize");
    assert_eq!(read.fetched_at, fetched_at);
}
