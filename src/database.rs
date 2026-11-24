use chrono::{DateTime, Utc};
use mongodb::{
    Client, Collection, IndexModel,
    bson::doc,
    error::Result,
    options::{ClientOptions, IndexOptions, ServerAddress, Tls},
};
use std::net::IpAddr;
use std::time::Duration;
use thiserror::Error;

// local modules
use crate::messaging;
use crate::models::{
    AlertsSorted, AvalancheData, InboundMessageRecord, InvalidAlert, InvalidAvyArea,
    SkippedFeature, UserNumber, UserPoint,
};

// debugging records are deleted this long after an alert or zone last failed validation
const DEBUG_RECORD_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
// a user's number record is deleted this long after the number last texted
pub const USER_NUMBER_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
// an inbound message's record is deleted this long after it was handled
// long TTL chosen due to unknown retry timing across SMS providers
pub const INBOUND_MESSAGE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

// MongoDB's error code for an insert whose _id is already stored
const DUPLICATE_KEY: i32 = 11000;

#[derive(Clone, Debug)]
pub struct DB {
    pub alerts_collection: Collection<AlertsSorted>,
    pub points_collection: Collection<UserPoint>,
    // the users' phone numbers, stored as HMACs (see sms::user_numbers)
    pub user_numbers_collection: Collection<UserNumber>,
    // the inbound messages handled in the last INBOUND_MESSAGE_TTL, by provider id
    pub inbound_messages_collection: Collection<InboundMessageRecord>,
    pub avy_collection: Collection<AvalancheData>,
    // alerts that failed validation, kept for debugging
    pub invalid_alerts_collection: Collection<InvalidAlert>,
    // avalanche zones whose geometry could not be converted, kept for debugging
    pub invalid_avy_areas_collection: Collection<InvalidAvyArea>,
    // NWS and NAC features that couldn't be deserialized, kept for debugging
    pub skipped_features_collection: Collection<SkippedFeature>,
}

// why the server couldn't connect to MongoDB. neither variant's message includes
// the URI, which can hold the database password
#[derive(Debug, Error)]
pub enum ConnectError {
    #[error(transparent)]
    Mongo(#[from] mongodb::error::Error),
    // the privacy policy promises encrypted connections between services, so a
    // database on another machine is only reached over TLS with its certificate checked
    #[error(
        "a MongoDB that isn't on this machine needs TLS: add tls=true to the URI or use a \
         mongodb+srv:// URI, and leave certificate checks on (no tlsInsecure or \
         tlsAllowInvalidCertificates)"
    )]
    Unencrypted,
}

impl DB {
    pub async fn init() -> std::result::Result<Self, ConnectError> {
        // both are checked at startup in main
        let mongodb_uri = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let database_name = std::env::var("RIDGECAST_DB").expect("RIDGECAST_DB must be set");
        Self::connect(&mongodb_uri, &database_name).await
    }

    // connects to the named database and creates its indexes. apart from init,
    // which reads the environment, so each database test can use its own database.
    // the URI is checked for TLS before anything is sent to the server
    pub async fn connect(
        mongodb_uri: &str,
        database_name: &str,
    ) -> std::result::Result<Self, ConnectError> {
        let mut client_options = ClientOptions::parse(mongodb_uri).await?;
        check_tls(&client_options)?;
        client_options.app_name = Some(database_name.to_string());

        let client = Client::with_options(client_options)?;
        let database = client.database(database_name);

        // pinging the database
        database.run_command(doc! { "ping": 1 }).await?;

        let alerts_collection = database.collection::<AlertsSorted>("alerts");
        let points_collection = database.collection::<UserPoint>("points");
        let user_numbers_collection = database.collection::<UserNumber>("user_numbers");
        let inbound_messages_collection =
            database.collection::<InboundMessageRecord>("inbound_messages");
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
        let ttl_index = |ttl| {
            IndexModel::builder()
                .keys(doc! { "last_seen": 1 })
                .options(IndexOptions::builder().expire_after(ttl).build())
                .build()
        };
        invalid_alerts_collection
            .create_index(ttl_index(DEBUG_RECORD_TTL))
            .await?;
        invalid_avy_areas_collection
            .create_index(ttl_index(DEBUG_RECORD_TTL))
            .await?;
        skipped_features_collection
            .create_index(ttl_index(DEBUG_RECORD_TTL))
            .await?;
        // the same for user numbers: each text updates last_seen, so a record
        // is deleted USER_NUMBER_TTL after its number last texted
        user_numbers_collection
            .create_index(ttl_index(USER_NUMBER_TTL))
            .await?;
        // an inbound message's record is deleted INBOUND_MESSAGE_TTL after it
        // was handled, once no provider would deliver it again
        inbound_messages_collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "claimed_at": 1 })
                    .options(
                        IndexOptions::builder()
                            .expire_after(INBOUND_MESSAGE_TTL)
                            .build(),
                    )
                    .build(),
            )
            .await?;
        // trackers are deleted at expires_at. nothing checks an expired tracker:
        // the last check before expiry sends the tracker's final message
        points_collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "expires_at": 1 })
                    .options(IndexOptions::builder().expire_after(Duration::ZERO).build())
                    .build(),
            )
            .await?;

        tracing::info!("Pinged your database. Successfully connected to MongoDB!");

        Ok(Self {
            alerts_collection,
            points_collection,
            user_numbers_collection,
            inbound_messages_collection,
            avy_collection,
            invalid_alerts_collection,
            invalid_avy_areas_collection,
            skipped_features_collection,
        })
    }
}

