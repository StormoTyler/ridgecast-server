use axum::
{
    extract::{Path, Query, Request, Extension, State},
    body::Body,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put, delete},
    Json, Router,
};
use serde::{Serialize, Deserialize};
use serde_json::json;
use mongodb::
{
    results::{DeleteResult, InsertOneResult,
    UpdateResult,},
    Client, Collection
};
use std::sync::Arc;
use futures::stream::{StreamExt, TryStreamExt};
use geojson::{GeoJson, Geometry, GeometryValue};
use geo;
use thiserror::Error;

// local modules
mod handlers;
use crate::handlers::nws;
mod database;
use crate::database::DB;
use crate::snapshots::Snapshot;
mod models;
mod sms;
mod snapshots;
mod daily_forecast;
mod geometry;
#[cfg(test)]
mod tests;
use crate::models::{UserPoint, Point, ForecastVars, ForecastProperties, PointGeometry, PointProperties,
    NWR, Alerts, Alert, GeometryTypes, AlertData,
    Geocode, AlertsSorted, Forecast, Properties,
    Elevation, IntVec, TimeInt, FloatVec, TimeFloat, ForecastResponse,
    NWSError, AvalancheData, AvyArea, AvyProperties};

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum APIError
{
    #[error("received error from NWS endpoint")]
    NWSError,
    #[error("received error from NAC endpoint")]
    NACError,
    #[error("could not retrieve forecast from NWS")]
    ForecastError,
    #[error("could not retrieve alerts from NWS")]
    AlertsError,
    #[error("upstream API timed out")]
    UpstreamTimeout,
    #[error("database error")]
    DatabaseError,
    #[error("lat must be between -90 and 90 and lon between -180 and 180")]
    InvalidLocation,
    #[error("location is outside National Weather Service coverage")]
    LocationNotCovered,
}

impl IntoResponse for APIError
{
    fn into_response(self) -> Response
    {
        let status = match self
        {
            // upstream API failed or returned bad data
            APIError::NWSError
            | APIError::NACError
            | APIError::ForecastError
            | APIError::AlertsError => StatusCode::BAD_GATEWAY,
            APIError::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
            APIError::DatabaseError => StatusCode::INTERNAL_SERVER_ERROR,
            APIError::InvalidLocation => StatusCode::BAD_REQUEST,
            APIError::LocationNotCovered => StatusCode::NOT_FOUND,
        };

        (status, self.to_string()).into_response()
    }
}

#[tokio::main]                                                                
async fn main() -> Result<(), Box<dyn std::error::Error>>                     
{                                                                             
    let db = DB::init().await?;
                                                                              
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 5001));            
    println!("Listening on: {}", addr);                                       
    let listener = tokio::net::TcpListener::bind(addr).await?;                
    axum::serve(listener, app(db)).await?;                                
    Ok(())                                                                    
}                                                                             

fn app(db: DB) -> Router
{
    Router::new()
        .route("/", get(homepage))
        .route("/forecast", get(new_forecast))
        .route("/forecast/scheduled", post(schedule_forecast))
        .route("/alerts", get(get_alerts))
        .route("/check-alerts", get(check_alerts))
        .route("/get-avy", get(get_avy))
        .route("/check-avy", get(check_avy))
        .with_state(db)
}

async fn homepage()
{

}

async fn new_forecast(State(db): State<DB>) -> Response
{
    let Snapshot { data: alerts, stale } = match snapshots::load_alerts(&db).await
    {
        Ok(snapshot) => snapshot,
        Err(e) => return e.into_response(),
    };

    let forecast = match
    nws::get_forecast(44.0, -123.0, &alerts).await
    {
        Ok(result) => result,
        Err(e) =>
        {
            eprintln!("new_forecast: failed to get forecast: {e}");
            return if e.is_timeout() { APIError::UpstreamTimeout }
                else { APIError::ForecastError }.into_response();
        }
    };
    //println!("{:?}", forecast);

    let user_point = UserPoint
    {
        user: "whatever".to_string(),
        tracker_name: None,
        // all options off: forecast only, no alert tracking
        forecast_vars: ForecastVars::default(),
        point: forecast.point.clone(),
        avy: None,
        time_added: chrono::Utc::now(),
        valid_duration: 0,
        next_daily_forecast: None,
    };
    if let Err(e) = add_alert_tracker(&user_point, &db).await
    {
        eprintln!("new_forecast: failed to save user point: {e}");
        return APIError::DatabaseError.into_response();
    }
    with_stale_header(Json(forecast).into_response(), stale)
}

async fn add_alert_tracker(user_point: &UserPoint, db: &DB) -> mongodb::error::Result<()>
{
    db.points_collection.insert_one(user_point).await?;
    Ok(())
}

async fn schedule_forecast()
{

}

async fn get_alerts(State(db): State<DB>) -> Response
{
    match snapshots::refresh_alerts(&db).await
    {
        Ok(alerts) => Json(alerts).into_response(),
        Err(e) => e.into_response(),
    }
}

// marks a response built from a snapshot that could not be refreshed,
// the header is left off fresh responses so their JSON and headers are unchanged
fn with_stale_header(mut response: Response, stale: bool) -> Response
{
    if stale
    {
        response.headers_mut().insert("x-snapshot-stale", HeaderValue::from_static("true"));
    }
    response
}

#[derive(Debug, Deserialize)]
struct Location
{
    lat: f32,
    lon: f32,
}

// GET /check-alerts?lat=44.0512&lon=-123.0921
async fn check_alerts(State(db): State<DB>, Query(location): Query<Location>)
    -> Response
{
    let Location { lat, lon } = location;
    // f32 parsing also accepts "NaN" and "inf", which fail is_finite
    if !(lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0)
    {
        return APIError::InvalidLocation.into_response();
    }

    // load the snapshot first, so an unavailable snapshot fails without
    // calling the NWS points endpoint
    let Snapshot { data: alerts, stale } = match snapshots::load_alerts(&db).await
    {
        Ok(snapshot) => snapshot,
        Err(e) => return e.into_response(),
    };

    let point = match nws::get_point(lat, lon).await
    {
        Ok(point) => point,
        Err(e) if e.is_not_found() => return APIError::LocationNotCovered.into_response(),
        Err(e) =>
        {
            eprintln!("check_alerts: failed to get point {lat},{lon}: {e}");
            return if e.is_timeout() { APIError::UpstreamTimeout }
                else { APIError::NWSError }.into_response();
        }
    };

    let active_alerts = nws::check_alerts(&alerts, &point);
    with_stale_header(Json(active_alerts).into_response(), stale)
}

async fn get_avy(State(db): State<DB>) -> Response
{
    match snapshots::refresh_avy(&db).await
    {
        Ok(avy) => Json(avy).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn check_avy()
{

}
