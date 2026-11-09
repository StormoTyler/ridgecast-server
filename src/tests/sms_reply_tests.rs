// tests for sms::reply against the format in docs/sms-reply-format.md
use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::errors;
use crate::models::{
    Alert, AvyProperties, Elevation, FloatVec, ForecastProperties, ForecastVars, IntVec, Nwr,
    Point, PointGeometry, PointProperties, TimeFloat, TimeInt,
};
use crate::sms::{forecast, reply, request};

const TZ: Tz = chrono_tz::America::Los_Angeles;

fn utc(time: &str) -> DateTime<Utc> {
    time.parse().unwrap()
}

// Tue 10/6 9AM PDT. Tuesday runs from 07:00 UTC to 07:00 UTC on 10/7
fn now() -> DateTime<Utc> {
    utc("2026-10-06T16:00:00Z")
}

fn floats(uom: &str, values: &[(&str, f32)]) -> FloatVec {
    FloatVec {
        uom: Some(uom.to_string()),
        values: values
            .iter()
            .map(|(time, value)| TimeFloat {
                valid_time: Some(time.to_string()),
                value: Some(*value),
            })
            .collect(),
    }
}

fn ints(uom: Option<&str>, values: &[(&str, i32)]) -> IntVec {
    IntVec {
        uom: uom.map(str::to_string),
        values: values
            .iter()
            .map(|(time, value)| TimeInt {
                valid_time: Some(time.to_string()),
                value: Some(*value),
            })
            .collect(),
    }
}

// a forecast with no data, which tests fill in
fn empty_forecast() -> forecast::RequestedForecast {
    forecast::RequestedForecast {
        temperature: floats("wmoUnit:degC", &[]),
        max_temperature: floats("wmoUnit:degC", &[]),
        min_temperature: floats("wmoUnit:degC", &[]),
        apparent_temperature: floats("wmoUnit:degC", &[]),
        quantitative_precipitation: floats("wmoUnit:mm", &[]),
        probability_of_precipitation: ints(Some("wmoUnit:percent"), &[]),
        snowfall_amount: floats("wmoUnit:mm", &[]),
        ice_accumulation: floats("wmoUnit:mm", &[]),
        wind_direction: ints(Some("wmoUnit:degree_(angle)"), &[]),
        wind_speed: floats("wmoUnit:km_h-1", &[]),
        elevation: None,
        wet_bulb_globe_temperature: None,
        heat_risk: None,
        relative_humidity: None,
        visibility: None,
        ceiling_height: None,
        pressure: None,
        snow_level: None,
        wind_gust: None,
    }
}

// a light wind on Tuesday, so the day has data
fn calm_tuesday() -> forecast::RequestedForecast {
    let mut forecast = empty_forecast();
    forecast.wind_speed = floats(
        "wmoUnit:km_h-1",
        &[("2026-10-06T16:00:00+00:00/PT1H", 5.0 * 1.609344)],
    );
    forecast
}

// the sentences of each forecast day, without the day names, in order
fn day_sentences(
    forecast: &forecast::RequestedForecast,
    flags: &ForecastProperties,
    days: u32,
) -> Vec<Vec<String>> {
    reply::forecast_days(forecast, flags, TZ, now(), days, reply::Format::Full)
        .into_iter()
        .map(|lines| {
            lines
                .iter()
                .flat_map(|line| {
                    assert!(line.len() <= reply::BODY_LENGTH, "line too long: {line}");
                    let (_, sentences) = line.split_once(": ").unwrap();
                    split_sentences(sentences)
                })
                .collect()
        })
        .collect()
}

// the labels that start every sentence after high/low, which is always first.
// sentences are separated by a single space, so a sentence starts wherever
// " <label> " does. Snow level is found by "Snow"
const SENTENCE_LABELS: [&str; 11] = [
    "Wet bulb globe",
    "Heat risk",
    "Humidity",
    "Precip",
    "Snow",
    "Elevation",
    "Ice",
    "Wind",
    "Visibility",
    "Pressure",
    "Cloud ceiling",
];

fn split_sentences(line: &str) -> Vec<String> {
    let mut starts: Vec<usize> = SENTENCE_LABELS
        .iter()
        .flat_map(|label| {
            let pattern = format!(" {label} ");
            line.match_indices(&pattern)
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        })
        .collect();
    starts.sort_unstable();
    starts.dedup();
    let mut sentences = Vec::new();
    let mut start = 0;
    for end in starts {
        sentences.push(line[start..end].to_string());
        start = end + 1;
    }
    sentences.push(line[start..].to_string());
    sentences
}

fn alert(event: &str, severity: Option<&str>, ends: Option<&str>) -> Alert {
    serde_json::from_value(serde_json::json!({
        "id": event,
        "geometry": null,
        "properties": { "event": event, "severity": severity, "ends": ends },
    }))
    .unwrap()
}

fn avy(level: i32) -> AvyProperties {
    AvyProperties {
        name: Some("Mt Hood".to_string()),
        center: None,
        center_link: None,
        off_season: Some(false),
        travel_advice: None,
        danger: None,
        danger_level: Some(level),
        link: None,
        timezone: None,
        start_date: None,
        end_date: None,
    }
}

fn point() -> Point {
    Point {
        properties: PointProperties {
            cwa: "PQR".to_string(),
            grid_x: 0,
            grid_y: 0,
            nwr: Nwr {
                same_code: "041005".to_string(),
            },
            astronomical_data: None,
            time_zone: Some("America/Los_Angeles".to_string()),
        },
        id: None,
        geometry: PointGeometry {
            coordinates: geo::Point::new(-121.6959, 45.3735),
        },
    }
}

// a forecast-only request. it sets no tracker flag, so it has no tracker name
// and a duration of 0, as docs/sms-format.md requires
fn request(brief: bool, forecast_length: u32) -> request::SmsRequest {
    request::SmsRequest {
        lat: 45.3735,
        lon: -121.6959,
        tracker_duration: 0,
        forecast_length,
        tracker_name: None,
        forecast_vars: ForecastVars {
            brief,
            ..ForecastVars::default()
        },
    }
}

// a request that tracks alerts and avalanche danger for 72 hours as Mt Hood,
// like the spec's examples but without their optional fields
fn tracker_request(brief: bool, forecast_length: u32) -> request::SmsRequest {
    let mut request = request(brief, forecast_length);
    request.tracker_duration = 72;
    request.tracker_name = Some("Mt Hood".to_string());
    request.forecast_vars.track_alerts = true;
    request.forecast_vars.track_avy = true;
    request
}

