// tests for snapshots::is_stale and the fetched_at default of stored snapshots,
// and for storing and loading snapshots, which needs MongoDB and is ignored by
// default (see test_db). a loader's refresh calls NWS or NAC, so only loading
// a fresh stored snapshot is tested
use chrono::{DateTime, TimeDelta, Utc};
use mongodb::bson::doc;

use crate::models::{AlertsSorted, AvalancheData, RawFeatures, SkippedFeature, SkippedFeatures};
use crate::snapshots;
use crate::tests::{nac_tests, nws_tests, test_db};
use crate::upstream::{nac, nws};

#[test]
fn test_snapshot_within_max_age_is_fresh() {
    let now = Utc::now();
    assert!(!snapshots::is_stale(
        now - TimeDelta::minutes(14),
        snapshots::ALERTS_MAX_AGE,
        now
    ));
    assert!(!snapshots::is_stale(
        now - TimeDelta::minutes(119),
        snapshots::AVY_MAX_AGE,
        now
    ));
}

#[test]
fn test_snapshot_at_max_age_is_fresh() {
    // stale only once it is older than the max age
    let now = Utc::now();
    assert!(!snapshots::is_stale(
        now - snapshots::ALERTS_MAX_AGE,
        snapshots::ALERTS_MAX_AGE,
        now
    ));
}

#[test]
fn test_snapshot_past_max_age_is_stale() {
    let now = Utc::now();
    assert!(snapshots::is_stale(
        now - TimeDelta::minutes(16),
        snapshots::ALERTS_MAX_AGE,
        now
    ));
    assert!(snapshots::is_stale(
        now - TimeDelta::minutes(121),
        snapshots::AVY_MAX_AGE,
        now
    ));
}

#[test]
fn test_snapshot_stored_without_fetched_at_is_stale() {
    // a document stored before fetched_at was added must be refreshed
    let alerts: AlertsSorted = bson::from_document(bson::doc! {
        "with_geometry": [],
        "by_same_code": {},
    })
    .expect("an alerts document without fetched_at should still deserialize");
    let avy: AvalancheData = bson::from_document(bson::doc! { "features": [] })
        .expect("an avalanche document without fetched_at should still deserialize");

    assert_eq!(alerts.fetched_at, DateTime::<Utc>::UNIX_EPOCH);
    assert!(snapshots::is_stale(
        alerts.fetched_at,
        snapshots::ALERTS_MAX_AGE,
        Utc::now()
    ));
    assert!(snapshots::is_stale(
        avy.fetched_at,
        snapshots::AVY_MAX_AGE,
        Utc::now()
    ));
}

#[test]
fn test_fetched_at_round_trips_through_bson() {
    // fetched_at is stored as a bson datetime, which keeps millisecond precision
    let fetched_at =
        DateTime::from_timestamp_millis(1_760_000_000_123).expect("timestamp should be valid");
    let avy = AvalancheData {
        features: Vec::new(),
        fetched_at,
    };

    let stored = bson::to_document(&avy).expect("avalanche data should serialize");
    let read: AvalancheData =
        bson::from_document(stored).expect("avalanche data should deserialize");
    assert_eq!(read.fetched_at, fetched_at);
}

