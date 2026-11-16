//! Protocol conformance without a script engine or a renderer: the handshake
//! of every revision, the tool list, typed errors, the tools that work at
//! Tier 1, and both transports. Tools that need V8 or the render tier are in
//! `tools.rs` and `scenario.rs`.

mod common;

use std::time::Duration;

use common::{Harness, find_ref};
use serde_json::{Value, json};
use tenta_core::Tier;
use tenta_mcp::{HttpOptions, HttpServer, stdio};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

const TOOL_NAMES: [&str; 19] = [
    "browser_navigate",
    "browser_close",
    "browser_back",
    "browser_forward",
    "browser_reload",
    "browser_snapshot",
    "browser_extract",
    "browser_click",
    "browser_type",
    "browser_fill",
    "browser_press_key",
    "browser_select_option",
    "browser_hover",
    "browser_wait_for",
    "browser_evaluate",
    "browser_screenshot",
    "browser_console_messages",
    "browser_network_requests",
    "browser_web_search",
];

fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

// ---- handshake and versions ----------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn initialize_agrees_on_the_clients_revision_or_our_newest_session_based_one() {
    let harness = Harness::start(Tier::Dom).await;
    for requested in ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"] {
        let client = harness.client();
        let result = client
            .result(
                "initialize",
                json!({"protocolVersion": requested, "capabilities": {}}),
            )
            .await;
        assert_eq!(result["protocolVersion"], requested);
        assert_eq!(result["serverInfo"]["name"], "tentabrowser");
        assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
        assert!(
            result["instructions"]
                .as_str()
                .unwrap()
                .contains("browser_snapshot")
        );
        assert_eq!(
            client.connection.negotiated_version().as_deref(),
            Some(requested)
        );
    }
    let client = harness.client();
    let result = client
        .result(
            "initialize",
            json!({"protocolVersion": "1999-01-01", "capabilities": {}}),
        )
        .await;
    assert_eq!(result["protocolVersion"], "2025-11-25");
    // A future revision the server does not know is answered with its own newest.
    let result = client
        .result("initialize", json!({"protocolVersion": "2030-01-01"}))
        .await;
    assert_eq!(result["protocolVersion"], "2025-11-25");
    let missing = client.request("initialize", json!({})).await;
    assert_eq!(missing["error"]["code"], -32602);
}

#[tokio::test(flavor = "multi_thread")]
async fn ping_and_unknown_methods() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    assert_eq!(client.result("ping", json!({})).await, json!({}));
    let unknown = client.request("resources/list", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32601);
    // `ping` is gone from the stateless revision.
    let modern = client
        .request("ping", json!({"_meta": modern_meta()}))
        .await;
    assert_eq!(modern["error"]["code"], -32601);
}

#[tokio::test(flavor = "multi_thread")]
async fn messages_that_are_not_requests_get_the_right_answer() {
    let harness = Harness::start(Tier::Dom).await;
    let connection = harness.client().connection;
    // Notifications and client responses are never answered.
    assert!(
        connection
            .handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await
            .is_none()
    );
    assert!(
        connection
            .handle(json!({"jsonrpc": "2.0", "method": "notifications/unheard-of"}))
            .await
            .is_none()
    );
    assert!(
        connection
            .handle(json!({"jsonrpc": "2.0", "id": 1, "result": {}}))
            .await
            .is_none()
    );
    for bad in [
        json!([]),
        json!("text"),
        json!({"jsonrpc": "2.0", "id": 4}),
        json!({"id": [], "method": "ping"}),
    ] {
        let response = connection.handle(bad).await.expect("an error response");
        assert_eq!(response["error"]["code"], -32600, "{response}");
    }
    let response = connection
        .handle(json!({"jsonrpc": "2.0", "id": 9, "method": "ping", "params": [1]}))
        .await
        .unwrap();
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(response["id"], 9);
}

#[tokio::test(flavor = "multi_thread")]
async fn server_discover_and_per_request_versions() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    let discovered = client
        .result("server/discover", json!({"_meta": modern_meta()}))
        .await;
    assert_eq!(discovered["resultType"], "complete");
    assert_eq!(discovered["supportedVersions"][0], "2026-07-28");
    assert!(
        discovered["supportedVersions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "2025-11-25")
    );
    assert!(discovered["capabilities"]["tools"].is_object());
    assert_eq!(discovered["cacheScope"], "public");
    assert!(discovered["ttlMs"].as_u64().is_some());
    assert_eq!(
        discovered["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "tentabrowser"
    );

    let mut meta = modern_meta();
    meta["io.modelcontextprotocol/protocolVersion"] = json!("1900-01-01");
    let refused = client.request("tools/list", json!({"_meta": meta})).await;
    assert_eq!(refused["error"]["code"], -32022);
    assert_eq!(refused["error"]["data"]["requested"], "1900-01-01");
    assert_eq!(refused["error"]["data"]["supported"][0], "2026-07-28");
}

// ---- tools/list ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_list_is_complete_documented_and_versioned() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    let legacy = client.result("tools/list", json!({})).await;
    assert!(
        legacy.get("resultType").is_none(),
        "legacy results are unchanged"
    );
    let tools = legacy["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    let mut expected: Vec<&str> = TOOL_NAMES.to_vec();
    expected.sort_unstable();
    assert_eq!(names, expected);
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert!(tool["description"].as_str().unwrap().len() > 80, "{name}");
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
        assert!(
            tool["inputSchema"]["examples"]
                .as_array()
                .is_some_and(|e| !e.is_empty()),
            "{name}"
        );
        for (property, schema) in tool["inputSchema"]["properties"].as_object().unwrap() {
            assert!(
                schema["description"].is_string() || schema["enum"].is_array(),
                "{name}.{property}"
            );
        }
        assert!(tool["annotations"]["readOnlyHint"].is_boolean(), "{name}");
    }
    assert_eq!(
        tools
            .iter()
            .find(|t| t["name"] == "browser_snapshot")
            .unwrap()["annotations"]["readOnlyHint"],
        true
    );
    // The same list, deterministically ordered, is what the stateless era gets.
    let modern = client
        .result("tools/list", json!({"_meta": modern_meta()}))
        .await;
    assert_eq!(modern["tools"], legacy["tools"]);
    assert_eq!(modern["resultType"], "complete");
    assert_eq!(modern["cacheScope"], "public");
    assert!(modern["ttlMs"].as_u64().unwrap() > 0);
}

