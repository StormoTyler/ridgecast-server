// parser for SMS requests sent by the RidgeCast IOS and Android apps
// the message format is documented in docs/sms-format.md

use std::str::FromStr;
use thiserror::Error;

use chrono::{DateTime, Days, TimeZone, Utc};

use mongodb::bson::doc;
use serde::Serialize;

// local modules
use crate::{APIError, add_alert_tracker};
use crate::daily_forecast;
use crate::database::DB;
use crate::handlers::{nac, nws};
use crate::handlers::nac::NACError;
use crate::models::{Alert, AvalancheData, AvyProperties, Elevation, FloatVec, ForecastProperties,
    ForecastResponse, ForecastVars, IntVec, NWSError, Point, Properties,
    UserPoint};

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
pub struct SmsRequest
{
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
pub enum SmsParseError
{
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

        // everything after the forecast length is the tracker name
        let [_, lat, lon, flags, duration, length, ref name @ ..] = fields[..]
        else
        {
            return Err(SmsParseError::WrongFieldCount(fields.len()));
        };

        let lat = parse_coordinate(lat, 2, 90.0)
            .ok_or_else(|| SmsParseError::InvalidLatitude(lat.to_string()))?;
        let lon = parse_coordinate(lon, 3, 180.0)
            .ok_or_else(|| SmsParseError::InvalidLongitude(lon.to_string()))?;
        let forecast_vars = parse_flags(flags)
            .ok_or_else(|| SmsParseError::InvalidFlags(flags.to_string()))?;
        // daily_forecast with a duration that ends before the next 6 AM is
        // prevented by the apps (see docs/sms-format.md), not checked here
        let creates_tracker = forecast_vars.track_alerts || forecast_vars.track_avy
            || forecast_vars.daily_forecast;
        let tracker_duration = parse_tracker_duration(duration, creates_tracker)
            .ok_or_else(|| SmsParseError::InvalidTrackerDuration(duration.to_string()))?;
        let forecast_length = parse_forecast_length(length)
            .ok_or_else(|| SmsParseError::InvalidForecastLength(length.to_string()))?;
        let tracker_name = match (creates_tracker, name)
        {
            (true, []) => return Err(SmsParseError::MissingTrackerName),
            (true, words) =>
            {
                let name = words.join(" ");
                match is_valid_tracker_name(&name)
                {
                    true => Some(name),
                    false => return Err(SmsParseError::InvalidTrackerName(name)),
                }
            }
            (false, []) => None,
            // a request without a tracker has exactly 6 fields
            (false, _) => return Err(SmsParseError::WrongFieldCount(fields.len())),
        };

        Ok(SmsRequest { lat, lon, tracker_duration, forecast_length, tracker_name, forecast_vars })
    }
}

