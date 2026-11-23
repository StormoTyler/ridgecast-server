// scheduler for retreiving NWS alerts, and NAC forecasts
// checking alerts, and avalanche forecast trackers
// and sending daily forecasts

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Duration, Timelike, Utc};
use mongodb::bson::doc;

// local modules
use crate::database;
use crate::messaging;
use crate::models::{AlertsSorted, AvalancheData, Snapshot, UserPoint};
use crate::sms::{forecast, outbound, reply};
use crate::snapshots;
use crate::tracking::tracker_checks;
use crate::upstream::nws;

// refreshes should occur 5 minutes before the tracker check
// i.e. refreshes occur at 23:55 and tracker check occurs at 00:00

// retrieves NWS alerts every 15 minutes, at :10, :25, :40 and :55, from
// https://api.weather.gov/alerts/active?status=actual
// using snapshots::refresh_alerts, which also stores the snapshot.
// :55 keeps one refresh 5 minutes before each hourly tracker check
pub async fn refresh_alerts(db: database::DB) {
    loop {
        // the next of :10, :25, :40 or :55 in UTC, worked out from the clock
        // each time, so the schedule doesn't drift
        let now = Utc::now();
        let mut next = now
            .date_naive()
            .and_hms_opt(now.hour(), 10, 0)
            .expect("10 minutes past a valid hour is a valid time")
            .and_utc();
        while next <= now {
            next += Duration::minutes(15);
        }
        tokio::time::sleep((next - now).to_std().unwrap_or_default()).await;

        // a failed refresh is only logged, the loaders refresh stale data on use
        // and the next scheduled refresh tries again
        if let Err(e) = snapshots::refresh_alerts(&db).await {
            tracing::warn!("refresh_alerts: scheduled alerts refresh failed: {e}");
        }
    }
}

// retrieves NAC forecasts hourly, 5 minutes before the hour, from
// https://api.avalanche.org/v2/public/products/map-layer
// using snapshots::refresh_avy, which also stores the snapshot
pub async fn refresh_avy(db: database::DB) {
    loop {
        // the next :55 in UTC, which is :55 local time in every whole-hour time zone.
        // worked out from the clock each time, so the schedule doesn't drift
        let now = Utc::now();
        let mut next = now
            .date_naive()
            .and_hms_opt(now.hour(), 55, 0)
            .expect("55 minutes past a valid hour is a valid time")
            .and_utc();
        if next <= now {
            next += Duration::hours(1);
        }
        tokio::time::sleep((next - now).to_std().unwrap_or_default()).await;

        // a failed refresh is only logged, the loaders refresh stale data on use
        // and the next scheduled refresh tries again
        if let Err(e) = snapshots::refresh_avy(&db).await {
            tracing::warn!("refresh_avy: scheduled avalanche refresh failed: {e}");
        }
    }
}

// checks every tracker hourly on the hour, at 00:00, 01:00 ... 23:00 UTC, which is
// on the hour local time in every whole-hour time zone, so a daily forecast due at
// 6 AM is checked at 6 AM. the snapshots were refreshed at :55
pub async fn check_trackers(db: database::DB, sender: Arc<dyn messaging::MessageSender>) {
    loop {
        // the next top of the hour, worked out from the clock each time, so the
        // schedule doesn't drift
        let now = Utc::now();
        let hour = now
            .date_naive()
            .and_hms_opt(now.hour(), 0, 0)
            .expect("the top of a valid hour is a valid time")
            .and_utc()
            + Duration::hours(1);
        tokio::time::sleep((hour - now).to_std().unwrap_or_default()).await;

        // loaded once for every tracker. a snapshot that can't be loaded is None,
        // which the checks report to the trackers that use it
        let alerts = snapshots::load_alerts(&db)
            .await
            .inspect_err(|e| tracing::warn!("check_trackers: alerts unavailable: {e}"))
            .ok();
        let avy = snapshots::load_avy(&db)
            .await
            .inspect_err(|e| tracing::warn!("check_trackers: avalanche data unavailable: {e}"))
            .ok();
        check_all(&db, sender.as_ref(), alerts.as_ref(), avy.as_ref(), hour).await;
    }
}

// checks every tracker for the hourly check at hour, with the snapshots loaded
// for it, and sends and saves what each check found
pub async fn check_all(
    db: &database::DB,
    sender: &dyn messaging::MessageSender,
    alerts: Option<&Snapshot<AlertsSorted>>,
    avy: Option<&Snapshot<AvalancheData>>,
    hour: DateTime<Utc>,
) {
    let trackers = read_trackers(db).await;
    // a user who opted out had all their trackers deleted, so their other trackers
    // read before that are skipped rather than sent to again
    let mut opted_out: HashSet<messaging::PhoneNumber> = HashSet::new();
    for tracker in trackers {
        if opted_out.contains(&tracker.user) {
            continue;
        }
        if let Checked::OptedOut = check_one(db, sender, tracker.clone(), alerts, avy, hour).await {
            opted_out.insert(tracker.user);
        }
    }
}

