//! Local time as Home Assistant sees it. HA's conditions and time triggers use its configured
//! time zone, so everything the engine does with local time uses that zone too; until HA has
//! told us its zone, the container's zone is used.

use std::sync::RwLock;

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

static ZONE: RwLock<Option<Tz>> = RwLock::new(None);

/// Sets the zone from an IANA name ("Europe/Amsterdam"); false if the name is unknown.
pub fn set_zone(name: &str) -> bool {
    let Ok(tz) = name.parse::<Tz>() else { return false };
    *ZONE.write().unwrap_or_else(|p| p.into_inner()) = Some(tz);
    true
}

/// HA's zone, once known.
pub fn zone() -> Option<Tz> {
    *ZONE.read().unwrap_or_else(|p| p.into_inner())
}

/// The container's own zone, as an IANA name when it can be found out.
pub fn container_zone() -> Option<String> {
    iana_time_zone::get_timezone().ok()
}

pub fn now() -> DateTime<FixedOffset> {
    local(Utc::now())
}

/// An instant in local time.
pub fn local<Z: TimeZone>(t: DateTime<Z>) -> DateTime<FixedOffset> {
    match zone() {
        Some(tz) => t.with_timezone(&tz).fixed_offset(),
        None => t.with_timezone(&Local).fixed_offset(),
    }
}

/// A wall-clock time in local time (the earlier one when clocks go back); None if it doesn't
/// exist (when clocks go forward).
pub fn from_local(naive: NaiveDateTime) -> Option<DateTime<FixedOffset>> {
    match zone() {
        Some(tz) => naive.and_local_timezone(tz).earliest().map(|t| t.fixed_offset()),
        None => naive.and_local_timezone(Local).earliest().map(|t| t.fixed_offset()),
    }
}

/// The next time a cron schedule comes round, in local time.
pub fn next_cron(cron: &croner::Cron) -> Option<DateTime<FixedOffset>> {
    fn next<Z: TimeZone>(cron: &croner::Cron, now: DateTime<Z>) -> Option<DateTime<FixedOffset>> {
        cron.find_next_occurrence(&now, false).ok().map(|t| t.fixed_offset())
    }
    match zone() {
        Some(tz) => next(cron, Utc::now().with_timezone(&tz)),
        None => next(cron, Local::now()),
    }
}
