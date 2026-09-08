//! `search` on an [`Engine`] and on a [`BrowserContext`]
//! (`docs/spec/08-search.md` §3).
//!
//! `tenta-search` is built over a `tenta_net::Client`; this module is what
//! gives it a browser context to run in. Two entry points:
//!
//! - [`EngineSearch::search`] uses the engine's own search context, created
//!   on first use and kept for the engine's life, so a second query reuses
//!   its connections, its cookie jar and its response cache. The context is
//!   persistent under the id `search-warm` unless the engine is ephemeral.
//! - [`ContextSearch::search`] runs the query through a context the caller
//!   already has: same cookies, same proxy, same profile as that context's
//!   other traffic.
//!
//! ```no_run
//! use tentabrowser::{Engine, EngineSearch, Provider, SearchRequest};
//!
//! # async fn example() -> Result<(), tentabrowser::SearchError> {
//! let engine = Engine::builder().build().expect("engine");
//! let response = engine
//!     .search(SearchRequest::new("rust browser engine").provider(Provider::DuckDuckGo))
//!     .await?;
//! for result in &response.results {
//!     println!("{} {}", result.rank, result.url);
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tenta_browser::{BrowserContext, ContextOptions, Engine};
use tenta_core::Error;
use tenta_search::search::{Search, SearchOptions, WarmReport};
use tenta_search::{Provider, ProviderId, SearchError, SearchRequest, SearchResponse};
use url::Url;

/// The persistence id of the engine's internal search context
/// (`docs/spec/03-network.md` §4). An ephemeral engine ignores it.
pub const SEARCH_PERSISTENCE_ID: &str = "search-warm";

/// How a search subsystem is configured when it is created.
///
/// The engine builds its search context and plan set once, on the first
/// query, from the setup of that call; later calls reuse it. Callers that
/// need two different configurations at the same time use
/// [`ContextSearch::search_with`] on two contexts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchSetup {
    /// Providers the subsystem may query and the warm pool pre-connects to.
    /// Empty means every loaded plan.
    pub providers: Vec<Provider>,
    /// Replaces a plan's endpoint, by plan id: fixture replay and mirrors.
    pub endpoint_override: BTreeMap<ProviderId, Url>,
    /// Pre-connect to every enabled provider before the first query.
    pub warm_on_start: bool,
    /// Lifetime of a cached provider answer; `None` keeps the default.
    pub cache_ttl: Option<Duration>,
    /// Deadline for one provider request; `None` keeps the default.
    pub request_timeout: Option<Duration>,
    /// Options of the engine's internal search context. Ignored by
    /// [`ContextSearch`], which uses the caller's context as it is.
    pub context: SearchContextOptions,
}

impl SearchSetup {
    /// The setup a bare [`EngineSearch::search`] uses: the request's own
    /// providers, no overrides, a private-network refusing context.
    pub fn for_request(request: &SearchRequest) -> Self {
        Self {
            providers: request.providers.clone(),
            ..Self::default()
        }
    }

    pub fn providers(mut self, providers: impl IntoIterator<Item = Provider>) -> Self {
        self.providers = providers.into_iter().collect();
        self
    }

    /// Points one plan at another endpoint, which is how fixtures are
    /// replayed (`docs/spec/15-testing-and-benchmarks.md` §2).
    pub fn endpoint_override(mut self, plan_id: ProviderId, endpoint: Url) -> Self {
        self.endpoint_override.insert(plan_id, endpoint);
        self
    }

    fn options(&self) -> SearchOptions {
        let defaults = SearchOptions::default();
        // Every embedded plan is enabled up front (plans are cheap data), so
        // a query for a provider outside the caller's list never rebuilds the
        // `Search` and never loses its response cache or warm connections;
        // only the caller's providers are warmed.
        SearchOptions {
            providers_enabled: Vec::new(),
            warm_providers: self.providers.clone(),
            cache_ttl: self.cache_ttl.unwrap_or(defaults.cache_ttl),
            warm_on_start: self.warm_on_start,
            endpoint_override: self
                .endpoint_override
                .iter()
                .map(|(id, url)| (id.clone(), url.clone()))
                .collect(),
            request_timeout: self.request_timeout.or(defaults.request_timeout),
            user_agent: self.context.user_agent.clone(),
            accept_language: self.context.accept_language.clone(),
            ..defaults
        }
    }
}

