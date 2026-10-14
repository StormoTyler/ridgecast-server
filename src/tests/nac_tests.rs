// tests for nac::check_avy_geometry and nac::check_avy_id against a saved NAC map-layer response
use crate::handlers::nac::{self, NACError};
use crate::models::{AvyArea, AvyHashMap, AvyProperties, DateTimeTypes, ForecastVars,
    GeometryTypes, NWR, Point, PointGeometry, PointProperties, RawFeatures, UserPoint};

const TEST_DATA: &str = include_str!("test_data/nac_test_data.json");

// Swan Range, has polygon geometry and a current forecast
const POLYGON_ZONE_ID: &str = "3042";
// Mount Shasta, has multipolygon geometry and a current forecast
const MULTIPOLYGON_ZONE_ID: &str = "1833";
// CAIC zone, a multipolygon whose first polygon has an excluded area
const EXCLUSION_ZONE_ID: &str = "2755";
// Tetons, has a forecast with a start_date
const FORECAST_ZONE_ID: &str = "2855";

// parses the test data the same way nac::get_avy parses a live response
fn load_avy() -> AvyHashMap
{
    let raw: RawFeatures = serde_json::from_str(TEST_DATA)
        .expect("test data should be a geojson feature collection");
    let features: Vec<AvyArea> = raw.parse("NAC test data");
    nac::build_avy_map(features)
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

// an avalanche tracker for the given zone with the given stored forecast
fn make_tracker(avy_id: Option<&str>, avy: Option<AvyProperties>) -> UserPoint
{
    UserPoint
    {
        user: "test".to_string(),
        tracker_name: Some("test".to_string()),
        forecast_vars: ForecastVars { track_avy: true, ..ForecastVars::default() },
        point: make_point(43.74, -110.80),
        avy,
        avy_id: avy_id.map(str::to_string),
        time_added: chrono::Utc::now(),
        valid_duration: 24,
        next_daily_forecast: None,
    }
}

// the current forecast for the zone, as stored in the test data
fn current_forecast(avy: &AvyHashMap) -> AvyProperties
{
    avy.lookup_map[FORECAST_ZONE_ID].properties.clone()
}

// runs check_avy_geometry and returns the id of the matched zone, if any
async fn matched_id(avy: &AvyHashMap, lat: f64, lon: f64) -> Option<String>
{
    nac::check_avy_geometry(&make_point(lat, lon), avy).await
        .expect("point should be checkable")
        .map(|(id, _)| id)
}

#[test]
fn test_zones_have_expected_geometry()
{
    // guards the other tests: each zone must convert to the geometry it is used for
    let avy = load_avy();
    assert!(matches!(avy.lookup_map[POLYGON_ZONE_ID].geometry, GeometryTypes::Polygon(_)));
    assert!(matches!(avy.lookup_map[MULTIPOLYGON_ZONE_ID].geometry, GeometryTypes::MultiPolygon(_)));
    assert!(matches!(avy.lookup_map[EXCLUSION_ZONE_ID].geometry, GeometryTypes::MultiPolygon(_)));
}

#[tokio::test]
async fn test_point_inside_polygon_zone()
{
    let avy = load_avy();
    let zone = nac::check_avy_geometry(&make_point(48.08, -113.86), &avy).await
        .expect("point should be checkable");
    let (id, properties) = zone.expect("point should be inside Swan Range");
    assert_eq!(id, POLYGON_ZONE_ID);
    assert_eq!(properties.name.as_deref(), Some("Swan Range"));
}

#[tokio::test]
async fn test_point_inside_multipolygon_zone()
{
    let avy = load_avy();
    assert_eq!(matched_id(&avy, 41.40, -122.29).await.as_deref(), Some(MULTIPOLYGON_ZONE_ID));
}

#[tokio::test]
async fn test_point_on_polygon_zone_edge()
{
    // a vertex of the Swan Range boundary
    let avy = load_avy();
    assert_eq!(matched_id(&avy, 48.310457276, -113.979234344).await.as_deref(),
        Some(POLYGON_ZONE_ID));
}

#[tokio::test]
async fn test_point_on_multipolygon_zone_edge()
{
    // a vertex of the Mount Shasta boundary
    let avy = load_avy();
    assert_eq!(matched_id(&avy, 41.45795, -121.87457).await.as_deref(),
        Some(MULTIPOLYGON_ZONE_ID));
}

#[tokio::test]
async fn test_point_outside_polygon_zone()
{
    // just west of Swan Range, not in any zone
    let avy = load_avy();
    assert_eq!(matched_id(&avy, 48.08, -114.18).await, None);
}

#[tokio::test]
async fn test_point_outside_multipolygon_zone()
{
    // just west of Mount Shasta, not in any zone
    let avy = load_avy();
    assert_eq!(matched_id(&avy, 41.40, -122.78).await, None);
}

#[tokio::test]
async fn test_point_in_multipolygon_exclusion()
{
    // inside the CAIC zone's outer boundary but within its excluded area,
    // which no other zone covers
    let avy = load_avy();
    assert_eq!(matched_id(&avy, 40.08, -106.36).await, None);
}

#[tokio::test]
async fn test_point_on_multipolygon_exclusion_edge()
{
    // a vertex of the CAIC zone's excluded area boundary,
    // the edge belongs to the zone so the point is included
    let avy = load_avy();
    assert_eq!(matched_id(&avy, 40.2076, -106.5401).await.as_deref(),
        Some(EXCLUSION_ZONE_ID));
}

#[tokio::test]
async fn test_point_in_zone_without_forecast()
{
    // Central Cascades is in season but has no start_date
    let avy = load_avy();
    let result = nac::check_avy_geometry(&make_point(44.19, -121.75), &avy).await;
    assert!(matches!(result, Err(NACError::NoForecastError(id)) if id == "2470"));
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
async fn test_avy_id_without_tracked_zone()
{
    let avy = load_avy();
    let mut tracker = make_tracker(None, None);
    assert!(!nac::check_avy_id(&mut tracker, &avy).await);
    assert!(tracker.avy.is_none());
}

#[tokio::test]
async fn test_avy_id_for_unknown_zone()
{
    // a zone id that is not in the current data
    let avy = load_avy();
    let mut tracker = make_tracker(Some("0"), None);
    assert!(!nac::check_avy_id(&mut tracker, &avy).await);
    assert!(tracker.avy.is_none());
}

#[tokio::test]
async fn test_avy_id_with_no_stored_forecast()
{
    // the first forecast seen for a tracked zone counts as new
    let avy = load_avy();
    let mut tracker = make_tracker(Some(FORECAST_ZONE_ID), None);
    assert!(nac::check_avy_id(&mut tracker, &avy).await);

    let stored = tracker.avy.expect("forecast should be stored");
    assert_eq!(stored.start_date, current_forecast(&avy).start_date);
}

#[tokio::test]
async fn test_avy_id_for_zone_without_forecast()
{
    // Central Cascades has no start_date, so there is no forecast to store yet
    let avy = load_avy();
    let mut tracker = make_tracker(Some("2470"), None);
    assert!(!nac::check_avy_id(&mut tracker, &avy).await);
    assert!(tracker.avy.is_none());
}

#[tokio::test]
async fn test_avy_id_for_off_season_zone()
{
    // Chugach State Park is off season
    let avy = load_avy();
    let mut tracker = make_tracker(Some("2818"), None);
    assert!(!nac::check_avy_id(&mut tracker, &avy).await);
    assert!(tracker.avy.is_none());
}

#[tokio::test]
async fn test_avy_id_with_same_forecast()
{
    // only start_date marks a new forecast, so other changed fields
    // leave the stored forecast alone
    let avy = load_avy();
    let mut stored = current_forecast(&avy);
    stored.travel_advice = Some("stored advice".to_string());
    let mut tracker = make_tracker(Some(FORECAST_ZONE_ID), Some(stored));

    assert!(!nac::check_avy_id(&mut tracker, &avy).await);
    let stored = tracker.avy.expect("forecast should still be stored");
    assert_eq!(stored.travel_advice.as_deref(), Some("stored advice"));
}

#[tokio::test]
async fn test_avy_id_with_new_forecast()
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
    let mut tracker = make_tracker(Some(FORECAST_ZONE_ID), Some(stored));

    assert!(nac::check_avy_id(&mut tracker, &avy).await);
    let stored = tracker.avy.expect("forecast should be stored");
    assert_eq!(stored.start_date, current.start_date);
    assert_eq!(stored.travel_advice, current.travel_advice);
}
