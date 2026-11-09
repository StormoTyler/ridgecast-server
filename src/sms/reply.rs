// formats replies to SMS requests as the text messages defined in
// docs/sms-reply-format.md. the alert, avalanche, forecast day, and packing
// helpers are public so the scheduler's tracker messages can reuse them

use chrono::{
    DateTime, Datelike, Days, FixedOffset, NaiveDate, TimeDelta, TimeZone, Timelike, Utc, Weekday,
};
use chrono_tz::Tz;

// local modules
use crate::daily_forecast;
use crate::errors;
use crate::models::{Alert, AvyProperties, FloatVec, ForecastProperties, IntVec};
use crate::sms::{forecast, request};

// ---- message limits, line order, and alert severity ----

// the most restrictive limit found, from SMS via satellite on iPhone. the code
// works from BODY_LENGTH, which is derived from it, so only the tests use it
#[allow(dead_code)]
pub const MESSAGE_LENGTH: usize = 140;
// MESSAGE_LENGTH less the longest header and its line feed, "(x/n) " and a
// 10 character tracker name, so every body fits under any header
pub const BODY_LENGTH: usize = 123;
// the n in an (x/n) header is one digit
pub const MAX_MESSAGES: usize = 9;
// forecast days in the brief format add messages only up to this many. more
// than one, so a forecast is less likely to be dropped entirely when alerts
// take most of the first message
pub const BRIEF_MAX_MESSAGES: usize = 3;

const PASCALS_PER_INCH_OF_MERCURY: f64 = 3386.389;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    // plain words, used for every message unless the request set brief
    Full,
    // short labels, used only for the reply to a request that set brief
    Brief,
}

// the order of the lines in a reply. changing it changes every reply,
// so update docs/sms-reply-format.md and its examples with it
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReplySection {
    Notices,
    Avalanche,
    Alerts,
    ForecastDays,
    Tracker,
}

// notices come first, then avalanche and alerts, since they are the most safety critical
pub const REPLY_ORDER: [ReplySection; 5] = [
    ReplySection::Notices,
    ReplySection::Avalanche,
    ReplySection::Alerts,
    ReplySection::ForecastDays,
    ReplySection::Tracker,
];

// the order of the sentences in a forecast day, which the brief format's
// tokens follow too. update docs/sms-reply-format.md with any change
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DaySentence {
    HighLow,
    WetBulbGlobe,
    HeatRisk,
    Humidity,
    Precip,
    Snow,
    SnowLevel,
    Elevation,
    Ice,
    Wind,
    Visibility,
    Pressure,
    CloudCeiling,
}

pub const DAY_ORDER: [DaySentence; 13] = [
    DaySentence::HighLow,
    DaySentence::WetBulbGlobe,
    DaySentence::HeatRisk,
    DaySentence::Humidity,
    DaySentence::Precip,
    DaySentence::Snow,
    DaySentence::SnowLevel,
    DaySentence::Elevation,
    DaySentence::Ice,
    DaySentence::Wind,
    DaySentence::Visibility,
    DaySentence::Pressure,
    DaySentence::CloudCeiling,
];

// NWS alert severity. declared in sort order, so deriving Ord sorts the most
// severe first. Unknown sorts right after Severe, since an unknown severity
// could be serious
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Extreme,
    Severe,
    Unknown,
    Moderate,
    Minor,
}

impl Severity {
    // an alert without a severity, or with one NWS doesn't define, is
    // Unknown, so a missing severity is never read as a low one
    pub fn of(alert: &Alert) -> Self {
        match alert.properties.severity.as_deref() {
            Some("Extreme") => Severity::Extreme,
            Some("Severe") => Severity::Severe,
            Some("Moderate") => Severity::Moderate,
            Some("Minor") => Severity::Minor,
            _ => Severity::Unknown,
        }
    }

    fn label(self, format: Format) -> &'static str {
        match (self, format) {
            (Severity::Extreme, Format::Full) => "Extreme",
            (Severity::Severe, Format::Full) => "Severe",
            (Severity::Unknown, Format::Full) => "Unknown",
            (Severity::Moderate, Format::Full) => "Moderate",
            (Severity::Minor, Format::Full) => "Minor",
            (Severity::Extreme, Format::Brief) => "Ext",
            (Severity::Severe, Format::Brief) => "Sev",
            (Severity::Unknown, Format::Brief) => "Unk",
            (Severity::Moderate, Format::Brief) => "Mod",
            (Severity::Minor, Format::Brief) => "Min",
        }
    }
}

