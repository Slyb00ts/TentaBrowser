//! `Page::screenshot` on real pages served by the in-process fixture server:
//! pixels, sizes, stylesheets, captures, ordering against other entries and
//! the typed failures (`docs/spec/11-library-api.md` §1).

#![cfg(feature = "render")]

use std::io::Cursor;
use std::time::Duration;

use tenta_browser::{
    BrowserContext, Capture, Clip, ContextOptions, Engine, Goto, ImageFormat, Page, PageEvent,
    PageEvents, PageOptions, Screenshot, ScreenshotOptions, Settle, Viewport,
};
use tenta_core::{BudgetKind, Error, FsPolicy, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

const PATIENCE: Duration = Duration::from_secs(30);

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
#[cfg(feature = "v8")]
const ORANGE: [u8; 4] = [255, 128, 0, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];

const SOLID: &str = "<!doctype html><html><head><style>\
    html,body{margin:0}body{background:#00ff00}\
    #box{width:100px;height:50px;background:#ff0000}\
    #hidden{display:none}\
    </style></head><body><div id=\"box\"></div><div id=\"hidden\">x</div></body></html>";

const LINKED: &str = "<!doctype html><html><head>\
    <style>html,body{margin:0}</style>\
    <link rel=\"stylesheet\" href=\"site.css\"></head>\
    <body><div class=\"swatch\"></div><script>window.ready = 1;</script></body></html>";

const SITE_CSS: &str = ".swatch{width:80px;height:80px;background:#ff00ff}";

const MISSING_SHEET: &str = "<!doctype html><html><head>\
    <style>html,body{margin:0}#b{width:40px;height:40px;background:#ff0000}</style>\
    <link rel=\"stylesheet\" href=\"nope.css\"></head><body><div id=\"b\"></div></body></html>";

const LONG: &str = "<!doctype html><html><head><style>html,body{margin:0}\
    .top{height:1500px;background:#00ff00}.bottom{height:1500px;background:#0000ff}\
    </style></head><body><div class=\"top\"></div><div class=\"bottom\"></div></body></html>";

const SLOW_SHEET: &str = "<!doctype html><html><head><style>html,body{margin:0}</style>\
    <link rel=\"stylesheet\" href=\"slow.css\"></head><body><p>x</p><script>window.ready = 1;</script></body></html>";

const EDITABLE: &str = "<!doctype html><html><head><style id=\"s\">\
    html,body{margin:0}#b{width:50px;height:50px;background:#ff0000}\
    </style></head><body><div id=\"b\"></div></body></html>";

const MUTATING: &str = "<!doctype html><html><head><style>html,body{margin:0}</style></head>\
    <body><script>\
    document.body.style.cssText = 'background:#0000ff';\
    var made = document.createElement('div');\
    made.style.cssText = 'width:60px;height:60px;background:#ff8000';\
    document.body.appendChild(made);\
    </script></body></html>";

const GATED: &str = "<!doctype html><html><head><style>html,body{margin:0}</style>\
    <link rel=\"stylesheet\" href=\"gate.css\"></head>\
    <body><div class=\"late\"></div><script>window.ready = 1;</script></body></html>";

const LATE_CSS: &str = ".late{width:70px;height:70px;background:#ff0000}";

struct Rig {
    server: FixtureServer,
    context: BrowserContext,
    engine: Engine,
    _files: TempDir,
}

impl Rig {
    async fn new() -> Rig {
        let temp = tenta_testutil::temp_dir("screenshot");
        for (name, contents) in [
            ("solid.html", SOLID),
            ("linked.html", LINKED),
            ("site.css", SITE_CSS),
            ("missing.html", MISSING_SHEET),
            ("long.html", LONG),
            ("slow.html", SLOW_SHEET),
            ("slow.css", SITE_CSS),
            ("editable.html", EDITABLE),
            ("mutating.html", MUTATING),
            ("gated.html", GATED),
            ("gate.css", "html{}"),
            ("late.css", LATE_CSS),
        ] {
            temp.write(name, contents);
        }
        // An http document must not read a local file even from an allowed root.
        temp.write(
            "filelink.html",
            format!(
                "<!doctype html><html><head><style>html,body{{margin:0}}</style>\
                 <link rel=\"stylesheet\" href=\"file://{}\"></head>\
                 <body><div class=\"swatch\"></div></body></html>",
                temp.path().join("site.css").display()
            ),
        );
        let config = FixtureServerConfig::new(temp.path())
            .delay_path("/slow.css", Duration::from_secs(600))
            .delay_path("/gate.css", Duration::from_secs(1));
        let server = FixtureServer::start(config).await.expect("fixture server");
        let builder = Engine::builder()
            .max_tier(Tier::Render)
            .io_threads(1)
            .page_threads(1)
            .fs_policy(FsPolicy::none().allow_read(temp.path()))
            .ephemeral(true);
        #[cfg(feature = "v8")]
        let builder = builder.js(tenta_browser::JsEngine::V8);
        let engine = builder.build().expect("engine");
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

    /// A page that has loaded `path`, with scripts off.
    async fn tier1(&self, path: &str, viewport: Viewport) -> Page {
        self.loaded(path, false, viewport).await
    }

    /// A page that has loaded `path`, with scripts on.
    #[cfg(feature = "v8")]
    async fn scripted(&self, path: &str, viewport: Viewport) -> Page {
        self.loaded(path, true, viewport).await
    }

    async fn loaded(&self, path: &str, javascript: bool, viewport: Viewport) -> Page {
        let page = self
            .context
            .new_page(PageOptions {
                javascript,
                viewport,
                ..PageOptions::default()
            })
            .await
            .expect("page");
        page.goto(
            &self.server.url(path),
            Goto::default().wait_until(Settle::Load).timeout(PATIENCE),
        )
        .await
        .expect("navigation");
        page
    }

    fn finish(self) {
        self.context.close();
        self.engine.close();
    }
}

fn viewport(width: u32, height: u32, device_scale_factor: f32) -> Viewport {
    Viewport {
        width,
        height,
        device_scale_factor,
        mobile: false,
    }
}

struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Decoded {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width && y < self.height, "({x}, {y}) is outside");
        let at = ((y * self.width + x) * 4) as usize;
        self.rgba[at..at + 4].try_into().expect("four channels")
    }
}