// the tracker that tracker_request creates, ending 72 hours after now()
fn created_tracker(daily_forecast: bool) -> forecast::TrackerStatus {
    forecast::TrackerStatus::Created(forecast::CreatedTracker {
        name: "Mt Hood".to_string(),
        alerts: true,
        avy: true,
        daily_forecast,
        ends: utc("2026-10-09T16:00:00Z"),
    })
}

fn response(forecast: Option<forecast::RequestedForecast>) -> forecast::SmsForecastResponse {
    forecast::SmsForecastResponse {
        point: point(),
        forecast,
        alerts: Vec::new(),
        alerts_notice: false,
        alerts_stale_notice: false,
        avy: None,
        avy_notice: None,
        avy_stale_notice: false,
        daily_notice: false,
        tracker: None,
    }
}

// formats a reply and checks every message is within the length limit
fn reply(
    request: &request::SmsRequest,
    response: &forecast::SmsForecastResponse,
) -> Vec<Vec<String>> {
    let responses = reply::format_reply(request, response, now());
    for message in responses.iter().flatten() {
        assert!(
            message.len() <= reply::MESSAGE_LENGTH,
            "message too long: {message}"
        );
    }
    responses
}

// the body lines of a message, without its header
fn body(message: &str) -> Vec<&str> {
    message.lines().skip(1).collect()
}

fn line(text: &str) -> reply::Part {
    reply::Part::Line(text.to_string())
}

#[test]
fn reply_order_puts_notices_first_then_avalanche_and_alerts() {
    assert_eq!(
        reply::REPLY_ORDER,
        [
            reply::ReplySection::Notices,
            reply::ReplySection::Avalanche,
            reply::ReplySection::Alerts,
            reply::ReplySection::ForecastDays,
            reply::ReplySection::Tracker,
        ]
    );
}

// every field on Tuesday, with the values in the spec's sentence table
fn every_field_tuesday() -> forecast::RequestedForecast {
    let mut forecast = empty_forecast();
    forecast.max_temperature = floats(
        "wmoUnit:degC",
        &[("2026-10-06T15:00:00+00:00/PT12H", (34.0 - 32.0) * 5.0 / 9.0)],
    );
    // starts at 8PM, the night that follows Tuesday
    forecast.min_temperature = floats(
        "wmoUnit:degC",
        &[("2026-10-07T03:00:00+00:00/PT14H", (22.0 - 32.0) * 5.0 / 9.0)],
    );
    forecast.wet_bulb_globe_temperature = Some(floats(
        "wmoUnit:degC",
        &[("2026-10-06T20:00:00+00:00/PT1H", (85.0 - 32.0) * 5.0 / 9.0)],
    ));
    // NWS sends heat risk without a unit code
    forecast.heat_risk = Some(ints(None, &[("2026-10-06T16:00:00+00:00/PT12H", 2)]));
    forecast.relative_humidity = Some(ints(
        Some("wmoUnit:percent"),
        &[
            ("2026-10-06T16:00:00+00:00/PT1H", 80),
            ("2026-10-06T22:00:00+00:00/PT1H", 30),
        ],
    ));
    forecast.probability_of_precipitation = ints(
        Some("wmoUnit:percent"),
        &[("2026-10-06T18:00:00+00:00/PT6H", 90)],
    );
    forecast.quantitative_precipitation = floats(
        "wmoUnit:mm",
        &[("2026-10-06T12:00:00+00:00/PT6H", 0.85 * 25.4)],
    );
    forecast.snowfall_amount = floats(
        "wmoUnit:mm",
        &[("2026-10-06T12:00:00+00:00/PT6H", 11.0 * 25.4)],
    );
    forecast.snow_level = Some(floats(
        "wmoUnit:m",
        &[("2026-10-06T16:00:00+00:00/PT6H", 4500.0 * 0.3048)],
    ));
    forecast.elevation = Some(Elevation {
        value: Some(5240.0 * 0.3048),
    });
    forecast.ice_accumulation = floats(
        "wmoUnit:mm",
        &[("2026-10-06T12:00:00+00:00/PT6H", 0.1 * 25.4)],
    );
    forecast.wind_speed = floats(
        "wmoUnit:km_h-1",
        &[
            ("2026-10-06T21:00:00+00:00/PT1H", 25.0 * 1.609344),
            ("2026-10-06T22:00:00+00:00/PT1H", 10.0),
        ],
    );
    forecast.wind_direction = ints(
        Some("wmoUnit:degree_(angle)"),
        &[("2026-10-06T21:00:00+00:00/PT2H", 225)],
    );
    forecast.wind_gust = Some(floats(
        "wmoUnit:km_h-1",
        &[("2026-10-06T21:00:00+00:00/PT1H", 45.0 * 1.609344)],
    ));
    forecast.visibility = Some(floats(
        "wmoUnit:m",
        &[("2026-10-06T16:00:00+00:00/PT1H", 0.5 * 1609.344)],
    ));
    forecast.ceiling_height = Some(floats(
        "wmoUnit:m",
        &[("2026-10-06T16:00:00+00:00/PT1H", 1200.0 * 0.3048)],
    ));
    // NWS sends pressure without a unit code, in pascals
    forecast.pressure = Some(FloatVec {
        uom: None,
        ..floats("", &[("2026-10-06T16:00:00+00:00/PT1H", 29.85 * 3386.389)])
    });
    forecast
}

// every optional field requested
fn every_flag() -> ForecastProperties {
    ForecastProperties {
        elevation: true,
        wet_bulb_globe_temperature: true,
        heat_risk: true,
        relative_humidity: true,
        visibility: true,
        ceiling_height: true,
        pressure: true,
        snow_level: true,
        wind_gust: true,
    }
}

// every field of a forecast day, in the order the doc's table gives
#[test]
fn day_sentences_follow_the_documented_order() {
    let forecast = every_field_tuesday();
    let flags = every_flag();
    assert_eq!(
        day_sentences(&forecast, &flags, 1),
        [[
            "high 34F, low 22F",
            "Wet bulb globe 85F",
            "Heat risk 2 of 4",
            "Humidity 30-80%",
            "Precip 90%, 0.85in",
            "Snow 11in",
            "Snow level 4500ft",
            "Elevation 5240ft",
            "Ice 0.10in",
            "Wind SW 25mph, gusts 45",
            "Visibility 0.5mi",
            "Pressure 29.85inHg",
            "Cloud ceiling 1200ft",
        ]]
    );
}

// a value in a Tuesday hour, for the converter test
const TUESDAY: &str = "2026-10-06T16:00:00+00:00/PT1H";

