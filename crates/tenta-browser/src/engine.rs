//! The engine: io runtime, page threads and the contexts that share them
//! (`docs/spec/02-architecture.md` §1, §3; `docs/spec/11-library-api.md` §1).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use tenta_core::{Budgets, ContextId, Error, FsPolicy, Result, Tier};
use tenta_net::NetRuntime;
use tenta_net::policy::session_cache::SessionCache;
use tenta_net::profile::Profile;
use tenta_page::settle::SettleConfig;

use crate::context::{BrowserContext, ContextInner, ContextOptions};
use crate::extensions::Extensions;
use crate::threads::PagePool;

/// Which JavaScript backend JavaScript-enabled pages use.
///
/// A compiled backend is selected only when a page asks to run JavaScript.
/// [`JsEngine::None`] explicitly refuses those pages while leaving Tier 0 and
/// Tier 1 pages available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JsEngine {
    /// V8 where it is built, QuickJS otherwise.
    #[default]
    Auto,
    V8,
    QuickJs,
    /// Explicitly disable page engines, even when a backend is compiled in.
    /// JavaScript-enabled pages return [`Error::Unsupported`]; Tier 0 and Tier
    /// 1 pages remain available.
    None,
}

/// How hard [`Engine::trim_memory`] tries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimLevel {
    /// Drop what is cheap to rebuild.
    Moderate,
    /// Drop everything that is not needed to keep the live pages working.
    Critical,
}

/// What was released by a [`Engine::trim_memory`] call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrimReport {
    /// Cached response bytes dropped.
    pub cache_bytes: u64,
    /// Expired cookies removed.
    pub cookies: usize,
    /// Page group records of closed pages that were forgotten.
    pub groups: usize,
}

/// Everything the engine was built with.
#[derive(Clone, Debug)]
pub struct EngineOptions {
    /// The backend JavaScript-enabled pages may use. [`JsEngine::None`] refuses
    /// JavaScript-enabled pages without affecting Tier 0 or Tier 1 pages.
    pub js: JsEngine,
    pub max_tier: Tier,
    pub profile: Profile,
    pub budgets: Budgets,
    pub fs_policy: FsPolicy,
    pub io_threads: usize,
    pub page_threads: usize,
    /// Stored for P2: the time zone every context reports.
    pub timezone: Option<String>,
    /// BCP 47 tag; picks the fallback encoding and, in P2, `navigator.language`.
    pub locale: String,
    /// Where persistent contexts keep DNS answers and TLS tickets.
    pub session_cache_dir: Option<PathBuf>,
    /// When set, no context persists anything, whatever its `persistence_id`.
    pub ephemeral: bool,
    pub settle: SettleConfig,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            js: JsEngine::Auto,
            max_tier: Tier::Dom,
            profile: Profile::None,
            budgets: Budgets::platform_default(),
            fs_policy: FsPolicy::none(),
            io_threads: 1,
            page_threads: default_page_threads(),
            timezone: None,
            locale: "en".to_owned(),
            session_cache_dir: SessionCache::default_dir(),
            ephemeral: false,
            settle: SettleConfig::library(),
        }
    }
}

/// One page thread per logical core, two on mobile
/// (`docs/spec/02-architecture.md` §3).
pub fn default_page_threads() -> usize {
    if cfg!(any(target_os = "android", target_os = "ios")) {
        2
    } else {
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(4)
    }
}

/// Builds an [`Engine`].
#[derive(Debug, Default)]
pub struct EngineBuilder {
    options: EngineOptions,
}

