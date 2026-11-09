// handler for National Avalanche Center api

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use geo;
use geo::Intersects;
use reqwest::Client;
use serde_json;
use std::sync::LazyLock;
use std::time::Duration;
use thiserror::Error;

// local modules
use crate::geometry;
use crate::models::{
    AvalancheData, AvyArea, AvyProperties, DateTimeTypes, GeometryTypes, InvalidAvyArea, Point,
    RawFeatures, SkippedFeatures, Snapshot, UserPoint,
};
use crate::tracking;

const BASE_URL: &str = "https://api.avalanche.org/v2/public/products/map-layer";

static NAC_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        // NWS requires contact info in the User-Agent, checked at startup in main
        .user_agent(
            std::env::var("RIDGECAST_USER_AGENT").expect("RIDGECAST_USER_AGENT must be set"),
        )
        // fail fast if the host is unreachable
        .connect_timeout(Duration::from_secs(10))
        // cap the whole request, including reading the body
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build client")
});

#[derive(Debug, Error)]
pub enum NACError {
    #[error("could not retrieve data from NAC API: {0}")]
    APICallError(#[from] reqwest::Error),
    #[error("json parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
}

impl NACError {
    pub fn is_timeout(&self) -> bool {
        matches!(self, NACError::APICallError(e) if e.is_timeout())
    }

    // the response arrived but its body was not valid json
    // or not shaped like a feature collection
    pub fn is_malformed(&self) -> bool {
        match self {
            NACError::APICallError(e) => e.is_decode(),
            NACError::JsonError(_) => true,
        }
    }
}

async fn fetch_avy() -> Result<RawFeatures, NACError> {
    let avy_response = NAC_CLIENT.get(BASE_URL).send().await?.error_for_status()?;

    let avy_json: serde_json::Value = avy_response.json().await?;

    //println!("{avy_json}");

    Ok(serde_json::from_value(avy_json)?)
}

// returns the avalanche zones, the zones whose geometry could not be converted,
// and the features that couldn't be deserialized
pub async fn get_avy() -> Result<(AvalancheData, Vec<InvalidAvyArea>, SkippedFeatures), NACError> {
    // retry once if the response is malformed
    let raw = match fetch_avy().await {
        Err(e) if e.is_malformed() => {
            eprintln!("get_avy: malformed response, retrying once: {e}");
            fetch_avy().await?
        }
        result => result?,
    };
    //println!("{:?}", avy);

    let (features, skipped) = raw.parse("NAC map layer");
    let (features, invalid_areas) = build_avy_map(features);
    // wrap the zones so the snapshot is stored as a single document
    Ok((
        AvalancheData {
            features,
            fetched_at: chrono::Utc::now(),
        },
        invalid_areas,
        skipped,
    ))
}

// converts each zone's dates to local time and its geometry to geo types,
// so check_avy_geometry can test points against them
// a zone whose geometry can't be converted keeps its Geojson geometry, which
// check_avy_geometry never matches. it is logged and returned as an InvalidAvyArea
pub fn build_avy_map(mut features: Vec<AvyArea>) -> (Vec<AvyArea>, Vec<InvalidAvyArea>) {
    let mut invalid_areas = Vec::new();
    let now = chrono::Utc::now();

    for area in features.iter_mut() {
        // NAC dates have no offset but are in UTC,
        // so convert them to the zone's local timezone
        if let Some(tz) = area
            .properties
            .timezone
            .as_deref()
            .and_then(|tz| tz.parse::<Tz>().ok())
        {
            for date in [
                &mut area.properties.start_date,
                &mut area.properties.end_date,
            ] {
                if let Some(DateTimeTypes::Naive(naive)) = date {
                    let local = naive.and_utc().with_timezone(&tz).fixed_offset();
                    *date = Some(DateTimeTypes::Aware(local));
                }
            }
        }

        if let GeometryTypes::Geojson(geo) = &area.geometry {
            match geometry::convert_geometry(geo) {
                Ok(new_geo) => area.geometry = new_geo,
                Err(error) => {
                    let zone_name = area.properties.name.clone();
                    let center = area.properties.center.clone();
                    eprintln!(
                        "build_avy_map: zone {} ({}) can't be matched to a location, {error}",
                        zone_name.as_deref().unwrap_or("without name"),
                        center.as_deref().unwrap_or("unknown center")
                    );
                    invalid_areas.push(InvalidAvyArea {
                        zone_name,
                        center,
                        reason: error.reason,
                        error: error.message,
                        area: area.clone(),
                        first_seen: now,
                        last_seen: now,
                    });
                }
            }
        }
    }

    (features, invalid_areas)
}

// the forecast state of the avalanche zone containing a point. these are
// normal states, not errors, so callers match each one instead of logging them.
// Current borrows the forecast from the avalanche data, so it is only cloned
// by callers that keep it
#[derive(Debug, Clone, PartialEq)]
pub enum ZoneForecast<'a> {
    // a valid forecast: it has a start_date, an end_date that hasn't passed,
    // and a danger level of 1 to 5 or -1 (no rating). a forecast that hasn't
    // started yet is valid
    Current(&'a AvyProperties),
    // the point isn't in any zone
    NoZone,
    // the zone's center is in its off season
    OffSeason,
    // the zone has no start_date, no end_date, or a danger level that is
    // missing or neither -1 nor 1 to 5
    NoForecast,
    // the zone's forecast has passed its end_date
    Expired,
}

// checks which avalanche forecast zone a point intersects with
// and returns the state of its forecast
pub async fn check_avy_geometry<'a>(
    point: &Point,
    avy_data: &'a AvalancheData,
    now: DateTime<Utc>,
) -> ZoneForecast<'a> {
    let coordinates = point.geometry.coordinates;

    // intersects, so a point on a zone boundary still matches
    let zone = avy_data.features.iter().find(|area| match &area.geometry {
        GeometryTypes::Polygon(value) => value.intersects(&coordinates),
        GeometryTypes::MultiPolygon(value) => value.intersects(&coordinates),
        GeometryTypes::Geojson(_) => false,
    });
    let Some(area) = zone else {
        return ZoneForecast::NoZone;
    };

    // off season is checked first, since off season zones can keep an old start_date
    if area.properties.off_season == Some(true) {
        return ZoneForecast::OffSeason;
    }
    let start = area
        .properties
        .start_date
        .as_ref()
        .map(DateTimeTypes::to_utc);
    let end = area.properties.end_date.as_ref().map(DateTimeTypes::to_utc);
    match tracking::validity(start, end, now) {
        // a level on the North American Public Avalanche Danger Scale, 1 (low)
        // to 5 (extreme), or NAC's -1, a valid forecast with no rating.
        // any other level is invalid
        tracking::Validity::Current | tracking::Validity::NotStarted
            if area
                .properties
                .danger_level
                .is_some_and(|level| matches!(level, -1 | 1..=5)) =>
        {
            ZoneForecast::Current(&area.properties)
        }
        tracking::Validity::Current | tracking::Validity::NotStarted => ZoneForecast::NoForecast,
        tracking::Validity::Expired => ZoneForecast::Expired,
        tracking::Validity::NoStart | tracking::Validity::NoEnd => ZoneForecast::NoForecast,
    }
}

