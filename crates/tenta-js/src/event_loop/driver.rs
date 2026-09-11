//! The event-loop driver that ties the clock, timer heap, task queues, and
//! animation-frame callbacks into one processing model (`docs/spec/05-javascript.md`
//! §5).
//!
//! The [`Scheduler`] is the surface script sees: `setTimeout`, `clearTimeout`,
//! `requestAnimationFrame`, `queueMicrotask`. It is a cloneable single-threaded
//! handle so a callback can re-enter it to schedule more work. [`EventLoop`]
//! owns the scheduler and the driving methods — firing due timers, running a
//! rendering opportunity's animation-frame callbacks — none of which hold a
//! borrow of the shared state while a callback runs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use tenta_core::{Origin, TimerRule};

use super::tasks::{TaskRunner, TaskSink};
use super::time::{Clock, MonoTime};
use super::timers::{TimerId, TimerQueue};

/// One rendering opportunity at 60 Hz, the cadence a virtual clock steps for
/// `requestAnimationFrame` (`docs/spec/05` §5). 1/60 s rounded to the nearest
/// nanosecond.
pub const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

/// Identifies a scheduled animation-frame callback (`requestAnimationFrame`'s
/// return value).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct AnimationFrameId(u64);

impl AnimationFrameId {
    /// The integer handle exposed to script.
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Reconstructs an `AnimationFrameId` from the integer handle script holds,
    /// so `cancelAnimationFrame(id)` can round-trip a value that crossed the JS
    /// boundary. An id that names no pending callback is a harmless no-op.
    pub fn from_u64(value: u64) -> Self {
        AnimationFrameId(value)
    }
}

/// A scheduled timer's callback, the nesting level to install while it runs so
/// a timer it schedules is clamped one level deeper, and the provenance the
/// firing carries (decided once, at schedule time, by [`TimerRule`]).
enum TimerCallback {
    /// A `setTimeout` callback: runs once, then its slot is dropped.
    OneShot {
        nesting: u32,
        origin: Origin,
        run: Box<dyn FnOnce()>,
    },
    /// A `setInterval` callback: runs every `period_ms`, re-armed under the same
    /// id after each firing until cancelled. Callable more than once, so it is
    /// an `FnMut` moved back into the timer map between firings.
    Interval {
        nesting: u32,
        origin: Origin,
        period_ms: u64,
        run: Box<dyn FnMut()>,
    },
}

/// A pending animation-frame callback. It receives the frame timestamp in
/// milliseconds, as `requestAnimationFrame` callbacks do. Once run or cancelled
/// its closure is taken, leaving a tombstone that the current rendering
/// opportunity drops when it finishes.
struct FrameCallback {
    id: AnimationFrameId,
    run: Option<Box<dyn FnOnce(f64)>>,
}

/// Restores the previous timer nesting depth on scope exit, *including on
/// unwind*: a page thread survives a panicking callback, and a leaked depth
/// would clamp unrelated timers that were never nested at all.
struct NestingGuard {
    shared: Rc<RefCell<Shared>>,
    previous: u32,
}

impl Drop for NestingGuard {
    fn drop(&mut self) {
        self.shared.borrow_mut().current_nesting = self.previous;
    }
}

/// Runs `body` with `nesting` installed as the current timer depth.
fn with_nesting_on<R>(shared: &Rc<RefCell<Shared>>, nesting: u32, body: impl FnOnce() -> R) -> R {
    let previous = std::mem::replace(&mut shared.borrow_mut().current_nesting, nesting);
    let _guard = NestingGuard {
        shared: shared.clone(),
        previous,
    };
    body()
}

/// The mutable state shared between the scheduler handle and the driver.
struct Shared {
    clock: Clock,
    timers: TimerQueue,
    timer_callbacks: HashMap<TimerId, TimerCallback>,
    /// Nesting level of the timer callback currently running; 0 at top level.
    current_nesting: u32,
    /// When a timer defers tracked work rather than being page activity.
    timer_rule: TimerRule,
    next_frame_id: u64,
    /// Callbacks registered for the next rendering opportunity.
    frame_callbacks: Vec<FrameCallback>,
}