#[test]
fn the_tool_list_matches_its_golden_file() {
    let golden = common::fixtures_dir().join("../golden/tools.json");
    let actual = serde_json::to_string_pretty(&tenta_mcp::tool_list()).unwrap() + "\n";
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(&golden).unwrap_or_default();
    assert!(
        expected == actual,
        "the tool list changed: bump SCHEMA_VERSION ({}) and bless with UPDATE_GOLDEN=1",
        tenta_mcp::SCHEMA_VERSION
    );
}

// ---- tools/call errors ---------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn protocol_errors_and_tool_errors_are_different_things() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    // Not a tool: a protocol error.
    let unknown = client
        .request("tools/call", json!({"name": "browser_teleport"}))
        .await;
    assert_eq!(unknown["error"]["code"], -32602);
    assert!(
        unknown["error"]["message"]
            .as_str()
            .unwrap()
            .contains("browser_teleport")
    );
    let nameless = client.request("tools/call", json!({})).await;
    assert_eq!(nameless["error"]["code"], -32602);
    let bad_arguments = client
        .request(
            "tools/call",
            json!({"name": "browser_snapshot", "arguments": [1]}),
        )
        .await;
    assert_eq!(bad_arguments["error"]["code"], -32602);

    // A tool that ran and failed: a result the model can read.
    for (name, arguments, needle) in [
        (
            "browser_navigate",
            json!({}),
            "missing required property `url`",
        ),
        ("browser_navigate", json!({"url": 5}), "expected string"),
        (
            "browser_navigate",
            json!({"url": "https://x.test", "wait_until": "soon"}),
            "must be one of",
        ),
        ("browser_click", json!({}), "either `ref` or both"),
        (
            "browser_click",
            json!({"ref": "e1", "x": 1, "y": 2}),
            "either `ref` or both",
        ),
        ("browser_click", json!({"x": 1}), "together"),
        (
            "browser_click",
            json!({"ref": "e1", "colour": "red"}),
            "unknown property `colour`",
        ),
        ("browser_snapshot", json!({"max_chars": 5}), "at least 1000"),
        ("browser_wait_for", json!({}), "at least one of"),
        (
            "browser_press_key",
            json!({"key": "Hyper+Q"}),
            "unknown modifier",
        ),
        (
            "browser_evaluate",
            json!({"expression": ""}),
            "at least 1 characters",
        ),
    ] {
        let result = client.tool(name, arguments).await;
        assert_eq!(
            result.error_type(),
            "invalid_arguments",
            "{name}: {}",
            result.0
        );
        assert!(result.text().contains(needle), "{name}: {}", result.text());
        assert_eq!(result.0["structuredContent"]["error"]["retryable"], false);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_that_need_a_tier_say_tier_denied() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/form.html")).await;
    let name = client.ref_of(&["textbox", "Your name"]).await;
    for (tool, arguments) in [
        ("browser_evaluate", json!({"expression": "1"})),
        ("browser_fill", json!({"ref": name, "value": "x"})),
        ("browser_type", json!({"ref": name, "text": "x"})),
        ("browser_press_key", json!({"key": "Enter"})),
        ("browser_screenshot", json!({})),
        ("browser_click", json!({"x": 5, "y": 5})),
    ] {
        let result = client.tool(tool, arguments).await;
        assert_eq!(result.error_type(), "tier_denied", "{tool}: {}", result.0);
    }
}

// ---- navigate, snapshot, wait_for at Tier 1 ------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn navigate_reports_the_page_and_failures_are_typed() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    let result = client.navigate(&harness.url("/index.html")).await;
    let structured = result.structured();
    assert_eq!(structured["status"], 200);
    assert_eq!(structured["title"], "Fixture Home");
    assert_eq!(structured["final_url"], harness.url("/index.html"));
    assert_eq!(structured["settled"]["timed_out"], false);
    assert!(result.text().contains("Fixture Home"));

    let missing = client.navigate(&harness.url("/no-such-page")).await;
    assert_eq!(
        missing.structured()["status"],
        404,
        "an HTTP error is content, not a failure"
    );

    let refused = client
        .tool("browser_navigate", json!({"url": "http://127.0.0.1:1/"}))
        .await;
    assert_eq!(refused.error_type(), "navigation_failed", "{}", refused.0);
    assert!(refused.text().contains("net::ERR_"), "{}", refused.text());
    assert_eq!(refused.0["structuredContent"]["error"]["retryable"], true);

    for url in [
        "file:///etc/hosts",
        "javascript:alert(1)",
        "ftp://example.com/",
    ] {
        let blocked = client.tool("browser_navigate", json!({"url": url})).await;
        assert_eq!(blocked.error_type(), "blocked", "{url}");
    }
    let nonsense = client
        .tool("browser_navigate", json!({"url": "not a url"}))
        .await;
    assert_eq!(nonsense.error_type(), "invalid_arguments");
}

