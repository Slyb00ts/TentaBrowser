//! A page keeps its engine after the navigation settles, on the real backends
//! and the real page-thread pool (`docs/spec/02-architecture.md` §3, §5.1).
//!
//! The tests live in the crate, not in `tests/`, because they hand-build pages
//! on a [`PagePool`] with a factory that counts the engines it made and the
//! engines that were dropped — the only way to observe "no leak" from outside
//! the backend. Every scenario runs once per compiled backend.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tenta_core::{Budgets, ContextId, Error, PageId, Tier, TimerRule};
use tenta_dom::SharedDocument;
use tenta_js::engine::{
    EngineWatchdog, LifecycleEvent, PageEngine, PageEngineConfig, PageEngineFactory, ScriptError,
    ScriptKind,
};
use tenta_js::event_loop::ProvenanceSink;
use tenta_js::fetch::{FetchCompletion, FetchDriver};
use tenta_js::geometry::GeometryHost;
use tenta_js::inspect::InspectorSink;
use tenta_js::xhr::{XhrCompletion, XhrDriver};
#[cfg(feature = "quickjs")]
use tenta_js_qjs::QuickJsEngineFactory;
#[cfg(feature = "v8")]
use tenta_js_v8::V8EngineFactory;
use tenta_net::policy::cookies::CookieJar;
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::{Client, ClientConfig, NetRuntime};
use tenta_page::page::{PageConfig, TestHooks};
use tenta_page::settle::Settle;
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

use crate::threads::PagePool;
use crate::{Goto, Page, PageEvent, SettleConfig, SettleState, Viewport};

const PATIENCE: Duration = Duration::from_secs(20);

/// How many engines were made and how many are alive.
#[derive(Default)]
struct Count {
    created: AtomicUsize,
    live: AtomicUsize,
}

impl Count {
    fn live(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }

    fn created(&self) -> usize {
        self.created.load(Ordering::Acquire)
    }
}

struct CountingFactory {
    backend: Arc<dyn PageEngineFactory>,
    count: Arc<Count>,
}

impl PageEngineFactory for CountingFactory {
    fn create(
        &self,
        document: SharedDocument,
        config: &PageEngineConfig,
    ) -> Result<Box<dyn PageEngine>, String> {
        let inner = self.backend.create(document, config)?;
        self.count.created.fetch_add(1, Ordering::AcqRel);
        self.count.live.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(CountedEngine {
            inner,
            count: Arc::clone(&self.count),
        }))
    }
}

struct CountedEngine {
    inner: Box<dyn PageEngine>,
    count: Arc<Count>,
}

impl Drop for CountedEngine {
    fn drop(&mut self) {
        self.count.live.fetch_sub(1, Ordering::AcqRel);
    }
}

impl PageEngine for CountedEngine {
    fn native_failure(&self) -> Option<&'static str> {
        self.inner.native_failure()
    }
    fn document(&self) -> SharedDocument {
        self.inner.document()
    }
    fn run_script(&mut self, source: &str, kind: ScriptKind) -> Result<(), ScriptError> {
        self.inner.run_script(source, kind)
    }
    fn evaluate(&mut self, source: &str, timeout: Duration) -> Result<String, ScriptError> {
        self.inner.evaluate(source, timeout)
    }
    fn set_inspector_sink(&mut self, sink: Rc<dyn InspectorSink>) {
        self.inner.set_inspector_sink(sink)
    }
    fn inspector_call(
        &mut self,
        op: &str,
        argument: &str,
        timeout: Duration,
    ) -> Result<String, ScriptError> {
        self.inner.inspector_call(op, argument, timeout)
    }
    fn inspector_run(
        &mut self,
        kind: &str,
        source: &str,
        argument: &str,
        timeout: Duration,
    ) -> Result<String, ScriptError> {
        self.inner.inspector_run(kind, source, argument, timeout)
    }
    fn dispatch_lifecycle(&mut self, event: LifecycleEvent) {
        self.inner.dispatch_lifecycle(event)
    }
    fn set_timer_rule(&mut self, rule: TimerRule) {
        self.inner.set_timer_rule(rule)
    }
    fn set_provenance_sink(&mut self, sink: Rc<dyn ProvenanceSink>) {
        self.inner.set_provenance_sink(sink)
    }
    fn set_fetch_driver(&mut self, driver: Rc<dyn FetchDriver>) {
        self.inner.set_fetch_driver(driver)
    }
    fn set_geometry_host(&mut self, host: Rc<dyn GeometryHost>) {
        self.inner.set_geometry_host(host)
    }
    fn settle_fetches(&mut self, completions: Vec<FetchCompletion>) {
        self.inner.settle_fetches(completions)
    }
    fn set_xhr_driver(&mut self, driver: Rc<dyn XhrDriver>) {
        self.inner.set_xhr_driver(driver)
    }
    fn settle_xhr(&mut self, completions: Vec<XhrCompletion>) {
        self.inner.settle_xhr(completions)
    }
    fn next_wakeup(&self) -> Option<Duration> {
        self.inner.next_wakeup()
    }
    fn pump_due_timers(&mut self, max_tasks: usize) -> usize {
        self.inner.pump_due_timers(max_tasks)
    }
    fn frame_due(&self) -> bool {
        self.inner.frame_due()
    }
    fn run_animation_frame(&mut self) {
        self.inner.run_animation_frame()
    }
    fn run_microtasks(&mut self) {
        self.inner.run_microtasks()
    }
    fn trim_memory(&mut self) {
        self.inner.trim_memory()
    }
    fn gc_epoch(&self) -> u64 {
        self.inner.gc_epoch()
    }
    fn watchdog(&self) -> Option<Box<dyn EngineWatchdog>> {
        self.inner.watchdog()
    }
}

