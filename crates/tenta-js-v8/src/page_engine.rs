//! The V8 backend's implementation of the engine boundary
//! ([`tenta_js::engine`]): the [`PageEngine`] surface a page drives, and the
//! [`PageEngineFactory`] the assembly crate installs to build one.
//!
//! It lives here, not in `tenta-js`, for the layering reason the trait's module
//! doc gives: the lower crate defines what a page may ask, and each backend
//! supplies its own answer. This module is a child of the crate root so it can
//! reach [`Engine`]'s private state — the wrapper world, the event loop's task
//! sink and the GC counter — without widening [`Engine`]'s public surface for
//! one caller.
//!
//! Every method here is a mapping onto an inherent [`Engine`] method, and the
//! places it is not are the ones that cash an invariant of the page layer: a
//! page engine is always built over a document (`document`), a script failure
//! is classified by where it failed and a script the page handed out counts as
//! part of the load chain (`run_script`).
//!
//! The borrow and re-entrancy rules in `tenta_js::engine`'s module doc apply to
//! these calls as much as to any other engine entry point. In particular
//! [`PageEngine::trim_memory`] borrows the shared document to clear
//! `HAS_WRAPPER`, and a caller must not be holding a borrow of it at the time.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use tenta_core::{Origin, TimerRule};
use tenta_js::bindings::host::SharedDocument;
use tenta_js::engine::{
    EngineClock, EngineWatchdog, LifecycleEvent, PageEngine, PageEngineConfig, PageEngineFactory,
    ScriptError, ScriptErrorKind, ScriptKind,
};
use tenta_js::event_loop::{Clock, MonoTime, ProvenanceSink};
use tenta_js::fetch::{FetchCompletion, FetchDriver};
use tenta_js::geometry::GeometryHost;
use tenta_js::image::{ImageEvent, ImageHost};
use tenta_js::inspect::InspectorSink;
use tenta_js::xhr::{XhrCompletion, XhrDriver};

use crate::Engine;

