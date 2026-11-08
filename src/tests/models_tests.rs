// tests for how models are stored in MongoDB
use crate::messaging;
use crate::models::{ForecastVars, Nwr, Point, PointGeometry, PointProperties, UserPoint};

const USER: &str = "+15415550100";
const SERVER: &str = "+18005550100";

// a tracker with nothing but its numbers set to anything meaningful
fn tracker() -> UserPoint {
    UserPoint {
        id: None,
        user: messaging::PhoneNumber::new(USER).unwrap(),
        reply_from: messaging::PhoneNumber::new(SERVER).unwrap(),
        tracker_name: Some("Mt Hood".to_string()),
        forecast_vars: ForecastVars::default(),
        point: Point {
            properties: PointProperties {
                cwa: "PQR".to_string(),
                grid_x: 0,
                grid_y: 0,
                nwr: Nwr {
                    same_code: "041005".to_string(),
                },
                astronomical_data: None,
                time_zone: None,
            },
            id: None,
            geometry: PointGeometry {
                coordinates: geo::Point::new(-121.6959, 45.3735),
            },
        },
        avy: None,
        avy_lost: Default::default(),
        avy_off_season: Default::default(),
        time_added: chrono::Utc::now(),
        valid_duration: 24,
        next_daily_forecast: None,
    }
}

// a tracker's numbers are stored under user and reply_from as plain E.164
// strings, which the scheduler reads back to send from reply_from to user, and
// each is validated when read, so a tracker with a bad stored number fails to
// load instead of being sent to
#[test]
fn tracker_numbers_are_stored_as_plain_strings_and_validated_when_read() {
    let stored = bson::to_document(&tracker()).expect("tracker should serialize");
    assert_eq!(stored.get_str("user"), Ok(USER));
    assert_eq!(stored.get_str("reply_from"), Ok(SERVER));

    let read: UserPoint = bson::from_document(stored.clone()).expect("tracker should deserialize");
    assert_eq!(read.user.expose(), USER);
    assert_eq!(read.reply_from.expose(), SERVER);

    for field in ["user", "reply_from"] {
        let mut bad = stored.clone();
        bad.insert(field, "whatever");
        assert!(
            bson::from_document::<UserPoint>(bad).is_err(),
            "{field} should be validated when read"
        );
    }
}
