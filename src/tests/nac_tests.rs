// tests for nac::check_avy_geometry and nac::check_avy_tracker against a saved NAC map-layer response
use crate::handlers::nac::{self, NACError};
use crate::models::{AvalancheData, AvyArea, AvyProperties, DateTimeTypes, ForecastVars,
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
    AvalancheData { features: nac::build_avy_map(features), fetched_at: chrono::Utc::now() }
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
