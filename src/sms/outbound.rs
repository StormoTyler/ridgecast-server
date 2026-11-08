// sends messages to a user through the server's MessageSender: replies now, and
// tracker messages once the scheduler sends them. every message goes out in
// order, with the retry policy in CLAUDE.md, so callers only decide what to send

use std::time::Duration;

// local modules
use crate::messaging;

// attempts at sending one message when it fails with SendError::Transient
const SEND_ATTEMPTS: usize = 3;
// the wait before each retry. transient failures usually clear within seconds,
// and a longer wait only delays the rest of the reply
const RETRY_DELAYS: [Duration; SEND_ATTEMPTS - 1] =
    [Duration::from_secs(2), Duration::from_secs(8)];
// added to each wait, so retries after a provider outage don't all arrive at once
const MAX_JITTER: Duration = Duration::from_millis(500);

// one message and what happened when it was sent
#[derive(Debug)]
pub struct Delivery {
    pub body: String,
    pub result: Result<messaging::SendReceipt, messaging::SendError>,
}

// sends responses, each a list of messages numbered from (1/n), one after another
// as a single stream in order, from one of the server's numbers to the user.
// returns a Delivery for each message attempted, in order
pub async fn send_responses(
    sender: &dyn messaging::MessageSender,
    from: &messaging::PhoneNumber,
    to: &messaging::PhoneNumber,
    responses: &[Vec<String>],
) -> Vec<Delivery> {
    let messages: Vec<&str> = responses.iter().flatten().map(String::as_str).collect();
    send_in_order(sender, from, to, &messages).await
}

// each message is retried before the next is sent, so a satellite messenger never
// receives (3/n) before (2/n). a message that still fails after SEND_ATTEMPTS is
// logged and skipped, since the rest may hold alerts and the gap in the numbering
// tells the user something is missing. a permanent failure or an opt out stops the
// rest, since every message would fail the same way
async fn send_in_order(
    sender: &dyn messaging::MessageSender,
    from: &messaging::PhoneNumber,
    to: &messaging::PhoneNumber,
    messages: &[&str],
) -> Vec<Delivery> {
    let mut deliveries = Vec::with_capacity(messages.len());
    for (index, body) in messages.iter().enumerate() {
        let result = send_with_retries(sender, from, to, body).await;
        let stop = match &result {
            Ok(_) => false,
            Err(messaging::SendError::Transient(error)) => {
                eprintln!(
                    "outbound: message {} of {} failed after {SEND_ATTEMPTS} attempts, \
                     sending the rest: {error}",
                    index + 1,
                    messages.len()
                );
                false
            }
            Err(error) => {
                eprintln!(
                    "outbound: message {} of {} not sent, stopping: {error}",
                    index + 1,
                    messages.len()
                );
                true
            }
        };
        deliveries.push(Delivery {
            body: body.to_string(),
            result,
        });
        if stop {
            break;
        }
    }
    deliveries
}

// one message, retried only on SendError::Transient. adapters make one attempt
// per send, so these are the only retries
async fn send_with_retries(
    sender: &dyn messaging::MessageSender,
    from: &messaging::PhoneNumber,
    to: &messaging::PhoneNumber,
    body: &str,
) -> Result<messaging::SendReceipt, messaging::SendError> {
    let mut result = sender.send(from, to, body).await;
    for delay in RETRY_DELAYS {
        if !matches!(result, Err(messaging::SendError::Transient(_))) {
            break;
        }
        tokio::time::sleep(delay + jitter()).await;
        result = sender.send(from, to, body).await;
    }
    result
}

// up to MAX_JITTER, from the clock's nanoseconds, which is random enough to
// spread retries without adding a dependency
fn jitter() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    MAX_JITTER.mul_f64(f64::from(nanos) / 1_000_000_000.0)
}
