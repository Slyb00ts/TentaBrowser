//! The browser context: one cookie jar, one cache, one session cache, one
//! destination policy and one client, shared by its pages
//! (`docs/spec/03-network.md` §4, §6; `docs/spec/19-security.md` §1, §2).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use tenta_core::{Budgets, ContextId, Error, PageId, Result, Tier};
use tenta_js::engine::PageEngineFactory;
use tenta_net::hooks::{Hooks, Interceptor, Observer};
use tenta_net::policy::cache::{CacheConfig, HttpCache};
use tenta_net::policy::cookies::{CookieJar, CookiePolicy, StorageState};
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::policy::session_cache::{SessionCache, SessionKey};
use tenta_net::profile::Profile;
use tenta_net::tls::TlsOptions;
use tenta_net::{Client, ClientConfig};
use tenta_page::document::Document;
use tenta_page::page::{Goto, Page, PageConfig, TestHooks, Viewport};
use tenta_page::settle::Settle;
use url::Url;

use crate::engine::{EngineInner, TrimLevel, TrimReport};
use crate::extensions::Extensions;
use crate::hooks::{InterceptHub, ObserverHub};

/// A page group: the unit of JavaScript isolation among pages
/// (`docs/spec/02-architecture.md` §3). Tier 1 groups have no engine.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct GroupId(pub u64);

struct GroupRecord {
    id: GroupId,
    thread: usize,
    page: Page,
    has_engine: bool,
}

/// Options of a browser context.
#[derive(Clone, Debug, Default)]
pub struct ContextOptions {
    /// Proxy URL (`http://`, `https://`, `socks5://`), with credentials.
    pub proxy: Option<String>,
    /// Loopback, RFC 1918, link-local and `.local` destinations are refused
    /// unless this is set (`docs/spec/19-security.md` §2). Test harnesses set
    /// it; production contexts do not.
    pub allow_private_network: bool,
    /// Overrides the engine's fingerprint profile.
    pub profile: Option<Profile>,
    pub cookie_policy: CookiePolicy,
    /// On-disk HTTP cache directory. Checked against the engine's `FsPolicy`.
    pub cache_dir: Option<std::path::PathBuf>,
    /// `None` is an ephemeral context: it reads and writes no session cache,
    /// so a server cannot link two contexts through TLS ticket reuse
    /// (`docs/spec/03-network.md` §4). The CLI's default context uses
    /// `cli-default`.
    pub persistence_id: Option<String>,
    /// Extra trust anchors in PEM form.
    pub extra_root_cas: Vec<Vec<u8>>,
    /// Playwright's `ignoreHTTPSErrors`.
    pub ignore_https_errors: bool,
    /// Overrides the profile's `User-Agent` (O-3).
    pub user_agent: Option<String>,
    /// Overrides the profile's `Accept-Language`.
    pub accept_language: Option<String>,
    /// Lowers the engine's budgets for this context; never raises them.
    pub budgets: Option<Budgets>,
}

/// Options of one page.
#[derive(Clone, Debug)]
pub struct PageOptions {
    /// Whether scripts run. `true` needs `max_tier >= js`, which is P2.
    pub javascript: bool,
    /// Pins the ceiling this page may reach below the engine's
    /// (`docs/spec/02-architecture.md` §1.1). `None` is the engine's ceiling.
    pub max_tier: Option<Tier>,
    pub viewport: Viewport,
    #[doc(hidden)]
    pub crash_in_parse_for_test: bool,
}

impl Default for PageOptions {
    /// `javascript` defaults to `true` for `new_page`
    /// (`docs/spec/02-architecture.md` §1.1); Tier 1 callers use
    /// [`PageOptions::tier1`].
    fn default() -> Self {
        Self {
            javascript: true,
            max_tier: None,
            viewport: Viewport::default(),
            crash_in_parse_for_test: false,
        }
    }
}

