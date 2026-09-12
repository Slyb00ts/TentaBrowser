//! The event loop's clock (`docs/spec/05-javascript.md` §5).
//!
//! Time is a monotonic count of nanoseconds since the loop started. It is
//! monotonic in every mode, which `performance.now` requires. In headless runs
//! the clock can be *virtual*: it only moves when advanced explicitly
//! (`Emulation.setVirtualTimePolicy`), so timers, `Date.now` and animation
//! clocks are deterministic. A *real* clock reads the OS monotonic clock.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    Real {
        base: Instant,
        /// Unix-epoch milliseconds at `base`, which `performance.timeOrigin`
        /// reports. Read once, from the wall clock, because the wall clock can
        /// step backwards later and `timeOrigin` may not.
        epoch_ms: f64,
    },
    /// Advances only when told to, for deterministic headless runs.
    Virtual { now: MonoTime },
}

impl Clock {
    /// A clock backed by the OS monotonic clock.
    pub fn real() -> Self {
        Clock::Real {
            base: Instant::now(),
            epoch_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| since.as_secs_f64() * 1000.0)
                .unwrap_or(0.0),
        }
    }

    /// A virtual clock starting at `start`.
    pub fn virtual_at(start: MonoTime) -> Self {
        Clock::Virtual { now: start }
    }

    /// The current time.
    pub fn now(&self) -> MonoTime {
        match self {
            Clock::Real { base, .. } => {
                MonoTime(u64::try_from(base.elapsed().as_nanos()).unwrap_or(u64::MAX))
            }
            Clock::Virtual { now } => *now,
        }
    }

    /// Unix-epoch milliseconds at the loop origin, which `performance.timeOrigin`
    /// reports so that `timeOrigin + performance.now()` is a wall-clock reading.
    /// A virtual clock has no wall-clock anchor: it reports 0, leaving
    /// `performance.now()` itself the whole timestamp, which is what keeps a
    /// headless trace reproducible.
    pub fn time_origin_ms(&self) -> f64 {
        match self {
            Clock::Real { epoch_ms, .. } => *epoch_ms,
            Clock::Virtual { .. } => 0.0,
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

/// A clock handle shared by the event loop and everything that timestamps
/// against it.
///
/// `Event.timeStamp` and `performance.now()` are readings script compares
/// directly (`evt.timeStamp - performance.now()` is how a page measures
/// dispatch latency), so they must come from one clock. Holding separate
/// origins made the difference meaningless under a virtual clock, where one
/// side moved with the scheduler and the other with wall time.
#[derive(Clone, Debug)]
pub struct SharedClock(Rc<RefCell<Clock>>);

impl SharedClock {
    /// Wraps `clock` in a handle the loop and its timestamp consumers share.
    pub fn new(clock: Clock) -> Self {
        SharedClock(Rc::new(RefCell::new(clock)))
    }

    /// The current time.
    pub fn now(&self) -> MonoTime {
        self.0.borrow().now()
    }

    /// Unix-epoch milliseconds at the loop origin (`performance.timeOrigin`).
    pub fn time_origin_ms(&self) -> f64 {
        self.0.borrow().time_origin_ms()
    }

    /// Advances a virtual clock by `delta`; a no-op on a real clock.
    pub fn advance(&self, delta: Duration) {
        self.0.borrow_mut().advance(delta);
    }

    /// Moves a virtual clock forward to `target`, never backward; a no-op on a
    /// real clock.
    pub fn advance_to(&self, target: MonoTime) {
        self.0.borrow_mut().advance_to(target);
    }

    /// Whether this clock is virtual.
    pub fn is_virtual(&self) -> bool {
        self.0.borrow().is_virtual()
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
    fn a_real_clock_anchors_its_origin_to_the_wall_clock() {
        let clock = Clock::real();
        let wall_now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            * 1000.0;
        // `timeOrigin + performance.now()` must read as wall-clock time, so the
        // origin sits just behind the current wall clock, not at zero.
        let origin = clock.time_origin_ms();
        assert!(origin > 0.0);
        assert!(
            (wall_now - origin) < 1000.0,
            "origin {origin} vs {wall_now}"
        );
        assert!(origin + clock.now().as_millis_f64() <= wall_now + 1000.0);
    }

    #[test]
    fn a_virtual_clock_has_no_wall_clock_origin() {
        // Reproducibility: a headless trace must not embed the day it ran.
        assert_eq!(Clock::virtual_at(MonoTime::ZERO).time_origin_ms(), 0.0);
    }

    #[test]
    fn a_shared_clock_handle_sees_one_clock_from_every_holder() {
        let clock = SharedClock::new(Clock::virtual_at(MonoTime::ZERO));
        let timestamps = clock.clone();
        clock.advance(Duration::from_millis(40));
        // The point of the handle: a second holder reads the advance, so an
        // event timestamped through it is comparable with `performance.now()`.
        assert_eq!(timestamps.now().as_millis_f64(), 40.0);
        assert!(timestamps.is_virtual());
    }

    #[test]
    fn saturating_add_never_wraps_the_deadline_into_the_past() {
        let late = MonoTime(u64::MAX - 10);
        let deadline = late.saturating_add(Duration::from_secs(1));
        assert_eq!(deadline, MonoTime(u64::MAX));
        assert!(deadline >= late);
    }
}