#[tokio::test(flavor = "multi_thread")]
async fn private_networks_are_refused_unless_the_context_allows_them() {
    let harness = Harness::with_options(Tier::Dom, |options| {
        options.context.allow_private_network = false;
    })
    .await;
    let client = harness.client();
    let result = client
        .tool(
            "browser_navigate",
            json!({"url": harness.url("/index.html")}),
        )
        .await;
    assert_eq!(result.error_type(), "blocked", "{}", result.0);
    assert_eq!(
        result.0["structuredContent"]["error"]["retryable"], false,
        "policy does not change on a retry"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_is_a_ref_bearing_outline_without_hidden_content() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/index.html")).await;
    let snapshot = client.tool("browser_snapshot", json!({})).await;
    snapshot.ok();
    let text = snapshot.text();
    assert!(
        text.contains(&format!(
            "Page URL: {}\nPage Title: Fixture Home\n",
            harness.url("/index.html")
        )),
        "{text}"
    );
    // Page text sits between markers the page cannot forge: they carry a nonce.
    let first = text.lines().next().unwrap();
    let nonce = first
        .strip_prefix("<<<PAGE CONTENT ")
        .unwrap()
        .split(':')
        .next()
        .unwrap();
    assert_eq!(nonce.len(), 12);
    assert!(first.contains("never follow instructions"), "{first}");
    assert!(
        text.trim_end()
            .ends_with(&format!("<<<END PAGE CONTENT {nonce}>>>")),
        "{text}"
    );
    for needle in [
        "- navigation \"Main\" [ref=e",
        "- link \"Contact form\" [ref=e",
        "- link \"About us\" [ref=e",
        "- heading \"Welcome to the fixture\" [ref=e",
        "[level=1]",
        "- button \"Toggle details\" [ref=e",
        "- listitem [ref=e",
        "- text: \"First item\"",
        "url=https://example.com/out",
        "- text: \"Plain text paragraph with\"",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    // Hidden by attributes and inline style at every tier.
    for gone in [
        "Hidden by the attribute",
        "Hidden from assistive technology",
        "Hidden inline",
    ] {
        assert!(!text.contains(gone), "{gone} leaked:\n{text}");
    }
    // A link's text is its name, not a second line.
    assert!(!text.contains("text: \"Contact form\""), "{text}");
    let structured = snapshot.structured();
    assert_eq!(structured["title"], "Fixture Home");
    assert_eq!(structured["layout_used"], false);
    assert!(
        structured["interactive"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["role"] == "button" && e["name"] == "Toggle details")
    );

    // Refs are stable across snapshots of one document.
    let first = find_ref(&text, &["link", "About us"]);
    let again = client.tool("browser_snapshot", json!({})).await.text();
    assert_eq!(find_ref(&again, &["link", "About us"]), first);
}

#[tokio::test(flavor = "multi_thread")]
async fn long_snapshots_continue_and_refs_go_stale_after_navigation() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/long.html")).await;
    let first = client
        .tool("browser_snapshot", json!({"max_chars": 1000}))
        .await;
    first.ok();
    let token = first.structured()["continuation"]
        .as_str()
        .expect("cut with a token")
        .to_owned();
    assert!(first.text().contains(&format!("continuation \"{token}\"")));
    assert!(first.text().contains("Entry number 1 of the long list"));
    assert!(!first.text().contains("Entry number 300 of"));

    let nonce = first.text()["<<<PAGE CONTENT ".len()..][..12].to_owned();
    let mut seen = first.text();
    let mut next = Some(token);
    while let Some(token) = next {
        let part = client
            .tool(
                "browser_snapshot",
                json!({"max_chars": 1000, "continuation": token}),
            )
            .await;
        part.ok();
        assert!(
            !part.text().contains("Page URL"),
            "only the first part has the header"
        );
        assert!(
            part.text()
                .contains(&format!("<<<END PAGE CONTENT {nonce}>>>")),
            "every part is bounded"
        );
        seen.push_str(&part.text());
        next = part.structured()["continuation"]
            .as_str()
            .map(str::to_owned);
    }
    assert!(seen.contains("Entry number 300 of the long list"));

    let bad = client
        .tool("browser_snapshot", json!({"continuation": "s999:0"}))
        .await;
    assert_eq!(bad.error_type(), "invalid_arguments");

    // A ref survives until the page navigates.
    client.navigate(&harness.url("/index.html")).await;
    let link = client.ref_of(&["link", "About us"]).await;
    client.navigate(&harness.url("/about.html")).await;
    let stale = client.tool("browser_click", json!({"ref": link})).await;
    assert_eq!(stale.error_type(), "element_not_found", "{}", stale.0);
    assert!(stale.text().contains("stale"), "{}", stale.text());
    let unknown = client.tool("browser_click", json!({"ref": "e99999"})).await;
    assert_eq!(unknown.error_type(), "element_not_found");
    let malformed = client.tool("browser_click", json!({"ref": "link-3"})).await;
    assert_eq!(malformed.error_type(), "invalid_arguments");
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_for_succeeds_immediately_or_times_out_naming_what_is_missing() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/index.html")).await;
    for arguments in [
        json!({"text": "Welcome   to the fixture"}),
        json!({"selector": "#toggle"}),
        json!({"text_gone": "no such sentence"}),
        json!({"text": "Second item", "selector": "ul > li", "time_ms": 10}),
    ] {
        let done = client.tool("browser_wait_for", arguments).await;
        done.ok();
        assert!(done.structured()["waited_ms"].as_u64().is_some());
    }
    let missing = client
        .tool(
            "browser_wait_for",
            json!({"text": "never appears", "timeout_ms": 150}),
        )
        .await;
    assert_eq!(missing.error_type(), "timeout");
    assert!(
        missing.text().contains("never appears"),
        "{}",
        missing.text()
    );
    let gone = client
        .tool(
            "browser_wait_for",
            json!({"text_gone": "Welcome", "selector": "#absent", "timeout_ms": 100}),
        )
        .await;
    assert_eq!(gone.error_type(), "timeout");
    assert!(
        gone.text().contains("still shown") && gone.text().contains("#absent"),
        "{}",
        gone.text()
    );
    let invalid = client
        .tool(
            "browser_wait_for",
            json!({"selector": "div[", "timeout_ms": 100}),
        )
        .await;
    assert_eq!(invalid.error_type(), "invalid_arguments");
}

#[tokio::test(flavor = "multi_thread")]
async fn close_releases_the_page_and_the_next_call_opens_a_fresh_one() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    let none = client.tool("browser_close", json!({})).await;
    assert_eq!(none.structured()["closed"], false);
    client.navigate(&harness.url("/index.html")).await;
    let closed = client.tool("browser_close", json!({})).await;
    assert_eq!(closed.structured()["closed"], true);
    let blank = client.tool("browser_snapshot", json!({})).await;
    blank.ok();
    assert!(
        blank.text().contains("Page URL: about:blank"),
        "{}",
        blank.text()
    );
    client.navigate(&harness.url("/about.html")).await;
    assert!(
        client
            .tool("browser_snapshot", json!({}))
            .await
            .text()
            .contains("We make browsers.")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn connections_are_limited_and_isolated() {
    let harness = Harness::with_options(Tier::Dom, |options| options.max_sessions = 2).await;
    let (a, b) = (harness.client(), harness.client());
    assert!(
        harness.server.connect().is_err(),
        "the third session is refused"
    );
    a.navigate(&harness.url("/about.html")).await;
    let other = b.tool("browser_snapshot", json!({})).await;
    assert!(
        other.text().contains("about:blank"),
        "sessions do not share pages"
    );
    drop(b);
    assert!(
        harness.server.connect().is_ok(),
        "a closed connection frees its slot"
    );
}

// ---- stdio ---------------------------------------------------------------

async fn read_json(lines: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>) -> Value {
    let line = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
        .await
        .expect("a reply in time")
        .expect("readable")
        .expect("a line");
    serde_json::from_str(&line).unwrap_or_else(|_| panic!("JSON, got {line:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_speaks_newline_delimited_json_rpc() {
    let harness = Harness::start(Tier::Dom).await;
    let (mut to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, from_server) = tokio::io::duplex(64 * 1024);
    let server = harness.server.clone();
    let serving = tokio::spawn(async move { stdio::serve(&server, server_in, server_out).await });
    let mut lines = BufReader::new(from_server).lines();

    let send = |value: Value| format!("{value}\n");
    to_server
        .write_all(
            send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}}))
                .as_bytes(),
        )
        .await
        .unwrap();
    assert_eq!(
        read_json(&mut lines).await["result"]["protocolVersion"],
        "2025-11-25"
    );
    to_server
        .write_all(
            send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).as_bytes(),
        )
        .await
        .unwrap();

    to_server.write_all(b"{this is not json\n\n").await.unwrap();
    let parse_error = read_json(&mut lines).await;
    assert_eq!(parse_error["error"]["code"], -32700);
    assert_eq!(parse_error["id"], Value::Null);

    to_server
        .write_all(send(json!({"jsonrpc": "2.0", "id": "a", "method": "tools/list"})).as_bytes())
        .await
        .unwrap();
    let listed = read_json(&mut lines).await;
    assert_eq!(listed["id"], "a");
    assert_eq!(
        listed["result"]["tools"].as_array().unwrap().len(),
        TOOL_NAMES.len()
    );

    // A request cancelled in flight gets no response; the next one does.
    to_server
        .write_all(
            send(
                json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {
                "name": "browser_wait_for", "arguments": {"text": "never", "timeout_ms": 60000}}}),
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    to_server
        .write_all(send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7}})).as_bytes())
        .await
        .unwrap();
    to_server
        .write_all(send(json!({"jsonrpc": "2.0", "id": 8, "method": "ping"})).as_bytes())
        .await
        .unwrap();
    let after = read_json(&mut lines).await;
    assert_eq!(
        after["id"], 8,
        "the cancelled request must not be answered: {after}"
    );

    // End of input: requests still running are answered before the server returns.
    to_server
        .write_all(
            send(
                json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {
                "name": "browser_navigate", "arguments": {"url": harness.url("/about.html")}}}),
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    drop(to_server);
    let last = read_json(&mut lines).await;
    assert_eq!(last["id"], 9);
    assert_eq!(last["result"]["structuredContent"]["status"], 200);
    serving.await.unwrap().unwrap();
    assert!(
        lines.next_line().await.unwrap().is_none(),
        "the output ends with the session"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_refuses_a_message_over_the_limit() {
    let harness = Harness::start(Tier::Dom).await;
    let (mut to_server, server_in) = tokio::io::duplex(1024 * 1024);
    let (server_out, from_server) = tokio::io::duplex(64 * 1024);
    let server = harness.server.clone();
    let serving = tokio::spawn(async move { stdio::serve(&server, server_in, server_out).await });
    let writer = tokio::spawn(async move {
        let chunk = vec![b'a'; 1024 * 1024];
        for _ in 0..=(stdio::MAX_MESSAGE_BYTES / chunk.len()) {
            if to_server.write_all(&chunk).await.is_err() {
                return;
            }
        }
    });
    let mut lines = BufReader::new(from_server).lines();
    let reply = read_json(&mut lines).await;
    assert_eq!(reply["error"]["code"], -32600);
    assert!(serving.await.unwrap().is_err());
    writer.abort();
}

// ---- streamable HTTP -----------------------------------------------------

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|_| panic!("JSON body, got {:?}", self.body))
    }
}

