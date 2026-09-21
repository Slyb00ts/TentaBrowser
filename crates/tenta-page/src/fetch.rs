//! The `tenta-net` driver behind the page's `fetch()` and `XMLHttpRequest`
//! (`docs/spec/05-javascript.md` §3, P2 slices 4a-1b and b-6).
//!
//! `tenta_js::fetch` and `tenta_js::xhr` are the two seams and `crate::runtime`
//! is where a completion is drained back into script; this module is what sits
//! between them. It turns a [`FetchRequest`] into a `tenta-net` request, sends it
//! from the page thread's `LocalSet`, reads the body to the end, and leaves the
//! answer in the page's [`FetchCompletions`] table — or, for XHR, its
//! [`XhrCompletions`](tenta_js::xhr::XhrCompletions) table — with a wake behind
//! it.
//!
//! One driver serves both routes rather than two drivers sharing helpers. The
//! two differ in exactly two things — the resource type the request carries and
//! which table the answer is left in — while *everything* else is the same code
//! path, and it is the same code path on purpose: **both cross-origin gates live
//! here**, and a second driver would be a second place for gate 2 to be
//! forgotten. See [`NetFetchDriver`] for the two gates and why both are needed.
//!
//! It lives in `tenta-page` because that is the only crate that can see
//! everything it needs — `tenta-net`, the `tenta_js::fetch` seam, the
//! [`SettleTracker`] and `crate::page::next_request_id` — while keeping the
//! dependency direction of `docs/spec/02-architecture.md` §2: a network driver
//! holds no engine, and neither `tenta-js` nor `tenta-js-v8` gains an edge to
//! the network tier.
//!
//! # What this does not do
//!
//! There is no CORS and no preflight, and there is no `RequestInit`, no
//! streaming `Response.body`, and no `AbortSignal`-driven cancellation. A
//! request that leaves the document's origin is *refused* rather than made and
//! then filtered, which is the one security property this module owns: see
//! [`NetFetchDriver`] for the two gates and why both are needed.
//!
//! XHR's own `abort()` is not here either (b-7), so an abandoned request is
//! *discarded* when its completion arrives rather than terminated where it
//! stands — the record's request id is what makes the two distinguishable, and
//! the discard itself is
//! [`deliver_xhr_completion`](tenta_js::bindings::host::deliver_xhr_completion)'s.
//! What this driver owes that arrangement is that the task still lands in
//! `inflight`, so [`Drop`] ends it with the navigation.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_js::fetch::{
    FetchCompletions, FetchDriver, FetchOutcome, FetchRequest, FetchStartError, FetchedResponse,
};
use tenta_js::xhr::{XhrCompletions, XhrDriver, XhrHandle};
use tenta_net::policy::cookies::Site;
use tenta_net::request::same_origin;
use tenta_net::{Client, NetError, Request, ResourceType, Response};
use tokio::task::JoinHandle;
use url::Url;

use crate::page::{PageConfig, next_request_id};
use crate::settle::{RequestClass, SettleTracker};
use crate::task::{catch_unwind_future, panic_message};

/// The rejection a request that leaves the document's origin gets, from either
/// gate.
///
/// Engine-authored and stable on purpose: what a page may branch on is that the
/// promise rejected with a `TypeError`, never which of this engine's internal
/// names refused it.
const CROSS_ORIGIN: &str = "Failed to fetch: cross-origin requests are not supported yet";

/// The rejection every transport failure gets.
///
/// Fetch's own network error is a fixed `TypeError: Failed to fetch`, and a
/// page that branched on `net::ERR_NAME_NOT_RESOLVED` against
/// `net::ERR_CONNECTION_REFUSED` would be branching on this project's
/// event/document vocabulary rather than on a script API. The detail is not
/// lost — it goes to `tracing` — it just does not reach script.
const FAILED: &str = "Failed to fetch";

/// The refusal a page header that cannot go on the wire produces.
///
/// On the `fetch` route such a header can only come from the page's own
/// configuration (`PageConfig::extra_headers`), because `RequestInit` is refused
/// before a request is ever built. XHR reaches this constant through
/// `setRequestHeader` as well.
const INVALID_HEADER: &str = "Failed to fetch: a request header cannot be sent";

