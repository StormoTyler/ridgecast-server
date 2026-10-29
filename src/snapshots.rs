// loading and refreshing the stored NWS alerts and NAC avalanche snapshots
// a missing or stale snapshot is refreshed from the upstream API before it is used,
// and is only reported as stale when the refresh fails

use chrono::{DateTime, TimeDelta, Utc};
use mongodb::Collection;
use mongodb::bson::{Document, doc};
use serde::Serialize;

// local modules
use crate::database;
use crate::errors;
use crate::handlers::{nac, nws};
use crate::models::{
    AlertsSorted, AvalancheData, InvalidAlert, InvalidAvyArea, SKIPPED_FEATURES_FAIL_PERCENT,
    SkippedFeatures, Snapshot,
};

// NWS updates alerts continuously
pub const ALERTS_MAX_AGE: TimeDelta = TimeDelta::minutes(15);
// NAC issues forecasts about once a day, with occasional updates
pub const AVY_MAX_AGE: TimeDelta = TimeDelta::hours(2);

pub fn is_stale(fetched_at: DateTime<Utc>, max_age: TimeDelta, now: DateTime<Utc>) -> bool {
    now - fetched_at > max_age
}

// fetches the active alerts from NWS and replaces the stored snapshot
pub async fn refresh_alerts(db: &database::DB) -> Result<AlertsSorted, errors::APIError> {
    let alerts = fetch_alerts(db).await?;
    store_alerts(db, &alerts).await?;
    Ok(alerts)
}

// fetches the avalanche zones from NAC and replaces the stored snapshot
pub async fn refresh_avy(db: &database::DB) -> Result<AvalancheData, errors::APIError> {
    let avy = fetch_avy(db).await?;
    store_avy(db, &avy).await?;
    Ok(avy)
}

// reads the stored alerts, refreshing them first if they are missing, unreadable, or stale
// errors only when NWS can't be reached and there is no stored snapshot to fall back on
pub async fn load_alerts(db: &database::DB) -> Result<Snapshot<AlertsSorted>, errors::APIError> {
    if let Some(alerts) = read_alerts(db)
        .await
        .filter(|alerts| !is_stale(alerts.fetched_at, ALERTS_MAX_AGE, Utc::now()))
    {
        return Ok(Snapshot {
            data: alerts,
            stale: false,
        });
    }

    let refresh_error = match fetch_alerts(db).await {
        Ok(alerts) => {
            // the fresh alerts are used even if they could not be stored
            let _ = store_alerts(db, &alerts).await;
            return Ok(Snapshot {
                data: alerts,
                stale: false,
            });
        }
        Err(e) => e,
    };

    // check again before reporting the snapshot as stale,
    // another request may have refreshed it in the meantime
    match read_alerts(db).await {
        Some(alerts) => {
            let stale = is_stale(alerts.fetched_at, ALERTS_MAX_AGE, Utc::now());
            if stale {
                eprintln!(
                    "load_alerts: using stale alerts fetched at {}",
                    alerts.fetched_at
                );
            }
            Ok(Snapshot {
                data: alerts,
                stale,
            })
        }
        None => Err(refresh_error),
    }
}

// reads the stored avalanche zones, refreshing them first if they are missing,
// unreadable, or stale. errors only when NAC can't be reached and there is no
// stored snapshot to fall back on
pub async fn load_avy(db: &database::DB) -> Result<Snapshot<AvalancheData>, errors::APIError> {
    if let Some(avy) = read_avy(db)
        .await
        .filter(|avy| !is_stale(avy.fetched_at, AVY_MAX_AGE, Utc::now()))
    {
        return Ok(Snapshot {
            data: avy,
            stale: false,
        });
    }

    let refresh_error = match fetch_avy(db).await {
        Ok(avy) => {
            // the fresh zones are used even if they could not be stored
            let _ = store_avy(db, &avy).await;
            return Ok(Snapshot {
                data: avy,
                stale: false,
            });
        }
        Err(e) => e,
    };

    // check again before reporting the snapshot as stale,
    // another request may have refreshed it in the meantime
    match read_avy(db).await {
        Some(avy) => {
            let stale = is_stale(avy.fetched_at, AVY_MAX_AGE, Utc::now());
            if stale {
                eprintln!(
                    "load_avy: using stale avalanche data fetched at {}",
                    avy.fetched_at
                );
            }
            Ok(Snapshot { data: avy, stale })
        }
        None => Err(refresh_error),
    }
}

