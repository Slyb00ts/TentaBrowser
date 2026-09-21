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
//! `add_completion_callback`, the numeric status enums, a completion that runs
//! from a `setTimeout(0)` queued by the window's `load` listener (the shape
//! `testharness.js` itself has, and the reason `Settle::Load` cannot be used),
//! and the two pieces of `testharness.js`'s timing that `wpt.rs` depends on: a
//! timer armed from `setup`'s `timeout_multiplier`, and a `load` completion
//! gated on there being no pending test.

#![cfg(feature = "v8")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tenta_browser::wpt::{Plan, PlanTest, Results};
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// The stand-in `resources/testharness.js`, with `__BUDGET__` standing in for
/// the budget the harness arms its own timer at.
///
/// Status numbers are `testharness.js`'s: subtest `PASS: 0`, `FAIL: 1`,
/// `NOTRUN: 3`, and harness `OK: 0`, `ERROR: 1`, `TIMEOUT: 2`. `__wpt` is a
/// global so a test page can set the harness status directly, which is what the
/// real harness does internally when it raises one.
///
/// The stand-in reproduces two things beyond the contract the driver relies on,
/// because the driver's two clocks are what the multiplier is for:
/// `setup(properties)` records `properties.timeout_multiplier` and re-arms the
/// harness's timer at `budget * multiplier` — `Tests.prototype.set_timeout`
/// clearing and re-arming at `timeout_length` after `setup` has scaled it — and
/// the `load` listener completes only when no test is pending
/// (`Tests.prototype.all_done`), which is why a harness with a pending
/// `async_test` has to be reported by its own timer.
const HARNESS_TEMPLATE: &str = r#"
var __wpt = { tests: [], pending: 0, status: { status: 0, message: null },
              completion: null, budget: __BUDGET__, multiplier: 1,
              reported: false, timer: null };
function __wpt_arm(millis) {
  clearTimeout(__wpt.timer);
  __wpt.timer = setTimeout(function () {
    __wpt.status.status = 2;
    __wpt_report();
  }, millis);
}
function __wpt_report() {
  if (__wpt.reported) { return; }
  __wpt.reported = true;
  if (__wpt.completion) { __wpt.completion(__wpt.tests, __wpt.status); }
}
function setup(properties) {
  if (properties && properties.timeout_multiplier) {
    __wpt.multiplier = properties.timeout_multiplier;
  }
  __wpt_arm(__wpt.budget * __wpt.multiplier);
}
function add_completion_callback(callback) { __wpt.completion = callback; }
function test(name, body) {
  try {
    body();
    __wpt.tests.push({ name: name, status: 0, message: null });
  } catch (error) {
    __wpt.tests.push({ name: name, status: 1, message: String(error && error.message) });
  }
}
function async_test(name) {
  // `Test`'s own initial status is NOTRUN, and it stays there while the test is
  // pending: that is what a harness that times out reports for it.
  var subtest = { name: name, status: 3, message: null };
  __wpt.tests.push(subtest);
  __wpt.pending++;
  return { done: function () { subtest.status = 0; __wpt.pending--; } };
}
function assert_equals(actual, expected, message) {
  if (actual !== expected) {
    throw new Error(message + ": expected " + expected + ", got " + actual);
  }
}
window.addEventListener("load", function () {
  setTimeout(function () {
    if (__wpt.pending === 0) { __wpt_report(); }
  }, 0);
});
__wpt_arm(__wpt.budget);
"#;

/// The budget `wpt/run.py` gives every fixture below that loads the harness:
/// their plans say 10 000 ms. The harness's own `test_timeout()` and the plan's
/// `timeout_ms` are the same number in a real run, because the runner and the
/// server derive both from the manifest's metadata.
const HARNESS_BUDGET_MS: u64 = 10_000;

/// The stand-in at `budget_ms`, served at `/resources/testharness.js`.
fn harness(budget_ms: u64) -> String {
    assert!(
        HARNESS_TEMPLATE.contains("__BUDGET__"),
        "the stand-in's budget placeholder moved"
    );
    HARNESS_TEMPLATE.replace("__BUDGET__", &budget_ms.to_string())
}

/// A page with one passing and one failing subtest.
const PAGE: &str = "<!doctype html><html><head><title>driver fixture</title>\
     <script src=\"/resources/testharness.js\"></script>\
     <script src=\"/resources/testharnessreport.js\"></script></head><body>\
     <script>\
     test('a passing subtest', function () { assert_equals(1 + 1, 2, 'sum'); });\
     test('a failing subtest', function () { assert_equals(1 + 1, 3, 'sum'); });\
     </script></body></html>";