impl PageEngine for Engine {
    fn native_failure(&self) -> Option<&'static str> {
        Engine::native_failure(self)
    }

    /// **Invariant.** A page engine is always built over a document, and this
    /// is where that invariant is cashed: the only constructors that leave the
    /// engine without one — [`Engine::new`] and [`Engine::with_heap_limit`] —
    /// are one-shot evaluators, and a page reaches an engine through a
    /// [`PageEngineFactory`], which builds it with
    /// [`Engine::with_document_on_clock`].
    ///
    /// The returned handle is a clone of the shared one, so a paused parse and
    /// running script see the same tree. It is deliberately not invented here:
    /// a fresh `Document` would be a tree the parser is not writing into,
    /// which is the exact failure this method exists to rule out.
    fn document(&self) -> SharedDocument {
        self.dom
            .as_ref()
            .expect(
                "PageEngine::document on an engine built without a document: a page engine \
                 comes from PageEngineFactory (Engine::with_document_on_clock), never from \
                 Engine::new or Engine::with_heap_limit",
            )
            .ctx
            .doc
            .clone()
    }

    /// Runs one classic script under the deadline the page configured
    /// ([`Engine::set_script_timeout`]), reporting *where* it failed rather than
    /// only what V8 said.
    ///
    /// # A script the page hands out runs as part of the load chain
    ///
    /// The script runs under [`Origin::Tracked`], so the work it starts is
    /// attributed to the page's load rather than to the page's own background
    /// activity. A page only calls this for a `<script>` its parse is paused on
    /// (`tenta_page::runtime`), which is by definition part of loading the
    /// document — and it is what makes a short `setTimeout` an inline script
    /// schedules a *deferral* the settle call waits for rather than background
    /// activity it ignores. Without this the timer's parent would be the
    /// default [`Origin::Background`] and [`TimerRule::origin_for`] could never
    /// promote it, whatever the page's rule says.
    ///
    /// The origin is set on the loop's shared task state for the body of the
    /// call only, so it reaches exactly the tasks this script queues. An entry
    /// that is not a script the page handed out — [`Engine::evaluate`] — is
    /// left as background, which is what it is: work started after the page has
    /// loaded, on its own initiative.
    ///
    /// The microtask checkpoint the script's own work owes is the caller's to
    /// perform ([`PageEngine::run_microtasks`]), as it is for
    /// [`Engine::evaluate`]: the drive methods pay one after each callback, and
    /// an entry that is not one of them records the debt instead
    /// (`HostState::microtasks_pending`).
    fn run_script(&mut self, source: &str, kind: ScriptKind) -> Result<(), ScriptError> {
        match kind {
            // The only kind that exists today. A module script needs the module
            // map and the import graph (`docs/spec/05-javascript.md`); when one
            // lands, this arm is where the module path has to be implemented.
            ScriptKind::Classic => {}
            // `ScriptKind` is `#[non_exhaustive]`, so a variant added in
            // `tenta-js` cannot fail this match at compile time. Refusing it
            // keeps an unimplemented kind from silently running as a classic
            // script, which is the property that matters; no `ScriptErrorKind`
            // honestly describes "this engine has no implementation for it", so
            // the message names the kind and the script counts as not having
            // run.
            other => {
                return Err(ScriptError {
                    kind: ScriptErrorKind::Uncaught,
                    message: format!("this engine cannot run a {other:?} script yet"),
                });
            }
        }
        let timeout = self.script_timeout;
        // The scheduler is cloned out because `with_origin` borrows it while
        // the body borrows the engine mutably (`HostState::scheduler` is the
        // same handle the timer callbacks schedule through).
        let scheduler = self.host.scheduler.clone();
        scheduler
            .with_origin(Origin::Tracked, || self.run_classified(source, timeout))
            .map_err(|failure| ScriptError {
                kind: failure.kind,
                message: failure.message,
            })
    }

    fn evaluate(&mut self, source: &str, timeout: Duration) -> Result<String, ScriptError> {
        self.run_inner(source, Some(timeout))
            .map_err(|failure| ScriptError {
                kind: failure.kind,
                message: failure.message,
            })
    }

    fn set_inspector_sink(&mut self, sink: Rc<dyn InspectorSink>) {
        self.install_inspector(sink);
    }

    fn inspector_call(
        &mut self,
        op: &str,
        argument: &str,
        timeout: Duration,
    ) -> Result<String, ScriptError> {
        self.call_inspector(op, argument, timeout)
            .map_err(|failure| ScriptError {
                kind: failure.kind,
                message: failure.message,
            })
    }

    fn inspector_run(
        &mut self,
        kind: &str,
        source: &str,
        argument: &str,
        timeout: Duration,
    ) -> Result<String, ScriptError> {
        self.run_inspector(kind, source, argument, timeout)
            .map_err(|failure| ScriptError {
                kind: failure.kind,
                message: failure.message,
            })
    }

    /// Fires one user-agent lifecycle event at the target HTML prescribes,
    /// under the same [`Origin::Tracked`] scope [`PageEngine::run_script`] runs
    /// in — and for the same reason.
    ///
    /// A lifecycle event is part of loading the document, so the work its
    /// handlers start belongs to that load and not to the page's own background
    /// activity. It matters most for `load`, which is the last thing the load
    /// chain does: a `load` handler that calls `setTimeout` or `fetch` must keep
    /// a settle waiting for what it queued, or the page is torn down underneath
    /// the very work the event started. Without this wrapper the timer's parent
    /// would be the loop default and [`TimerRule::origin_for`] could never
    /// promote it, whatever the page's rule says.
    ///
    /// The scope is `with_origin`'s, so it covers exactly the tasks this
    /// dispatch queues and restores the previous provenance afterwards, even
    /// when a listener queued work of its own (`Scheduler::with_origin`).
    fn dispatch_lifecycle(&mut self, event: LifecycleEvent) {
        // Cloned out for the same borrow reason `run_script` clones it.
        let scheduler = self.host.scheduler.clone();
        scheduler.with_origin(Origin::Tracked, || Engine::dispatch_lifecycle(self, event));
    }

    fn set_timer_rule(&mut self, rule: TimerRule) {
        self.host.scheduler.set_timer_rule(rule);
    }

    fn set_provenance_sink(&mut self, sink: Rc<dyn ProvenanceSink>) {
        // The loop's own sink and the one the host callbacks schedule through
        // are the same `TaskSink`, so installing the observer here covers both
        // the tasks a callback queues and the ones the loop queues itself.
        self.event_loop.task_sink().set_provenance_sink(sink);
    }

    /// Installs the driver the engine's `fetch` global hands its requests to.
    ///
    /// The write lands in a slot on this engine's [`HostState`] rather than in a
    /// field of its own because a V8 callback reaches only the context slot
    /// ([`crate::host::host_state`]), and because `fetch` is defined by
    /// `host::install` before any driver exists. See
    /// [`crate::host::HostState::set_fetch_driver`].
    ///
    /// [`HostState`]: crate::host::HostState
    fn set_fetch_driver(&mut self, driver: Rc<dyn FetchDriver>) {
        self.host.set_fetch_driver(driver);
    }

    /// Stored on the [`HostState`] slot like the drivers, so geometry callbacks
    /// reach it through the context. See
    /// [`crate::host::HostState::set_geometry_host`].
    ///
    /// [`HostState`]: crate::host::HostState
    fn set_geometry_host(&mut self, host: Rc<dyn GeometryHost>) {
        self.host.set_geometry_host(host);
    }

    /// Named explicitly so the inherent method is what runs and a later trait
    /// default cannot shadow it.
    ///
    /// Each run of completions is entered under the provenance of the requests
    /// it answers, which the checkpoint that runs their reactions inherits
    /// (`HostState::microtask_origin`): the reactions are where the page's next
    /// request is made, and it has to be `Tracked` to keep the settle open for
    /// a page that loads its data link by link.
    fn settle_fetches(&mut self, completions: Vec<FetchCompletion>) {
        let scheduler = self.host.scheduler.clone();
        for (origin, run) in runs_by_origin(completions, |completion| completion.origin) {
            scheduler.with_origin(origin, || Engine::settle_fetches(self, run));
        }
    }

    /// Installs the driver the page's XHR objects hand their requests to.
    ///
    /// Stored in a slot on this engine's [`HostState`], like the `fetch` driver
    /// and for the same reason: the XHR surface reaches the world through the
    /// context slot, not through the engine. See
    /// [`crate::host::HostState::set_xhr_driver`].
    ///
    /// [`HostState`]: crate::host::HostState
    fn set_xhr_driver(&mut self, driver: Rc<dyn XhrDriver>) {
        self.host.set_xhr_driver(driver);
    }

    /// Named explicitly so the inherent method is what runs and a later trait
    /// default cannot shadow it.
    ///
    /// Entered under the requests' provenance, like [`Self::settle_fetches`].
    fn settle_xhr(&mut self, completions: Vec<XhrCompletion>) {
        let scheduler = self.host.scheduler.clone();
        for (origin, run) in runs_by_origin(completions, |completion| completion.origin) {
            scheduler.with_origin(origin, || Engine::settle_xhr(self, run));
        }
    }

    fn set_image_host(&mut self, host: Rc<dyn ImageHost>) {
        Engine::set_image_host(self, host);
    }

    fn set_style_host(&mut self, host: Rc<dyn tenta_js::stylesheets::StyleHost>) {
        Engine::set_style_host(self, host);
    }

    fn install_preludes(&mut self) {
        Engine::install_preludes(self);
    }

    /// Entered under [`Origin::Tracked`], like the lifecycle events: a `load`
    /// handler of an image is part of loading the document, so a timer it
    /// schedules is a deferral of that load.
    fn settle_images(
        &mut self,
        events: Vec<ImageEvent>,
        current: &mut dyn FnMut(usize, &ImageEvent) -> bool,
    ) {
        let scheduler = self.host.scheduler.clone();
        scheduler.with_origin(Origin::Tracked, || {
            Engine::settle_images(self, events, current);
        });
    }

    fn next_wakeup(&self) -> Option<Duration> {
        // Named explicitly: the inherent method would win method resolution
        // anyway, but saying which one is meant keeps a later trait default or
        // an added inherent method from silently changing this.
        Engine::next_wakeup(self)
    }

    fn pump_due_timers(&mut self, max_tasks: usize) -> usize {
        Engine::pump_due_timers(self, max_tasks)
    }

    fn frame_due(&self) -> bool {
        Engine::frame_due(self)
    }

    fn run_animation_frame(&mut self) {
        Engine::run_animation_frame(self)
    }

    fn run_microtasks(&mut self) {
        Engine::run_microtasks(self)
    }

    fn supports_modules(&self) -> bool {
        true
    }

    fn add_import_map(&mut self, json: &str, base: &url::Url) -> Result<(), ScriptError> {
        Engine::add_import_map(self, json, base).map_err(|error| ScriptError {
            kind: ScriptErrorKind::Syntax,
            message: error.message,
        })
    }

    fn has_module(&self, key: &str) -> bool {
        Engine::has_module(self, key)
    }

    /// Compiled under [`Origin::Tracked`]: a module script the document named
    /// is part of loading it, like a classic one.
    fn compile_module(
        &mut self,
        key: &str,
        source: &str,
        base: &url::Url,
    ) -> Result<Vec<url::Url>, ScriptError> {
        let scheduler = self.host.scheduler.clone();
        scheduler
            .with_origin(Origin::Tracked, || {
                self.compile_module_classified(key, source, base)
            })
            .map_err(|failure| ScriptError {
                kind: failure.kind,
                message: failure.message,
            })
    }

    fn run_module(&mut self, key: &str) -> Result<(), ScriptError> {
        let scheduler = self.host.scheduler.clone();
        scheduler
            .with_origin(Origin::Tracked, || self.run_module_classified(key))
            .map_err(|failure| ScriptError {
                kind: failure.kind,
                message: failure.message,
            })
    }

    fn take_dynamic_imports(&mut self) -> Vec<tenta_js::modules::DynamicImport> {
        Engine::take_dynamic_imports(self)
    }

    fn take_inserted_scripts(&mut self) -> Vec<tenta_js::scripts::InsertedScript> {
        Engine::take_inserted_scripts(self)
    }

    fn run_inserted_script(&mut self, source: &str, origin: Origin) -> Result<(), ScriptError> {
        let timeout = self.script_timeout;
        let scheduler = self.host.scheduler.clone();
        scheduler
            .with_origin(origin, || self.run_classified(source, timeout))
            .map_err(|failure| ScriptError {
                kind: failure.kind,
                message: failure.message,
            })
    }

    fn finish_dynamic_import(&mut self, id: u64, outcome: Result<&str, &str>) {
        let scheduler = self.host.scheduler.clone();
        scheduler.with_origin(Origin::Tracked, || {
            Engine::finish_dynamic_import(self, id, outcome);
        });
    }

    /// Forces a full collection and then applies the wrapper bookkeeping the
    /// collector just invalidated: the `HAS_WRAPPER` bit of every node wrapper
    /// whose finalizer ran, and the slab entries of collected snapshot and event
    /// wrappers.
    ///
    /// This is the first half of reclaiming detached nodes
    /// (`docs/spec/04-dom-and-css.md` §1.1): a node the engine no longer wraps
    /// still looks reachable to the DOM until the flag is cleared, so a sweep
    /// that runs before this keeps it. That bookkeeping is the same function the
    /// lazy sweep in `V8World::wrapper` uses, so the two cannot drift.
    ///
    /// It is also where the listeners of the nodes the *last* sweep freed are
    /// drained, and the reason this has to happen at an engine entry point:
    /// those entries live in this world's listener table and own a stored V8
    /// reference each, and the page thread cannot reach either. Draining is
    /// correct lazily — a freed node's entries can no longer fire — so running
    /// one sweep behind costs nothing but the entries' lifetime. A
    /// `ScriptHost` is installed here for every engine a page builds
    /// (`Engine::install_script_host` in `Engine::build`), which is what makes
    /// the release real rather than a silent no-op.
    fn trim_memory(&mut self) {
        if self.native_failure().is_some() {
            return;
        }
        let _entry = self.entered();
        self.graph_checkpoint();
    }

    fn gc_epoch(&self) -> u64 {
        self.gc_counter
            .epoch
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Always `Some`: a V8 isolate is interruptible from another thread through
    /// its thread-safe handle, which is exactly what this hands out. `None`
    /// would tell the page it has no way to break a runaway script.
    fn watchdog(&self) -> Option<Box<dyn EngineWatchdog>> {
        Some(Box::new(V8Watchdog(Arc::clone(&self.termination))))
    }
}