// every tracker in the points collection, read into memory before any is checked:
// the TTL index deletes a tracker about a minute after it expires, so a slow check
// reading from a cursor could lose one before its last check. a tracker that can't
// be read, such as one with a bad stored number, is logged and skipped
async fn read_trackers(db: &database::DB) -> Vec<UserPoint> {
    let mut cursor = match db.points_collection.find(doc! {}).await {
        Ok(cursor) => cursor,
        Err(e) => {
            tracing::error!("check_trackers: could not read the trackers: {e}");
            return Vec::new();
        }
    };
    let mut trackers = Vec::new();
    loop {
        match cursor.advance().await {
            Ok(true) => match cursor.deserialize_current() {
                Ok(tracker) => trackers.push(tracker),
                Err(e) => {
                    tracing::error!("check_trackers: skipped a tracker that can't be read: {e}")
                }
            },
            Ok(false) => break,
            Err(e) => {
                tracing::error!("check_trackers: stopped reading the trackers: {e}");
                break;
            }
        }
    }
    trackers
}

// how a tracker's check ended, for check_all
enum Checked {
    Done,
    // the user opted out, and all their trackers were deleted
    OptedOut,
}

// the messages one check sends, each its own response, so each one's delivery
// decides whether what it reports is recorded
#[derive(Clone, Copy, PartialEq)]
enum Message {
    // replaces the hour's notifications, so it also reports the avalanche and alerts
    Daily,
    Avy,
    Alerts,
    // the closing lines alone, when the check sends nothing else
    Closing,
}