/// The parts of [`ContextOptions`] the engine's internal search context takes
/// from its caller. The rest is fixed: its own cookie jar, its own cache and
/// the `search-warm` persistence id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchContextOptions {
    /// Needed to reach a fixture server on loopback
    /// (`docs/spec/19-security.md` §2).
    pub allow_private_network: bool,
    pub proxy: Option<String>,
    pub user_agent: Option<String>,
    pub accept_language: Option<String>,
}

impl SearchContextOptions {
    fn context_options(&self, persistence_id: Option<String>) -> ContextOptions {
        ContextOptions {
            proxy: self.proxy.clone(),
            allow_private_network: self.allow_private_network,
            persistence_id,
            user_agent: self.user_agent.clone(),
            accept_language: self.accept_language.clone(),
            ..ContextOptions::default()
        }
    }
}

/// The search subsystem attached to an engine or a context.
pub struct SearchState {
    context: BrowserContext,
    search: Mutex<Arc<Search>>,
    setup: Mutex<SearchSetup>,
}

impl std::fmt::Debug for SearchState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchState")
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

impl SearchState {
    fn new(context: BrowserContext, setup: SearchSetup) -> Result<Self, Error> {
        let search = build(&context, &setup)?;
        Ok(Self {
            context,
            search: Mutex::new(search),
            setup: Mutex::new(setup),
        })
    }

    /// The context the queries run in.
    pub fn context(&self) -> &BrowserContext {
        &self.context
    }

    /// A `Search` whose enabled providers cover `wanted`.
    ///
    /// The engine enables every embedded plan, so this returns the existing
    /// `Search` without a rebuild; the rebuild path remains for a `Search`
    /// built over an explicit allow-list (a `plans_dir` caller), where
    /// widening the list is less surprising than refusing a provider the
    /// caller just asked for.
    fn search_for(&self, wanted: &[Provider]) -> Result<Arc<Search>, SearchError> {
        {
            let search = self.search.lock();
            let enabled = &search.options().providers_enabled;
            if enabled.is_empty() || wanted.iter().all(|p| enabled.contains(p)) {
                return Ok(Arc::clone(&search));
            }
        }
        let mut setup = self.setup.lock();
        for provider in wanted {
            if !setup.providers.contains(provider) {
                setup.providers.push(provider.clone());
            }
        }
        let rebuilt = build(&self.context, &setup).map_err(to_search_error)?;
        let mut search = self.search.lock();
        *search = Arc::clone(&rebuilt);
        Ok(rebuilt)
    }

    /// Runs one query.
    pub async fn query(&self, request: SearchRequest) -> Result<SearchResponse, SearchError> {
        let search = self.search_for(&request.providers)?;
        search.query(request).await
    }

    /// Resolves and connects to every enabled provider
    /// (`docs/spec/08-search.md` §3.1).
    pub async fn warm(&self) -> WarmReport {
        let search = { Arc::clone(&self.search.lock()) };
        search.warm().await
    }
}

fn build(context: &BrowserContext, setup: &SearchSetup) -> Result<Arc<Search>, Error> {
    Search::new(context.client().clone(), setup.options())
        .map(Arc::new)
        .map_err(|error| Error::Other {
            code: "search_init".to_owned(),
            message: error.to_string(),
        })
}

fn to_search_error(error: Error) -> SearchError {
    match error {
        Error::Net { code, message } => SearchError::Net(tenta_net::NetError::new(
            tenta_net::policy::error_names::ErrorName::parse(&code)
                .unwrap_or(tenta_net::policy::error_names::ErrorName::Failed),
            message,
        )),
        other => SearchError::InvalidRequest(other.to_string()),
    }
}

/// The engine's internal search context.
///
/// It is persistent under [`SEARCH_PERSISTENCE_ID`] so a cold process reaches
/// a provider in one RTT (`docs/spec/03-network.md` §4). Persistence needs a
/// session-cache directory inside the engine's [`tentabrowser::FsPolicy`],
/// and the default policy is empty (`docs/spec/19-security.md` §1); a host
/// that has not opened one gets an ephemeral search context rather than a
/// failed query, because the session cache is a latency optimisation and
/// never a requirement.
fn search_context(
    engine: &Engine,
    options: &SearchContextOptions,
) -> Result<BrowserContext, Error> {
    if !engine.options().ephemeral {
        let persistent =
            engine.new_context(options.context_options(Some(SEARCH_PERSISTENCE_ID.to_owned())));
        match persistent {
            Ok(context) => return Ok(context),
            Err(Error::FsDenied { path }) => {
                tracing::debug!(
                    path = %path.display(),
                    "no writable session cache; the search context is ephemeral"
                );
            }
            Err(error) => return Err(error),
        }
    }
    engine.new_context(options.context_options(None))
}

