//! Short, per-isolate handoff between watchdog requests and closed GC recovery.
//!
//! The mutex serializes state transitions with the actual V8 terminate/cancel
//! call. It is never held while preparing the graph, running GC, restoring
//! roots, invoking a host callback, or executing page JavaScript.

use std::sync::{Arc, Mutex, MutexGuard};

struct State {
    collecting: bool,
    requested: bool,
}

pub(crate) struct TerminationGate {
    handle: v8::IsolateHandle,
    state: Mutex<State>,
}

pub(crate) struct CollectionLease {
    gate: Arc<TerminationGate>,
}

impl TerminationGate {
    pub(crate) fn new(handle: v8::IsolateHandle) -> Arc<Self> {
        Arc::new(Self {
            handle,
            state: Mutex::new(State {
                collecting: false,
                requested: false,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A watchdog request may arrive while the graph is temporarily weakened.
    /// Record it then, and deliver the V8 interrupt after recovery finishes.
    pub(crate) fn request(&self) {
        let mut state = self.lock();
        state.requested = true;
        if !state.collecting {
            self.handle.terminate_execution();
        }
    }

    /// Clears a request that belongs to a completed earlier entry. Holding the
    /// same lock across cancel closes the reset/delayed-delivery race.
    pub(crate) fn reset(&self) {
        let _ = self.take_requested_and_reset();
    }

    /// Returns the request that affected this completed task while clearing
    /// its pending V8 interrupt in the same critical section.
    pub(crate) fn take_requested_and_reset(&self) -> bool {
        let mut state = self.lock();
        debug_assert!(!state.collecting);
        let requested = state.requested;
        state.requested = false;
        self.handle.cancel_terminate_execution();
        requested
    }

    pub(crate) fn requested(&self) -> bool {
        self.lock().requested
    }

    /// Refuses GC if a request already won. The lease ends recovery even when
    /// the Rust body unwinds; native poison is delivered separately and at once.
    pub(crate) fn begin_collection(self: &Arc<Self>) -> Option<CollectionLease> {
        let mut state = self.lock();
        if state.requested || state.collecting {
            return None;
        }
        state.collecting = true;
        Some(CollectionLease {
            gate: Arc::clone(self),
        })
    }

    fn end_collection(&self) {
        let mut state = self.lock();
        assert!(state.collecting, "GC recovery lease was not active");
        state.collecting = false;
        if state.requested {
            self.handle.terminate_execution();
        }
    }
}

impl Drop for CollectionLease {
    fn drop(&mut self) {
        self.gate.end_collection();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> (v8::OwnedIsolate, Arc<TerminationGate>) {
        crate::ensure_platform();
        let isolate = v8::Isolate::new(Default::default());
        let gate = TerminationGate::new(isolate.thread_safe_handle());
        (isolate, gate)
    }

    #[test]
    fn request_before_collection_refuses_and_reset_clears_it() {
        let (_isolate, gate) = gate();
        gate.request();
        assert!(gate.begin_collection().is_none());
        assert!(gate.take_requested_and_reset());
        assert!(gate.begin_collection().is_some());
    }

    #[test]
    fn request_during_collection_survives_until_handoff() {
        let (_isolate, gate) = gate();
        let lease = gate.begin_collection().expect("collector owns the handoff");
        gate.request();
        assert!(gate.requested());
        assert!(gate.begin_collection().is_none());
        drop(lease);
        assert!(gate.take_requested_and_reset());
        assert!(!gate.requested());
        assert!(gate.begin_collection().is_some());
    }

    #[test]
    fn request_after_recovery_is_seen_before_a_new_task() {
        let (_isolate, gate) = gate();
        drop(gate.begin_collection().expect("collector begins"));
        gate.request();
        assert!(gate.take_requested_and_reset());
        assert!(!gate.take_requested_and_reset());
    }
}