impl EngineBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn js(mut self, js: JsEngine) -> Self {
        self.options.js = js;
        self
    }

    pub fn max_tier(mut self, tier: Tier) -> Self {
        self.options.max_tier = tier;
        self
    }

    pub fn profile(mut self, profile: Profile) -> Self {
        self.options.profile = profile;
        self
    }

    pub fn budgets(mut self, budgets: Budgets) -> Self {
        self.options.budgets = budgets;
        self
    }

    pub fn fs_policy(mut self, policy: FsPolicy) -> Self {
        self.options.fs_policy = policy;
        self
    }

    pub fn io_threads(mut self, threads: usize) -> Self {
        self.options.io_threads = threads.max(1);
        self
    }

    pub fn page_threads(mut self, threads: usize) -> Self {
        self.options.page_threads = threads.max(1);
        self
    }

    /// Stored for P2 (`docs/spec/20-platform-behaviour.md` §5).
    pub fn timezone(mut self, timezone: impl Into<String>) -> Self {
        self.options.timezone = Some(timezone.into());
        self
    }

    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.options.locale = locale.into();
        self
    }

    /// Where persistent contexts keep DNS answers and TLS tickets
    /// (`docs/spec/03-network.md` §4). Checked against the filesystem policy.
    pub fn session_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.options.session_cache_dir = Some(dir.into());
        self
    }

    /// Nothing is written to disk, whatever a context's `persistence_id`.
    pub fn ephemeral(mut self, ephemeral: bool) -> Self {
        self.options.ephemeral = ephemeral;
        self
    }

    /// The settle tunables every page of this engine uses.
    pub fn settle(mut self, settle: SettleConfig) -> Self {
        self.options.settle = settle;
        self
    }

    pub fn build(self) -> Result<Engine> {
        let options = self.options;
        // The ceiling is what this build implements: Tier 3 needs the `render`
        // feature, Tier 2 a compiled-in backend. A higher `max_tier` is refused
        // rather than silently ignored.
        let ceiling = if cfg!(feature = "render") {
            Tier::Render
        } else if cfg!(any(feature = "v8", feature = "quickjs")) {
            Tier::Js
        } else {
            Tier::Dom
        };
        if options.max_tier > ceiling {
            return Err(Error::unsupported(format!(
                "max_tier {} is above what this build implements: {ceiling}",
                options.max_tier,
            )));
        }
        #[cfg(not(feature = "v8"))]
        if options.js == JsEngine::V8 {
            return Err(Error::unsupported(
                "V8 is not compiled in; enable the `v8` feature",
            ));
        }
        #[cfg(not(feature = "quickjs"))]
        if options.js == JsEngine::QuickJs {
            return Err(Error::unsupported(
                "QuickJS is not compiled in; enable the `quickjs` feature",
            ));
        }
        let net = NetRuntime::new(options.io_threads).map_err(Error::from)?;
        let pool = PagePool::start(options.page_threads)?;
        Ok(Engine {
            inner: Arc::new(EngineInner {
                net,
                pool: Mutex::new(pool),
                contexts: Mutex::new(Vec::new()),
                next_context: AtomicU64::new(1),
                closed: AtomicBool::new(false),
                extensions: Extensions::default(),
                options,
            }),
        })
    }
}

pub(crate) struct EngineInner {
    pub(crate) net: Arc<NetRuntime>,
    pub(crate) pool: Mutex<PagePool>,
    pub(crate) options: EngineOptions,
    contexts: Mutex<Vec<std::sync::Weak<ContextInner>>>,
    next_context: AtomicU64,
    closed: AtomicBool,
    extensions: Extensions,
}

impl EngineInner {
    pub(crate) fn check_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(Error::Closed);
        }
        Ok(())
    }

    fn live_contexts(&self) -> Vec<Arc<ContextInner>> {
        let mut contexts = self.contexts.lock();
        contexts.retain(|weak| weak.strong_count() > 0);
        contexts
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .collect()
    }
}

