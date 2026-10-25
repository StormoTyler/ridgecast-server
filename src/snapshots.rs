// loading and refreshing the stored NWS alerts and NAC avalanche snapshots
// a missing or stale snapshot is refreshed from the upstream API before it is used,
// and is only reported as stale when the refresh fails

use chrono::{DateTime, TimeDelta, Utc};
use mongodb::bson::doc;

// local modules
use crate::APIError;
use crate::database::DB;
use crate::handlers::{nac, nws};
use crate::models::{AlertsSorted, AvalancheData};

// NWS updates alerts continuously
pub const ALERTS_MAX_AGE: TimeDelta = TimeDelta::minutes(15);
// NAC issues forecasts about once a day, with occasional updates
pub const AVY_MAX_AGE: TimeDelta = TimeDelta::hours(2);

// a snapshot read for use, stale is True when it is older than its max age
// because it could not be refreshed
pub struct Snapshot<T>
{
    pub data: T,
    pub stale: bool,
}

pub fn is_stale(fetched_at: DateTime<Utc>, max_age: TimeDelta, now: DateTime<Utc>) -> bool
{
    now - fetched_at > max_age
}

// fetches the active alerts from NWS and replaces the stored snapshot
pub async fn refresh_alerts(db: &DB) -> Result<AlertsSorted, APIError>
{
    let alerts = fetch_alerts().await?;
    store_alerts(db, &alerts).await?;
    Ok(alerts)
}

// fetches the avalanche zones from NAC and replaces the stored snapshot
pub async fn refresh_avy(db: &DB) -> Result<AvalancheData, APIError>
{
    let avy = fetch_avy().await?;
    store_avy(db, &avy).await?;
    Ok(avy)
}

// reads the stored alerts, refreshing them first if they are missing, unreadable, or stale
// errors only when NWS can't be reached and there is no stored snapshot to fall back on
pub async fn load_alerts(db: &DB) -> Result<Snapshot<AlertsSorted>, APIError>
{
    if let Some(alerts) = read_alerts(db).await
        .filter(|alerts| !is_stale(alerts.fetched_at, ALERTS_MAX_AGE, Utc::now()))
    {
        return Ok(Snapshot { data: alerts, stale: false });
    }

    let refresh_error = match fetch_alerts().await
    {
        Ok(alerts) =>
        {
            // the fresh alerts are used even if they could not be stored
            let _ = store_alerts(db, &alerts).await;
            return Ok(Snapshot { data: alerts, stale: false });
        }
        Err(e) => e,
    };

    // check again before reporting the snapshot as stale,
    // another request may have refreshed it in the meantime
    match read_alerts(db).await
    {
        Some(alerts) =>
        {
            let stale = is_stale(alerts.fetched_at, ALERTS_MAX_AGE, Utc::now());
            if stale
            {
                eprintln!("load_alerts: using stale alerts fetched at {}", alerts.fetched_at);
            }
            Ok(Snapshot { data: alerts, stale })
        }
        None => Err(refresh_error),
    }
}

// reads the stored avalanche zones, refreshing them first if they are missing,
// unreadable, or stale. errors only when NAC can't be reached and there is no
// stored snapshot to fall back on
pub async fn load_avy(db: &DB) -> Result<Snapshot<AvalancheData>, APIError>
{
    if let Some(avy) = read_avy(db).await
        .filter(|avy| !is_stale(avy.fetched_at, AVY_MAX_AGE, Utc::now()))
    {
        return Ok(Snapshot { data: avy, stale: false });
    }

    let refresh_error = match fetch_avy().await
    {
        Ok(avy) =>
        {
            // the fresh zones are used even if they could not be stored
            let _ = store_avy(db, &avy).await;
            return Ok(Snapshot { data: avy, stale: false });
        }
        Err(e) => e,
    };

    // check again before reporting the snapshot as stale,
    // another request may have refreshed it in the meantime
    match read_avy(db).await
    {
        Some(avy) =>
        {
            let stale = is_stale(avy.fetched_at, AVY_MAX_AGE, Utc::now());
            if stale
            {
                eprintln!("load_avy: using stale avalanche data fetched at {}", avy.fetched_at);
            }
            Ok(Snapshot { data: avy, stale })
        }
        None => Err(refresh_error),
    }
}

async fn fetch_alerts() -> Result<AlertsSorted, APIError>
{
    nws::get_alerts().await.map_err(|e|
    {
        eprintln!("fetch_alerts: failed to get alerts: {e}");
        if e.is_timeout() { APIError::UpstreamTimeout } else { APIError::AlertsError }
    })
}

async fn fetch_avy() -> Result<AvalancheData, APIError>
{
    nac::get_avy().await.map_err(|e|
    {
        eprintln!("fetch_avy: failed to get avalanche data: {e}");
        if e.is_timeout() { APIError::UpstreamTimeout } else { APIError::NACError }
    })
}

// swaps the stored snapshot in one operation, so a failed write
// never leaves the collection empty
async fn store_alerts(db: &DB, alerts: &AlertsSorted) -> Result<(), APIError>
{
    db.alerts_collection
        .replace_one(doc! {}, alerts)
        .upsert(true)
        .await
        .map(|_| ())
        .map_err(|e|
        {
            eprintln!("store_alerts: failed to store alerts: {e}");
            APIError::DatabaseError
        })
}

async fn store_avy(db: &DB, avy: &AvalancheData) -> Result<(), APIError>
{
    db.avy_collection
        .replace_one(doc! {}, avy)
        .upsert(true)
        .await
        .map(|_| ())
        .map_err(|e|
        {
            eprintln!("store_avy: failed to store avalanche data: {e}");
            APIError::DatabaseError
        })
}

// a read error is logged and handled like a missing snapshot, so the
// snapshot is refreshed. this also replaces a document in an outdated shape
async fn read_alerts(db: &DB) -> Option<AlertsSorted>
{
    db.alerts_collection.find_one(doc! {}).await.unwrap_or_else(|e|
    {
        eprintln!("read_alerts: failed to read alerts from database: {e}");
        None
    })
}

async fn read_avy(db: &DB) -> Option<AvalancheData>
{
    db.avy_collection.find_one(doc! {}).await.unwrap_or_else(|e|
    {
        eprintln!("read_avy: failed to read avalanche data from database: {e}");
        None
    })
}
