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
    #[error("error: not a Forecast")]
    ForecastError,
    #[error("error: could not retrieve alerts")]
    AlertsError,
    #[error("error retrieving from database")]
    DatabaseError,
}

impl IntoResponse for APIError
{
    fn into_response(self) -> Response
    {
        let body = match self
        {
            APIError::NWSError => "recieved error from NWS endpoint",
            APIError::NACError => "received error from NAC endpoint",
            APIError::ForecastError => "error, not a forecast",
            APIError::AlertsError => "error, coud not retrieve alerts",
            APIError::DatabaseError => "error retreiving from database",
        };

        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
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
        Ok(None) => return APIError::AlertsError.into_response(),
        Err(_e) => return APIError::DatabaseError.into_response(),
    };

    let forecast = match
    nws::get_forecast(44.0, -123.0, alerts).await
    {
        Ok(result) => result,
        Err(_e) => return APIError::ForecastError.into_response(),
    };
    //println!("{:?}", forecast);

    let user_point = UserPoint
    {
        user: "whatever".to_string(),
        point: forecast.point.clone(),
        avy: None,
    };
    add_alert_tracker(user_point, &db).await;
    Json(forecast).into_response()
}

async fn add_alert_tracker(user_point: UserPoint, db: &DB)
{
    db.points_collection.insert_one(&user_point).await;
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
            Err(_e) => return APIError::AlertsError.into_response(),
        };

    db.alerts_collection.delete_many(doc! {}).await;

    db.alerts_collection.insert_one(&alerts).await;
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
        Ok(None) => return APIError::AlertsError.into_response(),
        Err(_e) => return APIError::DatabaseError.into_response(),
    };

    let active_alerts = nws::check_alerts(alerts, user_point.point).await;
    Json(active_alerts).into_response()
}

async fn get_avy(State(db): State<DB>) -> Response
{
    let avy = match
        nac::get_avy().await
        {
            Ok(result) => result,
            Err(_e) => return APIError::NACError.into_response(),
        };

    db.avy_collection.delete_many(doc! {}).await;

    db.avy_collection.insert_one(&avy).await;
    Json(avy).into_response()
}

async fn store_avy()
{

}

async fn check_avy()
{

}