// a database on this machine may be reached without TLS, any other only over TLS
// with its certificate checked. the hostname is always checked: the driver's TLS
// here (rustls) has no option to skip it. read from the parsed options, so a
// mongodb+srv:// URI, which turns TLS on unless it says tls=false, passes
// without tls=true. a URI naming several hosts passes only if every connection
// to them would
pub fn check_tls(options: &ClientOptions) -> std::result::Result<(), ConnectError> {
    if options.hosts.iter().all(is_on_this_machine) {
        return Ok(());
    }
    match &options.tls {
        Some(Tls::Enabled(tls)) if tls.allow_invalid_certificates != Some(true) => Ok(()),
        _ => Err(ConnectError::Unencrypted),
    }
}

// localhost, a loopback address, or a Unix socket. names are compared as
// written, never looked up, so a name that resolves to this machine still needs TLS
fn is_on_this_machine(address: &ServerAddress) -> bool {
    match address {
        ServerAddress::Tcp { host, .. } => {
            let host = host.trim_start_matches('[').trim_end_matches(']');
            host.eq_ignore_ascii_case("localhost")
                || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        }
        ServerAddress::Unix { .. } => true,
        // an address kind added in a later driver version counts as another machine
        _ => false,
    }
}

// saves a new tracker to the points collection
pub async fn add_tracker(user_point: &UserPoint, db: &DB) -> Result<()> {
    db.points_collection.insert_one(user_point).await?;
    Ok(())
}

// saves a checked tracker back over the stored one, matched by its _id. returns
// false when nothing was saved because the tracker is no longer stored. never an
// upsert: a tracker deleted while it was being checked, by its TTL index or by
// delete_user_trackers after the user replied STOP, must stay deleted, or a
// stopped tracker would come back and text a user who opted out
pub async fn update_tracker(user_point: &UserPoint, db: &DB) -> Result<bool> {
    // a tracker without an _id was never inserted, so there is nothing to save over
    let Some(id) = user_point.id else {
        tracing::warn!("update_tracker: tracker has no _id, so it was never stored");
        return Ok(false);
    };
    let result = db
        .points_collection
        .replace_one(doc! { "_id": id }, user_point)
        .await?;
    if result.matched_count == 0 {
        tracing::debug!("update_tracker: tracker {id} is no longer stored, not saved");
        return Ok(false);
    }
    Ok(true)
}

// deletes every tracker of a user, for a user who replied STOP. trackers are
// deleted rather than paused, and nothing brings them back if the user opts in
// again. returns how many were deleted
pub async fn delete_user_trackers(user: &messaging::PhoneNumber, db: &DB) -> Result<u64> {
    // the number is serialized the way UserPoint.user is stored, a plain E.164
    // string, without exposing it in this module
    let result = db
        .points_collection
        .delete_many(doc! { "user": bson::to_bson(user)? })
        .await?;
    Ok(result.deleted_count)
}

// records that an inbound message is being handled, by the provider's id for
// it. returns false when it was already recorded: the provider delivered the
// same message again, and it must not be answered twice. _id is unique, so two
// deliveries arriving together can't both claim it
pub async fn claim_inbound(provider_message_id: &str, now: DateTime<Utc>, db: &DB) -> Result<bool> {
    let record = InboundMessageRecord {
        id: provider_message_id.to_string(),
        claimed_at: now,
    };
    match db.inbound_messages_collection.insert_one(record).await {
        Ok(_) => Ok(true),
        Err(error) if is_duplicate_key(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn is_duplicate_key(error: &mongodb::error::Error) -> bool {
    matches!(
        error.kind.as_ref(),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(write_error))
            if write_error.code == DUPLICATE_KEY
    )
}
