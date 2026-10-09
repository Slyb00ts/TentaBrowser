//! The page's JavaScript runtime: one engine bound to one parsed document
//! (`docs/spec/02-architecture.md` §3).
//!
//! A runtime exists while a page with an engine has a document it is bound to.
//! It is built lazily on the first `</script>` pause or a requested evaluation.
//! It runs script elements against the very tree the parser is writing into and
//! reports the engine's task provenance to the page's settle tracker.
//!
//! A navigation does not end it: a page whose navigation settled keeps its
//! runtime, so timers, fetch completions and later host entries run in the same
//! realm and the same tracker keeps accounting for them. It is dropped when the
//! next navigation commits, when the page closes, or when the page hands its
//! document out.
//!
//! *When* a classic script runs is the HTML standard's rule, not this
//! project's: the local spec leaves script timing to the standard
//! (`docs/spec/02-architecture.md` §5). The rules this module implements are
//! the standard's "the `script` element": an external classic script blocks the
//! parse unless it carries `async` or `defer`, `async` executes as soon as it
//! arrives, `defer` executes in document order once the document is parsed,
//! `defer` and `async` are ignored on an inline script, and `nomodule` is a
//! gate rather than a timing attribute.
//!
//! It lives in a thread-local keyed by page id because neither half of it is
//! `Send`: a document held jointly with script is `Rc<RefCell<_>>`, and an
//! engine is created, used and dropped on the page thread that owns its
//! document (`docs/spec/02-architecture.md` §3). The page thread is the only
//! place they may live, and it is the only place that reaches them.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use hashbrown::HashMap;
use tenta_core::atom::atoms;
use tenta_core::{Error, NativeMemoryBudget, NativeMemoryFailure, Origin, PageId, Result};
use tenta_dom::{SharedDocument, SharedSink};
use tenta_html::{Encoding, Parser};
use tenta_js::engine::{
    EngineClock, EngineWatchdog, LifecycleEvent, PageEngine, PageEngineConfig, ScriptKind,
};
use tenta_js::event_loop::ProvenanceSink;
use tenta_js::fetch::{FetchCompletions, FetchDriver};
use tenta_js::geometry::ViewportMetrics;
use tenta_js::inspect::{
    self, BindingCall, Completion, ConsoleCall, ExceptionEvent, InspectError, InspectorSink,
};
use tenta_js::xhr::{XhrCompletions, XhrDriver};
use tenta_net::policy::cookies::Site;
use tenta_net::{Client, NetError, Request, ResourceType};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use url::Url;

use crate::fetch::NetFetchDriver;
use crate::page::{PageConfig, Viewport, next_request_id};
use crate::settle::{RequestClass, Settle, SettleTracker};

thread_local! {
    /// The runtimes of this page thread, by page.
    static RUNTIMES: RefCell<HashMap<PageId, PageRuntime>> = RefCell::new(HashMap::new());
}

/// Forwards the engine's task provenance into the settle tracker of the page
/// (`docs/spec/02-architecture.md` §5.1).
///
/// The engine is handed this and reports every task it queues and finishes for
/// as long as it lives, which is why the tracker is the runtime's rather than a
/// settle call's: work started after `load` is accounted in the same machine a
/// later settle call evaluates. The tracker lives behind `Rc<RefCell<_>>`
/// because the page mutates it too; neither direction holds a borrow across
/// anything that can call back in.
struct SettleBridge {
    tracker: Rc<RefCell<SettleTracker>>,
}

impl ProvenanceSink for SettleBridge {
    fn task_queued(&self, origin: Origin) {
        self.tracker.borrow_mut().note_task_queued(origin);
    }

    fn task_finished(&self, origin: Origin) {
        self.tracker.borrow_mut().note_task_finished(origin);
    }
}

/// What an awaited promise settled with.
pub(crate) type Settlement = std::result::Result<Completion, InspectError>;

/// The evaluations waiting for a promise to settle, by ticket.
///
/// It belongs to the runtime, so an engine that ends takes its waiters with it:
/// their senders drop and each waiter learns its execution context is gone.
#[derive(Default)]
pub(crate) struct AwaitTable {
    waiting: RefCell<HashMap<u64, oneshot::Sender<Settlement>>>,
}

impl AwaitTable {
    pub(crate) fn register(&self, ticket: u64, sender: oneshot::Sender<Settlement>) {
        self.waiting.borrow_mut().insert(ticket, sender);
    }

    /// Forgets a waiter that gave up. A settlement that arrives later finds
    /// nobody and is dropped.
    pub(crate) fn cancel(&self, ticket: u64) {
        self.waiting.borrow_mut().remove(&ticket);
    }

    fn settle(&self, ticket: u64, settlement: Settlement) {
        if let Some(sender) = self.waiting.borrow_mut().remove(&ticket) {
            // A waiter that was dropped is not an error.
            let _ = sender.send(settlement);
        }
    }
}

/// Carries what the realm reports to the page: events to subscribers,
/// settlements to the evaluations awaiting them.
struct PageInspector {
    reporter: Arc<dyn ScriptBudget>,
    awaits: Rc<AwaitTable>,
}

impl InspectorSink for PageInspector {
    fn listening(&self) -> bool {
        self.reporter.has_subscribers()
    }

    fn console(&self, call: ConsoleCall) {
        self.reporter.emit(crate::event::PageEvent::Console(call));
    }

    fn exception(&self, event: ExceptionEvent) {
        self.reporter
            .emit(crate::event::PageEvent::Exception(event));
    }

    fn binding_called(&self, call: BindingCall) {
        self.reporter
            .emit(crate::event::PageEvent::BindingCalled(call));
    }

    fn settled(&self, ticket: u64, result: Settlement) {
        self.awaits.settle(ticket, result);
    }
}

/// How many mutations the page lets accumulate before reclaiming detached
/// nodes (`docs/spec/04-dom-and-css.md` §1.1 leaves the interval `N` open).
///
/// A mutation is any append, removal, attribute write or text edit, and the
/// parser counts one per inserted node, so `N` is really "about `N` nodes built
/// or rewritten". Two costs trade off:
///
///  * The sweep itself is cheap, and its cost scales with the work already done
///    rather than with `N`: it collects the detached-root set and walks only the
///    subtrees it frees, so repeated small sweeps cost no more in total than one
///    large one. `N` therefore does not need to be large to keep sweeps cheap.
///  * `trim_memory` is the expensive half and is what makes a sweep able to free
///    anything: it is the forced collection whose finalizers clear
///    `HAS_WRAPPER`, it walks the whole heap, and it must be amortised over far
///    more than a handful of mutations.
///
/// 4096 satisfies both for the workload this exists for. A framework re-render
/// is on the order of hundreds of mutations, so a detached subtree outlives the
/// render that detached it by only a few renders, while the forced collection
/// is spread over thousands of mutations instead of punctuating every render.
const MUTATIONS_PER_SWEEP: u64 = 4096;

