// tests for nws::sort_alerts and nws::check_alerts against a saved /alerts/active response
use std::collections::HashSet;

use crate::handlers::nws;
use crate::models::{
    Alert, Alerts, AlertsSorted, Geocode, GeometryErrorReason, GeometryTypes, InvalidAlert,
    InvalidAlertReason, Nwr, Point, PointGeometry, PointProperties, RawFeatures,
};

const TEST_DATA: &str = include_str!("test_data/nws_alerts_test_data");

// Red Flag Warning for Gallatin National Forest, has no geometry
const RED_FLAG_WARNING_ID: &str = "https://api.weather.gov/alerts/urn:oid:2.49.0.1.840.0.98ed8e221a941affa647df5a3746567e0c58d8bc.001.1";
// Flood Advisory for Glynn, GA, has a polygon
const FLOOD_ADVISORY_ID: &str = "https://api.weather.gov/alerts/urn:oid:2.49.0.1.840.0.2ec9803dbea4b774a0cc081c7640d75ab17c11d5.001.1";

// parses the test data the same way nws::get_alerts parses a live response
fn load_alerts() -> AlertsSorted {
    let raw: RawFeatures =
        serde_json::from_str(TEST_DATA).expect("test data should be a geojson feature collection");
    nws::sort_alerts(nws::parse_alerts(raw).0)
}

// ids of the features in the raw test data, read straight from the json
// so features the parser would skip are still counted
fn raw_feature_ids(with_geometry: bool) -> HashSet<String> {
    let raw: RawFeatures =
        serde_json::from_str(TEST_DATA).expect("test data should be a geojson feature collection");
    raw.features
        .iter()
        .filter(|feature| feature["geometry"].is_null() != with_geometry)
        .map(|feature| {
            feature["id"]
                .as_str()
                .expect("every test feature should have an id")
                .to_string()
        })
        .collect()
}

fn make_point(lat: f64, lon: f64, same_code: &str) -> Point {
    Point {
        properties: PointProperties {
            cwa: "TEST".to_string(),
            grid_x: 0,
            grid_y: 0,
            nwr: Nwr {
                same_code: same_code.to_string(),
            },
            astronomical_data: None,
            time_zone: None,
        },
        id: None,
        geometry: PointGeometry {
            coordinates: geo::Point::new(lon, lat),
        },
    }
}

fn ids(alerts: &[Alert]) -> Vec<&str> {
    alerts
        .iter()
        .filter_map(|alert| alert.id.as_deref())
        .collect()
}

#[test]
fn check_alerts_matches_by_same_code() {
    let alerts = load_alerts();
    // Bozeman, MT: Gallatin County, outside every alert polygon
    let point = make_point(45.68, -111.04, "030031");

    let matched = nws::check_alerts(&alerts, &point);

    assert_eq!(ids(&matched), vec![RED_FLAG_WARNING_ID]);
}

#[test]
fn check_alerts_matches_by_geometry() {
    let alerts = load_alerts();
    // inside the Glynn, GA flood advisory polygon, with a SAME code
    // that no alert uses so only the geometry test can match
    let point = make_point(31.15, -81.40, "999999");

    let matched = nws::check_alerts(&alerts, &point);

    assert_eq!(ids(&matched), vec![FLOOD_ADVISORY_ID]);
}

#[test]
fn check_alerts_matches_nothing() {
    let alerts = load_alerts();
    // Eugene, OR: Lane County has no active alerts in the test data
    let point = make_point(44.05, -123.09, "041039");

    let matched = nws::check_alerts(&alerts, &point);

    assert!(
        matched.is_empty(),
        "expected no alerts, got {:?}",
        ids(&matched)
    );
}

#[test]
fn check_alerts_point_on_geometry_boundary() {
    let alerts = load_alerts();
    // on the vertical edge from (-81.37, 31.28) to (-81.37, 31.3)
    // of the Glynn, GA flood advisory polygon
    let point = make_point(31.29, -81.37, "999999");

    let matched = nws::check_alerts(&alerts, &point);

    assert_eq!(ids(&matched), vec![FLOOD_ADVISORY_ID]);
}

