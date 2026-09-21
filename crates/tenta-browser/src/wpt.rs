//! The `tenta-wpt-driver` transport: `wpt/run.py --product tentabrowser` as a
//! plan-in/JSON-out subprocess (`docs/spec/15-testing-and-benchmarks.md` §6).
//!
//! `wpt/run.py` writes a plan, runs `tenta-wpt-driver <plan.json>
//! <results.json>` and reads the file back. Both shapes are the JSON contract
//! documented on `Driver` in `wpt/run.py`; this module is the Rust half of it
//! and `src/bin/tenta-wpt-driver.rs` is the shim that reads and writes the two
//! files.
//!
//! # How one result is collected
//!
//! Chromium's driver routes `/resources/testharnessreport.js` to a hook that
//! hands the harness result back over an exposed binding. Nothing like that
//! binding exists here, and none is needed: the context's interceptor answers
//! that path **on the wire** with the hook script, and the hook posts the same
//! payload to a driver-owned path with `fetch()`. The interceptor reads the
//! request body before it fulfils, so the payload arrives with no page-visible
//! API involved.
//!
//! The POST body is a `Body::Bytes` and not a stream because that is the only
//! shape a page's `fetch()` can produce: `tenta-page`'s `net_request` builds the
//! transport request from `FetchRequest::body`, a `Vec<u8>`, through
//! `Request::with_body`.
//!
//! # Why a test's settle never self-reports, and how the navigation is ended
//!
//! `Settle::Load` is unusable: it tears the engine down at `load`, and the
//! harness completes strictly *after* it — `testharness.js`'s `load` listener
//! queues the `setTimeout(0)` that runs the completion callbacks.
//! `Settle::NetworkIdle` is not enough either: the hook's own `fetch()` is
//! issued from a timer callback, so it is classified as background work and does
//! not hold the network open the way a request from a script entry does.
//!
//! The driver therefore waits with `Settle::VisuallyStable` and a quiet window
//! larger than any test's budget, so the machine can never report itself
//! finished inside the budget and the settle loop keeps pumping until the hook
//! lands. The navigation is raced against the hook signal and the per-test
//! deadline; whichever wins, the loop leaves the scope and the navigation future
//! is **dropped**, which is the cancellation `GotoGuard` implements (it bumps
//! the page's navigation generation, so the page thread's navigation job
//! unwinds). The page is then closed, which drops its runtime on its own thread.
//!
//! The hook signal is delivered from the interceptor, i.e. from the moment the
//! request reaches the network layer — never from the response settling in
//! script — so a cancelled fetch cannot lose the result.
//!
//! # Resolver rules
//!
//! With no `/etc/hosts` entry the WPT hosts must be mapped to loopback from
//! inside the browser. `wpt/run.py` reports that as `resolver_rules`, and the
//! driver answers it by `Resolver::remember`-ing every WPT host it sees. It is
//! done **per request** rather than once at start-up because `remember` caps an
//! entry's life at the resolver's `max_ttl` (60 s for the system resolver), so a
//! single seed would expire mid-run and later requests would fall through to the
//! system resolver and fail. The interceptor runs before the DNS lookup of the
//! request it is looking at, so remembering there is always in time.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::StatusCode;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tenta_core::{Error, Tier};
use tenta_net::dns::Resolver;
use tenta_net::hooks::{BoxFuture, FulfilledResponse, Interception, Interceptor};
use tenta_net::request::Request;
use tenta_page::page::{Goto, Page};
use tenta_page::settle::Settle;
use tokio::sync::oneshot;

use crate::context::{BrowserContext, ContextOptions, PageOptions};
use crate::engine::Engine;

/// Harness statuses, indexed the way `testharness.js` numbers them.
const HARNESS_STATUS: [&str; 4] = ["OK", "ERROR", "TIMEOUT", "PRECONDITION_FAILED"];

/// Subtest statuses, indexed the way `testharness.js` numbers them.
const SUBTEST_STATUS: [&str; 5] = ["PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED"];

/// The path suffix whose response is replaced by the completion hook. A suffix
/// rather than an equality, because a test may reference the file through a
/// relative path that resolves below its own directory.
const REPORT_SUFFIX: &str = "/resources/testharnessreport.js";