impl PageOptions {
    /// A page that parses HTML and runs nothing: what `ctx.fetch` uses.
    pub fn tier1() -> Self {
        Self {
            javascript: false,
            max_tier: Some(Tier::Dom),
            viewport: Viewport::default(),
            crash_in_parse_for_test: false,
        }
    }
}

pub(crate) struct ContextInner {
    id: ContextId,
    engine: Arc<EngineInner>,
    client: Client,
    cookies: CookieJar,
    cache: HttpCache,
    session_cache: SessionCache,
    destination: DestinationPolicy,
    interceptors: Arc<InterceptHub>,
    observers: Arc<ObserverHub>,
    budgets: Budgets,
    extra_headers: Vec<(String, String)>,
    next_page: AtomicU64,
    next_group: AtomicU64,
    groups: Mutex<Vec<GroupRecord>>,
    closed: AtomicBool,
    extensions: Extensions,
}

impl ContextInner {
    pub(crate) fn create(
        engine: Arc<EngineInner>,
        id: ContextId,
        options: ContextOptions,
    ) -> Result<BrowserContext> {
        let profile = options
            .profile
            .clone()
            .unwrap_or_else(|| engine.options.profile.clone());
        let cookies = CookieJar::new();
        cookies.set_policy(options.cookie_policy);

        let cache = match &options.cache_dir {
            Some(dir) => {
                // Check the path first to produce FsDenied errors (not Io errors)
                engine.options.fs_policy.check_write(dir)?;
                // Then create the directory with proper permissions
                engine
                    .options
                    .fs_policy
                    .create_dir_all(dir)
                    .map_err(Error::from)?;
                HttpCache::open_disk(CacheConfig::default(), dir.clone())?
            }
            None => HttpCache::new(CacheConfig::default()),
        };

        let session_cache = session_cache_for(&engine, &options, profile.name())?;

        let destination =
            DestinationPolicy::default().allow_private_network(options.allow_private_network);

        let proxy = match &options.proxy {
            Some(url) => Some(tenta_net::proxy::Proxy::parse(url).map_err(Error::from)?),
            None => None,
        };

        let interceptors = Arc::new(InterceptHub::default());
        let observers = Arc::new(ObserverHub::default());
        let budgets = match &options.budgets {
            Some(budgets) => budgets.clamped_to(&engine.options.budgets),
            None => engine.options.budgets.clone(),
        };

        let config = ClientConfig {
            profile,
            tls: TlsOptions {
                ignore_https_errors: options.ignore_https_errors,
                extra_root_cas: options.extra_root_cas.clone(),
                session_cache: session_cache.clone(),
                ..TlsOptions::default()
            },
            hooks: Hooks {
                interceptors: vec![Arc::clone(&interceptors) as Arc<dyn Interceptor>],
                observers: vec![Arc::clone(&observers) as Arc<dyn Observer>],
            },
            cookie_jar: Some(cookies.clone()),
            cache: Some(cache.clone()),
            session_cache: session_cache.clone(),
            destination: destination.clone(),
            proxy,
            context_id: id.raw(),
            max_response_bytes: Some(budgets.network_bytes),
            ..ClientConfig::default()
        };
        let client = engine.net.client(config).map_err(Error::from)?;

        let mut extra_headers = Vec::new();
        if let Some(user_agent) = &options.user_agent {
            extra_headers.push(("User-Agent".to_owned(), user_agent.clone()));
        }
        if let Some(accept_language) = &options.accept_language {
            extra_headers.push(("Accept-Language".to_owned(), accept_language.clone()));
        }

        Ok(BrowserContext {
            inner: Arc::new(ContextInner {
                id,
                engine,
                client,
                cookies,
                cache,
                session_cache,
                destination,
                interceptors,
                observers,
                budgets,
                extra_headers,
                next_page: AtomicU64::new(1),
                next_group: AtomicU64::new(1),
                groups: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
                extensions: Extensions::default(),
            }),
        })
    }

    fn check_open(&self) -> Result<()> {
        self.engine.check_open()?;
        if self.closed.load(Ordering::Relaxed) {
            return Err(Error::Closed);
        }
        Ok(())
    }

