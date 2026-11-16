//! The reading tools without a script engine: `browser_extract`, the session
//! history (`browser_back`, `browser_forward`, `browser_reload`) and
//! `browser_network_requests`.

mod common;

use common::Harness;
use serde_json::json;
use tenta_core::Tier;

/// The page's own text of a fenced result, without the marker lines.
fn inside_fence(text: &str) -> String {
    let mut lines = text.lines();
    let first = lines.next().expect("a first line");
    assert!(first.starts_with("<<<PAGE CONTENT "), "{text}");
    let nonce = first["<<<PAGE CONTENT ".len()..]
        .split(':')
        .next()
        .expect("a nonce");
    let end = format!("<<<END PAGE CONTENT {nonce}>>>");
    let body: Vec<&str> = lines.take_while(|line| *line != end).collect();
    assert!(text.contains(&end), "the fence is closed: {text}");
    body.join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn extract_returns_text_markdown_links_html_metadata_and_forms() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/article.html")).await;

    let text = client.tool("browser_extract", json!({})).await;
    text.ok();
    let body = inside_fence(&text.text());
    assert!(body.contains("Page Title: Fixture Article"), "{body}");
    assert!(body.contains("Reading pages as text"), "{body}");
    assert!(
        body.contains("Footer boilerplate"),
        "the whole page: {body}"
    );
    assert!(!body.contains("Hidden paragraph"), "{body}");
    assert_eq!(text.structured()["format"], "text");
    assert_eq!(text.structured()["main_content"], false);
    assert_eq!(text.structured()["continuation"], json!(null));

    let article = client
        .tool(
            "browser_extract",
            json!({"format": "markdown", "main_content": true}),
        )
        .await;
    article.ok();
    let body = inside_fence(&article.text());
    assert!(body.contains("# Reading pages as text"), "{body}");
    assert!(body.contains("## What is kept"), "{body}");
    assert!(body.contains("[links]("), "{body}");
    assert!(body.contains("**emphasis**"), "{body}");
    assert!(
        !body.contains("Footer boilerplate"),
        "only the article: {body}"
    );
    assert!(!body.contains("About us"), "no navigation: {body}");
    assert_eq!(article.structured()["main_content"], true);

    let links = client
        .tool("browser_extract", json!({"format": "links"}))
        .await;
    links.ok();
    let body = inside_fence(&links.text());
    assert!(
        body.contains(&format!("- [Home]({})", harness.url("/index.html"))),
        "absolute URLs: {body}"
    );
    assert!(
        body.contains("- [Terms (legal)](https://example.com/terms)"),
        "brackets cannot end the link text: {body}"
    );
    assert_eq!(
        links.structured()["items"],
        3,
        "the two links to /about.html are listed once"
    );

    let scoped = client
        .tool(
            "browser_extract",
            json!({"format": "links", "selector": "nav"}),
        )
        .await;
    scoped.ok();
    assert_eq!(scoped.structured()["items"], 2);

    let html = client
        .tool(
            "browser_extract",
            json!({"format": "html", "selector": "ul"}),
        )
        .await;
    html.ok();
    assert!(
        inside_fence(&html.text()).contains("<ul><li>First point</li><li>Second point</li></ul>"),
        "{}",
        html.text()
    );

    let metadata = client
        .tool("browser_extract", json!({"format": "metadata"}))
        .await;
    metadata.ok();
    let body = inside_fence(&metadata.text());
    assert!(body.contains("\"title\": \"Fixture Article\""), "{body}");
    assert!(
        body.contains("An article for the extraction tests."),
        "{body}"
    );

    let forms = client
        .tool("browser_extract", json!({"format": "forms"}))
        .await;
    forms.ok();
    assert_eq!(forms.structured()["items"], 1);
    assert!(
        inside_fence(&forms.text()).contains("\"email\""),
        "{}",
        forms.text()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn extract_arguments_are_checked_and_a_page_without_an_article_says_so() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/about.html")).await;

    let whole = client
        .tool("browser_extract", json!({"main_content": true}))
        .await;
    whole.ok();
    assert_eq!(whole.structured()["main_content"], false);
    assert!(
        whole
            .text()
            .contains("no element that reads as a main article"),
        "{}",
        whole.text()
    );
    assert!(inside_fence(&whole.text()).contains("We make browsers."));

    for (arguments, kind) in [
        (
            json!({"format": "links", "main_content": true}),
            "invalid_arguments",
        ),
        (
            json!({"format": "metadata", "selector": "h1"}),
            "invalid_arguments",
        ),
        (json!({"selector": "p["}), "invalid_arguments"),
        (json!({"selector": "#absent"}), "element_not_found"),
        (json!({"ref": "e1", "selector": "h1"}), "invalid_arguments"),
        (json!({"continuation": "x999:0"}), "invalid_arguments"),
        (json!({"format": "pdf"}), "invalid_arguments"),
    ] {
        let result = client.tool("browser_extract", arguments.clone()).await;
        assert_eq!(result.error_type(), kind, "{arguments}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn long_extractions_continue_from_the_text_first_returned() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/long.html")).await;

    let whole = client
        .tool("browser_extract", json!({"max_chars": 200000}))
        .await;
    whole.ok();
    let expected = inside_fence(&whole.text());
    let total = whole.structured()["chars_total"].as_u64().unwrap();
    assert!(total > 3000, "the fixture is long enough to cut: {total}");

    let mut collected = String::new();
    let mut arguments = json!({"max_chars": 1000});
    let mut parts = 0;
    loop {
        let part = client.tool("browser_extract", arguments).await;
        part.ok();
        parts += 1;
        assert!(part.structured()["chars_returned"].as_u64().unwrap() <= 1000);
        assert_eq!(part.structured()["chars_total"], total);
        let body = inside_fence(&part.text());
        // The fence adds a line break after a part that does not end in one.
        collected.push_str(&body);
        collected.push('\n');
        match part.structured()["continuation"].as_str() {
            Some(token) => {
                assert!(part.text().contains(token), "the text names the token");
                arguments = json!({"continuation": token, "max_chars": 1000});
            }
            None => break,
        }
        assert!(parts < 200, "the continuation makes progress");
    }
    assert!(parts > 3, "{parts}");
    let squeeze = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(squeeze(&collected), squeeze(&expected));

    // A token of an extraction that has been replaced is refused.
    let first = client
        .tool("browser_extract", json!({"max_chars": 1000}))
        .await;
    let stale = first.structured()["continuation"]
        .as_str()
        .unwrap()
        .to_owned();
    client
        .tool("browser_extract", json!({"max_chars": 1000}))
        .await
        .ok();
    let refused = client
        .tool("browser_extract", json!({"continuation": stale}))
        .await;
    assert_eq!(refused.error_type(), "invalid_arguments");
}

#[tokio::test(flavor = "multi_thread")]
async fn back_forward_and_reload_walk_the_session_history() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();

    for tool in ["browser_back", "browser_forward", "browser_reload"] {
        let result = client.tool(tool, json!({})).await;
        assert_eq!(
            result.error_type(),
            "invalid_arguments",
            "{tool} with no history"
        );
    }

    let first = client.navigate(&harness.url("/index.html")).await;
    assert_eq!(
        first.structured()["history"],
        json!({"back": 0, "forward": 0})
    );
    let second = client.navigate(&harness.url("/about.html")).await;
    assert_eq!(
        second.structured()["history"],
        json!({"back": 1, "forward": 0})
    );
    // Loading the page that is shown adds no entry.
    let again = client.navigate(&harness.url("/about.html")).await;
    assert_eq!(
        again.structured()["history"],
        json!({"back": 1, "forward": 0})
    );

    let back = client.tool("browser_back", json!({})).await;
    back.ok();
    assert!(back.text().starts_with("Went back to "), "{}", back.text());
    assert_eq!(back.structured()["title"], "Fixture Home");
    assert_eq!(
        back.structured()["history"],
        json!({"back": 0, "forward": 1})
    );
    assert_eq!(
        client.tool("browser_back", json!({})).await.error_type(),
        "invalid_arguments"
    );

    let forward = client.tool("browser_forward", json!({})).await;
    forward.ok();
    assert_eq!(forward.structured()["title"], "About");
    assert_eq!(
        forward.structured()["history"],
        json!({"back": 1, "forward": 0})
    );

    let reloaded = client
        .tool("browser_reload", json!({"wait_until": "load"}))
        .await;
    reloaded.ok();
    assert!(
        reloaded.text().starts_with("Reloaded "),
        "{}",
        reloaded.text()
    );
    assert_eq!(reloaded.structured()["title"], "About");
    assert_eq!(
        reloaded.structured()["history"],
        json!({"back": 1, "forward": 0})
    );

    // A new navigation after going back drops the entries ahead.
    client.tool("browser_back", json!({})).await.ok();
    let third = client.navigate(&harness.url("/article.html")).await;
    assert_eq!(
        third.structured()["history"],
        json!({"back": 1, "forward": 0})
    );
    assert_eq!(
        client.tool("browser_forward", json!({})).await.error_type(),
        "invalid_arguments"
    );

    // A closed page starts a new history.
    client.tool("browser_close", json!({})).await.ok();
    assert_eq!(
        client.tool("browser_back", json!({})).await.error_type(),
        "invalid_arguments"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_load_leaves_the_history_where_it_was() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/index.html")).await;
    client.navigate(&harness.url("/about.html")).await;

    let failed = client
        .tool("browser_navigate", json!({"url": "http://127.0.0.1:1/"}))
        .await;
    assert_eq!(failed.error_type(), "navigation_failed");

    let back = client.tool("browser_back", json!({})).await;
    back.ok();
    assert_eq!(back.structured()["title"], "Fixture Home");
}

#[tokio::test(flavor = "multi_thread")]
async fn network_requests_list_the_current_page_and_filter() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    client.navigate(&harness.url("/index.html")).await;

    let all = client.tool("browser_network_requests", json!({})).await;
    all.ok();
    let requests = all.structured()["requests"].as_array().unwrap().clone();
    let document = requests
        .iter()
        .find(|request| request["url"] == harness.url("/index.html"))
        .unwrap_or_else(|| panic!("the document request is listed: {requests:?}"));
    assert_eq!(document["method"], "GET");
    assert_eq!(document["status"], 200);
    assert_eq!(document["resource_type"], "document");
    assert!(
        inside_fence(&all.text())
            .contains(&format!("GET 200 document {}", harness.url("/index.html"))),
        "{}",
        all.text()
    );
    assert_eq!(all.structured()["failed"], 0);

    let none = client
        .tool("browser_network_requests", json!({"failed_only": true}))
        .await;
    none.ok();
    assert_eq!(none.structured()["returned"], 0);
    assert!(
        none.text().contains("No requests match."),
        "{}",
        none.text()
    );

    // A navigation starts the list over: the earlier document is gone.
    client.navigate(&harness.url("/missing.html")).await;
    let failed = client
        .tool("browser_network_requests", json!({"failed_only": true}))
        .await;
    failed.ok();
    let requests = failed.structured()["requests"].as_array().unwrap().clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0]["status"], 404);
    let filtered = client
        .tool(
            "browser_network_requests",
            json!({"url_contains": "index.html"}),
        )
        .await;
    filtered.ok();
    assert_eq!(filtered.structured()["returned"], 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn web_search_needs_a_search_subsystem_from_the_host() {
    let harness = Harness::start(Tier::Dom).await;
    let client = harness.client();
    let refused = client
        .tool("browser_web_search", json!({"query": "rust"}))
        .await;
    assert_eq!(refused.error_type(), "unsupported");
    let invalid = client
        .tool("browser_web_search", json!({"query": ""}))
        .await;
    assert_eq!(invalid.error_type(), "invalid_arguments");
}
