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
//! path, and it is the same code path on purpose: **the CORS checks live
//! here**, and a second driver would be a second place for one to be
//! forgotten. See [`NetFetchDriver`] for the checks and why each is needed.
//!
//! It lives in `tenta-page` because that is the only crate that can see
//! everything it needs — `tenta-net`, the `tenta_js::fetch` seam, the
//! [`SettleTracker`] and `crate::page::next_request_id` — while keeping the
//! dependency direction of `docs/spec/02-architecture.md` §2: a network driver
//! holds no engine, and neither `tenta-js` nor `tenta-js-v8` gains an edge to
//! the network tier.
//!
//! # Requests to other origins
//!
//! A request that leaves the document's origin goes through the CORS protocol
//! (`crate::cors`): it is sent with an `Origin` header and, unless the page
//! asked for credentials, without cookies; a method or a header a form could
//! not have sent is preflighted first; and the response reaches script only
//! when the server names the page's origin, with only the headers the server
//! exposes. A response that fails the check is a network error, exactly as a
//! transport failure is, and the reason goes to the log. See
//! [`NetFetchDriver`] for where each step sits.
//!
//! # What this does not do
//!
//! There is no preflight cache and no streaming `Response.body`. A request that needed a preflight does not
//! follow a redirect to another origin, which would need a preflight of its
//! own.
//!
//! XHR's own `abort()` reaches the transport here, and the same machinery
//! answers `open()`'s step 10: both are [`XhrDriver::abort`], which names the
//! request by the handle it was started with and aborts the task that is running
//! it. That is what makes an abandoned request stop costing a socket rather than
//! merely having its answer discarded. The id-based discard stays as the second
//! half — [`deliver_xhr_completion`](tenta_js::bindings::host::deliver_xhr_completion)
//! refuses a completion for a request the object has moved on from, which is the
//! half a driver can never supply, and both are what the engine's
//! `end_xhr_request` performs together.
//!
//! What this driver owes that arrangement is that the task lands in `inflight`
//! with the handle it was started under, so [`XhrDriver::abort`] can find it and
//! [`Drop`] ends whatever is left with the navigation.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::RequestId;
use tenta_js::fetch::{
    Credentials, FetchCompletions, FetchDriver, FetchOutcome, FetchRequest, FetchStartError,
    FetchedResponse, Mode, Redirect,
};
use tenta_js::xhr::{XhrCompletions, XhrDriver, XhrHandle};
use tenta_net::policy::cookies::Site;
use tenta_net::request::{RedirectPolicy, same_origin};
use tenta_net::{Client, CredentialsMode, NetError, Request, ResourceType, Response};
use tokio::task::JoinHandle;
use url::Url;

use crate::cors;
use crate::page::{PageConfig, next_request_id};
use crate::settle::{RequestClass, SettleTracker};
use crate::task::{catch_unwind_future, panic_message};

/// The rejection a request in `same-origin` mode gets when it would leave the
/// document's origin, before it is sent or after a redirect took it there.
const CROSS_ORIGIN: &str =
    "Failed to fetch: the request mode is 'same-origin' and the URL is on another origin";

/// The rejection a URL that is not `http:` or `https:` gets.
const UNSUPPORTED_SCHEME: &str = "Failed to fetch: only http and https URLs can be fetched";

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
/// On the `fetch` route such a header reaches here through the initializer's
/// `headers` or through the page's own configuration
/// (`PageConfig::extra_headers`). XHR reaches this constant through
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
/// # Requests to other origins
///
/// 1. **Before the request is sent** (`FetchDriver::start`). A URL that is not
///    `http:` or `https:` is refused, and so is a cross-origin URL in
///    `same-origin` mode: no synchronous throw, no network, and no settle
///    registration. This is exactly what [`FetchStartError`] is for.
/// 2. **The preflight.** A cross-origin request whose method or author headers
///    are not CORS-safelisted is preceded by an `OPTIONS` request, and is sent
///    only if the answer allows it.
/// 3. **On the final response, before the body is read.** `Client::send`
///    follows redirects internally, so the check is made against the whole
///    chain: when any URL of it is off the document's origin the response has
///    to pass the CORS check, and its headers are cut down to the ones script
///    may read. A response that fails is dropped unread.
///
/// Credentials follow the request's mode hop by hop in the transport
/// (`tenta_net::CredentialsMode`), so a same-origin request that is redirected
/// to another origin stops carrying cookies at that hop.
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
    /// The page's network observation point.
    net: crate::network::NetworkHub,
    /// Every request still running, with the handle an XHR can be addressed by.
    ///
    /// Interior mutability because `FetchDriver::start` takes `&self`: the
    /// driver is reached through an `Rc` the engine owns, and the handle has to
    /// be stored anyway for [`Drop`] to abort it.
    inflight: RefCell<Vec<Inflight>>,
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
            net: config.net.clone(),
            inflight: RefCell::new(Vec::new()),
        }
    }
}

