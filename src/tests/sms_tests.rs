// tests for the SMS request parser in src/sms.rs
use crate::models::ForecastVars;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::models::{FloatVec, IntVec, TimeFloat, TimeInt};
use crate::sms::{SmsParseError, SmsRequest, forecast_end, trim_floats, trim_ints};

// flags in bit order, written out from the table in docs/sms-format.md
// so a change to the parser's mapping fails these tests
fn flags_in_bit_order(vars: &ForecastVars) -> [bool; 14]
{
    let p = &vars.forecast_properties;
    [
        vars.track_alerts,
        vars.no_forecast,
        vars.track_avy,
        p.elevation,
        p.wet_bulb_globe_temperature,
        p.heat_risk,
        p.relative_humidity,
        p.visibility,
        p.ceiling_height,
        p.pressure,
        p.snow_level,
        p.wind_gust,
        vars.brief,
        vars.daily_forecast,
    ]
}

fn parse(message: &str) -> Result<SmsRequest, SmsParseError>
{
    message.parse()
}

fn set_bits(request: &SmsRequest) -> Vec<usize>
{
    flags_in_bit_order(&request.forecast_vars)
        .iter()
        .enumerate()
        .filter(|(_, on)| **on)
        .map(|(n, _)| n)
        .collect()
}

#[test]
fn parses_documented_examples()
{
    let cases: [(&str, f32, f32, &[usize]); 8] =
    [
        ("RC1 44.0512 -123.0921 0000 0 1", 44.0512, -123.0921, &[]),
        ("RC1 44.0512 -123.0921 0003 24 1 Spencer", 44.0512, -123.0921, &[0, 1]),
        ("RC1 45.3735 -121.6959 1845 72 2 Mt Hood", 45.3735, -121.6959, &[0, 2, 6, 11, 12]),
        ("RC1 39.1968 -120.2354 0C81 168 3 Tahoe", 39.1968, -120.2354, &[0, 7, 10, 11]),
        ("RC1 44.0512 -123.0921 0FFF 24 1 Camp", 44.0512, -123.0921, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
        ("RC1 44.0512 -123.0921 1000 0 2", 44.0512, -123.0921, &[12]),
        ("RC1 44.0512 -123.0921 3000 72 1 Camp", 44.0512, -123.0921, &[12, 13]),
        ("RC1 44.0512 -123.0921 3FFF 24 7 Camp", 44.0512, -123.0921, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]),
    ];

    for (message, lat, lon, bits) in cases
    {
        let request = parse(message).unwrap_or_else(|e| panic!("{message}: {e}"));
        assert_eq!(request.lat, lat, "{message}");
        assert_eq!(request.lon, lon, "{message}");
        assert_eq!(set_bits(&request), bits, "{message}");
    }
}

#[test]
fn each_bit_maps_to_one_field()
{
    for n in 0..14
    {
        // bits 0, 2, and 13 create a tracker, which needs a name
        let name = if [0, 2, 13].contains(&n) { " Camp" } else { "" };
        let message = format!("RC1 44.0 -123.0 {:04X} 24 1{name}", 1u16 << n);
        let request = parse(&message).unwrap();
        assert_eq!(set_bits(&request), vec![n], "{message}");
    }
}

#[test]
fn accepts_lenient_input()
{
    // lowercase prefix and flags
    assert_eq!(set_bits(&parse("rc1 44.0 -123.0 3fff 24 1 Camp").unwrap()).len(), 14);
    // surrounding whitespace and repeated spaces
    let request = parse("  RC1   44.0512  -123.0921 0845  24  1   Camp \n").unwrap();
    assert_eq!(request.lat, 44.0512);
    // whole-number coordinates and the boundary values
    let request = parse("RC1 -90 180 0000 0 1").unwrap();
    assert_eq!((request.lat, request.lon), (-90.0, 180.0));
    let request = parse("RC1 90.0000 -180.0000 0000 0 1").unwrap();
    assert_eq!((request.lat, request.lon), (90.0, -180.0));
}

