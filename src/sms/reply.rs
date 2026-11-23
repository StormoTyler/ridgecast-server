// formats replies to SMS requests, and the messages trackers send, as the text
// messages defined in docs/sms-reply-format.md. the alert, avalanche, forecast day,
// and packing helpers are public so the scheduler's tracker messages can reuse them

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
use chrono_tz::Tz;

// local modules
use crate::daily_forecast;
use crate::errors;
use crate::models::{Alert, AvyProperties, ForecastProperties, Point};
use crate::sms::{daily_values, forecast, request};
use crate::tracking::tracker_checks;

// ---- message limits, line order, and alert severity ----

// the most restrictive limit found, from SMS via satellite on iPhone. the code
// works from BODY_LENGTH, which is derived from it, so only the tests use it
#[allow(dead_code)]
pub const MESSAGE_LENGTH: usize = 140;
// MESSAGE_LENGTH less the longest header and its line feed, "(x/n) " and a
// 10 character tracker name, so every body fits under any header
pub const BODY_LENGTH: usize = 123;
// the header name of replies, errors, and the help message, which identifies
// the sender. it fits in the space kept for a tracker name
pub const BRAND: &str = "RidgeCast";
// the n in an (x/n) header is one digit
pub const MAX_MESSAGES: usize = 9;
// forecast days in the brief format add messages only up to this many. more
// than one, so a forecast is less likely to be dropped entirely when alerts
// take most of the first message
pub const BRIEF_MAX_MESSAGES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    // plain words, used for every message unless the request set brief
    Full,
    // short labels, used for the reply to a request that set brief and for
    // every message from the tracker it created
    Brief,
}

impl Format {
    // the format a request's or a tracker's ForecastVars.brief asks for
    pub fn from_brief(brief: bool) -> Self {
        if brief { Format::Brief } else { Format::Full }
    }
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
    let format = Format::from_brief(request.forecast_vars.brief);
    let parts = body_parts(
        response,
        format,
        &request.forecast_vars.forecast_properties,
        request.forecast_length,
        now,
    );
    let bodies = match format {
        Format::Full => pack_within(parts, MAX_MESSAGES),
        Format::Brief => pack_brief(parts),
    };
    add_headers(bodies, None)
}

// a response's lines in REPLY_ORDER, before they are packed into messages.
// independent of the request, so a tracker's messages can be built from its own
// settings: flags are the optional forecast fields and days the number of forecast
// days. a response without a tracker has no tracker line
fn body_parts(
    response: &forecast::SmsForecastResponse,
    format: Format,
    flags: &ForecastProperties,
    days: u32,
    now: DateTime<Utc>,
) -> Vec<Part> {
    // NWS reports a time zone for every point it covers, so UTC is only a fallback
    let tz = daily_forecast::point_time_zone(&response.point).unwrap_or(Tz::UTC);
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
                    let day_lines = forecast_days(forecast, flags, tz, now, days, format);
                    parts.extend(day_lines.into_iter().map(Part::Day));
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
    parts
}

