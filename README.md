# RidgeCast server

![CI](https://github.com/StormoTyler/ridgecast-server/actions/workflows/ci.yml/badge.svg)

RidgeCast sends weather forecasts, National Weather Service alerts, and
avalanche forecasts via text message. Requests to the server are made over SMS
or satellite messenger.

This repository is the server: it answers requests, and (in progress) runs
trackers that follow a location for up to a week and send new alerts, new
avalanche forecasts, and daily forecasts.

> **Status: in development.** Answering requests works end to end with the
> `local` SMS provider, which prints replies instead of texting them. SMS
> provider adapters, and tracker messages, aren't implemented yet.

## How it works

A request is one line ([request format](docs/sms-format.md)):

```
RC1 45.3735 -121.6959 0000 0 2
```

The server replies with numbered messages of at most 140
characters ([reply format](docs/sms-reply-format.md)):

```
(1/2) RidgeCast
Avy danger 3 Considerable, Mt Hood
!Severe: Winter Storm Warning until Wed 10/7 6PM
```

Data comes from:
- the [National Weather Service API](https://www.weather.gov/documentation/services-web-api)
  (`api.weather.gov`): gridpoint forecasts and active alerts
- the [National Avalanche Center](https://avalanche.org/) (`api.avalanche.org`):
  avalanche forecasts from local and regional avalanche centers

Alerts and avalanche forecasts are refreshed on a schedule and stored in
MongoDB. Coverage is wherever NWS forecasts: the US and its territories.

## Running it locally

You need:
- [Rust](https://rustup.rs) 1.97 or newer
- A running MongoDB 7 server, where the server stores its data.
 
 Example with Docker:
  ```bash
  docker run -d --name mongodb -p 127.0.0.1:27017:27017 mongo:7.0
  ```

Then:

```bash
cp .env.example .env    # then set RIDGECAST_USER_AGENT to your app name and contact
cargo run               # serves on 127.0.0.1:5001
```

See `.env.example` for each variable. The server stops at startup if one is
missing or invalid.

With `SMS_PROVIDER=local`, send a text over HTTP and get the reply back:

```bash
curl -H 'Content-Type: application/json' \
  -d '{"from":"+15415550123","body":"RC1 45.3735 -121.6959 0000 0 2"}' \
  127.0.0.1:5001/dev/sms
```

`GET /health` reports whether MongoDB and the stored alert and avalanche data
are current, for uptime monitors.

> **The `local` provider is for development only.** Its `/dev/sms` route trusts
> any `from` number, so never run it against a database that serves real users.

## Development

```bash
cargo fmt         # format; CI rejects unformatted code
cargo clippy --all-targets -- -D warnings
cargo test        # uses saved API responses, needs no MongoDB or network
```

CI runs all three on every push and pull request to `main`.

SMS providers are adapters in `src/sms_providers/`, and the rest of the server
only sees the `messaging` interface, so adding a provider means adding one file
there.

## Safety

RidgeCast passes on information from NWS and avalanche centers, and it can be
late, incomplete, or wrong, and text messages can be delayed or lost. It doesn't
replace checking official sources, proper trip planning, and good judgment in
the backcountry.

## Self-hosting and the name

You're welcome to run your own server. If you run one for other people, please
use a different name than RidgeCast, so texts from your server aren't mistaken
for the RidgeCast service, and publish your own privacy policy and SMS terms.

RidgeCast is not affiliated with or endorsed by the National Weather Service or
the National Avalanche Center.

## License

[AGPL-3.0-or-later](LICENSE). If you run a modified version as a service for
others, you must offer them its source code.
