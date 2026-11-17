// converts values from the units NWS sends, named by its unit codes such as
// wmoUnit:degC, to the units RidgeCast shows. the caller picks the unit it
// wants, and a unit code that can't be converted to it gives no converter,
// so a value is never shown in the wrong unit

const PASCALS_PER_INCH_OF_MERCURY: f64 = 3386.389;
const METERS_PER_FOOT: f64 = 0.3048;

// the units a value can be converted to
#[derive(Debug, Clone, Copy)]
pub enum Unit {
    Fahrenheit,
    Inches,
    Feet,
    Miles,
    Mph,
    InchesOfMercury,
    Percent,
    Degrees,
    // a category without a unit, such as heat risk
    Category,
}

impl Unit {
    // converts a value with NWS's unit code to this unit. None for a unit
    // code that can't be converted, so a value is never shown in the wrong unit
    pub fn converter(self, uom: Option<&str>) -> Option<fn(f64) -> f64> {
        let convert: fn(f64) -> f64 = match (self, uom) {
            (Unit::Fahrenheit, Some("wmoUnit:degC")) => |celsius| celsius * 9.0 / 5.0 + 32.0,
            (Unit::Fahrenheit, Some("wmoUnit:degF")) => |fahrenheit| fahrenheit,
            (Unit::Inches, Some("wmoUnit:mm")) => |mm| mm / 25.4,
            (Unit::Inches, Some("wmoUnit:cm")) => |cm| cm / 2.54,
            (Unit::Feet, Some("wmoUnit:m")) => |meters| meters / METERS_PER_FOOT,
            (Unit::Feet, Some("wmoUnit:ft")) => |feet| feet,
            (Unit::Miles, Some("wmoUnit:m")) => |meters| meters / 1609.344,
            (Unit::Miles, Some("wmoUnit:km")) => |km| km / 1.609344,
            (Unit::Mph, Some("wmoUnit:km_h-1")) => |kmh| kmh / 1.609344,
            (Unit::Mph, Some("wmoUnit:m_s-1")) => |ms| ms * 3600.0 / 1609.344,
            // NWS sends pressure without a unit code, in pascals
            (Unit::InchesOfMercury, None | Some("wmoUnit:Pa")) => {
                |pascals| pascals / PASCALS_PER_INCH_OF_MERCURY
            }
            (Unit::InchesOfMercury, Some("wmoUnit:hPa")) => {
                |hectopascals| hectopascals * 100.0 / PASCALS_PER_INCH_OF_MERCURY
            }
            (Unit::Percent, Some("wmoUnit:percent")) => |percent| percent,
            (Unit::Degrees, Some("wmoUnit:degree_(angle)")) => |degrees| degrees,
            // NWS sends heat risk, a category from 0 to 4, without a unit code
            (Unit::Category, _) => |category| category,
            _ => return None,
        };
        Some(convert)
    }
}