/// The script-facing scheduling surface. Cloneable, single-threaded, and safe
/// to hold inside a running callback.
#[derive(Clone)]
pub struct Scheduler {
    shared: Rc<RefCell<Shared>>,
    sink: TaskSink,
}

impl Scheduler {
    /// The current time on the loop's clock.
    pub fn now(&self) -> MonoTime {
        self.shared.borrow().clock.now()
    }

    /// Schedules `callback` to run after `delay_ms` (`setTimeout`). The Chromium
    /// nesting clamp is applied relative to the timer currently running.
    pub fn set_timeout(&self, delay_ms: u64, callback: Box<dyn FnOnce()>) -> TimerId {
        let origin = self.timer_origin(false, delay_ms);
        let mut shared = self.shared.borrow_mut();
        let nesting = shared.current_nesting + 1;
        let now = shared.clock.now();
        let id = shared.timers.schedule(now, delay_ms, nesting);
        shared.timer_callbacks.insert(
            id,
            TimerCallback::OneShot {
                nesting,
                origin,
                run: callback,
            },
        );
        drop(shared);
        if origin == Origin::Tracked {
            // A tracked one-shot timer is a deferral of the load chain, so it
            // is pending from now, not from when it fires. Counting it only at
            // firing time would leave a window - the whole delay - in which a
            // settle poll sees no tracked work and declares the page done with
            // the continuation still in the heap.
            self.sink.note_queued(origin);
        }
        id
    }

    /// The provenance a timer scheduled right now would carry. Decided once,
    /// here, because the rule is about the *scheduling* task: by the time the
    /// timer fires that task is long gone.
    fn timer_origin(&self, repeating: bool, delay_ms: u64) -> Origin {
        let shared = self.shared.borrow();
        shared.timer_rule.origin_for(
            self.sink.current_origin(),
            repeating,
            Duration::from_millis(delay_ms),
            shared.current_nesting + 1,
        )
    }

    /// The nesting level a timer scheduled right now would get. The engine
    /// reads this when it defers the actual callback invocation past the task
    /// that dequeued the timer.
    pub fn current_nesting(&self) -> u32 {
        self.shared.borrow().current_nesting
    }

    /// Runs `body` with `nesting` installed as the current timer depth, so a
    /// `setTimeout` made from inside a timer handler is clamped one level
    /// deeper even when the handler runs outside the queued task.
    pub fn with_nesting<R>(&self, nesting: u32, body: impl FnOnce() -> R) -> R {
        with_nesting_on(&self.shared, nesting, body)
    }

    /// The provenance work queued right now inherits.
    pub fn current_origin(&self) -> Origin {
        self.sink.current_origin()
    }

    /// Runs `body` with `origin` as the inherited provenance.
    pub fn with_origin<R>(&self, origin: Origin, body: impl FnOnce() -> R) -> R {
        self.sink.with_origin(origin, body)
    }

    /// Replaces the timer rule, which the page derives from its `SettleConfig`.
    pub fn set_timer_rule(&self, rule: TimerRule) {
        self.shared.borrow_mut().timer_rule = rule;
    }

    /// Cancels a scheduled timer (`clearTimeout`).
    pub fn clear_timeout(&self, id: TimerId) {
        let cancelled = {
            let mut shared = self.shared.borrow_mut();
            shared.timers.cancel(id);
            shared.timer_callbacks.remove(&id)
        };
        // Balance the pending count of a tracked one-shot that will now never
        // run. A timer already taken out of the map by `fire_due_timers` is
        // not here, and its queued task reports the finish itself.
        if let Some(TimerCallback::OneShot {
            origin: Origin::Tracked,
            ..
        }) = cancelled
        {
            self.sink.note_finished(Origin::Tracked);
        }
    }

    /// Schedules `callback` to run every `delay_ms` (`setInterval`) and returns
    /// the id that names the repeating timer. The first firing is one `delay_ms`
    /// away; each firing re-arms the same id one nesting level deeper, so a
    /// `setInterval(f, 0)` is clamped like a nested `setTimeout` rather than
    /// busy-looping. The loop re-arms the timer, never script.
    pub fn set_interval(&self, delay_ms: u64, callback: Box<dyn FnMut()>) -> TimerId {
        let origin = self.timer_origin(true, delay_ms);
        let mut shared = self.shared.borrow_mut();
        let nesting = shared.current_nesting + 1;
        let now = shared.clock.now();
        let id = shared.timers.schedule(now, delay_ms, nesting);
        shared.timer_callbacks.insert(
            id,
            TimerCallback::Interval {
                nesting,
                origin,
                period_ms: delay_ms,
                run: callback,
            },
        );
        id
    }

