//! Many pages at once, and what happens when one of them dies
//! (`docs/spec/02-architecture.md` §3, §8; `docs/spec/14-memory.md` §1).

mod support;

use std::time::Duration;

use tenta_browser::{Goto, PageEvent, PageOptions};
use tenta_core::Error;
use tenta_net::policy::error_names::ErrorName;
use tokio::task::JoinSet;

/// `S-concurrency-t1`: 100 Tier 1 pages of the static article at once on a
/// four-thread pool. The RSS delta is reported, not gated
/// (`docs/spec/14-memory.md` §1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hundred_concurrent_fetches_complete_on_a_four_thread_pool() {
    let server = support::server().await;
    let engine = support::engine(4);
    let context = support::context(&engine);
    let url = server.url("/static-article/index.html");

    let before = support::rss_bytes();
    let started = std::time::Instant::now();
    let mut pages = JoinSet::new();
    for _ in 0..100 {
        let context = context.clone();
        let url = url.clone();
        pages.spawn(async move { context.fetch(&url).await });
    }

    // The documents are kept alive until the measurement, which is what
    // "100 pages open simultaneously" means.
    let mut documents = Vec::with_capacity(100);
    while let Some(joined) = pages.join_next().await {
        documents.push(joined.expect("no page task panicked").expect("document"));
    }
    let elapsed = started.elapsed();
    let after = support::rss_bytes();

    assert_eq!(documents.len(), 100);
    let first = documents[0].text().len();
    assert!(first > 500_000, "the article extracts to {first} bytes");
    assert!(
        documents
            .iter()
            .all(|document| document.status() == 200 && document.text().len() == first),
        "every page parsed the same document"
    );
    assert!(
        server.requests_served() >= 1,
        "identical in-flight GETs may share one response (03 §3)"
    );
    assert_eq!(
        context.page_groups(),
        0,
        "every page closed and its group was forgotten"
    );

    let arena: usize = documents
        .iter()
        .map(|document| document.memory_report().total())
        .sum();
    println!(
        "S-concurrency-t1: 100 x static-article on 4 page threads in {:.1} ms, \
         DOM arenas {:.2} MB total ({:.2} MB per page)",
        elapsed.as_secs_f64() * 1000.0,
        arena as f64 / 1e6,
        arena as f64 / 1e6 / 100.0
    );
    support::report_rss("S-concurrency-t1 (100 live documents)", before, after);
    drop(documents);
    engine.close();
}

/// A panic inside a page's parse task crashes that page and nothing else
/// (`docs/spec/02-architecture.md` §8).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_page_is_contained_and_its_neighbours_keep_working() {
    let server = support::server().await;
    // One page thread, so the crashing page and the healthy one share it.
    let engine = support::engine(1);
    let context = support::context(&engine);

    let doomed = context
        .new_page(PageOptions {
            crash_in_parse_for_test: true,
            ..PageOptions::tier1()
        })
        .await
        .expect("page");
    let healthy = context.new_page(PageOptions::tier1()).await.expect("page");
    assert_eq!(
        doomed.thread_index(),
        healthy.thread_index(),
        "both pages are on the same page thread"
    );
    let mut events = doomed.events();

    let error = doomed
        .goto(&server.url("/tiny/index.html"), Goto::default())
        .await
        .expect_err("the parse task panics");
    match &error {
        Error::Crashed { reason } => assert!(reason.contains("test hook"), "reason was {reason}"),
        other => panic!("expected Crashed, got {other:?}"),
    }
    assert!(doomed.crash_reason().is_some());
    assert!(
        events
            .drain()
            .iter()
            .any(|event| matches!(event, PageEvent::Crashed { .. })),
        "the crash is reported as an event"
    );
    let again = doomed
        .goto(&server.url("/tiny/index.html"), Goto::default())
        .await
        .expect_err("a crashed page stays crashed");
    assert!(matches!(again, Error::Crashed { .. }), "{again:?}");

    let navigation = healthy
        .goto(&server.url("/tiny/index.html"), Goto::default())
        .await
        .expect("the other page on the same thread is unaffected");
    assert_eq!(navigation.status, 200);
    assert!(healthy.crash_reason().is_none());

    doomed.close().await.expect("close");
    healthy.close().await.expect("close");
    engine.close();
}

