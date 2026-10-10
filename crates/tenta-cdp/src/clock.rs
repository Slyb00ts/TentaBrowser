//! The protocol's `MonotonicTime`: seconds on a clock that only moves forward,
//! shared by every domain so timestamps of different events compare.

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

/// `at` on the protocol's monotonic clock.
pub(crate) fn seconds_at(at: Instant) -> f64 {
    at.saturating_duration_since(origin()).as_secs_f64()
}

/// `at` as seconds since the Unix epoch, the protocol's `TimeSinceEpoch`.
pub(crate) fn wall_seconds_at(at: Instant) -> f64 {
    let age = Instant::now().saturating_duration_since(at);
    SystemTime::now()
        .checked_sub(age)
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |since| since.as_secs_f64())
}