/// `search` on the engine's own context.
pub trait EngineSearch {
    /// Runs `request` through the engine's internal search context, creating
    /// it on the first call.
    #[allow(
        async_fn_in_trait,
        reason = "the trait is an inherent-method extension, never a bound; \
                  no caller needs the future to be Send"
    )]
    async fn search(&self, request: SearchRequest) -> Result<SearchResponse, SearchError>;

    /// Runs `request` with an explicit setup. The setup of the first call is
    /// the one the engine's search context and provider allow-list are built
    /// from.
    #[allow(async_fn_in_trait, reason = "see EngineSearch::search")]
    async fn search_with(
        &self,
        setup: &SearchSetup,
        request: SearchRequest,
    ) -> Result<SearchResponse, SearchError>;

    /// Pre-connects to every provider in `setup`, so the first query costs
    /// one request on an open stream (`docs/spec/13-performance.md` §2).
    #[allow(async_fn_in_trait, reason = "see EngineSearch::search")]
    async fn warm_search(&self, setup: &SearchSetup) -> Result<WarmReport, SearchError>;

    /// The search subsystem, created if this is the first use.
    fn search_state(&self, setup: &SearchSetup) -> Result<Arc<SearchState>, Error>;
}

impl EngineSearch for Engine {
    async fn search(&self, request: SearchRequest) -> Result<SearchResponse, SearchError> {
        let setup = SearchSetup::for_request(&request);
        self.search_with(&setup, request).await
    }

    async fn search_with(
        &self,
        setup: &SearchSetup,
        request: SearchRequest,
    ) -> Result<SearchResponse, SearchError> {
        let state = self.search_state(setup).map_err(to_search_error)?;
        state.query(request).await
    }

    async fn warm_search(&self, setup: &SearchSetup) -> Result<WarmReport, SearchError> {
        let state = self.search_state(setup).map_err(to_search_error)?;
        Ok(state.warm().await)
    }

    fn search_state(&self, setup: &SearchSetup) -> Result<Arc<SearchState>, Error> {
        self.extension(|engine| {
            let context = search_context(engine, &setup.context)?;
            SearchState::new(context, setup.clone())
        })
    }
}

/// `search` through a context the caller owns.
pub trait ContextSearch {
    /// Runs `request` through this context.
    #[allow(async_fn_in_trait, reason = "see EngineSearch::search")]
    async fn search(&self, request: SearchRequest) -> Result<SearchResponse, SearchError>;

    /// Runs `request` through this context with an explicit setup. The setup
    /// of the first call builds this context's plan set and allow-list;
    /// `SearchSetup::context` is ignored, because the context already exists.
    #[allow(async_fn_in_trait, reason = "see EngineSearch::search")]
    async fn search_with(
        &self,
        setup: &SearchSetup,
        request: SearchRequest,
    ) -> Result<SearchResponse, SearchError>;

    /// Pre-connects to every provider in `setup` on this context.
    #[allow(async_fn_in_trait, reason = "see EngineSearch::search")]
    async fn warm_search(&self, setup: &SearchSetup) -> Result<WarmReport, SearchError>;

    /// The search subsystem of this context, created if this is the first use.
    fn search_state(&self, setup: &SearchSetup) -> Result<Arc<SearchState>, Error>;
}

impl ContextSearch for BrowserContext {
    async fn search(&self, request: SearchRequest) -> Result<SearchResponse, SearchError> {
        let setup = SearchSetup::for_request(&request);
        self.search_with(&setup, request).await
    }

    async fn search_with(
        &self,
        setup: &SearchSetup,
        request: SearchRequest,
    ) -> Result<SearchResponse, SearchError> {
        let state = self.search_state(setup).map_err(to_search_error)?;
        state.query(request).await
    }

    async fn warm_search(&self, setup: &SearchSetup) -> Result<WarmReport, SearchError> {
        let state = self.search_state(setup).map_err(to_search_error)?;
        Ok(state.warm().await)
    }

    fn search_state(&self, setup: &SearchSetup) -> Result<Arc<SearchState>, Error> {
        self.extension(|context| SearchState::new(context.clone(), setup.clone()))
    }
}