// returns the version digits from a "RC<version>" prefix
fn parse_prefix(prefix: &str) -> Result<&str, SmsParseError>
{
    // "RC" must be followed by digits only, so ordinary
    // texts like "RC car for sale" are not read as requests
    let version = match prefix.get(..2)
    {
        Some(rc) if rc.eq_ignore_ascii_case("RC") => &prefix[2..],
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

// hours a tracker created by the request stays active, 1 to 3 digits up to
// MAX_TRACKER_DURATION. 0 means no tracker, so it is rejected when a tracker flag is set
fn parse_tracker_duration(value: &str, creates_tracker: bool) -> Option<u32>
{
    if !(1..=3).contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let hours: u32 = value.parse().ok()?;
    // sets the tracker duration to the maximum duration
    // if the requested tracker exceeds the maximum duration
    if hours > MAX_TRACKER_DURATION
    {
        return Some(168);
    }
    if (creates_tracker && hours == 0)
    {
        return None;
    }
    Some(hours)
}

// days of forecast in the reply, one digit from 1 to MAX_FORECAST_LENGTH
fn parse_forecast_length(value: &str) -> Option<u32>
{
    if value.len() != 1
    {
        return None;
    }
    let days: u32 = value.parse().ok()?;
    (1..=MAX_FORECAST_LENGTH).contains(&days).then_some(days)
}

// a tracker name is 1 to MAX_TRACKER_NAME_LENGTH characters, counting the single
// spaces between words, and uses only letters, digits, and - ' . # & ( ) /,
// which are all in the GSM-7 basic character set
fn is_valid_tracker_name(name: &str) -> bool
{
    (1..=MAX_TRACKER_NAME_LENGTH).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b" -'.#&()/".contains(&b))
}

// flags are exactly 4 hex digits, one bit per ForecastVars field
fn parse_flags(value: &str) -> Option<ForecastVars>
{
    if value.len() != 4 || !value.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let bits = u16::from_str_radix(value, 16).ok()?;
    // unassigned bits are rejected, so an app built for a later
    // version of the bit table is not silently misread
    if bits & !USED_FLAG_BITS != 0
    {
        return None;
    }
    let bit = |n: u16| bits & (1 << n) != 0;

    // bit order must match the table in docs/sms-format.md
    Some(ForecastVars
    {
        track_alerts: bit(0),
        no_forecast: bit(1),
        track_avy: bit(2),
        brief: bit(12),
        daily_forecast: bit(13),
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

// forecast reply for an SMS request: the base forecast fields are always
// included, the optional ones only when their flag is set
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestedForecast
{
    pub temperature: FloatVec,
    pub max_temperature: FloatVec,
    pub min_temperature: FloatVec,
    pub apparent_temperature: FloatVec,
    pub quantitative_precipitation: FloatVec,
    pub probability_of_precipitation: IntVec,
    pub snowfall_amount: FloatVec,
    pub ice_accumulation: FloatVec,
    pub wind_direction: IntVec,
    pub wind_speed: FloatVec,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elevation: Option<Elevation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wet_bulb_globe_temperature: Option<FloatVec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heat_risk: Option<IntVec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_humidity: Option<IntVec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<FloatVec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceiling_height: Option<FloatVec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure: Option<FloatVec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snow_level: Option<FloatVec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wind_gust: Option<FloatVec>,
}

#[derive(Debug, Serialize)]
pub struct SmsForecastResponse
{
    pub point: Point,
    // None when the request set the no_forecast flag
    pub forecast: Option<RequestedForecast>,
    pub alerts: Vec<Alert>,
    // the avalanche zone containing the point, None outside every zone.
    // without track_avy it is only included at danger level 3 or higher
    pub avy: Option<AvyProperties>,
    // explains a missing avy for a track_avy request whose forecast could not be given
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avy_notice: Option<&'static str>,
    // explains why a daily_forecast request did not get a daily forecast tracker
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_notice: Option<&'static str>,
}

impl RequestedForecast
{
    // end is when the last requested forecast day ends, see forecast_end
    fn from_properties(properties: Properties, flags: &ForecastProperties, end: DateTime<Utc>)
        -> Self
    {
        // keeps an optional field only when its flag is set
        fn keep<T>(flag: bool, value: T) -> Option<T>
        {
            flag.then_some(value)
        }
        let floats = |series| trim_floats(series, end);
        let ints = |series| trim_ints(series, end);

        RequestedForecast
        {
            temperature: floats(properties.temperature),
            max_temperature: floats(properties.max_temperature),
            min_temperature: floats(properties.min_temperature),
            apparent_temperature: floats(properties.apparent_temperature),
            quantitative_precipitation: floats(properties.quantitative_precipitation),
            probability_of_precipitation: ints(properties.probability_of_precipitation),
            snowfall_amount: floats(properties.snowfall_amount),
            ice_accumulation: floats(properties.ice_accumulation),
            wind_direction: ints(properties.wind_direction),
            wind_speed: floats(properties.wind_speed),
            elevation: keep(flags.elevation, properties.elevation),
            // these are already optional because NWS leaves them out for some areas
            wet_bulb_globe_temperature: properties.wet_bulb_globe_temperature
                .filter(|_| flags.wet_bulb_globe_temperature).map(floats),
            heat_risk: properties.heat_risk.filter(|_| flags.heat_risk).map(ints),
            relative_humidity: keep(flags.relative_humidity, properties.relative_humidity)
                .map(ints),
            visibility: keep(flags.visibility, properties.visibility).map(floats),
            ceiling_height: properties.ceiling_height.filter(|_| flags.ceiling_height)
                .map(floats),
            pressure: keep(flags.pressure, properties.pressure).map(floats),
            snow_level: keep(flags.snow_level, properties.snow_level).map(floats),
            wind_gust: keep(flags.wind_gust, properties.wind_gust).map(floats),
        }
    }
}

// when the last of `days` forecast days ends: local midnight after the last day,
// counting today as the first day
pub(crate) fn forecast_end<T: TimeZone>(tz: &T, now: DateTime<Utc>, days: u32) -> DateTime<Utc>
{
    let fallback = now + chrono::Duration::days(days.into());
    let Some(last_midnight) = now.with_timezone(tz).date_naive()
        .checked_add_days(Days::new(days.into()))
        .and_then(|date| date.and_hms_opt(0, 0, 0))
    else
    {
        return fallback;
    };
    // earliest() is None only where a daylight saving change skips midnight,
    // which does not happen in US time zones
    tz.from_local_datetime(&last_midnight).earliest()
        .map_or(fallback, |end| end.with_timezone(&Utc))
}

// true when an NWS validTime interval such as "2026-10-06T13:00:00+00:00/PT1H"
// starts before end. a value without a readable time is dropped
fn starts_before(valid_time: &Option<String>, end: DateTime<Utc>) -> bool
{
    valid_time.as_deref()
        .and_then(|time| time.split('/').next())
        .and_then(|start| DateTime::parse_from_rfc3339(start).ok())
        .is_some_and(|start| start < end)
}

// keep the values of a forecast series that start before end
pub(crate) fn trim_floats(mut series: FloatVec, end: DateTime<Utc>) -> FloatVec
{
    series.values.retain(|value| starts_before(&value.valid_time, end));
    series
}

pub(crate) fn trim_ints(mut series: IntVec, end: DateTime<Utc>) -> IntVec
{
    series.values.retain(|value| starts_before(&value.valid_time, end));
    series
}

// takes an SmsRequest and returns data for the fields flagged as True
// adds an alerts, avalanche, or daily forecast tracker if those fields are True
// returns the avalanche zone's data at any danger level when track_avy is True,
// and only at danger level 3 or higher when it is False
// user is the sender's phone number, which identifies the user
pub async fn get_requested_forecast(request: &SmsRequest, user: &str, db: &DB)
    -> Result<SmsForecastResponse, APIError>
{
    let SmsRequest { lat, lon, tracker_duration, forecast_length, tracker_name, forecast_vars } = request;
    let (lat, lon) = (*lat, *lon);

    let alerts = match db.alerts_collection.find_one(doc! {}).await
    {
        Ok(Some(alerts)) => alerts,
        Ok(None) => return Err(APIError::NoAlertsStored),
        Err(e) =>
        {
            eprintln!("get_requested_forecast: failed to read alerts from database: {e}");
            return Err(APIError::DatabaseError);
        }
    };

    let (point, forecast, active_alerts) = if forecast_vars.no_forecast
    {
        // the point is still needed to check alerts and create trackers
        let point = nws::get_point(lat, lon).await
            .map_err(|e| nws_error("get_point", lat, lon, e, APIError::NWSError))?;
        let active_alerts = nws::check_alerts(&alerts, &point);
        (point, None, active_alerts)
    }
    else
    {
        let ForecastResponse { point, forecast, alerts: active_alerts } =
            nws::get_forecast(lat, lon, &alerts).await
            .map_err(|e| nws_error("get_forecast", lat, lon, e, APIError::ForecastError))?;
        // forecast days are local days at the location. NWS reports a time zone for
        // every point it covers, so UTC days are only a fallback
        let now = Utc::now();
        let end = match daily_forecast::point_time_zone(&point)
        {
            Some(tz) => forecast_end(&tz, now, *forecast_length),
            None => forecast_end(&Utc, now, *forecast_length),
        };
        // TODO: send the abbreviated forecast when forecast_vars.brief is set
        let forecast = RequestedForecast::from_properties(
            forecast.properties, &forecast_vars.forecast_properties, end);
        (point, Some(forecast), active_alerts)
    };

    let mut avy_notice = None;
    let track_avy = forecast_vars.track_avy;
    let avy = if forecast_vars.track_avy
    {
        let checked = match load_avy(db).await
        {
            Ok(avy_data) => nac::check_avy_geometry(&point, &avy_data).await,
            // no stored data or a database error, reported below as a failed check
            Err(_) => Err(NACError::CheckError),
        };
        match checked
        {
            Ok(zone) => zone,
            // the zone is still tracked with no stored forecast,
            // so its next forecast is reported as new
            Err(NACError::NoForecastError) =>
            {
                avy_notice = Some("There is no current avalanche forecast for this location.");
                None
            }
            // the zone is still tracked, so the first forecast
            // after its season starts is reported as new
            Err(NACError::OffSeasonError) =>
            {
                // TODO: when a tracker is created, notify the user that the zone is in
                // its off season and that it will still be tracked
                avy_notice = Some("The avalanche center for this location is in its off season.");
                None
            }
            // avalanche data could not be read
            Err(e) =>
            {
                eprintln!("get_requested_forecast: {e}");
                avy_notice = Some("Could not retrieve avalanche forecast data at this time.");
                None
            }
        }
    }
    else
    {
        // the user did not ask about avalanches, so missing avalanche
        // data should not fail the rest of the request
        match get_avy_no_tracker(&point, db).await
        {
            Err(APIError::NoAvyStored) => None,
            result => result?,
        }
    };

    let time_added = chrono::Utc::now();
    let expires = time_added + chrono::Duration::hours((*tracker_duration).into());
    let mut daily_notice = None;
    // the first daily forecast, None when the request has no daily forecast
    let next_daily_forecast = if forecast_vars.daily_forecast
    {
        match daily_forecast::point_time_zone(&point)
        {
            // None when the tracker expires before the next 6 AM. the apps do not
            // send such a request (see docs/sms-format.md)
            Some(tz) => daily_forecast::next_daily_forecast(tz, time_added, expires),
            None =>
            {
                eprintln!("get_requested_forecast: no usable time zone for {lat},{lon}: {:?}",
                    point.properties.time_zone);
                daily_notice = Some("Daily forecasts are not available for this location.");
                None
            }
        }
    }
    else
    {
        None
    };
    // a tracker that would never send a daily forecast does not track it
    let daily_forecast = next_daily_forecast.is_some();

    if forecast_vars.track_alerts || track_avy || daily_forecast
    {
        // one tracker covers every option, its forecast_vars record which are on
        let user_point = UserPoint
        {
            user: user.to_string(),
            tracker_name: tracker_name.clone(),
            forecast_vars: ForecastVars { track_avy, daily_forecast, ..forecast_vars.clone() },
            point: point.clone(),
            // only an avalanche tracker stores its zone's forecast
            avy: if track_avy { avy.clone() } else { None },
            time_added,
            valid_duration: *tracker_duration,
            next_daily_forecast,
        };
        if let Err(e) = add_alert_tracker(user_point, db).await
        {
            eprintln!("get_requested_forecast: failed to save tracker: {e}");
            return Err(APIError::DatabaseError);
        }
    }

    Ok(SmsForecastResponse { point, forecast, alerts: active_alerts, avy, avy_notice, daily_notice })
}

// maps an NWS error the same way the route handlers do
fn nws_error(call: &str, lat: f32, lon: f32, e: NWSError, fallback: APIError) -> APIError
{
    if e.is_not_found()
    {
        return APIError::LocationNotCovered;
    }
    eprintln!("get_requested_forecast: {call} failed for {lat},{lon}: {e}");
    if e.is_timeout() { APIError::UpstreamTimeout } else { fallback }
}

// reads the stored avalanche zones
async fn load_avy(db: &DB) -> Result<AvalancheData, APIError>
{
    match db.avy_collection.find_one(doc! {}).await
    {
        Ok(Some(avy)) => Ok(avy),
        Ok(None) => Err(APIError::NoAvyStored),
        Err(e) =>
        {
            eprintln!("load_avy: failed to read avalanche data from database: {e}");
            Err(APIError::DatabaseError)
        }
    }
}

// returns the forecast of the stored avalanche zone containing the point, if any
async fn find_avy_zone(point: &Point, db: &DB) -> Result<Option<AvyProperties>, APIError>
{
    let avy = load_avy(db).await?;

    match nac::check_avy_geometry(point, &avy).await
    {
        Ok(zone) => Ok(zone),
        // no zone, or a zone without a current forecast, is reported as no zone
        Err(e) =>
        {
            eprintln!("find_avy_zone: {e}");
            Ok(None)
        }
    }
}

// checks if the point is in any avalanche forecast zone
// returns the avy_properties for the area if danger_level >= 3
async fn get_avy_no_tracker(point: &Point, db: &DB) -> Result<Option<AvyProperties>, APIError>
{
    let zone = find_avy_zone(point, db).await?;
    // NAC rates danger 1 (low) to 5 (extreme), 3 is considerable
    Ok(zone.filter(|properties| properties.danger_level.is_some_and(|level| level >= 3)))
}
