//! The page's script CPU rate limit (`docs/spec/05-javascript.md` §11,
//! `docs/spec/18-decisions.md` D-033).
//!
//! A page may spend `Budgets::script_cpu` of script time inside any sliding
//! [`SCRIPT_CPU_WINDOW`] of wall-clock time before it is considered to have
//! made no progress and is crashed. The deadline a single engine entry gets —
//! [`crate::runtime`]'s `bounded` arms one — is a *different* limit
//! (`evaluate_time`): it decides whether one callback is terminated, and a
//! terminated callback is the good outcome, the one that lets a page survive a
//! runaway. This module decides the other thing: whether the page as a whole
//! keeps the thread busy for too large a share of the time.
//!
//! # Why a rate and not a lifetime total
//!
//! A page keeps its engine after `load`, so a lifetime total would eventually
//! crash every long-lived page however idle it is: a timer that costs 1 ms every
//! second reaches any finite cap. A rate limit separates a page that is merely
//! old from one that is saturating its thread.
//!
//! # Why the total is the page's and not the navigation's
//!
//! The spec gives a *page* a script CPU budget, not a navigation, and the
//! accumulator outlives a navigation to match. [`PageRuntime`] is rebuilt for
//! every navigation, so a window owned there would start empty on every `goto`:
//! a page that navigates — or reloads itself in a loop — could spend a fresh
//! budget each time.
//!
//! [`PageRuntime`]: crate::runtime::PageRuntime

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The sliding window the budget is measured over. Thirty seconds is the
/// default `evaluate` deadline's order of magnitude (`05` §11): long enough that
/// a burst of legitimate work such as a hydration does not count as saturation,
/// short enough that a page spinning its thread is stopped within the window.
pub(crate) const SCRIPT_CPU_WINDOW: Duration = Duration::from_secs(30);

/// Charges closer together than this share one entry, which bounds the window's
/// memory at `SCRIPT_CPU_WINDOW / BUCKET` entries however many timers fire.
const BUCKET: Duration = Duration::from_millis(100);

/// The page's script time inside the window, and the one decision that depends
/// on it.
///
/// Shared behind `Arc` across the page's handles, so it is `Send + Sync`. The
/// page thread is the only thread that ever charges it, but a page handle may be
/// read from another thread.
pub(crate) struct ScriptCpu {
    /// Script time a page may spend per window. Zero means unbounded, the same
    /// convention [`PageRuntime::bounded`] uses for a timeout that arms nothing.
    ///
    /// [`PageRuntime::bounded`]: crate::runtime::PageRuntime::bounded
    budget: Duration,
    window: Duration,
    /// `(bucket start, time charged in the bucket)`, oldest first.
    charges: Mutex<VecDeque<(Instant, Duration)>>,
    /// Whether the budget has already been crossed. A latch, so `charge`
    /// answers `true` exactly once: crashing the page is one decision.
    tripped: AtomicBool,
}

impl ScriptCpu {
    /// A fresh limit of `budget` per `window`. A zero budget never trips.
    pub(crate) fn new(budget: Duration, window: Duration) -> ScriptCpu {
        ScriptCpu {
            budget,
            window,
            charges: Mutex::new(VecDeque::new()),
            tripped: AtomicBool::new(false),
        }
    }

    /// [`Self::charge_at`] for an entry that ends now.
    pub(crate) fn charge(&self, elapsed: Duration) -> bool {
        self.charge_at(Instant::now(), elapsed)
    }

    /// Records `elapsed` of script time ending at `now` and reports whether this
    /// charge is the one that took the window to or past the budget.
    ///
    /// `true` is returned exactly once. A zero budget always returns `false`
    /// and records nothing.
    pub(crate) fn charge_at(&self, now: Instant, elapsed: Duration) -> bool {
        if self.budget.is_zero() {
            return false;
        }
        let mut charges = self.lock();
        Self::expire(&mut charges, now, self.window);
        match charges.back_mut() {
            Some((start, spent)) if now.saturating_duration_since(*start) < BUCKET => {
                *spent = spent.saturating_add(elapsed);
            }
            _ => charges.push_back((now, elapsed)),
        }
        let total = Self::total(&charges);
        drop(charges);
        total >= self.budget && !self.tripped.swap(true, Ordering::AcqRel)
    }

    /// The script time inside the window ending now, for the crash message.
    pub(crate) fn spent(&self) -> Duration {
        let mut charges = self.lock();
        Self::expire(&mut charges, Instant::now(), self.window);
        Self::total(&charges)
    }

    /// The budget per window. Zero means unbounded.
    pub(crate) fn budget(&self) -> Duration {
        self.budget
    }