// one known value through each unit code Unit::converter accepts, including
// the ones the saved NWS data doesn't use. each case starts from calm_tuesday, since
// a day with only optional fields counts as past the end of the forecast
#[test]
fn every_unit_code_converts_to_the_reply_unit() {
    type Setup = fn(&mut forecast::RequestedForecast, &mut ForecastProperties);
    let cases: [(&str, Setup, &[&str]); 16] = [
        (
            "degC",
            |forecast, _| {
                forecast.max_temperature = floats("wmoUnit:degC", &[(TUESDAY, 10.0)]);
                forecast.min_temperature = floats(
                    "wmoUnit:degC",
                    &[("2026-10-07T03:00:00+00:00/PT14H", -10.0)],
                );
            },
            &["high 50F, low 14F", "Wind 5mph"],
        ),
        (
            "degF",
            |forecast, _| {
                forecast.max_temperature = floats("wmoUnit:degF", &[(TUESDAY, 50.0)]);
                forecast.min_temperature =
                    floats("wmoUnit:degF", &[("2026-10-07T03:00:00+00:00/PT14H", 14.0)]);
            },
            &["high 50F, low 14F", "Wind 5mph"],
        ),
        (
            "mm",
            |forecast, _| {
                forecast.ice_accumulation = floats("wmoUnit:mm", &[(TUESDAY, 0.1 * 25.4)]);
            },
            &["Ice 0.10in", "Wind 5mph"],
        ),
        (
            "cm",
            |forecast, _| {
                forecast.snowfall_amount = floats("wmoUnit:cm", &[(TUESDAY, 11.0 * 2.54)]);
            },
            &["Snow 11in", "Wind 5mph"],
        ),
        (
            "m to feet",
            |forecast, flags| {
                forecast.ceiling_height = Some(floats("wmoUnit:m", &[(TUESDAY, 1200.0 * 0.3048)]));
                flags.ceiling_height = true;
            },
            &["Wind 5mph", "Cloud ceiling 1200ft"],
        ),
        (
            "ft",
            |forecast, flags| {
                forecast.snow_level = Some(floats("wmoUnit:ft", &[(TUESDAY, 4500.0)]));
                flags.snow_level = true;
            },
            &["Snow level 4500ft", "Wind 5mph"],
        ),
        (
            "m to miles",
            |forecast, flags| {
                forecast.visibility = Some(floats("wmoUnit:m", &[(TUESDAY, 2.3 * 1609.344)]));
                flags.visibility = true;
            },
            &["Wind 5mph", "Visibility 2.5mi"],
        ),
        (
            "km",
            |forecast, flags| {
                forecast.visibility = Some(floats("wmoUnit:km", &[(TUESDAY, 2.3 * 1.609344)]));
                flags.visibility = true;
            },
            &["Wind 5mph", "Visibility 2.5mi"],
        ),
        (
            "km_h-1",
            |forecast, _| {
                forecast.wind_speed = floats("wmoUnit:km_h-1", &[(TUESDAY, 22.0 * 1.609344)]);
            },
            &["Wind 22mph"],
        ),
        (
            "m_s-1",
            |forecast, _| {
                forecast.wind_speed = floats("wmoUnit:m_s-1", &[(TUESDAY, 10.0)]);
            },
            &["Wind 22mph"],
        ),
        (
            "pressure without a unit code",
            |forecast, flags| {
                forecast.pressure = Some(FloatVec {
                    uom: None,
                    ..floats("", &[(TUESDAY, 29.85 * 3386.389)])
                });
                flags.pressure = true;
            },
            &["Wind 5mph", "Pressure 29.85inHg"],
        ),
        (
            "Pa",
            |forecast, flags| {
                forecast.pressure = Some(floats("wmoUnit:Pa", &[(TUESDAY, 29.85 * 3386.389)]));
                flags.pressure = true;
            },
            &["Wind 5mph", "Pressure 29.85inHg"],
        ),
        (
            "hPa",
            |forecast, flags| {
                forecast.pressure = Some(floats("wmoUnit:hPa", &[(TUESDAY, 29.85 * 33.86389)]));
                flags.pressure = true;
            },
            &["Wind 5mph", "Pressure 29.85inHg"],
        ),
        (
            "percent",
            |forecast, flags| {
                forecast.relative_humidity = Some(ints(Some("wmoUnit:percent"), &[(TUESDAY, 50)]));
                flags.relative_humidity = true;
            },
            &["Humidity 50%", "Wind 5mph"],
        ),
        (
            "degree_(angle)",
            |forecast, _| {
                forecast.wind_speed = floats("wmoUnit:km_h-1", &[(TUESDAY, 5.0 * 1.609344)]);
                forecast.wind_direction = ints(Some("wmoUnit:degree_(angle)"), &[(TUESDAY, 225)]);
            },
            &["Wind SW 5mph"],
        ),
        (
            "heat risk without a unit code",
            |forecast, flags| {
                forecast.heat_risk = Some(ints(None, &[(TUESDAY, 2)]));
                flags.heat_risk = true;
            },
            &["Heat risk 2 of 4", "Wind 5mph"],
        ),
    ];
    for (unit, setup, expected) in cases {
        let mut forecast = calm_tuesday();
        let mut flags = ForecastProperties::default();
        setup(&mut forecast, &mut flags);
        assert_eq!(
            day_sentences(&forecast, &flags, 1),
            [expected],
            "unit code: {unit}"
        );
    }
}

#[test]
fn long_day_continues_on_a_line_starting_with_the_day_name() {
    let mut forecast = calm_tuesday();
    forecast.visibility = Some(floats("wmoUnit:m", &[]));
    forecast.pressure = Some(floats("wmoUnit:Pa", &[]));
    let days = reply::forecast_days(&forecast, &every_flag(), TZ, now(), 1, reply::Format::Full);
    assert!(days[0].len() > 1, "expected the day to need two lines");
    for line in &days[0] {
        assert!(line.starts_with("Tue: "));
        assert!(line.len() <= reply::BODY_LENGTH);
    }
}

#[test]
fn precip_chance_is_shown_at_zero_and_zero_amounts_are_left_out() {
    let mut forecast = calm_tuesday();
    forecast.probability_of_precipitation = ints(
        Some("wmoUnit:percent"),
        &[("2026-10-06T07:00:00+00:00/P1D", 0)],
    );
    forecast.quantitative_precipitation =
        floats("wmoUnit:mm", &[("2026-10-06T12:00:00+00:00/PT6H", 0.0)]);
    forecast.snowfall_amount = floats("wmoUnit:mm", &[("2026-10-06T12:00:00+00:00/PT6H", 0.0)]);
    forecast.ice_accumulation = floats("wmoUnit:mm", &[("2026-10-06T12:00:00+00:00/PT6H", 0.0)]);
    assert_eq!(
        day_sentences(&forecast, &ForecastProperties::default(), 1),
        [["Precip 0%", "Wind 5mph"]]
    );
}

