//! The `tenta-wpt-driver` per-test loop against a real page and a real socket
//! (P2 WPT gate, slice S1).
//!
//! `crates/tenta-browser/src/wpt.rs`'s unit tests cover the pure halves — the
//! status mapping, the host matching and the hook script's text. This file is
//! the end-to-end half: a fixture server serves a page and a **hand-written**
//! stand-in for `testharness.js`, the driver runs its per-test loop over the
//! page, and the JSON it produced is what is asserted. What only this level can
//! show is that the whole transport works — the hook really replaces
//! `resources/testharnessreport.js` on the wire, the hook's `fetch()` really
//! reaches the interceptor's request body, and the payload really comes back
//! through the driver's own JSON.
//!
//! The harness stand-in is deliberately not the WPT file: this is a test of the
//! driver, and depending on a gitignored checkout would make it unrunnable. It
//! reproduces only the contract the driver relies on — `setup`,
//! `add_completion_callback`, the two **numeric** status enums, and a completion
//! that runs from a `setTimeout(0)` queued by the window's `load` listener,
//! which is the shape `testharness.js` itself has and the reason `Settle::Load`
//! cannot be used.

#![cfg(feature = "v8")]

use tenta_browser::wpt::{Plan, PlanTest, Results};
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

/// The stand-in `resources/testharness.js`.
///
/// Status numbers are `testharness.js`'s: subtest `PASS: 0`, `FAIL: 1`, and
/// harness `OK: 0`, `ERROR: 1`. `__wpt` is a global so a test page can set the
/// harness status directly, which is what the real harness does internally when
/// it raises one.
const HARNESS: &str = r#"
var __wpt = { tests: [], status: { status: 0, message: null }, completion: null };
function setup(properties) {}
function add_completion_callback(callback) { __wpt.completion = callback; }
function test(name, body) {
  try {
    body();
    __wpt.tests.push({ name: name, status: 0, message: null });
  } catch (error) {
    __wpt.tests.push({ name: name, status: 1, message: String(error && error.message) });
  }
}
function assert_equals(actual, expected, message) {
  if (actual !== expected) {
    throw new Error(message + ": expected " + expected + ", got " + actual);
  }
}
window.addEventListener("load", function () {
  setTimeout(function () {
    if (__wpt.completion) {
      __wpt.completion(__wpt.tests, __wpt.status);
    }
  }, 0);
});
"#;

/// A page with one passing and one failing subtest.
const PAGE: &str = "<!doctype html><html><head><title>driver fixture</title>\
     <script src=\"/resources/testharness.js\"></script>\
     <script src=\"/resources/testharnessreport.js\"></script></head><body>\
     <script>\
     test('a passing subtest', function () { assert_equals(1 + 1, 2, 'sum'); });\
     test('a failing subtest', function () { assert_equals(1 + 1, 3, 'sum'); });\
     </script></body></html>";

/// A page whose harness ends in `ERROR`, with a subtest that passed anyway.
const ERROR_PAGE: &str = "<!doctype html><html><head><title>driver fixture</title>\
     <script src=\"/resources/testharness.js\"></script>\
     <script src=\"/resources/testharnessreport.js\"></script></head><body>\
     <script>\
     test('a passing subtest', function () { assert_equals(2 + 2, 4, 'sum'); });\
     __wpt.status.status = 1;\
     __wpt.status.message = 'the harness gave up';\
     </script></body></html>";

/// A page that never requests the report script, so no hook can ever arrive.
const STALL_PAGE: &str = "<!doctype html><html><head><title>stall</title></head>\
     <body><p>no harness here</p></body></html>";

/// Serves each `(relative path, contents)` on loopback.
async fn serve(files: &[(&str, &str)]) -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("wpt-driver");
    for (relative, contents) in files {
        temp.write(relative, contents);
    }
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
        .await
        .expect("fixture server");
    (server, temp)
}

/// The plan `run.py` would build for these tests.
fn plan(server: &FixtureServer, tests: &[(&str, u64)]) -> Plan {
    Plan {
        base: format!("http://{}", server.addr()),
        resolver_rules: false,
        jobs: 1,
        deadline_epoch_ms: None,
        tests: tests
            .iter()
            .map(|(url, timeout_ms)| PlanTest {
                url: (*url).to_owned(),
                timeout_ms: *timeout_ms,
            })
            .collect(),
    }
}

/// When `subtests` has `len` or more entries, the one at `index`.
fn subtest(results: &Results, index: usize) -> &tenta_browser::wpt::SubtestOutcome {
    let test = &results.tests[0];
    test.subtests
        .get(index)
        .unwrap_or_else(|| panic!("subtest {index} of {:?}", test.subtests))
}