/// A [`PageEngine`]'s watchdog: the isolate's thread-safe handle, which
/// `terminate_execution` can reach from another thread while the page thread is
/// blocked inside a run (`docs/spec/05-javascript.md` §11).
///
/// `IsolateHandle` is `Send + Sync` and holds a lock over the isolate pointer,
/// so a terminate that races teardown sees a disposed isolate and does nothing
/// rather than dereferencing it.
struct V8Watchdog(Arc<crate::termination::TerminationGate>);

impl EngineWatchdog for V8Watchdog {
    fn terminate(&self) {
        self.0.request();
    }
}

/// Builds V8 engines over the pages that ask for one.
///
/// The assembly crate registers this with the browser's configuration, which is
/// shared across page threads, so the factory itself is `Send + Sync`; the
/// engines it builds are not and never leave the page thread that created them.
pub struct V8EngineFactory;

impl PageEngineFactory for V8EngineFactory {
    /// Builds an engine over `document`, applying the page's timer rule and
    /// script deadline.
    ///
    /// # What this backend cannot do yet
    ///
    /// A `config.timezone` is refused rather than ignored. V8 takes its
    /// timezone from process-global ICU configuration that it only accepts
    /// before the platform is initialised — and the platform is initialised
    /// once per process — so a per-page timezone has no implementation here.
    /// Returning an engine that silently reported UTC would be worse than
    /// saying so: the caller turns this error into an unsupported-feature
    /// report for the host.
    ///
    /// `config.locale` is likewise not applied per page for the same reason
    /// (V8's default locale is process-global), but it is *not* an error: the
    /// engine runs with V8's process locale, which is what the page would get
    /// anyway until the host configures it.
    fn create(
        &self,
        document: SharedDocument,
        config: &PageEngineConfig,
    ) -> Result<Box<dyn PageEngine>, String> {
        if let Some(timezone) = config.timezone.as_deref() {
            return Err(format!(
                "the V8 backend cannot apply the timezone {timezone:?}: V8 reads it from \
                 process-global ICU configuration that must be set before the platform is \
                 initialised, so it cannot vary per page"
            ));
        }
        let clock = match config.clock {
            // A live page follows wall time, so timers fire and
            // `performance.now()` advances without anyone stepping the clock.
            EngineClock::Real => Clock::real(),
            // A deterministic harness starts at the loop origin and steps the
            // clock itself.
            EngineClock::Virtual => Clock::virtual_at(MonoTime::ZERO),
        };
        let mut engine = Engine::with_document_on_clock_storage_and_memory(
            document,
            clock,
            config.max_heap_bytes,
            config.cookies.clone(),
            config.storage.clone(),
            config.native_memory.clone(),
        );
        // The page derives the rule from its settle configuration, so the
        // engine and the settle machine cannot disagree about what defers a
        // settle.
        engine.set_timer_rule(config.timer_rule);
        engine.set_script_timeout(config.script_timeout);
        engine.host.viewport.set(config.viewport);
        Ok(Box::new(engine))
    }
}

/// Splits `items` into consecutive runs that share a provenance, keeping
/// their order: a batch of completions is delivered in the order it arrived,
/// and each run under the provenance of the requests it answers.
fn runs_by_origin<T>(items: Vec<T>, origin_of: impl Fn(&T) -> Origin) -> Vec<(Origin, Vec<T>)> {
    let mut runs: Vec<(Origin, Vec<T>)> = Vec::new();
    for item in items {
        let origin = origin_of(&item);
        match runs.last_mut() {
            Some((last, run)) if *last == origin => run.push(item),
            _ => runs.push((origin, vec![item])),
        }
    }
    runs
}
