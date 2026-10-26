// tracker helpers shared by the avalanche and alert checks

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// where now falls in a forecast or alert's valid period
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Validity
{
    // now is at or after start and before end
    Current,
    // start is still in the future
    NotStarted,
    // end has passed
    Expired,
    NoStart,
    NoEnd,
}

// checks a valid period against now. start is inclusive and end is exclusive,
// so a forecast is expired from the moment its end_date is reached.
// callers resolve their own fallbacks before calling, such as an alert using
// expires when it has no ends
pub fn validity(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Validity
{
    let Some(start) = start else { return Validity::NoStart };
    let Some(end) = end else { return Validity::NoEnd };
    if now >= end
    {
        Validity::Expired
    }
    else if now < start
    {
        Validity::NotStarted
    }
    else
    {
        Validity::Current
    }
}

// records that a tracker's user was told about a condition, so the notice is
// sent once when the condition starts and not on every scheduler check.
// stored on UserPoint as a bool
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoticeFlag(bool);

impl NoticeFlag
{
    // marks the condition as notified. returns true only when it wasn't already,
    // so the caller sends the notice exactly once
    pub fn raise(&mut self) -> bool
    {
        !std::mem::replace(&mut self.0, true)
    }

    // the condition has ended, so the notice is sent again if it returns
    pub fn clear(&mut self)
    {
        self.0 = false;
    }

    pub fn is_raised(&self) -> bool
    {
        self.0
    }
}
