// parser for SMS requests sent by the RidgeCast IOS and Android apps
// the message format is documented in docs/sms-format.md

use std::str::FromStr;
use thiserror::Error;

// local modules
use crate::models::{ForecastProperties, ForecastVars};

const SUPPORTED_VERSION: &str = "1";
// one week
const MAX_TRACKER_DURATION: u32 = 168;
// bits 0 to 13 of the flags field are assigned
const USED_FLAG_BITS: u16 = 0x3FFF;
// longest forecast in the reply to a request, in days
const MAX_FORECAST_LENGTH: u32 = 7;
// longest tracker name, in characters
const MAX_TRACKER_NAME_LENGTH: usize = 10;

#[derive(Debug, Clone)]
pub struct SmsRequest {
    pub lat: f32,
    pub lon: f32,
    // time in hours that trackers created by this request remain active
    pub tracker_duration: u32,
    // number of days forecast that is received on initial request
    // 1 to 7, always sent by the apps
    pub forecast_length: u32,
    // name of the tracker created by the request, shown in its messages
    // Some when a tracker flag is set, None otherwise
    pub tracker_name: Option<String>,
    // which optional forecast fields are used for a given forecast
    pub forecast_vars: ForecastVars,
}

#[derive(Debug, Error, PartialEq)]
pub enum SmsParseError {
    // the message is not a RidgeCast request at all
    #[error("message is not a RidgeCast request")]
    NotARequest,
    #[error("unsupported message format version {0}")]
    UnsupportedVersion(String),
    #[error("expected 6 fields, found {0}")]
    WrongFieldCount(usize),
    #[error("invalid latitude: {0}")]
    InvalidLatitude(String),
    #[error("invalid longitude: {0}")]
    InvalidLongitude(String),
    #[error("invalid flags: {0}")]
    InvalidFlags(String),
    #[error("invalid tracker duration: {0}")]
    InvalidTrackerDuration(String),
    #[error("invalid forecast length: {0}")]
    InvalidForecastLength(String),
    #[error("a tracker flag is set but the tracker name is missing")]
    MissingTrackerName,
    #[error("invalid tracker name: {0}")]
    InvalidTrackerName(String),
}

impl FromStr for SmsRequest {
    type Err = SmsParseError;

    fn from_str(message: &str) -> Result<Self, Self::Err> {
        // split_whitespace also trims and allows repeated spaces
        let fields: Vec<&str> = message.split_whitespace().collect();

        // the prefix is checked before the field count, so a future
        // version with a different layout reports the version instead
        let version = match fields.first() {
            Some(prefix) => parse_prefix(prefix)?,
            None => return Err(SmsParseError::NotARequest),
        };
        if version != SUPPORTED_VERSION {
            return Err(SmsParseError::UnsupportedVersion(version.to_string()));
        }

        // everything after the forecast length is the tracker name
        let [_, lat, lon, flags, duration, length, ref name @ ..] = fields[..] else {
            return Err(SmsParseError::WrongFieldCount(fields.len()));
        };

        let lat = parse_coordinate(lat, 2, 90.0)
            .ok_or_else(|| SmsParseError::InvalidLatitude(lat.to_string()))?;
        let lon = parse_coordinate(lon, 3, 180.0)
            .ok_or_else(|| SmsParseError::InvalidLongitude(lon.to_string()))?;
        let forecast_vars =
            parse_flags(flags).ok_or_else(|| SmsParseError::InvalidFlags(flags.to_string()))?;
        // daily_forecast with a duration that ends before the next 6 AM is
        // prevented by the apps (see docs/sms-format.md), not checked here
        let creates_tracker =
            forecast_vars.track_alerts || forecast_vars.track_avy || forecast_vars.daily_forecast;
        let tracker_duration = parse_tracker_duration(duration, creates_tracker)
            .ok_or_else(|| SmsParseError::InvalidTrackerDuration(duration.to_string()))?;
        let forecast_length = parse_forecast_length(length)
            .ok_or_else(|| SmsParseError::InvalidForecastLength(length.to_string()))?;
        let tracker_name = match (creates_tracker, name) {
            (true, []) => return Err(SmsParseError::MissingTrackerName),
            (true, words) => {
                let name = words.join(" ");
                match is_valid_tracker_name(&name) {
                    true => Some(name),
                    false => return Err(SmsParseError::InvalidTrackerName(name)),
                }
            }
            (false, []) => None,
            // a request without a tracker has exactly 6 fields
            (false, _) => return Err(SmsParseError::WrongFieldCount(fields.len())),
        };

        Ok(SmsRequest {
            lat,
            lon,
            tracker_duration,
            forecast_length,
            tracker_name,
            forecast_vars,
        })
    }
}