/// Answers the `fetch()` calls and the `XMLHttpRequest` objects of one
/// navigation over `tenta-net`.
///
/// One driver is built per navigation by `PageRuntime::create` and installed
/// twice — with `PageEngine::set_fetch_driver` and `PageEngine::set_xhr_driver` —
/// so a completion it leaves behind always names a resolver, or an event target,
/// of the engine that is current when the page drains it. A new navigation gets
/// a fresh document, hence a fresh runtime and a fresh driver, so a late
/// completion can never reach an earlier document's engine. (The name is older
/// than XHR and is kept: it is the *fetch* module, and the shared gates below are
/// its reason to be one driver.)
///
/// # The cross-origin gates
///
/// Neither gate is CORS, and both are required.
///
/// 1. **Before the request is sent** (`FetchDriver::start`). A URL that is not
///    same-origin with the document is refused, which the seam turns into a
///    `TypeError` rejection: no synchronous throw, no network, and no settle
///    registration. This is exactly what [`FetchStartError`] is for.
/// 2. **On the final URL, before the body is read.** `Client::send` follows
///    redirects internally, so a same-origin URL can redirect off-origin and
///    the transport would then hand script a cross-origin body and header list.
///    Gate 1 alone would therefore leave a cross-origin body leak behind the
///    refusal, which is a worse failure than the one it prevents.
///
/// The gate subsumes the scheme check: a `data:` or `ftp:` URL is not
/// same-origin with an `http:` document, so no separate test is needed.
pub(crate) struct NetFetchDriver {
    /// The page's transport handle, cloned per request. Cloning is cheap and
    /// every clone talks to the same io thread, pool and cookie jar.
    client: Client,
    /// The page's `fetch()` completion table, which `crate::runtime`'s drain
    /// reads.
    completions: Rc<FetchCompletions>,
    /// The page's XHR completion table, drained by the same turn.
    ///
    /// A second table rather than one table with a tagged payload, because the
    /// two are drained into different engine entries (`settle_fetches` settles a
    /// promise, `settle_xhr` dispatches an event) and because the XHR table's
    /// entries carry a request id the fetch table's have no use for.
    xhrs: Rc<XhrCompletions>,
    /// The settle call the requests are registered against. It is also the
    /// wake: `note_request_done` signals the notification the settle loop
    /// parks on.
    tracker: Rc<RefCell<SettleTracker>>,
    /// The body ceiling each request carries, from the page's network budget.
    max_body_bytes: u64,
    /// The page's own headers, which go on every request it issues — a
    /// `User-Agent` override that reached the document but not the requests
    /// script made would be a strange half of a profile.
    extra_headers: Vec<(String, String)>,
    /// Every fetch still running.
    ///
    /// Interior mutability because `FetchDriver::start` takes `&self`: the
    /// driver is reached through an `Rc` the engine owns, and the handle has to
    /// be stored anyway for [`Drop`] to abort it.
    inflight: RefCell<Vec<JoinHandle<()>>>,
}

impl NetFetchDriver {
    /// The driver one navigation uses. `config` supplies the transport, the
    /// network budget and the page's own headers.
    ///
    /// Both configuration values are copied rather than borrowed: the driver is
    /// installed as a `Rc<dyn FetchDriver>` and therefore has to be `'static`,
    /// and `PageConfig` is neither `Arc`-ed nor kept alive by the runtime.
    pub(crate) fn new(
        config: &PageConfig,
        tracker: Rc<RefCell<SettleTracker>>,
        completions: Rc<FetchCompletions>,
        xhrs: Rc<XhrCompletions>,
    ) -> NetFetchDriver {
        NetFetchDriver {
            client: config.client.clone(),
            completions,
            xhrs,
            tracker,
            max_body_bytes: config.budgets.network_bytes,
            extra_headers: config.extra_headers.clone(),
            inflight: RefCell::new(Vec::new()),
        }
    }
}

