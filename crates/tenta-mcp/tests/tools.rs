//! The acting tools against fixture pages, with V8 and no renderer: typing,
//! filling, key presses, clicks by ref, evaluation and waiting for content
//! that timers add.

mod common;

use common::{Harness, find_ref};
use serde_json::json;
use tenta_core::Tier;

async fn form() -> (Harness, common::Client) {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/form.html")).await;
    (harness, client)
}

#[tokio::test(flavor = "multi_thread")]
async fn fill_and_type_edit_a_field_through_key_and_input_events() {
    let (_harness, client) = form().await;
    let name = client.ref_of(&["textbox", "Your name"]).await;

    let filled = client
        .tool("browser_fill", json!({"ref": name, "value": "Ada"}))
        .await;
    filled.ok();
    assert_eq!(filled.structured()["value"], "Ada");
    assert_eq!(filled.structured()["settled"]["timed_out"], false);
    assert_eq!(
        client
            .evaluate("document.getElementById('name').value")
            .await,
        "Ada"
    );
    assert_eq!(
        client.evaluate("document.activeElement.id").await,
        "name",
        "fill focuses the field"
    );

    let typed = client
        .tool("browser_type", json!({"ref": name, "text": " Lovelace"}))
        .await;
    typed.ok();
    assert_eq!(
        typed.structured()["value"],
        "Ada Lovelace",
        "typing appends at the caret"
    );
    assert_eq!(typed.structured()["characters"], 9);
    let log = client.evaluate("window.log.join(',')").await;
    let log = log.as_str().unwrap();
    for event in [
        "input:Ada",
        "keydown:L",
        "keyup:L",
        "keydown: ",
        "input:Ada Lovelace",
    ] {
        assert!(log.contains(event), "{event} missing from {log}");
    }

    // The snapshot shows the typed value, not the attribute.
    let snapshot = client.tool("browser_snapshot", json!({})).await.text();
    assert!(snapshot.contains("value=\"Ada Lovelace\""), "{snapshot}");

    let replaced = client
        .tool("browser_fill", json!({"ref": name, "value": "Grace"}))
        .await;
    assert_eq!(
        replaced.structured()["value"],
        "Grace",
        "fill replaces, it does not append"
    );
    let cleared = client
        .tool("browser_fill", json!({"ref": name, "value": ""}))
        .await;
    cleared.ok();
    assert_eq!(
        client
            .evaluate("document.getElementById('name').value")
            .await,
        ""
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn passwords_are_never_echoed() {
    let (_harness, client) = form().await;
    let secret = client.ref_of(&["textbox", "Secret"]).await;
    let filled = client
        .tool("browser_fill", json!({"ref": secret, "value": "hunter2"}))
        .await;
    filled.ok();
    assert!(filled.structured()["value"].is_null(), "{}", filled.0);
    assert!(!filled.text().contains("hunter2"));
    let snapshot = client.tool("browser_snapshot", json!({})).await.text();
    assert!(snapshot.contains("value=\"********\""), "{snapshot}");
    assert!(!snapshot.contains("hunter2"));
    assert_eq!(
        client
            .evaluate("document.getElementById('secret').value")
            .await,
        "hunter2"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn enter_submits_the_form_and_the_tool_follows_the_navigation() {
    let (harness, client) = form().await;
    let name = client.ref_of(&["textbox", "Your name"]).await;
    client
        .tool(
            "browser_fill",
            json!({"ref": name, "value": "Ada Lovelace"}),
        )
        .await
        .ok();
    let pressed = client
        .tool("browser_press_key", json!({"key": "Enter", "ref": name}))
        .await;
    pressed.ok();
    let navigation = &pressed.structured()["navigation"];
    assert_eq!(navigation["status"], 200, "{}", pressed.0);
    let url = navigation["url"].as_str().unwrap();
    assert!(url.starts_with(&harness.url("/result.html?")), "{url}");
    assert!(
        url.contains("name=Ada+Lovelace") || url.contains("name=Ada%20Lovelace"),
        "{url}"
    );
    let snapshot = client.tool("browser_snapshot", json!({})).await.text();
    assert!(snapshot.contains("heading \"Submitted\""), "{snapshot}");

    // `submit: true` on type does the same from the typing tool.
    client.navigate(&harness.url("/form.html")).await;
    let name = client.ref_of(&["textbox", "Your name"]).await;
    let typed = client
        .tool(
            "browser_type",
            json!({"ref": name, "text": "Grace", "submit": true}),
        )
        .await;
    typed.ok();
    assert!(
        typed.structured()["navigation"]["url"]
            .as_str()
            .unwrap()
            .contains("name=Grace")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn click_runs_page_handlers_toggles_controls_and_follows_links() {
    let (harness, client) = form().await;
    let name = client.ref_of(&["textbox", "Your name"]).await;
    client
        .tool("browser_fill", json!({"ref": name, "value": "Ada"}))
        .await
        .ok();
    let send = client.ref_of(&["button", "Greet"]).await;
    let clicked = client.tool("browser_click", json!({"ref": send})).await;
    clicked.ok();
    assert!(
        clicked.text().contains("Clicked") && clicked.text().contains("<button#send>"),
        "{}",
        clicked.text()
    );
    assert_eq!(
        client
            .evaluate("document.getElementById('out').textContent")
            .await,
        "Hello, Ada!"
    );

    let agree = client.ref_of(&["checkbox", "Agree to terms"]).await;
    client
        .tool("browser_click", json!({"ref": agree}))
        .await
        .ok();
    assert_eq!(
        client
            .evaluate("document.getElementById('agree').checked")
            .await,
        true
    );
    assert!(
        client
            .tool("browser_snapshot", json!({}))
            .await
            .text()
            .contains("checkbox \"Agree to terms\"")
    );
    let state = client.tool("browser_snapshot", json!({})).await.text();
    let line = state
        .lines()
        .find(|line| line.contains("Agree to terms"))
        .unwrap();
    assert!(line.contains("[checked]"), "{line}");
    client
        .tool("browser_click", json!({"ref": agree}))
        .await
        .ok();
    assert_eq!(
        client
            .evaluate("document.getElementById('agree').checked")
            .await,
        false
    );

    // A link click navigates, waits for the new page and reports it.
    client.navigate(&harness.url("/index.html")).await;
    let about = client.ref_of(&["link", "About us"]).await;
    let followed = client.tool("browser_click", json!({"ref": about})).await;
    followed.ok();
    assert_eq!(
        followed.structured()["navigation"]["url"],
        harness.url("/about.html")
    );
    assert!(
        followed.text().contains("Navigated to"),
        "{}",
        followed.text()
    );
    assert!(
        client
            .tool("browser_snapshot", json!({}))
            .await
            .text()
            .contains("heading \"About us\"")
    );

    // Right clicks and double clicks reach the page.
    client.evaluate("window.seen = []; ['contextmenu','dblclick','click'].forEach(t => document.addEventListener(t, e => seen.push(t + e.detail)))").await;
    let home = client.ref_of(&["link", "Home"]).await;
    client
        .tool(
            "browser_click",
            json!({"ref": home, "button": "right", "settle": false}),
        )
        .await
        .ok();
    assert!(
        client
            .evaluate("seen.join()")
            .await
            .as_str()
            .unwrap()
            .contains("contextmenu")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn double_click_and_settle_opt_out() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/index.html")).await;
    client.evaluate("window.seen = []; ['dblclick','click'].forEach(t => document.addEventListener(t, e => seen.push(t + e.detail)))").await;
    let toggle = client.ref_of(&["button", "Toggle details"]).await;
    let clicked = client
        .tool(
            "browser_click",
            json!({"ref": toggle, "double": true, "settle": false}),
        )
        .await;
    clicked.ok();
    assert!(clicked.structured()["settled"].is_null(), "{}", clicked.0);
    assert!(clicked.text().starts_with("Double-clicked"));
    assert_eq!(
        client.evaluate("seen.join()").await,
        "click1,click2,dblclick2"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_refs_and_wrong_targets_are_typed_errors() {
    let (_harness, client) = form().await;
    let send = client.ref_of(&["button", "Greet"]).await;
    client
        .evaluate("document.getElementById('send').remove()")
        .await;
    let stale = client.tool("browser_click", json!({"ref": send})).await;
    assert_eq!(stale.error_type(), "element_not_found", "{}", stale.0);

    let agree = client.ref_of(&["checkbox", "Agree"]).await;
    let typed = client
        .tool("browser_type", json!({"ref": agree, "text": "x"}))
        .await;
    assert_eq!(typed.error_type(), "invalid_arguments");
    assert!(typed.text().contains("browser_click"), "{}", typed.text());
    let filled = client
        .tool("browser_fill", json!({"ref": "e1", "value": "x"}))
        .await;
    assert!(filled.is_error());
    let key = client
        .tool("browser_press_key", json!({"key": "NotAKey"}))
        .await;
    assert_eq!(key.error_type(), "invalid_arguments");
    // No coordinates without a layout.
    let point = client.tool("browser_click", json!({"x": 1, "y": 1})).await;
    assert_eq!(point.error_type(), "tier_denied");
}

#[tokio::test(flavor = "multi_thread")]
async fn tab_moves_focus_between_fields() {
    let (_harness, client) = form().await;
    let name = client.ref_of(&["textbox", "Your name"]).await;
    client
        .tool("browser_press_key", json!({"key": "a", "ref": name}))
        .await
        .ok();
    assert_eq!(
        client
            .evaluate("document.getElementById('name').value")
            .await,
        "a"
    );
    client
        .tool("browser_press_key", json!({"key": "Tab"}))
        .await
        .ok();
    assert_eq!(client.evaluate("document.activeElement.id").await, "agree");
    client
        .tool("browser_press_key", json!({"key": "Shift+Tab"}))
        .await
        .ok();
    assert_eq!(client.evaluate("document.activeElement.id").await, "name");
}

#[tokio::test(flavor = "multi_thread")]
async fn evaluate_returns_json_and_reports_failures() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/index.html")).await;
    assert_eq!(client.evaluate("document.title").await, "Fixture Home");
    assert_eq!(client.evaluate("1 + 2").await, 3);
    assert_eq!(
        client.evaluate("({a: [1, 'x', null], b: {c: true}})").await,
        json!({"a": [1, "x", null], "b": {"c": true}})
    );
    assert!(client.evaluate("undefined").await.is_null());
    assert_eq!(client.evaluate("NaN").await, "NaN");
    assert_eq!(client.evaluate("10n").await, "10n");
    assert_eq!(client.evaluate("Promise.resolve(7)").await, 7);
    assert_eq!(
        client
            .evaluate("new Promise(r => setTimeout(() => r('later'), 50))")
            .await,
        "later"
    );

    let text = client
        .tool("browser_evaluate", json!({"expression": "'plain text'"}))
        .await;
    assert_eq!(
        text.text(),
        "plain text",
        "strings are returned raw in the summary"
    );

    let threw = client
        .tool(
            "browser_evaluate",
            json!({"expression": "(() => { throw new Error('boom'); })()"}),
        )
        .await;
    assert_eq!(threw.error_type(), "script_error");
    assert!(threw.text().contains("boom"), "{}", threw.text());
    let syntax = client
        .tool("browser_evaluate", json!({"expression": "var = ;"}))
        .await;
    assert_eq!(syntax.error_type(), "script_error");
    let rejected = client
        .tool(
            "browser_evaluate",
            json!({"expression": "Promise.reject(new Error('nope'))"}),
        )
        .await;
    assert_eq!(rejected.error_type(), "script_error");
    assert!(rejected.text().contains("nope"));

    let raw = client
        .tool(
            "browser_evaluate",
            json!({"expression": "Promise.resolve(1)", "await_promise": false}),
        )
        .await;
    raw.ok();
    assert_ne!(
        raw.structured()["result"],
        1,
        "an unawaited promise is not its value"
    );

    let huge = client
        .tool(
            "browser_evaluate",
            json!({"expression": "'x'.repeat(70000)"}),
        )
        .await;
    huge.ok();
    assert_eq!(huge.structured()["truncated"], true);
    assert!(huge.structured()["result"].is_null());
    assert!(huge.text().ends_with("[output cut at 60000 characters]"));

    let runaway = client
        .tool(
            "browser_evaluate",
            json!({"expression": "while (true) {}", "timeout_ms": 300}),
        )
        .await;
    assert_eq!(runaway.error_type(), "timeout", "{}", runaway.0);
    // The page is usable afterwards.
    assert_eq!(client.evaluate("1").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_for_sees_content_a_timer_adds_after_the_click_settled() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/dynamic.html")).await;
    let start = client.ref_of(&["button", "Start"]).await;
    client
        .tool("browser_click", json!({"ref": start}))
        .await
        .ok();
    let waited = client
        .tool(
            "browser_wait_for",
            json!({"text": "Loaded later", "timeout_ms": 10000}),
        )
        .await;
    waited.ok();
    let by_selector = client
        .tool("browser_wait_for", json!({"selector": "#late"}))
        .await;
    by_selector.ok();
    let snapshot = client.tool("browser_snapshot", json!({})).await.text();
    assert!(
        snapshot.contains("Loaded later") && snapshot.contains("done"),
        "{snapshot}"
    );

    let gone = client
        .tool(
            "browser_wait_for",
            json!({"text_gone": "waiting", "timeout_ms": 2000}),
        )
        .await;
    gone.ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn refs_identify_the_same_element_across_snapshots_and_edits() {
    let (_harness, client) = form().await;
    let before = client.ref_of(&["button", "Greet"]).await;
    client
        .evaluate("document.getElementById('out').textContent = 'changed'")
        .await;
    let after = client.ref_of(&["button", "Greet"]).await;
    assert_eq!(before, after);
    let snapshot = client.tool("browser_snapshot", json!({})).await.text();
    assert!(find_ref(&snapshot, &["button", "Submit"]).starts_with('e'));
}

/// The tools call these members of the CDP host's page-side script; a rework
/// of that script that drops or reshapes one must fail here.
#[tokio::test(flavor = "multi_thread")]
async fn the_page_side_script_keeps_the_contract_the_tools_use() {
    let (_harness, client) = form().await;
    let members = client
        .evaluate(
            "['focus','mouse','key','insertText','nodeAt'].map(n => n + ':' + typeof __tentaCdp[n]).join()",
        )
        .await;
    assert_eq!(
        members,
        "focus:function,mouse:function,key:function,insertText:function,nodeAt:function"
    );
    // `mouse` and `key` report {navigate, unsupported} and key events edit the focused field.
    let report = client
        .evaluate(
            "var f = document.getElementById('name'); __tentaCdp.focus(f); \
             var r = __tentaCdp.key('keyDown', {key:'x', code:'KeyX', keyCode:88, text:'x'}); \
             JSON.stringify([Object.keys(r).sort(), f.value, document.activeElement === f])",
        )
        .await;
    assert_eq!(report, "[[\"navigate\",\"unsupported\"],\"x\",true]");
}
