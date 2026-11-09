// the SMS providers a server can use, and the only code that knows their names.
// SMS_PROVIDER picks one at startup: its sender goes in AppState and its routes
// are merged into the router, so main, the server's state, and the core never
// name a provider. adding one means its file and one arm in from_value

use std::sync::Arc;

use axum::Router;

// local modules
use crate::messaging;
use crate::state;

pub mod local;

// the values SMS_PROVIDER accepts, for the startup error
const PROVIDERS: &str = "local";

// a provider: how messages leave, and the routes that receive them
pub struct Provider {
    pub sender: Arc<dyn messaging::MessageSender>,
    pub routes: Router<state::AppState>,
}

pub fn from_env() -> Result<Provider, String> {
    from_value(std::env::var("SMS_PROVIDER").ok().as_deref())
}

// the provider for the value of SMS_PROVIDER, None when it isn't set. there is
// no default: a server that fell back to local would print its replies instead
// of texting anyone, with nothing to show it. an empty value counts as unset
pub fn from_value(value: Option<&str>) -> Result<Provider, String> {
    match value.filter(|value| !value.trim().is_empty()) {
        Some("local") => Ok(local::provider()),
        Some(other) => Err(format!(
            "SMS_PROVIDER {other:?} is not a provider, use one of: {PROVIDERS}"
        )),
        None => Err(format!("SMS_PROVIDER must be set, to one of: {PROVIDERS}")),
    }
}