async fn http(
    addr: std::net::SocketAddr,
    method: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Response {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut request = format!("{method} /mcp HTTP/1.1\r\n");
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        request.push_str(&format!("Host: {addr}\r\n"));
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    if let Some(body) = body {
        stream.write_all(body.as_bytes()).await.unwrap();
    }
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), stream.read_to_end(&mut raw))
        .await
        .expect("a response in time")
        .ok();
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let mut head_lines = head.lines();
    let status = head_lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("a status line in {raw:?}"));
    Response {
        status,
        headers: head_lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
            .collect(),
        body: body.to_owned(),
    }
}

const JSON_HEADERS: [(&str, &str); 1] = [("Content-Type", "application/json")];

async fn post(addr: std::net::SocketAddr, extra: &[(&str, &str)], message: Value) -> Response {
    let mut headers = JSON_HEADERS.to_vec();
    headers.extend_from_slice(extra);
    http(addr, "POST", &headers, Some(&message.to_string())).await
}

async fn bound(harness: &Harness, configure: impl FnOnce(&mut HttpOptions)) -> HttpServer {
    let mut options = HttpOptions {
        bind: "127.0.0.1:0".parse().unwrap(),
        ..HttpOptions::default()
    };
    configure(&mut options);
    HttpServer::bind(harness.server.clone(), options)
        .await
        .expect("bind")
}

