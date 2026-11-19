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

#[tokio::test(flavor = "multi_thread")]
async fn console_messages_carry_levels_exceptions_and_locations() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/console.html")).await;

    let all = client.tool("browser_console_messages", json!({})).await;
    all.ok();
    let messages = all.structured()["messages"].as_array().unwrap().clone();
    let line = |level: &str| {
        messages
            .iter()
            .find(|message| message["level"] == level)
            .unwrap_or_else(|| panic!("a `{level}` line in {messages:?}"))
            .clone()
    };
    assert_eq!(line("log")["text"], "plain message 42 {answer: true}");
    assert_eq!(line("warning")["text"], "a warning");
    assert_eq!(line("error")["text"], "an error line");
    let exception = line("exception");
    assert!(
        exception["text"]
            .as_str()
            .unwrap()
            .contains("undefinedFunctionOfTheFixture is not defined"),
        "{exception}"
    );
    // The engines do not name the scripts they compile yet, so there is no
    // `url:line` to report; a location that is not a URL is left out.
    assert_eq!(exception["location"], json!(null));
    assert_eq!(all.structured()["errors"], 2);
    let text = all.text();
    assert!(
        text.starts_with("<<<PAGE CONTENT "),
        "page text is fenced: {text}"
    );
    assert!(text.contains("[warning] a warning"), "{text}");

    let errors = client
        .tool("browser_console_messages", json!({"level": "error"}))
        .await;
    errors.ok();
    assert_eq!(errors.structured()["returned"], 2);
    let warnings = client
        .tool(
            "browser_console_messages",
            json!({"level": "warning", "limit": 1}),
        )
        .await;
    warnings.ok();
    assert_eq!(warnings.structured()["returned"], 1);
    assert_eq!(warnings.structured()["matching"], 3);
    assert_eq!(
        warnings.structured()["messages"][0]["level"],
        "exception",
        "the newest line is kept"
    );

    // What a later evaluation logs is added; a navigation starts over.
    client.evaluate("console.info('from the agent')").await;
    let more = client.tool("browser_console_messages", json!({})).await;
    assert!(more.text().contains("from the agent"), "{}", more.text());
    client.navigate(&harness.url("/about.html")).await;
    let fresh = client.tool("browser_console_messages", json!({})).await;
    fresh.ok();
    assert_eq!(fresh.structured()["total"], 0);
    assert!(
        fresh.text().contains("No console messages match."),
        "{}",
        fresh.text()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_form_is_submitted_by_the_page_and_the_tool_reports_the_navigation() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/login.html")).await;
    let snapshot = client.tool("browser_snapshot", json!({})).await;
    let text = snapshot.text();
    let user = find_ref(&text, &["textbox", "User"]);
    let pass = find_ref(&text, &["textbox", "Password"]);
    let remember = find_ref(&text, &["checkbox", "Remember me"]);
    let button = find_ref(&text, &["button", "Log in"]);

    client
        .tool(
            "browser_fill",
            json!({"ref": user, "value": "ada lovelace"}),
        )
        .await
        .ok();
    client
        .tool("browser_fill", json!({"ref": pass, "value": "p&ss=word"}))
        .await
        .ok();
    client
        .tool("browser_click", json!({"ref": remember}))
        .await
        .ok();
    client
        .evaluate("document.getElementById('role').value = 'a'")
        .await;

    let clicked = client.tool("browser_click", json!({"ref": button})).await;
    clicked.ok();
    let echo = harness.url("/api/echo?next=home");
    assert_eq!(
        clicked.structured()["navigation"]["url"],
        echo,
        "{}",
        clicked.0
    );
    assert_eq!(clicked.structured()["navigation"]["status"], 200);
    assert!(
        clicked
            .text()
            .contains(&format!("Navigated to {echo} (HTTP 200).")),
        "{}",
        clicked.text()
    );

    let page = client.tool("browser_extract", json!({})).await;
    page.ok();
    let body = page.text();
    let start = body.find('{').expect("the echoed JSON");
    let end = body.rfind('}').expect("the echoed JSON");
    let echoed: serde_json::Value = serde_json::from_str(&body[start..=end]).expect("JSON");
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["query"], "next=home");
    assert_eq!(
        echoed["body"],
        "user=ada+lovelace&pass=p%26ss%3Dword&remember=yes&role=a&action=login"
    );
    assert_eq!(
        echoed["headers"]["content-type"],
        "application/x-www-form-urlencoded"
    );

    // The submission is a history entry like any other navigation.
    let back = client.tool("browser_back", json!({})).await;
    back.ok();
    assert_eq!(back.structured()["title"], "Log in");
}

