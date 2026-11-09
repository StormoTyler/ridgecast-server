use mongodb::{
    Client, Collection, IndexModel,
    bson::doc,
    error::Result,
    options::{ClientOptions, IndexOptions},
};
use std::time::Duration;

// local modules
use crate::models::{
    AlertsSorted, AvalancheData, InvalidAlert, InvalidAvyArea, SkippedFeature, UserPoint,
};

// debugging records are deleted this long after an alert or zone last failed validation
const DEBUG_RECORD_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Clone, Debug)]
pub struct DB {
    pub alerts_collection: Collection<AlertsSorted>,
    pub points_collection: Collection<UserPoint>,
    pub avy_collection: Collection<AvalancheData>,
    // alerts that failed validation, kept for debugging
    pub invalid_alerts_collection: Collection<InvalidAlert>,
    // avalanche zones whose geometry could not be converted, kept for debugging
    pub invalid_avy_areas_collection: Collection<InvalidAvyArea>,
    // NWS and NAC features that couldn't be deserialized, kept for debugging
    pub skipped_features_collection: Collection<SkippedFeature>,
}

impl DB {
    pub async fn init() -> Result<Self> {
        // both are checked at startup in main
        let mongodb_uri = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let database_name = std::env::var("RIDGECAST_DB").expect("RIDGECAST_DB must be set");

        let mut client_options = ClientOptions::parse(mongodb_uri).await?;
        client_options.app_name = Some(database_name.clone());

        let client = Client::with_options(client_options)?;
        let database = client.database(database_name.as_str());

        // pinging the database
        database.run_command(doc! { "ping": 1 }).await?;

        let alerts_collection = database.collection::<AlertsSorted>("alerts");
        let points_collection = database.collection::<UserPoint>("points");
        let avy_collection = database.collection::<AvalancheData>("avy");
        let invalid_alerts_collection = database.collection::<InvalidAlert>("invalid_alerts");
        let invalid_avy_areas_collection =
            database.collection::<InvalidAvyArea>("invalid_avy_areas");
        let skipped_features_collection = database.collection::<SkippedFeature>("skipped_features");

        // the scheduler finds due daily forecasts with next_daily_forecast <= now.
        // creating an index that already exists does nothing
        points_collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "next_daily_forecast": 1 })
                    .build(),
            )
            .await?;
        // each refresh upserts invalid alerts by alert_id
        invalid_alerts_collection
            .create_index(IndexModel::builder().keys(doc! { "alert_id": 1 }).build())
            .await?;
        // each refresh upserts invalid avalanche zones by zone_name and center
        invalid_avy_areas_collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "zone_name": 1, "center": 1 })
                    .build(),
            )
            .await?;

        // each refresh upserts skipped features by source and feature_id
        skipped_features_collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "source": 1, "feature_id": 1 })
                    .build(),
            )
            .await?;
        // mongo deletes debugging records once last_seen is older than DEBUG_RECORD_TTL.
        // every refresh that sees the problem again updates last_seen, so only
        // problems NWS or NAC no longer send expire
        let ttl_index = || {
            IndexModel::builder()
                .keys(doc! { "last_seen": 1 })
                .options(
                    IndexOptions::builder()
                        .expire_after(DEBUG_RECORD_TTL)
                        .build(),
                )
                .build()
        };
        invalid_alerts_collection.create_index(ttl_index()).await?;
        invalid_avy_areas_collection
            .create_index(ttl_index())
            .await?;
        skipped_features_collection
            .create_index(ttl_index())
            .await?;

        tracing::info!("Pinged your database. Successfully connected to MongoDB!");

        Ok(Self {
            alerts_collection,
            points_collection,
            avy_collection,
            invalid_alerts_collection,
            invalid_avy_areas_collection,
            skipped_features_collection,
        })
    }
}

// saves a new tracker to the points collection
pub async fn add_tracker(user_point: &UserPoint, db: &DB) -> Result<()> {
    db.points_collection.insert_one(user_point).await?;
    Ok(())
}
