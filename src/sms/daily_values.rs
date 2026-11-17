// turns an NWS forecast's series into values for each local day, in the units
// a reply shows: highs and lows, extremes, and totals. reply turns them into text

use chrono::{DateTime, Days, FixedOffset, NaiveDate, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;

// local modules
use crate::models::{Elevation, FloatVec, IntVec};
use crate::sms::forecast;
use crate::unit_conversion;

// each forecast day's values, starting with today in tz. a day with no
// forecast data, such as one past the end of the NWS forecast, is left out
pub fn days(
    forecast: &forecast::RequestedForecast,
    tz: Tz,
    now: DateTime<Utc>,
    count: u32,
) -> Vec<DaySummary> {
    let series = Series::new(forecast);
    let today = now.with_timezone(&tz).date_naive();
    (0..count)
        .filter_map(|offset| {
            let date = today.checked_add_days(Days::new(offset.into()))?;
            let (start, end) = day_bounds(tz, date)?;
            let mut day = series.summarize(date, start, end);
            // the elevation is the same every day, so only the first day has it
            day.first_day = offset == 0;
            if day.first_day {
                day.elevation = forecast.elevation.as_ref().and_then(convert_elevation);
            }
            day.has_data().then_some(day)
        })
        .collect()
}

// local midnight to the next local midnight, in UTC
fn day_bounds(tz: Tz, date: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    // earliest() is None only where a daylight saving change skips midnight,
    // which does not happen in US time zones
    let midnight = |date: NaiveDate| {
        tz.from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
            .earliest()
            .map(|time| time.with_timezone(&Utc))
    };
    Some((midnight(date)?, midnight(date.succ_opt()?)?))
}

// one NWS value and the time it covers, converted to the units shown
#[derive(Debug, Clone, Copy)]
struct Interval {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    // None for a value NWS left out, such as a cloud ceiling when there is none
    value: Option<f64>,
}

// the forecast's series, converted once and then summarized for each day.
// a series that wasn't requested, or whose unit can't be converted, is empty
struct Series {
    max_temperature: Vec<Interval>,
    min_temperature: Vec<Interval>,
    wet_bulb_globe: Vec<Interval>,
    heat_risk: Vec<Interval>,
    humidity: Vec<Interval>,
    precip_chance: Vec<Interval>,
    precip: Vec<Interval>,
    snow: Vec<Interval>,
    snow_level: Vec<Interval>,
    ice: Vec<Interval>,
    wind_speed: Vec<Interval>,
    wind_direction: Vec<Interval>,
    gust: Vec<Interval>,
    visibility: Vec<Interval>,
    pressure: Vec<Interval>,
    ceiling: Vec<Interval>,
}

impl Series {
    fn new(forecast: &forecast::RequestedForecast) -> Self {
        // floats and ints have the same body, but a closure can't be generic, and
        // FloatVec and IntVec are different types (f32 and i32 values), so each
        // needs its own. keep the two the same
        let floats = |field, series: Option<&FloatVec>, unit| {
            series.map_or(Vec::new(), |series| {
                convert_series(
                    field,
                    series.uom.as_deref(),
                    unit,
                    &series.values,
                    |value| (&value.valid_time, value.value.map(f64::from)),
                )
            })
        };
        let ints = |field, series: Option<&IntVec>, unit| {
            series.map_or(Vec::new(), |series| {
                convert_series(
                    field,
                    series.uom.as_deref(),
                    unit,
                    &series.values,
                    |value| (&value.valid_time, value.value.map(f64::from)),
                )
            })
        };
        Series {
            max_temperature: floats(
                "max_temperature",
                Some(&forecast.max_temperature),
                unit_conversion::Unit::Fahrenheit,
            ),
            min_temperature: floats(
                "min_temperature",
                Some(&forecast.min_temperature),
                unit_conversion::Unit::Fahrenheit,
            ),
            wet_bulb_globe: floats(
                "wet_bulb_globe_temperature",
                forecast.wet_bulb_globe_temperature.as_ref(),
                unit_conversion::Unit::Fahrenheit,
            ),
            heat_risk: ints(
                "heat_risk",
                forecast.heat_risk.as_ref(),
                unit_conversion::Unit::Category,
            ),
            humidity: ints(
                "relative_humidity",
                forecast.relative_humidity.as_ref(),
                unit_conversion::Unit::Percent,
            ),
            precip_chance: ints(
                "probability_of_precipitation",
                Some(&forecast.probability_of_precipitation),
                unit_conversion::Unit::Percent,
            ),
            precip: floats(
                "quantitative_precipitation",
                Some(&forecast.quantitative_precipitation),
                unit_conversion::Unit::Inches,
            ),
            snow: floats(
                "snowfall_amount",
                Some(&forecast.snowfall_amount),
                unit_conversion::Unit::Inches,
            ),
            snow_level: floats(
                "snow_level",
                forecast.snow_level.as_ref(),
                unit_conversion::Unit::Feet,
            ),
            ice: floats(
                "ice_accumulation",
                Some(&forecast.ice_accumulation),
                unit_conversion::Unit::Inches,
            ),
            wind_speed: floats(
                "wind_speed",
                Some(&forecast.wind_speed),
                unit_conversion::Unit::Mph,
            ),
            wind_direction: ints(
                "wind_direction",
                Some(&forecast.wind_direction),
                unit_conversion::Unit::Degrees,
            ),
            gust: floats(
                "wind_gust",
                forecast.wind_gust.as_ref(),
                unit_conversion::Unit::Mph,
            ),
            visibility: floats(
                "visibility",
                forecast.visibility.as_ref(),
                unit_conversion::Unit::Miles,
            ),
            pressure: floats(
                "pressure",
                forecast.pressure.as_ref(),
                unit_conversion::Unit::InchesOfMercury,
            ),
            ceiling: floats(
                "ceiling_height",
                forecast.ceiling_height.as_ref(),
                unit_conversion::Unit::Feet,
            ),
        }
    }

    // the values for the day from start to end. highs, lows, and other extremes
    // use every value whose time overlaps the day. totals split a value across
    // days by the share of its time in each
    fn summarize(&self, date: NaiveDate, start: DateTime<Utc>, end: DateTime<Utc>) -> DaySummary {
        let max = |series: &[Interval]| values(series, start, end).reduce(f64::max);
        let min = |series: &[Interval]| values(series, start, end).reduce(f64::min);
        let (wind_speed, wind_direction) = self.wind(start, end);
        DaySummary {
            date,
            first_day: false,
            // the daytime high, and the low of the night that follows: NWS gives
            // each as one value for the period, which belongs to the day it starts in
            high: starting_in(&self.max_temperature, start, end)
                .filter_map(|interval| interval.value)
                .reduce(f64::max),
            low: starting_in(&self.min_temperature, start, end)
                .max_by_key(|interval| interval.start)
                .and_then(|interval| interval.value),
            wet_bulb_globe: max(&self.wet_bulb_globe),
            heat_risk: max(&self.heat_risk),
            humidity: min(&self.humidity).zip(max(&self.humidity)),
            precip_chance: max(&self.precip_chance),
            precip: total(&self.precip, start, end),
            snow: total(&self.snow, start, end),
            snow_level: min(&self.snow_level),
            elevation: None,
            ice: total(&self.ice, start, end),
            wind_speed,
            wind_direction,
            gust: max(&self.gust),
            visibility: min(&self.visibility),
            pressure: min(&self.pressure),
            ceiling: min(&self.ceiling),
        }
    }

    // the highest sustained wind of the day, and the direction at that time
    fn wind(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> (Option<f64>, Option<f64>) {
        let mut fastest: Option<(DateTime<Utc>, f64)> = None;
        for interval in overlapping(&self.wind_speed, start, end) {
            if let Some(speed) = interval.value
                && fastest.is_none_or(|(_, fastest)| speed > fastest)
            {
                fastest = Some((interval.start, speed));
            }
        }
        let Some((time, speed)) = fastest else {
            return (None, None);
        };
        let direction = self
            .wind_direction
            .iter()
            .find(|interval| interval.start <= time && time < interval.end)
            .and_then(|interval| interval.value);
        (Some(speed), direction)
    }
}

// a day's forecast values, in the units of the reply. None is no data
#[derive(Debug)]
pub struct DaySummary {
    pub date: NaiveDate,
    // the first day of the forecast, the only one with the elevation
    pub first_day: bool,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub wet_bulb_globe: Option<f64>,
    pub heat_risk: Option<f64>,
    pub humidity: Option<(f64, f64)>,
    pub precip_chance: Option<f64>,
    pub precip: Option<f64>,
    pub snow: Option<f64>,
    pub snow_level: Option<f64>,
    pub elevation: Option<f64>,
    pub ice: Option<f64>,
    pub wind_speed: Option<f64>,
    pub wind_direction: Option<f64>,
    pub gust: Option<f64>,
    pub visibility: Option<f64>,
    pub pressure: Option<f64>,
    pub ceiling: Option<f64>,
}

impl DaySummary {
    // the base fields, which every forecast day has unless it is past the
    // end of the NWS forecast
    fn has_data(&self) -> bool {
        [
            self.high,
            self.low,
            self.precip_chance,
            self.precip,
            self.snow,
            self.ice,
            self.wind_speed,
        ]
        .iter()
        .any(Option::is_some)
    }
}

// converts a series' values to unit, leaving out values without a readable time.
// a series whose unit can't be converted is logged and treated as no data
fn convert_series<T>(
    field: &str,
    uom: Option<&str>,
    unit: unit_conversion::Unit,
    values: &[T],
    read: impl Fn(&T) -> (&Option<String>, Option<f64>),
) -> Vec<Interval> {
    let Some(convert) = unit.converter(uom) else {
        tracing::warn!(
            "daily_values: {field} has unit {uom:?}, which can't be converted, leaving it out"
        );
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| {
            let (valid_time, value) = read(value);
            let (start, end) = parse_valid_time(valid_time.as_deref()?)?;
            Some(Interval {
                start,
                end,
                value: value.map(convert),
            })
        })
        .collect()
}

// converts the forecast grid cell's elevation to feet. one whose unit can't be
// converted is logged and left out, like a series
fn convert_elevation(elevation: &Elevation) -> Option<f64> {
    let value = elevation.value?;
    let uom = elevation.uom.as_deref();
    let Some(convert) = unit_conversion::Unit::Feet.converter(uom) else {
        tracing::warn!(
            "daily_values: elevation has unit {uom:?}, which can't be converted, leaving it out"
        );
        return None;
    };
    Some(convert(f64::from(value)))
}

// an NWS validTime such as "2026-10-06T13:00:00+00:00/PT1H" as its start and end
fn parse_valid_time(valid_time: &str) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let (start, duration) = valid_time.split_once('/')?;
    let start = DateTime::<FixedOffset>::parse_from_rfc3339(start)
        .ok()?
        .with_timezone(&Utc);
    Some((start, start + parse_duration(duration)?))
}

// an ISO 8601 duration as NWS writes them, such as PT1H, P3DT14H, or P7D
fn parse_duration(duration: &str) -> Option<TimeDelta> {
    let mut total = TimeDelta::zero();
    let mut in_time = false;
    let mut number = String::new();
    for c in duration.strip_prefix('P')?.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            designator => {
                let count: i64 = number.parse().ok()?;
                number.clear();
                total += match (in_time, designator) {
                    (false, 'W') => TimeDelta::weeks(count),
                    (false, 'D') => TimeDelta::days(count),
                    (true, 'H') => TimeDelta::hours(count),
                    (true, 'M') => TimeDelta::minutes(count),
                    (true, 'S') => TimeDelta::seconds(count),
                    _ => return None,
                };
            }
        }
    }
    (number.is_empty() && total > TimeDelta::zero()).then_some(total)
}

fn overlapping(
    series: &[Interval],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> impl Iterator<Item = &Interval> {
    series
        .iter()
        .filter(move |interval| interval.start < end && interval.end > start)
}

fn starting_in(
    series: &[Interval],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> impl Iterator<Item = &Interval> {
    series
        .iter()
        .filter(move |interval| start <= interval.start && interval.start < end)
}

fn values(
    series: &[Interval],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> impl Iterator<Item = f64> {
    overlapping(series, start, end).filter_map(|interval| interval.value)
}

// the sum of the values overlapping the day, each scaled by the share of its
// time inside the day. a null value is no data, not zero, so it is skipped.
// None when no value overlaps the day
fn total(series: &[Interval], start: DateTime<Utc>, end: DateTime<Utc>) -> Option<f64> {
    let mut sum = None;
    for interval in overlapping(series, start, end) {
        let Some(value) = interval.value else {
            continue;
        };
        let inside = (interval.end.min(end) - interval.start.max(start)).num_seconds() as f64;
        let length = (interval.end - interval.start).num_seconds() as f64;
        sum = Some(sum.unwrap_or(0.0) + value * inside / length);
    }
    sum
}