    /// Cancels a repeating timer (`clearInterval`) so it is not re-armed and its
    /// callback is dropped. The same mechanism as [`Self::clear_timeout`]; a
    /// distinct name mirrors the script-facing API.
    pub fn clear_interval(&self, id: TimerId) {
        self.clear_timeout(id);
    }

    /// Registers a callback for the next rendering opportunity
    /// (`requestAnimationFrame`).
    pub fn request_animation_frame(&self, callback: Box<dyn FnOnce(f64)>) -> AnimationFrameId {
        let mut shared = self.shared.borrow_mut();
        let id = AnimationFrameId(shared.next_frame_id);
        shared.next_frame_id += 1;
        shared.frame_callbacks.push(FrameCallback {
            id,
            run: Some(callback),
        });
        id
    }

    /// Cancels a pending animation-frame callback (`cancelAnimationFrame`).
    /// Cancelling from inside the same frame still prevents a not-yet-run
    /// callback from firing; an unknown id is a no-op that stores nothing.
    pub fn cancel_animation_frame(&self, id: AnimationFrameId) {
        let mut shared = self.shared.borrow_mut();
        if let Some(entry) = shared.frame_callbacks.iter_mut().find(|e| e.id == id) {
            entry.run = None;
        }
    }

    /// Queues a macrotask.
    pub fn queue_task(&self, task: impl FnOnce() + 'static) {
        self.sink.queue_task(task);
    }

    /// Queues a microtask (`queueMicrotask`, promise reactions).
    pub fn queue_microtask(&self, task: impl FnOnce() + 'static) {
        self.sink.queue_microtask(task);
    }
}

/// The event loop: a clock, a timer heap, task queues, and animation-frame
/// callbacks, driven together.
pub struct EventLoop {
    shared: Rc<RefCell<Shared>>,
    runner: TaskRunner,
    scheduler: Scheduler,
}

impl EventLoop {
    /// A loop driven by `clock` (real for live pages, virtual for headless
    /// determinism).
    pub fn new(clock: Clock) -> Self {
        let shared = Rc::new(RefCell::new(Shared {
            clock,
            timers: TimerQueue::new(),
            timer_callbacks: HashMap::new(),
            current_nesting: 0,
            timer_rule: TimerRule::default(),
            next_frame_id: 0,
            frame_callbacks: Vec::new(),
        }));
        let runner = TaskRunner::new();
        let scheduler = Scheduler {
            shared: shared.clone(),
            sink: runner.sink(),
        };
        Self {
            shared,
            runner,
            scheduler,
        }
    }

    /// The scheduling surface to hand to the engine and its callbacks.
    pub fn scheduler(&self) -> Scheduler {
        self.scheduler.clone()
    }

    /// The current clock time.
    pub fn now(&self) -> MonoTime {
        self.shared.borrow().clock.now()
    }

    /// Whether any timer is still scheduled.
    pub fn has_pending_timers(&self) -> bool {
        !self.shared.borrow().timers.is_empty()
    }

    /// The earliest scheduled timer deadline, if any.
    pub fn next_timer_deadline(&self) -> Option<MonoTime> {
        self.shared.borrow_mut().timers.next_deadline()
    }

    /// Drops every pending timer and animation-frame callback without running
    /// them. A callback may own a handle into an engine's script isolate (a V8
    /// `Global`); this lets the owner release those handles while the isolate is
    /// still alive, so teardown does not depend on struct field drop order. The
    /// clock and task queues are left untouched.
    pub fn clear_pending_callbacks(&self) {
        let mut shared = self.shared.borrow_mut();
        shared.timer_callbacks.clear();
        shared.frame_callbacks.clear();
    }

