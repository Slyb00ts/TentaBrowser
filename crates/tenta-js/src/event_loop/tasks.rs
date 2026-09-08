//! Task and microtask queues with the HTML event-loop checkpoint discipline
//! (`docs/spec/05-javascript.md` §5).
//!
//! A page thread runs one task, then drains the microtask queue to empty (the
//! "perform a microtask checkpoint" step), then repeats. Callbacks running
//! inside a task need to enqueue further work, so the queues live behind a
//! shared [`TaskSink`] handle (single-threaded `Rc`/`RefCell`, matching the
//! `!Send` page-thread model) that a callback can clone and hold.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// A unit of work run by the loop.
type Task = Box<dyn FnOnce()>;

#[derive(Default)]
struct Queues {
    /// Task sources are collapsed into one FIFO for now; per-source selection
    /// (`docs/spec/05` §5) is layered on when the sources exist.
    macrotasks: VecDeque<Task>,
    microtasks: VecDeque<Task>,
}

/// A cloneable handle for enqueuing tasks and microtasks, including from inside
/// a running callback.
#[derive(Clone, Default)]
pub struct TaskSink(Rc<RefCell<Queues>>);

impl TaskSink {
    /// Appends a task to the macrotask queue.
    pub fn queue_task(&self, task: impl FnOnce() + 'static) {
        self.0.borrow_mut().macrotasks.push_back(Box::new(task));
    }

    /// Appends a microtask, run before the loop takes its next task
    /// (`queueMicrotask`, promise reactions).
    pub fn queue_microtask(&self, task: impl FnOnce() + 'static) {
        self.0.borrow_mut().microtasks.push_back(Box::new(task));
    }

    fn pop_macrotask(&self) -> Option<Task> {
        self.0.borrow_mut().macrotasks.pop_front()
    }

    fn pop_microtask(&self) -> Option<Task> {
        self.0.borrow_mut().microtasks.pop_front()
    }

    fn has_work(&self) -> bool {
        let queues = self.0.borrow();
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
        while let Some(task) = self.sink.pop_microtask() {
            task();
        }
    }

    /// Runs the next macrotask, if any, followed by a microtask checkpoint.
    /// Returns whether a macrotask ran.
    pub fn run_next_task(&self) -> bool {
        match self.sink.pop_macrotask() {
            Some(task) => {
                task();
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
