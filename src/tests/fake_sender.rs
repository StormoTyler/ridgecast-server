// a MessageSender for tests, shared by the test files that send messages
use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::messaging;

// a MessageSender that returns scripted results in order, then succeeds, and
// records every attempt
#[derive(Default)]
pub struct FakeSender {
    pub results: Mutex<VecDeque<Result<messaging::SendReceipt, messaging::SendError>>>,
    pub attempts: Mutex<Vec<(messaging::PhoneNumber, messaging::PhoneNumber, String)>>,
}

impl FakeSender {
    pub fn scripted(results: Vec<Result<messaging::SendReceipt, messaging::SendError>>) -> Self {
        FakeSender {
            results: Mutex::new(results.into()),
            ..FakeSender::default()
        }
    }

    // the body of every attempt, in order
    pub fn attempted(&self) -> Vec<String> {
        let attempts = self.attempts.lock().unwrap();
        attempts.iter().map(|(_, _, body)| body.clone()).collect()
    }
}

#[async_trait]
impl messaging::MessageSender for FakeSender {
    async fn send(
        &self,
        from: &messaging::PhoneNumber,
        to: &messaging::PhoneNumber,
        body: &str,
    ) -> Result<messaging::SendReceipt, messaging::SendError> {
        self.attempts
            .lock()
            .unwrap()
            .push((from.clone(), to.clone(), body.to_string()));
        self.results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(sent)
    }
}

pub fn sent() -> Result<messaging::SendReceipt, messaging::SendError> {
    Ok(messaging::SendReceipt {
        provider_message_id: "sent".to_string(),
    })
}

pub fn transient() -> Result<messaging::SendReceipt, messaging::SendError> {
    Err(messaging::SendError::Transient("timeout".to_string()))
}

// stand-in message bodies, numbered like a reply
pub fn messages(count: usize) -> Vec<String> {
    (1..=count).map(|n| format!("({n}/{count})")).collect()
}