// what pack_messages places in messages
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    // a line that is never dropped
    Line(String),
    // a forecast day's lines, kept in one message when they fit. days are
    // the only lines dropped to stay within a message limit
    Day(Vec<String>),
}

// ---- the reply ----

// the reply to a request, as responses of at most MAX_MESSAGES messages each,
// sent in order. more than one response is needed only when the lines that
// are never dropped, such as alerts, don't fit in MAX_MESSAGES
pub fn format_reply(
    request: &request::SmsRequest,
    response: &forecast::SmsForecastResponse,
    now: DateTime<Utc>,
) -> Vec<Vec<String>> {
    // NWS reports a time zone for every point it covers, so UTC is only a fallback
    let tz = daily_forecast::point_time_zone(&response.point).unwrap_or(Tz::UTC);
    let format = if request.forecast_vars.brief {
        Format::Brief
    } else {
        Format::Full
    };
    let as_parts = |lines: Vec<String>| lines.into_iter().map(Part::Line);

    let mut parts = Vec::new();
    for section in REPLY_ORDER {
        match section {
            ReplySection::Notices => parts.extend(as_parts(notice_lines(response, format))),
            ReplySection::Avalanche => parts.extend(as_parts(avalanche_lines(
                response.avy.as_ref(),
                response.avy_notice,
                response.avy_stale_notice,
                format,
            ))),
            ReplySection::Alerts => parts.extend(as_parts(alert_lines(
                &response.alerts,
                response.alerts_notice,
                response.alerts_stale_notice,
                tz,
                format,
            ))),
            ReplySection::ForecastDays => {
                if let Some(forecast) = &response.forecast {
                    let days = forecast_days(
                        forecast,
                        &request.forecast_vars.forecast_properties,
                        tz,
                        now,
                        request.forecast_length,
                        format,
                    );
                    parts.extend(days.into_iter().map(Part::Day));
                }
            }
            // the brief format has no tracker line, only the !no tracker notice
            ReplySection::Tracker => {
                if format == Format::Full
                    && let Some(forecast::TrackerStatus::Created(tracker)) = &response.tracker
                {
                    parts.push(Part::Line(tracker_line(tracker, tz)));
                }
            }
        }
    }

    let bodies = match format {
        Format::Full => pack_within(parts, MAX_MESSAGES),
        Format::Brief => pack_brief(parts),
    };
    add_headers(bodies, None)
}

fn notice_lines(response: &forecast::SmsForecastResponse, format: Format) -> Vec<String> {
    let tracker_failed = response.tracker == Some(forecast::TrackerStatus::NotCreated);
    let mut lines = Vec::new();
    match format {
        Format::Full => {
            if response.daily_notice {
                lines.push("Daily forecast unavailable".to_string());
            }
            if tracker_failed {
                lines.push("Could not create tracker".to_string());
            }
        }
        // the brief format has no notice other than !no tracker
        Format::Brief => {
            if tracker_failed {
                lines.push("!no tracker".to_string());
            }
        }
    }
    lines
}

// ---- avalanche, alert, and tracker lines ----

// the avalanche line or notice, followed by the stale notice when the data is
// out of date. empty when there is nothing to say about avalanches
pub fn avalanche_lines(
    avy: Option<&AvyProperties>,
    notice: Option<forecast::AvyNotice>,
    stale: bool,
    format: Format,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(avy) = avy {
        let zone = avy
            .name
            .as_deref()
            .map(|name| format!(", {name}"))
            .unwrap_or_default();
        lines.push(match (avy.danger_level, format) {
            (Some(-1), Format::Full) => format!("Avy: no danger rating{zone}"),
            (Some(-1), Format::Brief) => "AV no rating".to_string(),
            (Some(level @ 1..=5), Format::Full) => {
                format!("Avy danger {level} {}{zone}", danger_rating(level))
            }
            (Some(level @ 1..=5), Format::Brief) => format!("AV{level} {}", danger_rating(level)),
            // nac::check_avy_geometry never returns another level as a forecast
            (_, Format::Full) => "Avy: no current forecast".to_string(),
            (_, Format::Brief) => "AV no forecast".to_string(),
        });
    } else if let Some(notice) = notice {
        lines.push(
            match (notice, format) {
                (forecast::AvyNotice::NoCurrentForecast, Format::Full) => {
                    "Avy: no current forecast"
                }
                (forecast::AvyNotice::NoCurrentForecast, Format::Brief) => "AV no forecast",
                (forecast::AvyNotice::OffSeason, Format::Full) => {
                    "Avy: center is in its off season"
                }
                (forecast::AvyNotice::OffSeason, Format::Brief) => "AV off season",
                (forecast::AvyNotice::Unavailable, Format::Full) => {
                    "Avy: data unavailable, try later"
                }
                (forecast::AvyNotice::Unavailable, Format::Brief) => "AV unavailable",
            }
            .to_string(),
        );
    }
    if stale {
        lines.push(
            match format {
                Format::Full => "Avy: data may be out of date",
                Format::Brief => "AV may be old",
            }
            .to_string(),
        );
    }
    lines
}

