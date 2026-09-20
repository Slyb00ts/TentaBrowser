//! The engine boundary is engine-independent, so it is testable without a
//! backend: a stub that records what the page asked for is enough to pin the
//! shape the page depends on.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tenta_core::{Origin, TimerRule};
use tenta_dom::Document;
use tenta_js::bindings::host::SharedDocument;
use tenta_js::engine::{
    EngineClock, EngineWatchdog, PageEngine, PageEngineConfig, PageEngineFactory, ScriptError,
    ScriptErrorKind, ScriptKind,
};
use tenta_js::event_loop::ProvenanceSink;
use tenta_js::fetch::{FetchCompletion, FetchDriver, FetchOutcome, FetchRequest, FetchStartError};

/// Records what it was asked to do, so a test can assert the page's calls
/// arrive with the arguments the page meant.
#[derive(Default)]
struct Recorder {
    scripts: Vec<(String, ScriptKind)>,
    timer_rule: Option<TimerRule>,
    trims: usize,
    terminated: bool,
    /// How many completions the page handed the engine to settle.
    settled: usize,
}

/// The factory is shared across page threads while the engines it builds stay
/// on one thread each, so the recorded state is behind an `Arc<Mutex<_>>`: a
/// factory holding an `Rc` would not be storable in browser-wide configuration.
struct StubEngine {
    document: SharedDocument,
    recorder: Arc<Mutex<Recorder>>,
}

impl StubEngine {
    fn new(document: SharedDocument, recorder: Arc<Mutex<Recorder>>) -> StubEngine {
        StubEngine { document, recorder }
    }
}

impl PageEngine for StubEngine {
    fn document(&self) -> SharedDocument {
        self.document.clone()
    }

    fn run_script(&mut self, source: &str, kind: ScriptKind) -> Result<(), ScriptError> {
        if source.contains("throw") {
            return Err(ScriptError {
                kind: ScriptErrorKind::Uncaught,
                message: "stub complains".to_string(),
            });
        }
        self.recorder
            .lock()
            .unwrap()
            .scripts
            .push((source.to_string(), kind));
        Ok(())
    }

    fn set_timer_rule(&mut self, rule: TimerRule) {
        self.recorder.lock().unwrap().timer_rule = Some(rule);
    }

    fn set_provenance_sink(&mut self, sink: Rc<dyn ProvenanceSink>) {
        // A real engine reports every task it queues and finishes; the stub
        // proves the trait is reachable from an engine and callable.
        sink.task_queued(Origin::Tracked);
        sink.task_finished(Origin::Tracked);
    }

    fn set_fetch_driver(&mut self, _driver: Rc<dyn FetchDriver>) {}

    fn settle_fetches(&mut self, completions: Vec<FetchCompletion>) {
        self.recorder.lock().unwrap().settled += completions.len();
    }

    fn next_wakeup(&self) -> Option<Duration> {
        Some(Duration::ZERO)
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
        self.recorder.lock().unwrap().trims += 1;
    }

    fn gc_epoch(&self) -> u64 {
        0
    }

    fn watchdog(&self) -> Option<Box<dyn EngineWatchdog>> {
        let recorder = self.recorder.clone();
        Some(Box::new(StubWatchdog { recorder }))
    }
}

struct StubWatchdog {
    recorder: Arc<Mutex<Recorder>>,
}

impl EngineWatchdog for StubWatchdog {
    fn terminate(&self) {
        self.recorder.lock().unwrap().terminated = true;
    }
}

/// Takes every request and never answers it: the stub engine stores the driver
/// without calling it, so this only has to be nameable and installable.
struct StubDriver;

impl FetchDriver for StubDriver {
    fn start(&self, _request: FetchRequest, _resolver: u64) -> Result<(), FetchStartError> {
        Ok(())
    }
}

struct StubFactory {
    recorder: Arc<Mutex<Recorder>>,
}

impl PageEngineFactory for StubFactory {
    fn create(
        &self,
        document: SharedDocument,
        _config: &PageEngineConfig,
    ) -> Result<Box<dyn PageEngine>, String> {
        Ok(Box::new(StubEngine::new(document, self.recorder.clone())))
    }
}

