// tests for nac::check_avy_geometry and nac::check_avy_tracker against a saved NAC map-layer response
use chrono::{DateTime, TimeZone, Utc};

use crate::models::{
    AvalancheData, AvyArea, AvyProperties, DateTimeTypes, ForecastVars, GeometryErrorReason,
    GeometryTypes, InvalidAvyArea, Nwr, Point, PointGeometry, PointProperties, RawFeatures,
    Snapshot, UserPoint,
};
use crate::upstream::nac;

const TEST_DATA: &str = include_str!("test_data/nac_test_data.json");

// has polygon geometry and a current forecast
const POLYGON_ZONE: &str = "Swan Range";
// has multipolygon geometry and a current forecast
const MULTIPOLYGON_ZONE: &str = "Mount Shasta";
// a multipolygon whose first polygon has an excluded area
const EXCLUSION_ZONE: &str = "CAIC zone";
// has a forecast with a start_date
const FORECAST_ZONE: &str = "Tetons";

// every forecast in the test data has started and none has ended
// (starts run to 2026-10-02 22:49, ends from 2026-10-31 08:00 UTC)
fn test_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 3, 0, 0, 0).unwrap()
}

// after the Tetons forecast ends at 2026-10-31 08:00 UTC
fn after_tetons_end() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 31, 8, 0, 0).unwrap()
}

// parses the test data the same way nac::get_avy parses a live response
fn load_avy() -> AvalancheData {
    let raw: RawFeatures =
        serde_json::from_str(TEST_DATA).expect("test data should be a geojson feature collection");
    let (features, _): (Vec<AvyArea>, _) = raw.parse("NAC test data");
    AvalancheData {
        features: nac::build_avy_map(features).0,
        fetched_at: chrono::Utc::now(),
    }
}

// the zone with the given name, zone names are unique in the test data
fn find_zone<'a>(avy: &'a AvalancheData, name: &str) -> &'a AvyArea {
    avy.features
        .iter()
        .find(|area| area.properties.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("test data should contain {name}"))
}