/// Sends one request and leaves its answer where `delivery` says. Both routes
/// are this one call.
impl NetFetchDriver {
    fn send(
        &self,
        request: FetchRequest,
        resource_type: ResourceType,
        delivery: Delivery,
    ) -> Result<(), FetchStartError> {
        // Gate 1, before anything is registered and before anything is sent.
        same_origin_allowed(&request.initiator, &request.url)?;
        // Built before the registration, so a header the transport cannot put
        // on the wire leaves no trace in the settle machine. The refusal is a
        // start refusal rather than a `Failed` outcome: the request cannot be
        // built at all, so there is nothing to send and nothing to wait for.
        let net = net_request(
            &request,
            resource_type,
            self.max_body_bytes,
            &self.extra_headers,
        )
        .map_err(|error| {
            tracing::warn!(
                url = %request.url,
                %error,
                "a request is refused: one of the page's headers cannot go on the wire"
            );
            FetchStartError::new(INVALID_HEADER)
        })?;
        // The non-parser entry: a `fetch()` and an `XMLHttpRequest` are both
        // script-initiated, so unlike a script's own fetch neither is exempt
        // from reclassification. The provenance is the calling task's, verbatim
        // — a request from a timer is `Background` and does not hold the settle
        // open, which is the settle machine's own rule rather than this
        // driver's. It is *not* what the cross-origin gate above compares; that
        // one is the initiator's, the document URL (`same_origin_allowed`).
        let request_id = next_request_id();
        self.tracker.borrow_mut().note_request_started(
            request_id,
            RequestClass::XhrFetch,
            request.origin,
        );
        // Where this request's answer goes, resolved *before* the spawn so the
        // task body below is one body for both routes: the two differ in this
        // and in nothing else, and a second body is a second place for the
        // push-then-wake order and the panic path to drift.
        let push: Rc<dyn Fn(FetchOutcome)> = match delivery {
            Delivery::Resolver(resolver) => {
                let completions = Rc::clone(&self.completions);
                Rc::new(move |outcome| completions.push(resolver, outcome))
            }
            Delivery::Xhr(handle) => {
                let completions = Rc::clone(&self.xhrs);
                Rc::new(move |outcome| completions.push(handle, outcome))
            }
        };
        // The task captures the pieces and never `self`: an `Rc` to the driver
        // inside it would keep the driver alive for as long as the request runs,
        // so the driver would never drop and [`Drop`] would abort nothing.
        let client = self.client.clone();
        let tracker = Rc::clone(&self.tracker);
        let initiator = request.initiator.clone();
        let handle = tokio::task::spawn_local(async move {
            let fetched = catch_unwind_future(async move {
                match client.send(net).await {
                    Ok(response) => fetched_response(response, &initiator).await,
                    Err(error) => transport_failure(&error),
                }
            })
            .await;
            let outcome = match fetched {
                Ok(outcome) => outcome,
                // A panic here must not leave a promise pending or an
                // `XMLHttpRequest` waiting, and either would hold the settle
                // until the page's own deadline: the request is over either way,
                // so the page is told so — the promise rejects, or the object's
                // *request error steps* run.
                Err(payload) => {
                    tracing::error!(
                        reason = %panic_message(&payload),
                        "the transport task of one request panicked"
                    );
                    FetchOutcome::Failed(FAILED.to_owned())
                }
            };
            // **Push, then wake, in that order.** The push alone is invisible
            // until a turn drains the table, and the wake is what makes the
            // settle loop take that turn: `note_request_done` signals the
            // notification the loop's `select!` parks on. Nothing can be lost
            // in between, because the page thread is a single-threaded
            // `LocalSet` whose `select!` is its only await point, so this task
            // runs only while the loop is parked and `notify_one` leaves a
            // permit behind.
            push(outcome);
            tracker.borrow_mut().note_request_done(request_id);
        });
        let mut inflight = self.inflight.borrow_mut();
        // A page that fetches in a loop must not accumulate handles for requests
        // that are over. Aborting a finished handle is a no-op, so dropping it
        // here costs nothing and keeps this list to the requests actually in
        // flight. The XHR route lands here too, which is what keeps an abandoned
        // request from outliving its navigation.
        inflight.retain(|handle| !handle.is_finished());
        inflight.push(handle);
        Ok(())
    }
}

/// Where a finished request's answer is left, which is the whole of the
/// difference between the two routes.
///
/// `fetch()` settles a promise, so its completion names the resolver the engine
/// handed out; XHR dispatches events, so its completion names the object that
/// made the request *and* which of that object's requests this is — the second
/// half is what lets a completion for a request the object abandoned be
/// discarded rather than delivered (`deliver_xhr_completion`).
#[derive(Clone, Copy, Debug)]
enum Delivery {
    /// The `fetch()` promise registered under this resolver.
    Resolver(u64),
    /// The `XMLHttpRequest` object, and the request of it that is being answered.
    Xhr(XhrHandle),
}

impl FetchDriver for NetFetchDriver {
    fn start(&self, request: FetchRequest, resolver: u64) -> Result<(), FetchStartError> {
        self.send(request, ResourceType::Fetch, Delivery::Resolver(resolver))
    }
}

impl XhrDriver for NetFetchDriver {
    /// `Err` is gate 1's refusal, and the caller turns it into the standard's
    /// *request error steps* rather than into a request left in the air: nothing
    /// is registered and nothing is sent, so there is nothing to wait for.
    fn start(&self, request: FetchRequest, handle: XhrHandle) -> Result<(), FetchStartError> {
        self.send(request, ResourceType::Xhr, Delivery::Xhr(handle))
    }
}

impl Drop for NetFetchDriver {
    /// Aborts every request still running, either route's.
    ///
    /// This is not needed for settlement *correctness*: both completion tables
    /// are per-navigation and a completion from a dropped navigation is
    /// discarded with it, and the promise a completion could have named is
    /// bounded by the engine's own `PromiseState::clear`. It is needed for real
    /// resource freedom: an aborted `Client::send` sees its reply channel
    /// closed, cancels the transport at `reply.closed()` and closes the
    /// connection instead of parking it. The socket-level observable is
    /// `FixtureServer::open_connections()` returning to its baseline; this
    /// commit asserts the future's drop beneath it and no further, and says so
    /// at the test.
    fn drop(&mut self) {
        for handle in self.inflight.borrow_mut().drain(..) {
            handle.abort();
        }
    }
}