/// The whole transport in one test, asserted from the driver's own output.
///
/// The harness status, both subtest names, both subtest statuses and the failing
/// subtest's message are each a different link: the status proves the numeric
/// enum was mapped, the names prove the harness's own objects survived
/// serialization, the `PASS` proves a page-side `test()` ran to completion, and
/// the message proves the assertion text travelled with it. The request count
/// is the non-vacuous one: the document and `resources/testharness.js` are the
/// only things the *server* saw, so `resources/testharnessreport.js` was
/// answered by the interceptor and the result POST was answered without touching
/// the network at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_driver_reports_the_harness_status_and_every_subtest() {
    let (server, _temp) =
        serve(&[("index.html", PAGE), ("resources/testharness.js", HARNESS)]).await;

    let results = tenta_browser::wpt::run(plan(&server, &[("/index.html", 10_000)]))
        .await
        .expect("the run finishes");

    assert_eq!(results.browser.name, "tentabrowser");
    assert_eq!(results.tests.len(), 1);
    assert_eq!(results.tests[0].test, "/index.html");
    assert_eq!(
        results.tests[0].status, "OK",
        "the hook arrived and its harness status was mapped"
    );
    assert_eq!(results.tests[0].message, None);
    assert_eq!(results.tests[0].subtests.len(), 2);
    assert_eq!(subtest(&results, 0).name, "a passing subtest");
    assert_eq!(subtest(&results, 0).status, "PASS");
    assert_eq!(subtest(&results, 0).message, None);
    assert_eq!(subtest(&results, 1).name, "a failing subtest");
    assert_eq!(subtest(&results, 1).status, "FAIL");
    assert_eq!(
        subtest(&results, 1).message.as_deref(),
        Some("sum: expected 3, got 2"),
        "the assertion's own message reached the driver"
    );
    assert_eq!(
        server.requests_served(),
        2,
        "the document and the harness script: the report script was fulfilled by \
         the interceptor and the result POST never reached the server"
    );
}

/// A harness status that is not `OK` travels with its message, and its subtests
/// still do: `run.py` records the two levels separately and only the per-test
/// status may fail a run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_ok_harness_status_keeps_its_message_and_its_subtests() {
    let (server, _temp) = serve(&[
        ("error.html", ERROR_PAGE),
        ("resources/testharness.js", HARNESS),
    ])
    .await;

    let results = tenta_browser::wpt::run(plan(&server, &[("/error.html", 10_000)]))
        .await
        .expect("the run finishes");

    assert_eq!(results.tests[0].status, "ERROR");
    assert_eq!(
        results.tests[0].message.as_deref(),
        Some("the harness gave up")
    );
    assert_eq!(results.tests[0].subtests.len(), 1);
    assert_eq!(subtest(&results, 0).status, "PASS");
}

/// A test whose hook never arrives is `TIMEOUT` with no subtests, and the driver
/// gives up at the plan's own deadline rather than at the navigation's.
///
/// The duration is asserted below the deadline so a driver that returned early
/// — one that settled on `load` or on a network-idle window, which is exactly
/// what a hook-less page looks like from the settle machine — is caught rather
/// than passed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hook_that_never_arrives_is_a_timeout_with_no_subtests() {
    let (server, _temp) = serve(&[("stall.html", STALL_PAGE)]).await;

    let results = tenta_browser::wpt::run(plan(&server, &[("/stall.html", 400)]))
        .await
        .expect("the run finishes");

    assert_eq!(results.tests[0].status, "TIMEOUT");
    assert!(results.tests[0].subtests.is_empty());
    assert_eq!(
        results.tests[0].message.as_deref(),
        Some("the completion hook did not arrive within 400 ms")
    );
    assert!(
        results.tests[0].duration_ms >= 350,
        "the driver waited for the deadline instead of settling early: {} ms",
        results.tests[0].duration_ms
    );
    assert_eq!(server.requests_served(), 1, "the document only");
}

/// A run past its deadline starts no test at all.
///
/// The request count is the assertion that makes this non-vacuous: a `NOTRUN`
/// that still created a page and navigated would report the same status while
/// having done the work the deadline exists to stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_past_its_deadline_reports_not_run_without_touching_the_server() {
    let (server, _temp) =
        serve(&[("index.html", PAGE), ("resources/testharness.js", HARNESS)]).await;

    let mut plan = plan(&server, &[("/index.html", 10_000)]);
    plan.deadline_epoch_ms = Some(1);

    let results = tenta_browser::wpt::run(plan)
        .await
        .expect("the run finishes");

    assert_eq!(results.tests[0].status, "NOTRUN");
    assert_eq!(
        results.tests[0].message.as_deref(),
        Some("run deadline reached")
    );
    assert_eq!(results.tests[0].duration_ms, 0);
    assert!(results.tests[0].subtests.is_empty());
    assert_eq!(server.requests_served(), 0);
}

/// Several tests run one after another against one engine, and each gets its own
/// page and its own result.
///
/// The second run's `subtest` index is what pins the per-test slot: a hook still
/// addressed to the first test would be dropped by the index check and the
/// second test would end in `TIMEOUT` instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_test_in_a_plan_gets_its_own_result() {
    let (server, _temp) = serve(&[
        ("index.html", PAGE),
        ("error.html", ERROR_PAGE),
        ("resources/testharness.js", HARNESS),
    ])
    .await;

    let results = tenta_browser::wpt::run(plan(
        &server,
        &[("/index.html", 10_000), ("/error.html", 10_000)],
    ))
    .await
    .expect("the run finishes");

    assert_eq!(results.tests.len(), 2);
    assert_eq!(results.tests[0].test, "/index.html");
    assert_eq!(results.tests[0].status, "OK");
    assert_eq!(results.tests[0].subtests.len(), 2);
    assert_eq!(results.tests[1].test, "/error.html");
    assert_eq!(results.tests[1].status, "ERROR");
    assert_eq!(results.tests[1].subtests.len(), 1);
}