/// Pages are handed to threads round-robin and stay there
/// (`docs/spec/02-architecture.md` §3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pages_are_pinned_to_the_thread_they_were_given() {
    let engine = support::engine(3);
    let context = support::context(&engine);
    let mut indexes = Vec::new();
    let mut pages = Vec::new();
    for _ in 0..6 {
        let page = context.new_page(PageOptions::tier1()).await.expect("page");
        indexes.push(page.thread_index());
        pages.push(page);
    }
    assert_eq!(indexes.len(), 6);
    let mut sorted = indexes.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, vec![0, 1, 2], "every thread got pages");
    assert_eq!(
        context.page_groups(),
        6,
        "one group per Tier 1 page: {:?}",
        context.group_threads()
    );
    assert!(
        context
            .group_threads()
            .iter()
            .all(|(_, _, has_engine)| !has_engine),
        "Tier 1 groups have no engine"
    );

    for page in &pages {
        assert_eq!(page.thread_index(), indexes[page.id().raw() as usize - 1]);
        page.close().await.expect("close");
    }
    assert_eq!(context.page_groups(), 0);
    engine.close();
}

/// JavaScript needs a ceiling at or above Tier 2, and a page may not ask for a
/// tier above the engine's ceiling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_that_asks_for_javascript_is_refused() {
    let engine = support::engine(1);
    let context = support::context(&engine);

    // `support::engine` ceilings at Dom, so JavaScript has no tier to run in:
    // without the backend the whole feature is unavailable, and with it the
    // ceiling is what denies the page.
    let error = context
        .new_page(PageOptions::default())
        .await
        .expect_err("javascript needs max_tier >= js");
    #[cfg(not(feature = "v8"))]
    assert!(matches!(error, Error::Unsupported { .. }), "{error:?}");
    #[cfg(feature = "v8")]
    match error {
        Error::TierDenied { needed, ceiling } => {
            assert_eq!(needed, tenta_core::Tier::Js);
            assert_eq!(ceiling, tenta_core::Tier::Dom);
        }
        other => panic!("expected TierDenied, got {other:?}"),
    }

    let error = context
        .new_page(PageOptions {
            max_tier: Some(tenta_core::Tier::Render),
            ..PageOptions::tier1()
        })
        .await
        .expect_err("the engine ceiling is dom");
    match error {
        Error::TierDenied { needed, ceiling } => {
            assert_eq!(needed, tenta_core::Tier::Render);
            assert_eq!(ceiling, tenta_core::Tier::Dom);
        }
        other => panic!("expected TierDenied, got {other:?}"),
    }
    engine.close();
}

/// Dropping a `goto` future cancels the navigation and leaves the page as if
/// it never happened; the abandoned navigation's document is never installed
/// (`docs/spec/11-library-api.md` §1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_navigation_future_never_installs_the_document() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // Start navigating to a URL that never responds: poll the future once so
    // the navigation job is posted, then let the block drop it.
    let slow_url = server.url("/api/never");
    {
        let slow_goto = page.goto(&slow_url, Goto::default().timeout(Duration::from_secs(30)));
        tokio::pin!(slow_goto);
        let _ = tokio::time::timeout(Duration::from_micros(1), &mut slow_goto).await;
    }

    // The slow navigation is now provably in flight (dropped, so will be cancelled).
    // Issue a fast navigation to a different URL while the slow one is being cancelled.
    let fast_url = server.url("/tiny/index.html");
    let fast_navigation = page
        .goto(&fast_url, Goto::default())
        .await
        .expect("fast navigation completes");
    assert_eq!(fast_navigation.status, 200);
    assert_eq!(fast_navigation.final_url.as_str(), &fast_url);

    // The page's current document should be the fast one, not the slow one
    let doc = page.document().expect("document installed");
    let text = doc.text();
    assert!(
        text.len() < 1_000,
        "page contains tiny document, not stalled /api/never"
    );
    drop(doc);

    // Verify the installed document matches the fast navigation
    let installed_doc = page.take_document().expect("document");
    assert_eq!(
        installed_doc.navigation_id(),
        fast_navigation.navigation_id,
        "installed document's navigation_id should match the fast navigation"
    );

    page.close().await.expect("close");
    engine.close();
}