    /// Fires every timer due at the current clock time, running each callback as
    /// a task followed by a microtask checkpoint, in deadline-then-insertion
    /// order. Timers scheduled by these callbacks do not fire until the clock
    /// advances again.
    ///
    /// Not re-entrant: an interval re-arms its id in the timer queue here but
    /// only re-inserts its callback while the queued task runs in
    /// [`TaskRunner::run_until_idle`] below, so between those points the re-armed
    /// id has no callback. Every caller lets this method (with its internal
    /// `run_until_idle`) return before the next `take_expired`, which closes that
    /// window; a future re-entrant caller would reopen it.
    pub fn fire_due_timers(&self) {
        let due = {
            let mut shared = self.shared.borrow_mut();
            let now = shared.clock.now();
            shared.timers.take_expired(now)
        };
        for id in due {
            let callback = self.shared.borrow_mut().timer_callbacks.remove(&id);
            let Some(callback) = callback else { continue };
            match callback {
                TimerCallback::OneShot {
                    nesting,
                    origin,
                    run,
                } => {
                    let shared = self.shared.clone();
                    // Install this timer's nesting level so a timer it
                    // schedules is clamped one level deeper; the guard restores
                    // the top level even if the callback panics.
                    let task = move || with_nesting_on(&shared, nesting, run);
                    if origin == Origin::Tracked {
                        // Already counted when it was scheduled.
                        self.scheduler.sink.queue_task_prearmed(origin, task);
                    } else {
                        self.scheduler.sink.queue_task_from(origin, task);
                    }
                }
                TimerCallback::Interval {
                    nesting,
                    origin,
                    period_ms,
                    mut run,
                } => {
                    // Re-arm the same id for the next period before running, one
                    // nesting level deeper, so the handle script holds stays
                    // valid and a zero-delay interval clamps rather than spins.
                    let next_nesting = nesting.saturating_add(1);
                    {
                        let mut shared = self.shared.borrow_mut();
                        let now = shared.clock.now();
                        shared.timers.reschedule(id, now, period_ms, next_nesting);
                    }
                    let shared = self.shared.clone();
                    self.scheduler.sink.queue_task_from(origin, move || {
                        with_nesting_on(&shared, next_nesting, &mut run);
                        let mut shared = shared.borrow_mut();
                        // Put the callback back under its id for the re-armed
                        // firing — unless `clearInterval` cancelled it meanwhile,
                        // in which case the re-armed heap slot is already dead.
                        if shared.timers.is_live(id) {
                            shared.timer_callbacks.insert(
                                id,
                                TimerCallback::Interval {
                                    nesting: next_nesting,
                                    origin,
                                    period_ms,
                                    run,
                                },
                            );
                        }
                    });
                }
            }
        }
        self.runner.run_until_idle();
    }

    /// Advances a virtual clock to the next timer deadline and fires it. Returns
    /// whether a timer was pending. A no-op returning `false` on a real clock or
    /// an empty queue.
    pub fn advance_to_next_timer(&self) -> bool {
        let (is_virtual, next) = {
            let mut shared = self.shared.borrow_mut();
            (shared.clock.is_virtual(), shared.timers.next_deadline())
        };
        match next {
            Some(deadline) if is_virtual => {
                self.shared.borrow_mut().clock.advance_to(deadline);
                self.fire_due_timers();
                true
            }
            _ => false,
        }
    }

    /// Drives a virtual clock through every scheduled timer, including timers
    /// those callbacks schedule, until none remain. Guards against a runaway by
    /// stopping after `max_steps` firings.
    pub fn run_timers_until_idle(&self, max_steps: usize) {
        for _ in 0..max_steps {
            if !self.advance_to_next_timer() {
                return;
            }
        }
    }

    /// Runs one rendering opportunity: every animation-frame callback registered
    /// before now, each with the current frame timestamp in milliseconds, a
    /// microtask checkpoint after each. Callbacks registered during the frame
    /// wait for the next one; cancelled callbacks are skipped.
    pub fn run_animation_frame(&self) {
        let timestamp = self.shared.borrow().clock.now().as_millis_f64();
        // The callbacks registered before now form this frame's batch; any a
        // callback registers during the frame append past this boundary and run
        // next frame. Iterating by index (not a snapshot) lets a running
        // callback cancel a later one in the same batch by clearing its closure.
        let count = self.shared.borrow().frame_callbacks.len();
        for index in 0..count {
            let run = self.shared.borrow_mut().frame_callbacks[index].run.take();
            let Some(run) = run else { continue };
            run(timestamp);
            self.runner.run_microtask_checkpoint();
        }
        self.shared.borrow_mut().frame_callbacks.drain(0..count);
    }

