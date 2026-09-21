//! `XMLHttpRequest`: the seam between a request a page makes through an XHR
//! object and the answer a network driver brings back.
//!
//! It is a sibling of [`crate::fetch`], not a reuse of it, because the two
//! completions are delivered differently. `fetch()` settles a promise, so its
//! completion names a resolver and the engine applies it as a microtask; an
//! XHR completion is an **event dispatch** at the object that made the request,
//! so its completion names the event target and the engine dispatches there.
//! The request and the answer are the same data either way, so
//! [`FetchOutcome`] and [`FetchStartError`] are reused verbatim: the driver
//! behind this trait is the same `tenta-net` code path, and a second answer
//! type would only let the two drift. The *request* is where the two part
//! company, because its two halves come from different sides of the callback
//! boundary — [`XhrRequest`] is the page's half, and the backend turns it into
//! a [`FetchRequest`] once it has added the engine's.
//!
//! [`XhrHandle`] is opaque bits for the same reason `resolver: u64` is — a
//! driver must not be able to name a handle type, or the network tier would
//! have to depend on the engine tier instead of the other way round
//! (`docs/spec/02-architecture.md` §2). It is deliberately not a node: its
//! `target` half is whatever the page's XHR object is keyed under, which is an
//! `XMLHttpRequest` host record rather than an element, and its `request` half
//! is the object's own identity for one request, which is what lets a
//! completion that belongs to a request the object has abandoned be discarded
//! instead of delivered (`deliver_xhr_completion` in the bindings).
//! [`XhrDriver::abort`] addresses a request by the same two halves — the target
//! is the object a driver would answer, and the request half is which of that
//! object's requests is meant — so the handle is one identity for a request in
//! both directions.
//!
//! Nothing here does any work. `start` only hands the request over, `abort` only
//! says which request to end, and the table only stores what it is given: the
//! scheduling belongs to the driver, and applying a completion belongs to the
//! engine.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use tenta_core::Origin;
use url::Url;

use crate::fetch::{FetchOutcome, FetchRequest, FetchStartError, header_pairs};

/// One request an `XMLHttpRequest` object made, ready to become a
/// [`FetchRequest`].
///
/// It exists because the two halves of a request come from different sides of
/// the callback boundary. The page's halves — the method, the URL and the
/// author request headers — are the host layer's, which is what builds this; the
/// request's *provenance* is the engine's, because [`Origin`] is the task's and
/// the scheduler that knows it is the backend's. So the carrier holds
/// everything **except** the origin, and the backend adds it
/// ([`Self::into_fetch_request`]).
///
/// The header halves are byte sequences rather than `String`s, unlike
/// [`FetchRequest`]'s: a `ByteString` value may hold obs-text, and a header list
/// that had already decoded it would have lost the distinction between a byte
/// and its replacement before any driver could see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XhrRequest {
    /// The *request URL*, resolved against the document by `open()`.
    pub url: Url,
    /// The *request method*, already validated and normalized by `open()`.
    pub method: http::Method,
    /// The *author request headers*: names and values as `setRequestHeader`
    /// combined them. The names are byte-lowercased, like every header name this
    /// engine stores. `send()` appends a `Content-Type` to this list when the
    /// body inferred one and the author set none.
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    /// The *request body*, once `send()` has converted the argument and dropped
    /// it for a `GET` or a `HEAD`; `None` is the standard's null body.
    ///
    /// It arrives here as bytes rather than as a source, because the conversion
    /// is the host layer's and its result is what the request carries: the two
    /// steps that come between — the header list `setRequestHeader` built and the
    /// `Content-Type` the body infers — are settled before this is built.
    pub body: Option<Vec<u8>>,
    /// The *initiator*: the document's URL, or the request's when the document
    /// has none. It is what a driver sends as the `Referer` and what a
    /// same-origin test is made against.
    pub initiator: Url,
}

impl XhrRequest {
    /// The request a network driver is handed, once the backend has supplied the
    /// provenance this side cannot name.
    ///
    /// The body crosses as it is: `tenta-net` derives `Content-Length` from the
    /// bytes itself, so a length is never authored here — it is a *forbidden
    /// request-header name*, and a second one would be a duplicate rather than a
    /// preference.
    ///
    /// The header conversion is this conversion's one lossy step, and it is
    /// [`header_pairs`] — the same function `fetch()`'s initializer reaches its
    /// headers through, so the loss is described in one place rather than two.
    pub fn into_fetch_request(self, origin: Origin) -> FetchRequest {
        let headers = header_pairs(self.headers);
        FetchRequest {
            url: self.url,
            method: self.method,
            headers,
            body: self.body,
            initiator: self.initiator,
            origin,
        }
    }
}