#[tokio::test(flavor = "multi_thread")]
async fn http_sessions_follow_the_session_based_flow() {
    let harness = Harness::start(Tier::Dom).await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    assert_eq!(server.url(), format!("http://{addr}/mcp"));

    let init = post(addr, &[], json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}})).await;
    assert_eq!(init.status, 200);
    assert_eq!(init.header("content-type"), Some("application/json"));
    let session = init
        .header("mcp-session-id")
        .expect("a session id")
        .to_owned();
    assert_eq!(session.len(), 32);

    let notified = post(
        addr,
        &[("Mcp-Session-Id", &session)],
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    assert_eq!(notified.status, 202);
    assert!(notified.body.is_empty());

    let navigate = post(
        addr,
        &[("Mcp-Session-Id", &session), ("MCP-Protocol-Version", "2025-11-25")],
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "browser_navigate", "arguments": {"url": harness.url("/about.html")}}}),
    )
    .await;
    assert_eq!(navigate.status, 200);
    assert_eq!(
        navigate.json()["result"]["structuredContent"]["title"],
        "About"
    );

    // The session's page is its own: another session sees a blank page.
    let other = post(addr, &[], json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})).await;
    let other_session = other.header("mcp-session-id").unwrap().to_owned();
    assert_ne!(other_session, session);
    let blank = post(
        addr,
        &[("Mcp-Session-Id", &other_session)],
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "browser_snapshot", "arguments": {}}}),
    )
    .await;
    assert!(
        blank.json()["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("about:blank")
    );
    let same = post(
        addr,
        &[("Mcp-Session-Id", &session)],
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "browser_snapshot", "arguments": {}}}),
    )
    .await;
    assert!(
        same.json()["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("We make browsers.")
    );

    // Ending a session forgets it.
    let deleted = http(addr, "DELETE", &[("Mcp-Session-Id", &session)], None).await;
    assert_eq!(deleted.status, 200);
    let gone = post(
        addr,
        &[("Mcp-Session-Id", &session)],
        json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}),
    )
    .await;
    assert_eq!(gone.status, 404);
    assert_eq!(
        http(addr, "DELETE", &[("Mcp-Session-Id", &session)], None)
            .await
            .status,
        404
    );
    assert_eq!(http(addr, "DELETE", &[], None).await.status, 400);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn http_serves_stateless_requests_and_validates_the_mirrored_headers() {
    let harness = Harness::start(Tier::Dom).await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let modern = |method: &str, params: Value| {
        let mut params = params;
        params["_meta"] = modern_meta();
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    };
    let good: [(&str, &str); 2] = [
        ("MCP-Protocol-Version", "2026-07-28"),
        ("Mcp-Method", "tools/list"),
    ];

    let listed = post(addr, &good, modern("tools/list", json!({}))).await;
    assert_eq!(listed.status, 200);
    assert!(
        listed.header("mcp-session-id").is_none(),
        "no protocol-level sessions"
    );
    assert_eq!(listed.json()["result"]["resultType"], "complete");

    let discover = post(
        addr,
        &[
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "server/discover"),
        ],
        modern("server/discover", json!({})),
    )
    .await;
    assert_eq!(
        discover.json()["result"]["supportedVersions"][0],
        "2026-07-28"
    );

    // The stateless era is served without ever initializing; calls share a browser.
    let call_headers = [
        ("MCP-Protocol-Version", "2026-07-28"),
        ("Mcp-Method", "tools/call"),
        ("Mcp-Name", "browser_navigate"),
    ];
    let navigated = post(
        addr,
        &call_headers,
        modern(
            "tools/call",
            json!({"name": "browser_navigate", "arguments": {"url": harness.url("/about.html")}}),
        ),
    )
    .await;
    assert_eq!(navigated.status, 200);
    assert_eq!(navigated.json()["result"]["resultType"], "complete");
    assert_eq!(
        navigated.json()["result"]["structuredContent"]["status"],
        200
    );

    let mismatches: Vec<(Vec<(&str, &str)>, Value)> = vec![
        (
            vec![("Mcp-Method", "tools/list")],
            modern("tools/list", json!({})),
        ),
        (
            vec![
                ("MCP-Protocol-Version", "2025-11-25"),
                ("Mcp-Method", "tools/list"),
            ],
            modern("tools/list", json!({})),
        ),
        (
            vec![("MCP-Protocol-Version", "2026-07-28")],
            modern("tools/list", json!({})),
        ),
        (
            vec![
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", "ping"),
            ],
            modern("tools/list", json!({})),
        ),
        (
            vec![
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "browser_close"),
            ],
            modern(
                "tools/call",
                json!({"name": "browser_navigate", "arguments": {}}),
            ),
        ),
        (
            vec![
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", "tools/call"),
            ],
            modern(
                "tools/call",
                json!({"name": "browser_navigate", "arguments": {}}),
            ),
        ),
    ];
    for (headers, message) in mismatches {
        let rejected = post(addr, &headers, message).await;
        assert_eq!(rejected.status, 400, "{headers:?}");
        assert_eq!(rejected.json()["error"]["code"], -32020, "{headers:?}");
    }
    // A base64-encoded Mcp-Name is decoded before the comparison.
    let encoded = post(
        addr,
        &[
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "=?base64?YnJvd3Nlcl9jbG9zZQ==?="),
        ],
        modern(
            "tools/call",
            json!({"name": "browser_close", "arguments": {}}),
        ),
    )
    .await;
    assert_eq!(encoded.status, 200, "{}", encoded.body);

    let mut meta = modern_meta();
    meta["io.modelcontextprotocol/protocolVersion"] = json!("2031-01-01");
    let unsupported = post(
        addr,
        &[
            ("MCP-Protocol-Version", "2031-01-01"),
            ("Mcp-Method", "tools/list"),
        ],
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta}}),
    )
    .await;
    assert_eq!(unsupported.status, 400);
    assert_eq!(unsupported.json()["error"]["code"], -32022);
    assert_eq!(
        unsupported.json()["error"]["data"]["supported"][0],
        "2026-07-28"
    );

    let unknown = post(
        addr,
        &[
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "resources/list"),
        ],
        modern("resources/list", json!({})),
    )
    .await;
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.json()["error"]["code"], -32601);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn http_rejects_malformed_requests_cleanly() {
    let harness = Harness::start(Tier::Dom).await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});

    assert_eq!(http(addr, "GET", &[], None).await.status, 405);
    assert_eq!(
        http(addr, "GET", &[], None).await.header("allow"),
        Some("POST, DELETE")
    );
    let wrong_type = http(addr, "POST", &[("Content-Type", "text/plain")], Some("{}")).await;
    assert_eq!(wrong_type.status, 415);
    let broken = http(addr, "POST", &JSON_HEADERS, Some("{nope")).await;
    assert_eq!(broken.status, 400);
    assert_eq!(broken.json()["error"]["code"], -32700);
    let huge = http(
        addr,
        "POST",
        &[
            ("Content-Type", "application/json"),
            ("Content-Length", "99999999"),
        ],
        None,
    )
    .await;
    assert_eq!(huge.status, 413);
    let chunked = http(
        addr,
        "POST",
        &[
            ("Content-Type", "application/json"),
            ("Transfer-Encoding", "chunked"),
        ],
        Some("0\r\n\r\n"),
    )
    .await;
    assert_eq!(chunked.status, 411);
    let no_length = {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\r\n",
            )
            .await
            .unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).await.unwrap();
        raw
    };
    assert!(no_length.starts_with("HTTP/1.1 411"), "{no_length}");
    let elsewhere = {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                format!("POST /other HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 2\r\n\r\n{{}}")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).await.unwrap();
        raw
    };
    assert!(elsewhere.starts_with("HTTP/1.1 404"), "{elsewhere}");
    // Without a session header or a handshake, a plain request still works.
    assert_eq!(post(addr, &[], ping).await.json()["result"], json!({}));
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn http_checks_host_origin_and_token() {
    let harness = Harness::start(Tier::Dom).await;
    let token = "0123456789abcdef-token";
    let server = bound(&harness, |options| {
        options.token = Some(token.to_owned());
        options.allowed_origins = vec!["http://app.test".to_owned()];
        options.allowed_hosts = vec!["mcp.internal".to_owned()];
    })
    .await;
    let addr = server.local_addr();
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    let bearer = format!("Bearer {token}");

    // The token comes first among what a client controls.
    for headers in [
        vec![],
        vec![("Authorization", "Bearer wrong-token-0000000")],
        vec![("Authorization", token)],
        vec![("Authorization", "Basic abc")],
    ] {
        let denied = post(addr, &headers, ping.clone()).await;
        assert_eq!(denied.status, 401, "{headers:?}");
        assert_eq!(denied.header("www-authenticate"), Some("Bearer"));
    }
    let ok = post(addr, &[("Authorization", &bearer)], ping.clone()).await;
    assert_eq!(ok.status, 200);

    // A web page's Origin must be on the list, whatever it knows of the token.
    let foreign = post(
        addr,
        &[("Authorization", &bearer), ("Origin", "http://evil.test")],
        ping.clone(),
    )
    .await;
    assert_eq!(foreign.status, 403);
    assert_eq!(foreign.json()["error"]["code"], -32600);
    let allowed = post(
        addr,
        &[("Authorization", &bearer), ("Origin", "HTTP://APP.TEST")],
        ping.clone(),
    )
    .await;
    assert_eq!(allowed.status, 200);

    // DNS rebinding: a name that is neither an address, localhost nor listed.
    let rebound = post(
        addr,
        &[
            ("Authorization", &bearer),
            ("Host", "attacker.example:8931"),
        ],
        ping.clone(),
    )
    .await;
    assert_eq!(rebound.status, 403);
    for host in ["localhost:8931", "mcp.internal", "[::1]:8931"] {
        let named = post(
            addr,
            &[("Authorization", &bearer), ("Host", host)],
            ping.clone(),
        )
        .await;
        assert_eq!(named.status, 200, "{host}");
    }
    // The same rules apply to every method.
    assert_eq!(
        http(addr, "DELETE", &[("Mcp-Session-Id", "x")], None)
            .await
            .status,
        401
    );
    assert_eq!(
        http(
            addr,
            "GET",
            &[("Origin", "http://evil.test"), ("Authorization", &bearer)],
            None
        )
        .await
        .status,
        403
    );
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn http_without_a_token_still_blocks_foreign_origins() {
    let harness = Harness::start(Tier::Dom).await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    assert_eq!(post(addr, &[], ping.clone()).await.status, 200);
    assert_eq!(
        post(addr, &[("Origin", "http://localhost:3000")], ping)
            .await
            .status,
        403
    );
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_bind_needs_a_bearer_token() {
    let harness = Harness::start(Tier::Dom).await;
    for token in [
        None,
        Some("short".to_owned()),
        Some("has space in it, long enough".to_owned()),
    ] {
        let result = HttpServer::bind(
            harness.server.clone(),
            HttpOptions {
                bind: "0.0.0.0:0".parse().unwrap(),
                token,
                ..HttpOptions::default()
            },
        )
        .await;
        assert!(result.is_err());
    }
    let remote = HttpServer::bind(
        harness.server.clone(),
        HttpOptions {
            bind: "0.0.0.0:0".parse().unwrap(),
            token: Some("0123456789abcdef".to_owned()),
            ..HttpOptions::default()
        },
    )
    .await
    .expect("a long token allows a remote bind");
    remote.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_resets_the_connection_cancels_its_request() {
    hang_up(true).await;
}

/// An aborted fetch closes its socket with a plain FIN, which looks like a
/// half-close; the server tells them apart by probing the connection.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_closes_the_connection_cancels_its_request() {
    hang_up(false).await;
}

async fn hang_up(with_reset: bool) {
    let harness = Harness::start(Tier::Dom).await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let message = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "browser_wait_for", "arguments": {"text": "never", "timeout_ms": 60000}}})
    .to_string();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            format!(
                "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{message}",
                message.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    if with_reset {
        reset(stream);
    } else {
        drop(stream);
    }
    // The browser the abandoned call held is free again for the next call.
    let next = tokio::time::timeout(
        Duration::from_secs(20),
        post(
            addr,
            &[],
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "browser_snapshot", "arguments": {}}}),
        ),
    )
    .await
    .expect("the cancelled call released the session");
    assert_eq!(next.status, 200);
    server.shutdown().await;
}

