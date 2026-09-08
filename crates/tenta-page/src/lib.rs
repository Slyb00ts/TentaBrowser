//! Page, navigation, lifecycle and settle (`docs/spec/02-architecture.md` §5).
//!
//! A page is pinned to one page thread for its life (§3). This crate owns the
//! page state and the work that runs on that thread; `tenta-browser` owns the
//! pool of threads and implements [`PageThread`].

pub mod clock;
pub mod document;
pub mod error_page;
pub mod event;
pub mod fetch_policy;
pub mod page;
pub mod settle;
pub mod task;
pub mod thread;

pub use clock::{Clock, ManualClock, SystemClock};
pub use document::Document;
pub use event::{PageEvent, PageEvents};
pub use fetch_policy::{FetchPolicy, HintDisposition, PreloadKind, RecordedHint};
pub use page::{Goto, Navigation, Page, PageConfig, TestHooks, Viewport};
pub use settle::{
    Origin, PendingSources, RequestClass, Settle, SettleConfig, SettleState, SettleTracker,
};
pub use task::{catch_unwind_future, catch_unwind_task, panic_message};
pub use tenta_net::RedirectHop;
pub use thread::{PageJob, PageTask, PageThread};
