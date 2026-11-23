// tests for tracker_checks::check_avy_tracker and check_alert_tracker against the
// saved NAC map-layer and NWS alerts responses that nac_tests and nws_tests load
use chrono::{DateTime, Utc};

use crate::messaging;
use crate::models::{
    AlertsSorted, AvalancheData, AvyProperties, DateTimeTypes, ForecastVars, Point, Snapshot,
    UserPoint,
};
use crate::tests::{nac_tests, nws_tests};
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
        alerts_sent: Vec::new(),
        alerts_uncheckable: Default::default(),
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

// ---- alert tracker ----

// Bozeman, MT: only the Red Flag Warning, matched by its SAME code
fn bozeman() -> Point {
    nws_tests::make_point(45.68, -111.04, "030031")
}

// Salt Lake City, UT: no active alerts in the test data
fn salt_lake_city() -> Point {
    nws_tests::make_point(40.76, -111.89, "049035")
}

// an alert tracker at point that was sent nothing yet
fn alert_tracker(point: Point) -> UserPoint {
    let mut tracker = make_tracker(None);
    tracker.forecast_vars = ForecastVars {
        track_alerts: true,
        ..ForecastVars::default()
    };
    tracker.point = point;
    tracker
}

// the test alerts as a snapshot, fresh or stale
fn snapshot(stale: bool) -> Snapshot<AlertsSorted> {
    Snapshot {
        data: nws_tests::load_alerts(),
        stale,
    }
}

fn check(
    tracker: &mut UserPoint,
    snapshot: &Snapshot<AlertsSorted>,
) -> tracker_checks::AlertTrackerUpdate {
    tracker_checks::check_alert_tracker(tracker, Some(snapshot))
}

// the ids of a NewAlerts update's alerts and its stale flag, or a panic naming the update
fn new_alerts(update: tracker_checks::AlertTrackerUpdate) -> (Vec<String>, bool) {
    match update {
        tracker_checks::AlertTrackerUpdate::NewAlerts { alerts, stale } => (
            nws_tests::ids(&alerts)
                .into_iter()
                .map(str::to_string)
                .collect(),
            stale,
        ),
        other => panic!("expected NewAlerts, got {other:?}"),
    }
}

#[test]
fn alert_tracker_sends_an_active_alert_once() {
    let fresh = snapshot(false);
    let mut tracker = alert_tracker(bozeman());

    let (sent, marked_stale) = new_alerts(check(&mut tracker, &fresh));
    assert_eq!(sent, vec![nws_tests::RED_FLAG_WARNING_ID]);
    assert!(!marked_stale);
    assert_eq!(tracker.alerts_sent, vec![nws_tests::RED_FLAG_WARNING_ID]);

    assert!(matches!(
        check(&mut tracker, &fresh),
        tracker_checks::AlertTrackerUpdate::Unchanged
    ));
}

// the reply that created the tracker already sent its alerts, and recorded them
#[test]
fn alert_tracker_does_not_resend_the_replys_alerts() {
    let fresh = snapshot(false);
    let mut tracker = alert_tracker(bozeman());
    tracker.alerts_sent = vec![nws_tests::RED_FLAG_WARNING_ID.to_string()];

    assert!(matches!(
        check(&mut tracker, &fresh),
        tracker_checks::AlertTrackerUpdate::Unchanged
    ));
}

#[test]
fn alert_tracker_with_no_active_alerts_sends_nothing() {
    let fresh = snapshot(false);
    let mut tracker = alert_tracker(salt_lake_city());

    assert!(matches!(
        check(&mut tracker, &fresh),
        tracker_checks::AlertTrackerUpdate::Unchanged
    ));
    assert!(tracker.alerts_sent.is_empty());
}

// an id that no alert in the test data has, standing for an alert the tracker
// was sent in an earlier check that has since ended
const ENDED_ALERT_ID: &str = "https://api.weather.gov/alerts/ended";

// the ids of alerts that ended are dropped, so the stored list doesn't grow,
// and the ids of alerts still active are kept
#[test]
fn alert_tracker_drops_ids_of_ended_alerts() {
    let fresh = snapshot(false);
    let mut tracker = alert_tracker(bozeman());
    tracker.alerts_sent = vec![
        ENDED_ALERT_ID.to_string(),
        nws_tests::RED_FLAG_WARNING_ID.to_string(),
    ];

    check(&mut tracker, &fresh);

    assert!(
        !tracker.alerts_sent.iter().any(|id| id == ENDED_ALERT_ID),
        "the ended alert's id should be dropped"
    );
    assert_eq!(tracker.alerts_sent, vec![nws_tests::RED_FLAG_WARNING_ID]);
}

