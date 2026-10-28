use axum::
{
    extract::{Path, Query, Request, Extension, State},
    body::Body,
    http::HeaderValue,
    response::{IntoResponse, Response},
    routing::{get, post, put, delete},
    Json, Router,
};
use serde::Deserialize;
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

// local modules
mod handlers;
use crate::handlers::nws;
mod database;
mod errors;
use crate::models::Snapshot;
mod models;
mod sms;
mod snapshots;
mod daily_forecast;
mod geometry;
mod tracking;
#[cfg(test)]
mod tests;
use crate::models::{UserPoint, Point, ForecastVars, ForecastProperties, PointGeometry, PointProperties,
    NWR, Alerts, Alert, GeometryTypes, AlertData,
    Geocode, AlertsSorted, Forecast, Properties,
    Elevation, IntVec, TimeInt, FloatVec, TimeFloat, ForecastResponse,
    NWSError, AvalancheData, AvyArea, AvyProperties};

#[tokio::main]                                                                
async fn main() -> Result<(), Box<dyn std::error::Error>>                     
{                                                                             
    // local development only. in production the host sets the variables,
    // and dotenvy never overwrites a variable that's already set
    dotenvy::dotenv().ok();
    // checked here so a missing value fails at startup, not on the first NWS or NAC request
    // or by silently connecting to the default local database
    for name in ["RIDGECAST_USER_AGENT", "DATABASE_URL", "RIDGECAST_DB"]
    {
        std::env::var(name).map_err(|_| format!("{name} must be set"))?;
    }

    let db = database::DB::init().await?;

    // refresh the snapshots in the background, so startup doesn't wait on NWS or NAC.
    // a failure is only logged, the loaders refresh missing or stale snapshots on use
    let refresh_db = db.clone();
    tokio::spawn(async move {
        let (alerts, avy) = tokio::join!(
            snapshots::refresh_alerts(&refresh_db),
            snapshots::refresh_avy(&refresh_db));
        if let Err(e) = alerts { eprintln!("startup: alerts refresh failed: {e}"); }
        if let Err(e) = avy { eprintln!("startup: avalanche refresh failed: {e}"); }
    });
                                                                              
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 5001));            
    println!("Listening on: {}", addr);                                       
    let listener = tokio::net::TcpListener::bind(addr).await?;                
    axum::serve(listener, app(db)).await?;                                
    Ok(())                                                                    
}                                                                             

fn app(db: database::DB) -> Router
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

async fn new_forecast(State(db): State<database::DB>) -> Response
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
            return if e.is_timeout() { errors::APIError::UpstreamTimeout }
                else { errors::APIError::ForecastError }.into_response();
        }
    };
    //println!("{:?}", forecast);

    let user_point = UserPoint
    {
        id: None,
        user: "whatever".to_string(),
        tracker_name: None,
        // all options off: forecast only, no alert tracking
        forecast_vars: ForecastVars::default(),
        point: forecast.point.clone(),
        avy: None,
        avy_lost: Default::default(),
        avy_off_season: Default::default(),
        time_added: chrono::Utc::now(),
        valid_duration: 0,
        next_daily_forecast: None,
    };
    if let Err(e) = add_alert_tracker(&user_point, &db).await
    {
        eprintln!("new_forecast: failed to save user point: {e}");
        return errors::APIError::DatabaseError.into_response();
    }
    with_stale_header(Json(forecast).into_response(), stale)
}

async fn add_alert_tracker(user_point: &UserPoint, db: &database::DB) -> mongodb::error::Result<()>
{
    db.points_collection.insert_one(user_point).await?;
    Ok(())
}

async fn schedule_forecast()
{

}

async fn get_alerts(State(db): State<database::DB>) -> Response
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
async fn check_alerts(State(db): State<database::DB>, Query(location): Query<Location>)
    -> Response
{
    let Location { lat, lon } = location;
    // f32 parsing also accepts "NaN" and "inf", which fail is_finite
    if !(lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0)
    {
        return errors::APIError::InvalidLocation.into_response();
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
        Err(e) if e.is_not_found() => return errors::APIError::LocationNotCovered.into_response(),
        Err(e) =>
        {
            eprintln!("check_alerts: failed to get point {lat},{lon}: {e}");
            return if e.is_timeout() { errors::APIError::UpstreamTimeout }
                else { errors::APIError::NWSError }.into_response();
        }
    };

    let active_alerts = nws::check_alerts(&alerts, &point);
    with_stale_header(Json(active_alerts).into_response(), stale)
}

async fn get_avy(State(db): State<database::DB>) -> Response
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
