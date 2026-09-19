//! Task and microtask queues with the HTML event-loop checkpoint discipline
//! (`docs/spec/05-javascript.md` §5).
//!
//! A page thread runs one task, then drains the microtask queue to empty (the
//! "perform a microtask checkpoint" step), then repeats. Callbacks running
//! inside a task need to enqueue further work, so the queues live behind a
//! shared [`TaskSink`] handle (single-threaded `Rc`/`RefCell`, matching the
//! `!Send` page-thread model) that a callback can clone and hold.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use tenta_core::Origin;

/// A unit of work run by the loop.
type Task = Box<dyn FnOnce()>;

/// Told about every tagged task as it is queued and as it finishes.
///
/// This is how the settle machine in `tenta-page` learns what the engine is
/// doing without `tenta-js` depending on it: the page installs an
/// implementation that forwards to `SettleTracker::note_task_queued` and
/// `note_task_finished` (`docs/spec/02-architecture.md` §5.1).
pub trait ProvenanceSink {
    fn task_queued(&self, origin: Origin);
    fn task_finished(&self, origin: Origin);
}

#[derive(Default)]
struct Queues {
    /// Task sources are collapsed into one FIFO for now; per-source selection
    /// (`docs/spec/05` §5) is layered on when the sources exist.
    macrotasks: VecDeque<(Origin, Task)>,
    microtasks: VecDeque<(Origin, Task)>,
}

struct Shared {
    queues: RefCell<Queues>,
    /// The provenance work queued right now inherits. The runner sets it to
    /// the tag of the task it is running, so a task and every microtask it
    /// queues carry that task's provenance onward; at the top level it is
    /// `Background`, because anything a live page starts on its own is
    /// background until something tracked claims it.
    current: Cell<Origin>,
    observer: RefCell<Option<Rc<dyn ProvenanceSink>>>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            queues: RefCell::new(Queues::default()),
            current: Cell::new(Origin::Background),
            observer: RefCell::new(None),
        }
    }
}

/// Restores the previous provenance when it goes out of scope, *including on
/// unwind*. A page thread catches panics and keeps running
/// (`tenta_page::catch_unwind_task`), so a tag leaked by a panicking task
/// would mislabel every task that followed it.
struct OriginGuard<'a> {
    shared: &'a Shared,
    previous: Origin,
}

impl Drop for OriginGuard<'_> {
    fn drop(&mut self) {
        self.shared.current.set(self.previous);
    }
}

/// Reports a unit of work finished when it goes out of scope, so a panicking
/// task cannot leave the settle machine counting work that will never end.
struct FinishGuard<'a> {
    sink: &'a TaskSink,
    origin: Origin,
}

impl Drop for FinishGuard<'_> {
    fn drop(&mut self) {
        self.sink.note_finished(self.origin);
    }
}

/// A cloneable handle for enqueuing tasks and microtasks, including from inside
/// a running callback.
#[derive(Clone, Default)]
pub struct TaskSink(Rc<Shared>);

impl TaskSink {
    /// Appends a task to the macrotask queue, inheriting the provenance of
    /// whatever is running.
    pub fn queue_task(&self, task: impl FnOnce() + 'static) {
        self.queue_task_from(self.current_origin().inherit(), task);
    }

    /// Appends a task with an explicit provenance. The page uses this for the
    /// work it starts itself - parser-driven script execution is `Tracked`,
    /// a socket message is `Background`.
    pub fn queue_task_from(&self, origin: Origin, task: impl FnOnce() + 'static) {
        self.queue_task_prearmed(origin, task);
        self.note_queued(origin);
    }

    /// Appends a task whose pending state was already reported through
    /// [`Self::note_queued`]. Running it still reports one `task_finished`, so
    /// the caller owns balancing that pair.
    ///
    /// A tracked one-shot timer was counted when it was scheduled, so its
    /// tracking task is queued this way: the task's `finished` is the release
    /// when the backend ran the callback inline. A backend that deferred the
    /// call instead re-announces the unit ([`Self::note_queued`]) from inside
    /// that task, so the release follows the call rather than the task.
    pub fn queue_task_prearmed(&self, origin: Origin, task: impl FnOnce() + 'static) {
        self.0
            .queues
            .borrow_mut()
            .macrotasks
            .push_back((origin, Box::new(task)));
    }