/// Waits up to `limit` for the fixture to report `expected` open connections.
async fn wait_for_open_connections(
    server: &tenta_testutil::FixtureServer,
    expected: u64,
    limit: Duration,
) -> u64 {
    let started = std::time::Instant::now();
    loop {
        let open = server.open_connections();
        if open == expected || started.elapsed() > limit {
            return open;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A navigation cancelled before its response head releases the connection
/// it was waiting on: the fixture sees the parked `/api/never` socket close
/// within a second, not at the navigation deadline (`docs/spec/11-library-api.md`
/// §1: "cancellation of a navigation aborts the request").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_pre_head_navigation_releases_its_connection() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // Cancelled by another goto.
    let slow_url = server.url("/api/never");
    let mut events = page.events();
    let page_for_slow = page.clone();
    let slow_task = tokio::spawn(async move {
        page_for_slow
            .goto(&slow_url, Goto::default().timeout(Duration::from_secs(30)))
            .await
    });
    support::wait_for_request(&mut events, "/api/never").await;
    assert_eq!(
        wait_for_open_connections(&server, 1, Duration::from_secs(2)).await,
        1,
        "the parked request holds one connection"
    );
    page.goto(&server.url("/tiny/index.html"), Goto::default())
        .await
        .expect("fast navigation");
    assert!(slow_task.await.expect("task").is_err());
    // The fast navigation's own keep-alive connection may stay open; the
    // parked one must be gone, so at most one remains.
    let open = wait_for_open_connections(&server, 1, Duration::from_secs(1)).await;
    assert!(
        open <= 1,
        "the cancelled request still holds its socket: {open} open connections"
    );

    // Dropped without any other navigation: the socket must still go away.
    let before = server.open_connections();
    {
        let slow_url = server.url("/api/never");
        let slow_goto = page.goto(&slow_url, Goto::default().timeout(Duration::from_secs(30)));
        tokio::pin!(slow_goto);
        let _ = tokio::time::timeout(Duration::from_millis(50), &mut slow_goto).await;
    }
    let open = wait_for_open_connections(&server, before, Duration::from_secs(1)).await;
    assert!(
        open <= before,
        "a dropped goto still holds its socket: {open} open connections (was {before})"
    );

    page.close().await.expect("close");
    engine.close();
}

/// Issuing a second `goto` while the first is in flight cancels the first
/// with `net::ERR_ABORTED` and installs the second's document
/// (`docs/spec/11-library-api.md` §1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_goto_cancels_first_with_err_aborted() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // Start navigating to a URL that never responds in a background task and
    // wait for its request event, so the navigation is provably in flight.
    let slow_url = server.url("/api/never");
    let mut events = page.events();
    let page_for_slow = page.clone();
    let slow_task = tokio::spawn(async move {
        page_for_slow
            .goto(&slow_url, Goto::default().timeout(Duration::from_secs(30)))
            .await
    });
    support::wait_for_request(&mut events, "/api/never").await;

    // Issue a fast navigation to a different URL
    let fast_url = server.url("/tiny/index.html");
    let cancel_started = std::time::Instant::now();
    let fast_navigation = page
        .goto(&fast_url, Goto::default())
        .await
        .expect("fast navigation completes");
    assert_eq!(fast_navigation.status, 200);
    assert_eq!(fast_navigation.final_url.as_str(), &fast_url);

    // The first navigation is cancelled promptly, not at its own deadline
    // (B-R4-1): the pre-head send races the generation.
    let slow_result = slow_task.await.expect("slow task completes");
    let cancel_elapsed = cancel_started.elapsed();
    assert!(
        cancel_elapsed < Duration::from_secs(1),
        "the cancelled pre-head navigation returned after {cancel_elapsed:?}"
    );
    assert!(
        slow_result.is_err(),
        "slow navigation was cancelled: {slow_result:?}"
    );
    if let Err(tenta_core::Error::Net { code, .. }) = slow_result {
        assert_eq!(
            code,
            ErrorName::Aborted.as_str(),
            "cancelled navigation returns ERR_ABORTED"
        );
    } else {
        panic!("expected Net error with ERR_ABORTED, got {slow_result:?}");
    }

    // The page's current document should be the fast one
    let doc = page.document().expect("document installed");
    let text = doc.text();
    assert!(
        text.len() < 1_000,
        "page contains tiny document, not stalled /api/never"
    );
    drop(doc);

    // Verify the installed document matches the fast navigation
    let installed_doc = page.take_document().expect("document");
    assert_eq!(
        installed_doc.navigation_id(),
        fast_navigation.navigation_id,
        "installed document's navigation_id should match the fast navigation"
    );

    page.close().await.expect("close");
    engine.close();
}

/// Closing a page while a navigation is in flight cancels the navigation
/// (`docs/spec/11-library-api.md` §1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_during_goto_cancels_navigation() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // Start navigating to a URL that never responds in a background task
    let url = server.url("/api/never");
    let mut events = page.events();
    let page_for_goto = page.clone();
    let goto_task = tokio::spawn(async move {
        page_for_goto
            .goto(&url, Goto::default().timeout(Duration::from_secs(30)))
            .await
    });
    support::wait_for_request(&mut events, "/api/never").await;

    // Close the page while the navigation is in flight
    let close_started = std::time::Instant::now();
    page.close().await.expect("close succeeds");

    // The in-flight navigation is cancelled promptly (B-R4-1).
    let result = goto_task.await.expect("goto task completes");
    let close_elapsed = close_started.elapsed();
    assert!(
        close_elapsed < Duration::from_secs(1),
        "the navigation cancelled by close() returned after {close_elapsed:?}"
    );
    assert!(
        result.is_err(),
        "navigation was cancelled by close: {result:?}"
    );
    // It might be Net/ERR_ABORTED or Closed, both are acceptable
    match result {
        Err(tenta_core::Error::Net { code, .. }) => {
            assert_eq!(code, ErrorName::Aborted.as_str());
        }
        Err(tenta_core::Error::Closed) => {
            // Also acceptable: close cancels the channel
        }
        other => panic!("expected Net/ERR_ABORTED or Closed, got {other:?}"),
    }

    engine.close();
}