/// The one request a [`FetchRequest`] becomes, whichever route it came from.
///
/// Follows `crate::runtime::script_request` wherever the two agree, because the
/// page's own headers and its first-party site must not differ between the
/// document, the scripts it loads and the requests its scripts make.
///
/// `resource_type` is the one thing the two routes disagree about, and it is a
/// parameter rather than two functions because everything else here is shared —
/// the two literals differ only in their name, and `tenta-net` gives `Xhr` and
/// `Fetch` the same mode and priority (`request.rs`: `Cors`, `Medium`, not
/// incremental). It is still worth carrying the true one: it is what the request
/// table, the priority queue and any CDP `Network.requestWillBeSent` report,
/// and a request whose type lied would be a wrong answer to all three.
fn net_request(
    request: &FetchRequest,
    resource_type: ResourceType,
    max_body_bytes: u64,
    extra_headers: &[(String, String)],
) -> Result<Request, NetError> {
    let mut out = Request::new(request.method.clone(), request.url.clone())
        // `.resource_type()` also sets the priority and the mode, which are the
        // right answers for a CORS-flagged subresource on either route.
        .resource_type(resource_type)
        // The initiator decides `Sec-Fetch-Site`, `Origin` and the default
        // `Referer`, all of which are the document's rather than the request's.
        // `.referrer()` is deliberately not called: the initiator supplies the
        // default, and overriding it here would bypass the referrer policy.
        .initiator(request.initiator.clone())
        .max_body_bytes(max_body_bytes);
    // What the request is first-party to is the **document**, not the request
    // URL: a request a page makes to a third-party origin is still a
    // first-party request of that page.
    out.first_party = Site::from_url(&request.initiator);
    // Script's own headers first, then the page's, in that order: the page's are
    // defaults for the request, and a header the caller set must win the
    // duplicate resolution rather than be overwritten by one.
    for (name, value) in request.headers.iter().chain(extra_headers) {
        out = out.header(name, value)?;
    }
    // `limits.total` is deliberately left unset, so `ClientConfig::request_timeout`
    // applies. There is no defensible per-request deadline for a `fetch()`:
    // `Goto::timeout` is the navigation's and a fetch may well outlive the
    // navigation that started it, `budgets.evaluate_time` is a *script entry*
    // deadline rather than a network one, and `settle.foreground_request_max` is
    // the settle machine's reclassification bound — using it would fail a
    // legitimately slow fetch that the machine had merely stopped waiting on.
    // The task is bounded anyway: the driver's `Drop` aborts it with the
    // navigation. No policy in this slice is better than a wrong one.
    //
    // The redirect policy is left at its default (`Follow { max: 20 }`), which
    // is what makes gate 2 below load-bearing. `user_activated` stays false: no
    // gesture started a `fetch()`.
    Ok(out)
}

/// Whether a request the document at `initiator` made may reach `url`; `Err` is
/// the refusal both cross-origin gates report.
///
/// Compared against [`FetchRequest::initiator`] — the document URL — and *not*
/// against [`FetchRequest::origin`], which is task provenance and a different
/// thing entirely. Origin equality is RFC 6454's: scheme, host and port.
///
/// The same check answers both gates. In `start` it is an `Err`; at completion
/// it is a `Failed` outcome, because by then the request has already been made
/// and the promise exists to be rejected.
fn same_origin_allowed(initiator: &Url, url: &Url) -> Result<(), FetchStartError> {
    if same_origin(initiator, url) {
        return Ok(());
    }
    Err(FetchStartError::new(CROSS_ORIGIN))
}