    /// Appends a microtask, run before the loop takes its next task
    /// (`queueMicrotask`, promise reactions).
    pub fn queue_microtask(&self, task: impl FnOnce() + 'static) {
        self.queue_microtask_from(self.current_origin().inherit(), task);
    }

    /// Appends a microtask with an explicit provenance.
    pub fn queue_microtask_from(&self, origin: Origin, task: impl FnOnce() + 'static) {
        self.0
            .queues
            .borrow_mut()
            .microtasks
            .push_back((origin, Box::new(task)));
        self.note_queued(origin);
    }

    /// The provenance newly queued work inherits.
    pub fn current_origin(&self) -> Origin {
        self.0.current.get()
    }

    /// Runs `body` with `origin` as the inherited provenance, restoring the
    /// previous one afterwards even if `body` queued work of its own.
    pub fn with_origin<R>(&self, origin: Origin, body: impl FnOnce() -> R) -> R {
        let _guard = OriginGuard {
            shared: &self.0,
            previous: self.0.current.replace(origin),
        };
        body()
    }

    /// Installs the observer that learns about every queued and finished task.
    pub fn set_provenance_sink(&self, sink: Rc<dyn ProvenanceSink>) {
        *self.0.observer.borrow_mut() = Some(sink);
    }

    /// The observer is read out before it is called so that an implementation
    /// re-entering the sink cannot find the `RefCell` still borrowed.
    fn observer(&self) -> Option<Rc<dyn ProvenanceSink>> {
        self.0.observer.borrow().clone()
    }

    /// Reports a unit of work pending without queuing anything.
    ///
    /// A timer is pending from the moment it is *scheduled*, not from the
    /// moment it fires: it waits in the timer heap, not in these queues. A
    /// tracked one-shot timer is a deferral of the load chain
    /// (`docs/spec/02-architecture.md` §5.1), so if it did not count while it
    /// waited, a settle poll in the gap would see zero tracked work and
    /// declare the page done with the continuation still to come.
    pub fn note_queued(&self, origin: Origin) {
        if let Some(observer) = self.observer() {
            observer.task_queued(origin);
        }
    }

    /// Balances a [`Self::note_queued`] for work that will never run - a
    /// cancelled timer.
    pub fn note_finished(&self, origin: Origin) {
        if let Some(observer) = self.observer() {
            observer.task_finished(origin);
        }
    }

    fn pop_macrotask(&self) -> Option<(Origin, Task)> {
        self.0.queues.borrow_mut().macrotasks.pop_front()
    }

    fn pop_microtask(&self) -> Option<(Origin, Task)> {
        self.0.queues.borrow_mut().microtasks.pop_front()
    }

    /// Runs one dequeued unit of work with its own provenance current, then
    /// reports it finished. The previous provenance is restored first so that
    /// the observer never sees the loop mid-task.
    fn run_tagged(&self, origin: Origin, task: Task) {
        // Declared first so it drops last: the provenance is restored before
        // the observer is told, and both happen even if `task` panics.
        let _finished = FinishGuard { sink: self, origin };
        self.with_origin(origin, task);
    }

    fn has_work(&self) -> bool {
        let queues = self.0.queues.borrow();
        !queues.macrotasks.is_empty() || !queues.microtasks.is_empty()
    }
}

/// The task-running half of the event loop: it owns nothing but the shared
/// [`TaskSink`] and drives the task-then-checkpoint cycle.
#[derive(Default)]
pub struct TaskRunner {
    sink: TaskSink,
}

impl TaskRunner {
    /// A new runner with empty queues.
    pub fn new() -> Self {
        Self::default()
    }

    /// A handle for enqueuing work, cloneable into callbacks.
    pub fn sink(&self) -> TaskSink {
        self.sink.clone()
    }

