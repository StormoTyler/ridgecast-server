# RidgeCast SMS reply format

**Status: draft.** `src/sms/reply.rs` implements replies to requests, errors,
and the help message. Daily forecasts and tracker notifications aren't
implemented yet. Replies are sent only through the `local` SMS provider's
development route, `POST /dev/sms`, until a real provider is added.

This document defines the text messages that the RidgeCast server sends to
users: replies to requests (see [sms-format.md](sms-format.md)), scheduled
daily forecasts, and tracker notifications.

There are two formats:

- The **full format**, with plain words that need no explanation. It is used
  for every message unless the request set `brief`.
- The **brief format**, a compact message with short labels, used for the
  reply to a request that set `brief` and for every message from the tracker
  that request created. Forecast days take at most 3 messages, and only
  alerts take more. See [Brief format](#brief-format).

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
- **Alerts are never dropped.** Alerts are safety critical, so every alert is
  sent, even when that takes more messages. Forecast days are dropped to keep
  a response short, never alerts.

## Message layout

Every message is a header line followed by body lines, separated by a line
feed (`\n`):

```
(1/4) RidgeCast
Avy danger 3 Considerable, Mt Hood
!Severe: Winter Storm Warning until Wed 10/7 6PM
```

The whole message, including the header and every line feed, is at most
**140 characters**. The longest header line, with a 10-character tracker name,
plus its line feed takes 17 characters, which leaves 123 for the body. The
server uses the same 123-character limit for every message, with or without a
tracker name, so a body line fits under any header.

### Header

The header is `(x/n)`: `x` is the number of this message and `n` is the
total number of messages in the response, followed by a space and a name. A
message sent by a tracker has the tracker's name, as given in the request (see
[sms-format.md](sms-format.md#tracker-name)), and every other message has
`RidgeCast`:

```
(1/2) Mt Hood
(1/2) RidgeCast
```

- The name is in the header of every message that a tracker sends on its own:
  daily forecasts, alert and avalanche notifications, and the tracking ended
  message. A user with several trackers can tell which one a message is
  about, even when only some of its messages arrive.
- Replies to requests, errors, and the help message have `RidgeCast` in the
  header, so every message identifies its sender, as toll-free verification
  expects. `RidgeCast` fits in the space kept for a tracker name, so it costs
  no body characters. A reply that creates a tracker names it in its tracker
  line, and the user knows the reply answers the request they just sent.

- Every message has a header, including a response that is a single message
  (`(1/1)`).
- `n` is at most 9. A response that needs more than 9 messages, which only
  happens with many alerts, continues as another response that is numbered
  from `(1/n)` again and sent right after the first.
- Messages carry no response id, so the server sends every message of one
  response before it starts another response to the same phone number. A
  tracker notification that becomes due while a reply is being sent waits
  until the reply is finished.

### Message kinds

| Kind | Sent when | Header name | Body |
|---|---|---|---|
| Reply | A request is answered | `RidgeCast` | Notices, avalanche, alerts, forecast days, tracker |
| Daily forecast | A `daily_forecast` tracker reaches 6 AM | Tracker name | Notices, avalanche, alerts, forecast days |
| Alert notification | A tracker finds a new NWS alert | Tracker name | The new alerts |
| Avalanche notification | A tracker finds a new avalanche forecast, or loses the forecast it last sent | Tracker name | Avalanche |
| Tracker ended | The tracker's last check before it expires | Tracker name | `Tracking ended`, the last line of that check's message, or alone when the check sends nothing else |
| Error | A request could not be answered | `RidgeCast` | Error line |
| Help | A message that is not a request | `RidgeCast` | Help text |
| Opt-in | Before the first reply to a number, unless that reply is the help message | `RidgeCast` | Help text |

Every kind sent by a tracker uses the format of the request that created it,
so a tracker created with `brief` sends brief daily forecasts and
notifications. Errors and the help message are always in the full format.

### Tracking ended

A tracker is checked every hour, on the hour, and is deleted when it expires.
Its last check is the last one before it expires: a tracker that expires at
2:10 PM is last checked at 2:00 PM, and nothing checks it after that. That
check ends the tracker, so it says `Tracking ended` rather than giving the
expiry time, even though the tracker expires up to an hour later: no alert,
avalanche forecast, or daily forecast is sent after it.

- When the last check sends a daily forecast or a notification, `Tracking
  ended` is the last line of its last message, which costs no extra message
  when it fits.
- When the last check has nothing else to send, `Tracking ended` is sent alone
  as `(1/1) <tracker name>`.
- The line is never dropped to stay within a limit. Forecast days are dropped
  instead, as for the tracker line in a reply.
- The reply that created the tracker already gave its expiry time. If the last
  check's message is not delivered after its retries, there is no later check
  to send it again.

## Full format

### Body lines

| Line | Example |
|---|---|
| Notice | `Daily forecast unavailable` |
| Avalanche | `Avy danger 3 Considerable, Mt Hood` |
| Alert | `!Severe: Winter Storm Warning until Wed 10/7 6PM` |
| Forecast day | `Tue: high 34F, low 22F Precip 90%, 0.85in Snow 11in Wind SW 25mph` |
| Tracker | `Tracking Mt Hood: alerts, avy until Fri 10/9 10AM` |
| Tracker ended | `Tracking ended` |
| Error | `Bad latitude: 95.1` |

A line is at most 123 characters and is never split across two messages.

### Times

All times and days are local time at the forecast location, in the time zone
that NWS reports for it, not the time zone of the user's phone. Messages do
not name the time zone.

- **Day:** three letters: `Mon` `Tue` `Wed` `Thu` `Fri` `Sat` `Sun`.
- **Point in time:** the day, the month and day of the month, then the
  12-hour time. Minutes are shown only when they are not zero. Examples:
  `Wed 10/7 6PM`, `Fri 10/9 10:30AM`.

### Notice

| Line | Meaning |
|---|---|
| `Daily forecast unavailable` | The request set `daily_forecast`, but no daily forecast tracker was created because NWS reported no time zone for the location |
| `Could not create tracker` | The request asked for a tracker, but it could not be saved, even after one retry, matching `TrackerStatus::NotCreated` in `src/sms/forecast.rs`. No tracker messages will be sent, so the user should send the request again. It takes the place of the tracker line, at the top of the reply instead of the bottom |

Notices are the first body lines, above the avalanche line.

### Avalanche

```
Avy danger 3 Considerable, Mt Hood
Avy: no danger rating, Mt Hood
Avy: center is in its off season
```

- `Avy danger`, the danger level (1 to 5), the danger rating, then the
  forecast zone name. Zone names are never shortened. The longest NAC zone
  name, `San Francisco Peaks / Kachina Peaks Wilderness` (46 characters), fits
  in a line with the longest rating.
- The line never gives the previous danger level. A user who received the
  previous message already knows it.
- `Avy: no danger rating`, then the forecast zone name, is sent for a valid
  forecast with danger level -1. NAC, and other services using its API, treat
  a -1 forecast as valid but without a rating.
- When there is no valid forecast, or the data can't be used, the line is one
  of these notices, matching `avy_notice` in `src/sms/forecast.rs`:

| Line | Meaning |
|---|---|
| `Avy: no current forecast` | The zone has no valid avalanche forecast: none was published, it has no end time, its end time has passed, or its danger level is missing or neither 1 to 5 nor -1 (no rating). A forecast that has not started yet is valid |
| `Avy: center is in its off season` | The zone's avalanche center is in its off season |
| `Avy: data unavailable, try later` | Avalanche data could not be read |
| `Avy: data may be out of date` | The stored avalanche data is more than 2 hours old and could not be refreshed from NAC, matching `avy_stale_notice` in `src/sms/forecast.rs` |

Without `track_avy`, the line is included only at danger level 3 or higher,
as `get_requested_forecast` already does. The `Avy: data unavailable, try
later` notice is included with or without `track_avy`, since a danger level of
3 or higher cannot be ruled out when the data cannot be read. For the same
reason, `Avy: data may be out of date` is included with or without
`track_avy`. It follows the danger line or other notice when there is one.

The line gives only the danger level, the rating, and the zone name, to keep
avalanche information to one short line. NAC's other forecast fields are not
sent: the travel advice, the link to the full forecast, the avalanche center,
and the avalanche warning or watch product (`warning`, which `AvyProperties`
does not read). The center's own forecast has the details. The map styling
fields (`color`, `stroke`, and the like), `state`, and `center_id` are not
read at all.

An avalanche tracker sends a notification when it finds a forecast that differs
from the last one it sent. When that forecast stops being valid, for any
reason, the tracker sends one notification:

- `Avy: center is in its off season` if the center entered its off season,
  since it may not publish another forecast before the tracker ends.
- `Avy: no current forecast` followed by `Avy: data may be out of date` if
  the avalanche data is out of date. A tracker can't confirm its forecast from
  out of date data, so this counts as losing it, even if the forecast's end
  time hasn't passed.
- `Avy: no current forecast` for any other reason, including the zone missing
  from the NAC data.

The loss is sent once. Later checks without a valid forecast send nothing,
with one exception: `Avy: center is in its off season` is still sent once after
any other kind of loss, since it tells the user forecasts may not resume before
the tracker ends. Other losses after the off season notice send nothing. If
the same forecast becomes valid again, nothing is sent, since the user already
has it. A tracker that never had a valid forecast sends no loss notification,
since its creation reply already gave the notice.

### Alert

```
!Severe: Winter Storm Warning until Wed 10/7 6PM
```

`!`, the alert's NWS severity and a colon, the NWS event name, `until`, and
the time the hazard ends (NWS's `ends`, not `expires`, which is when the alert
message expires and is often hours or days earlier). If the alert has no end
time, the line ends after the event name.

- The severity is one of `Extreme`, `Severe`, `Moderate`, `Minor`, and
  `Unknown`. It comes first so it is read first, and it is always included,
  since the event name doesn't always imply it: NWS sends a `Dense Fog
  Advisory` as either `Minor` or `Moderate`, and a watch can be `Moderate` or
  `Severe`. An alert without a severity is shown as `Unknown`, so a missing
  severity is never read as a low one.
- Alerts are sorted by severity: `Extreme`, `Severe`, `Unknown`, `Moderate`,
  then `Minor`. An unknown severity could be serious, so it sorts right after
  `Severe` instead of last. Alerts with the same severity keep the order NWS
  returns them in. When the alerts take several messages, the most severe
  arrive first.

Every alert is sent. Alert lines that don't fit in the first message continue
in the next ones, and alerts are never cut short to make room for anything
else.

```
!Alerts could not be checked
```

Sent in place of the alert lines when no alerts could be read: NWS could not
be reached and there are no stored alerts to fall back on, matching
`alerts_notice` in `src/sms/forecast.rs`. The rest of the reply is sent as usual. It
stays in the first message, so a reply without alert lines is never read as
having no active alerts.

```
!Alerts may be out of date
```

Sent before the alert lines when the stored alerts are more than 15 minutes
old and could not be refreshed from NWS, matching `alerts_stale_notice` in
`src/sms/forecast.rs`. It is included even when there are no alert lines, since
out-of-date alerts can miss a new alert. Coming before the alert lines keeps
it in the first message, however many alerts follow.

### Forecast day

```
Tue: high 34F, low 22F Humidity 80-100% Precip 90%, 0.85in Snow 11in Wind SW 25mph, gusts 45
```

Each day covers local midnight to midnight. A day starts with its name and a
colon, followed by sentences in the order of the table below. A sentence is
left out when its optional flag was not set, or when the table says it is
left out. An optional field that was requested but has no data says `N/A`,
such as `Pressure N/A`, so the user knows it was checked. A base field with no
data is left out, unless its sentence has another value to show: then the
missing value says `N/A` and the rest is shown, such as `Wind N/A, gusts 45`
or `Precip N/A, 0.20in`. A series in a unit the server can't convert counts as no
data, so a value is never shown in the wrong unit.

| Sentence | Field | Flag | Value |
|---|---|---|---|
| `high 34F, low 22F` | `max_temperature`, `min_temperature` | base | The daytime high and the low of the night that follows. A value with no data is left out, for example `low 41F` alone for today after the daytime high period has ended |
| `Wet bulb globe 85F` | `wet_bulb_globe_temperature` | `wet_bulb_globe_temperature` | Highest wet bulb globe temperature |
| `Heat risk 2 of 4` | `heat_risk` | `heat_risk` | Highest NWS HeatRisk category, 0 to 4 |
| `Humidity 30-80%` | `relative_humidity` | `relative_humidity` | Lowest-highest relative humidity |
| `Precip 90%, 0.85in` | `probability_of_precipitation`, `quantitative_precipitation` | base | Highest hourly chance of precipitation during the day, which is how NWS's own 12-hour chances matched in the data checked, always included, even at `Precip 0%`, since users expect it in a weather forecast. Then total precipitation as liquid in inches with 2 decimals, left out when it is zero. Left out with no data. An amount without a chance is `Precip N/A, 0.20in` |
| `Snow 11in` | `snowfall_amount` | base | Total snowfall in inches. 1 decimal under 1 inch, the nearest half inch under 4 inches (`Snow 2.5in`), whole inches from 4. Left out when it rounds to zero |
| `Snow level 4500ft` | `snow_level` | `snow_level` | Lowest snow level in feet, rounded to 100 |
| `Elevation 5240ft` | point elevation | `elevation` | The elevation of the forecast location in feet. Only in the first forecast day of a reply or daily forecast, since it is the same every day |
| `Ice 0.10in` | `ice_accumulation` | base | Total ice in inches, 2 decimals. Left out when it is zero |
| `Wind SW 25mph, gusts 45` | `wind_direction`, `wind_speed`, `wind_gust` | base, gusts with `wind_gust` | 8-point compass direction (`N`, `NE`, `E`, `SE`, `S`, `SW`, `W`, `NW`) at the time of the highest sustained wind, that speed, and the highest gust, in mph. With no wind data, `Wind N/A, gusts 45` when gusts were requested (`Wind N/A, gusts N/A` with no gust data either), and left out otherwise |
| `Visibility 0.5mi` | `visibility` | `visibility` | Lowest visibility in miles, 1 decimal under 1 mile and the nearest half mile above. `Visibility 10mi` when it is 10 miles or more, the most NWS reports |
| `Pressure 29.85inHg` | `pressure` | `pressure` | Lowest pressure in inches of mercury, 2 decimals. NWS sends pressure without a unit code, so it is read as pascals |
| `Cloud ceiling 1200ft` | `ceiling_height` | `ceiling_height` | Lowest cloud ceiling in feet. `Cloud ceiling N/A` when there is no ceiling |

The server converts NWS's metric units to the units above. Rounding is to the
nearest whole number unless the table says otherwise. Sentences are separated
by a single space, with no period, which saves a character per sentence. Each
sentence after the first starts with a capitalized label, which marks where it
begins.

A line holds at most 123 characters. If a day's sentences do not fit in one
line, they continue on another line that starts with the day name again:

```
Tue: high 34F, low 22F Humidity 80-100% Precip 90%, 0.85in Snow 11in Snow level 4500ft Elevation 5240ft
Tue: Wind SW 25mph, gusts 45 Visibility 0.5mi Pressure 29.85inHg Cloud ceiling 1200ft
```

### Tracker

```
Tracking Mt Hood: alerts, avy, daily forecast until Fri 10/9 10AM
```

Included in a reply when the request created a tracker, as the last body
line. It gives the tracker's name, what is tracked (`alerts`, `avy`,
`daily forecast`), and when the tracker ends. The tracker's own messages show
the name in their header, so the user can match them to this line. It is
never dropped, since it tells the user the tracker exists.

If the tracker could not be saved, there is no tracker line. The `Could not
create tracker` notice is sent at the top of the reply instead, and the rest
of the reply is sent as usual.

### Order and splitting

The server builds every body line of a response, then fills the messages in
this order, starting a new message when the next line does not fit:

1. Notices
2. Avalanche
3. Alerts
4. Forecast days, starting with today
5. Tracker

Avalanche and alert information is the most safety critical, so it comes
first. If only the first message arrives, the user still has the notices and
the avalanche danger, and the alerts start there.

The server defines this order, and the order of the sentences in a forecast
day, in one place. Either can change in any server release, but each release
uses one fixed order.

A forecast day is never split across two messages when it fits in one. If all
of a day's lines do not fit in the space left in the current message, the day
starts a new message. Only a day too long for a single message, which needs
many optional fields, continues from one message to the next.

Limits per response:

| Kind | Messages | Forecast days |
|---|---|---|
| Reply | at most 9 | the request's forecast length (1 to 7), dropping the last days to stay within 9 messages |
| Daily forecast | at most 3, or in the brief format more only when its alerts need more | today and the next 2 days |
| Alert notifications | at most 9 | none |
| Avalanche notifications | at most 2 | none |
| Tracker ended, when sent alone | exactly 1 | none |
| Error, help | exactly 1 | none |

Forecast days are the only lines dropped to stay within a limit. When the
notices, avalanche, alerts, and tracker line alone need more than 9 messages,
the rest continue as another response, so no alert or tracker line is ever
left out. The same applies to an alert notification with more alerts than fit
in 9 messages, and to a daily forecast whose alerts don't fit in 3.

A forecast day takes about 45 to 120 characters, depending on the weather and
the optional fields requested. A day with rain or snow and several optional
fields fills a message on its own, and two quiet days can share one. The
7-day reply in the [examples](#examples), with an alert, two optional fields,
and a tracker, takes 9 messages, and a 7-day reply with no optional fields
and no alerts can take as few as 4 or 5, so a 7-day reply takes about 4 to 9
messages. A request asks for fewer days with its forecast length field (see
[sms-format.md](sms-format.md)). The brief format uses fewer messages, but it
is harder to read, so it is never the default and the server never suggests
it. Whether to use it is the user's choice in the app.

## Brief format

A request with the `brief` flag gets its reply, and every message from the
tracker it creates, in a compact format with short labels. Each takes at
most 3 messages, unless its alerts need more.

A brief reply contains, in the same order as the full format:

1. `!no tracker`, only when the request asked for a tracker that could not be
   created.
2. The avalanche line without the zone name.
3. Alerts.
4. The request's forecast length in days, starting with today. Each day has
   a token for every sentence the full format would show, including the
   optional fields the request set. See [Brief forecast day](#brief-forecast-day).

There is no tracker line, and no notice line other than
`!no tracker`, so the brief format does not show the tracker name.

The lines other than forecast days come first, in as many messages as they
need, since alerts are never dropped. Forecast days then fill the space left
in the last message, and continue in new messages until the response has 3
messages. The days that don't fit are dropped from the last, so the response
keeps the days nearest today. When alerts already take 3 or more messages,
forecast days only fill the space left in the last one, and never add a
message.

Three messages is the limit, rather than one, so that a forecast is less
likely to be dropped entirely when alerts take most of the first message.
`!no tracker` is never dropped, since without it the user would wait for
tracker messages that will never be sent.

```
(1/1) RidgeCast
AV3 Considerable
!Sev Winter Storm Warning til We7 6PM
Tu 34/22 P90 R0.85 SN11 SW25
We 30/19 P70 R0.30 SN5 W18
```

### Brief tracker messages

A tracker created by a request with `brief` sends every message in the brief
format. The header still has the tracker's name, `(1/1) Mt Hood`: brief
avalanche lines have no zone name, so with several trackers the header is the
only thing that says which one a message is about.

| Kind | Brief body |
|---|---|
| Daily forecast | The avalanche line, alerts, then today and the next 2 days. At most 3 messages unless its alerts need more, filled the same way as a brief reply |
| Alert notification | The new alerts as brief alert lines, such as `!Sev Winter Storm Warning til We7 6PM`. Every new alert is sent, continuing in more messages and responses as in the full format |
| Avalanche notification | `AV4 High` for a new forecast, `AV no rating`, `AV off season` for an off season loss, `AV no forecast` for any other loss, and `AV no forecast` then `AV may be old` for a loss to out of date data |
| Tracker ended | `Tracking ended`, the same as the full format, as the last line of the last check's message or alone |

### Brief times

The same local time as the full format, written shorter:

- **Day:** two letters: `Mo` `Tu` `We` `Th` `Fr` `Sa` `Su`.
- **Point in time:** the day, the day of the month with no leading zero, a
  space, then the 12-hour time. Minutes are shown only when they are not zero.
  Examples: `We7 6PM`, `Fr9 10:30AM`.

### Brief lines

| Line | Example | Meaning |
|---|---|---|
| No tracker notice | `!no tracker` | The full format's `Could not create tracker` notice, matching `TrackerStatus::NotCreated` in `src/sms/forecast.rs` |
| Avalanche | `AV3 Considerable` | `AV`, the danger level, and the danger rating |
| Avalanche no rating | `AV no rating` | The full format's `Avy: no danger rating`, for danger level -1 |
| Avalanche notice | `AV no forecast`, `AV off season`, `AV unavailable`, `AV may be old` | The same notices as the full format |
| Alerts stale notice | `!alerts may be old` | The same notice as the full format, before the alert lines |
| Alert | `!Sev Winter Storm Warning til We7 6PM` | `!`, the severity as three letters (`Ext`, `Sev`, `Mod`, `Min`, `Unk`), the NWS event name, `til`, and the expiry time. Sorted by severity, as in the full format. Never dropped |
| Alerts unavailable notice | `!alerts not checked` | The full format's `!Alerts could not be checked`. It is never dropped |
| Forecast day | `Tu 34/22 P90 R0.85 SN11 SW25` | See below |

### Brief forecast day

```
Tu 34/22 H80-100 P90 R0.85 SN11 SW25G45
```

A brief day has a token for every sentence the full format would show for it,
in the full format's sentence order. Optional fields are included when the
request set their flag, and base fields that the full format leaves out at
zero are left out here too. An optional field that was requested but has no
data is its label then `--`, such as `H--`, matching the full format's `N/A`.

| Token | Full format sentence | Present |
|---|---|---|
| Day, such as `Tu` | the day name | Always |
| `34/22` | `high 34F, low 22F` | Always. A value with no data is `--`, for example `--/41` |
| `WB85` | `Wet bulb globe 85F` | With `wet_bulb_globe_temperature` |
| `HR2` | `Heat risk 2 of 4` | With `heat_risk` |
| `H30-80` | `Humidity 30-80%` | With `relative_humidity`. `H80` when the lowest and highest are the same |
| `P90` | `Precip 90%` | Always, even at `P0`, as in the full format. Left out with no data, as in the full format. `P--` before an amount without a chance: `P-- R0.20` |
| `R0.85` | the precipitation amount, `0.85in` | Left out when it is zero |
| `SN11` | `Snow 11in` | Left out when it rounds to zero. Rounded as in the full format: `SN0.5`, `SN2.5`, `SN11` |
| `SL4500` | `Snow level 4500ft` | With `snow_level` |
| `EL5240` | `Elevation 5240ft` | With `elevation`, in the first forecast day only, as in the full format |
| `I0.10` | `Ice 0.10in` | Left out when it is zero |
| `SW25` | `Wind SW 25mph` | Always. The direction then the speed, `--` with no data |
| `G45` | `gusts 45` | With `wind_gust`, joined to the wind token with no space: `SW25G45`. `SW25G--` with no gust data, and `--G45` with no wind data |
| `V0.5` | `Visibility 0.5mi` | With `visibility`. `V10` at 10 miles or more |
| `PR29.85` | `Pressure 29.85inHg` | With `pressure` |
| `C1200` | `Cloud ceiling 1200ft` | With `ceiling_height`. `C--` when there is no ceiling |

Every label starts with a letter that a compass direction does not, or adds
a second letter that no direction has (`SN`, `SL`, `EL`, `WB`), so a token
can't be read as the wind: `SN11` is snow and `S12` is wind from the south.

A day with every optional field takes about 70 to 95 characters, so with
alerts or several days, fewer days fit in a brief message.

The values are the same as the matching full format sentences.

## Errors

An error message has one line: a short explanation that may include the value
that could not be read.

| Cause | Line |
|---|---|
| `SmsParseError::UnsupportedVersion` | `Update the RidgeCast app` |
| `SmsParseError::WrongFieldCount` | `Expected 6 fields, found 5` |
| `SmsParseError::InvalidLatitude` | `Bad latitude: 95.1` |
| `SmsParseError::InvalidLongitude` | `Bad longitude: -123,09` |
| `SmsParseError::InvalidFlags` | `Bad flags: G845` |
| `SmsParseError::InvalidTrackerDuration` | `Bad tracker hours: 200` |
| `SmsParseError::InvalidForecastLength` | `Bad forecast days: 9` |
| `SmsParseError::MissingTrackerName` | `Tracker name missing` |
| `SmsParseError::InvalidTrackerName` | `Bad tracker name: Mt Hood!` |
| `APIError::LocationNotCovered` | `Location not covered by NWS forecasts` |
| NWS or NAC is down or timed out | `Weather service unavailable, try later` |
| Any other server error | `Server error, try later` |

A message that is not a request (`SmsParseError::NotARequest`), including
`HELP`, gets a help message instead. It gives the contact address and how to
opt out, which toll-free verification requires of the HELP response:

```
(1/1) RidgeCast
Help: support@ridgecast.app. Reply STOP to opt out.
```

The same message is the opt-in message. A number the server hasn't sent it to
gets it as its own message before the reply to its first text, so the
contact address and how to opt out arrive once rather than in every reply. A
first text that is not a request gets only the help message, which already
says the same. The server remembers that a number was sent it until 30 days
after the number last texted, so a number that texts again after that gets
it once more.

## Examples

A request for a 72-hour alert and avalanche tracker, relative humidity, and
wind gust, named `Mt Hood`, with a 7-day forecast
(`RC1 45.3735 -121.6959 0845 72 7 Mt Hood`), answered in 9 messages:

```
(1/9) RidgeCast
Avy danger 3 Considerable, Mt Hood
!Severe: Winter Storm Warning until Wed 10/7 6PM
```

```
(2/9) RidgeCast
Tue: high 34F, low 22F Humidity 80-100% Precip 90%, 0.85in Snow 11in Wind SW 25mph, gusts 45
```

```
(3/9) RidgeCast
Wed: high 30F, low 19F Humidity 75-95% Precip 70%, 0.30in Snow 5in Wind W 18mph, gusts 35
```

```
(4/9) RidgeCast
Thu: high 36F, low 24F Humidity 60-90% Precip 20% Snow 0.5in Wind NW 10mph, gusts 20
```

```
(5/9) RidgeCast
Fri: high 41F, low 27F Humidity 45-80% Precip 0% Wind N 6mph, gusts 12
```

```
(6/9) RidgeCast
Sat: high 44F, low 30F Humidity 40-75% Precip 0% Wind E 5mph, gusts 9
```

```
(7/9) RidgeCast
Sun: high 42F, low 29F Humidity 50-85% Precip 30% Wind SE 8mph, gusts 15
```

```
(8/9) RidgeCast
Mon: high 45F, low 31F Humidity 55-90% Precip 40%, 0.10in Wind S 10mph, gusts 18
```

```
(9/9) RidgeCast
Tracking Mt Hood: alerts, avy until Fri 10/9 10AM
```

The same request with `brief` and a 2-day forecast
(`RC1 45.3735 -121.6959 1845 72 2 Mt Hood`). The request set relative
humidity and wind gust, so each day has `H` and `G` tokens. Wednesday would
take the first message past 123 characters, so it starts a second one:

```
(1/2) RidgeCast
AV3 Considerable
!Sev Winter Storm Warning til We7 6PM
Tu 34/22 H80-100 P90 R0.85 SN11 SW25G45
```

```
(2/2) RidgeCast
We 30/19 H75-95 P70 R0.30 SN5 W18G35
```

A daily forecast from a tracker named `Camp`, then the same daily forecast
from a tracker created with `brief`:

```
(1/2) Camp
Tue: high 68F, low 47F Precip 10% Wind NW 8mph
Wed: high 72F, low 49F Precip 0% Wind N 6mph
```

```
(2/2) Camp
Thu: high 64F, low 50F Precip 60%, 0.20in Wind S 12mph
```

```
(1/1) Camp
Tu 68/47 P10 NW8
We 72/49 P0 N6
Th 64/50 P60 R0.20 S12
```

Notifications from a tracker named `Mt Hood`:

```
(1/1) Mt Hood
!Severe: Red Flag Warning until Thu 10/8 8PM
!Moderate: Wind Advisory until Wed 10/7 11AM
```

```
(1/1) Mt Hood
Avy danger 4 High, Mt Hood
```

The same notifications from a tracker created with `brief`:

```
(1/1) Mt Hood
!Sev Red Flag Warning til Th8 8PM
!Mod Wind Advisory til We7 11AM
```

```
(1/1) Mt Hood
AV4 High
```

```
(1/1) Mt Hood
Tracking ended
```

The last check of a tracker named `Mt Hood` that finds a new alert ends the
tracker in the same message:

```
(1/1) Mt Hood
!Severe: Winter Storm Warning until Wed 10/7 6PM
Tracking ended
```

A reply with notices, to a request named `Turnagain` that set `track_avy` and
`daily_forecast` for a location whose avalanche center is in its off season
and whose time zone NWS did not report. The avalanche tracker is still
created, so the first forecast after the season starts is sent as an
avalanche notification:

```
(1/2) RidgeCast
Daily forecast unavailable
Avy: center is in its off season
Tue: high 68F, low 47F Precip 10% Wind NW 8mph
```

```
(2/2) RidgeCast
Tracking Turnagain: avy until Fri 10/9 10AM
```

## Character set

Messages use only characters from the GSM-7 basic character set, which allows
160 characters per SMS segment. Text from NWS and NAC (event names and zone
names) is used as received: none of it uses characters outside GSM-7, so the
server doesn't convert it. Characters from the GSM-7 extension table (`{ } [ ] ~ ^ \ |
€`) are not used, because each one counts as 2 characters. The `°` sign is not
in GSM-7, so temperatures are written as `34F`.

## Changing the format

Only people read replies, so the format can change in any server release
without breaking the apps. Update the examples in this document with every
change.

## Decisions

- **Long messages over satellite on iPhone.** Apple limits messages sent over
  satellite to about 140 characters, but no limit is documented for incoming
  messages, and it hasn't been tested whether a longer SMS sent to an iPhone
  connected by satellite arrives whole, split, or truncated. Without the
  equipment to test it, the format assumes 140 characters per message.
- **Daily detail.** A forecast day gives daily highs and totals, so it does
  not show when during the day rain or wind will start. Days are not split
  into day and night, as NWS text forecasts do: that would use about twice the
  space, and keeping the number of messages low comes first, since every
  message costs satellite messenger users and the server's host.
- **Feels like temperature.** `apparent_temperature` is not sent.