/// The driver-owned path prefix the hook posts its payload to.
const RESULT_PREFIX: &str = "/_tenta/wpt-result/";

/// The `VisuallyStable` quiet window every test waits under.
///
/// Larger than any per-test budget, so the settle machine cannot report itself
/// finished before the hook arrives or the deadline fires. It is not a timeout:
/// the bounded wait is the per-test deadline below.
const SETTLE_QUIET: Duration = Duration::from_secs(3600);

/// How much longer than the per-test deadline the navigation's own deadline is,
/// so that the driver's deadline is the one that always fires and the navigation
/// never reports the timeout first.
const GOTO_SLACK: Duration = Duration::from_secs(5);

/// Life of a remembered WPT host answer. The resolver caps it at its own
/// `max_ttl`; the value only matters for runs that outlive it, and every request
/// refreshes it anyway.
const HOST_TTL: Duration = Duration::from_secs(60);

/// A run: the base URL to resolve test paths against and the tests to run.
#[derive(Clone, Debug, Deserialize)]
pub struct Plan {
    /// Origin every `tests[].url` is relative to, e.g.
    /// `http://web-platform.test:8000`.
    pub base: String,
    /// Whether the WPT hosts must be mapped to loopback in-process. `run.py`
    /// sets it when the machine's hosts file does not already do so.
    #[serde(default)]
    pub resolver_rules: bool,
    /// How many tests the runner would run at once. Read and reported, but S1
    /// runs them one at a time.
    #[serde(default)]
    pub jobs: usize,
    /// Epoch milliseconds after which no new test may start; `null` is no
    /// deadline.
    #[serde(default)]
    pub deadline_epoch_ms: Option<u64>,
    #[serde(default)]
    pub tests: Vec<PlanTest>,
}

/// One test to run.
#[derive(Clone, Debug, Deserialize)]
pub struct PlanTest {
    pub url: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    10_000
}

/// Everything the run produced.
#[derive(Clone, Debug, Serialize)]
pub struct Results {
    pub browser: BrowserInfo,
    pub tests: Vec<TestOutcome>,
}

/// The product identification `run.py` records in the report.
#[derive(Clone, Debug, Serialize)]
pub struct BrowserInfo {
    pub name: String,
    pub version: String,
}

/// One test's outcome.
#[derive(Clone, Debug, Serialize)]
pub struct TestOutcome {
    pub test: String,
    pub status: String,
    pub message: Option<String>,
    pub duration_ms: u64,
    pub subtests: Vec<SubtestOutcome>,
}

/// One subtest's outcome.
#[derive(Clone, Debug, Serialize)]
pub struct SubtestOutcome {
    pub name: String,
    pub status: String,
    pub message: Option<String>,
}

/// The payload the hook script posts, decoded from the request body.
#[derive(Debug, Deserialize)]
struct HookPayload {
    status: u32,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    tests: Vec<HookTest>,
}

#[derive(Debug, Deserialize)]
struct HookTest {
    name: String,
    status: u32,
    #[serde(default)]
    message: Option<String>,
}

/// The test whose hook the driver is waiting for.
struct Active {
    index: usize,
    sender: oneshot::Sender<HookPayload>,
}

/// What the interceptor and the per-test loop share.
struct Shared {
    /// The context's resolver, when the plan asked for resolver rules.
    resolver: Option<Arc<Resolver>>,
    active: Mutex<Option<Active>>,
}

impl Shared {
    fn begin(&self, index: usize, sender: oneshot::Sender<HookPayload>) {
        *self.active.lock() = Some(Active { index, sender });
    }

    /// Releases the slot, so a late hook of a closed page cannot be delivered
    /// to the next test.
    fn end(&self, index: usize) {
        let mut active = self.active.lock();
        if active
            .as_ref()
            .is_some_and(|current| current.index == index)
        {
            *active = None;
        }
    }

    /// The index the hook script must post to.
    fn active_index(&self) -> usize {
        self.active.lock().as_ref().map_or(0, |active| active.index)
    }

