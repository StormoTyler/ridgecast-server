// handler for National Avalanche Center api

use reqwest;
use serde::{Deserialize, Serialize};
use serde_json;
use std::io;
use thiserror::Error;
use geojson;
use geo;

const BASE_URL: &str = "https://api.avalanche.org/v2/public/products/map-layer";

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

pub async fn get_avy() -> Result<serde_json::Value, NACError>
{
    let avy_map: Result<serde_json::Value, NACError> = match reqwest::get(BASE_URL)
        .await
        {
            Ok(response) => match response.json::<serde_json::Value>().await
            {
                Ok(json) => Ok(json),
                Err(e) => Err(NACError::JsonError),
            }
            Err(e) => Err(NACError::APICallError),
        };
    return avy_map
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AvalancheRisk
{
    area: Option<String>,
    avalanche_center: Option<String>,
    avalanche_danger: Option<String>,
    travel_advice: Option<String>,
    warning: Option<String>,
}
// checks whether a point is inside an avalanche forecast zone 
// and returns the area name, avalanche center, and forecast danger
pub async fn check_avy() -> Result<AvalancheRisk, NACError>
{
    Ok(AvalancheRisk
    {
        area: Some(String::from("placeholder")),
        avalanche_center: Some(String::from("placeholder")),
        avalanche_danger: Some(String::from("placeholder")),
        travel_advice: Some(String::from("placeholder")),
        warning: Some(String::from("placeholder")),
    })
}
