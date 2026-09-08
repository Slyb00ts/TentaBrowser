//! `Search`: the warm pool, the response cache, the escalation ladder and the
//! fan-out merge (`docs/spec/08-search.md` §3, §4).
//!
//! At P1 a `Search` is built over a `tenta_net::Client` the caller hands in.
//! The spec's internal BrowserContext arrives with the engine assembly; the
//! contract here (warm pool, cache, ladder, merge) does not change when it
//! does, only where the client comes from.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tenta_net::proxy::Proxy;
use tenta_net::{Client, NetError};
use url::Url;

use crate::cache::{CacheStats, DEFAULT_CAPACITY, DEFAULT_TTL, ResponseCache, cache_key};
use crate::error::{BlockClass, PlanError, SearchError};
use crate::extract::{self, PATTERN_SAMPLE_BYTES};
use crate::fetch::{self, NetTiming};
use crate::model::{
    Answer, Mode, Provider, ProviderId, ProviderMeta, SearchRequest, SearchResponse, SearchResult,
    Timing,
};
use crate::plan::{ParsePlan, PlanPath, PlanSet};
use crate::rank::{self, Ranked};
use crate::util::join_all;

/// How a `Search` is configured (`docs/spec/08-search.md` §3).
#[derive(Clone, Debug)]
pub struct SearchOptions {
    /// Providers this `Search` may query. Empty means every loaded plan.
    pub providers_enabled: Vec<Provider>,
    /// Providers [`Search::warm`] opens connections to. Empty means every
    /// enabled provider; the engine narrows it to the providers the caller
    /// named so an allow-all `Search` does not warm hosts nobody asked for.
    pub warm_providers: Vec<Provider>,
    /// Lifetime of a cached provider answer.
    pub cache_ttl: Duration,
    /// Cached answers kept before the oldest is evicted.
    pub cache_capacity: usize,
    /// Exit IPs the ladder rotates through when a provider rate-limits or
    /// challenges this one (`08` §4, step 3).
    pub proxies: Vec<Proxy>,
    /// Resolve DNS and open the connections to every enabled provider before
    /// the first query runs.
    ///
    /// Off by default: with `providers_enabled` empty it would open
    /// connections to providers the caller never asked for. The engine turns
    /// it on together with the provider list, and a library caller can call
    /// [`Search::warm`] whenever it likes.
    pub warm_on_start: bool,
    /// Replaces a plan's endpoint, by plan id. Fixture replay and mirrors.
    pub endpoint_override: HashMap<ProviderId, Url>,
    /// Extra plans loaded at runtime, on top of the embedded ones.
    pub plans_dir: Option<PathBuf>,
    /// Deadline for one provider request.
    pub request_timeout: Option<Duration>,
    /// User-Agent override for search requests. If set, overrides the profile's UA.
    pub user_agent: Option<String>,
    /// Accept-Language override for search requests.
    pub accept_language: Option<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            providers_enabled: Vec::new(),
            warm_providers: Vec::new(),
            cache_ttl: DEFAULT_TTL,
            cache_capacity: DEFAULT_CAPACITY,
            proxies: Vec::new(),
            warm_on_start: false,
            endpoint_override: HashMap::new(),
            plans_dir: None,
            request_timeout: Some(Duration::from_secs(10)),
            user_agent: None,
            accept_language: None,
        }
    }
}

/// What `Search::warm` managed to open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WarmReport {
    /// Providers whose endpoint host is resolved and connected.
    pub warmed: Vec<Provider>,
    /// Providers that could not be reached, with the transport's reason.
    pub failed: Vec<(Provider, String)>,
}

