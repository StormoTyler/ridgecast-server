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
use geojson::GeometryValue;
use chrono_tz::Tz;

// local modules
use crate::models::{AvalancheData, AvyArea, AvyProperties, AvyWarning, 
    AvyHashMap, Point, DateTimeTypes, GeometryTypes, RawFeatures};

const BASE_URL: &str = "https://api.avalanche.org/v2/public/products/map-layer";

static NAC_CLIENT: Lazy<Client> = Lazy::new(||                        
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

#[derive(Debug, Error)]
pub enum NACError
{
    #[error("could not retrieve data from NAC API: {0}")]
    APICallError(#[from] reqwest::Error),
    #[error("json parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("could not complete avalanche danger check")]
    CheckError,
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

pub async fn get_avy() -> Result<AvyHashMap, NACError>
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
    let mut features: Vec<AvyArea> = raw.parse("NAC map layer");

    //println!("{:?}", avy);

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

    // key each zone by its NAC id for direct lookups
    let lookup_map = features
        .into_iter()
        .map(|area| (area.id.to_string(), area))
        .collect();

    Ok(AvyHashMap { lookup_map })
}

// checks whether a point intersects with an avalanche forecast zone 
// and returns the area name, avalanche center, and forecast danger
pub async fn check_avy(point: &Point, avy_data: &AvyHashMap) //-> 
    //Result<Option<AvyProperties>, NACError
{
    
}