fn make_point(lat: f64, lon: f64) -> Point {
    Point {
        properties: PointProperties {
            cwa: "TEST".to_string(),
            grid_x: 0,
            grid_y: 0,
            nwr: Nwr {
                same_code: "000000".to_string(),
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

// an avalanche tracker inside the Tetons zone with the given stored forecast
fn make_tracker(avy: Option<AvyProperties>) -> UserPoint {
    UserPoint {
        id: None,
        user: "test".to_string(),
        tracker_name: Some("test".to_string()),
        forecast_vars: ForecastVars {
            track_avy: true,
            ..ForecastVars::default()
        },
        point: make_point(43.74, -110.80),
        avy,
        avy_lost: Default::default(),
        avy_off_season: Default::default(),
        time_added: chrono::Utc::now(),
        valid_duration: 24,
        next_daily_forecast: None,
    }
}

// the current forecast for the tracker's zone, as stored in the test data
fn current_forecast(avy: &AvalancheData) -> AvyProperties {
    find_zone(avy, FORECAST_ZONE).properties.clone()
}

// runs check_avy_geometry and returns the name of the matched zone, if any
async fn matched_name(avy: &AvalancheData, lat: f64, lon: f64) -> Option<String> {
    match nac::check_avy_geometry(&make_point(lat, lon), avy, test_now()).await {
        nac::ZoneForecast::Current(properties) => properties.name.clone(),
        nac::ZoneForecast::NoZone => None,
        other => panic!("geometry test zones should have a current forecast, got {other:?}"),
    }
}

// runs check_avy_tracker on fresh avalanche data
async fn check_tracker(
    tracker: &mut UserPoint,
    avy: &AvalancheData,
    now: DateTime<Utc>,
) -> nac::AvyTrackerUpdate {
    nac::check_avy_tracker(
        tracker,
        &Snapshot {
            data: avy.clone(),
            stale: false,
        },
        now,
    )
    .await
}

// runs check_avy_tracker on stale avalanche data
async fn check_tracker_stale(
    tracker: &mut UserPoint,
    avy: &AvalancheData,
    now: DateTime<Utc>,
) -> nac::AvyTrackerUpdate {
    nac::check_avy_tracker(
        tracker,
        &Snapshot {
            data: avy.clone(),
            stale: true,
        },
        now,
    )
    .await
}

#[test]
fn test_zones_have_expected_geometry() {
    // guards the other tests: each zone must convert to the geometry it is used for
    let avy = load_avy();
    assert!(matches!(
        find_zone(&avy, POLYGON_ZONE).geometry,
        GeometryTypes::Polygon(_)
    ));
    assert!(matches!(
        find_zone(&avy, MULTIPOLYGON_ZONE).geometry,
        GeometryTypes::MultiPolygon(_)
    ));
    assert!(matches!(
        find_zone(&avy, EXCLUSION_ZONE).geometry,
        GeometryTypes::MultiPolygon(_)
    ));
}

#[tokio::test]
async fn test_point_inside_polygon_zone() {
    let avy = load_avy();
    let nac::ZoneForecast::Current(properties) =
        nac::check_avy_geometry(&make_point(48.08, -113.86), &avy, test_now()).await
    else {
        panic!("point should be inside Swan Range, which has a current forecast");
    };
    assert_eq!(properties.name.as_deref(), Some(POLYGON_ZONE));
    assert_eq!(
        properties.center.as_deref(),
        Some("Flathead Avalanche Center")
    );
}

#[tokio::test]
async fn test_point_inside_multipolygon_zone() {
    let avy = load_avy();
    assert_eq!(
        matched_name(&avy, 41.40, -122.29).await.as_deref(),
        Some(MULTIPOLYGON_ZONE)
    );
}

#[tokio::test]
async fn test_point_on_polygon_zone_edge() {
    // a vertex of the Swan Range boundary
    let avy = load_avy();
    assert_eq!(
        matched_name(&avy, 48.310457276, -113.979234344)
            .await
            .as_deref(),
        Some(POLYGON_ZONE)
    );
}

#[tokio::test]
async fn test_point_on_multipolygon_zone_edge() {
    // a vertex of the Mount Shasta boundary
    let avy = load_avy();
    assert_eq!(
        matched_name(&avy, 41.45795, -121.87457).await.as_deref(),
        Some(MULTIPOLYGON_ZONE)
    );
}

#[tokio::test]
async fn test_point_outside_polygon_zone() {
    // just west of Swan Range, not in any zone
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 48.08, -114.18).await, None);
}

#[tokio::test]
async fn test_point_outside_multipolygon_zone() {
    // just west of Mount Shasta, not in any zone
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 41.40, -122.78).await, None);
}

#[tokio::test]
async fn test_point_in_multipolygon_exclusion() {
    // inside the CAIC zone's outer boundary but within its excluded area,
    // which no other zone covers
    let avy = load_avy();
    assert_eq!(matched_name(&avy, 40.08, -106.36).await, None);
}

#[tokio::test]
async fn test_point_on_multipolygon_exclusion_edge() {
    // a vertex of the CAIC zone's excluded area boundary,
    // the edge belongs to the zone so the point is included
    let avy = load_avy();
    assert_eq!(
        matched_name(&avy, 40.2076, -106.5401).await.as_deref(),
        Some(EXCLUSION_ZONE)
    );
}

#[tokio::test]
async fn test_point_in_zone_without_forecast() {
    // Central Cascades is in season but has no start_date
    let avy = load_avy();
    let result = nac::check_avy_geometry(&make_point(44.19, -121.75), &avy, test_now()).await;
    assert_eq!(result, nac::ZoneForecast::NoForecast);
}

#[tokio::test]
async fn test_point_in_off_season_zone() {
    // Chugach State Park is off season and also has no start_date,
    // off season takes priority
    let avy = load_avy();
    let result = nac::check_avy_geometry(&make_point(61.13, -149.53), &avy, test_now()).await;
    assert_eq!(result, nac::ZoneForecast::OffSeason);
}

#[tokio::test]
async fn test_avy_tracker_with_same_forecast() {
    // a forecast identical to the stored one is not new
    let avy = load_avy();
    let mut tracker = make_tracker(Some(current_forecast(&avy)));

    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
    assert_eq!(tracker.avy, Some(current_forecast(&avy)));
}