/// One request's identity: the object that will receive its answer, and which
/// of that object's requests the answer belongs to.
///
/// Both halves are opaque `u64`s for the reason `resolver: u64` is: a driver
/// must not be able to name a handle type or a record, or the network tier
/// would have to depend on the engine tier instead of the other way round
/// (`docs/spec/02-architecture.md` §2).
///
/// The second half is what keeps an object that abandoned a request from
/// receiving it anyway. An `XMLHttpRequest` stays at `OPENED` for as long as
/// its request is unanswered, so `open()` is reachable with a live request and
/// leaves two of them in flight on one object; the request id is stamped by the
/// `send()` that started a request and carried back here, so the host layer can
/// tell the object's *current* request from the one it abandoned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XhrHandle {
    /// The event target's handle bits, as the XHR object handed them to the
    /// driver when the request started.
    ///
    /// Opaque here: the engine is what turns them back into a dispatch target
    /// and is what refuses a handle that is not one.
    pub target: u64,
    /// The *request id*: the identity `send()` stamped on this request.
    ///
    /// The engine compares it against the id the object currently holds, and
    /// discards a completion that does not match — see
    /// [`crate::bindings::host::XhrRecord::request_id`]. It is also how
    /// [`XhrDriver::abort`] names the request to end: `open()`, `send()` and
    /// `abort()` all move the id on, so only the object's *current* request can
    /// be addressed.
    pub request: u64,
}

