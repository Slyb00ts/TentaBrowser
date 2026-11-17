//! The page: one navigation, one parse, one settle, pinned to one page
//! thread (`docs/spec/02-architecture.md` §3 and §5).
//!
//! Tier 1 only fetches and parses the document. JavaScript pages run scripts
//! through a page-thread runtime while parsing and settling.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::{MappedMutexGuard, Mutex, MutexGuard};
use tenta_core::{
    BudgetKind, Budgets, ContextId, Error, FrameId, NativeMemoryBudget, NativeMemoryFailure,
    PageId, RequestId, Result, Tier,
};
use tenta_dom::{Document as DomDocument, SharedDocument, SharedSink};
use tenta_html::{Encoding, ParseStatus, Parser, ParserOptions, PreloadHint};
use tenta_js::engine::{LifecycleEvent, PageEngineFactory};
use tenta_js::storage::PageStorage;
use tenta_net::policy::cookies::{CookieJar, Site};
use tenta_net::policy::error_names::ErrorName;
use tenta_net::request::RequestLimits;
use tenta_net::{Client, RedirectHop, Request, ResourceType, Response, Timing};
use tokio::sync::{Notify, broadcast, oneshot};
use tokio::task::JoinHandle;
use url::Url;

use crate::clock::Clock;
use crate::document::Document;
use crate::error_page::error_document;
use crate::event::{Commit, EVENT_BUFFER, FrameState, LoadStage, PageEvent, PageEvents};
use crate::fetch_policy::{FetchPolicy, RecordedHint};
use crate::runtime;
use crate::runtime::ScriptBudget;
use crate::script_cpu::{SCRIPT_CPU_WINDOW, ScriptCpu};
use crate::settle::{
    Origin, PendingScripts, RequestClass, Settle, SettleConfig, SettleState, SettleTracker,
};
use crate::task::{catch_unwind_future, panic_message};
use crate::thread::PageThread;

mod evaluate;
#[cfg(feature = "render")]
mod geometry;
#[cfg(feature = "render")]
mod image_loader;
mod modules;
mod retained;
#[cfg(feature = "render")]
mod screenshot;
#[cfg(feature = "render")]
mod sheet_loader;
#[cfg(feature = "render")]
pub(crate) use image_loader::{ImageLoader, PageImageHost};
#[cfg(feature = "render")]
pub(crate) use sheet_loader::SheetLoader;

pub use evaluate::{EvalOptions, Evaluation, FromRemote, World};
#[cfg(feature = "render")]
pub use geometry::{BoxQuads, ElementGeometry, Quad};

/// How many raw bytes of a response are kept so a late `<meta charset>` can
/// restart the parse from the first byte.
///
/// The sniffing prescan looks at the first 1 KiB, so a declaration that
/// changes the encoding lives well inside this window on any real page. Past
/// it the bytes are released: holding a whole 1 MB document twice to serve a
/// case that does not occur would cost more than the case is worth
/// (`docs/spec/14-memory.md` §1).
pub const RESTART_WINDOW: usize = 64 * 1024;

/// How much later than the page's deadline the transport's own deadline is.
const TRANSPORT_DEADLINE_GRACE: Duration = Duration::from_millis(250);

/// The next id for a request this page issues.
///
/// Shared by the page and its script driver (`crate::runtime`): a request is
/// identified in the settle machine and in the event stream by this id, so both
/// issuers must draw from the same sequence.
pub(crate) fn next_request_id() -> RequestId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    RequestId(NEXT.fetch_add(1, Ordering::Relaxed))
}

fn next_frame_id() -> FrameId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    FrameId(NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The viewport a page reports. Tier 3 uses it; Tier 1 only stores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f32,
    pub mobile: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            device_scale_factor: 1.0,
            mobile: false,
        }
    }
}

/// Options of one navigation.
#[derive(Clone, Debug)]
pub struct Goto {
    /// What the navigation waits for.
    pub wait_until: Settle,
    /// Deadline for the whole navigation, settle included.
    pub timeout: Duration,
    /// `Referer` header to send.
    pub referer: Option<String>,
    /// The entity body of a POST navigation (a form submission); `None` is a
    /// GET.
    pub post: Option<tenta_dom::PostData>,
    /// The document that started the navigation, for one a page begins
    /// itself: it decides `Origin`, `Sec-Fetch-Site` and the default
    /// `Referer`. `None` is a navigation the host issued.
    pub initiator: Option<Url>,
}

impl Default for Goto {
    fn default() -> Self {
        Self {
            wait_until: Settle::Load,
            timeout: Duration::from_secs(30),
            referer: None,
            post: None,
            initiator: None,
        }
    }
}

impl Goto {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn wait_until(mut self, wait_until: Settle) -> Self {
        self.wait_until = wait_until;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn referer(mut self, referer: impl Into<String>) -> Self {
        self.referer = Some(referer.into());
        self
    }

    /// Makes the navigation a POST with `body` as its entity body.
    pub fn post(mut self, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        self.post = Some(tenta_dom::PostData {
            content_type: content_type.into(),
            body,
        });
        self
    }