/// A page whose harness ends in `TIMEOUT` on its own timer: one synchronous
/// subtest that passes, and one `async_test` that never completes, so
/// `all_done()` stays false and the `load` listener cannot report for it.
const SLOW_PAGE: &str = "<!doctype html><html><head><title>driver fixture</title>\
     <script src=\"/resources/testharness.js\"></script>\
     <script src=\"/resources/testharnessreport.js\"></script></head><body>\
     <script>\
     test('a passing subtest', function () { assert_equals(1 + 1, 2, 'sum'); });\
     async_test('a pending subtest');\
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

/// Serves each `(relative path, contents)` on loopback, with the fixture
/// server's own configuration: the two-clock test below needs a delay on one
/// path, which is a property of the server rather than of a file.
async fn serve_with(
    files: &[(&str, &str)],
    configure: impl FnOnce(FixtureServerConfig) -> FixtureServerConfig,
) -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("wpt-driver");
    for (relative, contents) in files {
        temp.write(relative, contents);
    }
    let server = FixtureServer::start(configure(FixtureServerConfig::new(temp.path())))
        .await
        .expect("fixture server");
    (server, temp)
}

/// Serves each `(relative path, contents)` on loopback.
async fn serve(files: &[(&str, &str)]) -> (FixtureServer, TempDir) {
    serve_with(files, |config| config).await
}

/// The plan `run.py` would build for these tests, one job.
fn plan(server: &FixtureServer, tests: &[(&str, u64)]) -> Plan {
    plan_for(&format!("http://{}", server.addr()), tests)
}