#[test]
fn rejects_messages_that_are_not_requests()
{
    for message in ["", "   ", "hello", "RC car for sale", "RC", "RC 44.0 -123.0 0000 0", "RCx1 44.0 -123.0 0000 0", "44.0 -123.0 000 0"]
    {
        assert_eq!(parse(message).unwrap_err(), SmsParseError::NotARequest, "{message:?}");
    }
}

#[test]
fn rejects_unsupported_versions()
{
    assert_eq!(parse("RC2 44.0 -123.0 0000 0").unwrap_err(),
        SmsParseError::UnsupportedVersion("2".to_string()));
    // checked before the field count, since another version may have a different layout
    assert_eq!(parse("RC2 something else entirely here").unwrap_err(),
        SmsParseError::UnsupportedVersion("2".to_string()));
    assert_eq!(parse("RC01 44.0 -123.0 0000 0").unwrap_err(),
        SmsParseError::UnsupportedVersion("01".to_string()));
}

#[test]
fn rejects_wrong_field_count()
{
    assert_eq!(parse("RC1").unwrap_err(), SmsParseError::WrongFieldCount(1));
    assert_eq!(parse("RC1 44.0 -123.0").unwrap_err(), SmsParseError::WrongFieldCount(3));
    // the version 1 layout before tracker_duration was added
    assert_eq!(parse("RC1 44.0 -123.0 000").unwrap_err(), SmsParseError::WrongFieldCount(4));
    // a request without a tracker flag has no tracker name
    assert_eq!(parse("RC1 44.0 -123.0 0000 0 3 extra").unwrap_err(), SmsParseError::WrongFieldCount(7));
    assert_eq!(parse("RC1 44.0 -123.0 0002 0 3 Mt Hood").unwrap_err(), SmsParseError::WrongFieldCount(8));
}

#[test]
fn rejects_invalid_latitude()
{
    for lat in ["90.0001", "-90.5", "100", "44,0512", "44.05123", "+44.0", "44.", ".5", "4e1", "inf", "NaN", "44.0N", "--44"]
    {
        let message = format!("RC1 {lat} -123.0 0000 0 1");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidLatitude(lat.to_string()), "{message}");
    }
}

#[test]
fn rejects_invalid_longitude()
{
    for lon in ["180.0001", "-181", "1000", "-123,0921", "-123.09214", "+123.0", "123W"]
    {
        let message = format!("RC1 44.0 {lon} 0000 0 1");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidLongitude(lon.to_string()), "{message}");
    }
}

#[test]
fn rejects_invalid_flags()
{
    // too short, too long, not hex, signed, split, full-width digits,
    // and unassigned bits 14 and 15
    for flags in ["845", "00845", "G845", "-845", "+845", "08 45", "０８４５", "4000", "8000", "FFFF"]
    {
        let message = format!("RC1 44.0 -123.0 {flags} 0 1");
        match parse(&message)
        {
            // "08 45" splits into two fields, so only "08" is read as the flags
            Err(SmsParseError::InvalidFlags(read)) if flags.contains(' ') => assert_eq!(read, "08"),
            result => assert_eq!(result.unwrap_err(),
                SmsParseError::InvalidFlags(flags.to_string()), "{message}"),
        }
    }
}

#[test]
fn parses_tracker_duration()
{
    let cases: [(&str, u32); 6] =
    [
        // no tracker flag, so 0 is allowed
        ("RC1 44.0 -123.0 0000 0 1", 0),
        ("RC1 44.0 -123.0 0002 0 1", 0),
        // a duration without a tracker flag is still accepted
        ("RC1 44.0 -123.0 0000 12 1", 12),
        ("RC1 44.0 -123.0 0001 1 1 Camp", 1),
        ("RC1 44.0 -123.0 0004 168 1 Camp", 168),
        ("RC1 44.0 -123.0 0005 024 1 Camp", 24),
    ];

    for (message, hours) in cases
    {
        let request = parse(message).unwrap_or_else(|e| panic!("{message}: {e}"));
        assert_eq!(request.tracker_duration, hours, "{message}");
    }
}

