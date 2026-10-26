use thiserror::Error;
use geojson;
use geo;
use geo_types;
use chrono::{DateTime, Duration, FixedOffset, Utc, format::ParseError};
use serde::{Serialize, Deserialize, de::DeserializeOwned};
use serde_json::json;
use std::collections::HashMap;

// local modules
use crate::tracking::NoticeFlag;

// user data models
#[derive(Debug, Serialize, Deserialize, Clone)]           
pub struct UserPoint       
{                          
   pub user: String,          
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
   pub avy_lost: NoticeFlag,
   // set once the user is told the center entered its off season. kept apart from
   // avy_lost, so the off season notice is still sent after any other kind of loss
   #[serde(default)]
   pub avy_off_season: NoticeFlag,
   #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
   pub time_added: DateTime<Utc>,
   pub valid_duration: u32,
   // when a daily_forecast tracker sends its next forecast, None for other
   // trackers and once no 6 AM is left before the tracker expires
   #[serde(default, with = "bson::serde_helpers::chrono_datetime_as_bson_datetime_optional")]
   pub next_daily_forecast: Option<DateTime<Utc>>,
}

impl UserPoint
{
    // the tracker stops valid_duration hours after it was added
    pub fn expires_at(&self) -> DateTime<Utc>
    {
        self.time_added + Duration::hours(self.valid_duration.into())
    }
}                          

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
// data structure for parsing SMS messages from the RidgeCast
// IOS and Android apps
pub struct ForecastVars
{
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
pub struct ForecastProperties
{
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
                           
#[derive(Debug)]           
pub struct User            
{                          
   pub user_id: String,       
   pub email: String,         
   pub phone: String,         
}                          
// end user data models

// data structures for nws points endpoint                   

#[derive(Debug, Serialize, Deserialize, Clone)]              
pub struct Point                                             
{                                                            
   pub properties: PointProperties,                             
   pub id: Option<String>,                                      
   pub geometry: PointGeometry,                                 
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize, Clone)]              
pub struct PointGeometry                                     
{                                                            
    pub coordinates: geo::Point,                         
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize, Clone)]              
#[serde(rename_all = "camelCase")]                           
pub struct PointProperties                                   
{                                                            
    // no need for option                                    
    // if the value is None                                  
    // there is an error with the json recieved from NWS     
   pub cwa: String,                                             
   pub grid_x: i32,                                             
   pub grid_y: i32,                                             
   pub nwr: NWR,                                                
    // This one is an option                                 
    // because the forecast is still useful without it       
    pub astronomical_data: Option<AstronomicalData>,
    // IANA time zone name such as "America/Los_Angeles", used to send
    // daily forecasts at local time. an option for the same reason
    pub time_zone: Option<String>,             
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize, Clone)]              
#[serde(rename_all = "camelCase")]                           
pub struct NWR                                               
{                                                            
    // not option because I dont want to return a forecast   
    // if I cant check alerts                                
    pub same_code: String,                                       
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize, Clone)]              
pub struct AstronomicalData                                  
{                                                            
    pub sunrise: Option<chrono::DateTime<FixedOffset>>,          
    pub sunset: Option<chrono::DateTime<FixedOffset>>,           
}                                                            
                                                             
// data structures for deserizlizing nws alerts endpoint json
#[derive(Debug, Serialize, Deserialize)]                     
pub struct Alerts                                            
{                                                            
    pub features: Vec<Alert>,                                    
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize, Clone)]              
pub struct Alert                                             
{                                                            
   // NWS alert id, used to tell which alerts a user was already sent
   pub id: Option<String>,
   pub geometry: Option<GeometryTypes>,                         
   pub properties: AlertData,                                   
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize, Clone)]              
#[serde(rename_all = "camelCase")]                           
pub struct AlertData                                         
{                                                            
   pub area_desc: Option<String>,                               
   pub effective: Option<chrono::DateTime<FixedOffset>>,        
   pub expires: Option<chrono::DateTime<FixedOffset>>,          
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
pub struct Geocode                                           
{                                                            
    // a null SAME list is read as empty, for the same reason as AlertData.geocode
    #[serde(rename = "SAME", default, deserialize_with = "null_as_default")]
    pub same: Vec<String>,                                       
}                                                            

impl Geocode
{
    // NWS specifies SAME codes as matching ^\d{6}$
    pub fn is_valid_same_code(code: &str) -> bool
    {
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
pub struct InvalidAlert
{
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
pub enum InvalidAlertReason
{
    // at least one SAME code didn't match ^\d{6}$
    InvalidSameCodes,
    // no SAME code was left after invalid ones were removed
    NoSameCode,
    // the geometry can't be used to match a point, so it was removed
    InvalidGeometry,
}

// data structures for utilizing nws alerts data
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AlertsSorted
{
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
   #[serde(default, with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
   pub fetched_at: DateTime<Utc>,
}

impl AlertsSorted
{
    pub fn add_geometry_alert(&mut self, alert: &Alert)
    {
        self.with_geometry.push(alert.clone());
    }

    pub fn add_same_code_alert(&mut self, same_code: String, alert: &Alert)
    {
        self.by_same_code
            .entry(same_code)
            .or_default()
            .push(alert.clone());
    }

    pub fn get_same_code_alerts(&self, same_code: &str) -> &[Alert]
    {
        self.by_same_code
            .get(same_code)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}
                                                             
// data structures for nws gridpoints endpoint               
#[derive(Debug, Serialize, Deserialize)]                     
pub struct Forecast                                          
{                                                            
    // if not properties, there is an issue                  
    pub properties: Properties,                                  
    // remove later                                          
    pub id: Option<String>,                                      
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize)]                     
#[serde(rename_all = "camelCase")]                           
pub struct Properties                                        
{                                                            
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
pub struct Elevation                                         
{                                                            
    pub value: Option<f32>,                                      
}                                                            
                                                             
// shared structures                                         
#[derive(Debug, Serialize, Deserialize)]                     
#[serde(rename_all = "camelCase")]                           
pub struct IntVec                                            
{                                                            
    pub values: Vec<TimeInt>,                                    
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize)]                     
#[serde(rename_all = "camelCase")]                           
pub struct TimeInt                                           
{                                                            
    pub valid_time: Option<String>,                              
    pub value: Option<i32>,                                      
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize)]                     
#[serde(rename_all = "camelCase")]                           
pub struct FloatVec                                          
{                                                            
    pub values: Vec<TimeFloat>,                                  
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize)]                     
#[serde(rename_all = "camelCase")]                           
pub struct TimeFloat                                         
{                                                            
    pub valid_time: Option<String>,                              
    pub value: Option<f32>,                                      
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize)]                     
pub struct ForecastResponse                                  
{                                                            
    pub point: Point,                                        
    pub forecast: Forecast,                                      
    pub alerts: Vec<Alert>,                                      
}                                                            
                                                             
#[derive(Debug, Error)]                                      
pub enum NWSError                                            
{                                                            
    #[error("could not retrieve data from NWS API: {0}")]
    APICallError(#[from] reqwest::Error),
    #[error("received error response from NWS endpoint")]
    NWSResponseError,
    #[error("json parsing failed: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("failed to fetch points")]                       
    PointsError,                                             
    #[error("invalid points data")]                          
    PointsDataError,                                         
    #[error("failed geometry conversion")]
    GeometryError,

}

impl NWSError
{
    pub fn is_timeout(&self) -> bool
    {
        matches!(self, NWSError::APICallError(e) if e.is_timeout())
    }