    /// Hands a decoded payload to the test that is waiting for it, if it is the
    /// current one.
    fn deliver(&self, index: usize, body: Option<Bytes>) {
        let Some(body) = body else {
            tracing::warn!(
                index,
                "a WPT result POST arrived with no buffered body; the test will time out"
            );
            return;
        };
        let payload = match serde_json::from_slice::<HookPayload>(&body) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!(index, %error, "a WPT result POST was not the hook's JSON");
                return;
            }
        };
        let mut active = self.active.lock();
        if !active
            .as_ref()
            .is_some_and(|current| current.index == index)
        {
            tracing::debug!(index, "a WPT result POST belonged to no current test");
            return;
        }
        if let Some(current) = active.take() {
            let _ = current.sender.send(payload);
        }
    }

    /// Maps a WPT host to loopback when the plan asked for resolver rules. It
    /// runs before the request's own DNS lookup, which interception precedes.
    fn remember_host(&self, host: Option<&str>) {
        let Some(resolver) = &self.resolver else {
            return;
        };
        let Some(host) = host else {
            return;
        };
        if is_wpt_host(host) {
            resolver.remember(host, &[IpAddr::V4(Ipv4Addr::LOCALHOST)], HOST_TTL);
        }
    }
}

/// Whether `host` is one of the WPT server's names or a subdomain of one
/// (`wpt/run.py`'s `HOSTS`, which the runner already resolved to loopback for
/// the `chromium` product).
fn is_wpt_host(host: &str) -> bool {
    const HOSTS: [&str; 2] = ["web-platform.test", "not-web-platform.test"];
    HOSTS.iter().any(|base| {
        host.eq_ignore_ascii_case(base)
            || host
                .strip_suffix(base)
                .is_some_and(|subdomain| subdomain.ends_with('.'))
    })
}

/// The completion hook served in place of `resources/testharnessreport.js`.
///
/// The shape is `driver-chromium.mjs`'s: `setup({ output: false })` and an
/// `add_completion_callback` that forwards the harness status, the message and
/// every subtest. The difference is the transport — a same-origin `fetch()` to
/// the driver's own path instead of an exposed binding — because the engine has
/// no binding to offer and the interceptor can answer a request instead.
fn hook_script(index: usize) -> String {
    format!(
        r#"setup({{ output: false }});
add_completion_callback(function (tests, harnessStatus) {{
  var payload = {{
    status: harnessStatus.status,
    message: harnessStatus.message,
    tests: tests.map(function (t) {{
      return {{ name: t.name, status: t.status, message: t.message }};
    }})
  }};
  fetch("{RESULT_PREFIX}{index}", {{
    method: "POST",
    headers: {{ "Content-Type": "application/json" }},
    body: JSON.stringify(payload)
  }});
}});
"#
    )
}

/// The context's one interceptor: the hook, the result path, and the resolver
/// rules. Everything else continues to the network.
struct WptInterceptor {
    shared: Arc<Shared>,
}

impl Interceptor for WptInterceptor {
    fn intercept<'a>(&'a self, request: &'a mut Request) -> BoxFuture<'a, Interception> {
        Box::pin(async move {
            let path = request.url.path();
            if path.ends_with(REPORT_SUFFIX) {
                return Interception::Fulfill(
                    FulfilledResponse::new(StatusCode::OK, hook_script(self.shared.active_index()))
                        .header("content-type", "text/javascript"),
                );
            }
            if let Some(index) = path.strip_prefix(RESULT_PREFIX) {
                let index = index.parse::<usize>();
                // The body is taken whatever the index parses to, so a
                // mis-addressed POST still gets an answer rather than a stream
                // the transport would try to read.
                let body = request.body.take_bytes();
                if let Ok(index) = index {
                    self.shared.deliver(index, body);
                }
                return Interception::Fulfill(FulfilledResponse::new(StatusCode::OK, Bytes::new()));
            }
            self.shared.remember_host(request.url.host_str());
            Interception::Continue
        })
    }
}