// an alert without an id can't be recorded, so it is sent on every check
#[test]
fn alert_tracker_sends_an_alert_without_an_id_every_check() {
    let mut fresh = snapshot(false);
    for alert in fresh.data.by_same_code.values_mut().flatten() {
        if alert.id.as_deref() == Some(nws_tests::RED_FLAG_WARNING_ID) {
            alert.id = None;
        }
    }
    let mut tracker = alert_tracker(bozeman());

    for _ in 0..2 {
        match check(&mut tracker, &fresh) {
            tracker_checks::AlertTrackerUpdate::NewAlerts { alerts, stale } => {
                assert_eq!(alerts.len(), 1);
                assert!(alerts[0].id.is_none());
                assert!(!stale);
            }
            other => panic!("expected NewAlerts, got {other:?}"),
        }
    }
    assert!(tracker.alerts_sent.is_empty());
}

// a stale snapshot can still be newer than the last check's, so its alerts the
// user hasn't been sent are sent, marked stale
#[test]
fn alert_tracker_sends_unsent_alerts_from_stale_data() {
    let stale = snapshot(true);
    let mut tracker = alert_tracker(bozeman());

    let (sent, marked_stale) = new_alerts(check(&mut tracker, &stale));

    assert_eq!(sent, vec![nws_tests::RED_FLAG_WARNING_ID]);
    assert!(marked_stale);
    assert_eq!(tracker.alerts_sent, vec![nws_tests::RED_FLAG_WARNING_ID]);
}

// stale data with nothing new means the alerts could not be checked, notified once
#[test]
fn alert_tracker_notifies_stale_alerts_once() {
    let fresh = snapshot(false);
    let stale = snapshot(true);
    let mut tracker = alert_tracker(bozeman());
    check(&mut tracker, &fresh);

    assert!(matches!(
        check(&mut tracker, &stale),
        tracker_checks::AlertTrackerUpdate::Uncheckable
    ));
    assert!(tracker.alerts_uncheckable.is_raised());
    assert!(matches!(
        check(&mut tracker, &stale),
        tracker_checks::AlertTrackerUpdate::Unchanged
    ));
}

#[test]
fn alert_tracker_notifies_unavailable_alerts_once() {
    let mut tracker = alert_tracker(bozeman());
    tracker.alerts_sent = vec![nws_tests::RED_FLAG_WARNING_ID.to_string()];

    assert!(matches!(
        tracker_checks::check_alert_tracker(&mut tracker, None),
        tracker_checks::AlertTrackerUpdate::Uncheckable
    ));
    assert!(matches!(
        tracker_checks::check_alert_tracker(&mut tracker, None),
        tracker_checks::AlertTrackerUpdate::Unchanged
    ));
    // nothing was checked, so nothing the user was sent is forgotten
    assert_eq!(tracker.alerts_sent, vec![nws_tests::RED_FLAG_WARNING_ID]);
}

// fresh alerts end the outage silently, so the next outage is notified again
#[test]
fn alert_tracker_notifies_each_outage() {
    let fresh = snapshot(false);
    let stale = snapshot(true);
    let mut tracker = alert_tracker(bozeman());
    check(&mut tracker, &fresh);
    tracker_checks::check_alert_tracker(&mut tracker, None);

    assert!(matches!(
        check(&mut tracker, &fresh),
        tracker_checks::AlertTrackerUpdate::CheckableAgain
    ));
    assert!(!tracker.alerts_uncheckable.is_raised());
    assert!(matches!(
        check(&mut tracker, &stale),
        tracker_checks::AlertTrackerUpdate::Uncheckable
    ));
}

// new alerts after an outage are sent, and end the outage
#[test]
fn alert_tracker_sends_new_alerts_after_an_outage() {
    let fresh = snapshot(false);
    let mut tracker = alert_tracker(bozeman());
    tracker_checks::check_alert_tracker(&mut tracker, None);

    let (sent, marked_stale) = new_alerts(check(&mut tracker, &fresh));

    assert_eq!(sent, vec![nws_tests::RED_FLAG_WARNING_ID]);
    assert!(!marked_stale);
    assert!(!tracker.alerts_uncheckable.is_raised());
}

// stale alerts don't end the outage, so the notice isn't sent again
#[test]
fn alert_tracker_stays_uncheckable_on_stale_new_alerts() {
    let stale = snapshot(true);
    let mut tracker = alert_tracker(bozeman());
    tracker_checks::check_alert_tracker(&mut tracker, None);

    let (_, marked_stale) = new_alerts(check(&mut tracker, &stale));

    assert!(marked_stale);
    assert!(tracker.alerts_uncheckable.is_raised());
    assert!(matches!(
        check(&mut tracker, &stale),
        tracker_checks::AlertTrackerUpdate::Unchanged
    ));
}
