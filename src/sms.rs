// parser for SMS requests sent by the TrailAlert IOS and Android apps
// the message format is documented in docs/sms-format.md

use std::str::FromStr;
use thiserror::Error;

// local modules
use crate::models::{ForecastVars, ForecastProperties};

const SUPPORTED_VERSION: &str = "1";

#[derive(Debug, Clone)]
pub struct SmsRequest
{
    pub lat: f32,
    pub lon: f32,
    pub forecast_vars: ForecastVars,
}

#[derive(Debug, Error, PartialEq)]
pub enum SmsParseError
{
    // the message is not a TrailAlert request at all
    #[error("message is not a TrailAlert request")]
    NotARequest,
    #[error("unsupported message format version {0}")]
    UnsupportedVersion(String),
    #[error("expected 4 fields, found {0}")]
    WrongFieldCount(usize),
    #[error("invalid latitude: {0}")]
    InvalidLatitude(String),
    #[error("invalid longitude: {0}")]
    InvalidLongitude(String),
    #[error("invalid flags: {0}")]
    InvalidFlags(String),
}

impl FromStr for SmsRequest
{
    type Err = SmsParseError;

    fn from_str(message: &str) -> Result<Self, Self::Err>
    {
        // split_whitespace also trims and allows repeated spaces
        let fields: Vec<&str> = message.split_whitespace().collect();

        // the prefix is checked before the field count, so a future
        // version with a different layout reports the version instead
        let version = match fields.first()
        {
            Some(prefix) => parse_prefix(prefix)?,
            None => return Err(SmsParseError::NotARequest),
        };
        if version != SUPPORTED_VERSION
        {
            return Err(SmsParseError::UnsupportedVersion(version.to_string()));
        }

        let [_, lat, lon, flags] = fields[..]
        else
        {
            return Err(SmsParseError::WrongFieldCount(fields.len()));
        };

        Ok(SmsRequest
        {
            lat: parse_coordinate(lat, 2, 90.0)
                .ok_or_else(|| SmsParseError::InvalidLatitude(lat.to_string()))?,
            lon: parse_coordinate(lon, 3, 180.0)
                .ok_or_else(|| SmsParseError::InvalidLongitude(lon.to_string()))?,
            forecast_vars: parse_flags(flags)
                .ok_or_else(|| SmsParseError::InvalidFlags(flags.to_string()))?,
        })
    }
}

// returns the version digits from a "TA<version>" prefix
fn parse_prefix(prefix: &str) -> Result<&str, SmsParseError>
{
    // "TA" must be followed by digits only, so ordinary
    // texts like "Take me home" are not read as requests
    let version = match prefix.get(..2)
    {
        Some(ta) if ta.eq_ignore_ascii_case("TA") => &prefix[2..],
        _ => return Err(SmsParseError::NotARequest),
    };

    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(SmsParseError::NotARequest);
    }
    Ok(version)
}

// accepts -?<1 to max_int_digits digits>[.<1 to 4 digits>] within ±limit
// rust's float parser alone would also accept "+1", "1e2", "inf", and "NaN"
fn parse_coordinate(value: &str, max_int_digits: usize, limit: f32) -> Option<f32>
{
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let (int_part, frac_part) = match unsigned.split_once('.')
    {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (unsigned, None),
    };

    let is_digits = |s: &str, max: usize|
        (1..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());

    if !is_digits(int_part, max_int_digits)
    {
        return None;
    }
    if let Some(frac_part) = frac_part
    {
        if !is_digits(frac_part, 4)
        {
            return None;
        }
    }

    let parsed: f32 = value.parse().ok()?;
    if parsed.abs() > limit
    {
        return None;
    }
    Some(parsed)
}

// flags are exactly 3 hex digits, one bit per ForecastVars field
fn parse_flags(value: &str) -> Option<ForecastVars>
{
    if value.len() != 3 || !value.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let bits = u16::from_str_radix(value, 16).ok()?;
    let bit = |n: u16| bits & (1 << n) != 0;

    // bit order must match the table in docs/sms-format.md
    Some(ForecastVars
    {
        track_alerts: bit(0),
        no_forecast: bit(1),
        avy: bit(2),
        forecast_properties: ForecastProperties
        {
            elevation: bit(3),
            wet_bulb_globe_temperature: bit(4),
            heat_risk: bit(5),
            relative_humidity: bit(6),
            visibility: bit(7),
            ceiling_height: bit(8),
            pressure: bit(9),
            snow_level: bit(10),
            wind_gust: bit(11),
        },
    })
}

#[cfg(test)]
mod tests
{
    use super::*;