/// Runs a plan: one engine, one context, one test after another.
///
/// The engine is created here rather than taken as a parameter so the binary
/// owns nothing but the two file paths, and so a test can drive the whole loop.
pub async fn run(plan: Plan) -> Result<Results, Error> {
    if plan.jobs > 1 {
        tracing::warn!(
            jobs = plan.jobs,
            "S1 runs every test sequentially; the plan's `jobs` is ignored"
        );
    }
    let engine = Engine::builder()
        .io_threads(1)
        .page_threads(1)
        .max_tier(Tier::Js)
        .build()?;
    let context = engine.new_context(ContextOptions {
        // The WPT server is on loopback (`wpt/serve-config.json`) and its TLS
        // certificates are ones no machine trusts.
        allow_private_network: true,
        ignore_https_errors: true,
        ..ContextOptions::default()
    })?;
    let shared = Arc::new(Shared {
        resolver: plan
            .resolver_rules
            .then(|| Arc::clone(context.client().resolver())),
        active: Mutex::new(None),
    });
    context.intercept(Arc::new(WptInterceptor {
        shared: Arc::clone(&shared),
    }))?;

    let total = plan.tests.len();
    let mut tests = Vec::with_capacity(total);
    for (index, test) in plan.tests.iter().enumerate() {
        let outcome = run_one(&context, &shared, &plan, index, test).await;
        tracing::info!(
            position = index + 1,
            total,
            status = %outcome.status,
            passed = outcome.subtests.iter().filter(|s| s.status == "PASS").count(),
            subtests = outcome.subtests.len(),
            test = %outcome.test,
            "ran a WPT test"
        );
        tests.push(outcome);
    }
    engine.close();
    Ok(Results {
        browser: BrowserInfo {
            name: "tentabrowser".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        tests,
    })
}

/// One test: a fresh page, a navigation raced against the hook and the
/// deadline, and a close that cancels whichever of the three lost.
async fn run_one(
    context: &BrowserContext,
    shared: &Arc<Shared>,
    plan: &Plan,
    index: usize,
    test: &PlanTest,
) -> TestOutcome {
    let started = Instant::now();
    // `TIMEOUT` is the default because it is what every way of not learning an
    // outcome ends in; `NOTRUN` is set below, on the one path that starts no
    // work at all.
    let mut outcome = TestOutcome {
        test: test.url.clone(),
        status: "TIMEOUT".to_owned(),
        message: None,
        duration_ms: 0,
        subtests: Vec::new(),
    };
    if deadline_reached(plan) {
        outcome.status = "NOTRUN".to_owned();
        outcome.message = Some("run deadline reached".to_owned());
        outcome.duration_ms = millis(started.elapsed());
        return outcome;
    }

    let url = format!("{}{}", plan.base.trim_end_matches('/'), test.url);
    let (sender, receiver) = oneshot::channel();
    shared.begin(index, sender);
    let page = match context.new_page(PageOptions::default()).await {
        Ok(page) => page,
        Err(error) => {
            shared.end(index);
            return error_outcome(test.url.clone(), "ERROR", error.to_string(), started);
        }
    };

    let wait = Duration::from_millis(test.timeout_ms.max(1));
    // The navigation is pinned so it stays alive across the race and is dropped
    // at its end — dropping it is what cancels a navigation the hook has already
    // answered.
    let raced = {
        let navigation = page.goto(
            &url,
            Goto::default()
                .wait_until(Settle::VisuallyStable {
                    quiet: SETTLE_QUIET,
                })
                .timeout(wait + GOTO_SLACK),
        );
        tokio::pin!(navigation);
        tokio::select! {
            biased;
            hook = receiver => Race::Hook(hook.ok()),
            result = &mut navigation => Race::Navigation(result.into()),
            _ = tokio::time::sleep(wait) => Race::Deadline,
        }
    };
    shared.end(index);
    finish_page(&page).await;
    outcome.duration_ms = millis(started.elapsed());

    match raced {
        Race::Hook(Some(payload)) => {
            outcome.status = harness_status(payload.status).to_owned();
            outcome.message = payload.message;
            outcome.subtests = payload
                .tests
                .into_iter()
                .map(|subtest| SubtestOutcome {
                    name: subtest.name,
                    status: subtest_status(subtest.status).to_owned(),
                    message: subtest.message,
                })
                .collect();
        }
        // The sender cannot be dropped while the test is current; a `None` here
        // is treated exactly as a hook that never came.
        Race::Hook(None) | Race::Deadline => {
            outcome.status = "TIMEOUT".to_owned();
            outcome.message = Some(format!(
                "the completion hook did not arrive within {} ms",
                test.timeout_ms
            ));
        }
        // `VisuallyStable` with a quiet window past the budget cannot settle on
        // its own, so a navigation that returned without a hook is a page that
        // finished without ever completing its harness.
        Race::Navigation(NavigationEnd::Settled) => {
            outcome.status = "TIMEOUT".to_owned();
            outcome.message = Some("the navigation settled before the hook arrived".to_owned());
        }
        Race::Navigation(NavigationEnd::Crashed(reason)) => {
            outcome.status = "CRASH".to_owned();
            outcome.message = Some(reason);
        }
        Race::Navigation(NavigationEnd::Failed(error)) => {
            outcome.status = "ERROR".to_owned();
            outcome.message = Some(error);
        }
    }
    outcome
}

/// What ended the per-test wait.
enum Race {
    Hook(Option<HookPayload>),
    Navigation(NavigationEnd),
    Deadline,
}

/// How a navigation that ended on its own ended. The navigation's own value is
/// large and is not needed past this point, so the race's type carries only what
/// the arms below read.
enum NavigationEnd {
    /// The navigation completed without the hook ever arriving.
    Settled,
    Crashed(String),
    Failed(String),
}

impl From<Result<tenta_page::page::Navigation, Error>> for NavigationEnd {
    fn from(result: Result<tenta_page::page::Navigation, Error>) -> Self {
        match result {
            Ok(_) => NavigationEnd::Settled,
            Err(Error::Crashed { reason }) => NavigationEnd::Crashed(reason),
            Err(error) => NavigationEnd::Failed(error.to_string()),
        }
    }
}

/// Closes a page, ignoring a close that fails because the page is already gone.
async fn finish_page(page: &Page) {
    if let Err(error) = page.close().await {
        tracing::warn!(%error, "closing a WPT test page failed");
    }
}

fn error_outcome(test: String, status: &str, message: String, started: Instant) -> TestOutcome {
    TestOutcome {
        test,
        status: status.to_owned(),
        message: Some(message),
        duration_ms: millis(started.elapsed()),
        subtests: Vec::new(),
    }
}

fn harness_status(index: u32) -> &'static str {
    HARNESS_STATUS
        .get(index as usize)
        .copied()
        .unwrap_or("ERROR")
}

