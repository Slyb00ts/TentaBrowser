//! Pages of two contexts on one page thread keep separate engines.
//!
//! A page thread keeps its engines in a thread-local table keyed by page id,
//! and the pool hands one thread to pages of every context. If each context
//! counted its own ids, the first page of context B would share a key with the
//! first page of context A and its first script would find A's engine bound to
//! A's tree.

#![cfg(feature = "v8")]

mod support;

use tenta_browser::{ContextOptions, Engine, Goto, PageOptions, SettleConfig};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

const PAGE: &str = "<!doctype html><html><body><div id=\"marker\">before</div>\
     <script>document.getElementById('marker').textContent = 'ran';</script>\
     </body></html>";

async fn marker_of(page: &tenta_browser::Page) -> String {
    page.with_engine(|engine| {
        let shared = engine.document();
        let document = shared.borrow();
        let marker = document.get_element_by_id("marker")?;
        let text = document.first_child(marker);
        document.text_data(text).map(str::to_owned)
    })
    .await
    .expect("the page still has its engine")
    .expect("the document has a #marker with a text node")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_pages_of_two_contexts_do_not_share_an_engine() {
    let temp = tenta_testutil::temp_dir("page-ids");
    temp.write("page.html", PAGE);
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
        .await
        .expect("fixture server");
    let engine = Engine::builder()
        .io_threads(1)
        .page_threads(1)
        .max_tier(Tier::Js)
        .settle(SettleConfig::library())
        .build()
        .expect("engine");
    let options = || ContextOptions {
        allow_private_network: true,
        ..ContextOptions::default()
    };
    let first_context = engine.new_context(options()).expect("context");
    let second_context = engine.new_context(options()).expect("context");

    let first = first_context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");
    first
        .goto(&server.url("/page.html"), Goto::default())
        .await
        .expect("navigation");
    let second = second_context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");
    second
        .goto(&server.url("/page.html"), Goto::default())
        .await
        .expect("navigation");

    assert_eq!(first.thread_index(), second.thread_index());
    assert_eq!(marker_of(&first).await, "ran");
    assert_eq!(marker_of(&second).await, "ran");
    assert_eq!(first.crash_reason(), None);
    assert_eq!(second.crash_reason(), None);
}