    // NWS returns 404 for points outside its coverage area
    pub fn is_not_found(&self) -> bool
    {
        matches!(self, NWSError::APICallError(e)
            if e.status() == Some(reqwest::StatusCode::NOT_FOUND))
    }

    // the response arrived but its body was not valid json
    // or not shaped like the expected data
    pub fn is_malformed(&self) -> bool
    {
        match self
        {
            NWSError::APICallError(e) => e.is_decode(),
            NWSError::JsonError(_) => true,
            _ => false,
        }
    }
}
// end nws data models

// data structures for NAC data
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AvalancheData
{
    pub features: Vec<AvyArea>,
    // when the zones were fetched from NAC. defaults to the Unix epoch, so a
    // snapshot stored without it is treated as stale
    #[serde(default, with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AvyArea
{
    pub properties: AvyProperties,
    // if there is no geometry, the data is compromised
    // therefore an option is not used
    pub geometry: GeometryTypes,
}

// an avalanche zone whose geometry could not be converted in nac::build_avy_map,
// stored in the invalid_avy_areas collection for debugging
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InvalidAvyArea
{
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
pub struct AvyProperties
{
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
pub enum GeometryTypes         
{                                              
    Geojson(geojson::Geometry),                 
    Polygon(geo_types::Polygon),
    MultiPolygon(geo_types::MultiPolygon),
}                                              

// why geometry::convert_geometry rejected a geometry, stored in the debugging collections
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GeometryErrorReason
{
    // a geometry type other than Polygon or MultiPolygon
    UnsupportedGeometryType,
    // a position with fewer than 2 coordinates, which geojson's conversion panics on
    MalformedCoordinates,
    // no polygons, or an exterior ring with fewer than the 4 positions GeoJSON
    // requires. it would convert, but could never contain a point
    DegenerateGeometry,
    // the conversion to geo_types returned an error
    GeometryConversionFailed,
}

// shared data structures for parsing geojson feature collections
// features are kept as raw json so one malformed feature
// can be skipped instead of failing the whole collection
#[derive(Debug, Deserialize)]
pub struct RawFeatures
{
    pub features: Vec<serde_json::Value>,
}

impl RawFeatures
{
    pub fn parse<T: DeserializeOwned>(self, source: &str) -> Vec<T>
    {
        self.features
            .into_iter()
            .filter_map(|feature|
            {
                let id = feature.get("id")
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "without id".to_string());

                match serde_json::from_value(feature)
                {
                    Ok(parsed) => Some(parsed),
                    Err(e) =>
                    {
                        eprintln!("{source}: skipping feature {id}: {e}");
                        None
                    }
                }
            })
            .collect()
    }
}

// shared data structures for dealing with datetime
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DateTimeTypes
{
    // aware is listed first so stored values with an offset
    // don't get matched as naive when deserialized from mongo
    Aware(chrono::DateTime<FixedOffset>),
    Naive(chrono::NaiveDateTime),
}

impl DateTimeTypes
{
    // naive values are UTC: NAC dates have no offset, and build_avy_map
    // leaves them naive only when the zone's timezone couldn't be read
    pub fn to_utc(&self) -> DateTime<Utc>
    {
        match self
        {
            DateTimeTypes::Aware(aware) => aware.with_timezone(&Utc),
            DateTimeTypes::Naive(naive) => naive.and_utc(),
        }
    }
}