fn notice_lines(response: &forecast::SmsForecastResponse, format: Format) -> Vec<String> {
    let tracker_failed = response.tracker == Some(forecast::TrackerStatus::NotCreated);
    let mut lines = Vec::new();
    match format {
        Format::Full => {
            if response.daily_notice {
                lines.push(DAILY_FORECAST_UNAVAILABLE.to_string());
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

// ---- tracker notifications ----

// the avalanche notification for what a tracker's avalanche check found, as
// responses headed with the tracker's name. empty when the update sends nothing.
// avy is the tracker's stored forecast after the check, which a new forecast
// replaced. format is the tracker's own, from the request that created it
// unused until the scheduler sends tracker messages, remove the allow then
#[allow(dead_code)]
pub fn format_avy_notification(
    update: tracker_checks::AvyTrackerUpdate,
    avy: Option<&AvyProperties>,
    format: Format,
    tracker_name: &str,
) -> Vec<Vec<String>> {
    let lines = match update {
        tracker_checks::AvyTrackerUpdate::NewForecast => match avy {
            Some(avy) => avalanche_lines(Some(avy), None, false, format),
            // check_avy_tracker always stores the new forecast, so this can't happen.
            // the loss notice is sent rather than nothing, since better safe than sorry
            None => {
                tracing::error!("format_avy_notification: a new forecast with none stored");
                avalanche_lines(
                    None,
                    Some(forecast::AvyNotice::NoCurrentForecast),
                    false,
                    format,
                )
            }
        },
        tracker_checks::AvyTrackerUpdate::ForecastLost => avalanche_lines(
            None,
            Some(forecast::AvyNotice::NoCurrentForecast),
            false,
            format,
        ),
        tracker_checks::AvyTrackerUpdate::OffSeason => {
            avalanche_lines(None, Some(forecast::AvyNotice::OffSeason), false, format)
        }
        // the forecast can't be confirmed from out of date data
        tracker_checks::AvyTrackerUpdate::DataStale => avalanche_lines(
            None,
            Some(forecast::AvyNotice::NoCurrentForecast),
            true,
            format,
        ),
        tracker_checks::AvyTrackerUpdate::DataUnavailable => {
            avalanche_lines(None, Some(forecast::AvyNotice::Unavailable), false, format)
        }
        // the user already has the restored forecast
        tracker_checks::AvyTrackerUpdate::Unchanged
        | tracker_checks::AvyTrackerUpdate::ForecastRestored => Vec::new(),
    };
    if lines.is_empty() {
        return Vec::new();
    }
    let parts: Vec<Part> = lines.into_iter().map(Part::Line).collect();
    add_headers(pack_messages(&parts), Some(tracker_name))
}

// the alert notification for what a tracker's alert check found, as responses
// headed with the tracker's name, of at most MAX_MESSAGES messages each, continuing
// in more responses so no alert is left out. empty when the update sends nothing.
// alert times are in the point's time zone. format is the tracker's own
// unused until the scheduler sends tracker messages, remove the allow then
#[allow(dead_code)]
pub fn format_alert_notification(
    update: &tracker_checks::AlertTrackerUpdate,
    point: &Point,
    format: Format,
    tracker_name: &str,
) -> Vec<Vec<String>> {
    // NWS reports a time zone for every point it covers, so UTC is only a fallback
    let tz = daily_forecast::point_time_zone(point).unwrap_or(Tz::UTC);
    let lines = match update {
        // the stale notice comes first, so it stays in the first message
        tracker_checks::AlertTrackerUpdate::NewAlerts { alerts, stale } => {
            alert_lines(alerts, false, *stale, tz, format)
        }
        // the could not be checked notice, sent alone
        tracker_checks::AlertTrackerUpdate::Uncheckable => {
            alert_lines(&[], true, false, tz, format)
        }
        tracker_checks::AlertTrackerUpdate::Unchanged
        | tracker_checks::AlertTrackerUpdate::CheckableAgain => Vec::new(),
    };
    if lines.is_empty() {
        return Vec::new();
    }
    let parts: Vec<Part> = lines.into_iter().map(Part::Line).collect();
    add_headers(pack_messages(&parts), Some(tracker_name))
}

// a daily forecast tracker's 6 AM message, as responses headed with the tracker's
// name: the response's lines in REPLY_ORDER, with today's forecast only. it has no
// fixed message limit, so today is never dropped: it fills the space left after
// the alerts or starts a new message. the response is built like a reply's, with
// all active alerts and the avalanche line, and no tracker, so it has no tracker
// line. flags and format are the tracker's own, and now is the check's hour, which
// decides what today is
// unused until the scheduler sends tracker messages, remove the allow then
#[allow(dead_code)]
pub fn format_daily_forecast(
    response: &forecast::SmsForecastResponse,
    flags: &ForecastProperties,
    format: Format,
    tracker_name: &str,
    now: DateTime<Utc>,
) -> Vec<Vec<String>> {
    let parts = body_parts(response, format, flags, 1, now);
    add_headers(pack_messages(&parts), Some(tracker_name))
}

// the notice sent once when a daily forecast could not be fetched by its last
// attempt, at the end of that check's notifications or alone. the same full
// wording as a reply's notice that no daily forecast tracker was created
// unused until the scheduler sends tracker messages, remove the allow then
#[allow(dead_code)]
pub fn daily_forecast_unavailable(format: Format) -> String {
    match format {
        Format::Full => DAILY_FORECAST_UNAVAILABLE,
        Format::Brief => "!no forecast",
    }
    .to_string()
}

const DAILY_FORECAST_UNAVAILABLE: &str = "Daily forecast unavailable";

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
    daily_values::days(forecast, tz, now, days)
        .iter()
        .filter_map(|day| {
            let lines = match format {
                Format::Full => full_day_lines(day, flags),
                Format::Brief => vec![brief_day_line(day, flags)],
            };
            (!lines.is_empty()).then_some(lines)
        })
        .collect()
}

// ---- forecast days: text ----

// the day's sentences in DAY_ORDER, on as many lines as they need. each line
// starts with the day name
fn full_day_lines(day: &daily_values::DaySummary, flags: &ForecastProperties) -> Vec<String> {
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
    day: &daily_values::DaySummary,
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
fn brief_day_line(day: &daily_values::DaySummary, flags: &ForecastProperties) -> String {
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

fn brief_token(
    kind: DaySentence,
    day: &daily_values::DaySummary,
    flags: &ForecastProperties,
) -> Option<String> {
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
            .map(|snow| format!("SN{snow}")),
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
// sends on its own and BRAND for everything else. bodies beyond MAX_MESSAGES
// continue as another response, numbered from (1/n) again
pub fn add_headers(bodies: Vec<Vec<String>>, tracker_name: Option<&str>) -> Vec<Vec<String>> {
    let name = tracker_name.unwrap_or(BRAND);
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
                        "({}/{}) {name}\n{}",
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

// SUPPORT_EMAIL, the contact address in the help message. set once at startup
// by set_support_email, before any message is handled
static SUPPORT_EMAIL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

// reads the value of SUPPORT_EMAIL, None when unset, and keeps it for the help
// message. an error stops the server at startup: the address is required, since
// toll-free verification requires it in the HELP response, and the help line
// must fit in BODY_LENGTH
pub fn set_support_email(value: Option<&str>) -> Result<(), String> {
    let email = value
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .ok_or("SUPPORT_EMAIL must be set")?;
    if !email.contains('@') || email.chars().any(char::is_whitespace) {
        return Err("SUPPORT_EMAIL must be an email address, such as support@example.com".into());
    }
    let room = BODY_LENGTH - help_line("").chars().count();
    if email.chars().count() > room {
        return Err(format!("SUPPORT_EMAIL must be at most {room} characters"));
    }
    SUPPORT_EMAIL
        .set(email.to_string())
        .map_err(|_| "SUPPORT_EMAIL was already set".to_string())
}

// the help message's text, which toll-free verification requires of the HELP
// response
fn help_line(email: &str) -> String {
    format!("Help: {email}. Reply STOP to opt out.")
}

fn help() -> String {
    help_line(
        SUPPORT_EMAIL
            .get()
            .expect("SUPPORT_EMAIL is set at startup"),
    )
}

// the help message, a single message
pub fn help_message() -> String {
    single_message(help())
}

// the opt-in message's text: what the service sends, how often, that rates may
// apply, and how to get help or opt out, which toll-free verification requires
// of the opt-in confirmation. no final period, so it fits in BODY_LENGTH
const OPT_IN: &str = "Forecasts you request and tracker alerts. Msg freq varies. \
    Msg&data rates may apply. Reply HELP for help, STOP to opt out";

// the opt-in message, a single message that inbound sends once, before the
// reply to a number's first text
pub fn opt_in_message() -> String {
    single_message(OPT_IN.to_string())
}

// the reply to a request that could not be read, a single message. a message
// that is not a request at all gets the help message
pub fn format_parse_error(error: &request::SmsParseError) -> String {
    let line = match error {
        request::SmsParseError::NotARequest => return help_message(),
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
    format!("(1/1) {BRAND}\n{line}")
}