/// A navigation cancelled during its settle wait returns `Err` and does not
/// overwrite the winner's `navigation()`/`settle_state()` (B-R3-1).
/// When navigation A (Text condition that never appears) is settling and a
/// concurrent navigation B wins and installs, A's settle timeout should not
/// overwrite B's navigation data.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settle_race_text_cancelled_by_goto() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // Start navigating to static-article with a Text condition that never appears.
    // This will install the document and start waiting for the condition.
    let mut events = page.events();
    let page_for_slow = page.clone();
    let slow_url = server.url("/static-article/index.html");
    let slow_task = tokio::spawn(async move {
        page_for_slow
            .goto(
                &slow_url,
                Goto::default()
                    .wait_until(tenta_browser::Settle::Text("never appears".to_string()))
                    .timeout(Duration::from_secs(3)),
            )
            .await
    });

    // Wait for the slow navigation's document to be installed (wait for TierChanged event).
    // With N-R4-3, TierChanged is emitted when document is installed.
    support::wait_for_tier_change(&mut events).await;

    // Issue a fast navigation that should win and measure cancel latency.
    let cancel_start = std::time::Instant::now();
    let fast_url = server.url("/tiny/index.html");
    let fast_nav = page
        .goto(&fast_url, Goto::default())
        .await
        .expect("fast navigation completes");
    assert_eq!(fast_nav.status, 200);
    assert_eq!(fast_nav.final_url.as_str(), &fast_url);
    let fast_id = fast_nav.navigation_id;

    // The slow navigation should have been cancelled and return an error promptly (B-R4-1).
    // Cancelled settle waits should wake immediately, not wait for the deadline.
    let slow_result = slow_task.await.expect("slow task completes");
    let cancel_elapsed = cancel_start.elapsed();
    assert!(
        slow_result.is_err(),
        "slow navigation was cancelled: {slow_result:?}"
    );
    // Should return quickly (~milliseconds), not wait for the 3-second deadline.
    // Cancel-safety bounds latency to < 1 s (B-R4-1).
    assert!(
        cancel_elapsed.as_secs() < 1,
        "cancelled settle wait should return within 1 s, got {cancel_elapsed:?}"
    );

    // Verify the page's navigation is the fast one, not the slow one
    let page_nav = page.navigation().expect("page has navigation");
    assert_eq!(
        page_nav.navigation_id, fast_id,
        "page.navigation() should be the fast navigation"
    );
    assert_eq!(
        page_nav.final_url.as_str(),
        &fast_url,
        "page.navigation() should have the fast URL"
    );

    page.close().await.expect("close");
    engine.close();
}

