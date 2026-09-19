//! The page's JavaScript runtime: one engine bound to one parsed document
//! (`docs/spec/05-javascript.md` §6).
//!
//! A runtime exists only while a page with an engine drives a parse. It is
//! built lazily on the first `</script>` pause, runs that parse's script
//! elements against the very tree the parser is writing into, reports the
//! engine's task provenance to the settle machine, and is dropped when the
//! navigation that built it ends.
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
use tenta_js::engine::{EngineClock, EngineWatchdog, PageEngine, PageEngineConfig, ScriptKind};
use tenta_js::event_loop::ProvenanceSink;
use tenta_net::policy::cookies::Site;
use tenta_net::{Client, NetError, Request, ResourceType};
use url::Url;

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
}

impl PageRuntime {
    /// Builds an engine over `document` and points it at `tracker`.
    fn create(
        page: PageId,
        document: SharedDocument,
        config: &PageConfig,
        tracker: Rc<RefCell<SettleTracker>>,
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
        bridge.attach(tracker);
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
        // Both entries below run callback script the engine does not bound
        // itself, so each is armed around its call (`PageRuntime::bounded`).
        self.bounded("timers", |engine| engine.pump_due_timers(budget));
        if self.engine.frame_due() {
            self.bounded("animation-frame", |engine| engine.run_animation_frame());
        }
        self.engine.next_wakeup()
    }

    /// Runs one engine entry under the page's per-entry script deadline, when
    /// the engine offers a watchdog to enforce it.
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
    /// no time left. Deciding what that failure does is the page's script-CPU
    /// budget work, not this stage's.
    fn bounded<R>(&mut self, entry: &'static str, run: impl FnOnce(&mut dyn PageEngine) -> R) -> R {
        let Some(handle) = self.watchdog.clone() else {
            return run(&mut *self.engine);
        };
        if self.script_timeout.is_zero() {
            return run(&mut *self.engine);
        }
        let arm = crate::watchdog::arm(handle, Instant::now() + self.script_timeout);
        let outcome = run(&mut *self.engine);
        if arm.fired() {
            tracing::warn!(
                page = ?self.page,
                entry,
                timeout = ?self.script_timeout,
                "an engine entry did not return within its deadline and was terminated"
            );
        }
        outcome
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

    /// Runs the script the parse is paused on, against the tree as it stands.
    ///
    /// This is the one place the page enters an engine with its parser
    /// stopped, and the three borrow rules of the boundary apply
    /// (`tenta_js::engine`): the document borrow ends before the engine is
    /// called, and nothing may free nodes while the parse is in flight. Every
    /// skip is a real decision about the HTML, so each one is logged: a
    /// non-classic type is not this driver's to run, `defer`/`async`/
    /// `nomodule` change when a script runs (P2 d-2b), and an empty element
    /// has nothing to run.
    ///
    /// A classic script with a `src` is not skipped, but its text is not here
    /// either: it comes from the network, so the caller fetches what
    /// [`ScriptStep::Fetch`] names and runs it through
    /// [`PageRuntime::run_source`]. Splitting the two is what keeps no borrow
    /// of this runtime alive across the fetch that separates them.
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
        let (source, kind, src, deferred) = {
            let document = self.document.borrow();
            let source = document.text_content(script);
            let kind = document.attribute(script, atoms::TYPE).map(str::to_owned);
            let src = document.attribute(script, atoms::SRC).map(str::to_owned);
            let deferred = document.has_attribute(script, atoms::DEFER)
                || document.has_attribute(script, atoms::ASYNC)
                || document.has_attribute(script, atoms::NOMODULE);
            (source, kind, src, deferred)
        };
        if deferred {
            tracing::debug!(
                page = ?self.page,
                "a deferred, async or nomodule script does not run inline (P2 d-2b)"
            );
            return Ok(ScriptStep::Done);
        }
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
        if let Some(src) = &src {
            // The base is the document's, exactly as a recorded preload hint's
            // is resolved (`crate::fetch_policy`): the URL a relative `src`
            // resolves against, not the script's own.
            return match base.join(src) {
                Ok(url) => Ok(ScriptStep::Fetch(url)),
                Err(error) => {
                    tracing::warn!(
                        page = ?self.page,
                        src = %src,
                        %error,
                        "a script's src does not resolve against the document; it does not run"
                    );
                    Ok(ScriptStep::Done)
                }
            };
        }
        if source.trim().is_empty() {
            tracing::debug!(page = ?self.page, "an empty script element runs nothing");
            return Ok(ScriptStep::Done);
        }
        self.run_source(&source);
        Ok(ScriptStep::Done)
    }

