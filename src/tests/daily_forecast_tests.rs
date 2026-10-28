// tests for daily forecast timing in src/daily_forecast.rs
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;

use crate::daily_forecast;
use crate::models::{NWR, Point, PointGeometry, PointProperties};

const LOS_ANGELES: Tz = chrono_tz::America::Los_Angeles;

fn utc(time: &str) -> DateTime<Utc>
{
    time.parse().unwrap()
}

// the next 6 AM with a week left on the tracker
fn next(tz: Tz, after: &str) -> Option<DateTime<Utc>>
{
    let after = utc(after);
    daily_forecast::next_daily_forecast(tz, after, after + Duration::hours(168))
}

fn point_with_time_zone(time_zone: Option<&str>) -> Point
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
            time_zone: time_zone.map(str::to_string),
        },
        id: None,
        geometry: PointGeometry { coordinates: geo::Point::new(-123.0921, 44.0512) },
    }
}

#[test]
fn before_6am_gives_same_day()
{
    // 5:00 AM PDT
    assert_eq!(next(LOS_ANGELES, "2026-10-06T12:00:00Z"), Some(utc("2026-10-06T13:00:00Z")));
}

#[test]
fn after_6am_gives_next_day()
{
    // 6:01 AM and 11:00 PM PDT
    assert_eq!(next(LOS_ANGELES, "2026-10-06T13:01:00Z"), Some(utc("2026-10-07T13:00:00Z")));
    assert_eq!(next(LOS_ANGELES, "2026-10-07T06:00:00Z"), Some(utc("2026-10-07T13:00:00Z")));
}

#[test]
fn exactly_6am_gives_next_day()
{
    // passing the time a forecast was sent must not send it again
    assert_eq!(next(LOS_ANGELES, "2026-10-06T13:00:00Z"), Some(utc("2026-10-07T13:00:00Z")));
}

#[test]
fn local_date_is_used_not_utc_date()
{
    // 8:00 PM PDT on Oct 6 is already Oct 7 in UTC
    assert_eq!(next(LOS_ANGELES, "2026-10-07T03:00:00Z"), Some(utc("2026-10-07T13:00:00Z")));
}

#[test]
fn stays_at_6am_across_daylight_saving_end()
{
    // 7:00 AM PDT Oct 31, the clocks fall back at 2 AM Nov 1, so 6 AM PST is 14:00 UTC
    assert_eq!(next(LOS_ANGELES, "2026-10-31T14:00:00Z"), Some(utc("2026-11-01T14:00:00Z")));
}

#[test]
fn stays_at_6am_across_daylight_saving_start()
{
    // 7:00 AM PST Mar 7, the clocks spring forward at 2 AM Mar 8, so 6 AM PDT is 13:00 UTC
    assert_eq!(next(LOS_ANGELES, "2026-03-07T15:00:00Z"), Some(utc("2026-03-08T13:00:00Z")));
}

#[test]
fn zones_without_daylight_saving()
{
    // Arizona stays at UTC-7 and Hawaii at UTC-10 all year
    assert_eq!(next(chrono_tz::America::Phoenix, "2026-11-01T14:00:00Z"),
        Some(utc("2026-11-02T13:00:00Z")));
    assert_eq!(next(chrono_tz::Pacific::Honolulu, "2026-11-01T14:00:00Z"),
        Some(utc("2026-11-01T16:00:00Z")));
}

#[test]
fn none_when_tracker_expires_first()
{
    // 8:00 PM PDT with 9 hours left, expiring at 5:00 AM
    let after = utc("2026-10-07T03:00:00Z");
    assert_eq!(daily_forecast::next_daily_forecast(LOS_ANGELES, after, after + Duration::hours(9)), None);
}

#[test]
fn included_when_tracker_expires_exactly_at_6am()
{
    // 8:00 PM PDT with 10 hours left, expiring at 6:00 AM
    let after = utc("2026-10-07T03:00:00Z");
    assert_eq!(daily_forecast::next_daily_forecast(LOS_ANGELES, after, after + Duration::hours(10)),
        Some(utc("2026-10-07T13:00:00Z")));
}

#[test]
fn reads_point_time_zone()
{
    assert_eq!(daily_forecast::point_time_zone(&point_with_time_zone(Some("America/Los_Angeles"))),
        Some(LOS_ANGELES));
    assert_eq!(daily_forecast::point_time_zone(&point_with_time_zone(Some("Not/AZone"))), None);
    assert_eq!(daily_forecast::point_time_zone(&point_with_time_zone(Some(""))), None);
    assert_eq!(daily_forecast::point_time_zone(&point_with_time_zone(None)), None);
}