/// A navigation cancelled during its settle wait by close() returns `Err`
/// and leaves the page with no navigation (B-R3-1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settle_race_text_cancelled_by_close() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // Start navigating to static-article with a Text condition that never appears.
    let mut events = page.events();
    let page_for_slow = page.clone();
    let slow_url = server.url("/static-article/index.html");
    let slow_task = tokio::spawn(async move {
        page_for_slow
            .goto(
                &slow_url,
                Goto::default()
                    .wait_until(tenta_browser::Settle::Text("never appears".to_string()))
                    .timeout(Duration::from_secs(3)),
            )
            .await
    });

    // Wait for the slow navigation's document to be installed
    support::wait_for_tier_change(&mut events).await;

    // Close the page while the navigation is settling and measure cancel latency
    let cancel_start = std::time::Instant::now();
    page.close().await.expect("close succeeds");

    // The slow navigation should have been cancelled promptly (B-R4-1).
    // Cancelled settle waits should wake immediately when close() is called.
    let slow_result = slow_task.await.expect("slow task completes");
    let cancel_elapsed = cancel_start.elapsed();
    assert!(
        slow_result.is_err(),
        "slow navigation was cancelled by close: {slow_result:?}"
    );
    // Should return quickly (~milliseconds), not wait for the 3-second deadline.
    // Cancel-safety bounds latency to < 1 s (B-R4-1).
    assert!(
        cancel_elapsed.as_secs() < 1,
        "cancelled settle wait after close() should return within 1 s, got {cancel_elapsed:?}"
    );

    engine.close();
}

/// Dropping a `goto` future after the response head does not discard the
/// previous document (M-R3-1). The page should keep the previous document
/// and navigation until either a new document is installed or the page is closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_after_response_head_keeps_previous_document() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // Install the tiny document first
    let tiny_url = server.url("/tiny/index.html");
    let tiny_nav = page
        .goto(&tiny_url, Goto::default())
        .await
        .expect("tiny navigation completes");
    assert_eq!(tiny_nav.status, 200);
    let tiny_doc = page.document().expect("document installed");
    let tiny_text_len = tiny_doc.text().len();
    assert!(tiny_text_len < 1_000);
    drop(tiny_doc);

    // Navigate to /api/sse, which streams forever (or until the timeout), on
    // a task so the future is actually polled; once the response head has
    // been reported, abort the task, which drops the goto future mid-body.
    let sse_url = server.url("/api/sse");
    let mut events = page.events();
    let page_for_sse = page.clone();
    let sse_task = tokio::spawn(async move {
        page_for_sse
            .goto(&sse_url, Goto::default().timeout(Duration::from_secs(10)))
            .await
    });
    support::wait_for_response(&mut events, "/api/sse").await;
    sse_task.abort();
    let join = sse_task.await;
    assert!(
        join.as_ref().is_err_and(|error| error.is_cancelled()),
        "the SSE goto should have been aborted, got {join:?}"
    );

    // The page should still have the tiny document, not None
    let doc = page.document();
    assert!(
        doc.is_some(),
        "page should still have the previous document after a dropped goto"
    );
    if let Some(doc) = doc {
        assert_eq!(
            doc.text().len(),
            tiny_text_len,
            "page should have the tiny document, not a partial SSE response"
        );
    }

    // The page's navigation should still be the tiny one
    let page_nav = page.navigation().expect("page has navigation");
    assert_eq!(
        page_nav.final_url.as_str(),
        &tiny_url,
        "page.navigation() should still be the tiny navigation"
    );

    page.close().await.expect("close");
    engine.close();
}

