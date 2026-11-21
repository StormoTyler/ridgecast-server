// tests for tracker_checks::check_avy_tracker against the saved NAC map-layer
// response that nac_tests loads
use chrono::{DateTime, Utc};

use crate::messaging;
use crate::models::{
    AvalancheData, AvyProperties, DateTimeTypes, ForecastVars, Snapshot, UserPoint,
};
use crate::tests::nac_tests;
use crate::tracking::tracker_checks;

// an avalanche tracker inside the Tetons zone with the given stored forecast
fn make_tracker(avy: Option<AvyProperties>) -> UserPoint {
    UserPoint {
        id: None,
        user: messaging::PhoneNumber::new("+15415550100").unwrap(),
        reply_from: messaging::PhoneNumber::new("+18005550100").unwrap(),
        tracker_name: Some("test".to_string()),
        forecast_vars: ForecastVars {
            track_avy: true,
            ..ForecastVars::default()
        },
        point: nac_tests::make_point(43.74, -110.80),
        avy,
        avy_lost: Default::default(),
        avy_off_season: Default::default(),
        time_added: chrono::Utc::now(),
        valid_duration: 24,
        expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
        next_daily_forecast: None,
    }
}

// runs check_avy_tracker on fresh avalanche data
fn check_tracker(
    tracker: &mut UserPoint,
    avy: &AvalancheData,
    now: DateTime<Utc>,
) -> tracker_checks::AvyTrackerUpdate {
    tracker_checks::check_avy_tracker(
        tracker,
        &Snapshot {
            data: avy.clone(),
            stale: false,
        },
        now,
    )
}

// runs check_avy_tracker on stale avalanche data
fn check_tracker_stale(
    tracker: &mut UserPoint,
    avy: &AvalancheData,
    now: DateTime<Utc>,
) -> tracker_checks::AvyTrackerUpdate {
    tracker_checks::check_avy_tracker(
        tracker,
        &Snapshot {
            data: avy.clone(),
            stale: true,
        },
        now,
    )
}

#[test]
fn test_avy_tracker_with_same_forecast() {
    // a forecast identical to the stored one is not new
    let avy = nac_tests::load_avy();
    let mut tracker = make_tracker(Some(nac_tests::current_forecast(&avy)));

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
    assert_eq!(tracker.avy, Some(nac_tests::current_forecast(&avy)));
}

#[test]
fn test_avy_tracker_with_changed_field() {
    // any changed field counts as a new forecast, even with the same start_date
    let avy = nac_tests::load_avy();
    let current = nac_tests::current_forecast(&avy);
    let mut stored = current.clone();
    stored.travel_advice = Some("stored advice".to_string());
    let mut tracker = make_tracker(Some(stored));

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::NewForecast
    );
    let stored = tracker.avy.expect("forecast should be stored");
    assert_eq!(stored.start_date, current.start_date);
    assert_eq!(stored.travel_advice, current.travel_advice);
}

#[test]
fn test_avy_tracker_with_earlier_start_date() {
    // the stored forecast started a day before the current one
    let avy = nac_tests::load_avy();
    let current = nac_tests::current_forecast(&avy);
    let Some(DateTimeTypes::Aware(start)) = current.start_date else {
        panic!("test zone start_date should be converted to local time");
    };
    let mut stored = current.clone();
    stored.start_date = Some(DateTimeTypes::Aware(start - chrono::Duration::days(1)));
    stored.travel_advice = Some("stored advice".to_string());
    let mut tracker = make_tracker(Some(stored));

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::NewForecast
    );
    // the whole forecast is replaced, not just its start_date
    assert_eq!(tracker.avy, Some(current));
}

#[test]
fn test_avy_tracker_with_no_stored_forecast() {
    // a tracker without a stored forecast still checks for one,
    // and the first forecast found counts as new
    let avy = nac_tests::load_avy();
    let mut tracker = make_tracker(None);

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::NewForecast
    );
    assert_eq!(tracker.avy, Some(nac_tests::current_forecast(&avy)));
}