/// The backends this build can run, with the name a failure reports.
fn backends() -> Vec<(&'static str, Arc<dyn PageEngineFactory>)> {
    Vec::from([
        #[cfg(feature = "v8")]
        (
            "v8",
            Arc::new(V8EngineFactory) as Arc<dyn PageEngineFactory>,
        ),
        #[cfg(feature = "quickjs")]
        (
            "quickjs",
            Arc::new(QuickJsEngineFactory) as Arc<dyn PageEngineFactory>,
        ),
    ])
}

/// A fixture server, one page thread and a counting factory over one backend.
struct Rig {
    backend: &'static str,
    server: FixtureServer,
    client: Client,
    pool: PagePool,
    factory: Arc<CountingFactory>,
    count: Arc<Count>,
    _fixtures: TempDir,
    _net: Arc<NetRuntime>,
}

impl Rig {
    async fn new(backend: &'static str, factory: Arc<dyn PageEngineFactory>) -> Rig {
        let fixtures = tenta_testutil::temp_dir("persistent-engine");
        for (name, who) in [("one.html", "one"), ("two.html", "two")] {
            fixtures.write(
                name,
                format!(
                    "<!doctype html><html><body><div id=\"m\">before</div>\
                     <script>window.who = '{who}';</script></body></html>"
                ),
            );
        }
        fixtures.write("data.txt", "payload");
        let server = FixtureServer::start(FixtureServerConfig::new(fixtures.path()))
            .await
            .expect("fixture server");
        let net = NetRuntime::new(1).expect("network runtime");
        let client = net
            .client(ClientConfig {
                destination: DestinationPolicy::permissive(),
                ..ClientConfig::default()
            })
            .expect("client");
        let count = Arc::new(Count::default());
        Rig {
            backend,
            server,
            client,
            // One thread, so every page of a scenario shares it.
            pool: PagePool::start(1).expect("page pool"),
            factory: Arc::new(CountingFactory {
                backend: factory,
                count: Arc::clone(&count),
            }),
            count,
            _fixtures: fixtures,
            _net: net,
        }
    }

    fn page(&self, id: u64, budgets: Budgets) -> Page {
        Page::new(PageConfig {
            id: PageId(id),
            context: ContextId(1),
            client: self.client.clone(),
            thread: self.pool.assign(),
            clock: tenta_page::clock::system_clock(),
            max_tier: Tier::Js,
            javascript: true,
            engine: Some(Arc::clone(&self.factory) as Arc<dyn PageEngineFactory>),
            budgets,
            settle: SettleConfig::library(),
            locale: "en-US".to_owned(),
            extra_headers: Vec::new(),
            viewport: Viewport::default(),
            system_fonts: false,
            fs_policy: tenta_core::FsPolicy::none(),
            cookies: CookieJar::new(),
            storage: tenta_js::storage::PageStorage::isolated(),
            on_close: None,
            test_hooks: TestHooks::default(),
        })
    }

    fn url(&self, path: &str) -> String {
        self.server.url(path)
    }
}