#[tokio::test]
async fn test_avy_tracker_with_changed_field() {
    // any changed field counts as a new forecast, even with the same start_date
    let avy = load_avy();
    let current = current_forecast(&avy);
    let mut stored = current.clone();
    stored.travel_advice = Some("stored advice".to_string());
    let mut tracker = make_tracker(Some(stored));

    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::NewForecast
    );
    let stored = tracker.avy.expect("forecast should be stored");
    assert_eq!(stored.start_date, current.start_date);
    assert_eq!(stored.travel_advice, current.travel_advice);
}

#[tokio::test]
async fn test_avy_tracker_with_earlier_start_date() {
    // the stored forecast started a day before the current one
    let avy = load_avy();
    let current = current_forecast(&avy);
    let Some(DateTimeTypes::Aware(start)) = current.start_date else {
        panic!("test zone start_date should be converted to local time");
    };
    let mut stored = current.clone();
    stored.start_date = Some(DateTimeTypes::Aware(start - chrono::Duration::days(1)));
    stored.travel_advice = Some("stored advice".to_string());
    let mut tracker = make_tracker(Some(stored));

    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::NewForecast
    );
    // the whole forecast is replaced, not just its start_date
    assert_eq!(tracker.avy, Some(current));
}

#[tokio::test]
async fn test_avy_tracker_with_no_stored_forecast() {
    // a tracker without a stored forecast still checks for one,
    // and the first forecast found counts as new
    let avy = load_avy();
    let mut tracker = make_tracker(None);

    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::NewForecast
    );
    assert_eq!(tracker.avy, Some(current_forecast(&avy)));
}

#[tokio::test]
async fn test_avy_tracker_in_off_season_zone() {
    // Chugach State Park is off season, so there is no forecast to store
    let avy = load_avy();
    let mut tracker = make_tracker(None);
    tracker.point = make_point(61.13, -149.53);

    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
    assert!(tracker.avy.is_none());
    // there was no forecast to lose, so no loss is recorded
    assert!(!tracker.avy_lost.is_raised());
}

// the zone with the given name, for tests that change its forecast
fn find_zone_mut<'a>(avy: &'a mut AvalancheData, name: &str) -> &'a mut AvyArea {
    avy.features
        .iter_mut()
        .find(|area| area.properties.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("test data should contain {name}"))
}

// a tracker in the Tetons zone that was already sent its current forecast
fn tracker_with_current_forecast(avy: &AvalancheData) -> UserPoint {
    make_tracker(Some(current_forecast(avy)))
}

#[tokio::test]
async fn test_forecast_past_end_date_is_expired() {
    let avy = load_avy();
    let result =
        nac::check_avy_geometry(&make_point(43.74, -110.80), &avy, after_tetons_end()).await;
    assert_eq!(result, nac::ZoneForecast::Expired);
}

#[tokio::test]
async fn test_forecast_not_started_is_valid() {
    // before the Tetons forecast starts at 2026-10-02 22:49 UTC
    let avy = load_avy();
    let before_start = Utc.with_ymd_and_hms(2026, 10, 2, 0, 0, 0).unwrap();
    let nac::ZoneForecast::Current(properties) =
        nac::check_avy_geometry(&make_point(43.74, -110.80), &avy, before_start).await
    else {
        panic!("a forecast that hasn't started should be valid");
    };
    assert_eq!(properties.name.as_deref(), Some(FORECAST_ZONE));
}

#[tokio::test]
async fn test_forecast_without_end_date_is_invalid() {
    let mut avy = load_avy();
    find_zone_mut(&mut avy, FORECAST_ZONE).properties.end_date = None;
    let result = nac::check_avy_geometry(&make_point(43.74, -110.80), &avy, test_now()).await;
    assert_eq!(result, nac::ZoneForecast::NoForecast);
}