    /// The window the budget is measured over.
    pub(crate) fn window(&self) -> Duration {
        self.window
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<(Instant, Duration)>> {
        self.charges
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Drops the buckets that ended before the window began.
    fn expire(charges: &mut VecDeque<(Instant, Duration)>, now: Instant, window: Duration) {
        while let Some((start, _)) = charges.front() {
            if now.saturating_duration_since(*start) >= window {
                charges.pop_front();
            } else {
                break;
            }
        }
    }

    /// Saturating, so an absurd charge reads as "over budget" rather than
    /// wrapping back to "spent nothing".
    fn total(charges: &VecDeque<(Instant, Duration)>) -> Duration {
        charges
            .iter()
            .fold(Duration::ZERO, |sum, (_, spent)| sum.saturating_add(*spent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(budget_ms: u64) -> (ScriptCpu, Instant) {
        (
            ScriptCpu::new(Duration::from_millis(budget_ms), SCRIPT_CPU_WINDOW),
            Instant::now(),
        )
    }

    #[test]
    fn a_budget_crosses_exactly_once() {
        let (cpu, start) = cpu(100);
        assert!(
            !cpu.charge_at(start, Duration::from_millis(60)),
            "under the budget"
        );
        assert!(
            cpu.charge_at(start, Duration::from_millis(60)),
            "the charge that reaches the budget is the crossing"
        );
        assert_eq!(cpu.spent(), Duration::from_millis(120));
    }

    #[test]
    fn the_crossing_latches() {
        let (cpu, start) = cpu(100);
        assert!(cpu.charge_at(start, Duration::from_millis(100)));
        for _ in 0..1000 {
            assert!(
                !cpu.charge_at(start, Duration::from_millis(10)),
                "the page crashes on the first crossing and no later one"
            );
        }
    }

    #[test]
    fn a_zero_budget_never_trips_and_records_nothing() {
        let (_, start) = cpu(0);
        let cpu = ScriptCpu::new(Duration::ZERO, SCRIPT_CPU_WINDOW);
        for _ in 0..1000 {
            assert!(!cpu.charge_at(start, Duration::from_secs(1)));
        }
        assert_eq!(
            cpu.spent(),
            Duration::ZERO,
            "an unbounded budget is not accumulated"
        );
    }

    #[test]
    fn the_total_saturates_at_huge_inputs() {
        let (cpu, start) = cpu(1000);
        // Two charges that would overflow a wrapping sum together must clamp
        // rather than wrap back to "spent nothing".
        assert!(cpu.charge_at(start, Duration::MAX));
        assert!(!cpu.charge_at(start, Duration::MAX));
        assert_eq!(cpu.spent(), Duration::MAX);
    }

    /// A page that costs 100 ms of script every second is a 10% duty page. Over
    /// ten virtual minutes it spends 60 s — the cap the lifetime total used to
    /// impose — and stays inside a 15 s window budget the whole time.
    #[test]
    fn a_long_low_duty_page_outlives_any_lifetime_total() {
        let cpu = ScriptCpu::new(Duration::from_secs(15), SCRIPT_CPU_WINDOW);
        let start = Instant::now();
        let mut lifetime = Duration::ZERO;
        for second in 0..600u64 {
            let now = start + Duration::from_secs(second);
            assert!(
                !cpu.charge_at(now, Duration::from_millis(100)),
                "tripped at virtual second {second}"
            );
            lifetime += Duration::from_millis(100);
        }
        assert!(lifetime >= Duration::from_secs(60));
    }

    /// A page that keeps its thread 80% busy spends 24 s of a 30 s window and is
    /// stopped within the window.
    #[test]
    fn a_sustained_high_duty_page_is_stopped_within_the_window() {
        let cpu = ScriptCpu::new(Duration::from_secs(15), SCRIPT_CPU_WINDOW);
        let start = Instant::now();
        let tripped_at = (0..600u64).find(|&tick| {
            // 800 ms of script in every second, charged as it ends.
            let now = start + Duration::from_millis(tick * 1000);
            cpu.charge_at(now, Duration::from_millis(800))
        });
        let tripped_at = tripped_at.expect("a saturating page trips");
        assert!(
            tripped_at < 30,
            "it was stopped after {tripped_at} s, inside the 30 s window"
        );
    }

    #[test]
    fn a_burst_ages_out_of_the_window() {
        let (cpu, start) = cpu(1000);
        assert!(!cpu.charge_at(start, Duration::from_millis(900)));
        let later = start + SCRIPT_CPU_WINDOW + Duration::from_secs(1);
        assert!(
            !cpu.charge_at(later, Duration::from_millis(900)),
            "the first burst left the window"
        );
    }
}