    /// Names the document that started the navigation.
    pub fn initiator(mut self, url: Url) -> Self {
        self.initiator = Some(url);
        self
    }
}

/// A guard that bumps the navigation generation if the goto future is dropped
/// before completion, so a dropped navigation doesn't install its document.
struct GotoGuard {
    answer: oneshot::Receiver<Result<NavigateOutcome>>,
    inner: Arc<PageInner>,
    generation: u64,
    completed: bool,
}

impl Drop for GotoGuard {
    fn drop(&mut self) {
        // If this guard is dropped before the future completes,
        // bump the generation so the navigation is cancelled.
        // Don't bump if the future completed normally (N-R3-3).
        if !self.completed {
            let mut state = self.inner.state.lock();
            if state.navigation_generation == self.generation {
                state.navigation_generation = state.navigation_generation.wrapping_add(1);
                // Wake any settle waiters so they check the new generation immediately (B-R3-1)
                drop(state);
                self.inner.generation_changed.notify_waiters();
            }
        }
    }
}

impl std::future::Future for GotoGuard {
    type Output = Result<NavigateOutcome>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::pin::Pin::new(&mut self.answer).poll(cx) {
            std::task::Poll::Ready(Ok(result)) => {
                self.completed = true;
                std::task::Poll::Ready(result)
            }
            std::task::Poll::Ready(Err(_)) => {
                self.completed = true;
                std::task::Poll::Ready(Err(Error::Closed))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

/// Who asked for a navigation, which decides whether its document starts a
/// new script CPU window (`crate::script_cpu`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Requester {
    /// The host: [`Page::goto`] and its siblings.
    Host,
    /// The document itself: `location`, a form submission, an activated link.
    Document,
}

/// A navigation that has started, from [`Page::begin_goto`].
pub struct PendingNavigation {
    guard: GotoGuard,
}

impl PendingNavigation {
    /// The `navigation_id` of this navigation's events.
    pub fn navigation_id(&self) -> u64 {
        self.guard.generation
    }

    /// Waits for the navigation to end, as [`Page::goto`] does.
    pub async fn finish(self) -> Result<Navigation> {
        Ok(self.guard.await?.navigation)
    }
}

struct NavigateOutcome {
    navigation: Navigation,
    evaluation: Option<Result<String>>,
}

/// What a navigation learned.
#[derive(Clone, Debug)]
pub struct Navigation {
    /// The URL the caller asked for.
    pub url: Url,
    /// Where the document came from after redirects.
    pub final_url: Url,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub content_type: Option<String>,
    /// The encoding the body was decoded with.
    pub encoding: String,
    /// The hops before `final_url`, oldest first.
    pub redirects: Vec<RedirectHop>,
    pub timing: Timing,
    /// Decoded response body bytes.
    pub bytes: u64,
    /// Whether the HTTP cache answered.
    pub from_cache: bool,
    pub protocol: &'static str,
    /// How the settle call ended.
    pub settle: SettleState,
    /// The `net::ERR_*` name when this navigation produced an error document.
    pub error: Option<String>,
    /// The navigation generation ID that produced this result.
    pub navigation_id: u64,
}

impl Navigation {
    /// The first value of `name`, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Whether the status is in the 2xx range.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Test-only behaviour a page can be built with.
#[derive(Clone, Copy, Debug, Default)]
#[doc(hidden)]
pub struct TestHooks {
    /// Panic inside the parse task, to prove a crash is contained
    /// (`docs/spec/02-architecture.md` §8).
    pub panic_in_parse: bool,
}

/// Everything a page needs before its first navigation.
pub struct PageConfig {
    pub id: PageId,
    pub context: ContextId,
    pub client: Client,
    pub thread: Arc<dyn PageThread>,
    pub clock: Arc<dyn Clock>,
    /// The ceiling this page may reach.
    pub max_tier: Tier,
    /// Whether scripts run. Tier 1 pages always have this off.
    pub javascript: bool,
    /// The JavaScript backend this page runs scripts with.
    ///
    /// `None` means the page never enters an engine: every script element is
    /// skipped and the page stays at Tier 1. It is a factory rather than an
    /// engine because a page thread holds one engine per page at a time
    /// (`docs/spec/05-javascript.md` §6) and the factory is what survives a
    /// crash (`docs/spec/02-architecture.md` §8).
    pub engine: Option<Arc<dyn PageEngineFactory>>,
    pub budgets: Budgets,
    pub settle: SettleConfig,
    /// BCP 47 tag that picks the fallback encoding when nothing declares one.
    pub locale: String,
    /// Headers added to every request this page issues (user agent and
    /// `Accept-Language` overrides).
    pub extra_headers: Vec<(String, String)>,
    pub viewport: Viewport,
    /// Lets text fall back to the platform fonts for scripts the bundled fonts
    /// lack (D-036). Off keeps rendering independent of the machine.
    pub system_fonts: bool,
    /// Gates every host file the page reads on its own behalf, such as the
    /// stylesheets of a `file:` document. Never widened by the page.
    pub fs_policy: tenta_core::FsPolicy,
    /// The browsing context's cookie jar, handed to the engine so
    /// `document.cookie` writes into the same store the network client sends
    /// from. It is a clone of one `Rc`-shared handle; see
    /// [`tenta_js::engine::PageEngineConfig::cookies`].
    pub cookies: CookieJar,
    /// The context's local areas and this page's session namespace.
    pub storage: PageStorage,
    /// The page's network observation point; inactive until a host uses it.
    pub net: crate::network::NetworkHub,
    /// Called once, from [`Page::close`], so whoever registered the page
    /// (its context's group table) can forget it without polling.
    pub on_close: Option<Arc<dyn Fn(PageId) + Send + Sync>>,
    #[doc(hidden)]
    pub test_hooks: TestHooks,
}

struct PageState {
    document: Option<DomDocument>,
    navigation: Option<Navigation>,
    hints: Vec<RecordedHint>,
    settle: Option<SettleState>,
    tier: Tier,
    crashed: Option<String>,
    native_memory_crash: Option<NativeMemoryFailure>,
    closed: bool,
    /// Generation counter for navigation cancellation. Each new navigation
    /// captures this value; if it changes before commit, the navigation was
    /// cancelled (by a concurrent goto or by close).
    navigation_generation: u64,
    /// The navigation generation of the currently installed document.
    document_navigation_id: u64,
    /// Whether the page keeps a live engine bound to its document. While it
    /// does, the tree is the engine's and `document` is empty.
    engine_held: bool,
    /// Whether a render has the document out of `document` right now. Another
    /// thread that asks for the tree meanwhile is told the page is busy.
    document_lent: bool,
    /// Navigations running on the page thread right now. A navigation builds
    /// its own engine at its first script, so a host entry may not build one
    /// for the document it is replacing.
    navigations_in_flight: usize,
    /// The title of the tree an engine keeps, which no other thread can read
    /// from the tree itself; refreshed whenever the tree's mutation generation
    /// moved since `title_generation`.
    title: Option<String>,
    title_generation: Option<u64>,
    /// The main frame's current document and how far its load got.
    frame: FrameState,
    /// The navigation that has started and has neither reached `load` nor
    /// been reported stopped.
    loading: Option<u64>,
}

impl PageState {
    /// The frame now shows the document `commit` describes, which has reached
    /// no stage yet.
    fn commit_frame(&mut self, commit: &Commit) {
        self.frame = FrameState {
            navigation_id: commit.navigation_id,
            commit: Some(commit.clone()),
            stages: Vec::new(),
        };
    }

    /// Re-reads the title of `document` if it changed since the last look.
    fn cache_title(&mut self, document: &DomDocument) {
        let generation = document.mutation_generation();
        if self.title_generation == Some(generation) {
            return;
        }
        self.title_generation = Some(generation);
        self.title = document.metadata().title.filter(|title| !title.is_empty());
    }
}

pub(crate) struct PageInner {
    /// This page, for the one caller that has only `&self`: a navigation the
    /// document's script asks for arrives through [`ScriptBudget`].
    this: std::sync::Weak<PageInner>,
    config: PageConfig,
    frame: FrameId,
    state: Mutex<PageState>,
    // Lazy event channel: allocated on first events() call (N-R3-5).
    // Before that, emit() is a no-op.
    events: Mutex<Option<broadcast::Sender<PageEvent>>>,
    // Notified when the generation changes (cancelled by goto or close).
    // Wakes settle waiters so they check the generation immediately (B-R3-1).
    generation_changed: Notify,
    /// Notified when a navigation stops being in flight, however it ends, for
    /// callers that need the document it installs to be settled in place.
    navigation_done: Notify,
    /// The page's cumulative script time. It lives here, not on a runtime,
    /// because a runtime is rebuilt for every navigation and the budget is a
    /// property of the page (`crate::script_cpu`).
    script_cpu: ScriptCpu,
    /// One native accounting state for the page, including all navigations
    /// and any Send wire that outlives a navigation's engine.
    native_memory: NativeMemoryBudget,
    /// Held for the length of one settle call over a kept engine, so two
    /// waiters never fight over the engine's driver or restart one tracker
    /// under each other.
    settle_gate: tokio::sync::Mutex<()>,
    /// The binding names engines expose. Separate from `state` so building an
    /// engine, which reads it, never contends with a holder of the state lock.
    bindings: Mutex<Vec<String>>,
    /// Scripts every new engine runs before the document's own, with the ids
    /// they were registered under.
    init_scripts: Mutex<Vec<(u64, String)>>,
    /// The viewport now in force: the configured one until [`Page::set_viewport`].
    viewport: Mutex<Viewport>,
}

impl PageInner {
    /// Makes `commit` the frame's document and announces it. Callers hold the
    /// state lock only to call [`PageState::commit_frame`]; the event goes out
    /// after.
    fn announce_commit(&self, commit: Commit) {
        self.emit(PageEvent::NavigationCommitted {
            frame: self.frame,
            commit,
        });
    }

    /// Called when a navigation commits its document. One the host asked for
    /// starts with an empty script CPU window; one the document asked for goes
    /// on spending the window it was started in (`crate::script_cpu`).
    fn start_script_window(&self, requester: Requester) {
        if requester == Requester::Host {
            self.script_cpu.restart();
        }
    }

    /// Records that the document of `navigation_id` reached `stage` and
    /// reports it, once. A stage of a document the frame no longer shows is
    /// dropped.
    fn reach(&self, navigation_id: u64, stage: LoadStage) {
        {
            let mut state = self.state.lock();
            let frame = &mut state.frame;
            if frame.navigation_id != navigation_id || frame.stages.contains(&stage) {
                return;
            }
            frame.stages.push(stage);
            if stage == LoadStage::Load && state.loading == Some(navigation_id) {
                state.loading = None;
            }
        }
        self.emit(PageEvent::Lifecycle {
            frame: self.frame,
            navigation_id,
            stage,
        });
    }

    /// Reports a navigation that ended without its document reaching `load`:
    /// a failed or timed-out load, or a stop. Once per navigation, and not
    /// for one a newer navigation replaced.
    fn finish_navigation(&self, navigation_id: u64) {
        let stopped = {
            let mut state = self.state.lock();
            (state.loading == Some(navigation_id)).then(|| state.loading.take())
        };
        if stopped.is_some() {
            self.emit(PageEvent::NavigationStopped {
                frame: self.frame,
                navigation_id,
            });
        }
    }

    fn emit(&self, event: PageEvent) {
        // Events are only sent if there's a channel and it has subscribers.
        if let Some(sender) = self.events.lock().as_ref() {
            // No subscribers is not an error: events are optional.
            let _ = sender.send(event);
        }
    }

    fn crash_reason(&self) -> Option<String> {
        self.state.lock().crashed.clone()
    }

    fn mark_crashed(&self, reason: &str) {
        self.mark_crashed_with(reason, None);
    }

    fn native_memory_crash(&self) -> Option<NativeMemoryFailure> {
        self.state.lock().native_memory_crash
    }

    fn mark_crashed_with(&self, reason: &str, native_memory: Option<NativeMemoryFailure>) {
        {
            let mut state = self.state.lock();
            // Several unwinding paths can observe the same terminal failure.
            // Keep its first cause and emit one crash event for this page.
            if state.crashed.is_some() {
                return;
            }
            state.crashed = Some(reason.to_owned());
            state.native_memory_crash = native_memory;
            state.document = None;
            // A crash is raised on the page thread while the navigation that
            // caused it is parked in `settle_loop`, waiting on
            // `generation_changed`. The settled pattern in this file is
            // "register, then re-check": bumping here is what a waiter that has
            // not yet re-checked sees as a generation change, and it re-checks
            // and finds the crash; the notification below wakes one already
            // parked. Without both, a crash inside an engine turn would leave
            // the navigation waiting for a deadline that `evaluate_time` may
            // place far away.
            state.navigation_generation = state.navigation_generation.wrapping_add(1);
        }
        self.generation_changed.notify_waiters();
        self.emit(PageEvent::Crashed {
            reason: reason.to_owned(),
        });
    }
}

/// The page is where script time accumulates: the runtime reports what one
/// entry spent, [`ScriptCpu`] holds the total, and this is the one place the
/// two meet.
///
/// Only the first crossing crashes the page. A terminated entry — the outcome
/// of the per-entry deadline — is deliberately *not* a crash: termination is
/// how a page survives a runaway, and only burning the cumulative budget is
/// the fact that the page has not made progress (`docs/spec/05-javascript.md`
/// §11). This is called from the page thread, inside the engine entry, so the
/// crash lands while the navigation driving that entry is parked.
impl ScriptBudget for PageInner {
    #[cfg(feature = "render")]
    fn render_settings(&self) -> Option<crate::render::RenderSettings> {
        (self.config.max_tier >= Tier::Render).then(|| crate::render::RenderSettings {
            viewport: *self.viewport.lock(),
            system_fonts: self.config.system_fonts,
        })
    }

    fn charge(&self, elapsed: Duration) {
        if !self.script_cpu.charge(elapsed) {
            return;
        }
        let reason = format!(
            "the page ran out of script time: {} ms spent of a {} ms budget within {} s",
            self.script_cpu.spent().as_millis(),
            self.script_cpu.budget().as_millis(),
            self.script_cpu.window().as_secs(),
        );
        self.mark_crashed(&reason);
    }

    fn crash_reason(&self) -> Option<String> {
        PageInner::crash_reason(self)
    }

    fn native_memory(&self) -> NativeMemoryBudget {
        self.native_memory.clone()
    }

    fn entry_terminated(&self, entry: &'static str) {
        self.emit(PageEvent::ScriptTerminated { entry });
    }

    fn request_navigation(&self, request: tenta_dom::NavigationRequest) {
        if let Some(inner) = self.this.upgrade() {
            start_requested_navigation(inner, request);
        }
    }

    fn request_imports(
        &self,
        imports: Vec<tenta_js::modules::DynamicImport>,
        context: runtime::ImportContext,
    ) {
        if let Some(inner) = self.this.upgrade() {
            start_dynamic_imports(inner, imports, context);
        }
    }

    fn emit(&self, event: PageEvent) {
        PageInner::emit(self, event);
    }

    fn has_subscribers(&self) -> bool {
        self.events
            .lock()
            .as_ref()
            .is_some_and(|sender| sender.receiver_count() > 0)
    }

    fn binding_names(&self) -> Vec<String> {
        self.bindings.lock().clone()
    }

    fn init_scripts(&self) -> Vec<String> {
        self.init_scripts
            .lock()
            .iter()
            .map(|(_, source)| source.clone())
            .collect()
    }

    fn crash_native_memory(&self, failure: NativeMemoryFailure) {
        self.mark_crashed_with(
            &format!(
                "{} budget exceeded in {}",
                failure.kind,
                failure.component.name()
            ),
            Some(failure),
        );
    }
}

fn native_memory_budget_error(failure: NativeMemoryFailure) -> Error {
    Error::BudgetExceeded {
        kind: failure.kind,
        component: Some(failure.component.name().to_owned()),
    }
}

/// A page. Cloning gives another handle to the same page; the state lives on
/// the page thread (`docs/spec/11-library-api.md` §1).
#[derive(Clone)]
pub struct Page {
    inner: Arc<PageInner>,
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page")
            .field("id", &self.inner.config.id)
            .field("tier", &self.inner.state.lock().tier)
            .finish_non_exhaustive()
    }
}

/// The `about:blank` document every page starts with, so a script can run
/// before the first navigation. The engine for it is built on first use, not
/// at creation: most pages navigate at once, which would discard it unused.
fn initial_document(javascript: bool) -> DomDocument {
    let url = Url::parse("about:blank").expect("a valid URL");
    let (sink, shared) = SharedSink::with_new_document();
    shared.borrow_mut().set_url(Some(url.clone()));
    let mut parser = Parser::new(
        sink,
        ParserOptions {
            scripting_enabled: javascript,
            ..ParserOptions::default()
        },
    );
    parser.feed_last(b"<html><head></head><body></body></html>");
    drop(parser.finish());
    let mut document = match Rc::try_unwrap(shared) {
        Ok(cell) => cell.into_inner(),
        // The parser's sink is gone, so this is the only owner; the fallback
        // keeps a page usable (without `<body>`) rather than panicking.
        Err(_) => DomDocument::with_url(url),
    };
    document.set_ready_state(tenta_dom::ReadyState::Complete);
    document
}

impl Page {
    /// Builds a page pinned to the thread in `config`.
    pub fn new(config: PageConfig) -> Page {
        let frame = next_frame_id();
        config.net.bind_frame(frame);
        Page {
            inner: Arc::new_cyclic(|this| PageInner {
                this: this.clone(),
                frame,
                state: Mutex::new(PageState {
                    document: Some(initial_document(config.javascript)),
                    navigation: None,
                    hints: Vec::new(),
                    settle: None,
                    tier: Tier::Net,
                    crashed: None,
                    native_memory_crash: None,
                    closed: false,
                    navigation_generation: 0,
                    document_navigation_id: 0,
                    engine_held: false,
                    document_lent: false,
                    navigations_in_flight: 0,
                    title: None,
                    title_generation: None,
                    frame: FrameState::initial(),
                    loading: None,
                }),
                // Lazy event channel: allocated on first events() call (N-R3-5)
                events: Mutex::new(None),
                // Notified when generation changes (B-R3-1)
                generation_changed: Notify::new(),
                navigation_done: Notify::new(),
                script_cpu: ScriptCpu::new(config.budgets.script_cpu, SCRIPT_CPU_WINDOW),
                native_memory: NativeMemoryBudget::for_page(config.budgets.page_memory_mb),
                settle_gate: tokio::sync::Mutex::new(()),
                bindings: Mutex::new(Vec::new()),
                init_scripts: Mutex::new(Vec::new()),
                viewport: Mutex::new(config.viewport),
                config,
            }),
        }
    }

    pub fn id(&self) -> PageId {
        self.inner.config.id
    }

    pub fn context(&self) -> ContextId {
        self.inner.config.context
    }

    pub fn main_frame(&self) -> FrameId {
        self.inner.frame
    }

    /// The page's network events and overrides (`crate::network`).
    pub fn network(&self) -> &crate::network::NetworkHub {
        &self.inner.config.net
    }

    /// The index of the page thread this page is pinned to.
    pub fn thread_index(&self) -> usize {
        self.inner.config.thread.index()
    }

    pub fn viewport(&self) -> Viewport {
        *self.inner.viewport.lock()
    }

    pub fn max_tier(&self) -> Tier {
        self.inner.config.max_tier
    }

    /// The highest tier this page has actually reached.
    pub fn tier(&self) -> Tier {
        self.inner.state.lock().tier
    }

    /// The event stream. Subscribe before the operation you want to observe.
    pub fn events(&self) -> PageEvents {
        // Lazily allocate the broadcast channel on first subscriber (N-R3-5).
        // Events emitted before the first subscriber are dropped.
        let mut events_lock = self.inner.events.lock();
        let sender =
            events_lock.get_or_insert_with(|| broadcast::channel::<PageEvent>(EVENT_BUFFER).0);
        PageEvents::new(sender.subscribe())
    }

    /// The reason this page crashed, if it did.
    pub fn crash_reason(&self) -> Option<String> {
        self.inner.crash_reason()
    }

    pub fn is_closed(&self) -> bool {
        self.inner.state.lock().closed
    }

    /// How the last settle call ended.
    pub fn settle_state(&self) -> Option<SettleState> {
        self.inner.state.lock().settle
    }

    /// What the last navigation learned.
    pub fn navigation(&self) -> Option<Navigation> {
        self.inner.state.lock().navigation.clone()
    }

    /// The main frame's current document and the load stages it has reached.
    ///
    /// The initial `about:blank` document has no commit and has reached every
    /// stage. A navigation takes the frame over at its response head, not
    /// before.
    pub fn frame_state(&self) -> FrameState {
        self.inner.state.lock().frame.clone()
    }

    /// Cancels the navigation in flight, if any, like a browser's stop button:
    /// its `goto` fails as aborted. Returns whether a navigation was in flight.
    pub fn stop_loading(&self) -> bool {
        let stopped = {
            let mut state = self.inner.state.lock();
            if state.navigations_in_flight == 0 {
                return false;
            }
            state.navigation_generation = state.navigation_generation.wrapping_add(1);
            state.loading.take()
        };
        self.inner.generation_changed.notify_waiters();
        if let Some(navigation_id) = stopped {
            self.inner.emit(PageEvent::NavigationStopped {
                frame: self.inner.frame,
                navigation_id,
            });
        }
        true
    }

    /// Registers `source` to run in every document the page loads from now on,
    /// before any script of the document itself, and returns the id
    /// [`Page::remove_init_script`] takes. A document runs it when its engine is
    /// built: at its first script, or at the first evaluation of a document
    /// that has none. The document the page shows now is not affected.
    pub fn add_init_script(&self, source: &str) -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        self.inner.init_scripts.lock().push((id, source.to_owned()));
        id
    }

    /// Stops running a script registered by [`Page::add_init_script`] in new
    /// documents. Returns whether the id was registered.
    pub fn remove_init_script(&self, id: u64) -> bool {
        let mut scripts = self.inner.init_scripts.lock();
        let before = scripts.len();
        scripts.retain(|(candidate, _)| *candidate != id);
        scripts.len() != before
    }

    /// Subresource hints the last parse recorded.
    pub fn recorded_hints(&self) -> Vec<RecordedHint> {
        self.inner.state.lock().hints.clone()
    }

    /// The document's title, for a page that keeps an engine as well as one that
    /// does not. For a kept engine it is the title as of the last engine entry,
    /// so it can lag a script's very latest write by one turn.
    pub fn title(&self) -> Option<String> {
        let state = self.inner.state.lock();
        match &state.document {
            Some(document) => document.metadata().title.filter(|title| !title.is_empty()),
            None => state.title.clone(),
        }
    }

    /// The parsed document, for extraction.
    ///
    /// Empty while the page keeps a live engine: the tree is then the engine's,
    /// on the page thread, and [`Page::with_engine`] is the way to read it.
    /// [`Page::take_document`] ends the engine and moves the tree out.
    ///
    /// The returned guard holds the page's state lock, so drop it before
    /// calling another method on the same page: keeping it alive across
    /// `goto` or `close` would block the page thread. [`take_document`] moves
    /// the document out instead and needs no guard.
    ///
    /// [`take_document`]: Self::take_document
    pub fn document(&self) -> Option<MappedMutexGuard<'_, DomDocument>> {
        let state = self.inner.state.lock();
        state.document.as_ref()?;
        Some(MutexGuard::map(state, |state| {
            state
                .document
                .as_mut()
                .expect("checked before the projection")
        }))
    }

    /// Takes the parsed document and the navigation out of the page.
    ///
    /// A page that keeps an engine ends it first, on the page thread: the tree
    /// can only leave once nothing is bound to it, and its realm goes with it.
    pub async fn take_document(&self) -> Result<Document> {
        retained::release_engine(&self.inner).await?;
        let mut state = self.inner.state.lock();
        if let Some(reason) = &state.crashed {
            return Err(Error::Crashed {
                reason: reason.clone(),
            });
        }
        let dom = match state.document.take() {
            Some(dom) => dom,
            None if state.document_lent => return Err(Error::Busy),
            None => {
                return Err(Error::InvalidArgument {
                    reason: "the page has no document yet".to_owned(),
                });
            }
        };
        let navigation = state
            .navigation
            .clone()
            .ok_or_else(|| Error::invalid("the page has not navigated"))?;
        let hints = std::mem::take(&mut state.hints);
        let navigation_id = state.document_navigation_id;
        Ok(Document::new(dom, navigation, hints, navigation_id))
    }

    /// Navigates to `url` and waits for `options.wait_until`.
    ///
    /// The document request runs on the io runtime; the parse and the settle
    /// run on this page's thread. Dropping the returned future cancels the
    /// navigation entirely, leaving the page as if the navigation never happened.
    pub async fn goto(&self, url: &str, options: Goto) -> Result<Navigation> {
        let url =
            Url::parse(url).map_err(|err| Error::invalid(format!("invalid URL {url:?}: {err}")))?;
        self.goto_url(url, options).await
    }

