// builds the response to an SMS request: the point, the requested forecast,
// active alerts, the avalanche zone, and any tracker the request creates.
// sms::reply turns the response into text messages

use chrono::{DateTime, Days, TimeZone, Utc};

use serde::Serialize;

// local modules
use crate::daily_forecast;
use crate::database;
use crate::errors;
use crate::messaging;
use crate::models::{
    Alert, AlertsSorted, AvalancheData, AvyProperties, Elevation, FloatVec, ForecastProperties,
    ForecastResponse, ForecastVars, IntVec, Point, Properties, Snapshot, UserPoint,
};
use crate::sms::request;
use crate::snapshots;
use crate::upstream::{nac, nws};

// forecast reply for an SMS request: the base forecast fields are always
// included, the optional ones only when their flag is set
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestedForecast {
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

// the notices record what happened, sms::reply picks their wording for the
// full or brief format
#[derive(Debug, Serialize)]
pub struct SmsForecastResponse {
    pub point: Point,
    // None when the request set the no_forecast flag
    pub forecast: Option<RequestedForecast>,
    pub alerts: Vec<Alert>,
    // set when no alerts could be loaded, so alerts is empty because they
    // weren't checked, not because there are none
    pub alerts_notice: bool,
    // set when the alerts are older than snapshots::ALERTS_MAX_AGE because
    // they could not be refreshed from NWS
    pub alerts_stale_notice: bool,
    // the avalanche zone containing the point, None outside every zone.
    // without track_avy it is only included at danger level 3 or higher
    pub avy: Option<AvyProperties>,
    // explains a missing avy for a track_avy request whose forecast could not be given,
    // or for any request whose avalanche data could not be read
    pub avy_notice: Option<AvyNotice>,
    // set when the avalanche data is older than snapshots::AVY_MAX_AGE because
    // it could not be refreshed from NAC
    pub avy_stale_notice: bool,
    // set when a daily_forecast request did not get a daily forecast tracker
    pub daily_notice: bool,
    // None when the request asked for no tracker
    pub tracker: Option<TrackerStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum AvyNotice {
    // the zone has no valid forecast
    NoCurrentForecast,
    // the zone's center is in its off season
    OffSeason,
    // the avalanche data could not be read
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum TrackerStatus {
    Created(CreatedTracker),
    // the tracker could not be saved, even after a retry
    NotCreated,
}

// what the reply's tracker line tells the user
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreatedTracker {
    pub name: String,
    pub alerts: bool,
    pub avy: bool,
    pub daily_forecast: bool,
    pub ends: DateTime<Utc>,
}

impl RequestedForecast {
    // end is when the last requested forecast day ends, see forecast_end
    fn from_properties(
        properties: Properties,
        flags: &ForecastProperties,
        end: DateTime<Utc>,
    ) -> Self {
        // keeps an optional field only when its flag is set
        fn keep<T>(flag: bool, value: T) -> Option<T> {
            flag.then_some(value)
        }
        let floats = |series| trim_floats(series, end);
        let ints = |series| trim_ints(series, end);

        RequestedForecast {
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
            wet_bulb_globe_temperature: properties
                .wet_bulb_globe_temperature
                .filter(|_| flags.wet_bulb_globe_temperature)
                .map(floats),
            heat_risk: properties.heat_risk.filter(|_| flags.heat_risk).map(ints),
            relative_humidity: keep(flags.relative_humidity, properties.relative_humidity)
                .map(ints),
            visibility: keep(flags.visibility, properties.visibility).map(floats),
            ceiling_height: properties
                .ceiling_height
                .filter(|_| flags.ceiling_height)
                .map(floats),
            pressure: keep(flags.pressure, properties.pressure).map(floats),
            snow_level: keep(flags.snow_level, properties.snow_level).map(floats),
            wind_gust: keep(flags.wind_gust, properties.wind_gust).map(floats),
        }
    }
}

// when the last of `days` forecast days ends: local midnight after the last day,
// counting today as the first day
pub(crate) fn forecast_end<T: TimeZone>(tz: &T, now: DateTime<Utc>, days: u32) -> DateTime<Utc> {
    let fallback = now + chrono::Duration::days(days.into());
    let Some(last_midnight) = now
        .with_timezone(tz)
        .date_naive()
        .checked_add_days(Days::new(days.into()))
        .and_then(|date| date.and_hms_opt(0, 0, 0))
    else {
        return fallback;
    };
    // earliest() is None only where a daylight saving change skips midnight,
    // which does not happen in US time zones
    tz.from_local_datetime(&last_midnight)
        .earliest()
        .map_or(fallback, |end| end.with_timezone(&Utc))
}

// true when an NWS validTime interval such as "2026-10-06T13:00:00+00:00/PT1H"
// starts before end. a value without a readable time is dropped
fn starts_before(valid_time: &Option<String>, end: DateTime<Utc>) -> bool {
    valid_time
        .as_deref()
        .and_then(|time| time.split('/').next())
        .and_then(|start| DateTime::parse_from_rfc3339(start).ok())
        .is_some_and(|start| start < end)
}

// keep the values of a forecast series that start before end
pub(crate) fn trim_floats(mut series: FloatVec, end: DateTime<Utc>) -> FloatVec {
    series
        .values
        .retain(|value| starts_before(&value.valid_time, end));
    series
}

pub(crate) fn trim_ints(mut series: IntVec, end: DateTime<Utc>) -> IntVec {
    series
        .values
        .retain(|value| starts_before(&value.valid_time, end));
    series
}

// takes an request::SmsRequest and returns data for the fields flagged as True
// adds an alerts, avalanche, or daily forecast tracker if those fields are True
// returns the avalanche zone's data at any danger level when track_avy is True,
// and only at danger level 3 or higher when it is False
// user is the sender's phone number, which identifies the user. reply_from is the
// server number they texted, which the tracker stores to send its messages from.
// now is read once by the caller and also given to sms::reply::format_reply, so the
// forecast's local days, the avalanche check, the tracker's times, and the reply all
// agree, even when the request arrives just before local midnight
pub async fn get_requested_forecast(
    request: &request::SmsRequest,
    user: &messaging::PhoneNumber,
    reply_from: &messaging::PhoneNumber,
    db: &database::DB,
    now: DateTime<Utc>,
) -> Result<SmsForecastResponse, errors::APIError> {
    let request::SmsRequest {
        lat,
        lon,
        tracker_duration,
        forecast_length,
        tracker_name,
        forecast_vars,
    } = request;
    let (lat, lon) = (*lat, *lon);

    // alerts that can't be loaded don't fail the request, the forecast is still sent.
    // the user is told the alerts couldn't be checked, so no alert lines
    // aren't read as no active alerts
    let mut alerts_notice = false;
    let mut alerts_stale_notice = false;
    let alerts = match snapshots::load_alerts(db).await {
        Ok(Snapshot { data, stale }) => {
            alerts_stale_notice = stale;
            data
        }
        Err(e) => {
            eprintln!("get_requested_forecast: alerts unavailable: {e}");
            alerts_notice = true;
            // matches no point, so active_alerts is empty
            AlertsSorted::default()
        }
    };

    let (point, forecast, active_alerts) = if forecast_vars.no_forecast {
        // the point is still needed to check alerts and create trackers
        let point = nws::get_point(lat, lon)
            .await
            .map_err(|e| nws_error("get_point", lat, lon, e, errors::APIError::NWSError))?;
        let active_alerts = nws::check_alerts(&alerts, &point);
        (point, None, active_alerts)
    } else {
        let ForecastResponse {
            point,
            forecast,
            alerts: active_alerts,
        } = nws::get_forecast(lat, lon, &alerts)
            .await
            .map_err(|e| nws_error("get_forecast", lat, lon, e, errors::APIError::ForecastError))?;
        // forecast days are local days at the location. NWS reports a time zone for
        // every point it covers, so UTC days are only a fallback
        let end = match daily_forecast::point_time_zone(&point) {
            Some(tz) => forecast_end(&tz, now, *forecast_length),
            None => forecast_end(&Utc, now, *forecast_length),
        };
        let forecast =
            RequestedForecast::from_properties(forecast, &forecast_vars.forecast_properties, end);
        (point, Some(forecast), active_alerts)
    };

    let mut avy_notice = None;
    let mut avy_stale_notice = false;
    let track_avy = forecast_vars.track_avy;
    // the user may not have asked about avalanches, so unavailable avalanche
    // data should not fail the rest of the request
    let avy_data = match snapshots::load_avy(db).await {
        Ok(Snapshot { data, stale }) => {
            // sent with or without track_avy, since stale data could miss
            // a danger level of 3 or higher
            avy_stale_notice = stale;
            Some(data)
        }
        Err(e) => {
            eprintln!("get_requested_forecast: avalanche data unavailable: {e}");
            None
        }
    };
    let avy = match &avy_data {
        // the notice is sent with or without track_avy, since
        // danger level 3 or higher could not be ruled out
        None => {
            avy_notice = Some(AvyNotice::Unavailable);
            None
        }
        Some(avy_data) if track_avy => match nac::check_avy_geometry(&point, avy_data, now).await {
            nac::ZoneForecast::Current(zone) => Some(zone.clone()),
            nac::ZoneForecast::NoZone => None,
            // the zone is still tracked with no stored forecast,
            // so its next forecast is reported as new
            nac::ZoneForecast::NoForecast | nac::ZoneForecast::Expired => {
                avy_notice = Some(AvyNotice::NoCurrentForecast);
                None
            }
            // the zone is still tracked, so the first forecast
            // after its season starts is reported as new
            nac::ZoneForecast::OffSeason => {
                avy_notice = Some(AvyNotice::OffSeason);
                None
            }
        },
        Some(avy_data) => get_avy_no_tracker(&point, avy_data, now).await,
    };

    let time_added = now;
    let expires = time_added + chrono::Duration::hours((*tracker_duration).into());
    let mut daily_notice = false;
    // the first daily forecast, None when the request has no daily forecast
    let next_daily_forecast = if forecast_vars.daily_forecast {
        match daily_forecast::point_time_zone(&point) {
            // None when the tracker expires before the next 6 AM. the apps do not
            // send such a request (see docs/sms-format.md)
            Some(tz) => daily_forecast::next_daily_forecast(tz, time_added, expires),
            None => {
                eprintln!(
                    "get_requested_forecast: no usable time zone for {lat},{lon}: {:?}",
                    point.properties.time_zone
                );
                daily_notice = true;
                None
            }
        }
    } else {
        None
    };
    // a tracker that would never send a daily forecast does not track it
    let daily_forecast = next_daily_forecast.is_some();

    let mut tracker = None;
    if forecast_vars.track_alerts || track_avy || daily_forecast {
        // one tracker covers every option, its forecast_vars record which are on
        let user_point = UserPoint {
            id: None,
            user: user.clone(),
            reply_from: reply_from.clone(),
            tracker_name: tracker_name.clone(),
            forecast_vars: ForecastVars {
                track_avy,
                daily_forecast,
                ..forecast_vars.clone()
            },
            point: point.clone(),
            // only an avalanche tracker stores its zone's forecast
            avy: if track_avy { avy.clone() } else { None },
            avy_lost: Default::default(),
            avy_off_season: Default::default(),
            time_added,
            valid_duration: *tracker_duration,
            next_daily_forecast,
        };
        // the forecast is the safety information, so it is sent even if the
        // tracker can't be saved. the save is retried once, then the user is told
        let name = tracker_name.as_deref().unwrap_or("without name");
        let mut saved = database::add_tracker(&user_point, db).await;
        if let Err(e) = &saved {
            eprintln!("get_requested_forecast: failed to save tracker {name}, retrying once: {e}");
            saved = database::add_tracker(&user_point, db).await;
        }
        tracker = Some(match saved {
            Ok(()) => TrackerStatus::Created(CreatedTracker {
                // a request that creates a tracker always has a name
                name: tracker_name.clone().unwrap_or_default(),
                alerts: forecast_vars.track_alerts,
                avy: track_avy,
                daily_forecast,
                ends: expires,
            }),
            Err(e) => {
                eprintln!(
                    "get_requested_forecast: retry failed, tracker {name} was not created: {e}"
                );
                TrackerStatus::NotCreated
            }
        });
    }

    Ok(SmsForecastResponse {
        point,
        forecast,
        alerts: active_alerts,
        alerts_notice,
        alerts_stale_notice,
        avy,
        avy_notice,
        avy_stale_notice,
        daily_notice,
        tracker,
    })
}

// maps an NWS error the same way the route handlers do
fn nws_error(
    call: &str,
    lat: f32,
    lon: f32,
    e: nws::NWSError,
    fallback: errors::APIError,
) -> errors::APIError {
    if e.is_not_found() {
        return errors::APIError::LocationNotCovered;
    }
    eprintln!("get_requested_forecast: {call} failed for {lat},{lon}: {e}");
    if e.is_timeout() {
        errors::APIError::UpstreamTimeout
    } else {
        fallback
    }
}

// checks if the point is in any avalanche forecast zone
// returns the avy_properties for the area if danger_level >= 3
async fn get_avy_no_tracker(
    point: &Point,
    avy_data: &AvalancheData,
    now: DateTime<Utc>,
) -> Option<AvyProperties> {
    match nac::check_avy_geometry(point, avy_data, now).await {
        // NAC rates danger 1 (low) to 5 (extreme), 3 is considerable
        nac::ZoneForecast::Current(zone) => Some(zone)
            .filter(|properties| properties.danger_level.is_some_and(|level| level >= 3))
            .cloned(),
        // without track_avy, a zone without a valid forecast isn't reported
        nac::ZoneForecast::NoZone
        | nac::ZoneForecast::OffSeason
        | nac::ZoneForecast::NoForecast
        | nac::ZoneForecast::Expired => None,
    }
}