#[test]
fn test_avy_tracker_in_off_season_zone() {
    // Chugach State Park is off season, so there is no forecast to store
    let avy = nac_tests::load_avy();
    let mut tracker = make_tracker(None);
    tracker.point = nac_tests::make_point(61.13, -149.53);

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
    assert!(tracker.avy.is_none());
    // there was no forecast to lose, so no loss is recorded
    assert!(!tracker.avy_lost.is_raised());
}

// a tracker in the Tetons zone that was already sent its current forecast
fn tracker_with_current_forecast(avy: &AvalancheData) -> UserPoint {
    make_tracker(Some(nac_tests::current_forecast(avy)))
}

#[test]
fn test_avy_tracker_sends_rated_forecast_after_no_rating() {
    // the stored forecast has no rating (-1), so a rating is a new forecast
    let mut avy = nac_tests::load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    nac_tests::find_zone_mut(&mut avy, nac_tests::FORECAST_ZONE)
        .properties
        .danger_level = Some(3);
    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::NewForecast
    );
}

#[test]
fn test_avy_tracker_notifies_danger_level_off_the_scale() {
    let mut avy = nac_tests::load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    nac_tests::find_zone_mut(&mut avy, nac_tests::FORECAST_ZONE)
        .properties
        .danger_level = Some(0);
    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::ForecastLost
    );
}

#[test]
fn test_avy_tracker_notifies_expired_forecast_once() {
    let avy = nac_tests::load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::after_tetons_end()),
        tracker_checks::AvyTrackerUpdate::ForecastLost
    );
    assert!(tracker.avy_lost.is_raised());
    // the last forecast sent is kept, so it isn't resent if it becomes valid again
    assert_eq!(tracker.avy, Some(nac_tests::current_forecast(&avy)));
    // later checks with no valid forecast send nothing
    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::after_tetons_end()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
}

#[test]
fn test_avy_tracker_notifies_lost_forecast() {
    // the zone stops publishing a forecast
    let mut avy = nac_tests::load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    nac_tests::find_zone_mut(&mut avy, nac_tests::FORECAST_ZONE)
        .properties
        .start_date = None;

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::ForecastLost
    );
}

#[test]
fn test_avy_tracker_notifies_lost_zone() {
    // the point is no longer in any zone, such as after a boundary change
    let mut avy = nac_tests::load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    avy.features
        .retain(|area| area.properties.name.as_deref() != Some(nac_tests::FORECAST_ZONE));

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::ForecastLost
    );
}

#[test]
fn test_avy_tracker_notifies_off_season_loss() {
    // the center enters its off season mid tracker, which gets its own notice
    let mut avy = nac_tests::load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);
    nac_tests::find_zone_mut(&mut avy, nac_tests::FORECAST_ZONE)
        .properties
        .off_season = Some(true);

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::OffSeason
    );
    assert!(tracker.avy_lost.is_raised());
    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
}

#[test]
fn test_avy_tracker_restores_same_forecast_silently() {
    // the zone is missing from one response, then returns with the same forecast
    let avy = nac_tests::load_avy();
    let mut missing = avy.clone();
    missing
        .features
        .retain(|area| area.properties.name.as_deref() != Some(nac_tests::FORECAST_ZONE));
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &missing, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::ForecastLost
    );
    // the user already has this forecast, so it isn't sent again
    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::ForecastRestored
    );
    assert!(!tracker.avy_lost.is_raised());
    assert_eq!(tracker.avy, Some(nac_tests::current_forecast(&avy)));
    // once restored, a new loss is notified again
    assert_eq!(
        check_tracker(&mut tracker, &missing, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::ForecastLost
    );
}

