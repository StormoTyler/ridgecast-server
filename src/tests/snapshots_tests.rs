// tests for snapshots::is_stale and the fetched_at default of stored snapshots
use chrono::{DateTime, TimeDelta, Utc};

use crate::models::{AlertsSorted, AvalancheData, RawFeatures, SkippedFeature, SkippedFeatures};
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

// skipped features built from counts, for the fetch failure threshold
fn skipped_counts(skipped: usize, total: usize) -> SkippedFeatures
{
    let raw: RawFeatures = serde_json::from_value(serde_json::json!({
        "features": (0..skipped).map(|i| serde_json::json!({ "id": i, "value": "not a number" }))
            .chain((skipped..total).map(|i| serde_json::json!({ "id": i, "value": 1 })))
            .collect::<Vec<_>>(),
    })).expect("test features should be a feature collection");
    let (_, skipped_features): (Vec<TestFeature>, _) = raw.parse("test features");
    skipped_features
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct TestFeature
{
    value: i64,
}

#[test]
fn test_fetch_fails_at_ten_percent_skipped()
{
    assert!(skipped_counts(10, 100).too_many());
    assert!(skipped_counts(1, 10).too_many());
    assert!(skipped_counts(10, 10).too_many());
}

#[test]
fn test_fetch_succeeds_under_ten_percent_skipped()
{
    assert!(!skipped_counts(9, 100).too_many());
    assert!(!skipped_counts(0, 10).too_many());
    // an empty collection, such as no active alerts, isn't a failure
    assert!(!skipped_counts(0, 0).too_many());
}

#[test]
fn test_parse_records_skipped_features()
{
    let raw: RawFeatures = serde_json::from_value(serde_json::json!({ "features": [
        { "id": "https://api.weather.gov/alerts/bad", "value": "bad" },
        { "id": 42, "value": "bad" },
        { "value": "bad" },
        { "id": "good", "value": 1 },
    ]})).expect("test features should be a feature collection");
    let (parsed, skipped): (Vec<TestFeature>, _) = raw.parse("test features");

    assert_eq!(parsed.len(), 1);
    assert_eq!(skipped.total, 4);
    let ids: Vec<Option<&str>> = skipped.features.iter().map(|feature| feature.feature_id.as_deref()).collect();
    // string ids are stored without their json quotes
    assert_eq!(ids, vec![Some("https://api.weather.gov/alerts/bad"), Some("42"), None]);
    let record = &skipped.features[0];
    assert_eq!(record.source, "test features");
    assert!(!record.error.is_empty());
    // the feature is kept as received
    assert_eq!(record.feature["value"], "bad");
}

#[test]
fn test_skipped_feature_round_trips_through_bson()
{
    let raw: RawFeatures = serde_json::from_value(serde_json::json!({ "features": [
        { "id": "stored", "@context": ["geojson"], "value": { "nested": [1.5, null, true] } },
    ]})).expect("test features should be a feature collection");
    let (_, skipped): (Vec<TestFeature>, _) = raw.parse("test features");

    let stored = bson::to_document(&skipped.features[0]).expect("skipped feature should serialize");
    assert!(stored.get_datetime("last_seen").is_ok(), "last_seen should be a bson datetime for the TTL index");
    let read: SkippedFeature = bson::from_document(stored).expect("skipped feature should deserialize");
    assert_eq!(read.feature, skipped.features[0].feature);
}