#[test]
fn sort_alerts_puts_geometry_alerts_only_in_with_geometry() {
    let alerts = load_alerts();
    let geometry_ids = raw_feature_ids(true);
    let sorted_geometry_ids: HashSet<String> = ids(&alerts.with_geometry)
        .into_iter()
        .map(String::from)
        .collect();
    // an alert is listed under each of its SAME codes,
    // so collect the ids across every bucket
    let same_code_ids: HashSet<&str> = alerts
        .by_same_code
        .values()
        .flat_map(|bucket| ids(bucket))
        .collect();

    assert!(
        !geometry_ids.is_empty(),
        "test data should contain alerts with geometry"
    );
    let missing: Vec<&String> = geometry_ids.difference(&sorted_geometry_ids).collect();
    assert!(
        missing.is_empty(),
        "alerts with geometry missing from with_geometry: {missing:?}"
    );
    let double_dipped: Vec<&String> = geometry_ids
        .iter()
        .filter(|id| same_code_ids.contains(id.as_str()))
        .collect();
    assert!(
        double_dipped.is_empty(),
        "alerts with geometry also in by_same_code: {double_dipped:?}"
    );
}

#[test]
fn sort_alerts_keeps_every_alert() {
    let alerts = load_alerts();
    let all_ids: HashSet<String> = raw_feature_ids(true)
        .union(&raw_feature_ids(false))
        .cloned()
        .collect();
    // unmatched alerts are kept too, they just can't be matched to a location
    let sorted_ids: HashSet<&str> = ids(&alerts.with_geometry)
        .into_iter()
        .chain(alerts.by_same_code.values().flat_map(|bucket| ids(bucket)))
        .chain(ids(&alerts.unmatched))
        .collect();

    let skipped: Vec<&String> = all_ids
        .iter()
        .filter(|id| !sorted_ids.contains(id.as_str()))
        .collect();
    assert!(
        skipped.is_empty(),
        "{} of {} alerts were skipped by sort_alerts: {skipped:?}",
        skipped.len(),
        all_ids.len()
    );
}

// an alert built from raw NWS json, so its geometry deserializes the same way
fn make_alert(id: &str, geometry: serde_json::Value, same_codes: &[&str]) -> Alert {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "geometry": geometry,
        "properties": {
            "event": "Test Warning",
            "geocode": { "SAME": same_codes },
        },
    }))
    .expect("test alert should deserialize")
}

#[test]
fn sort_alerts_keeps_alert_without_geometry_or_same_code_in_unmatched() {
    let alert = make_alert("no-location", serde_json::Value::Null, &[]);
    let alerts = nws::sort_alerts(Alerts {
        features: vec![alert],
    });

    assert_eq!(ids(&alerts.unmatched), vec!["no-location"]);
    assert!(alerts.with_geometry.is_empty());
    assert!(alerts.by_same_code.is_empty());
}

#[test]
fn sort_alerts_matches_unconvertible_geometry_by_same_code() {
    // a point can't be converted to a polygon, so the SAME codes are used instead
    let point = serde_json::json!({ "type": "Point", "coordinates": [-123.0, 44.0] });
    let alert = make_alert("point-geometry", point, &["041039"]);
    let alerts = nws::sort_alerts(Alerts {
        features: vec![alert],
    });

    assert_eq!(
        ids(alerts.get_same_code_alerts("041039")),
        vec!["point-geometry"]
    );
    assert!(alerts.with_geometry.is_empty());
    assert!(alerts.unmatched.is_empty());
}