/// Runs `scenario` once per compiled backend.
async fn each_backend<F, Fut>(scenario: F)
where
    F: Fn(Rig) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    for (name, factory) in backends() {
        scenario(Rig::new(name, factory).await).await;
    }
}

/// Starts page work the way a host action does: as a tracked script.
async fn run_tracked(page: &Page, source: &'static str) {
    page.with_engine(move |engine| engine.run_script(source, ScriptKind::Classic))
        .await
        .expect("the page has an engine")
        .expect("the script ran");
}

/// The JavaScript string completion of `expression` in the page's realm.
async fn eval(page: &Page, expression: &'static str) -> String {
    page.with_engine(move |engine| engine.evaluate(expression, PATIENCE))
        .await
        .expect("the page has an engine")
        .unwrap_or_else(|error| panic!("{expression} did not evaluate: {error:?}"))
}

async fn settle(page: &Page) -> SettleState {
    page.wait_for_settle(Settle::NetworkIdle, PATIENCE)
        .await
        .expect("settle")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timer_set_after_load_fires_in_the_same_realm_and_mutates_the_dom() {
    each_backend(|rig| async move {
        let page = rig.page(1, Budgets::default());
        page.goto(&rig.url("/one.html"), Goto::default())
            .await
            .expect("navigation");
        assert_eq!(eval(&page, "window.who").await, "one", "{}", rig.backend);

        run_tracked(
            &page,
            "setTimeout(function () { \
               document.body.setAttribute('data-late', window.who); \
             }, 10);",
        )
        .await;

        assert_eq!(
            settle(&page).await,
            SettleState::Settled { busy: false },
            "{}",
            rig.backend
        );
        assert_eq!(
            eval(&page, "document.body.getAttribute('data-late')").await,
            "one",
            "{}: the timer ran in the realm that loaded the page",
            rig.backend
        );
        page.close().await.expect("close");
        assert_eq!(rig.count.live(), 0, "{}", rig.backend);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fetch_started_by_a_post_load_timer_completes_and_the_settle_waits_for_it() {
    each_backend(|rig| async move {
        let page = rig.page(1, Budgets::default());
        page.goto(&rig.url("/one.html"), Goto::default())
            .await
            .expect("navigation");
        let served = rig.server.requests_served();

        run_tracked(
            &page,
            "setTimeout(function () { \
               fetch('/data.txt') \
                 .then(function (response) { return response.text(); }) \
                 .then(function (text) { \
                   document.body.setAttribute('data-fetched', text); \
                 }); \
             }, 10);",
        )
        .await;

        assert_eq!(
            settle(&page).await,
            SettleState::Settled { busy: false },
            "{}",
            rig.backend
        );
        assert_eq!(
            eval(&page, "document.body.getAttribute('data-fetched')").await,
            "payload",
            "{}: the settle ended after the fetch completed and its continuation ran",
            rig.backend
        );
        assert_eq!(rig.server.requests_served(), served + 1, "{}", rig.backend);
        page.close().await.expect("close");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_navigation_replaces_the_realm() {
    each_backend(|rig| async move {
        let page = rig.page(1, Budgets::default());
        page.goto(&rig.url("/one.html"), Goto::default())
            .await
            .expect("navigation");
        assert_eq!(eval(&page, "window.who").await, "one", "{}", rig.backend);
        run_tracked(&page, "window.leak = 'old realm';").await;

        page.goto(&rig.url("/two.html"), Goto::default())
            .await
            .expect("navigation");

        assert_eq!(eval(&page, "window.who").await, "two", "{}", rig.backend);
        assert_eq!(
            eval(&page, "typeof window.leak").await,
            "undefined",
            "{}: the old realm's globals are gone",
            rig.backend
        );
        assert_eq!(rig.count.created(), 2, "{}", rig.backend);
        assert_eq!(
            rig.count.live(),
            1,
            "{}: the first engine was dropped, not kept beside the second",
            rig.backend
        );
        page.close().await.expect("close");
        assert_eq!(rig.count.live(), 0, "{}", rig.backend);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_pages_on_one_thread_keep_separate_engines_and_drop_in_any_order() {
    for oldest_first in [true, false] {
        each_backend(|rig| async move {
            let older = rig.page(1, Budgets::default());
            let newer = rig.page(2, Budgets::default());
            assert_eq!(older.thread_index(), newer.thread_index());
            older
                .goto(&rig.url("/one.html"), Goto::default())
                .await
                .expect("navigation");
            newer
                .goto(&rig.url("/two.html"), Goto::default())
                .await
                .expect("navigation");
            assert_eq!(rig.count.live(), 2, "{}", rig.backend);
            assert_eq!(eval(&older, "window.who").await, "one", "{}", rig.backend);
            assert_eq!(eval(&newer, "window.who").await, "two", "{}", rig.backend);

            let (first, second, survivor) = if oldest_first {
                (&older, &newer, "two")
            } else {
                (&newer, &older, "one")
            };
            first.close().await.expect("close");
            assert_eq!(rig.count.live(), 1, "{}", rig.backend);
            assert_eq!(first.crash_reason(), None, "{}", rig.backend);
            assert_eq!(
                eval(second, "window.who").await,
                survivor,
                "{}: the survivor still answers from its own realm",
                rig.backend
            );

            second.close().await.expect("close");
            assert_eq!(rig.count.live(), 0, "{}", rig.backend);
            assert_eq!(second.crash_reason(), None, "{}", rig.backend);
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_context_side_of_a_page_drops_its_engine_without_an_await() {
    each_backend(|rig| async move {
        let page = rig.page(1, Budgets::default());
        page.goto(&rig.url("/one.html"), Goto::default())
            .await
            .expect("navigation");
        assert_eq!(rig.count.live(), 1, "{}", rig.backend);

        assert!(page.begin_close());

        let deadline = std::time::Instant::now() + PATIENCE;
        while rig.count.live() != 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "{}: the engine was never dropped",
                rig.backend
            );
            tokio::task::yield_now().await;
        }
        assert!(matches!(page.with_engine(|_| ()).await, Err(Error::Closed)));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runaway_post_load_timer_is_terminated_and_reported() {
    each_backend(|rig| async move {
        let page = rig.page(
            1,
            Budgets::default().evaluate_time(Duration::from_millis(150)),
        );
        let mut events = page.events();
        page.goto(&rig.url("/one.html"), Goto::default())
            .await
            .expect("navigation");

        run_tracked(&page, "setTimeout(function () { while (true) {} }, 0);").await;
        let state = settle(&page).await;

        assert_eq!(
            state,
            SettleState::Settled { busy: false },
            "{}",
            rig.backend
        );
        assert_eq!(page.crash_reason(), None, "{}", rig.backend);
        assert!(
            events
                .drain()
                .iter()
                .any(|event| matches!(event, PageEvent::ScriptTerminated { .. })),
            "{}: the watchdog's kill is reported",
            rig.backend
        );
        assert_eq!(
            eval(&page, "window.who").await,
            "one",
            "{}: the realm survived the runaway",
            rig.backend
        );
        page.close().await.expect("close");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_driver_delivers_a_fetch_completion_nobody_is_waiting_for() {
    each_backend(|rig| async move {
        let page = rig.page(1, Budgets::default());
        page.goto(&rig.url("/one.html"), Goto::default())
            .await
            .expect("navigation");

        run_tracked(
            &page,
            "fetch('/data.txt') \
               .then(function (response) { return response.text(); }) \
               .then(function (text) { \
                 document.body.setAttribute('data-fetched', text); \
               });",
        )
        .await;

        // No settle call: only the page's own driver can take the completion.
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            let seen = eval(&page, "String(document.body.getAttribute('data-fetched'))").await;
            if seen == "payload" {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{}: the completion was never delivered",
                rig.backend
            );
            tokio::task::yield_now().await;
        }
        page.close().await.expect("close");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_title_a_timer_sets_reaches_page_title_with_no_waiter() {
    each_backend(|rig| async move {
        let page = rig.page(1, Budgets::default());
        page.goto(&rig.url("/one.html"), Goto::default())
            .await
            .expect("navigation");
        assert_eq!(page.title(), None, "{}", rig.backend);

        run_tracked(
            &page,
            "setTimeout(function () { document.title = 'Retitled'; }, 5);",
        )
        .await;

        let deadline = std::time::Instant::now() + PATIENCE;
        while page.title().as_deref() != Some("Retitled") {
            assert!(
                std::time::Instant::now() < deadline,
                "{}: the title never reached the cache",
                rig.backend
            );
            tokio::task::yield_now().await;
        }
        page.close().await.expect("close");
    })
    .await;
}
