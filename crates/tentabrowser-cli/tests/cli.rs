//! The `tentabrowser` binary driven the way a shell drives it: a real
//! process, real fixtures over loopback, exit codes and bytes on the streams.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir, fixtures_root, temp_dir};

const BIN: &str = env!("CARGO_BIN_EXE_tentabrowser");

/// The recorded SERPs of the provider survey (`providers/fixtures/`).
fn provider_fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("providers")
        .join("fixtures")
}

async fn server() -> FixtureServer {
    FixtureServer::start(FixtureServerConfig::new(fixtures_root()))
        .await
        .expect("fixture server")
}

/// Runs the binary with a home directory of its own, so a test never reads or
/// writes the developer's session cache.
struct Cli {
    home: TempDir,
}

impl Cli {
    fn new(prefix: &str) -> Self {
        Self {
            home: temp_dir(prefix),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path())
            .env("LOCALAPPDATA", self.home.path())
            .output()
            .expect("the binary runs")
    }

    /// Where a persistent context's session cache lands under this home.
    fn session_dir(&self) -> PathBuf {
        if cfg!(target_os = "windows") {
            self.home.path().join("tentabrowser").join("session")
        } else if cfg!(any(target_os = "macos", target_os = "ios")) {
            self.home
                .path()
                .join("Library")
                .join("Caches")
                .join("tentabrowser")
                .join("session")
        } else {
            self.home.path().join("tentabrowser").join("session")
        }
    }
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("the process exited normally")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn files_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn eval_rejects_conflicting_output_flags_before_network_access() {
    let cli = Cli::new("cli-eval-flags");
    let output = cli.run(&[
        "fetch",
        "http://127.0.0.1:1/",
        "--eval",
        "document.title",
        "--dump",
        "text",
    ]);
    assert_eq!(code(&output), 2);
    assert!(
        stderr(&output).contains("omit --dump"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).is_empty());
}

#[cfg(not(feature = "v8"))]
#[test]
fn eval_without_v8_exits_unsupported_before_network_access() {
    let cli = Cli::new("cli-eval-disabled");
    let output = cli.run(&["fetch", "http://127.0.0.1:1/", "--eval", "1 + 2"]);
    assert_eq!(code(&output), 5);
    assert!(
        stderr(&output).contains("--features v8"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).is_empty());
}

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eval_runs_in_the_settled_page_and_serializes_json_values() {
    let server = server().await;
    let cli = Cli::new("cli-eval-v8");
    let url = server.url("/js-xhr/index.html");
    let output = cli.run(&[
        "fetch",
        &url,
        "--wait",
        "networkidle",
        "--eval",
        "({ title: document.title, settled: window.__settled, status: document.getElementById('status').textContent })",
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let value: Value = serde_json::from_str(stdout(&output).trim()).expect("JSON value");
    assert_eq!(value["settled"], true);
    assert_eq!(value["status"], "js-xhr: done (2 requests)");
    assert_eq!(value["title"], "JS + XHR + fetch: done");
}

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eval_returns_string_text_and_reports_script_errors() {
    let server = server().await;
    let cli = Cli::new("cli-eval-result");
    let url = server.url("/tiny/index.html");
    let title = cli.run(&[
        "fetch",
        &url,
        "--eval",
        "document.title",
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&title), 0, "{}", stderr(&title));
    assert_eq!(stdout(&title), "Tiny\n");

    let thrown = cli.run(&[
        "fetch",
        &url,
        "--eval",
        "(() => { throw new Error('eval failed'); })()",
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&thrown), 1, "{}", stderr(&thrown));
    assert!(
        stderr(&thrown).contains("eval failed"),
        "{}",
        stderr(&thrown)
    );
    assert!(stdout(&thrown).is_empty());

    let promise = cli.run(&[
        "fetch",
        &url,
        "--eval",
        "Promise.resolve(7)",
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&promise), 1, "{}", stderr(&promise));
    assert!(stderr(&promise).contains("not awaited"));
    assert!(stdout(&promise).is_empty());
}

// --------------------------------------------------------------------- fetch

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_dump_kind_produces_its_shape() {
    let server = server().await;
    let cli = Cli::new("cli-dumps");
    let url = server.url("/static-article/index.html");

    let text = cli.run(&["fetch", &url, "--allow-private-network"]);
    assert_eq!(code(&text), 0, "{}", stderr(&text));
    assert!(
        stdout(&text).contains("END-OF-ARTICLE-7F3A"),
        "the default dump is the article's text"
    );

    let html = cli.run(&["fetch", &url, "--dump", "html", "--allow-private-network"]);
    assert!(
        stdout(&html).starts_with("<html"),
        "{}",
        &stdout(&html)[..40]
    );

    let markdown = cli.run(&[
        "fetch",
        &url,
        "--dump",
        "markdown",
        "--allow-private-network",
    ]);
    assert!(stdout(&markdown).contains("# "), "markdown keeps headings");

    let links = cli.run(&["fetch", &url, "--dump", "links", "--allow-private-network"]);
    let first: Value =
        serde_json::from_str(stdout(&links).lines().next().expect("a link")).expect("json line");
    assert!(
        first["url"].as_str().expect("url").starts_with("http"),
        "links are absolute: {first}"
    );

    let json = cli.run(&["fetch", &url, "--dump", "json", "--allow-private-network"]);
    let document: Value = serde_json::from_str(&stdout(&json)).expect("json object");
    assert_eq!(document["status"], 200);
    assert_eq!(document["ok"], true);
    assert!(document["text"].as_str().expect("text").len() > 500_000);
    assert!(document["timing_ms"]["total"].as_f64().expect("total") >= 0.0);

    let metadata = cli.run(&[
        "fetch",
        &url,
        "--dump",
        "metadata",
        "--allow-private-network",
    ]);
    let metadata: Value = serde_json::from_str(&stdout(&metadata)).expect("json object");
    assert!(metadata["title"].as_str().is_some_and(|t| !t.is_empty()));

    let snapshot = cli.run(&[
        "fetch",
        &url,
        "--dump",
        "snapshot",
        "--allow-private-network",
    ]);
    let snapshot: Value = serde_json::from_str(&stdout(&snapshot)).expect("json object");
    assert!(snapshot.is_object(), "the snapshot is a tree of nodes");

    let forms = cli.run(&["fetch", &url, "--dump", "forms", "--allow-private-network"]);
    let forms: Value = serde_json::from_str(&stdout(&forms)).expect("json array");
    assert!(forms.is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tiny_fixture_dumps_exactly_its_text() {
    let server = server().await;
    let cli = Cli::new("cli-tiny");
    let output = cli.run(&[
        "fetch",
        &server.url("/tiny/index.html"),
        "--dump",
        "text",
        "--allow-private-network",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "Tiny fixture\nOne paragraph, no scripts, no subresources.\n"
    );
    assert!(
        stderr(&output).is_empty(),
        "stderr stays quiet at --log warn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn main_content_headers_and_wait_change_what_is_reported() {
    let server = server().await;
    let cli = Cli::new("cli-flags");
    let url = server.url("/static-article/index.html");

    let whole = cli.run(&["fetch", &url, "--allow-private-network"]);
    let main = cli.run(&["fetch", &url, "--main-content", "--allow-private-network"]);
    assert!(
        stdout(&main).len() < stdout(&whole).len(),
        "the article body is smaller than the page"
    );
    assert!(stdout(&main).contains("END-OF-ARTICLE-7F3A"));

    let with_headers = cli.run(&["fetch", &url, "--headers", "--allow-private-network"]);
    assert!(
        stderr(&with_headers).starts_with("200 http://"),
        "headers go to stderr: {}",
        stderr(&with_headers)
    );
    assert!(stderr(&with_headers).contains("content-type: text/html"));
    assert_eq!(
        stdout(&with_headers),
        stdout(&whole),
        "stdout carries the extraction and nothing else"
    );

    for wait in ["load", "domcontent", "networkidle"] {
        let output = cli.run(&["fetch", &url, "--wait", wait, "--allow-private-network"]);
        assert_eq!(code(&output), 0, "--wait {wait}: {}", stderr(&output));
        assert!(
            stdout(&output).contains("END-OF-ARTICLE-7F3A"),
            "--wait {wait}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_global_json_flag_is_the_json_dump() {
    let server = server().await;
    let cli = Cli::new("cli-json");
    let url = server.url("/tiny/index.html");

    let shorthand = cli.run(&["fetch", &url, "--json", "--allow-private-network"]);
    assert_eq!(code(&shorthand), 0, "{}", stderr(&shorthand));
    let document: Value = serde_json::from_str(&stdout(&shorthand)).expect("json object");
    assert_eq!(document["status"], 200);
    assert_eq!(document["title"], "Tiny");

    // The two forms differ only in the timings they measured.
    let explicit: Value = serde_json::from_str(&stdout(&cli.run(&[
        "fetch",
        &url,
        "--dump",
        "json",
        "--allow-private-network",
    ])))
    .expect("json object");
    for (a, b) in [(&document, &explicit), (&explicit, &document)] {
        for (key, value) in a.as_object().expect("object") {
            if key == "timing_ms" || key == "headers" {
                continue;
            }
            assert_eq!(b[key], *value, "--json and --dump json differ on {key}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn debug_logging_reports_where_a_cold_run_spent_its_time() {
    let server = server().await;
    let cli = Cli::new("cli-phases");
    let output = cli.run(&[
        "fetch",
        &server.url("/tiny/index.html"),
        "--log",
        "debug",
        "--allow-private-network",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let logs = stderr(&output);
    let phases = logs
        .lines()
        .find(|line| line.contains("phases"))
        .unwrap_or_else(|| panic!("no phases line in:\n{logs}"));
    for stage in [
        "engine=",
        "navigate=",
        "extract=",
        "write=",
        "close=",
        "total_ms",
    ] {
        assert!(phases.contains(stage), "{stage} missing from {phases}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_output_file_needs_allow_write() {
    let server = server().await;
    let cli = Cli::new("cli-output");
    let out = temp_dir("cli-out");
    let target = out.path().join("page.txt");
    let url = server.url("/tiny/index.html");

    let denied = cli.run(&[
        "fetch",
        &url,
        "-o",
        target.to_str().expect("utf-8"),
        "--allow-private-network",
    ]);
    assert_eq!(code(&denied), 2, "{}", stderr(&denied));
    assert!(
        stderr(&denied).contains("denied by policy"),
        "{}",
        stderr(&denied)
    );
    assert!(stderr(&denied).contains("--allow-write"));
    assert!(
        !target.exists(),
        "nothing is written when the path is refused"
    );

    let allowed = cli.run(&[
        "fetch",
        &url,
        "-o",
        target.to_str().expect("utf-8"),
        "--allow-write",
        out.path().to_str().expect("utf-8"),
        "--allow-private-network",
    ]);
    assert_eq!(code(&allowed), 0, "{}", stderr(&allowed));
    assert!(stdout(&allowed).is_empty(), "the dump went to the file");
    let written = std::fs::read_to_string(&target).expect("the file exists");
    assert!(written.contains("Tiny fixture"), "{written}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exit_codes_separate_transport_http_and_usage_failures() {
    let server = server().await;
    let cli = Cli::new("cli-exits");

    let missing = cli.run(&[
        "fetch",
        &server.url("/no/such/page.html"),
        "--allow-private-network",
    ]);
    assert_eq!(code(&missing), 3, "{}", stderr(&missing));
    assert!(stderr(&missing).contains("HTTP 404"));

    let accepted = cli.run(&[
        "fetch",
        &server.url("/no/such/page.html"),
        "--allow-http-errors",
        "--allow-private-network",
    ]);
    assert_eq!(code(&accepted), 0, "{}", stderr(&accepted));
    assert!(stdout(&accepted).contains("not found"));

    // A port whose server has just gone away: the OS refuses the connection.
    let port = server.port();
    server.shutdown();
    let refused = cli.run(&[
        "fetch",
        &format!("http://127.0.0.1:{port}/tiny/index.html"),
        "--allow-private-network",
    ]);
    assert_eq!(code(&refused), 1, "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("net::ERR_CONNECTION_REFUSED"),
        "{}",
        stderr(&refused)
    );

    // A string that is not a URL never reaches the network, so it is a
    // usage failure, not a transport one.
    let nonsense = cli.run(&["fetch", "not a url"]);
    assert_eq!(code(&nonsense), 2, "{}", stderr(&nonsense));
    assert!(
        stderr(&nonsense).contains("invalid URL"),
        "{}",
        stderr(&nonsense)
    );

    let unknown_flag = cli.run(&["fetch", "http://example.test/", "--no-such-flag"]);
    assert_eq!(code(&unknown_flag), 2, "{}", stderr(&unknown_flag));

    let usage = cli.run(&["--help"]);
    assert_eq!(code(&usage), 0);
    assert!(stdout(&usage).contains("fetch"));
    assert!(stdout(&usage).contains("scrape"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_private_destination_is_refused_until_the_flag_allows_it() {
    let server = server().await;
    let cli = Cli::new("cli-private");
    let url = server.url("/tiny/index.html");

    let refused = cli.run(&["fetch", &url]);
    assert_eq!(code(&refused), 1, "{}", stderr(&refused));
    let message = stderr(&refused);
    assert!(message.contains("net::ERR_ACCESS_DENIED"), "{message}");
    assert!(message.contains("private-network destination"), "{message}");
    assert!(message.contains("--allow-private-network"), "{message}");

    let allowed = cli.run(&["fetch", &url, "--allow-private-network"]);
    assert_eq!(code(&allowed), 0, "{}", stderr(&allowed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persistent_run_keeps_a_session_dir_and_ephemeral_does_not() {
    let server = server().await;
    // `localhost` is loopback without a resolver (as in Chrome), so a
    // loopback fetch records no DNS entry and writes no session file; what
    // a persistent run must leave behind is its private session directory
    // with the advisory lock. Session-file contents are covered by the
    // `tenta-net` session cache and resolver tests.
    let url = format!("http://localhost:{}/tiny/index.html", server.port());

    let persistent = Cli::new("cli-session");
    let output = persistent.run(&["fetch", &url, "--allow-private-network"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        persistent.session_dir().is_dir(),
        "a persistent run creates its session directory"
    );
    let entries = files_in(&persistent.session_dir());
    assert!(
        entries.iter().all(|name| name.ends_with(".lock")),
        "nothing was recorded, so only the advisory lock may exist: {entries:?}"
    );

    let ephemeral = Cli::new("cli-ephemeral");
    let output = ephemeral.run(&["fetch", &url, "--ephemeral", "--allow-private-network"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        !ephemeral.session_dir().exists(),
        "--ephemeral writes nothing: {:?}",
        files_in(&ephemeral.session_dir())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clean_cache_is_not_rewritten() {
    let server = server().await;
    // IP-literal URL produces no DNS entry and does not dirty the cache
    let url = server.url("/tiny/index.html");

    let persistent = Cli::new("cli-clean-cache");
    let output = persistent.run(&["fetch", &url, "--allow-private-network"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    // The advisory lock file is the only thing a run may leave behind; the
    // session file itself is not written when nothing was recorded.
    let entries: Vec<String> = files_in(&persistent.session_dir())
        .into_iter()
        .filter(|name| !name.ends_with(".lock"))
        .collect();
    assert!(
        entries.is_empty(),
        "an IP-literal fetch records no DNS entry and must not write the session file: {entries:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn debug_logs_redact_the_query_string_unless_asked_not_to() {
    let server = server().await;
    let cli = Cli::new("cli-redact");
    let url = server.url("/tiny/index.html?token=secret");

    let redacted = cli.run(&["fetch", &url, "--log", "debug", "--allow-private-network"]);
    assert_eq!(code(&redacted), 0, "{}", stderr(&redacted));
    assert!(
        !stderr(&redacted).contains("secret"),
        "{}",
        stderr(&redacted)
    );
    assert!(
        stderr(&redacted).contains("<redacted>"),
        "{}",
        stderr(&redacted)
    );

    let raw = cli.run(&[
        "fetch",
        &url,
        "--log",
        "debug",
        "--log-unredacted",
        "--allow-private-network",
    ]);
    assert!(stderr(&raw).contains("token=secret"), "{}", stderr(&raw));
}

// -------------------------------------------------------------------- search

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_reads_a_recorded_serp_through_an_endpoint_override() {
    let serps = FixtureServer::start(FixtureServerConfig::new(provider_fixtures()))
        .await
        .expect("provider fixture server");
    let cli = Cli::new("cli-search");
    let endpoint = format!(
        "duckduckgo_html={}",
        serps.url("/duckduckgo_html/rust-browser-engine.html")
    );

    let json = cli.run(&[
        "search",
        "rust browser engine",
        "--provider",
        "ddg",
        "--limit",
        "3",
        "--json",
        "--search-endpoint",
        &endpoint,
        "--allow-private-network",
    ]);
    assert_eq!(code(&json), 0, "{}", stderr(&json));
    let printed = stdout(&json);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines.len(), 4, "three results and one meta line: {lines:?}");

    let first: Value = serde_json::from_str(lines[0]).expect("result line");
    assert_eq!(first["rank"], 1);
    assert_eq!(first["source"], "duckduckgo");
    assert!(first["url"].as_str().expect("url").starts_with("https://"));
    assert!(!first["title"].as_str().expect("title").is_empty());

    let meta: Value = serde_json::from_str(lines[3]).expect("meta line");
    assert_eq!(meta["meta"]["results"], 3);
    assert_eq!(meta["meta"]["providers"][0]["provider"], "duckduckgo");
    assert_eq!(meta["meta"]["providers"][0]["mode_used"], "fast");
    assert!(
        meta["meta"]["timing"]["engine_overhead_ms"]
            .as_f64()
            .expect("engine overhead")
            >= 0.0
    );

    // The table form carries the same results in a form a person reads.
    let table = cli.run(&[
        "search",
        "rust browser engine",
        "--provider",
        "ddg",
        "--limit",
        "3",
        "--search-endpoint",
        &endpoint,
        "--allow-private-network",
    ]);
    assert_eq!(code(&table), 0, "{}", stderr(&table));
    let rendered = stdout(&table);
    assert!(rendered.contains(" 1. "), "{rendered}");
    assert!(
        rendered.contains(first["url"].as_str().expect("url")),
        "{rendered}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_refuses_google_with_the_reason_and_the_phase() {
    let cli = Cli::new("cli-google");
    let output = cli.run(&["search", "anything", "--provider", "google"]);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains("JavaScript"), "{message}");
    assert!(message.contains("P2"), "{message}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_search_that_cannot_parse_the_page_reports_a_block() {
    let server = server().await;
    let cli = Cli::new("cli-blocked");
    // The tiny fixture is a valid page with none of DuckDuckGo's markup: the
    // plan matches nothing, which is the `layout` block class.
    let endpoint = format!("duckduckgo_html={}", server.url("/tiny/index.html"));
    let output = cli.run(&[
        "search",
        "anything",
        "--search-endpoint",
        &endpoint,
        "--allow-private-network",
    ]);
    assert_eq!(code(&output), 4, "{}", stderr(&output));
    assert!(stderr(&output).contains("layout"), "{}", stderr(&output));
}

// -------------------------------------------------------------------- scrape

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scrape_honours_robots_and_prints_one_json_line_per_page() {
    let server = server().await;
    let cli = Cli::new("cli-scrape");
    let allowed_index = server.url("/robots/index.html");
    let allowed = server.url("/robots/allowed.html");
    let disallowed = server.url("/robots/private/secret.html");

    let output = cli.run(&[
        "scrape",
        &allowed_index,
        &allowed,
        &disallowed,
        "--ordered",
        "--allow-private-network",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let lines: Vec<Value> = stdout(&output)
        .lines()
        .map(|line| serde_json::from_str(line).expect("json line"))
        .collect();
    assert_eq!(lines.len(), 3);

    assert_eq!(lines[0]["url"], allowed_index.as_str());
    assert_eq!(lines[0]["status"], 200);
    assert_eq!(lines[0]["title"], "Robots fixture");
    assert!(
        lines[0]["text"]
            .as_str()
            .expect("text")
            .contains("ROBOTS-INDEX-OK")
    );
    assert!(lines[0]["timing_ms"].as_f64().expect("timing") >= 0.0);
    assert_eq!(lines[0]["final_url"], allowed_index.as_str());

    assert_eq!(lines[1]["url"], allowed.as_str());
    assert!(
        lines[1]["text"]
            .as_str()
            .expect("text")
            .contains("ROBOTS-ALLOWED-OK")
    );

    assert_eq!(lines[2]["url"], disallowed.as_str());
    assert_eq!(lines[2]["error"], "robots");
    assert!(
        lines[2]["status"].is_null(),
        "a refused URL is never fetched"
    );
    assert!(
        !stdout(&output).contains("ROBOTS-SECRET-LEAKED"),
        "the disallowed page must not be fetched"
    );

    let ignoring = cli.run(&[
        "scrape",
        &disallowed,
        "--ignore-robots",
        "--allow-private-network",
    ]);
    assert_eq!(code(&ignoring), 0, "{}", stderr(&ignoring));
    assert!(stdout(&ignoring).contains("ROBOTS-SECRET-LEAKED"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scrape_follows_links_within_the_host_and_stops_at_max_pages() {
    let server = server().await;
    let cli = Cli::new("cli-follow");

    let output = cli.run(&[
        "scrape",
        &server.url("/robots/index.html"),
        "--follow-links",
        "--same-host",
        "--depth",
        "1",
        "--dump",
        "links",
        "--ordered",
        "--allow-private-network",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let urls: Vec<String> = stdout(&output)
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).expect("json line")["url"]
                .as_str()
                .expect("url")
                .to_owned()
        })
        .collect();
    assert!(
        urls.contains(&server.url("/robots/allowed.html")),
        "{urls:?}"
    );
    assert!(
        urls.iter().any(|url| url.ends_with("/private/secret.html")),
        "the disallowed link is still reported, as a robots line: {urls:?}"
    );

    let capped = cli.run(&[
        "scrape",
        &server.url("/robots/index.html"),
        &server.url("/robots/allowed.html"),
        "--max-pages",
        "1",
        "--allow-private-network",
    ]);
    assert_eq!(code(&capped), 0, "{}", stderr(&capped));
    assert_eq!(stdout(&capped).lines().count(), 1, "--max-pages 1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scrape_waits_out_a_retry_after_before_giving_up_on_a_host() {
    let server = server().await;
    let cli = Cli::new("cli-retry");
    let url = server.url("/api/status?code=503&retry_after=1");

    let started = std::time::Instant::now();
    let output = cli.run(&["scrape", &url, "--ignore-robots", "--allow-private-network"]);
    let elapsed = started.elapsed();

    // The 503 is retried once after the second the server asked for, and the
    // second answer is reported.
    assert!(
        elapsed >= std::time::Duration::from_millis(900),
        "the Retry-After was not waited out: {elapsed:?}"
    );
    let line: Value = serde_json::from_str(stdout(&output).trim()).expect("json line");
    assert_eq!(line["status"], 503);
    assert_eq!(code(&output), 1, "every page failed");
    assert!(
        stderr(&output).contains("all 1 pages failed"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scrape_reads_its_urls_from_a_file_and_reports_transport_failures() {
    let server = server().await;
    let cli = Cli::new("cli-from");
    let list = cli.home.write(
        "urls.txt",
        format!(
            "# two pages, one comment\n{}\n{}\n",
            server.url("/robots/allowed.html"),
            server.url("/tiny/index.html")
        ),
    );

    let output = cli.run(&[
        "scrape",
        "--from",
        list.to_str().expect("utf-8"),
        "--ordered",
        "--allow-private-network",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(stdout(&output).lines().count(), 2);

    let port = server.port();
    server.shutdown();
    let gone = cli.run(&[
        "scrape",
        &format!("http://127.0.0.1:{port}/tiny/index.html"),
        "--ignore-robots",
        "--allow-private-network",
    ]);
    let line: Value = serde_json::from_str(stdout(&gone).trim()).expect("json line");
    assert_eq!(line["error"], "net::ERR_CONNECTION_REFUSED");
    assert_eq!(code(&gone), 1);
}

// ------------------------------------------------------------------- version

#[test]
fn version_is_reported_by_the_flag_and_the_subcommand() {
    let cli = Cli::new("cli-version");
    let flag = cli.run(&["--version"]);
    assert_eq!(code(&flag), 0);
    assert!(
        stdout(&flag).starts_with("tentabrowser "),
        "{}",
        stdout(&flag)
    );

    let subcommand = cli.run(&["version"]);
    assert_eq!(code(&subcommand), 0);
    assert_eq!(stdout(&subcommand), stdout(&flag));
    assert!(stdout(&subcommand).contains(env!("CARGO_PKG_VERSION")));
}

// ------------------------------------------------------------------- userinfo stripping (N-R4-4)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_strips_userinfo_from_url_output() {
    let server = server().await;
    let cli = Cli::new("cli-userinfo");

    // Fetch with userinfo in the URL
    let url_with_userinfo = format!(
        "http://user:password@127.0.0.1:{}/tiny/index.html",
        server.port()
    );
    let output = cli.run(&[
        "fetch",
        &url_with_userinfo,
        "--dump",
        "json",
        "--allow-private-network",
    ]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));

    // Parse the JSON output
    let json: Value = serde_json::from_str(&stdout(&output)).expect("valid JSON output");

    // Check that url and final_url do not contain userinfo (user:password@)
    let url = json["url"].as_str().expect("url field");
    let final_url = json["final_url"].as_str().expect("final_url field");

    assert!(
        !url.contains("user:password"),
        "url should not contain userinfo: {url}"
    );
    assert!(
        !final_url.contains("user:password"),
        "final_url should not contain userinfo: {final_url}"
    );

    // Verify the URL still points to the correct location
    assert!(
        url.contains("127.0.0.1"),
        "url should contain the host: {url}"
    );
    assert!(
        final_url.contains("127.0.0.1"),
        "final_url should contain the host: {final_url}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_with_headers_flag_strips_userinfo() {
    let server = server().await;
    let cli = Cli::new("cli-userinfo-headers");

    // Fetch with userinfo in the URL and --headers flag
    let url_with_userinfo = format!("http://user:pw@127.0.0.1:{}/tiny/index.html", server.port());
    let output = cli.run(&[
        "fetch",
        &url_with_userinfo,
        "--headers",
        "--allow-private-network",
    ]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));

    // Check stderr for the status and URL
    let err = stderr(&output);
    assert!(
        !err.contains("user:pw"),
        "headers output should not contain userinfo: {err}"
    );
    assert!(
        err.contains("127.0.0.1"),
        "headers output should contain the host: {err}"
    );
}

// -------------------------------------------------------------------- screenshot

#[cfg(feature = "render")]
const SOLID: &str = "<!doctype html><html><head><style>html,body{margin:0}body{background:#00ff00}\
    #box{width:100px;height:50px;background:#ff0000}</style></head>\
    <body><div id=\"box\"></div></body></html>";

#[cfg(feature = "render")]
const LONG: &str = "<!doctype html><html><head><style>html,body{margin:0}\
    .top{height:1500px;background:#00ff00}.bottom{height:1500px;background:#0000ff}\
    </style></head><body><div class=\"top\"></div><div class=\"bottom\"></div></body></html>";

#[cfg(all(feature = "render", feature = "v8"))]
const SCRIPTED: &str = "<!doctype html><html><head><style>html,body{margin:0}</style></head>\
    <body><script>document.body.style.background = '#ff0000';</script></body></html>";

#[cfg(feature = "render")]
const SHADOW: &str = "<!doctype html><html><head><style>html,body{margin:0}\
    #card{width:60px;height:40px;margin:20px;background:#ffffff;box-shadow:0 0 8px #000000}\
    </style></head><body><div id=\"card\"></div></body></html>";

#[cfg(feature = "render")]
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
/// The IEND chunk: the last bytes of a PNG, with no newline after them.
#[cfg(feature = "render")]
const PNG_END: [u8; 12] = [0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82];

#[cfg(feature = "render")]
const RED: [u8; 4] = [255, 0, 0, 255];
#[cfg(feature = "render")]
const GREEN: [u8; 4] = [0, 255, 0, 255];
#[cfg(feature = "render")]
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// Serves the screenshot pages from a directory of their own.
#[cfg(feature = "render")]
async fn screenshot_server() -> (FixtureServer, TempDir) {
    let pages = temp_dir("cli-screenshot-pages");
    pages.write("solid.html", SOLID);
    pages.write("long.html", LONG);
    pages.write("shadow.html", SHADOW);
    #[cfg(feature = "v8")]
    pages.write("scripted.html", SCRIPTED);
    let server = FixtureServer::start(FixtureServerConfig::new(pages.path()))
        .await
        .expect("fixture server");
    (server, pages)
}

#[cfg(feature = "render")]
fn utf8(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

#[cfg(feature = "render")]
struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[cfg(feature = "render")]
impl Decoded {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        self.rgba[at..at + 4].try_into().expect("four bytes")
    }
}

#[cfg(feature = "render")]
fn decode_png(bytes: &[u8]) -> Decoded {
    assert_eq!(&bytes[..8], PNG_SIGNATURE, "a PNG signature");
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .expect("a decodable PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("a sized buffer")];
    let info = reader.next_frame(&mut rgba).expect("a frame");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    rgba.truncate(info.buffer_size());
    Decoded {
        width: info.width,
        height: info.height,
        rgba,
    }
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_screenshot_writes_a_png_of_the_requested_size() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/solid.html");
    let cli = Cli::new("cli-shot-png");
    let out = temp_dir("cli-shot-out");
    let target = out.path().join("solid.png");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
        "--width",
        "320",
        "--height",
        "200",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).is_empty(), "the image went to the file");
    assert!(
        stderr(&output).contains("320x200 png"),
        "{}",
        stderr(&output)
    );

    let shot = decode_png(&std::fs::read(&target).expect("the file exists"));
    assert_eq!((shot.width, shot.height), (320, 200));
    assert_eq!(shot.pixel(50, 25), RED, "inside the 100x50 box");
    assert_eq!(shot.pixel(300, 150), GREEN, "the body background");
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stdout_receives_the_exact_png_bytes() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/solid.html");
    let cli = Cli::new("cli-shot-stdout");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        "-",
        "--allow-private-network",
        "--ephemeral",
        "--width",
        "64",
        "--height",
        "48",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(output.stdout.starts_with(&PNG_SIGNATURE), "a PNG on stdout");
    assert!(
        output.stdout.ends_with(&PNG_END),
        "no newline appended to the image"
    );
    assert!(
        stderr(&output).contains("wrote stdout"),
        "{}",
        stderr(&output)
    );
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jpeg_output_carries_the_jpeg_markers() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/solid.html");
    let cli = Cli::new("cli-shot-jpeg");
    let out = temp_dir("cli-shot-jpeg-out");
    let target = out.path().join("solid.jpg");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--format",
        "jpeg",
        "--quality",
        "75",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stderr(&output).contains("jpeg"), "{}", stderr(&output));

    let bytes = std::fs::read(&target).expect("the file exists");
    assert_eq!(&bytes[..3], [0xFF, 0xD8, 0xFF], "the JPEG start of image");
    assert_eq!(
        &bytes[bytes.len() - 2..],
        [0xFF, 0xD9],
        "the JPEG end of image"
    );
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_selector_captures_the_element_and_full_page_the_whole_document() {
    let (server, _pages) = screenshot_server().await;
    let solid = server.url("/solid.html");
    let long = server.url("/long.html");
    let cli = Cli::new("cli-shot-areas");
    let out = temp_dir("cli-shot-areas-out");
    let element = out.path().join("element.png");
    let selected = cli.run(&[
        "screenshot",
        &solid,
        "-o",
        utf8(&element),
        "--selector",
        "#box",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&selected), 0, "{}", stderr(&selected));
    let shot = decode_png(&std::fs::read(&element).expect("the element image"));
    assert_eq!(
        (shot.width, shot.height),
        (100, 50),
        "the box is 100x50 CSS px"
    );
    assert_eq!(shot.pixel(99, 49), RED);

    let whole = out.path().join("long.png");
    let full = cli.run(&[
        "screenshot",
        &long,
        "-o",
        utf8(&whole),
        "--full-page",
        "--width",
        "320",
        "--height",
        "200",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&full), 0, "{}", stderr(&full));
    let page = decode_png(&std::fs::read(&whole).expect("the full-page image"));
    assert_eq!(page.width, 320);
    assert!(
        page.height >= 3000,
        "the scroll extent, not the 200px viewport: {}",
        page.height
    );
    assert_eq!(page.pixel(10, 10), GREEN, "the first screen");
    assert_eq!(page.pixel(10, 2000), BLUE, "the second half of the page");
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clip_captures_its_rectangle_in_css_pixels() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/solid.html");
    let cli = Cli::new("cli-shot-clip");
    let out = temp_dir("cli-shot-clip-out");
    let target = out.path().join("clip.png");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--clip",
        "10,20,30,40",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let shot = decode_png(&std::fs::read(&target).expect("the clip image"));
    assert_eq!((shot.width, shot.height), (30, 40));
    assert_eq!(shot.pixel(0, 0), RED, "the clip starts inside the box");
    assert_eq!(shot.pixel(0, 39), GREEN, "and ends below it");
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_image_file_needs_allow_write_and_is_not_created_when_refused() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/solid.html");
    let cli = Cli::new("cli-shot-denied");
    let out = temp_dir("cli-shot-denied-out");
    let target = out.path().join("denied.png");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--allow-private-network",
    ]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("denied by policy"),
        "{}",
        stderr(&output)
    );
    assert!(
        !target.exists(),
        "nothing is written when the path is refused"
    );
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_http_error_is_refused_unless_the_flag_accepts_it() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/missing.html");
    let cli = Cli::new("cli-shot-http");
    let out = temp_dir("cli-shot-http-out");
    let target = out.path().join("missing.png");
    let refused = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&refused), 3, "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("HTTP 404"),
        "{}",
        stderr(&refused)
    );
    assert!(!target.exists());

    let accepted = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--allow-http-errors",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&accepted), 0, "{}", stderr(&accepted));
    assert!(target.exists());
}

#[cfg(all(feature = "render", feature = "v8"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripts_run_before_the_capture_when_the_build_has_v8() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/scripted.html");
    let cli = Cli::new("cli-shot-js");
    let out = temp_dir("cli-shot-js-out");
    let target = out.path().join("scripted.png");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--width",
        "64",
        "--height",
        "48",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let shot = decode_png(&std::fs::read(&target).expect("the image"));
    assert_eq!(shot.pixel(10, 10), RED, "the script painted the body");
}

#[test]
fn screenshot_options_are_refused_before_any_request() {
    let cli = Cli::new("cli-shot-usage");
    let url = "http://127.0.0.1:1/";
    let cases: [&[&str]; 5] = [
        &["--quality", "0"],
        &["--quality", "101"],
        &["--quality", "50"],
        &["--selector", "#box", "--full-page"],
        &["--clip", "1,2,0,40"],
    ];
    for case in cases {
        let output = cli.run(&[&["screenshot", url][..], case].concat());
        assert_eq!(code(&output), 2, "{case:?}: {}", stderr(&output));
        assert!(stdout(&output).is_empty(), "{case:?}");
    }
    let quality = cli.run(&["screenshot", url, "--quality", "50"]);
    assert!(
        stderr(&quality).contains("--format jpeg"),
        "{}",
        stderr(&quality)
    );
}

#[cfg(not(feature = "render"))]
#[test]
fn screenshot_without_render_exits_unsupported_before_network_access() {
    let cli = Cli::new("cli-shot-disabled");
    let output = cli.run(&["screenshot", "http://127.0.0.1:1/"]);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("--features render"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).is_empty());
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dpr_doubles_the_output_size_and_the_device_pixels() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/solid.html");
    let cli = Cli::new("cli-shot-dpr");
    let out = temp_dir("cli-shot-dpr-out");
    let target = out.path().join("dpr.png");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--width",
        "320",
        "--height",
        "200",
        "--dpr",
        "2",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let shot = decode_png(&std::fs::read(&target).expect("the image"));
    assert_eq!((shot.width, shot.height), (640, 400));
    assert_eq!(
        shot.pixel(150, 80),
        RED,
        "the 100x50 box covers 200x100 device px"
    );
    assert_eq!(shot.pixel(400, 300), GREEN);
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn json_prints_one_summary_object_on_stderr() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/solid.html");
    let cli = Cli::new("cli-shot-json");
    let out = temp_dir("cli-shot-json-out");
    let target = out.path().join("summary.png");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--width",
        "320",
        "--height",
        "200",
        "--json",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).is_empty(), "the image went to the file");
    let summary: Value = serde_json::from_str(stderr(&output).trim()).expect("one JSON object");
    assert!(
        summary["url"]
            .as_str()
            .expect("url")
            .ends_with("/solid.html")
    );
    assert_eq!(summary["format"], "png");
    assert_eq!(summary["width"], 320);
    assert_eq!(summary["height"], 200);
    assert_eq!(
        summary["bytes"].as_u64(),
        Some(std::fs::metadata(&target).expect("the file").len())
    );
    assert!(summary["ms"].is_number());
    assert_eq!(summary["clamped"], false);
    assert!(summary["skipped"].is_object());
    assert!(summary["approximated"].is_object());
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_box_shadow_is_reported_as_not_drawn() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/shadow.html");
    let cli = Cli::new("cli-shot-shadow");
    let out = temp_dir("cli-shot-shadow-out");
    let target = out.path().join("shadow.png");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("warning: not drawn: BoxShadow x1"),
        "{}",
        stderr(&output)
    );
    assert!(target.exists(), "the image is written anyway");
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_selector_that_matches_nothing_is_a_usage_error() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/solid.html");
    let cli = Cli::new("cli-shot-nothing");
    let out = temp_dir("cli-shot-nothing-out");
    let target = out.path().join("none.png");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--selector",
        "#no-such-element",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(!target.exists());
}

#[cfg(feature = "render")]
#[test]
fn the_default_file_is_refused_without_allow_write() {
    let cli = Cli::new("cli-shot-default-file");
    let output = cli.run(&["screenshot", "http://127.0.0.1:1/"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("denied by policy: screenshot.png"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).is_empty());
}

#[cfg(all(feature = "render", feature = "v8"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn js_off_leaves_the_scripts_unrun() {
    let (server, _pages) = screenshot_server().await;
    let url = server.url("/scripted.html");
    let cli = Cli::new("cli-shot-js-off");
    let out = temp_dir("cli-shot-js-off-out");
    let target = out.path().join("off.png");
    let output = cli.run(&[
        "screenshot",
        &url,
        "-o",
        utf8(&target),
        "--js",
        "off",
        "--width",
        "64",
        "--height",
        "48",
        "--allow-write",
        utf8(out.path()),
        "--allow-private-network",
        "--ephemeral",
    ]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let shot = decode_png(&std::fs::read(&target).expect("the image"));
    assert_ne!(shot.pixel(10, 10), RED, "the script did not paint the body");
}

#[cfg(not(feature = "v8"))]
#[test]
fn js_on_needs_a_build_with_v8() {
    let cli = Cli::new("cli-shot-js-on");
    let output = cli.run(&["screenshot", "http://127.0.0.1:1/", "--js", "on"]);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("--features v8"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).is_empty());
}