fn decode(shot: &Screenshot) -> Decoded {
    assert_eq!(&shot.bytes[..8], b"\x89PNG\r\n\x1a\n", "a PNG signature");
    let mut reader = png::Decoder::new(Cursor::new(&shot.bytes))
        .read_info()
        .expect("a decodable PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("a sized buffer")];
    let info = reader.next_frame(&mut rgba).expect("a frame");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    rgba.truncate(info.buffer_size());
    assert_eq!((info.width, info.height), (shot.width, shot.height));
    Decoded {
        width: info.width,
        height: info.height,
        rgba,
    }
}

async fn capture(page: &Page, capture: Capture) -> Screenshot {
    page.screenshot(ScreenshotOptions {
        capture,
        ..ScreenshotOptions::default()
    })
    .await
    .expect("screenshot")
}

async fn until_served(server: &FixtureServer, requests: u64) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while server.requests_served() < requests {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server never saw request {requests}"
        );
        tokio::task::yield_now().await;
    }
}

fn stylesheet_failures(events: &mut PageEvents) -> Vec<String> {
    events
        .drain()
        .into_iter()
        .filter_map(|event| match event {
            PageEvent::StylesheetFailed { url, .. } => Some(url.path().to_owned()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tier1_page_renders_at_the_viewport_times_the_device_ratio() {
    let rig = Rig::new().await;
    let page = rig.tier1("/solid.html", viewport(320, 200, 2.0)).await;
    let mut events = page.events();

    let shot = capture(&page, Capture::Viewport).await;
    assert_eq!((shot.width, shot.height), (640, 400));
    assert!(!shot.clamped);
    assert!(shot.stats.is_complete(), "{:?}", shot.stats);
    let image = decode(&shot);
    // The 100x50 CSS px box covers 200x100 device pixels from the corner.
    assert_eq!(image.pixel(100, 50), RED);
    assert_eq!(image.pixel(199, 99), RED);
    // The body background reaches the whole canvas.
    assert_eq!(image.pixel(400, 300), GREEN);
    assert_eq!(image.pixel(210, 10), GREEN);
    assert_eq!(page.tier(), Tier::Render);
    assert!(
        events
            .drain()
            .contains(&PageEvent::TierChanged { tier: Tier::Render })
    );

    // The clip scale enlarges the raster and nothing else.
    let scaled = page
        .screenshot(ScreenshotOptions {
            scale: Some(0.5),
            ..ScreenshotOptions::default()
        })
        .await
        .expect("scaled screenshot");
    assert_eq!((scaled.width, scaled.height), (320, 200));

    page.close().await.expect("close");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_linked_stylesheet_is_fetched_and_applied() {
    let rig = Rig::new().await;
    let page = rig.tier1("/linked.html", viewport(200, 120, 1.0)).await;

    let shot = capture(&page, Capture::Viewport).await;
    let image = decode(&shot);
    assert_eq!(image.pixel(40, 40), MAGENTA);
    assert_eq!(image.pixel(120, 40), [255, 255, 255, 255]);
    let counters = page
        .render_counters()
        .await
        .expect("counters")
        .expect("a session");
    assert_eq!(
        counters.source_sets, 1,
        "the sheet arrived before the first style pass"
    );

    page.close().await.expect("close");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stylesheet_that_fails_is_reported_and_the_page_still_renders() {
    let rig = Rig::new().await;
    let page = rig.tier1("/missing.html", viewport(200, 120, 1.0)).await;
    let mut events = page.events();

    let first = decode(&capture(&page, Capture::Viewport).await);
    assert_eq!(first.pixel(20, 20), RED);
    assert_eq!(stylesheet_failures(&mut events), ["/nope.css"]);
    // A 404 is remembered, so it is neither refetched nor reported again.
    capture(&page, Capture::Viewport).await;
    assert!(stylesheet_failures(&mut events).is_empty());

    page.close().await.expect("close");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_jpeg_decodes_to_the_page_it_was_taken_of() {
    let rig = Rig::new().await;
    let page = rig.tier1("/solid.html", viewport(160, 120, 1.0)).await;

    let shot = page
        .screenshot(ScreenshotOptions {
            format: ImageFormat::Jpeg { quality: 90 },
            ..ScreenshotOptions::viewport()
        })
        .await
        .expect("a JPEG");
    assert_eq!((shot.width, shot.height), (160, 120));
    let mut decoder =
        zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(&shot.bytes));
    let rgb = decoder.decode().expect("a decodable JPEG");
    assert_eq!(decoder.dimensions(), Some((160, 120)));
    let at = |x: usize, y: usize| &rgb[(y * 160 + x) * 3..][..3];
    let near = |got: &[u8], want: [u8; 3]| got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 12);
    assert!(near(at(130, 100), [0, 255, 0]), "{:?}", at(130, 100));
    assert!(near(at(120, 20), [0, 255, 0]), "{:?}", at(120, 20));
    assert!(near(at(10, 10), [255, 0, 0]), "{:?}", at(10, 10));

    page.close().await.expect("close");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_http_page_cannot_load_a_local_stylesheet_even_from_an_allowed_root() {
    let rig = Rig::new().await;
    let page = rig.tier1("/filelink.html", viewport(200, 120, 1.0)).await;
    let mut events = page.events();

    let image = decode(&capture(&page, Capture::Viewport).await);
    assert_eq!(
        image.pixel(40, 40),
        [255, 255, 255, 255],
        "the sheet was not applied"
    );
    let failed = stylesheet_failures(&mut events);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].ends_with("/site.css"), "{failed:?}");

    page.close().await.expect("close");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_screenshot_of_an_unchanged_page_does_no_work() {
    let rig = Rig::new().await;
    let page = rig.tier1("/linked.html", viewport(200, 120, 1.0)).await;
    assert_eq!(page.render_counters().await.expect("counters"), None);

    let first = capture(&page, Capture::Viewport).await;
    let after_first = page
        .render_counters()
        .await
        .expect("counters")
        .expect("a session");
    assert!(after_first.layouts >= 1 && after_first.display_lists >= 1);
    let served = rig.server.requests_served();

    let second = capture(&page, Capture::Viewport).await;
    let after_second = page
        .render_counters()
        .await
        .expect("counters")
        .expect("a session");
    assert_eq!(
        after_second, after_first,
        "no restyle, layout or display list"
    );
    assert_eq!(rig.server.requests_served(), served, "no refetch");
    assert_eq!(decode(&first).rgba, decode(&second).rgba);

    page.close().await.expect("close");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_page_capture_spans_the_scroll_height() {
    let rig = Rig::new().await;
    let page = rig.tier1("/long.html", viewport(400, 300, 1.0)).await;

    let viewport_shot = capture(&page, Capture::Viewport).await;
    assert_eq!((viewport_shot.width, viewport_shot.height), (400, 300));
    let shot = capture(&page, Capture::FullPage).await;
    assert_eq!((shot.width, shot.height), (400, 3000));
    assert!(!shot.clamped);
    let image = decode(&shot);
    assert_eq!(image.pixel(10, 10), GREEN);
    assert_eq!(image.pixel(10, 1499), GREEN);
    assert_eq!(image.pixel(10, 1500), BLUE);
    assert_eq!(image.pixel(10, 2990), BLUE);

    page.close().await.expect("close");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_element_or_a_clip_is_cut_out_of_the_document() {
    let rig = Rig::new().await;
    let page = rig.tier1("/solid.html", viewport(320, 200, 1.0)).await;

    let element = capture(&page, Capture::Element("#box".to_owned())).await;
    assert_eq!((element.width, element.height), (100, 50));
    let image = decode(&element);
    assert_eq!(image.pixel(0, 0), RED);
    assert_eq!(image.pixel(99, 49), RED);

    let clip = capture(
        &page,
        Capture::Clip(Clip {
            x: 90.0,
            y: 40.0,
            width: 20.0,
            height: 20.0,
        }),
    )
    .await;
    assert_eq!((clip.width, clip.height), (20, 20));
    let image = decode(&clip);
    assert_eq!(image.pixel(5, 5), RED, "inside the box");
    assert_eq!(image.pixel(15, 15), GREEN, "past its corner");

    page.close().await.expect("close");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_that_cannot_be_served_fail_with_typed_errors() {
    let rig = Rig::new().await;
    let page = rig.tier1("/solid.html", viewport(320, 200, 1.0)).await;
    let shoot = |capture, format| {
        page.screenshot(ScreenshotOptions {
            capture,
            format,
            ..ScreenshotOptions::default()
        })
    };

    let error = shoot(Capture::Element("#absent".to_owned()), ImageFormat::Png)
        .await
        .expect_err("no such element");
    assert!(matches!(error, Error::InvalidArgument { .. }), "{error:?}");
    let error = shoot(Capture::Element("#hidden".to_owned()), ImageFormat::Png)
        .await
        .expect_err("display: none has no box");
    assert!(
        matches!(&error, Error::InvalidArgument { reason } if reason.contains("no box")),
        "{error:?}"
    );
    let error = shoot(Capture::Viewport, ImageFormat::Jpeg { quality: 0 })
        .await
        .expect_err("quality is 1 to 100");
    assert!(matches!(error, Error::InvalidArgument { .. }), "{error:?}");
    let error = shoot(Capture::Viewport, ImageFormat::Webp)
        .await
        .expect_err("WebP is not encoded");
    assert!(matches!(error, Error::Unsupported { .. }), "{error:?}");
    let error = page
        .screenshot(ScreenshotOptions::clip(0.0, 0.0, 20_000.0, 20_000.0))
        .await
        .expect_err("beyond the render limits");
    assert!(
        matches!(
            error,
            Error::BudgetExceeded {
                kind: BudgetKind::PageMemory,
                component: Some(_)
            }
        ),
        "{error:?}"
    );

    // None of that hurt the page.
    assert_eq!(
        decode(&capture(&page, Capture::Viewport).await).pixel(10, 10),
        RED
    );

    let pinned = rig
        .context
        .new_page(PageOptions::tier1())
        .await
        .expect("a Tier 1 page");
    let error = pinned
        .screenshot(ScreenshotOptions::default())
        .await
        .expect_err("a Tier 1 page may not render");
    assert!(
        matches!(
            error,
            Error::TierDenied {
                needed: Tier::Render,
                ceiling: Tier::Dom
            }
        ),
        "{error:?}"
    );
    page.close().await.expect("close");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_that_has_not_navigated_renders_its_blank_document() {
    let rig = Rig::new().await;
    let page = rig
        .context
        .new_page(PageOptions {
            javascript: false,
            viewport: viewport(120, 80, 1.0),
            ..PageOptions::default()
        })
        .await
        .expect("a page");

    let shot = capture(&page, Capture::Viewport).await;
    assert_eq!((shot.width, shot.height), (120, 80));
    assert_eq!(decode(&shot).pixel(60, 40), [255, 255, 255, 255]);

    page.close().await.expect("close");
    let error = page
        .screenshot(ScreenshotOptions::default())
        .await
        .expect_err("the page is closed");
    assert!(matches!(error, Error::Closed), "{error:?}");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_page_ends_a_screenshot_waiting_for_a_stylesheet() {
    let rig = Rig::new().await;
    let page = rig.tier1("/slow.html", viewport(200, 120, 1.0)).await;
    let before = rig.server.requests_served();

    let pending = tokio::spawn({
        let page = page.clone();
        async move { page.screenshot(ScreenshotOptions::default()).await }
    });
    until_served(&rig.server, before + 1).await;
    page.close().await.expect("close");

    let error = tokio::time::timeout(PATIENCE, pending)
        .await
        .expect("the screenshot ended")
        .expect("the task did not panic")
        .expect_err("the page closed under it");
    assert!(matches!(error, Error::Closed), "{error:?}");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_screenshot_waiting_for_a_stylesheet_leaves_the_page_usable() {
    let rig = Rig::new().await;
    let page = rig.tier1("/slow.html", viewport(200, 120, 1.0)).await;
    let before = rig.server.requests_served();

    {
        let waiting = page.screenshot(ScreenshotOptions::default());
        tokio::pin!(waiting);
        tokio::select! {
            biased;
            () = until_served(&rig.server, before + 1) => {}
            _ = &mut waiting => panic!("the sheet is 10 minutes away"),
        }
    }
    // The document is still the page's own.
    assert!(page.document().is_some());
    assert!(page.take_document().await.is_ok());

    page.close().await.expect("close");
    rig.finish();
}

#[cfg(feature = "v8")]
mod scripted {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_script_page_shows_what_its_scripts_built() {
        let rig = Rig::new().await;
        let page = rig
            .scripted("/mutating.html", viewport(200, 120, 1.0))
            .await;

        let image = decode(&capture(&page, Capture::Viewport).await);
        assert_eq!(image.pixel(30, 30), ORANGE);
        assert_eq!(image.pixel(150, 100), BLUE);

        // A change after the first render is picked up by the next.
        page.evaluate("document.body.style.cssText = 'background:#00ff00'")
            .await
            .expect("evaluate");
        let image = decode(&capture(&page, Capture::Viewport).await);
        assert_eq!(image.pixel(150, 100), GREEN);
        // The engine still owns the tree.
        assert!(page.document().is_none());

        page.close().await.expect("close");
        rig.finish();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn editing_a_style_element_restyles_once_and_only_then() {
        let rig = Rig::new().await;
        let page = rig
            .scripted("/editable.html", viewport(200, 120, 1.0))
            .await;

        assert_eq!(
            decode(&capture(&page, Capture::Viewport).await).pixel(10, 10),
            RED
        );
        let first = page
            .render_counters()
            .await
            .expect("counters")
            .expect("a session");
        capture(&page, Capture::Viewport).await;
        let unchanged = page
            .render_counters()
            .await
            .expect("counters")
            .expect("a session");
        assert_eq!(unchanged, first);

        page.evaluate("document.getElementById('s').textContent = 'html,body{margin:0}#b{width:50px;height:50px;background:#0000ff}'")
            .await
            .expect("evaluate");
        assert_eq!(
            decode(&capture(&page, Capture::Viewport).await).pixel(10, 10),
            BLUE
        );
        let edited = page
            .render_counters()
            .await
            .expect("counters")
            .expect("a session");
        assert_eq!(edited.source_sets, first.source_sets + 1);

        page.close().await.expect("close");
        rig.finish();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn screenshots_and_evaluations_of_one_page_are_serialised() {
        let rig = Rig::new().await;
        let page = rig.scripted("/linked.html", viewport(200, 120, 1.0)).await;

        let (first, second, evaluated) = tokio::join!(
            page.screenshot(ScreenshotOptions::default()),
            page.screenshot(ScreenshotOptions::default()),
            page.evaluate("document.title = 'busy'; 7"),
        );
        let (first, second) = (first.expect("first"), second.expect("second"));
        assert_eq!(decode(&first).rgba, decode(&second).rgba);
        assert_eq!(decode(&first).pixel(40, 40), MAGENTA);
        assert!(page.document().is_none(), "the engine kept the tree");
        assert_eq!(
            evaluated
                .expect("evaluate")
                .into_value::<f64>()
                .expect("a number"),
            7.0
        );
        assert!(page.crash_reason().is_none());

        page.close().await.expect("close");
        rig.finish();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closing_a_script_page_ends_a_screenshot_waiting_for_a_stylesheet() {
        let rig = Rig::new().await;
        let page = rig.scripted("/slow.html", viewport(200, 120, 1.0)).await;
        let before = rig.server.requests_served();

        let pending = tokio::spawn({
            let page = page.clone();
            async move { page.screenshot(ScreenshotOptions::default()).await }
        });
        until_served(&rig.server, before + 1).await;
        page.close().await.expect("close");

        let error = tokio::time::timeout(PATIENCE, pending)
            .await
            .expect("the screenshot ended")
            .expect("the task did not panic")
            .expect_err("the page closed under it");
        assert!(matches!(error, Error::Closed), "{error:?}");
        rig.finish();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stylesheet_added_while_another_is_being_fetched_is_waited_for() {
        let rig = Rig::new().await;
        let page = rig.scripted("/gated.html", viewport(200, 120, 1.0)).await;
        let before = rig.server.requests_served();

        let pending = tokio::spawn({
            let page = page.clone();
            async move { page.screenshot(ScreenshotOptions::default()).await }
        });
        // gate.css is held back by the server; the script adds a sheet meanwhile.
        until_served(&rig.server, before + 1).await;
        page.evaluate(
            "var l = document.createElement('link'); l.setAttribute('rel', 'stylesheet'); \
             l.setAttribute('href', 'late.css'); document.head.appendChild(l); 1",
        )
        .await
        .expect("evaluate");

        let shot = tokio::time::timeout(PATIENCE, pending)
            .await
            .expect("the screenshot ended")
            .expect("the task did not panic")
            .expect("screenshot");
        assert_eq!(
            decode(&shot).pixel(30, 30),
            RED,
            "late.css was fetched and applied"
        );

        page.close().await.expect("close");
        rig.finish();
    }
}
