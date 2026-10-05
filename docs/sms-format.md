# TrailAlert SMS request format

This document defines the text message that the TrailAlert iOS and Android apps
send to the TrailAlert server to request a forecast, alert tracking, or an
avalanche forecast. The apps and the server must both follow it exactly.

The format is kept short so that every request fits in a single SMS segment,
including when it is sent over satellite messaging.

Current version: **1**

## Message layout

A request is one line made up of four fields separated by single spaces:

```
TA1 44.0512 -123.0921 845
│   │       │         └─ flags      which options are turned on (3 hex digits)
│   │       └─────────── longitude  decimal degrees
│   └─────────────────── latitude   decimal degrees
└─────────────────────── prefix     "TA" followed by the format version
```

| Field | Format | Example |
|---|---|---|
| Prefix | `TA` followed by the format version as a decimal integer | `TA1` |
| Latitude | Decimal degrees from `-90` to `90`, at most 4 decimal places | `44.0512` |
| Longitude | Decimal degrees from `-180` to `180`, at most 4 decimal places | `-123.0921` |
| Flags | Exactly 3 hexadecimal digits (`0`–`9`, `A`–`F`) | `845` |

The longest possible message, `TA1 -90.0000 -180.0000 FFF`, is 26 characters.

The sender's phone number identifies the user, so the message body does not
include an account or user ID.

### Coordinates

- Use `.` as the decimal separator and no thousands separator. Format the
  numbers with a fixed locale (`Locale.US` on Android, `en_US_POSIX` on iOS).
  Many device locales use `,` as the decimal separator, which the server
  rejects.
- Use a leading `-` for south latitudes and west longitudes. Do not use `+`,
  exponent notation, or compass letters (`N`, `S`, `E`, `W`).
- Round to at most 4 decimal places, which is about 11 m. The NWS points API
  does not accept more precision than this.
- NWS forecasts and alerts only cover the United States and its territories.
  The server replies with an error for a location outside that coverage.

### Flags

The flags field packs 12 on/off options into a 12-bit number written as 3
hexadecimal digits. A bit set to `1` means the option is on.

Each bit corresponds to a field of the `ForecastVars` struct in
`src/models.rs`. Bit 0 is the least significant bit.

| Bit | Value (hex) | `ForecastVars` field | Meaning when set |
|---|---|---|---|
| 0 | `001` | `track_alerts` | Track this location and send new NWS alerts for it |
| 1 | `002` | `no_forecast` | Do not include a weather forecast in the reply |
| 2 | `004` | `track_avy` | Create an avalanche danger tracker for this location |
| 3 | `008` | `forecast_properties.elevation` | Include elevation |
| 4 | `010` | `forecast_properties.wet_bulb_globe_temperature` | Include wet bulb globe temperature |
| 5 | `020` | `forecast_properties.heat_risk` | Include heat risk |
| 6 | `040` | `forecast_properties.relative_humidity` | Include relative humidity |
| 7 | `080` | `forecast_properties.visibility` | Include visibility |
| 8 | `100` | `forecast_properties.ceiling_height` | Include ceiling height |
| 9 | `200` | `forecast_properties.pressure` | Include pressure |
| 10 | `400` | `forecast_properties.snow_level` | Include snow level |
| 11 | `800` | `forecast_properties.wind_gust` | Include wind gust |

To build the flags field, add (bitwise OR) the values of every option that is
on, then write the result as exactly 3 hex digits, padding with leading zeros.
Apps should send uppercase hex digits. The server also accepts lowercase.

```kotlin
// Kotlin
val flags = String.format(Locale.US, "%03X", bits)
```

```swift
// Swift
let flags = String(format: "%03X", bits)
```

## Examples

| Request | Flags | Message |
|---|---|---|
| Forecast only, no optional fields | `000` | `TA1 44.0512 -123.0921 000` |
| Track alerts, no forecast | `001` + `002` = `003` | `TA1 44.0512 -123.0921 003` |
| Track alerts, track avalanche danger, relative humidity, wind gust | `001` + `004` + `040` + `800` = `845` | `TA1 44.0512 -123.0921 845` |
| Track alerts, visibility, snow level, wind gust | `001` + `080` + `400` + `800` = `C81` | `TA1 39.1968 -120.2354 C81` |
| Every option on | `FFF` | `TA1 44.0512 -123.0921 FFF` |

## Character set

Use only the characters `T`, `A`, the digits `0`–`9`, `A`–`F`, `-`, `.`, and
space. These are all in the GSM-7 character set, which allows 160 characters
per SMS segment. A single character outside GSM-7, such as an emoji or a smart
quote added by autocorrect, switches the whole message to UCS-2, which only
allows 70 characters per segment.

## How the server reads a message

- Leading and trailing whitespace is ignored, and fields may be separated by
  more than one space. Apps should still send exactly one space between fields.
- The prefix is matched without regard to case, so `ta1` is accepted.
- A message that does not start with `TA` is not treated as a request. The
  server replies with a short help message.
- A message with an unsupported version number gets a reply asking the user to
  update the app.
- A message with the right prefix but an invalid field (a coordinate out of
  range, too many decimal places, or flags that are not exactly 3 hex digits)
  gets a reply naming the field that could not be read.

The format of the server's reply messages is not yet defined.

As a regular expression, a valid version 1 message (after trimming whitespace
and ignoring case) is:

```
^TA1 +(-?\d{1,2}(\.\d{1,4})?) +(-?\d{1,3}(\.\d{1,4})?) +([0-9A-F]{3})$
```

The pattern checks the shape of the message only. The server also checks that
the latitude is within ±90 and the longitude within ±180.

## Sending from the apps

- **iOS** does not let an app send an SMS on its own. The app should open
  `MFMessageComposeViewController` with the recipient and message body filled
  in, and the user taps Send.
- **Android** can either open the messaging app with the message filled in
  (`ACTION_SENDTO` with an `smsto:` URI), or send it directly with
  `SmsManager`. Sending directly requires the `SEND_SMS` permission, which
  Google Play restricts to a small set of app types, so opening the messaging
  app is the safer default.

## Changing the format

Older app versions stay installed after a release, so the server must keep
accepting every format version that apps in use still send.

- Any change to the fields, their order, or the meaning of a flag bit requires
  a new version number (`TA2`). All 12 flag bits are in use, so adding an
  option also requires a new version.
- Keep the bit table in this document in sync with `ForecastVars` and
  `ForecastProperties` in `src/models.rs`. Adding, removing, or reordering a
  field there does not change the wire format by itself, but the parser's
  mapping and this table must be updated together.
