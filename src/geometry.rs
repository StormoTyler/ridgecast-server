// geometry helpers shared by the upstream NWS and NAC modules

use geojson::GeometryValue;
use std::fmt;

// local modules
use crate::models::{GeometryErrorReason, GeometryTypes};

// why a geometry can't be used to match a point
#[derive(Debug, Clone, PartialEq)]
pub struct GeometryError {
    pub reason: GeometryErrorReason,
    pub message: String,
}

impl GeometryError {
    fn new(reason: GeometryErrorReason, message: impl Into<String>) -> Self {
        GeometryError {
            reason,
            message: message.into(),
        }
    }
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.reason, self.message)
    }
}

// converts geojson to the geo struct its type tag names, after checking
// it can be converted safely and used for location checks
pub fn convert_geometry(geo: &geojson::Geometry) -> Result<GeometryTypes, GeometryError> {
    let conversion_error = |e: geojson::Error| {
        GeometryError::new(GeometryErrorReason::GeometryConversionFailed, e.to_string())
    };
    match &geo.value {
        GeometryValue::Polygon { coordinates } => {
            check_polygons(std::slice::from_ref(coordinates))?;
            geo::Polygon::try_from(geo)
                .map(GeometryTypes::Polygon)
                .map_err(conversion_error)
        }
        GeometryValue::MultiPolygon { coordinates } => {
            check_polygons(coordinates)?;
            geo::MultiPolygon::try_from(geo)
                .map(GeometryTypes::MultiPolygon)
                .map_err(conversion_error)
        }
        // other geometry kinds can't be used for location checks
        other => Err(GeometryError::new(
            GeometryErrorReason::UnsupportedGeometryType,
            format!(
                "{} geometry can't be used for location checks",
                other.type_name()
            ),
        )),
    }
}

// geojson's conversion panics on a position with fewer than 2 coordinates, and
// a polygon whose exterior ring has fewer than the 4 positions GeoJSON requires
// converts without error but can never contain a point
pub fn check_polygons(polygons: &[geojson::PolygonType]) -> Result<(), GeometryError> {
    if polygons.is_empty() {
        return Err(GeometryError::new(
            GeometryErrorReason::DegenerateGeometry,
            "geometry has no polygons",
        ));
    }
    for polygon in polygons {
        if let Some(position) = polygon.iter().flatten().find(|position| position.len() < 2) {
            return Err(GeometryError::new(
                GeometryErrorReason::MalformedCoordinates,
                format!(
                    "position {:?} has fewer than 2 coordinates",
                    position.as_slice()
                ),
            ));
        }
        let exterior_len = polygon.first().map_or(0, Vec::len);
        if exterior_len < 4 {
            return Err(GeometryError::new(
                GeometryErrorReason::DegenerateGeometry,
                format!("exterior ring has {exterior_len} positions, at least 4 are required"),
            ));
        }
    }
    Ok(())
}
