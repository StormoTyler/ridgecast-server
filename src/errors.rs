// errors returned by the route handlers and the code they call, such as the
// snapshot loaders and SMS requests. each variant maps to an HTTP status code

use axum::
{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Serialize, Deserialize};
use thiserror::Error;

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum APIError
{
    #[error("received error from NWS endpoint")]
    NWSError,
    #[error("received error from NAC endpoint")]
    NACError,
    #[error("could not retrieve forecast from NWS")]
    ForecastError,
    #[error("could not retrieve alerts from NWS")]
    AlertsError,
    #[error("upstream API timed out")]
    UpstreamTimeout,
    #[error("database error")]
    DatabaseError,
    #[error("lat must be between -90 and 90 and lon between -180 and 180")]
    InvalidLocation,
    #[error("location is outside National Weather Service coverage")]
    LocationNotCovered,
}

impl IntoResponse for APIError
{
    fn into_response(self) -> Response
    {
        let status = match self
        {
            // upstream API failed or returned bad data
            APIError::NWSError
            | APIError::NACError
            | APIError::ForecastError
            | APIError::AlertsError => StatusCode::BAD_GATEWAY,
            APIError::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
            APIError::DatabaseError => StatusCode::INTERNAL_SERVER_ERROR,
            APIError::InvalidLocation => StatusCode::BAD_REQUEST,
            APIError::LocationNotCovered => StatusCode::NOT_FOUND,
        };

        (status, self.to_string()).into_response()
    }
}