#[test]
fn visibility_has_one_decimal_under_a_mile_and_halves_above() {
    let flags = ForecastProperties {
        visibility: true,
        ..ForecastProperties::default()
    };
    for (miles, expected) in [
        (0.5, "Visibility 0.5mi"),
        (1.0, "Visibility 1mi"),
        (2.3, "Visibility 2.5mi"),
        (12.0, "Visibility 10mi"),
    ] {
        let mut forecast = calm_tuesday();
        forecast.visibility = Some(floats(
            "wmoUnit:m",
            &[("2026-10-06T16:00:00+00:00/PT1H", miles * 1609.344)],
        ));
        assert_eq!(day_sentences(&forecast, &flags, 1)[0][1], expected);
    }
}

#[test]
fn snow_has_tenths_under_an_inch_halves_under_four_and_whole_inches_above() {
    for (inches, expected) in [
        (0.04, &["Wind 5mph"][..]),
        (0.2, &["Snow 0.2in", "Wind 5mph"]),
        (0.96, &["Snow 1in", "Wind 5mph"]),
        (1.3, &["Snow 1.5in", "Wind 5mph"]),
        (2.0, &["Snow 2in", "Wind 5mph"]),
        (3.74, &["Snow 3.5in", "Wind 5mph"]),
        (3.8, &["Snow 4in", "Wind 5mph"]),
        (4.4, &["Snow 4in", "Wind 5mph"]),
        (4.6, &["Snow 5in", "Wind 5mph"]),
    ] {
        let mut forecast = calm_tuesday();
        forecast.snowfall_amount = floats("wmoUnit:mm", &[(TUESDAY, inches * 25.4)]);
        assert_eq!(
            day_sentences(&forecast, &ForecastProperties::default(), 1),
            [expected],
            "{inches} inches"
        );
    }
    // the brief format rounds the same way
    let mut forecast = calm_tuesday();
    forecast.snowfall_amount = floats("wmoUnit:mm", &[(TUESDAY, 2.5 * 25.4)]);
    let days = reply::forecast_days(
        &forecast,
        &ForecastProperties::default(),
        TZ,
        now(),
        1,
        reply::Format::Brief,
    );
    assert_eq!(days, [["Tu --/-- S2.5 5"]]);
}

#[test]
fn requested_fields_without_a_value_say_n_a() {
    let mut forecast = calm_tuesday();
    // NWS gives no value for a cloud ceiling when there is none
    forecast.ceiling_height = Some(FloatVec {
        uom: Some("wmoUnit:m".to_string()),
        values: vec![TimeFloat {
            valid_time: Some("2026-10-06T16:00:00+00:00/P1D".to_string()),
            value: None,
        }],
    });
    forecast.pressure = Some(FloatVec {
        uom: None,
        values: Vec::new(),
    });
    let flags = ForecastProperties {
        ceiling_height: true,
        pressure: true,
        ..ForecastProperties::default()
    };
    assert_eq!(
        day_sentences(&forecast, &flags, 1),
        [["Wind 5mph", "Pressure N/A", "Cloud ceiling N/A"]]
    );
}

// a base value with no data is N/A when its sentence has another value to show
#[test]
fn a_missing_base_value_is_n_a_when_its_sentence_has_other_data() {
    let gusts = ForecastProperties {
        wind_gust: true,
        ..ForecastProperties::default()
    };
    let no_precip = || {
        ints(
            Some("wmoUnit:percent"),
            &[("2026-10-06T07:00:00+00:00/P1D", 0)],
        )
    };

    // a gust without a wind speed
    let mut forecast = empty_forecast();
    forecast.probability_of_precipitation = no_precip();
    forecast.wind_gust = Some(floats("wmoUnit:km_h-1", &[(TUESDAY, 45.0 * 1.609344)]));
    assert_eq!(
        day_sentences(&forecast, &gusts, 1),
        [["Precip 0%", "Wind N/A, gusts 45"]]
    );

    // gusts requested, with neither a wind speed nor a gust
    let mut forecast = empty_forecast();
    forecast.probability_of_precipitation = no_precip();
    assert_eq!(
        day_sentences(&forecast, &gusts, 1),
        [["Precip 0%", "Wind N/A, gusts N/A"]]
    );
    let days = reply::forecast_days(&forecast, &gusts, TZ, now(), 1, reply::Format::Brief);
    assert_eq!(days, [["Tu --/-- P0 --G--"]]);

    // a precipitation amount without a chance
    let mut forecast = empty_forecast();
    forecast.quantitative_precipitation = floats("wmoUnit:mm", &[(TUESDAY, 0.2 * 25.4)]);
    assert_eq!(
        day_sentences(&forecast, &ForecastProperties::default(), 1),
        [["Precip N/A, 0.20in"]]
    );
    let days = reply::forecast_days(
        &forecast,
        &ForecastProperties::default(),
        TZ,
        now(),
        1,
        reply::Format::Brief,
    );
    assert_eq!(days, [["Tu --/-- P-- R0.20 --"]]);
}

#[test]
fn series_with_a_unit_that_cant_be_converted_is_left_out() {
    let mut forecast = calm_tuesday();
    forecast.max_temperature = floats("wmoUnit:K", &[("2026-10-06T15:00:00+00:00/PT12H", 280.0)]);
    assert_eq!(
        day_sentences(&forecast, &ForecastProperties::default(), 1),
        [["Wind 5mph"]]
    );
}

#[test]
fn elevation_is_only_in_the_first_day() {
    let mut forecast = calm_tuesday();
    forecast.wind_speed = floats(
        "wmoUnit:km_h-1",
        &[
            ("2026-10-06T16:00:00+00:00/PT1H", 5.0 * 1.609344),
            ("2026-10-07T16:00:00+00:00/PT1H", 5.0 * 1.609344),
        ],
    );
    forecast.elevation = Some(Elevation {
        value: Some(5240.0 * 0.3048),
    });
    let flags = ForecastProperties {
        elevation: true,
        ..ForecastProperties::default()
    };
    assert_eq!(
        day_sentences(&forecast, &flags, 2),
        [vec!["Elevation 5240ft", "Wind 5mph"], vec!["Wind 5mph"]]
    );
}

