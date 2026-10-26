// tests for nac::check_avy_geometry and nac::check_avy_tracker against a saved NAC map-layer response
use crate::handlers::nac::{self, NACError};
use crate::models::{AvalancheData, AvyArea, AvyProperties, DateTimeTypes, ForecastVars,
    InvalidAvyArea, GeometryErrorReason,
    GeometryTypes, NWR, Point, PointGeometry, PointProperties, RawFeatures, UserPoint};

const TEST_DATA: &str = include_str!("test_data/nac_test_data.json");

// has polygon geometry and a current forecast
const POLYGON_ZONE: &str = "Swan Range";
// has multipolygon geometry and a current forecast
const MULTIPOLYGON_ZONE: &str = "Mount Shasta";
// a multipolygon whose first polygon has an excluded area
const EXCLUSION_ZONE: &str = "CAIC zone";
// has a forecast with a start_date
const FORECAST_ZONE: &str = "Tetons";

// parses the test data the same way nac::get_avy parses a live response
fn load_avy() -> AvalancheData
{
    let raw: RawFeatures = serde_json::from_str(TEST_DATA)
        .expect("test data should be a geojson feature collection");
    let features: Vec<AvyArea> = raw.parse("NAC test data");
    AvalancheData { features: nac::build_avy_map(features).0, fetched_at: chrono::Utc::now() }
}

// the zone with the given name, zone names are unique in the test data
fn find_zone<'a>(avy: &'a AvalancheData, name: &str) -> &'a AvyArea
{
    avy.features.iter()
        .find(|area| area.properties.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("test data should contain {name}"))
}

fn make_point(lat: f64, lon: f64) -> Point
{
    Point
    {
        properties: PointProperties
        {
            cwa: "TEST".to_string(),
            grid_x: 0,
            grid_y: 0,
            nwr: NWR { same_code: "000000".to_string() },
            astronomical_data: None,
            time_zone: None,
        },
        id: None,
        geometry: PointGeometry { coordinates: geo::Point::new(lon, lat) },
    }
}

// an avalanche tracker inside the Tetons zone with the given stored forecast
fn make_tracker(avy: Option<AvyProperties>) -> UserPoint
{
    UserPoint
    {
        user: "test".to_string(),
        tracker_name: Some("test".to_string()),
        forecast_vars: ForecastVars { track_avy: true, ..ForecastVars::default() },
        point: make_point(43.74, -110.80),
        avy,
        time_added: chrono::Utc::now(),
        valid_duration: 24,
        next_daily_forecast: None,
    }
}

// the current forecast for the tracker's zone, as stored in the test data
fn current_forecast(avy: &AvalancheData) -> AvyProperties
{
    find_zone(avy, FORECAST_ZONE).properties.clone()
}

// runs check_avy_geometry and returns the name of the matched zone, if any
async fn matched_name(avy: &AvalancheData, lat: f64, lon: f64) -> Option<String>
{
    nac::check_avy_geometry(&make_point(lat, lon), avy).await
        .expect("point should be checkable")
        .and_then(|properties| properties.name)
}

#[test]
fn test_zones_have_expected_geometry()
{
    // guards the other tests: each zone must convert to the geometry it is used for
    let avy = load_avy();
    assert!(matches!(find_zone(&avy, POLYGON_ZONE).geometry, GeometryTypes::Polygon(_)));
    assert!(matches!(find_zone(&avy, MULTIPOLYGON_ZONE).geometry, GeometryTypes::MultiPolygon(_)));
    assert!(matches!(find_zone(&avy, EXCLUSION_ZONE).geometry, GeometryTypes::MultiPolygon(_)));
}

#[tokio::test]
async fn test_point_inside_polygon_zone()
{
    let avy = load_avy();
    let zone = nac::check_avy_geometry(&make_point(48.08, -113.86), &avy).await
        .expect("point should be checkable");
    let properties = zone.expect("point should be inside Swan Range");
    assert_eq!(properties.name.as_deref(), Some(POLYGON_ZONE));
    assert_eq!(properties.center.as_deref(), Some("Flathead Avalanche Center"));
}

#[tokio::test]
async fn test_point_inside_multipolygon_zone()
{
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 41.40, -122.29).await.as_deref(), Some(MULTIPOLYGON_ZONE));
}

#[tokio::test]
async fn test_point_on_polygon_zone_edge()
{
    // a vertex of the Swan Range boundary
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 48.310457276, -113.979234344).await.as_deref(),
        Some(POLYGON_ZONE));
}

