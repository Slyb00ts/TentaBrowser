//! A real native boundary failure must end one page, not its thread.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use tenta_js::xhr::{XhrCompletion, XhrDriver};
#[cfg(feature = "quickjs")]
use tenta_js_qjs::QuickJsEngineFactory;
#[cfg(feature = "v8")]
use tenta_js_v8::V8EngineFactory;
use tenta_net::policy::cookies::CookieJar;
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::{ClientConfig, NetRuntime};
use tenta_page::page::{PageConfig, TestHooks};
use tenta_testutil::{FixtureServer, FixtureServerConfig};

use crate::threads::PagePool;
use crate::{Goto, Page, PageEvent, SettleConfig, Viewport};

#[derive(Clone, Copy)]
enum FaultPoint {
    Script,
    Trim,
}

struct FaultFactory {
    backend: Arc<dyn PageEngineFactory>,
    point: FaultPoint,
    dropped: Arc<AtomicBool>,
    mutations_at_drop: Arc<AtomicU64>,
    returned: Option<Arc<tokio::sync::Notify>>,
}

impl PageEngineFactory for FaultFactory {
    fn create(
        &self,
        document: SharedDocument,
        config: &PageEngineConfig,
    ) -> Result<Box<dyn PageEngine>, String> {
        Ok(Box::new(FaultEngine {
            inner: self.backend.create(document, config)?,
            point: self.point,
            dropped: Arc::clone(&self.dropped),
            mutations_at_drop: Arc::clone(&self.mutations_at_drop),
            returned: self.returned.clone(),
        }))
    }
}

struct FaultEngine {
    point: FaultPoint,
    returned: Option<Arc<tokio::sync::Notify>>,
    inner: Box<dyn PageEngine>,
    dropped: Arc<AtomicBool>,
    mutations_at_drop: Arc<AtomicU64>,
}

impl Drop for FaultEngine {
    fn drop(&mut self) {
        self.mutations_at_drop.store(
            self.inner.document().borrow().mutations_since_sweep(),
            Ordering::Release,
        );
        self.dropped.store(true, Ordering::Release);
    }
}

impl PageEngine for FaultEngine {
    fn run_script(&mut self, source: &str, kind: ScriptKind) -> Result<(), ScriptError> {
        if matches!(self.point, FaultPoint::Trim) {
            return self.inner.run_script(source, kind);
        }
        let document = self.inner.document();
        let _conflicting_borrow = document.borrow_mut();
        // This invokes the real generated getter and its RefCell borrow, not
        // a test hook that simply reports a poisoned engine.
        let result = self.inner.run_script("document.body", kind);
        if self.inner.native_failure().is_some()
            && let Some(returned) = &self.returned
        {
            returned.notify_one();
        }
        result
    }

    fn native_failure(&self) -> Option<&'static str> {
        self.inner.native_failure()
    }
    fn document(&self) -> SharedDocument {
        self.inner.document()
    }
    fn evaluate(&mut self, source: &str, timeout: Duration) -> Result<String, ScriptError> {
        self.inner.evaluate(source, timeout)
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
        self.inner.trim_memory();
        if matches!(self.point, FaultPoint::Trim) && self.inner.native_failure().is_none() {
            let document = self.inner.document();
            let _conflicting_borrow = document.borrow_mut();
            // Return from this trim call with genuine sticky native poison.
            // Collection now correctly defers external document borrows, so
            // invoke a real generated getter after the inner trim instead.
            // Backend collector tests cover failures inside GC transactions;
            // this fixture checks the page's failure check before arena sweep.
            let _ = self.inner.run_script("document.body", ScriptKind::Classic);
        }
    }
    fn gc_epoch(&self) -> u64 {
        self.inner.gc_epoch()
    }
    fn watchdog(&self) -> Option<Box<dyn EngineWatchdog>> {
        self.inner.watchdog()
    }
}

