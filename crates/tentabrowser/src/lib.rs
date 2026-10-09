//! TentaBrowser: a headless browser engine that pays only for what the task
//! needs (`docs/spec/02-architecture.md` §1).
//!
//! A page declares its tier and nothing above it is initialised. Tier 1 —
//! HTTP, a streaming HTML parse into an arena DOM and extraction — is what
//! this release implements; JavaScript, layout and media land in later
//! phases, and their feature flags are already named here so hosts can pin
//! them.
//!
//! ```no_run
//! use tentabrowser::{ContextOptions, Engine, Tier};
//!
//! # async fn example() -> tentabrowser::Result<()> {
//! let engine = Engine::builder()
//!     .max_tier(Tier::Dom)
//!     .budgets(tentabrowser::Budgets::default().page_memory_mb(256))
//!     .fs_policy(tentabrowser::FsPolicy::none())
//!     .build()?;
//!
//! let ctx = engine.new_context(ContextOptions::default())?;
//!
//! // Tier 1 only: no JavaScript, no render. Subresource hints are recorded
//! // and never fetched.
//! let doc = ctx.fetch("https://example.com").await?;
//! let text = doc.text();
//! let markdown = doc.markdown();
//! let links = doc.links();
//! println!("{} {} {} links", doc.status(), text.len(), links.len());
//!
//! engine.close();
//! # Ok(())
//! # }
//! ```
//!
//! A page, for callers that want the navigation and the settle in their own
//! hands:
//!
//! ```no_run
//! use tentabrowser::{ContextOptions, Engine, Goto, PageOptions, Settle};
//!
//! # async fn example() -> tentabrowser::Result<()> {
//! let engine = Engine::builder().build()?;
//! let ctx = engine.new_context(ContextOptions {
//!     allow_private_network: true,
//!     ..Default::default()
//! })?;
//! let page = ctx.new_page(PageOptions::tier1()).await?;
//! let mut events = page.events();
//! page.goto("http://127.0.0.1:8080/", Goto::default().wait_until(Settle::NetworkIdle))
//!     .await?;
//! while let Some(event) = events.try_next() {
//!     println!("{event:?}");
//! }
//! let title = page.title();
//! page.close().await?;
//! # let _ = title;
//! # Ok(())
//! # }
//! ```
//!
//! Handles ([`Engine`], [`BrowserContext`], [`Page`]) are `Clone + Send +
//! Sync`; the state lives on the page threads
//! (`docs/spec/11-library-api.md` §1). Every future is cancel-safe: dropping
//! one cancels the wait, never the page.

pub mod search;

pub use search::{
    ContextSearch, EngineSearch, SEARCH_PERSISTENCE_ID, SearchContextOptions, SearchSetup,
    SearchState,
};
pub use tenta_browser::{
    BrowserContext, ContextOptions, Engine, EngineBuilder, EngineOptions, JsEngine, PageOptions,
    TrimLevel, TrimReport, default_page_threads,
};
#[cfg(feature = "render")]
pub use tenta_browser::{
    Capture, Clip, ImageFormat, RasterStats, Screenshot, ScreenshotOptions, StageTimes,
    WorkCounters,
};
pub use tenta_core::{Budgets, DownloadPolicy, Error, FsPolicy, Result, Tier};
pub use tenta_net::Profile;
pub use tenta_net::profile::ClientProfile;
pub use tenta_page::document::Document;
pub use tenta_page::event::{PageEvent, PageEvents};
pub use tenta_page::fetch_policy::{FetchPolicy, HintDisposition, PreloadKind, RecordedHint};
pub use tenta_page::page::{Goto, Navigation, Page, Viewport};
pub use tenta_page::settle::{
    Origin, PendingSources, RequestClass, Settle, SettleConfig, SettleState,
};
pub use tenta_page::{
    BindingCall, CallArgument, Completion, ConsoleCall, EvalOptions, Evaluation, ExceptionDetails,
    ExceptionEvent, FromRemote, ObjectPreview, PropertyPreview, RemoteKind, RemoteObject,
    RemoteValue, StackFrame, World,
};
pub use tenta_search::{
    BlockClass, Freshness, Kind, Locale, Mode, Provider, ProviderId, ProviderMeta, SafeSearch,
    SearchError, SearchRequest, SearchResponse, SearchResult, WarmReport,
};

/// The search model and its plans, for hosts that build their own queries
/// (`docs/spec/08-search.md`).
pub mod search_model {
    pub use tenta_search::{Answer, Link, MAX_LIMIT, ParsePlan, PlanSet, Timing};
}

/// The DOM, for callers that walk it directly. `tentabrowser::Document` wraps
/// one of these together with the response it came from.
pub mod dom {
    pub use tenta_core::{NodeId, NodeRef};
    pub use tenta_dom::extract::{forms, links, main_content, markdown, metadata, snapshot, text};
    pub use tenta_dom::{Document, MemoryReport, NodeKind};
}

/// Network policy types a host configures or inspects
/// (`docs/spec/03-network.md`).
pub mod net {
    pub use tenta_net::hooks::{FulfilledResponse, Interception, Interceptor, Observer};
    pub use tenta_net::policy::cookies::{Cookie, CookieJar, CookiePolicy, Site, StorageState};
    pub use tenta_net::policy::error_names::ErrorName;
    pub use tenta_net::policy::private_network::DestinationPolicy;
    pub use tenta_net::policy::session_cache::SessionCache;
    pub use tenta_net::{Client, NetError, Protocol, Request, ResourceType, Response, Timing};

    /// `robots.txt` parsing and its per-origin cache, for the crawlers a host
    /// builds on this library (`docs/spec/20-platform-behaviour.md` §11).
    pub mod robots {
        pub use tenta_net::policy::robots::{
            RobotsCache, RobotsStatus, RobotsTxt, origin_of, robots_url,
        };
    }

    /// Per-host concurrency, spacing and `Retry-After`.
    pub mod rate_limit {
        pub use tenta_net::policy::rate_limit::{
            Acquire, HostLimiter, HostLimits, Permit, parse_retry_after,
        };
    }
}