// checks one tracker, sends what it found, and saves it. the check runs on a copy,
// and only what was delivered is saved: a notification that fails every attempt
// isn't recorded as sent, so the next check finds the same change and tries again
async fn check_one(
    db: &database::DB,
    sender: &dyn messaging::MessageSender,
    stored: UserPoint,
    alerts: Option<&Snapshot<AlertsSorted>>,
    avy: Option<&Snapshot<AvalancheData>>,
    hour: DateTime<Utc>,
) -> Checked {
    let mut checked = stored.clone();
    let Some(check) = tracker_checks::check_tracker(&mut checked, alerts, avy, hour) else {
        // the previous check ended it
        return Checked::Done;
    };
    let format = reply::Format::from_brief(checked.forecast_vars.brief);
    // a tracker always has a name, BRAND is only a fallback
    let name = checked.tracker_name.as_deref().unwrap_or(reply::BRAND);

    // the daily forecast, when due. a forecast that can't be fetched is retried at
    // the next check, and after its last attempt is given up with a notice
    let mut daily = None;
    let mut daily_unavailable = false;
    let mut advance_daily = false;
    match check.daily {
        tracker_checks::DailyForecastCheck::NotDue => {}
        // too late to be useful: skipped without a notice
        tracker_checks::DailyForecastCheck::TooLate => advance_daily = true,
        tracker_checks::DailyForecastCheck::Attempt { last } => {
            match forecast::fetch_forecast(
                "check_trackers",
                &checked.point,
                &checked.forecast_vars.forecast_properties,
                1,
                hour,
            )
            .await
            {
                Ok(today) => daily = Some(daily_response(&checked, today, alerts, avy, hour)),
                Err(_) if last => daily_unavailable = true,
                // fetch_forecast logged it, and the next check tries again
                Err(_) => {}
            }
        }
    }
    let closing = reply::closing_lines(daily_unavailable, check.ended, format);

    // a daily forecast replaces the notifications, since it has their lines.
    // otherwise the notifications go in the reply's order, avalanche then alerts,
    // and the closing lines end the last one sent, or are sent alone
    let avy_message = |closing: &[String]| {
        check.avy.map_or_else(Vec::new, |update| {
            reply::format_avy_notification(update, checked.avy.as_ref(), format, name, closing)
        })
    };
    let alerts_message = |closing: &[String]| {
        check.alerts.as_ref().map_or_else(Vec::new, |update| {
            reply::format_alert_notification(update, &checked.point, format, name, closing)
        })
    };
    let avy_sends_nothing = avy_message(&[]).is_empty();
    let alerts_sends_nothing = alerts_message(&[]).is_empty();
    // the message that carries the closing lines, if there are any
    let closing_carrier = if daily.is_some() {
        Message::Daily
    } else if !alerts_sends_nothing {
        Message::Alerts
    } else if !avy_sends_nothing {
        Message::Avy
    } else {
        Message::Closing
    };
    let messages: Vec<(Message, Vec<Vec<String>>)> = match &daily {
        Some(response) => vec![(
            Message::Daily,
            reply::format_daily_forecast(
                response,
                &checked.forecast_vars.forecast_properties,
                format,
                name,
                hour,
                &closing,
            ),
        )],
        None => {
            let closing_for = |message: Message| {
                if message == closing_carrier {
                    closing.as_slice()
                } else {
                    &[]
                }
            };
            vec![
                (Message::Avy, avy_message(closing_for(Message::Avy))),
                (
                    Message::Alerts,
                    alerts_message(closing_for(Message::Alerts)),
                ),
                (
                    Message::Closing,
                    reply::format_closing(closing_for(Message::Closing), name),
                ),
            ]
        }
    };

    // sent from the number the user texted, so the conversation never switches numbers
    let mut delivered = Vec::new();
    for (message, responses) in messages {
        if responses.is_empty() {
            continue;
        }
        let count: usize = responses.iter().map(Vec::len).sum();
        let deliveries =
            outbound::send_responses(sender, &checked.reply_from, &checked.user, &responses).await;
        let failure = deliveries
            .iter()
            .find_map(|delivery| delivery.result.as_ref().err());
        match failure {
            None if deliveries.len() == count => delivered.push(message),
            // the user replied STOP: every tracker of theirs is deleted, not paused
            Some(messaging::SendError::OptedOut) => {
                match database::delete_user_trackers(&checked.user, db).await {
                    Ok(deleted) => {
                        tracing::info!("check_trackers: user opted out, deleted {deleted} trackers")
                    }
                    Err(e) => tracing::error!(
                        "check_trackers: user opted out, but their trackers could not be deleted: {e}"
                    ),
                }
                return Checked::OptedOut;
            }
            // retrying won't help, and every message would fail the same way. the
            // tracker is kept, since the cause may be the server's, not the user's
            Some(messaging::SendError::Permanent(e)) => {
                tracing::error!(
                    "check_trackers: tracker {:?} message rejected, not recorded: {e}",
                    checked.id
                );
                break;
            }
            // outbound logged each failed message. not recorded, so the next check
            // tries again
            Some(messaging::SendError::Transient(_)) | None => {}
        }
    }

    // only what was delivered is saved: the stored tracker takes the parts of the
    // checked one that a delivered message reported, or that had nothing to send.
    // a delivered daily forecast reports the avalanche and alerts too
    let mut save = stored;
    let mut changed = false;
    let daily_delivered = delivered.contains(&Message::Daily);
    let recorded = |message: Message, sends_nothing: bool| {
        daily_delivered || delivered.contains(&message) || (sends_nothing && daily.is_none())
    };
    if let Some(update) = check.avy
        && recorded(Message::Avy, avy_sends_nothing)
    {
        changed |= update != tracker_checks::AvyTrackerUpdate::Unchanged;
        save.avy = checked.avy.clone();
        save.avy_lost = checked.avy_lost;
        save.avy_off_season = checked.avy_off_season;
    }
    if let Some(update) = &check.alerts
        && recorded(Message::Alerts, alerts_sends_nothing)
    {
        // Unchanged may only have dropped ids of ended alerts, which needn't be saved
        changed |= !matches!(update, tracker_checks::AlertTrackerUpdate::Unchanged);
        save.alerts_sent = checked.alerts_sent.clone();
        save.alerts_uncheckable = checked.alerts_uncheckable;
    }
    // a sent daily forecast moves to the next day, and so does one too late to send,
    // or given up on once its notice, in the closing lines, was delivered
    let notice_delivered = daily_unavailable && delivered.contains(&closing_carrier);
    if daily_delivered || notice_delivered || advance_daily {
        tracker_checks::advance_daily_forecast(&mut save, hour);
        changed = true;
    }

    if changed {
        match database::update_tracker(&save, db).await {
            Ok(_) => {}
            Err(e) => tracing::error!(
                "check_trackers: tracker {:?} could not be saved: {e}",
                save.id
            ),
        }
    }
    Checked::Done
}

// a daily forecast is built like a reply: every active alert at the point, the
// avalanche line, and today's forecast, with the notices for data that is out of
// date or couldn't be loaded. it has no tracker line
fn daily_response(
    tracker: &UserPoint,
    today: forecast::RequestedForecast,
    alerts: Option<&Snapshot<AlertsSorted>>,
    avy: Option<&Snapshot<AvalancheData>>,
    hour: DateTime<Utc>,
) -> forecast::SmsForecastResponse {
    let (avy_zone, avy_notice) = forecast::avalanche_for_point(
        &tracker.point,
        avy.map(|snapshot| &snapshot.data),
        tracker.forecast_vars.track_avy,
        hour,
    );
    forecast::SmsForecastResponse {
        point: tracker.point.clone(),
        forecast: Some(today),
        alerts: alerts.map_or_else(Vec::new, |snapshot| {
            nws::check_alerts(&snapshot.data, &tracker.point)
        }),
        alerts_notice: alerts.is_none(),
        alerts_stale_notice: alerts.is_some_and(|snapshot| snapshot.stale),
        avy: avy_zone,
        avy_notice,
        avy_stale_notice: avy.is_some_and(|snapshot| snapshot.stale),
        daily_notice: false,
        tracker: None,
    }
}