/// The engine. Cloning gives another handle to the same engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("max_tier", &self.inner.options.max_tier)
            .field("io_threads", &self.inner.options.io_threads)
            .field("page_threads", &self.inner.pool.lock().len())
            .finish_non_exhaustive()
    }
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    pub fn options(&self) -> &EngineOptions {
        &self.inner.options
    }

    pub fn max_tier(&self) -> Tier {
        self.inner.options.max_tier
    }

    pub fn budgets(&self) -> &Budgets {
        &self.inner.options.budgets
    }

    pub fn fs_policy(&self) -> &FsPolicy {
        &self.inner.options.fs_policy
    }

    pub fn io_threads(&self) -> usize {
        self.inner.net.io_threads()
    }

    pub fn page_threads(&self) -> usize {
        self.inner.pool.lock().len()
    }

    /// Live contexts.
    pub fn contexts(&self) -> usize {
        self.inner.live_contexts().len()
    }

    /// Live page groups across every context
    /// (`docs/spec/02-architecture.md` §3).
    pub fn page_groups(&self) -> usize {
        self.inner
            .live_contexts()
            .iter()
            .map(|context| context.live_groups())
            .sum()
    }

    /// Creates a browser context: its own cookies, cache, session cache and
    /// destination policy, and one client pinned to one io thread.
    pub fn new_context(&self, options: ContextOptions) -> Result<BrowserContext> {
        self.inner.check_open()?;
        let id = ContextId(self.inner.next_context.fetch_add(1, Ordering::Relaxed));
        let context = ContextInner::create(Arc::clone(&self.inner), id, options)?;
        self.inner
            .contexts
            .lock()
            .push(Arc::downgrade(context.inner()));
        Ok(context)
    }

    /// Pre-connects to `urls` on every live context, so a later request pays
    /// no handshake (`docs/spec/11-library-api.md` §1). No isolates are warmed:
    /// there is no JavaScript engine before P2.
    pub async fn warm(&self, urls: &[&str]) -> Result<usize> {
        self.inner.check_open()?;
        let mut warmed = 0;
        for context in self.inner.live_contexts() {
            for url in urls {
                if context.preconnect(url).await.is_ok() {
                    warmed += 1;
                }
            }
        }
        Ok(warmed)
    }

    /// State an assembly crate above this one keeps for the life of the
    /// engine, created once on first use.
    ///
    /// `tentabrowser::EngineSearch` stores the internal search context and
    /// its plan set here, because `tenta-search` sits above `tenta-browser`
    /// and cannot be a field of this type (`docs/spec/02-architecture.md`
    /// §2). `init` must not ask the same engine for an extension again.
    pub fn extension<T, F>(&self, init: F) -> Result<Arc<T>>
    where
        T: std::any::Any + Send + Sync,
        F: FnOnce(&Engine) -> Result<T>,
    {
        self.inner.check_open()?;
        self.inner.extensions.get_or_try_init(|| init(self))
    }

    /// Drops what can be rebuilt (`docs/spec/14-memory.md` §4).
    pub fn trim_memory(&self, level: TrimLevel) -> TrimReport {
        let mut report = TrimReport::default();
        for context in self.inner.live_contexts() {
            let context_report = context.trim_memory(level);
            report.cache_bytes += context_report.cache_bytes;
            report.cookies += context_report.cookies;
            report.groups += context_report.groups;
        }
        report
    }

    /// Closes every context and stops the page threads. Idempotent.
    pub fn close(&self) {
        if self.inner.closed.swap(true, Ordering::Relaxed) {
            return;
        }
        for context in self.inner.live_contexts() {
            context.close();
        }
        self.inner.pool.lock().shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tier_ceiling_follows_the_compiled_in_backend() {
        // Tier 2 is reachable only when a backend can serve it, or when the
        // render feature raises the ceiling past it.
        #[cfg(not(any(feature = "v8", feature = "quickjs", feature = "render")))]
        {
            let error = Engine::builder()
                .max_tier(Tier::Js)
                .build()
                .expect_err("Tier 2 needs a backend");
            assert!(matches!(error, Error::Unsupported { .. }), "{error:?}");
        }
        #[cfg(any(feature = "v8", feature = "quickjs"))]
        {
            let engine = Engine::builder()
                .io_threads(1)
                .page_threads(1)
                .max_tier(Tier::Js)
                .build()
                .expect("the v8 feature serves Tier 2");
            assert_eq!(engine.max_tier(), Tier::Js);
            engine.close();
        }
        #[cfg(not(feature = "render"))]
        {
            let error = Engine::builder()
                .max_tier(Tier::Render)
                .build()
                .expect_err("Tier 3 needs the render feature");
            assert!(matches!(error, Error::Unsupported { .. }), "{error:?}");
        }
        #[cfg(feature = "render")]
        {
            let engine = Engine::builder()
                .io_threads(1)
                .page_threads(1)
                .max_tier(Tier::Render)
                .build()
                .expect("the render feature serves Tier 3");
            assert_eq!(engine.max_tier(), Tier::Render);
            engine.close();
        }
        // A selector naming a backend this build did not compile in is refused;
        // one naming a backend no build serves yet is refused in every build.
        #[cfg(not(feature = "v8"))]
        {
            let error = Engine::builder()
                .js(JsEngine::V8)
                .build()
                .expect_err("no backend is compiled in");
            assert!(matches!(error, Error::Unsupported { .. }), "{error:?}");
        }
        #[cfg(feature = "v8")]
        {
            let engine = Engine::builder()
                .io_threads(1)
                .page_threads(1)
                .js(JsEngine::V8)
                .build()
                .expect("this build serves the V8 selector");
            engine.close();
        }
        #[cfg(not(feature = "quickjs"))]
        {
            let error = Engine::builder()
                .js(JsEngine::QuickJs)
                .build()
                .expect_err("QuickJS feature is absent");
            assert!(matches!(error, Error::Unsupported { .. }), "{error:?}");
        }
        #[cfg(feature = "quickjs")]
        {
            let engine = Engine::builder()
                .io_threads(1)
                .page_threads(1)
                .js(JsEngine::QuickJs)
                .build()
                .expect("QuickJS selector");
            engine.close();
        }
    }

    #[test]
    fn an_engine_starts_its_threads_and_stops_them() {
        let engine = Engine::builder()
            .io_threads(1)
            .page_threads(2)
            .build()
            .expect("engine");
        assert_eq!(engine.page_threads(), 2);
        assert_eq!(engine.io_threads(), 1);
        assert_eq!(engine.max_tier(), Tier::Dom);
        assert_eq!(engine.contexts(), 0);
        engine.close();
        engine.close();
        let error = engine
            .new_context(ContextOptions::default())
            .expect_err("a closed engine takes no contexts");
        assert!(matches!(error, Error::Closed), "{error:?}");
    }

    #[test]
    fn the_default_pool_is_one_thread_per_core() {
        assert!(default_page_threads() >= 1);
        let options = EngineOptions::default();
        assert_eq!(options.max_tier, Tier::Dom);
        assert_eq!(options.locale, "en");
        assert!(!options.ephemeral);
    }
}
