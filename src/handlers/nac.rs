// handler for National Avalanche Center api

use reqwest::Client;
use once_cell::sync::Lazy;
use std::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json;
use std::io;
use thiserror::Error;
use geojson;
use geo;
use geo::Intersects;
use geojson::GeometryValue;
use chrono_tz::Tz;

// local modules
use crate::models::{AvalancheData, AvyArea, AvyProperties, Point, 
    DateTimeTypes, GeometryTypes, RawFeatures, UserPoint};

const BASE_URL: &str = "https://api.avalanche.org/v2/public/products/map-layer";

static NAC_CLIENT: Lazy<Client> = Lazy::new(||                        
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

#[derive(Debug, Error)]
pub enum NACError
{
    #[error("could not retrieve data from NAC API: {0}")]
    APICallError(#[from] reqwest::Error),
    #[error("json parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("could not complete avalanche danger check")]
    CheckError,
    #[error("no current avalanche forecast available")]
    NoForecastError,
    #[error("relevant avalanche center is in its off season")]
    OffSeasonError,
}

impl NACError
{
    pub fn is_timeout(&self) -> bool
    {
        matches!(self, NACError::APICallError(e) if e.is_timeout())
    }

    // the response arrived but its body was not valid json
    // or not shaped like a feature collection
    pub fn is_malformed(&self) -> bool
    {
        match self
        {
            NACError::APICallError(e) => e.is_decode(),
            NACError::JsonError(_) => true,
            _ => false,
        }
    }
}

async fn fetch_avy() -> Result<RawFeatures, NACError>
{
    let avy_response =
        NAC_CLIENT
        .get(BASE_URL)
        .send()
        .await?
        .error_for_status()?;

    let avy_json: serde_json::Value =
        avy_response
        .json()
        .await?;

    //println!("{avy_json}");

    Ok(serde_json::from_value(avy_json)?)
}

pub async fn get_avy() -> Result<AvalancheData, NACError>
{
    // retry once if the response is malformed
    let raw = match fetch_avy().await
    {
        Err(e) if e.is_malformed() =>
        {
            eprintln!("get_avy: malformed response, retrying once: {e}");
            fetch_avy().await?
        }
        result => result?,
    };
    //println!("{:?}", avy);

    // wrap the zones so the snapshot is stored as a single document
    Ok(AvalancheData { features: build_avy_map(raw.parse("NAC map layer")) })
}

// converts each zone's dates to local time and its geometry to geo_types,
// so check_avy_geometry can test points against them
pub fn build_avy_map(mut features: Vec<AvyArea>) -> Vec<AvyArea>
{
    for area in features.iter_mut()
    {
        // NAC dates have no offset but are in UTC,
        // so convert them to the zone's local timezone
        if let Some(tz) = area.properties.timezone
            .as_deref()
            .and_then(|tz| tz.parse::<Tz>().ok())
        {
            for date in [&mut area.properties.start_date, &mut area.properties.end_date]
            {
                if let Some(DateTimeTypes::Naive(naive)) = date
                {
                    let local = naive.and_utc().with_timezone(&tz).fixed_offset();
                    *date = Some(DateTimeTypes::Aware(local));
                }
            }
        }

        if let GeometryTypes::Geojson(geo) = &area.geometry
        {
            // the geojson type tag decides which geo_types struct to convert into
            let converted = match &geo.value
            {
                GeometryValue::Polygon { .. } =>
                    geo_types::Polygon::try_from(geo)
                    .map(GeometryTypes::Polygon),
                GeometryValue::MultiPolygon { .. } =>
                    geo_types::MultiPolygon::try_from(geo)
                    .map(GeometryTypes::MultiPolygon),
                // other geometry kinds can't be used for zone checks
                _ => continue,
            };

            match converted
            {
                Ok(new_geo) => area.geometry = new_geo,
                // leave the geometry as Geojson if conversion fails
                Err(_e) => {}
            }
        }
    }

    features
}

// checks whether a point intersects with an avalanche forecast zone 
// and returns the area name, avalanche center, and forecast danger
// errors if the zone's center is in its off season or the zone has no current forecast
pub async fn check_avy_geometry(point: &Point, avy_data: &AvalancheData) ->
    Result<Option<AvyProperties>, NACError>
{
    let coordinates = point.geometry.coordinates;

    // intersects, so a point on a zone boundary still matches
    let zone = avy_data.features.iter().find(|area| match &area.geometry
    {
        GeometryTypes::Polygon(value) => value.intersects(&coordinates),
        GeometryTypes::MultiPolygon(value) => value.intersects(&coordinates),
        GeometryTypes::Geojson(_) => false,
    });
    let Some(area) = zone
    else
    {
        return Ok(None);
    };

    // off season is checked first, since off season zones can keep an old start_date
    if area.properties.off_season == Some(true)
    {
        return Err(NACError::OffSeasonError);
    }
    if area.properties.start_date.is_none()
    {
        return Err(NACError::NoForecastError);
    }
    Ok(Some(area.properties.clone()))
}

// checks if there is a new avalanche forecast at a UserPoint's location
// if new forecast is found, updates UserPoint.avy with the new forecast
// returns True if UserPoint.avy is updated
pub async fn check_avy_tracker(user_point: &mut UserPoint, avy_data: &AvalancheData) -> bool
{
    // no zone, an off season zone, or a zone without a current forecast
    // has nothing new to report
    let Ok(Some(current)) = check_avy_geometry(&user_point.point, avy_data).await
    else
    {
        return false;
    };

    // any changed field counts as a new forecast, since avalanche
    // information is safety critical it is better to over report
    let is_new = user_point.avy.as_ref() != Some(&current);

    if is_new
    {
        user_point.avy = Some(current);
    }
    is_new
}
