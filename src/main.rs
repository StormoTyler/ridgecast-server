use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use mongodb::bson::doc;
use serde_json::json;
use std::time::Duration;

// local modules
mod daily_forecast;
mod database;
mod errors;
mod geometry;
mod messaging;
mod models;
mod scheduler;
mod sms;
mod sms_providers;
mod snapshots;
mod state;
#[cfg(test)]
mod tests;
mod tracking;
mod unit_conversion;
mod upstream;
use crate::sms::{reply, user_numbers};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // local development only. in production the host sets the variables,
    // and dotenvy never overwrites a variable that's already set
    dotenvy::dotenv().ok();
    // logs go to stderr. RUST_LOG overrides the level, such as RUST_LOG=ridgecast=debug;
    // by default RidgeCast logs at info and its dependencies only warnings
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        // colors only on a terminal, so a log file or journal doesn't get escape codes
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,ridgecast=info")),
        )
        .init();
    // checked here so a missing value fails at startup, not on the first NWS or NAC request
    // or by silently connecting to the default local database
    for name in ["RIDGECAST_USER_AGENT", "DATABASE_URL", "RIDGECAST_DB"] {
        std::env::var(name).map_err(|_| format!("{name} must be set"))?;
    }

    // read before connecting to MongoDB, so a missing or invalid number stops the server at
    // once. a string like the checks above, so the message is printed rather than the variant
    let numbers = state::Numbers::from_env().map_err(|e| e.to_string())?;
    let number_key = user_numbers::NumberKey::from_env().map_err(|e| e.to_string())?;
    reply::set_support_email(std::env::var("SUPPORT_EMAIL").ok().as_deref())?;
    let provider = sms_providers::from_env()?;

    // a string, so a refused unencrypted connection prints its message rather than the variant
    let db = database::DB::init().await.map_err(|e| e.to_string())?;

    // bound before anything is spawned, so a server that can't listen, such as when
    // another one already holds the port, exits without starting refreshes that it
    // would cancel, which would print misleading refresh failures
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 5001));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("could not listen on {addr}: {e}"))?;

    // refresh the snapshots in the background, so startup doesn't wait on NWS or NAC.
    // a failure is only logged, the loaders refresh missing or stale snapshots on use
    let refresh_db = db.clone();
    tokio::spawn(async move {
        let (alerts, avy) = tokio::join!(
            snapshots::refresh_alerts(&refresh_db),
            snapshots::refresh_avy(&refresh_db)
        );
        if let Err(e) = alerts {
            tracing::warn!("startup: alerts refresh failed: {e}");
        }
        if let Err(e) = avy {
            tracing::warn!("startup: avalanche refresh failed: {e}");
        }
    });

    // scheduled refreshes keep the snapshots fresh while the server runs:
    // alerts at :10, :25, :40 and :55, avalanche data at :55
    tokio::spawn(scheduler::refresh_alerts(db.clone()));
    tokio::spawn(scheduler::refresh_avy(db.clone()));
    // every tracker is checked on the hour, 5 minutes after the refreshes, and sends
    // its messages through the same provider as replies
    tokio::spawn(scheduler::check_trackers(
        db.clone(),
        provider.sender.clone(),
    ));

    // printed only once the port is bound
    tracing::info!("Listening on: {addr}");
    let state = state::AppState {
        db,
        numbers,
        number_key,
        sender: provider.sender,
    };
    axum::serve(listener, app(state, provider.routes)).await?;
    Ok(())
}

// provider_routes are the SMS provider's own routes, such as the local provider's
// dev route, so the router never names a provider
fn app(state: state::AppState, provider_routes: Router<state::AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .merge(provider_routes)
        .with_state(state)
}

// a snapshot only fails the health check once it is this far past its max age.
// the scheduler refreshes alerts every ALERTS_MAX_AGE, so the snapshot passes its
// max age for the few seconds each refresh takes, which shouldn't alert anyone
const HEALTH_STALE_GRACE: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

// for uptime monitors: 503 when MongoDB can't be reached or a snapshot is missing
// or stale, since the scheduler keeps both snapshots fresh while the server runs.
// a stale snapshot means the scheduled refreshes are failing, such as NWS or NAC
// being down or changing their format
async fn health(State(db): State<database::DB>) -> Response {
    // bounded, since the driver waits up to 30 seconds for an unreachable
    // server and a monitor would time out first
    let admin = db.points_collection.client().database("admin");
    let ping = admin.run_command(doc! { "ping": 1 });
    if !matches!(
        tokio::time::timeout(Duration::from_secs(5), ping).await,
        Ok(Ok(_))
    ) {
        eprintln!("health: MongoDB did not respond to a ping");
        let body = json!({ "status": "unavailable", "database": "unreachable" });
        return (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response();
    }

    let now = chrono::Utc::now();
    // a missing snapshot counts as stale
    let is_stale = |fetched_at: Option<chrono::DateTime<chrono::Utc>>, max_age| {
        fetched_at.is_none_or(|fetched_at| {
            snapshots::is_stale(fetched_at, max_age + HEALTH_STALE_GRACE, now)
        })
    };
    let alerts = snapshots::read_fetched_at(&db.alerts_collection).await;
    let avy = snapshots::read_fetched_at(&db.avy_collection).await;
    let alerts_stale = is_stale(alerts, snapshots::ALERTS_MAX_AGE);
    let avy_stale = is_stale(avy, snapshots::AVY_MAX_AGE);

    let (status_code, status) = if alerts_stale || avy_stale {
        (StatusCode::SERVICE_UNAVAILABLE, "stale")
    } else {
        (StatusCode::OK, "ok")
    };
    let body = json!({
        "status": status,
        "database": "ok",
        "alerts": { "fetched_at": alerts, "stale": alerts_stale },
        "avy": { "fetched_at": avy, "stale": avy_stale },
    });
    (status_code, Json(body)).into_response()
}
