// handler for NWS api
use reqwest::Client;
use serde_json;
use thiserror::Error;
use geo::Intersects;
use once_cell::sync::Lazy;
use std::time::Duration;

use crate::models::{Point, Alerts, Alert, GeometryTypes, Geocode, AlertsSorted,
    Forecast, Properties, ForecastResponse, RawFeatures,
    InvalidAlert, InvalidAlertReason, SkippedFeatures};
use crate::geometry;

const BASE_URL: &str = "https://api.weather.gov";

static NWS_CLIENT: Lazy<Client> = Lazy::new(|| 
{
    Client::builder()
        // NWS requires contact info in the User-Agent, checked at startup in main
        .user_agent(std::env::var("RIDGECAST_USER_AGENT").expect("RIDGECAST_USER_AGENT must be set"))
        // fail fast if the host is unreachable
        .connect_timeout(Duration::from_secs(10))
        // cap the whole request, including reading the body
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build client")
});

#[derive(Debug, Error)]
pub enum NWSError
{
    #[error("could not retrieve data from NWS API: {0}")]
    APICallError(#[from] reqwest::Error),
    #[error("json parsing failed: {0}")]
    JsonError(#[from] serde_json::Error),
}

impl NWSError
{
    pub fn is_timeout(&self) -> bool
    {
        matches!(self, NWSError::APICallError(e) if e.is_timeout())
    }

    // NWS returns 404 for points outside its coverage area
    pub fn is_not_found(&self) -> bool
    {
        matches!(self, NWSError::APICallError(e)
            if e.status() == Some(reqwest::StatusCode::NOT_FOUND))
    }

    // the response arrived but its body was not valid json
    // or not shaped like the expected data
    pub fn is_malformed(&self) -> bool
    {
        match self
        {
            NWSError::APICallError(e) => e.is_decode(),
            NWSError::JsonError(_) => true,
        }
    }

    // a failure that may not happen again: a 5xx response, a timeout,
    // a connection failure, or a malformed body. a 404 is not transient
    pub fn is_transient(&self) -> bool
    {
        self.is_malformed() || matches!(self, NWSError::APICallError(e)
            if e.status().is_some_and(|status| status.is_server_error())
                || e.is_timeout()
                || e.is_connect())
    }
}

async fn fetch_alerts() -> Result<RawFeatures, NWSError>
{
    let alerts_response =
        NWS_CLIENT
        .get(format!("{BASE_URL}/alerts/active?status=actual"))
        .send()
        .await?
        .error_for_status()?;

    //println!("data received from NWS endpoint");

    let alerts_json: serde_json::Value =
        alerts_response
        .json()
        .await?;

    //println!("NWS data jsonified");
    //println!("{:?}", alerts_json);

    Ok(serde_json::from_value(alerts_json)?)
}

// returns the sorted alerts, the alerts that failed validation,
// and the features that couldn't be deserialized
pub async fn get_alerts() -> Result<(AlertsSorted, Vec<InvalidAlert>, SkippedFeatures), NWSError>
{
    //println!("get_alerts called)");

    // retry once if the response is malformed
    let raw = match fetch_alerts().await
    {
        Err(e) if e.is_malformed() =>
        {
            eprintln!("get_alerts: malformed response, retrying once: {e}");
            fetch_alerts().await?
        }
        result => result?,
    };
    let (alerts, invalid_alerts, skipped) = parse_alerts(raw);

    let mut alerts_sorted = sort_alerts(alerts);
    alerts_sorted.fetched_at = chrono::Utc::now();
    Ok((alerts_sorted, invalid_alerts, skipped))
}

// deserializes the alerts and checks their SAME codes against the NWS spec
// codes that don't match ^\d{6}$ are logged and removed, since they can never
// match a point, and every alert left without a SAME code is logged
// geometry is converted for sort_alerts, geometry that can't be used is logged
// and removed so the alert is matched by its SAME codes instead
// returns the alerts, a record of each alert that failed validation,
// and the features that couldn't be deserialized
pub fn parse_alerts(raw: RawFeatures) -> (Alerts, Vec<InvalidAlert>, SkippedFeatures)
{
    let (features, skipped) = raw.parse("NWS alerts");
    let mut alerts = Alerts { features };
    let mut invalid_alerts = Vec::new();
    let now = chrono::Utc::now();

    for alert in alerts.features.iter_mut()
    {
        // kept as received, in case the alert fails validation
        let received = alert.clone();
        let mut reasons = Vec::new();

        let (valid, invalid): (Vec<String>, Vec<String>) =
            std::mem::take(&mut alert.properties.geocode.same)
            .into_iter()
            .partition(|code| Geocode::is_valid_same_code(code));
        if !invalid.is_empty()
        {
            eprintln!("parse_alerts: alert {} ({}): removing SAME codes that don't match \
                ^\\d{{6}}$: {invalid:?}", alert_id(alert), alert_event(alert));
            reasons.push(InvalidAlertReason::InvalidSameCodes);
        }
        alert.properties.geocode.same = valid;

        if alert.properties.geocode.same.is_empty()
        {
            eprintln!("parse_alerts: alert {} ({}) has no SAME code",
                alert_id(alert), alert_event(alert));
            reasons.push(InvalidAlertReason::NoSameCode);
        }

        // converted here so geometry that can't be used is recorded,
        // the alert then falls back to its SAME codes in sort_alerts
        let mut geometry_error = None;
        let mut geometry_error_reason = None;
        if let Some(GeometryTypes::Geojson(geo)) = &alert.geometry
        {
            match geometry::convert_geometry(geo)
            {
                Ok(new_geo) => alert.geometry = Some(new_geo),
                Err(error) =>
                {
                    eprintln!("parse_alerts: alert {} ({}): removing geometry that can't be used, \
                        matching by SAME code instead: {error}", alert_id(alert), alert_event(alert));
                    alert.geometry = None;
                    reasons.push(InvalidAlertReason::InvalidGeometry);
                    geometry_error_reason = Some(error.reason);
                    geometry_error = Some(error.message);
                }
            }
        }

        if !reasons.is_empty()
        {
            invalid_alerts.push(InvalidAlert
            {
                alert_id: received.id.clone(),
                reasons,
                invalid_same_codes: invalid,
                geometry_error_reason,
                geometry_error,
                alert: received,
                first_seen: now,
                last_seen: now,
            });
        }
    }
    (alerts, invalid_alerts, skipped)
}

// buckets alerts by whether they have usable polygon geometry,
// so check_alerts can match by polygon or by SAME code
// an alert with neither can't be matched to a location, so it is
// logged and kept in unmatched instead of being dropped
pub fn sort_alerts(mut alerts: Alerts) -> AlertsSorted
{
    let mut alerts_sorted = AlertsSorted::default();

    for alert in alerts.features.iter_mut()
    {
        match &alert.geometry
        {
            // parse_alerts already converts geojson, this handles alerts that skipped it
            Some(GeometryTypes::Geojson(geo)) =>
            {
                match geometry::convert_geometry(geo)
                {
                    Ok(new_geo) =>
                    {
                        alert.geometry = Some(new_geo);
                        alerts_sorted.add_geometry_alert(alert);
                    }
                    Err(e) =>
                    {
                        eprintln!("sort_alerts: alert {} ({}): geometry conversion failed, \
                            matching by SAME code instead: {e}", alert_id(alert), alert_event(alert));
                        add_by_same_code(&mut alerts_sorted, alert);
                    }
                }
            }
            Some(GeometryTypes::Polygon(_)) | Some(GeometryTypes::MultiPolygon(_)) =>
            {
                alerts_sorted.add_geometry_alert(alert);
            }
            None => add_by_same_code(&mut alerts_sorted, alert),
        }
    }

    if !alerts_sorted.unmatched.is_empty()
    {
        eprintln!("sort_alerts: {} alerts have no usable geometry or SAME code \
            and can't be matched to a location", alerts_sorted.unmatched.len());
    }
    alerts_sorted
}

// files an alert under each of its SAME codes, or in unmatched if it has none
fn add_by_same_code(alerts_sorted: &mut AlertsSorted, alert: &Alert)
{
    if alert.properties.geocode.same.is_empty()
    {
        eprintln!("sort_alerts: alert {} ({}) has no usable geometry or SAME code",
            alert_id(alert), alert_event(alert));
        alerts_sorted.unmatched.push(alert.clone());
        return;
    }
    for same_code in alert.properties.geocode.same.iter()
    {
        alerts_sorted.add_same_code_alert(same_code.to_string(), alert);
    }
}

fn alert_id(alert: &Alert) -> &str
{
    alert.id.as_deref().unwrap_or("without id")
}

fn alert_event(alert: &Alert) -> &str
{
    alert.properties.event.as_deref().unwrap_or("unknown event")
}

pub fn check_alerts(alerts: &AlertsSorted, point: &Point) -> Vec<Alert>
{
    // alerts without geometry: direct hash lookup by the point's SAME code
    let mut checked: Vec<Alert> =
        alerts.get_same_code_alerts(&point.properties.nwr.same_code).to_vec();

    // alerts with geometry: check whether the point is inside the polygon
    // or on its boundary, so a point on the edge of an alert area still matches
    let coordinates = point.geometry.coordinates;
    for alert in alerts.with_geometry.iter()
    {
        let intersects = match &alert.geometry
        {
            Some(GeometryTypes::Polygon(value)) => value.intersects(&coordinates),
            Some(GeometryTypes::MultiPolygon(value)) => value.intersects(&coordinates),
            _ => false,
        };

        if intersects
        {
            println!("alert matched via geometry!");
            checked.push(alert.clone());
        }
    }
    checked
}

pub async fn get_point(lat:f32, lon: f32) -> Result<Point, NWSError>
{
    // NWS often returns a temporary 500 or 503, so a transient failure is retried once
    let mut retried = false;
    loop
    {
        let result: Result<Point, NWSError> = async
        {
            let points_response =
                NWS_CLIENT
                .get(format!("{BASE_URL}/points/{lat},{lon}"))
                .send()
                .await?
                .error_for_status()?;

            let points_json: serde_json::Value =
                points_response
                .json()
                .await?;

            Ok(serde_json::from_value(points_json)?)
        }.await;

        match result
        {
            Err(e) if !retried && e.is_transient() =>
            {
                eprintln!("get_point: transient failure, retrying once: {e}");
                retried = true;
            }
            result => return result,
        }
    }
}

// fetches gridpoint data from NWS API gridpoints endpoint 
pub async fn get_forecast(lat: f32, lon: f32, alerts: &AlertsSorted) ->
    Result<ForecastResponse, NWSError>
{
    let point = get_point(lat, lon).await?;
    let forecast = get_gridpoints(&point).await?;
    //println!("forecast is: {:?}", forecast);

    let active_alerts: Vec<Alert> =
        check_alerts(alerts, &point);

    let forecast_response = ForecastResponse
    {
        point,
        forecast,
        alerts: active_alerts,
    };
    Ok(forecast_response)
}

// gets a forecast's properties from the nws gridpoints endpoint 
pub async fn get_gridpoints(point: &Point) -> Result<Properties, NWSError>
{
    // retried once on a transient failure, like get_point
    let mut retried = false;
    loop
    {
        let result: Result<Forecast, NWSError> = async
        {
            let gridpoints_response =
                NWS_CLIENT
                .get(format!("{BASE_URL}/gridpoints/{}/{},{}",
                point.properties.cwa,
                point.properties.grid_x,
                point.properties.grid_y))
                .send()
                .await?
                .error_for_status()?;

            let gridpoints: serde_json::Value =
                gridpoints_response
                .json()
                .await?;

            //println!("gridpoints is {:?}", gridpoints);

            Ok(serde_json::from_value(gridpoints)?)
        }.await;

        match result
        {
            Err(e) if !retried && e.is_transient() =>
            {
                eprintln!("get_gridpoints: transient failure, retrying once: {e}");
                retried = true;
            }
            result => return result.map(|forecast| forecast.properties),
        }
    }
}