// what check_avy_tracker found, for the scheduler to act on
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AvyTrackerUpdate {
    // nothing to send or save
    Unchanged,
    // UserPoint.avy was replaced with a new forecast, send it and save the tracker
    NewForecast,
    // the stored forecast is no longer valid, send "Avy: no current forecast" and save the tracker
    ForecastLost,
    // the stored forecast was lost because the center entered its off season,
    // send the off season notice and save the tracker
    OffSeason,
    // the avalanche data is stale, so the stored forecast can't be confirmed and
    // counts as lost. send "Avy: no current forecast" with the stale notice and save the tracker
    DataStale,
    // the stored forecast is valid again after a loss. the user already has it,
    // so nothing is sent, but the tracker is saved so the next loss is notified
    ForecastRestored,
}

// checks a tracker's avalanche zone against the current data.
// UserPoint.avy keeps the last valid forecast the user was sent, and
// UserPoint.avy_lost records that they were told it was lost, so a loss is
// notified once and a forecast that returns unchanged is not sent again.
// a stale snapshot counts as losing the forecast
// unused until the scheduler calls it, remove the allow then
#[allow(dead_code)]
pub async fn check_avy_tracker(
    user_point: &mut UserPoint,
    snapshot: &Snapshot<AvalancheData>,
    now: DateTime<Utc>,
) -> AvyTrackerUpdate {
    if snapshot.stale {
        return record_avy_loss(user_point, AvyTrackerUpdate::DataStale);
    }
    match check_avy_geometry(&user_point.point, &snapshot.data, now).await {
        ZoneForecast::Current(current) => {
            let was_lost = user_point.avy_lost.is_raised();
            user_point.avy_lost.clear();
            user_point.avy_off_season.clear();
            // any changed field counts as a new forecast, since avalanche
            // information is safety critical it is better to over report
            if user_point.avy.as_ref() != Some(current) {
                user_point.avy = Some(current.clone());
                AvyTrackerUpdate::NewForecast
            } else if was_lost {
                AvyTrackerUpdate::ForecastRestored
            } else {
                AvyTrackerUpdate::Unchanged
            }
        }
        // sent once, even after another kind of loss was notified, since the
        // center may not publish another forecast before the tracker ends
        ZoneForecast::OffSeason => {
            if user_point.avy.is_none() || !user_point.avy_off_season.raise() {
                return AvyTrackerUpdate::Unchanged;
            }
            user_point.avy_lost.raise();
            AvyTrackerUpdate::OffSeason
        }
        ZoneForecast::NoZone | ZoneForecast::NoForecast | ZoneForecast::Expired => {
            record_avy_loss(user_point, AvyTrackerUpdate::ForecastLost)
        }
    }
}

// the user is only told about a loss when they had a valid forecast,
// and only once until it is valid again
fn record_avy_loss(user_point: &mut UserPoint, loss: AvyTrackerUpdate) -> AvyTrackerUpdate {
    if user_point.avy.is_none() || !user_point.avy_lost.raise() {
        return AvyTrackerUpdate::Unchanged;
    }
    loss
}
