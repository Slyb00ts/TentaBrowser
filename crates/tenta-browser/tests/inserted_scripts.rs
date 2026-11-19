//! Scripts that script inserts, end to end: an element made with
//! `createElement('script')` and put into the document is fetched, runs and is
//! told `load`, or `error` when there is nothing to run. Pages are served by
//! the in-process fixture server and run the V8 engine.
//!
//! What runs inside the DOM call (an inline script) is checked at the engine
//! (`crates/tenta-js-v8/tests/dom_inserted_scripts.rs`); these tests are about
//! what the page has to load.

#![cfg(feature = "v8")]

use std::time::Duration;

use tenta_browser::{
    BrowserContext, ContextOptions, Engine, Goto, JsEngine, Page, PageOptions, Settle,
};
use tenta_core::{FsPolicy, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig, PathDelay, TempDir};

const PATIENCE: Duration = Duration::from_secs(60);

struct Rig {
    server: FixtureServer,
    context: BrowserContext,
    engine: Engine,
    _files: TempDir,
}

impl Rig {
    async fn new(files: Vec<(&str, &str)>) -> Rig {
        Rig::with_delays(files, &[]).await
    }

    /// `delays` are the server's `GLOB=MS` rules: how long it holds a
    /// response back, which is how a test makes one script the slow one.
    async fn with_delays(files: Vec<(&str, &str)>, delays: &[&str]) -> Rig {
        let temp = tenta_testutil::temp_dir("inserted-scripts");
        for (name, contents) in files {
            temp.write(name, contents.as_bytes());
        }
        let config = FixtureServerConfig {
            path_delays: delays
                .iter()
                .map(|rule| PathDelay::parse(rule).expect("a delay rule"))
                .collect(),
            ..FixtureServerConfig::new(temp.path())
        };
        let server = FixtureServer::start(config).await.expect("fixture server");
        let engine = Engine::builder()
            .max_tier(Tier::Js)
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

    /// Opens `path` and waits for the page to settle: what a script the load
    /// inserted has asked for is part of that.
    async fn open(&self, path: &str) -> Page {
        let page = self
            .context
            .new_page(PageOptions {
                javascript: true,
                ..PageOptions::default()
            })
            .await
            .expect("page");
        page.goto(
            &self.server.url(path),
            Goto::default()
                .wait_until(Settle::NetworkIdle)
                .timeout(PATIENCE),
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

async fn text(page: &Page, expression: &str) -> String {
    page.evaluate(expression)
        .await
        .expect("ran")
        .into_value::<String>()
        .expect("a string")
}

fn page_with(script: &str) -> String {
    format!(
        "<!doctype html><html><head></head><body><script>window.log = [];\n{script}\n\
         </script></body></html>"
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inserted_external_script_is_fetched_runs_and_is_told_load() {
    let html = page_with(
        "var s = document.createElement('script'); s.src = 'lib.js'; \
         s.onload = function (event) { log.push('load:' + (event.target === s) + ':' + \
           event.bubbles + ':' + String(document.currentScript)); }; \
         s.onerror = function () { log.push('error'); }; \
         document.head.appendChild(s); log.push('inserted:' + typeof lib);",
    );
    let rig = Rig::new(vec![
        ("index.html", &html),
        (
            "lib.js",
            "window.lib = 1; log.push('lib:' + (document.currentScript && \
             document.currentScript.getAttribute('src')));",
        ),
    ])
    .await;
    let page = rig.open("/index.html").await;
    assert_eq!(
        text(&page, "log.join(' ')").await,
        "inserted:undefined lib:lib.js load:true:false:null"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_that_cannot_be_fetched_is_told_error() {
    let html = page_with(
        "function add(src) { var s = document.createElement('script'); \
           if (src !== null) s.setAttribute('src', src); \
           s.onload = function () { log.push('load:' + src); }; \
           s.onerror = function (event) { log.push('error:' + src + ':' + (event.target === s)); }; \
           document.head.appendChild(s); return s; } \
         add('missing.js'); add(''); add('http://[bad'); \
         var late = add(null); late.src = 'present.js';",
    );
    let rig = Rig::new(vec![
        ("index.html", &html),
        ("present.js", "log.push('present ran');"),
    ])
    .await;
    let page = rig.open("/index.html").await;
    let mut log: Vec<String> = text(&page, "log.join('|')")
        .await
        .split('|')
        .map(str::to_owned)
        .collect();
    // The four elements load independently, so only the set is fixed, and
    // that the one script that exists ran before its `load`.
    let ran = log.iter().position(|entry| entry == "present ran");
    let loaded = log.iter().position(|entry| entry == "load:null");
    assert!(ran < loaded, "{log:?}");
    log.sort();
    assert_eq!(
        log,
        [
            "error::true",
            "error:http://[bad:true",
            "error:missing.js:true",
            "load:null",
            "present ran",
        ]
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scripts_with_async_false_run_in_the_order_they_were_inserted() {
    // The first script is the slow one: the server holds it back, and the two
    // behind it must wait for it although their text arrives first.
    let html = page_with(
        "['slow.js', 'second.js', 'third.js'].forEach(function (src) { \
           var s = document.createElement('script'); s.src = src; s.async = false; \
           s.onload = function () { log.push('load:' + src); }; \
           document.head.appendChild(s); }); \
         var free = document.createElement('script'); free.src = 'free.js'; \
         document.head.appendChild(free);",
    );
    let rig = Rig::with_delays(
        vec![
            ("index.html", &html),
            ("slow.js", "log.push('slow');"),
            ("second.js", "log.push('second');"),
            ("third.js", "log.push('third');"),
            ("free.js", "log.push('free');"),
        ],
        &["/slow.js=300"],
    )
    .await;
    let page = rig.open("/index.html").await;
    let log = text(&page, "log.join(' ')").await;
    let ordered: Vec<&str> = log.split(' ').filter(|entry| *entry != "free").collect();
    assert_eq!(
        ordered,
        [
            "slow",
            "load:slow.js",
            "second",
            "load:second.js",
            "third",
            "load:third.js"
        ],
        "{log}"
    );
    // The script that did not ask for order did not wait for the slow one.
    assert!(log.starts_with("free "), "{log}");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inserted_script_can_insert_the_next() {
    let html = page_with(
        "window.insert = function (src) { var s = document.createElement('script'); \
           s.src = src; document.body.appendChild(s); }; insert('one.js');",
    );
    let rig = Rig::new(vec![
        ("index.html", &html),
        ("one.js", "log.push('one'); insert('two.js');"),
        (
            "two.js",
            "log.push('two'); var inline = document.createElement('script'); \
             inline.text = \"log.push('inline')\"; document.body.appendChild(inline); \
             insert('three.js');",
        ),
        ("three.js", "log.push('three');"),
    ])
    .await;
    let page = rig.open("/index.html").await;
    assert_eq!(text(&page, "log.join(' ')").await, "one two inline three");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inserted_module_script_loads_its_graph_and_runs() {
    let html = page_with(
        "var external = document.createElement('script'); external.type = 'module'; \
         external.src = 'main.mjs'; \
         external.onload = function () { log.push('load:external'); }; \
         document.head.appendChild(external); \
         var inline = document.createElement('script'); inline.type = 'module'; \
         inline.textContent = \"import { name } from './dep.mjs'; \
           log.push('inline:' + name + ':' + String(document.currentScript));\"; \
         document.head.appendChild(inline); log.push('inserted'); \
         var broken = document.createElement('script'); broken.type = 'module'; \
         broken.src = 'nowhere.mjs'; \
         broken.onerror = function () { log.push('error:broken'); }; \
         document.head.appendChild(broken);",
    );
    let rig = Rig::new(vec![
        ("index.html", &html),
        (
            "main.mjs",
            "import { name } from './dep.mjs'; log.push('main:' + name);",
        ),
        ("dep.mjs", "export const name = 'dep';"),
    ])
    .await;
    let page = rig.open("/index.html").await;
    let mut log: Vec<String> = text(&page, "log.join('|')")
        .await
        .split('|')
        .map(str::to_owned)
        .collect();
    assert_eq!(log[0], "inserted", "a module script never runs in the call");
    let ran = log.iter().position(|entry| entry == "main:dep");
    let loaded = log.iter().position(|entry| entry == "load:external");
    assert!(ran < loaded, "{log:?}");
    log.sort();
    assert_eq!(
        log,
        [
            "error:broken",
            "inline:dep:null",
            "inserted",
            "load:external",
            "main:dep",
        ]
    );
    rig.finish();
}

/// The parser's own external scripts are told how their load ended as well:
/// a blocking one, a deferred one and an async one, present and missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_parser_s_external_scripts_are_told_load_and_error() {
    let html = "<!doctype html><html><head><script>window.log = []; \
                window.note = function (what) { log.push(what); };</script>\
                <script src=\"blocking.js\" onload=\"note('load:blocking')\" \
                  onerror=\"note('error:blocking')\"></script>\
                <script src=\"gone.js\" onload=\"note('load:gone')\" \
                  onerror=\"note('error:gone')\"></script>\
                <script>note('inline after')</script>\
                <script defer src=\"deferred.js\" onload=\"note('load:deferred')\"></script>\
                <script defer src=\"gone-deferred.js\" onerror=\"note('error:gone-deferred')\"></script>\
                <script async src=\"async.js\" onload=\"note('load:async')\"></script>\
                <script async src=\"gone-async.js\" onerror=\"note('error:gone-async')\"></script>\
                </head><body><p>text</p></body></html>";
    let rig = Rig::new(vec![
        ("index.html", html),
        ("blocking.js", "note('blocking');"),
        ("deferred.js", "note('deferred');"),
        ("async.js", "note('async');"),
    ])
    .await;
    let page = rig.open("/index.html").await;
    let log: Vec<String> = text(&page, "log.join('|')")
        .await
        .split('|')
        .map(str::to_owned)
        .collect();
    let at = |entry: &str| {
        log.iter()
            .position(|known| known == entry)
            .unwrap_or_else(|| panic!("{entry} is missing from {log:?}"))
    };
    // The blocking scripts are in document order with what follows them.
    assert_eq!(
        log[..4],
        ["blocking", "load:blocking", "error:gone", "inline after"]
    );
    // A script's `load` follows its run; the deferred ones keep their order.
    assert!(at("deferred") < at("load:deferred"));
    assert!(at("load:deferred") < at("error:gone-deferred"));
    assert!(at("async") < at("load:async"));
    at("error:gone-async");
    assert_eq!(log.len(), 10, "{log:?}");
    rig.finish();
}