    /// Navigates, waits for `options.wait_until`, then evaluates `source` in
    /// the loaded document's JavaScript world.
    ///
    /// The completion is converted to a string by JavaScript. A returned
    /// Promise is not awaited, and work queued by this expression is not part
    /// of the navigation's preceding settle. The evaluation happens inside the
    /// navigation, before it returns, and the page then keeps the same engine
    /// like any other navigation.
    pub async fn goto_with_eval(
        &self,
        url: &str,
        options: Goto,
        source: &str,
    ) -> Result<(Navigation, String)> {
        self.require_javascript()?;
        let url =
            Url::parse(url).map_err(|err| Error::invalid(format!("invalid URL {url:?}: {err}")))?;
        let outcome = self
            .goto_url_inner(url, options, Some(source.to_owned()))
            .await?;
        let value = outcome
            .evaluation
            .expect("goto_with_eval always requests an evaluation")?;
        Ok((outcome.navigation, value))
    }

    /// [`Page::goto`] with an already parsed URL.
    pub async fn goto_url(&self, url: Url, options: Goto) -> Result<Navigation> {
        Ok(self.goto_url_inner(url, options, None).await?.navigation)
    }

    async fn goto_url_inner(
        &self,
        url: Url,
        options: Goto,
        evaluation: Option<String>,
    ) -> Result<NavigateOutcome> {
        self.begin_navigation(url, options, evaluation, Requester::Host)?
            .await
    }

    /// Starts navigating to `url` and returns at once with the navigation's
    /// identity, which the events about it carry as `navigation_id`.
    ///
    /// Dropping the [`PendingNavigation`] cancels the navigation, like
    /// dropping a [`Page::goto`] future.
    pub fn begin_goto(&self, url: &str, options: Goto) -> Result<PendingNavigation> {
        let url =
            Url::parse(url).map_err(|err| Error::invalid(format!("invalid URL {url:?}: {err}")))?;
        let guard = self.begin_navigation(url, options, None, Requester::Host)?;
        Ok(PendingNavigation { guard })
    }

    fn begin_navigation(
        &self,
        mut url: Url,
        options: Goto,
        evaluation: Option<String>,
        requester: Requester,
    ) -> Result<GotoGuard> {
        // Credentials in the URL never reach the wire (tenta-net strips them)
        // and never show in `Navigation.url`, events or the document URL,
        // as `location.href` never shows them in Chrome.
        if url.has_authority() && (!url.username().is_empty() || url.password().is_some()) {
            let _ = url.set_username("");
            let _ = url.set_password(None);
        }
        let generation = {
            let mut state = self.inner.state.lock();
            if state.closed {
                return Err(Error::Closed);
            }
            if let Some(reason) = &state.crashed {
                return Err(Error::Crashed {
                    reason: reason.clone(),
                });
            }
            // Bump generation to cancel any previous in-flight navigation
            state.navigation_generation = state.navigation_generation.wrapping_add(1);
            let generation = state.navigation_generation;
            // Wake any settle waiters so they check the new generation immediately (B-R3-1)
            drop(state);
            self.inner.generation_changed.notify_waiters();
            generation
        };
        let inner = Arc::clone(&self.inner);
        let reporter = Arc::clone(&self.inner);
        let thread = Arc::clone(&self.inner.config.thread);
        let inner_for_drop = Arc::clone(&self.inner);
        let (reply, answer) = oneshot::channel();
        thread.post_job(Box::new(move || {
            Box::pin(async move {
                let (result, panicked) = match catch_unwind_future(navigate(
                    inner, url, options, generation, evaluation, requester,
                ))
                .await
                {
                    Ok(result) => (result, false),
                    Err(payload) => {
                        let reason = panic_message(&payload);
                        reporter.mark_crashed(&reason);
                        (Err(Error::Crashed { reason }), true)
                    }
                };
                // A crash that `navigate` returned as an ordinary error — a page
                // that burned its script budget — is normalized here: the page's
                // own reason is the truth, and the aborted navigation it caused
                // (an `Aborted` net error, or the `Crashed` the checks inside
                // produced) must not mask it.
                let result = if panicked {
                    // A native unwind outranks any simultaneous budget flag.
                    result
                } else {
                    match (result, reporter.crash_reason()) {
                        (_, Some(reason)) => Err(reporter
                            .native_memory_crash()
                            .map(native_memory_budget_error)
                            .unwrap_or(Error::Crashed { reason })),
                        (result, _) => result,
                    }
                };
                reporter.finish_navigation(generation);
                let _ = reply.send(result);
            })
        }))?;
        Ok(GotoGuard {
            answer,
            inner: inner_for_drop,
            generation,
            completed: false,
        })
    }

    /// The synchronous half of [`Page::close`]: marks the page closed, cancels
    /// any in-flight navigation by bumping the generation, wakes settle waiters
    /// and runs the `on_close` hook. Returns `false` if the page was already
    /// closed. A closed page's `goto` fails and its in-flight navigation aborts
    /// and drops its socket; the DOM is reclaimed when the last handle drops.
    /// `BrowserContext::close` uses this to close its pages without an await.
    pub fn begin_close(&self) -> bool {
        let engine_held = {
            let mut state = self.inner.state.lock();
            if state.closed {
                return false;
            }
            state.closed = true;
            // Bump generation to cancel any in-flight navigation; close is terminal
            state.navigation_generation = state.navigation_generation.wrapping_add(1);
            // Wake any settle waiters so they check the new generation and closed flag immediately (B-R3-1)
            let engine_held = state.engine_held;
            drop(state);
            self.inner.generation_changed.notify_waiters();
            engine_held
        };
        // A kept engine only the page thread can drop, and a context closing its
        // pages has no await to wait for it with.
        if engine_held {
            retained::teardown_later(&self.inner);
        }
        #[cfg(feature = "render")]
        screenshot::release_later(&self.inner);
        // Area wrappers may outlive the page runtime; close the session here
        // so they cannot retain or write page data after terminal close.
        self.inner.config.storage.close_session();
        if let Some(on_close) = &self.inner.config.on_close {
            on_close(self.inner.config.id);
        }
        true
    }

