// handles a text message sent to the server: decides what to do with it and
// builds the reply, which sms::outbound sends from the number the user texted.
// provider neutral: an adapter builds the InboundMessage, and the reply goes
// through whichever MessageSender the server was started with

use chrono::{DateTime, Utc};

// local modules
use crate::database;
use crate::messaging;
use crate::sms::{forecast, outbound, reply, request};
use crate::state;

// what to do with an inbound message, decided before anything is fetched
#[derive(Debug)]
pub enum Plan {
    // not sent to one of this server's numbers, such as another number on the
    // same provider account: logged and dropped, with no reply
    Drop,
    // a reply that needs no forecast: the help message or a parse error
    Reply {
        reply_from: messaging::PhoneNumber,
        message: String,
    },
    // a request to answer with a forecast
    Forecast {
        reply_from: messaging::PhoneNumber,
        request: request::SmsRequest,
    },
}

// the reply is always sent from the number the user texted, so a message to any
// other number is dropped rather than answered from a number this server doesn't use
pub fn plan(message: &messaging::InboundMessage, numbers: &state::Numbers) -> Plan {
    let Some(reply_from) = numbers.matching(&message.to) else {
        return Plan::Drop;
    };
    let reply_from = reply_from.clone();
    match message.body.parse::<request::SmsRequest>() {
        Ok(request) => Plan::Forecast {
            reply_from,
            request,
        },
        Err(error) => Plan::Reply {
            reply_from,
            message: reply::format_parse_error(&error),
        },
    }
}

// handles one inbound message and returns what was sent, in order. empty when the
// message was dropped. the clock is read once here, so the forecast, the tracker,
// and the reply all use the same time
pub async fn handle(
    message: &messaging::InboundMessage,
    numbers: &state::Numbers,
    db: &database::DB,
    sender: &dyn messaging::MessageSender,
) -> Vec<outbound::Delivery> {
    let now = Utc::now();
    let (reply_from, responses) = match plan(message, numbers) {
        Plan::Drop => {
            // no numbers in the log, a message to the wrong number is still a user's text
            tracing::warn!(
                "inbound: dropped message {}, not sent to one of this server's numbers",
                message.provider_message_id
            );
            return Vec::new();
        }
        Plan::Reply {
            reply_from,
            message,
        } => (reply_from, vec![vec![message]]),
        Plan::Forecast {
            reply_from,
            request,
        } => {
            let responses = forecast_reply(&request, message, &reply_from, db, now).await;
            (reply_from, responses)
        }
    };
    outbound::send_responses(sender, &reply_from, &message.from, &responses).await
}

// the reply to a request, as responses of messages, or a one-message error if the
// forecast couldn't be built
async fn forecast_reply(
    request: &request::SmsRequest,
    message: &messaging::InboundMessage,
    reply_from: &messaging::PhoneNumber,
    db: &database::DB,
    now: DateTime<Utc>,
) -> Vec<Vec<String>> {
    match forecast::get_requested_forecast(request, &message.from, reply_from, db, now).await {
        Ok(response) => reply::format_reply(request, &response, now),
        Err(error) => vec![vec![reply::format_api_error(&error)]],
    }
}
