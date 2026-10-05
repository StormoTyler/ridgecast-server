// parser for SMS requests sent by the TrailAlert IOS and Android apps
// the message format is documented in docs/sms-format.md

use std::str::FromStr;
use thiserror::Error;

use geo::Intersects;
use mongodb::bson::doc;
use serde::Serialize;

// local modules
use crate::{APIError, add_alert_tracker};
use crate::database::DB;
use crate::handlers::nws;
use crate::models::{Alert, AvyProperties, Elevation, FloatVec, ForecastProperties,
    ForecastResponse, ForecastVars, GeometryTypes, IntVec, NWSError, Point, Properties,
    UserPoint};

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
        track_avy: bit(2),
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
}

impl RequestedForecast
{
    fn from_properties(properties: Properties, flags: &ForecastProperties) -> Self
    {
        // keeps an optional field only when its flag is set
        fn keep<T>(flag: bool, value: T) -> Option<T>
        {
            flag.then_some(value)
        }

        RequestedForecast
        {
            temperature: properties.temperature,
            max_temperature: properties.max_temperature,
            min_temperature: properties.min_temperature,
            apparent_temperature: properties.apparent_temperature,
            quantitative_precipitation: properties.quantitative_precipitation,
            probability_of_precipitation: properties.probability_of_precipitation,
            snowfall_amount: properties.snowfall_amount,
            ice_accumulation: properties.ice_accumulation,
            wind_direction: properties.wind_direction,
            wind_speed: properties.wind_speed,
            elevation: keep(flags.elevation, properties.elevation),
            // these are already optional because NWS leaves them out for some areas
            wet_bulb_globe_temperature: properties.wet_bulb_globe_temperature
                .filter(|_| flags.wet_bulb_globe_temperature),
            heat_risk: properties.heat_risk.filter(|_| flags.heat_risk),
            relative_humidity: keep(flags.relative_humidity, properties.relative_humidity),
            visibility: keep(flags.visibility, properties.visibility),
            ceiling_height: properties.ceiling_height.filter(|_| flags.ceiling_height),
            pressure: keep(flags.pressure, properties.pressure),
            snow_level: keep(flags.snow_level, properties.snow_level),
            wind_gust: keep(flags.wind_gust, properties.wind_gust),
        }
    }
}

// takes an SmsRequest and returns data for the fields flagged as True
// adds an alerts tracker or avalanche tracker if those fields are True
// returns the avalanche zone's data at any danger level when track_avy is True,
// and only at danger level 3 or higher when it is False
// user is the sender's phone number, which identifies the user
pub async fn get_requested_forecast(request: &SmsRequest, user: &str, db: &DB)
    -> Result<SmsForecastResponse, APIError>
{
    let SmsRequest { lat, lon, forecast_vars } = request;
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
        let forecast = RequestedForecast::from_properties(
            forecast.properties, &forecast_vars.forecast_properties);
        (point, Some(forecast), active_alerts)
    };

    let avy = if forecast_vars.track_avy
    {
        find_avy_zone(&point, db).await?
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

    if forecast_vars.track_alerts || forecast_vars.track_avy
    {
        // one tracker covers both options, its forecast_vars record which are on
        let user_point = UserPoint
        {
            user: user.to_string(),
            forecast_vars: forecast_vars.clone(),
            point: point.clone(),
            // only an avalanche tracker stores its zone
            avy: if forecast_vars.track_avy { avy.clone() } else { None },
            time_added: chrono::Utc::now(),
        };
        if let Err(e) = add_alert_tracker(user_point, db).await
        {
            eprintln!("get_requested_forecast: failed to save tracker: {e}");
            return Err(APIError::DatabaseError);
        }
    }

    Ok(SmsForecastResponse { point, forecast, alerts: active_alerts, avy })
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

// returns the stored avalanche forecast zone containing the point, if any
async fn find_avy_zone(point: &Point, db: &DB) -> Result<Option<AvyProperties>, APIError>
{
    let Some(coordinates) = point.geometry.coordinates
    else
    {
        return Ok(None);
    };

    let avy = match db.avy_collection.find_one(doc! {}).await
    {
        Ok(Some(avy)) => avy,
        Ok(None) => return Err(APIError::NoAvyStored),
        Err(e) =>
        {
            eprintln!("find_avy_zone: failed to read avalanche data from database: {e}");
            return Err(APIError::DatabaseError);
        }
    };

    // intersects, so a point on a zone boundary still matches
    let zone = avy.lookup_map.into_values().find(|area| match &area.geometry
    {
        GeometryTypes::Polygon(value) => value.intersects(&coordinates),
        GeometryTypes::MultiPolygon(value) => value.intersects(&coordinates),
        GeometryTypes::Geojson(_) => false,
    });
    Ok(zone.map(|area| area.properties))
}

// checks if the point is in any avalanche forecast zone
// returns avy_properties for the area if danger_level >= 3
async fn get_avy_no_tracker(point: &Point, db: &DB) -> Result<Option<AvyProperties>, APIError>
{
    let zone = find_avy_zone(point, db).await?;
    // NAC rates danger 1 (low) to 5 (extreme), 3 is considerable
    Ok(zone.filter(|properties| properties.danger_level.is_some_and(|level| level >= 3)))
}