#[tokio::test]
async fn test_point_on_multipolygon_zone_edge()
{
    // a vertex of the Mount Shasta boundary
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 41.45795, -121.87457).await.as_deref(),
        Some(MULTIPOLYGON_ZONE));
}

#[tokio::test]
async fn test_point_outside_polygon_zone()
{
    // just west of Swan Range, not in any zone
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 48.08, -114.18).await, None);
}

#[tokio::test]
async fn test_point_outside_multipolygon_zone()
{
    // just west of Mount Shasta, not in any zone
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 41.40, -122.78).await, None);
}

#[tokio::test]
async fn test_point_in_multipolygon_exclusion()
{
    // inside the CAIC zone's outer boundary but within its excluded area,
    // which no other zone covers
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 40.08, -106.36).await, None);
}

#[tokio::test]
async fn test_point_on_multipolygon_exclusion_edge()
{
    // a vertex of the CAIC zone's excluded area boundary,
    // the edge belongs to the zone so the point is included
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 40.2076, -106.5401).await.as_deref(),
        Some(EXCLUSION_ZONE));
}

#[tokio::test]
async fn test_point_in_zone_without_forecast()
{
    // Central Cascades is in season but has no start_date
    let avy = load_avy();
    let result = nac::check_avy_geometry(&make_point(44.19, -121.75), &avy).await;
    assert!(matches!(result, Err(NACError::NoForecastError)));
}

#[tokio::test]
async fn test_point_in_off_season_zone()
{
    // Chugach State Park is off season and also has no start_date,
    // off season takes priority
    let avy = load_avy();
    let result = nac::check_avy_geometry(&make_point(61.13, -149.53), &avy).await;
    assert!(matches!(result, Err(NACError::OffSeasonError)));
}

#[tokio::test]
async fn test_avy_tracker_with_same_forecast()
{
    // a forecast identical to the stored one is not new
    let avy = load_avy();
    let mut tracker = make_tracker(Some(current_forecast(&avy)));

    assert!(!nac::check_avy_tracker(&mut tracker, &avy).await);
    assert_eq!(tracker.avy, Some(current_forecast(&avy)));
}

#[tokio::test]
async fn test_avy_tracker_with_changed_field()
{
    // any changed field counts as a new forecast, even with the same start_date
    let avy = load_avy();
    let current = current_forecast(&avy);
    let mut stored = current.clone();
    stored.travel_advice = Some("stored advice".to_string());
    let mut tracker = make_tracker(Some(stored));

    assert!(nac::check_avy_tracker(&mut tracker, &avy).await);
    let stored = tracker.avy.expect("forecast should be stored");
    assert_eq!(stored.start_date, current.start_date);
    assert_eq!(stored.travel_advice, current.travel_advice);
}

#[tokio::test]
async fn test_avy_tracker_with_earlier_start_date()
{
    // the stored forecast started a day before the current one
    let avy = load_avy();
    let current = current_forecast(&avy);
    let Some(DateTimeTypes::Aware(start)) = current.start_date
    else
    {
        panic!("test zone start_date should be converted to local time");
    };
    let mut stored = current.clone();
    stored.start_date = Some(DateTimeTypes::Aware(start - chrono::Duration::days(1)));
    stored.travel_advice = Some("stored advice".to_string());
    let mut tracker = make_tracker(Some(stored));

    assert!(nac::check_avy_tracker(&mut tracker, &avy).await);
    // the whole forecast is replaced, not just its start_date
    assert_eq!(tracker.avy, Some(current));
}

#[tokio::test]
async fn test_avy_tracker_with_no_stored_forecast()
{
    // a tracker without a stored forecast still checks for one,
    // and the first forecast found counts as new
    let avy = load_avy();
    let mut tracker = make_tracker(None);

    assert!(nac::check_avy_tracker(&mut tracker, &avy).await);
    assert_eq!(tracker.avy, Some(current_forecast(&avy)));
}

#[tokio::test]
async fn test_avy_tracker_in_off_season_zone()
{
    // Chugach State Park is off season, so there is no forecast to store
    let avy = load_avy();
    let mut tracker = make_tracker(None);
    tracker.point = make_point(61.13, -149.53);

    assert!(!nac::check_avy_tracker(&mut tracker, &avy).await);
    assert!(tracker.avy.is_none());
}