/// One answer waiting to be delivered: the request it belongs to, and what to
/// dispatch there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XhrCompletion {
    /// The object to deliver at, and the request of that object the answer
    /// belongs to.
    ///
    /// Opaque here: the engine is what turns the target into a dispatch target
    /// and is what refuses one that is not.
    pub handle: XhrHandle,
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

    /// Records the answer to `handle`. Called by a driver, from whatever task
    /// or thread it completes on — the table is not `Sync`, so a driver that
    /// completes off the page thread must hand the completion back to it first.
    ///
    /// See [`XhrDriver::start`] for what the caller owes in addition to this.
    pub fn push(&self, handle: XhrHandle, outcome: FetchOutcome) {
        self.queue
            .borrow_mut()
            .push_back(XhrCompletion { handle, outcome });
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
/// order is this trait's contract too.
///
/// A refusal from `start` is not a request left in the air: the caller turns it
/// into the standard's *request error steps* synchronously, so a refused
/// request is over before `send()` returns.
///
/// **That turn is this engine's, not the standard's.** The standard runs
/// `send()`'s fetch *in parallel*, so a network error reaches the page after
/// `send()` has returned; a refusal is a start-time decision made on the calling
/// turn and the caller runs the error steps there. The order a page observes is
/// the standard's — `loadstart`, then `error`, then `loadend` — and only the
/// turn they land on differs. CORS landing would make the cross-origin case a
/// network error like any other and the difference would go with it;
/// `a_cross_origin_xhr_is_refused_before_it_reaches_the_other_origin` in
/// `tenta-browser/tests/xhr_js.rs` pins it so it cannot change unnoticed.
///
/// What a driver must never do is accept a request and then neither push nor
/// wake for it: that leaves the object waiting and holds the settle until the
/// page's own deadline.
///
/// # Ending a request early
///
/// [`XhrDriver::abort`] is the other direction: the page can end a request it
/// started, before any answer exists, and the driver has to stop the work rather
/// than leave it running for an answer nothing will use. It is the
/// `XMLHttpRequest.abort()` method's first step ("Abort this's fetch
/// controller", <https://xhr.spec.whatwg.org/#the-abort()-method>) and
/// `open()`'s step 10, which terminates the request a re-opened object walks
/// away from.
///
/// The engine's own guard against a late answer is separate and stays: the
/// object's request id moves on when it ends a request, so a completion that
/// crosses the seam anyway is discarded (`XhrRecord::request_id`,
/// `deliver_xhr_completion`). A driver that has already pushed a completion
/// before the abort reached it therefore needs no rollback — the engine is what
/// refuses it.
pub trait XhrDriver {
    /// Takes `request`, to be answered at `handle` when it completes.
    ///
    /// `Err` refuses the request before any of it is sent: the caller turns the
    /// message into the failure the page observes, rather than leaving the XHR
    /// object waiting for a driver that was never asked. A refusal is checked
    /// here rather than in the caller because only the driver knows whether it
    /// can serve the request — a cross-origin URL is the first case.
    fn start(&self, request: FetchRequest, handle: XhrHandle) -> Result<(), FetchStartError>;

    /// Ends the request `handle` names, reporting whether a request was still
    /// running under it.
    ///
    /// `false` means **nothing was terminated**, for any reason: no request was
    /// ever started under this handle, the one that was has already finished, or
    /// the driver does not hold it any more. That is a normal outcome rather
    /// than an error — `abort()` may run on an object with nothing in flight —
    /// so the caller does not branch on it; the engine's discard is what
    /// guarantees no answer arrives either way.
    ///
    /// Ending a request is not the same as answering it: this pushes no
    /// completion and does not wake the page. A driver that has an answer in
    /// hand when this arrives may deliver it as usual — the engine's request id
    /// is what makes it invisible.
    fn abort(&self, handle: XhrHandle) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failing completion for `target`, belonging to request `request`.
    fn hand(message: &str, target: u64, request: u64) -> XhrCompletion {
        XhrCompletion {
            handle: XhrHandle { target, request },
            outcome: FetchOutcome::Failed(message.to_owned()),
        }
    }

    /// A handle for `target`, belonging to request `request`.
    fn handle(target: u64, request: u64) -> XhrHandle {
        XhrHandle { target, request }
    }

    #[test]
    fn take_all_hands_the_completions_back_in_push_order() {
        let table = XhrCompletions::new();
        table.push(handle(7, 1), FetchOutcome::Failed("first".to_owned()));
        table.push(handle(1, 2), FetchOutcome::Failed("second".to_owned()));
        table.push(handle(4, 3), FetchOutcome::Failed("third".to_owned()));
        assert_eq!(
            table.take_all(),
            vec![
                hand("first", 7, 1),
                hand("second", 1, 2),
                hand("third", 4, 3),
            ],
        );
    }

    #[test]
    fn take_all_empties_the_table() {
        let table = XhrCompletions::new();
        table.push(handle(1, 1), FetchOutcome::Failed("only".to_owned()));
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
        table.push(handle(1, 1), FetchOutcome::Failed("before".to_owned()));
        assert_eq!(table.take_all(), vec![hand("before", 1, 1)]);
        table.push(handle(2, 2), FetchOutcome::Failed("after".to_owned()));
        assert_eq!(
            table.take_all(),
            vec![hand("after", 2, 2)],
            "the drain is a boundary, not a subscription"
        );
    }

    /// Both halves of a handle survive the table unchanged: the target bits an
    /// engine decodes back into a dispatch target, and the request id it
    /// compares against the object's own.
    #[test]
    fn a_handle_keeps_both_halves() {
        let table = XhrCompletions::new();
        table.push(
            handle(0x0A_00_00_07, 9),
            FetchOutcome::Failed("x".to_owned()),
        );
        let delivered = table.take_all().pop().expect("one completion");
        assert_eq!(delivered.handle.target, 0x0A_00_00_07);
        assert_eq!(delivered.handle.request, 9);
    }

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a test URL")
    }

    #[test]
    fn a_carrier_becomes_the_request_the_driver_is_handed() {
        let request = XhrRequest {
            url: url("http://fixture.test/data?x=1"),
            method: http::Method::POST,
            headers: vec![(b"x-token".to_vec(), b"abc".to_vec())],
            body: Some(b"a=1&b=2".to_vec()),
            initiator: url("http://fixture.test/dir/page.html"),
        };

        let converted = request.into_fetch_request(Origin::Background);

        assert_eq!(converted.url, url("http://fixture.test/data?x=1"));
        assert_eq!(converted.method, http::Method::POST);
        assert_eq!(
            converted.headers,
            vec![("x-token".to_owned(), "abc".to_owned())],
            "the page's headers reach the driver"
        );
        assert_eq!(
            converted.body,
            Some(b"a=1&b=2".to_vec()),
            "the bytes `send()` converted reach the driver unchanged"
        );
        assert_eq!(
            converted.initiator,
            url("http://fixture.test/dir/page.html")
        );
        assert_eq!(
            converted.origin,
            Origin::Background,
            "the provenance is the caller's, not the carrier's"
        );
    }

    /// The null body crosses as `None` rather than as an empty body, because the
    /// two are not the same request: `tenta-net` derives a `Content-Length` from
    /// the bytes it is given, and a request with no body must carry none.
    #[test]
    fn a_carrier_with_no_body_becomes_a_request_with_no_body() {
        let request = XhrRequest {
            url: url("http://fixture.test/data"),
            method: http::Method::GET,
            headers: Vec::new(),
            body: None,
            initiator: url("http://fixture.test/data"),
        };

        let converted = request.into_fetch_request(Origin::Tracked);

        assert!(converted.body.is_none());
        assert!(
            converted.headers.is_empty(),
            "no `Content-Type` was inferred"
        );
    }

    #[test]
    fn a_header_value_that_is_not_utf_8_decodes_with_replacement() {
        // The one lossy step of the conversion, pinned rather than documented
        // only: `FetchRequest` carries `String`s because its driver is
        // `tenta-net`'s `Request::header(&str, &str)`, while an XHR value is a
        // byte sequence that may hold obs-text.
        let request = XhrRequest {
            url: url("http://fixture.test/x"),
            method: http::Method::GET,
            headers: vec![(b"x-raw".to_vec(), vec![0xE9, b'a'])],
            body: None,
            initiator: url("http://fixture.test/x"),
        };

        let converted = request.into_fetch_request(Origin::Tracked);

        assert_eq!(
            converted.headers,
            vec![("x-raw".to_owned(), "\u{fffd}a".to_owned())]
        );
    }
}
