//! The page's configured viewport reaches script as the window metrics, on
//! both backends, through the real page loop.
#![cfg(any(feature = "v8", feature = "quickjs"))]

use std::time::Duration;

use tenta_browser::{ContextOptions, Engine, Goto, JsEngine, PageOptions, Settle, Viewport};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

async fn window_metrics_follow_the_page_viewport(backend: JsEngine) {
    let fixtures = tenta_testutil::temp_dir("window-metrics");
    fixtures.write("index.html", "<!doctype html><body>metrics</body>");
    let server = FixtureServer::start(FixtureServerConfig::new(fixtures.path()))
        .await
        .expect("fixture server");
    let engine = Engine::builder()
        .js(backend)
        .max_tier(Tier::Js)
        .io_threads(1)
        .page_threads(1)
        .ephemeral(true)
        .build()
        .expect("browser");
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context");
    let page = context
        .new_page(PageOptions {
            viewport: Viewport {
                width: 640,
                height: 480,
                device_scale_factor: 1.5,
                mobile: false,
            },
            ..PageOptions::default()
        })
        .await
        .expect("page");
    let (_, metrics) = page
        .goto_with_eval(
            &server.url("/index.html"),
            Goto::default()
                .wait_until(Settle::Load)
                .timeout(Duration::from_secs(10)),
            "[innerWidth, innerHeight, devicePixelRatio].join(',')",
        )
        .await
        .expect("navigation and evaluation");
    assert_eq!(metrics, "640,480,1.5");
    page.close().await.expect("close page");
    context.close();
    engine.close();
}

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v8_window_metrics_follow_the_page_viewport() {
    window_metrics_follow_the_page_viewport(JsEngine::V8).await;
}

#[cfg(feature = "quickjs")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quickjs_window_metrics_follow_the_page_viewport() {
    window_metrics_follow_the_page_viewport(JsEngine::QuickJs).await;
}
