//! Two live JavaScript pages on one page thread, closed in both orders
//! (`docs/spec/02-architecture.md` §3, §8).
//!
//! A page thread hosts one `Engine` per live page, and a page thread is handed
//! out round-robin, so two pages of the same context land on the same thread as
//! soon as the pool has fewer threads than the context has pages
//! (`crates/tenta-browser/src/threads.rs`). That is the shape this file pins:
//! **two V8 isolates live at once on one thread**, each with a document of its
//! own, and disposal happens while the other isolate is still live.
//!
//! `crates/tenta-js-v8/tests/isolate_entry.rs` is the differential test for the
//! entry guard itself — two engines, one thread, no page pool between them.
//! What only a real navigation can show is the lifecycle the page layer adds:
//! engines built during a parse, reclaimed by `Page::close`'s task, with the
//! other page's engine live across both.
//!
//! **The assertion that is not vacuous by accident.** A panic inside a page
//! thread's close task is caught by the pool's `catch_unwind_task` and only
//! logged (`tenta-page::task`), so `Page::close` returns `Ok` whether the
//! engine was reclaimed or exploded. `Page::close` therefore records a panic in
//! its own teardown on the page, and these tests read `Page::crash_reason()` —
//! the one observable that distinguishes the two.

#![cfg(feature = "v8")]

mod support;

use tenta_browser::{Engine, Goto, PageOptions, SettleConfig};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

/// An engine whose ceiling reaches Tier 2, so a page may ask for JavaScript.
///
/// One page thread, so the two pages of these tests provably share it.
fn js_engine(settle: SettleConfig) -> Engine {
    Engine::builder()
        .io_threads(1)
        .page_threads(1)
        .max_tier(Tier::Js)
        .settle(settle)
        .build()
        .expect("engine")
}

/// Serves `/a.html` and `/b.html`, each running an inline script that writes a
/// marker of its own into the document.
///
/// The marker is what makes "two isolates" observable rather than assumed: a
/// backend that quietly served both pages from one engine's context would
/// answer both readings with whichever value was written last.
///
/// The script comes after the element it writes, which is when the parse is
/// paused on it and `getElementById` can see it.
const PAGE_A: &str = "<!doctype html><html><head><title>a</title></head><body>\
     <div id=\"marker\">before</div>\
     <script>document.getElementById('marker').textContent = 'a';</script>\
     </body></html>";

const PAGE_B: &str = "<!doctype html><html><head><title>b</title></head><body>\
     <div id=\"marker\">before</div>\
     <script>document.getElementById('marker').textContent = 'b';</script>\
     </body></html>";

async fn serve_both() -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("isolate-lifecycle");
    temp.write("a.html", PAGE_A);
    temp.write("b.html", PAGE_B);
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
        .await
        .expect("fixture server");
    (server, temp)
}

/// Reads back the marker a page's own script wrote.
///
/// Through the engine the page keeps for its document: the tree is the engine's
/// while the page lives, and reading it on the page thread is what proves each
/// page still answers from the isolate that ran its script.
async fn marker_of(page: &tenta_browser::Page, what: &str) -> String {
    let reading = page
        .with_engine(|engine| {
            let shared = engine.document();
            let document = shared.borrow();
            let marker = document.get_element_by_id("marker")?;
            let text = document.first_child(marker);
            document.text_data(text).map(str::to_owned)
        })
        .await
        .unwrap_or_else(|error| panic!("{what} still has its engine: {error:?}"));
    reading.unwrap_or_else(|| panic!("{what}'s document has a #marker with a text node"))
}

/// Both pages are on the one page thread, or the test is not testing anything.
fn assert_one_page_thread<'a>(pages: impl IntoIterator<Item = &'a tenta_browser::Page>) {
    let threads: Vec<usize> = pages.into_iter().map(|page| page.thread_index()).collect();
    assert!(
        threads.windows(2).all(|pair| pair[0] == pair[1]),
        "the pages are on threads {threads:?}; one page thread is the point of this test"
    );
}

/// Asserts a closed page neither crashed nor was left marked as crashed.
fn assert_closed_cleanly(page: &tenta_browser::Page, what: &str) {
    assert!(page.is_closed(), "{what} is closed");
    assert_eq!(
        page.crash_reason(),
        None,
        "{what}'s page thread panicked while reclaiming its engine"
    );
}

/// Two pages, two engines, one page thread; the older page is closed first.
///
/// Closing the *older* page first is the order the page pool produces and the
/// order that used to tear the process down: the surviving newer engine's
/// isolate is the one the thread last entered, and disposing the older engine
/// from there is what V8 refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_live_pages_on_one_page_thread_close_oldest_first() {
    run_two_pages(true).await;
}

/// The mirror: the newer page is closed first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_live_pages_on_one_page_thread_close_newest_first() {
    run_two_pages(false).await;
}

async fn run_two_pages(close_oldest_first: bool) {
    let (server, _temp) = serve_both().await;
    // The library settle: nothing here waits for the network beyond the
    // document itself, and no page schedules work after its load.
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);

    let older = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");
    let older_navigation = older
        .goto(&server.url("/a.html"), Goto::default())
        .await
        .expect("navigation");
    assert_eq!(older_navigation.status, 200);
    assert_eq!(
        marker_of(&older, "the older page").await,
        "a",
        "the older page's own script ran"
    );

    // The second page's engine is built while the first one's is still live.
    // This is the state the page thread could not host.
    let newer = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");
    let newer_navigation = newer
        .goto(&server.url("/b.html"), Goto::default())
        .await
        .expect("navigation");
    assert_eq!(newer_navigation.status, 200);

    assert_one_page_thread([&older, &newer]);
    assert_eq!(
        marker_of(&newer, "the newer page").await,
        "b",
        "the newer page's own script ran"
    );
    assert_eq!(
        marker_of(&older, "the older page").await,
        "a",
        "the older page kept its own global through the second engine's build"
    );

    let (first, first_name, second, second_name) = if close_oldest_first {
        (&older, "older", &newer, "newer")
    } else {
        (&newer, "newer", &older, "older")
    };

    // Disposal of one engine happens with the other page's engine live.
    first.close().await.expect("close");
    assert_closed_cleanly(first, first_name);
    assert!(
        !second.is_closed(),
        "{second_name} was not closed by closing {first_name}"
    );
    assert_eq!(
        second.crash_reason(),
        None,
        "closing {first_name} took {second_name}'s engine down with it"
    );
    let surviving = marker_of(second, &format!("the {second_name} page")).await;
    assert!(
        surviving == if close_oldest_first { "b" } else { "a" },
        "the surviving page's script wrote {surviving:?}, so it was read from the \
         wrong isolate"
    );

    second.close().await.expect("close");
    assert_closed_cleanly(second, second_name);
    assert_eq!(
        context.page_groups(),
        0,
        "both pages closed and their groups were forgotten"
    );
    engine.close();
}
