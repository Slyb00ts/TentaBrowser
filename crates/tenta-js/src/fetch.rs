//! `fetch()`: the seam between the request a page makes and the answer a
//! network driver brings back (`docs/spec/05-javascript.md` §3).
//!
//! A host record holds no isolate and a network driver holds no engine, so
//! neither can call the other. The two meet over plain owned data: the page
//! hands a driver a [`FetchRequest`], and the driver leaves the answer in a
//! [`FetchCompletions`] table for the page to drain back into the engine.
//! `resolver: u64` is opaque bits for the same reason
//! [`PromiseRef`](crate::dom::events::PromiseRef) is — a driver must not be
//! able to name a V8 type, or the network tier would have to depend on the
//! engine tier instead of the other way round
//! (`docs/spec/02-architecture.md` §2).
//!
//! Neither direction does any work here. `start` only hands the request over,
//! and the table only stores what it is given: the scheduling belongs to the
//! driver, and applying a completion belongs to the engine.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use tenta_core::Origin;
use url::Url;

/// One request a page made through `fetch()` or an `XMLHttpRequest`.
///
/// Everything the driver needs is already resolved here: `url` has been
/// resolved against the document's base URL, and `origin` is the provenance of
/// the task that called `fetch`, which is what the driver tags the request with
/// so the settle machine can tell a foreground fetch from a background one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchRequest {
    /// The request URL, resolved against the document.
    pub url: Url,
    /// The request method. `fetch()` is always `GET` until `RequestInit` lands —
    /// the initializer that could ask for another method is refused rather than
    /// ignored — while an `XMLHttpRequest`'s is the one `open()` validated and
    /// normalised.
    pub method: http::Method,
    /// The author request headers, in the order they were authored.
    ///
    /// `XMLHttpRequest` is the first producer that fills these: `setRequestHeader`
    /// combines the page's pairs into them and `send()` hands them over. A
    /// `fetch()` leaves them empty until `RequestInit` lands. The values are
    /// `String`s because `tenta-net`'s `Request::header` takes `&str` pairs; an
    /// XHR value is a byte sequence, so the conversion
    /// ([`XhrRequest::into_fetch_request`](crate::xhr::XhrRequest::into_fetch_request))
    /// is the one step where a byte that is not UTF-8 becomes U+FFFD.
    pub headers: Vec<(String, String)>,
    /// The body, or `None` for a request with none. Both producers leave it
    /// `None` today: `fetch()` needs `RequestInit` and an `XMLHttpRequest`
    /// refuses a body rather than dropping it.
    pub body: Option<Vec<u8>>,
    /// The document's URL, or the request URL when the document has none. The
    /// `Referer` a driver sends, and the URL a same-origin test is made
    /// against.
    pub initiator: Url,
    /// Where the calling task came from. A `fetch` from an inline script is
    /// [`Origin::Tracked`] and holds the settle open; one from a timer is
    /// [`Origin::Background`] and does not. That asymmetry is the settle
    /// machine's own rule, so it is carried rather than assumed.
    pub origin: Origin,
}

impl FetchRequest {
    /// A `GET` with no headers and no body — the only request `fetch()` can
    /// make while `RequestInit` is refused.
    ///
    /// It exists so the engine's `fetch` global can build a request without
    /// naming `http::Method` itself: `tenta-js-v8` depends on neither `http`
    /// nor `url`, and a struct literal there would add two dependency edges for
    /// one constant. The fields stay public for a driver that builds its own.
    pub fn get(url: Url, initiator: Url, origin: Origin) -> Self {
        Self {
            url,
            method: http::Method::GET,
            headers: Vec::new(),
            body: None,
            initiator,
            origin,
        }
    }
}

/// What a driver answers a [`FetchRequest`] with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchOutcome {
    /// The request completed. The response is delivered as data, not as bytes
    /// plus a connection: the engine builds the `Response` record from it.
    Response(FetchedResponse),
    /// The request could not be completed. The message reaches script as the
    /// rejection's `TypeError.message`, which is what the Fetch Standard does
    /// for a network error — a fetch failure is an exception, never a
    /// response with a failure status.
    Failed(String),
}

/// A completed response, mirroring
/// [`ResponseRecord`](crate::bindings::host::ResponseRecord) field for field
/// so building the record is a struct copy.
///
/// It is deliberately shaped so a later CORS slice can add a filtered header
/// list without changing the type: nothing here assumes the headers are the
/// ones script may see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedResponse {
    /// The URL the response came from — after any redirects, which is why the
    /// driver reports it rather than the engine assuming the request's.
    pub url: String,
    /// The status code.
    pub status: u16,
    /// Whether the request was redirected. A driver that follows none always
    /// reports `false`.
    pub redirected: bool,
    /// The header list in wire order, names lowercased. A driver hands over
    /// what it received; the engine is what drops the names script may not see.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The body, or `None` for a null-body status — the driver is not expected
    /// to know which statuses those are, so it may report `None` for an empty
    /// body too and the engine's own rule decides.
    pub body: Option<Vec<u8>>,
}

/// One answer waiting to be applied: the promise it settles, and what to
/// settle it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchCompletion {
    /// The resolver's bits, as [`PromiseRef::resolver`] handed them to the
    /// driver.
    ///
    /// [`PromiseRef::resolver`]: crate::dom::events::PromiseRef::resolver
    pub resolver: u64,
    pub outcome: FetchOutcome,
}

/// Why a driver would not take a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchStartError {
    message: String,
}

