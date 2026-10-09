//! Images in the page runtime (IM4): an image loads when its element enters
//! the document, its `load` and `error` events reach script before the
//! window's `load`, the document's `load` waits for it, and
//! `complete`/`naturalWidth`/`currentSrc`/`decode()` follow the load.
//!
//! Every page is served by the in-process fixture server and runs the V8
//! engine; the logs the pages keep are read back with `evaluate`.

#![cfg(all(feature = "render", feature = "v8"))]

use std::time::{Duration, Instant};

use tenta_browser::{
    BrowserContext, ContextOptions, Engine, EvalOptions, Goto, JsEngine, Page, PageOptions,
    ScreenshotOptions, Settle, Viewport,
};
use tenta_core::{FsPolicy, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

const PATIENCE: Duration = Duration::from_secs(60);
const RED: [u8; 4] = [255, 0, 0, 255];

fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    let data: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
    writer.write_image_data(&data).expect("png pixels");
    drop(writer);
    out
}

fn page_of(head: &str, body: &str) -> Vec<u8> {
    format!(
        "<!doctype html><html><head><style>html,body{{margin:0}}{head}</style></head>\
         <body>{body}</body></html>"
    )
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
        Self::build(files, delays, Tier::Render, true).await
    }

    /// `permissive` lets the context reach loopback; the default policy does not.
    async fn build(
        files: Vec<(&str, Vec<u8>)>,
        delays: &[(&str, Duration)],
        tier: Tier,
        permissive: bool,
    ) -> Rig {
        let temp = tenta_testutil::temp_dir("image-events");
        for (name, contents) in files {
            temp.write(name, contents);
        }
        let mut config = FixtureServerConfig::new(temp.path());
        for (glob, delay) in delays {
            config = config.delay_path(glob, *delay);
        }
        let server = FixtureServer::start(config).await.expect("fixture server");
        let engine = Engine::builder()
            .max_tier(tier)
            .io_threads(1)
            .page_threads(1)
            .fs_policy(FsPolicy::none().allow_read(temp.path()))
            .js(JsEngine::V8)
            .ephemeral(true)
            .build()
            .expect("engine");
        let context = engine
            .new_context(ContextOptions {
                allow_private_network: permissive,
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

    /// Navigates to `path`, waiting for `settle`, and says how long that took.
    async fn open(&self, path: &str, settle: Settle) -> (Page, Duration) {
        let page = self
            .context
            .new_page(PageOptions {
                javascript: true,
                viewport: Viewport {
                    width: 40,
                    height: 40,
                    device_scale_factor: 1.0,
                    mobile: false,
                },
                ..PageOptions::default()
            })
            .await
            .expect("page");
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

async fn awaited(page: &Page, expression: &str) -> String {
    page.evaluate_with(
        expression,
        EvalOptions {
            await_promise: true,
            ..EvalOptions::default()
        },
    )
    .await
    .expect("ran")
    .into_value::<String>()
    .expect("a string")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn onload_fires_before_the_window_load_and_the_natural_size_is_known() {
    let html = page_of(
        "",
        "<img id=a src='a.png'>\
         <script>\
         var log = [];\
         var a = document.getElementById('a');\
         a.onload = function (e) {\
           log.push('onload ' + a.naturalWidth + 'x' + a.naturalHeight + ' complete=' + a.complete\
             + ' ' + a.currentSrc.split('/').pop() + ' trusted=' + e.isTrusted + ' bubbles=' + e.bubbles);\
         };\
         a.addEventListener('load', function () { log.push('listener'); });\
         window.addEventListener('load', function () { log.push('window complete=' + a.complete); });\
         log.push('script complete=' + a.complete + ' natural=' + a.naturalWidth);\
         </script>",
    );
    let rig = Rig::new(vec![("index.html", html), ("a.png", png(20, 10, RED))], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(
        text(&page, "log.join(' | ')").await,
        "script complete=false natural=0 \
         | onload 20x10 complete=true a.png trusted=true bubbles=false \
         | listener \
         | window complete=true"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_waits_for_a_slow_image_but_not_for_a_lazy_one_far_away() {
    let html = page_of(
        "",
        "<img id=slow src='slow.png'>\
         <div style='height:6000px'></div>\
         <img id=far src='far.png' loading=lazy>\
         <script>\
         var log = [];\
         document.getElementById('slow').onload = function () { log.push('slow'); };\
         document.getElementById('far').onload = function () { log.push('far'); };\
         window.addEventListener('load', function () { log.push('window'); });\
         </script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("slow.png", png(4, 4, RED)),
            ("far.png", png(4, 4, RED)),
        ],
        &[
            ("/slow.png", Duration::from_millis(1200)),
            ("/far.png", Duration::from_secs(600)),
        ],
    )
    .await;
    let (page, took) = rig.open("/index.html", Settle::Load).await;
    assert!(
        took >= Duration::from_millis(1100),
        "load waited for the slow image: {took:?}"
    );
    assert!(
        took < Duration::from_secs(4),
        "and not for the lazy one: {took:?}"
    );
    assert_eq!(text(&page, "log.join(',')").await, "slow,window");
    assert_eq!(
        text(&page, "String(document.getElementById('far').complete)").await,
        "false",
        "the lazy image is left to the capture"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_and_a_corrupt_image_fire_error_before_the_window_load() {
    let html = page_of(
        "",
        "<img id=missing src='missing.png'><img id=corrupt src='corrupt.png'>\
         <script>\
         var log = [];\
         ['missing', 'corrupt'].forEach(function (id) {\
           var img = document.getElementById(id);\
           img.onerror = function (e) { log.push(id + ' error complete=' + img.complete\
             + ' natural=' + img.naturalWidth); };\
           img.onload = function () { log.push(id + ' LOAD'); };\
         });\
         window.addEventListener('load', function () { log.push('window'); });\
         </script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("corrupt.png", b"\x89PNG\r\n\x1a\ngarbage".to_vec()),
        ],
        &[],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    let log = text(&page, "log.slice().sort().join(',')").await;
    assert_eq!(
        log,
        "corrupt error complete=true natural=0,missing error complete=true natural=0,window"
    );
    assert_eq!(
        text(&page, "log[log.length - 1]").await,
        "window",
        "the window's load comes last"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_preloader_image_that_is_never_attached_still_loads() {
    let html = page_of(
        "",
        "<script>\
         var log = [];\
         var p = new Image();\
         p.onload = function () { log.push('preload ' + p.naturalWidth + 'x' + p.naturalHeight); };\
         p.src = 'pre.png';\
         log.push('sync complete=' + p.complete + ' connected=' + p.isConnected);\
         window.addEventListener('load', function () { log.push('window'); });\
         </script>",
    );
    let rig = Rig::new(vec![("index.html", html), ("pre.png", png(8, 6, RED))], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(
        text(&page, "log.join(' | ')").await,
        "sync complete=false connected=false | preload 8x6 | window"
    );
    assert_eq!(
        rig.server.requests_served(),
        2,
        "the document and one fetch"
    );
    assert_eq!(
        text(
            &page,
            "var s = new Image(3, 4); [s instanceof HTMLImageElement, s.width, s.height, \
             Image.prototype === HTMLImageElement.prototype, s.complete].join()"
        )
        .await,
        "true,3,4,true,true"
    );
    assert!(
        page.evaluate("Image()")
            .await
            .expect("ran")
            .exception()
            .is_some(),
        "the constructor needs new"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_image_added_after_load_loads_and_decode_follows_it() {
    let html = page_of("", "<img id=ok src='a.png'><img id=bad src='missing.png'>");
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("a.png", png(5, 7, RED)),
            ("later.png", png(9, 3, RED)),
        ],
        &[("/later.png", Duration::from_millis(300))],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(
        awaited(
            &page,
            "document.getElementById('ok').decode().then(function () { return 'resolved'; })"
        )
        .await,
        "resolved"
    );
    assert_eq!(
        awaited(
            &page,
            "document.getElementById('bad').decode().then(function () { return 'resolved'; }, \
             function (e) { return e.name; })"
        )
        .await,
        "EncodingError"
    );
    // Pending when decode() is called, settled by the load that follows.
    assert_eq!(
        awaited(
            &page,
            "(function () { var i = new Image(); i.src = 'later.png'; \
             var before = i.complete; \
             return i.decode().then(function () { return before + ' ' + i.naturalWidth + 'x' + i.naturalHeight; }); })()"
        )
        .await,
        "false 9x3"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_src_loads_the_new_image_and_fires_load_again() {
    let html = page_of(
        "",
        "<img id=a src='a.png'>\
         <script>\
         var log = [];\
         var a = document.getElementById('a');\
         a.onload = function () { log.push(a.currentSrc.split('/').pop() + ' ' + a.naturalWidth); };\
         </script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("a.png", png(5, 7, RED)),
            ("b.png", png(11, 2, RED)),
        ],
        &[],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    let _ = text(&page, "a.src = 'b.png'; String(a.complete)").await;
    // The reload is a task of its own; wait for its event.
    let log = awaited(
        &page,
        "new Promise(function (resolve) { var t = setInterval(function () { \
           if (log.length >= 2) { clearInterval(t); resolve(log.join(',')); } }, 10); })",
    )
    .await;
    assert_eq!(log, "a.png 5,b.png 11");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fade_in_page_shows_the_image_once_onload_has_run() {
    let html = page_of(
        "img{display:block;width:10px;height:10px;opacity:0}img.loaded{opacity:1}",
        "<img id=f src='red.png'>\
         <script>\
         var f = document.getElementById('f');\
         f.onload = function () { f.classList.add('loaded'); };\
         </script>",
    );
    let rig = Rig::new(
        vec![("index.html", html), ("red.png", png(10, 10, RED))],
        &[],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    let shot = page
        .screenshot(ScreenshotOptions::default())
        .await
        .expect("screenshot");
    let mut reader = png::Decoder::new(std::io::Cursor::new(&shot.bytes))
        .read_info()
        .expect("a PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("sized")];
    let info = reader.next_frame(&mut rgba).expect("frame");
    let at = ((5 * info.width + 5) * 4) as usize;
    assert_eq!(&rgba[at..at + 4], &RED, "the faded-in image is drawn");
    assert_eq!(
        rig.server.requests_served(),
        2,
        "the capture reused the loaded image"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_with_no_images_creates_no_render_session() {
    let rig = Rig::new(vec![("index.html", page_of("", "<p>text</p>"))], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert!(
        page.render_counters().await.expect("counters").is_none(),
        "nothing asked for a session"
    );
    rig.finish();
}

/// The order headless Chrome 154 reports for this page (recorded with
/// `--dump-dom` from a `file:` copy of it):
///
/// `script a.complete=false p.complete=false | DCL | a.onload 20x10
/// complete=true trusted=true bubbles=false | a.listener | m.onerror
/// complete=true nat=0 | pre 8x6 | window a.complete=true m.complete=true`
///
/// Which of the three images ends first depends on the network, in Chrome as
/// here, so the images' own order among themselves is not pinned; everything
/// else is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_event_order_matches_headless_chrome() {
    let html = page_of(
        "",
        "<img id=a src='a.png'><img id=m src='missing.png'>\
         <script>\
         var log = [];\
         var a = document.getElementById('a'), m = document.getElementById('m');\
         a.onload = function (e) { log.push('a.onload ' + a.naturalWidth + 'x' + a.naturalHeight\
           + ' complete=' + a.complete + ' trusted=' + e.isTrusted + ' bubbles=' + e.bubbles); };\
         a.addEventListener('load', function () { log.push('a.listener'); });\
         m.onerror = function () { log.push('m.onerror complete=' + m.complete + ' nat=' + m.naturalWidth); };\
         var p = new Image();\
         p.onload = function () { log.push('pre ' + p.naturalWidth + 'x' + p.naturalHeight); };\
         p.src = 'pre.png';\
         log.push('script a.complete=' + a.complete + ' p.complete=' + p.complete);\
         document.addEventListener('DOMContentLoaded', function () { log.push('DCL'); });\
         window.addEventListener('load', function () {\
           log.push('window a.complete=' + a.complete + ' m.complete=' + m.complete); });\
         </script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("a.png", png(20, 10, RED)),
            ("pre.png", png(8, 6, RED)),
        ],
        &[],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    let log = text(&page, "log.join(' | ')").await;
    let entries: Vec<&str> = log.split(" | ").collect();
    assert_eq!(entries.len(), 7, "{log}");
    assert_eq!(entries[0], "script a.complete=false p.complete=false");
    assert_eq!(entries[1], "DCL");
    assert_eq!(entries[6], "window a.complete=true m.complete=true");
    let middle: Vec<&str> = entries[2..6].to_vec();
    let at = |prefix: &str| {
        middle
            .iter()
            .position(|e| e.starts_with(prefix))
            .expect(prefix)
    };
    assert_eq!(
        middle[at("a.onload")],
        "a.onload 20x10 complete=true trusted=true bubbles=false"
    );
    assert_eq!(
        at("a.listener"),
        at("a.onload") + 1,
        "handler before listener: {log}"
    );
    assert_eq!(middle[at("m.onerror")], "m.onerror complete=true nat=0");
    assert_eq!(middle[at("pre")], "pre 8x6");
    rig.finish();
}

// ---------------------------------------------------------------- the critic's cases

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreferenced_preloader_still_loads_and_fires() {
    let html = page_of(
        "",
        "<script>\
         window.log = [];\
         (function () {\
           var p = new Image();\
           p.onload = function () { log.push('onload ' + this.naturalWidth); };\
           p.src = 'pix.png?a';\
         })();\
         </script>",
    );
    let rig = Rig::new(vec![("index.html", html), ("pix.png", png(6, 2, RED))], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(text(&page, "log.join(',')").await, "onload 6");
    assert_eq!(
        rig.server.requests_served(),
        2,
        "the document and the image"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loop_of_unreferenced_images_makes_one_request_each() {
    let html = page_of(
        "",
        "<script>\
         window.log = [];\
         for (var i = 0; i < 200; i++) { var p = new Image(); p.onload = function () { log.push(1); }; p.src = 'pix.png?i=' + i; }\
         </script>",
    );
    let rig = Rig::new(vec![("index.html", html), ("pix.png", png(2, 2, RED))], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    // `load` stops waiting for a slow image after the cap, so a loaded machine
    // can reach it first: wait for the events themselves.
    let _ = awaited(
        &page,
        "new Promise(function (resolve) { var t = setInterval(function () { if (log.length >= 200) { clearInterval(t); resolve('done'); } }, 20); })",
    )
    .await;
    assert_eq!(text(&page, "String(log.length)").await, "200");
    assert_eq!(rig.server.requests_served(), 201);
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn more_pending_images_than_the_cap_fail_the_excess_with_error() {
    let html = page_of(
        "",
        "<script>\
         window.log = [];\
         for (var i = 0; i < 700; i++) { var p = new Image(); p.onload = function () { log.push('l'); };\
           p.onerror = function () { log.push('e'); }; p.src = 'pix.png?i=' + i; }\
         </script>",
    );
    let rig = Rig::new(vec![("index.html", html), ("pix.png", png(2, 2, RED))], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    let _ = awaited(
        &page,
        "new Promise(function (resolve) { var t = setInterval(function () { if (log.length >= 700) { clearInterval(t); resolve('done'); } }, 20); })",
    )
    .await;
    let counts = text(
        &page,
        "log.filter(function (x) { return x === 'l'; }).length + '/' + log.filter(function (x) { return x === 'e'; }).length",
    )
    .await;
    assert_eq!(counts, "512/188");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inputs_that_are_not_images_create_no_render_session() {
    let html = page_of(
        "",
        "<form><input name=a><input type=text><video></video><button>go</button></form>\
         <script>document.querySelector('input').setAttribute('value', 'x');</script>",
    );
    let rig = Rig::new(vec![("index.html", html)], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert!(page.render_counters().await.expect("counters").is_none());
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_load_for_a_source_the_element_has_left_is_not_reported() {
    let html = page_of(
        "",
        "<img id=x src='a.png'><img id=y src='a.png'>\
         <script>\
         var log = [];\
         var x = document.getElementById('x'), y = document.getElementById('y');\
         x.onload = function () { log.push('x'); y.src = 'b.png'; };\
         y.onload = function () { log.push('y ' + y.currentSrc.split('/').pop()); };\
         </script>",
    );
    let rig = Rig::new(
        vec![
            ("index.html", html),
            ("a.png", png(2, 2, RED)),
            ("b.png", png(3, 3, RED)),
        ],
        &[],
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    let log = awaited(
        &page,
        "new Promise(function (resolve) { var t = setInterval(function () { \
           if (log.length >= 2) { clearInterval(t); resolve(log.join(',')); } }, 10); })",
    )
    .await;
    assert_eq!(log, "x,y b.png", "y never reports the a.png it left");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_image_constructor_looks_native() {
    let rig = Rig::new(vec![("index.html", page_of("", "<p>x</p>"))], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    let probe = text(
        &page,
        "(function () {\
           var out = [];\
           out.push(Image.length, Image.name);\
           var d = Object.getOwnPropertyDescriptor(Image, 'prototype');\
           out.push(d.writable, d.configurable);\
           out.push(String(Image));\
           out.push(new Image(undefined).hasAttribute('width'));\
           out.push(new Image(undefined, 7).getAttribute('height'));\
           var create = document.createElement;\
           document.createElement = function () { throw new Error('patched'); };\
           try { out.push(new Image() instanceof HTMLImageElement); } catch (e) { out.push('threw'); }\
           document.createElement = create;\
           class Sub extends Image {}\
           out.push(new Sub() instanceof Sub);\
           return out.join('|');\
         })()",
    )
    .await;
    assert_eq!(
        probe,
        "0|Image|false|false|function Image() { [native code] }|false|7|true|true"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn below_the_render_tier_images_are_complete_and_decode_rejects() {
    let html = page_of("", "<img id=a src='a.png'>");
    let rig = Rig::build(
        vec![("index.html", html), ("a.png", png(2, 2, RED))],
        &[],
        Tier::Js,
        true,
    )
    .await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(rig.server.requests_served(), 1, "no image is fetched");
    assert_eq!(
        awaited(
            &page,
            "var a = document.getElementById('a'); \
             a.decode().then(function () { return 'resolved'; }, function (e) { return a.complete + ' ' + e.name; })"
        )
        .await,
        "true EncodingError"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lazy_image_is_incomplete_until_a_capture_loads_it_and_decode_waits() {
    let html = page_of(
        "",
        "<img id=l src='a.png' loading=lazy style='display:block;width:10px;height:10px'>\
         <script>\
         var log = [];\
         var l = document.getElementById('l');\
         l.onload = function () { log.push('load ' + l.naturalWidth); };\
         window.decP = l.decode().then(function () { return 'resolved'; });\
         </script>",
    );
    let rig = Rig::new(vec![("index.html", html), ("a.png", png(4, 4, RED))], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(text(&page, "String(l.complete)").await, "false");
    let _ = page
        .screenshot(ScreenshotOptions::default())
        .await
        .expect("screenshot");
    assert_eq!(awaited(&page, "decP").await, "resolved");
    assert_eq!(
        text(&page, "l.complete + ' ' + log.join()").await,
        "true load 4"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_policy_blocks_loopback_and_metadata_addresses() {
    let rig = Rig::build(vec![("a.png", png(2, 2, RED))], &[], Tier::Render, false).await;
    let page = rig
        .context
        .new_page(PageOptions {
            javascript: true,
            ..PageOptions::default()
        })
        .await
        .expect("page");
    let started = text(
        &page,
        &format!(
            "window.log = [];\
             ['{}', 'http://169.254.169.254/x.png'].forEach(function (u) {{\
               var p = new Image(); p.onerror = function () {{ log.push('error'); }};\
               p.onload = function () {{ log.push('LOAD'); }}; p.src = u; }});\
             'started'",
            rig.server.url("/a.png")
        ),
    )
    .await;
    assert_eq!(started, "started");
    let log = awaited(
        &page,
        "new Promise(function (resolve) { var t = setInterval(function () { \
           if (log.length >= 2) { clearInterval(t); resolve(log.join(',')); } }, 10); })",
    )
    .await;
    assert_eq!(log, "error,error");
    assert_eq!(rig.server.requests_served(), 0, "nothing reached loopback");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_image_never_reports_into_the_next_document() {
    let first = page_of(
        "",
        "<img id=a src='slow.png'><script>document.getElementById('a').onload = function () { window.leak = 1; };</script>",
    );
    let second = page_of(
        "",
        "<img id=a><script>window.log = []; var a = document.getElementById('a');\
         a.onload = function () { log.push('LEAK'); }; a.onerror = function () { log.push('LEAK'); };</script>",
    );
    let rig = Rig::new(
        vec![
            ("one.html", first),
            ("two.html", second),
            ("slow.png", png(2, 2, RED)),
        ],
        &[("/slow.png", Duration::from_millis(600))],
    )
    .await;
    let page = rig
        .context
        .new_page(PageOptions {
            javascript: true,
            ..PageOptions::default()
        })
        .await
        .expect("page");
    page.goto(
        &rig.server.url("/one.html"),
        Goto::default()
            .wait_until(Settle::DomContent)
            .timeout(PATIENCE),
    )
    .await
    .expect("first navigation");
    page.goto(
        &rig.server.url("/two.html"),
        Goto::default().wait_until(Settle::Load).timeout(PATIENCE),
    )
    .await
    .expect("second navigation");
    let log = awaited(
        &page,
        "new Promise(function (resolve) { setTimeout(function () { resolve(log.join()); }, 1000); })",
    )
    .await;
    assert_eq!(log, "", "the old document's image reported nowhere");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cloned_image_loads_while_detached() {
    let html = page_of(
        "",
        "<img id=a src='a.png'><script>\
         var log = [];\
         var c = document.getElementById('a').cloneNode(true);\
         c.onload = function () { log.push('clone ' + c.naturalWidth + ' ' + c.isConnected); };\
         </script>",
    );
    let rig = Rig::new(vec![("index.html", html), ("a.png", png(5, 5, RED))], &[]).await;
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    assert_eq!(text(&page, "log.join()").await, "clone 5 false");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_throwing_onload_is_reported_to_the_page_like_any_handler() {
    let html = page_of(
        "",
        "<img id=a src='a.png'><script>\
         document.getElementById('a').onload = function () { throw new Error('boom from onload'); };\
         </script>",
    );
    let rig = Rig::new(vec![("index.html", html), ("a.png", png(2, 2, RED))], &[]).await;
    let page = rig
        .context
        .new_page(PageOptions {
            javascript: true,
            ..PageOptions::default()
        })
        .await
        .expect("page");
    let mut events = page.events();
    page.goto(
        &rig.server.url("/index.html"),
        Goto::default().wait_until(Settle::Load).timeout(PATIENCE),
    )
    .await
    .expect("navigation");
    let reported = events.drain().into_iter().any(|event| {
        matches!(&event, tenta_browser::PageEvent::Exception(e) if format!("{e:?}").contains("boom from onload"))
    });
    assert!(reported, "the handler's exception reached the page events");
    rig.finish();
}

/// Makes `count` unreferenced preloaders, each with an `onload` and an
/// `onerror` that count themselves, and returns how long `goto` and the wait for
/// every event took.
async fn flood(count: usize) -> (Duration, String) {
    let html = page_of(
        "",
        &format!(
            "<script>window.log = []; window.n = 0;\
             for (var i = 0; i < {count}; i++) {{ var p = new Image();\
               p.onload = function () {{ n++; }}; p.onerror = function () {{ n++; }};\
               p.src = 'pix.png?i=' + i; }}</script>"
        ),
    );
    let rig = Rig::new(vec![("index.html", html), ("pix.png", png(2, 2, RED))], &[]).await;
    let started = Instant::now();
    let (page, _) = rig.open("/index.html", Settle::Load).await;
    let _ = awaited(
        &page,
        &format!(
            "new Promise(function (resolve) {{ var t = setInterval(function () {{ \
               if (n >= {count}) {{ clearInterval(t); resolve('done'); }} }}, 5); }})"
        ),
    )
    .await;
    let took = started.elapsed();
    let served = rig.server.requests_served();
    rig.finish();
    (took, format!("{served} requests"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thousand_image_events_are_all_delivered() {
    // Counted, not timed: the cost of delivery is pinned engine-side by
    // `tenta-js-v8`'s `single_event_entries_stay_cheap_however_many_elements_are_pinned`.
    let (_, served) = flood(1000).await;
    assert_eq!(
        served, "513 requests",
        "the cap bounds the requests, not the events"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_thousand_unreferenced_images_do_not_crash_the_page() {
    // `flood` waits for all 5000 events and evaluates in the page afterwards, so
    // reaching the end means the script budget held and the page survived.
    let (_, served) = flood(5000).await;
    assert_eq!(served, "513 requests");
}