/// The same plan against any base URL, for the hand-written servers below.
fn plan_for(base: &str, tests: &[(&str, u64)]) -> Plan {
    Plan {
        base: base.to_owned(),
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

/// The clock `wpt::run` reads, for a run deadline relative to now.
fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after the epoch")
        .as_millis() as u64
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
    let harness = harness(HARNESS_BUDGET_MS);
    let (server, _temp) =
        serve(&[("index.html", PAGE), ("resources/testharness.js", &harness)]).await;

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
    let harness = harness(HARNESS_BUDGET_MS);
    let (server, _temp) = serve(&[
        ("error.html", ERROR_PAGE),
        ("resources/testharness.js", &harness),
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
/// than passed. The bound is the plan's budget itself, which is the half of
/// `ORDER_GRACE` a unit test can hold: the grace may only ever *add* to the
/// deadline, never let the driver report before the budget it was given.
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
        results.tests[0].duration_ms >= 400,
        "the driver waited for the deadline instead of settling early: {} ms",
        results.tests[0].duration_ms
    );
    assert_eq!(server.requests_served(), 1, "the document only");
}

/// The budget of the two-clock test below, in milliseconds.
const TWO_CLOCK_BUDGET_MS: u64 = 1_000;

/// A live harness reports from its own timeout, before the driver's deadline,
/// and its subtests travel with it.
///
/// This is the two-clock defect at its smallest. The driver's deadline starts
/// before the page exists; the harness's own timer starts when
/// `testharness.js` executes, which here is half a budget late — the page-side
/// half of a real page's set-up (page creation, document fetch, the parse up to
/// the harness tag, the harness script's own fetch). The delay is that subject,
/// not a synchronisation sleep, and nothing here asserts on elapsed time.
///
/// The hook script is what closes the gap: answered with the plan's *remaining*
/// budget as `setup({ timeout_multiplier })`, the stand-in re-arms its timer at
/// `budget * multiplier` — the re-arm `Tests.prototype.set_timeout` performs —
/// so it fires around 1 000 ms, while a harness left at its natural length arms
/// a whole budget from that later start, roughly 1 500 ms, and loses to the
/// driver's 1 250 ms. The status and the empty message assertions are the pair
/// that says which of the two clocks reported; the subtest count is what only
/// the harness can supply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_harness_reports_from_its_own_timeout_before_the_driver_gives_up() {
    let harness = harness(TWO_CLOCK_BUDGET_MS);
    let (server, _temp) = serve_with(
        &[
            ("two-clocks.html", SLOW_PAGE),
            ("resources/testharness.js", &harness),
        ],
        |config| {
            config.delay_path(
                "/resources/testharness.js",
                Duration::from_millis(TWO_CLOCK_BUDGET_MS / 2),
            )
        },
    )
    .await;

    let results =
        tenta_browser::wpt::run(plan(&server, &[("/two-clocks.html", TWO_CLOCK_BUDGET_MS)]))
            .await
            .expect("the run finishes");

    assert_eq!(
        results.tests[0].status, "TIMEOUT",
        "the harness timed itself out: {:?}",
        results.tests[0].message
    );
    assert_eq!(
        results.tests[0].message, None,
        "the harness's own status has no message; the driver's says \
         \"the completion hook did not arrive within {TWO_CLOCK_BUDGET_MS} ms\""
    );
    assert_eq!(
        results.tests[0].subtests.len(),
        2,
        "the harness's own subtest list travelled: {:?}",
        results.tests[0].subtests
    );
    assert_eq!(subtest(&results, 0).name, "a passing subtest");
    assert_eq!(subtest(&results, 0).status, "PASS");
    assert_eq!(subtest(&results, 1).name, "a pending subtest");
    assert_eq!(
        subtest(&results, 1).status,
        "NOTRUN",
        "a test that never finished is in the harness's list as NOTRUN"
    );
}

/// A run past its deadline starts no test at all.
///
/// The request count is the assertion that makes this non-vacuous: a `NOTRUN`
/// that still created a page and navigated would report the same status while
/// having done the work the deadline exists to stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_past_its_deadline_reports_not_run_without_touching_the_server() {
    let harness = harness(HARNESS_BUDGET_MS);
    let (server, _temp) =
        serve(&[("index.html", PAGE), ("resources/testharness.js", &harness)]).await;

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
    let harness = harness(HARNESS_BUDGET_MS);
    let (server, _temp) = serve(&[
        ("index.html", PAGE),
        ("error.html", ERROR_PAGE),
        ("resources/testharness.js", &harness),
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

// --- the pool: S3a, S3b, S3c -------------------------------------------------

/// Two tests at two jobs, over the fixtures the sequential tests use.
///
/// The worker pool must not change what a result *is*: both outcomes come back
/// in plan order, and each carries its own subtests rather than a mixture. Both
/// pages here name their subtest `a passing subtest`, so the distinct statuses
/// and subtest counts are what separate them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_tests_at_two_jobs_keep_their_own_results() {
    let harness = harness(HARNESS_BUDGET_MS);
    let (server, _temp) = serve(&[
        ("index.html", PAGE),
        ("error.html", ERROR_PAGE),
        ("resources/testharness.js", &harness),
    ])
    .await;

    let mut plan = plan(&server, &[("/index.html", 10_000), ("/error.html", 10_000)]);
    plan.jobs = 2;

    let results = tenta_browser::wpt::run(plan)
        .await
        .expect("the run finishes");

    assert_eq!(results.tests.len(), 2);
    assert_eq!(
        results.tests[0].test, "/index.html",
        "results are in plan order, not completion order"
    );
    assert_eq!(results.tests[0].status, "OK");
    assert_eq!(results.tests[0].subtests.len(), 2);
    assert_eq!(results.tests[0].subtests[0].name, "a passing subtest");
    assert_eq!(results.tests[1].test, "/error.html");
    assert_eq!(results.tests[1].status, "ERROR");
    assert_eq!(
        results.tests[1].subtests.len(),
        1,
        "the second worker's own subtest list, not the first's"
    );
}

/// A page that loads the harness stand-in and the report script and reports one
/// subtest named `subtest`. When `gate` is given, its gate script is a
/// parser-blocking `<script src>` **before** the report script, so the parser
/// cannot reach the harness until the gate is answered; `None` is an ordinary
/// page with no gate in front of it.
fn harness_page(gate: Option<&str>, subtest: &str) -> String {
    let gate = gate.map_or_else(String::new, |gate| {
        format!("<script src=\"/gate/{gate}\"></script>")
    });
    format!(
        "<!doctype html><html><head><title>harness</title>{gate}\
         <script src=\"/resources/testharness.js\"></script>\
         <script src=\"/resources/testharnessreport.js\"></script></head><body>\
         <script>test({subtest:?}, function () {{ assert_equals(1 + 1, 2, 'sum'); }});</script>\
         </body></html>"
    )
}

/// Both gate paths: bit 0 is `/gate/a`, bit 1 is `/gate/b`.
const GATE_BOTH: usize = 0b11;

/// The meeting point the two gate scripts must reach together. One arrival
/// never opens it, however long it waits.
#[derive(Default)]
struct Gate {
    /// Bit `n` set once gate `n` has arrived.
    seen: AtomicUsize,
    /// Arrival count per gate path, for the tests' own assertions.
    arrivals: [AtomicUsize; 2],
    opened: Notify,
}

impl Gate {
    /// Records an arrival on gate `index` and waits until **both** gates have
    /// arrived. The re-check after registering the waiter is what makes a
    /// notification that fires early impossible to miss.
    async fn arrive(&self, index: usize) {
        self.arrivals[index].fetch_add(1, Ordering::SeqCst);
        self.seen.fetch_or(1 << index, Ordering::SeqCst);
        if self.seen.load(Ordering::SeqCst) & GATE_BOTH == GATE_BOTH {
            self.opened.notify_waiters();
            return;
        }
        loop {
            let notified = self.opened.notified();
            if self.seen.load(Ordering::SeqCst) & GATE_BOTH == GATE_BOTH {
                return;
            }
            notified.await;
        }
    }

    fn arrivals(&self, index: usize) -> usize {
        self.arrivals[index].load(Ordering::SeqCst)
    }
}

/// A bespoke HTTP/1.1 server, no new dependency: it serves a page, a harness
/// stand-in and two gate scripts, and holds each gate script until a request for
/// *the other* one has arrived.
///
/// It exists to make "`jobs: 2` really overlaps" categorical rather than
/// timing-based. Because the gate script is parser-blocking and comes before the
/// report script, a page cannot reach the harness — and the driver cannot see a
/// hook — until the gate opens; the gate opens only when both gate paths are in
/// flight at the same time, which one page at a time cannot do.
struct GateServer {
    addr: SocketAddr,
    gate: Arc<Gate>,
    task: tokio::task::JoinHandle<()>,
}

impl GateServer {
    async fn start() -> GateServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let gate = Arc::new(Gate::default());
        let accept_gate = Arc::clone(&gate);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let gate = Arc::clone(&accept_gate);
                tokio::spawn(serve_connection(stream, gate));
            }
        });
        GateServer { addr, gate, task }
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// How many times gate `index` (`0` = `/gate/a`, `1` = `/gate/b`) was
    /// requested. Counted on arrival, before the hold, so a connection torn down
    /// mid-gate cannot change it.
    fn gate_arrivals(&self, index: usize) -> usize {
        self.gate.arrivals(index)
    }
}

