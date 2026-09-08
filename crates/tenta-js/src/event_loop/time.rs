//! The event loop's clock (`docs/spec/05-javascript.md` §5).
//!
//! Time is a monotonic count of nanoseconds since the loop started. It is
//! monotonic in every mode, which `performance.now` requires. In headless runs
//! the clock can be *virtual*: it only moves when advanced explicitly
//! (`Emulation.setVirtualTimePolicy`), so timers, `Date.now` and animation
//! clocks are deterministic. A *real* clock reads the OS monotonic clock.

use std::time::{Duration, Instant};

/// A monotonic timestamp, nanoseconds since the loop's origin.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
pub struct MonoTime(u64);

impl MonoTime {
    /// The loop origin.
    pub const ZERO: MonoTime = MonoTime(0);

    /// Nanoseconds since the origin.
    pub fn as_nanos(self) -> u64 {
        self.0
    }

    /// Milliseconds since the origin, as `performance.now` reports them.
    pub fn as_millis_f64(self) -> f64 {
        self.0 as f64 / 1_000_000.0
    }

    /// This time plus `delta`, saturating at the representable maximum so a
    /// pathological delay can never wrap the deadline back into the past.
    pub fn saturating_add(self, delta: Duration) -> MonoTime {
        let nanos = u64::try_from(delta.as_nanos()).unwrap_or(u64::MAX);
        MonoTime(self.0.saturating_add(nanos))
    }
}

/// The event loop's clock.
#[derive(Debug)]
pub enum Clock {
    /// Reads the OS monotonic clock relative to the loop's construction.
    Real { base: Instant },
    /// Advances only when told to, for deterministic headless runs.
    Virtual { now: MonoTime },
}

impl Clock {
    /// A clock backed by the OS monotonic clock.
    pub fn real() -> Self {
        Clock::Real {
            base: Instant::now(),
        }
    }

    /// A virtual clock starting at `start`.
    pub fn virtual_at(start: MonoTime) -> Self {
        Clock::Virtual { now: start }
    }

    /// The current time.
    pub fn now(&self) -> MonoTime {
        match self {
            Clock::Real { base } => {
                MonoTime(u64::try_from(base.elapsed().as_nanos()).unwrap_or(u64::MAX))
            }
            Clock::Virtual { now } => *now,
        }
    }

    /// Advances a virtual clock by `delta`; a no-op on a real clock, which the
    /// OS advances on its own.
    pub fn advance(&mut self, delta: Duration) {
        if let Clock::Virtual { now } = self {
            *now = now.saturating_add(delta);
        }
    }

    /// Moves a virtual clock forward to `target`. Never moves it backward, so a
    /// `target` already in the past leaves the clock unchanged. A no-op on a
    /// real clock.
    pub fn advance_to(&mut self, target: MonoTime) {
        if let Clock::Virtual { now } = self
            && target > *now
        {
            *now = target;
        }
    }

    /// Whether this clock is virtual.
    pub fn is_virtual(&self) -> bool {
        matches!(self, Clock::Virtual { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_virtual_clock_moves_only_when_advanced() {
        let mut clock = Clock::virtual_at(MonoTime::ZERO);
        assert_eq!(clock.now(), MonoTime::ZERO);
        clock.advance(Duration::from_millis(50));
        assert_eq!(clock.now().as_nanos(), 50_000_000);
        // Reading again does not move virtual time.
        assert_eq!(clock.now().as_nanos(), 50_000_000);
        clock.advance(Duration::from_millis(50));
        assert_eq!(clock.now().as_millis_f64(), 100.0);
    }

    #[test]
    fn advance_to_moves_forward_but_never_backward() {
        let mut clock = Clock::virtual_at(MonoTime::ZERO);
        clock.advance_to(MonoTime(100));
        assert_eq!(clock.now(), MonoTime(100));
        // A target in the past is ignored; the clock is monotonic.
        clock.advance_to(MonoTime(50));
        assert_eq!(clock.now(), MonoTime(100));
    }

    #[test]
    fn advancing_a_real_clock_is_a_no_op() {
        let mut clock = Clock::real();
        let before = clock.now();
        clock.advance(Duration::from_secs(3600));
        // The real clock ignores the advance; it only moves with wall time, so
        // the jump is nowhere near an hour.
        assert!(clock.now().as_nanos() < before.as_nanos() + 1_000_000_000);
    }

    #[test]
    fn saturating_add_never_wraps_the_deadline_into_the_past() {
        let late = MonoTime(u64::MAX - 10);
        let deadline = late.saturating_add(Duration::from_secs(1));
        assert_eq!(deadline, MonoTime(u64::MAX));
        assert!(deadline >= late);
    }
}
