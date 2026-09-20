//! The harness the page-runtime tests drive a page with: a page thread that
//! runs its queues on the test's own task set, and a stub engine over a real
//! `tenta_js` event loop.
//!
//! Nothing here uses a browser context or the thread pool: a page is built by
//! hand from `PageConfig`, so a test observes the one thing it is about — the
//! parse, the scripts it pauses on, and the settle call that follows.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::StatusCode;
use tenta_core::atom::atoms;
use tenta_core::{Budgets, ContextId, Error, Origin, PageId, Result, Tier, TimerRule};
use tenta_dom::{Document as DomDocument, Mutation, SharedDocument};
use tenta_html::QualName;
use tenta_js::engine::{
    EngineWatchdog, PageEngine, PageEngineConfig, PageEngineFactory, ScriptError, ScriptKind,
};
use tenta_js::event_loop::{Clock, EventLoop, ProvenanceSink, Scheduler};
use tenta_js::fetch::{FetchCompletion, FetchDriver};
use tenta_net::hooks::{BoxFuture, FulfilledResponse, Hooks, Interception, Interceptor};
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::{ClientConfig, ErrorName, NetError, NetRuntime};
use tenta_page::page::{Goto, Navigation, Page, PageConfig, TestHooks, Viewport};
use tenta_page::settle::{Settle, SettleConfig};
use tenta_page::thread::{PageJob, PageTask, PageThread};

/// The URL every fixture is served at. A `.test` name can never resolve, so a
/// request that reached the network would fail loudly instead of hanging.
pub const URL: &str = "http://fixture.test/";

/// How long the driver waits for a navigation before calling it stalled.
const PATIENCE: Duration = Duration::from_secs(20);

/// How often the driver lets the test's task set run.
const TICK: Duration = Duration::from_millis(1);

// --------------------------------------------------------------- the thread

/// The page thread, driven by the test itself.
///
/// The real pool runs jobs on a local task set and tasks inline; this does the
/// same on the test's own thread, so a test can read everything the page did
/// without synchronising with anything (`crates/tenta-page/src/thread.rs`).
#[derive(Clone)]
pub struct TestThread {
    index: usize,
    queues: Arc<Queues>,
}

#[derive(Default)]
struct Queues {
    tasks: Mutex<VecDeque<PageTask>>,
    jobs: Mutex<VecDeque<PageJob>>,
}

impl TestThread {
    pub fn new() -> TestThread {
        TestThread {
            index: 0,
            queues: Arc::new(Queues::default()),
        }
    }

    /// Runs everything queued, including what running it queues in turn.
    fn drain(&self) {
        loop {
            let task = self.queues.tasks.lock().unwrap().pop_front();
            match task {
                Some(task) => task(),
                None => break,
            }
        }
        loop {
            let job = self.queues.jobs.lock().unwrap().pop_front();
            match job {
                // A job builds its future on the page thread, so it is spawned
                // on the test's local set rather than awaited here. Dropping
                // the handle detaches it; the local set drops it with the test.
                Some(job) => drop(tokio::task::spawn_local(job())),
                None => break,
            }
        }
    }
}

impl PageThread for TestThread {
    fn post_task(&self, task: PageTask) -> Result<()> {
        self.queues.tasks.lock().unwrap().push_back(task);
        Ok(())
    }

    fn post_job(&self, job: PageJob) -> Result<()> {
        self.queues.jobs.lock().unwrap().push_back(job);
        Ok(())
    }

    fn index(&self) -> usize {
        self.index
    }
}

/// Runs `operation` on this thread while driving the page's queues, the way
/// the pool would. Panics if the page stops making progress.
pub async fn drive<F: Future + 'static>(thread: &TestThread, operation: F) -> F::Output {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let handle = tokio::task::spawn_local(operation);
            let deadline = Instant::now() + PATIENCE;
            loop {
                thread.drain();
                if handle.is_finished() {
                    return handle.await.expect("the page future panicked");
                }
                assert!(
                    Instant::now() < deadline,
                    "the page thread stalled: no queued work ran and the operation never finished"
                );
                tokio::time::sleep(TICK).await;
            }
        })
        .await
}