fn subtest_status(index: u32) -> &'static str {
    SUBTEST_STATUS
        .get(index as usize)
        .copied()
        .unwrap_or("FAIL")
}

fn deadline_reached(plan: &Plan) -> bool {
    plan.deadline_epoch_ms
        .is_some_and(|deadline| now_epoch_ms() >= deadline)
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn millis(elapsed: Duration) -> u64 {
    elapsed.as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_wpt_hosts_and_their_subdomains_are_mapped() {
        assert!(is_wpt_host("web-platform.test"));
        assert!(is_wpt_host("WEB-PLATFORM.TEST"));
        assert!(is_wpt_host("www.web-platform.test"));
        assert!(is_wpt_host("xn--n8j6ds53lwwkrqhv28a.web-platform.test"));
        assert!(is_wpt_host("not-web-platform.test"));
        assert!(is_wpt_host("www.not-web-platform.test"));
    }

    #[test]
    fn a_lookalike_host_is_not_mapped() {
        // `not-web-platform.test` ends with `web-platform.test` as a byte
        // suffix, which is exactly the trap a plain `ends_with` would fall into.
        assert!(!is_wpt_host("not-web-platform.test.evil"));
        assert!(!is_wpt_host("web-platform.test.evil"));
        assert!(!is_wpt_host("wwwweb-platform.test"));
        assert!(!is_wpt_host("example.test"));
        assert!(!is_wpt_host(""));
    }

    #[test]
    fn the_harness_and_subtest_statuses_are_mapped_by_index() {
        assert_eq!(harness_status(0), "OK");
        assert_eq!(harness_status(1), "ERROR");
        assert_eq!(harness_status(2), "TIMEOUT");
        assert_eq!(harness_status(3), "PRECONDITION_FAILED");
        assert_eq!(harness_status(4), "ERROR", "an unknown status is an error");
        assert_eq!(subtest_status(0), "PASS");
        assert_eq!(subtest_status(1), "FAIL");
        assert_eq!(subtest_status(2), "TIMEOUT");
        assert_eq!(subtest_status(3), "NOTRUN");
        assert_eq!(subtest_status(4), "PRECONDITION_FAILED");
        assert_eq!(subtest_status(5), "FAIL", "an unknown status is a failure");
    }

    #[test]
    fn the_hook_script_posts_to_the_current_tests_path() {
        let script = hook_script(7);
        assert!(script.contains("\"/_tenta/wpt-result/7\""));
        assert!(
            script.contains("setup({ output: false })"),
            "the harness must not try to write a results table: {script}"
        );
        assert!(script.contains("add_completion_callback"));
    }

    /// The plan `wpt/run.py` writes, field for field.
    #[test]
    fn a_plan_from_run_py_deserializes() {
        let json = r#"{"base":"http://web-platform.test:8000","resolver_rules":true,
            "jobs":4,"deadline_epoch_ms":null,
            "tests":[{"url":"/dom/nodes/x.html","timeout_ms":10000}]}"#;
        let plan: Plan = serde_json::from_str(json).expect("the plan parses");
        assert_eq!(plan.base, "http://web-platform.test:8000");
        assert!(plan.resolver_rules);
        assert_eq!(plan.jobs, 4);
        assert_eq!(plan.deadline_epoch_ms, None);
        assert_eq!(plan.tests[0].url, "/dom/nodes/x.html");
        assert_eq!(plan.tests[0].timeout_ms, 10_000);
    }

    /// The payload `testharness.js` produces: numeric statuses, and a `message`
    /// that is present on a failure and omitted when there is none.
    #[test]
    fn the_hooks_payload_decodes_and_maps_by_index() {
        let json = br#"{"status":0,"message":null,
            "tests":[{"name":"a","status":0,"message":null},
                     {"name":"b","status":1,"message":"boom"}]}"#;
        let payload: HookPayload = serde_json::from_slice(json).expect("the payload parses");
        assert_eq!(harness_status(payload.status), "OK");
        assert_eq!(payload.message, None);
        assert_eq!(subtest_status(payload.tests[0].status), "PASS");
        assert_eq!(subtest_status(payload.tests[1].status), "FAIL");
        assert_eq!(payload.tests[1].message.as_deref(), Some("boom"));
    }

    /// The result JSON's field names are the contract `run.py` reads; a rename
    /// would surface as a `KeyError` in the runner rather than here.
    #[test]
    fn the_result_json_uses_the_field_names_run_py_reads() {
        let results = Results {
            browser: BrowserInfo {
                name: "tentabrowser".to_owned(),
                version: "0.0.1".to_owned(),
            },
            tests: vec![TestOutcome {
                test: "/index.html".to_owned(),
                status: "OK".to_owned(),
                message: None,
                duration_ms: 12,
                subtests: vec![SubtestOutcome {
                    name: "a".to_owned(),
                    status: "PASS".to_owned(),
                    message: None,
                }],
            }],
        };
        let value = serde_json::to_value(&results).expect("serializes");
        assert_eq!(value["browser"]["name"], "tentabrowser");
        assert_eq!(value["browser"]["version"], "0.0.1");
        assert_eq!(value["tests"][0]["test"], "/index.html");
        assert_eq!(value["tests"][0]["status"], "OK");
        assert_eq!(value["tests"][0]["duration_ms"], 12);
        assert!(value["tests"][0]["message"].is_null());
        assert_eq!(value["tests"][0]["subtests"][0]["name"], "a");
        assert_eq!(value["tests"][0]["subtests"][0]["status"], "PASS");
    }
}
