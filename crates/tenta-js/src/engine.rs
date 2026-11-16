//! The engine boundary: what a page asks of a JavaScript engine, and what an
//! engine crate has to provide.
//!
//! The trait lives here because `tenta-js` is the only crate that both sides
//! can name. `tenta-page` owns the page thread and drives the parse, and may
//! depend on `tenta-js`, but `cargo xtask lint-deps` strips every backend
//! (`tenta-js-v8`, `tenta-js-qjs`) from every assembly crate except
//! `tenta-browser` and `tentabrowser`; the backend implementations, in turn,
//! sit below the assembly layer. So the page never names a concrete engine,
//! and the crate that selects a backend supplies a [`PageEngineFactory`].
//! That is the shape `docs/spec/02-architecture.md` §2 prescribes: the lower
//! crate defines the trait, the higher crate registers the implementation.
//!
//! # Borrow and re-entrancy rules
//!
//! These are the engine-boundary analogue of the listener rules in
//! [`crate::dom::events`]; breaking one causes a `RefCell` panic or a
//! use-after-free, not a wrong answer.
//!
//! 1. A caller of [`PageEngine`] must not hold a borrow of the shared document
//!    or of any settle bookkeeping across the call. Engine calls re-enter Rust
//!    — `setTimeout` reaches the settle machine through [`ProvenanceSink`], and
//!    script can touch the document through the bindings runtime — so a live
//!    borrow on the stack when that happens panics.
//! 2. [`PageEngine::document`] returns a clone of the shared handle. Borrow it
//!    for the body of one operation and drop it, exactly as
//!    `tenta_dom::SharedSink` does for the parser.
//! 3. Every raw handle a parser can reuse must have a native lifetime root.
//!    `tenta_dom::SharedSink` pins each published handle until finish/drop, so
//!    an engine pressure checkpoint may reclaim unrelated script-created nodes
//!    while parsing is paused. A checkpoint may run only before host work with
//!    no active conversion frame or document borrow; explicit trim does not
//!    sweep the native arena.
//!
//! Unlike the rest of `tenta-js`, this module needs no engine feature: the
//! traits are engine-independent by construction, and `tenta-js` compiles
//! without the `v8` or `quickjs` feature.

use std::rc::Rc;
use std::time::Duration;

use tenta_core::{NativeMemoryBudget, TimerRule};
use tenta_net::CookieJar;
use url::Url;

use crate::bindings::host::SharedDocument;
use crate::event_loop::ProvenanceSink;
use crate::fetch::{FetchCompletion, FetchDriver};
use crate::geometry::{GeometryHost, ViewportMetrics};
use crate::image::{ImageEvent, ImageHost};
use crate::modules::DynamicImport;
use crate::storage::PageStorage;
use crate::xhr::{XhrCompletion, XhrDriver};

/// How a script was delivered to the document.
///
/// [`PageEngine::run_script`] runs classic scripts. A module script is not a
/// kind of it: a module is compiled into the engine's module map and its root
/// is run once the page has loaded the graph
/// ([`PageEngine::compile_module`], [`PageEngine::run_module`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ScriptKind {
    Classic,
}

/// A lifecycle event the user agent fires into page script once per document.
///
/// Both variants are the HTML standard's own fire-and-forget events, and the
/// page only ever reports them: the target, the order and the flags are fixed
/// by the standard rather than by the caller, which is why this is a closed
/// enum rather than a type string. See [`PageEngine::dispatch_lifecycle`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum LifecycleEvent {
    /// `DOMContentLoaded`, at the `Document`, once the document has been parsed
    /// and its `defer` scripts have run (HTML "the end").
    DomContentLoaded,
    /// `load`, at the `Window`, once nothing delays the document's load chain
    /// any more (HTML "the load event").
    Load,
}

/// Why a script did not run to completion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ScriptErrorKind {
    /// The source could not be compiled.
    Syntax,
    /// It compiled and then threw.
    Uncaught,
    /// The engine stopped it — a script deadline or a heap limit.
    Terminated,
}

/// A script that did not run to completion, with the engine's own wording.
///
/// The message is carried for logging and page events; nothing in the engine
/// branches on it. A script error is a page-level fact, not an engine failure:
/// the page keeps loading, exactly as a real browser does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ScriptError {
    pub kind: ScriptErrorKind,
    pub message: String,
}

/// The clock the engine drives itself with.
///
/// A live page asks for [`Real`](EngineClock::Real) so timer delays and
/// `performance.now()` follow wall time. A deterministic harness asks for
/// [`Virtual`](EngineClock::Virtual) and steps the clock by hand, which is how
/// the timer and settle tests avoid sleeping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EngineClock {
    Real,
    Virtual,
}

