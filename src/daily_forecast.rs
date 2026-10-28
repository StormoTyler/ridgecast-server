// timing for daily_forecast trackers, which send a forecast at
// DAILY_FORECAST_HOUR local time at the tracked location
// documented in docs/sms-format.md

use chrono::{DateTime, Days, TimeZone, Utc};
use chrono_tz::Tz;

// local modules
use crate::models::Point;

pub const DAILY_FORECAST_HOUR: u32 = 6;

// the point's IANA time zone from the NWS points endpoint, None if NWS
// left it out or sent a name chrono-tz does not know
pub fn point_time_zone(point: &Point) -> Option<Tz> {
    point.properties.time_zone.as_deref()?.parse().ok()
}

// the first DAILY_FORECAST_HOUR in tz strictly after `after`, or None if it
// falls after `expires`. strictly after, so passing the time a forecast was
// sent gives the next day's. each date is built in local time instead of adding
// 24 hours, so the hour stays fixed across daylight saving changes
pub fn next_daily_forecast(
    tz: Tz,
    after: DateTime<Utc>,
    expires: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let mut date = after.with_timezone(&tz).date_naive();
    // today's and tomorrow's, plus a spare day in case one does not exist in tz
    for _ in 0..3 {
        // earliest() picks the first of a repeated time and is None for a skipped
        // one. US daylight saving changes at 2 AM, so neither happens at 6 AM
        let local = date
            .and_hms_opt(DAILY_FORECAST_HOUR, 0, 0)
            .and_then(|naive| tz.from_local_datetime(&naive).earliest());
        if let Some(local) = local {
            let next = local.with_timezone(&Utc);
            if next > after {
                return (next <= expires).then_some(next);
            }
        }
        date = date.checked_add_days(Days::new(1))?;
    }
    None
}
