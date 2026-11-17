//! Stylesheets in the page runtime (CSS1): a `<link rel=stylesheet>` loads
//! when it enters the document, `load` waits for it, a parser-inserted one
//! holds the scripts after it back, and its `load` and `error` events reach
//! script. Every page is served by the in-process fixture server and runs the
//! V8 engine; the logs the pages keep are read back with `evaluate`.

#![cfg(all(feature = "render", feature = "v8"))]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tenta_browser::network::{
    BoxFuture, NetworkEvent, NetworkSink, RequestInfo, Resolution, ResourceType,
};
use tenta_browser::{
    BrowserContext, ContextOptions, Engine, Goto, JsEngine, Page, PageEvent, PageOptions, Settle,
    Viewport,
};
use tenta_core::{FsPolicy, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

const PATIENCE: Duration = Duration::from_secs(60);

/// Logs every `load` and `error` that reaches a `<link>`, whatever the page
/// does with handlers (event handler content attributes are not implemented, so
/// pages here observe the events in the capture phase).
const LOGGER: &str = "<script>var log = [];\
    ['load', 'error'].forEach(function (type) {\
      document.addEventListener(type, function (e) {\
        if (e.target.localName === 'link') log.push(e.target.getAttribute('href') + ' ' + type);\
      }, true);\
    });</script>";

fn page_of(head: &str, body: &str) -> Vec<u8> {
    format!("<!doctype html><html><head>{LOGGER}{head}</head><body>{body}</body></html>")
        .into_bytes()
}

struct Rig {
    server: FixtureServer,
    context: BrowserContext,
    engine: Engine,
    _files: TempDir,
}

impl Rig {
    async fn new(files: Vec<(&str, Vec<u8>)>, delays: &[(&str, Duration)]) -> Rig {
        let temp = tenta_testutil::temp_dir("css-loading");
        for (name, contents) in files {
            temp.write(name, contents);
        }
        let mut config = FixtureServerConfig::new(temp.path());
        for (glob, delay) in delays {
            config = config.delay_path(glob, *delay);
        }
        let server = FixtureServer::start(config).await.expect("fixture server");
        let engine = Engine::builder()
            .max_tier(Tier::Render)
            .io_threads(1)
            .page_threads(1)
            .fs_policy(FsPolicy::none().allow_read(temp.path()))
            .js(JsEngine::V8)
            .ephemeral(true)
            .build()
            .expect("engine");
        let context = engine
            .new_context(ContextOptions {
                allow_private_network: true,
                ..ContextOptions::default()
            })
            .expect("context");
        Rig {
            server,
            context,
            engine,
            _files: temp,
        }
    }

    async fn open(&self, path: &str, settle: Settle) -> (Page, Duration) {
        let page = self.new_page().await;
        self.go(page, path, settle).await
    }

    async fn new_page(&self) -> Page {
        self.context
            .new_page(PageOptions {
                javascript: true,
                viewport: Viewport {
                    width: 200,
                    height: 100,
                    device_scale_factor: 1.0,
                    mobile: false,
                },
                ..PageOptions::default()
            })
            .await
            .expect("page")
    }

    async fn go(&self, page: Page, path: &str, settle: Settle) -> (Page, Duration) {
        let started = Instant::now();
        page.goto(
            &self.server.url(path),
            Goto::default().wait_until(settle).timeout(PATIENCE),
        )
        .await
        .expect("navigation");
        (page, started.elapsed())
    }

    fn finish(self) {
        self.context.close();
        self.engine.close();
    }
}

async fn text(page: &Page, expression: &str) -> String {
    page.evaluate(expression)
        .await
        .expect("ran")
        .into_value::<String>()
        .expect("a string")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_waits_for_a_slow_stylesheet_and_the_link_fires_load_first() {
    let html = page_of(
        "<link rel=stylesheet href=slow.css id=l>",
        "<script>window.addEventListener('load', function () { log.push('window'); });</script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("slow.css", b"body{color:red}".to_vec()),
        ],
        &[("/slow.css", Duration::from_millis(1200))],
    )
    .await;
    let (page, took) = rig.open("/index.html", Settle::Load).await;
    assert!(
        took >= Duration::from_millis(1100),
        "load waited for the slow sheet: {took:?}"
    );
    assert_eq!(text(&page, "log.join(',')").await, "slow.css load,window");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parser_inserted_sheet_holds_the_next_script_back() {
    let html = page_of(
        "<script>var t0 = Date.now();</script>\
         <link rel=stylesheet href=slow.css>\
         <script>log.push('script waited=' + (Date.now() - t0 >= 700));</script>",
        "<script>log.push('body script');</script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("slow.css", b"body{color:red}".to_vec()),
        ],
        &[("/slow.css", Duration::from_millis(900))],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(
        text(&page, "log.join(' | ')").await,
        "slow.css load | script waited=true | body script"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_inserted_sheet_delays_load_but_not_scripts() {
    let html = page_of(
        "<script>\
         var l = document.createElement('link');\
         l.rel = 'stylesheet'; l.href = 'slow.css';\
         document.head.appendChild(l);\
         </script>\
         <script>log.push('next script');</script>",
        "<script>window.addEventListener('load', function () { log.push('window'); });</script>",
    );
    let rig = Rig::new(
        vec![("index.html", html), ("slow.css", b"p{color:red}".to_vec())],
        &[("/slow.css", Duration::from_millis(900))],
    )
    .await;
    let (page, took) = rig.open("/index.html", Settle::Load).await;
    assert!(
        took >= Duration::from_millis(800),
        "load waited for the script-inserted sheet: {took:?}"
    );
    assert_eq!(
        text(&page, "log.join(' | ')").await,
        "next script | slow.css load | window"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_a_mistyped_and_a_tolerated_sheet_report_error_or_load() {
    // Standards mode wants text/css; quirks mode tolerates another type.
    let standards = page_of(
        "<link rel=stylesheet href=missing.css>\
         <link rel=stylesheet href=plain.txt>",
        "",
    );
    let quirks = format!(
        "<html><head>{LOGGER}<link rel=stylesheet href=plain.txt></head><body></body></html>"
    )
    .into_bytes();
    let rig = Rig::new(
        vec![
            ("standards.html", standards),
            ("quirks.html", quirks),
            ("plain.txt", b"p{color:red}".to_vec()),
        ],
        &[],
    )
    .await;
    let (page, _) = rig.open("/standards.html", Settle::Load).await;
    let log = text(&page, "log.slice().sort().join(',')").await;
    assert_eq!(log, "missing.css error,plain.txt error");
    let (page, _) = rig.open("/quirks.html", Settle::Load).await;
    assert_eq!(text(&page, "log.join(',')").await, "plain.txt load");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disabled_or_print_only_sheet_never_holds_anything_back() {
    let html = page_of(
        "<link rel=stylesheet href=slow.css disabled>\
         <link rel=stylesheet href=slow2.css media=print>\
         <script>log.push('script');</script>",
        "",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("slow.css", b"p{}".to_vec()),
            ("slow2.css", b"p{}".to_vec()),
        ],
        &[
            ("/slow.css", Duration::from_secs(5)),
            ("/slow2.css", Duration::from_secs(5)),
        ],
    )
    .await;
    let (page, took) = rig.open("/index.html", Settle::DomContent).await;
    assert!(took < Duration::from_secs(3), "nothing waited: {took:?}");
    assert_eq!(text(&page, "log.join(',')").await, "script");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_after_a_blocking_sheet_reads_the_styles_it_defines() {
    let html = page_of(
        "<link rel=stylesheet href=slow.css>\
         <script>\
         var cs = getComputedStyle(document.documentElement);\
         window.seen = [cs.color, cs.width, cs.getPropertyValue('display'), cs.marginTop,\
           cs.backgroundColor].join(' | ');\
         </script>",
        "<div id=box>text</div>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            (
                "slow.css",
                b"html{color:rgb(255,0,0);margin:3px;background-color:#00ff00}".to_vec(),
            ),
        ],
        &[("/slow.css", Duration::from_millis(400))],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(
        text(&page, "window.seen").await,
        "rgb(255, 0, 0) | 194px | block | 3px | rgb(0, 255, 0)"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_inserted_sheet_applies_after_its_load_event_not_before() {
    let html = page_of(
        "<style>body{margin:0}</style>",
        "<p id=p>hi</p>\
         <script>\
         var p = document.getElementById('p');\
         window.out = [];\
         var l = document.createElement('link');\
         l.rel = 'stylesheet'; l.href = 'late.css';\
         l.addEventListener('load', function () {\
           out.push('load color=' + getComputedStyle(p).color);\
         });\
         document.head.appendChild(l);\
         out.push('sync color=' + getComputedStyle(p).color);\
         </script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("late.css", b"p{color:rgb(0,0,255)}".to_vec()),
        ],
        &[("/late.css", Duration::from_millis(300))],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(
        text(&page, "out.join(' | ')").await,
        "sync color=rgb(0, 0, 0) | load color=rgb(0, 0, 255)"
    );
    rig.finish();
}

/// Records the requests a page reports to a network host.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<(ResourceType, String, &'static str)>>,
}

impl NetworkSink for Recorder {
    fn event(&self, event: NetworkEvent) {
        let (kind, resource_type, url) = match &event {
            NetworkEvent::Request(info) => {
                ("request", info.resource_type, info.url.path().to_owned())
            }
            NetworkEvent::Response(info) => {
                ("response", info.resource_type, info.url.path().to_owned())
            }
            _ => return,
        };
        self.seen.lock().unwrap().push((resource_type, url, kind));
    }

    fn pause<'a>(&'a self, _request: &'a RequestInfo) -> BoxFuture<'a, Resolution> {
        Box::pin(async { Resolution::Continue(Default::default()) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stylesheets_appear_in_the_network_events_while_the_document_loads() {
    let html = page_of(
        "<link rel=stylesheet href=a.css>",
        "<p>x</p><script>window.sheetsAtScript = document.querySelectorAll('link').length;</script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("a.css", b"@import url('b.css'); p{color:red}".to_vec()),
            ("b.css", b"p{margin:0}".to_vec()),
        ],
        &[],
    )
    .await;
    let page = rig.new_page().await;
    let recorder = Arc::new(Recorder::default());
    page.network()
        .set_sink(Some(recorder.clone() as Arc<dyn NetworkSink>));
    page.network().set_events(true);
    rig.go(page.clone(), "/index.html", Settle::Load).await;
    let seen = recorder.seen.lock().unwrap().clone();
    let sheets: Vec<_> = seen
        .iter()
        .filter(|(kind, ..)| *kind == ResourceType::Stylesheet)
        .map(|(_, url, what)| format!("{url} {what}"))
        .collect();
    assert_eq!(
        sheets,
        [
            "/a.css request",
            "/a.css response",
            "/b.css request",
            "/b.css response"
        ],
        "{seen:?}"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_import_chain_loads_before_the_link_fires_and_a_screenshot_fetches_nothing() {
    let html = page_of(
        "<link rel=stylesheet href=a.css>\
         <script>var cs = getComputedStyle(document.documentElement);\
         window.seen = cs.color + ' ' + cs.backgroundColor + ' ' + cs.fontSize;</script>",
        "<p>x</p>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            (
                "a.css",
                b"@import url('b.css'); html{color:rgb(1,2,3)}".to_vec(),
            ),
            (
                "b.css",
                b"@import 'c.css'; html{background-color:rgb(4,5,6)}".to_vec(),
            ),
            ("c.css", b"html{font-size:21px}".to_vec()),
        ],
        &[("/c.css", Duration::from_millis(300))],
    )
    .await;
    let (page, took) = rig.open("/index.html", Settle::Load).await;
    assert!(
        took >= Duration::from_millis(250),
        "the deepest import was waited for: {took:?}"
    );
    assert_eq!(
        text(&page, "window.seen").await,
        "rgb(1, 2, 3) rgb(4, 5, 6) 21px"
    );
    let served = rig.server.requests_served();
    assert_eq!(served, 4, "the document and three sheets");
    page.screenshot(Default::default())
        .await
        .expect("a screenshot");
    assert_eq!(
        rig.server.requests_served(),
        served,
        "the capture fetched nothing"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_import_chain_past_the_depth_limit_is_cut_and_reported() {
    let mut files = vec![(
        "index.html".to_owned(),
        page_of("<link rel=stylesheet href=s0.css>", "<p>x</p>"),
    )];
    for n in 0..8 {
        files.push((
            format!("s{n}.css"),
            format!("@import url('s{}.css'); html{{--d{n}:1}}", n + 1).into_bytes(),
        ));
    }
    files.push(("s8.css".to_owned(), b"html{--d8:1}".to_vec()));
    let rig = Rig::new(
        files
            .iter()
            .map(|(name, body)| (name.as_str(), body.clone()))
            .collect(),
        &[],
    )
    .await;
    let page = rig.new_page().await;
    let mut events = page.events();
    let (page, _) = rig.go(page, "/index.html", Settle::Load).await;
    let reasons: Vec<String> = events
        .drain()
        .into_iter()
        .filter_map(|event| match event {
            PageEvent::StylesheetFailed { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("nest deeper"), "{reasons:?}");
    // Depth 4 below the linked sheet is followed, the rest is not.
    assert_eq!(
        text(
            &page,
            "var cs = getComputedStyle(document.documentElement); \
             [0,1,2,3,4,5,6,7,8].map(function (n) { return cs.getPropertyValue('--d' + n); }).join(',')"
        )
        .await,
        "1,1,1,1,1,,,,"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alternate_and_print_sheets_neither_apply_nor_block_and_a_disabled_one_is_not_fetched() {
    let html = page_of(
        "<link rel='alternate stylesheet' title=x href=alt.css>\
         <link rel=stylesheet media=print href=print.css>\
         <link rel=stylesheet href=off.css disabled>\
         <script>window.seen = getComputedStyle(document.documentElement).color;</script>",
        "",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("alt.css", b"html{color:rgb(9,9,9)}".to_vec()),
            ("print.css", b"html{color:rgb(8,8,8)}".to_vec()),
            ("off.css", b"html{color:rgb(7,7,7)}".to_vec()),
        ],
        &[],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(text(&page, "window.seen").await, "rgb(0, 0, 0)");
    assert_eq!(
        rig.server.requests_served(),
        3,
        "the document, the alternate and the print sheet; not the disabled one"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_sheet_and_document_style_sheets_reflect_what_loaded() {
    let html = page_of(
        "<style id=inline>p{color:red} .x{margin:0}</style>\
         <link id=good rel=stylesheet href=good.css>\
         <link id=bad rel=stylesheet href=missing.css>\
         <script>window.out = [];\
         var good = document.getElementById('good');\
         out.push('sheet-after-block=' + (good.sheet !== null));</script>",
        "<script>\
         var sheets = document.styleSheets;\
         out.push('length=' + sheets.length + ' same=' + (sheets === document.styleSheets));\
         out.push('inline=' + sheets[0].cssRules.length + ' owner=' + (sheets[0].ownerNode === document.getElementById('inline'))\
           + ' href=' + sheets[0].href + ' same=' + (sheets[0] === document.getElementById('inline').sheet));\
         out.push('good=' + sheets[1].cssRules.length + ' owner=' + (sheets[1].ownerNode === good)\
           + ' href=' + (sheets[1].href || '').split('/').pop() + ' same=' + (sheets[1] === good.sheet));\
         out.push('bad=' + document.getElementById('bad').sheet);\
         out.push('detached=' + document.createElement('style').sheet);\
         out.push('rule=' + sheets[0].cssRules[0].selectorText + ' ' + sheets[1].cssRules.length);\
         </script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            (
                "good.css",
                b"a{color:blue} b{color:green} @media print{i{color:red}}".to_vec(),
            ),
        ],
        &[],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(
        text(&page, "out.join(' | ')").await,
        "sheet-after-block=true \
         | length=2 same=true \
         | inline=2 owner=true href=null same=true \
         | good=3 owner=true href=good.css same=true \
         | bad=null \
         | detached=null \
         | rule=p 3"
    );
    rig.finish();
}

/// A one-purpose HTTP/1.1 server: `path -> (content type, extra header, body)`.
struct Origin {
    base: String,
}

impl Origin {
    async fn start(routes: Vec<(&'static str, &'static str, &'static str, String)>) -> Origin {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes = Arc::new(routes);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let routes = Arc::clone(&routes);
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut chunk = [0_u8; 2048];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let text = String::from_utf8_lossy(&head).to_string();
                    let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let response = match routes.iter().find(|(p, ..)| *p == path) {
                        Some((_, content_type, header, body)) => format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\n{header}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        ),
                        None => "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned(),
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Origin { base }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cross_origin_sheet_applies_but_its_rules_are_sealed() {
    let other = Origin::start(vec![
        (
            "/secret.css",
            "text/css",
            "",
            "@import url('/inner.css'); .secret{content:\"TOKEN-12345\"} html{color:rgb(1,2,3)}"
                .to_owned(),
        ),
        (
            "/inner.css",
            "text/css",
            "",
            "html{background-color:rgb(4,5,6)}".to_owned(),
        ),
        (
            "/cors.css",
            "text/css",
            "access-control-allow-origin: *\r\n",
            "html{color:rgb(7,8,9)}".to_owned(),
        ),
        (
            "/nocors.css",
            "text/css",
            "",
            "html{color:rgb(9,9,9)}".to_owned(),
        ),
        ("/plain.txt", "text/plain", "", "html{color:red}".to_owned()),
    ])
    .await;
    let html = page_of(
        &format!(
            "<link id=a rel=stylesheet href='{}'>\
             <link id=c rel=stylesheet crossorigin href='{}'>\
             <link id=n rel=stylesheet crossorigin href='{}'>",
            other.url("/secret.css"),
            other.url("/cors.css"),
            other.url("/nocors.css")
        ),
        "<script>\
         function t(f) { try { return String(f()); } catch (e) { return e.name; } }\
         var sa = document.getElementById('a').sheet, sc = document.getElementById('c').sheet;\
         window.out = [sa !== null, t(function () { return sa.cssRules.length; }),\
           t(function () { return sa.insertRule('p{}', 0); }), t(function () { return sa.deleteRule(0); }),\
           sc !== null && sc.cssRules.length, document.getElementById('n').sheet,\
           getComputedStyle(document.documentElement).backgroundColor,\
           document.styleSheets.length].join(' | ');\
         </script>",
    );
    let rig = Rig::new(vec![("index.html", html)], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    // `#n` asked for CORS and the answer carries no allow-origin: it failed.
    assert_eq!(
        text(&page, "window.out").await,
        "true | SecurityError | SecurityError | SecurityError | 1 |  | rgb(4, 5, 6) | 2"
    );
    // The cross-origin sheet's rules still applied (the last one wins the colour).
    let colour = text(&page, "getComputedStyle(document.documentElement).color").await;
    assert_eq!(colour, "rgb(7, 8, 9)");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standards_mode_wants_a_css_type_and_quirks_tolerance_is_same_origin_only() {
    let other = Origin::start(vec![
        (
            "/plain.txt",
            "text/plain",
            "",
            "html{color:rgb(1,1,1)}".to_owned(),
        ),
        (
            "/bare.css",
            "text/plain",
            "",
            "html{color:rgb(2,2,2)}".to_owned(),
        ),
    ])
    .await;
    let quirks = format!(
        "<html><head>{LOGGER}<link rel=stylesheet href='{}'><link rel=stylesheet href=local.txt></head><body></body></html>",
        other.url("/plain.txt")
    )
    .into_bytes();
    let standards = page_of(
        &format!("<link rel=stylesheet href='{}'>", other.url("/bare.css")),
        "",
    );
    let rig = Rig::new(
        vec![
            ("quirks.html", quirks),
            ("standards.html", standards),
            ("local.txt", b"html{color:rgb(3,3,3)}".to_vec()),
        ],
        &[],
    )
    .await;
    let (page, _) = rig.open("/quirks.html", Settle::Load).await;
    let log = text(
        &page,
        "log.map(function (l) { return l.split('/').pop(); }).sort().join(',')",
    )
    .await;
    assert_eq!(
        log, "local.txt load,plain.txt error",
        "cross-origin never gets the tolerance"
    );
    let (page, _) = rig.open("/standards.html", Settle::Load).await;
    assert!(text(&page, "log.join(',')").await.ends_with("error"));
    rig.finish();
}

/// The server holds the rest of the body back until the stylesheet has been
/// asked for, so the order of the two is a fact of the page and not of the
/// clock: a page that asks for a stylesheet only once the body has ended never
/// releases the body, and the wait below gives up and says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stylesheet_is_requested_while_the_body_is_still_streaming() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = Arc::clone(&seen);
    let sheet_asked = Arc::new(tokio::sync::Notify::new());
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let log = Arc::clone(&log);
            let sheet_asked = Arc::clone(&sheet_asked);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut chunk = [0_u8; 2048];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&chunk[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&head).to_string();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                if path == "/doc.html" {
                    let first = "<!doctype html><html><head><meta charset=utf-8>\
                                 <link rel=stylesheet href=/s.css></head><body><p>first</p>";
                    let _ = stream
                        .write_all(
                            format!("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\nconnection: close\r\n\r\n{first}{}", " ".repeat(2048))
                                .as_bytes(),
                        )
                        .await;
                    let _ = stream.flush().await;
                    // The permit is stored when the sheet is asked for first,
                    // so the order of the two tasks does not matter.
                    let released = tokio::time::timeout(PATIENCE, sheet_asked.notified())
                        .await
                        .is_ok();
                    log.lock().unwrap().push(
                        if released {
                            "the body ended after the sheet was asked for"
                        } else {
                            "the body ended without the sheet being asked for"
                        }
                        .to_owned(),
                    );
                    let _ = stream.write_all(b"<p>second</p></body></html>").await;
                } else {
                    log.lock().unwrap().push(path);
                    sheet_asked.notify_one();
                    let body = "p{color:red}";
                    let _ = stream
                        .write_all(
                            format!("HTTP/1.1 200 OK\r\ncontent-type: text/css\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
                                .as_bytes(),
                        )
                        .await;
                }
            });
        }
    });
    let rig = Rig::new(vec![("unused.html", b"x".to_vec())], &[]).await;
    let page = rig.new_page().await;
    page.goto(
        &format!("{base}/doc.html"),
        Goto::default()
            .wait_until(Settle::Load)
            .timeout(PATIENCE * 2),
    )
    .await
    .expect("navigation");
    assert_eq!(
        *seen.lock().unwrap(),
        ["/s.css", "the body ended after the sheet was asked for"]
    );
    rig.finish();
}
