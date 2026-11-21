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
    RawFeatures, SkippedFeatures,
};
use crate::tracking::shared;

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

    Ok(serde_json::from_value(avy_json)?)
}

// returns the avalanche zones, the zones whose geometry could not be converted,
// and the features that couldn't be deserialized
pub async fn get_avy() -> Result<(AvalancheData, Vec<InvalidAvyArea>, SkippedFeatures), NACError> {
    // retry once if the response is malformed
    let raw = match fetch_avy().await {
        Err(e) if e.is_malformed() => {
            tracing::warn!("get_avy: malformed response, retrying once: {e}");
            fetch_avy().await?
        }
        result => result?,
    };

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
                    tracing::warn!(
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
pub fn check_avy_geometry<'a>(
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
    match shared::validity(start, end, now) {
        // a level on the North American Public Avalanche Danger Scale, 1 (low)
        // to 5 (extreme), or NAC's -1, a valid forecast with no rating.
        // any other level is invalid
        shared::Validity::Current | shared::Validity::NotStarted
            if area
                .properties
                .danger_level
                .is_some_and(|level| matches!(level, -1 | 1..=5)) =>
        {
            ZoneForecast::Current(&area.properties)
        }
        shared::Validity::Current | shared::Validity::NotStarted => ZoneForecast::NoForecast,
        shared::Validity::Expired => ZoneForecast::Expired,
        shared::Validity::NoStart | shared::Validity::NoEnd => ZoneForecast::NoForecast,
    }
}