    /// Closes the page. The DOM is dropped on the page thread that built it,
    /// so its arenas return to that thread's free lists
    /// (`docs/spec/14-memory.md` §4). Any in-flight navigation is cancelled.
    pub async fn close(&self) -> Result<()> {
        if !self.begin_close() {
            return Ok(());
        }
        let inner = Arc::clone(&self.inner);
        let thread = Arc::clone(&self.inner.config.thread);
        let (reply, answer) = oneshot::channel();
        let posted = thread.post_task(Box::new(move || {
            retained::teardown(&inner);
            let _ = reply.send(());
        }));
        match posted {
            // The thread is gone; the state is dropped with the last handle.
            Err(_) => Ok(()),
            Ok(()) => {
                let _ = answer.await;
                Ok(())
            }
        }
    }
}

/// Counts a navigation as in flight for as long as it runs, however it ends.
struct InFlight(Arc<PageInner>);

impl InFlight {
    fn enter(inner: &Arc<PageInner>) -> InFlight {
        inner.state.lock().navigations_in_flight += 1;
        InFlight(Arc::clone(inner))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.state.lock().navigations_in_flight -= 1;
        self.0.navigation_done.notify_waiters();
    }
}

/// How long a navigation the page started itself may take.
const REQUESTED_NAVIGATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Starts the navigation the page's own script asked for: a `location`
/// assignment, a form submission or an activated link. It is an ordinary
/// navigation with no caller awaiting it, so its outcome is what the page
/// shows and reports: the new document or the error document, the events, and
/// [`Page::navigation`]. The page counts it as in flight from this moment, so
/// a host that asks right after the script entry that caused it sees it.
///
/// Only `http` and `https` targets are followed, the same as for a navigation
/// an action reports to a host; anything else is dropped.
fn start_requested_navigation(inner: Arc<PageInner>, request: tenta_dom::NavigationRequest) {
    if !matches!(request.url.scheme(), "http" | "https") {
        tracing::debug!(
            page = ?inner.config.id,
            scheme = request.url.scheme(),
            "a navigation the page asked for was not followed: only http and https are"
        );
        return;
    }
    let initiator = inner
        .state
        .lock()
        .navigation
        .as_ref()
        .map(|navigation| navigation.final_url.clone());
    let options = Goto {
        timeout: REQUESTED_NAVIGATION_TIMEOUT,
        post: request.post,
        initiator,
        ..Goto::default()
    };
    let in_flight = InFlight::enter(&inner);
    let thread = Arc::clone(&inner.config.thread);
    let page = Page {
        inner: Arc::clone(&inner),
    };
    let guard = match page.begin_navigation(request.url, options, None, Requester::Document) {
        Ok(guard) => guard,
        // The page is closed or crashed: there is nothing to navigate.
        Err(_) => return,
    };
    let posted = thread.post_job(Box::new(move || {
        Box::pin(async move {
            let _in_flight = in_flight;
            if let Err(error) = guard.await {
                tracing::debug!(%error, "a navigation the page asked for did not complete");
            }
        })
    }));
    if posted.is_err() {
        tracing::debug!("the page thread is shutting down; the navigation was not started");
    }
}

/// The navigation itself, running on the page thread.
/// `generation` is the navigation generation at the time goto_url was called;
/// if it changes, a concurrent goto or close cancelled this navigation.
async fn navigate(
    inner: Arc<PageInner>,
    url: Url,
    options: Goto,
    generation: u64,
    evaluation: Option<String>,
    requester: Requester,
) -> Result<NavigateOutcome> {
    let _in_flight = InFlight::enter(&inner);
    // Check if this navigation was cancelled; if so, return an aborted error.
    let check_cancelled = |inner: &Arc<PageInner>| -> Result<()> {
        let state = inner.state.lock();
        if state.navigation_generation != generation {
            return Err(Error::Net {
                code: ErrorName::Aborted.as_str().to_string(),
                message: "navigation was cancelled by a concurrent goto or close".into(),
            });
        }
        Ok(())
    };
    let config = &inner.config;
    let started = config.clock.now();
    let deadline = started + options.timeout;
    let timeout_millis = options.timeout.as_millis() as u64;

    // The tracker lives behind a shared cell because the engine's provenance
    // reports reach it from inside an engine call, while the page feeds it
    // from here: both sides borrow it for one operation and no longer
    // (`crates/tenta-page/src/runtime.rs`).
    let tracker = Rc::new(RefCell::new(
        SettleTracker::new(
            config.settle,
            options.wait_until.clone(),
            Arc::clone(&config.clock),
        )
        .with_timeout(options.timeout),
    ));
    tracker.borrow_mut().note_navigation_started();
    // Commit immediately to reset idle windows for the new navigation (M-R2-6).
    // This ensures that the document request we're about to start is tracked
    // correctly and the idle windows are restarted when the request completes.
    tracker.borrow_mut().note_navigation_commit();

    let request_id = next_request_id();
    let mut request = build_request(config, &url, &options)?;
    let mut tracked = config.net.begin(&mut request, request_id, Some(generation));
    // A navigation that a newer one already replaced never started, as far as
    // an observer is concerned: the newer one is the one it should attribute
    // the next commit to.
    let current = {
        let mut state = inner.state.lock();
        let current = state.navigation_generation == generation;
        if current {
            state.loading = Some(generation);
        }
        current
    };
    if current {
        inner.emit(PageEvent::NavigationStarted {
            frame: inner.frame,
            navigation_id: generation,
            url: url.clone(),
        });
    }
    inner.emit(PageEvent::Request {
        id: request_id,
        url: url.clone(),
        method: if options.post.is_some() {
            "POST"
        } else {
            "GET"
        }
        .to_owned(),
        resource_type: "document",
    });
    tracker
        .borrow_mut()
        .note_request_started(request_id, RequestClass::Document, Origin::Tracked);

    // The request races the navigation generation (B-R4-1): another `goto`
    // or `close()` while the head is outstanding aborts it at once instead
    // of at the deadline. The notified future is registered before the
    // generation is re-read, so a bump between the two is never lost.
    let send_future = config.client.send(request);
    let at = tokio::time::Instant::from_std(deadline);
    tokio::pin!(send_future);
    let cancelled = || Error::Net {
        code: ErrorName::Aborted.as_str().to_string(),
        message: "navigation was cancelled".into(),
    };
    let sent = loop {
        let generation_notified = inner.generation_changed.notified();
        tokio::pin!(generation_notified);
        // `notify_waiters` only reaches registered waiters: enable before
        // the check, or a bump between the check and the first poll is lost.
        generation_notified.as_mut().enable();
        {
            let state = inner.state.lock();
            if state.navigation_generation != generation || state.closed {
                return Err(cancelled());
            }
        }
        tokio::select! {
            biased;
            _ = &mut generation_notified => {
                let state = inner.state.lock();
                if state.navigation_generation != generation || state.closed {
                    return Err(cancelled());
                }
            }
            result = &mut send_future => break Ok(result),
            _ = tokio::time::sleep_until(at) => {
                break Err(Error::timeout("navigation", timeout_millis));
            }
        }
    };

    let response = match sent {
        Ok(Ok(response)) => response,
        Ok(Err(net_error)) => {
            if let Some(tracked) = &mut tracked {
                tracked.failed(&net_error);
            }
            let raw: Error = net_error.into();
            let (name, message) = net_error_parts(&raw);
            inner.emit(PageEvent::RequestFailed {
                id: request_id,
                url: url.clone(),
                error: name.clone(),
            });
            let mapped = navigation_error(raw, config.budgets.network_bytes, timeout_millis);
            // A failed navigation replaces the document with the error page
            // (`docs/spec/20-platform-behaviour.md` §1); a deadline that
            // expires before the response head does the same with
            // ERR_TIMED_OUT (11 §1 timeout matrix). Anything else (a closed
            // page) leaves the document alone.
            let error_page = match &mapped {
                Error::Net { .. } | Error::NetDenied { .. } => {
                    Some((name.as_str(), message.as_str()))
                }
                Error::Timeout { .. } => Some((
                    ErrorName::TimedOut.as_str(),
                    "navigation deadline exceeded before the response head",
                )),
                _ => None,
            };
            if let Some((code, message)) = error_page
                && !commit_error_document(
                    &inner,
                    &url,
                    code,
                    message,
                    started,
                    config.clock.now(),
                    generation,
                )
            {
                // A newer navigation or a close won the page meanwhile.
                return Err(Error::Net {
                    code: ErrorName::Aborted.as_str().to_string(),
                    message: "navigation was cancelled".into(),
                });
            }
            if error_page.is_some() {
                inner.start_script_window(requester);
            }
            return Err(mapped);
        }
        Err(timeout_error) => {
            let raw: Error = timeout_error;
            // For pre-head timeout, emit ERR_TIMED_OUT, consistent with the document
            // that will be created (N-R3-1). Determine the error code before consuming raw.
            let is_timeout = matches!(&raw, Error::Timeout { .. });
            let (name, message) = if is_timeout {
                (
                    ErrorName::TimedOut.as_str().to_string(),
                    "navigation deadline exceeded before the response head".to_string(),
                )
            } else {
                net_error_parts(&raw)
            };
            if let Some(tracked) = &mut tracked {
                tracked.failed_with(&name);
            }
            inner.emit(PageEvent::RequestFailed {
                id: request_id,
                url: url.clone(),
                error: name.clone(),
            });
            let mapped = navigation_error(raw, config.budgets.network_bytes, timeout_millis);
            // A failed navigation replaces the document with the error page
            // (`docs/spec/20-platform-behaviour.md` §1); a deadline that
            // expires before the response head does the same with
            // ERR_TIMED_OUT (11 §1 timeout matrix). Anything else (a closed
            // page) leaves the document alone.
            let error_page = match &mapped {
                Error::Net { .. } | Error::NetDenied { .. } => {
                    Some((name.as_str(), message.as_str()))
                }
                Error::Timeout { .. } => Some((
                    ErrorName::TimedOut.as_str(),
                    "navigation deadline exceeded before the response head",
                )),
                _ => None,
            };
            if let Some((code, message)) = error_page
                && !commit_error_document(
                    &inner,
                    &url,
                    code,
                    message,
                    started,
                    config.clock.now(),
                    generation,
                )
            {
                // A newer navigation or a close won the page meanwhile.
                return Err(Error::Net {
                    code: ErrorName::Aborted.as_str().to_string(),
                    message: "navigation was cancelled".into(),
                });
            }
            if error_page.is_some() {
                inner.start_script_window(requester);
            }
            return Err(mapped);
        }
    };

    // Check if this navigation was cancelled before continuing with the parse
    check_cancelled(&inner)?;
    // The response head commits the navigation: the previous document, and the
    // engine the page kept for it, end here. A navigation dropped before this
    // point leaves them as they were.
    retained::release(&inner);

    let final_url = response.url.clone();
    let status = response.status.as_u16();
    let headers: Vec<(String, String)> = response
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let content_type = response
        .header("content-type")
        .map(|value| value.trim().to_owned());

    // Emit per-hop events for each redirect in the chain (N-R3-6).
    // For each hop, emit a Response event with the real 3xx status, then a
    // Request event for the next hop. Finally emit the Response for the final URL.
    for (i, hop) in response.redirects.iter().enumerate() {
        // Emit Response for this hop with its real status
        // BLOCKING-3: Use from_cache field to report whether the redirect was cached.
        inner.emit(PageEvent::Response {
            id: request_id,
            url: hop.url.clone(),
            status: hop.status,
            from_cache: hop.from_cache,
        });

        // Emit Request for the next hop (unless this is the last redirect)
        if i < response.redirects.len() - 1 {
            let next_hop = &response.redirects[i + 1];
            inner.emit(PageEvent::Request {
                id: request_id,
                url: next_hop.url.clone(),
                method: "GET".to_owned(),
                resource_type: "document",
            });
        } else {
            // After the last redirect hop, emit Request for the final URL
            inner.emit(PageEvent::Request {
                id: request_id,
                url: final_url.clone(),
                method: "GET".to_owned(),
                resource_type: "document",
            });
        }
    }

    inner.emit(PageEvent::Response {
        id: request_id,
        url: final_url.clone(),
        status,
        from_cache: response.from_cache,
    });
    if let Some(tracked) = &mut tracked {
        tracked.response(&response);
    }
    config.net.note_document(generation, &final_url);
    let commit = Commit {
        navigation_id: generation,
        url: final_url.clone(),
        mime_type: content_type.as_deref().map(mime_of).unwrap_or_default(),
        status,
        error: None,
        unreachable_url: None,
    };
    inner.state.lock().commit_frame(&commit);
    inner.start_script_window(requester);
    inner.announce_commit(commit);

    tracker.borrow_mut().note_parser_running(true);
    // The `defer`/`async` scripts of this navigation, and the fetches they
    // started. It is created here, on this navigation's own stack, rather than
    // inside the parse: an `async` script may still be in flight after the
    // parse has handed the document over, and a queue that died with
    // `parse_body` would abort it.
    let mut jobs = ScriptJobs::new(Rc::clone(&tracker), generation);
    let parsed = parse_body(
        &inner,
        response,
        BodyMeta {
            final_url: &final_url,
            content_type: &content_type,
            tracked: tracked.take(),
        },
        deadline,
        timeout_millis,
        generation,
        &mut jobs,
    )
    .await
    .inspect_err(|_| {
        tracker.borrow_mut().note_parser_running(false);
    })?;
    let ParsedBody {
        document,
        hints,
        bytes,
        encoding,
        timing,
        from_cache,
        protocol,
        redirects,
        mid_body_timeout,
    } = parsed;

    // The tree is taken into a `DeferredDocument` here, not at the install
    // point below, because `parse_body` disarmed its own cleanup before
    // returning: from that moment nothing else owns the engine the parse may
    // have built, and every early return between here and the install point
    // would strand it in the runtime table with its document. The gap is one
    // `check_cancelled` wide, and a concurrent `goto`/`close` bumping the
    // navigation generation is what lands in it.
    let mut deferred = DeferredDocument::new(inner.config.id, document);

    // A document with scripts builds its engine at the first of them, which runs
    // the init scripts first. One without scripts would otherwise build none, and
    // an init script that adds a `DOMContentLoaded` or `load` listener would
    // never be heard, so it gets its engine here, before either event.
    if !inner.init_scripts.lock().is_empty()
        && inner.config.javascript
        && inner.config.engine.is_some()
        && let Some(document) = deferred.shared()
    {
        runtime::ensure_runtime(
            inner.config.id,
            document.clone(),
            &inner.config,
            Rc::clone(&tracker),
            Arc::clone(&inner) as Arc<dyn ScriptBudget>,
            generation,
        )
        .map_err(Error::unsupported)?;
        runtime::note_parse_finished(inner.config.id, document);
    }

    tracker.borrow_mut().note_parser_running(false);
    tracker.borrow_mut().note_request_done(request_id);
    // `DOMContentLoaded` is reported here, after the parse has run the
    // `defer` scripts it was holding and after it handed the tree over
    // (`docs/spec/02-architecture.md` §5.1). `load` is *not* reported here:
    // this page may still have an `async` script in flight, and `load` waits
    // for it. The settle loop fires it once nothing delays it.
    tracker.borrow_mut().note_domcontentloaded();

    let policy = FetchPolicy::for_tier(Tier::Dom);
    // Read through `deferred` rather than the local binding: the tree was moved
    // into it above and is the only owner from here on. Cloning the local handle
    // instead would leave the count above one at `take_if_owned`, whose
    // `Rc::try_unwrap` would then put the tree back and never install it.
    let base = deferred.shared().and_then(|document| {
        let document = document.borrow();
        document.base_url().or(document.url()).cloned()
    });
    let recorded: Vec<RecordedHint> = hints
        .iter()
        .map(|hint| RecordedHint::from_hint(hint, base.as_ref(), &policy))
        .collect();

    // Check if this navigation was cancelled before installing the document
    check_cancelled(&inner)?;

    // Emit FrameNavigated, clear hints, and set tier when document is installed (N-R4-3).
    // A dropped goto after the head will not reach here, so these events are only emitted
    // for navigations that actually install a document (11 §1).
    inner.emit(PageEvent::FrameNavigated {
        frame: inner.frame,
        url: final_url.clone(),
    });

    // The tree becomes the page's at the first install point at which the page
    // owns it alone; a tree an engine is bound to stays the engine's, and the
    // page keeps that engine once the navigation settles (`DeferredDocument`).
    // An evaluation also keeps the tree shared until it runs after settle. The
    // engine is not released here: the settle call this navigation is about to
    // run pumps it.
    {
        let mut state = inner.state.lock();
        if evaluation.is_none()
            && let Some(document) = deferred.take_if_owned()
        {
            state.document = Some(document);
        }
        state.hints = recorded;
        state.tier = Tier::Dom;
        state.document_navigation_id = generation;
    }
    // The render session of the document this one replaces, with its pinned
    // images and retained bodies, ends with it, not at the next render.
    #[cfg(feature = "render")]
    crate::render::release_unless(inner.config.id, generation);
    inner.emit(PageEvent::TierChanged { tier: Tier::Dom });

    // `DOMContentLoaded` reaches page script here: after the parse has run the
    // `defer` scripts it was holding and handed the tree over, and after the
    // tree became this page's document — which is the moment HTML puts the
    // event at. It is fired *not* at the booking point above but here, so a
    // navigation that is cancelled before it installs never runs a page's
    // handler for a document it then throws away.
    //
    // The booking still comes first, and that order is what makes the event
    // once-per-navigation: a handler that unwinds out of the dispatch — a
    // watchdog termination inside a listener — cannot earn a second one. Firing
    // before any settle turn is what lets a handler that adds a listener for
    // `load`, or queues a tracked timer, be in place before anything asks
    // whether the load has finished.
    //
    // The tracker borrow above ended at its semicolon: this dispatch runs page
    // script, and holding settle bookkeeping across it is the one thing this
    // boundary forbids. `deferred.shared()` is `None` for a page with no
    // engine, which is exactly the case where there is nothing to dispatch to;
    // a tree an engine is bound to is still shared here, because
    // `take_if_owned` above leaves it that way.
    //
    // The mid-body timeout below returns after installing a partial document,
    // so it stays after this point: the event is booked, and a partial document
    // that ended the load chain is still one page script should hear about.
    if let Some(document) = deferred.shared() {
        runtime::with_runtime_for(inner.config.id, document, |runtime| {
            runtime.dispatch_lifecycle(LifecycleEvent::DomContentLoaded)
        });
    }

    inner.reach(generation, LoadStage::DomContentLoaded);

    // If a mid-body timeout occurred while reading the response body,
    // return an error after installing the partial document.
    if mid_body_timeout {
        let pending = tracker.borrow().pending();
        let navigation = Navigation {
            url: url.clone(),
            final_url: final_url.clone(),
            status,
            headers,
            content_type,
            encoding,
            redirects,
            timing,
            bytes,
            from_cache,
            protocol,
            settle: SettleState::Timeout { pending },
            error: None,
            navigation_id: generation,
        };
        // Re-check generation to prevent overwriting a newer navigation (N-R4-2)
        {
            let mut state = inner.state.lock();
            if state.navigation_generation != generation {
                return Err(Error::Net {
                    code: ErrorName::Aborted.as_str().to_string(),
                    message: "navigation was cancelled".into(),
                });
            }
            if let Some(document) = deferred.take() {
                state.document = Some(document);
            }
            state.navigation = Some(navigation.clone());
            state.settle = Some(SettleState::Timeout { pending });
        }
        return Err(Error::Timeout {
            what: "navigation".to_owned(),
            millis: timeout_millis,
        });
    }

    // Evaluate Selector/Text conditions once after install (Tier 1: once and for all;
    // at Tier 2: on DOM mutation). This avoids spinning the page thread on repeated
    // evaluations in the settle loop (B-R2-2).
    if tracker.borrow().wait_until().needs_condition() {
        let met = {
            let tracker = tracker.borrow();
            condition_met(&inner, &deferred.handle(), tracker.wait_until())
        };
        tracker.borrow_mut().note_condition_met(met);
    }

    let settle = match wait_for_settle(
        &inner,
        &tracker,
        &deferred.handle(),
        deadline,
        timeout_millis,
        true,
        generation,
    )
    .await
    {
        Ok(state) => state,
        Err(Error::Timeout { what: _, millis: _ }) => {
            // Navigation timed out while settling (after the document was installed).
            // Keep the partial document and return the Timeout outcome.
            // Check and write under one lock to prevent race with close() (N-R4-1/N-R4-2).
            let pending = tracker.borrow().pending();
            let navigation = Navigation {
                url: url.clone(),
                final_url: final_url.clone(),
                status,
                headers,
                content_type,
                encoding,
                redirects,
                timing,
                bytes,
                from_cache,
                protocol,
                settle: SettleState::Timeout { pending },
                // A settle deadline is an outcome of a successful navigation,
                // not a network error (`docs/spec/02-architecture.md` §5.1).
                error: None,
                navigation_id: generation,
            };
            let kept = {
                let mut state = inner.state.lock();
                // Check if this navigation was cancelled before writing to state (B-R3-1/N-R4-2)
                if state.navigation_generation != generation || state.closed {
                    return Err(Error::Net {
                        code: ErrorName::Aborted.as_str().to_string(),
                        message: "navigation was cancelled".into(),
                    });
                }
                let kept = deferred.install(&mut state);
                state.navigation = Some(navigation.clone());
                state.settle = Some(SettleState::Timeout { pending });
                kept
            };
            if let Some(tree) = kept {
                retained::start_pump(&inner, tree);
            }
            report_network_idle(&inner, &tracker, generation);
            return Ok(NavigateOutcome {
                navigation,
                evaluation: evaluation.map(|_| Err(Error::timeout("evaluate", timeout_millis))),
            });
        }
        Err(e) => return Err(e),
    };

    // The engine still owns the shared document here. Evaluating before
    // `deferred.take()` preserves the globals and handlers the page's scripts
    // installed during navigation. A page with no script creates its engine
    // lazily over this same parsed tree.
    let evaluation = evaluation.map(|source| {
        evaluate_in_navigation(
            &inner,
            &deferred,
            &tracker,
            &source,
            deadline,
            timeout_millis,
        )
    });

    // Check and write under one lock to prevent race with close() (N-R4-1/N-R4-2)
    let navigation = Navigation {
        url,
        final_url,
        status,
        headers,
        content_type,
        encoding,
        redirects,
        timing,
        bytes,
        from_cache,
        protocol,
        settle,
        error: None,
        navigation_id: generation,
    };
    let kept = {
        let mut state = inner.state.lock();
        // Check if this navigation was cancelled before writing to state (B-R3-1/N-R4-1)
        if state.navigation_generation != generation || state.closed {
            return Err(Error::Net {
                code: ErrorName::Aborted.as_str().to_string(),
                message: "navigation was cancelled".into(),
            });
        }
        let kept = deferred.install(&mut state);
        state.navigation = Some(navigation.clone());
        state.settle = Some(settle);
        kept
    };
    if let Some(tree) = kept {
        retained::start_pump(&inner, tree);
    }
    report_network_idle(&inner, &tracker, generation);
    Ok(NavigateOutcome {
        navigation,
        evaluation,
    })
}

/// Reports `networkAlmostIdle` and `networkIdle` for the document of
/// `generation` when the tracker's idle windows complete after `load`.
///
/// Settling does not wait for the windows: a page is settled the moment its
/// work is done, and the windows are half a second of silence on top of that.
/// A task on the page thread keeps watching the same tracker the engine
/// reports into, so requests the page starts later move the windows, and ends
/// when the page closes or navigates again. It runs whether or not anyone is
/// subscribed, because the frame state it fills in is what a client that joins
/// later is told.
fn report_network_idle(
    inner: &Arc<PageInner>,
    tracker: &Rc<RefCell<SettleTracker>>,
    generation: u64,
) {
    let (inner, tracker) = (Arc::clone(inner), Rc::clone(tracker));
    tokio::task::spawn_local(async move {
        let mut changes = tracker.borrow().changes();
        let mut almost_idle = false;
        loop {
            // Registered before the check, so a navigation that starts in
            // between wakes the wait below.
            let navigated = inner.generation_changed.notified();
            tokio::pin!(navigated);
            navigated.as_mut().enable();
            let wake = {
                let state = inner.state.lock();
                if state.closed || state.navigation_generation != generation {
                    return;
                }
                drop(state);
                let tracker = tracker.borrow();
                let now = inner.config.clock.now();
                if tracker.load_fired() {
                    if !almost_idle && tracker.network_almost_idle(now) {
                        almost_idle = true;
                        inner.reach(generation, LoadStage::NetworkAlmostIdle);
                    }
                    if tracker.network_idle(now) {
                        inner.reach(generation, LoadStage::NetworkIdle);
                        return;
                    }
                }
                tracker
                    .next_idle_wakeup(now)
                    .map(|at| at.saturating_duration_since(now))
            };
            let elapsed = async {
                match wake {
                    Some(wait) => tokio::time::sleep(wait).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                () = &mut navigated => {}
                _ = changes.changed() => {}
                () = elapsed => {}
            }
        }
    });
}

fn evaluate_in_navigation(
    inner: &Arc<PageInner>,
    deferred: &DeferredDocument,
    tracker: &Rc<RefCell<SettleTracker>>,
    source: &str,
    deadline: Instant,
    timeout_millis: u64,
) -> Result<String> {
    let document = deferred
        .shared()
        .ok_or_else(|| Error::invalid("the page has no document to evaluate"))?;
    let navigation = inner.state.lock().navigation_generation;
    runtime::ensure_runtime(
        inner.config.id,
        document.clone(),
        &inner.config,
        Rc::clone(tracker),
        Arc::clone(inner) as Arc<dyn ScriptBudget>,
        navigation,
    )
    .map_err(Error::unsupported)?;
    runtime::note_parse_finished(inner.config.id, document);
    let remaining = deadline.saturating_duration_since(inner.config.clock.now());
    if remaining.is_zero() {
        return Err(Error::timeout("evaluate", timeout_millis));
    }
    let outcome = runtime::with_runtime_for(inner.config.id, document, |runtime| {
        runtime.evaluate(source, remaining)
    })
    .ok_or_else(|| Error::unsupported("the JavaScript runtime was unavailable"))?;
    if let Some(reason) = inner.crash_reason() {
        return Err(inner
            .native_memory_crash()
            .map(native_memory_budget_error)
            .unwrap_or(Error::Crashed { reason }));
    }
    outcome
}

struct ParsedBody {
    document: SharedDocument,
    hints: Vec<PreloadHint>,
    bytes: u64,
    encoding: String,
    timing: Timing,
    from_cache: bool,
    protocol: &'static str,
    redirects: Vec<RedirectHop>,
    mid_body_timeout: bool,
}

/// Releases the runtime a failed parse built.
///
/// A parse that ends in an error hands no tree to a navigation, so nothing
/// later would release the engine it built at a script pause: without this the
/// engine and its document would stay on the page thread until some later
/// navigation of the same page replaced them.
struct ParseCleanup {
    page: PageId,
    document: SharedDocument,
    armed: bool,
}

impl ParseCleanup {
    fn new(page: PageId, document: SharedDocument) -> ParseCleanup {
        ParseCleanup {
            page,
            document,
            armed: true,
        }
    }

    /// The parse replaced its tree with a re-parse; the engine can only be
    /// bound to the tree in use, and at this point it can only be bound to
    /// none.
    fn rebind(&mut self, document: &SharedDocument) {
        self.document = document.clone();
    }

    /// The parse handed its tree over: a navigation owns it from here.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ParseCleanup {
    fn drop(&mut self) {
        if self.armed {
            drop(runtime::take_runtime_for(self.page, &self.document));
        }
    }
}

/// How far one parse step got.
///
/// A `</script>` pause is reported as a *status*, not through a callback, so
/// driving it is a loop rather than a single call: take the script, run it,
/// resume, and look at what the resume reported. A pause can be followed by
/// another pause (two adjacent scripts in one chunk), by the chunk simply
/// being consumed, by the end of the document, or by a declaration that
/// changes the encoding.
enum ParseStep {
    Fed,
    Stopped,
    Restart(&'static Encoding),
}

/// The `defer`/`async` scripts one navigation has discovered, the fetches
/// behind them, and the settle call they are counted against.
///
/// This is one local on `navigate`'s stack, like `ParseCleanup` and
/// `DeferredDocument`, and for the same reason: it holds work that outlives the
/// call which started it, so every early return out of the navigation has to
/// own it. It cannot live in `PageRuntime` instead — the runtime table is a
/// thread-local `RefCell` that a second navigation of the same page reaches at
/// its own awaits, and anything borrowed across one of those awaits panics
/// (`crate::runtime`).
///
/// The tracker is a field rather than a parameter: every count in this queue is
/// held against that one settle call, so the parse would otherwise carry the
/// same pair through every function it passes the queue to.
///
/// Two timings share it, because they differ only in what they wait for. An
/// `async` script's task runs it as soon as its fetch completes. A `defer`
/// script's task only fetches it: the text is handed to `deferred`, and the
/// drain runs the queue in document order once the document is parsed.
///
/// A known limitation, recorded rather than fixed: this queue lives for one
/// navigation, and a caller who asks for less than `load` ends it while an
/// `async` script may still be in flight. That script's fetch is aborted with
/// the queue, so it never runs even though the page keeps its engine. Closing
/// this means handing the in-flight fetches to the kept engine's driver.
struct ScriptJobs {
    /// The document generation the last chunk-by-chunk stylesheet sweep saw.
    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    swept: Option<u64>,
    /// The settle call of the navigation that owns this queue.
    tracker: Rc<RefCell<SettleTracker>>,
    /// The navigation's generation, which keys its render session.
    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    generation: u64,
    /// `defer` scripts awaiting the end of the parse, in document order.
    deferred: Vec<DeferredEntry>,
    /// Every parallel fetch still running. Aborted when the navigation ends, so
    /// a script the page will never run stops being fetched.
    inflight: Vec<JoinHandle<()>>,
}

impl ScriptJobs {
    fn new(tracker: Rc<RefCell<SettleTracker>>, generation: u64) -> ScriptJobs {
        ScriptJobs {
            swept: None,
            tracker,
            generation,
            deferred: Vec::new(),
            inflight: Vec::new(),
        }
    }
}

impl Drop for ScriptJobs {
    fn drop(&mut self) {
        for handle in &self.inflight {
            handle.abort();
        }
    }
}

/// One `defer` script whose text is on its way, and the claim it has on the
/// settle until the drain runs it.
struct DeferredEntry {
    url: Url,
    /// The script, or `None` when there is nothing to run: the fetch failed,
    /// the origin answered with a status that is not ok, the document's
    /// encoding could not decode it, or a module's graph did not load.
    script: oneshot::Receiver<Option<DeferredScript>>,
    pending: PendingScripts,
}

/// What a deferred entry runs once its turn comes.
enum DeferredScript {
    /// A classic script's decoded text and its `<script>` element, which is
    /// `document.currentScript` while the script runs.
    Classic {
        source: String,
        element: tenta_core::NodeRef,
    },
    /// A module script whose graph is loaded, by its key in the module map.
    Module { key: String },
}

/// How long an `import()` made after the navigation is over may take to load
/// its graph. The navigation's own deadline bounds a module script the parser
/// found; a later import has none to inherit.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(30);

/// Starts the parallel fetch of one `defer`/`async` external classic script.
///
/// The fetch runs as its own task on the page thread's `LocalSet` rather than
/// as part of the parse: that is what lets the parse carry on while a script is
/// in flight, and it is the whole difference between these two timings and a
/// parser-blocking script.
///
/// `spawn_local`, not `post_job`: the task holds the settle tracker, which is
/// an `Rc` and cannot enter a `Send + 'static` closure. That
/// makes a `LocalSet` a precondition of the navigation — one is provided by
/// every in-tree driver (`crates/tenta-browser/src/threads.rs`,
/// `crates/tenta-page/tests/support/mod.rs`), and a caller that drives
/// `Page::goto` outside one panics here rather than misbehaving.
///
/// The tree reaches an `async` script's task weakly: the fetch outlives the
/// parse, and a task holding the tree would keep the page from ever installing
/// it alone.
///
/// The task's body runs inside `catch_unwind_future`. A panic in an async
/// script crashes its page and discards the runtime immediately, even while
/// the main document is suspended on a streaming response body; otherwise a
/// dropped `JoinHandle` would swallow the crash (`02-architecture.md` §8).
fn start_deferred_script(
    timing: runtime::DeferredTiming,
    meta: runtime::ScriptMeta,
    encoding: Option<&'static Encoding>,
    inner: &Arc<PageInner>,
    document: &SharedDocument,
    pending: PendingScripts,
    jobs: &mut ScriptJobs,
) {
    let page = inner.config.id;
    let inner = Arc::clone(inner);
    let document = Rc::downgrade(document);
    let tracker = Rc::clone(&jobs.tracker);
    match timing {
        // `async` waits only for its own fetch: its task runs the script the
        // moment the text arrives, which is why the count it holds ends with
        // that task rather than with the parse.
        runtime::DeferredTiming::Async(url, element) => {
            let handle = tokio::task::spawn_local(async move {
                let fetched = catch_unwind_future(async {
                    let source =
                        fetch_deferred_script(&inner, &tracker, &meta, &url, encoding).await;
                    if let Some(source) = source {
                        run_fetched_script(page, &document, &url, &source, element);
                        // What it inserted is script's, never the parser's.
                        #[cfg(feature = "render")]
                        sweep_script_sheets(&inner, &tracker, &document, meta.navigation);
                    }
                })
                .await;
                if let Err(payload) = fetched {
                    let reason = panic_message(&payload);
                    inner.mark_crashed(&reason);
                    if let Some(document) = document.upgrade() {
                        drop(runtime::take_runtime_for(page, &document));
                    }
                    tracing::error!(?page, reason, "an async script task panicked");
                }
                drop(pending);
            });
            jobs.inflight.push(handle);
        }
        // `defer` waits for the document too, and for every deferred script
        // before it, so its task only delivers the text: the drain decides when
        // it runs, and owns the count until it has.
        runtime::DeferredTiming::Defer(url, element) => {
            let (sender, text) = oneshot::channel();
            // The entry keeps the URL for the drain's log lines; the task needs
            // its own copy for the fetch and its own messages.
            let fetched_url = url.clone();
            let handle = tokio::task::spawn_local(async move {
                let fetched = catch_unwind_future(async {
                    let source =
                        fetch_deferred_script(&inner, &tracker, &meta, &fetched_url, encoding)
                            .await;
                    // A receiver that is gone means the navigation ended before
                    // the drain reached this script; the text has nowhere to go.
                    let _ = sender
                        .send(source.map(|source| DeferredScript::Classic { source, element }));
                })
                .await;
                if let Err(payload) = fetched {
                    tracing::error!(
                        ?page,
                        reason = %panic_message(&payload),
                        "the fetch of a deferred script panicked"
                    );
                }
            });
            jobs.deferred.push(DeferredEntry {
                url,
                script: text,
                pending,
            });
            jobs.inflight.push(handle);
        }
        // A module script with `async` runs when its graph has loaded, like an
        // `async` classic script runs when its text has.
        runtime::DeferredTiming::AsyncModule(root) => {
            let handle = tokio::task::spawn_local(async move {
                let loaded = catch_unwind_future(async {
                    match modules::load_graph(&inner, &tracker, &meta, &document, root).await {
                        Ok(key) => {
                            run_loaded_module(page, &document, &key);
                            #[cfg(feature = "render")]
                            sweep_script_sheets(&inner, &tracker, &document, meta.navigation);
                        }
                        Err(reason) => {
                            tracing::warn!(?page, %reason, "an async module script did not load")
                        }
                    }
                })
                .await;
                if let Err(payload) = loaded {
                    let reason = panic_message(&payload);
                    inner.mark_crashed(&reason);
                    if let Some(document) = document.upgrade() {
                        drop(runtime::take_runtime_for(page, &document));
                    }
                    tracing::error!(?page, reason, "an async module script task panicked");
                }
                drop(pending);
            });
            jobs.inflight.push(handle);
        }
        // A module script without `async` is deferred: its graph loads in
        // parallel with the parse and it runs in document order with the
        // `defer` scripts, before `DOMContentLoaded`.
        runtime::DeferredTiming::Module(root) => {
            let (sender, script) = oneshot::channel();
            let url = match &root {
                runtime::ModuleRoot::External(url) => url.clone(),
                runtime::ModuleRoot::Inline { .. } => meta.document_url.clone(),
            };
            let handle = tokio::task::spawn_local(async move {
                let loaded = catch_unwind_future(async {
                    let key = modules::load_graph(&inner, &tracker, &meta, &document, root).await;
                    if let Err(reason) = &key {
                        tracing::warn!(?page, %reason, "a module script did not load");
                    }
                    let _ = sender.send(key.ok().map(|key| DeferredScript::Module { key }));
                })
                .await;
                if let Err(payload) = loaded {
                    tracing::error!(
                        ?page,
                        reason = %panic_message(&payload),
                        "the load of a module script panicked"
                    );
                }
            });
            jobs.deferred.push(DeferredEntry {
                url,
                script,
                pending,
            });
            jobs.inflight.push(handle);
        }
    }
}

/// Runs a module whose graph is loaded, if this navigation's engine is still
/// bound to the tree. See [`run_fetched_script`] for why the tree is named
/// weakly.
fn run_loaded_module(page: PageId, document: &Weak<RefCell<DomDocument>>, key: &str) {
    let Some(document) = document.upgrade() else {
        return;
    };
    if runtime::with_runtime_for(page, &document, |runtime| runtime.run_module(key)).is_none() {
        tracing::debug!(
            ?page,
            module = %key,
            "a module script did not run: its engine is no longer bound to this tree"
        );
    }
}

/// Loads the graphs the page's script asked for with `import()` and answers
/// each import when its graph is there.
///
/// Each import is its own task on the page thread, and each holds the settle
/// while it loads: an `import()` is how a page built from modules gets its
/// code, so a navigation that settled with one in flight would hand back a
/// page that has not started.
fn start_dynamic_imports(
    inner: Arc<PageInner>,
    imports: Vec<tenta_js::modules::DynamicImport>,
    context: runtime::ImportContext,
) {
    let page = inner.config.id;
    let navigation = inner.state.lock().navigation_generation;
    for import in imports {
        let inner = Arc::clone(&inner);
        let tracker = Rc::clone(&context.tracker);
        let document = context.document.clone();
        let document_url = context
            .document_url
            .clone()
            .unwrap_or_else(|| import.url.clone());
        // Held from here rather than from inside the task: the task has not
        // run yet when the entry that made the import returns, and the settle
        // must not be judged finished in between.
        let pending = PendingScripts::hold(&tracker);
        tokio::task::spawn_local(async move {
            let meta = runtime::ScriptMeta {
                base: document_url.clone(),
                document_url,
                deadline: inner.config.clock.now() + IMPORT_TIMEOUT,
                navigation,
            };
            let root = runtime::ModuleRoot::External(import.url);
            let loaded = catch_unwind_future(modules::load_graph(
                &inner, &tracker, &meta, &document, root,
            ))
            .await;
            let outcome = match loaded {
                Ok(outcome) => outcome,
                Err(payload) => {
                    tracing::error!(
                        ?page,
                        reason = %panic_message(&payload),
                        "the load of a dynamic import panicked"
                    );
                    Err("the module could not be loaded".to_owned())
                }
            };
            if let Some(document) = document.upgrade() {
                let outcome = outcome.as_ref().map(String::as_str).map_err(String::as_str);
                runtime::with_runtime_for(page, &document, |runtime| {
                    runtime.finish_dynamic_import(import.id, outcome)
                });
            }
            drop(pending);
        });
    }
}

/// Fetches one `defer`/`async` external classic script and decodes it with the
/// document's encoding.
///
/// It shares the parser-blocking path's fetch verbatim
/// ([`runtime::fetch_script`]), so a parallel script is exactly as tracked and
/// as exempt from reclassification as a blocking one; only the waiting differs.
///
/// `None` means there is nothing to run, which is the same set of failures the
/// blocking path reports: a fetch that failed or was not served, a deadline
/// that left no time to issue the request, or a body the document's encoding
/// cannot make text of.
async fn fetch_deferred_script(
    inner: &Arc<PageInner>,
    tracker: &Rc<RefCell<SettleTracker>>,
    meta: &runtime::ScriptMeta,
    url: &Url,
    encoding: Option<&'static Encoding>,
) -> Option<String> {
    let body = runtime::fetch_script(&inner.config, meta, url, tracker).await?;
    let source = runtime::decode_script(&body, encoding);
    if source.is_none() {
        tracing::warn!(
            page = ?inner.config.id,
            url = %url,
            "a parallel external script is skipped: it cannot be decoded"
        );
    }
    source
}

/// Runs one already-fetched classic script against the tree, if this
/// navigation's engine is still bound to it.
///
/// The tree is named *weakly*, because a parallel fetch outlives the parse that
/// started it and nothing here may keep the tree alive across that: the page
/// installs its document by taking it from the navigation that owns it alone
/// (`DeferredDocument::take_if_owned`), and a fetch task still holding a
/// reference would deny the page its own tree — silently, since the navigation
/// would then drop the document it could not install.
///
/// `with_runtime_for` names the tree as well as the page, so a script fetched
/// for a navigation that has been superseded does not run in the engine of the
/// one that replaced it.
fn run_fetched_script(
    page: PageId,
    document: &Weak<RefCell<DomDocument>>,
    url: &Url,
    source: &str,
    element: tenta_core::NodeRef,
) {
    let Some(document) = document.upgrade() else {
        tracing::debug!(
            ?page,
            url = %url,
            "a fetched script did not run: the navigation that asked for it is over"
        );
        return;
    };
    if runtime::with_runtime_for(page, &document, |runtime| {
        runtime.run_script(source, Some(element))
    })
    .is_none()
    {
        tracing::debug!(
            ?page,
            url = %url,
            "a fetched script did not run: its engine is no longer bound to this tree"
        );
    }
}

/// Runs the `defer` scripts the parse collected, in document order, once the
/// document is parsed.
///
/// This is the one thing `defer` does that `async` does not: the standard runs
/// a deferred classic script "as soon as the document has finished parsing", in
/// document order, and before `DOMContentLoaded` — which the caller reports
/// only after this returns. An `async` script is deliberately not here; it runs
/// from its own task, whenever its fetch arrives.
///
/// The wait is raced against the two things that can end this navigation: a
/// newer `goto`/`close` and the deadline. A deferred script whose fetch has not
/// arrived by either is dropped, and so is its claim on the settle — the
/// navigation is over, and waiting out the script's own fetch timeout would
/// keep a superseded navigation alive for work nobody will read.
async fn drain_deferred(
    jobs: &mut ScriptJobs,
    inner: &Arc<PageInner>,
    document: &SharedDocument,
    deadline: Instant,
    generation: u64,
) {
    let page = inner.config.id;
    let at = tokio::time::Instant::from_std(deadline);
    // The scripts run against this navigation's tree, named weakly for the same
    // reason an `async` task names it weakly (see `run_fetched_script`): the
    // navigation itself is what keeps the tree alive here.
    let document = Rc::downgrade(document);
    // Taken up front so an early return still releases every script it did not
    // reach: each entry holds a count on the settle, and a count released by no
    // one is a settle that can never end.
    let entries = std::mem::take(&mut jobs.deferred);
    #[cfg(feature = "render")]
    let tracker = Rc::clone(&jobs.tracker);
    for entry in entries {
        let DeferredEntry {
            url,
            script: mut text,
            pending,
        } = entry;
        // Registered before the check, so a bump between the two is not lost
        // (`notify_waiters` reaches only registered waiters).
        let received = loop {
            let generation_notified = inner.generation_changed.notified();
            tokio::pin!(generation_notified);
            generation_notified.as_mut().enable();
            if inner.state.lock().navigation_generation != generation {
                return;
            }
            tokio::select! {
                biased;
                _ = &mut generation_notified => {
                    if inner.state.lock().navigation_generation != generation {
                        return;
                    }
                }
                received = &mut text => break received,
                _ = tokio::time::sleep_until(at) => {
                    tracing::warn!(
                        ?page,
                        url = %url,
                        "a deferred script did not arrive before the navigation deadline; \
                         the document ends without it"
                    );
                    return;
                }
            }
        };
        match received {
            Ok(Some(DeferredScript::Classic { source, element })) => {
                run_fetched_script(page, &document, &url, &source, element);
                #[cfg(feature = "render")]
                sweep_script_sheets(inner, &tracker, &document, generation);
            }
            Ok(Some(DeferredScript::Module { key })) => {
                run_loaded_module(page, &document, &key);
                #[cfg(feature = "render")]
                sweep_script_sheets(inner, &tracker, &document, generation);
            }
            // The fetch already logged why there is nothing to run.
            Ok(None) => {}
            Err(_) => tracing::debug!(
                ?page,
                url = %url,
                "a deferred script's fetch ended without delivering it"
            ),
        }
        // The script has run, so the load chain is one script lighter: the drop
        // signals the settle loop, whose `load` gate is what waits for it.
        drop(pending);
    }
}

/// Drives one parse status to the point where the caller may feed again,
/// running the scripts the parse paused on.
///
/// A `Script` pause is the one place the page enters an engine with its parser
/// stopped: everything before the script is in the tree, nothing after it is,
/// and the parse stays where it is until [`Parser::resume`]. A parser-blocking
/// external classic script is fetched inside the pause, so this is one of the
/// few places the page thread awaits with a parser stopped on a node — and the
/// only one that resumes it afterwards.
///
/// A script carrying `async` or `defer` is the opposite case: its fetch must
/// not hold the parse at all, so this starts it as its own task and resumes
/// immediately (`start_deferred_script`). Which is which is the HTML
/// standard's question, answered in `crate::runtime`.
async fn drive_parse(
    parser: &mut Parser<'_, SharedSink>,
    status: ParseStatus,
    inner: &Arc<PageInner>,
    document: &SharedDocument,
    jobs: &mut ScriptJobs,
    final_url: &Url,
    deadline: Instant,
) -> Result<ParseStep> {
    let config = &inner.config;
    // The stylesheet loader (render builds) is keyed by the navigation.
    #[cfg(feature = "render")]
    let generation = jobs.generation;
    document.borrow().check_observer_health()?;
    let mut status = status;
    while status == ParseStatus::Script {
        // A script the parse is paused on is work in flight: it holds a
        // `NetworkIdle` settle open, exactly as a subresource would. The count
        // spans the fetch of the script's own `src`, so it is owned by the
        // guard rather than by this loop iteration's two ends.
        let pending = PendingScripts::hold(&jobs.tracker);
        let base = {
            let document = document.borrow();
            document.base_url().or(document.url()).cloned()
        }
        .unwrap_or_else(|| final_url.clone());
        let meta = runtime::ScriptMeta {
            base,
            document_url: final_url.clone(),
            deadline,
            navigation: jobs.generation,
        };
        // The stylesheets the parser has inserted so far start loading now, so
        // an external script's own fetch below overlaps with them; the script
        // then waits for the ones that block scripts (HTML's "a style sheet
        // that is blocking scripts") and sees their rules when it reads styles.
        #[cfg(feature = "render")]
        sheet_loader::sweep_parsed(
            inner,
            &jobs.tracker,
            document,
            generation,
            true,
            parser.encoding(),
        );
        let pending_script = runtime::run_pending_script(
            parser,
            document,
            config,
            &jobs.tracker,
            &meta,
            Arc::clone(inner) as Arc<dyn ScriptBudget>,
        )
        .await?;
        let deferred = match pending_script {
            runtime::PendingScript::Ready(source, element) => {
                #[cfg(feature = "render")]
                sheet_loader::wait_for_blocking_sheets(
                    inner,
                    &jobs.tracker,
                    document,
                    generation,
                    deadline,
                )
                .await;
                runtime::run_script_source(config.id, document, &source, Some(element));
                // What the script inserted is its own, not the parser's: it
                // does not block the scripts after it.
                #[cfg(feature = "render")]
                sheet_loader::sweep_parsed(
                    inner,
                    &jobs.tracker,
                    document,
                    generation,
                    false,
                    parser.encoding(),
                );
                None
            }
            runtime::PendingScript::Deferred(timing) => Some(timing),
            runtime::PendingScript::Done => None,
        };
        // A script that ran above may have burned the page's script budget,
        // which crashes the page from inside that entry. The parse must not
        // carry on with a page that is gone. Returning here drops `deferred`
        // and `pending` on the way out, and that is complete: `deferred` holds
        // only the URL of a script whose fetch has not been started, and
        // `PendingScripts` releases its hold in its own `Drop`, so nothing is
        // left in flight.
        if let Some(reason) = inner.crash_reason() {
            return Err(Error::Crashed { reason });
        }
        // Handing a script out makes the document's encoding certain, so this
        // is the encoding the script's bytes are decoded with — including for a
        // parallel fetch, whose decoding happens after the pause is over.
        let encoding = parser.encoding();
        // Resuming is the caller's statement that it is done with the pause.
        status = parser.resume();
        document.borrow().check_observer_health()?;
        match deferred {
            // The script's fetch outlives this iteration, so its count travels
            // with it rather than being released here.
            Some(timing) => {
                start_deferred_script(timing, meta, encoding, inner, document, pending, jobs)
            }
            None => drop(pending),
        }
    }
    document.borrow().check_observer_health()?;
    Ok(match status {
        ParseStatus::Continue => ParseStep::Fed,
        ParseStatus::Stopped => ParseStep::Stopped,
        ParseStatus::EncodingChanged(encoding) => ParseStep::Restart(encoding),
        // The loop above leaves no `Script` behind.
        ParseStatus::Script => ParseStep::Fed,
    })
}

/// The encoding the document's text was decoded with, for `characterSet`.
///
/// It is the parser's choice (or the restart's), not a second sniff of the
/// first bytes: the parser may have decided from more bytes than the prescan
/// window. A parser that never decided had no bytes to go by, so the transport
/// and the locale are all there is.
fn decoded_encoding(
    chosen: Option<&'static tenta_html::Encoding>,
    transport_charset: Option<&str>,
    locale: &str,
) -> String {
    chosen
        .unwrap_or_else(|| tenta_html::sniff(&[], transport_charset, locale).encoding)
        .name()
        .to_owned()
}

/// What the response head told the parse about the body it is about to read.
///
/// Separate from [`Response`] because the body stream is consumed while these
/// are still needed — the URL every relative reference resolves against, and
/// the `Content-Type` that decides the MIME type and the encoding hint.
struct BodyMeta<'a> {
    final_url: &'a Url,
    content_type: &'a Option<String>,
    /// Reports the body to a network host while it is read.
    tracked: Option<crate::network::Tracked>,
}

/// Streams the response into the parser, running each script the parse pauses
/// on and restarting once if a `<meta charset>` overrides the sniffed encoding.
///
/// `jobs` is the navigation's queue of `defer`/`async` scripts, threaded in
/// from `navigate`: this is where they are started (through `drive_parse`) and
/// where the deferred ones are run, at either end of the parse.
async fn parse_body(
    inner: &Arc<PageInner>,
    response: Response,
    meta: BodyMeta<'_>,
    deadline: Instant,
    timeout_millis: u64,
    generation: u64,
    jobs: &mut ScriptJobs,
) -> Result<ParsedBody> {
    let config = &inner.config;
    let final_url = meta.final_url;
    let content_type = meta.content_type;
    let mut tracked = meta.tracked;
    let transport_charset = content_type.as_deref().and_then(charset_of);
    let mime = content_type
        .as_deref()
        .map(mime_of)
        .unwrap_or_else(|| "text/html".to_owned());
    let timing = response.timing;
    let from_cache = response.from_cache;
    let protocol = response.protocol.as_str();
    let redirects = response.redirects.clone();
    let max_bytes = config.budgets.network_bytes;

    let hints: Arc<Mutex<Vec<PreloadHint>>> = Arc::new(Mutex::new(Vec::new()));
    // The parse is written into a document that script may read while it runs,
    // so the sink holds it behind `Rc<RefCell<_>>` rather than owning it
    // (`docs/spec/05-javascript.md` §6). A page without an engine uses the same
    // sink: one tree type, one install path.
    let make_document = || {
        let (sink, document) = SharedSink::with_new_document();
        {
            let mut fresh = document.borrow_mut();
            fresh.set_url(Some(final_url.clone()));
            fresh.set_content_type(mime.clone());
        }
        (sink, document)
    };
    // Pausing is only useful to a caller that can run the script it is pausing
    // on (`crates/tenta-html/src/parser.rs`).
    let pause_on_script = config.javascript && config.engine.is_some();
    let make_options = |encoding: Option<&'static Encoding>| {
        let sink = Arc::clone(&hints);
        ParserOptions {
            scripting_enabled: config.javascript,
            pause_on_script,
            declarative_shadow_roots: true,
            default_locale: config.locale.clone(),
            transport_charset: transport_charset.clone(),
            encoding,
            preload: Some(Box::new(move |hint: PreloadHint| {
                sink.lock().push(hint);
            })),
            ..ParserOptions::default()
        }
    };

    let (sink, mut document) = make_document();
    let mut cleanup = ParseCleanup::new(inner.config.id, document.clone());
    let mut parser = Parser::new(sink, make_options(None));
    let mut body = response.body;
    let mut total: u64 = 0;
    let mut restart_buffer: Option<Vec<Bytes>> = Some(Vec::new());
    let mut buffered = 0usize;
    let mut restarted: Option<&'static tenta_html::Encoding> = None;

    let cancelled = || Error::Net {
        code: ErrorName::Aborted.as_str().to_string(),
        message: "navigation was cancelled by a concurrent goto or close".into(),
    };
    let deadline_at = tokio::time::Instant::from_std(deadline);
    loop {
        // Wait for the next body chunk, a cancellation, or the deadline —
        // whichever comes first. A body that has delivered its head and then
        // stalls (a slow origin, a `/never` endpoint) must not pin the
        // cancelled navigation and its socket until the next chunk or the
        // deadline: racing `generation_changed` here aborts within one poll
        // and drops the body stream, which closes the connection (B-R5-1).
        let next = {
            let generation_notified = inner.generation_changed.notified();
            tokio::pin!(generation_notified);
            // Register the waiter before re-reading the generation, or a bump
            // between the check and the first poll is lost.
            generation_notified.as_mut().enable();
            {
                let state = inner.state.lock();
                if state.navigation_generation != generation {
                    return Err(cancelled());
                }
            }
            let chunk_future = body.next_chunk();
            tokio::pin!(chunk_future);
            tokio::select! {
                biased;
                _ = &mut generation_notified => {
                    let state = inner.state.lock();
                    if state.navigation_generation != generation {
                        return Err(cancelled());
                    }
                    // Spurious wake without a generation change: try again.
                    continue;
                }
                chunk = &mut chunk_future => Ok(chunk),
                _ = tokio::time::sleep_until(deadline_at) => {
                    Err(Error::timeout("navigation", timeout_millis))
                }
            }
        };
        let chunk = match next {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(Error::Timeout { .. }) => {
                // Mid-body timeout: finish parsing the document with what we have so far.
                // Check if this navigation was cancelled before finishing the parse
                {
                    let state = inner.state.lock();
                    if state.navigation_generation != generation {
                        return Err(Error::Net {
                            code: ErrorName::Aborted.as_str().to_string(),
                            message: "navigation was cancelled by a concurrent goto or close"
                                .into(),
                        });
                    }
                }
                // The parse is over, whatever the body did: the tree it built
                // so far is the page's, and no parser writes into it again.
                let chosen = restarted.or(parser.encoding());
                document = parser.finish();
                document.borrow().check_observer_health()?;
                runtime::note_parse_finished(inner.config.id, &document);
                let encoding =
                    decoded_encoding(chosen, transport_charset.as_deref(), &config.locale);
                // Apply the same post-parse fix-ups as the full parse (N-R3-2)
                {
                    let mut document = document.borrow_mut();
                    document.set_scripting_enabled(config.javascript);
                    document.set_encoding(encoding.clone());
                    document.refresh_base_url();
                    document.set_ready_state(tenta_dom::ReadyState::Complete);
                }
                let hints = std::mem::take(&mut *hints.lock());
                if let Some(tracked) = &mut tracked {
                    tracked.finish();
                }
                // The deferred scripts run at this end of the parse too, and
                // for the same reason: the document is as complete as it will
                // be. The deadline has already passed here, so the drain only
                // takes what has already arrived.
                drain_deferred(jobs, inner, &document, deadline, generation).await;
                cleanup.disarm();
                return Ok(ParsedBody {
                    document,
                    hints,
                    bytes: total,
                    encoding,
                    timing,
                    from_cache,
                    protocol,
                    redirects,
                    mid_body_timeout: true,
                });
            }
            Err(error) => {
                return Err(navigation_error(error, max_bytes, timeout_millis));
            }
        };
        let chunk = chunk
            .map_err(|error| navigation_error(Error::from(error), max_bytes, timeout_millis))?;
        total += chunk.len() as u64;
        if let Some(tracked) = &mut tracked {
            tracked.chunk(&chunk);
        }
        if total > max_bytes {
            if let Some(tracked) = &mut tracked {
                tracked.failed_with("net::ERR_FILE_TOO_BIG");
            }
            return Err(over_budget(max_bytes));
        }
        if let Some(buffer) = &mut restart_buffer {
            buffered += chunk.len();
            if buffered > RESTART_WINDOW {
                restart_buffer = None;
            } else {
                buffer.push(chunk.clone());
            }
        }
        if config.test_hooks.panic_in_parse {
            panic!("test hook: panic inside the parse task");
        }
        let status = parser.feed(&chunk);
        match drive_parse(
            &mut parser,
            status,
            inner,
            &document,
            jobs,
            final_url,
            deadline,
        )
        .await?
        {
            ParseStep::Fed => {
                // The stylesheets of the markup so far start loading without
                // waiting for a script or for the end of the body.
                #[cfg(feature = "render")]
                sweep_sheets_so_far(jobs, inner, &document, parser.encoding());
            }
            ParseStep::Stopped => break,
            ParseStep::Restart(encoding) => {
                // What the old tree started is thrown away with it.
                #[cfg(feature = "render")]
                restart_sheets(jobs, inner);
                let Some(buffer) = restart_buffer.take() else {
                    tracing::warn!(
                        url = %final_url,
                        encoding = encoding.name(),
                        "a <meta charset> past the restart window cannot re-parse the document"
                    );
                    break;
                };
                hints.lock().clear();
                // The restart throws the tree away, so it gets a fresh one.
                // The parser only reports an encoding change before it has
                // handed a script out — handing one out makes the encoding
                // certain — so no engine is bound to the tree being dropped,
                // and every script of the new pass still runs.
                let (sink, fresh) = make_document();
                document = fresh;
                cleanup.rebind(&document);
                parser = parser.restart_with_encoding(sink, encoding);
                restarted = Some(encoding);
                for buffered_chunk in &buffer {
                    let status = parser.feed(buffered_chunk);
                    match drive_parse(
                        &mut parser,
                        status,
                        inner,
                        &document,
                        jobs,
                        final_url,
                        deadline,
                    )
                    .await?
                    {
                        ParseStep::Fed => {}
                        ParseStep::Stopped => break,
                        ParseStep::Restart(next) => {
                            // The encoding is explicit for the re-parse, so a
                            // second declaration cannot change it.
                            tracing::warn!(
                                url = %final_url,
                                encoding = next.name(),
                                "a second <meta charset> during the re-parse is ignored"
                            );
                            break;
                        }
                    }
                }
                restart_buffer = Some(buffer);
            }
        }
    }

    // Check if this navigation was cancelled before finishing the parse
    {
        let state = inner.state.lock();
        if state.navigation_generation != generation {
            return Err(Error::Net {
                code: ErrorName::Aborted.as_str().to_string(),
                message: "navigation was cancelled by a concurrent goto or close".into(),
            });
        }
    }

    if let Some(tracked) = &mut tracked {
        tracked.finish();
    }

    // `feed_last` is what reports the scripts of a body shorter than the
    // sniff window: the rest of the stream is over, and pausing still
    // applies (`crates/tenta-html/src/parser.rs`).
    let status = parser.feed_last(b"");
    match drive_parse(
        &mut parser,
        status,
        inner,
        &document,
        jobs,
        final_url,
        deadline,
    )
    .await?
    {
        ParseStep::Fed => {}
        ParseStep::Stopped => {}
        ParseStep::Restart(encoding) => {
            tracing::warn!(
                url = %final_url,
                encoding = encoding.name(),
                "an encoding declared at the end of the stream is ignored"
            );
        }
    }

    let chosen = restarted.or(parser.encoding());
    document = parser.finish();
    document.borrow().check_observer_health()?;
    runtime::note_parse_finished(inner.config.id, &document);
    let encoding = decoded_encoding(chosen, transport_charset.as_deref(), &config.locale);
    {
        let mut document = document.borrow_mut();
        document.set_scripting_enabled(config.javascript);
        document.set_encoding(encoding.clone());
        document.refresh_base_url();
        // Pre-existing, and not this pass's to change: the tree reports
        // `complete` from here — before the `defer` scripts below and before
        // the caller reports `DOMContentLoaded` — where the standard goes
        // `interactive`, then the deferred scripts, then `complete` at `load`.
        document.set_ready_state(tenta_dom::ReadyState::Complete);
    }
    let hints = std::mem::take(&mut *hints.lock());
    // The `defer` scripts run here: the document is parsed and its tree is
    // finished, and the caller reports `DOMContentLoaded` only after this
    // returns. The `async` ones are not waited for — theirs started at their
    // own pauses and run whenever their fetch arrives. The stylesheets the rest
    // of the document linked start loading now, and a deferred script waits for
    // the ones that block scripts like any other.
    #[cfg(feature = "render")]
    {
        sheet_loader::sweep_parsed(inner, &jobs.tracker, &document, generation, true, chosen);
        if !jobs.deferred.is_empty() {
            sheet_loader::wait_for_blocking_sheets(
                inner,
                &jobs.tracker,
                &document,
                generation,
                deadline,
            )
            .await;
        }
    }
    drain_deferred(jobs, inner, &document, deadline, generation).await;
    // The parse handed the tree over: the navigation owns it, and the engine
    // behind it, from here.
    cleanup.disarm();
    Ok(ParsedBody {
        document,
        hints,
        bytes: total,
        encoding,
        timing,
        from_cache,
        protocol,
        redirects,
        mid_body_timeout: false,
    })
}

/// Starts the stylesheets of the markup parsed so far, at most once per
/// document generation, once the parser has settled on an encoding.
#[cfg(feature = "render")]
fn sweep_sheets_so_far(
    jobs: &mut ScriptJobs,
    inner: &Arc<PageInner>,
    document: &SharedDocument,
    encoding: Option<&'static Encoding>,
) {
    if encoding.is_none() {
        return;
    }
    let Ok(doc) = document.try_borrow() else {
        return;
    };
    let generation = doc.mutation_generation();
    drop(doc);
    if jobs.swept == Some(generation) {
        return;
    }
    jobs.swept = Some(generation);
    sheet_loader::sweep_parsed(
        inner,
        &jobs.tracker,
        document,
        jobs.generation,
        true,
        encoding,
    );
}

/// Starts the stylesheets a script that ran outside the parse's pauses (an
/// `async` or `defer` script) inserted: they are not parser-inserted, so they
/// never hold a later script back.
#[cfg(feature = "render")]
fn sweep_script_sheets(
    inner: &Arc<PageInner>,
    tracker: &Rc<RefCell<SettleTracker>>,
    document: &Weak<RefCell<DomDocument>>,
    navigation: u64,
) {
    if let Some(document) = document.upgrade() {
        sheet_loader::sweep_parsed(inner, tracker, &document, navigation, false, None);
    }
}

/// A `<meta charset>` restarts the parse into a fresh tree: the render session
/// the old tree's stylesheets were fetched for goes with it.
#[cfg(feature = "render")]
fn restart_sheets(jobs: &mut ScriptJobs, inner: &Arc<PageInner>) {
    jobs.swept = None;
    crate::render::release(inner.config.id);
}

/// How many due timer callbacks one turn of the settle loop runs before the
/// machine gets a look at the page. A page whose timers are always due would
/// otherwise run them forever and never let the settle call end.
const TIMERS_PER_TURN: usize = 64;

/// The shortest park the settle loop allows. A turn has just run, so parking
/// at zero would spin on the engine's own work.
const MINIMUM_PARK: Duration = Duration::from_millis(1);

/// Evaluates the settle machine, running the engine between evaluations and
/// waking on state changes, engine work or the deadline.
async fn wait_for_settle(
    inner: &Arc<PageInner>,
    tracker: &Rc<RefCell<SettleTracker>>,
    tree: &TreeHandle,
    deadline: Instant,
    timeout_millis: u64,
    condition_already_evaluated: bool,
    generation: u64,
) -> Result<SettleState> {
    settle_loop(
        inner,
        tracker,
        tree,
        deadline,
        timeout_millis,
        condition_already_evaluated,
        generation,
    )
    .await
}

async fn settle_loop(
    inner: &Arc<PageInner>,
    tracker: &Rc<RefCell<SettleTracker>>,
    tree: &TreeHandle,
    deadline: Instant,
    timeout_millis: u64,
    condition_already_evaluated: bool,
    generation: u64,
) -> Result<SettleState> {
    let page = inner.config.id;
    let notify = tracker.borrow().notify_handle();
    // The navigation's own generation, never re-read from the state: a bump
    // that lands between the install and this wait would otherwise be taken
    // as the baseline and the wait would run to its deadline.
    let initial_generation = generation;
    // Mutations are detected by generation rather than by a callback
    // (`docs/spec/02-architecture.md` §5.1): one `u64` compare per turn.
    let mut mutation_generation =
        with_document(inner, tree, |document| document.mutation_generation());
    loop {
        // The generation waiter is registered before the check, so a bump
        // between the check and the select below is never lost
        // (`notify_waiters` only reaches registered waiters).
        let generation_notified = inner.generation_changed.notified();
        tokio::pin!(generation_notified);
        generation_notified.as_mut().enable();
        {
            let state = inner.state.lock();
            // A crash wins over the generation check: a crashed page is not
            // merely "a new navigation started", and the generation it bumped
            // on the way here must not be reported as an abort.
            if let Some(reason) = state.crashed.clone() {
                return Err(Error::Crashed { reason });
            }
            if state.navigation_generation != initial_generation || state.closed {
                return Err(Error::Net {
                    code: ErrorName::Aborted.as_str().to_string(),
                    message: "navigation was cancelled".into(),
                });
            }
        }
        // One turn of the page loop: the timer callbacks that came due and the
        // rendering opportunity the engine owes. A script that queued work
        // therefore runs it while this settle call is waiting, and both kinds
        // of callback can mutate the tree. The runtime is named by the tree
        // this navigation parsed rather than by the page alone, because a
        // second navigation of the same page is interleaved at awaits and
        // acting on whichever runtime is in the table would drive its engine
        // (`crates/tenta-page/src/runtime.rs`).
        let engine_wakeup = tree
            .as_ref()
            .and_then(Weak::upgrade)
            .and_then(|document| {
                runtime::with_runtime_for(page, &document, |runtime| runtime.turn(TIMERS_PER_TURN))
            })
            .flatten();
        // A crash is raised *inside* that turn — the page's script-CPU budget
        // accumulates while the entry runs and `mark_crashed` fires there — so
        // nothing else notices it until the loop is checked again. Checking here
        // rather than only at the top of the loop ends the settle at once
        // instead of after another park.
        if let Some(reason) = inner.crash_reason() {
            return Err(Error::Crashed { reason });
        }
        // Images that entered the document, or whose source changed, start
        // loading here, and the ones whose load ended report to their
        // elements: all before the check below asks whether anything still
        // delays `load`, so an image the turn's scripts added holds it back,
        // and the `load` event of an element comes before the window's.
        #[cfg(feature = "render")]
        {
            image_loader::pump(inner, tracker, tree, initial_generation);
            if let Some(reason) = inner.crash_reason() {
                return Err(Error::Crashed { reason });
            }
        }
        let mutated = with_document(inner, tree, |document| document.mutation_generation());
        if mutated != mutation_generation {
            mutation_generation = mutated;
            tracker.borrow_mut().note_dom_mutation();
        }
        let now = inner.config.clock.now();
        // `load` fires here, once nothing holds it back: the parse is over and
        // no script is still awaiting its fetch or its turn to run
        // (`SettleTracker::load_delayed`). It is decided *before* `evaluate`,
        // because `evaluate` is what reports this turn's state and latches a
        // final one; a `load` set after it would be a turn late, and the park
        // computed in between would be the whole deadline on a page that is
        // otherwise finished. This is why the page no longer reports `load`
        // when its parse ends: an `async` script is still part of the
        // document's load chain after that.
        if !tracker.borrow().load_fired() && !tracker.borrow().load_delayed() {
            // Booked before it is fired, and that order is load-bearing twice
            // over. It is what makes the event once-per-navigation: a handler
            // that unwinds out of the dispatch — a watchdog termination inside
            // a listener — must not be able to earn a second one. And it is
            // what the settle machine reads: `evaluate` below runs after the
            // dispatch and must already see the load as fired, while the work
            // the dispatch queued (a tracked timer, a fetch) is counted in the
            // same turn, so a `load` handler cannot be torn down underneath the
            // continuation it starts.
            tracker.borrow_mut().note_load_fired();
            if let Some(document) = tree.as_ref().and_then(Weak::upgrade) {
                runtime::with_runtime_for(page, &document, |runtime| {
                    runtime.dispatch_lifecycle(LifecycleEvent::Load)
                });
            }
            inner.reach(initial_generation, LoadStage::Load);
        }
        // At Tier 1 the condition is evaluated once after install and no
        // mutation can change it. With an engine it is re-evaluated whenever
        // the tree may have moved.
        if tracker.borrow().wait_until().needs_condition() && !condition_already_evaluated {
            let met = condition_met(inner, tree, tracker.borrow().wait_until());
            tracker.borrow_mut().note_condition_met(met);
        }
        match tracker.borrow_mut().evaluate(now) {
            SettleState::Settled { busy } => return Ok(SettleState::Settled { busy }),
            SettleState::Timeout { pending } => {
                return Err(Error::Timeout {
                    what: format!("settle ({pending:?})"),
                    millis: timeout_millis,
                });
            }
            _ => {}
        }
        // Park until the machine, the engine or the deadline next needs the
        // thread. Only *remaining* durations are compared: the engine's clock
        // and the page's clock are not the same clock, so their instants never
        // meet — their delays do.
        let until_settle = tracker
            .borrow()
            .next_wakeup(now)
            .unwrap_or(deadline)
            .saturating_duration_since(now);
        let mut park = until_settle.min(deadline.saturating_duration_since(now));
        if let Some(engine_remaining) = engine_wakeup {
            park = park.min(engine_remaining);
        }
        // A turn has just run, so work that is due now is the *next* turn's
        // work: never park at zero and spin.
        let park_until = tokio::time::Instant::from_std(
            (now + park.max(MINIMUM_PARK)).min(deadline + MINIMUM_PARK),
        );

        // A tracker change, a generation bump (B-R4-1) or the next tracker
        // deadline all loop back to the checks above.
        let tracker_notified = notify.notified();
        tokio::select! {
            biased;
            _ = &mut generation_notified => {}
            _ = tracker_notified => {}
            _ = tokio::time::sleep_until(park_until) => {}
        }
    }
}

/// Whether the document satisfies a `Selector` or `Text` wait.
fn condition_met(inner: &Arc<PageInner>, tree: &TreeHandle, wait_until: &Settle) -> bool {
    with_document(inner, tree, |document| match wait_until {
        Settle::Selector(selector) => document
            .query_selector(document.document_node(), selector)
            .ok()
            .flatten()
            .is_some(),
        Settle::Text(text) => document.text().contains(text.as_str()),
        _ => true,
    })
    .unwrap_or(false)
}

/// How a settle call names the tree it reads. Weak because the tree belongs to
/// the navigation, the engine or the page, never to the call.
type TreeHandle = Option<Weak<RefCell<DomDocument>>>;

/// The tree a settle call reads while its navigation is still in flight.
///
/// `PageState` holds its document by value, so a tree the page owns alone is
/// moved in as soon as the parse hands it over, and every reader finds it
/// there. A tree an engine is bound to is not owned alone, so until the
/// navigation ends this is the only place it is reachable from. When the
/// navigation settles the engine is kept and the tree stays its own
/// ([`DeferredDocument::install`]); every other exit releases the engine, so a
/// cancelled or failed navigation cannot leave one behind on the page thread.
struct DeferredDocument {
    page: PageId,
    document: Option<SharedDocument>,
}

impl DeferredDocument {
    fn new(page: PageId, document: SharedDocument) -> DeferredDocument {
        DeferredDocument {
            page,
            document: Some(document),
        }
    }