// ---- scope, hidden-ness, batches -----------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn snapshots_can_be_scoped_to_a_ref_or_a_selector() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/index.html")).await;
    let full = client.tool("browser_snapshot", json!({})).await.text();
    let main = find_ref(&full, &["- main"]);

    for arguments in [json!({"selector": "main"}), json!({"ref": main})] {
        let scoped = client.tool("browser_snapshot", arguments).await;
        scoped.ok();
        let text = scoped.text();
        assert!(
            text.contains("heading \"Welcome to the fixture\""),
            "{text}"
        );
        assert!(
            !text.contains("Contact form"),
            "the navigation is outside the scope:\n{text}"
        );
        assert!(text.len() < full.len());
    }
    let both = client
        .tool("browser_snapshot", json!({"selector": "main", "ref": main}))
        .await;
    assert_eq!(both.error_type(), "invalid_arguments");
    let none = client
        .tool("browser_snapshot", json!({"selector": "#absent"}))
        .await;
    assert_eq!(none.error_type(), "element_not_found");
    let bad = client
        .tool("browser_snapshot", json!({"selector": "div["}))
        .await;
    assert_eq!(bad.error_type(), "invalid_arguments");
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_visibility_is_judged_the_same_with_and_without_layout() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/vis.html")).await;
    let text = client.tool("browser_snapshot", json!({})).await.text();
    assert!(text.contains("Visible text"), "{text}");
    assert!(
        text.contains("Shown child"),
        "a child may override visibility: {text}"
    );
    assert!(
        !text.contains("Invisible inline") && !text.contains("Hidden sibling"),
        "{text}"
    );
    assert!(
        text.contains("Ghost inline"),
        "opacity 0 stays in the tree: {text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn batches_exist_only_in_the_two_oldest_revisions() {
    let harness = Harness::start(Tier::Dom).await;
    let batch = || {
        json!([
            {"jsonrpc": "2.0", "id": 1, "method": "ping"},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        ])
    };
    for (version, allowed) in [
        ("2024-11-05", true),
        ("2025-03-26", true),
        ("2025-06-18", false),
        ("2025-11-25", false),
    ] {
        let client = harness.client();
        client
            .result("initialize", json!({"protocolVersion": version}))
            .await;
        let reply = client.connection.handle(batch()).await.expect("a reply");
        if allowed {
            let replies = reply
                .as_array()
                .unwrap_or_else(|| panic!("an array, got {reply}"));
            assert_eq!(replies.len(), 2, "the notification has no reply: {reply}");
            assert_eq!(replies[0]["id"], 1);
            assert_eq!(
                replies[1]["result"]["tools"].as_array().unwrap().len(),
                TOOL_NAMES.len()
            );
        } else {
            assert_eq!(reply["error"]["code"], -32600, "{version}: {reply}");
        }
    }
    let client = harness.client();
    client
        .result("initialize", json!({"protocolVersion": "2025-03-26"}))
        .await;
    let empty = client.connection.handle(json!([])).await.unwrap();
    assert_eq!(empty["error"]["code"], -32600);
    let only_notifications = client
        .connection
        .handle(json!([{"jsonrpc": "2.0", "method": "notifications/initialized"}]))
        .await;
    assert!(only_notifications.is_none());
}