// the North American Public Avalanche Danger Scale
fn danger_rating(level: i32) -> &'static str {
    match level {
        1 => "Low",
        2 => "Moderate",
        3 => "Considerable",
        4 => "High",
        _ => "Extreme",
    }
}

// the alert lines, most severe first. unavailable replaces them with a
// notice, and stale adds a notice before them, so it stays in the first
// message however many alerts follow
pub fn alert_lines(
    alerts: &[Alert],
    unavailable: bool,
    stale: bool,
    tz: Tz,
    format: Format,
) -> Vec<String> {
    if unavailable {
        return vec![
            match format {
                Format::Full => "!Alerts could not be checked",
                Format::Brief => "!alerts not checked",
            }
            .to_string(),
        ];
    }
    let mut lines = Vec::new();
    if stale {
        lines.push(
            match format {
                Format::Full => "!Alerts may be out of date",
                Format::Brief => "!alerts may be old",
            }
            .to_string(),
        );
    }
    let mut sorted: Vec<&Alert> = alerts.iter().collect();
    // a stable sort, so alerts with the same severity keep the order NWS sent them in
    sorted.sort_by_key(|alert| Severity::of(alert));
    lines.extend(
        sorted
            .into_iter()
            .map(|alert| alert_line(alert, tz, format)),
    );
    lines
}

pub fn alert_line(alert: &Alert, tz: Tz, format: Format) -> String {
    let severity = Severity::of(alert).label(format);
    let event = alert.properties.event.as_deref().unwrap_or("Alert");
    let mut line = match format {
        Format::Full => format!("!{severity}: {event}"),
        Format::Brief => format!("!{severity} {event}"),
    };
    // ends is when the hazard ends. an alert without it ends after the event name
    if let Some(ends) = alert.properties.ends {
        let until = match format {
            Format::Full => "until",
            Format::Brief => "til",
        };
        line.push_str(&format!(
            " {until} {}",
            point_in_time(ends.with_timezone(&tz), format)
        ));
    }
    line
}

