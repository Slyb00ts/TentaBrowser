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
use std::time::Duration;

use hashbrown::HashMap;
use tenta_core::atom::atoms;
use tenta_core::{Error, Origin, PageId};
use tenta_dom::{SharedDocument, SharedSink};
use tenta_html::Parser;
use tenta_js::engine::{EngineClock, PageEngine, PageEngineConfig, ScriptKind};
use tenta_js::event_loop::ProvenanceSink;
use url::Url;

use crate::page::PageConfig;
use crate::settle::SettleTracker;

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

/// One engine and the document it is bound to, on the page thread.
pub(crate) struct PageRuntime {
    page: PageId,
    engine: Box<dyn PageEngine>,
    document: SharedDocument,
    /// Whether a `tenta_html::Parser` is still writing into `document`. A
    /// script only ever runs while one is, which is what keeps the engine from
    /// being entered against a tree the parser is holding arena handles into.
    parse_in_flight: bool,
    /// Kept so the bridge can be detached when the settle call ends; the
    /// engine also holds it, for as long as the engine lives.
    bridge: Rc<SettleBridge>,
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
        Ok(PageRuntime {
            page,
            engine,
            document,
            // A runtime is only ever built while a parse is paused on a
            // script, so a parser over `document` is in flight by definition.
            parse_in_flight: true,
            bridge,
        })
    }

    /// One turn of the page loop: run what the engine owes now, then say when
    /// it next has something to do.
    ///
    /// `Some(Duration::ZERO)` means work is due already and the caller must
    /// take another turn rather than park; `None` means the engine has nothing
    /// scheduled. Only remaining durations leave this method — the engine
    /// clock and the page clock are never compared.
    pub(crate) fn turn(&mut self, budget: usize) -> Option<Duration> {
        self.engine.pump_due_timers(budget);
        if self.engine.frame_due() {
            self.engine.run_animation_frame();
        }
        self.engine.next_wakeup()
    }

    /// Runs the script the parse is paused on, against the tree as it stands.
    ///
    /// This is the one place the page enters an engine with its parser
    /// stopped, and the three borrow rules of the boundary apply
    /// (`tenta_js::engine`): the document borrow ends before the engine is
    /// called, and nothing may free nodes while the parse is in flight. Every
    /// skip is a real decision about the HTML, so each one is logged: an
    /// external script is not fetched yet (P2 d-2), a non-classic type is not
    /// this driver's to run, `defer`/`async`/`nomodule` change when a script
    /// runs (P2 d-2), and an empty element has nothing to run.
    fn run_pending_script(
        &mut self,
        parser: &mut Parser<'_, SharedSink>,
        base: &Url,
    ) -> Result<(), Error> {
        let Some(script) = parser.take_pending_script() else {
            tracing::debug!(page = ?self.page, "the parse is not paused on a script");
            return Ok(());
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
        if let Some(src) = &src {
            tracing::debug!(
                page = ?self.page,
                src = %src,
                base = %base,
                "an external script is not fetched yet (P2 d-2); it does not run"
            );
            return Ok(());
        }
        if deferred {
            tracing::debug!(
                page = ?self.page,
                "a deferred, async or nomodule script does not run inline (P2 d-2)"
            );
            return Ok(());
        }
        if let Some(kind) = &kind
            && !is_classic_javascript(kind)
        {
            tracing::debug!(
                page = ?self.page,
                kind = %kind,
                "a non-classic script type is not run inline (P2 d-2)"
            );
            return Ok(());
        }
        if source.trim().is_empty() {
            tracing::debug!(page = ?self.page, "an empty script element runs nothing");
            return Ok(());
        }
        // A script error is a page-level fact, not an engine failure: the page
        // keeps loading, exactly as a browser does.
        if let Err(error) = self.engine.run_script(&source, ScriptKind::Classic) {
            tracing::warn!(
                page = ?self.page,
                kind = ?error.kind,
                message = %error.message,
                "a script did not run to completion"
            );
        }
        // The script's own checkpoints are owed even when it threw.
        self.engine.run_microtasks();
        Ok(())
    }
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

/// The runtime of `page`, if it has one.
pub(crate) fn with_runtime<R>(page: PageId, f: impl FnOnce(&mut PageRuntime) -> R) -> Option<R> {
    RUNTIMES.with(|cell| cell.borrow_mut().get_mut(&page).map(f))
}

/// The runtime of `page`, if it has one *and* it is still bound to `document`.
///
/// Two navigations of one page can be in flight at once — the page thread
/// interleaves them at awaits — so anything that acts on the runtime of a
/// navigation must name the tree that navigation parsed, or it would take the
/// other navigation's engine with it.
fn with_runtime_for<R>(
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
pub(crate) fn run_pending_script(
    parser: &mut Parser<'_, SharedSink>,
    document: &SharedDocument,
    config: &PageConfig,
    tracker: &Rc<RefCell<SettleTracker>>,
    base: &Url,
) -> Result<(), Error> {
    let page = config.id;
    ensure_runtime(page, document.clone(), config, Rc::clone(tracker));
    if let Some(result) = with_runtime(page, |runtime| runtime.run_pending_script(parser, base)) {
        return result;
    }
    if parser.take_pending_script().is_some() {
        tracing::debug!(?page, "a script is skipped: no engine can run it");
    }
    Ok(())
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
