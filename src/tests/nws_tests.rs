// tests for nws::sort_alerts and nws::check_alerts against a saved /alerts/active response
use std::collections::HashSet;

use crate::handlers::nws;
use crate::models::{Alert, Alerts, AlertsSorted, NWR, Point, PointGeometry,
    PointProperties, RawFeatures};

const TEST_DATA: &str = include_str!("test_data/nws_alerts_test_data");

// Red Flag Warning for Gallatin National Forest, has no geometry
const RED_FLAG_WARNING_ID: &str =
    "https://api.weather.gov/alerts/urn:oid:2.49.0.1.840.0.98ed8e221a941affa647df5a3746567e0c58d8bc.001.1";
// Flood Advisory for Glynn, GA, has a polygon
const FLOOD_ADVISORY_ID: &str =
    "https://api.weather.gov/alerts/urn:oid:2.49.0.1.840.0.2ec9803dbea4b774a0cc081c7640d75ab17c11d5.001.1";

// parses the test data the same way nws::get_alerts parses a live response
fn load_alerts() -> AlertsSorted
{
    let raw: RawFeatures = serde_json::from_str(TEST_DATA)
        .expect("test data should be a geojson feature collection");
    nws::sort_alerts(Alerts { features: raw.parse("NWS alerts test data") })
}

// ids of the features in the raw test data, read straight from the json
// so features the parser would skip are still counted
fn raw_feature_ids(with_geometry: bool) -> HashSet<String>
{
    let raw: RawFeatures = serde_json::from_str(TEST_DATA)
        .expect("test data should be a geojson feature collection");
    raw.features.iter()
        .filter(|feature| feature["geometry"].is_null() != with_geometry)
        .map(|feature| feature["id"].as_str()
            .expect("every test feature should have an id")
            .to_string())
        .collect()
}

fn make_point(lat: f64, lon: f64, same_code: &str) -> Point
{
    Point
    {
        properties: PointProperties
        {
            cwa: "TEST".to_string(),
            grid_x: 0,
            grid_y: 0,
            nwr: NWR { same_code: same_code.to_string() },
            astronomical_data: None,
            time_zone: None,
        },
        id: None,
        geometry: PointGeometry { coordinates: geo::Point::new(lon, lat) },
    }
}

fn ids(alerts: &[Alert]) -> Vec<&str>
{
    alerts.iter().filter_map(|alert| alert.id.as_deref()).collect()
}

#[test]
fn check_alerts_matches_by_same_code()
{
    let alerts = load_alerts();
    // Bozeman, MT: Gallatin County, outside every alert polygon
    let point = make_point(45.68, -111.04, "030031");

    let matched = nws::check_alerts(&alerts, &point);

    assert_eq!(ids(&matched), vec![RED_FLAG_WARNING_ID]);
}

#[test]
fn check_alerts_matches_by_geometry()
{
    let alerts = load_alerts();
    // inside the Glynn, GA flood advisory polygon, with a SAME code
    // that no alert uses so only the geometry test can match
    let point = make_point(31.15, -81.40, "999999");

    let matched = nws::check_alerts(&alerts, &point);

    assert_eq!(ids(&matched), vec![FLOOD_ADVISORY_ID]);
}

#[test]
fn check_alerts_matches_nothing()
{
    let alerts = load_alerts();
    // Eugene, OR: Lane County has no active alerts in the test data
    let point = make_point(44.05, -123.09, "041039");

    let matched = nws::check_alerts(&alerts, &point);

    assert!(matched.is_empty(), "expected no alerts, got {:?}", ids(&matched));
}

#[test]
fn check_alerts_point_on_geometry_boundary()
{
    let alerts = load_alerts();
    // on the vertical edge from (-81.37, 31.28) to (-81.37, 31.3)
    // of the Glynn, GA flood advisory polygon
    let point = make_point(31.29, -81.37, "999999");

    let matched = nws::check_alerts(&alerts, &point);

    assert_eq!(ids(&matched), vec![FLOOD_ADVISORY_ID]);
}