#[test]
fn sort_alerts_keeps_unconvertible_geometry_without_same_code_in_unmatched() {
    let point = serde_json::json!({ "type": "Point", "coordinates": [-123.0, 44.0] });
    let alert = make_alert("point-no-same", point, &[]);
    let alerts = nws::sort_alerts(Alerts {
        features: vec![alert],
    });

    assert_eq!(ids(&alerts.unmatched), vec!["point-no-same"]);
    assert!(alerts.with_geometry.is_empty());
    assert!(alerts.by_same_code.is_empty());
}

#[test]
fn sort_alerts_test_data_has_no_unmatched_alerts() {
    // every alert in the saved response has geometry or a SAME code
    let alerts = load_alerts();
    assert!(
        alerts.unmatched.is_empty(),
        "unexpected unmatched alerts: {:?}",
        ids(&alerts.unmatched)
    );
}

// runs parse_alerts on a json array of raw NWS features, returning the parsed
// alerts and the records of alerts that failed validation. every feature must
// deserialize, so a mistake in a test alert fails here instead of being skipped
fn parse_features(features: serde_json::Value) -> (Alerts, Vec<InvalidAlert>) {
    let count = features
        .as_array()
        .expect("test features should be a json array")
        .len();
    let raw: RawFeatures = serde_json::from_value(serde_json::json!({ "features": features }))
        .expect("test features should be a feature collection");
    let (alerts, invalid, skipped) = nws::parse_alerts(raw);
    assert!(
        skipped.features.is_empty(),
        "every test feature should deserialize, check the logged skipped features"
    );
    assert_eq!(alerts.features.len(), count);
    (alerts, invalid)
}

#[test]
fn same_code_spec_requires_six_ascii_digits() {
    assert!(Geocode::is_valid_same_code("041039"));
    for code in [
        "41039",
        "0410390",
        " 041039",
        "041039 ",
        "04103a",
        "",
        "٠٤١٠٣٩",
    ] {
        assert!(
            !Geocode::is_valid_same_code(code),
            "{code:?} should be invalid"
        );
    }
}

#[test]
fn parse_alerts_removes_same_codes_outside_spec() {
    let (alerts, _) = parse_features(serde_json::json!([{
        "id": "mixed-codes",
        "geometry": null,
        "properties": { "geocode": { "SAME": ["041039", "41039", "04103a"] } },
    }]));
    assert_eq!(alerts.features[0].properties.geocode.same, vec!["041039"]);
}

#[test]
fn parse_alerts_keeps_alert_with_only_invalid_same_codes() {
    // the alert is kept with no SAME codes, so sort_alerts puts it in unmatched
    let (alerts, _) = parse_features(serde_json::json!([{
        "id": "invalid-codes",
        "geometry": null,
        "properties": { "geocode": { "SAME": ["123"] } },
    }]));
    assert!(alerts.features[0].properties.geocode.same.is_empty());
    assert_eq!(
        ids(&nws::sort_alerts(alerts).unmatched),
        vec!["invalid-codes"]
    );
}

#[test]
fn parse_alerts_keeps_alert_with_null_or_missing_same_codes() {
    // none of these fail deserialization, so each alert is kept and logged
    let (alerts, _) = parse_features(serde_json::json!([
        { "id": "null-same", "geometry": null, "properties": { "geocode": { "SAME": null } } },
        { "id": "missing-same", "geometry": null, "properties": { "geocode": {} } },
        { "id": "null-geocode", "geometry": null, "properties": { "geocode": null } },
        { "id": "missing-geocode", "geometry": null, "properties": {} },
    ]));
    assert_eq!(
        ids(&alerts.features),
        vec![
            "null-same",
            "missing-same",
            "null-geocode",
            "missing-geocode"
        ]
    );
    assert!(
        alerts
            .features
            .iter()
            .all(|alert| alert.properties.geocode.same.is_empty())
    );
}

