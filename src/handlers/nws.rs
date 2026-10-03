// handler for NWS api
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json;
use std::io;
use thiserror::Error;
use geojson;
use geo::Contains;
use geo_types::{Polygon, line_string, point};
use once_cell::sync::Lazy;
use chrono::{DateTime, FixedOffset, format::ParseError};

use crate::models::{Point, PointGeometry, PointProperties,
    NWR, Alerts, Alert, AlertGeometry,
    AlertData, Geocode, AlertsSorted,
    Forecast, Properties, Elevation, IntVec, TimeInt,
    FloatVec, TimeFloat, ForecastResponse, NWSError};

const BASE_URL: &str = "https://api.weather.gov";

static NWS_CLIENT: Lazy<Client> = Lazy::new(|| 
{
    Client::builder()
        .user_agent("User-Agent: (TrailAlert, stormotyler@gmail.com)")
        .build()
        .expect("Failed to build client")
});

pub async fn get_alerts() -> Result<AlertsSorted, NWSError>
{
    //println!("get_alerts called)");

    let alerts_response =
        NWS_CLIENT
        .get(format!("{BASE_URL}/alerts/active?status=actual"))
        .send()
        .await?;
    
    //println!("data received from NWS endpoint");

    let alerts_json: serde_json::Value =
        alerts_response
        .json()
        .await?;

    //println!("NWS data jsonified");
    //println!("{:?}", alerts_json);

    //let alerts = serde_json::from_value(alerts_json)?;
    let mut alerts: Alerts = match 
        serde_json::from_value(alerts_json)
        {
            Ok(alert) => alert,
            Err(_e) => return Err(NWSError::JsonError(_e)),
        };

    let mut alerts_sorted = AlertsSorted::default();

    for alert in alerts.features.iter_mut()                                                
    {                                                                                      
        match &alert.geometry                                             
        {                                                                                  
            Some(AlertGeometry::Geojson(geo)) =>                                           
            {                                                                              
                match geo.clone().try_into()                                               
                {                                                                          
                    Ok(new_geo) => 
                    {
                        alert.geometry = Some(AlertGeometry::Polygon(new_geo));                  
                        alerts_sorted.add_geometry_alert(alert);
                    }
                    Err(_e) => 
                    {
                        for same_code in alert.properties.geocode.same.iter()
                        {
                            alerts_sorted.add_same_code_alert(same_code.to_string(), alert);
                        }
                    }
                }                                                                          
            }                                                                              
            Some(AlertGeometry::Polygon(geo)) =>
            {
                alerts_sorted.add_geometry_alert(alert);
            }
            None =>
            {
                for same_code in alert.properties.geocode.same.iter()
                {
                    alerts_sorted.add_same_code_alert(same_code.to_string(), alert);
                }
            }
        }                                                                           
    }                                                                                 

    Ok(alerts_sorted)
}

pub async fn check_alerts(alerts: AlertsSorted, point: Point) -> Vec<Alert>
{
    // alerts without geometry: direct hash lookup by the point's SAME code
    let mut checked: Vec<Alert> =
        alerts.get_same_code_alerts(&point.properties.nwr.same_code).to_vec();
    if !checked.is_empty()
    {
        println!("alert matched from samecode");
    }

    // alerts with geometry: check whether the polygon contains the point
    if let Some(coordinates) = point.geometry.coordinates
    {
        for alert in alerts.with_geometry.iter()
        {
            if let Some(AlertGeometry::Polygon(value)) = &alert.geometry
            {
                if value.contains(&coordinates)
                {
                    println!("alert matched via geometry!");
                    checked.push(alert.clone());
                }
            }
        }
    }
    return checked
}

// update to also return the points data
// may have to update the error handling to still return points
// even if there is an issue with the forecast
// but maybe its better to return neither if there is an issue
pub async fn get_forecast(lat: f32, lon: f32, alerts: AlertsSorted) ->
    Result<ForecastResponse, NWSError>
{
    let points_response =
        NWS_CLIENT
        .get(format!("{BASE_URL}/points/{lat},{lon}"))
        .send()
        .await?;
                                                                                                                                                                         
    let points_json: serde_json::Value =
        points_response
        .json()
        .await?;

    let point: Point = match
        serde_json::from_value(points_json)
        {
        Ok(point) => point,
        Err(_e) => return Err(NWSError::NWSResponseError),
        };
    
    //point.geometry = match &point.geometry                                             
    //{                                                                                  
    //    Some(PointGeometry::Geojson(geo)) =>                                           
    //    {                                                                              
    //        match geo.clone().try_into()                                               
    //        {                                                                          
    //            Ok(new_geo) => Some(PointGeometry::Point(new_geo)),                  
    //            Err(_e) => Some(PointGeometry::Geojson(geo.clone())),                  
    //        }                                                                          
    //    }                                                                              
    //    Some(PointGeometry::Point(geo)) => Some(PointGeometry::Point(geo.clone())),
    //    None => None,                                                                  
    //};                                                                                 

    println!("{:?}", point.geometry);

    //println!("{:?}", points);
    //println!("{BASE_URL}/gridpoints/{}/{},{}",
    //    points.properties.cwa,
    //    points.properties.grid_x,
    //    points.properties.grid_y);

    let gridpoints_response =  
        NWS_CLIENT
        .get(format!("{BASE_URL}/gridpoints/{}/{},{}",
        point.properties.cwa,                                                                                                                                
        point.properties.grid_x,
        point.properties.grid_y))                                                                                                                            
        .send()
        .await?;                                                                                                                                                          

    let gridpoints: serde_json::Value = 
        gridpoints_response
        .json()
        .await?;

    //println!("gridpoints is {:?}", gridpoints);

    
    let forecast: Forecast = serde_json::from_value(gridpoints)?;
    //println!("forecast is: {:?}", forecast);

    let active_alerts: Vec<Alert> =
        check_alerts(alerts, point.clone()).await;

    let forecast_response = ForecastResponse
    {
        point: point,
        forecast: forecast,
        alerts: active_alerts,
    };
    Ok(forecast_response)
}