#[test]
fn rejects_invalid_tracker_duration()
{
    for duration in ["169", "999", "1000", "-1", "+24", "24.0", "2e1", "24h", "x"]
    {
        let message = format!("RC1 44.0 -123.0 0001 {duration} 1 Camp");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidTrackerDuration(duration.to_string()), "{message}");
    }
}

#[test]
fn rejects_zero_duration_with_tracker()
{
    // track_alerts, track_avy, daily_forecast, and combinations
    for flags in ["0001", "0004", "2000", "0005", "2002", "3FFF"]
    {
        let message = format!("RC1 44.0 -123.0 {flags} 0 1 Camp");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidTrackerDuration("0".to_string()), "{message}");
    }
}

#[test]
fn forecast_length_is_required()
{
    assert_eq!(parse("RC1 44.0 -123.0 0000 0").unwrap_err(), SmsParseError::WrongFieldCount(5));
}

#[test]
fn parses_forecast_length()
{
    for days in 1..=7
    {
        let message = format!("RC1 44.0 -123.0 0000 0 {days}");
        assert_eq!(parse(&message).unwrap().forecast_length, days, "{message}");
    }
    // surrounding whitespace and repeated spaces, like the other fields
    assert_eq!(parse(" RC1 44.0 -123.0 0001 24   3 Camp ").unwrap().forecast_length, 3);
}

#[test]
fn rejects_invalid_forecast_length()
{
    for length in ["0", "8", "9", "07", "10", "-1", "+3", "3.0", "x", "３"]
    {
        let message = format!("RC1 44.0 -123.0 0000 0 {length}");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidForecastLength(length.to_string()), "{message}");
    }
}

#[test]
fn forecast_length_is_checked_after_the_other_fields()
{
    assert_eq!(parse("RC1 44.0 -123.0 0001 0 9").unwrap_err(),
        SmsParseError::InvalidTrackerDuration("0".to_string()));
}

#[test]
fn parses_tracker_name()
{
    let cases: [(&str, Option<&str>); 7] =
    [
        // no tracker flag, no name
        ("RC1 44.0 -123.0 0000 0 1", None),
        ("RC1 44.0 -123.0 1002 0 1", None),
        ("RC1 44.0 -123.0 0001 24 1 Camp", Some("Camp")),
        // a name may contain spaces, and repeated spaces are read as one
        ("RC1 44.0 -123.0 0004 24 1 Mt Hood", Some("Mt Hood")),
        ("RC1 44.0 -123.0 2000 24 1   Mt   Hood  ", Some("Mt Hood")),
        // case is kept, and the longest name is 10 characters including spaces
        ("RC1 44.0 -123.0 0001 24 1 bob's #2 (", Some("bob's #2 (")),
        ("RC1 44.0 -123.0 0001 24 1 A-B.C&D/E)", Some("A-B.C&D/E)")),
    ];

    for (message, name) in cases
    {
        let request = parse(message).unwrap_or_else(|e| panic!("{message}: {e}"));
        assert_eq!(request.tracker_name.as_deref(), name, "{message}");
    }
}

#[test]
fn tracker_name_is_required_with_a_tracker()
{
    for flags in ["0001", "0004", "2000", "3FFF"]
    {
        let message = format!("RC1 44.0 -123.0 {flags} 24 1");
        assert_eq!(parse(&message).unwrap_err(), SmsParseError::MissingTrackerName, "{message}");
    }
}

#[test]
fn rejects_invalid_tracker_name()
{
    // too long, including the space, and characters outside the allowed set,
    // such as an autocorrected apostrophe, an accented letter, or an emoji
    for name in ["Cascade Lks", "ABCDEFGHIJK", "Bob\u{2019}s", "Cafe\u{301}", "Ros\u{e9}", "\u{26f0}", "Hood!", "a_b", "a,b", "a\"b"]
    {
        let message = format!("RC1 44.0 -123.0 0001 24 1 {name}");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidTrackerName(name.to_string()), "{message}");
    }
}