fn make_page(
    pool: &PagePool,
    client: &tenta_net::Client,
    id: u64,
    factory: Arc<dyn PageEngineFactory>,
) -> Page {
    Page::new(PageConfig {
        id: PageId(id),
        context: ContextId(1),
        client: client.clone(),
        thread: pool.assign(),
        clock: tenta_page::clock::system_clock(),
        max_tier: Tier::Js,
        javascript: true,
        engine: Some(factory),
        budgets: Budgets::default(),
        settle: SettleConfig::library(),
        locale: "en-US".to_owned(),
        extra_headers: Vec::new(),
        viewport: Viewport::default(),
        cookies: CookieJar::new(),
        storage: tenta_js::storage::PageStorage::isolated(),
        on_close: None,
        test_hooks: TestHooks::default(),
    })
}

async fn run_native_boundary_failure(
    backend: Arc<dyn PageEngineFactory>,
    boundary: &str,
    point: FaultPoint,
) {
    let fixture = tenta_testutil::temp_dir("native-panic");
    fixture.write(
        "index.html",
        match point {
            FaultPoint::Script => {
                "<body><script>document.body.setAttribute('data-ok', 'yes')</script></body>"
            }
            FaultPoint::Trim => {
                // Exceed the runtime's 4096-mutation reclamation threshold
                // using real DOM mutations, without synthesizing a GC epoch.
                "<body><script>document.body.setAttribute('data-ok', 'yes');
                 for(let i=0;i<5000;i++) document.body.setAttribute('data-turn',String(i));
                 </script></body>"
            }
        },
    );
    let server = FixtureServer::start(FixtureServerConfig::new(fixture.path()))
        .await
        .expect("fixture server");
    let net = NetRuntime::new(1).expect("network runtime");
    let client = net
        .client(ClientConfig {
            destination: DestinationPolicy::permissive(),
            ..ClientConfig::default()
        })
        .expect("client");
    let pool = PagePool::start(1).expect("page pool");
    let dropped = Arc::new(AtomicBool::new(false));
    let mutations_at_drop = Arc::new(AtomicU64::new(0));
    let doomed = make_page(
        &pool,
        &client,
        1,
        Arc::new(FaultFactory {
            backend: backend.clone(),
            point,
            dropped: Arc::clone(&dropped),
            mutations_at_drop: Arc::clone(&mutations_at_drop),
            returned: None,
        }) as Arc<dyn PageEngineFactory>,
    );
    let healthy = make_page(&pool, &client, 2, backend);
    assert_eq!(doomed.thread_index(), healthy.thread_index());
    let mut events = doomed.events();
    let url = server.url("/index.html");
    let error = tokio::time::timeout(Duration::from_secs(10), doomed.goto(&url, Goto::default()))
        .await
        .expect("native panic returned through the engine")
        .expect_err("page crashed");
    let Error::Crashed { reason } = error else {
        panic!("expected crash, got {error:?}")
    };
    assert!(reason.contains(boundary), "{reason}");
    assert_eq!(doomed.crash_reason().as_deref(), Some(reason.as_str()));
    assert!(
        dropped.load(Ordering::Acquire),
        "poisoned engine was discarded"
    );
    if matches!(point, FaultPoint::Trim) {
        assert!(
            mutations_at_drop.load(Ordering::Acquire) >= 5000,
            "the arena must not be swept after trim returns native poison"
        );
    }
    let crash_events = std::iter::from_fn(|| events.try_next())
        .filter(|event| matches!(event, PageEvent::Crashed { .. }))
        .count();
    assert_eq!(crash_events, 1);
    assert!(matches!(
        doomed.goto(&url, Goto::default()).await,
        Err(Error::Crashed { .. })
    ));

    let (_, value) = tokio::time::timeout(
        Duration::from_secs(10),
        healthy.goto_with_eval(
            &url,
            Goto::default(),
            "document.body.getAttribute('data-ok')",
        ),
    )
    .await
    .expect("neighbor remains responsive")
    .expect("healthy navigation");
    assert_eq!(value, "yes");
    assert!(healthy.crash_reason().is_none());
    doomed.close().await.expect("close crashed page");
    healthy.close().await.expect("close healthy page");
}