impl WarmReport {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// One provider's answer, as it is cached.
#[derive(Clone, Debug)]
struct ProviderOutcome {
    endpoint: Url,
    plan_version: u32,
    mode_used: Mode,
    results: Vec<SearchResult>,
    related: Vec<String>,
    answer: Option<Answer>,
    timing: NetTiming,
}

/// The search subsystem: plans, warm pool, cache and ladder.
pub struct Search {
    client: Client,
    plans: PlanSet,
    options: SearchOptions,
    cache: ResponseCache<ProviderOutcome>,
    warmed: tokio::sync::OnceCell<WarmReport>,
}

impl std::fmt::Debug for Search {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Search")
            .field("plans", &self.plans.len())
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl Search {
    /// Builds a `Search` over `client`. Plans are the embedded ones plus
    /// anything in `options.plans_dir`.
    pub fn new(client: Client, options: SearchOptions) -> Result<Self, SearchError> {
        let mut plans = PlanSet::embedded()?;
        if let Some(directory) = &options.plans_dir {
            plans = plans.with_dir(directory)?;
        }
        for provider in &options.providers_enabled {
            if plans.get(provider.plan_id()).is_none() {
                return Err(SearchError::Plan(PlanError::new(
                    provider.plan_id(),
                    "enabled in SearchOptions but no such plan is loaded",
                )));
            }
        }
        let cache = ResponseCache::new(options.cache_ttl, options.cache_capacity);
        Ok(Self {
            client,
            plans,
            options,
            cache,
            warmed: tokio::sync::OnceCell::new(),
        })
    }

    pub fn plans(&self) -> &PlanSet {
        &self.plans
    }

    pub fn options(&self) -> &SearchOptions {
        &self.options
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Providers this `Search` will query, in plan order.
    pub fn providers(&self) -> Vec<Provider> {
        if !self.options.providers_enabled.is_empty() {
            return self.options.providers_enabled.clone();
        }
        self.plans
            .ids()
            .filter_map(|id| Provider::from_plan_id(id).ok())
            .collect()
    }

    /// Providers [`Search::warm`] targets: `warm_providers`, or every
    /// enabled provider when that list is empty.
    pub fn warm_providers(&self) -> Vec<Provider> {
        if !self.options.warm_providers.is_empty() {
            return self.options.warm_providers.clone();
        }
        self.providers()
    }

    /// Resolves DNS and opens a connection to every warm provider's
    /// endpoint, so a query costs one request on an existing stream
    /// (`docs/spec/08-search.md` §3.1).
    pub async fn warm(&self) -> WarmReport {
        let targets: Vec<(Provider, Url)> = self
            .warm_providers()
            .into_iter()
            .filter_map(|provider| {
                let plan = self.plans.get(provider.plan_id())?;
                // Only the fast path has an endpoint worth holding open, and
                // a detection-only plan (`[expect] blocked`) is not a
                // provider anyone queries.
                let warmable = plan.path == PlanPath::Fast && !plan.expect_blocked;
                warmable.then(|| (provider.clone(), self.endpoint(plan)))
            })
            .collect();

        let futures: Vec<_> = targets
            .iter()
            .map(|(provider, endpoint)| async move {
                if let Some(host) = endpoint.host_str() {
                    // Resolution is cached by the resolver; failure here is
                    // reported by the connect below.
                    let _ = self.client.resolver().resolve(host).await;
                }
                (
                    provider.clone(),
                    self.client.preconnect(endpoint.as_str()).await,
                )
            })
            .collect();

        let mut report = WarmReport::default();
        for (provider, outcome) in join_all(futures).await {
            match outcome {
                Ok(()) => report.warmed.push(provider),
                Err(error) => {
                    tracing::debug!(%provider, %error, "search warm pool: preconnect failed");
                    report.failed.push((provider, error.to_string()));
                }
            }
        }
        report
    }

    /// Runs `request` (`docs/spec/08-search.md` §3).
    pub async fn query(&self, request: SearchRequest) -> Result<SearchResponse, SearchError> {
        request.validate()?;
        let started = Instant::now();
        if self.options.warm_on_start {
            self.warmed.get_or_init(|| self.warm()).await;
        }

        let mut targets: Vec<(usize, Provider)> = Vec::with_capacity(request.providers.len());
        let mut disabled: Vec<Provider> = Vec::new();
        for (index, provider) in request.providers.iter().enumerate() {
            if self.is_enabled(provider) {
                targets.push((index, provider.clone()));
            } else {
                disabled.push(provider.clone());
            }
        }
        if targets.is_empty() {
            return Err(SearchError::NoProvider(format!(
                "{} not enabled in SearchOptions::providers_enabled",
                disabled
                    .iter()
                    .map(Provider::name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        let futures: Vec<_> = targets
            .iter()
            .map(|(_, provider)| self.provider_query(provider, &request))
            .collect();
        let outcomes = join_all(futures).await;

        let mut ranked: Vec<Ranked> = Vec::new();
        let mut providers: Vec<ProviderMeta> = Vec::new();
        let mut related: Vec<String> = Vec::new();
        let mut answer: Option<Answer> = None;
        let mut critical: Option<NetTiming> = None;
        let mut first_error: Option<SearchError> = None;

        for ((index, provider), outcome) in targets.into_iter().zip(outcomes) {
            match outcome {
                Ok((outcome, cache_hit)) => {
                    providers.push(ProviderMeta {
                        provider: provider.clone(),
                        endpoint: outcome.endpoint.clone(),
                        plan_version: outcome.plan_version,
                        mode_used: outcome.mode_used,
                        cache_hit,
                        results: outcome.results.len() as u32,
                        error: None,
                    });
                    // A cache hit cost no network time, so it must not be
                    // subtracted from the query's wall time.
                    let timing = if cache_hit {
                        NetTiming::default()
                    } else {
                        outcome.timing
                    };
                    if critical.is_none_or(|current| current.network < timing.network) {
                        critical = Some(timing);
                    }
                    for candidate in &outcome.related {
                        if !related.iter().any(|known| known == candidate) {
                            related.push(candidate.clone());
                        }
                    }
                    if answer.is_none() {
                        answer = outcome.answer.clone();
                    }
                    ranked.push(Ranked {
                        provider_index: index,
                        results: outcome.results.clone(),
                    });
                }
                Err(error) => {
                    tracing::debug!(%provider, %error, "provider failed");
                    if let Some(plan) = self.plans.get(provider.plan_id()) {
                        providers.push(ProviderMeta {
                            provider: provider.clone(),
                            endpoint: self.endpoint(plan),
                            plan_version: plan.version,
                            mode_used: match plan.path {
                                PlanPath::Fast => Mode::Fast,
                                PlanPath::Full => Mode::Full,
                            },
                            cache_hit: false,
                            results: 0,
                            error: Some(error.to_string()),
                        });
                    }
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        if ranked.is_empty() {
            return Err(first_error.unwrap_or_else(|| {
                SearchError::NoProvider("no provider produced an answer".into())
            }));
        }

        let results = rank::fuse(ranked, request.limit);
        let network = critical.unwrap_or_default();
        let total = started.elapsed();
        Ok(SearchResponse {
            results,
            related,
            answer,
            timing: Timing {
                dns: network.dns,
                connect: network.connect,
                ttfb: network.ttfb,
                total,
                engine_overhead: total.saturating_sub(network.network),
            },
            providers,
        })
    }

    fn is_enabled(&self, provider: &Provider) -> bool {
        self.options.providers_enabled.is_empty()
            || self.options.providers_enabled.contains(provider)
    }

    /// The endpoint a plan is queried on, after any override.
    fn endpoint(&self, plan: &ParsePlan) -> Url {
        ProviderId::new(&plan.provider)
            .ok()
            .and_then(|id| self.options.endpoint_override.get(&id))
            .cloned()
            .unwrap_or_else(|| plan.request.endpoint.clone())
    }

    /// One provider, through the cache.
    async fn provider_query(
        &self,
        provider: &Provider,
        request: &SearchRequest,
    ) -> Result<(Arc<ProviderOutcome>, bool), SearchError> {
        let plan = self.plans.for_provider(provider)?;
        let mode = resolve_mode(provider, &plan, request.mode)?;
        let key = cache_key(&plan.provider, request);
        self.cache
            .get_or_fill(key, || self.run_ladder(provider, &plan, request, mode))
            .await
    }

    /// The escalation ladder (`docs/spec/08-search.md` §4) minus the steps
    /// that need JavaScript: one profile version exists at P1, and `Full` is
    /// P2, so what remains is the reset retry and the proxy rotation.
    async fn run_ladder(
        &self,
        provider: &Provider,
        plan: &ParsePlan,
        request: &SearchRequest,
        mode: Mode,
    ) -> Result<ProviderOutcome, SearchError> {
        let endpoint = self.endpoint(plan);
        let attempt = AttemptContext {
            provider,
            plan,
            prepared: plan.build_request(request, Some(&endpoint))?,
            mode,
            default_locale: request.locale.language.as_str(),
            query: &request.query,
        };
        let mut reset_retried = false;
        let mut proxy_index: Option<usize> = None;

        loop {
            let proxy = proxy_index.and_then(|index| self.options.proxies.get(index));
            let error = match self.attempt(&attempt, proxy, reset_retried).await {
                Ok(outcome) => return Ok(outcome),
                Err(error) => error,
            };

            match error.block_class() {
                // Step 0: a reset before any response is the transient
                // failure DuckDuckGo shows on a fresh connection.
                Some(BlockClass::Reset) if !reset_retried => {
                    tracing::debug!(%provider, "reset before response; retrying on a new connection");
                    reset_retried = true;
                }
                // Step 3: rate limiting and CAPTCHAs are IP-keyed, so the
                // only step that can help is the next exit IP.
                Some(class) if class.is_ip_keyed() => {
                    let next = proxy_index.map_or(0, |index| index + 1);
                    if next >= self.options.proxies.len() {
                        return Err(error);
                    }
                    tracing::debug!(%provider, %class, proxy = next, "rotating to the next proxy");
                    proxy_index = Some(next);
                }
                _ => return Err(error),
            }
        }
    }

    /// One request, one parse, one extraction.
    async fn attempt(
        &self,
        context: &AttemptContext<'_>,
        proxy: Option<&Proxy>,
        fresh_connection: bool,
    ) -> Result<ProviderOutcome, SearchError> {
        let provider = context.provider;
        let plan = context.plan;
        let prepared = &context.prepared;
        let fetched = fetch::fetch_and_parse(
            &self.client,
            plan,
            prepared,
            fetch::FetchOptions {
                proxy,
                fresh_connection,
                timeout: self.options.request_timeout,
                default_locale: context.default_locale,
                overrides: fetch::HeaderOverrides {
                    user_agent: self.options.user_agent.as_deref(),
                    accept_language: self.options.accept_language.as_deref(),
                },
            },
        )
        .await
        .map_err(|error| classify_transport(provider, error))?;

        let blocked_status = plan.block.status.contains(&fetched.status);
        if blocked_status || fetched.status >= 400 {
            if let Some(signal) =
                extract::detect_block(plan, &fetched.document, &fetched.body, fetched.status)
            {
                return Err(SearchError::Blocked {
                    provider: provider.clone(),
                    reason: signal.class,
                    detail: format!("HTTP {}: {}", fetched.status, signal.detail),
                });
            }
            return Err(SearchError::Net(NetError::new(
                tenta_net::policy::error_names::ErrorName::Failed,
                format!("{provider} answered HTTP {}", fetched.status),
            )));
        }

        let extraction = extract::extract(&fetched.document, plan, &fetched.url, provider);
        if extraction.results.is_empty() {
            // Only a page with no results is worth scanning for a refusal;
            // that keeps the pattern scan off the hot path entirely.
            if let Some(signal) =
                extract::detect_block(plan, &fetched.document, &fetched.body, fetched.status)
            {
                return Err(SearchError::Blocked {
                    provider: provider.clone(),
                    reason: signal.class,
                    detail: signal.detail,
                });
            }
            return Err(SearchError::Blocked {
                provider: provider.clone(),
                reason: BlockClass::Layout,
                detail: layout_hint(plan, &fetched),
            });
        }

        // Check for soft blocks: results that don't echo any query term (M-R4-1).
        if let Some(signal) =
            extract::detect_soft_block(context.query, &extraction.results, &fetched.document)
        {
            return Err(SearchError::Blocked {
                provider: provider.clone(),
                reason: signal.class,
                detail: signal.detail,
            });
        }

        Ok(ProviderOutcome {
            endpoint: prepared.url.clone(),
            plan_version: plan.version,
            mode_used: context.mode,
            results: extraction.results,
            related: extraction.related,
            answer: extraction.answer,
            timing: fetched.timing,
        })
    }
}

/// Everything one attempt needs; grouped so the ladder can retry it with a
/// different connection or proxy without rebuilding the request.
struct AttemptContext<'a> {
    provider: &'a Provider,
    plan: &'a ParsePlan,
    prepared: crate::plan::PreparedRequest,
    mode: Mode,
    default_locale: &'a str,
    query: &'a str,
}

/// Which mode a provider can actually serve in this build.
fn resolve_mode(
    provider: &Provider,
    plan: &ParsePlan,
    requested: Mode,
) -> Result<Mode, SearchError> {
    match plan.path {
        // Google requires JavaScript since 15 January 2025, so its plan is a
        // Full-mode plan and Full mode is P2 (`docs/spec/08-search.md` §2).
        PlanPath::Full => Err(SearchError::Unsupported {
            provider: provider.clone(),
            mode: Mode::Full,
        }),
        PlanPath::Fast => match requested {
            Mode::Fast | Mode::Auto => Ok(Mode::Fast),
            Mode::Full => Err(SearchError::Unsupported {
                provider: provider.clone(),
                mode: Mode::Full,
            }),
        },
    }
}

/// A transport failure before any response head is the `reset` block class
/// (`docs/spec/08-search.md` §4); anything else is a transport error.
fn classify_transport(provider: &Provider, error: NetError) -> SearchError {
    use tenta_net::policy::error_names::ErrorName;
    let reset = matches!(
        error.name(),
        ErrorName::ConnectionReset
            | ErrorName::ConnectionClosed
            | ErrorName::ConnectionAborted
            | ErrorName::EmptyResponse
    );
    if reset {
        SearchError::Blocked {
            provider: provider.clone(),
            reason: BlockClass::Reset,
            detail: error.to_string(),
        }
    } else {
        SearchError::Net(error)
    }
}

/// The message a layout change gets: what matched, what did not, and where
/// the fixture that would show the difference lives.
fn layout_hint(plan: &ParsePlan, fetched: &fetch::Fetched) -> String {
    let container = plan
        .results
        .container
        .as_ref()
        .map(|selector| selector.as_str())
        .unwrap_or("(document)");
    format!(
        "plan {} v{}: container {container:?}, item {:?} matched no result in {} bytes \
         (first {PATTERN_SAMPLE_BYTES} scanned for block patterns). Record a fresh SERP into \
         providers/fixtures/{}/ and diff it: `python3 providers/check_plans.py {}`",
        plan.provider,
        plan.version,
        plan.results.item.as_str(),
        fetched.body.len(),
        plan.provider,
        plan.provider,
    )
}
