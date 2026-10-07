# TrailAlert SMS reply format

**Status: draft.** Nothing in the server implements this yet.

This document defines the text messages that the TrailAlert server sends to
users: replies to requests (see [sms-format.md](sms-format.md)), scheduled
daily forecasts, and tracker notifications.

There are two formats:

- The **full format**, with plain words that need no explanation. It is used
  for every message unless the request set `brief`.
- The **brief format**, a compact single message with short labels, used for
  replies to requests that set `brief`. See [Brief format](#brief-format).

## Design goals

- **Read by people, as raw text.** iOS never lets an app read incoming SMS,
  and satellite messengers such as Garmin inReach show messages on their own
  device, so the app never reads replies. Users read them in their messaging
  app or on their satellite messenger, and every message must make sense
  there.
- **No learning curve.** The full format uses plain words and units, so a new
  user can read it without a legend. Only the brief format trades readability
  for length.
- **At most 140 characters per message.** This is the most restrictive limit
  found so far, from SMS via satellite on iPhone. Garmin inReach splits
  incoming texts at about 160 characters.
- **Each message stands on its own.** A long response is sent as several
  separate messages, never as one long SMS that the phone has to reassemble.
  Messages can arrive out of order or not at all, so the most important
  information goes in the first message.

## Message layout

Every message is a header line followed by body lines, separated by a line
feed (`\n`):

```
(1/4)
!Winter Storm Warning until Wed 10/7 6PM
Avy danger 3 Considerable, Mt Hood
```

The whole message, including the header and every line feed, is at most
**140 characters**. The header line plus its line feed takes 6 characters,
which leaves 134 for the body.

### Header

The header is only `(x/n)`: `x` is the number of this message and `n` is the
total number of messages in the response.

- Every message has a header, including a response that is a single message
  (`(1/1)`).
- `n` is at most 9.
- Messages carry no response id, so the server sends every message of one
  response before it starts another response to the same phone number. A
  tracker notification that becomes due while a reply is being sent waits
  until the reply is finished.

### Message kinds

| Kind | Sent when | Body |
|---|---|---|
| Reply | A request is answered | Elevation, alerts, avalanche, tracker, forecast days |
| Daily forecast | A `daily_forecast` tracker reaches 6 AM | Elevation, alerts, avalanche, forecast days |
| Alert notification | A tracker finds a new NWS alert | The new alerts |
| Avalanche notification | A tracker finds a new avalanche forecast | Avalanche |
| Tracker ended | A tracker has ended | `Tracking ended` |
| Error | A request could not be answered | Error line |
| Help | A message that is not a request | Help text |

## Full format

### Body lines

| Line | Example |
|---|---|
| Elevation | `Elevation 5240ft` |
| Alert | `!Winter Storm Warning until Wed 10/7 6PM` |
| Avalanche | `Avy danger 3 Considerable, Mt Hood` |
| Tracker | `Tracking alerts, avy until Fri 10/9 10AM` |
| Notice | `Daily forecast unavailable` |
| Tracker ended | `Tracking ended` |
| Forecast day | `Tue: high 34F, low 22F. Wind SW 25mph. Precip 90%, 0.85in. Snow 11in` |
| Error | `Bad latitude: 95.1` |

A line is at most 134 characters and is never split across two messages.

### Times

All times and days are local time at the forecast location, in the time zone
that NWS reports for it, not the time zone of the user's phone. Messages do
not name the time zone.

- **Day:** three letters: `Mon` `Tue` `Wed` `Thu` `Fri` `Sat` `Sun`.
- **Point in time:** the day, the month and day of the month, then the
  12-hour time. Minutes are shown only when they are not zero. Examples:
  `Wed 10/7 6PM`, `Fri 10/9 10:30AM`.

### Elevation

```
Elevation 5240ft
```

The elevation of the forecast location in feet. It is the first body line of
replies and daily forecasts when the request set the `elevation` flag, and is
left out otherwise.

### Alert

```
!Winter Storm Warning until Wed 10/7 6PM
```

`!`, the NWS event name, `until`, and the alert's expiry time. If the
alert has no expiry time, the line ends after the event name. Alerts are
listed in the order NWS returns them.

If the alert lines would not fit in the first message, the server lists as
many as fit and ends with `!+2 more alerts` (the number of alerts left out).

### Avalanche

```
Avy danger 3 Considerable, Mt Hood
Avy danger 4 High (was 3), Mt Hood
Avy: center is in its off season
```

- `Avy danger`, the danger level (1 to 5), the danger rating, then the
  forecast zone name. Zone names are shortened to fit if needed.
- In an avalanche notification, `(was <level>)` gives the previous danger
  level.
- When there is no rating, the line is one of these notices, matching
  `avy_notice` in `src/sms.rs`:

| Line | Meaning |
|---|---|
| `Avy: no current forecast` | The zone has no current avalanche forecast |
| `Avy: center is in its off season` | The zone's avalanche center is in its off season |
| `Avy: data unavailable, try later` | Avalanche data could not be read |

Without `track_avy`, the line is included only at danger level 3 or higher,
as `get_requested_forecast` already does.

### Tracker

```
Tracking alerts, avy, daily forecast until Fri 10/9 10AM
```

Included in a reply when the request created a tracker. It lists what is
tracked (`alerts`, `avy`, `daily forecast`) and when the tracker ends.

### Notice

| Line | Meaning |
|---|---|
| `Daily forecast unavailable` | The request set `daily_forecast`, but no daily forecast tracker was created because NWS reported no time zone for the location |

### Forecast day

```
Tue: high 34F, low 22F. Wind SW 25mph, gusts 45. Precip 90%, 0.85in. Snow 11in. Feels like 18F/8F. Humidity 80-100%
```

Each day covers local midnight to midnight. A day starts with its name and a
colon, followed by sentences in the order of the table below. A sentence is
left out when its value is zero, has no data, or (for optional fields) its
flag was not set. The first two are always included.

| Sentence | Field | Flag | Value |
|---|---|---|---|
| `high 34F, low 22F.` | `max_temperature`, `min_temperature` | base | The daytime high and the low of the night that follows. A value with no data is left out, for example `low 41F.` alone for today after the daytime high period has ended |
| `Wind SW 25mph, gusts 45.` | `wind_direction`, `wind_speed`, `wind_gust` | base, gusts with `wind_gust` | 8-point compass direction (`N`, `NE`, `E`, `SE`, `S`, `SW`, `W`, `NW`) at the time of the highest sustained wind, that speed, and the highest gust, in mph |
| `Precip 90%, 0.85in.` | `probability_of_precipitation`, `quantitative_precipitation` | base | Highest chance of precipitation, and total precipitation as liquid in inches with 2 decimals. The amount is left out when it is zero |
| `Snow 11in.` | `snowfall_amount` | base | Total snowfall in inches. 1 decimal under 1 inch, whole inches above |
| `Ice 0.10in.` | `ice_accumulation` | base | Total ice in inches, 2 decimals |
| `Feels like 18F/8F.` | `apparent_temperature` | base | Highest/lowest feels-like temperature. Included only when either one differs from the high or low by 5 °F or more |
| `Heat risk 2 of 4.` | `heat_risk` | `heat_risk` | Highest NWS HeatRisk category, 0 to 4 |
| `Wet bulb globe 85F.` | `wet_bulb_globe_temperature` | `wet_bulb_globe_temperature` | Highest wet bulb globe temperature |
| `Humidity 30-80%.` | `relative_humidity` | `relative_humidity` | Lowest-highest relative humidity |
| `Visibility 0.5mi.` | `visibility` | `visibility` | Lowest visibility in miles. Left out when it is 10 miles or more |
| `Cloud ceiling 1200ft.` | `ceiling_height` | `ceiling_height` | Lowest cloud ceiling in feet. Left out when there is no ceiling |
| `Pressure 29.85inHg.` | `pressure` | `pressure` | Lowest pressure in inches of mercury |
| `Snow level 4500ft.` | `snow_level` | `snow_level` | Lowest snow level in feet, rounded to 100 |

The server converts NWS's metric units to the units above. Rounding is to the
nearest whole number unless the table says otherwise. The last sentence of a
line has no final period.

A line holds at most 134 characters. If a day's sentences do not fit in one
line, they continue on another line that starts with the day name again:

```
Tue: high 34F, low 22F. Wind SW 25mph, gusts 45. Precip 90%, 0.85in. Snow 11in
Tue: Heat risk 2 of 4. Visibility 0.5mi. Cloud ceiling 1200ft
```

### Order and splitting

The server builds every body line of a response, then fills the messages in
this order, starting a new message when the next line does not fit:

1. Elevation
2. Alerts
3. Avalanche
4. Tracker and notices
5. Forecast days, starting with today

If only the first message arrives, the user still has the alerts and
avalanche danger.

A forecast day is never split across two messages when it fits in one. If all
of a day's lines do not fit in the space left in the current message, the day
starts a new message. Only a day too long for a single message, which needs
many optional fields, continues from one message to the next.

Limits per response:

| Kind | Messages | Forecast days |
|---|---|---|
| Reply | at most 9 | up to 7, dropping the last days to stay within 9 messages |
| Daily forecast | at most 3 | today and the next 2 days |
| Alert and avalanche notifications | at most 2 | none |
| Tracker ended, error, help | exactly 1 | none |

A forecast day takes about 60 to 120 characters, depending on the weather and
the optional fields requested. A day with rain or snow and several optional
fields fills a message on its own, and two quiet days share one. The 7-day
reply in the [examples](#examples) takes 7 messages, including the first
message with the alert, avalanche, and tracker lines. Users who pay per
message, such as Garmin inReach users, should use `brief`.

## Brief format

A reply to a request with the `brief` flag is a single message, `(1/1)`, in a
compact format with short labels. It contains:

1. Alerts.
2. The avalanche line without the zone name.
3. Today's and tomorrow's forecast, with only the three always-present tokens
   and `P`, `R`, and `S`.

There is no elevation, tracker, or notice line. If the message does not fit in
140 characters, forecast days are dropped first, then the alert lines are
replaced with `!+<n> alerts`.

```
(1/1)
!Winter Storm Warning til We7 6PM
AV3 Considerable
Tu 34/22 SW25 P90 R0.85 S11
We 30/19 W18 P70 R0.30 S5
```

`brief` applies only to the reply to the request. Daily forecasts and tracker
notifications use the full format.

### Brief times

The same local time as the full format, written shorter:

- **Day:** two letters: `Mo` `Tu` `We` `Th` `Fr` `Sa` `Su`.
- **Point in time:** the day, the day of the month with no leading zero, a
  space, then the 12-hour time. Minutes are shown only when they are not zero.
  Examples: `We7 6PM`, `Fr9 10:30AM`.

### Brief lines

| Line | Example | Meaning |
|---|---|---|
| Alert | `!Winter Storm Warning til We7 6PM` | `!`, the NWS event name, `til`, and the expiry time. If the alerts do not fit, they are replaced with `!+<n> alerts` |
| Avalanche | `AV3 Considerable` | `AV`, the danger level, and the danger rating |
| Avalanche notice | `AV no forecast`, `AV off season`, `AV unavailable` | The same notices as the full format |
| Forecast day | `Tu 34/22 SW25 P90 R0.85 S11` | See below |

### Brief forecast day

```
Tu 34/22 SW25 P90 R0.85 S11
```

The first three tokens are always present, in this order:

1. **Day**, such as `Tu`.
2. **High/low** in °F. A value with no data is `--`, for example `--/41`.
3. **Wind**: the compass direction at the time of the highest sustained wind,
   then that speed in mph, such as `SW25`.

Labeled tokens follow, left out when their value is zero or has no data:

| Token | Value |
|---|---|
| `P90` | Highest chance of precipitation, % |
| `R0.85` | Total precipitation as liquid, inches, 2 decimals |
| `S11` | Total snowfall, inches. 1 decimal under 1 inch, whole inches above |

The values are the same as the matching full format sentences.

## Errors

An error message has one line: a short explanation that may include the value
that could not be read.

| Cause | Line |
|---|---|
| `SmsParseError::UnsupportedVersion` | `Update the TrailAlert app` |
| `SmsParseError::WrongFieldCount` | `Expected 5 fields, found 4` |
| `SmsParseError::InvalidLatitude` | `Bad latitude: 95.1` |
| `SmsParseError::InvalidLongitude` | `Bad longitude: -123,09` |
| `SmsParseError::InvalidFlags` | `Bad flags: G845` |
| `SmsParseError::InvalidTrackerDuration` | `Bad tracker hours: 200` |
| `APIError::LocationNotCovered` | `Location not covered by NWS forecasts` |
| NWS or NAC is down or timed out | `Weather service unavailable, try later` |
| Any other server error | `Server error, try later` |

A message that is not a request (`SmsParseError::NotARequest`) gets a help
message instead:

```
(1/1)
TrailAlert: send requests from the TrailAlert app. Typed format: TA1 lat lon flags hours. Reply STOP to opt out.
```

## Examples

A request for a 72-hour alert and avalanche tracker, relative humidity, and
wind gust (`TA1 45.3735 -121.6959 0845 72`), answered with a 7-day forecast in
7 messages:

```
(1/7)
!Winter Storm Warning until Wed 10/7 6PM
Avy danger 3 Considerable, Mt Hood
Tracking alerts, avy until Fri 10/9 10AM
```

```
(2/7)
Tue: high 34F, low 22F. Wind SW 25mph, gusts 45. Precip 90%, 0.85in. Snow 11in. Feels like 18F/8F. Humidity 80-100%
```

```
(3/7)
Wed: high 30F, low 19F. Wind W 18mph, gusts 35. Precip 70%, 0.30in. Snow 5in. Feels like 15F/5F. Humidity 75-95%
```

```
(4/7)
Thu: high 36F, low 24F. Wind NW 10mph, gusts 20. Precip 20%. Snow 0.5in. Humidity 60-90%
```

```
(5/7)
Fri: high 41F, low 27F. Wind N 6mph, gusts 12. Humidity 45-80%
Sat: high 44F, low 30F. Wind E 5mph, gusts 9. Humidity 40-75%
```

```
(6/7)
Sun: high 42F, low 29F. Wind SE 8mph, gusts 15. Precip 30%. Humidity 50-85%
```

```
(7/7)
Mon: high 45F, low 31F. Wind S 10mph, gusts 18. Precip 40%, 0.10in. Humidity 55-90%
```

The same request with `brief` (`TA1 45.3735 -121.6959 1845 72`):

```
(1/1)
!Winter Storm Warning til We7 6PM
AV3 Considerable
Tu 34/22 SW25 P90 R0.85 S11
We 30/19 W18 P70 R0.30 S5
```

A daily forecast:

```
(1/2)
Tue: high 68F, low 47F. Wind NW 8mph. Precip 10%
Wed: high 72F, low 49F. Wind N 6mph
```

```
(2/2)
Thu: high 64F, low 50F. Wind S 12mph. Precip 60%, 0.20in
```

Notifications from a tracker:

```
(1/1)
!Red Flag Warning until Thu 10/8 8PM
!Wind Advisory until Wed 10/7 11AM
```

```
(1/1)
Avy danger 4 High (was 3), Mt Hood
```

A reply with notices, to a request that set `track_avy` and `daily_forecast`
for a location whose avalanche center is in its off season and whose time
zone NWS did not report:

```
(1/1)
Avy: center is in its off season
Daily forecast unavailable
Tue: high 68F, low 47F. Wind NW 8mph. Precip 10%
```

## Character set

Messages use only characters from the GSM-7 basic character set, which allows
160 characters per SMS segment. The server must replace any other character
in text that comes from NWS or NAC, such as an accented letter in a zone name,
before sending. Characters from the GSM-7 extension table (`{ } [ ] ~ ^ \ |
€`) are not used, because each one counts as 2 characters. The `°` sign is not
in GSM-7, so temperatures are written as `34F`.

## Changing the format

Only people read replies, so the format can change in any server release
without breaking the apps. Update the examples in this document with every
change.

## Open questions

- **Long messages over satellite on iPhone.** Apple limits messages sent over
  satellite to about 140 characters, but no limit is documented for incoming
  messages. Test whether a 300-character SMS sent to an iPhone connected by
  satellite arrives whole, split, or truncated.
- **Daily detail.** A forecast day gives daily highs and totals, so it does
  not show when during the day rain or wind will start. Splitting days into
  day and night, as NWS text forecasts do, would show more and use about twice
  the space.
- **Message count.** A full 7-day reply takes about 5 to 8 messages, 7 in the
  example. If that is too many, the reply could cover fewer days, with the
  rest available through `daily_forecast`.