#[tokio::test]
async fn test_forecast_with_danger_level_off_the_scale_is_invalid() {
    // not on the 1 to 5 danger scale, and not NAC's -1 (no rating)
    for level in [None, Some(0), Some(6), Some(-2)] {
        let mut avy = load_avy();
        find_zone_mut(&mut avy, FORECAST_ZONE)
            .properties
            .danger_level = level;
        let result = nac::check_avy_geometry(&make_point(43.74, -110.80), &avy, test_now()).await;
        assert_eq!(
            result,
            nac::ZoneForecast::NoForecast,
            "danger level {level:?}"
        );
    }
}

#[tokio::test]
async fn test_forecast_with_danger_rating_or_no_rating_is_valid() {
    // -1 is NAC's valid forecast with no rating, as every zone in the test data has
    for level in [-1, 1, 2, 3, 4, 5] {
        let mut avy = load_avy();
        find_zone_mut(&mut avy, FORECAST_ZONE)
            .properties
            .danger_level = Some(level);
        let result = nac::check_avy_geometry(&make_point(43.74, -110.80), &avy, test_now()).await;
        assert!(
            matches!(result, nac::ZoneForecast::Current(_)),
            "danger level {level} gave {result:?}"
        );
    }
}

#[tokio::test]
async fn test_avy_tracker_sends_rated_forecast_after_no_rating() {
    // the stored forecast has no rating (-1), so a rating is a new forecast
    let mut avy = load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    find_zone_mut(&mut avy, FORECAST_ZONE)
        .properties
        .danger_level = Some(3);
    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::NewForecast
    );
}

#[tokio::test]
async fn test_avy_tracker_notifies_danger_level_off_the_scale() {
    let mut avy = load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    find_zone_mut(&mut avy, FORECAST_ZONE)
        .properties
        .danger_level = Some(0);
    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::ForecastLost
    );
}

#[tokio::test]
async fn test_avy_tracker_notifies_expired_forecast_once() {
    let avy = load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &avy, after_tetons_end()).await,
        nac::AvyTrackerUpdate::ForecastLost
    );
    assert!(tracker.avy_lost.is_raised());
    // the last forecast sent is kept, so it isn't resent if it becomes valid again
    assert_eq!(tracker.avy, Some(current_forecast(&avy)));
    // later checks with no valid forecast send nothing
    assert_eq!(
        check_tracker(&mut tracker, &avy, after_tetons_end()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
}

#[tokio::test]
async fn test_avy_tracker_notifies_lost_forecast() {
    // the zone stops publishing a forecast
    let mut avy = load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    find_zone_mut(&mut avy, FORECAST_ZONE).properties.start_date = None;

    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::ForecastLost
    );
}

#[tokio::test]
async fn test_avy_tracker_notifies_lost_zone() {
    // the point is no longer in any zone, such as after a boundary change
    let mut avy = load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    avy.features
        .retain(|area| area.properties.name.as_deref() != Some(FORECAST_ZONE));

    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::ForecastLost
    );
}

#[tokio::test]
async fn test_avy_tracker_notifies_off_season_loss() {
    // the center enters its off season mid tracker, which gets its own notice
    let mut avy = load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    find_zone_mut(&mut avy, FORECAST_ZONE).properties.off_season = Some(true);

    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::OffSeason
    );
    assert!(tracker.avy_lost.is_raised());
    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
}

#[tokio::test]
async fn test_avy_tracker_restores_same_forecast_silently() {
    // the zone is missing from one response, then returns with the same forecast
    let avy = load_avy();
    let mut missing = avy.clone();
    missing
        .features
        .retain(|area| area.properties.name.as_deref() != Some(FORECAST_ZONE));
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &missing, test_now()).await,
        nac::AvyTrackerUpdate::ForecastLost
    );
    // the user already has this forecast, so it isn't sent again
    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::ForecastRestored
    );
    assert!(!tracker.avy_lost.is_raised());
    assert_eq!(tracker.avy, Some(current_forecast(&avy)));
    // once restored, a new loss is notified again
    assert_eq!(
        check_tracker(&mut tracker, &missing, test_now()).await,
        nac::AvyTrackerUpdate::ForecastLost
    );
}