    /// Runs every queued microtask, including microtasks queued by microtasks,
    /// until the microtask queue is empty (a microtask checkpoint).
    pub fn run_microtask_checkpoint(&self) {
        // The borrow is dropped before each task runs, so a task may re-enter
        // the sink to enqueue more work.
        while let Some((origin, task)) = self.sink.pop_microtask() {
            self.sink.run_tagged(origin, task);
        }
    }

    /// Runs the next macrotask, if any, followed by a microtask checkpoint.
    /// Returns whether a macrotask ran.
    pub fn run_next_task(&self) -> bool {
        match self.sink.pop_macrotask() {
            Some((origin, task)) => {
                self.sink.run_tagged(origin, task);
                self.run_microtask_checkpoint();
                true
            }
            None => {
                // Even with no macrotask, drain any pending microtasks.
                self.run_microtask_checkpoint();
                false
            }
        }
    }

    /// Runs tasks and their checkpoints until both queues are empty.
    pub fn run_until_idle(&self) {
        self.run_microtask_checkpoint();
        while self.sink.has_work() {
            self.run_next_task();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    type Log = Rc<RefCell<Vec<&'static str>>>;

    fn log_to(log: &Log, entry: &'static str) {
        log.borrow_mut().push(entry);
    }

    #[test]
    fn microtasks_run_after_the_task_that_queued_them_before_the_next_task() {
        let runner = TaskRunner::new();
        let sink = runner.sink();
        let log: Log = Rc::new(RefCell::new(Vec::new()));

        {
            let log = log.clone();
            let sink_inner = sink.clone();
            sink.queue_task(move || {
                log_to(&log, "t1");
                {
                    let log = log.clone();
                    sink_inner.queue_microtask(move || log_to(&log, "m1"));
                }
                {
                    let log = log.clone();
                    sink_inner.queue_task(move || log_to(&log, "t2"));
                }
            });
        }
        {
            let log = log.clone();
            sink.queue_task(move || log_to(&log, "t3"));
        }

        runner.run_until_idle();

        // t1 runs, its microtask m1 runs at the checkpoint before any further
        // task, then the already-queued t3, then t2 (queued during t1).
        assert_eq!(*log.borrow(), vec!["t1", "m1", "t3", "t2"]);
    }

    #[test]
    fn a_microtask_that_queues_a_microtask_is_drained_in_the_same_checkpoint() {
        let runner = TaskRunner::new();
        let sink = runner.sink();
        let log: Log = Rc::new(RefCell::new(Vec::new()));

        let sink_inner = sink.clone();
        let log_outer = log.clone();
        sink.queue_task(move || {
            log_to(&log_outer, "task");
            let log = log_outer.clone();
            let sink_deeper = sink_inner.clone();
            sink_inner.queue_microtask(move || {
                log_to(&log, "micro-1");
                let log = log.clone();
                sink_deeper.queue_microtask(move || log_to(&log, "micro-2"));
            });
        });

        runner.run_next_task();
        // Both microtasks run in the one checkpoint, in order.
        assert_eq!(*log.borrow(), vec!["task", "micro-1", "micro-2"]);
    }

    /// Records every queued/finished pair so a test can assert the counts the
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

    #[test]
    fn work_queued_inside_a_task_inherits_its_provenance() {
        let runner = TaskRunner::new();
        let sink = runner.sink();
        let seen: Rc<RefCell<Vec<Origin>>> = Rc::new(RefCell::new(Vec::new()));

        let inner = sink.clone();
        let log = seen.clone();
        sink.queue_task_from(Origin::Tracked, move || {
            log.borrow_mut().push(inner.current_origin());
            let micro_sink = inner.clone();
            let micro_log = log.clone();
            inner.queue_microtask(move || {
                micro_log.borrow_mut().push(micro_sink.current_origin());
            });
            let task_sink = inner.clone();
            let task_log = log.clone();
            inner.queue_task(move || {
                task_log.borrow_mut().push(task_sink.current_origin());
            });
        });

        runner.run_until_idle();

        // The task, its microtask and the task it queued all read `Tracked`;
        // provenance is inherited, never promoted or reset.
        assert_eq!(
            *seen.borrow(),
            vec![Origin::Tracked, Origin::Tracked, Origin::Tracked]
        );
    }

    #[test]
    fn provenance_returns_to_background_after_a_tracked_task() {
        let runner = TaskRunner::new();
        let sink = runner.sink();
        assert_eq!(sink.current_origin(), Origin::Background);

        sink.queue_task_from(Origin::Tracked, || {});
        runner.run_until_idle();

        assert_eq!(
            sink.current_origin(),
            Origin::Background,
            "the runner restores the provenance it found"
        );
    }

    #[test]
    fn with_origin_scopes_the_tag_and_restores_it() {
        let runner = TaskRunner::new();
        let sink = runner.sink();
        let inside = sink.with_origin(Origin::Tracked, || sink.current_origin());
        assert_eq!(inside, Origin::Tracked);
        assert_eq!(sink.current_origin(), Origin::Background);
    }

    #[test]
    fn the_observer_sees_one_queued_and_one_finished_per_unit_of_work() {
        let runner = TaskRunner::new();
        let sink = runner.sink();
        let observer = Rc::new(RecordingSink::default());
        sink.set_provenance_sink(observer.clone());

        let inner = sink.clone();
        sink.queue_task_from(Origin::Tracked, move || {
            inner.queue_microtask(|| {});
        });
        sink.queue_task_from(Origin::Background, || {});
        runner.run_until_idle();

        assert_eq!(
            *observer.events.borrow(),
            vec![
                ("queued", Origin::Tracked),
                ("queued", Origin::Background),
                // The microtask is queued from inside the tracked task, so it
                // is tracked too and its `queued` lands before that task's
                // `finished`.
                ("queued", Origin::Tracked),
                ("finished", Origin::Tracked),
                ("finished", Origin::Tracked),
                ("finished", Origin::Background),
            ]
        );

        // The ordering above is what keeps the settle machine honest: replayed
        // as a running count, the tracked total never returns to zero between
        // the task and the microtask it queued, so a settle cannot slip
        // through the gap.
        let events = observer.events.borrow();
        let tracked_events: Vec<&str> = events
            .iter()
            .filter(|(_, origin)| *origin == Origin::Tracked)
            .map(|(kind, _)| *kind)
            .collect();
        let mut tracked = 0i32;
        let last = tracked_events.len() - 1;
        for (index, kind) in tracked_events.iter().enumerate() {
            tracked += if *kind == "queued" { 1 } else { -1 };
            assert!(
                tracked > 0 || index == last,
                "tracked work hit zero at step {index} with more still to come"
            );
        }
        assert_eq!(tracked, 0, "every tracked task is accounted for");
    }

    #[test]
    fn a_panicking_task_still_restores_provenance_and_reports_finished() {
        // A page thread catches panics and keeps running, so the loop must not
        // be left mislabelling every later task as tracked, nor leave the
        // settle machine counting work that will never end.
        let runner = TaskRunner::new();
        let sink = runner.sink();
        let observer = Rc::new(RecordingSink::default());
        sink.set_provenance_sink(observer.clone());

        sink.queue_task_from(Origin::Tracked, || panic!("callback blew up"));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runner.run_until_idle();
        }));
        assert!(outcome.is_err(), "the panic propagated to the caller");

        assert_eq!(
            sink.current_origin(),
            Origin::Background,
            "the provenance was restored on unwind"
        );
        assert_eq!(
            *observer.events.borrow(),
            vec![("queued", Origin::Tracked), ("finished", Origin::Tracked)],
            "the unit of work was still reported finished"
        );
    }

    #[test]
    fn run_next_task_reports_whether_a_task_ran() {
        let runner = TaskRunner::new();
        assert!(!runner.run_next_task(), "no task queued");
        let ran = Rc::new(RefCell::new(false));
        let flag = ran.clone();
        runner.sink().queue_task(move || *flag.borrow_mut() = true);
        assert!(runner.run_next_task(), "a task was queued");
        assert!(*ran.borrow());
        assert!(!runner.run_next_task(), "queue is empty again");
    }
}
