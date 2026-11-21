// the tracker checks: what a tracker is sent when the scheduler checks it, and
// how its stored state changes. the avalanche check is here, the alert check
// will join it. matching a point against the data stays in upstream

use chrono::{DateTime, Utc};

// local modules
use crate::models::{AvalancheData, Snapshot, UserPoint};
use crate::upstream::nac;

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