    /// The tree while this navigation still holds it, for a settle call to
    /// read.
    fn shared(&self) -> Option<&SharedDocument> {
        self.document.as_ref()
    }

    /// A weak handle on the tree, for a settle call to read through. Weak so the
    /// call never keeps a tree alive that an engine release wants to move out.
    fn handle(&self) -> TreeHandle {
        self.document.as_ref().map(Rc::downgrade)
    }

    /// Ends the navigation's hold on the tree at the point the page settles.
    ///
    /// A tree an engine is bound to stays the engine's: the runtime is kept,
    /// `state.engine_held` records it and the returned handle is what the
    /// caller starts the engine's driver with. A tree nothing is bound to moves
    /// into the page. `None` means the tree went to the page, or is still
    /// shared and the page's installed document is left alone.
    fn install(&mut self, state: &mut PageState) -> Option<Weak<RefCell<DomDocument>>> {
        let document = self.document.take()?;
        if runtime::with_runtime_for(self.page, &document, |_| ()).is_some() {
            state.document = None;
            state.engine_held = true;
            state.cache_title(&document.borrow());
            return Some(Rc::downgrade(&document));
        }
        match Rc::try_unwrap(document) {
            Ok(cell) => state.document = Some(cell.into_inner()),
            Err(shared) => self.document = Some(shared),
        }
        None
    }

