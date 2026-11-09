use chrono::{DateTime, Duration, FixedOffset, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;

// local modules
use crate::messaging;
use crate::tracking;

// user data models
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserPoint {
    // the MongoDB _id, so a tracker can be saved back after it is checked.
    // None until the tracker is inserted, then MongoDB assigns it
    #[serde(rename = "_id", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<bson::oid::ObjectId>,
    // the user's phone number, which identifies them and receives the tracker's
    // messages. validated when read, so a stored number is always E.164
    pub user: messaging::PhoneNumber,
    // the server number the user texted to create the tracker, which every
    // message the tracker sends comes from, so the conversation never switches
    // numbers (a satellite messenger's gateway may not deliver from another)
    pub reply_from: messaging::PhoneNumber,
    // name the user gave the tracker, shown in its messages. None for a
    // UserPoint that is not a tracker
    #[serde(default)]
    pub tracker_name: Option<String>,
    pub forecast_vars: ForecastVars,
    pub point: Point,
    // the last valid avalanche forecast the user was sent
    pub avy: Option<AvyProperties>,
    // set once the user is told avy is no longer valid, so the loss is
    // notified once. trackers stored before this field read it as not raised
    #[serde(default)]
    pub avy_lost: tracking::NoticeFlag,
    // set once the user is told the center entered its off season. kept apart from
    // avy_lost, so the off season notice is still sent after any other kind of loss
    #[serde(default)]
    pub avy_off_season: tracking::NoticeFlag,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub time_added: DateTime<Utc>,
    pub valid_duration: u32,
    // when a daily_forecast tracker sends its next forecast, None for other
    // trackers and once no 6 AM is left before the tracker expires
    #[serde(
        default,
        with = "bson::serde_helpers::chrono_datetime_as_bson_datetime_optional"
    )]
    pub next_daily_forecast: Option<DateTime<Utc>>,
}

