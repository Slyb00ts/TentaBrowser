//! The tracked count of a timer whose firing a backend has deferred must follow
//! the timer's whole life: it is taken when the timer is scheduled and released
//! when the deferred call runs, when the timer is cancelled, or at teardown -
//! never by the task that only moved the handle
//! (`docs/spec/02-architecture.md` §5.1).
//!
//! The V8 backend is the one that defers: the task that dequeues a due timer
//! pushes the handler onto a ready queue and the engine calls it later
//! (`crates/tenta-js-v8/src/host.rs`). Between those two points the schedule-time
//! count used to be released already, so a settle poll landing in the window saw
//! a settled page that still had script owed. These tests drive the loop the way
//! that backend does - by recording a firing and claiming it back - and pin the
//! count across every exit from the window.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::Origin;
use tenta_js::event_loop::{
    Clock, EventLoop, FiringRelease, MonoTime, ProvenanceSink, Scheduler, TimerId,
};

/// Records every queued/finished pair so a test can replay the running count the
/// settle machine would see.
#[derive(Default)]
struct RecordingSink {
    events: RefCell<Vec<(&'static str, Origin)>>,
}

impl ProvenanceSink for RecordingSink {
    fn task_queued(&self, origin: Origin) {
        self.events.borrow_mut().push(("queued", origin));
    }

    fn task_finished(&self, origin: Origin) {
        self.events.borrow_mut().push(("finished", origin));
    }
}

/// A virtual-clock loop with its provenance observer installed.
fn loop_with_log() -> (EventLoop, Scheduler, Rc<RecordingSink>) {
    let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
    let scheduler = loop_.scheduler();
    let log = Rc::new(RecordingSink::default());
    loop_.task_sink().set_provenance_sink(log.clone());
    (loop_, scheduler, log)
}

/// Replays `events` as a running tracked count, one figure per event in order.
fn tracked_counts(events: &[(&'static str, Origin)]) -> Vec<i32> {
    let mut count = 0i32;
    events
        .iter()
        .filter(|(_, origin)| *origin == Origin::Tracked)
        .map(|(kind, _)| {
            count += if *kind == "queued" { 1 } else { -1 };
            count
        })
        .collect()
}

/// Schedules a `Tracked` one-shot whose callback defers its own firing the way
/// the V8 bridge does: it records the deferral and calls nothing, leaving the
/// call to a later claim.
fn schedule_deferred_tracked_one_shot(
    scheduler: &Scheduler,
    deferred: Rc<RefCell<Vec<TimerId>>>,
) -> TimerId {
    let callback_scheduler = scheduler.clone();
    scheduler.with_origin(Origin::Tracked, || {
        scheduler.set_timeout(
            0,
            Box::new(move || {
                if let Some(id) = callback_scheduler.defer_firing() {
                    deferred.borrow_mut().push(id);
                }
            }),
        )
    })
}

#[test]
fn a_panicking_handler_still_releases_the_firing() {
    // The page thread catches a panicking Rust callback and keeps running, so
    // the release has to happen while unwinding as well as after the call.
    // Otherwise a handler that panics leaves the settle machine counting work
    // that will never finish, and the page never settles. The V8 bridge holds a
    // `FiringRelease` across its call for exactly this reason; this drives the
    // same guard through the same catch the page thread does.
    let (loop_, scheduler, log) = loop_with_log();
    let deferred = Rc::new(RefCell::new(Vec::new()));
    let id = schedule_deferred_tracked_one_shot(&scheduler, deferred.clone());

    loop_.fire_due_timers();
    let firing = scheduler.claim_fired(id).expect("the firing is owed");

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _release = FiringRelease::new(&scheduler, firing);
        panic!("the handler blew up");
    }));
    assert!(outcome.is_err(), "the panic reached the page thread");

    let events = log.events.borrow();
    let counts = tracked_counts(&events);
    assert_eq!(
        counts.last(),
        Some(&0),
        "the release ran while unwinding: {events:?}"
    );
}

#[test]
fn a_tracked_one_shot_stays_counted_across_its_whole_life() {
    let (loop_, scheduler, log) = loop_with_log();
    let deferred = Rc::new(RefCell::new(Vec::new()));
    let id = schedule_deferred_tracked_one_shot(&scheduler, deferred.clone());

    loop_.fire_due_timers();
    assert_eq!(*deferred.borrow(), vec![id], "the backend took the firing");

    // The firing task moved the handle but not the count: a settle poll here
    // must still see the timer as work in flight.
    let events = log.events.borrow();
    assert_eq!(
        *events,
        vec![
            ("queued", Origin::Tracked),
            ("queued", Origin::Tracked),
            ("finished", Origin::Tracked),
        ],
        "the firing task is a neutral unit of work"
    );
    let counts = tracked_counts(&events);
    assert_eq!(
        counts.last(),
        Some(&1),
        "still owed while the call is: {counts:?}"
    );
    drop(events);

    // The backend finally calls the handler, then releases.
    let firing = scheduler.claim_fired(id).expect("the firing is owed");
    scheduler.note_firing_finished(firing.origin);

    let events = log.events.borrow();
    let counts = tracked_counts(&events);
    assert_eq!(counts.last(), Some(&0), "released by the call: {counts:?}");
    assert!(
        counts[..counts.len() - 1].iter().all(|count| *count > 0),
        "the count only reaches zero after the call: {counts:?}"
    );
}