#[test]
fn totals_are_split_across_local_midnight() {
    let mut forecast = empty_forecast();
    forecast.probability_of_precipitation = ints(
        Some("wmoUnit:percent"),
        &[("2026-10-06T07:00:00+00:00/P2D", 50)],
    );
    // 9PM Tuesday to 3AM Wednesday local time, half on each day
    forecast.quantitative_precipitation = floats(
        "wmoUnit:mm",
        &[("2026-10-07T04:00:00+00:00/PT6H", 0.4 * 25.4)],
    );
    assert_eq!(
        day_sentences(&forecast, &ForecastProperties::default(), 2),
        [["Precip 50%, 0.20in"], ["Precip 50%, 0.20in"]]
    );
}

#[test]
fn null_amounts_are_skipped_in_totals() {
    let mut forecast = empty_forecast();
    forecast.probability_of_precipitation = ints(
        Some("wmoUnit:percent"),
        &[("2026-10-06T07:00:00+00:00/P2D", 50)],
    );
    let amount = |time: &str, value: Option<f32>| TimeFloat {
        valid_time: Some(time.to_string()),
        value,
    };
    // Tuesday has one amount and one null, Wednesday only a null
    forecast.quantitative_precipitation = FloatVec {
        uom: Some("wmoUnit:mm".to_string()),
        values: vec![
            amount("2026-10-06T12:00:00+00:00/PT6H", Some(0.2 * 25.4)),
            amount("2026-10-06T18:00:00+00:00/PT6H", None),
            amount("2026-10-07T12:00:00+00:00/PT6H", None),
        ],
    };
    assert_eq!(
        day_sentences(&forecast, &ForecastProperties::default(), 2),
        [["Precip 50%, 0.20in"], ["Precip 50%"]]
    );
}

#[test]
fn wind_direction_is_taken_at_the_highest_wind() {
    let mut forecast = empty_forecast();
    forecast.wind_speed = floats(
        "wmoUnit:km_h-1",
        &[
            ("2026-10-06T16:00:00+00:00/PT1H", 10.0),
            ("2026-10-06T20:00:00+00:00/PT1H", 20.0 * 1.609344),
        ],
    );
    forecast.wind_direction = ints(
        Some("wmoUnit:degree_(angle)"),
        &[
            ("2026-10-06T16:00:00+00:00/PT4H", 0),
            ("2026-10-06T20:00:00+00:00/PT4H", 270),
        ],
    );
    assert_eq!(
        day_sentences(&forecast, &ForecastProperties::default(), 1),
        [["Wind W 20mph"]]
    );
}

#[test]
fn brief_day_follows_the_same_order() {
    let mut forecast = empty_forecast();
    forecast.max_temperature = floats(
        "wmoUnit:degC",
        &[("2026-10-06T15:00:00+00:00/PT12H", (34.0 - 32.0) * 5.0 / 9.0)],
    );
    forecast.min_temperature = floats(
        "wmoUnit:degC",
        &[("2026-10-07T03:00:00+00:00/PT14H", (22.0 - 32.0) * 5.0 / 9.0)],
    );
    forecast.probability_of_precipitation = ints(
        Some("wmoUnit:percent"),
        &[("2026-10-06T18:00:00+00:00/PT6H", 90)],
    );
    forecast.quantitative_precipitation = floats(
        "wmoUnit:mm",
        &[("2026-10-06T12:00:00+00:00/PT6H", 0.85 * 25.4)],
    );
    forecast.snowfall_amount = floats(
        "wmoUnit:mm",
        &[("2026-10-06T12:00:00+00:00/PT6H", 11.0 * 25.4)],
    );
    forecast.wind_speed = floats(
        "wmoUnit:km_h-1",
        &[("2026-10-06T21:00:00+00:00/PT1H", 25.0 * 1.609344)],
    );
    forecast.wind_direction = ints(
        Some("wmoUnit:degree_(angle)"),
        &[("2026-10-06T21:00:00+00:00/PT1H", 225)],
    );
    let days = reply::forecast_days(
        &forecast,
        &ForecastProperties::default(),
        TZ,
        now(),
        1,
        reply::Format::Brief,
    );
    assert_eq!(days, [["Tu 34/22 P90 R0.85 S11 SW25"]]);
}

#[test]
fn brief_day_has_p0_and_dashes_for_missing_temperatures() {
    let mut forecast = calm_tuesday();
    forecast.probability_of_precipitation = ints(
        Some("wmoUnit:percent"),
        &[("2026-10-06T07:00:00+00:00/P1D", 0)],
    );
    let days = reply::forecast_days(
        &forecast,
        &ForecastProperties::default(),
        TZ,
        now(),
        1,
        reply::Format::Brief,
    );
    assert_eq!(days, [["Tu --/-- P0 5"]]);
}

#[test]
fn brief_day_has_a_token_for_every_requested_field() {
    let days = reply::forecast_days(
        &every_field_tuesday(),
        &every_flag(),
        TZ,
        now(),
        1,
        reply::Format::Brief,
    );
    assert_eq!(
        days,
        [
            [
                "Tu 34/22 WB85 HR2 H30-80 P90 R0.85 S11 SL4500 EL5240 I0.10 SW25G45 V0.5 PR29.85 C1200"
            ]
        ]
    );
}

#[test]
fn brief_requested_fields_without_a_value_are_dashes() {
    let days = reply::forecast_days(
        &calm_tuesday(),
        &every_flag(),
        TZ,
        now(),
        1,
        reply::Format::Brief,
    );
    assert_eq!(
        days,
        [["Tu --/-- WB-- HR-- H-- SL-- EL-- 5G-- V-- PR-- C--"]]
    );
}

#[test]
fn brief_gusts_are_shown_without_wind_data() {
    let mut forecast = empty_forecast();
    forecast.probability_of_precipitation = ints(
        Some("wmoUnit:percent"),
        &[("2026-10-06T07:00:00+00:00/P1D", 0)],
    );
    forecast.wind_gust = Some(floats("wmoUnit:km_h-1", &[(TUESDAY, 45.0 * 1.609344)]));
    let flags = ForecastProperties {
        wind_gust: true,
        ..ForecastProperties::default()
    };
    let days = reply::forecast_days(&forecast, &flags, TZ, now(), 1, reply::Format::Brief);
    assert_eq!(days, [["Tu --/-- P0 --G45"]]);
}

