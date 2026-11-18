// tests for database::DB against a real MongoDB: its indexes and saving a
// tracker. ignored by default, see test_db
use std::time::Duration;

use mongodb::bson::{Document, doc};

use crate::database;
use crate::models::UserPoint;
use crate::tests::{models_tests, test_db};

// the key and expiry of each of a collection's indexes
async fn indexes<T: Send + Sync>(
    collection: &mongodb::Collection<T>,
) -> Vec<(Document, Option<Duration>)> {
    let mut cursor = collection
        .list_indexes()
        .await
        .expect("indexes should list");
    let mut indexes = Vec::new();
    while cursor.advance().await.expect("cursor should advance") {
        let index = cursor
            .deserialize_current()
            .expect("an index should deserialize");
        let expire_after = index.options.and_then(|options| options.expire_after);
        indexes.push((index.keys, expire_after));
    }
    indexes
}

const THIRTY_DAYS: Duration = Duration::from_secs(30 * 24 * 60 * 60);

// a tracker is deleted at expires_at, and a user's number record 30 days after
// the number last texted, both by MongoDB, so the indexes are the retention policy
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn the_retention_indexes_exist() {
    let db = test_db::fresh("retention_indexes").await;

    let points = indexes(&db.points_collection).await;
    assert!(points.contains(&(doc! { "expires_at": 1 }, Some(Duration::ZERO))));
    assert!(points.contains(&(doc! { "next_daily_forecast": 1 }, None)));

    assert_eq!(database::USER_NUMBER_TTL, THIRTY_DAYS);
    let user_numbers = indexes(&db.user_numbers_collection).await;
    assert!(user_numbers.contains(&(doc! { "last_seen": 1 }, Some(THIRTY_DAYS))));

    let last_seen = (doc! { "last_seen": 1 }, Some(THIRTY_DAYS));
    assert!(
        indexes(&db.invalid_alerts_collection)
            .await
            .contains(&last_seen)
    );
    assert!(
        indexes(&db.invalid_avy_areas_collection)
            .await
            .contains(&last_seen)
    );
    assert!(
        indexes(&db.skipped_features_collection)
            .await
            .contains(&last_seen)
    );

    test_db::remove(db).await;
}

// every server start connects to a database that already has its indexes
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn connecting_again_keeps_the_indexes() {
    let db = test_db::fresh("connecting_again").await;
    let before = indexes(&db.points_collection).await;

    let namespace = db.points_collection.namespace();
    let again = database::DB::connect(&test_db::url(), &namespace.db)
        .await
        .expect("connecting to an existing database should succeed");

    assert_eq!(indexes(&again.points_collection).await, before);
    test_db::remove(db).await;
}

#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_saved_tracker_is_read_back_with_its_id() {
    let db = test_db::fresh("saved_tracker").await;
    let tracker = models_tests::tracker();

    database::add_tracker(&tracker, &db)
        .await
        .expect("the tracker should save");

    let read: UserPoint = db
        .points_collection
        .find_one(doc! {})
        .await
        .expect("the tracker should be read")
        .expect("the tracker should be stored");
    // the scheduler saves a checked tracker back by this id
    assert!(read.id.is_some());
    assert_eq!(read.user, tracker.user);
    assert_eq!(read.reply_from, tracker.reply_from);
    assert_eq!(read.tracker_name, tracker.tracker_name);
    // MongoDB keeps milliseconds
    assert_eq!(
        read.expires_at.timestamp_millis(),
        tracker.expires_at.timestamp_millis()
    );

    // the TTL index only deletes by a date, and the numbers are plain strings
    let stored = db
        .points_collection
        .clone_with_type::<Document>()
        .find_one(doc! {})
        .await
        .expect("the tracker should be read")
        .expect("the tracker should be stored");
    assert!(stored.get_datetime("expires_at").is_ok());
    assert_eq!(stored.get_str("user"), Ok(tracker.user.expose()));

    test_db::remove(db).await;
}

// whether a URI passes database::check_tls. parsing a mongodb:// URI needs no
// server or network, so these tests aren't ignored
async fn tls_allowed(uri: &str) -> bool {
    let options = mongodb::options::ClientOptions::parse(uri)
        .await
        .unwrap_or_else(|e| panic!("{uri} should parse: {e}"));
    database::check_tls(&options).is_ok()
}

#[tokio::test]
async fn a_database_on_this_machine_needs_no_tls() {
    for uri in [
        "mongodb://localhost:27017",
        "mongodb://LOCALHOST",
        "mongodb://127.0.0.1:27018",
        "mongodb://127.0.0.2",
        "mongodb://[::1]:27017",
        "mongodb://user:secret@127.0.0.1:27017/?authSource=admin",
        "mongodb://%2Ftmp%2Fmongodb-27017.sock",
    ] {
        assert!(tls_allowed(uri).await, "{uri} should be allowed");
    }
}

#[tokio::test]
async fn another_machine_is_reached_only_over_tls() {
    assert!(tls_allowed("mongodb://db.example.com:27017/?tls=true").await);
    assert!(tls_allowed("mongodb://db.example.com:27017/?ssl=true").await);
    assert!(tls_allowed("mongodb://10.0.0.5:27017/?tls=true").await);
    for uri in [
        "mongodb://db.example.com:27017",
        "mongodb://db.example.com:27017/?tls=false",
        // a Docker service name isn't this machine either
        "mongodb://mongodb:27017",
        "mongodb://10.0.0.5:27017",
        "mongodb://192.168.1.20:27017",
        // one remote host among local ones is still reached without TLS
        "mongodb://127.0.0.1:27017,db.example.com:27017",
    ] {
        assert!(!tls_allowed(uri).await, "{uri} should be refused");
    }
}

// TLS without checking the certificate can be impersonated, so it isn't enough
#[tokio::test]
async fn tls_without_certificate_checks_is_refused() {
    for uri in [
        "mongodb://db.example.com:27017/?tls=true&tlsInsecure=true",
        "mongodb://db.example.com:27017/?tls=true&tlsAllowInvalidCertificates=true",
    ] {
        assert!(!tls_allowed(uri).await, "{uri} should be refused");
    }
}

// the URI can hold the database password, so the refusal never repeats it
#[tokio::test]
async fn the_refusal_leaves_the_uri_out() {
    let uri = "mongodb://ridgecast:hunter2@db.example.com:27017";
    let options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    let error = database::check_tls(&options).unwrap_err().to_string();
    assert!(!error.contains("hunter2"));
    assert!(!error.contains("db.example.com"));
}

// the check runs before anything is sent, so a refused URI fails at once even
// with no server there
#[tokio::test]
async fn connect_refuses_an_unencrypted_remote_database() {
    let result = database::DB::connect("mongodb://db.example.com:27017", "ridgecast").await;
    assert!(matches!(result, Err(database::ConnectError::Unencrypted)));
}

// the driver's TLS here (rustls) always checks the hostname, and it refuses the
// URI option that would turn that off, so check_tls needn't look for it. if this
// fails, the driver accepts the option and check_tls must refuse it
#[tokio::test]
async fn skipping_hostname_checks_is_refused_by_the_driver() {
    let uri = "mongodb://db.example.com:27017/?tls=true&tlsAllowInvalidHostnames=true";
    assert!(mongodb::options::ClientOptions::parse(uri).await.is_err());
}
