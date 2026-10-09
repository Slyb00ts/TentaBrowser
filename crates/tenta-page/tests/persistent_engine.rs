//! The engine a page keeps after its navigation settles
//! (`docs/spec/02-architecture.md` §3 and §5.1).
//!
//! These drive the stub engine over the real event loop (`support`), so the
//! timer and provenance bookkeeping the settle machine sees is the production
//! one. Every test runs on one task set that serves the page thread
//! (`support::with_page_thread`), because the driver of a kept engine is a task
//! that has to outlive the operation that started it.

mod support;

use std::time::{Duration, Instant};

use support::{Harness, URL, with_page_thread};
use tenta_core::{Budgets, Error};
use tenta_js::engine::ScriptKind;
use tenta_page::PageEvent;
use tenta_page::page::{Goto, Page};
use tenta_page::settle::{Settle, SettleState};

const PAGE: &str =
    "<!doctype html><html><body><script>start</script><p id=\"tail\">end</p></body></html>";

/// How long a test waits for something the page does by itself.
const PATIENCE: Duration = Duration::from_secs(10);

async fn load(harness: &Harness) {
    harness
        .page
        .goto(URL, Goto::new().wait_until(Settle::Load))
        .await
        .expect("navigation");
}

/// Runs one command line of the stub's language as a tracked script, the way a
/// host action that starts page work does.
async fn run(page: &Page, command: &'static str) {
    page.with_engine(move |engine| {
        engine
            .run_script(command, ScriptKind::Classic)
            .expect("the stub runs every script");
    })
    .await
    .expect("the page has an engine");
}

/// Whether a script command of the stub appended the marker `id`.
fn fired(harness: &Harness, id: &str) -> bool {
    harness.record.lock().unwrap().has_marker(id)
}

/// Whether the engine's document matches `selector`.
async fn has(page: &Page, selector: &'static str) -> bool {
    page.with_engine(move |engine| {
        let shared = engine.document();
        let document = shared.borrow();
        support::has_selector(&document, selector)
    })
    .await
    .expect("the page has an engine")
}