// -------------------------------------------------------------- the record

/// A task provenance event the engine reported.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskEvent {
    Queued,
    Finished,
}

/// One script the engine was asked to run, and what it could see.
#[derive(Clone, Debug)]
pub struct ScriptRun {
    /// The script element's text, trimmed.
    pub source: String,
    pub kind: ScriptKind,
    /// Whether markup that follows the script was already in the tree when it
    /// ran, which is what "the script saw only what came before it" means.
    pub tail_parsed: bool,
}

/// What the document looked like at the moment the page asked the engine to
/// trim, which is the only vantage point from which "the engine is trimmed
/// *before* the sweep" is observable from a page test.
///
/// The stub engine can see nothing about the sweep itself — that happens after
/// `trim_memory` returns — but a trim that saw a nonzero mutation count which
/// the turn then left at zero is a trim that ran first
/// (`PageRuntime::reclaim_detached`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrimObservation {
    /// `Document::mutations_since_sweep` when the trim was called.
    pub mutations_since_sweep: u64,
    /// `Document::detached_root_count` when the trim was called.
    pub detached_roots: usize,
}

/// What the engine did, for the test to assert on.
#[derive(Debug, Default)]
pub struct Record {
    pub scripts: Vec<ScriptRun>,
    pub tasks: Vec<(TaskEvent, Origin)>,
    /// The `id` of every marker the command language appended to the tree.
    pub markers: Vec<String>,
    /// Times the `loop-forever` chain ran a hop.
    pub hops: u32,
    pub frames_requested: u32,
    pub frames_run: u32,
    /// Every `Engine::trim_memory` the page called, in order.
    pub trims: Vec<TrimObservation>,
    /// Every `EngineWatchdog::terminate` the page's deadline sent to the
    /// engine. Zero means no entry was ever cut short.
    pub terminations: usize,
    /// Whether a `block` command has been reached and is waiting to be
    /// terminated. What a test polls to sequence two pages against each other.
    pub blocked: bool,
}

pub type Recorder = Arc<Mutex<Record>>;

impl Record {
    /// The script texts the engine ran, in order.
    pub fn sources(&self) -> Vec<String> {
        self.scripts.iter().map(|run| run.source.clone()).collect()
    }

    pub fn has_marker(&self, id: &str) -> bool {
        self.markers.iter().any(|marker| marker == id)
    }

    /// Origins of the queued events, in order.
    pub fn queued_origins(&self) -> Vec<Origin> {
        self.tasks
            .iter()
            .filter(|(event, _)| *event == TaskEvent::Queued)
            .map(|(_, origin)| *origin)
            .collect()
    }

    /// How many units of work the engine reported pending with `origin`.
    pub fn queued_of(&self, origin: Origin) -> usize {
        self.count_of(TaskEvent::Queued, origin)
    }

    /// How many it reported finished with `origin`.
    pub fn finished_of(&self, origin: Origin) -> usize {
        self.count_of(TaskEvent::Finished, origin)
    }

    fn count_of(&self, event: TaskEvent, origin: Origin) -> usize {
        self.tasks
            .iter()
            .filter(|(seen, seen_origin)| *seen == event && *seen_origin == origin)
            .count()
    }
}

// -------------------------------------------------------------- the engine

/// The stub's termination latch: what `Isolate::TerminateExecution` is to a
/// real backend.
///
/// A callback that "never returns" is one that reaches the `block` command,
/// which waits here until a terminate arrives. The wait is a `Condvar`, never a
/// spin: a page whose deadline is not enforced fails the test's patience
/// instead of burning a core, which is what makes the hang observable rather
/// than a wedged suite.
///
/// The count is monotonic, not a one-shot flag. V8's termination request is per
/// entry, and after stage (a) each entry can be terminated again, so a second
/// `block` in the same page must still be observable: a latch would make a
/// second wait return instantly, on the first termination, and the test would
/// lie about when the second entry was cut short. `terminate` therefore
/// increments, and `wait` waits for a later increment than the one it saw.
#[derive(Default)]
struct Termination {
    releases: Mutex<usize>,
    released: Condvar,
}