#[tokio::test]
async fn test_avy_tracker_sends_changed_forecast_after_loss() {
    let avy = load_avy();
    let mut changed = avy.clone();
    find_zone_mut(&mut changed, FORECAST_ZONE)
        .properties
        .travel_advice = Some("new advice".to_string());
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &avy, after_tetons_end()).await,
        nac::AvyTrackerUpdate::ForecastLost
    );
    assert_eq!(
        check_tracker(&mut tracker, &changed, test_now()).await,
        nac::AvyTrackerUpdate::NewForecast
    );
    assert!(!tracker.avy_lost.is_raised());
    assert_eq!(
        tracker
            .avy
            .and_then(|properties| properties.travel_advice)
            .as_deref(),
        Some("new advice")
    );
}

#[tokio::test]
async fn test_avy_tracker_without_forecast_sends_nothing_on_loss() {
    // the creation reply already said there was no forecast
    let avy = load_avy();
    let mut tracker = make_tracker(None);

    assert_eq!(
        check_tracker(&mut tracker, &avy, after_tetons_end()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
    assert!(!tracker.avy_lost.is_raised());
}

#[tokio::test]
async fn test_avy_tracker_stale_data_is_a_loss() {
    // the forecast is still within its end_date, but stale data can't confirm it
    let avy = load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::DataStale
    );
    assert!(tracker.avy_lost.is_raised());
    assert_eq!(tracker.avy, Some(current_forecast(&avy)));
    // the loss is sent once, while the data stays stale
    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
}

#[tokio::test]
async fn test_avy_tracker_restores_same_forecast_after_stale_data() {
    // fresh data with the forecast the user already has sends nothing
    let avy = load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::DataStale
    );
    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::ForecastRestored
    );
    assert!(!tracker.avy_lost.is_raised());
}

#[tokio::test]
async fn test_avy_tracker_without_forecast_sends_nothing_on_stale_data() {
    let avy = load_avy();
    let mut tracker = make_tracker(None);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
    assert!(tracker.avy.is_none());
}

#[tokio::test]
async fn test_avy_tracker_sends_off_season_after_stale_loss() {
    // the center enters its off season while the data is stale
    let avy = load_avy();
    let mut off_season = avy.clone();
    find_zone_mut(&mut off_season, FORECAST_ZONE)
        .properties
        .off_season = Some(true);
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::DataStale
    );
    assert_eq!(
        check_tracker(&mut tracker, &off_season, test_now()).await,
        nac::AvyTrackerUpdate::OffSeason
    );
    assert!(tracker.avy_lost.is_raised());
    // sent once, including when the data goes stale again
    assert_eq!(
        check_tracker(&mut tracker, &off_season, test_now()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
    assert_eq!(
        check_tracker_stale(&mut tracker, &off_season, test_now()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
}

#[tokio::test]
async fn test_avy_tracker_sends_nothing_for_other_losses_after_stale_loss() {
    // fresh data with an expired forecast: the user was already told there's no forecast
    let avy = load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::DataStale
    );
    assert_eq!(
        check_tracker(&mut tracker, &avy, after_tetons_end()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
}

#[tokio::test]
async fn test_avy_tracker_sends_off_season_after_other_losses() {
    // the forecast expired and the user was told, then the center went off season
    let avy = load_avy();
    let mut off_season = avy.clone();
    find_zone_mut(&mut off_season, FORECAST_ZONE)
        .properties
        .off_season = Some(true);
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &avy, after_tetons_end()).await,
        nac::AvyTrackerUpdate::ForecastLost
    );
    assert_eq!(
        check_tracker(&mut tracker, &off_season, after_tetons_end()).await,
        nac::AvyTrackerUpdate::OffSeason
    );
    assert_eq!(
        check_tracker(&mut tracker, &off_season, after_tetons_end()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
}

#[tokio::test]
async fn test_avy_tracker_sends_nothing_for_other_losses_after_off_season() {
    // the off season notice already told the user there's no forecast
    let avy = load_avy();
    let mut off_season = avy.clone();
    find_zone_mut(&mut off_season, FORECAST_ZONE)
        .properties
        .off_season = Some(true);
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &off_season, test_now()).await,
        nac::AvyTrackerUpdate::OffSeason
    );
    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
    assert_eq!(
        check_tracker(&mut tracker, &avy, after_tetons_end()).await,
        nac::AvyTrackerUpdate::Unchanged
    );
}

#[tokio::test]
async fn test_avy_tracker_sends_off_season_again_after_valid_forecast() {
    // a new season's forecast resets both notices
    let avy = load_avy();
    let mut off_season = avy.clone();
    find_zone_mut(&mut off_season, FORECAST_ZONE)
        .properties
        .off_season = Some(true);
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &off_season, test_now()).await,
        nac::AvyTrackerUpdate::OffSeason
    );
    assert_eq!(
        check_tracker(&mut tracker, &avy, test_now()).await,
        nac::AvyTrackerUpdate::ForecastRestored
    );
    assert_eq!(
        check_tracker(&mut tracker, &off_season, test_now()).await,
        nac::AvyTrackerUpdate::OffSeason
    );
}