    /// Runs `source` as a classic script against the tree, at the pause.
    ///
    /// A script error is a page-level fact, not an engine failure: the page
    /// keeps loading, exactly as a browser does. The script's own checkpoints
    /// are owed even when it threw.
    fn run_source(&mut self, source: &str) {
        debug_assert!(
            self.parse_in_flight,
            "a script ran while no parser was writing into its document"
        );
        if let Err(error) = self.engine.run_script(source, ScriptKind::Classic) {
            tracing::warn!(
                page = ?self.page,
                kind = ?error.kind,
                message = %error.message,
                "a script did not run to completion"
            );
        }
        // The script's own checkpoints are owed even when it threw, and the
        // checkpoint runs callback script the engine does not bound itself.
        self.bounded("microtasks", |engine| engine.run_microtasks());
    }
}

/// What running the script the parse is paused on needs next.
enum ScriptStep {
    /// Nothing to fetch: the script ran, or a rule skipped it. The parse may
    /// resume.
    Done,
    /// The script is classic and external, and its `src` resolved to this URL.
    /// It runs once the caller has fetched and decoded it.
    Fetch(Url),
}

/// What the parse knows about its document when it hands a script out
/// (`docs/spec/05-javascript.md` §6).
///
/// The two URLs are different facts and are needed for different reasons: the
/// base is what a relative `src` resolves against, while the document's own URL
/// is what the request is first-party to — a script served from a third-party
/// origin is still a first-party request of this page.
pub(crate) struct ScriptMeta<'a> {
    /// The document's base URL: its `<base href>` resolved against its URL, or
    /// its URL when it declares none.
    pub base: &'a Url,
    /// The document's own URL.
    pub document_url: &'a Url,
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
    match PageRuntime::create(page, document, config, tracker) {
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
pub(crate) async fn run_pending_script(
    parser: &mut Parser<'_, SharedSink>,
    document: &SharedDocument,
    config: &PageConfig,
    tracker: &Rc<RefCell<SettleTracker>>,
    meta: &ScriptMeta<'_>,
) -> Result<(), Error> {
    let page = config.id;
    ensure_runtime(page, document.clone(), config, Rc::clone(tracker));
    let Some(step) = with_runtime_for(page, document, |runtime| {
        runtime.run_pending_script(parser, meta.base)
    }) else {
        if parser.take_pending_script().is_some() {
            tracing::debug!(?page, "a script is skipped: no engine can run it");
        }
        return Ok(());
    };
    let ScriptStep::Fetch(url) = step? else {
        return Ok(());
    };
    let Some(body) = fetch_script(config, meta, &url, tracker).await else {
        // A script that could not be fetched is a page-level fact, not a failed
        // navigation: the parse goes on without it, exactly as a browser's does.
        return Ok(());
    };
    let Some(source) = decode_script(&body, parser.encoding()) else {
        tracing::warn!(
            ?page,
            url = %url,
            "an external script is skipped: the parse has no document encoding to decode it with"
        );
        return Ok(());
    };
    let ran = with_runtime_for(page, document, |runtime| runtime.run_source(&source));
    if ran.is_none() {
        tracing::debug!(
            ?page,
            url = %url,
            "a fetched script did not run: its engine is no longer bound to this tree"
        );
    }
    Ok(())
}

/// Issues the fetch of one parser-blocking external script and reads its body.
///
/// The request is registered as a *parser* request rather than an ordinary
/// tracked one: a script the parser inserted is exempt from reclassification,
/// so a slow origin cannot make this fetch background mid-settle and end a
/// settle the load chain is still waiting on (`docs/spec/02-architecture.md`
/// §5.1, and this is `note_parser_request_started`'s first production caller).
///
/// `None` means the script has no text: the fetch failed, the origin answered
/// with a status that is not ok, or the navigation's deadline left no time to
/// issue the request at all. Either way the caller resumes the parse.
async fn fetch_script(
    config: &PageConfig,
    meta: &ScriptMeta<'_>,
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

/// The request one parser-blocking script fetch issues.
///
/// The page's own headers go on it exactly as they go on the document request
/// (`crate::page::build_request`): a `User-Agent` override that reached the
/// document but not the scripts it loads would be a strange half of a profile.
/// One that cannot go on the wire is an error, as it is there.
fn script_request(
    config: &PageConfig,
    meta: &ScriptMeta<'_>,
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
    request.first_party = Site::from_url(meta.document_url);
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
/// HTML's "fetch a classic script" decodes with that encoding as the *fallback*
/// — "The decode algorithm overrides encoding if the file contains a BOM" — so
/// the BOM-aware entry point is the one that matches it.
fn decode_script(body: &[u8], encoding: Option<&'static Encoding>) -> Option<String> {
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
    use tenta_js::engine::{EngineWatchdog, ScriptError, ScriptKind};

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

        fn next_wakeup(&self) -> Option<Duration> {
            None
        }

        fn pump_due_timers(&mut self, _max_tasks: usize) -> usize {
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

    /// A runtime over an empty document and a recording engine, with
    /// `parse_in_flight` as the test sets it. The fields are this module's, so
    /// the test builds the state directly rather than driving a parse into it —
    /// which it has to, because the guard is unreachable from a page test
    /// (`turn` is called only from the settle loop, and the parse is always over
    /// by then).
    fn runtime(parse_in_flight: bool) -> (PageRuntime, Rc<Cell<usize>>, Rc<Cell<u64>>) {
        let document: SharedDocument = Rc::new(RefCell::new(Document::new()));
        let trims = Rc::new(Cell::new(0));
        let epoch = Rc::new(Cell::new(0));
        let engine = RecordingEngine {
            document: Rc::clone(&document),
            trims: Rc::clone(&trims),
            epoch: Rc::clone(&epoch),
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
        };
        (runtime, trims, epoch)
    }

    /// The engine boundary's third rule: nothing may free nodes while a parser
    /// holds arena handles into the tree. The epoch moving is the strongest
    /// possible trigger, and the guard must still keep the page out of the
    /// engine.
    #[test]
    fn a_turn_reclaims_nothing_while_a_parser_is_in_flight() {
        let (mut runtime, trims, epoch) = runtime(true);
        epoch.set(1);

        assert!(runtime.turn(1).is_none());
        assert_eq!(
            trims.get(),
            0,
            "the engine is never entered with a parse in flight"
        );
        assert_eq!(runtime.document.borrow().mutations_since_sweep(), 0);
    }

    /// Once the parse is over the same state reclaims, and the epoch it records
    /// is the one the trim left behind: a page that read the epoch before
    /// trimming would find it stale on the next turn and reclaim again, and
    /// again after that.
    #[test]
    fn a_turn_reclaims_once_the_parse_is_over_and_not_on_every_turn_after() {
        let (mut runtime, trims, epoch) = runtime(false);
        epoch.set(1);

        assert!(runtime.turn(1).is_none());
        assert_eq!(trims.get(), 1, "the epoch moved, so one reclaim was owed");

        assert!(runtime.turn(1).is_none());
        assert_eq!(
            trims.get(),
            1,
            "the epoch was recorded after the trim, not before it"
        );
    }
}
