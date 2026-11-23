// the tracker checks: what a tracker is sent when the scheduler checks it, and
// how its stored state changes, for the avalanche and alert checks, and when a
// check is a tracker's last or sends its daily forecast. matching a point against
// the data stays in upstream

use chrono::{DateTime, TimeDelta, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

// local modules
use crate::daily_forecast;
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
    // the avalanche data could not be loaded at all, so the stored forecast can't be
    // confirmed and counts as lost. send "Avy: data unavailable, try later" and save the tracker
    DataUnavailable,
    // the stored forecast is valid again after a loss. the user already has it,
    // so nothing is sent, but the tracker is saved so the next loss is notified
    ForecastRestored,
}

// checks a tracker's avalanche zone against the current data.
// UserPoint.avy keeps the last valid forecast the user was sent, and
// UserPoint.avy_lost records that they were told it was lost, so a loss is
// notified once and a forecast that returns unchanged is not sent again.
// a stale snapshot counts as losing the forecast, and so does avalanche data that
// could not be loaded, which snapshot is None for
// unused until the scheduler calls it, remove the allow then
#[allow(dead_code)]
pub fn check_avy_tracker(
    user_point: &mut UserPoint,
    snapshot: Option<&Snapshot<AvalancheData>>,
    now: DateTime<Utc>,
) -> AvyTrackerUpdate {
    let Some(snapshot) = snapshot else {
        return record_avy_loss(user_point, AvyTrackerUpdate::DataUnavailable);
    };
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

// ---- timing ----

// a daily forecast is tried at the check it is due and at the 3 hourly checks after
// it, 6 to 9 AM local time. a check whose forecast can't be fetched sends the hour's
// notifications as usual and leaves the forecast due, so the next check tries again
pub const DAILY_FORECAST_ATTEMPTS: i64 = 4;

// trackers are checked every hour on the hour
const CHECK_INTERVAL: TimeDelta = TimeDelta::hours(1);

// hour is the top of the hour being checked, rather than the time a tracker is
// reached, so every tracker in one check is timed the same however long it takes

// the previous check already ended the tracker, so nothing more is sent. its TTL
// index deletes it, but MongoDB's TTL process runs about once a minute, so the
// check can still read it
// unused until the scheduler calls it, remove the allow then
#[allow(dead_code)]
pub fn already_ended(user_point: &UserPoint, hour: DateTime<Utc>) -> bool {
    user_point.expires_at < hour
}

// no check runs before the tracker expires after this one, so this check sends
// Tracking ended. the expiry is inclusive, since replies show it to the minute: a
// tracker shown as until 2PM is last checked at 2:00 PM, whether it expires at
// 2:00:00 or 2:00:30, and one that expires at 2:10 PM is too
// unused until the scheduler calls it, remove the allow then
#[allow(dead_code)]
pub fn is_last_check(user_point: &UserPoint, hour: DateTime<Utc>) -> bool {
    !already_ended(user_point, hour) && user_point.expires_at < hour + CHECK_INTERVAL
}

// a late daily forecast, such as one whose checks were missed while the server was
// down, is no longer sent from this hour local time on the day it was due, since by
// then it is too late to be useful
pub const DAILY_FORECAST_CUTOFF_HOUR: u32 = 16;

// whether a check sends the tracker's daily forecast
// unused until the scheduler calls it, remove the allow then
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DailyForecastCheck {
    NotDue,
    // try to fetch the forecast. when it can't be fetched and last is set, no later
    // check tries again: send the daily forecast unavailable notice once and
    // advance_daily_forecast to the next day
    Attempt { last: bool },
    // the forecast that was due is too late to send, past DAILY_FORECAST_CUTOFF_HOUR
    // on its day. send nothing for it, not even the daily forecast unavailable
    // notice, advance_daily_forecast, and save the tracker
    TooLate,
}

// next_daily_forecast stays at its due time while a forecast is retried, so it can
// be a past day's. a check on a later day is about that day's forecast: from 6 AM
// it is due at that day's 6 AM, and before 6 AM the past day's forecast is TooLate,
// which moves it to that day's 6 AM. so a forecast that was never sent can't hold
// up the next day's
//
// the attempt is the last when it is the 4th (6 to 9 AM), when the tracker ends
// at this check, or when the checks were late, such as while the server was down:
// a forecast due at 6 AM is tried once at 10 AM rather than skipped, since it's
// still today's, until DAILY_FORECAST_CUTOFF_HOUR. an ended tracker's forecast is
// never due, even when it was still due at its last check because that forecast
// wasn't delivered
// unused until the scheduler calls it, remove the allow then
#[allow(dead_code)]
pub fn daily_forecast_check(user_point: &UserPoint, hour: DateTime<Utc>) -> DailyForecastCheck {
    let Some(stored_due) = user_point.next_daily_forecast else {
        return DailyForecastCheck::NotDue;
    };
    if stored_due > hour || already_ended(user_point, hour) {
        return DailyForecastCheck::NotDue;
    }
    // a daily forecast tracker always has a time zone, so UTC is only a fallback
    let tz = daily_forecast::point_time_zone(&user_point.point).unwrap_or(Tz::UTC);
    let local = hour.with_timezone(&tz);

    let due = if stored_due.with_timezone(&tz).date_naive() == local.date_naive() {
        stored_due
    } else {
        // a past day's forecast. this day's is due if its 6 AM has passed: the first
        // 6 AM after this day's local midnight, built in local time so a daylight
        // saving change can't shift it. US time zones change at 2 AM, never midnight
        let midnight = local
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|naive| tz.from_local_datetime(&naive).earliest());
        let todays = midnight.and_then(|midnight| {
            daily_forecast::next_daily_forecast(
                tz,
                midnight.with_timezone(&Utc) - TimeDelta::seconds(1),
                hour,
            )
        });
        match todays {
            Some(todays) => todays,
            None => return DailyForecastCheck::TooLate,
        }
    };
    if local.hour() >= DAILY_FORECAST_CUTOFF_HOUR {
        return DailyForecastCheck::TooLate;
    }
    let hours_late = (hour - due).num_hours();
    DailyForecastCheck::Attempt {
        last: hours_late >= DAILY_FORECAST_ATTEMPTS - 1 || is_last_check(user_point, hour),
    }
}

// moves the daily forecast to the next DAILY_FORECAST_HOUR after this check, once
// it was sent or its last attempt failed. None once no 6 AM is left before the
// tracker expires, or when the point has no usable time zone, which a daily
// forecast tracker always has
// unused until the scheduler calls it, remove the allow then
#[allow(dead_code)]
pub fn advance_daily_forecast(user_point: &mut UserPoint, hour: DateTime<Utc>) {
    user_point.next_daily_forecast = match daily_forecast::point_time_zone(&user_point.point) {
        Some(tz) => daily_forecast::next_daily_forecast(tz, hour, user_point.expires_at),
        None => {
            tracing::warn!(
                "advance_daily_forecast: daily forecast tracker has no usable time zone: {:?}",
                user_point.point.properties.time_zone
            );
            None
        }
    };
}
