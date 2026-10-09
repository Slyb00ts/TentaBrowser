//! Engine-selection behavior at the browser boundary.

use tenta_browser::{ContextOptions, Engine, JsEngine, PageOptions};
use tenta_core::Error;
#[cfg(feature = "v8")]
use tenta_core::Tier;

fn engine_with_javascript_disabled() -> Engine {
    let builder = Engine::builder()
        .io_threads(1)
        .page_threads(1)
        .js(JsEngine::None);

    // The V8 build must retain a Tier 2 ceiling to prove that the explicit
    // selector, rather than the absence of a compiled backend or a Tier 1
    // ceiling, refuses the JavaScript page.
    #[cfg(feature = "v8")]
    let builder = builder.max_tier(Tier::Js);

    builder.build().expect("engine")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicitly_disabled_javascript_refuses_js_pages_but_allows_tier_one() {
    let engine = engine_with_javascript_disabled();
    let context = engine
        .new_context(ContextOptions::default())
        .expect("context");

    let error = context
        .new_page(PageOptions::default())
        .await
        .expect_err("the explicit engine selector disables JavaScript pages");
    match error {
        Error::Unsupported { reason } => assert!(
            reason.contains("disabled by EngineOptions.js"),
            "the error should name the explicit selector: {reason}"
        ),
        other => panic!("expected JavaScript-disabled Unsupported, got {other:?}"),
    }
    assert_eq!(
        context.page_groups(),
        0,
        "a refused page must not leave a group behind"
    );

    let page = context
        .new_page(PageOptions::tier1())
        .await
        .expect("Tier 1 remains usable with JavaScript disabled");
    assert_eq!(context.group_threads().len(), 1);
    assert!(
        !context.group_threads()[0].2,
        "Tier 1 page has no JavaScript engine"
    );
    page.close().await.expect("close Tier 1 page");
    engine.close();
}