#[test]
fn parse_alerts_records_alert_with_invalid_same_codes() {
    let (alerts, invalid) = parse_features(serde_json::json!([{
        "id": "mixed-codes",
        "geometry": null,
        "properties": { "geocode": { "SAME": ["041039", "41039"] } },
    }]));

    assert_eq!(invalid.len(), 1);
    let record = &invalid[0];
    assert_eq!(record.alert_id.as_deref(), Some("mixed-codes"));
    assert_eq!(record.reasons, vec![InvalidAlertReason::InvalidSameCodes]);
    assert_eq!(record.invalid_same_codes, vec!["41039"]);
    // the record keeps the alert as received, the parsed alert has only valid codes
    assert_eq!(
        record.alert.properties.geocode.same,
        vec!["041039", "41039"]
    );
    assert_eq!(alerts.features[0].properties.geocode.same, vec!["041039"]);
}

#[test]
fn parse_alerts_records_every_reason_an_alert_failed() {
    // all of the alert's codes are invalid, so it also has no SAME code left
    let (_, invalid) = parse_features(serde_json::json!([
        { "id": "all-invalid", "geometry": null, "properties": { "geocode": { "SAME": ["123"] } } },
        { "id": "no-same", "geometry": null, "properties": {} },
    ]));

    assert_eq!(invalid.len(), 2);
    assert_eq!(
        invalid[0].reasons,
        vec![
            InvalidAlertReason::InvalidSameCodes,
            InvalidAlertReason::NoSameCode
        ]
    );
    assert_eq!(invalid[1].reasons, vec![InvalidAlertReason::NoSameCode]);
    assert!(invalid[1].invalid_same_codes.is_empty());
}

#[test]
fn parse_alerts_records_nothing_for_valid_alerts() {
    let raw: RawFeatures =
        serde_json::from_str(TEST_DATA).expect("test data should be a geojson feature collection");
    let (_, invalid, _) = nws::parse_alerts(raw);
    assert!(
        invalid.is_empty(),
        "unexpected invalid alerts: {:?}",
        invalid
            .iter()
            .map(|record| &record.alert_id)
            .collect::<Vec<_>>()
    );
}

#[test]
fn invalid_alert_round_trips_through_bson() {
    // the record is stored in mongo, including an alert with unconverted geojson
    let point = serde_json::json!({ "type": "Point", "coordinates": [-123.0, 44.0] });
    let (_, invalid) = parse_features(serde_json::json!([{
        "id": "stored",
        "geometry": point,
        "properties": { "event": "Test Warning", "geocode": { "SAME": ["bad"] } },
    }]));

    let stored = bson::to_document(&invalid[0]).expect("invalid alert should serialize");
    assert!(
        stored.get_datetime("first_seen").is_ok(),
        "first_seen should be a bson datetime"
    );
    let read: InvalidAlert = bson::from_document(stored).expect("invalid alert should deserialize");
    assert_eq!(read.alert_id.as_deref(), Some("stored"));
    assert_eq!(read.reasons, invalid[0].reasons);
    assert_eq!(read.alert.properties.event.as_deref(), Some("Test Warning"));
}

// a closed square ring around (-123.0, 44.0)
fn square() -> serde_json::Value {
    serde_json::json!([
        [-124.0, 43.0],
        [-122.0, 43.0],
        [-122.0, 45.0],
        [-124.0, 45.0],
        [-124.0, 43.0]
    ])
}

#[test]
fn parse_alerts_converts_valid_geometry() {
    let (alerts, invalid) = parse_features(serde_json::json!([
        { "id": "polygon", "geometry": { "type": "Polygon", "coordinates": [square()] },
            "properties": { "geocode": { "SAME": ["041039"] } } },
        { "id": "multipolygon", "geometry": { "type": "MultiPolygon", "coordinates": [[square()]] },
            "properties": { "geocode": { "SAME": ["041039"] } } },
    ]));
    assert!(invalid.is_empty());
    assert!(matches!(
        alerts.features[0].geometry,
        Some(GeometryTypes::Polygon(_))
    ));
    assert!(matches!(
        alerts.features[1].geometry,
        Some(GeometryTypes::MultiPolygon(_))
    ));
}

