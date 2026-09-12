//! The engine-independent HTML event loop (`docs/spec/05-javascript.md` §5).
//!
//! One implementation serves both engines. This module holds the parts that do
//! not touch V8 or QuickJS: the clock (real or virtual), the timer heap with
//! Chromium's nesting clamp, and — as they land — the task-source queues and
//! the microtask checkpoint. The engine backends drive it and run the callbacks
//! it schedules.

pub mod driver;
pub mod tasks;
pub mod time;
pub mod timers;

pub use driver::{AnimationFrameId, EventLoop, FRAME_INTERVAL, Scheduler};
pub use tasks::{ProvenanceSink, TaskRunner, TaskSink};
pub use time::{Clock, MonoTime, SharedClock};
pub use timers::{TimerId, TimerQueue};