impl Termination {
    /// Releases every waiter. A later `wait` still blocks until a *further*
    /// terminate, exactly as a fresh termination request does.
    fn terminate(&self) {
        let mut releases = self
            .releases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *releases += 1;
        self.released.notify_all();
    }

    /// Blocks the page thread until a terminate arrives after this call.
    fn wait(&self) {
        let mut releases = self
            .releases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let seen = *releases;
        while *releases <= seen {
            releases = self
                .released
                .wait(releases)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

/// The stub's watchdog: what the page arms a deadline through.
///
/// `terminate` is the only thing that releases a blocked callback, so a page
/// test that observes it is observing a real terminate rather than the stub
/// finishing on its own.
struct StubWatchdog {
    termination: Arc<Termination>,
    record: Recorder,
}

impl EngineWatchdog for StubWatchdog {
    fn terminate(&self) {
        self.record
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .terminations += 1;
        self.termination.terminate();
    }
}

/// Builds [`StubEngine`]s; the record is shared with the test, so the factory
/// is `Send + Sync` while the engines it builds are not.
pub struct StubFactory {
    record: Recorder,
    /// Whether the engines it builds offer a watchdog. A test that exercises
    /// the "no handle to arm" path leaves this off.
    watchdog: bool,
}

impl PageEngineFactory for StubFactory {
    fn create(
        &self,
        document: SharedDocument,
        _config: &PageEngineConfig,
    ) -> std::result::Result<Box<dyn PageEngine>, String> {
        Ok(Box::new(StubEngine::new(
            document,
            Arc::clone(&self.record),
            self.watchdog,
        )))
    }
}

/// What a script's callbacks hold: the loop's scheduling surface, the tree
/// they mutate, and the record the test reads.
///
/// A callback the loop still holds captures this, and this holds the scheduler
/// that registered it — the loop's callback table owns the scheduler that owns
/// the table. Without a way to release that handle, dropping the engine would
/// free neither the table, nor the callbacks in it, nor the document they hold.
/// No `Weak` handle is offered, so `Drop for StubEngine` clears the cell: that
/// is the one edge of the cycle the engine can break, and after it runs nothing
/// drives the loop again.
struct ScriptContext {
    scheduler: RefCell<Option<Scheduler>>,
    document: SharedDocument,
    record: Recorder,
    /// The engine's collection epoch, which the `gc` command advances and the
    /// page compares against the value it last saw.
    gc_epoch: Cell<u64>,
    /// What the `block` command waits on, and what the watchdog releases.
    termination: Arc<Termination>,
}

impl ScriptContext {
    fn scheduler(&self) -> Option<Scheduler> {
        self.scheduler.borrow().clone()
    }

    fn set_timeout(&self, delay: u64, run: Box<dyn FnOnce()>) {
        if let Some(scheduler) = self.scheduler() {
            scheduler.set_timeout(delay, run);
        }
    }

    fn set_interval(&self, delay: u64, run: Box<dyn FnMut()>) {
        if let Some(scheduler) = self.scheduler() {
            scheduler.set_interval(delay, run);
        }
    }

    fn request_animation_frame(&self, run: Box<dyn FnOnce(f64)>) {
        if let Some(scheduler) = self.scheduler() {
            scheduler.request_animation_frame(run);
        }
    }
}

/// An engine over a real event loop, interpreting a tiny command language.
///
/// The event loop is the production one; only the script interpreter is a
/// stub, so what the settle machine sees is the real timer and task
/// bookkeeping the engine reports.
pub struct StubEngine {
    page_loop: EventLoop,
    scheduler: Scheduler,
    context: Rc<ScriptContext>,
    /// Whether this engine hands out a watchdog handle. A V8 engine always
    /// does; a stub test picks, so both the arm and the no-handle path are
    /// reachable.
    watchdog: bool,
}

impl StubEngine {
    fn new(document: SharedDocument, record: Recorder, watchdog: bool) -> StubEngine {
        let page_loop = EventLoop::new(Clock::real());
        let scheduler = page_loop.scheduler();
        let context = Rc::new(ScriptContext {
            scheduler: RefCell::new(Some(scheduler.clone())),
            document,
            record,
            gc_epoch: Cell::new(0),
            termination: Arc::new(Termination::default()),
        });
        StubEngine {
            page_loop,
            scheduler,
            context,
            watchdog,
        }
    }
}

impl Drop for StubEngine {
    fn drop(&mut self) {
        // See `ScriptContext`: this is what lets the loop and the document a
        // callback holds be freed at all.
        let _ = self.context.scheduler.replace(None);
    }
}

impl PageEngine for StubEngine {
    fn document(&self) -> SharedDocument {
        self.context.document.clone()
    }

    fn run_script(
        &mut self,
        source: &str,
        kind: ScriptKind,
    ) -> std::result::Result<(), ScriptError> {
        self.context.record.lock().unwrap().scripts.push(ScriptRun {
            source: source.trim().to_owned(),
            kind,
            tail_parsed: has_selector(&self.context.document.borrow(), "p"),
        });
        // A script the parse handed out belongs to the document's load chain,
        // so what it defers is a deferral of the settle and what it starts on
        // its own is activity (`docs/spec/02-architecture.md` §5.1). The rule
        // itself is the engine's, installed by the page.
        let context = Rc::clone(&self.context);
        self.scheduler.with_origin(Origin::Tracked, || {
            for command in source.lines() {
                run_command(command, &context);
            }
        });
        Ok(())
    }

    fn set_timer_rule(&mut self, rule: TimerRule) {
        self.scheduler.set_timer_rule(rule);
    }

    fn set_provenance_sink(&mut self, sink: Rc<dyn ProvenanceSink>) {
        self.page_loop
            .task_sink()
            .set_provenance_sink(Rc::new(RecordingSink {
                record: Arc::clone(&self.context.record),
                inner: sink,
            }));
    }

    /// The stub page has no network: a driver installed here is accepted and
    /// never called, because these tests exercise the parse and the settle, not
    /// `fetch` (`d-4a-1b` is where a real driver arrives).
    fn set_fetch_driver(&mut self, _driver: Rc<dyn FetchDriver>) {}

    /// Nothing installs a driver, so nothing can complete a fetch and the page's
    /// table is always empty — a batch reaching here would mean the drain ran
    /// for a page that made no request.
    fn settle_fetches(&mut self, completions: Vec<FetchCompletion>) {
        assert!(
            completions.is_empty(),
            "the stub page never starts a fetch, so no completion can arrive"
        );
    }

    fn next_wakeup(&self) -> Option<Duration> {
        self.page_loop.next_wakeup()
    }

    fn pump_due_timers(&mut self, max_tasks: usize) -> usize {
        // The loop fires every timer due now and runs the tasks they queue, so
        // the bound is on how many times one turn may do that.
        let mut fired = 0;
        while fired < max_tasks {
            let now = self.page_loop.now();
            match self.page_loop.next_timer_deadline() {
                Some(deadline) if deadline <= now => {
                    self.page_loop.fire_due_timers();
                    fired += 1;
                }
                _ => break,
            }
        }
        fired
    }

    fn frame_due(&self) -> bool {
        self.page_loop.frame_due()
    }

    fn run_animation_frame(&mut self) {
        self.context.record.lock().unwrap().frames_run += 1;
        self.page_loop.run_animation_frame();
    }

    fn run_microtasks(&mut self) {
        // Stub: the loop exposes no standalone checkpoint — it drains the
        // microtask queue inside `fire_due_timers` and `run_animation_frame` —
        // and the command language queues no microtask of its own, so there is
        // nothing here to drain and nothing is silently dropped.
    }

    /// Records what the document looked like at the moment the page asked for a
    /// trim, which is what the ordering test asserts on. The stub has no
    /// collector to force, so it collects nothing and reclaims nothing: only
    /// the page's *sequence* of trim and sweep is real here.
    fn trim_memory(&mut self) {
        let document = self.context.document.borrow();
        self.context
            .record
            .lock()
            .unwrap()
            .trims
            .push(TrimObservation {
                mutations_since_sweep: document.mutations_since_sweep(),
                detached_roots: document.detached_root_count(),
            });
    }

    fn gc_epoch(&self) -> u64 {
        self.context.gc_epoch.get()
    }

    fn watchdog(&self) -> Option<Box<dyn EngineWatchdog>> {
        self.watchdog.then(|| {
            Box::new(StubWatchdog {
                termination: Arc::clone(&self.context.termination),
                record: Arc::clone(&self.context.record),
            }) as Box<dyn EngineWatchdog>
        })
    }
}

/// Records every provenance event, then forwards it to the page's bridge, so a
/// test can assert on what the engine reported and on what the settle machine
/// made of it.
struct RecordingSink {
    record: Recorder,
    inner: Rc<dyn ProvenanceSink>,
}

impl ProvenanceSink for RecordingSink {
    fn task_queued(&self, origin: Origin) {
        self.record
            .lock()
            .unwrap()
            .tasks
            .push((TaskEvent::Queued, origin));
        self.inner.task_queued(origin);
    }

    fn task_finished(&self, origin: Origin) {
        self.record
            .lock()
            .unwrap()
            .tasks
            .push((TaskEvent::Finished, origin));
        self.inner.task_finished(origin);
    }
}

// ------------------------------------------------------- the command language

/// Runs one command of the tiny language the fixtures are written in.
///
/// ```text
/// mutate <id>
/// detach <id>
/// gc
/// block
/// spin <ms>
/// setTimeout <command…> <ms>
/// setInterval <command…> <ms>
/// raf <command…>
/// raf-loop
/// loop-forever <label> <ms>
/// ```
///
/// Token separators are `(`, `)`, `,` and whitespace, so `setInterval(mutate
/// js,30)` and `setInterval mutate js 30` mean the same thing. A word that is
/// none of these commands is a script that runs and does nothing observable,
/// which is how the ordering tests label their scripts.
fn run_command(command: &str, context: &Rc<ScriptContext>) {
    let tokens: Vec<&str> = command
        .split(['(', ')', ',', ' ', '\t'])
        .filter(|token| !token.is_empty())
        .collect();
    let Some((name, rest)) = tokens.split_first() else {
        return;
    };
    match *name {
        "mutate" => {
            let id = rest.join(" ");
            append_marker(
                &context.document,
                &context.record,
                if id.is_empty() { "js" } else { &id },
            );
        }
        // Removes the element with that id, leaving its subtree detached: what
        // a framework does to a subtree it is finished with, and what the sweep
        // is for.
        "detach" => {
            let id = format!("#{}", rest.join(" "));
            let node = {
                let document = context.document.borrow();
                document
                    .query_selector(document.document_node(), &id)
                    .ok()
                    .flatten()
            };
            if let Some(node) = node {
                context
                    .document
                    .borrow_mut()
                    .mutate(Mutation::Remove { child: node })
                    .expect("detach the subtree");
            }
        }
        // Advances the engine's collection epoch, which is the page's second
        // "reclaim now" trigger and the one a test can pull directly. A real
        // engine advances it from a collector epilogue.
        "gc" => context.gc_epoch.set(context.gc_epoch.get() + 1),
        // A callback that does not return until it is terminated: the stand-in
        // for `while (true) {}`. It parks on the latch rather than spinning, so
        // a page whose deadline is not enforced stalls the test's patience
        // instead of the machine.
        "block" => {
            context
                .record
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .blocked = true;
            context.termination.wait();
        }
        // Busy-waits on the page thread for that many milliseconds and always
        // returns. It spends real time inside an engine entry *without* needing
        // the entry killed, which is what makes the page-lifetime script budget
        // test deterministic: the charge it produces is a fact of the command,
        // not of the watchdog thread's timing.
        "spin" => {
            let ms: u64 = rest
                .first()
                .and_then(|token| token.parse().ok())
                .unwrap_or(0);
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                std::hint::spin_loop();
            }
        }
        "setTimeout" | "setInterval" | "loop-forever" => {
            // The delay of a scheduled command is its last token; everything
            // before it is the command that runs.
            let (delay_token, inner) = match rest.split_last() {
                Some((delay, inner)) => (*delay, inner.join(" ")),
                None => ("0", String::new()),
            };
            let delay: u64 = delay_token.parse().unwrap_or(0);
            // `loop-forever` names itself, so the chain re-runs the same
            // command rather than the target it was given.
            let hop = *name == "loop-forever";
            let command = if hop {
                format!("loop-forever {}", rest.join(" "))
            } else {
                inner
            };
            let called = Rc::clone(context);
            let run = move || {
                if hop {
                    called.record.lock().unwrap().hops += 1;
                }
                run_command(&command, &called);
            };
            if *name == "setInterval" {
                context.set_interval(delay, Box::new(run));
            } else {
                context.set_timeout(delay, Box::new(run));
            }
        }
        "raf" | "raf-loop" => {
            // A loop re-registers itself from inside the frame, so it is
            // registered again on every opportunity, like a spinner.
            let command = if *name == "raf-loop" {
                "raf-loop".to_owned()
            } else {
                rest.join(" ")
            };
            context.record.lock().unwrap().frames_requested += 1;
            let called = Rc::clone(context);
            context.request_animation_frame(Box::new(move |_timestamp| {
                run_command(&command, &called);
            }));
        }
        _ => {}
    }
}

/// Appends a `<div id=…>` to the body, which is a DOM mutation the settle loop
/// can see and a test can look for.
fn append_marker(document: &SharedDocument, record: &Recorder, id: &str) {
    {
        let mut document = document.borrow_mut();
        let parent = document.body().unwrap_or_else(|| document.document_node());
        let div = document.create_html_element(atoms::DIV);
        document
            .mutate(Mutation::Append { parent, child: div })
            .expect("append the marker");
        document
            .mutate(Mutation::SetAttribute {
                element: div,
                name: QualName::new(atoms::NS_HTML, atoms::ID),
                value: id.to_owned(),
            })
            .expect("name the marker");
    }
    record.lock().unwrap().markers.push(id.to_owned());
}

// ------------------------------------------------------------------ the page

/// A page, the thread it runs on and what its engine did.
pub struct Harness {
    pub page: Page,
    pub thread: TestThread,
    pub record: Recorder,
    /// The paths of every request the page issued, in order, which is how a
    /// test tells a script that was fetched from one that was skipped.
    requests: Arc<Mutex<Vec<String>>>,
}

impl Harness {
    /// A page with a JavaScript engine and the library settle configuration.
    pub fn new(body: &str) -> Harness {
        Harness::builder(body).build()
    }

    pub fn builder(body: &str) -> Builder {
        Builder {
            body: body.to_owned(),
            // No `charset` parameter by default: the page sniffs the encoding,
            // which is the path a `<meta charset>` can still change.
            content_type: "text/html".to_owned(),
            settle: SettleConfig::library(),
            javascript: true,
            engine: true,
            watchdog: false,
            budgets: Budgets::default(),
            routes: Vec::new(),
        }
    }

    /// Navigates and waits, driving the page thread while it does. The page is
    /// moved into the job, as a page handle is in the pool.
    pub async fn goto(&self, options: Goto) -> Result<Navigation> {
        let page = self.page.clone();
        let thread = self.thread.clone();
        drive(&thread, async move { page.goto(URL, options).await }).await
    }

    /// Navigates to `URL` waiting for `wait_until` under the default deadline.
    pub async fn settle(&self, wait_until: Settle) -> Result<Navigation> {
        self.goto(Goto::new().wait_until(wait_until)).await
    }

    /// Reads the page's document without holding the page lock across another
    /// page call.
    pub fn document<R>(&self, f: impl FnOnce(&DomDocument) -> R) -> R {
        let document = self.page.document().expect("the page installed a document");
        f(&document)
    }

    /// Whether the installed document matches `selector`.
    pub fn has(&self, selector: &str) -> bool {
        self.document(|document| has_selector(document, selector))
    }

    /// The path of every request the fixture answered, in order. A fixture
    /// that serves one body for every path cannot tell "the script was
    /// fetched" from "the script was skipped"; this can.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

pub struct Builder {
    body: String,
    content_type: String,
    settle: SettleConfig,
    javascript: bool,
    engine: bool,
    /// Whether the stub engine hands out a watchdog handle.
    watchdog: bool,
    /// The page's budgets, which set the per-entry script deadline the page
    /// arms around an engine entry.
    budgets: Budgets,
    /// What the fixture answers for paths other than the document's.
    routes: Vec<(String, Answer)>,
}

impl Builder {
    pub fn content_type(mut self, content_type: &str) -> Builder {
        self.content_type = content_type.to_owned();
        self
    }

    pub fn settle(mut self, settle: SettleConfig) -> Builder {
        self.settle = settle;
        self
    }

    pub fn javascript(mut self, on: bool) -> Builder {
        self.javascript = on;
        self
    }

    /// Builds a page that parses without a JavaScript backend, which is what a
    /// Tier 1 page is.
    pub fn no_engine(mut self) -> Builder {
        self.engine = false;
        self
    }

    /// Whether the stub engine hands out a watchdog handle. Off by default, so
    /// only the tests that are about the watchdog arm a deadline.
    pub fn watchdog(mut self, on: bool) -> Builder {
        self.watchdog = on;
        self
    }

    /// The page's budgets. `evaluate_time` is the deadline the page arms
    /// around one engine entry, so a watchdog test sets it small.
    pub fn budgets(mut self, budgets: Budgets) -> Builder {
        self.budgets = budgets;
        self
    }

    /// Serves `body` at `path` as a script, for a page whose markup references
    /// it. The body is bytes rather than text so a test can serve a resource
    /// that is not valid UTF-8, which is the whole point of an encoding.
    pub fn route(mut self, path: &str, body: impl Into<Bytes>) -> Builder {
        self.routes.push((
            route_path(path),
            Answer::Body {
                body: body.into(),
                content_type: "text/javascript".to_owned(),
                delay: Duration::ZERO,
            },
        ));
        self
    }

    /// [`Builder::route`] with the answer held back, so a test can order two
    /// subresource fetches by hand.
    ///
    /// This is what makes a `defer`/`async` script's *timing* observable: which
    /// of two scripts arrives first, and whether the parse waited for one at
    /// all, are otherwise a race between two fetches that both answer at once.
    pub fn delayed_route(mut self, path: &str, body: impl Into<Bytes>, delay: Duration) -> Builder {
        self.routes.push((
            route_path(path),
            Answer::Body {
                body: body.into(),
                content_type: "text/javascript".to_owned(),
                delay,
            },
        ));
        self
    }

    /// Fails every request to `path` the way a dead origin does, so a page can
    /// reference a resource that cannot be fetched.
    pub fn failing_route(mut self, path: &str) -> Builder {
        self.routes.push((route_path(path), Answer::Fail));
        self
    }

    pub fn build(self) -> Harness {
        let record: Recorder = Arc::new(Mutex::new(Record::default()));
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let net = NetRuntime::new(1).expect("io runtime");
        let client = net
            .client(ClientConfig {
                hooks: Hooks {
                    interceptors: vec![Arc::new(Fixture {
                        document: Bytes::from(self.body),
                        document_content_type: self.content_type,
                        routes: self.routes,
                        requests: Arc::clone(&requests),
                    })],
                    observers: Vec::new(),
                },
                destination: DestinationPolicy::permissive(),
                ..ClientConfig::default()
            })
            .expect("client");
        let thread = TestThread::new();
        let engine = self.engine.then(|| {
            Arc::new(StubFactory {
                record: Arc::clone(&record),
                watchdog: self.watchdog,
            }) as Arc<dyn PageEngineFactory>
        });
        let page = Page::new(PageConfig {
            id: PageId(1),
            context: ContextId(1),
            client,
            thread: Arc::new(thread.clone()),
            clock: tenta_page::clock::system_clock(),
            max_tier: if self.engine { Tier::Js } else { Tier::Dom },
            javascript: self.javascript,
            engine,
            budgets: self.budgets,
            settle: self.settle,
            locale: "en-US".to_owned(),
            extra_headers: Vec::new(),
            viewport: Viewport::default(),
            on_close: None,
            test_hooks: TestHooks::default(),
        });
        Harness {
            page,
            thread,
            record,
            requests,
        }
    }
}

/// A path as the fixture matches it, so a test may name a route the way the
/// markup's `src` does (`x.js`) and still match the request's path (`/x.js`).
fn route_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}

/// What the fixture does with one path.
enum Answer {
    /// A body, with the content type it is served as, after `delay`.
    Body {
        body: Bytes,
        content_type: String,
        delay: Duration,
    },
    /// A transport failure, as a dead origin gives.
    Fail,
}

/// Answers each request with the body registered for its path — a script's
/// text for a script's path, the fixture body for the document's — so a test
/// never touches a socket or a DNS resolver.
///
/// The routing is what makes a fetched subresource testable: the same fixture
/// has to be able to serve a document that references `x.js` *and* the text of
/// `x.js`, and a request for a path it knows nothing about is a 404, which is
/// what a page would get from a real origin.
struct Fixture {
    document: Bytes,
    document_content_type: String,
    routes: Vec<(String, Answer)>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Interceptor for Fixture {
    fn intercept<'a>(
        &'a self,
        request: &'a mut tenta_net::request::Request,
    ) -> BoxFuture<'a, Interception> {
        let path = request.url.path().to_owned();
        self.requests.lock().unwrap().push(path.clone());
        if let Some((_, answer)) = self.routes.iter().find(|(route, _)| *route == path) {
            return match answer {
                Answer::Fail => Box::pin(async move {
                    Interception::Fail(NetError::new(
                        ErrorName::ConnectionRefused,
                        format!("the fixture refuses {path}"),
                    ))
                }),
                Answer::Body {
                    body,
                    content_type,
                    delay,
                } => {
                    let body = body.clone();
                    let content_type = content_type.clone();
                    let delay = *delay;
                    Box::pin(async move {
                        // Held on the io runtime, so the page thread is not
                        // blocked by a slow origin — this is the same wait a
                        // real one would impose.
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        Interception::Fulfill(fulfilled(StatusCode::OK, body, &content_type))
                    })
                }
            };
        }
        // The document is whatever the test navigates to; `URL` is its only
        // address, and its path is `/`.
        let (status, body, content_type) = if path == "/" {
            (
                StatusCode::OK,
                self.document.clone(),
                self.document_content_type.clone(),
            )
        } else {
            (
                StatusCode::NOT_FOUND,
                Bytes::from_static(b"not found\n"),
                "text/plain".to_owned(),
            )
        };
        Box::pin(async move { Interception::Fulfill(fulfilled(status, body, &content_type)) })
    }
}

/// A complete response, which is all the transport needs from an interceptor.
fn fulfilled(status: StatusCode, body: Bytes, content_type: &str) -> FulfilledResponse {
    FulfilledResponse::new(status, body).header("content-type", content_type)
}

// ------------------------------------------------------------------ helpers

/// Whether `selector` matches anything in `document`.
pub fn has_selector(document: &DomDocument, selector: &str) -> bool {
    document
        .query_selector(document.document_node(), selector)
        .ok()
        .flatten()
        .is_some()
}

/// The text of the first element matching `selector`, for a tail check that
/// reads what the parse actually produced.
pub fn text_of(document: &DomDocument, selector: &str) -> Option<String> {
    let node = document
        .query_selector(document.document_node(), selector)
        .ok()
        .flatten()?;
    Some(document.text_content(node))
}

/// A navigation error, for tests that expect one.
pub fn is_timeout(error: &Error) -> bool {
    matches!(error, Error::Timeout { .. })
}