fn tracker_line(tracker: &forecast::CreatedTracker, tz: Tz) -> String {
    let tracked: Vec<&str> = [
        (tracker.alerts, "alerts"),
        (tracker.avy, "avy"),
        (tracker.daily_forecast, "daily forecast"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    format!(
        "Tracking {}: {} until {}",
        tracker.name,
        tracked.join(", "),
        point_in_time(tracker.ends.with_timezone(&tz), Format::Full)
    )
}

// ---- times and day names ----

// a day, the month and day, and the 12-hour time, such as Wed 10/7 6PM, or We7 6PM
// in the brief format. minutes are shown only when they are not zero
pub fn point_in_time(time: DateTime<Tz>, format: Format) -> String {
    let hour = match time.hour() % 12 {
        0 => 12,
        hour => hour,
    };
    let minutes = match time.minute() {
        0 => String::new(),
        minute => format!(":{minute:02}"),
    };
    let suffix = if time.hour() < 12 { "AM" } else { "PM" };
    let day = day_name(time.weekday(), format);
    match format {
        Format::Full => format!(
            "{day} {}/{} {hour}{minutes}{suffix}",
            time.month(),
            time.day()
        ),
        Format::Brief => format!("{day}{} {hour}{minutes}{suffix}", time.day()),
    }
}

fn day_name(day: Weekday, format: Format) -> &'static str {
    match (day, format) {
        (Weekday::Mon, Format::Full) => "Mon",
        (Weekday::Tue, Format::Full) => "Tue",
        (Weekday::Wed, Format::Full) => "Wed",
        (Weekday::Thu, Format::Full) => "Thu",
        (Weekday::Fri, Format::Full) => "Fri",
        (Weekday::Sat, Format::Full) => "Sat",
        (Weekday::Sun, Format::Full) => "Sun",
        (Weekday::Mon, Format::Brief) => "Mo",
        (Weekday::Tue, Format::Brief) => "Tu",
        (Weekday::Wed, Format::Brief) => "We",
        (Weekday::Thu, Format::Brief) => "Th",
        (Weekday::Fri, Format::Brief) => "Fr",
        (Weekday::Sat, Format::Brief) => "Sa",
        (Weekday::Sun, Format::Brief) => "Su",
    }
}

// ---- forecast days ----

// the lines of each forecast day, starting with today in tz. a day with no
// forecast data, such as one past the end of the NWS forecast, is left out
pub fn forecast_days(
    forecast: &forecast::RequestedForecast,
    flags: &ForecastProperties,
    tz: Tz,
    now: DateTime<Utc>,
    days: u32,
    format: Format,
) -> Vec<Vec<String>> {
    let series = Series::new(forecast);
    let today = now.with_timezone(&tz).date_naive();
    (0..days)
        .filter_map(|offset| {
            let date = today.checked_add_days(Days::new(offset.into()))?;
            let (start, end) = day_bounds(tz, date)?;
            let mut day = series.summarize(date, start, end);
            // the elevation is the same every day, so only the first day has it
            day.first_day = offset == 0;
            if day.first_day {
                day.elevation = forecast
                    .elevation
                    .as_ref()
                    .and_then(|elevation| elevation.value)
                    .map(|meters| f64::from(meters) / 0.3048);
            }
            if !day.has_data() {
                return None;
            }
            let lines = match format {
                Format::Full => full_day_lines(&day, flags),
                Format::Brief => vec![brief_day_line(&day, flags)],
            };
            (!lines.is_empty()).then_some(lines)
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

// ---- forecast days: NWS series to daily values ----

// one NWS value and the time it covers, converted to the reply's units
#[derive(Debug, Clone, Copy)]
struct Interval {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    // None for a value NWS left out, such as a cloud ceiling when there is none
    value: Option<f64>,
}

// the units the reply uses
#[derive(Debug, Clone, Copy)]
enum Unit {
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
    fn converter(self, uom: Option<&str>) -> Option<fn(f64) -> f64> {
        let convert: fn(f64) -> f64 = match (self, uom) {
            (Unit::Fahrenheit, Some("wmoUnit:degC")) => |celsius| celsius * 9.0 / 5.0 + 32.0,
            (Unit::Fahrenheit, Some("wmoUnit:degF")) => |fahrenheit| fahrenheit,
            (Unit::Inches, Some("wmoUnit:mm")) => |mm| mm / 25.4,
            (Unit::Inches, Some("wmoUnit:cm")) => |cm| cm / 2.54,
            (Unit::Feet, Some("wmoUnit:m")) => |meters| meters / 0.3048,
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
                Unit::Fahrenheit,
            ),
            min_temperature: floats(
                "min_temperature",
                Some(&forecast.min_temperature),
                Unit::Fahrenheit,
            ),
            wet_bulb_globe: floats(
                "wet_bulb_globe_temperature",
                forecast.wet_bulb_globe_temperature.as_ref(),
                Unit::Fahrenheit,
            ),
            heat_risk: ints("heat_risk", forecast.heat_risk.as_ref(), Unit::Category),
            humidity: ints(
                "relative_humidity",
                forecast.relative_humidity.as_ref(),
                Unit::Percent,
            ),
            precip_chance: ints(
                "probability_of_precipitation",
                Some(&forecast.probability_of_precipitation),
                Unit::Percent,
            ),
            precip: floats(
                "quantitative_precipitation",
                Some(&forecast.quantitative_precipitation),
                Unit::Inches,
            ),
            snow: floats(
                "snowfall_amount",
                Some(&forecast.snowfall_amount),
                Unit::Inches,
            ),
            snow_level: floats("snow_level", forecast.snow_level.as_ref(), Unit::Feet),
            ice: floats(
                "ice_accumulation",
                Some(&forecast.ice_accumulation),
                Unit::Inches,
            ),
            wind_speed: floats("wind_speed", Some(&forecast.wind_speed), Unit::Mph),
            wind_direction: ints(
                "wind_direction",
                Some(&forecast.wind_direction),
                Unit::Degrees,
            ),
            gust: floats("wind_gust", forecast.wind_gust.as_ref(), Unit::Mph),
            visibility: floats("visibility", forecast.visibility.as_ref(), Unit::Miles),
            pressure: floats(
                "pressure",
                forecast.pressure.as_ref(),
                Unit::InchesOfMercury,
            ),
            ceiling: floats(
                "ceiling_height",
                forecast.ceiling_height.as_ref(),
                Unit::Feet,
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
struct DaySummary {
    date: NaiveDate,
    // the first day of the forecast, the only one with the elevation
    first_day: bool,
    high: Option<f64>,
    low: Option<f64>,
    wet_bulb_globe: Option<f64>,
    heat_risk: Option<f64>,
    humidity: Option<(f64, f64)>,
    precip_chance: Option<f64>,
    precip: Option<f64>,
    snow: Option<f64>,
    snow_level: Option<f64>,
    elevation: Option<f64>,
    ice: Option<f64>,
    wind_speed: Option<f64>,
    wind_direction: Option<f64>,
    gust: Option<f64>,
    visibility: Option<f64>,
    pressure: Option<f64>,
    ceiling: Option<f64>,
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
    unit: Unit,
    values: &[T],
    read: impl Fn(&T) -> (&Option<String>, Option<f64>),
) -> Vec<Interval> {
    let Some(convert) = unit.converter(uom) else {
        eprintln!("sms_reply: {field} has unit {uom:?}, which can't be converted, leaving it out");
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

// ---- forecast days: text ----

// the day's sentences in DAY_ORDER, on as many lines as they need. each line
// starts with the day name
fn full_day_lines(day: &DaySummary, flags: &ForecastProperties) -> Vec<String> {
    let prefix = format!("{}: ", day_name(day.date.weekday(), Format::Full));
    let mut lines: Vec<String> = Vec::new();
    for sentence in DAY_ORDER
        .iter()
        .filter_map(|&kind| full_sentence(kind, day, flags))
    {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + sentence.chars().count() <= BODY_LENGTH => {
                line.push(' ');
                line.push_str(&sentence);
            }
            _ => lines.push(format!("{prefix}{sentence}")),
        }
    }
    lines
}

// a requested optional field with no data says N/A, so the user knows it was checked
fn optional(requested: bool, label: &str, value: Option<String>) -> Option<String> {
    requested.then(|| format!("{label} {}", value.as_deref().unwrap_or("N/A")))
}

fn full_sentence(
    kind: DaySentence,
    day: &DaySummary,
    flags: &ForecastProperties,
) -> Option<String> {
    match kind {
        DaySentence::HighLow => match (day.high, day.low) {
            (Some(high), Some(low)) => Some(format!("high {}F, low {}F", whole(high), whole(low))),
            (Some(high), None) => Some(format!("high {}F", whole(high))),
            (None, Some(low)) => Some(format!("low {}F", whole(low))),
            (None, None) => None,
        },
        DaySentence::WetBulbGlobe => optional(
            flags.wet_bulb_globe_temperature,
            "Wet bulb globe",
            day.wet_bulb_globe.map(|value| format!("{}F", whole(value))),
        ),
        DaySentence::HeatRisk => optional(
            flags.heat_risk,
            "Heat risk",
            day.heat_risk.map(|value| format!("{} of 4", whole(value))),
        ),
        DaySentence::Humidity => optional(
            flags.relative_humidity,
            "Humidity",
            day.humidity
                .map(|(low, high)| match (whole(low), whole(high)) {
                    (low, high) if low == high => format!("{low}%"),
                    (low, high) => format!("{low}-{high}%"),
                }),
        ),
        // the chance is always included, even at 0%, since users expect it.
        // the amount is left out when it is zero
        // the chance is N/A when there is an amount without one, so the amount is
        // still shown
        DaySentence::Precip => match (day.precip_chance, day.precip.and_then(hundredths)) {
            (Some(chance), Some(amount)) => Some(format!("Precip {}%, {amount}in", whole(chance))),
            (Some(chance), None) => Some(format!("Precip {}%", whole(chance))),
            (None, Some(amount)) => Some(format!("Precip N/A, {amount}in")),
            (None, None) => None,
        },
        DaySentence::Snow => day
            .snow
            .and_then(snow_inches)
            .map(|snow| format!("Snow {snow}in")),
        DaySentence::SnowLevel => optional(
            flags.snow_level,
            "Snow level",
            day.snow_level
                .map(|feet| format!("{}ft", whole(feet / 100.0) * 100)),
        ),
        DaySentence::Elevation => optional(
            flags.elevation && day.first_day,
            "Elevation",
            day.elevation.map(|feet| format!("{}ft", whole(feet))),
        ),
        DaySentence::Ice => day
            .ice
            .and_then(hundredths)
            .map(|ice| format!("Ice {ice}in")),
        // the wind is N/A when gusts were requested and there is no wind speed,
        // so the gust, or its own N/A, is still shown
        DaySentence::Wind => {
            let gusts = match (flags.wind_gust, day.gust) {
                (true, Some(gust)) => format!(", gusts {}", whole(gust)),
                (true, None) => ", gusts N/A".to_string(),
                (false, _) => String::new(),
            };
            match day.wind_speed {
                Some(speed) => {
                    let direction = day
                        .wind_direction
                        .map(|degrees| format!("{} ", compass(degrees)))
                        .unwrap_or_default();
                    Some(format!("Wind {direction}{}mph{gusts}", whole(speed)))
                }
                None if flags.wind_gust => Some(format!("Wind N/A{gusts}")),
                None => None,
            }
        }
        DaySentence::Visibility => optional(
            flags.visibility,
            "Visibility",
            day.visibility
                .map(|miles| format!("{}mi", visibility_miles(miles))),
        ),
        DaySentence::Pressure => optional(
            flags.pressure,
            "Pressure",
            day.pressure.map(|pressure| format!("{pressure:.2}inHg")),
        ),
        // no value is no ceiling
        DaySentence::CloudCeiling => optional(
            flags.ceiling_height,
            "Cloud ceiling",
            day.ceiling.map(|feet| format!("{}ft", whole(feet))),
        ),
    }
}

// the day name, then a token for each sentence the full format would show, in
// DAY_ORDER. high/low, precip chance, and wind are always present, with -- for a
// value with no data
fn brief_day_line(day: &DaySummary, flags: &ForecastProperties) -> String {
    let mut tokens = vec![day_name(day.date.weekday(), Format::Brief).to_string()];
    tokens.extend(
        DAY_ORDER
            .iter()
            .filter_map(|&kind| brief_token(kind, day, flags)),
    );
    tokens.join(" ")
}

// a requested optional field with no data is its label then --, the brief N/A
fn brief_optional(requested: bool, label: &str, value: Option<String>) -> Option<String> {
    requested.then(|| format!("{label}{}", value.as_deref().unwrap_or("--")))
}

fn brief_token(kind: DaySentence, day: &DaySummary, flags: &ForecastProperties) -> Option<String> {
    let degrees =
        |value: Option<f64>| value.map_or("--".to_string(), |value| whole(value).to_string());
    match kind {
        DaySentence::HighLow => Some(format!("{}/{}", degrees(day.high), degrees(day.low))),
        DaySentence::WetBulbGlobe => brief_optional(
            flags.wet_bulb_globe_temperature,
            "WB",
            day.wet_bulb_globe.map(|value| whole(value).to_string()),
        ),
        DaySentence::HeatRisk => brief_optional(
            flags.heat_risk,
            "HR",
            day.heat_risk.map(|value| whole(value).to_string()),
        ),
        DaySentence::Humidity => brief_optional(
            flags.relative_humidity,
            "H",
            day.humidity
                .map(|(low, high)| match (whole(low), whole(high)) {
                    (low, high) if low == high => low.to_string(),
                    (low, high) => format!("{low}-{high}"),
                }),
        ),
        // P-- before an amount without a chance, as the full format's N/A
        DaySentence::Precip => match (day.precip_chance, day.precip.and_then(hundredths)) {
            (Some(chance), Some(amount)) => Some(format!("P{} R{amount}", whole(chance))),
            (Some(chance), None) => Some(format!("P{}", whole(chance))),
            (None, Some(amount)) => Some(format!("P-- R{amount}")),
            (None, None) => None,
        },
        DaySentence::Snow => day
            .snow
            .and_then(snow_inches)
            .map(|snow| format!("S{snow}")),
        DaySentence::SnowLevel => brief_optional(
            flags.snow_level,
            "SL",
            day.snow_level
                .map(|feet| (whole(feet / 100.0) * 100).to_string()),
        ),
        DaySentence::Elevation => brief_optional(
            flags.elevation && day.first_day,
            "EL",
            day.elevation.map(|feet| whole(feet).to_string()),
        ),
        DaySentence::Ice => day.ice.and_then(hundredths).map(|ice| format!("I{ice}")),
        // gusts are joined to the wind token, and shown even with no wind data
        // so a requested field is never hidden
        DaySentence::Wind => {
            let wind = match day.wind_speed {
                Some(speed) => format!(
                    "{}{}",
                    day.wind_direction.map(compass).unwrap_or_default(),
                    whole(speed)
                ),
                None => "--".to_string(),
            };
            let gusts = match (flags.wind_gust, day.gust) {
                (true, Some(gust)) => format!("G{}", whole(gust)),
                (true, None) => "G--".to_string(),
                (false, _) => String::new(),
            };
            Some(format!("{wind}{gusts}"))
        }
        DaySentence::Visibility => {
            brief_optional(flags.visibility, "V", day.visibility.map(visibility_miles))
        }
        DaySentence::Pressure => brief_optional(
            flags.pressure,
            "PR",
            day.pressure.map(|pressure| format!("{pressure:.2}")),
        ),
        // no value is no ceiling
        DaySentence::CloudCeiling => brief_optional(
            flags.ceiling_height,
            "C",
            day.ceiling.map(|feet| whole(feet).to_string()),
        ),
    }
}

// ---- number formatting ----

fn whole(value: f64) -> i64 {
    value.round() as i64
}

// 2 decimals, None when that rounds to zero
fn hundredths(value: f64) -> Option<String> {
    let rounded = (value * 100.0).round() / 100.0;
    (rounded > 0.0).then(|| format!("{rounded:.2}"))
}

// 1 decimal under 1 inch, the nearest half inch under 4, and whole inches
// from 4. None when that rounds to zero. each range is chosen by the rounded
// value, so 0.97 is 1 and 3.8 is 4, never 1.0 or 4.0
fn snow_inches(value: f64) -> Option<String> {
    let tenths = (value * 10.0).round() / 10.0;
    if tenths <= 0.0 {
        return None;
    }
    if tenths < 1.0 {
        return Some(format!("{tenths:.1}"));
    }
    let halves = (value * 2.0).round() / 2.0;
    if halves >= 4.0 {
        // from the value, since a half rounded again would round up: 4.4 is 4, not 5
        Some(whole(value).to_string())
    } else if halves.fract() == 0.0 {
        Some(whole(halves).to_string())
    } else {
        Some(format!("{halves:.1}"))
    }
}

// 1 decimal under 1 mile, the nearest half mile above, and 10 at 10 miles
// or more, the most NWS reports
fn visibility_miles(value: f64) -> String {
    let tenths = (value * 10.0).round() / 10.0;
    if tenths < 1.0 {
        return format!("{tenths:.1}");
    }
    let halves = ((value * 2.0).round() / 2.0).min(10.0);
    if halves.fract() == 0.0 {
        whole(halves).to_string()
    } else {
        format!("{halves:.1}")
    }
}

// the 8-point compass direction the wind blows from
fn compass(degrees: f64) -> &'static str {
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    POINTS[(degrees.rem_euclid(360.0) / 45.0).round() as usize % 8]
}

// ---- packing lines into messages ----

// the length of a message body, counting the line feeds between lines
fn body_length(body: &[String]) -> usize {
    body.iter().map(|line| line.chars().count()).sum::<usize>() + body.len().saturating_sub(1)
}

fn fits(body: &[String], lines: &[String]) -> bool {
    let length = match (body.is_empty(), lines.is_empty()) {
        (true, _) => body_length(lines),
        (false, true) => body_length(body),
        // a line feed joins the body and the new lines
        (false, false) => body_length(body) + 1 + body_length(lines),
    };
    length <= BODY_LENGTH
}

// fills message bodies in order, starting a new message when the next part
// doesn't fit. a forecast day starts a new message rather than being split,
// unless it is too long for any message, then it continues to the next one
pub fn pack_messages(parts: &[Part]) -> Vec<Vec<String>> {
    let mut bodies: Vec<Vec<String>> = Vec::new();
    for part in parts {
        let lines = match part {
            Part::Line(line) => std::slice::from_ref(line),
            Part::Day(lines) => lines.as_slice(),
        };
        if let Some(body) = bodies.last_mut()
            && fits(body, lines)
        {
            body.extend(lines.iter().cloned());
        } else if fits(&[], lines) {
            bodies.push(lines.to_vec());
        } else {
            for line in lines {
                match bodies.last_mut() {
                    Some(body) if fits(body, std::slice::from_ref(line)) => body.push(line.clone()),
                    _ => bodies.push(vec![line.clone()]),
                }
            }
        }
    }
    bodies
}

// packs the parts, dropping forecast days from the last until they fit in
// max_messages. lines that are never dropped can still need more, see add_headers
pub fn pack_within(mut parts: Vec<Part>, max_messages: usize) -> Vec<Vec<String>> {
    loop {
        let bodies = pack_messages(&parts);
        let last_day = parts.iter().rposition(|part| matches!(part, Part::Day(_)));
        match last_day {
            Some(index) if bodies.len() > max_messages => {
                parts.remove(index);
            }
            _ => return bodies,
        }
    }
}

// the other lines first, in as many messages as they need. forecast days then
// fill the last message and add messages up to BRIEF_MAX_MESSAGES, stopping at
// the first day that doesn't fit
fn pack_brief(parts: Vec<Part>) -> Vec<Vec<String>> {
    let (days, lines): (Vec<Part>, Vec<Part>) = parts
        .into_iter()
        .partition(|part| matches!(part, Part::Day(_)));
    let mut bodies = pack_messages(&lines);
    if bodies.is_empty() {
        bodies.push(Vec::new());
    }
    for day in days {
        let Part::Day(day) = day else { continue };
        let last = bodies.last_mut().expect("bodies has a message");
        if fits(last, &day) {
            last.extend(day);
        } else if bodies.len() < BRIEF_MAX_MESSAGES {
            bodies.push(day);
        } else {
            break;
        }
    }
    bodies
}

// numbers the messages (x/n), with the tracker's name for messages a tracker
// sends on its own. bodies beyond MAX_MESSAGES continue as another response,
// numbered from (1/n) again
pub fn add_headers(bodies: Vec<Vec<String>>, tracker_name: Option<&str>) -> Vec<Vec<String>> {
    let name = tracker_name
        .map(|name| format!(" {name}"))
        .unwrap_or_default();
    let bodies = if bodies.is_empty() {
        vec![Vec::new()]
    } else {
        bodies
    };
    bodies
        .chunks(MAX_MESSAGES)
        .map(|response| {
            response
                .iter()
                .enumerate()
                .map(|(index, body)| {
                    format!(
                        "({}/{}){name}\n{}",
                        index + 1,
                        response.len(),
                        body.join("\n")
                    )
                })
                .collect()
        })
        .collect()
}

// ---- errors and the help message ----

// the reply to a request that could not be read, a single message. a message
// that is not a request at all gets the help message
pub fn format_parse_error(error: &request::SmsParseError) -> String {
    let line = match error {
        request::SmsParseError::NotARequest => {
            "RidgeCast: send requests from the RidgeCast app. Typed format: RC1 lat lon flags \
             hours days name. Reply STOP to opt out."
                .to_string()
        }
        request::SmsParseError::UnsupportedVersion(_) => "Update the RidgeCast app".to_string(),
        request::SmsParseError::WrongFieldCount(count) => {
            format!("Expected 6 fields, found {count}")
        }
        request::SmsParseError::InvalidLatitude(value) => with_value("Bad latitude: ", value),
        request::SmsParseError::InvalidLongitude(value) => with_value("Bad longitude: ", value),
        request::SmsParseError::InvalidFlags(value) => with_value("Bad flags: ", value),
        request::SmsParseError::InvalidTrackerDuration(value) => {
            with_value("Bad tracker hours: ", value)
        }
        request::SmsParseError::InvalidForecastLength(value) => {
            with_value("Bad forecast days: ", value)
        }
        request::SmsParseError::MissingTrackerName => "Tracker name missing".to_string(),
        request::SmsParseError::InvalidTrackerName(value) => {
            with_value("Bad tracker name: ", value)
        }
    };
    single_message(line)
}

// the reply to a request that was read but could not be answered
pub fn format_api_error(error: &errors::APIError) -> String {
    let line = match error {
        errors::APIError::LocationNotCovered => "Location not covered by NWS forecasts",
        errors::APIError::NWSError
        | errors::APIError::NACError
        | errors::APIError::ForecastError
        | errors::APIError::AlertsError
        | errors::APIError::UpstreamTimeout => "Weather service unavailable, try later",
        errors::APIError::DatabaseError | errors::APIError::InvalidLocation => {
            "Server error, try later"
        }
    };
    single_message(line.to_string())
}

// the value is from the request, which can be any length, so it is cut to
// keep the line within BODY_LENGTH
fn with_value(label: &str, value: &str) -> String {
    let room = BODY_LENGTH - label.chars().count();
    format!("{label}{}", value.chars().take(room).collect::<String>())
}

fn single_message(line: String) -> String {
    format!("(1/1)\n{line}")
}
