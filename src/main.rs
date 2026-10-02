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
use crate::handlers::nws;
mod models;
use crate::models::{UserPoint, Point, PointGeometry, PointProperties,
    NWR, Alerts, Alert, AlertGeometry, AlertData,
    Geocode, AlertsSorted, AlertCategory, Forecast, Properties,
    Elevation, IntVec, TimeInt, FloatVec, TimeFloat, ForecastResponse,
    NWSError};

#[derive(Debug, Clone)]
pub struct AppState 
{
    mongo: Arc<Client>,
    collections: Collection,
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum APIError
{
    #[error("received error from NWS endpoint")]
    NWSError,
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
    let db_connection_str = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "mongodb://127.0.0.1:27017".to_string()                           
        });                                                                   
    let client = Client::with_uri_str(db_connection_str).await.unwrap();      
                                                                              
    // pinging the database                                                   
    client                                                                    
        .database("trail-alert")                                              
        .run_command(doc! { "ping": 1 })                                      
        .await                                                                
        .unwrap();                                                            
    println!("Pinged your database. Successfully connected to MongoDB!");     
                                                                              
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 5001));            
    println!("Listening on: {}", addr);                                       
    let listener = tokio::net::TcpListener::bind(addr).await?;                
    axum::serve(listener, app(client)).await?;                                
    Ok(())                                                                    
}                                                                             

fn app(client: Client, state: AppState) -> Router
{
    Router::new()
        .route("/", get(homepage))
        .route("/forecast", get(new_forecast))
        .route("/forecast/scheduled", post(schedule_forecast))
        .route("/alerts", get(get_alerts))
        .route("/check-alerts", get(check_alerts))
        .route("/get-avy", get(get_avy))
        .route("/check-avy", get(check_avy))
        .with_state(state)
}

async fn homepage()
{

}

async fn new_forecast(State(db): State<Collection<AlertsSorted>>) -> Response
{
    let alerts = match db.find_one(doc! {}).await
    {                                            
        Ok(alerts) => alerts,                    
        Err(_e) => None,                         
    };                                           
    
    let forecast = match 
    nws::get_forecast(44.0, -123.0, alerts.unwrap()).await
    {
        Ok(result) => result,
        Err(_e) => return APIError::ForecastError.into_response(),
    };
    //println!("{:?}", forecast);

    let user_point = UserPoint
    {
        user: "whatever".to_string(),
        point: forecast.point.clone(),
    };

    Json(forecast).into_response()
}

async fn add_alert_tracker()
{

}

async fn schedule_forecast()
{

}

async fn get_alerts(State(db): State<Collection<AlertsSorted>>) -> Response
{
    let alerts = match
        nws::get_alerts().await
        {
            Ok(result) => result,
            Err(_e) => return APIError::AlertsError.into_response(),
        };

    db.delete_many(doc! {}).await;

    db.insert_one(&alerts).await;
    Json(alerts).into_response()
    //let random_string = String::from("Sucessfully inserted alerts into database");
    //random_string.into_response()
}

async fn check_alerts(State(db): State<Collection<AlertsSorted>>)
{
    let alerts = match db.find_one(doc! {}).await
    {
        Ok(alerts) => alerts,
        Err(_e) => None,
    };

    if alerts.is_some()
    {
        //api::nws::check_alerts(alerts.unwrap()).await;
        println!("checked alerts");
    }
    //println!("{:?}", alerts);
    println!("didnt check alerts");
}

async fn get_avy()
{

}

async fn store_avy()
{

}

async fn check_avy()
{

}
