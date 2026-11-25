# RidgeCast server

![CI](https://github.com/StormoTyler/ridgecast-server/actions/workflows/ci.yml/badge.svg)

RidgeCast sends weather forecasts, National Weather Service alerts, and
avalanche forecasts via text message. Requests to the server are made over SMS
or satellite messenger.

This repository is the server: it answers requests, and runs trackers that
follow a location for up to a week and send new alerts, new avalanche
forecasts, and daily forecasts.

> **Status: in development.** Answering requests and the hourly tracker checks
> work end to end with the `local` SMS provider, which prints messages instead
> of texting them. A [Telnyx](https://telnyx.com) adapter is included for
> sending real texts, but hasn't been tested against Telnyx yet.

## How it works

A request is one line ([request format](docs/sms-format.md)):

```
RC1 45.3735 -121.6959 0000 0 2
```

The server replies with numbered messages of at most 140 characters
([reply format](docs/sms-reply-format.md)):

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
MongoDB. Coverage is only where the NWS provides regular forecasts (may not
cover all U.S. territories).

## Running it locally

You need:
- [Rust](https://rustup.rs) 1.97 or newer
- A running MongoDB 7 server, where the server stores its data

Example with Docker:

```bash
docker run -d --name mongodb -p 127.0.0.1:27017:27017 mongo:7.0
```

Then:

```bash
cp .env.example .env    # then set RIDGECAST_USER_AGENT to your app name and contact
cargo run               # serves on 127.0.0.1:5001, change it with HOST and PORT
```

See `.env.example` for each variable. The server stops at startup if one is
missing or invalid.

A MongoDB that isn't on the same machine must be reached over TLS, with its
certificate checked: add `tls=true` to `DATABASE_URL`, or use a
`mongodb+srv://` URI, which turns TLS on by default. Otherwise the server
refuses to start. Only `localhost`, `127.x.x.x`, `::1`, and Unix sockets count
as the same machine. That includes Docker: a MongoDB reached by a Compose
service name, such as `mongodb://mongodb:27017`, needs TLS too.

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

## Sending texts with Telnyx

The repository includes a Telnyx adapter (`src/sms_providers/telnyx.rs`). To
use it:

1. In the Telnyx portal, set up a number on a messaging profile, and copy an
   API key and the public key (Keys & Credentials > Public Key). A toll-free
   number must pass toll-free verification before it can send.
2. Set `SMS_PROVIDER=telnyx`, `TELNYX_API_KEY`, `TELNYX_PUBLIC_KEY`, and
   `SMS_NUMBER` (the Telnyx number). The server stops at startup if a key is
   missing or invalid.
3. Set the messaging profile's webhook URL to
   `https://<your server>/webhooks/telnyx`. The server must be reachable from
   the internet over HTTPS. Webhooks not signed with the public key get 401.
4. Telnyx answers `HELP`, `STOP`, and `START` itself, from the messaging
   profile's autoresponses, so the server doesn't reply to them. Set the `HELP`
   autoresponse to the server's help message, with your `SUPPORT_EMAIL`:

   ```
   (1/1) RidgeCast
   Help: support@example.com. Reply STOP to opt out.
   ```

   Keep it in step with the help message in
   [the reply format](docs/sms-reply-format.md) if that changes.

A `STOP` deletes all of the user's trackers. Telnyx's opt-outs cover every
number on the messaging profile, so a user who replies `STOP` to one number is
opted out of all of them until they reply `START`.

## Hosting

The repository includes a `Dockerfile`, which builds an image any container
host can run. The environment variables in `.env.example` come from the host
when the container starts; the image never contains them. Whatever the host,
the server needs:

- **Exactly one instance, always running.** Every running server checks the
  trackers on the hour, so two instances would send every tracker message
  twice, and a stopped one misses the checks. Turn off autoscaling and
  scale-to-zero, and use a deploy that stops the old instance before starting
  the new one (no blue-green or canary deploys, which run both for a while).
- **To listen on every interface.** The image sets `HOST=0.0.0.0`, and the
  server listens on `PORT`, 5001 by default.
- **A MongoDB it reaches over TLS.** The server refuses an unencrypted
  connection to a database on another machine. MongoDB Atlas connection
  strings use TLS; the free M0 tier's limits aren't meant for production.
- **A health check that doesn't take it out of service.** `/health` returns
  503 when a snapshot is stale, such as while NWS is down, and the server can
  still answer texts then. Use it for monitoring, never as a check that stops
  traffic to the server or restarts it: with one instance, that would cut off
  the SMS provider's webhooks.
- **HTTPS from the internet** for the SMS provider's webhooks (see Sending
  texts with Telnyx).
- **Logs kept only as long as your privacy policy allows.** The server writes
  its log to stderr, and the log includes the coordinates users request.
  RidgeCast's own policy keeps them 30 days.

Under plain `docker run`, add `--init`: the server doesn't handle signals, so
as the container's first process it would ignore `docker stop`'s SIGTERM until
Docker kills it 10 seconds later.

### Example: Fly.io

RidgeCast itself runs on Fly.io, with the config in `fly.toml`. It runs one
machine that never stops, deploys by replacing it, and checks `/health` for
monitoring only. To use it:

1. Create the MongoDB cluster in the same area as `primary_region` in
   `fly.toml`. Atlas only accepts connections from addresses on its IP access
   list, and a Fly machine's outbound address isn't fixed by default: either
   give the app a static egress address (see Fly's docs) or allow `0.0.0.0/0`
   and rely on the database user's password and TLS.
2. Install `flyctl`, run `fly auth login`, set `app` in `fly.toml` to a name of
   your own, and create the app with `fly apps create <name>`.
3. Set the server's settings as secrets. They're the variables in
   `.env.example`, except `SMS_PROVIDER` and `RIDGECAST_DB`, which `fly.toml`
   sets:

   ```bash
   fly secrets set DATABASE_URL='mongodb+srv://…' \
     USER_NUMBERS_KEY="$(openssl rand -hex 32)" \
     RIDGECAST_USER_AGENT='(RidgeCast, you@example.com)' \
     SMS_NUMBER='+1…' SUPPORT_EMAIL='you@example.com' \
     TELNYX_API_KEY='…' TELNYX_PUBLIC_KEY='…'
   ```

   Keep `USER_NUMBERS_KEY` once it's set: changing it sends every user the
   opt-in message again.
4. Deploy with `fly deploy --ha=false`. Fly's first deploy otherwise starts
   two machines. `fly scale count` should show one.
5. Set the Telnyx messaging profile's webhook URL to
   `https://<app>.fly.dev/webhooks/telnyx`.

`fly logs` shows the server's log, which Fly keeps for 7 days.

## Development

```bash
cargo fmt         # format; CI rejects unformatted code
cargo clippy --all-targets -- -D warnings
cargo test        # uses saved API responses, needs no MongoDB or network
TEST_DATABASE_URL=mongodb://127.0.0.1:27018 cargo test -- --include-ignored   # also the database tests, which need MongoDB, TEST_DATABASE_URL should be different than the DATABASE_URL set in .env
```

CI runs all three on every push and pull request to `main`. To catch
unformatted code before it's committed, turn on the pre-commit hook once per
clone:

```bash
git config core.hooksPath .githooks
```

SMS providers are adapters in `src/sms_providers/` (`local.rs` and
`telnyx.rs`), and the rest of the server only sees the `messaging` interface,
so adding a provider means adding its file there and one line in
`src/sms_providers/mod.rs`.

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
