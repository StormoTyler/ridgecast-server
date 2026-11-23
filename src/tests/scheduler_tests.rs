// tests for scheduler::check_all: what one hourly check sends and saves, against
// a real MongoDB (ignored by default, see test_db) with a FakeSender and the saved
// NWS alerts and NAC data. a daily forecast that is due calls NWS, so only the
// daily forecast that is too late to send is tested here
use chrono::{DateTime, TimeZone, Utc};
use mongodb::bson::doc;

use crate::database;
use crate::messaging;
use crate::models::{AlertsSorted, AvalancheData, Snapshot, UserPoint};
use crate::scheduler;
use crate::tests::fake_sender::FakeSender;
use crate::tests::{models_tests, nac_tests, nws_tests, test_db};

// the hour checked. every saved NAC forecast is current then
fn hour() -> DateTime<Utc> {
    nac_tests::test_now()
}

fn alerts() -> Snapshot<AlertsSorted> {
    Snapshot {
        data: nws_tests::load_alerts(),
        stale: false,
    }
}

fn avy() -> Snapshot<AvalancheData> {
    Snapshot {
        data: nac_tests::load_avy(),
        stale: false,
    }
}

// an alert tracker named Mt Hood in Bozeman, MT, where the Red Flag Warning is
// active, that has a day left
fn alert_tracker() -> UserPoint {
    let mut tracker = models_tests::tracker();
    tracker.forecast_vars.track_alerts = true;
    tracker.point = nws_tests::make_point(45.68, -111.04, "030031");
    tracker.expires_at = hour() + chrono::Duration::days(1);
    tracker
}

async fn add(tracker: &UserPoint, db: &database::DB) {
    database::add_tracker(tracker, db)
        .await
        .expect("the tracker should save");
}

async fn stored_trackers(db: &database::DB) -> Vec<UserPoint> {
    let mut cursor = db
        .points_collection
        .find(doc! {})
        .await
        .expect("the trackers should be read");
    let mut trackers = Vec::new();
    while cursor.advance().await.expect("the cursor should advance") {
        trackers.push(
            cursor
                .deserialize_current()
                .expect("a tracker should deserialize"),
        );
    }
    trackers
}

// the first stored tracker as MongoDB holds it, so a test can tell whether any
// field changed, without UserPoint implementing PartialEq
async fn stored_document(db: &database::DB) -> mongodb::bson::Document {
    db.points_collection
        .clone_with_type::<mongodb::bson::Document>()
        .find_one(doc! {})
        .await
        .expect("the tracker should be read")
        .expect("the tracker should be stored")
}

fn opted_out() -> Result<messaging::SendReceipt, messaging::SendError> {
    Err(messaging::SendError::OptedOut)
}

fn rejected() -> Result<messaging::SendReceipt, messaging::SendError> {
    Err(messaging::SendError::Permanent(
        "invalid number".to_string(),
    ))
}

// a new alert is sent from the number the user texted, and recorded, so the next
// check doesn't send it again
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_new_alert_is_sent_once() {
    let db = test_db::fresh("scheduler_new_alert").await;
    let tracker = alert_tracker();
    add(&tracker, &db).await;
    let sender = FakeSender::default();

    scheduler::check_all(&db, &sender, Some(&alerts()), None, hour()).await;
    scheduler::check_all(&db, &sender, Some(&alerts()), None, hour()).await;

    let attempted = sender.attempted();
    assert_eq!(attempted.len(), 1, "{attempted:?}");
    assert!(attempted[0].starts_with("(1/1) Mt Hood\n!Severe: Red Flag Warning"));
    let attempts = sender.attempts.lock().unwrap().clone();
    assert_eq!(attempts[0].0, tracker.reply_from);
    assert_eq!(attempts[0].1, tracker.user);
    assert_eq!(
        stored_trackers(&db).await[0].alerts_sent,
        vec![nws_tests::RED_FLAG_WARNING_ID]
    );
    test_db::remove(db).await;
}

// a notification that isn't delivered isn't recorded, so the next check sends it
// again. a rejected message doesn't delete the tracker
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn an_undelivered_alert_is_not_recorded() {
    let db = test_db::fresh("scheduler_undelivered").await;
    add(&alert_tracker(), &db).await;
    let sender = FakeSender::scripted(vec![rejected()]);

    scheduler::check_all(&db, &sender, Some(&alerts()), None, hour()).await;

    let trackers = stored_trackers(&db).await;
    assert_eq!(trackers.len(), 1);
    assert!(trackers[0].alerts_sent.is_empty());

    scheduler::check_all(&db, &sender, Some(&alerts()), None, hour()).await;
    assert_eq!(sender.attempted().len(), 2);
    test_db::remove(db).await;
}