/// A hammer test for the close() vs goto() race (N-R4-1). A goto that
/// finished before `close()` bumped the generation may return Ok; one that
/// was still in flight when `close()` returned must fail, and a goto that
/// returns Ok must be the navigation the page holds (its result was written
/// under the same lock that `close()` takes).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_vs_goto_race_hammer_test() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);

    let mut races = Vec::new();
    for seed in 0..120u64 {
        let page = context.new_page(PageOptions::tier1()).await.expect("page");
        let page_for_goto = page.clone();
        let slow_url = server.url("/static-article/index.html");
        let goto_task = tokio::spawn(async move {
            page_for_goto
                .goto(&slow_url, Goto::default().timeout(Duration::from_secs(10)))
                .await
        });

        tokio::time::sleep(Duration::from_millis(seed % 40)).await;
        page.close().await.expect("close");
        let goto_result = goto_task.await.expect("goto task completes");

        // An Ok result was written under the state lock before close()
        // bumped the generation, so the page must hold that navigation; a
        // result written after the bump is an error, never a stale Ok.
        if let Ok(navigation) = &goto_result {
            let held = page.navigation().map(|n| n.navigation_id);
            if held != Some(navigation.navigation_id) {
                races.push(format!(
                    "iteration {seed}: goto returned Ok for navigation {} but the page holds {held:?}",
                    navigation.navigation_id
                ));
            }
        }
    }
    assert!(races.is_empty(), "close/goto races:\n{}", races.join("\n"));

    engine.close();
}

/// A navigation whose head arrived and whose body then stalls must abort at
/// once when a concurrent `goto` cancels it — not wait for a body chunk that
/// never comes, holding the cancelled navigation and its socket to the
/// deadline (B-R5-1, round 5).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_navigation_cancelled_while_its_body_stalls_aborts_at_once() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    let stall_url = server.url("/api/stall");
    let mut events = page.events();
    let page_for_stall = page.clone();
    let stall_task = tokio::spawn(async move {
        page_for_stall
            .goto(&stall_url, Goto::default().timeout(Duration::from_secs(30)))
            .await
    });
    // The head has arrived: the body loop is now parked on a body that will
    // never send a chunk.
    support::wait_for_response(&mut events, "/api/stall").await;
    assert_eq!(
        wait_for_open_connections(&server, 1, Duration::from_secs(2)).await,
        1,
        "the stalled body holds one connection"
    );

    let cancel_started = std::time::Instant::now();
    page.goto(&server.url("/tiny/index.html"), Goto::default())
        .await
        .expect("fast navigation");
    let stalled = stall_task.await.expect("stall task completes");
    let cancel_elapsed = cancel_started.elapsed();
    assert!(
        cancel_elapsed < Duration::from_secs(2),
        "the body-phase cancel returned after {cancel_elapsed:?}, not promptly"
    );
    match &stalled {
        Err(Error::Net { code, .. }) => assert_eq!(code, ErrorName::Aborted.as_str()),
        other => panic!("expected ERR_ABORTED, got {other:?}"),
    }
    let open = wait_for_open_connections(&server, 1, Duration::from_secs(1)).await;
    assert!(
        open <= 1,
        "the stalled request still holds its socket: {open} open connections"
    );

    page.close().await.expect("close");
    engine.close();
}

/// Closing a context closes its pages: a page of a closed context stops
/// navigating and its in-flight navigation releases its socket (M-R5-1,
/// round 5).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_context_closes_its_pages() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // A navigation is in flight when the context closes.
    let never_url = server.url("/api/never");
    let mut events = page.events();
    let page_for_never = page.clone();
    let never_task = tokio::spawn(async move {
        page_for_never
            .goto(&never_url, Goto::default().timeout(Duration::from_secs(30)))
            .await
    });
    support::wait_for_request(&mut events, "/api/never").await;
    assert_eq!(
        wait_for_open_connections(&server, 1, Duration::from_secs(2)).await,
        1,
        "the parked navigation holds one connection"
    );

    context.close();

    // The page is closed, its in-flight navigation aborted and its socket
    // released, and it refuses new navigations.
    assert!(page.is_closed(), "the context's page is closed");
    let never_result = never_task.await.expect("never task completes");
    assert!(
        never_result.is_err(),
        "the in-flight navigation was cancelled: {never_result:?}"
    );
    let open = wait_for_open_connections(&server, 0, Duration::from_secs(2)).await;
    assert_eq!(open, 0, "the closed context left {open} sockets open");
    let refused = page
        .goto(&server.url("/tiny/index.html"), Goto::default())
        .await;
    assert!(
        matches!(refused, Err(Error::Closed)),
        "a page of a closed context refuses to navigate: {refused:?}"
    );
    assert_eq!(context.page_groups(), 0, "the group table is cleared");

    engine.close();
}