    /// How many groups still have an open page. A page's group is removed
    /// when the page closes ([`PageConfig::on_close`]); the sweep here only
    /// catches pages whose close never reached the hook.
    pub(crate) fn live_groups(&self) -> usize {
        let mut groups = self.groups.lock();
        groups.retain(|group| !group.page.is_closed());
        groups.len()
    }

    fn forget_page(&self, page: PageId) {
        self.groups.lock().retain(|group| group.page.id() != page);
    }

    pub(crate) async fn preconnect(&self, url: &str) -> Result<()> {
        self.check_open()?;
        self.client.preconnect(url).await.map_err(Error::from)
    }

    pub(crate) fn trim_memory(&self, level: TrimLevel) -> TrimReport {
        let before_groups = self.groups.lock().len();
        let groups = before_groups - self.live_groups();
        let cookies = self.cookies.purge_expired(std::time::SystemTime::now());
        let cache_bytes = match level {
            TrimLevel::Moderate => 0,
            TrimLevel::Critical => {
                let bytes = self.cache.bytes();
                self.cache.clear();
                bytes
            }
        };
        TrimReport {
            cache_bytes,
            cookies,
            groups,
        }
    }

    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::Relaxed) {
            return;
        }
        // Closing the context closes its pages (M-R5-1): a page whose context
        // is gone must not keep navigating with that context's cookies and
        // session, and its in-flight navigation must release its socket rather
        // than run to the deadline. `begin_close` is the synchronous half of
        // `Page::close`; the DOM is reclaimed when the page handle drops.
        //
        // The pages are taken out from under the lock first: `begin_close` runs
        // the page's `on_close` hook, which calls `forget_page` and re-locks
        // `groups` — holding the lock across it would deadlock.
        let pages: Vec<_> = {
            let groups = self.groups.lock();
            groups.iter().map(|group| group.page.clone()).collect()
        };
        for page in &pages {
            page.begin_close();
        }
        self.groups.lock().clear();
        self.cookies.remove_session_cookies();
        let _ = self.session_cache.flush(std::time::SystemTime::now());
        self.client.close();
    }
}

impl Drop for ContextInner {
    fn drop(&mut self) {
        self.close();
    }
}

/// A browser context. Cloning gives another handle to the same context.
#[derive(Clone)]
pub struct BrowserContext {
    inner: Arc<ContextInner>,
}

impl std::fmt::Debug for BrowserContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserContext")
            .field("id", &self.inner.id)
            .field("groups", &self.inner.groups.lock().len())
            .finish_non_exhaustive()
    }
}

impl BrowserContext {
    pub(crate) fn inner(&self) -> &Arc<ContextInner> {
        &self.inner
    }

    pub fn id(&self) -> ContextId {
        self.inner.id
    }

    /// The cookie store of this context.
    pub fn cookies(&self) -> &CookieJar {
        &self.inner.cookies
    }

    /// The response cache of this context.
    pub fn cache(&self) -> &HttpCache {
        &self.inner.cache
    }

    /// Whether this context persists DNS answers and TLS tickets.
    pub fn is_ephemeral(&self) -> bool {
        self.inner.session_cache.is_ephemeral()
    }

    /// The destination rules in force.
    pub fn destination_policy(&self) -> &DestinationPolicy {
        &self.inner.destination
    }

    pub fn budgets(&self) -> &Budgets {
        &self.inner.budgets
    }

    /// The transport client, for callers that need Tier 0 directly.
    pub fn client(&self) -> &Client {
        &self.inner.client
    }

    /// State an assembly crate above this one keeps for the life of the
    /// context; see [`crate::Engine::extension`].
    pub fn extension<T, F>(&self, init: F) -> Result<Arc<T>>
    where
        T: std::any::Any + Send + Sync,
        F: FnOnce(&BrowserContext) -> Result<T>,
    {
        self.inner.check_open()?;
        self.inner.extensions.get_or_try_init(|| init(self))
    }