#[tokio::test(flavor = "current_thread")]
async fn a_page_keeps_its_engine_when_the_navigation_settles() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        {
            let record = harness.record.lock().unwrap();
            assert_eq!(record.engine_creations, 1);
            assert_eq!(record.engine_drops, 0, "the engine outlived the navigation");
        }
        assert!(
            harness.page.document().is_none(),
            "the tree is the engine's while the page keeps one"
        );
        assert!(has(&harness.page, "#tail").await);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_tracked_timer_set_after_load_fires_and_a_later_settle_waits_for_it() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        run(&harness.page, "setTimeout(mutate late, 30)").await;
        assert!(
            !fired(&harness, "late"),
            "the timer has not fired at the moment it was set"
        );

        let state = harness
            .page
            .wait_for_settle(Settle::NetworkIdle, PATIENCE)
            .await
            .expect("settle");

        assert_eq!(state, SettleState::Settled { busy: false });
        assert!(
            fired(&harness, "late"),
            "the settle ended only after the timer ran in the same realm"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_timer_fires_without_anyone_waiting_for_it() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        run(&harness.page, "setTimeout(mutate unattended, 5)").await;

        // Nobody settles: only the page's own driver can run the timer.
        let deadline = Instant::now() + PATIENCE;
        while !fired(&harness, "unattended") {
            assert!(Instant::now() < deadline, "the timer never fired");
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test(start_paused = true, flavor = "current_thread")]
async fn an_idle_page_does_not_turn_its_engine() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        // Let the driver take its first turn and park.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let parked = harness.record.lock().unwrap().timer_pumps;

        tokio::time::sleep(Duration::from_secs(30)).await;

        assert_eq!(
            harness.record.lock().unwrap().timer_pumps,
            parked,
            "half a minute of virtual time with nothing scheduled turned the engine"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_settle_that_is_dropped_hands_the_engine_back_to_the_driver() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        run(&harness.page, "setTimeout(mutate after_cancel, 40)").await;
        let page = harness.page.clone();
        let waiting = tokio::task::spawn_local(async move {
            page.wait_for_settle(Settle::Selector("#never".to_owned()), PATIENCE)
                .await
        });
        // The wait has to be running before it is cancelled.
        tokio::task::yield_now().await;
        waiting.abort();

        let deadline = Instant::now() + PATIENCE;
        while !fired(&harness, "after_cancel") {
            assert!(
                Instant::now() < deadline,
                "nothing drove the engine after the settle was dropped"
            );
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_navigation_replaces_the_engine_and_the_document() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        run(&harness.page, "mutate first_realm").await;
        assert!(has(&harness.page, "div").await);

        load(&harness).await;

        {
            let record = harness.record.lock().unwrap();
            assert_eq!(
                record.engine_creations, 2,
                "the new document got a fresh engine"
            );
            assert_eq!(
                record.engine_drops, 1,
                "and the old one ended with its document"
            );
        }
        assert!(
            !has(&harness.page, "div").await,
            "the old document did not survive into the new realm"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn closing_a_page_drops_its_engine() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;

        harness.page.close().await.expect("close");

        assert_eq!(harness.record.lock().unwrap().engine_drops, 1);
        assert!(matches!(
            harness.page.with_engine(|_| ()).await,
            Err(Error::Closed)
        ));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn begin_close_drops_the_engine_without_waiting() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;

        assert!(harness.page.begin_close());

        // The release is a page task, which the serving loop runs.
        let deadline = Instant::now() + PATIENCE;
        while harness.record.lock().unwrap().engine_drops == 0 {
            assert!(Instant::now() < deadline, "the engine was never dropped");
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn taking_the_document_ends_the_engine_and_hands_over_the_scripted_tree() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        run(&harness.page, "mutate scripted").await;

        let document = harness.page.take_document().await.expect("document");

        assert_eq!(harness.record.lock().unwrap().engine_drops, 1);
        assert!(
            document
                .query_selector("div")
                .expect("selector parses")
                .is_some(),
            "the tree carries what the engine did to it"
        );
        assert!(matches!(
            harness.page.with_engine(|_| ()).await,
            Err(Error::InvalidArgument { .. })
        ));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_page_with_no_script_gets_an_engine_on_first_use() {
    let harness = Harness::new("<!doctype html><html><body><p id=\"tail\">end</p></body></html>");
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        assert_eq!(harness.record.lock().unwrap().engine_creations, 0);
        assert!(harness.page.document().is_some());

        run(&harness.page, "setTimeout(mutate lazy, 5)").await;

        assert_eq!(harness.record.lock().unwrap().engine_creations, 1);
        let state = harness
            .page
            .wait_for_settle(Settle::NetworkIdle, PATIENCE)
            .await
            .expect("settle");
        assert_eq!(state, SettleState::Settled { busy: false });
        assert!(fired(&harness, "lazy"));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_page_with_no_engine_settles_at_once() {
    let harness = Harness::builder(PAGE).no_engine().build();
    with_page_thread(&harness.thread, async {
        load(&harness).await;

        let state = harness
            .page
            .wait_for_settle(Settle::NetworkIdle, PATIENCE)
            .await
            .expect("settle");

        assert_eq!(state, SettleState::Settled { busy: false });
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_runaway_timer_after_load_is_terminated_and_reported() {
    let harness = Harness::builder(PAGE)
        .watchdog(true)
        .budgets(Budgets::default().evaluate_time(Duration::from_millis(100)))
        .build();
    with_page_thread(&harness.thread, async {
        let mut events = harness.page.events();
        load(&harness).await;

        run(&harness.page, "setTimeout(block, 0)").await;
        let state = harness
            .page
            .wait_for_settle(Settle::NetworkIdle, PATIENCE)
            .await
            .expect("settle");

        assert_eq!(state, SettleState::Settled { busy: false });
        assert_eq!(harness.record.lock().unwrap().terminations, 1);
        assert!(
            events
                .drain()
                .contains(&PageEvent::ScriptTerminated { entry: "timers" }),
            "the page reports the entry it cut short"
        );
        assert_eq!(
            harness.page.crash_reason(),
            None,
            "termination is not a crash"
        );
        assert!(
            !has(&harness.page, "#absent").await,
            "the page still answers after the runaway"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn two_simultaneous_settle_calls_both_resolve() {
    let harness = Harness::new(PAGE);
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        run(&harness.page, "setTimeout(mutate shared, 30)").await;

        let (first, second) = tokio::join!(
            harness.page.wait_for_settle(Settle::NetworkIdle, PATIENCE),
            harness.page.wait_for_settle(Settle::NetworkIdle, PATIENCE),
        );

        assert_eq!(first.expect("first"), SettleState::Settled { busy: false });
        assert_eq!(
            second.expect("second"),
            SettleState::Settled { busy: false }
        );
        assert!(fired(&harness, "shared"));
        // The driver is back once both are done.
        run(&harness.page, "setTimeout(mutate after_both, 5)").await;
        let deadline = Instant::now() + PATIENCE;
        while !fired(&harness, "after_both") {
            assert!(Instant::now() < deadline, "nothing drives the engine");
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn closing_a_page_ends_a_running_settle_and_drops_the_engine() {
    for graceful in [true, false] {
        let harness = Harness::new(PAGE);
        with_page_thread(&harness.thread, async {
            load(&harness).await;
            let page = harness.page.clone();
            let waiting = tokio::task::spawn_local(async move {
                page.wait_for_settle(Settle::Selector("#never".to_owned()), PATIENCE)
                    .await
            });
            // Let the wait start and take the engine over.
            for _ in 0..20 {
                tokio::task::yield_now().await;
            }

            if graceful {
                harness.page.close().await.expect("close");
            } else {
                assert!(harness.page.begin_close());
            }

            let outcome = waiting.await.expect("the wait task");
            assert!(
                outcome.is_err(),
                "the wait ended with an error: {outcome:?}"
            );
            let deadline = Instant::now() + PATIENCE;
            while harness.record.lock().unwrap().engine_drops == 0 {
                assert!(Instant::now() < deadline, "the engine was never dropped");
                tokio::task::yield_now().await;
            }
        })
        .await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_tier_one_page_refuses_host_entries() {
    let harness = Harness::builder(PAGE).no_engine().build();
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        assert!(matches!(
            harness.page.with_engine(|_| ()).await,
            Err(Error::TierDenied { .. })
        ));
    })
    .await;

    let harness = Harness::builder(PAGE).javascript(false).build();
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        assert!(matches!(
            harness.page.with_engine(|_| ()).await,
            Err(Error::Unsupported { .. })
        ));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_error_page_has_no_realm_and_keeps_its_document() {
    let harness = Harness::builder(PAGE).failing_route("bad").build();
    with_page_thread(&harness.thread, async {
        let bad = format!("{URL}bad");
        let _ = harness.page.goto(&bad, Goto::new()).await;
        assert!(
            harness.page.document().is_some(),
            "the error page is installed"
        );

        let refused = harness.page.with_engine(|_| ()).await;

        assert!(
            matches!(refused, Err(Error::InvalidArgument { .. })),
            "{refused:?}"
        );
        assert!(
            harness.page.document().is_some(),
            "a refused entry leaves the document where it was"
        );
        assert_eq!(harness.record.lock().unwrap().engine_creations, 0);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_settle_during_a_navigation_without_an_engine_does_not_settle_at_once() {
    let harness = Harness::builder("<!doctype html><html><body>x</body></html>")
        .delayed_route(
            "slow",
            "<!doctype html><html><body>slow</body></html>",
            Duration::from_millis(300),
        )
        .build();
    with_page_thread(&harness.thread, async {
        load(&harness).await;
        let mut events = harness.page.events();
        let page = harness.page.clone();
        let navigating =
            tokio::task::spawn_local(
                async move { page.goto(&format!("{URL}slow"), Goto::new()).await },
            );
        loop {
            match events.next().await {
                Some(PageEvent::Request { url, .. }) if url.path().ends_with("slow") => break,
                Some(_) => {}
                None => panic!("the event stream ended"),
            }
        }

        let during = harness
            .page
            .wait_for_settle(Settle::NetworkIdle, PATIENCE)
            .await;

        assert!(
            matches!(during, Err(Error::InvalidArgument { .. })),
            "{during:?}"
        );
        navigating.abort();
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_title_of_a_kept_engine_is_readable_from_any_thread() {
    let harness = Harness::new(
        "<!doctype html><html><head><title>Kept</title></head><body><script>start</script></body></html>",
    );
    with_page_thread(&harness.thread, async {
        load(&harness).await;

        assert!(harness.page.document().is_none());
        assert_eq!(harness.page.title().as_deref(), Some("Kept"));
    })
    .await;
}

/// With the default `evaluate_time` (30 s) and a window budget of 300 ms, an
/// entry is terminated at 150 ms: one runaway never exhausts the window alone,
/// two back to back do.
#[tokio::test(flavor = "current_thread")]
async fn one_terminated_runaway_survives_and_two_in_one_window_crash_the_page() {
    let harness = Harness::builder(PAGE)
        .watchdog(true)
        .budgets(Budgets::default().script_cpu(Duration::from_millis(300)))
        .build();
    with_page_thread(&harness.thread, async {
        load(&harness).await;

        run(&harness.page, "setTimeout(block, 0)").await;
        let first = harness
            .page
            .wait_for_settle(Settle::NetworkIdle, PATIENCE)
            .await
            .expect("settle");
        assert_eq!(first, SettleState::Settled { busy: false });
        assert_eq!(
            harness.page.crash_reason(),
            None,
            "one runaway is survivable"
        );
        assert_eq!(harness.record.lock().unwrap().terminations, 1);

        run(&harness.page, "setTimeout(block, 0)").await;
        let second = harness
            .page
            .wait_for_settle(Settle::NetworkIdle, PATIENCE)
            .await;

        assert!(matches!(second, Err(Error::Crashed { .. })), "{second:?}");
        assert!(harness.page.crash_reason().is_some());
    })
    .await;
}