// runs build_avy_map on a json array of raw NAC zones with the given geometries,
// returning the zones and the records of zones that failed. every zone must
// deserialize, so a mistake in a test zone fails here instead of being skipped
fn build_zones(geometries: &[serde_json::Value]) -> (Vec<AvyArea>, Vec<InvalidAvyArea>)
{
    let features: Vec<serde_json::Value> = geometries.iter().enumerate()
        .map(|(i, geometry)| serde_json::json!({
            "id": i,
            "properties": { "name": format!("Zone {i}"), "center": "Test Center" },
            "geometry": geometry,
        }))
        .collect();
    let raw: RawFeatures = serde_json::from_value(serde_json::json!({ "features": features }))
        .expect("test zones should be a feature collection");
    let parsed: Vec<AvyArea> = raw.parse("NAC test zones");
    assert_eq!(parsed.len(), geometries.len(),
        "every test zone should deserialize, check the logged skipped features");
    nac::build_avy_map(parsed)
}

// a closed square ring, the smallest valid exterior ring
fn square() -> serde_json::Value
{
    serde_json::json!([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]])
}

#[test]
fn test_build_avy_map_test_data_has_no_invalid_zones()
{
    let raw: RawFeatures = serde_json::from_str(TEST_DATA)
        .expect("test data should be a geojson feature collection");
    let (_, invalid) = nac::build_avy_map(raw.parse("NAC test data"));
    assert!(invalid.is_empty(), "unexpected invalid zones: {:?}",
        invalid.iter().map(|record| &record.zone_name).collect::<Vec<_>>());
}

#[test]
fn test_build_avy_map_converts_valid_polygon()
{
    let (zones, invalid) = build_zones(&[serde_json::json!({ "type": "Polygon", "coordinates": [square()] })]);
    assert!(invalid.is_empty());
    assert!(matches!(zones[0].geometry, GeometryTypes::Polygon(_)));
}

#[test]
fn test_build_avy_map_records_unsupported_geometry()
{
    let (zones, invalid) = build_zones(&[serde_json::json!({ "type": "Point", "coordinates": [0.0, 0.0] })]);

    assert_eq!(invalid.len(), 1);
    assert_eq!(invalid[0].reason, GeometryErrorReason::UnsupportedGeometryType);
    assert_eq!(invalid[0].zone_name.as_deref(), Some("Zone 0"));
    assert_eq!(invalid[0].center.as_deref(), Some("Test Center"));
    // the zone is kept with its geometry unconverted, so it never matches
    assert!(matches!(zones[0].geometry, GeometryTypes::Geojson(_)));
    assert!(matches!(invalid[0].area.geometry, GeometryTypes::Geojson(_)));
}

#[test]
fn test_build_avy_map_records_malformed_coordinates_instead_of_panicking()
{
    // geojson's conversion panics on a position with one coordinate
    let ring = serde_json::json!([[0.0, 0.0], [1.0], [1.0, 1.0], [0.0, 0.0]]);
    let (zones, invalid) = build_zones(&[
        serde_json::json!({ "type": "Polygon", "coordinates": [ring] }),
        serde_json::json!({ "type": "MultiPolygon", "coordinates": [[square()], [ring]] }),
    ]);

    assert_eq!(invalid.len(), 2);
    assert!(invalid.iter().all(|record| record.reason == GeometryErrorReason::MalformedCoordinates));
    assert!(zones.iter().all(|zone| matches!(zone.geometry, GeometryTypes::Geojson(_))));
}

#[test]
fn test_build_avy_map_records_degenerate_geometry()
{
    // these convert without error but could never contain a point
    let short_ring = serde_json::json!([[0.0, 0.0], [1.0, 0.0], [0.0, 0.0]]);
    let (_, invalid) = build_zones(&[
        serde_json::json!({ "type": "Polygon", "coordinates": [] }),
        serde_json::json!({ "type": "Polygon", "coordinates": [[]] }),
        serde_json::json!({ "type": "Polygon", "coordinates": [short_ring] }),
        serde_json::json!({ "type": "MultiPolygon", "coordinates": [] }),
    ]);

    assert_eq!(invalid.len(), 4);
    assert!(invalid.iter().all(|record| record.reason == GeometryErrorReason::DegenerateGeometry));
}

#[test]
fn test_invalid_avy_area_round_trips_through_bson()
{
    let (_, invalid) = build_zones(&[serde_json::json!({ "type": "Point", "coordinates": [0.0, 0.0] })]);

    let stored = bson::to_document(&invalid[0]).expect("invalid zone should serialize");
    assert!(stored.get_datetime("first_seen").is_ok(), "first_seen should be a bson datetime");
    let read: InvalidAvyArea = bson::from_document(stored).expect("invalid zone should deserialize");
    assert_eq!(read.zone_name.as_deref(), Some("Zone 0"));
    assert_eq!(read.reason, GeometryErrorReason::UnsupportedGeometryType);
}