// skipped features built from counts, for the fetch failure threshold
fn skipped_counts(skipped: usize, total: usize) -> SkippedFeatures {
    let raw: RawFeatures = serde_json::from_value(serde_json::json!({
        "features": (0..skipped).map(|i| serde_json::json!({ "id": i, "value": "not a number" }))
            .chain((skipped..total).map(|i| serde_json::json!({ "id": i, "value": 1 })))
            .collect::<Vec<_>>(),
    }))
    .expect("test features should be a feature collection");
    let (_, skipped_features): (Vec<TestFeature>, _) = raw.parse("test features");
    skipped_features
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct TestFeature {
    value: i64,
}

#[test]
fn test_fetch_fails_at_ten_percent_skipped() {
    assert!(skipped_counts(10, 100).too_many());
    assert!(skipped_counts(1, 10).too_many());
    assert!(skipped_counts(10, 10).too_many());
}

#[test]
fn test_fetch_succeeds_under_ten_percent_skipped() {
    assert!(!skipped_counts(9, 100).too_many());
    assert!(!skipped_counts(0, 10).too_many());
    // an empty collection, such as no active alerts, isn't a failure
    assert!(!skipped_counts(0, 0).too_many());
}

#[test]
fn test_parse_records_skipped_features() {
    let raw: RawFeatures = serde_json::from_value(serde_json::json!({ "features": [
        { "id": "https://api.weather.gov/alerts/bad", "value": "bad" },
        { "id": 42, "value": "bad" },
        { "value": "bad" },
        { "id": "good", "value": 1 },
    ]}))
    .expect("test features should be a feature collection");
    let (parsed, skipped): (Vec<TestFeature>, _) = raw.parse("test features");

    assert_eq!(parsed.len(), 1);
    assert_eq!(skipped.total, 4);
    let ids: Vec<Option<&str>> = skipped
        .features
        .iter()
        .map(|feature| feature.feature_id.as_deref())
        .collect();
    // string ids are stored without their json quotes
    assert_eq!(
        ids,
        vec![Some("https://api.weather.gov/alerts/bad"), Some("42"), None]
    );
    let record = &skipped.features[0];
    assert_eq!(record.source, "test features");
    assert!(!record.error.is_empty());
    // the feature is kept as received
    assert_eq!(record.feature["value"], "bad");
}

#[test]
fn test_skipped_feature_round_trips_through_bson() {
    let raw: RawFeatures = serde_json::from_value(serde_json::json!({ "features": [
        { "id": "stored", "@context": ["geojson"], "value": { "nested": [1.5, null, true] } },
    ]}))
    .expect("test features should be a feature collection");
    let (_, skipped): (Vec<TestFeature>, _) = raw.parse("test features");

    let stored = bson::to_document(&skipped.features[0]).expect("skipped feature should serialize");
    assert!(
        stored.get_datetime("last_seen").is_ok(),
        "last_seen should be a bson datetime for the TTL index"
    );
    let read: SkippedFeature =
        bson::from_document(stored).expect("skipped feature should deserialize");
    assert_eq!(read.feature, skipped.features[0].feature);
}

// a minute ago, so the snapshot is fresh, without the part of a millisecond
// MongoDB doesn't keep
fn a_minute_ago() -> DateTime<Utc> {
    let millis = (Utc::now() - TimeDelta::minutes(1)).timestamp_millis();
    DateTime::from_timestamp_millis(millis).unwrap()
}

// a refresh would set a new fetched_at, so the stored one coming back means the
// loader used the stored snapshot without calling NWS or NAC
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_fresh_stored_snapshot_is_loaded_without_a_refresh() {
    let db = test_db::fresh("fresh_snapshot").await;
    let fetched_at = a_minute_ago();
    let alerts = AlertsSorted {
        fetched_at,
        ..AlertsSorted::default()
    };
    let avy = AvalancheData {
        features: Vec::new(),
        fetched_at,
    };
    snapshots::store_alerts(&db, &alerts).await.unwrap();
    snapshots::store_avy(&db, &avy).await.unwrap();

    let alerts = snapshots::load_alerts(&db).await.unwrap();
    assert!(!alerts.stale);
    assert_eq!(alerts.data.fetched_at, fetched_at);
    let avy = snapshots::load_avy(&db).await.unwrap();
    assert!(!avy.stale);
    assert_eq!(avy.data.fetched_at, fetched_at);

    test_db::remove(db).await;
}

// each snapshot is one document, replaced by the next
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn storing_a_snapshot_replaces_the_last() {
    let db = test_db::fresh("replaced_snapshot").await;
    let first = a_minute_ago() - TimeDelta::minutes(1);
    let second = a_minute_ago();

    for fetched_at in [first, second] {
        let alerts = AlertsSorted {
            fetched_at,
            ..AlertsSorted::default()
        };
        let avy = AvalancheData {
            features: Vec::new(),
            fetched_at,
        };
        snapshots::store_alerts(&db, &alerts).await.unwrap();
        snapshots::store_avy(&db, &avy).await.unwrap();
    }

    let alerts = db.alerts_collection.count_documents(doc! {}).await;
    assert_eq!(alerts.expect("alerts should count"), 1);
    let avy = db.avy_collection.count_documents(doc! {}).await;
    assert_eq!(avy.expect("avalanche data should count"), 1);
    let loaded = snapshots::load_alerts(&db).await.unwrap();
    assert_eq!(loaded.data.fetched_at, second);
    let loaded = snapshots::load_avy(&db).await.unwrap();
    assert_eq!(loaded.data.fetched_at, second);

    test_db::remove(db).await;
}

// what /health reads, without loading the snapshot
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn fetched_at_is_read_from_a_stored_snapshot() {
    let db = test_db::fresh("read_fetched_at").await;
    assert_eq!(
        snapshots::read_fetched_at(&db.alerts_collection).await,
        None
    );
    assert_eq!(snapshots::read_fetched_at(&db.avy_collection).await, None);

    let fetched_at = a_minute_ago();
    let alerts = AlertsSorted {
        fetched_at,
        ..AlertsSorted::default()
    };
    snapshots::store_alerts(&db, &alerts).await.unwrap();

    assert_eq!(
        snapshots::read_fetched_at(&db.alerts_collection).await,
        Some(fetched_at)
    );
    test_db::remove(db).await;
}

// the saved NWS alerts match the same points after being stored and loaded, by
// SAME code and by polygon, so a geometry isn't read back as a type that never matches
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn stored_alerts_match_points_as_before() {
    let db = test_db::fresh("stored_alerts").await;
    let mut alerts = nws_tests::load_alerts();
    alerts.fetched_at = a_minute_ago();
    snapshots::store_alerts(&db, &alerts).await.unwrap();

    let loaded = snapshots::load_alerts(&db).await.unwrap().data;

    assert_eq!(loaded.fetched_at, alerts.fetched_at);
    assert_eq!(loaded.with_geometry.len(), alerts.with_geometry.len());
    assert_eq!(loaded.by_same_code.len(), alerts.by_same_code.len());
    let points = [
        // Bozeman, MT: matched by SAME code
        nws_tests::make_point(45.68, -111.04, "030031"),
        // inside the Glynn, GA flood advisory polygon
        nws_tests::make_point(31.15, -81.40, "999999"),
        // on the polygon's edge
        nws_tests::make_point(31.29, -81.37, "999999"),
        // Eugene, OR: no alerts
        nws_tests::make_point(44.05, -123.09, "041039"),
    ];
    for point in &points {
        let before = nws::check_alerts(&alerts, point);
        let after = nws::check_alerts(&loaded, point);
        assert_eq!(nws_tests::ids(&after), nws_tests::ids(&before));
    }
    // the first three points each match an alert
    assert_eq!(nws::check_alerts(&loaded, &points[2]).len(), 1);

    test_db::remove(db).await;
}

// the saved NAC zones give the same forecast after being stored and loaded:
// the geometry still matches and the dates keep their offsets
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn stored_avy_zones_match_points_as_before() {
    let db = test_db::fresh("stored_avy").await;
    let mut avy = nac_tests::load_avy();
    avy.fetched_at = a_minute_ago();
    snapshots::store_avy(&db, &avy).await.unwrap();

    let loaded = snapshots::load_avy(&db).await.unwrap().data;

    assert_eq!(loaded.features.len(), avy.features.len());
    let now = nac_tests::test_now();
    let points = [
        // inside Swan Range, a polygon
        nac_tests::make_point(48.08, -113.86),
        // inside Mount Shasta, a multipolygon
        nac_tests::make_point(41.40, -122.29),
        // a vertex of the Swan Range boundary
        nac_tests::make_point(48.310457276, -113.979234344),
    ];
    for point in &points {
        let before = nac::check_avy_geometry(point, &avy, now);
        let after = nac::check_avy_geometry(point, &loaded, now);
        assert!(matches!(before, nac::ZoneForecast::Current(_)));
        assert_eq!(after, before);
    }
    for (stored, read) in avy.features.iter().zip(&loaded.features) {
        assert_eq!(read.properties, stored.properties);
    }

    test_db::remove(db).await;
}
