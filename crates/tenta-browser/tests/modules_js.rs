//! Module scripts end to end: the parser finds a `<script type="module">`,
//! the page loads its graph over loopback, V8 links and runs it, and
//! `import()` goes round the same loop from script
//! (`docs/spec/05-javascript.md`; HTML "module scripts").
//!
//! `crates/tenta-js-v8/tests/dom_modules.rs` covers the engine half against a
//! test that plays the loader. Every navigation here waits for
//! `Settle::NetworkIdle`, for the reason `fetch_js.rs` gives.

#![cfg(feature = "v8")]

mod support;

use std::time::Duration;

use tenta_browser::{Document, Engine, Goto, JsEngine, PageOptions, Settle, SettleConfig};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

fn engine(js: JsEngine) -> Engine {
    Engine::builder()
        .js(js)
        .io_threads(1)
        .page_threads(1)
        .max_tier(Tier::Js)
        .settle(SettleConfig::library())
        .build()
        .expect("engine")
}

async fn serve(files: &[(&str, &str)]) -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("modules-js");
    for (relative, contents) in files {
        temp.write(relative, contents);
    }
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
        .await
        .expect("fixture server");
    (server, temp)
}

fn text_of(document: &Document, selector: &str) -> String {
    let node = document
        .query_selector(selector)
        .expect("selector parses")
        .unwrap_or_else(|| panic!("{selector} is in the document"));
    document.text_of(node).trim().to_owned()
}

/// Loads `/index.html` from `server` and returns the text of `#result`.
async fn result_of(js: JsEngine, server: &FixtureServer) -> String {
    let engine = engine(js);
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");
    tokio::time::timeout(
        Duration::from_secs(20),
        page.goto(
            &server.url("/index.html"),
            Goto::default().wait_until(Settle::NetworkIdle),
        ),
    )
    .await
    .expect("the navigation settled instead of hanging")
    .expect("navigation");
    let document = page.take_document().await.expect("document");
    let result = text_of(&document, "#result");
    engine.close();
    result
}

const ORDER_PAGE: &str = r#"<!doctype html><html><head>
<script>
  window.order = [];
  document.addEventListener('DOMContentLoaded', function () { order.push('dcl'); });
  window.addEventListener('load', function () {
    document.getElementById('result').textContent = order.join('|');
  });
</script>
<script type="module" src="/main.js"></script>
<script defer src="/defer.js"></script>
<script type="module">
  import { name } from './lib/a.js';
  order.push('inline-module ' + name + ' ' + (document.currentScript === null));
</script>
<script nomodule>order.push('nomodule');</script>
<script>order.push('classic');</script>
</head><body><div id="result">unset</div></body></html>"#;

const ORDER_FILES: [(&str, &str); 5] = [
    ("index.html", ORDER_PAGE),
    (
        "main.js",
        "import { name } from './lib/a.js';\nimport shared from '/shared.js';\n\
         order.push('main ' + name + ' ' + shared.runs);",
    ),
    (
        "lib/a.js",
        "import shared from '../shared.js';\nexport const name = 'alpha';",
    ),
    (
        "shared.js",
        "window.sharedRuns = (window.sharedRuns || 0) + 1;\n\
         export default { runs: window.sharedRuns };",
    ),
    ("defer.js", "order.push('defer');"),
];

/// A module script never blocks the parse: it runs after the document is
/// parsed, in document order with the `defer` scripts and before
/// `DOMContentLoaded`. A module two graphs share is fetched and run once, and
/// a `nomodule` script is the fallback this engine does not need.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn module_scripts_run_deferred_in_document_order() {
    let (server, _temp) = serve(&ORDER_FILES).await;
    assert_eq!(
        result_of(JsEngine::V8, &server).await,
        "classic|main alpha 1|defer|inline-module alpha true|dcl"
    );
    assert_eq!(
        server.requests_served(),
        5,
        "the document, the two scripts it names and the two modules they import, each once"
    );
}

/// An engine without module scripts leaves them unrun and runs the `nomodule`
/// fallback, which is what a page ships for it.
#[cfg(feature = "quickjs")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_engine_without_modules_runs_the_nomodule_fallback() {
    let (server, _temp) = serve(&ORDER_FILES).await;
    assert_eq!(
        result_of(JsEngine::QuickJs, &server).await,
        "nomodule|classic|defer|dcl"
    );
}