    /// Steps a virtual clock forward one 60 Hz frame, fires any timers that come
    /// due within it, then runs the rendering opportunity. The building block of
    /// a deterministic headless render loop.
    pub fn tick_frame(&self) {
        {
            let mut shared = self.shared.borrow_mut();
            shared.clock.advance(FRAME_INTERVAL);
        }
        self.fire_due_timers();
        self.run_animation_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collects the provenance every task ran under, in order.
    #[derive(Default)]
    struct OriginLog {
        finished: RefCell<Vec<Origin>>,
    }

    impl super::super::tasks::ProvenanceSink for OriginLog {
        fn task_queued(&self, _origin: Origin) {}
        fn task_finished(&self, origin: Origin) {
            self.finished.borrow_mut().push(origin);
        }
    }

    /// Collects both halves of the pending count, in order, so a test can see
    /// exactly when a timer starts and stops holding a settle open.
    #[derive(Default)]
    struct EventLog {
        events: RefCell<Vec<(&'static str, Origin)>>,
    }

    impl super::super::tasks::ProvenanceSink for EventLog {
        fn task_queued(&self, origin: Origin) {
            self.events.borrow_mut().push(("queued", origin));
        }
        fn task_finished(&self, origin: Origin) {
            self.events.borrow_mut().push(("finished", origin));
        }
    }

    /// Schedules one timer from a `Tracked` task and reports the provenance
    /// the firing ran under.
    fn timer_origin_for(repeating: bool, delay_ms: u64) -> Origin {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let scheduler = loop_.scheduler();
        let log = Rc::new(OriginLog::default());
        scheduler.sink.set_provenance_sink(log.clone());

        scheduler.with_origin(Origin::Tracked, || {
            if repeating {
                scheduler.set_interval(delay_ms, Box::new(|| {}));
            } else {
                scheduler.set_timeout(delay_ms, Box::new(|| {}));
            }
        });
        loop_.advance_to_next_timer();

        let finished = log.finished.borrow();
        *finished.first().expect("the timer fired")
    }

    #[test]
    fn a_tracked_timer_counts_from_the_moment_it_is_scheduled() {
        // The gap the count has to cover is the delay itself: while the timer
        // waits in the heap it is in neither task queue, and a settle poll in
        // that window must not see the page as done.
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let scheduler = loop_.scheduler();
        let log = Rc::new(EventLog::default());
        scheduler.sink.set_provenance_sink(log.clone());

        scheduler.with_origin(Origin::Tracked, || {
            scheduler.set_timeout(5, Box::new(|| {}));
        });
        assert_eq!(
            *log.events.borrow(),
            vec![("queued", Origin::Tracked)],
            "pending as soon as it is scheduled, before the clock moves"
        );

        loop_.advance_to_next_timer();
        assert_eq!(
            *log.events.borrow(),
            vec![("queued", Origin::Tracked), ("finished", Origin::Tracked)],
            "firing must not queue a second pending unit"
        );
    }

    #[test]
    fn cancelling_a_tracked_timer_balances_its_pending_count() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let scheduler = loop_.scheduler();
        let log = Rc::new(EventLog::default());
        scheduler.sink.set_provenance_sink(log.clone());

        let id = scheduler.with_origin(Origin::Tracked, || {
            scheduler.set_timeout(5, Box::new(|| {}))
        });
        scheduler.clear_timeout(id);

        assert_eq!(
            *log.events.borrow(),
            vec![("queued", Origin::Tracked), ("finished", Origin::Tracked)],
            "a cancelled deferral must not hold the settle open forever"
        );
    }

    #[test]
    fn a_background_timer_is_only_pending_once_it_fires() {
        // Background work cannot hold a settle open, so counting it early
        // would buy nothing and would misreport `background_tasks`.
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let scheduler = loop_.scheduler();
        let log = Rc::new(EventLog::default());
        scheduler.sink.set_provenance_sink(log.clone());

        scheduler.set_timeout(5, Box::new(|| {}));
        assert!(log.events.borrow().is_empty(), "nothing pending yet");

        loop_.advance_to_next_timer();
        assert_eq!(
            *log.events.borrow(),
            vec![
                ("queued", Origin::Background),
                ("finished", Origin::Background)
            ]
        );
    }

    #[test]
    fn a_panicking_timer_callback_does_not_leak_its_nesting_depth() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let scheduler = loop_.scheduler();
        scheduler.set_timeout(0, Box::new(|| panic!("handler blew up")));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop_.advance_to_next_timer();
        }));
        assert!(outcome.is_err(), "the panic propagated");
        assert_eq!(
            scheduler.current_nesting(),
            0,
            "a later timer must not inherit the panicking one's depth"
        );
    }

    #[test]
    fn a_short_one_shot_timer_from_tracked_work_stays_tracked() {
        assert_eq!(timer_origin_for(false, 0), Origin::Tracked);
        assert_eq!(timer_origin_for(false, 32), Origin::Tracked);
    }

    #[test]
    fn a_long_timer_or_an_interval_is_page_activity() {
        // Past `tracked_timer_max` a one-shot timer is no longer a deferral of
        // the load chain, and a repeating timer never is: both must be unable
        // to hold a settle open.
        assert_eq!(timer_origin_for(false, 33), Origin::Background);
        assert_eq!(timer_origin_for(true, 0), Origin::Background);
    }

    #[test]
    fn a_timer_scheduled_from_background_work_cannot_become_tracked() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let scheduler = loop_.scheduler();
        let log = Rc::new(OriginLog::default());
        scheduler.sink.set_provenance_sink(log.clone());

        // No `with_origin`: the top level is background.
        scheduler.set_timeout(0, Box::new(|| {}));
        loop_.advance_to_next_timer();

        assert_eq!(*log.finished.borrow(), vec![Origin::Background]);
    }

    #[test]
    fn a_chain_of_timers_falls_out_of_tracked_once_it_is_deep_enough() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let scheduler = loop_.scheduler();
        let log = Rc::new(OriginLog::default());
        scheduler.sink.set_provenance_sink(log.clone());

        // Each firing schedules the next, so the chain deepens by one level a
        // hop; the default rule allows depth 8.
        fn hop(scheduler: Scheduler, left: u32) {
            if left == 0 {
                return;
            }
            let next = scheduler.clone();
            scheduler.set_timeout(0, Box::new(move || hop(next, left - 1)));
        }
        scheduler.with_origin(Origin::Tracked, || hop(scheduler.clone(), 12));
        while loop_.advance_to_next_timer() {}

        let finished = log.finished.borrow();
        assert!(
            finished.len() >= 10,
            "the chain ran, got {} firings",
            finished.len()
        );
        assert_eq!(finished[0], Origin::Tracked, "the first hop is a deferral");
        assert_eq!(
            finished[7],
            Origin::Tracked,
            "depth 8 is the last tracked level"
        );
        assert_eq!(
            finished[8],
            Origin::Background,
            "past the depth limit the chain is page activity"
        );
    }

    type Log = Rc<RefCell<Vec<String>>>;

    fn new_log() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    fn push(log: &Log, entry: impl Into<String>) {
        log.borrow_mut().push(entry.into());
    }

    #[test]
    fn timers_fire_in_deadline_order_on_a_virtual_clock() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let log = new_log();

        {
            let log = log.clone();
            sched.set_timeout(50, Box::new(move || push(&log, "late")));
        }
        {
            let log = log.clone();
            sched.set_timeout(10, Box::new(move || push(&log, "early")));
        }

        loop_.run_timers_until_idle(100);
        assert_eq!(*log.borrow(), vec!["early", "late"]);
        assert!(!loop_.has_pending_timers());
    }

    #[test]
    fn a_cleared_timeout_never_runs() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let log = new_log();

        let keep = {
            let log = log.clone();
            sched.set_timeout(10, Box::new(move || push(&log, "kept")))
        };
        let drop = {
            let log = log.clone();
            sched.set_timeout(10, Box::new(move || push(&log, "dropped")))
        };
        let _ = keep;
        sched.clear_timeout(drop);

        loop_.run_timers_until_idle(100);
        assert_eq!(*log.borrow(), vec!["kept"]);
    }

    #[test]
    fn a_timer_that_schedules_a_timer_fires_it_after_the_clock_advances() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let log = new_log();

        let outer_sched = sched.clone();
        let outer_log = log.clone();
        sched.set_timeout(
            10,
            Box::new(move || {
                push(&outer_log, "outer");
                let inner_log = outer_log.clone();
                outer_sched.set_timeout(5, Box::new(move || push(&inner_log, "inner")));
            }),
        );

        loop_.run_timers_until_idle(100);
        // The inner timer is scheduled while the outer runs and fires once the
        // clock reaches its later deadline.
        assert_eq!(*log.borrow(), vec!["outer", "inner"]);
    }

    #[test]
    fn a_timer_callbacks_microtasks_run_before_the_next_timer() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let log = new_log();

        let s1 = sched.clone();
        let l1 = log.clone();
        sched.set_timeout(
            10,
            Box::new(move || {
                push(&l1, "timer-1");
                let l = l1.clone();
                s1.queue_microtask(move || push(&l, "micro-of-1"));
            }),
        );
        let l2 = log.clone();
        sched.set_timeout(20, Box::new(move || push(&l2, "timer-2")));

        loop_.run_timers_until_idle(100);
        assert_eq!(
            *log.borrow(),
            vec!["timer-1", "micro-of-1", "timer-2"],
            "the microtask checkpoint drains before the next timer task"
        );
    }

    #[test]
    fn animation_frame_callbacks_run_once_with_the_frame_timestamp() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let log = new_log();

        let log_cb = log.clone();
        sched.request_animation_frame(Box::new(move |ts| {
            push(&log_cb, format!("frame@{ts}"));
        }));

        loop_.tick_frame();
        // One frame at ~16.667 ms; the callback ran exactly once.
        assert_eq!(log.borrow().len(), 1);
        assert!(
            log.borrow()[0].starts_with("frame@16.66"),
            "got {:?}",
            log.borrow()[0]
        );

        // A second frame with no new registration does nothing.
        loop_.tick_frame();
        assert_eq!(log.borrow().len(), 1);
    }

    #[test]
    fn a_cancelled_animation_frame_does_not_run() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let log = new_log();

        let l = log.clone();
        sched.request_animation_frame(Box::new(move |_| push(&l, "kept")));
        let cancel = {
            let l = log.clone();
            sched.request_animation_frame(Box::new(move |_| push(&l, "cancelled")))
        };
        sched.cancel_animation_frame(cancel);

        loop_.tick_frame();
        assert_eq!(*log.borrow(), vec!["kept"]);
    }

    #[test]
    fn one_frame_callback_can_cancel_another_in_the_same_batch() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let log = new_log();

        // The `victim` id is captured so the first callback can cancel it before
        // it runs, within the one rendering opportunity.
        let victim: Rc<RefCell<Option<AnimationFrameId>>> = Rc::new(RefCell::new(None));

        let s = sched.clone();
        let v = victim.clone();
        let l = log.clone();
        sched.request_animation_frame(Box::new(move |_| {
            push(&l, "first");
            if let Some(id) = *v.borrow() {
                s.cancel_animation_frame(id);
            }
        }));
        let l2 = log.clone();
        let id = sched.request_animation_frame(Box::new(move |_| push(&l2, "second")));
        *victim.borrow_mut() = Some(id);

        loop_.tick_frame();
        // The second callback was cancelled by the first before it could run.
        assert_eq!(*log.borrow(), vec!["first"]);
        // No pending callbacks linger after the frame.
        loop_.tick_frame();
        assert_eq!(*log.borrow(), vec!["first"]);
    }

    #[test]
    fn cancelling_an_unknown_frame_id_is_a_harmless_no_op() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let log = new_log();

        // Cancelling ids that were never issued must not affect a real callback
        // and must not accumulate state.
        sched.cancel_animation_frame(AnimationFrameId(999));
        let l = log.clone();
        sched.request_animation_frame(Box::new(move |_| push(&l, "ran")));
        sched.cancel_animation_frame(AnimationFrameId(1000));

        loop_.tick_frame();
        assert_eq!(*log.borrow(), vec!["ran"]);
    }

    #[test]
    fn a_frame_callback_that_reschedules_runs_on_the_following_frame() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let count = Rc::new(RefCell::new(0u32));

        fn arm(sched: Scheduler, count: Rc<RefCell<u32>>) {
            let sched_inner = sched.clone();
            sched.request_animation_frame(Box::new(move |_| {
                *count.borrow_mut() += 1;
                if *count.borrow() < 3 {
                    arm(sched_inner.clone(), count.clone());
                }
            }));
        }
        arm(sched, count.clone());

        // Each frame runs one callback; the re-registration lands on the next.
        loop_.tick_frame();
        assert_eq!(*count.borrow(), 1);
        loop_.tick_frame();
        assert_eq!(*count.borrow(), 2);
        loop_.tick_frame();
        assert_eq!(*count.borrow(), 3);
        // Third callback did not re-arm, so a fourth frame is idle.
        loop_.tick_frame();
        assert_eq!(*count.borrow(), 3);
    }

    #[test]
    fn a_real_clock_never_auto_advances_timers() {
        let loop_ = EventLoop::new(Clock::real());
        let sched = loop_.scheduler();
        sched.set_timeout(3_600_000, Box::new(|| unreachable!("far-future timer")));
        // advance_to_next_timer only drives a virtual clock.
        assert!(!loop_.advance_to_next_timer());
        assert!(loop_.has_pending_timers());
    }

    #[test]
    fn an_interval_fires_once_per_period_until_cleared() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let count = Rc::new(RefCell::new(0u32));

        let id = {
            let count = count.clone();
            sched.set_interval(
                10,
                Box::new(move || {
                    *count.borrow_mut() += 1;
                }),
            )
        };

        // Five periods, one firing each — the id is re-armed by the loop.
        for _ in 0..5 {
            loop_.advance_to_next_timer();
        }
        assert_eq!(*count.borrow(), 5);

        // Cancelling under the same id stops re-arming, and no timer remains.
        sched.clear_interval(id);
        assert!(!loop_.has_pending_timers());
        for _ in 0..5 {
            loop_.advance_to_next_timer();
        }
        assert_eq!(*count.borrow(), 5);
    }

    #[test]
    fn a_zero_delay_interval_is_clamped_and_does_not_freeze_time() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let count = Rc::new(RefCell::new(0u32));
        {
            let count = count.clone();
            sched.set_interval(
                0,
                Box::new(move || {
                    *count.borrow_mut() += 1;
                }),
            );
        }

        // A zero-delay interval must not spin: the nesting clamp raises its
        // period to 4 ms once deeply nested, and the step guard bounds the run.
        loop_.run_timers_until_idle(20);
        assert_eq!(*count.borrow(), 20);
        assert!(
            loop_.now() > MonoTime::ZERO,
            "the clamp should have advanced the clock past the origin"
        );
    }

    #[test]
    fn clearing_one_interval_leaves_another_running() {
        let loop_ = EventLoop::new(Clock::virtual_at(MonoTime::ZERO));
        let sched = loop_.scheduler();
        let kept = Rc::new(RefCell::new(0u32));
        let dropped = Rc::new(RefCell::new(0u32));

        {
            let kept = kept.clone();
            sched.set_interval(
                10,
                Box::new(move || {
                    *kept.borrow_mut() += 1;
                }),
            );
        }
        let dropped_id = {
            let dropped = dropped.clone();
            sched.set_interval(
                10,
                Box::new(move || {
                    *dropped.borrow_mut() += 1;
                }),
            )
        };

        loop_.advance_to_next_timer();
        assert_eq!((*kept.borrow(), *dropped.borrow()), (1, 1));

        sched.clear_interval(dropped_id);
        for _ in 0..3 {
            loop_.advance_to_next_timer();
        }
        assert_eq!((*kept.borrow(), *dropped.borrow()), (4, 1));
    }
}
