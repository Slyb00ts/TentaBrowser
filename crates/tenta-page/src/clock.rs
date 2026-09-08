//! The clock the settle machine and the page deadlines read.
//!
//! Time is injected so tests drive the state machine deterministically
//! instead of sleeping (`docs/spec/15-testing-and-benchmarks.md` §1: no fixed
//! sleeps in tests).

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// A monotonic clock.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Instant;
}

/// The process clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A clock a test advances by hand.
#[derive(Clone, Debug)]
pub struct ManualClock {
    now: Arc<Mutex<Instant>>,
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualClock {
    pub fn new() -> Self {
        Self {
            now: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) -> Instant {
        let mut now = self.now.lock();
        *now += by;
        *now
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self.now.lock()
    }
}

/// The default clock as a shared handle.
pub fn system_clock() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_clock_only_moves_when_told_to() {
        let clock = ManualClock::new();
        let start = clock.now();
        assert_eq!(clock.now(), start);
        let moved = clock.advance(Duration::from_millis(40));
        assert_eq!(moved, start + Duration::from_millis(40));
        assert_eq!(clock.now(), start + Duration::from_millis(40));
    }

    #[test]
    fn the_system_clock_moves_on_its_own() {
        let clock = SystemClock;
        let first = clock.now();
        assert!(clock.now() >= first);
    }
}
