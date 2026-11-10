// the local provider, for running RidgeCast without an SMS account: replies are
// printed to the console, and texts come in over HTTP on POST /dev/sms, which
// waits for the reply and returns it. for development, or a staging server with
// its own database, never a server whose database also serves real users: the
// route trusts any from number, so a request could create a tracker for a
// stranger's number (see the local provider in CLAUDE.md)

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;

// local modules
use crate::messaging;
use crate::sms::{inbound, outbound};
use crate::sms_providers;
use crate::state;

pub fn provider() -> sms_providers::Provider {
    sms_providers::Provider {
        sender: Arc::new(LocalSender::default()),
        routes: Router::new().route("/dev/sms", post(dev_sms)),
    }
}

// prints each message instead of sending it, and never fails
#[derive(Default)]
pub struct LocalSender {
    sent: AtomicU64,
}

#[async_trait]
impl messaging::MessageSender for LocalSender {
    async fn send(
        &self,
        from: &messaging::PhoneNumber,
        to: &messaging::PhoneNumber,
        body: &str,
    ) -> Result<messaging::SendReceipt, messaging::SendError> {
        let id = self.sent.fetch_add(1, Ordering::Relaxed) + 1;
        // the numbers' Display masks them, even here
        println!("local sms {id}, to {to} from {from}:\n{body}\n");
        Ok(messaging::SendReceipt {
            provider_message_id: format!("local-{id}"),
        })
    }
}

// a text as the dev route receives it. a number that isn't E.164 fails to
// deserialize, so axum answers 422 before the handler runs, naming the field
// but not the number
#[derive(Deserialize)]
struct DevText {
    from: messaging::PhoneNumber,
    body: String,
    // the server number texted, SMS_NUMBER when left out
    to: Option<messaging::PhoneNumber>,
}

async fn dev_sms(State(state): State<state::AppState>, Json(text): Json<DevText>) -> String {
    let received_at = chrono::Utc::now();
    let message = messaging::InboundMessage {
        from: text.from,
        to: text.to.unwrap_or_else(|| state.numbers.primary().clone()),
        body: text.body,
        received_at,
        provider_message_id: format!("dev-{}", received_at.timestamp_micros()),
    };
    let deliveries = inbound::handle(
        &message,
        &state.numbers,
        &state.number_key,
        &state.db,
        state.sender.as_ref(),
    )
    .await;
    format_deliveries(&deliveries)
}

// the messages as a phone would show them, separated by a blank line. a message
// that failed is marked with its error, so the response matches what was sent
pub fn format_deliveries(deliveries: &[outbound::Delivery]) -> String {
    if deliveries.is_empty() {
        return "(no reply: not sent to one of this server's numbers, dropped)\n".to_string();
    }
    let messages: Vec<String> = deliveries
        .iter()
        .map(|delivery| match &delivery.result {
            Ok(_) => delivery.body.clone(),
            Err(error) => format!("(not sent: {error})\n{}", delivery.body),
        })
        .collect();
    messages.join("\n\n") + "\n"
}