#[test]
fn brief_elevation_is_only_in_the_first_day() {
    let mut forecast = calm_tuesday();
    forecast.wind_speed = floats(
        "wmoUnit:km_h-1",
        &[
            ("2026-10-06T16:00:00+00:00/PT1H", 5.0 * 1.609344),
            ("2026-10-07T16:00:00+00:00/PT1H", 5.0 * 1.609344),
        ],
    );
    forecast.elevation = Some(Elevation {
        value: Some(5240.0 * 0.3048),
    });
    let flags = ForecastProperties {
        elevation: true,
        ..ForecastProperties::default()
    };
    let days = reply::forecast_days(&forecast, &flags, TZ, now(), 2, reply::Format::Brief);
    assert_eq!(days, [["Tu --/-- EL5240 5"], ["We --/-- 5"]]);
}

#[test]
fn alerts_are_sorted_by_severity_with_unknown_after_severe() {
    let alerts = [
        alert("Small Craft Advisory", Some("Minor"), None),
        alert("First Moderate", Some("Moderate"), None),
        alert("No Severity", None, None),
        alert("Second Moderate", Some("Moderate"), None),
        alert("Flood Warning", Some("Severe"), None),
        alert("Tornado Warning", Some("Extreme"), None),
    ];
    assert_eq!(
        reply::alert_lines(&alerts, false, false, TZ, reply::Format::Full),
        [
            "!Extreme: Tornado Warning",
            "!Severe: Flood Warning",
            "!Unknown: No Severity",
            "!Moderate: First Moderate",
            "!Moderate: Second Moderate",
            "!Minor: Small Craft Advisory",
        ]
    );
}

#[test]
fn alert_line_ends_with_the_local_end_time() {
    let warning = alert(
        "Winter Storm Warning",
        Some("Severe"),
        Some("2026-10-08T01:00:00+00:00"),
    );
    assert_eq!(
        reply::alert_line(&warning, TZ, reply::Format::Full),
        "!Severe: Winter Storm Warning until Wed 10/7 6PM"
    );
    assert_eq!(
        reply::alert_line(&warning, TZ, reply::Format::Brief),
        "!Sev Winter Storm Warning til We7 6PM"
    );
    let advisory = alert(
        "Wind Advisory",
        Some("Moderate"),
        Some("2026-10-07T17:30:00+00:00"),
    );
    assert_eq!(
        reply::alert_line(&advisory, TZ, reply::Format::Full),
        "!Moderate: Wind Advisory until Wed 10/7 10:30AM"
    );
}

#[test]
fn stale_alerts_notice_comes_before_the_alerts() {
    let alerts = [alert("Flood Warning", Some("Severe"), None)];
    assert_eq!(
        reply::alert_lines(&alerts, false, true, TZ, reply::Format::Full),
        ["!Alerts may be out of date", "!Severe: Flood Warning"]
    );
    assert_eq!(
        reply::alert_lines(&[], true, false, TZ, reply::Format::Brief),
        ["!alerts not checked"]
    );
}

#[test]
fn avalanche_lines_in_both_formats() {
    let full = reply::Format::Full;
    let brief = reply::Format::Brief;
    assert_eq!(
        reply::avalanche_lines(Some(&avy(3)), None, false, full),
        ["Avy danger 3 Considerable, Mt Hood"]
    );
    assert_eq!(
        reply::avalanche_lines(Some(&avy(3)), None, false, brief),
        ["AV3 Considerable"]
    );
    assert_eq!(
        reply::avalanche_lines(Some(&avy(-1)), None, true, full),
        [
            "Avy: no danger rating, Mt Hood",
            "Avy: data may be out of date"
        ]
    );
    assert_eq!(
        reply::avalanche_lines(Some(&avy(-1)), None, false, brief),
        ["AV no rating"]
    );
    assert_eq!(
        reply::avalanche_lines(None, Some(forecast::AvyNotice::OffSeason), false, full),
        ["Avy: center is in its off season"]
    );
    assert_eq!(
        reply::avalanche_lines(None, Some(forecast::AvyNotice::Unavailable), true, brief),
        ["AV unavailable", "AV may be old"]
    );
    assert!(reply::avalanche_lines(None, None, false, full).is_empty());
}

// the request also sets daily_forecast, but NWS reports no time zone for the
// point, so the tracker is created without a daily forecast and the reply says
// so. without a time zone, times are in UTC
#[test]
fn full_reply_lines_are_in_reply_order() {
    let mut request = tracker_request(false, 1);
    request.forecast_vars.daily_forecast = true;
    let mut response = response(Some(calm_tuesday()));
    response.point.properties.time_zone = None;
    response.daily_notice = true;
    response.avy = Some(avy(3));
    response.alerts = vec![alert("Flood Warning", Some("Severe"), None)];
    response.alerts_stale_notice = true;
    response.tracker = Some(created_tracker(false));
    let responses = reply(&request, &response);
    assert_eq!(responses.len(), 1);
    // the request names the tracker, but a reply is always headed by the brand
    let count = responses[0].len();
    for (index, message) in responses[0].iter().enumerate() {
        let header = format!("({}/{count}) {}\n", index + 1, reply::BRAND);
        assert!(message.starts_with(&header), "{message}");
    }
    let lines: Vec<&str> = responses[0]
        .iter()
        .flat_map(|message| body(message))
        .collect();
    assert_eq!(
        lines,
        [
            "Daily forecast unavailable",
            "Avy danger 3 Considerable, Mt Hood",
            "!Alerts may be out of date",
            "!Severe: Flood Warning",
            "Tue: Wind 5mph",
            "Tracking Mt Hood: alerts, avy until Fri 10/9 4PM",
        ]
    );
}

#[test]
fn reply_with_a_failed_tracker_has_the_notice_at_the_top() {
    let mut response = response(Some(calm_tuesday()));
    response.avy = Some(avy(3));
    response.tracker = Some(forecast::TrackerStatus::NotCreated);
    let full = reply(&tracker_request(false, 1), &response);
    assert_eq!(
        body(&full[0][0]),
        [
            "Could not create tracker",
            "Avy danger 3 Considerable, Mt Hood",
            "Tue: Wind 5mph"
        ]
    );
    let brief = reply(&tracker_request(true, 1), &response);
    assert_eq!(
        brief,
        [["(1/1) RidgeCast\n!no tracker\nAV3 Considerable\nTu --/-- 5"]]
    );
}

#[test]
fn a_day_that_doesnt_fit_starts_a_new_message() {
    let day = reply::Part::Day(vec!["x".repeat(60), "y".repeat(40)]);
    let bodies = reply::pack_messages(&[line(&"a".repeat(50)), day]);
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0], ["a".repeat(50)]);
    assert_eq!(bodies[1], ["x".repeat(60), "y".repeat(40)]);
}

