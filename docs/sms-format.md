# RidgeCast SMS request format

This document defines the text message that the RidgeCast iOS and Android apps
send to the RidgeCast server to request a forecast, alert tracking, or an
avalanche forecast. The apps and the server must both follow it exactly.

The format is kept short so that every request fits in a single SMS segment,
including when it is sent over satellite messaging.

Current version: **1**

## Message layout

A request is one line made up of six fields separated by single spaces,
followed by a tracker name when the request creates a tracker:

```
RC1 44.0512 -123.0921 1845 24 3 Mt Hood
│   │       │         │    │  │ └─ name       tracker name, only with a tracker flag
│   │       │         │    │  └─── length     days of forecast in the reply
│   │       │         │    └────── duration   hours a created tracker stays active
│   │       │         └─────────── flags      which options are turned on (4 hex digits)
│   │       └───────────────────── longitude  decimal degrees
│   └───────────────────────────── latitude   decimal degrees
└───────────────────────────────── prefix     "RC" followed by the format version
```

| Field | Format | Example |
|---|---|---|
| Prefix | `RC` followed by the format version as a decimal integer | `RC1` |
| Latitude | Decimal degrees from `-90` to `90`, at most 4 decimal places | `44.0512` |
| Longitude | Decimal degrees from `-180` to `180`, at most 4 decimal places | `-123.0921` |
| Flags | Exactly 4 hexadecimal digits (`0`–`9`, `A`–`F`) | `1845` |
| Tracker duration | Whole number of hours from `0` to `168`, 1 to 3 digits | `24` |
| Forecast length | One digit from `1` to `7` | `3` |
| Tracker name | 1 to 10 characters, may contain spaces. Sent only when a tracker flag is set | `Mt Hood` |

The longest possible message, `RC1 -90.0000 -180.0000 3FFF 168 7 ABCDEFGHIJ`, is
44 characters.

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

The flags field packs 14 on/off options into a 16-bit number written as 4
hexadecimal digits. A bit set to `1` means the option is on. Bits 14 and 15 are
unassigned and must be `0`. The server rejects flags with any of them set.

Each bit corresponds to a field of the `ForecastVars` struct in
`src/models.rs`. Bit 0 is the least significant bit.

| Bit | Value (hex) | `ForecastVars` field | Meaning when set |
|---|---|---|---|
| 0 | `0001` | `track_alerts` | Track this location and send new NWS alerts for it |
| 1 | `0002` | `no_forecast` | Do not include a weather forecast in the reply |
| 2 | `0004` | `track_avy` | Create an avalanche danger tracker for this location |
| 3 | `0008` | `forecast_properties.elevation` | Include elevation |
| 4 | `0010` | `forecast_properties.wet_bulb_globe_temperature` | Include wet bulb globe temperature |
| 5 | `0020` | `forecast_properties.heat_risk` | Include heat risk |
| 6 | `0040` | `forecast_properties.relative_humidity` | Include relative humidity |
| 7 | `0080` | `forecast_properties.visibility` | Include visibility |
| 8 | `0100` | `forecast_properties.ceiling_height` | Include ceiling height |
| 9 | `0200` | `forecast_properties.pressure` | Include pressure |
| 10 | `0400` | `forecast_properties.snow_level` | Include snow level |
| 11 | `0800` | `forecast_properties.wind_gust` | Include wind gust |
| 12 | `1000` | `brief` | Send the forecast in abbreviated form |
| 13 | `2000` | `daily_forecast` | Send a forecast for this location every 24 hours while the tracker is active |

To build the flags field, add (bitwise OR) the values of every option that is
on, then write the result as exactly 4 hex digits, padding with leading zeros.
Apps should send uppercase hex digits. The server also accepts lowercase.

```kotlin
// Kotlin
val flags = String.format(Locale.US, "%04X", bits)
```

```swift
// Swift
let flags = String(format: "%04X", bits)
```

### Tracker duration