/// One request the driver is still running, and the handle that can end it.
///
/// The two halves are kept together because they are the two halves of one
/// request's life: the task is what runs, and the handle is what the page names
/// it by. A `fetch()` has no `abort()` to be named by, so its entry carries
/// `None` and only [`Drop`] can end it.
struct Inflight {
    /// The handle the request was started under, or `None` on the `fetch()`
    /// route.
    ///
    /// Both halves of the handle are compared when a request is looked up — the
    /// `target` bits are the object that made the request, so two objects that
    /// hold the same request id are still told apart.
    xhr: Option<XhrHandle>,
    /// The resolver of the promise the request settles, or `None` on the XHR
    /// route: what a `fetch()` whose signal aborted is found by.
    resolver: Option<u64>,
    /// The settle registration the request holds, which has to be released by
    /// whoever ends it.
    ///
    /// The task releases its own when it finishes, so this is only needed by
    /// [`XhrDriver::abort`], whose abort drops the task's future *before* that
    /// line can run. Without it an aborted request would stay counted as a
    /// foreground request and hold the settle open until the settle machine gave
    /// up on it (`settle.foreground_request_max`).
    request_id: RequestId,
    /// The task running the request.
    task: JoinHandle<()>,
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
        // Step 1, before anything is registered and before anything is sent.
        if !matches!(request.url.scheme(), "http" | "https") {
            return Err(FetchStartError::new(UNSUPPORTED_SCHEME));
        }
        let cross_origin = !same_origin(&request.initiator, &request.url);
        if cross_origin && request.mode == Mode::SameOrigin {
            return Err(FetchStartError::new(CROSS_ORIGIN));
        }
        let mut request = request;
        if request.mode == Mode::NoCors {
            // The `"request-no-cors"` guard: a header a form could not have
            // sent is not sent. With the method already limited to the simple
            // ones, nothing is left that a preflight would have to ask about.
            let unsafe_names = cors::unsafe_header_names(&request.headers);
            request
                .headers
                .retain(|(name, _)| !unsafe_names.contains(&name.to_ascii_lowercase()));
        }
        let policy = CorsPolicy {
            origin: cors::serialized_origin(&request.initiator),
            initiator: request.initiator.clone(),
            credentials: request.credentials == Credentials::Include,
            same_origin_only: request.mode == Mode::SameOrigin,
            opaque: request.mode == Mode::NoCors,
        };
        // Step 2's request, built here so a header the transport cannot carry
        // is a start refusal like the real request's.
        let preflight =
            if cross_origin && cors::needs_preflight(&request.method, &request.headers) {
                let built = preflight_request(&request, resource_type, &self.extra_headers)
                    .map_err(|error| {
                        tracing::warn!(url = %request.url, %error, "a preflight cannot be built");
                        FetchStartError::new(INVALID_HEADER)
                    })?;
                Some(Preflight {
                    request: built,
                    method: request.method.clone(),
                    unsafe_headers: cors::unsafe_header_names(&request.headers),
                })
            } else {
                None
            };
        // Built before the registration, so a header the transport cannot put
        // on the wire leaves no trace in the settle machine. The refusal is a
        // start refusal rather than a `Failed` outcome: the request cannot be
        // built at all, so there is nothing to send and nothing to wait for.
        let mut net = net_request(
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
        // driver's. It is *not* what the cross-origin test above compares; that
        // one is the initiator's, the document URL.
        let request_id = next_request_id();
        let mut tracked = self.net.begin(&mut net, request_id, None);
        self.tracker.borrow_mut().note_request_started(
            request_id,
            RequestClass::XhrFetch,
            request.origin,
        );
        // Where this request's answer goes, resolved *before* the spawn so the
        // task body below is one body for both routes: the two differ in this
        // and in nothing else, and a second body is a second place for the
        // push-then-wake order and the panic path to drift.
        let origin = request.origin;
        let push: Rc<dyn Fn(FetchOutcome)> = match delivery {
            Delivery::Resolver(resolver) => {
                let completions = Rc::clone(&self.completions);
                Rc::new(move |outcome| completions.push(resolver, outcome, origin))
            }
            Delivery::Xhr(handle) => {
                let completions = Rc::clone(&self.xhrs);
                Rc::new(move |outcome| completions.push(handle, outcome, origin))
            }
        };
        // The task captures the pieces and never `self`: an `Rc` to the driver
        // inside it would keep the driver alive for as long as the request runs,
        // so the driver would never drop and [`Drop`] would abort nothing.
        let client = self.client.clone();
        let tracker = Rc::clone(&self.tracker);
        let hub = self.net.clone();
        let handle = tokio::task::spawn_local(async move {
            let fetched = catch_unwind_future(async move {
                if let Some(preflight) = preflight
                    && let Err(failure) = run_preflight(&client, &hub, preflight, &policy).await
                {
                    tracing::warn!(%failure, "a cross-origin request failed its preflight");
                    if let Some(tracked) = &mut tracked {
                        tracked.failed_with("net::ERR_FAILED");
                    }
                    return FetchOutcome::Failed(FAILED.to_owned());
                }
                match client.send(net).await {
                    Ok(response) => fetched_response(response, &policy, &mut tracked).await,
                    Err(error) => {
                        if let Some(tracked) = &mut tracked {
                            tracked.failed(&error);
                        }
                        transport_failure(&error)
                    }
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
        inflight.retain(|entry| !entry.task.is_finished());
        inflight.push(Inflight {
            // `Delivery` is `Copy`, so the handle the task's closure took is
            // still available here: the task and the list name the same request.
            xhr: delivery.xhr(),
            resolver: delivery.resolver(),
            request_id,
            task: handle,
        });
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

impl Delivery {
    /// The handle the request can be addressed by, or `None` when it has none.
    ///
    /// A `fetch()` is settled by a resolver and has no `abort()`; only an XHR
    /// object can name its own request, and only its handle can end it.
    fn xhr(self) -> Option<XhrHandle> {
        match self {
            Delivery::Resolver(_) => None,
            Delivery::Xhr(handle) => Some(handle),
        }
    }
    /// The resolver the request settles, or `None` on the XHR route.
    fn resolver(self) -> Option<u64> {
        match self {
            Delivery::Resolver(resolver) => Some(resolver),
            Delivery::Xhr(_) => None,
        }
    }
}

/// What the CORS checks of one request compare against.
struct CorsPolicy {
    /// The document's URL.
    initiator: Url,
    /// Its origin, as `Access-Control-Allow-Origin` has to spell it.
    origin: String,
    /// Whether the credentials mode is `include`, which rules the wildcards
    /// out and needs the server's `Access-Control-Allow-Credentials`.
    credentials: bool,
    /// Whether the mode is `same-origin`, so a redirect off the origin fails
    /// the request whatever the server allows.
    same_origin_only: bool,
    /// Whether the mode is `no-cors`: a cross-origin response is not checked
    /// and not shared, script gets an opaque one instead.
    opaque: bool,
}

/// A preflight to make before the request it stands in front of.
struct Preflight {
    request: Request,
    /// The real request's method and non-safelisted header names: what the
    /// answer has to allow.
    method: http::Method,
    unsafe_headers: Vec<String>,
}

/// The CORS-preflight request for `request`
/// (<https://fetch.spec.whatwg.org/#cors-preflight-fetch>): an `OPTIONS` to the
/// same URL that names the method and the headers the real one will carry. It
/// never carries credentials and does not follow redirects.
fn preflight_request(
    request: &FetchRequest,
    resource_type: ResourceType,
    extra_headers: &[(String, String)],
) -> Result<Request, NetError> {
    let mut out = Request::new(http::Method::OPTIONS, request.url.clone())
        .resource_type(resource_type)
        .initiator(request.initiator.clone())
        .credentials(CredentialsMode::Omit)
        .redirect(RedirectPolicy::Manual)
        .header("Accept", "*/*")?
        .header("Access-Control-Request-Method", request.method.as_str())?;
    out.first_party = Site::from_url(&request.initiator);
    let names = cors::unsafe_header_names(&request.headers);
    if !names.is_empty() {
        out = out.header("Access-Control-Request-Headers", &names.join(","))?;
    }
    for (name, value) in extra_headers {
        out = out.header(name, value)?;
    }
    Ok(out)
}

/// Sends the preflight and checks its answer. `Err` is why the real request
/// must not be sent.
async fn run_preflight(
    client: &Client,
    hub: &crate::network::NetworkHub,
    mut preflight: Preflight,
    policy: &CorsPolicy,
) -> Result<(), String> {
    let mut tracked = hub.begin(&mut preflight.request, next_request_id(), None);
    let response = match client.send(preflight.request).await {
        Ok(response) => response,
        Err(error) => {
            if let Some(tracked) = &mut tracked {
                tracked.failed(&error);
            }
            return Err(format!("the preflight failed: {}", error.error_text()));
        }
    };
    if let Some(tracked) = &mut tracked {
        tracked.response(&response);
    }
    let status = response.status.as_u16();
    let headers: Vec<(String, Vec<u8>)> = response
        .headers
        .iter()
        .map(|(name, value)| (name.clone(), value.as_bytes().to_vec()))
        .collect();
    // The body of a preflight answer means nothing; reading it to the end is
    // what returns the connection to the pool.
    let body = response.body.collect().await;
    if let Some(tracked) = &mut tracked {
        match &body {
            Ok(body) => tracked.complete(body),
            Err(error) => tracked.failed(error),
        }
    }
    cors::check_preflight(
        &policy.origin,
        policy.credentials,
        &preflight.method,
        &preflight.unsafe_headers,
        status,
        &headers,
    )
    .map_err(|failure| failure.to_string())
}

impl FetchDriver for NetFetchDriver {
    fn start(&self, request: FetchRequest, resolver: u64) -> Result<(), FetchStartError> {
        self.send(request, ResourceType::Fetch, Delivery::Resolver(resolver))
    }

    /// Ends the request behind `resolver`, as [`XhrDriver::abort`] ends one
    /// behind a handle: the task is aborted, so no completion is left behind,
    /// and the settle registration it could no longer release is released
    /// here.
    fn abort(&self, resolver: u64) {
        let mut inflight = self.inflight.borrow_mut();
        let Some(index) = inflight
            .iter()
            .position(|entry| entry.resolver == Some(resolver))
        else {
            return;
        };
        let entry = inflight.swap_remove(index);
        drop(inflight);
        if entry.task.is_finished() {
            return;
        }
        entry.task.abort();
        self.tracker
            .borrow_mut()
            .note_request_done(entry.request_id);
    }
}

impl XhrDriver for NetFetchDriver {
    /// `Err` is gate 1's refusal, and the caller turns it into the standard's
    /// *request error steps* rather than into a request left in the air: nothing
    /// is registered and nothing is sent, so there is nothing to wait for.
    fn start(&self, request: FetchRequest, handle: XhrHandle) -> Result<(), FetchStartError> {
        self.send(request, ResourceType::Xhr, Delivery::Xhr(handle))
    }

    /// Finds the request by its handle and aborts the task running it.
    ///
    /// This is the driver's half of the engine's `end_xhr_request`, and the
    /// reason a request the page ended stops costing a socket rather than
    /// completing into a table nothing will deliver from.
    ///
    /// The request is **removed** from the list rather than left in it. An
    /// aborted task's entry would otherwise sit there until the next request
    /// swept it, and a second `abort()` for the same request would then find a
    /// finished task and report `false` — a wrong answer for a request this
    /// driver had itself ended. Removing it makes "the driver no longer holds
    /// this handle" the truth as soon as the termination lands.
    ///
    /// A task already finished is reported as `false` and not aborted: nothing
    /// is running under it, and its answer is already in the XHR table, where
    /// the engine's request id is what refuses it. That is the whole of why the
    /// two guards exist rather than one.
    ///
    /// An aborted task never reaches its own `note_request_done`, because
    /// aborting drops the future before it can, so the settle registration is
    /// released here instead. Skipping it would leave the request counted as a
    /// foreground request — the page would hold its settle open for a request
    /// that is over, until the settle machine reclassified it
    /// (`settle.foreground_request_max`).
    ///
    /// The lookup compares the whole handle. `handle.target` is the object that
    /// made the request, so an object that has moved on to another request can
    /// never end one that belongs to a different object, even if the two ids
    /// happened to match.
    fn abort(&self, handle: XhrHandle) -> bool {
        let ended = {
            let mut inflight = self.inflight.borrow_mut();
            let Some(index) = inflight.iter().position(|entry| entry.xhr == Some(handle)) else {
                return false;
            };
            inflight.swap_remove(index)
        };
        if ended.task.is_finished() {
            return false;
        }
        ended.task.abort();
        self.tracker
            .borrow_mut()
            .note_request_done(ended.request_id);
        true
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
    ///
    /// It is also what ends a request the page never abandoned: a navigation
    /// that arrives while a script's `fetch()` is still running drops the whole
    /// driver, so the entries left in `inflight` are exactly the requests that
    /// were still in flight.
    fn drop(&mut self) {
        for entry in self.inflight.borrow_mut().drain(..) {
            entry.task.abort();
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
        .max_body_bytes(max_body_bytes)
        // The transport applies the mode hop by hop, so a redirect that
        // leaves the document's origin stops carrying cookies there.
        .credentials(match request.credentials {
            Credentials::Omit => CredentialsMode::Omit,
            Credentials::SameOrigin => CredentialsMode::SameOrigin,
            Credentials::Include => CredentialsMode::Include,
        })
        // A request that needs a preflight has one for its own origin at
        // most, so it may not be redirected to another.
        .same_origin_redirects(cors::needs_preflight(&request.method, &request.headers));
    if request.redirect == Redirect::Error {
        out = out.redirect(RedirectPolicy::Error);
    }
    if request.bypass_cache {
        out = out.reload();
    }
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
    // The body, when the request has one. **`Content-Length` is deliberately not
    // set here**: the transport derives it from the body's own length and
    // inserts it only when the caller has not (`tenta-net`'s `headers::build`,
    // whose `sends_content_length` is what also gives an empty `POST` its
    // `Content-Length: 0`), and `Content-Length` is a *forbidden request-header
    // name* — a page cannot set it through the initializer's `headers` either,
    // because that list is filled under the `"request"` guard. A second one
    // would be a smuggling-shaped bug rather than a preference.
    if let Some(body) = request.body.clone() {
        out = out.with_body(body);
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
    // The redirect policy is `Follow { max: 20 }` unless the page asked for
    // `error`, which is what makes the check on the final response
    // load-bearing. `user_activated` stays false: no gesture started a
    // `fetch()`.
    Ok(out)
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
async fn fetched_response(
    response: Response,
    policy: &CorsPolicy,
    tracked: &mut Option<crate::network::Tracked>,
) -> FetchOutcome {
    // Step 3: the whole chain, before the body is read. A same-origin URL that
    // redirected off-origin is the case `Client::send` cannot report any other
    // way, because it follows redirects internally. Dropping the `Response`
    // returns the connection.
    let cross_origin = !same_origin(&policy.initiator, &response.url)
        || response
            .redirects
            .iter()
            .any(|hop| !same_origin(&policy.initiator, &hop.url));
    let mut headers: Vec<(String, Vec<u8>)> = response
        .headers
        .iter()
        // `as_bytes()`, never `to_str()`: a header value may be obs-text, and a
        // `to_str()` that failed would blank the header instead of carrying it.
        .map(|(name, value)| (name.clone(), value.as_bytes().to_vec()))
        .collect();
    if cross_origin && policy.opaque {
        // The request was made, which is all a `no-cors` caller wanted of
        // it; the answer is read to the end for a host that watches the
        // traffic and is not handed to script.
        if let Some(tracked) = tracked {
            tracked.response(&response);
            if let Ok(body) = response.body.collect().await {
                tracked.complete(&body);
            }
        }
        return FetchOutcome::Opaque;
    }
    if cross_origin {
        let refusal = if policy.same_origin_only {
            Some("the request mode is 'same-origin' and a redirect left the origin".to_owned())
        } else {
            cors::check_response(&policy.origin, policy.credentials, &headers)
                .err()
                .map(|failure| failure.to_string())
        };
        if let Some(reason) = refusal {
            tracing::warn!(
                url = %response.url,
                %reason,
                "a cross-origin response is not shared with the page; it is discarded"
            );
            if let Some(tracked) = tracked {
                tracked.failed_with("net::ERR_FAILED");
            }
            return FetchOutcome::Failed(FAILED.to_owned());
        }
        headers = cors::exposed_headers(policy.credentials, headers);
    }
    if let Some(tracked) = tracked {
        tracked.response(&response);
    }
    tracing::debug!(status = response.status.as_u16(), "a response head arrived");
    let status = response.status.as_u16();
    let url = response.url.to_string();
    let redirected = !response.redirects.is_empty();
    // Read to completion rather than streamed: the seam is frozen to bytes
    // (`FetchedResponse::body`), so there is nowhere for a stream to go until
    // `d-5b`. `Some(vec![])` for an empty body is correct — `None` would mean
    // "no body at all", which the engine cannot tell from an empty one.
    let collected = response.body.collect().await;
    if let Some(tracked) = tracked {
        match &collected {
            Ok(body) => tracked.complete(body),
            Err(error) => tracked.failed(error),
        }
    }
    match collected {
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
            net: crate::network::NetworkHub::default(),
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
            FetchOutcome::Opaque => panic!("expected a response, got an opaque one"),
        }
    }

    /// A `GET` with no headers and no body — what `fetch(url)` is — and every
    /// knob it turns is the one `script_request` turns.
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

    /// A request with a body carries it to the transport, and **nothing here
    /// sets `Content-Length`**: the transport derives it from the body's own
    /// length (`tenta-net`'s `headers::build`), and a page cannot reach this
    /// parameter with one — the initializer's `headers` are filled under the
    /// `"request"` guard, which drops the name.
    ///
    /// Both halves are asserted, because they are one decision: the body is on
    /// the request, and the framing header is *not* in the author header list
    /// the page built.
    #[test]
    fn a_post_body_reaches_the_transport_without_an_authored_content_length() {
        let initiator = url("https://origin.test/page");
        let mut request = FetchRequest::get(
            url("https://origin.test/api/echo"),
            initiator,
            Origin::Tracked,
        );
        request.method = http::Method::POST;
        request.headers = vec![("content-type".to_owned(), "application/json".to_owned())];
        request.body = Some(b"{\"item\":\"beta\"}".to_vec());

        let mut net =
            net_request(&request, ResourceType::Fetch, 4096, &[]).expect("the request is built");

        assert_eq!(net.method, http::Method::POST);
        assert_eq!(
            net.body.take_bytes().as_deref(),
            Some(b"{\"item\":\"beta\"}".as_slice()),
            "the bytes the page's body extracted to are what the transport is given"
        );
        assert_eq!(
            net.headers
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["content-type"],
            "and the framing header is the transport's to add, not the caller's"
        );
    }

    /// The checks of one request made by the document at `initiator`, in the
    /// default mode.
    fn policy(initiator: &Url) -> CorsPolicy {
        CorsPolicy {
            initiator: initiator.clone(),
            origin: cors::serialized_origin(initiator),
            credentials: false,
            same_origin_only: false,
            opaque: false,
        }
    }

    /// The refusals are checked before the registration, so a request that is
    /// refused leaves the settle machine exactly as it found it — no
    /// foreground request holding the settle open and nothing counted for
    /// CDP's idle signals.
    ///
    /// This is also what makes the test possible outside a `LocalSet`: the
    /// refusal returns before the `spawn_local` that a sent request would need.
    #[test]
    fn a_request_that_cannot_be_made_is_refused_before_it_registers() {
        let (tracker, _clock) = tracker();
        let driver = driver(Rc::clone(&tracker));
        let mut same_origin_mode = FetchRequest::get(
            url("http://other.test/x"),
            url("http://origin.test/page"),
            Origin::Tracked,
        );
        same_origin_mode.mode = Mode::SameOrigin;
        let error = FetchDriver::start(&driver, same_origin_mode, 7)
            .expect_err("same-origin mode does not leave the origin");
        assert_eq!(error.message(), CROSS_ORIGIN);

        let data = FetchRequest::get(
            url("data:text/plain,hello"),
            url("http://origin.test/page"),
            Origin::Tracked,
        );
        let error = FetchDriver::start(&driver, data, 8).expect_err("not an http URL");
        assert_eq!(error.message(), UNSUPPORTED_SCHEME);

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

    /// A response that came from another origin after a redirect, with the
    /// headers the server answered with.
    fn redirected_off_origin(headers: Vec<(&'static str, HeaderValue)>) -> Response {
        response(
            200,
            url("http://other.test/elsewhere"),
            headers,
            vec![RedirectHop {
                url: url("http://origin.test/moved"),
                status: 302,
                location: Some("http://other.test/elsewhere".to_owned()),
                from_cache: false,
            }],
            b"a body script sees only with the server's consent",
        )
    }

    /// Step 3. `Client::send` follows redirects internally, so a same-origin
    /// URL can end up answering from another origin; without the server's
    /// consent the response is discarded rather than delivered, body and
    /// headers included, and script is told only that the fetch failed.
    #[tokio::test]
    async fn a_cross_origin_response_without_consent_is_a_failure() {
        let initiator = url("http://origin.test/page");
        let answered = redirected_off_origin(vec![(
            "content-type",
            HeaderValue::from_static("text/plain"),
        )]);
        let outcome = fetched_response(answered, &policy(&initiator), &mut None).await;
        assert_eq!(outcome, FetchOutcome::Failed(FAILED.to_owned()));

        let other_origin = redirected_off_origin(vec![(
            "access-control-allow-origin",
            HeaderValue::from_static("http://third.test"),
        )]);
        let outcome = fetched_response(other_origin, &policy(&initiator), &mut None).await;
        assert_eq!(outcome, FetchOutcome::Failed(FAILED.to_owned()));
    }

    /// With consent the response is delivered, cut down to the headers the
    /// server exposes.
    #[tokio::test]
    async fn a_cross_origin_response_is_shared_when_the_server_names_the_origin() {
        let initiator = url("http://origin.test/page");
        let answered = redirected_off_origin(vec![
            ("content-type", HeaderValue::from_static("text/plain")),
            (
                "access-control-allow-origin",
                HeaderValue::from_static("http://origin.test"),
            ),
            ("x-internal", HeaderValue::from_static("hidden")),
            ("x-request-id", HeaderValue::from_static("r1")),
            (
                "access-control-expose-headers",
                HeaderValue::from_static("X-Request-Id"),
            ),
        ]);
        let fetched = resolved(fetched_response(answered, &policy(&initiator), &mut None).await);
        assert_eq!(fetched.url, "http://other.test/elsewhere");
        assert!(fetched.redirected);
        assert_eq!(
            fetched.headers,
            vec![
                ("content-type".to_owned(), b"text/plain".to_vec()),
                ("x-request-id".to_owned(), b"r1".to_vec()),
            ]
        );
    }

    /// Credentials rule the wildcard out, and `same-origin` mode rules out the
    /// redirect whatever the server allows.
    #[tokio::test]
    async fn credentials_and_same_origin_mode_tighten_the_check() {
        let initiator = url("http://origin.test/page");
        let wildcard = || {
            redirected_off_origin(vec![(
                "access-control-allow-origin",
                HeaderValue::from_static("*"),
            )])
        };
        let shared = fetched_response(wildcard(), &policy(&initiator), &mut None).await;
        assert!(matches!(shared, FetchOutcome::Response(_)));

        let mut with_credentials = policy(&initiator);
        with_credentials.credentials = true;
        let outcome = fetched_response(wildcard(), &with_credentials, &mut None).await;
        assert_eq!(outcome, FetchOutcome::Failed(FAILED.to_owned()));

        let mut same_origin_mode = policy(&initiator);
        same_origin_mode.same_origin_only = true;
        let outcome = fetched_response(wildcard(), &same_origin_mode, &mut None).await;
        assert_eq!(outcome, FetchOutcome::Failed(FAILED.to_owned()));
    }

    /// The modes a request carries reach the transport, and a request that
    /// needs a preflight may not be redirected to another origin.
    #[test]
    fn the_modes_reach_the_transport_request() {
        let initiator = url("https://origin.test/page");
        let mut request = FetchRequest::get(
            url("https://api.other.test/items"),
            initiator,
            Origin::Tracked,
        );
        let plain = net_request(&request, ResourceType::Fetch, 4096, &[]).expect("built");
        assert_eq!(plain.credentials, CredentialsMode::SameOrigin);
        assert_eq!(plain.redirect, RedirectPolicy::default());
        assert!(!plain.same_origin_redirects && !plain.bypass_cache_lookup);

        request.credentials = Credentials::Include;
        request.redirect = Redirect::Error;
        request.bypass_cache = true;
        request.method = http::Method::PUT;
        let net = net_request(&request, ResourceType::Fetch, 4096, &[]).expect("built");
        assert_eq!(net.credentials, CredentialsMode::Include);
        assert_eq!(net.redirect, RedirectPolicy::Error);
        assert!(net.same_origin_redirects && net.bypass_cache_lookup);
    }

    /// The preflight names what the real request will carry and carries none
    /// of it itself.
    #[test]
    fn a_preflight_names_the_method_and_the_unsafe_headers() {
        let initiator = url("https://origin.test/page");
        let mut request = FetchRequest::get(
            url("https://api.other.test/items/1"),
            initiator,
            Origin::Tracked,
        );
        request.method = http::Method::PUT;
        request.headers = vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("X-Token".to_owned(), "t".to_owned()),
            ("accept".to_owned(), "application/json".to_owned()),
        ];
        request.body = Some(b"{}".to_vec());
        let extra = [("x-page".to_owned(), "1".to_owned())];

        let mut preflight =
            preflight_request(&request, ResourceType::Fetch, &extra).expect("built");

        assert_eq!(preflight.method, http::Method::OPTIONS);
        assert_eq!(preflight.url, request.url);
        assert_eq!(preflight.credentials, CredentialsMode::Omit);
        assert_eq!(preflight.redirect, RedirectPolicy::Manual);
        assert!(
            preflight
                .body
                .take_bytes()
                .is_none_or(|body| body.is_empty())
        );
        let header = |name: &str| {
            preflight
                .headers
                .iter()
                .find(|(header, _)| header.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.to_str().expect("ascii").to_owned())
        };
        assert_eq!(
            header("access-control-request-method").as_deref(),
            Some("PUT")
        );
        assert_eq!(
            header("access-control-request-headers").as_deref(),
            Some("content-type,x-token")
        );
        assert_eq!(header("x-page").as_deref(), Some("1"));
        assert!(header("x-token").is_none() && header("content-type").is_none());
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

    /// The start refusal on the XHR route. It is the same one the `fetch`
    /// route reports and is checked in the same place — before the
    /// registration and before the spawn — so a refused `xhr.send()` leaves the
    /// settle machine exactly as it found it and leaves no task behind.
    #[test]
    fn an_xhr_that_cannot_be_made_is_refused_before_it_registers() {
        let (tracker, _clock) = tracker();
        let driver = driver(Rc::clone(&tracker));
        let request = FetchRequest::get(
            url("ftp://other.test/x"),
            url("http://origin.test/page"),
            Origin::Tracked,
        );
        let handle = XhrHandle {
            target: 0x0900_0000_0000_0007,
            request: 2,
        };

        let error = XhrDriver::start(&driver, request, handle).expect_err("not an http URL");

        assert_eq!(error.message(), UNSUPPORTED_SCHEME);
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
            task.task.await.expect("the task ends normally");

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

    /// `abort` ends the task the handle names, and releases the settle
    /// registration the request was holding.
    ///
    /// The release is the half that is easy to miss and expensive to get wrong:
    /// the task is what normally calls `note_request_done`, and aborting it drops
    /// its future *before* that line can run. Without the driver's own release the
    /// registration would stay forever, and the page would hold its settle open
    /// for a request that is over until the settle machine reclassified it
    /// (`settle.foreground_request_max`).
    ///
    /// The task is aborted before it has been polled once — `spawn_local` only
    /// queues it — so this is the in-flight case and not a race with the
    /// transport.
    #[test]
    fn aborting_an_xhr_ends_its_task_and_releases_the_settle() {
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
            let request = FetchRequest::get(
                url("http://127.0.0.1:1/data.json"),
                url("http://127.0.0.1:1/page"),
                Origin::Tracked,
            );
            XhrDriver::start(&driver, request, handle).expect("the request is started");
            assert_eq!(
                tracker.borrow().pending().foreground_requests,
                1,
                "the request is holding the settle open"
            );

            assert!(
                XhrDriver::abort(&driver, handle),
                "a request that is still running reports that it was ended"
            );
            assert_eq!(
                tracker.borrow().pending().foreground_requests,
                0,
                "and the registration goes with it, so the page is no longer waiting"
            );
            assert!(
                driver.inflight.borrow().is_empty(),
                "the ended request is not held any more, so a second `abort()` is a no-op"
            );
            assert!(
                !XhrDriver::abort(&driver, handle),
                "a handle the driver no longer holds reports nothing was terminated"
            );
            assert!(
                driver.xhrs.take_all().is_empty(),
                "ending a request is not answering it: no completion is pushed"
            );
        });
    }

    /// A request that has already finished is reported as nothing to end, and its
    /// settle registration is left alone.
    ///
    /// The entry is built by hand rather than driven, because the case is a state
    /// and not a race: production reaches it whenever an `abort()` arrives after
    /// the transport answered but before the next `start` swept the finished
    /// entry out of `inflight`. Calling `note_request_done` again would be
    /// harmless (`SettleTracker` removes by id and a second removal is a no-op),
    /// but reporting `true` would not: the engine drops the answer either way, so
    /// a driver that claimed to have terminated something would be wrong twice —
    /// once about the termination and once about the `false` the contract
    /// promises for "nothing was terminated, for any reason".
    #[test]
    fn aborting_a_finished_request_reports_nothing_was_terminated() {
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
            let finished = tokio::task::spawn_local(async {});
            for _ in 0..100 {
                if finished.is_finished() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(finished.is_finished(), "the task under test has ended");
            driver.inflight.borrow_mut().push(Inflight {
                xhr: Some(handle),
                resolver: None,
                request_id: RequestId(9),
                task: finished,
            });

            assert!(
                !XhrDriver::abort(&driver, handle),
                "there is nothing running under this handle any more"
            );
            assert_eq!(
                tracker.borrow().pending().foreground_requests,
                0,
                "and a finish is not an end: the settle is exactly as it was"
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

        let fetched = resolved(fetched_response(answered, &policy(&initiator), &mut None).await);

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

        let fetched = resolved(fetched_response(answered, &policy(&initiator), &mut None).await);

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
            driver.inflight.borrow_mut().push(Inflight {
                xhr: None,
                resolver: None,
                request_id: RequestId(1),
                task: handle,
            });
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
