// handler for NWS api
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json;
use std::io;
use thiserror::Error;
use geojson;
use geojson::GeometryValue;
use geo::Intersects;
use geo_types::{Polygon, line_string, point};
use once_cell::sync::Lazy;
use std::time::Duration;
use chrono::{DateTime, FixedOffset, format::ParseError};

use crate::models::{Point, PointGeometry, PointProperties,
    NWR, Alerts, Alert, GeometryTypes,
    AlertData, Geocode, AlertsSorted,
    Forecast, Properties, Elevation, IntVec, TimeInt,
    FloatVec, TimeFloat, ForecastResponse, NWSError, RawFeatures};

const BASE_URL: &str = "https://api.weather.gov";

static NWS_CLIENT: Lazy<Client> = Lazy::new(|| 
{
    Client::builder()
        .user_agent("User-Agent: (TrailAlert, stormotyler@gmail.com)")
        // fail fast if the host is unreachable
        .connect_timeout(Duration::from_secs(10))
        // cap the whole request, including reading the body
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build client")
});

async fn fetch_alerts() -> Result<RawFeatures, NWSError>
{
    let alerts_response =
        NWS_CLIENT
        .get(format!("{BASE_URL}/alerts/active?status=actual"))
        .send()
        .await?
        .error_for_status()?;

    //println!("data received from NWS endpoint");

    let alerts_json: serde_json::Value =
        alerts_response
        .json()
        .await?;

    //println!("NWS data jsonified");
    //println!("{:?}", alerts_json);

    Ok(serde_json::from_value(alerts_json)?)
}

pub async fn get_alerts() -> Result<AlertsSorted, NWSError>
{
    //println!("get_alerts called)");

    // retry once if the response is malformed
    let raw = match fetch_alerts().await
    {
        Err(e) if e.is_malformed() =>
        {
            eprintln!("get_alerts: malformed response, retrying once: {e}");
            fetch_alerts().await?
        }
        result => result?,
    };
    let alerts = Alerts { features: raw.parse("NWS alerts") };

    let alerts_sorted = sort_alerts(alerts);
    println!("{:?}", alerts_sorted);
    Ok(alerts_sorted)
}

// buckets alerts by whether they have usable polygon geometry,
// so check_alerts can match by polygon or by SAME code
pub fn sort_alerts(mut alerts: Alerts) -> AlertsSorted
{
    let mut alerts_sorted = AlertsSorted::default();

    for alert in alerts.features.iter_mut()                                                
    {                                                                                      
        match &alert.geometry                                             
        {                                                                                  
            Some(GeometryTypes::Geojson(geo)) =>
            {
                // the geojson type tag decides which geo_types struct to convert into
                let converted = match &geo.value
                {
                    GeometryValue::MultiPolygon { .. } =>
                        geo_types::MultiPolygon::try_from(geo)
                        .map(GeometryTypes::MultiPolygon),
                    _ =>
                        geo_types::Polygon::try_from(geo)
                        .map(GeometryTypes::Polygon),
                };

                match converted
                {
                    Ok(new_geo) =>
                    {
                        alert.geometry = Some(new_geo);
                        alerts_sorted.add_geometry_alert(alert);
                    }
                    // alerts without a SAME code are dropped here
                    Err(_e) =>
                    {
                        for same_code in alert.properties.geocode.same.iter()
                        {
                            alerts_sorted.add_same_code_alert(same_code.to_string(), alert);
                        }
                    }
                }                                                                          
            }                                                                              
            Some(GeometryTypes::Polygon(geo)) =>
            {
                alerts_sorted.add_geometry_alert(alert);
            }
            Some(GeometryTypes::MultiPolygon(geo)) =>
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
    alerts_sorted
}

pub fn check_alerts(alerts: &AlertsSorted, point: &Point) -> Vec<Alert>
{
    // alerts without geometry: direct hash lookup by the point's SAME code
    let mut checked: Vec<Alert> =
        alerts.get_same_code_alerts(&point.properties.nwr.same_code).to_vec();
    if !checked.is_empty()
    {
        println!("alert matched from samecode");
    }

    // alerts with geometry: check whether the point is inside the polygon
    // or on its boundary, so a point on the edge of an alert area still matches
    if let Some(coordinates) = point.geometry.coordinates
    {
        for alert in alerts.with_geometry.iter()
        {
            let intersects = match &alert.geometry
            {
                Some(GeometryTypes::Polygon(value)) => value.intersects(&coordinates),
                Some(GeometryTypes::MultiPolygon(value)) => value.intersects(&coordinates),
                _ => false,
            };

            if intersects
            {
                println!("alert matched via geometry!");
                checked.push(alert.clone());
            }
        }
    }
    return checked
}

pub async fn get_point(lat:f32, lon: f32) -> Result<Point, NWSError>
{
    let points_response =                                   
        NWS_CLIENT                                          
        .get(format!("{BASE_URL}/points/{lat},{lon}"))      
        .send()                                             
        .await?                                             
        .error_for_status()?;                               
                                                         
     let points_json: serde_json::Value =                    
        points_response                                     
        .json()                                             
        .await?;                                            
                                                         
    let point: Point = serde_json::from_value(points_json)?;

    Ok(point)
}

// fetches gridpoint data from NWS API gridpoints endpoint 
pub async fn get_forecast(lat: f32, lon: f32, alerts: &AlertsSorted) ->
    Result<ForecastResponse, NWSError>
{
    let point = get_point(lat, lon).await?;

    let gridpoints_response =  
        NWS_CLIENT
        .get(format!("{BASE_URL}/gridpoints/{}/{},{}",
        point.properties.cwa,                                                                                                                                
        point.properties.grid_x,
        point.properties.grid_y))                                                                                                                            
        .send()
        .await?
        .error_for_status()?;                                                                                                                                                          

    let gridpoints: serde_json::Value = 
        gridpoints_response
        .json()
        .await?;

    //println!("gridpoints is {:?}", gridpoints);

    
    let forecast: Forecast = serde_json::from_value(gridpoints)?;
    //println!("forecast is: {:?}", forecast);

    let active_alerts: Vec<Alert> =
        check_alerts(alerts, &point);

    let forecast_response = ForecastResponse
    {
        point: point,
        forecast: forecast,
        alerts: active_alerts,
    };
    Ok(forecast_response)
}
