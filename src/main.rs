use axum::
{
    extract::{Path, Query, Request, Extension, State},
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put, delete},
    Json, Router,
};
use serde::{Serialize, Deserialize};
use serde_json::json;
use mongodb::
{
    bson::doc, 
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
use crate::handlers::{nws, nac};
mod database;
use crate::database::DB;
mod models;
use crate::models::{UserPoint, Point, PointGeometry, PointProperties,
    NWR, Alerts, Alert, GeometryTypes, AlertData,
    Geocode, AlertsSorted, Forecast, Properties,
    Elevation, IntVec, TimeInt, FloatVec, TimeFloat, ForecastResponse,
    NWSError, AvalancheData, AvyArea, AvyProperties, AvyWarning};

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
    #[error("no alerts stored yet, fetch them with GET /alerts first")]
    NoAlertsStored,
    #[error("upstream API timed out")]
    UpstreamTimeout,
    #[error("database error")]
    DatabaseError,
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
            APIError::NoAlertsStored => StatusCode::SERVICE_UNAVAILABLE,
            APIError::DatabaseError => StatusCode::INTERNAL_SERVER_ERROR,
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
        .route("/check-alerts", post(check_alerts))
        .route("/get-avy", get(get_avy))
        .route("/check-avy", get(check_avy))
        .with_state(db)
}

async fn homepage()
{

}

async fn new_forecast(State(db): State<DB>) -> Response
{
    let alerts = match db.alerts_collection.find_one(doc! {}).await
    {
        Ok(Some(alerts)) => alerts,
        Ok(None) => return APIError::NoAlertsStored.into_response(),
        Err(e) =>
        {
            eprintln!("new_forecast: failed to read alerts from database: {e}");
            return APIError::DatabaseError.into_response();
        }
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
        point: forecast.point.clone(),
        avy: None,
    };
    if let Err(e) = add_alert_tracker(user_point, &db).await
    {
        eprintln!("new_forecast: failed to save user point: {e}");
        return APIError::DatabaseError.into_response();
    }
    Json(forecast).into_response()
}

async fn add_alert_tracker(user_point: UserPoint, db: &DB) -> mongodb::error::Result<()>
{
    db.points_collection.insert_one(&user_point).await?;
    Ok(())
}

async fn schedule_forecast()
{

}

async fn get_alerts(State(db): State<DB>) -> Response
{
    let alerts = match
        nws::get_alerts().await
        {
            Ok(result) => result,
            Err(e) =>
            {
                eprintln!("get_alerts: failed to get alerts: {e}");
                return if e.is_timeout() { APIError::UpstreamTimeout }
                    else { APIError::AlertsError }.into_response();
            }
        };

    // swap the stored snapshot in one operation, so a failed write
    // never leaves the collection empty
    if let Err(e) = db.alerts_collection
        .replace_one(doc! {}, &alerts)
        .upsert(true)
        .await
    {
        eprintln!("get_alerts: failed to store alerts: {e}");
        return APIError::DatabaseError.into_response();
    }
    Json(alerts).into_response()
    //let random_string = String::from("Sucessfully inserted alerts into database");
    //random_string.into_response()
}

async fn check_alerts(State(db): State<DB>, Json(user_point): Json<UserPoint>)
    -> Response
{
    let alerts = match db.alerts_collection.find_one(doc! {}).await
    {
        Ok(Some(alerts)) => alerts,
        Ok(None) => return APIError::NoAlertsStored.into_response(),
        Err(e) =>
        {
            eprintln!("check_alerts: failed to read alerts from database: {e}");
            return APIError::DatabaseError.into_response();
        }
    };

    let active_alerts = nws::check_alerts(&alerts, &user_point.point);
    Json(active_alerts).into_response()
}

async fn get_avy(State(db): State<DB>) -> Response
{
    let avy = match
        nac::get_avy().await
        {
            Ok(result) => result,
            Err(e) =>
            {
                eprintln!("get_avy: failed to get avalanche data: {e}");
                return if e.is_timeout() { APIError::UpstreamTimeout }
                    else { APIError::NACError }.into_response();
            }
        };

    // swap the stored snapshot in one operation, so a failed write
    // never leaves the collection empty
    if let Err(e) = db.avy_collection
        .replace_one(doc! {}, &avy)
        .upsert(true)
        .await
    {
        eprintln!("get_avy: failed to store avalanche data: {e}");
        return APIError::DatabaseError.into_response();
    }
    Json(avy).into_response()
}

async fn check_avy()
{

}
