//! The process-wide watchdog thread (`docs/spec/02-architecture.md` §8,
//! `docs/spec/05-javascript.md` §11).
//!
//! One thread serves every engine in the process. The spec's thread inventory
//! names a single `watchdog thread` — "terminates JS engines and enforces page
//! deadlines" — and both places that describe termination speak of *the*
//! watchdog thread, so this is not one per page and not one per
//! [`PageRuntime`](crate::runtime): a page thread blocked inside an engine
//! entry has no thread left to run a watchdog of its own, which is exactly the
//! state this exists for.
//!
//! # Why arming is concurrent-safe
//!
//! One thread serves many page threads, so two pages arm at once and neither
//! may lose its deadline. Every [`arm`] therefore gets an identity of its own
//! and its own `fired` flag, and the registry is a map keyed by that identity.
//! A single slot would be wrong for the reason the whole module exists: the
//! second page to arm would overwrite the first page's deadline *and its
//! handle*, and the first page would then have no terminate coming at all while
//! its own thread sat blocked in a callback that never returns.
//!
//! # What a fire does, and what it does not
//!
//! `terminate` is best effort and idempotent: the engine may be between
//! scripts, or the entry may have returned since the deadline passed. A fire
//! therefore only marks the deadline as elapsed — it never removes the entry,
//! because the page still has to read [`Arm::fired`] after the entry returns.
//! Removing is the [`Arm`]'s job, on drop. The pair is a guard rather than two
//! calls on purpose, for the reason `crate::settle::PendingScripts` states: an
//! error path between an arm and a disarm would otherwise leak the deadline
//! forever.
//!
//! `terminate` is a foreign call into an engine's thread-safe handle and can
//! block on the isolate lock the page thread is holding. The thread therefore
//! never holds its own lock across one: it collects what is due under the lock,
//! releases it, and only then fires.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use tenta_js::engine::EngineWatchdog;

/// A deadline armed on the watchdog thread.
///
/// Dropping it disarms the deadline: the entry leaves the registry and will not
/// be fired again. [`Arm::fired`] stays readable after a fire and after the
/// entry is gone, because the flag belongs to this guard, not to the registry.
pub(crate) struct Arm {
    id: ArmId,
    fired: Arc<AtomicBool>,
}

impl Arm {
    /// Whether this arm's deadline elapsed and the watchdog asked the engine to
    /// stop. Reading it is how a page learns that an engine entry was cut short
    /// rather than finishing on its own.
    pub(crate) fn fired(&self) -> bool {
        self.fired.load(Ordering::Acquire)
    }
}

impl Drop for Arm {
    fn drop(&mut self) {
        let registry = registry();
        registry
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.id);
        // The earliest live deadline may have been this arm's, so the thread
        // must look again rather than keep waiting for a deadline that is gone.
        registry.wake.notify_all();
    }
}

/// Identity of one armed deadline, from a process-wide counter so two arms can
/// never share one even when their deadlines and handles are identical.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct ArmId(u64);

/// One armed deadline as the watchdog thread sees it.
struct ArmEntry {
    deadline: Instant,
    handle: Arc<dyn EngineWatchdog>,
    /// Set exactly once, by the thread that fires this entry, before it calls
    /// `terminate`. Shared with the [`Arm`] the page holds.
    fired: Arc<AtomicBool>,
}

/// What the watchdog thread waits on and reads.
struct Registry {
    entries: Mutex<HashMap<ArmId, ArmEntry>>,
    /// Signalled whenever the set of armed deadlines changes, so a thread
    /// waiting on the earliest one looks again.
    wake: Condvar,
    next_id: AtomicU64,
}

/// Arms `handle` to be terminated when `deadline` passes.
///
/// The returned [`Arm`] owns the deadline: dropping it disarms. A deadline
/// already in the past fires on the thread's next look, but that is *not* a
/// substitute for a pre-entry check — a terminate that lands before the entry
/// starts is discarded by the entry's own start-of-entry clear
/// (`crates/tenta-js-v8`, stage (a)), leaving the entry to run unbounded. A
/// caller whose budget has already elapsed must decide before arming, never
/// here.
pub(crate) fn arm(handle: Arc<dyn EngineWatchdog>, deadline: Instant) -> Arm {
    let registry = registry();
    let id = ArmId(registry.next_id.fetch_add(1, Ordering::Relaxed));
    let fired = Arc::new(AtomicBool::new(false));
    registry
        .entries
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            id,
            ArmEntry {
                deadline,
                handle,
                fired: Arc::clone(&fired),
            },
        );
    registry.wake.notify_all();
    Arm { id, fired }
}

