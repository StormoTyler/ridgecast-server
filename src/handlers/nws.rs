// handler for NWS api
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json;
use std::io;
use thiserror::Error;
use geojson;
use geo::Intersects;
use geo_types::{Polygon, line_string, point};
use once_cell::sync::Lazy;
use std::time::Duration;
use chrono::{DateTime, FixedOffset, format::ParseError};

use crate::models::{Point, PointGeometry, PointProperties,
    NWR, Alerts, Alert, GeometryTypes,
    AlertData, Geocode, AlertsSorted,
    Forecast, Properties, Elevation, IntVec, TimeInt,
    FloatVec, TimeFloat, ForecastResponse, NWSError, RawFeatures,
    InvalidAlert, InvalidAlertReason};
use crate::geometry::convert_geometry;

const BASE_URL: &str = "https://api.weather.gov";

static NWS_CLIENT: Lazy<Client> = Lazy::new(|| 
{
    Client::builder()
        .user_agent("User-Agent: (RidgeCast, stormotyler@gmail.com)")
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

// returns the sorted alerts and the alerts that failed validation
pub async fn get_alerts() -> Result<(AlertsSorted, Vec<InvalidAlert>), NWSError>
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
    let (alerts, invalid_alerts) = parse_alerts(raw);

    let mut alerts_sorted = sort_alerts(alerts);
    alerts_sorted.fetched_at = chrono::Utc::now();
    println!("{:?}", alerts_sorted);
    Ok((alerts_sorted, invalid_alerts))
}

// deserializes the alerts and checks their SAME codes against the NWS spec
// codes that don't match ^\d{6}$ are logged and removed, since they can never
// match a point, and every alert left without a SAME code is logged
// geometry is converted for sort_alerts, geometry that can't be used is logged
// and removed so the alert is matched by its SAME codes instead
// returns the alerts and a record of each alert that failed validation
pub fn parse_alerts(raw: RawFeatures) -> (Alerts, Vec<InvalidAlert>)
{
    let mut alerts = Alerts { features: raw.parse("NWS alerts") };
    let mut invalid_alerts = Vec::new();
    let now = chrono::Utc::now();

    for alert in alerts.features.iter_mut()
    {
        // kept as received, in case the alert fails validation
        let received = alert.clone();
        let mut reasons = Vec::new();

        let (valid, invalid): (Vec<String>, Vec<String>) =
            std::mem::take(&mut alert.properties.geocode.same)
            .into_iter()
            .partition(|code| Geocode::is_valid_same_code(code));
        if !invalid.is_empty()
        {
            eprintln!("parse_alerts: alert {} ({}): removing SAME codes that don't match \
                ^\\d{{6}}$: {invalid:?}", alert_id(alert), alert_event(alert));
            reasons.push(InvalidAlertReason::InvalidSameCodes);
        }
        alert.properties.geocode.same = valid;

        if alert.properties.geocode.same.is_empty()
        {
            eprintln!("parse_alerts: alert {} ({}) has no SAME code",
                alert_id(alert), alert_event(alert));
            reasons.push(InvalidAlertReason::NoSameCode);
        }

        // converted here so geometry that can't be used is recorded,
        // the alert then falls back to its SAME codes in sort_alerts
        let mut geometry_error = None;
        let mut geometry_error_reason = None;
        if let Some(GeometryTypes::Geojson(geo)) = &alert.geometry
        {
            match convert_geometry(geo)
            {
                Ok(new_geo) => alert.geometry = Some(new_geo),
                Err(error) =>
                {
                    eprintln!("parse_alerts: alert {} ({}): removing geometry that can't be used, \
                        matching by SAME code instead: {error}", alert_id(alert), alert_event(alert));
                    alert.geometry = None;
                    reasons.push(InvalidAlertReason::InvalidGeometry);
                    geometry_error_reason = Some(error.reason);
                    geometry_error = Some(error.message);
                }
            }
        }

        if !reasons.is_empty()
        {
            invalid_alerts.push(InvalidAlert
            {
                alert_id: received.id.clone(),
                reasons,
                invalid_same_codes: invalid,
                geometry_error_reason,
                geometry_error,
                alert: received,
                first_seen: now,
                last_seen: now,
            });
        }
    }
    (alerts, invalid_alerts)
}

// buckets alerts by whether they have usable polygon geometry,
// so check_alerts can match by polygon or by SAME code
// an alert with neither can't be matched to a location, so it is
// logged and kept in unmatched instead of being dropped
pub fn sort_alerts(mut alerts: Alerts) -> AlertsSorted
{
    let mut alerts_sorted = AlertsSorted::default();

    for alert in alerts.features.iter_mut()
    {
        match &alert.geometry
        {
            // parse_alerts already converts geojson, this handles alerts that skipped it
            Some(GeometryTypes::Geojson(geo)) =>
            {
                match convert_geometry(geo)
                {
                    Ok(new_geo) =>
                    {
                        alert.geometry = Some(new_geo);
                        alerts_sorted.add_geometry_alert(alert);
                    }
                    Err(e) =>
                    {
                        eprintln!("sort_alerts: alert {} ({}): geometry conversion failed, \
                            matching by SAME code instead: {e}", alert_id(alert), alert_event(alert));
                        add_by_same_code(&mut alerts_sorted, alert);
                    }
                }
            }
            Some(GeometryTypes::Polygon(_)) | Some(GeometryTypes::MultiPolygon(_)) =>
            {
                alerts_sorted.add_geometry_alert(alert);
            }
            None => add_by_same_code(&mut alerts_sorted, alert),
        }
    }

    if !alerts_sorted.unmatched.is_empty()
    {
        eprintln!("sort_alerts: {} alerts have no usable geometry or SAME code \
            and can't be matched to a location", alerts_sorted.unmatched.len());
    }
    alerts_sorted
}

// files an alert under each of its SAME codes, or in unmatched if it has none
fn add_by_same_code(alerts_sorted: &mut AlertsSorted, alert: &Alert)
{
    if alert.properties.geocode.same.is_empty()
    {
        eprintln!("sort_alerts: alert {} ({}) has no usable geometry or SAME code",
            alert_id(alert), alert_event(alert));
        alerts_sorted.unmatched.push(alert.clone());
        return;
    }
    for same_code in alert.properties.geocode.same.iter()
    {
        alerts_sorted.add_same_code_alert(same_code.to_string(), alert);
    }
}

fn alert_id(alert: &Alert) -> &str
{
    alert.id.as_deref().unwrap_or("without id")
}

fn alert_event(alert: &Alert) -> &str
{
    alert.properties.event.as_deref().unwrap_or("unknown event")
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
    let coordinates = point.geometry.coordinates;
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
