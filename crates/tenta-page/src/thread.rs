//! The page thread as this crate sees it (`docs/spec/02-architecture.md` §3).
//!
//! A page is pinned to one page thread for its life. This crate does not own
//! the threads — `tenta-browser` runs the pool — so it queues work through
//! this trait. Every task is wrapped in `catch_unwind` by the pool (§8), so a
//! panic in one page never reaches another.

use std::future::Future;
use std::pin::Pin;

use tenta_core::Result;

/// A unit of synchronous work on a page thread.
pub type PageTask = Box<dyn FnOnce() + Send + 'static>;

/// A unit of asynchronous work on a page thread.
///
/// The closure runs on the page thread and builds the future there, so the
/// future itself never has to be `Send`: it can hold the page's `Rc`s, its
/// parser and its DOM.
pub type PageJob = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send + 'static>;

/// A handle to the one page thread a page lives on.
pub trait PageThread: Send + Sync + 'static {
    /// Queues synchronous work. Fails with [`tenta_core::Error::Closed`] when
    /// the thread is shutting down.
    fn post_task(&self, task: PageTask) -> Result<()>;

    /// Queues asynchronous work, which the thread drives to completion on its
    /// own local task set.
    fn post_job(&self, job: PageJob) -> Result<()>;

    /// Index of the thread in the pool; pages of one group share it.
    fn index(&self) -> usize;
}