/// `import()` loads a graph while the page runs, through the import map, and
/// a module with top-level `await` is waited for. The navigation does not
/// settle with any of it still in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_imports_and_the_import_map() {
    let (server, _temp) = serve(&[
        (
            "index.html",
            r#"<!doctype html><html><head>
<script type="importmap">{"imports": {"app/": "/pkg/", "greeter": "/pkg/greet.js"}}</script>
<script type="module">
  import { greet as direct } from 'greeter';
  const { greet } = await import('app/greet.js');
  const lazy = await import('./lazy.js');
  document.getElementById('result').textContent = [
    greet('x'), direct === greet, lazy.default, lazy.where.endsWith('/lazy.js'),
    import.meta.url.endsWith('/index.html')].join('|');
</script>
</head><body><div id="result">unset</div></body></html>"#,
        ),
        (
            "pkg/greet.js",
            "import { punctuation } from './mark.js';\n\
             export function greet(name) { return 'hello ' + name + punctuation; }",
        ),
        ("pkg/mark.js", "export const punctuation = '!';"),
        (
            "lazy.js",
            "await new Promise(function (resolve) { setTimeout(resolve, 5); });\n\
             export default 'lazy'; export const where = import.meta.url;",
        ),
    ])
    .await;
    assert_eq!(
        result_of(JsEngine::V8, &server).await,
        "hello x!|true|lazy|true|true"
    );
}

/// A module that cannot be loaded rejects its `import()` with a `TypeError`
/// and takes nothing else with it: a missing file, a response that is not
/// JavaScript, another origin that does not share its script.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_module_that_cannot_be_loaded_rejects_its_import() {
    let (other, _other_temp) = serve(&[("mod.js", "export const v = 'leaked';")]).await;
    let foreign = other.url("/mod.js");
    let html = format!(
        r#"<!doctype html><html><head>
<script>
  Promise.allSettled([
    import('/missing.js'), import('/data.json'), import('{foreign}'), import('/bad-import.js'),
    import('/ok.js')
  ]).then(function (results) {{
    document.getElementById('result').textContent = results.map(function (r) {{
      return r.status === 'fulfilled' ? 'ok ' + r.value.v : r.reason.name;
    }}).join('|');
  }});
</script>
<script type="module" src="/missing.js"></script>
<script type="module">document.title = 'later modules still run';</script>
</head><body><div id="result">unset</div></body></html>"#
    );
    let (server, _temp) = serve(&[
        ("index.html", &html),
        ("data.json", "{\"v\": 1}"),
        (
            "bad-import.js",
            "import { nothing } from './ok.js'; export const v = 0;",
        ),
        ("ok.js", "export const v = 1;"),
    ])
    .await;
    assert_eq!(
        result_of(JsEngine::V8, &server).await,
        "TypeError|TypeError|TypeError|SyntaxError|ok 1"
    );
    assert_eq!(other.requests_served(), 1, "the other origin was asked");
}

/// A module on another origin is the page's to import when that origin shares
/// it. The request is a CORS request: it says where it came from and carries
/// no cookie.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_module_another_origin_shares_can_be_imported() {
    let (other, _other_temp) = serve(&[]).await;
    let endpoint = other.url("/api/cors");
    let html = format!(
        r#"<!doctype html><html><head>
<script>
  document.cookie = 'sid=1; Path=/';
  var allow = encodeURIComponent(location.origin);
  Promise.allSettled([
    import('{endpoint}?module=1&origin=' + allow),
    import('{endpoint}?module=1&origin=*&v=2'),
    import('{endpoint}?module=1&v=3')
  ]).then(function (results) {{
    document.getElementById('result').textContent = results.map(function (r) {{
      if (r.status !== 'fulfilled') return r.reason.name;
      var seen = r.value.default;
      return (seen.origin === location.origin) + ' ' + seen.cookie + ' ' + seen.method;
    }}).join('|');
  }});
</script>
</head><body><div id="result">unset</div></body></html>"#
    );
    let (server, _temp) = serve(&[("index.html", &html)]).await;
    assert_eq!(
        result_of(JsEngine::V8, &server).await,
        "true null GET|true null GET|TypeError"
    );
}
