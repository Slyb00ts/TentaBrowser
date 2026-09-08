//! Browser and BrowserContext: cookies, storage, cache and Service Worker
//! isolation, page threads, isolate pool
//! (`docs/spec/02-architecture.md` §3, `docs/spec/11-library-api.md` §1).
//!
//! The engine owns the io runtime and the page-thread pool; a context owns the
//! state that must not leak between sessions (cookies, cache, session cache,
//! destination policy) and one client pinned to one io thread. Pages are
//! pinned to one page thread for their life.

pub mod context;
pub mod engine;
mod extensions;
mod hooks;
mod threads;

pub use context::{BrowserContext, ContextOptions, GroupId, PageOptions};
pub use engine::{
    Engine, EngineBuilder, EngineOptions, JsEngine, TrimLevel, TrimReport, default_page_threads,
};
pub use threads::PAGE_THREAD_STACK;

// Re-exported so a host does not have to depend on the internal crates.
pub use tenta_page::document::Document;
pub use tenta_page::fetch_policy::{FetchPolicy, HintDisposition, PreloadKind, RecordedHint};
pub use tenta_page::page::{Goto, Navigation, Page, Viewport};
pub use tenta_page::settle::{Settle, SettleConfig, SettleState};
pub use tenta_page::{PageEvent, PageEvents};