impl Drop for GateServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// `jobs: 2` is real overlap, shown by a meeting point rather than a clock.
///
/// Each page loads its gate script as a parser-blocking `<script src>` before
/// the report script, and the gate server answers a gate script only once a
/// request for *each* gate path has arrived. A single page, however long it
/// waits, cannot open its own gate — so a run in which both tests reach `OK`,
/// each reporting its own subtest, could not have been sequential. The distinct
/// subtest names are the attribution half: each page's hook reached its own
/// worker's slot, not a shared one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_jobs_overlap_at_a_gate_that_needs_both_pages() {
    let server = GateServer::start().await;

    let mut plan = plan_for(
        &server.base(),
        &[("/gate-a.html", 10_000), ("/gate-b.html", 10_000)],
    );
    plan.jobs = 2;

    let results = tenta_browser::wpt::run(plan)
        .await
        .expect("the run finishes");

    assert_eq!(results.tests.len(), 2);
    assert_eq!(
        results.tests[0].status, "OK",
        "the first page's gate opened: {:?}",
        results.tests[0].message
    );
    assert_eq!(
        results.tests[1].status, "OK",
        "the second page's gate opened: {:?}",
        results.tests[1].message
    );
    assert_eq!(results.tests[0].subtests.len(), 1);
    assert_eq!(results.tests[0].subtests[0].name, "gate a subtest");
    assert_eq!(results.tests[1].subtests.len(), 1);
    assert_eq!(results.tests[1].subtests[0].name, "gate b subtest");
    assert!(
        server.gate_arrivals(0) >= 1 && server.gate_arrivals(1) >= 1,
        "the server saw both gate requests (a: {}, b: {})",
        server.gate_arrivals(0),
        server.gate_arrivals(1)
    );
}