    /// Moves the tree into the page, releasing this navigation's engine first.
    ///
    /// Only for an install point at which the navigation is done with its
    /// engine. The engine is the other owner of the tree, so dropping it is
    /// what makes the tree movable — and it drops on the page thread that
    /// built it. Dropping it any earlier leaves a settling navigation with no
    /// engine to run the work it is settling for.
    ///
    /// `None` means the tree is still shared and the page's installed document
    /// is left alone; the caller tries again at its next install point.
    fn take(&mut self) -> Option<DomDocument> {
        if let Some(document) = self.document.as_ref() {
            drop(runtime::take_runtime_for(self.page, document));
        }
        self.take_if_owned()
    }

    /// Moves the tree into the page if the navigation already owns it alone.
    ///
    /// A tree an engine is bound to is shared with it, and this leaves it that
    /// way: the navigation is about to settle, and the engine is what the
    /// settle call pumps. The tree stays reachable through [`Self::shared`]
    /// until the install point that follows the settle, which is the first one
    /// at which the engine may go.
    fn take_if_owned(&mut self) -> Option<DomDocument> {
        let document = self.document.take()?;
        match Rc::try_unwrap(document) {
            Ok(cell) => Some(cell.into_inner()),
            Err(shared) => {
                self.document = Some(shared);
                None
            }
        }
    }
}

impl Drop for DeferredDocument {
    fn drop(&mut self) {
        if let Some(document) = self.document.take() {
            // A navigation that ends without installing still ends its engine:
            // nothing else owns it, and its tree is about to be dropped.
            drop(runtime::take_runtime_for(self.page, &document));
        }
    }
}

/// Runs `f` over the tree a settle call reads: the one an engine is bound to,
/// or the one the page has installed.
fn with_document<R>(
    inner: &Arc<PageInner>,
    tree: &TreeHandle,
    f: impl FnOnce(&DomDocument) -> R,
) -> Option<R> {
    if let Some(shared) = tree.as_ref().and_then(Weak::upgrade) {
        return Some(f(&shared.borrow()));
    }
    let state = inner.state.lock();
    state.document.as_ref().map(f)
}

/// Replaces the document with the error page a failed navigation leaves
/// behind (`docs/spec/20-platform-behaviour.md` §1).
/// Returns true if the error document was committed, false if the navigation
/// was cancelled or the page was closed.
fn commit_error_document(
    inner: &Arc<PageInner>,
    url: &Url,
    name: &str,
    message: &str,
    started: Instant,
    now: Instant,
    navigation_id: u64,
) -> bool {
    let document = error_document(url, name, message);
    let error_url = document
        .url()
        .cloned()
        .expect("the error document has a URL");
    let navigation = Navigation {
        url: url.clone(),
        final_url: error_url.clone(),
        status: 0,
        headers: Vec::new(),
        content_type: Some("text/html".to_owned()),
        encoding: "UTF-8".to_owned(),
        redirects: Vec::new(),
        timing: Timing {
            total: now.saturating_duration_since(started),
            ..Timing::default()
        },
        bytes: 0,
        from_cache: false,
        protocol: "local",
        settle: SettleState::Settled { busy: false },
        error: Some(name.to_owned()),
        navigation_id,
    };
    let commit = Commit {
        navigation_id,
        url: error_url.clone(),
        mime_type: "text/html".to_owned(),
        status: 0,
        error: Some(name.to_owned()),
        unreachable_url: Some(url.clone()),
    };
    let engine_held = {
        let mut state = inner.state.lock();
        // Check if this navigation was cancelled or the page was closed
        if state.navigation_generation != navigation_id || state.closed {
            return false;
        }
        state.commit_frame(&commit);
        state.document = Some(document);
        state.navigation = Some(navigation);
        state.hints.clear();
        state.settle = Some(SettleState::Settled { busy: false });
        state.tier = Tier::Dom;
        state.document_navigation_id = navigation_id;
        std::mem::take(&mut state.engine_held)
    };
    #[cfg(feature = "render")]
    crate::render::release(inner.config.id);
    // The error page replaces the document, so the engine the page kept for the
    // document it replaces ends with it.
    if engine_held {
        drop(runtime::take_runtime(inner.config.id));
    }
    inner.announce_commit(commit);
    inner.emit(PageEvent::FrameNavigated {
        frame: inner.frame,
        url: error_url,
    });
    // An error document has no scripts or subresources to wait for.
    for stage in LoadStage::ALL {
        inner.reach(navigation_id, stage);
    }
    true
}

fn build_request(config: &PageConfig, url: &Url, options: &Goto) -> Result<Request> {
    let method = if options.post.is_some() {
        http::Method::POST
    } else {
        http::Method::GET
    };
    let mut request = Request::new(method, url.clone())
        .resource_type(ResourceType::Document)
        .user_activated(true);
    if let Some(post) = &options.post {
        request.body = post.body.clone().into();
        request = request
            .header("content-type", &post.content_type)
            .map_err(|err| Error::invalid(err.to_string()))?;
    }
    if let Some(initiator) = &options.initiator {
        request = request.initiator(initiator.clone());
        if options.referer.is_none() {
            request = request.referrer(initiator.clone());
        }
    }
    // The transport gets a slightly later deadline than the page, so the
    // page's own timer is the one that decides and the error is `Timeout`
    // rather than a race between two clocks.
    request.limits = RequestLimits {
        total: Some(options.timeout + TRANSPORT_DEADLINE_GRACE),
        ..RequestLimits::default()
    };
    request.first_party = Site::from_url(url);
    for (name, value) in &config.extra_headers {
        request = request
            .header(name, value)
            .map_err(|err| Error::invalid(err.to_string()))?;
    }
    if let Some(referer) = &options.referer {
        let referer_url = Url::parse(referer)
            .map_err(|err| Error::invalid(format!("invalid referrer URL: {err}")))?;
        request = request.referrer(referer_url);
    }
    Ok(request)
}

/// The response body budget as the public error
/// (`docs/spec/14-memory.md` §3).
fn over_budget(max_bytes: u64) -> Error {
    Error::BudgetExceeded {
        kind: BudgetKind::NetworkBytes,
        component: Some(format!("document body over {max_bytes} bytes")),
    }
}

/// Maps a transport failure onto the public error a navigation reports.
///
/// The transport enforces the same byte limit and the same deadline as the
/// page and names them the way Chromium does; the public API reports a budget
/// as `BudgetExceeded` and a deadline as `Timeout`, whichever layer noticed
/// first.
fn navigation_error(error: Error, max_bytes: u64, timeout_millis: u64) -> Error {
    match &error {
        Error::Net { code, .. } if code == "net::ERR_FILE_TOO_BIG" => over_budget(max_bytes),
        Error::Net { code, .. } if code == "net::ERR_TIMED_OUT" => {
            Error::timeout("navigation", timeout_millis)
        }
        _ => error,
    }
}

/// The `net::ERR_*` name and the message of a transport failure.
fn net_error_parts(error: &Error) -> (String, String) {
    match error {
        Error::Net { code, message } => (code.clone(), message.clone()),
        Error::NetDenied { reason } => ("net::ERR_BLOCKED_BY_CLIENT".to_owned(), reason.clone()),
        other => ("net::ERR_FAILED".to_owned(), other.to_string()),
    }
}

/// The MIME type of a `Content-Type` value.
fn mime_of(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase()
}

/// The `charset` parameter of a `Content-Type` value.
fn charset_of(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let value = value.trim().trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_is_split_into_mime_and_charset() {
        assert_eq!(mime_of("text/html; charset=UTF-8"), "text/html");
        assert_eq!(mime_of("TEXT/HTML"), "text/html");
        assert_eq!(
            charset_of("text/html; charset=windows-1250").as_deref(),
            Some("windows-1250")
        );
        assert_eq!(
            charset_of("text/html;charset=\"utf-8\"").as_deref(),
            Some("utf-8")
        );
        assert_eq!(charset_of("text/html").as_deref(), None);
        assert_eq!(charset_of("text/html; charset=").as_deref(), None);
    }

    #[test]
    fn a_transport_failure_keeps_its_chromium_name() {
        let error = Error::Net {
            code: "net::ERR_CONNECTION_REFUSED".to_owned(),
            message: "refused".to_owned(),
        };
        assert_eq!(
            net_error_parts(&error),
            (
                "net::ERR_CONNECTION_REFUSED".to_owned(),
                "refused".to_owned()
            )
        );
        let denied = Error::NetDenied {
            reason: "loopback".to_owned(),
        };
        assert_eq!(net_error_parts(&denied).0, "net::ERR_BLOCKED_BY_CLIENT");
        assert_eq!(net_error_parts(&Error::Closed).0, "net::ERR_FAILED");
    }

    #[test]
    fn a_body_limit_from_either_layer_is_the_same_public_error() {
        let transport = Error::Net {
            code: "net::ERR_FILE_TOO_BIG".to_owned(),
            message: "response body is larger than the 32768-byte limit".to_owned(),
        };
        match navigation_error(transport, 32_768, 1_000) {
            Error::BudgetExceeded { kind, component } => {
                assert_eq!(kind, BudgetKind::NetworkBytes);
                assert!(component.expect("component").contains("32768"));
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
        let other = Error::Net {
            code: "net::ERR_CONNECTION_RESET".to_owned(),
            message: String::new(),
        };
        assert!(matches!(
            navigation_error(other, 32_768, 1_000),
            Error::Net { .. }
        ));
        let late = Error::Net {
            code: "net::ERR_TIMED_OUT".to_owned(),
            message: "waiting for headers exceeded the deadline".to_owned(),
        };
        assert!(matches!(
            navigation_error(late, 32_768, 150),
            Error::Timeout { millis: 150, .. }
        ));
    }

    #[test]
    fn goto_defaults_to_load_with_a_deadline() {
        let goto = Goto::default();
        assert_eq!(goto.wait_until, Settle::Load);
        assert_eq!(goto.timeout, Duration::from_secs(30));
        let goto = Goto::new()
            .wait_until(Settle::NetworkIdle)
            .timeout(Duration::from_millis(250))
            .referer("http://example.test/");
        assert_eq!(goto.wait_until, Settle::NetworkIdle);
        assert_eq!(goto.timeout, Duration::from_millis(250));
        assert_eq!(goto.referer.as_deref(), Some("http://example.test/"));
    }

    #[test]
    fn the_viewport_has_the_desktop_default() {
        let viewport = Viewport::default();
        assert_eq!((viewport.width, viewport.height), (1280, 800));
        assert!(!viewport.mobile);
    }
}
