//! `XMLHttpRequest`: the seam between a request a page makes through an XHR
//! object and the answer a network driver brings back.
//!
//! It is a sibling of [`crate::fetch`], not a reuse of it, because the two
//! completions are delivered differently. `fetch()` settles a promise, so its
//! completion names a resolver and the engine applies it as a microtask; an
//! XHR completion is an **event dispatch** at the object that made the request,
//! so its completion names the event target and the engine dispatches there.
//! The request and the answer are the same data either way, so
//! [`FetchRequest`], [`FetchOutcome`] and [`FetchStartError`] are reused
//! verbatim: the driver behind this trait is the same `tenta-net` code path,
//! and a second request type would only let the two drift.
//!
//! `target: u64` is opaque handle bits for the same reason `resolver: u64` is
//! — a driver must not be able to name a handle type, or the network tier would
//! have to depend on the engine tier instead of the other way round
//! (`docs/spec/02-architecture.md` §2). It is deliberately not a node: today it
//! is whatever the page's XHR object is keyed under, which b-5 makes an
//! `XMLHttpRequest` host record rather than an element.
//!
//! Neither direction does any work here. `start` only hands the request over,
//! and the table only stores what it is given: the scheduling belongs to the
//! driver, and applying a completion belongs to the engine.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::fetch::{FetchOutcome, FetchRequest, FetchStartError};

/// One answer waiting to be delivered: the event target it belongs to, and what
/// to dispatch there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XhrCompletion {
    /// The event target's handle bits, as the XHR object handed them to the
    /// driver when the request started.
    ///
    /// Opaque here: the engine is what turns them back into a dispatch target
    /// and is what refuses a handle that is not one.
    pub target: u64,
    /// The answer. Which event the outcome becomes is the engine's mapping, so
    /// a driver never has to know that a failure is an `error` event and a
    /// response is a `load`.
    pub outcome: FetchOutcome,
}

/// The completions a driver is waiting for the page to pick up.
///
/// The table lives on the page side and the driver holds a handle to it, which
/// is why it is an `Rc<RefCell<..>>` under a plain struct: the driver is called
/// from an XHR binding and the drain runs from the settle loop, so the two are
/// never on the stack together, but neither owns the other. Shaped exactly like
/// [`FetchCompletions`](crate::fetch::FetchCompletions) because the drain
/// contract is the same one.
#[derive(Default, Debug)]
pub struct XhrCompletions {
    queue: Rc<RefCell<VecDeque<XhrCompletion>>>,
}

impl XhrCompletions {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the answer to `target`. Called by a driver, from whatever task
    /// or thread it completes on — the table is not `Sync`, so a driver that
    /// completes off the page thread must hand the completion back to it first.
    ///
    /// See [`XhrDriver::start`] for what the caller owes in addition to this.
    pub fn push(&self, target: u64, outcome: FetchOutcome) {
        self.queue
            .borrow_mut()
            .push_back(XhrCompletion { target, outcome });
    }

    /// Takes every completion recorded so far, in the order they were pushed,
    /// and empties the table.
    ///
    /// Draining rather than borrowing is what keeps the drain free of an
    /// engine-boundary violation: a caller does not hold the table's borrow
    /// while the engine dispatches, which re-enters Rust (`crate::engine`).
    pub fn take_all(&self) -> Vec<XhrCompletion> {
        self.queue.borrow_mut().drain(..).collect()
    }
}

/// Answers the `XMLHttpRequest` objects of one page.
///
/// One driver is installed per navigation
/// ([`PageEngine::set_xhr_driver`](crate::engine::PageEngine::set_xhr_driver)),
/// so a completion it leaves behind always belongs to the document that is
/// current when the completion is drained.
///
/// # The lifetime contract
///
/// `start` returns as soon as the request is under way; the response arrives
/// later, and a driver that completes one must do **two** things — push the
/// completion into the [`XhrCompletions`] table it was given, and wake the
/// page, so the settle loop takes another turn and drains the table. The push
/// alone is not observable: the drain runs from the loop's turn
/// (`tenta_page::runtime`), so a completion nothing wakes for stays in the
/// table, and the page's XHR object stays in its `LOADING` state, until
/// something else happens to wake the page.
///
/// The same hold `fetch` relies on keeps the settle open while the request is
/// in flight: a tracked foreground request holds the settle
/// (`RequestRecord::holds_settle` in `tenta-page`), so 4a-1b's push-then-wake
/// order is this trait's contract too. Unlike `fetch`, a refusal from `start`
/// is not the only failure a page can observe — the driver that lands in b-6
/// registers the request *before* evaluating its cross-origin gate, so a
/// refused request still completes with a failure outcome rather than
/// disappearing.
pub trait XhrDriver {
    /// Takes `request`, to be answered at `target` when it completes.
    ///
    /// `Err` refuses the request before any of it is sent: the caller turns the
    /// message into the failure the page observes, rather than leaving the XHR
    /// object waiting for a driver that was never asked. A refusal is checked
    /// here rather than in the caller because only the driver knows whether it
    /// can serve the request — a cross-origin URL is the first case.
    fn start(&self, request: FetchRequest, target: u64) -> Result<(), FetchStartError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(message: &str, target: u64) -> XhrCompletion {
        XhrCompletion {
            target,
            outcome: FetchOutcome::Failed(message.to_owned()),
        }
    }

    #[test]
    fn take_all_hands_the_completions_back_in_push_order() {
        let table = XhrCompletions::new();
        table.push(7, FetchOutcome::Failed("first".to_owned()));
        table.push(1, FetchOutcome::Failed("second".to_owned()));
        table.push(4, FetchOutcome::Failed("third".to_owned()));
        assert_eq!(
            table.take_all(),
            vec![failed("first", 7), failed("second", 1), failed("third", 4)],
        );
    }

    #[test]
    fn take_all_empties_the_table() {
        let table = XhrCompletions::new();
        table.push(1, FetchOutcome::Failed("only".to_owned()));
        assert_eq!(table.take_all().len(), 1);
        assert!(
            table.take_all().is_empty(),
            "a second drain finds nothing, so a completion cannot be dispatched twice"
        );
    }

    #[test]
    fn an_empty_table_drains_to_nothing() {
        assert!(XhrCompletions::new().take_all().is_empty());
    }

    #[test]
    fn a_completion_pushed_after_a_drain_is_delivered_by_the_next_one() {
        let table = XhrCompletions::new();
        table.push(1, FetchOutcome::Failed("before".to_owned()));
        assert_eq!(table.take_all(), vec![failed("before", 1)]);
        table.push(2, FetchOutcome::Failed("after".to_owned()));
        assert_eq!(
            table.take_all(),
            vec![failed("after", 2)],
            "the drain is a boundary, not a subscription"
        );
    }
}