#[test]
fn days_are_dropped_from_the_last_to_stay_within_nine_messages() {
    let mut parts: Vec<reply::Part> = (0..12)
        .map(|day| reply::Part::Day(vec![format!("day {day} {}", "x".repeat(100))]))
        .collect();
    parts.push(line("Tracking Mt Hood: alerts until Fri 10/9 10AM"));
    let bodies = reply::pack_within(parts, reply::MAX_MESSAGES);
    assert_eq!(bodies.len(), reply::MAX_MESSAGES);
    assert!(bodies[0][0].starts_with("day 0 "));
    // the tracker line is never dropped
    assert_eq!(
        bodies.last().unwrap().last().unwrap(),
        "Tracking Mt Hood: alerts until Fri 10/9 10AM"
    );
}

#[test]
fn alerts_beyond_nine_messages_continue_as_another_response() {
    // one alert per message, so 12 alerts need 12 messages
    let parts: Vec<reply::Part> = (0..12)
        .map(|alert| {
            line(&format!(
                "!Severe: Flood Warning {alert} {}",
                "x".repeat(60)
            ))
        })
        .collect();
    let responses = reply::add_headers(
        reply::pack_within(parts, reply::MAX_MESSAGES),
        Some("Mt Hood"),
    );
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].len(), reply::MAX_MESSAGES);
    assert!(responses[0][0].starts_with("(1/9) Mt Hood\n"));
    assert!(responses[1][0].starts_with(&format!("(1/{}) Mt Hood\n", responses[1].len())));
    let sent = responses
        .iter()
        .flatten()
        .flat_map(|message| body(message))
        .count();
    assert_eq!(sent, 12);
}

#[test]
fn brief_reply_drops_days_to_stay_within_brief_max_messages() {
    // a light wind on each of the 7 days, with every optional field requested,
    // so each day says -- for them and at most 2 days fit in a message
    let times: Vec<String> = (6..13)
        .map(|day| format!("2026-10-{day:02}T16:00:00+00:00/PT1H"))
        .collect();
    let winds: Vec<(&str, f32)> = times
        .iter()
        .map(|time| (time.as_str(), 5.0 * 1.609344))
        .collect();
    let mut forecast = empty_forecast();
    forecast.wind_speed = floats("wmoUnit:km_h-1", &winds);
    let mut request = request(true, 7);
    request.forecast_vars.forecast_properties = every_flag();
    let mut response = response(Some(forecast));
    response.alerts = (0..2)
        .map(|n| alert(&format!("Winter Storm Warning {n}"), Some("Severe"), None))
        .collect();
    let responses = reply(&request, &response);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].len(), reply::BRIEF_MAX_MESSAGES);
    let lines: Vec<&str> = responses[0]
        .iter()
        .flat_map(|message| body(message))
        .collect();
    assert_eq!(lines[0], "!Sev Winter Storm Warning 0");
    assert_eq!(lines[1], "!Sev Winter Storm Warning 1");
    // the days that fit, starting with today, and the last ones dropped
    let days: Vec<&str> = lines[2..].iter().map(|line| &line[..2]).collect();
    assert_eq!(days, ["Tu", "We", "Th", "Fr", "Sa"]);
}

#[test]
fn brief_days_never_add_a_message_after_alerts_reach_the_cap() {
    // 3 alert lines of 38 characters fill a message, so 9 alerts take 3
    // messages and leave no room for the day
    let mut response = response(Some(calm_tuesday()));
    response.alerts = (0..9)
        .map(|n| {
            alert(
                &format!("Winter Storm Warning for zone {n:03}"),
                Some("Severe"),
                None,
            )
        })
        .collect();
    let responses = reply(&request(true, 1), &response);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].len(), reply::BRIEF_MAX_MESSAGES);
    let lines: Vec<&str> = responses[0]
        .iter()
        .flat_map(|message| body(message))
        .collect();
    assert_eq!(lines.len(), 9);
    assert!(lines.iter().all(|line| line.starts_with("!Sev")));
}

// the brief example in the spec: the request sets relative humidity and wind
// gust, and their tokens leave no room for Wednesday in the first message
#[test]
fn brief_reply_with_optional_fields_continues_days_in_another_message() {
    let mut forecast = empty_forecast();
    forecast.max_temperature = floats(
        "wmoUnit:degC",
        &[
            ("2026-10-06T15:00:00+00:00/PT12H", (34.0 - 32.0) * 5.0 / 9.0),
            ("2026-10-07T15:00:00+00:00/PT12H", (30.0 - 32.0) * 5.0 / 9.0),
        ],
    );
    forecast.min_temperature = floats(
        "wmoUnit:degC",
        &[
            ("2026-10-07T03:00:00+00:00/PT14H", (22.0 - 32.0) * 5.0 / 9.0),
            ("2026-10-08T03:00:00+00:00/PT14H", (19.0 - 32.0) * 5.0 / 9.0),
        ],
    );
    forecast.relative_humidity = Some(ints(
        Some("wmoUnit:percent"),
        &[
            ("2026-10-06T16:00:00+00:00/PT1H", 80),
            ("2026-10-06T22:00:00+00:00/PT1H", 100),
            ("2026-10-07T16:00:00+00:00/PT1H", 75),
            ("2026-10-07T22:00:00+00:00/PT1H", 95),
        ],
    ));
    forecast.probability_of_precipitation = ints(
        Some("wmoUnit:percent"),
        &[
            ("2026-10-06T18:00:00+00:00/PT6H", 90),
            ("2026-10-07T18:00:00+00:00/PT6H", 70),
        ],
    );
    forecast.quantitative_precipitation = floats(
        "wmoUnit:mm",
        &[
            ("2026-10-06T12:00:00+00:00/PT6H", 0.85 * 25.4),
            ("2026-10-07T12:00:00+00:00/PT6H", 0.30 * 25.4),
        ],
    );
    forecast.snowfall_amount = floats(
        "wmoUnit:mm",
        &[
            ("2026-10-06T12:00:00+00:00/PT6H", 11.0 * 25.4),
            ("2026-10-07T12:00:00+00:00/PT6H", 5.0 * 25.4),
        ],
    );
    forecast.wind_speed = floats(
        "wmoUnit:km_h-1",
        &[
            ("2026-10-06T21:00:00+00:00/PT1H", 25.0 * 1.609344),
            ("2026-10-07T21:00:00+00:00/PT1H", 18.0 * 1.609344),
        ],
    );
    forecast.wind_direction = ints(
        Some("wmoUnit:degree_(angle)"),
        &[
            ("2026-10-06T21:00:00+00:00/PT1H", 225),
            ("2026-10-07T21:00:00+00:00/PT1H", 270),
        ],
    );
    forecast.wind_gust = Some(floats(
        "wmoUnit:km_h-1",
        &[
            ("2026-10-06T21:00:00+00:00/PT1H", 45.0 * 1.609344),
            ("2026-10-07T21:00:00+00:00/PT1H", 35.0 * 1.609344),
        ],
    ));
    let flags = ForecastProperties {
        relative_humidity: true,
        wind_gust: true,
        ..ForecastProperties::default()
    };
    assert_eq!(
        reply::forecast_days(&forecast, &flags, TZ, now(), 2, reply::Format::Brief),
        [
            ["Tu 34/22 H80-100 P90 R0.85 S11 SW25G45"],
            ["We 30/19 H75-95 P70 R0.30 S5 W18G35"],
        ]
    );

    let mut request = tracker_request(true, 2);
    request.forecast_vars.forecast_properties = flags;
    let mut response = response(Some(forecast));
    // the brief format has no tracker line
    response.tracker = Some(created_tracker(false));
    response.avy = Some(avy(3));
    response.alerts = vec![alert(
        "Winter Storm Warning",
        Some("Severe"),
        Some("2026-10-08T01:00:00+00:00"),
    )];
    assert_eq!(
        reply(&request, &response),
        [[
            "(1/2) RidgeCast\nAV3 Considerable\n!Sev Winter Storm Warning til We7 6PM\n\
             Tu 34/22 H80-100 P90 R0.85 S11 SW25G45",
            "(2/2) RidgeCast\nWe 30/19 H75-95 P70 R0.30 S5 W18G35",
        ]]
    );
}

