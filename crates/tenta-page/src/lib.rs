//! Page, navigation, lifecycle and settle (`docs/spec/02-architecture.md` §5).
//!
//! A page is pinned to one page thread for its life (§3). This crate owns the
//! page state and the work that runs on that thread; `tenta-browser` owns the
//! pool of threads and implements [`PageThread`].

pub mod clock;
mod cors;
pub mod document;
pub mod error_page;
pub mod event;
// Private: the `tenta-net` driver behind the page's `fetch()` is installed into
// the engine the page owns, not an API (`crates/tenta-page/src/fetch.rs`).
mod fetch;
pub mod fetch_policy;
pub mod network;
pub mod page;
// Private: the runtime is page-thread-local state the page owns, not an API
// (`crates/tenta-page/src/runtime.rs`).
mod runtime;
// Private: fetching and decoding the images of a render
// (`crates/tenta-page/src/images.rs`).
#[cfg(feature = "render")]
mod images;
// Private module: the render session is page-thread state the page owns. Its
// option and result types are re-exported below (`crates/tenta-page/src/render.rs`).
#[cfg(feature = "render")]
mod render;
// Private: the page's cumulative script budget is page-thread state the page
// owns, not an API (`crates/tenta-page/src/script_cpu.rs`).
mod script_cpu;
#[cfg(feature = "render")]
mod script_geometry;
pub mod settle;
// Private: stylesheet collection feeds the render session and is not an API
// (`crates/tenta-page/src/styles.rs`).
#[cfg(feature = "render")]
mod styles;
pub mod task;
pub mod thread;
// Private: the watchdog is process-wide machinery the page owns, not an API
// (`crates/tenta-page/src/watchdog.rs`).
mod watchdog;

pub use clock::{Clock, ManualClock, SystemClock};
pub use document::Document;
pub use event::{Commit, FrameState, LoadStage, PageEvent, PageEvents};
pub use fetch_policy::{FetchPolicy, HintDisposition, PreloadKind, RecordedHint};
#[cfg(feature = "render")]
pub use page::{BoxQuads, ElementGeometry, Quad};
pub use page::{
    EvalOptions, Evaluation, FromRemote, Goto, Navigation, Page, PageConfig, PendingNavigation,
    TestHooks, Viewport, World,
};
#[cfg(feature = "render")]
pub use render::{
    Capture, Clip, ContentSize, ImageFormat, RasterStats, Screenshot, ScreenshotOptions,
    StageTimes, WorkCounters,
};
pub use runtime::drop_thread_runtimes;
pub use settle::{
    Origin, PendingSources, RequestClass, Settle, SettleConfig, SettleState, SettleTracker,
};
pub use task::{catch_unwind_future, catch_unwind_task, panic_message};
pub use tenta_js::inspect::{
    BindingCall, CallArgument, Completion, ConsoleCall, ExceptionDetails, ExceptionEvent,
    ObjectPreview, PropertyDescriptor, PropertyPreview, PropertyQuery, RemoteKind, RemoteObject,
    RemoteValue, StackFrame,
};
pub use tenta_net::RedirectHop;
pub use thread::{PageJob, PageTask, PageThread};
