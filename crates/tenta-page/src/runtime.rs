//! The page's JavaScript runtime: one engine bound to one parsed document
//! (`docs/spec/02-architecture.md` §3).
//!
//! A runtime exists only while a page with an engine drives a parse. It is
//! built lazily on the first `</script>` pause, runs that parse's script
//! elements against the very tree the parser is writing into, reports the
//! engine's task provenance to the settle machine, and is dropped when the
//! navigation that built it ends.
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
use tenta_core::{Error, Origin, PageId, Result};
use tenta_dom::{SharedDocument, SharedSink};
use tenta_html::{Encoding, Parser};
use tenta_js::engine::{
    EngineClock, EngineWatchdog, LifecycleEvent, PageEngine, PageEngineConfig, ScriptKind,
};
use tenta_js::event_loop::ProvenanceSink;
use tenta_js::fetch::{FetchCompletions, FetchDriver};
use tenta_js::xhr::{XhrCompletions, XhrDriver};
use tenta_net::policy::cookies::Site;
use tenta_net::{Client, NetError, Request, ResourceType};
use url::Url;

use crate::fetch::NetFetchDriver;
use crate::page::{PageConfig, next_request_id};
use crate::settle::{RequestClass, SettleTracker};

thread_local! {
    /// The runtimes of this page thread, by page.
    static RUNTIMES: RefCell<HashMap<PageId, PageRuntime>> = RefCell::new(HashMap::new());
}

/// Forwards the engine's task provenance into the settle tracker of the settle
/// call in progress (`docs/spec/02-architecture.md` §5.1).
///
/// The engine is handed this and reports every task it queues and finishes;
/// the tracker lives behind `Rc<RefCell<_>>` because the page mutates it too.
/// The handle is read out of the cell before the tracker is touched, so a
/// callback that re-enters this sink cannot find the cell still borrowed, and
/// neither direction holds a borrow across anything that can call back in.
#[derive(Default)]
pub(crate) struct SettleBridge {
    tracker: RefCell<Option<Rc<RefCell<SettleTracker>>>>,
}

impl SettleBridge {
    fn new() -> SettleBridge {
        SettleBridge::default()
    }

    /// Points the bridge at the tracker collecting for the settle call that is
    /// about to run.
    fn attach(&self, tracker: Rc<RefCell<SettleTracker>>) {
        *self.tracker.borrow_mut() = Some(tracker);
    }

    /// Points the bridge at nothing. The settle call it was collecting for has
    /// ended, so a task the engine reports afterwards must not reach it.
    fn detach(&self) {
        *self.tracker.borrow_mut() = None;
    }

    fn tracker(&self) -> Option<Rc<RefCell<SettleTracker>>> {
        self.tracker.borrow().clone()
    }
}

impl ProvenanceSink for SettleBridge {
    fn task_queued(&self, origin: Origin) {
        if let Some(tracker) = self.tracker() {
            tracker.borrow_mut().note_task_queued(origin);
        }
    }