#[test]
fn tracker_name_is_checked_after_the_other_fields()
{
    assert_eq!(parse("RC1 44.0 -123.0 0001 24 9 Camp").unwrap_err(),
        SmsParseError::InvalidForecastLength("9".to_string()));
    assert_eq!(parse("RC1 44.0 -123.0 0001 24 Mt Hood").unwrap_err(),
        SmsParseError::InvalidForecastLength("Mt".to_string()));
}

const LOS_ANGELES: Tz = chrono_tz::America::Los_Angeles;

fn utc(time: &str) -> DateTime<Utc>
{
    time.parse().unwrap()
}

#[test]
fn one_day_ends_at_local_midnight()
{
    // 10:00 AM PDT Oct 6 ends at midnight PDT starting Oct 7
    assert_eq!(forecast_end(&LOS_ANGELES, utc("2026-10-06T17:00:00Z"), 1),
        utc("2026-10-07T07:00:00Z"));
    // 8:00 PM PDT Oct 6 is already Oct 7 in UTC, but still Oct 6 locally
    assert_eq!(forecast_end(&LOS_ANGELES, utc("2026-10-07T03:00:00Z"), 1),
        utc("2026-10-07T07:00:00Z"));
}

#[test]
fn seven_days_end_at_local_midnight_after_the_seventh_day()
{
    assert_eq!(forecast_end(&LOS_ANGELES, utc("2026-10-06T17:00:00Z"), 7),
        utc("2026-10-13T07:00:00Z"));
}

#[test]
fn forecast_end_follows_daylight_saving_end()
{
    // Oct 31 PDT, 2 days later is midnight PST starting Nov 2, which is 08:00 UTC
    assert_eq!(forecast_end(&LOS_ANGELES, utc("2026-10-31T17:00:00Z"), 2),
        utc("2026-11-02T08:00:00Z"));
}

#[test]
fn forecast_end_in_utc()
{
    assert_eq!(forecast_end(&Utc, utc("2026-10-06T17:00:00Z"), 1),
        utc("2026-10-07T00:00:00Z"));
}

fn floats(times: &[&str]) -> FloatVec
{
    FloatVec
    {
        values: times.iter()
            .map(|time| TimeFloat { valid_time: Some(time.to_string()), value: Some(1.0) })
            .collect(),
    }
}

#[test]
fn trims_values_starting_at_or_after_the_end()
{
    let end = utc("2026-10-07T07:00:00Z");
    let series = floats(&[
        "2026-10-06T17:00:00+00:00/PT1H",
        // starts before the end and runs past it, so it is kept
        "2026-10-07T01:00:00+00:00/PT13H",
        "2026-10-07T07:00:00+00:00/PT1H",
        "2026-10-08T07:00:00+00:00/PT1H",
    ]);
    let kept: Vec<_> = trim_floats(series, end).values.into_iter()
        .map(|value| value.valid_time.unwrap())
        .collect();
    assert_eq!(kept, ["2026-10-06T17:00:00+00:00/PT1H", "2026-10-07T01:00:00+00:00/PT13H"]);
}

#[test]
fn trim_compares_times_with_offsets()
{
    // 11:00 PM PDT Oct 6 is 06:00 UTC Oct 7, before the end
    let series = floats(&["2026-10-06T23:00:00-07:00/PT1H"]);
    assert_eq!(trim_floats(series, utc("2026-10-07T07:00:00Z")).values.len(), 1);
}

#[test]
fn trim_drops_values_without_a_readable_time()
{
    let series = IntVec
    {
        values: vec![
            TimeInt { valid_time: None, value: Some(1) },
            TimeInt { valid_time: Some("not a time".to_string()), value: Some(2) },
            TimeInt { valid_time: Some("2026-10-06T17:00:00+00:00/PT1H".to_string()), value: Some(3) },
        ],
    };
    let kept: Vec<_> = trim_ints(series, utc("2026-10-07T07:00:00Z")).values.into_iter()
        .map(|value| value.value)
        .collect();
    assert_eq!(kept, [Some(3)]);
}
