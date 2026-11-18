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