/// The gate does not open for one page, which is why the two-jobs test above is
/// evidence of overlap rather than of a permissive server.
///
/// At `jobs: 1` the first test reaches its gate, waits for a second arrival that
/// cannot come while the driver runs one test at a time, and times out. The
/// assertion is deliberately the single `TIMEOUT`: the driver tears the first
/// page's connection down mid-gate, and how the server's held response unwinds in
/// that case must not be able to make this flake. A `TIMEOUT` can only come from
/// a page that loaded and then stalled, and the arrival count below shows the
/// stall was at the gate, so neither half is vacuous.
///
/// The second test is deliberately an ordinary page rather than `/gate-b.html`.
/// Asking for the other gate path here would release the first page's held gate
/// script into the parse that the driver already cancelled. **That is a
/// pre-existing engine defect, still open:** a parse parked on a
/// parser-blocking script fetch resumes and runs the script after its navigation
/// is gone. The entry guard in `tenta-js-v8` does not touch it — the script now
/// runs on the right isolate instead of aborting the process, which leaves the
/// defect with no symptom at all rather than fixing it. It is not this slice's
/// to fix, and this test must not be written in a way that depends on either
/// behaviour. The evidence this test owes is that one page alone cannot open the
/// gate, and that is exactly what a never-answered gate script shows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_job_cannot_open_the_gate_and_times_out() {
    let server = GateServer::start().await;

    let plan = plan_for(
        &server.base(),
        &[("/gate-a.html", 600), ("/plain.html", 10_000)],
    );

    let results = tenta_browser::wpt::run(plan)
        .await
        .expect("the run finishes");

    assert_eq!(results.tests.len(), 2);
    assert_eq!(
        results.tests[0].status, "TIMEOUT",
        "a lone page's gate never opened: {:?}",
        results.tests[0].message
    );
    assert!(
        server.gate_arrivals(0) >= 1,
        "the first page did reach its gate, so it stalled there rather than \
         failing to load at all"
    );
}

/// A test started before the deadline drains; a test that no worker can claim
/// before it is never run.
///
/// Two workers claim the two stall pages while the deadline is still ahead and
/// run them to their own 1.5 s per-test timeout, past the ~1 s run deadline.
/// Only then does a worker claim the third test, by which time the deadline has
/// passed, so it is `NOTRUN` with no page created. This is the one assertion in
/// the slice with a real-time margin: the deadline is the mechanism under test,
/// so the margin is the point and not a dressed-up determinism.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tests_started_before_the_deadline_drain_and_the_next_is_not_run() {
    let harness = harness(HARNESS_BUDGET_MS);
    let (server, _temp) = serve(&[
        ("stall-a.html", STALL_PAGE),
        ("stall-b.html", STALL_PAGE),
        ("index.html", PAGE),
        ("resources/testharness.js", &harness),
    ])
    .await;

    let mut plan = plan(
        &server,
        &[
            ("/stall-a.html", 1_500),
            ("/stall-b.html", 1_500),
            ("/index.html", 10_000),
        ],
    );
    plan.jobs = 2;
    plan.deadline_epoch_ms = Some(now_epoch_ms() + 1_000);

    let results = tenta_browser::wpt::run(plan)
        .await
        .expect("the run finishes");

    assert_eq!(results.tests.len(), 3);
    assert_eq!(
        results.tests[0].status, "TIMEOUT",
        "started before the deadline, then drained"
    );
    assert_eq!(
        results.tests[1].status, "TIMEOUT",
        "started before the deadline, then drained"
    );
    assert_eq!(
        results.tests[2].status, "NOTRUN",
        "no worker was free before the deadline"
    );
    assert_eq!(
        results.tests[2].message.as_deref(),
        Some("run deadline reached")
    );
    assert_eq!(results.tests[2].duration_ms, 0);
    assert!(results.tests[2].subtests.is_empty());
}

/// Reads one request head and returns its request-target, or `None` if the peer
/// closed first or sent no well-formed request line.
async fn read_request_path(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            return None;
        }
    }
    let head = String::from_utf8_lossy(&buffer);
    head.lines()
        .next()?
        .split_whitespace()
        .nth(1)
        .map(str::to_owned)
}

/// Answers one connection and closes it (`Connection: close` is sent with every
/// response, so no connection serves two requests). The two gate paths are held
/// until both have arrived.
async fn serve_connection(mut stream: TcpStream, gate: Arc<Gate>) {
    let Some(path) = read_request_path(&mut stream).await else {
        return;
    };
    let (content_type, body): (&str, Vec<u8>) = match path.as_str() {
        "/gate/a" => {
            gate.arrive(0).await;
            ("text/javascript; charset=utf-8", b"/* gate a */".to_vec())
        }
        "/gate/b" => {
            gate.arrive(1).await;
            ("text/javascript; charset=utf-8", b"/* gate b */".to_vec())
        }
        "/resources/testharness.js" => (
            "text/javascript; charset=utf-8",
            harness(HARNESS_BUDGET_MS).into_bytes(),
        ),
        "/gate-a.html" => (
            "text/html; charset=utf-8",
            harness_page(Some("a"), "gate a subtest").into_bytes(),
        ),
        "/gate-b.html" => (
            "text/html; charset=utf-8",
            harness_page(Some("b"), "gate b subtest").into_bytes(),
        ),
        "/plain.html" => (
            "text/html; charset=utf-8",
            harness_page(None, "plain subtest").into_bytes(),
        ),
        _ => ("text/plain; charset=utf-8", b"not found\n".to_vec()),
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}