impl UserPoint {
    // the tracker stops valid_duration hours after it was added
    // unused until the scheduler checks tracker expiry, remove the allow then
    #[allow(dead_code)]
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.time_added + Duration::hours(self.valid_duration.into())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
// data structure for parsing SMS messages from the RidgeCast
// IOS and Android apps
pub struct ForecastVars {
    pub track_alerts: bool,
    pub no_forecast: bool,
    // create an avalanche danger tracker for this location
    pub track_avy: bool,
    pub brief: bool,
    // create a tracker that sends a forecast at 6 AM local time each day while it is active
    pub daily_forecast: bool,
    pub forecast_properties: ForecastProperties,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
// optional weather forecast categories
pub struct ForecastProperties {
    pub elevation: bool,
    pub wet_bulb_globe_temperature: bool,
    pub heat_risk: bool,
    pub relative_humidity: bool,
    pub visibility: bool,
    pub ceiling_height: bool,
    pub pressure: bool,
    pub snow_level: bool,
    pub wind_gust: bool,
}

// end user data models

// data structures for checking and responding to trackers

// unused until the scheduler builds it, remove the allow then
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
pub struct TrackerResponse {
    pub user: UserPoint,
    pub alerts: Option<Vec<Alert>>,
    pub avy: Option<AvyProperties>,
    pub daily: Option<Properties>,
}

// data structures for nws points endpoint

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Point {
    pub properties: PointProperties,
    pub id: Option<String>,
    pub geometry: PointGeometry,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PointGeometry {
    pub coordinates: geo::Point,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PointProperties {
    // no need for option
    // if the value is None
    // there is an error with the json recieved from NWS
    pub cwa: String,
    pub grid_x: i32,
    pub grid_y: i32,
    pub nwr: Nwr,
    // This one is an option
    // because the forecast is still useful without it
    pub astronomical_data: Option<AstronomicalData>,
    // IANA time zone name such as "America/Los_Angeles", used to send
    // daily forecasts at local time. an option for the same reason
    pub time_zone: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Nwr {
    // not option because I dont want to return a forecast
    // if I cant check alerts
    pub same_code: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AstronomicalData {
    pub sunrise: Option<chrono::DateTime<FixedOffset>>,
    pub sunset: Option<chrono::DateTime<FixedOffset>>,
}

// data structures for deserizlizing nws alerts endpoint json
#[derive(Debug, Serialize, Deserialize)]
pub struct Alerts {
    pub features: Vec<Alert>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Alert {
    // NWS alert id, used to tell which alerts a user was already sent
    pub id: Option<String>,
    pub geometry: Option<GeometryTypes>,
    pub properties: AlertData,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AlertData {
    pub area_desc: Option<String>,
    pub effective: Option<chrono::DateTime<FixedOffset>>,
    pub ends: Option<chrono::DateTime<FixedOffset>>,
    pub severity: Option<String>,
    pub certainty: Option<String>,
    pub urgency: Option<String>,
    pub event: Option<String>,
    pub headline: Option<String>,
    pub instruction: Option<String>,
    // a null geocode is read as empty, so the alert is kept and
    // logged by nws::parse_alerts instead of failing to deserialize
    #[serde(default, deserialize_with = "null_as_default")]
    pub geocode: Geocode,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Geocode {
    // a null SAME list is read as empty, for the same reason as AlertData.geocode
    #[serde(rename = "SAME", default, deserialize_with = "null_as_default")]
    pub same: Vec<String>,
}

impl Geocode {
    // NWS specifies SAME codes as matching ^\d{6}$
    pub fn is_valid_same_code(code: &str) -> bool {
        code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit())
    }
}

// reads a null value as the type's default instead of failing
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

// an alert that failed validation in nws::parse_alerts, stored in the
// invalid_alerts collection for debugging
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InvalidAlert {
    // the NWS alert id, which identifies the record across alert refreshes
    pub alert_id: Option<String>,
    pub reasons: Vec<InvalidAlertReason>,
    // the SAME codes that didn't match ^\d{6}$
    pub invalid_same_codes: Vec<String>,
    // why the alert's geometry can't be used, if it can't
    pub geometry_error_reason: Option<GeometryErrorReason>,
    pub geometry_error: Option<String>,
    // the alert as received from NWS, before invalid SAME codes and geometry were removed
    pub alert: Alert,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub first_seen: DateTime<Utc>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InvalidAlertReason {
    // at least one SAME code didn't match ^\d{6}$
    InvalidSameCodes,
    // no SAME code was left after invalid ones were removed
    NoSameCode,
    // the geometry can't be used to match a point, so it was removed
    InvalidGeometry,
}

// data structures for utilizing nws alerts data
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AlertsSorted {
    // alerts with geometry
    pub with_geometry: Vec<Alert>,
    // alerts without geometry
    pub by_same_code: HashMap<String, Vec<Alert>>,
    // alerts with neither usable geometry nor a SAME code, which can't be
    // matched to a location. kept so they are visible instead of dropped
    #[serde(default)]
    pub unmatched: Vec<Alert>,
    // when the alerts were fetched from NWS. defaults to the Unix epoch, so a
    // snapshot stored without it is treated as stale
    #[serde(
        default,
        with = "bson::serde_helpers::chrono_datetime_as_bson_datetime"
    )]
    pub fetched_at: DateTime<Utc>,
}

impl AlertsSorted {
    pub fn add_geometry_alert(&mut self, alert: &Alert) {
        self.with_geometry.push(alert.clone());
    }

    pub fn add_same_code_alert(&mut self, same_code: String, alert: &Alert) {
        self.by_same_code
            .entry(same_code)
            .or_default()
            .push(alert.clone());
    }

    pub fn get_same_code_alerts(&self, same_code: &str) -> &[Alert] {
        self.by_same_code
            .get(same_code)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

// data structures for nws gridpoints endpoint
#[derive(Debug, Serialize, Deserialize)]
pub struct Forecast {
    // if not properties, there is an issue
    pub properties: Properties,
    // remove later
    pub id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Properties {
    pub elevation: Elevation,
    pub temperature: FloatVec,
    pub max_temperature: FloatVec,
    pub min_temperature: FloatVec,
    pub apparent_temperature: FloatVec,
    pub wet_bulb_globe_temperature: Option<FloatVec>,
    pub heat_risk: Option<IntVec>,
    pub relative_humidity: IntVec,
    pub visibility: FloatVec,
    pub ceiling_height: Option<FloatVec>,
    pub pressure: FloatVec,
    pub quantitative_precipitation: FloatVec,
    pub probability_of_precipitation: IntVec,
    pub snowfall_amount: FloatVec,
    pub snow_level: FloatVec,
    pub ice_accumulation: FloatVec,
    pub wind_direction: IntVec,
    pub wind_speed: FloatVec,
    pub wind_gust: FloatVec,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Elevation {
    pub value: Option<f32>,
}

// shared structures
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntVec {
    // the unit code, such as "wmoUnit:percent". NWS leaves it out for
    // some series, such as heatRisk and pressure
    #[serde(default)]
    pub uom: Option<String>,
    pub values: Vec<TimeInt>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeInt {
    pub valid_time: Option<String>,
    pub value: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatVec {
    // the unit code, such as "wmoUnit:degC". NWS leaves it out for some series
    #[serde(default)]
    pub uom: Option<String>,
    pub values: Vec<TimeFloat>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeFloat {
    pub valid_time: Option<String>,
    pub value: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ForecastResponse {
    pub point: Point,
    pub forecast: Properties,
    pub alerts: Vec<Alert>,
}

// end nws data models

// data structures for NAC data
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AvalancheData {
    pub features: Vec<AvyArea>,
    // when the zones were fetched from NAC. defaults to the Unix epoch, so a
    // snapshot stored without it is treated as stale
    #[serde(
        default,
        with = "bson::serde_helpers::chrono_datetime_as_bson_datetime"
    )]
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AvyArea {
    pub properties: AvyProperties,
    // if there is no geometry, the data is compromised
    // therefore an option is not used
    pub geometry: GeometryTypes,
}

// an avalanche zone whose geometry could not be converted in nac::build_avy_map,
// stored in the invalid_avy_areas collection for debugging
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InvalidAvyArea {
    // the zone's name and center, which identify the record across refreshes
    pub zone_name: Option<String>,
    pub center: Option<String>,
    pub reason: GeometryErrorReason,
    pub error: String,
    // the zone as received from NAC, with its geometry left unconverted
    pub area: AvyArea,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub first_seen: DateTime<Utc>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvyProperties {
    pub name: Option<String>,
    pub center: Option<String>,
    pub center_link: Option<String>,
    pub off_season: Option<bool>,
    pub travel_advice: Option<String>,
    pub danger: Option<String>,
    pub danger_level: Option<i32>,
    pub link: Option<String>,
    pub timezone: Option<String>,
    pub start_date: Option<DateTimeTypes>,
    pub end_date: Option<DateTimeTypes>,
    //pub warning: Option<AvyWarningTypes>,
}

// shared data structures for dealing with gemetric areas
// enum for handling geometry deserialization
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum GeometryTypes {
    Geojson(geojson::Geometry),
    Polygon(geo::Polygon),
    MultiPolygon(geo::MultiPolygon),
}

// why geometry::convert_geometry rejected a geometry, stored in the debugging collections
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GeometryErrorReason {
    // a geometry type other than Polygon or MultiPolygon
    UnsupportedGeometryType,
    // a position with fewer than 2 coordinates, which geojson's conversion panics on
    MalformedCoordinates,
    // no polygons, or an exterior ring with fewer than the 4 positions GeoJSON
    // requires. it would convert, but could never contain a point
    DegenerateGeometry,
    // the conversion to a geo type returned an error
    GeometryConversionFailed,
}

// shared data structures for parsing geojson feature collections
// features are kept as raw json so one malformed feature
// can be skipped instead of failing the whole collection
#[derive(Debug, Deserialize)]
pub struct RawFeatures {
    pub features: Vec<serde_json::Value>,
}

impl RawFeatures {
    // deserializes each feature on its own, returning the features that
    // deserialized and a record of each one that didn't
    pub fn parse<T: DeserializeOwned>(self, source: &str) -> (Vec<T>, SkippedFeatures) {
        let total = self.features.len();
        let now = Utc::now();
        let mut parsed = Vec::with_capacity(total);
        let mut skipped = Vec::new();

        for feature in self.features {
            // read from a reference, so the feature can still be stored if it fails
            match T::deserialize(&feature) {
                Ok(value) => parsed.push(value),
                Err(e) => {
                    // NWS ids are strings and NAC ids are numbers
                    let feature_id = feature.get("id").map(|id| match id {
                        serde_json::Value::String(id) => id.clone(),
                        other => other.to_string(),
                    });
                    eprintln!(
                        "{source}: skipping feature {}: {e}",
                        feature_id.as_deref().unwrap_or("without id")
                    );
                    skipped.push(SkippedFeature {
                        source: source.to_string(),
                        feature_id,
                        error: e.to_string(),
                        feature,
                        first_seen: now,
                        last_seen: now,
                    });
                }
            }
        }

        if !skipped.is_empty() {
            eprintln!("{source}: skipped {} of {total} features", skipped.len());
        }
        (
            parsed,
            SkippedFeatures {
                features: skipped,
                total,
            },
        )
    }
}

// a fetch fails when this percentage or more of its features can't be
// deserialized, since that suggests the upstream format changed
pub const SKIPPED_FEATURES_FAIL_PERCENT: usize = 10;

// the features RawFeatures::parse couldn't deserialize, out of the total it was given
#[derive(Debug, Default)]
pub struct SkippedFeatures {
    pub features: Vec<SkippedFeature>,
    pub total: usize,
}

impl SkippedFeatures {
    pub fn too_many(&self) -> bool {
        self.total > 0 && self.features.len() * 100 >= self.total * SKIPPED_FEATURES_FAIL_PERCENT
    }
}

// a feature that couldn't be deserialized, stored in the skipped_features
// collection for debugging
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkippedFeature {
    // the feature collection it came from, such as "NWS alerts"
    pub source: String,
    // the source and feature id identify the record across refreshes
    pub feature_id: Option<String>,
    pub error: String,
    // the feature's json as received
    pub feature: serde_json::Value,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub first_seen: DateTime<Utc>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub last_seen: DateTime<Utc>,
}

// shared data structures for dealing with datetime
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DateTimeTypes {
    // aware is listed first so stored values with an offset
    // don't get matched as naive when deserialized from mongo
    Aware(chrono::DateTime<FixedOffset>),
    Naive(chrono::NaiveDateTime),
}

impl DateTimeTypes {
    // naive values are UTC: NAC dates have no offset, and build_avy_map
    // leaves them naive only when the zone's timezone couldn't be read
    pub fn to_utc(&self) -> DateTime<Utc> {
        match self {
            DateTimeTypes::Aware(aware) => aware.with_timezone(&Utc),
            DateTimeTypes::Naive(naive) => naive.and_utc(),
        }
    }
}

// a snapshot read for use, stale is True when it is older than its max age
// because it could not be refreshed
pub struct Snapshot<T> {
    pub data: T,
    pub stale: bool,
}