impl FetchStartError {
    /// Refuses the request with `message`, which is handed to script verbatim
    /// as the rejection's `TypeError.message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The reason, for the rejection the caller builds from it.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Answers the `fetch()` calls of one page.
///
/// One driver is installed per navigation ([`PageEngine::set_fetch_driver`]),
/// so a completion it leaves behind always belongs to the document that is
/// current when the completion is drained.
///
/// # The lifetime contract
///
/// `start` returns as soon as the request is under way; the response arrives
/// later, and a driver that completes one must do **two** things — push the
/// completion into the [`FetchCompletions`] table it was given, and wake the
/// page, so the settle loop takes another turn and drains the table. The push
/// alone is not observable: the drain runs from the loop's turn
/// (`tenta_page::runtime`), so a completion nothing wakes for stays in the
/// table, and the promise it names stays pending, until something else happens
/// to wake the page.
///
/// The same wake is what keeps the settle open while the request is in flight:
/// a tracked foreground request holds the settle (`RequestRecord::holds_settle`
/// in `tenta-page`), and while one is outstanding the settle machine stays in
/// `Draining` with the busy-cap countdown suspended, so the loop keeps turning
/// and drains the table as soon as the driver pushes. 4a-1b's `tenta-net`
/// driver is where the two calls land — the push, plus `note_request_done` for
/// the wake. The one way this hangs is a driver that neither pushes nor
/// completes its request, and even then only until the page's own
/// `Settle::Timeout`.
///
/// [`PageEngine::set_fetch_driver`]: crate::engine::PageEngine::set_fetch_driver
pub trait FetchDriver {
    /// Takes `request` and leaves its answer in the table when it arrives.
    ///
    /// `Err` refuses the request before any of it is sent: the page rejects the
    /// promise with the message rather than leaving it pending for a driver
    /// that was never asked. A refusal is checked here rather than in the
    /// caller because only the driver knows whether it can serve the request —
    /// a cross-origin URL is the first case.
    fn start(&self, request: FetchRequest, resolver: u64) -> Result<(), FetchStartError>;
}

/// The completions a driver is waiting for the page to pick up.
///
/// The table lives on the page side and the driver holds a handle to it, which
/// is why it is an `Rc<RefCell<..>>` under a plain struct: the driver is called
/// from a `fetch()` binding and the drain runs from the settle loop, so the two
/// are never on the stack together, but neither owns the other.
#[derive(Default, Debug)]
pub struct FetchCompletions {
    queue: Rc<RefCell<VecDeque<FetchCompletion>>>,
}

impl FetchCompletions {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the answer to `resolver`. Called by a driver, from whatever
    /// task or thread it completes on — the table is not `Sync`, so a driver
    /// that completes off the page thread must hand the completion back to it
    /// first.
    ///
    /// See [`FetchDriver::start`] for what the caller owes in addition to this.
    pub fn push(&self, resolver: u64, outcome: FetchOutcome) {
        self.queue
            .borrow_mut()
            .push_back(FetchCompletion { resolver, outcome });
    }

    /// Takes every completion recorded so far, in the order they were pushed,
    /// and empties the table.
    ///
    /// Draining rather than borrowing is what keeps the drain free of an
    /// engine-boundary violation: a caller does not hold the table's borrow
    /// while the engine settles the promises, which re-enters Rust
    /// (`crate::engine`).
    pub fn take_all(&self) -> Vec<FetchCompletion> {
        self.queue.borrow_mut().drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(message: &str, resolver: u64) -> FetchCompletion {
        FetchCompletion {
            resolver,
            outcome: FetchOutcome::Failed(message.to_owned()),
        }
    }

    #[test]
    fn take_all_hands_the_completions_back_in_push_order() {
        let table = FetchCompletions::new();
        table.push(7, FetchOutcome::Failed("first".to_owned()));
        table.push(1, FetchOutcome::Failed("second".to_owned()));
        table.push(4, FetchOutcome::Failed("third".to_owned()));
        assert_eq!(
            table.take_all(),
            vec![failed("first", 7), failed("second", 1), failed("third", 4),],
        );
    }

    #[test]
    fn take_all_empties_the_table() {
        let table = FetchCompletions::new();
        table.push(1, FetchOutcome::Failed("only".to_owned()));
        assert_eq!(table.take_all().len(), 1);
        assert!(
            table.take_all().is_empty(),
            "a second drain finds nothing, so a completion cannot be settled twice"
        );
    }

    #[test]
    fn an_empty_table_drains_to_nothing() {
        assert!(FetchCompletions::new().take_all().is_empty());
    }

    #[test]
    fn a_completion_pushed_after_a_drain_is_delivered_by_the_next_one() {
        let table = FetchCompletions::new();
        table.push(1, FetchOutcome::Failed("before".to_owned()));
        assert_eq!(table.take_all(), vec![failed("before", 1)]);
        table.push(2, FetchOutcome::Failed("after".to_owned()));
        assert_eq!(
            table.take_all(),
            vec![failed("after", 2)],
            "the drain is a boundary, not a subscription"
        );
    }

    #[test]
    fn a_refusal_carries_its_message() {
        let error = FetchStartError::new("cross-origin");
        assert_eq!(error.message(), "cross-origin");
    }

    #[test]
    fn a_get_has_no_headers_and_no_body() {
        let url = Url::parse("https://example.test/a").expect("a test URL");
        let request = FetchRequest::get(url.clone(), url, Origin::Tracked);
        assert_eq!(request.method, http::Method::GET);
        assert!(request.headers.is_empty());
        assert!(request.body.is_none());
        assert_eq!(request.origin, Origin::Tracked);
    }
}
