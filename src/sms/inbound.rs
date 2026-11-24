// handles a text message sent to the server: decides what to do with it and
// builds the reply, which sms::outbound sends from the number the user texted.
// provider neutral: an adapter builds the InboundMessage, and the reply goes
// through whichever MessageSender the server was started with

use chrono::{DateTime, Utc};

// local modules
use crate::database;
use crate::messaging;
use crate::sms::{forecast, outbound, reply, request, user_numbers};
use crate::state;

// what to do with an inbound message, decided before anything is fetched
#[derive(Debug)]
pub enum Plan {
    // not sent to one of this server's numbers, such as another number on the
    // same provider account: logged and dropped, with no reply
    Drop,
    // a message that isn't a request, such as HELP: answered with the help
    // message alone, without the opt-in message, since asking for help isn't
    // asking for forecasts or alerts
    Help {
        reply_from: messaging::PhoneNumber,
    },
    // a reply to a request that couldn't be read: its parse error
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
        Err(request::SmsParseError::NotARequest) => Plan::Help { reply_from },
        Err(error) => Plan::Reply {
            reply_from,
            message: reply::format_parse_error(&error),
        },
    }
}

// handles one inbound message and returns what was sent, in order. empty when the
// message was dropped. the clock is read once here, so the forecast, the tracker,
// and the reply all use the same time. a number with no record gets
// the opt-in message before the reply to a request, readable or not, but not
// before the help message, and every message that isn't dropped refreshes the
// number's record
pub async fn handle(
    message: &messaging::InboundMessage,
    numbers: &state::Numbers,
    number_key: &user_numbers::NumberKey,
    db: &database::DB,
    sender: &dyn messaging::MessageSender,
) -> Vec<outbound::Delivery> {
    let now = Utc::now();
    if !claim(message, db, now).await {
        return Vec::new();
    }
    // whether the reply is one the opt-in message goes before
    let (reply_from, responses, opt_in_applies) = match plan(message, numbers) {
        Plan::Drop => {
            // no numbers in the log, a message to the wrong number is still a user's text
            tracing::warn!(
                "inbound: dropped message {}, not sent to one of this server's numbers",
                message.provider_message_id
            );
            return Vec::new();
        }
        Plan::Help { reply_from } => (reply_from, vec![vec![reply::help_message()]], false),
        Plan::Reply {
            reply_from,
            message,
        } => (reply_from, vec![vec![message]], true),
        Plan::Forecast {
            reply_from,
            request,
        } => {
            let responses = forecast_reply(&request, message, &reply_from, db, now).await;
            (reply_from, responses, true)
        }
    };
    let needs_opt_in =
        opt_in_applies && user_numbers::needs_opt_in_message(db, number_key, &message.from).await;
    let responses = with_opt_in_message(needs_opt_in, responses);
    let deliveries = outbound::send_responses(sender, &reply_from, &message.from, &responses).await;
    // the user replied STOP before this text, so a tracker this request created,
    // and any other, would only be refused at every check until it expired
    let opted_out = deliveries
        .iter()
        .any(|delivery| matches!(delivery.result, Err(messaging::SendError::OptedOut)));
    if opted_out {
        delete_trackers(&message.from, db).await;
    }
    let opt_in = reply::opt_in_message();
    let sent_opt_in = deliveries
        .iter()
        .any(|delivery| delivery.body == opt_in && delivery.result.is_ok());
    user_numbers::record_use(db, number_key, &message.from, sent_opt_in, now).await;
    deliveries
}

// whether to handle the message: false for a delivery of a message already
// handled. a message that can't be checked is handled anyway, since a
// duplicate reply is better than none
async fn claim(message: &messaging::InboundMessage, db: &database::DB, now: DateTime<Utc>) -> bool {
    let id = &message.provider_message_id;
    match database::claim_inbound(id, now, db).await {
        Ok(true) => true,
        Ok(false) => {
            tracing::warn!("inbound: dropped message {id}, already handled");
            false
        }
        Err(error) => {
            tracing::warn!(
                "inbound: could not check message {id} for a repeat, handling it: {error}"
            );
            true
        }
    }
}

// deletes every tracker of a user who opted out: by replying STOP
pub async fn delete_trackers(user: &messaging::PhoneNumber, db: &database::DB) {
    match database::delete_user_trackers(user, db).await {
        Ok(deleted) => tracing::info!("inbound: user opted out, deleted {deleted} trackers"),
        Err(error) => tracing::error!(
            "inbound: user opted out, but their trackers could not be deleted: {error}"
        ),
    }
}

// the opt-in message goes before the reply as its own (1/1) message
pub fn with_opt_in_message(needs_opt_in: bool, responses: Vec<Vec<String>>) -> Vec<Vec<String>> {
    if !needs_opt_in {
        return responses;
    }
    std::iter::once(vec![reply::opt_in_message()])
        .chain(responses)
        .collect()
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
