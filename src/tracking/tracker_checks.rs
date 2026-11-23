// the tracker checks: what a tracker is sent when the scheduler checks it, and
// how its stored state changes, for the avalanche and alert checks. matching a
// point against the data stays in upstream

use chrono::{DateTime, Utc};

// local modules
use crate::models::{Alert, AlertsSorted, AvalancheData, Snapshot, UserPoint};
use crate::upstream::{nac, nws};

// what check_avy_tracker found, for the scheduler to act on
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AvyTrackerUpdate {
    // nothing to send or save
    Unchanged,
    // UserPoint.avy was replaced with a new forecast, send it and save the tracker
    NewForecast,
    // the stored forecast is no longer valid, send "Avy: no current forecast" and save the tracker
    ForecastLost,
    // the stored forecast was lost because the center entered its off season,
    // send the off season notice and save the tracker
    OffSeason,
    // the avalanche data is stale, so the stored forecast can't be confirmed and
    // counts as lost. send "Avy: no current forecast" with the stale notice and save the tracker
    DataStale,
    // the stored forecast is valid again after a loss. the user already has it,
    // so nothing is sent, but the tracker is saved so the next loss is notified
    ForecastRestored,
}

// checks a tracker's avalanche zone against the current data.
// UserPoint.avy keeps the last valid forecast the user was sent, and
// UserPoint.avy_lost records that they were told it was lost, so a loss is
// notified once and a forecast that returns unchanged is not sent again.
// a stale snapshot counts as losing the forecast
// unused until the scheduler calls it, remove the allow then
#[allow(dead_code)]
pub fn check_avy_tracker(
    user_point: &mut UserPoint,
    snapshot: &Snapshot<AvalancheData>,
    now: DateTime<Utc>,
) -> AvyTrackerUpdate {
    if snapshot.stale {
        return record_avy_loss(user_point, AvyTrackerUpdate::DataStale);
    }
    match nac::check_avy_geometry(&user_point.point, &snapshot.data, now) {
        nac::ZoneForecast::Current(current) => {
            let was_lost = user_point.avy_lost.is_raised();
            user_point.avy_lost.clear();
            user_point.avy_off_season.clear();
            // any changed field counts as a new forecast, since avalanche
            // information is safety critical it is better to over report
            if user_point.avy.as_ref() != Some(current) {
                user_point.avy = Some(current.clone());
                AvyTrackerUpdate::NewForecast
            } else if was_lost {
                AvyTrackerUpdate::ForecastRestored
            } else {
                AvyTrackerUpdate::Unchanged
            }
        }
        // sent once, even after another kind of loss was notified, since the
        // center may not publish another forecast before the tracker ends
        nac::ZoneForecast::OffSeason => {
            if user_point.avy.is_none() || !user_point.avy_off_season.raise() {
                return AvyTrackerUpdate::Unchanged;
            }
            user_point.avy_lost.raise();
            AvyTrackerUpdate::OffSeason
        }
        nac::ZoneForecast::NoZone | nac::ZoneForecast::NoForecast | nac::ZoneForecast::Expired => {
            record_avy_loss(user_point, AvyTrackerUpdate::ForecastLost)
        }
    }
}

// the user is only told about a loss when they had a valid forecast,
// and only once until it is valid again
fn record_avy_loss(user_point: &mut UserPoint, loss: AvyTrackerUpdate) -> AvyTrackerUpdate {
    if user_point.avy.is_none() || !user_point.avy_lost.raise() {
        return AvyTrackerUpdate::Unchanged;
    }
    loss
}

// what check_alert_tracker found, for the scheduler to act on
// unused until the scheduler sends the alerts, remove the allow then
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum AlertTrackerUpdate {
    // nothing to send. the tracker may have dropped ids of alerts that ended,
    // which doesn't need saving, since an ended alert's id doesn't come back
    Unchanged,
    // alerts the user hasn't been sent, now recorded in UserPoint.alerts_sent.
    // send them, with the stale notice first when stale is set, and save the tracker
    NewAlerts { alerts: Vec<Alert>, stale: bool },
    // the alerts could not be checked: they could not be loaded, or they are
    // stale with nothing new. send the could not be checked notice and save the tracker
    Uncheckable,
    // fresh alerts after the could not be checked notice, with nothing new.
    // nothing is sent, but the tracker is saved so the next outage is notified
    CheckableAgain,
}

// checks a tracker's location against the current alerts. alerts is None when the
// alerts could not be loaded. UserPoint.alerts_sent keeps the ids of the alerts the
// user was sent, so each alert is sent once, and UserPoint.alerts_uncheckable records
// that they were told the alerts could not be checked, so that is notified once per
// outage. alerts in a stale snapshot that the user hasn't been sent are still sent,
// with the stale notice, since the snapshot can be newer than the last check's
// unused until the scheduler calls it, remove the allow then
#[allow(dead_code)]
pub fn check_alert_tracker(
    user_point: &mut UserPoint,
    alerts: Option<&Snapshot<AlertsSorted>>,
) -> AlertTrackerUpdate {
    let Some(snapshot) = alerts else {
        return record_uncheckable(user_point);
    };
    let active = nws::check_alerts(&snapshot.data, &user_point.point);

    // only alerts which are active are retained
    user_point.alerts_sent.retain(|sent| {
        active
            .iter()
            .any(|alert| alert.id.as_deref() == Some(sent.as_str()))
    });
    let new: Vec<Alert> = active
        .into_iter()
        .filter(|alert| match &alert.id {
            Some(id) => !user_point.alerts_sent.contains(id),
            // an alert without an id can't be recorded, so it is sent on every
            // check while it is active, which over reports rather than miss it
            None => {
                tracing::warn!(
                    "check_alert_tracker: {} alert has no id, sending it again",
                    alert.properties.event.as_deref().unwrap_or("unknown")
                );
                true
            }
        })
        .collect();
    user_point
        .alerts_sent
        .extend(new.iter().filter_map(|alert| alert.id.clone()));

    // only fresh alerts show the alerts can be checked again
    let checked_again = !snapshot.stale && user_point.alerts_uncheckable.is_raised();
    if !snapshot.stale {
        user_point.alerts_uncheckable.clear();
    }
    if !new.is_empty() {
        AlertTrackerUpdate::NewAlerts {
            alerts: new,
            stale: snapshot.stale,
        }
    } else if snapshot.stale {
        record_uncheckable(user_point)
    } else if checked_again {
        AlertTrackerUpdate::CheckableAgain
    } else {
        AlertTrackerUpdate::Unchanged
    }
}

// the could not be checked notice is sent once, until fresh alerts are checked again
fn record_uncheckable(user_point: &mut UserPoint) -> AlertTrackerUpdate {
    if user_point.alerts_uncheckable.raise() {
        AlertTrackerUpdate::Uncheckable
    } else {
        AlertTrackerUpdate::Unchanged
    }
}