#[test]
fn sort_alerts_puts_geometry_alerts_only_in_with_geometry()
{
    let alerts = load_alerts();
    let geometry_ids = raw_feature_ids(true);
    let sorted_geometry_ids: HashSet<String> = ids(&alerts.with_geometry)
        .into_iter().map(String::from).collect();
    // an alert is listed under each of its SAME codes,
    // so collect the ids across every bucket
    let same_code_ids: HashSet<&str> = alerts.by_same_code.values()
        .flat_map(|bucket| ids(bucket))
        .collect();

    assert!(!geometry_ids.is_empty(), "test data should contain alerts with geometry");
    let missing: Vec<&String> = geometry_ids.difference(&sorted_geometry_ids).collect();
    assert!(missing.is_empty(), "alerts with geometry missing from with_geometry: {missing:?}");
    let double_dipped: Vec<&String> = geometry_ids.iter()
        .filter(|id| same_code_ids.contains(id.as_str()))
        .collect();
    assert!(double_dipped.is_empty(),
        "alerts with geometry also in by_same_code: {double_dipped:?}");
}

#[test]
fn sort_alerts_keeps_every_alert()
{
    let alerts = load_alerts();
    let all_ids: HashSet<String> = raw_feature_ids(true)
        .union(&raw_feature_ids(false)).cloned().collect();
    // unmatched alerts are kept too, they just can't be matched to a location
    let sorted_ids: HashSet<&str> = ids(&alerts.with_geometry).into_iter()
        .chain(alerts.by_same_code.values().flat_map(|bucket| ids(bucket)))
        .chain(ids(&alerts.unmatched))
        .collect();

    let skipped: Vec<&String> = all_ids.iter()
        .filter(|id| !sorted_ids.contains(id.as_str()))
        .collect();
    assert!(skipped.is_empty(),
        "{} of {} alerts were skipped by sort_alerts: {skipped:?}",
        skipped.len(), all_ids.len());
}

// an alert built from raw NWS json, so its geometry deserializes the same way
fn make_alert(id: &str, geometry: serde_json::Value, same_codes: &[&str]) -> Alert
{
    serde_json::from_value(serde_json::json!({
        "id": id,
        "geometry": geometry,
        "properties": {
            "event": "Test Warning",
            "geocode": { "SAME": same_codes },
        },
    })).expect("test alert should deserialize")
}

#[test]
fn sort_alerts_keeps_alert_without_geometry_or_same_code_in_unmatched()
{
    let alert = make_alert("no-location", serde_json::Value::Null, &[]);
    let alerts = nws::sort_alerts(Alerts { features: vec![alert] });

    assert_eq!(ids(&alerts.unmatched), vec!["no-location"]);
    assert!(alerts.with_geometry.is_empty());
    assert!(alerts.by_same_code.is_empty());
}

#[test]
fn sort_alerts_matches_unconvertible_geometry_by_same_code()
{
    // a point can't be converted to a polygon, so the SAME codes are used instead
    let point = serde_json::json!({ "type": "Point", "coordinates": [-123.0, 44.0] });
    let alert = make_alert("point-geometry", point, &["041039"]);
    let alerts = nws::sort_alerts(Alerts { features: vec![alert] });

    assert_eq!(ids(alerts.get_same_code_alerts("041039")), vec!["point-geometry"]);
    assert!(alerts.with_geometry.is_empty());
    assert!(alerts.unmatched.is_empty());
}

#[test]
fn sort_alerts_keeps_unconvertible_geometry_without_same_code_in_unmatched()
{
    let point = serde_json::json!({ "type": "Point", "coordinates": [-123.0, 44.0] });
    let alert = make_alert("point-no-same", point, &[]);
    let alerts = nws::sort_alerts(Alerts { features: vec![alert] });

    assert_eq!(ids(&alerts.unmatched), vec!["point-no-same"]);
    assert!(alerts.with_geometry.is_empty());
    assert!(alerts.by_same_code.is_empty());
}

#[test]
fn sort_alerts_test_data_has_no_unmatched_alerts()
{
    // every alert in the saved response has geometry or a SAME code
    let alerts = load_alerts();
    assert!(alerts.unmatched.is_empty(), "unexpected unmatched alerts: {:?}", ids(&alerts.unmatched));
}
