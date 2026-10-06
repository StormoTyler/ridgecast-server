// tests for the SMS request parser in src/sms.rs
use crate::models::ForecastVars;
use crate::sms::{SmsParseError, SmsRequest};

// flags in bit order, written out from the table in docs/sms-format.md
// so a change to the parser's mapping fails these tests
fn flags_in_bit_order(vars: &ForecastVars) -> [bool; 13]
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
    let cases: [(&str, f32, f32, &[usize]); 7] =
    [
        ("TA1 44.0512 -123.0921 0000 0", 44.0512, -123.0921, &[]),
        ("TA1 44.0512 -123.0921 0003 24", 44.0512, -123.0921, &[0, 1]),
        ("TA1 44.0512 -123.0921 0845 72", 44.0512, -123.0921, &[0, 2, 6, 11]),
        ("TA1 39.1968 -120.2354 0C81 168", 39.1968, -120.2354, &[0, 7, 10, 11]),
        ("TA1 44.0512 -123.0921 0FFF 24", 44.0512, -123.0921, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
        ("TA1 44.0512 -123.0921 1000 0", 44.0512, -123.0921, &[12]),
        ("TA1 44.0512 -123.0921 1FFF 24", 44.0512, -123.0921, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]),
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
    for n in 0..13
    {
        let message = format!("TA1 44.0 -123.0 {:04X} 24", 1u16 << n);
        let request = parse(&message).unwrap();
        assert_eq!(set_bits(&request), vec![n], "{message}");
    }
}

#[test]
fn accepts_lenient_input()
{
    // lowercase prefix and flags
    assert_eq!(set_bits(&parse("ta1 44.0 -123.0 1fff 24").unwrap()).len(), 13);
    // surrounding whitespace and repeated spaces
    let request = parse("  TA1   44.0512  -123.0921 0845  24 \n").unwrap();
    assert_eq!(request.lat, 44.0512);
    // whole-number coordinates and the boundary values
    let request = parse("TA1 -90 180 0000 0").unwrap();
    assert_eq!((request.lat, request.lon), (-90.0, 180.0));
    let request = parse("TA1 90.0000 -180.0000 0000 0").unwrap();
    assert_eq!((request.lat, request.lon), (90.0, -180.0));
}

#[test]
fn rejects_messages_that_are_not_requests()
{
    for message in ["", "   ", "hello", "Take me home", "TA", "TA 44.0 -123.0 0000 0", "TAx1 44.0 -123.0 0000 0", "44.0 -123.0 000 0"]
    {
        assert_eq!(parse(message).unwrap_err(), SmsParseError::NotARequest, "{message:?}");
    }
}

#[test]
fn rejects_unsupported_versions()
{
    assert_eq!(parse("TA2 44.0 -123.0 0000 0").unwrap_err(),
        SmsParseError::UnsupportedVersion("2".to_string()));
    // checked before the field count, since another version may have a different layout
    assert_eq!(parse("TA2 something else entirely here").unwrap_err(),
        SmsParseError::UnsupportedVersion("2".to_string()));
    assert_eq!(parse("TA01 44.0 -123.0 0000 0").unwrap_err(),
        SmsParseError::UnsupportedVersion("01".to_string()));
}

#[test]
fn rejects_wrong_field_count()
{
    assert_eq!(parse("TA1").unwrap_err(), SmsParseError::WrongFieldCount(1));
    assert_eq!(parse("TA1 44.0 -123.0").unwrap_err(), SmsParseError::WrongFieldCount(3));
    // the version 1 layout before tracker_duration was added
    assert_eq!(parse("TA1 44.0 -123.0 000").unwrap_err(), SmsParseError::WrongFieldCount(4));
    assert_eq!(parse("TA1 44.0 -123.0 0000 0 extra").unwrap_err(), SmsParseError::WrongFieldCount(6));
}

#[test]
fn rejects_invalid_latitude()
{
    for lat in ["90.0001", "-90.5", "100", "44,0512", "44.05123", "+44.0", "44.", ".5", "4e1", "inf", "NaN", "44.0N", "--44"]
    {
        let message = format!("TA1 {lat} -123.0 0000 0");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidLatitude(lat.to_string()), "{message}");
    }
}

#[test]
fn rejects_invalid_longitude()
{
    for lon in ["180.0001", "-181", "1000", "-123,0921", "-123.09214", "+123.0", "123W"]
    {
        let message = format!("TA1 44.0 {lon} 0000 0");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidLongitude(lon.to_string()), "{message}");
    }
}

#[test]
fn rejects_invalid_flags()
{
    // too short, too long, not hex, signed, split, full-width digits,
    // and unassigned bits 13 to 15
    for flags in ["845", "00845", "G845", "-845", "+845", "08 45", "０８４５", "2000", "4000", "8000", "FFFF"]
    {
        let message = format!("TA1 44.0 -123.0 {flags} 0");
        match parse(&message)
        {
            // "8 45" splits into an extra field
            Err(SmsParseError::WrongFieldCount(6)) if flags.contains(' ') => {}
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
        ("TA1 44.0 -123.0 0000 0", 0),
        ("TA1 44.0 -123.0 0002 0", 0),
        // a duration without a tracker flag is still accepted
        ("TA1 44.0 -123.0 0000 12", 12),
        ("TA1 44.0 -123.0 0001 1", 1),
        ("TA1 44.0 -123.0 0004 168", 168),
        ("TA1 44.0 -123.0 0005 024", 24),
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
        let message = format!("TA1 44.0 -123.0 0001 {duration}");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidTrackerDuration(duration.to_string()), "{message}");
    }
}

#[test]
fn rejects_zero_duration_with_tracker()
{
    // track_alerts, track_avy, and both
    for flags in ["0001", "0004", "0005", "1FFF"]
    {
        let message = format!("TA1 44.0 -123.0 {flags} 0");
        assert_eq!(parse(&message).unwrap_err(),
            SmsParseError::InvalidTrackerDuration("0".to_string()), "{message}");
    }
}