/// The process-wide watchdog registry, started on first use.
fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<&'static Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        // Leaked deliberately: the registry lives for the whole process and the
        // thread below borrows this reference for as long as it runs, so there
        // is no point at which dropping it would be right.
        let registry: &'static Registry = Box::leak(Box::new(Registry {
            entries: Mutex::new(HashMap::new()),
            wake: Condvar::new(),
            next_id: AtomicU64::new(0),
        }));
        thread::Builder::new()
            .name("tenta-watchdog".to_owned())
            .spawn(move || serve(registry))
            .expect("spawn the watchdog thread");
        registry
    })
}

/// Fires every expired deadline, then waits for the earliest live one.
///
/// The loop only ever ends with the process.
fn serve(registry: &'static Registry) -> ! {
    loop {
        let due = {
            let entries = registry
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let now = Instant::now();
            let mut due: Vec<Arc<dyn EngineWatchdog>> = Vec::new();
            for entry in entries.values() {
                // Swapping rather than reading is what fires each entry exactly
                // once: a later look sees `true` and skips it.
                if entry.deadline <= now && !entry.fired.swap(true, Ordering::AcqRel) {
                    due.push(Arc::clone(&entry.handle));
                }
            }
            if due.is_empty() {
                // An entry that has already fired is not a reason to wake
                // again, so only unfired deadlines count here. Waiting on a
                // fired one would spin until its `Arm` dropped.
                match earliest_unfired(&entries) {
                    Some(deadline) => {
                        let wait = deadline.saturating_duration_since(Instant::now());
                        drop(registry.wake.wait_timeout(entries, wait));
                    }
                    None => drop(registry.wake.wait(entries)),
                }
            }
            due
        };
        // Outside the lock: `terminate` reaches into the engine and can block
        // on the isolate lock a page thread holds, and holding our lock across
        // it would stop every other page's deadline from firing too.
        for handle in due {
            tracing::debug!("a watchdog deadline elapsed; terminating the engine entry");
            handle.terminate();
        }
    }
}

/// The earliest deadline that has not fired yet, if any.
fn earliest_unfired(entries: &HashMap<ArmId, ArmEntry>) -> Option<Instant> {
    entries
        .values()
        .filter(|entry| !entry.fired.load(Ordering::Acquire))
        .map(|entry| entry.deadline)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    /// Counts the terminates it is asked for, so a test can tell a fire from an
    /// absence of one.
    #[derive(Default)]
    struct RecordingWatchdog {
        fires: AtomicUsize,
    }

    impl RecordingWatchdog {
        fn fires(&self) -> usize {
            self.fires.load(Ordering::Acquire)
        }
    }

    impl EngineWatchdog for RecordingWatchdog {
        fn terminate(&self) {
            self.fires.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Waits until `condition` holds, or fails the test rather than hanging.
    fn wait_until(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "the watchdog thread did not fire within the test's patience"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_deadline_fires_once_and_stays_readable() {
        let handle = Arc::new(RecordingWatchdog::default());
        let arm = arm(
            Arc::clone(&handle) as Arc<dyn EngineWatchdog>,
            Instant::now() + Duration::from_millis(10),
        );

        wait_until(|| arm.fired());
        wait_until(|| handle.fires() == 1);
        // The entry is still registered, so a second look must not fire it
        // again: `terminate` is a foreign call and must not be spammed.
        thread::sleep(Duration::from_millis(20));
        assert_eq!(handle.fires(), 1, "each deadline fires exactly once");
    }

    /// The property a single global slot would break. Two arms are live at the
    /// same time with different deadlines; both handles must be terminated, and
    /// the later arm must not have displaced the earlier one.
    #[test]
    fn two_live_arms_do_not_clobber_each_other() {
        let early = Arc::new(RecordingWatchdog::default());
        let late = Arc::new(RecordingWatchdog::default());
        // The early deadline is already in the past by the time the late one is
        // armed, so a single-slot registry would replace it before it fired.
        let early_arm = arm(
            Arc::clone(&early) as Arc<dyn EngineWatchdog>,
            Instant::now() + Duration::from_millis(5),
        );
        thread::sleep(Duration::from_millis(20));
        let late_arm = arm(
            Arc::clone(&late) as Arc<dyn EngineWatchdog>,
            Instant::now() + Duration::from_millis(5),
        );

        wait_until(|| early_arm.fired() && late_arm.fired());
        assert_eq!(early.fires(), 1, "the early arm kept its own deadline");
        assert_eq!(late.fires(), 1, "the late arm fired too");
    }
}