#[test]
fn test_avy_tracker_sends_changed_forecast_after_loss() {
    let avy = nac_tests::load_avy();
    let mut changed = avy.clone();
    nac_tests::find_zone_mut(&mut changed, nac_tests::FORECAST_ZONE)
        .properties
        .travel_advice = Some("new advice".to_string());
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::after_tetons_end()),
        tracker_checks::AvyTrackerUpdate::ForecastLost
    );
    assert_eq!(
        check_tracker(&mut tracker, &changed, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::NewForecast
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

#[test]
fn test_avy_tracker_without_forecast_sends_nothing_on_loss() {
    // the creation reply already said there was no forecast
    let avy = nac_tests::load_avy();
    let mut tracker = make_tracker(None);

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::after_tetons_end()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
    assert!(!tracker.avy_lost.is_raised());
}

#[test]
fn test_avy_tracker_stale_data_is_a_loss() {
    // the forecast is still within its end_date, but stale data can't confirm it
    let avy = nac_tests::load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::DataStale
    );
    assert!(tracker.avy_lost.is_raised());
    assert_eq!(tracker.avy, Some(nac_tests::current_forecast(&avy)));
    // the loss is sent once, while the data stays stale
    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
}

#[test]
fn test_avy_tracker_restores_same_forecast_after_stale_data() {
    // fresh data with the forecast the user already has sends nothing
    let avy = nac_tests::load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::DataStale
    );
    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::ForecastRestored
    );
    assert!(!tracker.avy_lost.is_raised());
}

#[test]
fn test_avy_tracker_without_forecast_sends_nothing_on_stale_data() {
    let avy = nac_tests::load_avy();
    let mut tracker = make_tracker(None);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
    assert!(tracker.avy.is_none());
}

#[test]
fn test_avy_tracker_sends_off_season_after_stale_loss() {
    // the center enters its off season while the data is stale
    let avy = nac_tests::load_avy();
    let mut off_season = avy.clone();
    nac_tests::find_zone_mut(&mut off_season, nac_tests::FORECAST_ZONE)
        .properties
        .off_season = Some(true);
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::DataStale
    );
    assert_eq!(
        check_tracker(&mut tracker, &off_season, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::OffSeason
    );
    assert!(tracker.avy_lost.is_raised());
    // sent once, including when the data goes stale again
    assert_eq!(
        check_tracker(&mut tracker, &off_season, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
    assert_eq!(
        check_tracker_stale(&mut tracker, &off_season, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
}

#[test]
fn test_avy_tracker_sends_nothing_for_other_losses_after_stale_loss() {
    // fresh data with an expired forecast: the user was already told there's no forecast
    let avy = nac_tests::load_avy();
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::DataStale
    );
    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::after_tetons_end()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
}

#[test]
fn test_avy_tracker_sends_off_season_after_other_losses() {
    // the forecast expired and the user was told, then the center went off season
    let avy = nac_tests::load_avy();
    let mut off_season = avy.clone();
    nac_tests::find_zone_mut(&mut off_season, nac_tests::FORECAST_ZONE)
        .properties
        .off_season = Some(true);
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::after_tetons_end()),
        tracker_checks::AvyTrackerUpdate::ForecastLost
    );
    assert_eq!(
        check_tracker(&mut tracker, &off_season, nac_tests::after_tetons_end()),
        tracker_checks::AvyTrackerUpdate::OffSeason
    );
    assert_eq!(
        check_tracker(&mut tracker, &off_season, nac_tests::after_tetons_end()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
}

#[test]
fn test_avy_tracker_sends_nothing_for_other_losses_after_off_season() {
    // the off season notice already told the user there's no forecast
    let avy = nac_tests::load_avy();
    let mut off_season = avy.clone();
    nac_tests::find_zone_mut(&mut off_season, nac_tests::FORECAST_ZONE)
        .properties
        .off_season = Some(true);
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &off_season, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::OffSeason
    );
    assert_eq!(
        check_tracker_stale(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::after_tetons_end()),
        tracker_checks::AvyTrackerUpdate::Unchanged
    );
}

#[test]
fn test_avy_tracker_sends_off_season_again_after_valid_forecast() {
    // a new season's forecast resets both notices
    let avy = nac_tests::load_avy();
    let mut off_season = avy.clone();
    nac_tests::find_zone_mut(&mut off_season, nac_tests::FORECAST_ZONE)
        .properties
        .off_season = Some(true);
    let mut tracker = tracker_with_current_forecast(&avy);

    assert_eq!(
        check_tracker(&mut tracker, &off_season, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::OffSeason
    );
    assert_eq!(
        check_tracker(&mut tracker, &avy, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::ForecastRestored
    );
    assert_eq!(
        check_tracker(&mut tracker, &off_season, nac_tests::test_now()),
        tracker_checks::AvyTrackerUpdate::OffSeason
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