/// What a page tells an engine when it creates one.
///
/// This is deliberately a plain data struct rather than a builder: it is
/// constructed once, on the page thread, from `tenta-page`'s own configuration
/// and handed to the factory.
#[derive(Clone, Debug)]
pub struct PageEngineConfig {
    /// When a timer's delay counts as a deferral rather than activity. The
    /// page derives this from its settle configuration, so the engine and the
    /// settle machine cannot disagree about what holds a settle open.
    pub timer_rule: TimerRule,
    pub clock: EngineClock,
    /// The engine's heap limit, from the page budget.
    pub max_heap_bytes: Option<usize>,
    /// A deadline for one engine call. `None` leaves the engine's own
    /// watchdog as the only limit.
    pub script_timeout: Option<Duration>,
    pub locale: String,
    pub timezone: Option<String>,
    /// The cookie jar `document.cookie` reads and writes.
    ///
    /// It is the browsing context's jar — the same store `tenta-net` sends
    /// from — so the value here is a clone of one `Rc`-shared handle, never a
    /// per-engine copy. The binding layer only passes it through; the jar's
    /// policy and its storage rules stay in `tenta-net`.
    pub cookies: CookieJar,
    /// The context's local storage and this page's session namespace.
    pub storage: PageStorage,
    /// Shared native accounting for the page and all of its document contexts.
    /// The Page owns this Arc across navigation-runtime replacement.
    pub native_memory: NativeMemoryBudget,
    /// The window metrics script sees until a [`GeometryHost`] is installed.
    pub viewport: ViewportMetrics,
}