#[test]
fn clear_timeout_in_the_deferral_window_releases_the_count_once() {
    let (loop_, scheduler, log) = loop_with_log();
    let deferred = Rc::new(RefCell::new(Vec::new()));
    let id = schedule_deferred_tracked_one_shot(&scheduler, deferred.clone());

    loop_.fire_due_timers();
    assert_eq!(*deferred.borrow(), vec![id], "the backend took the firing");

    scheduler.clear_timeout(id);

    // The scheduled count is released by the cancel, exactly once: the firing
    // task's own pair is neutral, so nothing else may report a finish.
    assert_eq!(
        *log.events.borrow(),
        vec![
            ("queued", Origin::Tracked),
            ("queued", Origin::Tracked),
            ("finished", Origin::Tracked),
            ("finished", Origin::Tracked),
        ]
    );
    assert!(
        scheduler.claim_fired(id).is_none(),
        "nothing may claim the aborted firing afterwards"
    );
}

#[test]
fn clear_pending_callbacks_releases_a_deferred_firing() {
    let (loop_, scheduler, log) = loop_with_log();
    let deferred = Rc::new(RefCell::new(Vec::new()));
    let id = schedule_deferred_tracked_one_shot(&scheduler, deferred.clone());
    // This timer has not fired yet when teardown begins. It used to remain
    // live in the heap even after its callback had been discarded, keeping the
    // loop awake at its old deadline.
    scheduler.set_timeout(100, Box::new(|| {}));

    loop_.fire_due_timers();
    assert_eq!(*deferred.borrow(), vec![id], "the backend took the firing");

    loop_.clear_pending_callbacks();

    assert_eq!(
        *log.events.borrow(),
        vec![
            ("queued", Origin::Tracked),
            ("queued", Origin::Tracked),
            ("finished", Origin::Tracked),
            ("finished", Origin::Tracked),
        ],
        "teardown releases the deferred firing exactly once"
    );

    // A second teardown must not release it again.
    loop_.clear_pending_callbacks();
    assert_eq!(
        log.events.borrow().len(),
        4,
        "the firing was already gone: {:?}",
        log.events.borrow()
    );
    assert!(scheduler.claim_fired(id).is_none());
    assert!(
        !loop_.has_pending_timers(),
        "teardown drops unfired timers too"
    );
    assert_eq!(loop_.next_timer_deadline(), None);
    assert_eq!(loop_.next_wakeup(), None);
    assert!(
        !loop_.advance_to_next_timer(),
        "a discarded deadline must not advance virtual time"
    );
}

#[test]
fn an_interval_firing_is_released_per_firing() {
    let (loop_, scheduler, log) = loop_with_log();
    let deferred = Rc::new(RefCell::new(Vec::new()));

    let callback_scheduler = scheduler.clone();
    let callback_deferred = deferred.clone();
    let id = scheduler.set_interval(
        0,
        Box::new(move || {
            if let Some(id) = callback_scheduler.defer_firing() {
                callback_deferred.borrow_mut().push(id);
            }
        }),
    );

    loop_.advance_to_next_timer();
    loop_.advance_to_next_timer();
    assert_eq!(*deferred.borrow(), vec![id, id], "two firings deferred");

    // Each firing is spent by its own claim, in order, and none is left over.
    for _ in 0..2 {
        let firing = scheduler.claim_fired(id).expect("a firing is owed");
        assert_eq!(
            firing.origin,
            Origin::Background,
            "the timer rule never counts a repeating timer, so this firing holds nothing"
        );
        scheduler.note_firing_finished(firing.origin);
    }
    assert!(
        scheduler.claim_fired(id).is_none(),
        "both firings are spent"
    );

    // Because no firing carried a count, every release above was a no-op: one
    // queued/finished pair per firing, and nothing else.
    assert_eq!(
        *log.events.borrow(),
        vec![
            ("queued", Origin::Background),
            ("finished", Origin::Background),
            ("queued", Origin::Background),
            ("finished", Origin::Background),
        ]
    );
}

#[test]
fn a_background_firing_is_not_released() {
    // Tracked: counted at schedule time, released by the call.
    let (loop_, scheduler, log) = loop_with_log();
    let deferred = Rc::new(RefCell::new(Vec::new()));
    let id = schedule_deferred_tracked_one_shot(&scheduler, deferred.clone());
    loop_.fire_due_timers();
    let firing = scheduler.claim_fired(id).expect("the firing is owed");
    assert_eq!(firing.origin, Origin::Tracked);
    scheduler.note_firing_finished(firing.origin);
    assert_eq!(
        *log.events.borrow(),
        vec![
            ("queued", Origin::Tracked),
            ("queued", Origin::Tracked),
            ("finished", Origin::Tracked),
            ("finished", Origin::Tracked),
        ],
        "a tracked firing is released by its call"
    );

    // Background: nothing was counted before it fired, so releasing it would
    // report a finish with no matching queued and unbalance the observer.
    let (loop_, scheduler, log) = loop_with_log();
    let callback_scheduler = scheduler.clone();
    let id = scheduler.set_timeout(
        0,
        Box::new(move || {
            callback_scheduler.defer_firing();
        }),
    );
    loop_.fire_due_timers();
    let firing = scheduler.claim_fired(id).expect("the firing is owed");
    assert_eq!(firing.origin, Origin::Background);
    scheduler.note_firing_finished(firing.origin);
    assert_eq!(
        *log.events.borrow(),
        vec![
            ("queued", Origin::Background),
            ("finished", Origin::Background),
        ],
        "a background firing must not emit a release"
    );
}