The number of hours that a tracker created by the request stays active. The
maximum is `168` (one week). The tracker flags are `track_alerts`, `track_avy`,
and `daily_forecast`. A request with several of them set creates one tracker
that covers all of them.

- When any tracker flag is set, the duration must be from `1` to `168`. A
  duration of `0` is rejected. Trackers requested over 168 hours are set
  to 168 hours by default.
- A `daily_forecast` tracker sends a forecast each day at 6 AM while it is
  active. See [Daily forecast](#daily-forecast).
- When no tracker flag is set, the request creates no tracker. Send `0`.
  The server accepts any value up to `168` and ignores it.
- A request with a tracker flag set must end with a tracker name. See
  [Tracker name](#tracker-name).
- Write the number in decimal digits only, with no sign, decimal point, or unit.

### Forecast length

The number of days of forecast in the reply to the request, counting today as
the first day. Days are local days at the forecast location, in the time zone
that NWS reports for it, so a 1-day forecast ends at the location's next
midnight.

- One digit from `1` to `7`. `0`, `8`, `9`, and leading zeros are rejected.
- The field is required. A message without it is rejected.
- It applies only to the forecast in the reply to the request. Daily
  forecasts from a `daily_forecast` tracker have their own length (see
  [sms-reply-format.md](sms-reply-format.md)).
- With `no_forecast` set, the reply has no forecast. The field must still be
  sent, and the server ignores its value. Send `1`.

### Tracker name

A name the user gives the tracker, such as `Mt Hood` or `Camp 2`. Every
message the tracker sends (alert and avalanche notifications, daily
forecasts, and the tracking ended message) shows the name, so a user with
several trackers can tell which location a message is about. See
[sms-reply-format.md](sms-reply-format.md).

- The name is the last field. Everything after the forecast length is the
  name, so it may contain spaces.
- Send it only when a tracker flag (`track_alerts`, `track_avy`, or
  `daily_forecast`) is set. A request with a tracker flag and no name is
  rejected, and so is a request with a name and no tracker flag.
- 1 to 10 characters, counting the spaces between words. The server reads
  repeated spaces as one and ignores trailing spaces, so send single spaces
  and trim the name before counting.
- Only the letters `A`–`Z` and `a`–`z`, the digits `0`–`9`, space, and
  `-` `'` `.` `#` `&` `(` `)` `/`. Accented letters, emoji, and other
  punctuation are rejected. The apps should limit the text field to these
  characters and turn off smart punctuation for it, since iOS replaces `'`
  with `’`, which is rejected.
- Case is kept as sent.
- Names do not need to be unique. The server does not use the name to find a
  tracker.

### Daily forecast

A `daily_forecast` tracker sends a forecast at 6:00 AM local time at the
tracked location. Local time is the location's time zone as reported by the
NWS points API, not the time zone of the phone that sent the request. A
forecast is sent at every 6 AM that falls while the tracker is active, so the
number of daily forecasts depends on both the duration and the time of day the
request was sent.

`daily_forecast` may be set together with `no_forecast`. The reply to the
request then has no forecast, and the tracker still sends a forecast each
morning. This is for a user who wants a recurring forecast but not one right
now.

The server does not reject a `daily_forecast` request with a tracker duration
that ends before the next 6 AM at the location. It answers the rest of the
request but does not create a daily forecast tracker, since it would never send
a forecast. **The apps' request generator must not produce this request.**

If NWS does not report a time zone for the location, the server answers the
rest of the request, does not create a daily forecast tracker, and says so in
the reply.

## Examples

| Request | Flags | Message |
|---|---|---|
| 1-day forecast only, no optional fields | `0000` | `RC1 44.0512 -123.0921 0000 0 1` |
| 5-day forecast, no optional fields | `0000` | `RC1 44.0512 -123.0921 0000 0 5` |
| Brief 2-day forecast only | `1000` | `RC1 44.0512 -123.0921 1000 0 2` |
| Track alerts for 24 hours as `Spencer`, no forecast | `0001` + `0002` = `0003` | `RC1 44.0512 -123.0921 0003 24 1 Spencer` |
| Track alerts and avalanche danger for 72 hours as `Mt Hood`, brief 2-day forecast, relative humidity, wind gust | `0001` + `0004` + `0040` + `0800` + `1000` = `1845` | `RC1 45.3735 -121.6959 1845 72 2 Mt Hood` |
| Track alerts for one week as `Tahoe`, 3-day forecast, visibility, snow level, wind gust | `0001` + `0080` + `0400` + `0800` = `0C81` | `RC1 39.1968 -120.2354 0C81 168 3 Tahoe` |
| Daily forecast for 72 hours as `Camp`, brief 1-day forecast | `2000` + `1000` = `3000` | `RC1 44.0512 -123.0921 3000 72 1 Camp` |
| Every option on, trackers for 24 hours as `Camp`, 7-day forecast | `3FFF` | `RC1 44.0512 -123.0921 3FFF 24 7 Camp` |

## Character set

Outside the tracker name, use only the characters `R`, `C`, the digits
`0`–`9`, `A`–`F`, `-`, `.`, and space. The tracker name adds the other
letters and `'` `#` `&` `(` `)` `/` (see [Tracker name](#tracker-name)). These
are all in the GSM-7 character set, which allows 160 characters
per SMS segment. A single character outside GSM-7, such as an emoji or a smart
quote added by autocorrect, switches the whole message to UCS-2, which only
allows 70 characters per segment.

## How the server reads a message

- Leading and trailing whitespace is ignored, and fields may be separated by
  more than one space. Apps should still send exactly one space between fields.
- The prefix and flags are matched without regard to case, so `rc1` is
  accepted. The tracker name keeps its case.
- A message that does not start with `RC` is not treated as a request. The
  server replies with a short help message.
- A message with an unsupported version number gets a reply asking the user to
  update the app.
- A message with the right prefix but an invalid field (a coordinate out of
  range, too many decimal places, flags that are not exactly 4 hex digits
  or set an unassigned bit, or
  a tracker duration that is out of range or `0` with a tracker flag set,
  a forecast length that is not one digit from `1` to `7`, or a tracker name
  that is too long or uses a character that is not allowed) gets a reply
  naming the field that could not be read.
- A request with a tracker flag and no tracker name gets a reply saying the
  name is missing. A request with no tracker flag and anything after the
  forecast length gets a reply about the field count.
- Fields are checked in order, so the reply names the first field that could
  not be read. The tracker name is checked last.
- The server does not check whether a `daily_forecast` tracker's duration
  includes a 6 AM (see [Daily forecast](#daily-forecast)). The apps are
  responsible for not sending a request that would never send a forecast.

The server's reply messages are defined in [sms-reply-format.md](sms-reply-format.md) (draft).

As a regular expression, a valid version 1 message (after trimming whitespace)
is:

```
^[Rr][Cc]1 +(-?\d{1,2}(\.\d{1,4})?) +(-?\d{1,3}(\.\d{1,4})?) +([0-9A-Fa-f]{4}) +(\d{1,3}) +([1-7])( +([A-Za-z0-9'.#&()/-]+( +[A-Za-z0-9'.#&()/-]+)*))?$
```

The pattern checks the shape of the message only. The server also checks that
the latitude is within ±90, the longitude within ±180, that no unassigned flag
bit is set, that the tracker duration follows the rules above, that the
tracker name is present exactly when a tracker flag is set, and that the name
is at most 10 characters with repeated spaces read as one. The pattern checks
the forecast length completely.

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
  a new version number (`RC2`). Assigning one of the unassigned bits 14 and 15
  to a new option also requires a new version, because servers that do not
  know the bit reject it.
- Keep the bit table in this document in sync with `ForecastVars` and
  `ForecastProperties` in `src/models.rs`. Adding, removing, or reordering a
  field there does not change the wire format by itself, but the parser's
  mapping and this table must be updated together.
