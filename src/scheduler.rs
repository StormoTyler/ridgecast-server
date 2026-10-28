// scheduler for retreiving NWS alerts, and NAC forecasts 
// checking alerts, and avalanche forecast trackers
// and sending daily forecasts

use chrono::{Duration, Timelike, Utc};

// local modules
use crate::database::DB;
use crate::snapshots;

// refreshes should occur 5 minutes before the tracker check
// i.e. refreshes occur at 23:55 and tracker check occurs at 00:00

// retrieves NWS alerts every 15 minutes, at :10, :25, :40 and :55, from
// https://api.weather.gov/alerts/active?status=actual
// using snapshots::refresh_alerts, which also stores the snapshot.
// :55 keeps one refresh 5 minutes before each hourly tracker check
async fn refresh_alerts(db: DB)
{
    loop
    {
        // the next of :10, :25, :40 or :55 in UTC, worked out from the clock
        // each time, so the schedule doesn't drift
        let now = Utc::now();
        let mut next = now.date_naive().and_hms_opt(now.hour(), 10, 0)
            .expect("10 minutes past a valid hour is a valid time")
            .and_utc();
        while next <= now
        {
            next += Duration::minutes(15);
        }
        tokio::time::sleep((next - now).to_std().unwrap_or_default()).await;

        // a failed refresh is only logged, the loaders refresh stale data on use
        // and the next scheduled refresh tries again
        if let Err(e) = snapshots::refresh_alerts(&db).await
        {
            eprintln!("refresh_alerts: scheduled alerts refresh failed: {e}");
        }
    }
}

// retrieves NAC forecasts hourly, 5 minutes before the hour, from
// https://api.avalanche.org/v2/public/products/map-layer
// using snapshots::refresh_avy, which also stores the snapshot
async fn refresh_avy(db: DB)
{
    loop
    {
        // the next :55 in UTC, which is :55 local time in every whole-hour time zone.
        // worked out from the clock each time, so the schedule doesn't drift
        let now = Utc::now();
        let mut next = now.date_naive().and_hms_opt(now.hour(), 55, 0)
            .expect("55 minutes past a valid hour is a valid time")
            .and_utc();
        if next <= now
        {
            next += Duration::hours(1);
        }
        tokio::time::sleep((next - now).to_std().unwrap_or_default()).await;

        // a failed refresh is only logged, the loaders refresh stale data on use
        // and the next scheduled refresh tries again
        if let Err(e) = snapshots::refresh_avy(&db).await
        {
            eprintln!("refresh_avy: scheduled avalanche refresh failed: {e}");
        }
    }
}

// checks every tracker hourly on the hour 
// i.e. checks at 00:00, 01:00 ... 23:00
// for each UserPoint in the point_collection
async fn check_trackers() 
{
    // retrieve each UserPoint in point_collection
    // load alerts snapshot 
    // load avalanche snapshot
    // then call check_tracker on the point
}

async fn check_tracker(user_point: UserPoint, alerts: )
{
    let point: Point = user_point.point

    if user_point.forecast_vars.track_alerts
    {
        nws:check_alerts(point);
    }

    if user_point.forecast_vars.track_avy
    {
        nac:check_avy(point);
    }

    // gets daily weather forecast if the daily_forecast option is true
    // and it is 6am in the time zone of user_point
    if user_point.forecast_vars.daily_forecast && 6am user time
    {
        nws:get_gridpoints(point);
    }

    // assemble each of the reponses into a TrackerResponse
    let response TrackerResponse
    {

    };

    // in the future will make a call to a function in sms.rs
    // to send a message to the user if there is a new daily forecast
    // new alerts or new avalanche forecast
    // provide logic to check this, but do not call any function
    // at this time 
}