// STOP deletes every tracker of the user, so their other tracker isn't sent to,
// and another user's tracker still is
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn an_opted_out_user_has_every_tracker_deleted() {
    let db = test_db::fresh("scheduler_opted_out").await;
    let tracker = alert_tracker();
    let mut other = alert_tracker();
    other.user = messaging::PhoneNumber::new("+15415550199").unwrap();
    for user_point in [&tracker, &tracker, &other] {
        add(user_point, &db).await;
    }
    let sender = FakeSender::scripted(vec![opted_out()]);

    scheduler::check_all(&db, &sender, Some(&alerts()), None, hour()).await;

    assert_eq!(sender.attempted().len(), 2);
    let trackers = stored_trackers(&db).await;
    assert_eq!(trackers.len(), 1);
    assert_eq!(trackers[0].user, other.user);
    assert_eq!(
        trackers[0].alerts_sent,
        vec![nws_tests::RED_FLAG_WARNING_ID]
    );
    test_db::remove(db).await;
}

// alerts that can't be loaded are notified once per outage
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn unavailable_alerts_are_notified_once() {
    let db = test_db::fresh("scheduler_uncheckable").await;
    add(&alert_tracker(), &db).await;
    let sender = FakeSender::default();

    scheduler::check_all(&db, &sender, None, None, hour()).await;
    scheduler::check_all(&db, &sender, None, None, hour()).await;

    assert_eq!(
        sender.attempted(),
        ["(1/1) Mt Hood\n!Alerts could not be checked"]
    );
    assert!(stored_trackers(&db).await[0].alerts_uncheckable.is_raised());
    test_db::remove(db).await;
}

// notifications go in the reply's order, avalanche then alerts, and at the last
// check Tracking ended ends the last one
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn the_last_check_ends_its_last_notification() {
    let db = test_db::fresh("scheduler_last_check").await;
    let mut tracker = alert_tracker();
    tracker.forecast_vars.track_avy = true;
    // a stored forecast, which avalanche data that can't be loaded loses
    tracker.avy = Some(avy().data.features[0].properties.clone());
    tracker.expires_at = hour() + chrono::Duration::minutes(30);
    add(&tracker, &db).await;
    let sender = FakeSender::default();

    scheduler::check_all(&db, &sender, Some(&alerts()), None, hour()).await;

    let attempted = sender.attempted();
    assert_eq!(attempted.len(), 2, "{attempted:?}");
    assert_eq!(
        attempted[0],
        "(1/1) Mt Hood\nAvy: data unavailable, try later"
    );
    assert!(
        attempted[1].contains("Red Flag Warning"),
        "{}",
        attempted[1]
    );
    assert!(
        attempted[1].ends_with("\nTracking ended"),
        "{}",
        attempted[1]
    );
    test_db::remove(db).await;
}

// a last check with nothing else to send sends Tracking ended alone, and a
// tracker the previous check ended sends nothing
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn tracking_ended_is_sent_alone_once() {
    let db = test_db::fresh("scheduler_tracking_ended").await;
    let mut tracker = alert_tracker();
    // Salt Lake City has no active alerts in the test data
    tracker.point = nws_tests::make_point(40.76, -111.89, "049035");
    tracker.expires_at = hour() + chrono::Duration::minutes(30);
    add(&tracker, &db).await;
    let sender = FakeSender::default();

    scheduler::check_all(&db, &sender, Some(&alerts()), None, hour()).await;
    scheduler::check_all(
        &db,
        &sender,
        Some(&alerts()),
        None,
        hour() + chrono::Duration::hours(1),
    )
    .await;

    assert_eq!(sender.attempted(), ["(1/1) Mt Hood\nTracking ended"]);
    test_db::remove(db).await;
}

// a daily forecast too late to send is skipped without a notice, and moves to the
// next day. 23:00 UTC is 4 PM in Denver
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_daily_forecast_too_late_to_send_moves_to_the_next_day() {
    let db = test_db::fresh("scheduler_too_late").await;
    let six_am = Utc.with_ymd_and_hms(2026, 11, 23, 13, 0, 0).unwrap();
    let mut tracker = models_tests::tracker();
    tracker.point.properties.time_zone = Some("America/Denver".to_string());
    tracker.forecast_vars.daily_forecast = true;
    tracker.next_daily_forecast = Some(six_am);
    tracker.expires_at = six_am + chrono::Duration::days(3);
    add(&tracker, &db).await;
    let sender = FakeSender::default();

    scheduler::check_all(
        &db,
        &sender,
        None,
        None,
        six_am + chrono::Duration::hours(10),
    )
    .await;

    assert!(sender.attempted().is_empty());
    assert_eq!(
        stored_trackers(&db).await[0].next_daily_forecast,
        Some(six_am + chrono::Duration::days(1))
    );
    test_db::remove(db).await;
}

// a tracker that sends nothing isn't sent anything or changed
#[tokio::test]
#[ignore = "needs MongoDB"]
async fn a_quiet_tracker_sends_nothing() {
    let db = test_db::fresh("scheduler_quiet").await;
    let mut tracker = alert_tracker();
    tracker.alerts_sent = vec![nws_tests::RED_FLAG_WARNING_ID.to_string()];
    add(&tracker, &db).await;
    let before = stored_document(&db).await;
    let sender = FakeSender::default();

    scheduler::check_all(&db, &sender, Some(&alerts()), Some(&avy()), hour()).await;

    assert!(sender.attempted().is_empty());
    assert_eq!(stored_document(&db).await, before);
    test_db::remove(db).await;
}