    fn task_finished(&self, origin: Origin) {
        if let Some(tracker) = self.tracker() {
            tracker.borrow_mut().note_task_finished(origin);
        }
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
}

/// One engine and the document it is bound to, on the page thread.
pub(crate) struct PageRuntime {
    page: PageId,
    engine: Box<dyn PageEngine>,
    document: SharedDocument,
    /// Whether a `tenta_html::Parser` is still writing into `document`.
    ///
    /// The engine boundary's third rule forbids freeing nodes while a parser
    /// holds arena handles into the tree, and
    /// [`PageRuntime::run_pending_script`] is the one place the two
    /// interleave — it enters the engine with the parse paused on a script.
    /// So this is what [`PageRuntime::turn`] consults before reclaiming, and
    /// the assertion in `run_pending_script` keeps the two from disagreeing.
    parse_in_flight: bool,
    /// Kept so the bridge can be detached when the settle call ends; the
    /// engine also holds it, for as long as the engine lives.
    bridge: Rc<SettleBridge>,
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
}

impl PageRuntime {
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
        let engine_config = engine_config(config);
        let mut engine = factory.create(document.clone(), &engine_config)?;
        // The rule the engine tags timers with comes from the settle
        // configuration, so the two cannot disagree about what holds a settle
        // open (`docs/spec/02-architecture.md` §5.1).
        engine.set_timer_rule(engine_config.timer_rule);
        let bridge = Rc::new(SettleBridge::new());
        // A clone, not the handle itself: `attach` consumes its argument, and
        // the fetch driver below is registered against this same settle call —
        // it is the machine the requests it issues are counted in, and the
        // notification the settle loop parks on is the one it wakes.
        bridge.attach(Rc::clone(&tracker));
        engine.set_provenance_sink(Rc::clone(&bridge) as Rc<dyn ProvenanceSink>);
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
        let driver = Rc::new(NetFetchDriver::new(
            config,
            Rc::clone(&tracker),
            Rc::clone(&fetches),
            Rc::clone(&xhrs),
        ));
        engine.set_fetch_driver(Rc::clone(&driver) as Rc<dyn FetchDriver>);
        engine.set_xhr_driver(driver as Rc<dyn XhrDriver>);
        Ok(PageRuntime {
            page,
            engine,
            document,
            // A runtime is only ever built while a parse is paused on a
            // script, so a parser over `document` is in flight by definition.
            parse_in_flight: true,
            bridge,
            last_gc_epoch,
            watchdog,
            script_timeout,
            script_budget,
            fetches,
            xhrs,
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
    /// **This is where detached nodes are reclaimed, and it is the only place.**
    /// `turn` has one caller — the settle loop (`crate::page`) — so reclamation
    /// runs while a settle is being driven and not between navigations: a page
    /// whose settle ends, or one that never settles, keeps its detached
    /// subtrees until something drives a turn again. Reclamation is also a
    /// memory question rather than a correctness one (a freed node's listeners
    /// can no longer fire), so being late is safe; being *early* is not, and
    /// the `parse_in_flight` guard below is what keeps it from being.
    pub(crate) fn turn(&mut self, budget: usize) -> Option<Duration> {
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
        }
        // Both entries below run callback script the engine does not bound
        // itself, so each is armed around its call (`PageRuntime::bounded`).
        self.bounded("timers", |engine| engine.pump_due_timers(budget));
        if self.engine.frame_due() {
            self.bounded("animation-frame", |engine| engine.run_animation_frame());
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
        self.script_budget.charge(started.elapsed());
        if let Some(arm) = &arm
            && arm.fired()
        {
            tracing::warn!(
                page = ?self.page,
                entry,
                timeout = ?self.script_timeout,
                "an engine entry did not return within its deadline and was terminated"
            );
        }
        outcome
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
    /// sweep that ran first would keep it. `trim_memory` is also the only entry
    /// point that reaches the things a sweep invalidates but does not own — the
    /// freed nodes' listener entries and the stored callbacks in them live in
    /// the engine's world.
    fn reclaim_detached(&mut self) {
        self.engine.trim_memory();
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
    /// called, and nothing may free nodes while the parse is in flight. Every
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
    /// exactly when a deferred script runs. That a script may be run without a
    /// parser in flight is not a loosening of the boundary — the boundary
    /// forbids *freeing nodes* while a parser is in flight, and the reclaim
    /// guard is on `turn`, which this never reaches.
    pub(crate) fn run_source(&mut self, source: &str) {
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
        self.script_budget.charge(started.elapsed());
        // The script's own checkpoints are owed even when it threw, and the
        // checkpoint runs callback script the engine does not bound itself.
        self.bounded("microtasks", |engine| engine.run_microtasks());
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
fn engine_config(config: &PageConfig) -> PageEngineConfig {
    PageEngineConfig {
        timer_rule: config.settle.timer_rule(),
        // A live page follows wall time.
        clock: EngineClock::Real,
        max_heap_bytes: Some(config.budgets.js_heap_mb as usize * 1024 * 1024),
        script_timeout: Some(config.budgets.evaluate_time),
        locale: config.locale.clone(),
        timezone: None,
        cookies: config.cookies.clone(),
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
) {
    let existing = RUNTIMES.with(|cell| {
        cell.borrow()
            .get(&page)
            .map(|runtime| Rc::ptr_eq(&runtime.document, &document))
    });
    match existing {
        Some(true) => return,
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
        }
        // A failed engine is not a failed page: the parse goes on and the
        // scripts it pauses on are skipped, which is what a Tier 1 parse does
        // with every script.
        Err(reason) => {
            tracing::error!(
                ?page,
                %reason,
                "no JavaScript engine could be created; scripts are skipped"
            );
        }
    }
}

/// Runs the script the parse of `page` is paused on, creating the runtime on
/// the first pause.
///
/// Without a runtime there is nothing to run the script with, but the parse is
/// still paused on it: it is taken and dropped, because resuming without
/// taking it would abandon it silently.
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
    );
    let Some(step) = with_runtime_for(page, document, |runtime| {
        runtime.run_pending_script(parser, &meta.base)
    }) else {
        if parser.take_pending_script().is_some() {
            tracing::debug!(?page, "a script is skipped: no engine can run it");
        }
        return Ok(None);
    };
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

/// The settle call of `page` over `document` has ended: the engine's provenance
/// has no tracker to feed until a later navigation attaches one.
pub(crate) fn end_settle(page: PageId, document: Option<&SharedDocument>) {
    if let Some(document) = document {
        with_runtime_for(page, document, |runtime| runtime.bridge.detach());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use tenta_core::TimerRule;
    use tenta_dom::Document;
    use tenta_js::engine::{EngineWatchdog, LifecycleEvent, ScriptError, ScriptKind};
    use tenta_js::fetch::{FetchCompletion, FetchDriver, FetchOutcome};
    use tenta_js::xhr::{XhrCompletion, XhrDriver, XhrHandle};

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
    }

    impl PageEngine for RecordingEngine {
        fn document(&self) -> SharedDocument {
            self.document.clone()
        }

        fn run_script(&mut self, _source: &str, _kind: ScriptKind) -> Result<(), ScriptError> {
            Ok(())
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
            false
        }

        fn run_animation_frame(&mut self) {}

        fn run_microtasks(&mut self) {}

        fn trim_memory(&mut self) {
            self.trims.set(self.trims.get() + 1);
            self.epoch.set(self.epoch.get() + 1);
        }

        fn gc_epoch(&self) -> u64 {
            self.epoch.get()
        }

        fn watchdog(&self) -> Option<Box<dyn EngineWatchdog>> {
            None
        }
    }

    /// A budget that ignores what it is charged: the tests here drive `turn`
    /// for the reclaim guard and the fetch drain, not for the page's own script
    /// accounting.
    struct NullBudget;

    impl ScriptBudget for NullBudget {
        fn charge(&self, _elapsed: Duration) {}
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
    }

    impl Harness {
        fn new(parse_in_flight: bool) -> Harness {
            let document: SharedDocument = Rc::new(RefCell::new(Document::new()));
            let trims = Rc::new(Cell::new(0));
            let epoch = Rc::new(Cell::new(0));
            let settled = Rc::new(RefCell::new(Vec::new()));
            let settled_xhr = Rc::new(RefCell::new(Vec::new()));
            let fetches = Rc::new(FetchCompletions::new());
            let xhrs = Rc::new(XhrCompletions::new());
            let entries = Rc::new(RefCell::new(Vec::new()));
            let lifecycle = Rc::new(RefCell::new(Vec::new()));
            let engine = RecordingEngine {
                document: Rc::clone(&document),
                trims: Rc::clone(&trims),
                epoch: Rc::clone(&epoch),
                settled: Rc::clone(&settled),
                settled_xhr: Rc::clone(&settled_xhr),
                entries: Rc::clone(&entries),
                lifecycle: Rc::clone(&lifecycle),
            };
            let runtime = PageRuntime {
                page: PageId(1),
                engine: Box::new(engine),
                document,
                parse_in_flight,
                bridge: Rc::new(SettleBridge::new()),
                last_gc_epoch: 0,
                watchdog: None,
                script_timeout: Duration::from_secs(30),
                script_budget: Arc::new(NullBudget),
                fetches: Rc::clone(&fetches),
                xhrs: Rc::clone(&xhrs),
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

    /// The engine boundary's third rule: nothing may free nodes while a parser
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