// fetches the alerts from NWS and stores any that failed validation or
// couldn't be deserialized. fails if too many couldn't be deserialized, so
// the loader falls back to the last good snapshot instead of storing a
// snapshot that is missing alerts
async fn fetch_alerts(db: &database::DB) -> Result<AlertsSorted, errors::APIError> {
    let (alerts, invalid_alerts, skipped) = nws::get_alerts().await.map_err(|e| {
        eprintln!("fetch_alerts: failed to get alerts: {e}");
        if e.is_timeout() {
            errors::APIError::UpstreamTimeout
        } else {
            errors::APIError::AlertsError
        }
    })?;
    store_skipped_features(db, &skipped).await;
    if skipped.too_many() {
        eprintln!(
            "fetch_alerts: {} of {} alerts couldn't be deserialized, at least \
            {SKIPPED_FEATURES_FAIL_PERCENT}%, so the alerts are not used",
            skipped.features.len(),
            skipped.total
        );
        return Err(errors::APIError::AlertsError);
    }

    // the invalid alerts are only kept for debugging, so failing
    // to store them doesn't fail the fetch
    for invalid_alert in invalid_alerts.iter() {
        if let Err(e) = store_invalid_alert(db, invalid_alert).await {
            eprintln!(
                "fetch_alerts: failed to store invalid alert {:?}: {e}",
                invalid_alert.alert_id
            );
        }
    }
    Ok(alerts)
}

// an alert with an id is upserted, so an alert that fails validation on every
// refresh keeps one record
async fn store_invalid_alert(
    db: &database::DB,
    invalid_alert: &InvalidAlert,
) -> mongodb::error::Result<()> {
    let Some(alert_id) = &invalid_alert.alert_id else {
        db.invalid_alerts_collection
            .insert_one(invalid_alert)
            .await?;
        return Ok(());
    };
    upsert_debug_record(
        &db.invalid_alerts_collection,
        doc! { "alert_id": alert_id },
        invalid_alert,
    )
    .await
}

// a zone with a name is upserted by its name and center, so a zone that fails
// on every refresh keeps one record
async fn store_invalid_avy_area(
    db: &database::DB,
    invalid_area: &InvalidAvyArea,
) -> mongodb::error::Result<()> {
    let Some(zone_name) = &invalid_area.zone_name else {
        db.invalid_avy_areas_collection
            .insert_one(invalid_area)
            .await?;
        return Ok(());
    };
    let filter = doc! { "zone_name": zone_name, "center": invalid_area.center.as_deref() };
    upsert_debug_record(&db.invalid_avy_areas_collection, filter, invalid_area).await
}

// stores the features that couldn't be deserialized. they are only kept for
// debugging, so a failure to store them is logged and doesn't fail the fetch.
// a feature with an id is upserted by its source and id, so a feature that
// fails on every refresh keeps one record
async fn store_skipped_features(db: &database::DB, skipped: &SkippedFeatures) {
    for feature in skipped.features.iter() {
        let result = match &feature.feature_id {
            Some(feature_id) => {
                let filter = doc! { "source": &feature.source, "feature_id": feature_id };
                upsert_debug_record(&db.skipped_features_collection, filter, feature).await
            }
            None => db
                .skipped_features_collection
                .insert_one(feature)
                .await
                .map(|_| ()),
        };
        if let Err(e) = result {
            eprintln!(
                "store_skipped_features: failed to store {} feature {:?}: {e}",
                feature.source, feature.feature_id
            );
        }
    }
}

