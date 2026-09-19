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
//! two places it is not (`document`, `run_script`) are the two that cash an
//! invariant of the page layer: a page engine is always built over a document,
//! and a script failure is classified by where it failed.
//!
//! The borrow and re-entrancy rules in `tenta_js::engine`'s module doc apply to
//! these calls as much as to any other engine entry point. In particular
//! [`PageEngine::trim_memory`] borrows the shared document to clear
//! `HAS_WRAPPER`, and a caller must not be holding a borrow of it at the time.

use std::rc::Rc;
use std::time::Duration;

use tenta_core::TimerRule;
use tenta_js::bindings::host::SharedDocument;
use tenta_js::engine::{
    EngineClock, EngineWatchdog, PageEngine, PageEngineConfig, PageEngineFactory, ScriptError,
    ScriptErrorKind, ScriptKind,
};
use tenta_js::event_loop::{Clock, MonoTime, ProvenanceSink};

use crate::{Engine, world};

impl PageEngine for Engine {
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
        self.run_classified(source, timeout)
            .map_err(|failure| ScriptError {
                kind: failure.kind,
                message: failure.message,
            })
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

    /// Forces a full collection and then applies the wrapper bookkeeping the
    /// collector just invalidated: the `HAS_WRAPPER` bit of every node wrapper
    /// whose finalizer ran, and the slab entries of collected snapshot and event
    /// wrappers.
    ///
    /// This is the first half of reclaiming detached nodes
    /// (`docs/spec/04-dom-and-css.md` §1.1): a node the engine no longer wraps
    /// still looks reachable to the DOM until the flag is cleared, so a sweep
    /// that runs before this keeps it. The drain is the same function the lazy
    /// sweep in `V8World::wrapper` uses, so the two cannot drift.
    fn trim_memory(&mut self) {
        // V8's embedder memory-pressure entry point, which collects
        // synchronously — the browser's replacement for an exposed `gc()`.
        self.isolate.low_memory_notification();
        if let Some(dom) = &self.dom {
            world::reclaim_dead_bookkeeping(&dom.state, &dom.ctx);
        }
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
        Some(Box::new(V8Watchdog(self.isolate.thread_safe_handle())))
    }
}

/// A [`PageEngine`]'s watchdog: the isolate's thread-safe handle, which
/// `terminate_execution` can reach from another thread while the page thread is
/// blocked inside a run (`docs/spec/05-javascript.md` §11).
///
/// `IsolateHandle` is `Send + Sync` and holds a lock over the isolate pointer,
/// so a terminate that races teardown sees a disposed isolate and does nothing
/// rather than dereferencing it.
struct V8Watchdog(v8::IsolateHandle);

impl EngineWatchdog for V8Watchdog {
    fn terminate(&self) {
        self.0.terminate_execution();
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
        let mut engine = Engine::with_document_on_clock(document, clock, config.max_heap_bytes);
        // The page derives the rule from its settle configuration, so the
        // engine and the settle machine cannot disagree about what defers a
        // settle.
        engine.set_timer_rule(config.timer_rule);
        engine.set_script_timeout(config.script_timeout);
        Ok(Box::new(engine))
    }
}