/// One isolate driving one document.
///
/// Not `Send`: an engine is created, used and dropped on the page thread that
/// owns its document (`docs/spec/02-architecture.md` §3). The only part of it
/// that crosses threads is the watchdog an engine hands out, which is `Send`
/// for exactly that reason.
///
/// `document`, [`PageEngine::run_script`], [`PageEngine::evaluate`] and
/// [`PageEngine::dispatch_lifecycle`] touch page state. Engine entries may
/// run native callbacks; the page checks [`PageEngine::native_failure`] after
/// each one and discards a poisoned engine.
///
/// One lifecycle entry serves both `DOMContentLoaded` and `load`: the event
/// fixes its target and flags, while the page supplies the moment it fires.
pub trait PageEngine {
    /// The native callback that failed, if a Rust panic poisoned this engine.
    ///
    /// Sticky for the engine's remaining lifetime. Check after every entry,
    /// including those returning no result: this requires crashing the page
    /// group and discarding the engine, not reporting a script exception.
    fn native_failure(&self) -> Option<&'static str> {
        None
    }

    /// The document this engine is bound to — the same handle the parser
    /// writes into, so a paused parse and running script see one tree.
    fn document(&self) -> SharedDocument;

    /// Compiles and runs one script. The caller performs its microtask
    /// checkpoint after checking [`Self::native_failure`]. Without a native
    /// failure, a [`ScriptError`] is a script failure the page can survive.
    fn run_script(&mut self, source: &str, kind: ScriptKind) -> Result<(), ScriptError>;

    /// Evaluates an expression in the current document's world and returns its
    /// JavaScript string completion. The caller provides a per-call deadline,
    /// performs the microtask checkpoint after this entry returns, and keeps
    /// the document alive through both entries. This method does not perform
    /// the checkpoint or await a returned Promise.
    fn evaluate(&mut self, source: &str, timeout: Duration) -> Result<String, ScriptError>;

    /// Fires one user-agent lifecycle event at the target HTML prescribes, as a
    /// real DOM event the page can observe: it reaches `on*` handlers and
    /// `addEventListener` listeners alike, its `isTrusted` is true (the user
    /// agent, not script, fired it), and it supports `dispatchEvent` semantics
    /// — `preventDefault`, `stopPropagation` and a propagation path.
    ///
    /// The standard fixes everything about the event but its moment, so the
    /// caller supplies only which one to fire:
    ///
    /// - `DOMContentLoaded` is dispatched at the `Document`, bubbles, and is not
    ///   cancelable (HTML "the end").
    /// - `load` is dispatched at the `Window` with the legacy target override
    ///   flag set — so `event.target` is the document — and is neither bubbling
    ///   nor cancelable (HTML "the load event").
    ///
    /// # What the caller owes it
    ///
    /// This entry runs **page script**, so the caller must bound it the way it
    /// bounds a timer callback (`tenta_page::PageRuntime`), and must not hold a
    /// borrow of the shared document or of any settle bookkeeping across the
    /// call — the first rule of this boundary.
    ///
    /// Once-ness is the caller's, not this method's: a listener that calls
    /// `dispatchEvent(new Event('load'))` re-enters this path legitimately, so
    /// nothing here remembers what it has fired. The caller's own bookkeeping
    /// (`SettleTracker`) is what guarantees the document sees each event once.
    fn dispatch_lifecycle(&mut self, event: LifecycleEvent);

    /// Installs the inspection surface (`crate::inspect`) in the realm and
    /// points its events at `sink`: the `console` object, the dispatcher
    /// [`PageEngine::inspector_call`] drives, and the reporting of uncaught
    /// exceptions and unhandled rejections. Called once, before any page
    /// script runs, so `console` exists for the document's first script.
    ///
    /// The default installs nothing, which leaves the realm without a `console`
    /// and [`PageEngine::inspector_call`] refusing.
    fn set_inspector_sink(&mut self, _sink: Rc<dyn crate::inspect::InspectorSink>) {}

    /// One operation of the realm's inspection script, under a deadline of
    /// `timeout`. `argument` and the returned string are the JSON of
    /// `crate::inspect`'s protocol. A page exception is part of the reply; an
    /// `Err` means the engine stopped the entry.
    ///
    /// Like [`PageEngine::evaluate`], this does not perform the microtask
    /// checkpoint; the caller does.
    fn inspector_call(
        &mut self,
        _op: &str,
        _argument: &str,
        _timeout: Duration,
    ) -> Result<String, ScriptError> {
        Err(ScriptError {
            kind: ScriptErrorKind::Uncaught,
            message: "this engine does not provide inspection".to_owned(),
        })
    }

    /// Runs page code for the host and reports how it ended, as the JSON of
    /// `crate::inspect`'s protocol. `kind` is `"eval"`, where `source` is a
    /// script, or `"call"`, where `source` is a function expression and
    /// `argument` names its receiver and arguments.
    ///
    /// The backend compiles and runs `source` itself, so the page's stack ends
    /// at its own frame, and hands the completion or the exception to the realm
    /// script to describe. Deadline and microtask rules are those of
    /// [`PageEngine::inspector_call`].
    fn inspector_run(
        &mut self,
        _kind: &str,
        _source: &str,
        _argument: &str,
        _timeout: Duration,
    ) -> Result<String, ScriptError> {
        Err(ScriptError {
            kind: ScriptErrorKind::Uncaught,
            message: "this engine does not provide inspection".to_owned(),
        })
    }

    /// Applies the page's timer provenance rule. Called once at creation, and
    /// again whenever the page's settle configuration changes.
    fn set_timer_rule(&mut self, rule: TimerRule);

    /// Re-points the task-provenance observer at the settle machine that is
    /// collecting for the current settle.
    ///
    /// A page has one tracker per settle call, so this is called per
    /// navigation rather than once at creation (an engine outlives the settle
    /// that observed its first navigation).
    fn set_provenance_sink(&mut self, sink: Rc<dyn ProvenanceSink>);

    /// Points the engine's `fetch` global at the driver that answers its
    /// requests, replacing any driver installed before.
    ///
    /// Like [`PageEngine::set_provenance_sink`], this is called per navigation
    /// rather than once at creation: a driver's completions belong to the
    /// document that is current, and the table it leaves them in is the page's,
    /// one per navigation. An engine created without a driver still has a
    /// `fetch`; calling it rejects, because a promise the engine cannot keep is
    /// worse than a loud refusal.
    fn set_fetch_driver(&mut self, driver: Rc<dyn FetchDriver>);

    /// Gives the engine the layout-backed answers for script geometry,
    /// replacing any host installed before.
    ///
    /// Per navigation, like the drivers above: the host answers for the
    /// document that is current. An engine that never receives one behaves as
    /// a page with no layout (see [`crate::geometry`]). The default ignores
    /// the host, so a backend that has no geometry members yet needs no code.
    fn set_geometry_host(&mut self, _host: Rc<dyn GeometryHost>) {}

    /// Defines the members of the engine's built-in interfaces that cannot be
    /// declared in WebIDL (the static `DOMRect.fromRect`, `toJSON`): scripts the
    /// engine runs itself, once, before page script. Called once when a page
    /// builds an engine. The default does nothing, for a backend without them.
    fn install_preludes(&mut self) {}

    /// Gives the engine the page's image state, replacing any host installed
    /// before. `complete`, `naturalWidth`, `naturalHeight`, `currentSrc` and
    /// `decode()` read it. An engine that never receives one reports every
    /// image as complete with no size, which is what a page that loads no
    /// images shows. The default ignores the host, so a backend without these
    /// members needs no code.
    fn set_image_host(&mut self, _host: Rc<dyn ImageHost>) {}

    /// Gives the engine the page's linked stylesheets, replacing any host
    /// installed before: `link.sheet` and `document.styleSheets` read them. An
    /// engine that never receives one has no sheet for any `<link>`. The default
    /// ignores the host.
    fn set_style_host(&mut self, _host: Rc<dyn crate::stylesheets::StyleHost>) {}

    /// Reports the loads that ended: each event is fired at its element as a
    /// trusted `load` or `error` (neither bubbles nor is cancelable), and the
    /// `decode()` promises waiting on the element settle. One entry for the
    /// whole batch, entered like [`PageEngine::settle_xhr`]: with no V8
    /// callback on the stack, so the backend opens a plain scope. An empty
    /// batch must not be entered at all.
    ///
    /// Each event is its own task: the microtask queue is drained after every
    /// one, as HTML's "queue an element task" does, but the wrapper graph is
    /// checked once for the batch, because that walk is what makes a flood of
    /// single-event entries superlinear. `current(index, event)` is asked just
    /// before event `index` is fired, in order, and an event it refuses is
    /// skipped: an earlier handler of the same batch may have changed the
    /// element's source. It must not call back into script. The default does
    /// nothing.
    fn settle_images(
        &mut self,
        _events: Vec<ImageEvent>,
        _current: &mut dyn FnMut(usize, &ImageEvent) -> bool,
    ) {
    }

    /// Settles the promises this batch of completions names, as one entry: the
    /// values are built, the settlements queued as microtasks, and the engine
    /// records that a checkpoint is owed.
    ///
    /// The batch is passed in rather than drained from a table the engine owns
    /// because the table's owner is the page: the page is what decides when a
    /// completion becomes visible, and it is the page's turn that makes it so
    /// (`tenta_page::runtime`). An empty batch settles nothing and must not be
    /// entered at all.
    fn settle_fetches(&mut self, completions: Vec<FetchCompletion>);

    /// Points the engine's `XMLHttpRequest` objects at the driver that answers
    /// their requests, replacing any driver installed before.
    ///
    /// The same per-navigation contract as [`PageEngine::set_fetch_driver`], for
    /// the same reason: a completion belongs to the document that is current
    /// when it is drained. The XHR objects themselves land in b-5; this is the
    /// seam they will reach the driver through.
    fn set_xhr_driver(&mut self, driver: Rc<dyn XhrDriver>);

    /// Delivers the answers this batch of completions carries, as one entry:
    /// each completion is dispatched at the event target it names.
    ///
    /// A sibling of [`PageEngine::settle_fetches`] rather than a shape of it,
    /// because an XHR completion is an *event dispatch* and a fetch completion
    /// is a *promise settlement* — the two differ in what the engine does, not
    /// only in what it is told. The batch is passed in rather than drained from
    /// a table the engine owns for the same reason: the table is the page's, and
    /// it is the page's turn that makes a completion visible
    /// (`tenta_page::runtime`). An empty batch settles nothing and must not be
    /// entered at all.
    ///
    /// This entry runs with **no V8 callback on the stack** — it is the first
    /// engine entry that dispatches a DOM event from native code — so a backend
    /// has to bootstrap its scope the way a plain entry does rather than the way
    /// a binding call does.
    fn settle_xhr(&mut self, completions: Vec<XhrCompletion>);

    /// How long until the engine next has work of its own to do, as a delay
    /// from now. `None` means nothing is scheduled; `Some(Duration::ZERO)`
    /// means work is already due. A caller that treats `None` as "park
    /// forever" and `Some(ZERO)` as "run again now" is correct; a caller that
    /// sleeps on a `Some(ZERO)` spins.
    fn next_wakeup(&self) -> Option<Duration>;

    /// Runs at most `max_tasks` due timer callbacks, and returns how many it
    /// ran. Bounded so one turn of a page loop cannot be starved by a timer
    /// chain that reschedules itself.
    fn pump_due_timers(&mut self, max_tasks: usize) -> usize;

    /// True when an animation frame callback is due at the current time.
    fn frame_due(&self) -> bool;

    /// Runs the animation frame callbacks that were due when the frame was
    /// requested.
    fn run_animation_frame(&mut self);

    /// Runs the microtask queue to empty. Public because a page that has just
    /// run a script through [`PageEngine::run_script`] may still owe the
    /// checkpoints that script queued before the engine's own loop notices.
    fn run_microtasks(&mut self);

    /// Whether this engine runs module scripts. One that does not leaves
    /// `<script type="module">` unrun and runs `nomodule` scripts, which is
    /// what a page ships for exactly that case.
    fn supports_modules(&self) -> bool {
        false
    }

    /// Adds a document's import map (the text of a `<script
    /// type="importmap">`) to the realm's. `base` is the document's base URL.
    fn add_import_map(&mut self, json: &str, base: &Url) -> Result<(), ScriptError> {
        let _ = (json, base);
        Err(modules_unsupported())
    }

    /// Whether the module `key` is in the realm's module map.
    fn has_module(&self, key: &str) -> bool {
        let _ = key;
        false
    }

    /// Compiles `source` into the module map under `key` — the module's URL,
    /// or a name of the page's own for an inline module script — and returns
    /// the URLs it imports statically, resolved against `base` and through the
    /// import map. Nothing runs. The page loads those modules the same way and
    /// then runs the root with [`Self::run_module`], so that every module the
    /// engine asks for while linking is already there.
    fn compile_module(
        &mut self,
        key: &str,
        source: &str,
        base: &Url,
    ) -> Result<Vec<Url>, ScriptError> {
        let _ = (key, source, base);
        Err(modules_unsupported())
    }

    /// Links and evaluates the module `key`. The caller performs the microtask
    /// checkpoint, as for [`Self::run_script`].
    fn run_module(&mut self, key: &str) -> Result<(), ScriptError> {
        let _ = key;
        Err(modules_unsupported())
    }

    /// The `import()` calls script has made since the last call. The page
    /// loads each one's graph and answers with
    /// [`Self::finish_dynamic_import`].
    fn take_dynamic_imports(&mut self) -> Vec<DynamicImport> {
        Vec::new()
    }

    /// Settles the promise of the `import()` named `id`: `Ok` is the key of
    /// the module, whose graph is in the module map; `Err` is why it could not
    /// be loaded, which rejects the promise with a `TypeError`. The caller
    /// performs the microtask checkpoint.
    fn finish_dynamic_import(&mut self, id: u64, outcome: Result<&str, &str>) {
        let _ = (id, outcome);
    }

    /// Forces a collection and then clears the wrapper bookkeeping for every
    /// node whose wrapper the collector freed.
    ///
    /// This is the first half of reclaiming detached nodes: a node the engine
    /// no longer wraps still looks reachable to the DOM until the flag is
    /// cleared, so a sweep that runs before this keeps it. Callers run
    /// `trim_memory` and then sweep.
    fn trim_memory(&mut self);

    /// How many collections have completed. A page compares this against the
    /// value it saw last to decide whether collection bookkeeping may have
    /// changed under it — the "after engine GC callbacks" trigger for a sweep.
    fn gc_epoch(&self) -> u64;

    /// A handle the watchdog thread can use to stop a script that will not
    /// return. `None` when this engine cannot be interrupted from another
    /// thread, in which case the caller has no way to break a runaway script
    /// and must not pretend otherwise.
    fn watchdog(&self) -> Option<Box<dyn EngineWatchdog>>;
}

/// The part of an engine a watchdog thread may touch: `Send + Sync`, because
/// the page thread is blocked inside an engine call while this runs.
pub trait EngineWatchdog: Send + Sync {
    /// Asks the engine to stop the script it is running. Best effort and
    /// idempotent: the engine may be between scripts, or already stopped.
    fn terminate(&self);
}

/// Installed by the assembly crate that also selects the backend.
///
/// `Send + Sync` because it is stored in shared browser configuration, not on
/// the page thread; the engines it builds are not.
pub trait PageEngineFactory: Send + Sync + 'static {
    /// Builds an engine over `document`, or explains what the backend could
    /// not provide. The error is a string because the only caller turns it
    /// into an unsupported-feature report for the host.
    fn create(
        &self,
        document: SharedDocument,
        config: &PageEngineConfig,
    ) -> Result<Box<dyn PageEngine>, String>;
}

/// The failure every module operation reports on an engine without modules.
fn modules_unsupported() -> ScriptError {
    ScriptError {
        kind: ScriptErrorKind::Uncaught,
        message: "this engine does not run module scripts".to_owned(),
    }
}