// ---- HTTP sessions under pressure ----------------------------------------

/// Closes the socket with a reset (zero linger sends RST, and does not block).
#[allow(deprecated)]
fn reset(stream: TcpStream) {
    stream.set_linger(Some(Duration::ZERO)).unwrap();
    drop(stream);
}

async fn initialize(addr: std::net::SocketAddr) -> Response {
    post(
        addr,
        &[],
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}}),
    )
    .await
}

fn session_of(response: &Response) -> String {
    assert_eq!(response.status, 200, "{}", response.body);
    response
        .header("mcp-session-id")
        .expect("a session id")
        .to_owned()
}

async fn alive(addr: std::net::SocketAddr, session: &str) -> bool {
    post(
        addr,
        &[("Mcp-Session-Id", session)],
        json!({"jsonrpc": "2.0", "id": 9, "method": "ping"}),
    )
    .await
    .status
        == 200
}

fn wait_forever(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
        "name": "browser_wait_for", "arguments": {"text": "never", "timeout_ms": 20000}}})
}

fn snapshot_call(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
        "name": "browser_snapshot", "arguments": {}}})
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_server_refuses_new_sessions_instead_of_dropping_working_ones() {
    let harness = Harness::with_options(Tier::Dom, |options| {
        options.max_sessions = 3;
        options.unused_session_ttl = Duration::from_millis(300);
    })
    .await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let working = session_of(&initialize(addr).await);
    post(addr, &[("Mcp-Session-Id", &working)], snapshot_call(2)).await;
    let idle_a = session_of(&initialize(addr).await);
    let idle_b = session_of(&initialize(addr).await);
    // Full of sessions that are fresh: the answer is 503, nobody is dropped.
    assert_eq!(initialize(addr).await.status, 503);
    assert!(alive(addr, &working).await && alive(addr, &idle_a).await);
    // A client between two calls keeps its session however long the pause.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let fresh = session_of(&initialize(addr).await);
    assert!(
        alive(addr, &working).await,
        "a session that made a call stays"
    );
    assert!(
        !alive(addr, &idle_a).await && !alive(addr, &idle_b).await,
        "sessions that never called a tool expired and made room"
    );
    assert!(alive(addr, &fresh).await);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_with_a_call_in_flight_is_never_dropped() {
    let harness = Harness::with_options(Tier::Dom, |options| {
        options.max_sessions = 2;
        options.unused_session_ttl = Duration::from_millis(200);
    })
    .await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let busy = session_of(&initialize(addr).await);
    let waiting = tokio::spawn({
        let busy = busy.clone();
        async move { post(addr, &[("Mcp-Session-Id", &busy)], wait_forever(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(600)).await;
    // `busy` is past the unused TTL by age, yet it has a call running.
    let other = session_of(&initialize(addr).await);
    assert!(alive(addr, &busy).await);
    assert!(alive(addr, &other).await);
    // Ending the session cancels its call.
    assert!(server_close(addr, &busy).await);
    let cancelled = waiting.await.unwrap();
    assert_eq!(cancelled.json()["error"]["code"], -32800);
    server.shutdown().await;
}

async fn server_close(addr: std::net::SocketAddr, session: &str) -> bool {
    http(addr, "DELETE", &[("Mcp-Session-Id", session)], None)
        .await
        .status
        == 200
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_that_never_call_a_tool_expire_quickly_and_each_source_has_a_cap() {
    let harness = Harness::with_options(Tier::Dom, |options| {
        options.max_sessions = 40;
        options.unused_session_ttl = Duration::from_millis(150);
    })
    .await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let used = session_of(&initialize(addr).await);
    let unused = session_of(&initialize(addr).await);
    post(addr, &[("Mcp-Session-Id", &used)], snapshot_call(2)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    session_of(&initialize(addr).await); // any open prunes expired sessions
    assert!(
        alive(addr, &used).await,
        "a session that made a call lives on"
    );
    assert!(!alive(addr, &unused).await, "an unused one expired");

    // One address (here, loopback) holds at most MAX_SESSIONS_PER_SOURCE.
    let cap = tenta_mcp::server::MAX_SESSIONS_PER_SOURCE;
    // Let the stray session opened above expire so only working ones count.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut ids = vec![used];
    while ids.len() < cap {
        let id = session_of(&initialize(addr).await);
        post(addr, &[("Mcp-Session-Id", &id)], snapshot_call(3)).await;
        ids.push(id);
    }
    // The source is at its cap and every session of it is working: refused.
    assert_eq!(initialize(addr).await.status, 503);
    for id in &ids {
        assert!(alive(addr, id).await);
    }
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancellation_notification_stops_a_running_call_over_http() {
    let harness = Harness::start(Tier::Dom).await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let session = session_of(&initialize(addr).await);
    let started = std::time::Instant::now();
    let long = tokio::spawn({
        let session = session.clone();
        async move { post(addr, &[("Mcp-Session-Id", &session)], wait_forever(7)).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let cancel = post(
        addr,
        &[("Mcp-Session-Id", &session)],
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7}}),
    )
    .await;
    assert_eq!(cancel.status, 202);
    let cancelled = long.await.unwrap();
    assert_eq!(
        cancelled.json()["error"]["code"],
        -32800,
        "{}",
        cancelled.body
    );
    // The browser is free again: the next call does not wait out the 20 s.
    let next = post(addr, &[("Mcp-Session-Id", &session)], snapshot_call(8)).await;
    assert_eq!(next.status, 200);
    assert!(next.json()["result"]["isError"].is_null());
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    // Cancelling an id that is not running is harmless.
    let late = post(
        addr,
        &[("Mcp-Session-Id", &session)],
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7}}),
    )
    .await;
    assert_eq!(late.status, 202);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_half_closed_request_is_still_answered() {
    let harness = Harness::start(Tier::Dom).await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "browser_navigate", "arguments": {"url": harness.url("/about.html")}}})
    .to_string();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            format!(
                "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.shutdown().await.unwrap(); // done writing, still reading
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    assert!(raw.contains("\"title\":\"About\""), "{raw}");
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_session_header_is_ignored_by_stateless_requests() {
    let harness = Harness::start(Tier::Dom).await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let headers = [
        ("MCP-Protocol-Version", "2026-07-28"),
        ("Mcp-Method", "tools/list"),
        ("Mcp-Session-Id", "0123456789abcdef"),
    ];
    let mut params = json!({});
    params["_meta"] = modern_meta();
    let stateless = post(
        addr,
        &headers,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": params}),
    )
    .await;
    assert_eq!(stateless.status, 200, "{}", stateless.body);
    // The session-based era still treats an unknown id as an error.
    let legacy = post(
        addr,
        &[("Mcp-Session-Id", "0123456789abcdef")],
        json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
    )
    .await;
    assert_eq!(legacy.status, 404);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_host_header_must_be_present_once() {
    let harness = Harness::start(Tier::Dom).await;
    let server = bound(&harness, |_| {}).await;
    let addr = server.local_addr();
    let raw = |request: String| async move {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut out = String::new();
        stream.read_to_string(&mut out).await.unwrap();
        out
    };
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
    let head = |host: &str| {
        format!(
            "POST /mcp HTTP/1.1\r\n{host}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    assert!(
        raw(head("")).await.starts_with("HTTP/1.1 400"),
        "HTTP/1.1 needs a Host"
    );
    assert!(
        raw(head("Host: 127.0.0.1\r\nHost: evil.example\r\n"))
            .await
            .starts_with("HTTP/1.1 400"),
        "two Hosts"
    );
    assert!(
        raw(head("Host: 127.0.0.1\r\n"))
            .await
            .starts_with("HTTP/1.1 200")
    );
    let http10 = format!(
        "POST /mcp HTTP/1.0\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    assert!(
        raw(http10).await.starts_with("HTTP/1.1 200"),
        "HTTP/1.0 may omit Host"
    );
    server.shutdown().await;
}
