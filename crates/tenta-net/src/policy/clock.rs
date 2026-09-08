//! Time injection for the policy modules.
//!
//! Most policy entry points take an explicit `now: SystemTime` so they stay
//! pure functions. The rate limiter keeps state across calls and would need
//! the caller to thread a clock through every call site, so it holds a
//! [`Clock`] instead; tests inject [`ManualClock`].

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;

pub trait Clock: Send + Sync + fmt::Debug {
    fn now(&self) -> SystemTime;
}

/// The host wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A clock that only moves when a test moves it.
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<SystemTime>,
}

impl ManualClock {
    pub fn new(start: SystemTime) -> Self {
        Self {
            now: Mutex::new(start),
        }
    }

    /// Starts at the Unix epoch plus one year, far enough from `UNIX_EPOCH`
    /// that subtracting test durations never underflows.
    pub fn epoch() -> Self {
        Self::new(SystemTime::UNIX_EPOCH + Duration::from_secs(365 * 24 * 3600))
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.now.lock();
        *now += by;
    }

    pub fn set(&self, to: SystemTime) {
        *self.now.lock() = to;
    }

    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.now.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_only_moves_when_told() {
        let clock = ManualClock::epoch();
        let first = clock.now();
        assert_eq!(clock.now(), first);
        clock.advance(Duration::from_secs(5));
        assert_eq!(clock.now(), first + Duration::from_secs(5));
    }

    #[test]
    fn system_clock_moves_forward() {
        let clock = SystemClock;
        let a = clock.now();
        let b = clock.now();
        assert!(b >= a);
    }
}