#[tokio::test(flavor = "multi_thread")]
async fn enter_in_a_field_submits_a_post_form() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/login.html")).await;
    let user = client.ref_of(&["textbox", "User"]).await;
    let typed = client
        .tool(
            "browser_type",
            json!({"ref": user, "text": "grace", "submit": true}),
        )
        .await;
    typed.ok();
    assert_eq!(
        typed.structured()["navigation"]["url"],
        harness.url("/api/echo?next=home"),
        "{}",
        typed.0
    );
    let body = client
        .evaluate("JSON.parse(document.body.textContent).body")
        .await;
    assert_eq!(
        body, "user=grace&pass=&role=u&action=login",
        "implicit submission goes through the form's default button"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn navigations_the_pages_own_script_starts_are_followed_and_reported() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();

    // A redirect written in JavaScript: the page asked for is the page shown.
    let redirected = client
        .tool(
            "browser_navigate",
            json!({"url": harness.url("/jsredirect.html")}),
        )
        .await;
    redirected.ok();
    assert_eq!(redirected.structured()["title"], "About");
    assert_eq!(
        redirected.structured()["final_url"],
        harness.url("/about.html?from=redirect")
    );

    // A click handler that assigns `location`.
    client.navigate(&harness.url("/login.html")).await;
    let link = client.ref_of(&["link", "About (scripted)"]).await;
    let clicked = client.tool("browser_click", json!({"ref": link})).await;
    clicked.ok();
    assert_eq!(
        clicked.structured()["navigation"]["url"],
        harness.url("/about.html?via=script"),
        "{}",
        clicked.0
    );

    // An evaluation that navigates says so.
    let evaluated = client
        .tool(
            "browser_evaluate",
            json!({"expression": "location.href = '/index.html'; 'going'"}),
        )
        .await;
    evaluated.ok();
    assert_eq!(evaluated.structured()["result"], "going");
    assert_eq!(
        evaluated.structured()["navigation"]["url"],
        harness.url("/index.html")
    );
    assert!(
        evaluated
            .text()
            .contains("The script navigated the page to"),
        "{}",
        evaluated.text()
    );
    let snapshot = client.tool("browser_snapshot", json!({})).await;
    assert_eq!(snapshot.structured()["title"], "Fixture Home");

    // The session history holds each of them.
    let back = client.tool("browser_back", json!({})).await;
    back.ok();
    assert_eq!(
        back.structured()["final_url"],
        harness.url("/about.html?via=script")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_into_the_page_moves_to_the_fragment_without_loading() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/login.html")).await;
    let help = client.ref_of(&["link", "Help"]).await;
    let clicked = client.tool("browser_click", json!({"ref": help})).await;
    clicked.ok();
    assert_eq!(
        clicked.structured()["navigation"],
        json!(null),
        "nothing was loaded: {}",
        clicked.0
    );
    let snapshot = client.tool("browser_snapshot", json!({})).await;
    snapshot.ok();
    assert_eq!(
        snapshot.structured()["url"],
        harness.url("/login.html#help"),
        "the page URL follows the document"
    );
    assert_eq!(
        client
            .evaluate("hashes.join(',') + '|' + history.length")
            .await,
        "help|2"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn select_option_chooses_by_value_or_text_and_fires_the_events() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/login.html")).await;
    let snapshot = client.tool("browser_snapshot", json!({})).await;
    let text = snapshot.text();
    let role_line = text
        .lines()
        .find(|line| line.contains("combobox \"Role\""))
        .unwrap_or_else(|| panic!("the select is in the snapshot: {text}"));
    assert!(
        role_line.contains("value=\"User\"") && role_line.contains("options=[\"User\", \"Admin\"]"),
        "{role_line}"
    );
    let role = find_ref(&text, &["combobox", "Role"]);
    let tags = find_ref(&text, &["listbox", "Tags"]);

    let chosen = client
        .tool(
            "browser_select_option",
            json!({"ref": role, "values": ["admin"]}),
        )
        .await;
    chosen.ok();
    assert_eq!(
        chosen.structured()["selected"],
        json!([{"value": "a", "text": "Admin"}]),
        "matched by text, ignoring case"
    );
    assert_eq!(chosen.structured()["changed"], true);
    assert_eq!(
        client
            .evaluate("changes.join(' ') + '|' + document.getElementById('role').value")
            .await,
        "input:a change:a|a"
    );
    let again = client
        .tool(
            "browser_select_option",
            json!({"ref": role, "values": ["a"]}),
        )
        .await;
    again.ok();
    assert_eq!(again.structured()["changed"], false);
    assert_eq!(
        client.evaluate("changes.length").await,
        2,
        "no events for a selection that did not change"
    );
    let after = client.tool("browser_snapshot", json!({})).await.text();
    assert!(
        after.contains("combobox \"Role\"") && after.contains("value=\"Admin\""),
        "{after}"
    );

    let many = client
        .tool(
            "browser_select_option",
            json!({"ref": tags, "values": ["Alpha", "c"]}),
        )
        .await;
    many.ok();
    assert_eq!(
        many.structured()["selected"],
        json!([{"value": "a", "text": "Alpha"}, {"value": "c", "text": "Gamma"}]),
        "a multiple select ends up with exactly the options named"
    );
    let cleared = client
        .tool("browser_select_option", json!({"ref": tags, "values": []}))
        .await;
    cleared.ok();
    assert_eq!(cleared.structured()["selected"], json!([]));

    let missing = client
        .tool(
            "browser_select_option",
            json!({"ref": role, "values": ["Owner"]}),
        )
        .await;
    assert_eq!(missing.error_type(), "invalid_arguments");
    assert!(
        missing.text().contains("\"User\", \"Admin\""),
        "the error lists the options: {}",
        missing.text()
    );
    let two = client
        .tool(
            "browser_select_option",
            json!({"ref": role, "values": ["u", "a"]}),
        )
        .await;
    assert_eq!(two.error_type(), "invalid_arguments");
    let user = find_ref(&text, &["textbox", "User"]);
    let wrong = client
        .tool(
            "browser_select_option",
            json!({"ref": user, "values": ["x"]}),
        )
        .await;
    assert_eq!(wrong.error_type(), "invalid_arguments");

    // The chosen option is what the form submits.
    let button = find_ref(&text, &["button", "Log in"]);
    client
        .tool("browser_click", json!({"ref": button}))
        .await
        .ok();
    let body = client
        .evaluate("JSON.parse(document.body.textContent).body")
        .await;
    assert_eq!(body, "user=&pass=&role=a&action=login");
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_fires_the_pointer_events_a_menu_listens_for() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/login.html")).await;
    let before = client.tool("browser_snapshot", json!({})).await.text();
    assert!(!before.contains("Menu content"), "{before}");
    let button = find_ref(&before, &["button", "Menu"]);
    let hovered = client.tool("browser_hover", json!({"ref": button})).await;
    hovered.ok();
    assert!(
        hovered.text().starts_with("Moved the pointer over "),
        "{}",
        hovered.text()
    );
    let after = client.tool("browser_snapshot", json!({})).await.text();
    assert!(after.contains("Menu content"), "{after}");
}

/// Every request of the page is listed, not only its document: what the
/// parser asked for, what script asked for, each hop of a redirect, and what
/// failed with the name of the failure.
#[tokio::test(flavor = "multi_thread")]
async fn network_requests_list_what_the_page_and_its_script_asked_for() {
    let harness = Harness::start(Tier::Js).await;
    let client = harness.client();
    client.navigate(&harness.url("/network.html")).await;
    let waited = client
        .tool(
            "browser_wait_for",
            json!({"text": "200 404 200 failed", "timeout_ms": 10_000}),
        )
        .await;
    waited.ok();

    let all = client.tool("browser_network_requests", json!({})).await;
    all.ok();
    let listed: Vec<String> = all.structured()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| {
            let outcome = match (&request["error"], &request["status"]) {
                (serde_json::Value::String(error), _) => error.clone(),
                (_, status) => status.to_string(),
            };
            format!(
                "{} {} {} {}",
                request["method"].as_str().unwrap(),
                outcome,
                request["resource_type"].as_str().unwrap(),
                request["url"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches(harness.url("").trim_end_matches('/')),
            )
        })
        .collect();
    for expected in [
        "GET 200 document /network.html",
        "GET 200 script /network.js",
        "POST 201 xhr /api/status?code=201",
        "GET 200 fetch /about.html",
        "GET 404 fetch /api/status?code=404",
        "GET 302 fetch /api/redirect?to=/about.html",
        "GET net::ERR_UNSAFE_PORT fetch http://127.0.0.1:1/unreachable",
    ] {
        assert!(
            listed.iter().any(|line| line == expected),
            "`{expected}` is listed: {listed:#?}"
        );
    }
    // The redirect's second hop is a line of its own, beside the direct
    // request for the same page.
    assert_eq!(
        listed
            .iter()
            .filter(|line| *line == "GET 200 fetch /about.html")
            .count(),
        2,
        "{listed:#?}"
    );
    assert_eq!(listed[0], "GET 200 document /network.html", "{listed:#?}");

    let failed = client
        .tool("browser_network_requests", json!({"failed_only": true}))
        .await;
    failed.ok();
    assert_eq!(failed.structured()["returned"], 2, "{}", failed.text());
    assert!(
        failed.text().contains("GET 404 fetch") && failed.text().contains("net::ERR_UNSAFE_PORT"),
        "{}",
        failed.text()
    );

    // A navigation starts the list over, and the new document is in it.
    client.navigate(&harness.url("/about.html")).await;
    let after = client.tool("browser_network_requests", json!({})).await;
    after.ok();
    let urls: Vec<&str> = after.structured()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["url"].as_str().unwrap())
        .collect();
    assert!(
        urls.contains(&harness.url("/about.html").as_str()),
        "{urls:?}"
    );
    assert!(
        !urls.iter().any(|url| url.contains("network")),
        "the earlier page's requests are gone: {urls:?}"
    );
}