#[test]
fn parse_alerts_records_malformed_geometry_without_panicking() {
    // geojson's conversion panics on a position with fewer than 2 coordinates
    let malformed_ring =
        serde_json::json!([[-124.0, 43.0], [-122.0], [-122.0, 45.0], [-124.0, 43.0]]);
    let (alerts, invalid) = parse_features(serde_json::json!([
        { "id": "malformed-polygon", "geometry": { "type": "Polygon", "coordinates": [malformed_ring] },
            "properties": { "geocode": { "SAME": ["041039"] } } },
        { "id": "malformed-multipolygon",
            "geometry": { "type": "MultiPolygon", "coordinates": [[square()], [malformed_ring]] },
            "properties": { "geocode": { "SAME": ["041039"] } } },
    ]));

    assert_eq!(invalid.len(), 2);
    for record in &invalid {
        assert_eq!(record.reasons, vec![InvalidAlertReason::InvalidGeometry]);
        assert_eq!(
            record.geometry_error_reason,
            Some(GeometryErrorReason::MalformedCoordinates)
        );
        assert!(record.geometry_error.is_some());
        // the record keeps the geometry as received
        assert!(matches!(
            record.alert.geometry,
            Some(GeometryTypes::Geojson(_))
        ));
    }
    // the parsed alerts fall back to their SAME codes
    assert!(alerts.features.iter().all(|alert| alert.geometry.is_none()));
    let sorted = nws::sort_alerts(alerts);
    assert_eq!(
        ids(sorted.get_same_code_alerts("041039")),
        vec!["malformed-polygon", "malformed-multipolygon"]
    );
    assert!(sorted.with_geometry.is_empty());
}

#[test]
fn parse_alerts_records_degenerate_and_unsupported_geometry() {
    let (_, invalid) = parse_features(serde_json::json!([
        { "id": "degenerate", "geometry": { "type": "Polygon",
            "coordinates": [[[-124.0, 43.0], [-122.0, 43.0], [-124.0, 43.0]]] },
            "properties": { "geocode": { "SAME": ["041039"] } } },
        { "id": "point", "geometry": { "type": "Point", "coordinates": [-123.0, 44.0] },
            "properties": { "geocode": { "SAME": ["041039"] } } },
    ]));
    assert_eq!(
        ids(&invalid
            .iter()
            .map(|record| record.alert.clone())
            .collect::<Vec<_>>()),
        vec!["degenerate", "point"]
    );
    assert!(
        invalid
            .iter()
            .all(|record| record.reasons == vec![InvalidAlertReason::InvalidGeometry])
    );
}

#[test]
fn parse_alerts_keeps_alert_with_invalid_geometry_and_no_same_code_in_unmatched() {
    let (alerts, invalid) = parse_features(serde_json::json!([{
        "id": "no-location",
        "geometry": { "type": "Polygon", "coordinates": [[[-124.0], [-122.0, 43.0]]] },
        "properties": {},
    }]));
    assert_eq!(
        invalid[0].reasons,
        vec![
            InvalidAlertReason::NoSameCode,
            InvalidAlertReason::InvalidGeometry
        ]
    );
    assert_eq!(
        ids(&nws::sort_alerts(alerts).unmatched),
        vec!["no-location"]
    );
}

#[test]
fn sort_alerts_does_not_panic_on_malformed_geometry() {
    // sort_alerts checks geojson it's given directly, without parse_alerts
    let ring = serde_json::json!([[[-124.0, 43.0], [-122.0], [-122.0, 45.0], [-124.0, 43.0]]]);
    let alert = make_alert(
        "malformed",
        serde_json::json!({ "type": "Polygon", "coordinates": ring }),
        &["041039"],
    );
    let alerts = nws::sort_alerts(Alerts {
        features: vec![alert],
    });

    assert_eq!(
        ids(alerts.get_same_code_alerts("041039")),
        vec!["malformed"]
    );
    assert!(alerts.with_geometry.is_empty());
}