// upserts a debugging record that has first_seen and last_seen fields,
// keeping the first_seen of an existing record and updating everything else
async fn upsert_debug_record<T>(
    collection: &Collection<T>,
    filter: Document,
    record: &T,
) -> mongodb::error::Result<()>
where
    T: Serialize + Send + Sync,
{
    let mut fields = bson::to_document(record)?;
    let first_seen = fields.remove("first_seen");
    collection
        .update_one(
            filter,
            doc! { "$set": fields, "$setOnInsert": { "first_seen": first_seen } },
        )
        .upsert(true)
        .await?;
    Ok(())
}

// fetches the avalanche zones from NAC and stores any whose geometry could not
// be converted or that couldn't be deserialized. fails if too many couldn't be
// deserialized, for the same reason as fetch_alerts
async fn fetch_avy(db: &database::DB) -> Result<AvalancheData, errors::APIError> {
    let (avy, invalid_areas, skipped) = nac::get_avy().await.map_err(|e| {
        eprintln!("fetch_avy: failed to get avalanche data: {e}");
        if e.is_timeout() {
            errors::APIError::UpstreamTimeout
        } else {
            errors::APIError::NACError
        }
    })?;
    store_skipped_features(db, &skipped).await;
    if skipped.too_many() {
        eprintln!(
            "fetch_avy: {} of {} avalanche zones couldn't be deserialized, at least \
            {SKIPPED_FEATURES_FAIL_PERCENT}%, so the avalanche data is not used",
            skipped.features.len(),
            skipped.total
        );
        return Err(errors::APIError::NACError);
    }

    // the invalid zones are only kept for debugging, so failing
    // to store them doesn't fail the fetch
    for invalid_area in invalid_areas.iter() {
        if let Err(e) = store_invalid_avy_area(db, invalid_area).await {
            eprintln!(
                "fetch_avy: failed to store invalid avalanche zone {:?}: {e}",
                invalid_area.zone_name
            );
        }
    }
    Ok(avy)
}

// swaps the stored snapshot in one operation, so a failed write
// never leaves the collection empty
async fn store_alerts(db: &database::DB, alerts: &AlertsSorted) -> Result<(), errors::APIError> {
    db.alerts_collection
        .replace_one(doc! {}, alerts)
        .upsert(true)
        .await
        .map(|_| ())
        .map_err(|e| {
            eprintln!("store_alerts: failed to store alerts: {e}");
            errors::APIError::DatabaseError
        })
}

async fn store_avy(db: &database::DB, avy: &AvalancheData) -> Result<(), errors::APIError> {
    db.avy_collection
        .replace_one(doc! {}, avy)
        .upsert(true)
        .await
        .map(|_| ())
        .map_err(|e| {
            eprintln!("store_avy: failed to store avalanche data: {e}");
            errors::APIError::DatabaseError
        })
}

// a read error is logged and handled like a missing snapshot, so the
// snapshot is refreshed. this also replaces a document in an outdated shape
async fn read_alerts(db: &database::DB) -> Option<AlertsSorted> {
    db.alerts_collection
        .find_one(doc! {})
        .await
        .unwrap_or_else(|e| {
            eprintln!("read_alerts: failed to read alerts from database: {e}");
            None
        })
}

async fn read_avy(db: &database::DB) -> Option<AvalancheData> {
    db.avy_collection
        .find_one(doc! {})
        .await
        .unwrap_or_else(|e| {
            eprintln!("read_avy: failed to read avalanche data from database: {e}");
            None
        })
}

// when a stored snapshot was fetched, read without loading the snapshot itself,
// which for alerts can be close to a megabyte. None when it's missing or unreadable
pub async fn read_fetched_at<T: Send + Sync>(collection: &Collection<T>) -> Option<DateTime<Utc>> {
    let document = collection
        .clone_with_type::<Document>()
        .find_one(doc! {})
        .projection(doc! { "fetched_at": 1 })
        .await
        .ok()??;
    document
        .get_datetime("fetched_at")
        .ok()
        .map(|fetched_at| fetched_at.to_chrono())
}