    /// Registers an interceptor (`docs/spec/03-network.md` §9). The first one
    /// that does not continue decides.
    pub fn intercept(&self, interceptor: Arc<dyn Interceptor>) -> Result<()> {
        self.inner.check_open()?;
        self.inner.interceptors.push(interceptor);
        Ok(())
    }

    /// Registers a passive observer.
    pub fn observe(&self, observer: Arc<dyn Observer>) -> Result<()> {
        self.inner.check_open()?;
        self.inner.observers.push(observer);
        Ok(())
    }

    pub fn interceptor_count(&self) -> usize {
        self.inner.interceptors.len()
    }

    pub fn observer_count(&self) -> usize {
        self.inner.observers.len()
    }

    /// Playwright's `storageState`: the cookies of this context. Origin
    /// storage is Tier 2 and lands with it (P2), so `origins` is empty.
    pub fn storage_state(&self) -> StorageState {
        self.inner.cookies.to_storage_state()
    }

    /// Loads cookies from a `storageState` into this context.
    pub fn set_storage_state(&self, state: &StorageState) -> Result<usize> {
        self.inner.check_open()?;
        self.inner
            .cookies
            .load_storage_state(state, std::time::SystemTime::now())
    }

    /// Live page groups of this context.
    pub fn page_groups(&self) -> usize {
        self.inner.live_groups()
    }

    /// Opens a page pinned to one page thread.
    pub async fn new_page(&self, options: PageOptions) -> Result<Page> {
        self.inner.check_open()?;
        let ceiling = self.inner.engine.options.max_tier;
        if let Some(pinned) = options.max_tier
            && pinned > ceiling
        {
            return Err(Error::TierDenied {
                needed: pinned,
                ceiling,
            });
        }
        let max_tier = options.max_tier.unwrap_or(ceiling).min(ceiling);
        // A page that asked for JavaScript gets the backend's factory; a Tier 1
        // page never enters an engine, so it is not given one to leave unused.
        let engine = if options.javascript {
            Some(javascript_engine(max_tier)?)
        } else {
            None
        };
        // Read before the factory moves into the page: the group records
        // whether its page got one, which `group_threads` reports.
        let has_engine = engine.is_some();
        let thread = self.inner.engine.pool.lock().assign();
        let id = PageId(self.inner.next_page.fetch_add(1, Ordering::Relaxed));
        let page = Page::new(PageConfig {
            id,
            context: self.inner.id,
            client: self.inner.client.clone(),
            thread: Arc::clone(&thread),
            clock: tenta_page::clock::system_clock(),
            max_tier,
            javascript: options.javascript,
            engine,
            budgets: self.inner.budgets.clone(),
            settle: self.inner.engine.options.settle,
            locale: self.inner.engine.options.locale.clone(),
            extra_headers: self.inner.extra_headers.clone(),
            viewport: options.viewport,
            on_close: Some({
                let inner = Arc::downgrade(&self.inner);
                Arc::new(move |page| {
                    if let Some(inner) = inner.upgrade() {
                        inner.forget_page(page);
                    }
                })
            }),
            test_hooks: TestHooks {
                panic_in_parse: options.crash_in_parse_for_test,
            },
        });
        // One group per page, because nothing shares an engine across pages
        // yet; a group therefore carries an engine exactly when its page was
        // given one (`02-architecture.md` §3).
        let group = GroupId(self.inner.next_group.fetch_add(1, Ordering::Relaxed));
        self.inner.groups.lock().push(GroupRecord {
            id: group,
            thread: thread.index(),
            page: page.clone(),
            has_engine,
        });
        Ok(page)
    }

    /// Tier 1 convenience: fetch a URL, parse it, return the document and
    /// close the page (`docs/spec/11-library-api.md` §1).
    pub async fn fetch(&self, url: &str) -> Result<Document> {
        let page = self.new_page(PageOptions::tier1()).await?;
        let outcome = page
            .goto(url, Goto::default().wait_until(Settle::Load))
            .await;
        let document = match outcome {
            Ok(_) => page.take_document(),
            Err(error) => Err(error),
        };
        let _ = page.close().await;
        document
    }

