//! The page's cumulative script time (`docs/spec/05-javascript.md` §11,
//! P2 d-1b-7 stage (c)).
//!
//! A page may consume `Budgets::script_cpu` of script before it is considered
//! to have made no progress and is crashed. The deadline a single engine entry
//! gets — [`crate::runtime`]'s `bounded` arms one — is a *different* budget
//! (`evaluate_time`): it decides whether one callback is terminated, and a
//! terminated callback is the good outcome, the one that lets a page survive a
//! runaway. This module decides the other thing: whether the page as a whole
//! has spent the time it is allowed to spend at all.
//!
//! # Why the total is the page's and not the navigation's
//!
//! The spec gives a *page* a script CPU budget, not a navigation, and the
//! accumulator outlives a navigation to match. [`PageRuntime`] is rebuilt for
//! every navigation, so a total owned there would start at zero on every
//! `goto`: a page that navigates — or reloads itself in a loop — could spend a
//! fresh budget each time, and the budget is exactly the statement that such a
//! page has not made progress.
//!
//! [`PageRuntime`]: crate::runtime::PageRuntime

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// The page's cumulative script time, and the one decision that depends on it.
///
/// Shared behind `Arc` across the page's handles, so the accumulator is
/// interior-mutable and the type is `Send + Sync`. The page thread is the only
/// thread that ever charges it — every engine entry runs there — but a page
/// handle may be read from another thread, so there is no single-owner
/// shortcut to take.
pub(crate) struct ScriptCpu {
    /// The page's script budget. Zero means unbounded, the same convention
    /// [`PageRuntime::bounded`] uses for a timeout that arms nothing.
    ///
    /// [`PageRuntime::bounded`]: crate::runtime::PageRuntime::bounded
    budget: Duration,
    /// Milliseconds charged so far, saturating. An atomic because the type is
    /// shared; a lock would be heavier than a compare-exchange for the one
    /// word involved.
    spent_ms: AtomicU64,
    /// Whether the budget has already been crossed. A latch, so `charge`
    /// answers `true` exactly once however many later charges cross again:
    /// crashing the page is one decision, not one per crossing.
    tripped: AtomicBool,
}

impl ScriptCpu {
    /// A fresh accumulator against `budget`. A zero budget never trips.
    pub(crate) fn new(budget: Duration) -> ScriptCpu {
        ScriptCpu {
            budget,
            spent_ms: AtomicU64::new(0),
            tripped: AtomicBool::new(false),
        }
    }

    /// Adds `elapsed` to the total and reports whether this charge is the one
    /// that took it to or past the budget.
    ///
    /// `true` is returned exactly once, on the first crossing. Every later
    /// charge returns `false`, including one that crosses again — a page is
    /// crashed once. A zero budget always returns `false`.
    pub(crate) fn charge(&self, elapsed: Duration) -> bool {
        if self.budget.is_zero() {
            return false;
        }
        let elapsed_ms = millis_of(elapsed);
        // Saturating, not wrapping: the accumulator is a page-lifetime counter,
        // and a wrapped total would read as "spent almost nothing" and unlock a
        // budget that was in fact exhausted. The compare-exchange is what makes
        // two concurrent charges still add, rather than one overwriting the
        // other.
        let mut current = self.spent_ms.load(Ordering::Acquire);
        let total = loop {
            let next = current.saturating_add(elapsed_ms);
            match self.spent_ms.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break next,
                Err(seen) => current = seen,
            }
        };
        if total >= millis_of(self.budget) {
            return !self.tripped.swap(true, Ordering::AcqRel);
        }
        false
    }

    /// The total script time charged so far, for the crash message.
    pub(crate) fn spent(&self) -> Duration {
        Duration::from_millis(self.spent_ms.load(Ordering::Acquire))
    }

    /// The budget this accumulator enforces, for the crash message. Zero means
    /// unbounded.
    pub(crate) fn budget(&self) -> Duration {
        self.budget
    }
}

/// A duration in whole milliseconds, saturated rather than truncated to zero
/// at absurd magnitudes. `as_millis` is a `u128`, and a `Duration` can name
/// more milliseconds than a `u64` holds.
fn millis_of(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_crosses_exactly_once() {
        let cpu = ScriptCpu::new(Duration::from_millis(100));
        assert!(!cpu.charge(Duration::from_millis(60)), "under the budget");
        assert!(
            cpu.charge(Duration::from_millis(60)),
            "the charge that reaches the budget is the crossing"
        );
        assert_eq!(cpu.spent(), Duration::from_millis(120));
    }

    #[test]
    fn the_crossing_latches() {
        let cpu = ScriptCpu::new(Duration::from_millis(100));
        assert!(cpu.charge(Duration::from_millis(100)));
        for _ in 0..1000 {
            assert!(
                !cpu.charge(Duration::from_millis(10)),
                "the page crashes on the first crossing and no later one"
            );
        }
    }

    #[test]
    fn a_zero_budget_never_trips_and_records_nothing() {
        let cpu = ScriptCpu::new(Duration::ZERO);
        for _ in 0..1000 {
            assert!(!cpu.charge(Duration::from_secs(1)));
        }
        assert_eq!(
            cpu.spent(),
            Duration::ZERO,
            "an unbounded budget is not accumulated"
        );
    }

    #[test]
    fn the_total_saturates_at_huge_inputs() {
        let cpu = ScriptCpu::new(Duration::from_secs(1));
        // Two charges that would overflow a wrapping `u64` together. The
        // accumulator must clamp at the maximum rather than wrap back to zero,
        // which would read as "spent nothing" and unlock the budget.
        assert!(cpu.charge(Duration::from_millis(u64::MAX)));
        assert!(!cpu.charge(Duration::from_millis(u64::MAX)));
        assert_eq!(cpu.spent(), Duration::from_millis(u64::MAX));
    }
}
