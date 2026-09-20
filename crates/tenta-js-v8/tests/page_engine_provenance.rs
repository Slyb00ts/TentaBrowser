//! The provenance a script's own work inherits, asserted against the real
//! backend.
//!
//! A page only ever calls [`PageEngine::run_script`] for a `<script>` its parse
//! is paused on, which is part of loading the document. The work such a script
//! starts therefore has to be attributed to the load chain: a short
//! `setTimeout` an inline script schedules is a *deferral* a settle call waits
//! for, and the settle machine can only know that if the engine reports the
//! timer as [`Origin::Tracked`] (`docs/spec/02-architecture.md` §5.1).
//!
//! The sink these tests read is a deferral counter, not a log of the tasks the
//! engine queues: a scheduler announces a unit only when it is
//! [`Origin::Tracked`], because only tracked work holds a settle open
//! (`TimerScheduler::set_timeout`, and `note_firing_finished`'s note on why a
//! background release would unbalance the count). So "announced as `Tracked`"
//! and "not announced at all" are the two observations, and background work is
//! the silent one.
//!
//! This is asserted here rather than in `tenta-page`, because the page's own
//! tests drive a stub engine: without this, nothing would notice the real
//! backend reporting every script's timers as unannounced, and a settle call
//! would end while an inline script's deferral was still pending.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_core::{Origin, TimerRule};
use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::{EngineClock, PageEngine, PageEngineConfig, PageEngineFactory, ScriptKind};
use tenta_js::event_loop::{Clock, MonoTime, ProvenanceSink};
use tenta_js_v8::{Engine, V8EngineFactory};
use tenta_net::CookieJar;

/// A delay short enough to be a deferral under [`TimerRule::default`], so the
/// only thing that can make a timer background is its parent's provenance.
const SHORT: u64 = 5;

/// The tracked work the engine announced, in order.
#[derive(Default)]
struct Recording {
    queued: RefCell<Vec<Origin>>,
}

impl Recording {
    /// What the engine reported as pending tracked work. Empty means the
    /// settle call has nothing to wait for.
    fn deferrals(&self) -> Vec<Origin> {
        self.queued.borrow().clone()
    }
}

impl ProvenanceSink for Recording {
    fn task_queued(&self, origin: Origin) {
        self.queued.borrow_mut().push(origin);
    }

    fn task_finished(&self, _origin: Origin) {}
}

fn document() -> SharedDocument {
    Rc::new(RefCell::new(Document::new()))
}

fn config() -> PageEngineConfig {
    PageEngineConfig {
        // The library rule, which tracks a 5 ms timer whose parent is tracked.
        timer_rule: TimerRule::default(),
        // A virtual clock so the timers never fire: this test is about what the
        // engine reports when it queues them, not about them running.
        clock: EngineClock::Virtual,
        max_heap_bytes: None,
        script_timeout: None,
        locale: "en-US".to_owned(),
        timezone: None,
        cookies: CookieJar::new(),
    }
}

/// One engine built the way a page builds it: through the factory, over a
/// document, reporting to `sink`.
fn page_engine(sink: &Rc<Recording>) -> Box<dyn PageEngine> {
    let mut engine = V8EngineFactory
        .create(document(), &config())
        .expect("the factory builds an engine over a document");
    engine.set_provenance_sink(Rc::clone(sink) as Rc<dyn ProvenanceSink>);
    engine
}

/// The regression this file exists for: without the attribution, the timer is
/// silent, the settle call sees no tracked work and ends while the deferral is
/// still pending.
#[test]
fn a_script_the_page_hands_out_owns_the_work_it_starts() {
    let recorded = Rc::new(Recording::default());
    let mut engine = page_engine(&recorded);

    engine
        .run_script(
            &format!("setTimeout(function () {{}}, {SHORT});"),
            ScriptKind::Classic,
        )
        .expect("a one-statement script runs");

    assert_eq!(
        recorded.deferrals(),
        vec![Origin::Tracked],
        "a timer an inline script schedules is part of loading the document, \
         so a settle call must wait for it"
    );
}

#[test]
fn an_interval_is_never_a_deferral_even_from_a_tracked_script() {
    // The attribution is the script's provenance, not a blanket tag: the
    // engine's timer rule still separates a deferral from activity. An interval
    // runs for as long as the page stays open, so it must never hold a settle
    // open however short it is (`TimerRule::origin_for`).
    let recorded = Rc::new(Recording::default());
    let mut engine = page_engine(&recorded);

    engine
        .run_script(
            &format!("setInterval(function () {{}}, {SHORT});"),
            ScriptKind::Classic,
        )
        .expect("a one-statement script runs");

    assert!(
        recorded.deferrals().is_empty(),
        "an interval is the page's own activity even from a script that runs tracked, \
         so the settle must not wait for it"
    );
}

#[test]
fn work_started_outside_a_page_script_is_not_a_deferral() {
    // `evaluate` is the harness entry point, not a script the page handed out:
    // it runs after the page has loaded, so anything it starts is the page's
    // own initiative. Asserting the contrast keeps the attribution in
    // `run_script` from being widened into the engine core, where it would make
    // every evaluation a caller makes hold a settle open.
    let recorded = Rc::new(Recording::default());
    let mut engine = Engine::with_document_on_clock(
        document(),
        Clock::virtual_at(MonoTime::ZERO),
        None,
        CookieJar::new(),
    );
    engine.set_timer_rule(TimerRule::default());
    engine.set_provenance_sink(Rc::clone(&recorded) as Rc<dyn ProvenanceSink>);

    engine
        .evaluate(&format!("setTimeout(function () {{}}, {SHORT});"))
        .expect("a one-statement script evaluates");

    assert!(
        recorded.deferrals().is_empty(),
        "an evaluation is not a parser-blocking script, so the settle must not wait for it"
    );
}

#[test]
fn a_timer_past_the_rules_delay_is_not_a_deferral_even_from_a_script() {
    // The other side of the same coin: a long timer is activity, so a settle
    // call ends without waiting for it even though the script ran tracked. This
    // is what says the attribution did not disable the rule.
    let recorded = Rc::new(Recording::default());
    let mut engine = page_engine(&recorded);
    let long = TimerRule::default().max_delay + Duration::from_millis(1);

    engine
        .run_script(
            &format!("setTimeout(function () {{}}, {});", long.as_millis()),
            ScriptKind::Classic,
        )
        .expect("a one-statement script runs");

    assert!(
        recorded.deferrals().is_empty(),
        "a timer past the rule's delay is activity, not a deferral, \
         so the settle must not wait for it"
    );
}
