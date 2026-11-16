//! Custom elements end to end: the parser builds the elements, a script
//! defines them, and the document the page hands back has what their
//! reactions rendered (HTML §4.13).
//!
//! `crates/tenta-js-v8/tests/dom_custom_elements.rs` covers the registry and
//! each reaction against the engine alone.

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
    let temp = tenta_testutil::temp_dir("custom-elements-js");
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

async fn load(js: JsEngine, server: &FixtureServer) -> Document {
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
    engine.close();
    document
}

/// One element stands before the script that defines it and is upgraded by
/// `define()`; the other is parsed after it and is upgraded when script is
/// next entered. Both render from their attributes, and the order of the
/// reactions is the one HTML gives for an element the page already has.
const PAGE: &str = r#"<!doctype html><html><head>
<script>window.order = [];</script>
</head><body>
<x-greeting id="early" name="Ada"></x-greeting>
<script>
  class Greeting extends HTMLElement {
    static get observedAttributes() { return ['name']; }
    constructor() { super(); order.push(this.id + ':new'); }
    attributeChangedCallback(name, before, now) {
      order.push(this.id + ':' + name + '=' + now);
      this.render();
    }
    connectedCallback() { order.push(this.id + ':connected'); this.render(); }
    render() { this.textContent = 'Hello, ' + this.getAttribute('name') + '!'; }
  }
  customElements.define('x-greeting', Greeting);
  customElements.whenDefined('x-greeting').then(function (constructor) {
    order.push('defined:' + (constructor === Greeting));
  });
</script>
<x-greeting id="late" name="Grace"></x-greeting>
<div id="result">unset</div>
<script type="module">
  const late = document.getElementById('late');
  order.push('module:' + (late.constructor.name) + ':' + late.textContent);
  late.setAttribute('name', 'Hopper');
  document.getElementById('result').textContent = order.join('|');
</script>
</body></html>"#;

#[tokio::test]
async fn a_page_s_custom_elements_render_into_the_document() {
    let (server, _files) = serve(&[("index.html", PAGE)]).await;
    let document = load(JsEngine::V8, &server).await;
    assert_eq!(text_of(&document, "#early"), "Hello, Ada!");
    assert_eq!(text_of(&document, "#late"), "Hello, Hopper!");
    assert_eq!(
        text_of(&document, "#result"),
        "early:new|early:name=Ada|early:connected|defined:true|\
         late:new|late:name=Grace|late:connected|\
         module:Greeting:Hello, Grace!|late:name=Hopper"
    );
}

/// A page that guards on `customElements` keeps working on QuickJS, which has
/// no registry: the elements stay as the markup left them.
const GUARDED: &str = r#"<!doctype html><html><body>
<x-note id="note">fallback</x-note>
<div id="result">unset</div>
<script>
  if (window.customElements) {
    customElements.define('x-note', class extends HTMLElement {
      connectedCallback() { this.textContent = 'upgraded'; }
    });
  }
  document.getElementById('result').textContent =
    typeof window.customElements + ':' + document.getElementById('note').textContent;
</script>
</body></html>"#;

#[tokio::test]
async fn a_guarded_page_degrades_without_a_registry() {
    let (server, _files) = serve(&[("index.html", GUARDED)]).await;
    let document = load(JsEngine::V8, &server).await;
    assert_eq!(text_of(&document, "#result"), "object:upgraded");
    #[cfg(feature = "quickjs")]
    {
        let document = load(JsEngine::QuickJs, &server).await;
        assert_eq!(text_of(&document, "#result"), "undefined:fallback");
    }
}

/// A component that renders into a shadow tree: the text the page hands back
/// is the rendered one, with the shadow tree's own text and the slotted
/// children in the places their slots give them.
const SHADOW: &str = r#"<!doctype html><html><body>
<main id="main">
<user-card><span slot="name">Ada Lovelace</span><em>ignored without a slot</em></user-card>
</main>
<div id="result">unset</div>
<script>
  customElements.define('user-card', class extends HTMLElement {
    constructor() {
      super();
      this.attachShadow({mode: 'open'}).innerHTML =
        '<h2>Name: <slot name="name">nobody</slot></h2><p>Role: <slot name="role">guest</slot></p>';
    }
    connectedCallback() {
      this.shadowRoot.querySelector('p').addEventListener('click', event => {
        document.getElementById('result').textContent =
          event.target.localName + '/' + event.composedPath().length;
      });
      document.addEventListener('click', event => {
        document.getElementById('result').textContent += '|' + event.target.localName;
      });
      this.shadowRoot.querySelector('p').click();
    }
  });
</script>
</body></html>"#;

#[tokio::test]
async fn a_shadow_tree_is_part_of_the_text_the_page_returns() {
    let (server, _files) = serve(&[("index.html", SHADOW)]).await;
    let document = load(JsEngine::V8, &server).await;
    let main = text_of(&document, "#main");
    let lines: Vec<&str> = main.lines().map(str::trim).collect();
    assert_eq!(lines, ["Name: Ada Lovelace", "Role: guest"]);
    // Inside the tree the paragraph is the target; the document sees the host.
    // The path is p, the shadow root, the host, main, body, html, the
    // document and the window.
    assert_eq!(text_of(&document, "#result"), "p/8|user-card");
}