#[test]
fn test_tracker_without_avy_lost_field_reads_as_not_raised() {
    // trackers stored before avy_lost was added still load
    let tracker = make_tracker(None);
    let mut stored = bson::to_document(&tracker).expect("tracker should serialize");
    stored
        .remove("avy_lost")
        .expect("avy_lost should be stored");
    stored
        .remove("avy_off_season")
        .expect("avy_off_season should be stored");
    let read: UserPoint = bson::from_document(stored).expect("old tracker should deserialize");
    assert!(!read.avy_lost.is_raised());
    assert!(!read.avy_off_season.is_raised());
}

// runs build_avy_map on a json array of raw NAC zones with the given geometries,
// returning the zones and the records of zones that failed. every zone must
// deserialize, so a mistake in a test zone fails here instead of being skipped
fn build_zones(geometries: &[serde_json::Value]) -> (Vec<AvyArea>, Vec<InvalidAvyArea>) {
    let features: Vec<serde_json::Value> = geometries
        .iter()
        .enumerate()
        .map(|(i, geometry)| {
            serde_json::json!({
                "id": i,
                "properties": { "name": format!("Zone {i}"), "center": "Test Center" },
                "geometry": geometry,
            })
        })
        .collect();
    let raw: RawFeatures = serde_json::from_value(serde_json::json!({ "features": features }))
        .expect("test zones should be a feature collection");
    let (parsed, skipped): (Vec<AvyArea>, _) = raw.parse("NAC test zones");
    assert!(
        skipped.features.is_empty(),
        "every test zone should deserialize, check the logged skipped features"
    );
    assert_eq!(parsed.len(), geometries.len());
    nac::build_avy_map(parsed)
}

// a closed square ring, the smallest valid exterior ring
fn square() -> serde_json::Value {
    serde_json::json!([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]])
}

// every zone in the saved response deserializes, so a model change a real NAC
// response doesn't match fails here instead of quietly skipping zones. every
// AvyProperties field is an Option, so a renamed or mistyped field would read
// as None rather than fail: each field is also compared with the raw JSON
#[test]
fn nac_test_data_deserializes() {
    let raw: RawFeatures =
        serde_json::from_str(TEST_DATA).expect("test data should be a geojson feature collection");
    let (zones, skipped): (Vec<AvyArea>, _) = raw.parse("NAC test data");
    assert!(
        skipped.features.is_empty(),
        "every zone should deserialize: {:?}",
        skipped
            .features
            .iter()
            .map(|feature| &feature.error)
            .collect::<Vec<_>>()
    );
    assert!(skipped.total > 0, "test data should have zones");
    assert_eq!(zones.len(), skipped.total);

    let json: serde_json::Value = serde_json::from_str(TEST_DATA).unwrap();
    let features = json["features"].as_array().unwrap();
    for (zone, feature) in zones.iter().zip(features) {
        let props = &zone.properties;
        let raw = &feature["properties"];
        for (field, parsed) in [
            ("name", props.name.is_some()),
            ("center", props.center.is_some()),
            ("center_link", props.center_link.is_some()),
            ("off_season", props.off_season.is_some()),
            ("travel_advice", props.travel_advice.is_some()),
            ("danger", props.danger.is_some()),
            ("danger_level", props.danger_level.is_some()),
            ("link", props.link.is_some()),
            ("timezone", props.timezone.is_some()),
            ("start_date", props.start_date.is_some()),
            ("end_date", props.end_date.is_some()),
        ] {
            assert_eq!(
                parsed,
                !raw[field].is_null(),
                "{field} of {:?} should be read when the response has it",
                props.name
            );
        }
    }
}

