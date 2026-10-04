// handler for National Avalanche Center api

use reqwest::Client;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json;
use std::io;
use thiserror::Error;
use geojson;
use geo;
use geojson::GeometryValue;
use chrono_tz::Tz;

// local modules
use crate::models::{AvalancheData, AvyArea, AvyProperties, AvyWarning, DateTimeTypes, GeometryTypes};

const BASE_URL: &str = "https://api.avalanche.org/v2/public/products/map-layer";

static NAC_CLIENT: Lazy<Client> = Lazy::new(||                        
{                                                                     
    Client::builder()                                                 
        .user_agent("User-Agent: (TrailAlert, stormotyler@gmail.com)")
        .build()                                                      
        .expect("Failed to build client")                             
});                                                                   

#[derive(Debug, Error, Serialize, Deserialize)]
pub enum NACError
{
    #[error("could not retrieve data from NAC API")]
    APICallError,
    #[error("json parsing error")]
    JsonError,
    #[error("could not complete avalanche danger check")]
    CheckError,
}

pub async fn get_avy() -> Result<AvalancheData, NACError>
{
    let avy_response =
        NAC_CLIENT
        .get(BASE_URL)
        .send()
        .await
        .map_err(|_e| NACError::APICallError)?;

    let avy_json: serde_json::Value =
        avy_response
        .json()
        .await
        .map_err(|_e| NACError::JsonError)?;

    //println!("{avy_json}");

    let mut avy: AvalancheData = match
        serde_json::from_value(avy_json)
        {
            Ok(avy) => avy,
            Err(_e) => 
            {
                println!("error serializing");
                return Err(NACError::JsonError)
            }
        };

    println!("{:?}", avy);

    for area in avy.features.iter_mut()
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

    Ok(avy)
}

// checks whether a point is inside an avalanche forecast zone 
// and returns the area name, avalanche center, and forecast danger
pub async fn check_avy()
{
}