// returns the version digits from a "RC<version>" prefix
fn parse_prefix(prefix: &str) -> Result<&str, SmsParseError> {
    // "RC" must be followed by digits only, so ordinary
    // texts like "RC car for sale" are not read as requests
    let version = match prefix.get(..2) {
        Some(rc) if rc.eq_ignore_ascii_case("RC") => &prefix[2..],
        _ => return Err(SmsParseError::NotARequest),
    };

    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SmsParseError::NotARequest);
    }
    Ok(version)
}

// accepts -?<1 to max_int_digits digits>[.<1 to 4 digits>] within ±limit
// rust's float parser alone would also accept "+1", "1e2", "inf", and "NaN"
fn parse_coordinate(value: &str, max_int_digits: usize, limit: f32) -> Option<f32> {
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let (int_part, frac_part) = match unsigned.split_once('.') {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (unsigned, None),
    };

    let is_digits =
        |s: &str, max: usize| (1..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());

    if !is_digits(int_part, max_int_digits) {
        return None;
    }
    if let Some(frac_part) = frac_part
        && !is_digits(frac_part, 4)
    {
        return None;
    }

    let parsed: f32 = value.parse().ok()?;
    if parsed.abs() > limit {
        return None;
    }
    Some(parsed)
}

// hours a tracker created by the request stays active, 1 to 3 digits up to
// MAX_TRACKER_DURATION. 0 means no tracker, so it is rejected when a tracker flag is set
fn parse_tracker_duration(value: &str, creates_tracker: bool) -> Option<u32> {
    if !(1..=3).contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: u32 = value.parse().ok()?;
    // sets the tracker duration to the maximum duration
    // if the requested tracker exceeds the maximum duration
    if hours > MAX_TRACKER_DURATION {
        return Some(168);
    }
    if creates_tracker && hours == 0 {
        return None;
    }
    Some(hours)
}

// days of forecast in the reply, one digit from 1 to MAX_FORECAST_LENGTH
fn parse_forecast_length(value: &str) -> Option<u32> {
    if value.len() != 1 {
        return None;
    }
    let days: u32 = value.parse().ok()?;
    (1..=MAX_FORECAST_LENGTH).contains(&days).then_some(days)
}

// a tracker name is 1 to MAX_TRACKER_NAME_LENGTH characters, counting the single
// spaces between words, and uses only letters, digits, and - ' . # & ( ) /,
// which are all in the GSM-7 basic character set
fn is_valid_tracker_name(name: &str) -> bool {
    (1..=MAX_TRACKER_NAME_LENGTH).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" -'.#&()/".contains(&b))
}

// flags are exactly 4 hex digits, one bit per ForecastVars field
fn parse_flags(value: &str) -> Option<ForecastVars> {
    if value.len() != 4 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bits = u16::from_str_radix(value, 16).ok()?;
    // unassigned bits are rejected, so an app built for a later
    // version of the bit table is not silently misread
    if bits & !USED_FLAG_BITS != 0 {
        return None;
    }
    let bit = |n: u16| bits & (1 << n) != 0;

    // bit order must match the table in docs/sms-format.md
    Some(ForecastVars {
        track_alerts: bit(0),
        no_forecast: bit(1),
        track_avy: bit(2),
        brief: bit(12),
        daily_forecast: bit(13),
        forecast_properties: ForecastProperties {
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
