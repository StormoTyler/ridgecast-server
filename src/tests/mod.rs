mod daily_forecast_tests;
mod fake_sender;
mod inbound_tests;
mod messaging_tests;
mod models_tests;
mod nac_tests;
mod nws_tests;
mod outbound_tests;
mod sms_providers_tests;
mod sms_reply_tests;
mod sms_tests;
mod snapshots_tests;
mod state_tests;
mod tracking_tests;
mod user_numbers_tests;

// the SUPPORT_EMAIL tests use, set once for the whole test binary, since
// sms::reply keeps it in a OnceLock. call it in any test that builds the help message
pub const SUPPORT_EMAIL: &str = "support@example.com";

pub fn set_support_email() {
    // an error only means another test set it first, to the same address
    let _ = crate::sms::reply::set_support_email(Some(SUPPORT_EMAIL));
}