// brief alert lines of 34 characters, 3 to a message
fn brief_alerts(count: usize) -> Vec<Alert> {
    (0..count)
        .map(|n| {
            alert(
                &format!("Winter Storm Warning number {n}"),
                Some("Severe"),
                None,
            )
        })
        .collect()
}

// 10 alerts take 4 messages, past BRIEF_MAX_MESSAGES, since alerts are never
// dropped. the day fills the space left in the last one
#[test]
fn brief_reply_continues_alerts_past_the_cap_and_fills_the_last_message_with_days() {
    let mut response = response(Some(calm_tuesday()));
    response.alerts = brief_alerts(10);
    let responses = reply(&request(true, 1), &response);
    assert_eq!(responses.len(), 1);
    let messages = &responses[0];
    assert_eq!(messages.len(), 4);
    assert!(messages[3].starts_with("(4/4) RidgeCast\n"));
    let alerts_sent = messages
        .iter()
        .flat_map(|message| body(message))
        .filter(|line| line.starts_with("!Sev"))
        .count();
    assert_eq!(alerts_sent, 10);
    assert_eq!(
        body(&messages[3]),
        ["!Sev Winter Storm Warning number 9", "Tu --/-- 5"]
    );
}

// 9 alerts take exactly BRIEF_MAX_MESSAGES. the last has room for the day,
// which goes there rather than being dropped
#[test]
fn brief_days_fill_the_last_message_when_alerts_reach_the_cap() {
    let mut response = response(Some(calm_tuesday()));
    response.alerts = brief_alerts(9);
    let responses = reply(&request(true, 1), &response);
    assert_eq!(responses.len(), 1);
    let messages = &responses[0];
    assert_eq!(messages.len(), reply::BRIEF_MAX_MESSAGES);
    assert_eq!(
        body(&messages[2]),
        [
            "!Sev Winter Storm Warning number 6",
            "!Sev Winter Storm Warning number 7",
            "!Sev Winter Storm Warning number 8",
            "Tu --/-- 5",
        ]
    );
}

// out-of-date alerts can miss a new alert, so the notice is sent in both
// formats, first among the alert lines, and even when there are no alerts
#[test]
fn alerts_stale_notice_is_sent_in_every_reply() {
    for (brief, notice, alert_line, day) in [
        (
            false,
            "!Alerts may be out of date",
            "!Severe: Flood Warning",
            "Tue: Wind 5mph",
        ),
        (
            true,
            "!alerts may be old",
            "!Sev Flood Warning",
            "Tu --/-- 5",
        ),
    ] {
        let mut response = response(Some(calm_tuesday()));
        response.alerts_stale_notice = true;
        assert_eq!(
            reply(&request(brief, 1), &response),
            [[format!("(1/1) RidgeCast\n{notice}\n{day}")]],
            "brief: {brief}, no alerts"
        );

        response.avy = Some(avy(3));
        response.alerts = vec![alert("Flood Warning", Some("Severe"), None)];
        let responses = reply(&request(brief, 1), &response);
        let lines = body(&responses[0][0]);
        assert_eq!(&lines[1..3], [notice, alert_line], "brief: {brief}");
    }
}

#[test]
fn parse_errors_are_one_line() {
    assert_eq!(
        reply::format_parse_error(&request::SmsParseError::InvalidLatitude("95.1".to_string())),
        "(1/1) RidgeCast\nBad latitude: 95.1"
    );
    assert_eq!(
        reply::format_parse_error(&request::SmsParseError::WrongFieldCount(5)),
        "(1/1) RidgeCast\nExpected 6 fields, found 5"
    );
    assert_eq!(
        reply::format_parse_error(&request::SmsParseError::UnsupportedVersion("2".to_string())),
        "(1/1) RidgeCast\nUpdate the RidgeCast app"
    );
}

#[test]
fn a_long_value_in_a_parse_error_is_cut_to_fit() {
    let message = reply::format_parse_error(&request::SmsParseError::InvalidFlags("F".repeat(500)));
    assert!(message.len() <= reply::MESSAGE_LENGTH);
    assert!(message.starts_with("(1/1) RidgeCast\nBad flags: FFF"));
}

#[test]
fn a_message_that_isnt_a_request_gets_the_help_message() {
    let message = reply::format_parse_error(&request::SmsParseError::NotARequest);
    assert_eq!(
        message,
        "(1/1) RidgeCast\nHelp: support@ridgecast.app. Reply STOP to opt out."
    );
    assert!(message.len() <= reply::MESSAGE_LENGTH);
}

#[test]
fn api_errors_are_one_line() {
    assert_eq!(
        reply::format_api_error(&errors::APIError::LocationNotCovered),
        "(1/1) RidgeCast\nLocation not covered by NWS forecasts"
    );
    assert_eq!(
        reply::format_api_error(&errors::APIError::UpstreamTimeout),
        "(1/1) RidgeCast\nWeather service unavailable, try later"
    );
    assert_eq!(
        reply::format_api_error(&errors::APIError::DatabaseError),
        "(1/1) RidgeCast\nServer error, try later"
    );
}