/// What the transport answered, as the seam's outcome.
///
/// Every status is a `FetchOutcome::Response` — a 404 or a 503 is a response,
/// which is what `Response.ok` is for — and only a transport failure is
/// `Failed`. `crate::runtime::read_script_body` does test `is_success()`; that
/// is a script *fetch*, where a 404 body is an error page rather than the
/// program the element named, and copying the test here would be wrong.
///
/// Nothing is filtered: `insert_fetched_response` already drops the forbidden
/// response-header names and the NUL/CR/LF values, and corrects the body of a
/// null-body status. A second filter here would be a second, drifting copy of
/// the Fetch rule. `response.headers` is preferred over `raw_headers` because
/// the transport removes `Content-Encoding` and `Content-Length` from it when
/// it decoded the body, so it does not lie about the bytes the engine holds.
async fn fetched_response(response: Response, initiator: &Url) -> FetchOutcome {
    // Gate 2: the final URL, before the body is read. A same-origin URL that
    // redirected off-origin is the case `Client::send` cannot report any other
    // way, because it follows redirects internally. Dropping the `Response`
    // returns the connection.
    if let Err(refusal) = same_origin_allowed(initiator, &response.url) {
        tracing::warn!(
            url = %response.url,
            "a response was redirected off the requesting origin; it is discarded"
        );
        return FetchOutcome::Failed(refusal.message().to_owned());
    }
    tracing::debug!(status = response.status.as_u16(), "a response head arrived");
    let status = response.status.as_u16();
    let url = response.url.to_string();
    let redirected = !response.redirects.is_empty();
    let headers = response
        .headers
        .into_iter()
        // `as_bytes()`, never `to_str()`: a header value may be obs-text, and a
        // `to_str()` that failed would blank the header instead of carrying it.
        .map(|(name, value)| (name, value.as_bytes().to_vec()))
        .collect();
    // Read to completion rather than streamed: the seam is frozen to bytes
    // (`FetchedResponse::body`), so there is nowhere for a stream to go until
    // `d-5b`. `Some(vec![])` for an empty body is correct — `None` would mean
    // "no body at all", which the engine cannot tell from an empty one.
    match response.body.collect().await {
        Ok(body) => FetchOutcome::Response(FetchedResponse {
            url,
            status,
            redirected,
            headers,
            body: Some(body.to_vec()),
        }),
        // One honest deviation, recorded in `docs/dev/idl.md`: because head and
        // body arrive together, a body that fails mid-stream rejects the whole
        // `fetch()`, where the standard would have resolved a `Response` whose
        // `text()` rejects later.
        Err(error) => transport_failure(&error),
    }
}