    /// Which page thread a group runs on, for tests and diagnostics.
    pub fn group_threads(&self) -> Vec<(u64, usize, bool)> {
        self.inner
            .groups
            .lock()
            .iter()
            .map(|group| (group.id.0, group.thread, group.has_engine))
            .collect()
    }

    /// Closes the context: session cookies are dropped, the session cache is
    /// flushed and the client's connections are closed.
    pub fn close(&self) {
        self.inner.close();
    }
}

/// The session cache a context gets: a real one for a persistent context, a
/// stub for an ephemeral one (`docs/spec/03-network.md` §4).
fn session_cache_for(
    engine: &EngineInner,
    options: &ContextOptions,
    profile_name: &str,
) -> Result<SessionCache> {
    let Some(persistence_id) = &options.persistence_id else {
        return Ok(SessionCache::ephemeral());
    };
    if engine.options.ephemeral {
        return Ok(SessionCache::ephemeral());
    }
    let Some(dir) = &engine.options.session_cache_dir else {
        return Ok(SessionCache::ephemeral());
    };
    // The session cache is a cache directory, so it goes through `FsPolicy`
    // like every other host path (`docs/spec/19-security.md` §1).
    let dir = ensure_dir(&engine.options.fs_policy, dir)?;
    let key = SessionKey::new(profile_name, persistence_id.clone());
    SessionCache::open(&dir, &key)
}

fn ensure_dir(policy: &tenta_core::FsPolicy, dir: &Path) -> Result<std::path::PathBuf> {
    // Check the path first to produce FsDenied errors (not Io errors)
    let checked = policy.check_write(dir)?;
    // Then create the directory with proper permissions
    policy.create_dir_all(&checked).map_err(Error::from)?;
    Ok(checked)
}

/// Parses a URL the way a context's navigation does, so callers get the same
/// error for a bad URL wherever they pass one.
pub fn parse_url(url: &str) -> Result<Url> {
    Url::parse(url).map_err(|err| Error::invalid(format!("invalid URL {url:?}: {err}")))
}

/// The engine factory a JavaScript page at `ceiling` runs with, or the reason
/// it cannot have one.
///
/// A build without a compiled-in backend refuses JavaScript outright and names
/// the feature that supplies one. A build with the V8 backend refuses only a
/// page whose ceiling is below Tier 2, with the same error a page pinned above
/// the engine's own ceiling gets.
fn javascript_engine(ceiling: Tier) -> Result<Arc<dyn PageEngineFactory>> {
    #[cfg(not(feature = "v8"))]
    {
        let _ = ceiling;
        Err(Error::unsupported(
            "javascript needs the `v8` feature: rebuild tenta-browser with `--features v8`",
        ))
    }
    #[cfg(feature = "v8")]
    {
        if ceiling < Tier::Js {
            return Err(Error::TierDenied {
                needed: Tier::Js,
                ceiling,
            });
        }
        Ok(Arc::new(tenta_js_v8::V8EngineFactory))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_options_default_to_javascript_on() {
        let options = PageOptions::default();
        assert!(options.javascript, "new_page defaults to javascript on");
        assert!(
            options.max_tier.is_none(),
            "no pin below the engine ceiling"
        );
        let tier1 = PageOptions::tier1();
        assert!(!tier1.javascript);
        assert_eq!(tier1.max_tier, Some(Tier::Dom));
    }

    #[test]
    fn a_context_defaults_to_refusing_private_networks() {
        let options = ContextOptions::default();
        assert!(!options.allow_private_network);
        assert!(options.persistence_id.is_none(), "the default is ephemeral");
        assert!(parse_url("not a url").is_err());
        assert!(parse_url("http://example.test/").is_ok());
    }
}
