//! Search over the providers' public pages (`docs/spec/08-search.md`).
//!
//! `search` is what a person does with a browser: open the provider's public
//! page, submit the query, read the results. No vendor search API is ever
//! called, keyed or billed (O-7); `Mode::Fast` is a shortcut to the same
//! public page without running its scripts, and it exists only where the
//! provider serves a complete SERP that way.
//!
//! ```no_run
//! use tenta_search::{Provider, Search, SearchOptions, SearchRequest};
//!
//! # async fn run(client: tenta_net::Client) -> Result<(), tenta_search::SearchError> {
//! let search = Search::new(client, SearchOptions::default())?;
//! search.warm().await;
//! let response = search
//!     .query(
//!         SearchRequest::new("rust browser engine")
//!             .providers([Provider::DuckDuckGo, Provider::Bing])
//!             .limit(10),
//!     )
//!     .await?;
//! print!("{}", response.to_json_lines());
//! # Ok(())
//! # }
//! ```
//!
//! What is here at P1: the fast path for DuckDuckGo (HTML and Lite), Bing and
//! Brave, the plan loader and validator, block detection with the ladder
//! steps that do not need JavaScript, the response cache with coalescing, the
//! fan-out merge and the warm pool. What is not: `Mode::Full`, and therefore
//! Google, which has required JavaScript since 15 January 2025 and answers
//! `SearchError::Unsupported { provider: Google, mode: Full }` until P2 lands
//! the Tier 2 page load and the `human` ladder step.
//!
//! Timing (`docs/spec/15-testing-and-benchmarks.md` §2, `S-search-fast`):
//! `Timing::total` is the wall time of `Search::query`, `network` is the time
//! the query spent parked on the transport (from `send()` to the response
//! head, plus every wait for a body chunk), and
//! `engine_overhead = total - network` is the number R-06 budgets at 5 ms
//! p50. A cache hit contributes no network time, so its overhead is its whole
//! wall time. In a fan-out the network time is the slowest provider's, not
//! the sum, because the providers run concurrently.

pub mod cache;
pub mod error;
pub mod extract;
pub mod fetch;
pub mod model;
pub mod plan;
pub mod rank;
pub mod search;
mod util;

pub use cache::{CacheStats, DEFAULT_TTL, cache_key, normalise_query};
pub use error::{BlockClass, PlanError, SearchError};
pub use extract::Extraction;
pub use model::{
    Answer, Freshness, Kind, LanguageTag, Link, Locale, MAX_LIMIT, Mode, Provider, ProviderId,
    ProviderMeta, Region, SafeSearch, SearchRequest, SearchResponse, SearchResult, Timing,
};
pub use plan::{ParsePlan, PlanPath, PlanSet, PreparedRequest};
pub use rank::{RRF_K, normalise_url};
pub use search::{Search, SearchOptions, WarmReport};