fn config() -> PageEngineConfig {
    PageEngineConfig {
        timer_rule: TimerRule::default(),
        clock: EngineClock::Real,
        max_heap_bytes: None,
        script_timeout: None,
        locale: "en-US".to_string(),
        timezone: None,
        cookies: tenta_net::CookieJar::new(),
    }
}

/// The page reaches its engine only through the trait object it was handed, so
/// this is the whole surface it can depend on.
#[test]
fn a_page_drives_an_engine_through_the_trait_object_alone() {
    let document: SharedDocument = Rc::new(RefCell::new(Document::new()));
    let recorder = Arc::new(Mutex::new(Recorder::default()));
    let factory: Box<dyn PageEngineFactory> = Box::new(StubFactory {
        recorder: recorder.clone(),
    });

    let mut engine = factory
        .create(document.clone(), &config())
        .expect("the stub never refuses");

    // One tree: the handle the engine reports is the one the parser would
    // write into, not a copy.
    assert!(Rc::ptr_eq(&engine.document(), &document));

    engine.set_timer_rule(TimerRule::default());
    engine
        .run_script("var x = 1;", ScriptKind::Classic)
        .unwrap();
    assert_eq!(
        recorder.lock().unwrap().scripts,
        vec![("var x = 1;".to_string(), ScriptKind::Classic)]
    );

    // A script error is reported to the caller, not swallowed into an engine
    // failure: the page decides what a page-level failure means.
    let error = engine
        .run_script("throw 1;", ScriptKind::Classic)
        .expect_err("the stub throws");
    assert_eq!(error.kind, ScriptErrorKind::Uncaught);
    assert_eq!(recorder.lock().unwrap().scripts.len(), 1);

    engine.trim_memory();
    assert_eq!(recorder.lock().unwrap().trims, 1);

    // The fetch seam is reachable the same way: a driver is installed through
    // the trait object, and a batch of completions is handed over to settle.
    engine.set_fetch_driver(Rc::new(StubDriver));
    engine.settle_fetches(vec![
        FetchCompletion {
            resolver: 1,
            outcome: FetchOutcome::Failed("stub".to_string()),
        },
        FetchCompletion {
            resolver: 2,
            outcome: FetchOutcome::Failed("stub".to_string()),
        },
    ]);
    assert_eq!(recorder.lock().unwrap().settled, 2);

    let watchdog = engine.watchdog().expect("the stub can be interrupted");
    watchdog.terminate();
    assert!(recorder.lock().unwrap().terminated);
}

/// The provenance bridge is the seam the settle machine hangs off. Both
/// directions have to arrive, or the settle machine counts work that never
/// ends and settles late.
#[test]
fn an_engine_reaches_the_settle_machine_through_the_provenance_sink() {
    #[derive(Default)]
    struct Counts {
        tracked: usize,
        finished: usize,
        other: usize,
    }

    struct Counting {
        counts: Arc<Mutex<Counts>>,
    }

    impl ProvenanceSink for Counting {
        fn task_queued(&self, origin: Origin) {
            let mut counts = self.counts.lock().unwrap();
            match origin {
                Origin::Tracked => counts.tracked += 1,
                Origin::Background => counts.other += 1,
            }
        }

        fn task_finished(&self, origin: Origin) {
            let mut counts = self.counts.lock().unwrap();
            match origin {
                Origin::Tracked => counts.finished += 1,
                Origin::Background => counts.other += 1,
            }
        }
    }

    let document: SharedDocument = Rc::new(RefCell::new(Document::new()));
    let counts = Arc::new(Mutex::new(Counts::default()));
    let mut engine = StubEngine::new(document, Arc::new(Mutex::new(Recorder::default())));

    engine.set_provenance_sink(Rc::new(Counting {
        counts: counts.clone(),
    }));

    let observed = counts.lock().unwrap();
    assert_eq!(observed.tracked, 1);
    assert_eq!(observed.finished, 1);
    assert_eq!(observed.other, 0);
}

/// The factory is stored in browser-wide configuration, which is shared across
/// page threads, while the engines it builds stay on one thread each. Both
/// halves of that split are load-bearing: assert them here rather than
/// discovering the first when a page thread is written and the second when a
/// page and its parser stop sharing one tree.
#[test]
fn the_factory_is_shareable_across_page_threads() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<StubFactory>();
    assert_send_sync::<Box<dyn PageEngineFactory>>();
    assert_send_sync::<Arc<dyn PageEngineFactory>>();
}