async fn run_async_native_panic(backend: Arc<dyn PageEngineFactory>, boundary: &str) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_head(socket: &mut TcpStream) {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.expect("request header"));
            assert!(head.len() < 32_768, "bounded request header");
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}/index.html", listener.local_addr().unwrap());
    let returned = Arc::new(tokio::sync::Notify::new());
    let server_returned = Arc::clone(&returned);
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(30), async move {
            let (mut document, _) = listener.accept().await.expect("document request");
            read_head(&mut document).await;
            // Enough bytes for encoding prescan, but never a completed body:
            // the navigation must observe the crash while waiting for more.
            let prefix = format!("<body><script async src='/fault.js'></script>{}", " ".repeat(2048));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{prefix}",
                prefix.len() + 1024,
            );
            document.write_all(response.as_bytes()).await.expect("partial HTML");
            let (mut script, _) = listener.accept().await.expect("async script request");
            read_head(&mut script).await;
            let source = "document.body";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{source}", source.len(),
            );
            script.write_all(response.as_bytes()).await.expect("script response");
            drop(script);
            server_returned.notified().await;
            // Trigger a body-stream error only after the native callback has
            // returned. It must not hide the page crash as a network failure.
            drop(document);
        }).await.expect("fixture completed without hanging");
    });

    let net = NetRuntime::new(1).expect("network runtime");
    let client = net
        .client(ClientConfig {
            destination: DestinationPolicy::permissive(),
            ..ClientConfig::default()
        })
        .expect("client");
    let pool = PagePool::start(1).expect("page pool");
    let dropped = Arc::new(AtomicBool::new(false));
    let page = make_page(
        &pool,
        &client,
        1,
        Arc::new(FaultFactory {
            backend,
            point: FaultPoint::Script,
            dropped: Arc::clone(&dropped),
            mutations_at_drop: Arc::new(AtomicU64::new(0)),
            returned: Some(returned),
        }),
    );
    let mut events = page.events();
    let error = tokio::time::timeout(
        Duration::from_secs(20),
        page.goto(&url, Goto::default().timeout(Duration::from_secs(15))),
    )
    .await
    .expect("async native crash wakes the pending body read")
    .expect_err("page crashes");
    assert!(
        matches!(&error, Error::Crashed { reason } if reason.contains(boundary)),
        "{error:?}"
    );
    assert!(
        dropped.load(Ordering::Acquire),
        "poisoned runtime was discarded"
    );
    assert_eq!(
        events
            .drain()
            .iter()
            .filter(|event| matches!(event, PageEvent::Crashed { .. }))
            .count(),
        1
    );
    server.await.expect("fixture task");
    page.close().await.expect("close crashed page");
}

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_callback_panic_discards_the_page_and_preserves_its_neighbor() {
    run_native_boundary_failure(
        Arc::new(V8EngineFactory),
        "get_document_body",
        FaultPoint::Script,
    )
    .await;
}

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_native_panic_crashes_before_the_document_body_finishes() {
    run_async_native_panic(Arc::new(V8EngineFactory), "get_document_body").await;
}

#[cfg(feature = "quickjs")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quickjs_native_callback_panic_preserves_the_neighbor() {
    run_native_boundary_failure(Arc::new(QuickJsEngineFactory), "body", FaultPoint::Script).await;
}

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_failure_returned_from_trim_discards_the_page_and_preserves_its_neighbor() {
    run_native_boundary_failure(
        Arc::new(V8EngineFactory),
        "get_document_body",
        FaultPoint::Trim,
    )
    .await;
}

#[cfg(feature = "quickjs")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quickjs_failure_returned_from_trim_discards_the_page_and_preserves_its_neighbor() {
    run_native_boundary_failure(Arc::new(QuickJsEngineFactory), "body", FaultPoint::Trim).await;
}

#[cfg(feature = "quickjs")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quickjs_async_native_panic_crashes_before_the_body_finishes() {
    run_async_native_panic(Arc::new(QuickJsEngineFactory), "body").await;
}
