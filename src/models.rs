use thiserror::Error;
use geojson;
use geo;
use geo_types;
use chrono::{DateTime, FixedOffset, format::ParseError};
use serde::{Serialize, Deserialize};
use serde_json::json;

// user data models
#[derive(Debug)]           
pub struct UserPoint       
{                          
   pub user: String,          
   pub point: Point,
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
    pub coordinates: Option<geo::Point>,                         
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
   pub geometry: Option<AlertGeometry>,                         
   pub properties: AlertData,                                   
}                                                            
                                                             
// enum for handling alert geometry                          
#[derive(Debug, Serialize, Deserialize, Clone)]              
#[serde(untagged)]                                           
pub enum AlertGeometry                                       
{                                                            
   Geojson(geojson::Geometry),                              
   Polygon(geo_types::Polygon),                             
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize, Clone)]              
#[serde(rename_all = "camelCase")]                           
pub struct AlertData                                         
{                                                            
   pub area_desc: Option<String>,                               
   pub effective: Option<chrono::DateTime<FixedOffset>>,        
   pub expires: Option<chrono::DateTime<FixedOffset>>,          
   pub severtiy: Option<String>,                                
   pub certainty: Option<String>,                               
   pub urgency: Option<String>,                                 
   pub event: Option<String>,                                   
   pub headline: Option<String>,                                
   pub instruction: Option<String>,                             
   pub geocode: Geocode,                                        
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize, Clone)]              
pub struct Geocode                                           
{                                                            
    #[serde(rename = "SAME")]                                
    pub same: Vec<String>,                                       
}                                                            
                                                             
// data structures for utilizing nws alerts data             
#[derive(Debug, Serialize, Deserialize)]                     
pub struct AlertsSorted                                      
{                                                            
   pub alert_categories: Vec<AlertCategory>,                    
   pub category_names: Vec<String>,                             
}                                                            
                                                             
#[derive(Debug, Serialize, Deserialize)]                     
pub struct AlertCategory                                     
{                                                            
   pub id: String,                                              
   pub alerts: Vec<Alert>,                                      
}                                                            
                                                             
impl AlertsSorted                                            
{                                                            
    pub fn add_alert(&mut self, id: String, alert: &Alert)       
    {                                                        
        if !self.category_names.contains(&id)                
        {                                                    
            self.alert_categories.push(AlertCategory         
            {                                                
                id: id.clone(),                              
                alerts: vec![alert.clone()],                 
            });                                              
            self.category_names.push(id.clone());            
        }                                                    
        else                                                 
        {                                                    
            for category in self.alert_categories.iter_mut() 
            {                                                
                if category.id == id                         
                {                                            
                    category.alerts.push(alert.clone());     
                }                                            
            }                                                
        }                                                    
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
   pub wet_bulb_globe_temperature: FloatVec,                    
   pub heat_risk: IntVec,                                       
   pub relative_humidity: IntVec,                               
   pub visibility: FloatVec,                                    
   pub ceiling_height: FloatVec,                                
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
    #[error("could not retrieve data from NWS API")]         
    APICallError(#[from] reqwest::Error),                    
    #[error("received error response from NWS endpoint")]    
    NWSResponseError,                                        
    #[error("json parsing failed")]                          
    JsonError(#[from] serde_json::Error),                    
    #[error("failed to fetch points")]                       
    PointsError,                                             
    #[error("invalid points data")]                          
    PointsDataError,                                         
    #[error("failed geometry conversion")]                   
    GeometryError,                                           
                                                             
}                                                            
// end nws data models