/// Where one navigation's runtime reports the script time it spent.
///
/// The runtime itself is rebuilt per navigation, so it cannot own the total —
/// see `crate::script_cpu`. It reports, the page accumulates and decides.
pub(crate) trait ScriptBudget: Send + Sync {
    fn charge(&self, elapsed: Duration);
    fn crash_reason(&self) -> Option<String>;
    /// Returns the page-owned native ledger, never a navigation-local copy.
    fn native_memory(&self) -> NativeMemoryBudget;
    /// Marks a hard native-byte failure without classifying it as a Rust panic.
    fn crash_native_memory(&self, failure: NativeMemoryFailure);
    /// Reports that the watchdog cut an engine entry short. Termination is how
    /// a page survives a runaway, so it is a fact to report, not a crash.
    fn entry_terminated(&self, entry: &'static str);
    /// Reports a page-level event from inside an engine entry.
    fn emit(&self, _event: crate::event::PageEvent) {}
    /// Whether anything is subscribed to page events, so events whose payload
    /// costs something to build can be skipped when nobody would read them.
    fn has_subscribers(&self) -> bool {
        false
    }
    /// The names of the bindings every new engine must expose from its first
    /// script on.
    fn binding_names(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The task that runs a retained runtime's turns while no settle call does.
///
/// Dropping it ends the task, so a runtime that goes away takes its driver
/// with it and a driver can never outlive the engine it turns.
pub(crate) struct Pump(JoinHandle<()>);

impl Pump {
    pub(crate) fn new(task: JoinHandle<()>) -> Pump {
        Pump(task)
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One engine and the document it is bound to, on the page thread.
pub(crate) struct PageRuntime {
    page: PageId,
    engine: Box<dyn PageEngine>,
    document: SharedDocument,
    /// Whether a `tenta_html::Parser` is still writing into `document`.
    ///
    /// Page-loop reclamation stays deferred until parsing finishes. Engine
    /// pressure checkpoints can reclaim unrelated script-created nodes while
    /// paused: SharedSink's native pins retain every published parser handle.
    /// The assertion in `run_pending_script` still checks that this is a real
    /// parser pause, rather than an ordinary deferred/async script entry.
    parse_in_flight: bool,
    /// The page's settle machine. The engine reports into it through the
    /// bridge, the fetch driver registers requests in it, and a settle call
    /// restarts it ([`SettleTracker::begin_settle`]); it lives exactly as long as
    /// this runtime, so work that outlives a settle call stays accounted for.
    tracker: Rc<RefCell<SettleTracker>>,
    /// The task that runs this runtime's turns while no settle call does.
    pump: Option<Pump>,
    /// The engine's collection epoch as of the last turn that looked. A change
    /// means the collector may have freed a wrapper since, which is a "reclaim
    /// now" trigger of its own (`docs/spec/04-dom-and-css.md` §1.1).
    last_gc_epoch: u64,
    /// The engine's watchdog handle, taken once when the engine was built.
    ///
    /// `PageEngine::watchdog` is `&self` but hands out a fresh `Box` each call:
    /// the engine's thread-safe handle is valid for the isolate's life, so the
    /// page takes it once and names it, rather than asking the engine again
    /// before every entry. `None` means the engine cannot be interrupted from
    /// another thread, in which case no entry is armed and a callback that does
    /// not return hangs the page (`docs/spec/05-javascript.md` §11).
    watchdog: Option<Arc<dyn EngineWatchdog>>,
    /// The deadline one engine entry gets, from the page's script budget.
    ///
    /// The engine already carries the same value as its `script_timeout`
    /// (`engine_config`); this is that value in the page's own hands, because
    /// it is the page that arms the deadline around the three entries the
    /// engine does not bound itself. Zero means "no deadline here" — an
    /// already-elapsed budget is a pre-entry failure, not a terminate
    /// ([`PageRuntime::bounded`]).
    script_timeout: Duration,
    /// Where the page accumulates the script time this runtime spends.
    ///
    /// The runtime reports into it but does not own it: the total is the page's
    /// and outlives this navigation (`crate::script_cpu`).
    script_budget: Arc<dyn ScriptBudget>,
    /// One Page-owned ledger across this navigation and any Send wire.
    native_memory: NativeMemoryBudget,
    /// The completions this navigation's fetch driver has left for the page to
    /// settle.
    ///
    /// One table per navigation, like the runtime itself: a completion names a
    /// resolver of the engine that is current, and a table that outlived its
    /// engine would hold bits no engine can settle. The driver holds a handle to
    /// it (`tenta_js::fetch::FetchDriver`), so it is shared rather than owned
    /// outright.
    fetches: Rc<FetchCompletions>,
    /// The completions this navigation's XHR driver has left for the page to
    /// dispatch.
    ///
    /// One table per navigation, for the same reason [`Self::fetches`] is: a
    /// completion names an event target of the document that is current, and a
    /// table that outlived its engine would hold a handle no engine can
    /// dispatch at. The driver holds a handle to it (`tenta_js::xhr::XhrDriver`),
    /// so it is shared rather than owned outright.
    xhrs: Rc<XhrCompletions>,
    /// The evaluations waiting for a promise of this runtime's realm.
    awaits: Rc<AwaitTable>,
}

impl PageRuntime {
    /// Native panic is dominant. A hard memory charge instead marks the page
    /// crashed through its typed budget path after the engine frame returns.
    pub(crate) fn check_engine_failure(&self) {
        if let Some(callback) = self.engine.native_failure() {
            panic!("native boundary `{callback}` failed; the engine was discarded");
        }
        if let Some(failure) = self.native_memory.first_failure() {
            self.script_budget.crash_native_memory(failure);
        }
    }

    fn native_memory_error(&self) -> Option<Error> {
        self.native_memory
            .first_failure()
            .map(|failure| Error::BudgetExceeded {
                kind: failure.kind,
                component: Some(failure.component.name().to_owned()),
            })
    }

    /// Builds an engine over `document` and points it at `tracker`.
    fn create(
        page: PageId,
        document: SharedDocument,
        config: &PageConfig,
        tracker: Rc<RefCell<SettleTracker>>,
        script_budget: Arc<dyn ScriptBudget>,
    ) -> Result<PageRuntime, String> {
        let factory = config
            .engine
            .as_ref()
            .ok_or_else(|| "the page has no JavaScript engine factory".to_owned())?;
        let native_memory = script_budget.native_memory();
        let engine_config = engine_config(config, native_memory.clone());
        let mut engine = factory.create(document.clone(), &engine_config)?;
        // The rule the engine tags timers with comes from the settle
        // configuration, so the two cannot disagree about what holds a settle
        // open (`docs/spec/02-architecture.md` §5.1).
        engine.set_timer_rule(engine_config.timer_rule);
        engine.set_provenance_sink(Rc::new(SettleBridge {
            tracker: Rc::clone(&tracker),
        }) as Rc<dyn ProvenanceSink>);
        // Taken from the engine as built, so the first turn does not read the
        // engine's own construction-time collection as a change that owes a
        // reclaim.
        let last_gc_epoch = engine.gc_epoch();
        // Asked once, before the engine is boxed: the handle is valid for the
        // isolate's life and is what the watchdog thread terminates through.
        let watchdog = engine.watchdog().map(Arc::from);
        if watchdog.is_none() {
            tracing::debug!(
                page = ?page,
                "the engine cannot be terminated from another thread; a callback that does \
                 not return will hang this page"
            );
        }
        let script_timeout = engine_config.script_timeout.unwrap_or_default();
        // The table belongs to the navigation and the driver gets a handle to
        // it, so a completion the driver leaves behind is drained by this
        // navigation's `turn` and never by another's. The driver is installed
        // before this runtime is reachable, and `create` runs before the paused
        // script is run, so a driver exists before any script can call `fetch`
        // or `xhr.send()`.
        let fetches = Rc::new(FetchCompletions::new());
        // The XHR table is the fetch table's sibling and is created here for the
        // same reason; it is built before the driver because the driver is what
        // fills it. Both tables are handed to the *one* driver: `fetch()` and
        // `XMLHttpRequest` differ in the resource type and in which table the
        // answer goes to, and share both cross-origin gates, so a second driver
        // would be a second place for gate 2 to be forgotten
        // (`NetFetchDriver`).
        let xhrs = Rc::new(XhrCompletions::new());
        // The driver registers requests in the same machine, and the
        // notification the page's driver loop parks on is the one it wakes.
        let driver = Rc::new(NetFetchDriver::new(
            config,
            Rc::clone(&tracker),
            Rc::clone(&fetches),
            Rc::clone(&xhrs),
        ));
        engine.set_fetch_driver(Rc::clone(&driver) as Rc<dyn FetchDriver>);
        engine.set_xhr_driver(driver as Rc<dyn XhrDriver>);
        // Before any script runs, so the document's first script already finds
        // `console` and the bindings the host registered.
        let awaits = Rc::new(AwaitTable::default());
        engine.set_inspector_sink(Rc::new(PageInspector {
            reporter: Arc::clone(&script_budget),
            awaits: Rc::clone(&awaits),
        }));
        for name in script_budget.binding_names() {
            if let Err(error) = inspect::bind(&mut *engine, &name, script_timeout) {
                tracing::warn!(page = ?page, binding = %name, %error, "a binding could not be installed");
            }
        }
        Ok(PageRuntime {
            page,
            engine,
            document,
            // A parse-pause runtime starts with a parser in flight. A runtime
            // created for post-settle evaluation is marked parse-finished
            // before its first engine entry.
            parse_in_flight: true,
            tracker,
            pump: None,
            last_gc_epoch,
            watchdog,
            script_timeout,
            script_budget,
            native_memory,
            fetches,
            xhrs,
            awaits,
        })
    }

    /// One turn of the page loop: reclaim detached nodes when it is safe and
    /// due, run what the engine owes now, then say when it next has something
    /// to do.
    ///
    /// `Some(Duration::ZERO)` means work is due already and the caller must
    /// take another turn rather than park; `None` means the engine has nothing
    /// scheduled. Only remaining durations leave this method — the engine
    /// clock and the page clock are never compared.
    ///
    /// This is the page-loop detached-node reclamation point. Engines also
    /// perform pressure checkpoints during long scripts; those must preserve
    /// native parser pins and run before host work without active borrows.
    /// The page-loop guard remains conservative while parsing is in flight.
    /// Once settled, engine task-end collection releases wrapper roots, but
    /// native arena sweeping waits for this turn or a later pressure checkpoint.
    pub(crate) fn turn(&mut self, budget: usize) -> Option<Duration> {
        if self.script_budget.crash_reason().is_some() {
            return None;
        }
        if !self.parse_in_flight && self.should_reclaim_detached() {
            self.reclaim_detached();
        }
        // The fetch drain comes *before* the timers, because HTML's order is
        // "run a task, then perform a microtask checkpoint": the drain is the
        // task a driver's completion belongs to, and `pump_due_timers`' leading
        // checkpoint runs the reactions it queued in the same turn, so the debt
        // is paid at once rather than through another turn
        // (`crates/tenta-js-v8/src/lib.rs`, `pump_due_timers`).
        //
        // This is also where a completion that arrived off the engine's call
        // stack becomes observable. The page thread is a single-threaded
        // `LocalSet` and the settle loop's `select!` is its only await point, so
        // a task can only run while the loop is parked — and when it wakes, the
        // loop restarts and calls `turn` before `evaluate`. A completion such a
        // task pushed is therefore always drained before the settle can be
        // judged finished, which is what makes the driver's push-then-wake order
        // a contract rather than a courtesy (`tenta_js::fetch::FetchDriver`).
        //
        // One entry for the whole batch: one watchdog arm, one context entry,
        // one stray-termination clear. An empty table costs no entry at all.
        let completions = self.fetches.take_all();
        if !completions.is_empty() {
            self.bounded("fetch", |engine| engine.settle_fetches(completions));
            if self.script_budget.crash_reason().is_some() {
                return None;
            }
        }
        // The XHR drain sits immediately after the fetch drain and before the
        // timers, for the same reason the fetch drain does: it is the task a
        // completion belongs to, and `pump_due_timers`' leading microtask
        // checkpoint is what pays the debt the dispatch creates, in the same
        // turn. A `load` handler that queues a reaction must not wait for an
        // unrelated timer to come due. One entry for the whole batch — one
        // watchdog arm, one native-dispatch guard, one stray-termination clear —
        // and an empty table costs no entry at all.
        let xhr_completions = self.xhrs.take_all();
        if !xhr_completions.is_empty() {
            self.bounded("xhr", |engine| engine.settle_xhr(xhr_completions));
            if self.script_budget.crash_reason().is_some() {
                return None;
            }
        }
        // Both entries below run callback script the engine does not bound
        // itself, so each is armed around its call (`PageRuntime::bounded`).
        self.bounded("timers", |engine| engine.pump_due_timers(budget));
        if self.script_budget.crash_reason().is_some() {
            return None;
        }
        if self.engine.frame_due() {
            self.bounded("animation-frame", |engine| engine.run_animation_frame());
            if self.script_budget.crash_reason().is_some() {
                return None;
            }
        }
        self.engine.next_wakeup()
    }

    /// Runs one engine entry under the page's per-entry script deadline, when
    /// the engine offers a watchdog to enforce it, and charges the page's
    /// cumulative script budget for whatever the entry spends either way.
    ///
    /// `run_script` bounds itself, by a private per-run thread inside the
    /// backend, and is deliberately not armed here — arming it too would put a
    /// second terminate on the same run. The entries this wraps are the ones
    /// with no deadline at all: the timers and animation frames of a turn, and
    /// the microtask checkpoint a script's own entry owes.
    ///
    /// The deadline is armed immediately before the entry and disarmed when
    /// `arm` drops, after it returns. A fire that lands after the entry
    /// returned but before the disarm still sets `fired` — the page learns the
    /// deadline elapsed — while the terminate it issued is dropped by the next
    /// entry's start-of-entry clear (`crates/tenta-js-v8`, stage (a), commit
    /// `d1ee1d9`). A deadline that has already passed at arming time is a
    /// pre-entry failure, not a terminate: the entry's clear would swallow it
    /// and the entry would run unbounded, so nothing is armed when the page has
    /// no time left; what such an entry spends is still charged below.
    ///
    /// The *charge* is unconditional, unlike the arm: the page's cumulative
    /// script budget is a fact about the entry however the entry was bounded,
    /// so an engine with no watchdog and a zero per-entry timeout are still
    /// accounted for (`crate::script_cpu`).
    fn bounded<R>(&mut self, entry: &'static str, run: impl FnOnce(&mut dyn PageEngine) -> R) -> R {
        let started = Instant::now();
        // The deadline is armed only when there is something to arm it through
        // and a non-zero timeout to enforce; the decision that used to be two
        // early returns is now local to the arm.
        let arm = match &self.watchdog {
            Some(handle) if !self.script_timeout.is_zero() => Some(crate::watchdog::arm(
                Arc::clone(handle),
                started + self.script_timeout,
            )),
            _ => None,
        };
        let outcome = run(&mut *self.engine);
        self.check_engine_failure();
        let elapsed = started.elapsed();
        self.script_budget.charge(elapsed);
        // An engine that also enforces its own deadline may stop the entry
        // before the watchdog's arm fires, and the page cannot tell the two
        // apart; an entry that used its whole deadline was cut short either way.
        let cut_short = arm.as_ref().is_some_and(crate::watchdog::Arm::fired)
            || (!self.script_timeout.is_zero() && elapsed >= self.script_timeout);
        if cut_short {
            tracing::warn!(
                page = ?self.page,
                entry,
                timeout = ?self.script_timeout,
                "an engine entry did not return within its deadline and was terminated"
            );
            self.script_budget.entry_terminated(entry);
        }
        outcome
    }

    /// Runs a host-supplied entry under the same per-entry deadline, script
    /// budget and native-failure check the page's own entries get.
    ///
    /// The entry may have scheduled work the page's driver has not heard of, so
    /// the driver is woken afterwards.
    pub(crate) fn run_in_engine<R>(&mut self, run: impl FnOnce(&mut dyn PageEngine) -> R) -> R {
        let outcome = self.bounded("host", run);
        self.tracker.borrow().notify_handle().notify_one();
        outcome
    }

    /// [`Self::run_in_engine`] for an entry that may leave an evaluation
    /// waiting on a promise of this realm.
    pub(crate) fn run_in_engine_awaiting<R>(
        &mut self,
        run: impl FnOnce(&mut dyn PageEngine, &AwaitTable) -> R,
    ) -> R {
        let awaits = Rc::clone(&self.awaits);
        self.run_in_engine(|engine| run(engine, &awaits))
    }

    /// The evaluations waiting for a promise of this runtime's realm.
    pub(crate) fn awaits(&self) -> &Rc<AwaitTable> {
        &self.awaits
    }

    /// The settle machine this runtime reports into.
    pub(crate) fn tracker(&self) -> &Rc<RefCell<SettleTracker>> {
        &self.tracker
    }

    /// The tree this runtime is bound to.
    pub(crate) fn document(&self) -> &SharedDocument {
        &self.document
    }

    /// Records the task that drives this runtime between settle calls.
    pub(crate) fn set_pump(&mut self, pump: Pump) {
        self.pump = Some(pump);
    }

    /// Stops the task that drives this runtime, so a settle call can drive it
    /// itself. Reports whether one was running.
    pub(crate) fn stop_pump(&mut self) -> bool {
        self.pump.take().is_some()
    }

    /// Ends the runtime and hands its tree back, if nothing else holds it.
    ///
    /// The engine goes first: it is the other owner of the tree, so dropping it
    /// is what makes the tree movable, and it drops on the page thread that
    /// built it.
    pub(crate) fn into_document(self) -> Option<tenta_dom::Document> {
        let PageRuntime {
            engine,
            document,
            pump,
            ..
        } = self;
        drop(pump);
        drop(engine);
        Rc::try_unwrap(document).ok().map(RefCell::into_inner)
    }

    /// Fires one user-agent lifecycle event into page script, under the same
    /// per-entry deadline and budget accounting as the entries [`Self::turn`]
    /// drives.
    ///
    /// This entry runs whatever the page registered for the event, so it is one
    /// of the entries `run_script` does not bound: an `onload` handler with an
    /// infinite loop must hang neither the page nor the process. Routing it
    /// through [`PageRuntime::bounded`] is what arms the page's deadline around
    /// it and charges the page's cumulative script budget for what it spends,
    /// exactly as the timer pump and the animation frame are treated.
    ///
    /// It is deliberately *not* part of [`Self::turn`]. Both lifecycle events
    /// are fired once, at moments the settle loop and the parse decide, so
    /// putting them in a method that runs on every turn would need a flag here
    /// as well as in the settle machine — two places to keep once-ness in step
    /// instead of one.
    pub(crate) fn dispatch_lifecycle(&mut self, event: LifecycleEvent) {
        if self.script_budget.crash_reason().is_some() {
            return;
        }
        self.bounded("lifecycle", |engine| engine.dispatch_lifecycle(event));
    }

    /// Whether this turn should reclaim detached nodes: the collector has run
    /// since the last look, or enough mutations have accumulated.
    ///
    /// The engine is asked first and the document is borrowed for the length of
    /// one statement — never across the engine call, which is the first rule of
    /// the boundary (`tenta_js::engine`).
    fn should_reclaim_detached(&self) -> bool {
        if self.engine.gc_epoch() != self.last_gc_epoch {
            return true;
        }
        self.document.borrow().mutations_since_sweep() >= MUTATIONS_PER_SWEEP
    }

    /// Collects, then reclaims the detached subtrees the collection made
    /// unreachable.
    ///
    /// The order is the boundary's, not this module's: a node the engine no
    /// longer wraps still looks reachable to the DOM until `trim_memory` clears
    /// the wrapper's flag and drains the listeners of the previous sweep, so a
    /// sweep that ran first would keep it. The engine also owns the listener
    /// entries and stored callbacks invalidated by a sweep. Explicit trim and
    /// internal pressure checkpoints both apply that exact-generation cleanup.
    fn reclaim_detached(&mut self) {
        self.engine.trim_memory();
        self.check_engine_failure();
        if self.script_budget.crash_reason().is_some() {
            return;
        }
        let stats = self.document.borrow_mut().sweep_detached();
        if stats.nodes_freed > 0 {
            tracing::debug!(
                page = ?self.page,
                examined = stats.roots_examined,
                kept = stats.roots_kept,
                freed = stats.nodes_freed,
                "reclaimed detached nodes"
            );
        }
        // Read after the trim, never before: the trim forces a collection,
        // which advances the epoch, and an epoch sampled first would read as "a
        // collection happened" on every later turn and reclaim forever.
        self.last_gc_epoch = self.engine.gc_epoch();
    }

    /// Runs the script the parse is paused on, against the tree as it stands,
    /// or says how it must be run instead.
    ///
    /// This is the one place the page enters an engine with its parser
    /// stopped, and the three borrow rules of the boundary apply
    /// (`tenta_js::engine`): the document borrow ends before the engine is
    /// called, and the sink's native pins retain every handle the parser may
    /// reuse across an engine pressure checkpoint. Every
    /// skip is a real decision about the HTML, so each one is logged: a
    /// non-classic type is not this driver's to run, and an empty element has
    /// nothing to run.
    ///
    /// The decisions are taken in the order the standard takes them, because
    /// each one is a precondition of the next: the `type` first, since a
    /// non-classic script is not a classic script whatever else it carries and
    /// must never become a deferred one; then `src`, which is what makes a
    /// script external and therefore able to be deferred at all; then `async`
    /// over `defer` over blocking.
    ///
    /// A classic script with a `src` is not skipped, but its text is not here
    /// either: it comes from the network, so the caller fetches what
    /// [`ScriptStep`] names and runs it through [`PageRuntime::run_source`].
    /// Splitting the two is what keeps no borrow of this runtime alive across
    /// the fetch that separates them.
    fn run_pending_script(
        &mut self,
        parser: &mut Parser<'_, SharedSink>,
        base: &Url,
    ) -> Result<ScriptStep, Error> {
        let Some(script) = parser.take_pending_script() else {
            tracing::debug!(page = ?self.page, "the parse is not paused on a script");
            return Ok(ScriptStep::Done);
        };
        debug_assert!(
            self.parse_in_flight,
            "a script ran while no parser was writing into its document"
        );
        debug_assert!(
            Rc::ptr_eq(&self.document, &parser.sink().document()),
            "the engine is bound to a different tree than the parser writes into"
        );
        // One short borrow: everything the decision needs is copied out before
        // the engine is entered.
        let (source, kind, src, timing) = {
            let document = self.document.borrow();
            let source = document.text_content(script);
            let kind = document.attribute(script, atoms::TYPE).map(str::to_owned);
            let src = document.attribute(script, atoms::SRC).map(str::to_owned);
            let timing = ScriptTiming {
                asynchronous: document.has_attribute(script, atoms::ASYNC),
                deferred: document.has_attribute(script, atoms::DEFER),
            };
            (source, kind, src, timing)
        };
        // Before `src`, because a module's text is not a classic script's:
        // running one inline would run it in the wrong scope.
        if let Some(kind) = &kind
            && !is_classic_javascript(kind)
        {
            tracing::debug!(
                page = ?self.page,
                kind = %kind,
                "a non-classic script type is not run inline (P2 d-2c)"
            );
            return Ok(ScriptStep::Done);
        }
        // `nomodule` is deliberately not consulted anywhere here: it is a
        // gate, not a timing attribute, and it says "do not run this in a user
        // agent that supports module scripts". This one refuses `type=module`
        // (P2 d-2c), so the gate is open and a `nomodule` script runs exactly
        // like the same script without it — which leaves a lone external one
        // parser-blocking, since it carries no `async` or `defer`. **When
        // module support lands, this gate must close.**
        if let Some(src) = &src {
            // The base is the document's, exactly as a recorded preload hint's
            // is resolved (`crate::fetch_policy`): the URL a relative `src`
            // resolves against, not the script's own.
            let url = match base.join(src) {
                Ok(url) => url,
                Err(error) => {
                    tracing::warn!(
                        page = ?self.page,
                        src = %src,
                        %error,
                        "a script's src does not resolve against the document; it does not run"
                    );
                    return Ok(ScriptStep::Done);
                }
            };
            // `async` wins when both are present; `defer` is the legacy
            // fallback the standard keeps for it.
            return Ok(if timing.asynchronous {
                ScriptStep::Deferred(DeferredTiming::Async(url))
            } else if timing.deferred {
                ScriptStep::Deferred(DeferredTiming::Defer(url))
            } else {
                ScriptStep::Fetch(url)
            });
        }
        // Inline: `defer` and `async` are ignored on a script with no `src`,
        // so it runs here whatever it carries.
        if source.trim().is_empty() {
            tracing::debug!(page = ?self.page, "an empty script element runs nothing");
            return Ok(ScriptStep::Done);
        }
        self.run_source(&source);
        Ok(ScriptStep::Done)
    }

    /// Runs `source` as a classic script against the tree.
    ///
    /// A script error is a page-level fact, not an engine failure: the page
    /// keeps loading, exactly as a browser does. The script's own checkpoints
    /// are owed even when it threw.
    ///
    /// Two callers, and they are the two points at which a classic script
    /// runs: the parse pause, for an inline script and for an external
    /// parser-blocking one (`run_pending_script` above), and the fetch that
    /// resolves later, for `defer` and `async` (`crate::page`). The second is
    /// why there is no `parse_in_flight` assertion here: after
    /// `parser.finish()` no parser is writing into the tree, and that is
    /// exactly when a deferred script runs. SharedSink pins preserve parser
    /// handles at pressure checkpoints while the parse is paused; ordinary
    /// page-loop reclamation remains guarded by `turn`.
    pub(crate) fn run_source(&mut self, source: &str) {
        if self.script_budget.crash_reason().is_some() {
            return;
        }
        // `run_script` bounds itself, with a private per-run thread inside the
        // backend, so it is not wrapped in `bounded` — but its time is still
        // the page's script time and is charged here. Charging only the
        // microtask checkpoint below would miss the script itself.
        let started = Instant::now();
        if let Err(error) = self.engine.run_script(source, ScriptKind::Classic) {
            tracing::warn!(
                page = ?self.page,
                kind = ?error.kind,
                message = %error.message,
                "a script did not run to completion"
            );
        }
        self.check_engine_failure();
        self.script_budget.charge(started.elapsed());
        // The script's own checkpoints are owed even when it threw, and the
        // checkpoint runs callback script the engine does not bound itself.
        // A budget crash already requires discarding this engine, however;
        // entering its queued callbacks could only obscure that first failure.
        if self.script_budget.crash_reason().is_some() {
            return;
        }
        self.bounded("microtasks", |engine| engine.run_microtasks());
    }

    /// Evaluates a host expression in the navigation's live world. The
    /// navigation deadline bounds the script and its immediate microtasks;
    /// async tasks queued here are not awaited by this call.
    pub(crate) fn evaluate(&mut self, source: &str, remaining: Duration) -> Result<String> {
        // A zero page entry timeout disables that local limit, but the
        // navigation's remaining deadline still bounds this host operation.
        let timeout = if self.script_timeout.is_zero() {
            remaining
        } else {
            self.script_timeout.min(remaining)
        };

        let started = Instant::now();
        let result = self.engine.evaluate(source, timeout);
        // A native callback panic is a page crash, not a catchable script
        // exception. The backend reports it only after the V8 frame unwinds.
        self.check_engine_failure();
        self.script_budget.charge(started.elapsed());
        if let Some(error) = self.native_memory_error() {
            return Err(error);
        }
        if let Some(reason) = self.script_budget.crash_reason() {
            return Err(Error::Crashed { reason });
        }

        let checkpoint_budget = remaining.saturating_sub(started.elapsed());
        if checkpoint_budget.is_zero() {
            return Err(Error::timeout("evaluate", remaining.as_millis() as u64));
        }
        let checkpoint_limit = if self.script_timeout.is_zero() {
            checkpoint_budget
        } else {
            self.script_timeout.min(checkpoint_budget)
        };
        let checkpoint_started = Instant::now();
        let arm = self.watchdog.as_ref().map(|handle| {
            crate::watchdog::arm(Arc::clone(handle), checkpoint_started + checkpoint_limit)
        });
        self.engine.run_microtasks();
        let checkpoint_elapsed = checkpoint_started.elapsed();
        let checkpoint_fired = arm.as_ref().is_some_and(crate::watchdog::Arm::fired);
        drop(arm);
        self.check_engine_failure();
        self.script_budget.charge(checkpoint_elapsed);
        if let Some(error) = self.native_memory_error() {
            return Err(error);
        }
        if let Some(reason) = self.script_budget.crash_reason() {
            return Err(Error::Crashed { reason });
        }
        if checkpoint_fired
            || checkpoint_elapsed >= checkpoint_limit
            || started.elapsed() >= remaining
        {
            return Err(Error::timeout("evaluate", remaining.as_millis() as u64));
        }

        result.map_err(|error| match error.kind {
            tenta_js::engine::ScriptErrorKind::Terminated if error.message.contains("deadline") => {
                Error::timeout("evaluate", timeout.as_millis() as u64)
            }
            _ => Error::Script {
                message: error.message,
                stack: None,
            },
        })
    }
}

/// How a script element asks to be timed.
///
/// `defer` and `async` are read together because they are decided together:
/// HTML ignores both on an inline script, and gives `async` precedence when an
/// external one carries both.
#[derive(Clone, Copy)]
struct ScriptTiming {
    asynchronous: bool,
    deferred: bool,
}

/// What running the script the parse is paused on needs next.
enum ScriptStep {
    /// Nothing to fetch: the script ran, or a rule skipped it. The parse may
    /// resume.
    Done,
    /// The script is classic and external with no `async` or `defer`: it is
    /// fetched now, and the parse resumes only once it has run.
    Fetch(Url),
    /// The script is classic and external with `async` or `defer`: its fetch
    /// runs in parallel with the parse, so the caller starts it and the parse
    /// resumes at once.
    Deferred(DeferredTiming),
}

/// When an external classic script with `async` or `defer` runs.
///
/// The two are different facts, and the difference is what each one waits for:
/// an `async` script waits for nothing but its own fetch and runs the moment
/// it arrives, while a `defer` script waits for the document to be parsed and
/// for every earlier deferred script to have run.
pub(crate) enum DeferredTiming {
    /// `async`: fetched in parallel with the parse, run as soon as it arrives,
    /// in whatever order arrivals happen.
    Async(Url),
    /// `defer`: fetched in parallel with the parse, run in document order once
    /// the document is parsed and before `DOMContentLoaded`.
    Defer(Url),
}

/// What the parse knows about its document when it hands a script out
/// (the HTML standard's "fetch a classic script").
///
/// The two URLs are different facts and are needed for different reasons: the
/// base is what a relative `src` resolves against, while the document's own URL
/// is what the request is first-party to — a script served from a third-party
/// origin is still a first-party request of this page. Both are owned rather
/// than borrowed because a `defer`/`async` script is fetched by a task that
/// outlives the parse pause which discovered it.
pub(crate) struct ScriptMeta {
    /// The document's base URL: its `<base href>` resolved against its URL, or
    /// its URL when it declares none.
    pub base: Url,
    /// The document's own URL.
    pub document_url: Url,
    /// The navigation's deadline. A script may not outlive it: the deadline is
    /// the whole navigation's, and the parse is part of the navigation.
    pub deadline: Instant,
}

/// The engine configuration a page derives from its own
/// (`crates/tenta-js/src/engine.rs`).
fn engine_config(config: &PageConfig, native_memory: NativeMemoryBudget) -> PageEngineConfig {
    PageEngineConfig {
        timer_rule: config.settle.timer_rule(),
        // A live page follows wall time.
        clock: EngineClock::Real,
        max_heap_bytes: Some(config.budgets.js_heap_mb as usize * 1024 * 1024),
        script_timeout: Some(entry_timeout(&config.budgets)),
        locale: config.locale.clone(),
        timezone: None,
        cookies: config.cookies.clone(),
        storage: config.storage.clone(),
        native_memory,
        viewport: viewport_metrics(&config.viewport),
    }
}

/// The deadline one engine entry gets: `evaluate_time`, but never more than half
/// of the script CPU the page may spend per window (D-033). An entry terminated
/// at its deadline is charged its whole length, so a deadline at or above the
/// window budget would let one terminated runaway crash the page alone.
fn entry_timeout(budgets: &tenta_core::Budgets) -> Duration {
    if budgets.script_cpu.is_zero() {
        return budgets.evaluate_time;
    }
    budgets.evaluate_time.min(budgets.script_cpu / 2)
}

/// The window metrics script sees before a layout answers: the page's viewport,
/// unscrolled, with the outer window the size of the inner one.
fn viewport_metrics(viewport: &Viewport) -> ViewportMetrics {
    ViewportMetrics {
        inner_width: f64::from(viewport.width),
        inner_height: f64::from(viewport.height),
        outer_width: f64::from(viewport.width),
        outer_height: f64::from(viewport.height),
        device_pixel_ratio: f64::from(viewport.device_scale_factor),
        scroll_x: 0.0,
        scroll_y: 0.0,
    }
}

/// Whether a `type` attribute names a classic script this driver can run.
///
/// An absent or empty type is JavaScript by default; the JavaScript MIME types
/// are the ones the preload scanner accepts. `module` is deliberately not here:
/// a module has its own fetch and evaluation rules and lands in P2 d-2, and
/// running one as classic would run it with the wrong scope.
fn is_classic_javascript(kind: &str) -> bool {
    let kind = kind.trim();
    kind.is_empty()
        || kind.eq_ignore_ascii_case("text/javascript")
        || kind.eq_ignore_ascii_case("application/javascript")
        || kind.eq_ignore_ascii_case("application/ecmascript")
        || kind.eq_ignore_ascii_case("text/ecmascript")
}

/// The runtime of `page`, whatever tree it is bound to.
///
/// For the page's own entry points, which act on the engine the page keeps
/// after a navigation. Work that belongs to one navigation names its tree
/// through [`with_runtime_for`] instead.
pub(crate) fn with_runtime<R>(page: PageId, f: impl FnOnce(&mut PageRuntime) -> R) -> Option<R> {
    RUNTIMES.with(|cell| cell.borrow_mut().get_mut(&page).map(f))
}

/// The runtime of `page`, if it has one *and* it is still bound to `document`.
///
/// Every use of a runtime names the tree its navigation parsed, because two
/// navigations of one page can be in flight at once — the page thread
/// interleaves them at awaits, and a script fetch is one of those awaits — so
/// acting on the runtime of a navigation without naming its tree would take
/// the other navigation's engine with it.
pub(crate) fn with_runtime_for<R>(
    page: PageId,
    document: &SharedDocument,
    f: impl FnOnce(&mut PageRuntime) -> R,
) -> Option<R> {
    RUNTIMES.with(|cell| {
        let mut runtimes = cell.borrow_mut();
        let runtime = runtimes.get_mut(&page)?;
        if !Rc::ptr_eq(&runtime.document, document) {
            return None;
        }
        Some(f(runtime))
    })
}

/// Removes the runtime of `page`, whoever built it.
pub(crate) fn take_runtime(page: PageId) -> Option<PageRuntime> {
    RUNTIMES.with(|cell| cell.borrow_mut().remove(&page))
}

/// Removes the runtime of `page` if it is still the one bound to `document`.
pub(crate) fn take_runtime_for(page: PageId, document: &SharedDocument) -> Option<PageRuntime> {
    RUNTIMES.with(|cell| {
        let mut runtimes = cell.borrow_mut();
        let ours = runtimes
            .get(&page)
            .is_some_and(|runtime| Rc::ptr_eq(&runtime.document, document));
        if ours { runtimes.remove(&page) } else { None }
    })
}

/// Builds the runtime of `page` over `document` if it has none yet, pointing
/// it at the settle call `tracker` is collecting for.
///
/// Called at every script pause: the first one builds the engine, and the rest
/// find it already bound to the same tree. A runtime bound to a *different*
/// tree can only mean the parse restarted after an engine was built, which the
/// parser forbids — it makes the encoding certain as soon as it hands a script
/// out, so a restart can only happen before the first script. If it happens
/// anyway the engine is rebuilt, because the tree it holds is no longer the one
/// being parsed.
pub(crate) fn ensure_runtime(
    page: PageId,
    document: SharedDocument,
    config: &PageConfig,
    tracker: Rc<RefCell<SettleTracker>>,
    script_budget: Arc<dyn ScriptBudget>,
) -> std::result::Result<(), String> {
    let existing = RUNTIMES.with(|cell| {
        cell.borrow()
            .get(&page)
            .map(|runtime| Rc::ptr_eq(&runtime.document, &document))
    });
    match existing {
        Some(true) => return Ok(()),
        Some(false) => {
            debug_assert!(
                false,
                "the parser restarted a tree that an engine was already bound to"
            );
            tracing::error!(
                ?page,
                "the document changed under a running engine; the engine is rebuilt"
            );
            drop(take_runtime(page));
        }
        None => {}
    }
    match PageRuntime::create(page, document, config, tracker, script_budget) {
        Ok(runtime) => {
            RUNTIMES.with(|cell| {
                cell.borrow_mut().insert(page, runtime);
            });
            Ok(())
        }
        Err(reason) => {
            tracing::error!(
                ?page,
                %reason,
                "the JavaScript engine could not be created"
            );
            Err(reason)
        }
    }
}

/// Runs the script the parse of `page` is paused on, creating the runtime on
/// the first pause.
///
/// Runtime creation failures propagate to the navigation. A configured engine
/// that cannot start must not turn a JavaScript page into a successful parse
/// with its scripts silently skipped.
///
/// A classic script with a `src` is fetched here, between the two visits to the
/// runtime, because it is the fetch that awaits and the runtime is
/// page-thread-local state another navigation of this page reaches at its own
/// awaits. Nothing may borrow it across one, so the script is run on the far
/// side of the fetch and only if the same engine is still bound to the same
/// tree.
///
/// `Some` means the script is not this call's to run at all: it carries `async`
/// or `defer`, so its fetch runs in parallel with the parse and the caller
/// starts it (`crate::page`). This is a classification rather than a spawn
/// because starting one needs the page's `PageInner`, which belongs to that
/// module.
pub(crate) async fn run_pending_script(
    parser: &mut Parser<'_, SharedSink>,
    document: &SharedDocument,
    config: &PageConfig,
    tracker: &Rc<RefCell<SettleTracker>>,
    meta: &ScriptMeta,
    script_budget: Arc<dyn ScriptBudget>,
) -> Result<Option<DeferredTiming>, Error> {
    let page = config.id;
    ensure_runtime(
        page,
        document.clone(),
        config,
        Rc::clone(tracker),
        Arc::clone(&script_budget),
    )
    .map_err(Error::unsupported)?;
    let step = with_runtime_for(page, document, |runtime| {
        runtime.run_pending_script(parser, &meta.base)
    })
    .ok_or_else(|| Error::unsupported("the JavaScript runtime was unavailable"))?;
    let url = match step? {
        ScriptStep::Done => return Ok(None),
        ScriptStep::Deferred(timing) => return Ok(Some(timing)),
        ScriptStep::Fetch(url) => url,
    };
    let Some(body) = fetch_script(config, meta, &url, tracker).await else {
        // A script that could not be fetched is a page-level fact, not a failed
        // navigation: the parse goes on without it, exactly as a browser's does.
        return Ok(None);
    };
    let Some(source) = decode_script(&body, parser.encoding()) else {
        tracing::warn!(
            ?page,
            url = %url,
            "an external script is skipped: the parse has no document encoding to decode it with"
        );
        return Ok(None);
    };
    let ran = with_runtime_for(page, document, |runtime| runtime.run_source(&source));
    if ran.is_none() {
        tracing::debug!(
            ?page,
            url = %url,
            "a fetched script did not run: its engine is no longer bound to this tree"
        );
    }
    Ok(None)
}

/// Issues the fetch of one external classic script and reads its body.
///
/// Every timing of external script goes through here, parser-blocking and
/// parallel alike, so a deferred script's provenance is exactly a blocking
/// one's. The request is registered as a *parser* request rather than an
/// ordinary tracked one: a script the parser inserted is exempt from
/// reclassification, so a slow origin cannot make this fetch background
/// mid-settle and end a settle the load chain is still waiting on
/// (`docs/spec/02-architecture.md` §5.1, and this is
/// `note_parser_request_started`'s first production caller).
///
/// `None` means the script has no text: the fetch failed, the origin answered
/// with a status that is not ok, or the navigation's deadline left no time to
/// issue the request at all. Either way the caller runs nothing and the parse
/// goes on.
pub(crate) async fn fetch_script(
    config: &PageConfig,
    meta: &ScriptMeta,
    url: &Url,
    tracker: &Rc<RefCell<SettleTracker>>,
) -> Option<Bytes> {
    // A fetch that cannot finish inside the navigation's deadline is not issued
    // at all: the parse would be sitting in it when the deadline expired, and
    // the body loop is what turns an expired deadline into the navigation's
    // outcome.
    let remaining = meta.deadline.saturating_duration_since(config.clock.now());
    if remaining.is_zero() {
        tracing::warn!(
            url = %url,
            "an external script is skipped: the navigation deadline has passed"
        );
        return None;
    }
    let request_id = next_request_id();
    let request = match script_request(config, meta, url, remaining) {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(
                page = ?config.id,
                url = %url,
                ?error,
                "an external script is skipped: one of the page's headers cannot go on the wire"
            );
            return None;
        }
    };
    tracker
        .borrow_mut()
        .note_parser_request_started(request_id, RequestClass::Script);
    // One place ends the request, whatever the fetch did. A request left in the
    // settle machine's table would hold the settle open for a fetch that is
    // over — the failure mode this fetch is most likely to cause.
    let fetched = read_script_body(&config.client, request).await;
    tracker.borrow_mut().note_request_done(request_id);
    match fetched {
        Ok(Some(body)) => {
            tracing::debug!(
                page = ?config.id,
                url = %url,
                bytes = body.len(),
                "an external script was fetched"
            );
            Some(body)
        }
        // The origin answered, so there is nothing to report beyond the status
        // the read already logged, and no text to run.
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(
                page = ?config.id,
                url = %url,
                ?error,
                "an external script could not be fetched; the parse goes on without it"
            );
            None
        }
    }
}

/// The request one external classic script's fetch issues.
///
/// The page's own headers go on it exactly as they go on the document request
/// (`crate::page::build_request`): a `User-Agent` override that reached the
/// document but not the scripts it loads would be a strange half of a profile.
/// One that cannot go on the wire is an error, as it is there.
fn script_request(
    config: &PageConfig,
    meta: &ScriptMeta,
    url: &Url,
    remaining: Duration,
) -> std::result::Result<Request, NetError> {
    let mut request = Request::new(http::Method::GET, url.clone())
        .resource_type(ResourceType::Script)
        // The initiator decides `Sec-Fetch-Site`, `Origin` and the default
        // `Referer`, all of which are the document's rather than the script's.
        .initiator(meta.document_url.clone())
        // No grace behind a page timer here: this deadline *is* the bound.
        .timeout(remaining)
        .max_body_bytes(config.budgets.network_bytes);
    // What the request is first-party to is the document it is for, not the
    // script's own origin: a script served from a third-party origin is still
    // a first-party request of this page.
    request.first_party = Site::from_url(&meta.document_url);
    for (name, value) in &config.extra_headers {
        request = request.header(name, value)?;
    }
    Ok(request)
}

/// Sends `request` and reads its body to the end.
///
/// `None` means the origin answered but with a status that is not ok, which is
/// a failed script fetch rather than a script: HTML's "fetch a classic script"
/// says "if bodyBytes is null or failure, or response's status is not an ok
/// status, then run onComplete given null" — the body of a 404 is an error
/// page, not the script the element named, and running it would execute
/// whatever that page happened to contain.
async fn read_script_body(
    client: &Client,
    request: Request,
) -> std::result::Result<Option<Bytes>, NetError> {
    let response = client.send(request).await?;
    tracing::debug!(
        status = response.status.as_u16(),
        "a script response head arrived"
    );
    if !response.status.is_success() {
        tracing::warn!(
            status = response.status.as_u16(),
            "an external script was not served; the parse goes on without it"
        );
        return Ok(None);
    }
    response.body.collect().await.map(Some)
}

/// Decodes the text of an external classic script.
///
/// A classic script has no encoding of its own: it is decoded with the
/// document's (`docs/spec/03-network.md`). The parser's is that encoding — it
/// is the one the document's own bytes were decoded with — and it is certain by
/// the time a script is handed out, since handing one out is what makes it
/// certain. `None` means the parser has no encoding yet, which cannot happen at
/// a pause; the script is skipped rather than decoded on a guess.
///
/// The encoding is passed in rather than read from a parser because a
/// `defer`/`async` script outlives the pause that discovered it: its fetch runs
/// as a task, and the encoding the pause made certain is captured there.
///
/// HTML's "fetch a classic script" decodes with that encoding as the *fallback*
/// — "The decode algorithm overrides encoding if the file contains a BOM" — so
/// the BOM-aware entry point is the one that matches it.
pub(crate) fn decode_script(body: &[u8], encoding: Option<&'static Encoding>) -> Option<String> {
    let encoding = encoding?;
    let (text, _, had_errors) = encoding.decode(body);
    if had_errors {
        tracing::debug!("an external script had bytes that are invalid in the document's encoding");
    }
    Some(text.into_owned())
}

/// The parse of `page` over `document` has ended: no parser writes into that
/// tree any more.
pub(crate) fn note_parse_finished(page: PageId, document: &SharedDocument) {
    with_runtime_for(page, document, |runtime| runtime.parse_in_flight = false);
}

/// Binds a fresh runtime to a document that has already loaded, for a page
/// that had no script of its own and is asked to run some.
///
/// The settle machine it gets starts after `load`: the document finished
/// loading without an engine, so nothing is outstanding and no lifecycle event
/// is owed.
pub(crate) fn adopt_loaded_document(
    page: PageId,
    document: SharedDocument,
    config: &PageConfig,
    script_budget: Arc<dyn ScriptBudget>,
) -> std::result::Result<(), String> {
    let mut tracker = SettleTracker::new(config.settle, Settle::Load, Arc::clone(&config.clock));
    tracker.note_domcontentloaded();
    tracker.note_load_fired();
    ensure_runtime(
        page,
        document.clone(),
        config,
        Rc::new(RefCell::new(tracker)),
        script_budget,
    )?;
    note_parse_finished(page, &document);
    Ok(())
}

/// Drops every runtime this page thread still holds.
///
/// Called as the thread winds down, while the process can still tear an engine
/// down: a runtime left to thread-local destruction would be dropped during
/// thread exit instead, after the tasks that used it are gone.
pub fn drop_thread_runtimes() {
    // A session never outlives the document it renders, which the runtimes
    // below may still own.
    #[cfg(feature = "render")]
    crate::render::release_all();
    let runtimes: Vec<(PageId, PageRuntime)> =
        RUNTIMES.with(|cell| cell.borrow_mut().drain().collect());
    // One at a time, so a runtime whose teardown panics does not leave the rest
    // to thread-local destruction.
    for (page, runtime) in runtimes {
        if let Err(reason) = crate::task::catch_unwind_task(|| drop(runtime)) {
            tracing::error!(
                ?page,
                reason,
                "a page runtime panicked while it was dropped"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tenta_core::TimerRule;
    use tenta_dom::Document;
    use tenta_js::engine::{EngineWatchdog, LifecycleEvent, ScriptError, ScriptKind};
    use tenta_js::fetch::{FetchCompletion, FetchDriver, FetchOutcome};
    use tenta_js::xhr::{XhrCompletion, XhrDriver, XhrHandle};

    #[test]
    fn an_await_table_delivers_once_and_closes_its_waiters_with_the_runtime() {
        let table = AwaitTable::default();
        let (settled_tx, mut settled_rx) = oneshot::channel();
        let (cancelled_tx, mut cancelled_rx) = oneshot::channel();
        let (orphan_tx, mut orphan_rx) = oneshot::channel();
        table.register(1, settled_tx);
        table.register(2, cancelled_tx);
        table.register(3, orphan_tx);

        table.settle(1, Ok(Completion::Value(inspect::RemoteValue::Bool(true))));
        assert_eq!(
            settled_rx.try_recv().expect("delivered"),
            Ok(Completion::Value(inspect::RemoteValue::Bool(true)))
        );
        // A second settlement for the same ticket finds nobody.
        table.settle(1, Ok(Completion::Value(inspect::RemoteValue::Null)));

        table.cancel(2);
        assert!(matches!(
            cancelled_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));

        // Dropping the table is what a navigation does to its waiters.
        assert!(matches!(
            orphan_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        drop(table);
        assert!(matches!(
            orphan_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }

    #[test]
    fn an_entry_deadline_never_alone_exhausts_the_window_budget() {
        let desktop = tenta_core::Budgets::desktop();
        assert_eq!(entry_timeout(&desktop), Duration::from_millis(7_500));
        let mobile = tenta_core::Budgets::mobile();
        assert_eq!(entry_timeout(&mobile), Duration::from_millis(3_750));
        let unbounded = tenta_core::Budgets::desktop().script_cpu(Duration::ZERO);
        assert_eq!(entry_timeout(&unbounded), unbounded.evaluate_time);
    }

    #[test]
    fn the_default_viewport_is_the_engine_default_metrics() {
        assert_eq!(
            viewport_metrics(&Viewport::default()),
            ViewportMetrics::default()
        );
    }

    /// An engine that records whether the page trimmed it and does nothing
    /// else: what the guard under test decides is whether the page enters the
    /// engine at all, not what a backend makes of the entry.
    struct RecordingEngine {
        document: SharedDocument,
        trims: Rc<Cell<usize>>,
        /// Advanced by a trim, which is what a forced collection does in a real
        /// backend — the property that makes reading the epoch *after* the trim
        /// different from reading it before.
        epoch: Rc<Cell<u64>>,
        /// Every batch of fetch completions the page handed over, in call order.
        /// One entry per `settle_fetches` call, so a test can tell "the page
        /// drained two completions in one entry" from "the page made two
        /// entries".
        settled: Rc<RefCell<Vec<Vec<FetchCompletion>>>>,
        /// The same for the XHR drain: one entry per `settle_xhr` call, so the
        /// drain's position and its one-entry-per-batch shape are observable.
        settled_xhr: Rc<RefCell<Vec<Vec<XhrCompletion>>>>,
        /// The engine entries of each turn, in call order, so the drain's
        /// position relative to the fetch drain and the timer pump is pinned
        /// rather than merely commented.
        entries: Rc<RefCell<Vec<&'static str>>>,
        /// The lifecycle events the page fired at this engine, in order — which
        /// is what makes "both events reach the engine through the bounded
        /// entry" observable rather than inferred from the engine's internals.
        lifecycle: Rc<RefCell<Vec<LifecycleEvent>>>,
        microtask_signal: Option<Arc<AtomicBool>>,
        frames_due: Rc<Cell<bool>>,
    }

    struct SignalWatchdog(Arc<AtomicBool>);

    impl EngineWatchdog for SignalWatchdog {
        fn terminate(&self) {
            self.0.store(true, Ordering::Release);
        }
    }

    impl PageEngine for RecordingEngine {
        fn document(&self) -> SharedDocument {
            self.document.clone()
        }

        fn run_script(&mut self, _source: &str, _kind: ScriptKind) -> Result<(), ScriptError> {
            Ok(())
        }

        fn evaluate(&mut self, source: &str, _timeout: Duration) -> Result<String, ScriptError> {
            Ok(source.to_owned())
        }

        fn set_timer_rule(&mut self, _rule: TimerRule) {}

        fn set_provenance_sink(&mut self, _sink: Rc<dyn ProvenanceSink>) {}

        fn set_fetch_driver(&mut self, _driver: Rc<dyn FetchDriver>) {}

        fn settle_fetches(&mut self, completions: Vec<FetchCompletion>) {
            self.entries.borrow_mut().push("fetch");
            self.settled.borrow_mut().push(completions);
        }

        fn set_xhr_driver(&mut self, _driver: Rc<dyn XhrDriver>) {}

        fn settle_xhr(&mut self, completions: Vec<XhrCompletion>) {
            self.entries.borrow_mut().push("xhr");
            self.settled_xhr.borrow_mut().push(completions);
        }

        fn dispatch_lifecycle(&mut self, event: LifecycleEvent) {
            self.entries.borrow_mut().push("lifecycle");
            self.lifecycle.borrow_mut().push(event);
        }

        fn next_wakeup(&self) -> Option<Duration> {
            None
        }

        fn pump_due_timers(&mut self, _max_tasks: usize) -> usize {
            self.entries.borrow_mut().push("timers");
            0
        }

        fn frame_due(&self) -> bool {
            self.frames_due.get()
        }

        fn run_animation_frame(&mut self) {
            self.entries.borrow_mut().push("frame");
        }

        fn run_microtasks(&mut self) {
            self.entries.borrow_mut().push("microtasks");
            if let Some(signal) = &self.microtask_signal {
                let fail_at = Instant::now() + Duration::from_secs(10);
                while !signal.load(Ordering::Acquire) {
                    assert!(
                        Instant::now() < fail_at,
                        "the test watchdog did not terminate the microtask checkpoint"
                    );
                    std::thread::yield_now();
                }
            }
        }

        fn trim_memory(&mut self) {
            self.trims.set(self.trims.get() + 1);
            self.epoch.set(self.epoch.get() + 1);
        }

        fn gc_epoch(&self) -> u64 {
            self.epoch.get()
        }

        fn watchdog(&self) -> Option<Box<dyn EngineWatchdog>> {
            self.microtask_signal.as_ref().map(|signal| {
                Box::new(SignalWatchdog(Arc::clone(signal))) as Box<dyn EngineWatchdog>
            })
        }
    }

    /// A budget that ignores what it is charged: the tests here drive `turn`
    /// for the reclaim guard and the fetch drain, not for the page's own script
    /// accounting.
    struct NullBudget;

    impl ScriptBudget for NullBudget {
        fn charge(&self, _elapsed: Duration) {}

        fn entry_terminated(&self, _entry: &'static str) {}

        fn crash_native_memory(&self, _failure: NativeMemoryFailure) {}

        fn crash_reason(&self) -> Option<String> {
            None
        }

        fn native_memory(&self) -> NativeMemoryBudget {
            NativeMemoryBudget::for_page(tenta_core::Budgets::platform_default().page_memory_mb)
        }
    }

    struct CrashingBudget(AtomicBool);

    impl ScriptBudget for CrashingBudget {
        fn charge(&self, _elapsed: Duration) {
            self.0.store(true, Ordering::Release);
        }

        fn entry_terminated(&self, _entry: &'static str) {}

        fn crash_native_memory(&self, _failure: NativeMemoryFailure) {}

        fn crash_reason(&self) -> Option<String> {
            self.0
                .load(Ordering::Acquire)
                .then(|| "script budget exhausted".to_owned())
        }

        fn native_memory(&self) -> NativeMemoryBudget {
            NativeMemoryBudget::for_page(tenta_core::Budgets::platform_default().page_memory_mb)
        }
    }

    /// A runtime over an empty document and a recording engine, plus the handles
    /// a test reads it through. The fields are this module's, so the test builds
    /// the state directly rather than driving a parse into it — which it has to,
    /// because the guard is unreachable from a page test (`turn` is called only
    /// from the settle loop, and the parse is always over by then).
    struct Harness {
        runtime: PageRuntime,
        trims: Rc<Cell<usize>>,
        epoch: Rc<Cell<u64>>,
        settled: Rc<RefCell<Vec<Vec<FetchCompletion>>>>,
        settled_xhr: Rc<RefCell<Vec<Vec<XhrCompletion>>>>,
        /// The runtime's own table, which is the handle a driver is given. A
        /// test pushes into it in the driver's place.
        fetches: Rc<FetchCompletions>,
        /// The runtime's XHR table, for the same reason.
        xhrs: Rc<XhrCompletions>,
        entries: Rc<RefCell<Vec<&'static str>>>,
        lifecycle: Rc<RefCell<Vec<LifecycleEvent>>>,
        frames_due: Rc<Cell<bool>>,
    }

    impl Harness {
        fn new(parse_in_flight: bool) -> Harness {
            Self::new_with_microtask_signal(parse_in_flight, None)
        }

        fn new_with_microtask_signal(
            parse_in_flight: bool,
            microtask_signal: Option<Arc<AtomicBool>>,
        ) -> Harness {
            let document: SharedDocument = Rc::new(RefCell::new(Document::new()));
            let trims = Rc::new(Cell::new(0));
            let epoch = Rc::new(Cell::new(0));
            let settled = Rc::new(RefCell::new(Vec::new()));
            let settled_xhr = Rc::new(RefCell::new(Vec::new()));
            let fetches = Rc::new(FetchCompletions::new());
            let xhrs = Rc::new(XhrCompletions::new());
            let entries = Rc::new(RefCell::new(Vec::new()));
            let lifecycle = Rc::new(RefCell::new(Vec::new()));
            let frames_due = Rc::new(Cell::new(false));
            let engine = RecordingEngine {
                document: Rc::clone(&document),
                trims: Rc::clone(&trims),
                epoch: Rc::clone(&epoch),
                settled: Rc::clone(&settled),
                settled_xhr: Rc::clone(&settled_xhr),
                entries: Rc::clone(&entries),
                lifecycle: Rc::clone(&lifecycle),
                microtask_signal: microtask_signal.clone(),
                frames_due: Rc::clone(&frames_due),
            };
            let runtime = PageRuntime {
                page: PageId(1),
                engine: Box::new(engine),
                document,
                parse_in_flight,
                tracker: Rc::new(RefCell::new(SettleTracker::new(
                    crate::settle::SettleConfig::library(),
                    Settle::Load,
                    Arc::new(crate::clock::SystemClock),
                ))),
                pump: None,
                last_gc_epoch: 0,
                watchdog: microtask_signal
                    .map(|signal| Arc::new(SignalWatchdog(signal)) as Arc<dyn EngineWatchdog>),
                script_timeout: Duration::from_secs(30),
                script_budget: Arc::new(NullBudget),
                native_memory: NativeMemoryBudget::for_page(1),
                fetches: Rc::clone(&fetches),
                xhrs: Rc::clone(&xhrs),
                awaits: Rc::default(),
            };
            Harness {
                runtime,
                trims,
                epoch,
                settled,
                settled_xhr,
                fetches,
                xhrs,
                entries,
                lifecycle,
                frames_due,
            }
        }

        /// Records a completion the way a driver would: pushes it, and nothing
        /// else. The wake is the driver's other half and has no meaning here,
        /// where the test drives `turn` itself.
        fn complete(&self, resolver: u64, message: &str) {
            self.fetches
                .push(resolver, FetchOutcome::Failed(message.to_owned()));
        }

        /// Records an XHR completion the way a driver would. `request` is the
        /// request id the object's `send()` stamped, which the drain carries
        /// through untouched and hands to the bindings' discard test.
        fn complete_xhr(&self, target: u64, request: u64, message: &str) {
            self.xhrs.push(
                XhrHandle { target, request },
                FetchOutcome::Failed(message.to_owned()),
            );
        }

        /// The resolvers of every completion the page has handed the engine, one
        /// inner list per entry.
        fn batches(&self) -> Vec<Vec<u64>> {
            self.settled
                .borrow()
                .iter()
                .map(|batch| batch.iter().map(|done| done.resolver).collect())
                .collect()
        }

        /// Both halves of every XHR completion the page has handed the engine,
        /// one inner list per entry — the id as well as the target, because the
        /// drain has to carry it for the bindings to discard an abandoned
        /// request's completion.
        fn xhr_batches(&self) -> Vec<Vec<(u64, u64)>> {
            self.settled_xhr
                .borrow()
                .iter()
                .map(|batch| {
                    batch
                        .iter()
                        .map(|done| (done.handle.target, done.handle.request))
                        .collect()
                })
                .collect()
        }
    }

    #[test]
    fn host_evaluation_keeps_the_navigation_deadline_when_entry_limit_is_disabled() {
        let mut harness = Harness::new(false);
        harness.runtime.script_timeout = Duration::ZERO;
        let value = harness
            .runtime
            .evaluate("answer", Duration::from_secs(1))
            .expect("navigation deadline bounds the evaluation");
        assert_eq!(value, "answer");
    }

    #[test]
    fn host_evaluation_reports_a_microtask_checkpoint_deadline() {
        let signal = Arc::new(AtomicBool::new(false));
        let mut harness = Harness::new_with_microtask_signal(false, Some(Arc::clone(&signal)));
        harness.runtime.script_timeout = Duration::from_millis(5);
        let error = harness
            .runtime
            .evaluate("answer", Duration::from_secs(1))
            .expect_err("the watchdog ends the stalled microtask checkpoint");
        assert!(matches!(error, Error::Timeout { .. }), "{error}");
        assert!(signal.load(Ordering::Acquire));
    }

    #[test]
    fn host_evaluation_does_not_run_microtasks_after_a_page_crash() {
        let mut harness = Harness::new(false);
        harness.runtime.script_budget = Arc::new(CrashingBudget(AtomicBool::new(false)));
        let error = harness
            .runtime
            .evaluate("answer", Duration::from_secs(1))
            .expect_err("the page crashed during the expression");
        assert!(matches!(error, Error::Crashed { .. }), "{error}");
        assert!(!harness.entries.borrow().contains(&"microtasks"));
    }

    #[test]
    fn a_script_budget_crash_skips_its_microtasks_and_later_lifecycle_entries() {
        let mut harness = Harness::new(false);
        harness.runtime.script_budget = Arc::new(CrashingBudget(AtomicBool::new(false)));
        harness.runtime.run_source("script");
        harness.runtime.dispatch_lifecycle(LifecycleEvent::Load);
        assert!(harness.entries.borrow().is_empty());
    }

    #[test]
    fn a_turn_stops_at_the_entry_that_exhausted_the_page_budget() {
        for first in ["fetch", "xhr", "timers"] {
            let mut harness = Harness::new(false);
            harness.runtime.script_budget = Arc::new(CrashingBudget(AtomicBool::new(false)));
            harness.frames_due.set(true);
            if first == "fetch" {
                harness.complete(1, "fetch complete");
            }
            if first != "timers" {
                harness.complete_xhr(1, 1, "xhr complete");
            }
            assert!(harness.runtime.turn(10).is_none());
            assert_eq!(*harness.entries.borrow(), vec![first]);
            assert!(harness.runtime.turn(10).is_none());
            assert_eq!(
                *harness.entries.borrow(),
                vec![first],
                "no post-crash entry"
            );
        }
    }

    /// The page-loop guard still defers sweeping while a parser
    /// holds arena handles into the tree. The epoch moving is the strongest
    /// possible trigger, and the guard must still keep the page out of the
    /// engine.
    #[test]
    fn a_turn_reclaims_nothing_while_a_parser_is_in_flight() {
        let mut harness = Harness::new(true);
        harness.epoch.set(1);

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(
            harness.trims.get(),
            0,
            "the engine is never entered with a parse in flight"
        );
        assert_eq!(harness.runtime.document.borrow().mutations_since_sweep(), 0);
    }

    /// Once the parse is over the same state reclaims, and the epoch it records
    /// is the one the trim left behind: a page that read the epoch before
    /// trimming would find it stale on the next turn and reclaim again, and
    /// again after that.
    #[test]
    fn a_turn_reclaims_once_the_parse_is_over_and_not_on_every_turn_after() {
        let mut harness = Harness::new(false);
        harness.epoch.set(1);

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(
            harness.trims.get(),
            1,
            "the epoch moved, so one reclaim was owed"
        );

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(
            harness.trims.get(),
            1,
            "the epoch was recorded after the trim, not before it"
        );
    }

    /// One drain is one engine entry, however many completions are waiting: the
    /// page pays one watchdog arm, one context entry and one stray-termination
    /// clear for the batch, not one each.
    #[test]
    fn a_turn_drains_the_fetch_table_into_one_batch() {
        let mut harness = Harness::new(true);
        harness.complete(7, "first");
        harness.complete(9, "second");

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(
            harness.batches(),
            vec![vec![7, 9]],
            "one entry, in push order"
        );
        assert!(
            harness.fetches.take_all().is_empty(),
            "the table is drained, not read"
        );

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(
            harness.batches(),
            vec![vec![7, 9]],
            "a turn with nothing waiting makes no entry"
        );
    }

    /// An empty table costs no engine entry at all: a page that never called
    /// `fetch` must not pay a watchdog arm and a context entry every turn for a
    /// drain that settles nothing.
    #[test]
    fn an_empty_fetch_table_costs_no_engine_entry() {
        let mut harness = Harness::new(true);

        assert!(harness.runtime.turn(1).is_none());
        assert!(
            harness.batches().is_empty(),
            "settle_fetches was not called"
        );
    }

    /// A completion pushed after a turn returns is not delivered by that turn:
    /// the drain is a boundary, and what a driver pushes once the entry has
    /// returned waits for the next one. This is why a driver has to *wake* the
    /// loop as well as push — the push alone leaves the promise pending until
    /// something else wakes the page.
    #[test]
    fn a_completion_pushed_after_a_turn_waits_for_the_next_one() {
        let mut harness = Harness::new(true);
        harness.complete(1, "before");

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(harness.batches(), vec![vec![1]]);

        harness.complete(2, "after");
        assert_eq!(
            harness.batches(),
            vec![vec![1]],
            "the turn that returned cannot see what arrived after it"
        );

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(harness.batches(), vec![vec![1], vec![2]]);
    }

    /// One XHR drain is one engine entry, however many completions are waiting:
    /// the page pays one watchdog arm and one native-dispatch guard for the
    /// batch, not one each.
    #[test]
    fn a_turn_drains_the_xhr_table_into_one_batch() {
        let mut harness = Harness::new(true);
        harness.complete_xhr(0x11, 1, "first");
        harness.complete_xhr(0x22, 2, "second");

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(
            harness.xhr_batches(),
            vec![vec![(0x11, 1), (0x22, 2)]],
            "one entry, in push order, with each request id carried alongside its target"
        );
        assert!(
            harness.xhrs.take_all().is_empty(),
            "the table is drained, not read"
        );

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(
            harness.xhr_batches(),
            vec![vec![(0x11, 1), (0x22, 2)]],
            "a turn with nothing waiting makes no entry"
        );
    }

    /// The XHR drain sits between the fetch drain and the timer pump — the
    /// position the microtask-debt argument requires — and is skipped entirely
    /// when its table is empty, so a page that made no XHR request pays no
    /// engine entry for one.
    #[test]
    fn an_empty_xhr_table_costs_no_engine_entry_and_the_drain_sits_before_the_timers() {
        let mut harness = Harness::new(true);
        harness.complete(7, "fetch");
        harness.complete_xhr(0x33, 3, "xhr");

        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(
            harness.entries.borrow().as_slice(),
            ["fetch", "xhr", "timers"],
            "the XHR drain is the task its completion belongs to, before the \
             checkpoint the timer pump runs"
        );

        harness.entries.borrow_mut().clear();
        assert!(harness.runtime.turn(1).is_none());
        assert_eq!(
            harness.entries.borrow().as_slice(),
            ["timers"],
            "settle_xhr was not called for an empty table"
        );
    }

    /// The lifecycle entry belongs to the page's own moments, not to the turn:
    /// a turn fires no event, and each explicit call is exactly one engine
    /// entry — which is what makes the page's deadline arm around it
    /// (`PageRuntime::bounded`).
    ///
    /// Once-ness is deliberately *not* pinned here. This layer has none: it
    /// fires whatever it is asked to fire, and the settle loop and the parse are
    /// what decide the moment (`crate::page`). Two calls are therefore two
    /// entries, and a page that called this every turn would be the caller's
    /// bug, not this method's.
    #[test]
    fn a_lifecycle_dispatch_is_one_bounded_entry_and_no_turn_fires_one() {
        let mut harness = Harness::new(false);

        assert!(harness.runtime.turn(1).is_none());
        assert!(
            harness.lifecycle.borrow().is_empty(),
            "a turn fires no lifecycle event"
        );

        harness
            .runtime
            .dispatch_lifecycle(LifecycleEvent::DomContentLoaded);
        harness.runtime.dispatch_lifecycle(LifecycleEvent::Load);

        assert_eq!(
            harness.lifecycle.borrow().as_slice(),
            [LifecycleEvent::DomContentLoaded, LifecycleEvent::Load],
            "each call is one entry, in the order the page fired them"
        );
        assert_eq!(
            harness.entries.borrow().as_slice(),
            ["timers", "lifecycle", "lifecycle"],
            "the turn's own entries first, then one lifecycle entry per event"
        );
    }
}
