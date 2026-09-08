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

/// A timer's callback plus the nesting level to install while it runs, so a
/// timer it schedules is clamped one level deeper.
struct TimerCallback {
    nesting: u32,
    run: Box<dyn FnOnce()>,
}

/// A pending animation-frame callback. It receives the frame timestamp in
/// milliseconds, as `requestAnimationFrame` callbacks do. Once run or cancelled
/// its closure is taken, leaving a tombstone that the current rendering
/// opportunity drops when it finishes.
struct FrameCallback {
    id: AnimationFrameId,
    run: Option<Box<dyn FnOnce(f64)>>,
}

/// The mutable state shared between the scheduler handle and the driver.
struct Shared {
    clock: Clock,
    timers: TimerQueue,
    timer_callbacks: HashMap<TimerId, TimerCallback>,
    /// Nesting level of the timer callback currently running; 0 at top level.
    current_nesting: u32,
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
        let mut shared = self.shared.borrow_mut();
        let nesting = shared.current_nesting + 1;
        let now = shared.clock.now();
        let id = shared.timers.schedule(now, delay_ms, nesting);
        shared.timer_callbacks.insert(
            id,
            TimerCallback {
                nesting,
                run: callback,
            },
        );
        id
    }

    /// Cancels a scheduled timer (`clearTimeout`).
    pub fn clear_timeout(&self, id: TimerId) {
        let mut shared = self.shared.borrow_mut();
        shared.timers.cancel(id);
        shared.timer_callbacks.remove(&id);
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

    /// Fires every timer due at the current clock time, running each callback as
    /// a task followed by a microtask checkpoint, in deadline-then-insertion
    /// order. Timers scheduled by these callbacks do not fire until the clock
    /// advances again.
    pub fn fire_due_timers(&self) {
        let due = {
            let mut shared = self.shared.borrow_mut();
            let now = shared.clock.now();
            shared.timers.take_expired(now)
        };
        for id in due {
            let callback = self.shared.borrow_mut().timer_callbacks.remove(&id);
            let Some(callback) = callback else { continue };
            let shared = self.shared.clone();
            self.scheduler.sink.queue_task(move || {
                // Install this timer's nesting level so a timer it schedules is
                // clamped one level deeper, then restore the top level.
                shared.borrow_mut().current_nesting = callback.nesting;
                (callback.run)();
                shared.borrow_mut().current_nesting = 0;
            });
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
}