/// The outcome a transport failure produces: one fixed message for script, the
/// detail to `tracing`.
fn transport_failure(error: &NetError) -> FetchOutcome {
    tracing::warn!(
        name = %error.error_text(),
        message = %error.message,
        "a request failed at the transport"
    );
    FetchOutcome::Failed(FAILED.to_owned())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use bytes::Bytes;
    use http::{HeaderValue, StatusCode, Version};
    use tenta_net::request::{RedirectPolicy, RequestMode};
    use tenta_net::{
        BodyStream, ClientConfig, ErrorName, NetRuntime, Protocol, RedirectHop, Timing,
    };

    use super::*;
    use crate::clock::ManualClock;
    use crate::settle::{Origin, Settle, SettleConfig};

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a test URL")
    }

    /// A tracker over a manual clock, at the `NetworkIdle` wait that reads the
    /// request table.
    fn tracker() -> (Rc<RefCell<SettleTracker>>, ManualClock) {
        let clock = ManualClock::new();
        let tracker = SettleTracker::new(
            SettleConfig::library(),
            Settle::NetworkIdle,
            std::sync::Arc::new(clock.clone()),
        );
        (Rc::new(RefCell::new(tracker)), clock)
    }

    /// A driver that can be reached without an engine. Its transport is a real
    /// one: the refusal paths under test never send anything, so the client is
    /// only ever constructed and dropped.
    fn driver(tracker: Rc<RefCell<SettleTracker>>) -> NetFetchDriver {
        let net = NetRuntime::new(1).expect("io runtime");
        let client = net.client(ClientConfig::default()).expect("client");
        NetFetchDriver {
            client,
            completions: Rc::new(FetchCompletions::new()),
            xhrs: Rc::new(XhrCompletions::new()),
            tracker,
            max_body_bytes: 4096,
            extra_headers: Vec::new(),
            inflight: RefCell::new(Vec::new()),
        }
    }

    /// The body of a `Response`, as the tests build one: complete before it is
    /// ever read.
    fn response(
        status: u16,
        url: Url,
        headers: Vec<(&str, HeaderValue)>,
        redirects: Vec<RedirectHop>,
        body: &'static [u8],
    ) -> Response {
        Response {
            status: StatusCode::from_u16(status).expect("a test status"),
            version: Version::HTTP_11,
            url,
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect(),
            raw_headers: Vec::new(),
            body: BodyStream::once(Bytes::from_static(body)),
            timing: Timing::default(),
            remote_addr: None,
            protocol: Protocol::Http11,
            from_cache: false,
            redirects,
        }
    }

    /// The `Response` of a `fetched_response` call that resolved, panicking on
    /// a failure with the message the page would have seen.
    fn resolved(outcome: FetchOutcome) -> FetchedResponse {
        match outcome {
            FetchOutcome::Response(fetched) => fetched,
            FetchOutcome::Failed(message) => panic!("expected a response, got: {message}"),
        }
    }

    /// A `GET` with no headers and no body is the only request `fetch()` can
    /// make while `RequestInit` is refused, and every knob it turns is the one
    /// `script_request` turns.
    #[test]
    fn a_get_maps_to_a_fetch_resource_request() {
        let initiator = url("https://origin.test/page");
        let mut request = FetchRequest::get(
            url("https://origin.test/data.json"),
            initiator.clone(),
            Origin::Tracked,
        );
        request.headers = vec![("x-from-script".to_owned(), "one".to_owned())];
        let extra = vec![("user-agent".to_owned(), "tenta/0".to_owned())];

        let net =
            net_request(&request, ResourceType::Fetch, 4096, &extra).expect("the request is built");

        assert_eq!(net.method, http::Method::GET);
        assert_eq!(net.url, request.url);
        assert_eq!(net.resource_type, ResourceType::Fetch);
        assert_eq!(
            net.mode,
            RequestMode::Cors,
            "a fetch is a CORS-flagged subresource"
        );
        assert_eq!(net.initiator.as_ref(), Some(&initiator));
        assert_eq!(net.first_party, Site::from_url(&initiator));
        assert_eq!(net.limits.max_body_bytes, Some(4096));
        assert!(
            net.limits.total.is_none(),
            "no per-request deadline: the client's own timeout applies"
        );
        assert!(net.body.is_empty(), "the request has no body");
        assert!(!net.user_activated);
        assert_eq!(net.redirect, RedirectPolicy::Follow { max: 20 });
        assert_eq!(
            net.headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.to_str().expect("ASCII")))
                .collect::<Vec<_>>(),
            vec![("x-from-script", "one"), ("user-agent", "tenta/0")],
            "script's headers first, then the page's, in order"
        );
    }

    /// Origin equality is RFC 6454's, so the gate subsumes the scheme check: a
    /// `data:` URL is not same-origin with an `http:` document, and neither is
    /// the same host on another port or another scheme.
    #[test]
    fn the_gate_is_rfc_6454_origin_equality() {
        let initiator = url("http://origin.test:8080/page");
        assert!(same_origin_allowed(&initiator, &url("http://origin.test:8080/x")).is_ok());
        assert!(
            same_origin_allowed(&initiator, &url("http://origin.test/x")).is_err(),
            "a different port is a different origin"
        );
        assert!(same_origin_allowed(&initiator, &url("https://origin.test:8080/x")).is_err());
        assert!(same_origin_allowed(&initiator, &url("http://other.test:8080/x")).is_err());
        assert!(
            same_origin_allowed(&initiator, &url("data:text/plain,hello")).is_err(),
            "a scheme the document is not served over is not same-origin"
        );
        assert_eq!(
            same_origin_allowed(&initiator, &url("http://other.test/x"))
                .expect_err("refused")
                .message(),
            CROSS_ORIGIN
        );
    }

    /// The refusal is checked before the registration, so a cross-origin
    /// `fetch()` leaves the settle machine exactly as it found it — no
    /// foreground request holding the settle open and nothing counted for
    /// CDP's idle signals.
    ///
    /// This is also what makes the test possible outside a `LocalSet`: the
    /// refusal returns before the `spawn_local` that a sent request would need.
    #[test]
    fn a_cross_origin_request_is_refused_before_it_registers() {
        let (tracker, _clock) = tracker();
        let driver = driver(Rc::clone(&tracker));
        let request = FetchRequest::get(
            url("http://other.test/x"),
            url("http://origin.test/page"),
            Origin::Tracked,
        );

        let error =
            FetchDriver::start(&driver, request, 7).expect_err("a cross-origin request is refused");

        assert_eq!(error.message(), CROSS_ORIGIN);
        assert_eq!(
            tracker.borrow().pending().foreground_requests,
            0,
            "nothing was registered, so nothing holds the settle open"
        );
        assert_eq!(
            tracker.borrow().in_flight_for_idle(),
            0,
            "and nothing was counted for the idle signals"
        );
        assert!(
            driver.inflight.borrow().is_empty(),
            "and no task was spawned, so nothing is in flight"
        );
    }

    /// Gate 2. `Client::send` follows redirects internally, so a same-origin
    /// URL can end up answering from another origin; the response is discarded
    /// rather than delivered, body and headers included.
    #[tokio::test]
    async fn a_cross_origin_final_url_after_a_redirect_is_a_failure() {
        let initiator = url("http://origin.test/page");
        let answered = response(
            200,
            url("http://other.test/elsewhere"),
            vec![("content-type", HeaderValue::from_static("text/plain"))],
            vec![RedirectHop {
                url: url("http://origin.test/moved"),
                status: 302,
                location: Some("http://other.test/elsewhere".to_owned()),
                from_cache: false,
            }],
            b"a body script must never see",
        );

        let outcome = fetched_response(answered, &initiator).await;

        assert_eq!(outcome, FetchOutcome::Failed(CROSS_ORIGIN.to_owned()));
    }

    /// The other route, and the only thing `net_request` does differently for
    /// it: the resource type. Everything else — the mode, the priority, the
    /// first-party site, the header order — is the same request, which is why
    /// the two routes share one function.
    #[test]
    fn an_xhr_maps_to_an_xhr_resource_request() {
        let initiator = url("https://origin.test/page");
        let request = FetchRequest::get(
            url("https://origin.test/data.json"),
            initiator.clone(),
            Origin::Tracked,
        );

        let net =
            net_request(&request, ResourceType::Xhr, 4096, &[]).expect("the request is built");

        assert_eq!(net.resource_type, ResourceType::Xhr);
        assert_eq!(
            net.mode,
            RequestMode::Cors,
            "an XHR is a CORS-flagged subresource too"
        );
        assert_eq!(
            net.priority,
            ResourceType::Fetch.default_priority(),
            "and `tenta-net` gives the two the same priority, so the \
             parameter is about the *name* on the request, not a scheduling \
             difference"
        );
    }

    /// Gate 1 on the XHR route. The refusal is the same one the `fetch` route
    /// reports and is checked in the same place — before the registration and
    /// before the spawn — so a cross-origin `xhr.send()` leaves the settle
    /// machine exactly as it found it and leaves no task behind.
    ///
    /// XHR is the route where this matters most: a page's XHR is the request
    /// shape a cross-origin call is most often written with.
    #[test]
    fn a_cross_origin_xhr_is_refused_before_it_registers() {
        let (tracker, _clock) = tracker();
        let driver = driver(Rc::clone(&tracker));
        let request = FetchRequest::get(
            url("http://other.test/x"),
            url("http://origin.test/page"),
            Origin::Tracked,
        );
        let handle = XhrHandle {
            target: 0x0900_0000_0000_0007,
            request: 2,
        };

        let error = XhrDriver::start(&driver, request, handle)
            .expect_err("a cross-origin request is refused");

        assert_eq!(error.message(), CROSS_ORIGIN);
        assert_eq!(tracker.borrow().pending().foreground_requests, 0);
        assert_eq!(tracker.borrow().in_flight_for_idle(), 0);
        assert!(driver.inflight.borrow().is_empty());
        assert!(
            driver.xhrs.take_all().is_empty(),
            "a refused request leaves no completion behind either"
        );
    }

    /// The handle survives the whole XHR route unchanged, which is what the
    /// host layer's discard depends on: `DispatchTarget::from_bits` must resolve
    /// the `target` half back to the object that sent the request, and
    /// `deliver_xhr_completion` must see the `request` half it will compare
    /// against the record.
    ///
    /// The transport here is a real one and there is no server on the port, so
    /// the answer is a transport failure — which is exactly as good a completion
    /// to carry a handle as a response, and is the one this test can produce
    /// without a fixture server (`tenta-page` has no `tenta-testutil`
    /// dev-dependency). The e2e in `tenta-browser` is where the response half of
    /// the route is exercised against a real server.
    #[test]
    fn the_xhr_handle_arrives_back_at_the_table_unchanged() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (tracker, _clock) = tracker();
            let driver = driver(Rc::clone(&tracker));
            let handle = XhrHandle {
                target: 0x0900_0000_0000_0007,
                request: 2,
            };
            // Same-origin, so gate 1 passes; nothing listens on port 1, so the
            // transport fails rather than the request being refused.
            let request = FetchRequest::get(
                url("http://127.0.0.1:1/data.json"),
                url("http://127.0.0.1:1/page"),
                Origin::Tracked,
            );

            XhrDriver::start(&driver, request, handle).expect("the request is started");
            assert_eq!(
                tracker.borrow().pending().foreground_requests,
                1,
                "the request is registered before it is sent"
            );
            let task = driver
                .inflight
                .borrow_mut()
                .pop()
                .expect("the request's task is in flight");
            task.await.expect("the task ends normally");

            let completions = driver.xhrs.take_all();
            assert_eq!(completions.len(), 1, "the request left exactly one answer");
            assert_eq!(
                completions[0].handle, handle,
                "both halves of the handle reach the table as they were handed over"
            );
            assert_eq!(
                completions[0].outcome,
                FetchOutcome::Failed(FAILED.to_owned()),
                "and the answer is the transport's failure, not a refusal"
            );
            assert_eq!(
                tracker.borrow().pending().foreground_requests,
                0,
                "the wake behind the push is what releases the settle"
            );
        });
    }

    /// Every field of the seam's response comes from the transport's, and the
    /// driver filters nothing: a non-UTF-8 value keeps its bytes and a repeated
    /// `Set-Cookie` survives to the engine, which is where Fetch's own drop
    /// rule lives.
    #[tokio::test]
    async fn a_response_maps_every_field() {
        let initiator = url("http://origin.test/page");
        let target = url("http://origin.test/data.json");
        let obs_text = HeaderValue::from_bytes(b"\x80\xff").expect("obs-text is a valid value");
        let answered = response(
            200,
            target.clone(),
            vec![
                ("content-type", HeaderValue::from_static("application/json")),
                ("x-raw", obs_text),
                ("set-cookie", HeaderValue::from_static("a=1")),
                ("set-cookie", HeaderValue::from_static("b=2")),
            ],
            vec![RedirectHop {
                url: url("http://origin.test/old.json"),
                status: 301,
                location: Some("/data.json".to_owned()),
                from_cache: false,
            }],
            b"{\"ok\":true}",
        );

        let fetched = resolved(fetched_response(answered, &initiator).await);

        assert_eq!(fetched.url, target.to_string());
        assert_eq!(fetched.status, 200);
        assert!(fetched.redirected, "the chain was not empty");
        assert_eq!(fetched.body, Some(b"{\"ok\":true}".to_vec()));
        assert_eq!(
            fetched.headers,
            vec![
                ("content-type".to_owned(), b"application/json".to_vec()),
                ("x-raw".to_owned(), b"\x80\xff".to_vec()),
                ("set-cookie".to_owned(), b"a=1".to_vec()),
                ("set-cookie".to_owned(), b"b=2".to_vec()),
            ],
            "the bytes survive verbatim and both cookies are handed over"
        );
    }

    /// A status that is not ok is still a *response*: `Response.ok` is what
    /// answers that question, and only a transport failure rejects. This is the
    /// guard on the `is_success()` test a script fetch makes and a `fetch()`
    /// must not.
    #[tokio::test]
    async fn a_non_ok_status_is_a_response_not_a_failure() {
        let initiator = url("http://origin.test/page");
        let answered = response(
            503,
            url("http://origin.test/api/status?code=503"),
            Vec::new(),
            Vec::new(),
            b"<h1>HTTP 503</h1>",
        );

        let fetched = resolved(fetched_response(answered, &initiator).await);

        assert_eq!(fetched.status, 503);
        assert!(!fetched.redirected, "nothing redirected");
        assert_eq!(fetched.body, Some(b"<h1>HTTP 503</h1>".to_vec()));
    }

    /// A transport failure is one fixed message for script, whatever the
    /// transport called it: the `net::ERR_*` vocabulary is this project's, not
    /// a script API.
    #[test]
    fn a_transport_failure_is_one_message() {
        let named = NetError::new(ErrorName::NameNotResolved, "no such host: other.test");
        assert_eq!(
            transport_failure(&named),
            FetchOutcome::Failed(FAILED.to_owned())
        );
        let refused = NetError::new(ErrorName::ConnectionRefused, "");
        assert_eq!(
            transport_failure(&refused),
            FetchOutcome::Failed(FAILED.to_owned())
        );
    }

    /// Sets a flag when it is dropped, so a test can tell whether the future it
    /// lives in was dropped.
    struct Aborted(Rc<Cell<bool>>);

    impl Drop for Aborted {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    /// `Drop` aborts the fetches still in flight, and aborting a task drops its
    /// future — which is what closes the connection underneath it: the aborted
    /// `Client::send` sees its reply channel closed, the transport cancels at
    /// `reply.closed()` and closes the connection instead of parking it
    /// (`crates/tenta-net/src/client.rs`).
    ///
    /// The observable here is the future's own drop, because a socket cannot be
    /// reached without a server: `tenta-page` has no `tenta-testutil`
    /// dev-dependency, so the socket-level observable,
    /// `FixtureServer::open_connections()` returning to its baseline, is **not**
    /// asserted in this commit. What is proven here is the link the driver owns
    /// — `abort()` drops the task's future, so the `Client::send` inside it is
    /// dropped too; `tenta-net`'s own cancellation behaviour, which is what
    /// turns that drop into a closed connection, is covered at its layer
    /// (`crates/tenta-net/tests/http.rs`).
    #[test]
    fn dropping_the_driver_aborts_the_fetches_in_flight() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (tracker, _clock) = tracker();
            let driver = driver(tracker);
            let aborted = Rc::new(Cell::new(false));
            let handle = tokio::task::spawn_local({
                let aborted = Rc::clone(&aborted);
                async move {
                    // Held for as long as the task's future lives, and never
                    // reached past the await: only an abort can end this task.
                    let _guard = Aborted(aborted);
                    std::future::pending::<()>().await;
                }
            });
            driver.inflight.borrow_mut().push(handle);
            // Let the task be polled once, so it is parked on its await rather
            // than still sitting unpolled in the queue.
            tokio::task::yield_now().await;
            assert!(!aborted.get(), "the task is still running");

            drop(driver);

            // An abort marks the task cancelled and schedules it; the drop
            // lands when the scheduler next reaches it.
            for _ in 0..100 {
                if aborted.get() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(
                aborted.get(),
                "the aborted task's future was dropped, so its work is over"
            );
        });
    }
}
