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
//! 3. Nothing may free nodes in a document a `tenta_html::Parser` is currently
//!    parsing into: the parser holds arena handles it has not finished with,
//!    and a sweep through a shared handle could hand a recycled slot back to
//!    it. A caller that pauses a parse to run script is the one place the
//!    engine and the parser legitimately interleave, and it must resume the
//!    parse before anything reclaims.
//!
//! Unlike the rest of `tenta-js`, this module needs no engine feature: the
//! traits are engine-independent by construction, and `tenta-js` compiles
//! without the `v8` or `quickjs` feature.

use std::rc::Rc;
use std::time::Duration;

use tenta_core::TimerRule;

use crate::bindings::host::SharedDocument;
use crate::event_loop::ProvenanceSink;

/// How a script was delivered to the document.
///
/// Only classic scripts exist today. Module scripts need the module map and
/// the import graph (`docs/spec/05-javascript.md`), so the enum is here to
/// keep the call sites honest when they land rather than to be matched on now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ScriptKind {
    Classic,
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
}

/// One isolate driving one document.
///
/// Not `Send`: an engine is created, used and dropped on the page thread that
/// owns its document (`docs/spec/02-architecture.md` §3). The only part of it
/// that crosses threads is the watchdog an engine hands out, which is `Send`
/// for exactly that reason.
///
/// `document` and `run_script` are the two methods that touch page state;
/// everything else advances or interrogates the engine's own queues.
pub trait PageEngine {
    /// The document this engine is bound to — the same handle the parser
    /// writes into, so a paused parse and running script see one tree.
    fn document(&self) -> SharedDocument;

    /// Compiles and runs one script to completion, including its microtask
    /// checkpoint. A [`ScriptError`] is a page-level failure the caller
    /// reports and survives; it is not an engine failure.
    fn run_script(&mut self, source: &str, kind: ScriptKind) -> Result<(), ScriptError>;

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