    // flags in bit order, written out from the table in docs/sms-format.md
    // so a change to the parser's mapping fails these tests
    fn flags_in_bit_order(vars: &ForecastVars) -> [bool; 12]
    {
        let p = &vars.forecast_properties;
        [
            vars.track_alerts,
            vars.no_forecast,
            vars.avy,
            p.elevation,
            p.wet_bulb_globe_temperature,
            p.heat_risk,
            p.relative_humidity,
            p.visibility,
            p.ceiling_height,
            p.pressure,
            p.snow_level,
            p.wind_gust,
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
        let cases: [(&str, f32, f32, &[usize]); 5] =
        [
            ("TA1 44.0512 -123.0921 000", 44.0512, -123.0921, &[]),
            ("TA1 44.0512 -123.0921 003", 44.0512, -123.0921, &[0, 1]),
            ("TA1 44.0512 -123.0921 845", 44.0512, -123.0921, &[0, 2, 6, 11]),
            ("TA1 39.1968 -120.2354 C81", 39.1968, -120.2354, &[0, 7, 10, 11]),
            ("TA1 44.0512 -123.0921 FFF", 44.0512, -123.0921, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
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
        for n in 0..12
        {
            let message = format!("TA1 44.0 -123.0 {:03X}", 1u16 << n);
            let request = parse(&message).unwrap();
            assert_eq!(set_bits(&request), vec![n], "{message}");
        }
    }

    #[test]
    fn accepts_lenient_input()
    {
        // lowercase prefix and flags
        assert_eq!(set_bits(&parse("ta1 44.0 -123.0 fff").unwrap()).len(), 12);
        // surrounding whitespace and repeated spaces
        let request = parse("  TA1   44.0512  -123.0921 845 \n").unwrap();
        assert_eq!(request.lat, 44.0512);
        // whole-number coordinates and the boundary values
        let request = parse("TA1 -90 180 000").unwrap();
        assert_eq!((request.lat, request.lon), (-90.0, 180.0));
        let request = parse("TA1 90.0000 -180.0000 000").unwrap();
        assert_eq!((request.lat, request.lon), (90.0, -180.0));
    }

    #[test]
    fn rejects_messages_that_are_not_requests()
    {
        for message in ["", "   ", "hello", "Take me home", "TA", "TA 44.0 -123.0 000", "TAx1 44.0 -123.0 000", "44.0 -123.0 000"]
        {
            assert_eq!(parse(message).unwrap_err(), SmsParseError::NotARequest, "{message:?}");
        }
    }

    #[test]
    fn rejects_unsupported_versions()
    {
        assert_eq!(parse("TA2 44.0 -123.0 000").unwrap_err(),
            SmsParseError::UnsupportedVersion("2".to_string()));
        // checked before the field count, since another version may have a different layout
        assert_eq!(parse("TA2 something else entirely here").unwrap_err(),
            SmsParseError::UnsupportedVersion("2".to_string()));
        assert_eq!(parse("TA01 44.0 -123.0 000").unwrap_err(),
            SmsParseError::UnsupportedVersion("01".to_string()));
    }

    #[test]
    fn rejects_wrong_field_count()
    {
        assert_eq!(parse("TA1").unwrap_err(), SmsParseError::WrongFieldCount(1));
        assert_eq!(parse("TA1 44.0 -123.0").unwrap_err(), SmsParseError::WrongFieldCount(3));
        assert_eq!(parse("TA1 44.0 -123.0 000 extra").unwrap_err(), SmsParseError::WrongFieldCount(5));
    }

    #[test]
    fn rejects_invalid_latitude()
    {
        for lat in ["90.0001", "-90.5", "100", "44,0512", "44.05123", "+44.0", "44.", ".5", "4e1", "inf", "NaN", "44.0N", "--44"]
        {
            let message = format!("TA1 {lat} -123.0 000");
            assert_eq!(parse(&message).unwrap_err(),
                SmsParseError::InvalidLatitude(lat.to_string()), "{message}");
        }
    }

    #[test]
    fn rejects_invalid_longitude()
    {
        for lon in ["180.0001", "-181", "1000", "-123,0921", "-123.09214", "+123.0", "123W"]
        {
            let message = format!("TA1 44.0 {lon} 000");
            assert_eq!(parse(&message).unwrap_err(),
                SmsParseError::InvalidLongitude(lon.to_string()), "{message}");
        }
    }

    #[test]
    fn rejects_invalid_flags()
    {
        for flags in ["84", "0845", "G45", "-45", "+45", "8 45", "８４５"]
        {
            let message = format!("TA1 44.0 -123.0 {flags}");
            match parse(&message)
            {
                // "8 45" splits into an extra field
                Err(SmsParseError::WrongFieldCount(5)) if flags.contains(' ') => {}
                result => assert_eq!(result.unwrap_err(),
                    SmsParseError::InvalidFlags(flags.to_string()), "{message}"),
            }
        }
    }
}