#[test]
fn test_build_avy_map_test_data_has_no_invalid_zones() {
    let raw: RawFeatures =
        serde_json::from_str(TEST_DATA).expect("test data should be a geojson feature collection");
    let (_, invalid) = nac::build_avy_map(raw.parse("NAC test data").0);
    assert!(
        invalid.is_empty(),
        "unexpected invalid zones: {:?}",
        invalid
            .iter()
            .map(|record| &record.zone_name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_build_avy_map_converts_valid_polygon() {
    let (zones, invalid) =
        build_zones(&[serde_json::json!({ "type": "Polygon", "coordinates": [square()] })]);
    assert!(invalid.is_empty());
    assert!(matches!(zones[0].geometry, GeometryTypes::Polygon(_)));
}

#[test]
fn test_build_avy_map_records_unsupported_geometry() {
    let (zones, invalid) =
        build_zones(&[serde_json::json!({ "type": "Point", "coordinates": [0.0, 0.0] })]);

    assert_eq!(invalid.len(), 1);
    assert_eq!(
        invalid[0].reason,
        GeometryErrorReason::UnsupportedGeometryType
    );
    assert_eq!(invalid[0].zone_name.as_deref(), Some("Zone 0"));
    assert_eq!(invalid[0].center.as_deref(), Some("Test Center"));
    // the zone is kept with its geometry unconverted, so it never matches
    assert!(matches!(zones[0].geometry, GeometryTypes::Geojson(_)));
    assert!(matches!(
        invalid[0].area.geometry,
        GeometryTypes::Geojson(_)
    ));
}

#[test]
fn test_build_avy_map_records_malformed_coordinates_instead_of_panicking() {
    // geojson's conversion panics on a position with one coordinate
    let ring = serde_json::json!([[0.0, 0.0], [1.0], [1.0, 1.0], [0.0, 0.0]]);
    let (zones, invalid) = build_zones(&[
        serde_json::json!({ "type": "Polygon", "coordinates": [ring] }),
        serde_json::json!({ "type": "MultiPolygon", "coordinates": [[square()], [ring]] }),
    ]);

    assert_eq!(invalid.len(), 2);
    assert!(
        invalid
            .iter()
            .all(|record| record.reason == GeometryErrorReason::MalformedCoordinates)
    );
    assert!(
        zones
            .iter()
            .all(|zone| matches!(zone.geometry, GeometryTypes::Geojson(_)))
    );
}

#[test]
fn test_build_avy_map_records_degenerate_geometry() {
    // these convert without error but could never contain a point
    let short_ring = serde_json::json!([[0.0, 0.0], [1.0, 0.0], [0.0, 0.0]]);
    let (_, invalid) = build_zones(&[
        serde_json::json!({ "type": "Polygon", "coordinates": [] }),
        serde_json::json!({ "type": "Polygon", "coordinates": [[]] }),
        serde_json::json!({ "type": "Polygon", "coordinates": [short_ring] }),
        serde_json::json!({ "type": "MultiPolygon", "coordinates": [] }),
    ]);

    assert_eq!(invalid.len(), 4);
    assert!(
        invalid
            .iter()
            .all(|record| record.reason == GeometryErrorReason::DegenerateGeometry)
    );
}

#[test]
fn test_invalid_avy_area_round_trips_through_bson() {
    let (_, invalid) =
        build_zones(&[serde_json::json!({ "type": "Point", "coordinates": [0.0, 0.0] })]);

    let stored = bson::to_document(&invalid[0]).expect("invalid zone should serialize");
    assert!(
        stored.get_datetime("first_seen").is_ok(),
        "first_seen should be a bson datetime"
    );
    let read: InvalidAvyArea =
        bson::from_document(stored).expect("invalid zone should deserialize");
    assert_eq!(read.zone_name.as_deref(), Some("Zone 0"));
    assert_eq!(read.reason, GeometryErrorReason::UnsupportedGeometryType);
}
