// the SMS feature: parsing a request, building its response, formatting the
// reply, and recording the users' numbers. the formats are in docs/sms-format.md and docs/sms-reply-format.md
pub mod forecast;
pub mod inbound;
pub mod outbound;
pub mod reply;
pub mod request;
pub mod user_numbers;
