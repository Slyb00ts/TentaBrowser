//! The DOM, Input, Emulation and Performance domains against a real page
//! engine, in the command sequences Puppeteer and Playwright send.

mod common;

use base64::Engine as _;
use common::{Fixture, Page, decode_png, error_of};
use serde_json::{Value, json};

/// Absolutely placed boxes, so every coordinate below is a fact of the CSS.
const FORM: &str = "<!doctype html><html><head><title>Form</title><style>\
    body{margin:0;font:16px sans-serif}\
    #name{position:absolute;left:20px;top:20px;width:200px;height:30px;border:2px solid #36c;padding:0;margin:0}\
    #go{position:absolute;left:20px;top:80px;width:120px;height:40px;border:0;margin:0;padding:0;background:#2a9d4a}\
    #box{position:absolute;left:30px;top:150px;width:100px;height:50px;padding:5px;border:2px solid #000;margin:0;background:#eee}\
    #hidden{display:none}\
    #agree{position:absolute;left:300px;top:20px;width:20px;height:20px;margin:0}\
    #out{position:absolute;left:300px;top:80px;width:200px;height:30px}\
    </style></head><body>\
    <form id=f><input id=name type=text placeholder=name><input id=agree type=checkbox>\
    <button id=go type=button>Greet</button></form>\
    <div id=box>box</div><div id=hidden>x</div><div id=out>idle</div>\
    <script>\
    window.log = [];\
    var note = function (e) { window.log.push(e.type + ':' + (e.target.id || e.target.localName) + ':' + (e.clientX === undefined ? '' : e.clientX + ',' + e.clientY)); };\
    ['mousedown','mouseup','click','mouseover','mousemove'].forEach(function (t) { document.addEventListener(t, note); });\
    ['focus','blur','focusin','focusout'].forEach(function (t) { document.addEventListener(t, function (e) { window.log.push(e.type + ':' + e.target.id); }, true); });\
    ['keydown','keypress','keyup'].forEach(function (t) { document.addEventListener(t, function (e) { window.log.push(t + ':' + e.key); }); });\
    document.getElementById('name').addEventListener('input', function (e) { window.log.push('input:' + e.target.value + ':' + e.inputType); });\
    document.getElementById('agree').addEventListener('change', function (e) { window.log.push('change:' + e.target.checked); });\
    document.getElementById('f').addEventListener('submit', function (e) { e.preventDefault(); window.log.push('submit'); });\
    document.getElementById('go').addEventListener('click', function () {\
      document.getElementById('out').textContent = 'Hello, ' + document.getElementById('name').value + '!';\
    });\
    </script></body></html>";

async fn loaded(fx: &Fixture) -> Page {
    let mut page = fx.page().await;
    page.begin().await;
    let url = fx.site.url("/form");
    page.navigate(&url).await;
    page.event("Page.loadEventFired").await;
    page
}

async fn form() -> (Fixture, Page) {
    let fx = Fixture::start(&[("/form", FORM)]).await;
    let page = loaded(&fx).await;
    (fx, page)
}

async fn node(page: &mut Page, selector: &str) -> i64 {
    let root = page.result("DOM.getDocument", json!({})).await["root"]["nodeId"]
        .as_i64()
        .unwrap();
    page.result(
        "DOM.querySelector",
        json!({ "nodeId": root, "selector": selector }),
    )
    .await["nodeId"]
        .as_i64()
        .unwrap()
}

async fn click(page: &mut Page, x: f64, y: f64) {
    for (kind, buttons) in [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 0)] {
        page.result(
            "Input.dispatchMouseEvent",
            json!({ "type": kind, "x": x, "y": y, "button": if kind == "mouseMoved" { "none" } else { "left" },
                    "buttons": buttons, "clickCount": if kind == "mouseMoved" { 0 } else { 1 } }),
        )
        .await;
    }
}

async fn key(page: &mut Page, key: &str, text: Option<&str>) {
    let mut down =
        json!({ "type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": key });
    if let Some(text) = text {
        down["text"] = json!(text);
    }
    page.result("Input.dispatchKeyEvent", down).await;
    page.result(
        "Input.dispatchKeyEvent",
        json!({ "type": "keyUp", "key": key }),
    )
    .await;
}

async fn log(page: &mut Page) -> Vec<String> {
    page.evaluate("window.log.splice(0).join('|')")
        .await
        .as_str()
        .unwrap()
        .split('|')
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

fn quad_bounds(quad: &Value) -> (f64, f64, f64, f64) {
    let points: Vec<f64> = quad
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(points.len(), 8);
    let xs = [points[0], points[2], points[4], points[6]];
    let ys = [points[1], points[3], points[5], points[7]];
    let min = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
    let max = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
    (min(xs), min(ys), max(xs) - min(xs), max(ys) - min(ys))
}

// ---- DOM ------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_document_is_described_with_its_children_and_queries_find_nodes() {
    let (fx, mut page) = form().await;
    page.result("DOM.enable", json!({})).await;
    let document = page.result("DOM.getDocument", json!({})).await["root"].clone();
    assert_eq!(document["nodeType"], 9);
    assert_eq!(document["nodeName"], "#document");
    assert_eq!(document["frameId"], page.target.as_str());
    assert!(document["documentURL"].as_str().unwrap().ends_with("/form"));
    let children = document["children"].as_array().unwrap();
    assert_eq!(children.last().unwrap()["nodeName"], "HTML");
    assert!(
        children.last().unwrap().get("children").is_none(),
        "depth 1 stops at the document's children"
    );
    assert_eq!(children.last().unwrap()["childNodeCount"], 2);

    let deep = page.result("DOM.getDocument", json!({ "depth": -1 })).await["root"].clone();
    let html = deep["children"].as_array().unwrap().last().unwrap().clone();
    let body = html["children"][1].clone();
    assert_eq!(body["nodeName"], "BODY");
    let form_node = &body["children"][0];
    assert_eq!(form_node["localName"], "form");
    assert_eq!(form_node["attributes"], json!(["id", "f"]));

    let go = node(&mut page, "#go").await;
    assert!(go > 0);
    let all = page
        .result(
            "DOM.querySelectorAll",
            json!({ "nodeId": document["nodeId"], "selector": "input" }),
        )
        .await;
    assert_eq!(all["nodeIds"].as_array().unwrap().len(), 2);
    assert_eq!(
        node(&mut page, "#missing").await,
        0,
        "no match is node id 0"
    );
    let bad = page
        .call(
            "DOM.querySelector",
            json!({ "nodeId": document["nodeId"], "selector": "div[" }),
        )
        .await;
    assert_eq!(error_of(&bad), (-32000, "DOM Error while querying"));
    let unknown = page
        .call("DOM.describeNode", json!({ "backendNodeId": 999_999 }))
        .await;
    assert_eq!(error_of(&unknown).0, -32000);
    let none = page.call("DOM.describeNode", json!({})).await;
    assert_eq!(error_of(&none).0, -32602);

    let described = page
        .result("DOM.describeNode", json!({ "backendNodeId": go }))
        .await["node"]
        .clone();
    assert_eq!(described["nodeName"], "BUTTON");
    assert_eq!(described["backendNodeId"], go);
    assert_eq!(
        described["attributes"],
        json!(["id", "go", "type", "button"])
    );
    let html = page
        .result("DOM.getOuterHTML", json!({ "backendNodeId": go }))
        .await;
    assert_eq!(
        html["outerHTML"],
        "<button id=\"go\" type=\"button\">Greet</button>"
    );
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nodes_and_remote_objects_name_each_other() {
    let (fx, mut page) = form().await;
    let go = node(&mut page, "#go").await;
    let object = page
        .result(
            "DOM.resolveNode",
            json!({ "backendNodeId": go, "objectGroup": "test" }),
        )
        .await["object"]
        .clone();
    assert_eq!(object["subtype"], "node");
    let object_id = object["objectId"].as_str().unwrap().to_owned();

    let id = page
        .result(
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": "function () { return this.id; }",
                    "returnByValue": true }),
        )
        .await;
    assert_eq!(id["result"]["value"], "go");

    let back = page
        .result("DOM.describeNode", json!({ "objectId": object_id }))
        .await["node"]["backendNodeId"]
        .clone();
    assert_eq!(back, go);
    let requested = page
        .result("DOM.requestNode", json!({ "objectId": object_id }))
        .await;
    assert_eq!(requested["nodeId"], go);

    // An object that is not a node, and a node that left the document.
    let plain = page
        .result("Runtime.evaluate", json!({ "expression": "({ a: 1 })" }))
        .await["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = page
        .call("DOM.describeNode", json!({ "objectId": plain }))
        .await;
    assert_eq!(
        error_of(&refused),
        (-32000, "Object id doesn't reference a Node")
    );
    let hidden = node(&mut page, "#hidden").await;
    page.evaluate("document.getElementById('hidden').remove()")
        .await;
    let gone = page
        .call("DOM.resolveNode", json!({ "backendNodeId": hidden }))
        .await;
    assert_eq!(error_of(&gone).0, -32000);
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn box_model_and_quads_follow_the_layout() {
    let (fx, mut page) = form().await;
    let boxed = node(&mut page, "#box").await;
    let model = page
        .result("DOM.getBoxModel", json!({ "backendNodeId": boxed }))
        .await["model"]
        .clone();
    // left 30, top 150, 100x50 content, 5 padding, 2 border.
    assert_eq!(quad_bounds(&model["content"]), (37.0, 157.0, 100.0, 50.0));
    assert_eq!(quad_bounds(&model["padding"]), (32.0, 152.0, 110.0, 60.0));
    assert_eq!(quad_bounds(&model["border"]), (30.0, 150.0, 114.0, 64.0));
    assert_eq!(quad_bounds(&model["margin"]), (30.0, 150.0, 114.0, 64.0));
    assert_eq!(model["width"], 114);
    assert_eq!(model["height"], 64);
    assert_eq!(
        model["content"][0].as_f64(),
        Some(37.0),
        "clockwise from the top-left corner"
    );
    assert_eq!(model["content"][1].as_f64(), Some(157.0));

    let quads = page
        .result("DOM.getContentQuads", json!({ "backendNodeId": boxed }))
        .await["quads"]
        .clone();
    assert_eq!(quads.as_array().unwrap().len(), 1);
    assert_eq!(quad_bounds(&quads[0]), (37.0, 157.0, 100.0, 50.0));

    // A node without a box has no geometry.
    let hidden = node(&mut page, "#hidden").await;
    for method in [
        "DOM.getBoxModel",
        "DOM.getContentQuads",
        "DOM.scrollIntoViewIfNeeded",
    ] {
        let failed = page.call(method, json!({ "backendNodeId": hidden })).await;
        assert_eq!(error_of(&failed).0, -32000, "{method}");
    }
    page.result(
        "DOM.scrollIntoViewIfNeeded",
        json!({ "backendNodeId": boxed }),
    )
    .await;

    let at = page
        .result("DOM.getNodeForLocation", json!({ "x": 60, "y": 170 }))
        .await;
    assert_eq!(at["backendNodeId"], boxed);
    assert_eq!(at["frameId"], page.target.as_str());
    let off = page
        .call("DOM.getNodeForLocation", json!({ "x": -5, "y": 170 }))
        .await;
    assert_eq!(error_of(&off).0, -32000);
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_file_input_files_is_refused_by_name() {
    let (fx, mut page) = form().await;
    let input = node(&mut page, "#name").await;
    let refused = page
        .call(
            "DOM.setFileInputFiles",
            json!({ "files": ["/etc/hostname"], "backendNodeId": input }),
        )
        .await;
    let (code, message) = error_of(&refused);
    assert_eq!(code, -32000);
    assert!(
        message.starts_with("DOM.setFileInputFiles is not supported"),
        "{message}"
    );
    fx.stop().await;
}

// ---- Input ----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_click_reaches_the_element_under_the_point_in_the_order_a_browser_uses() {
    let (fx, mut page) = form().await;
    click(&mut page, 60.0, 100.0).await;
    assert_eq!(
        log(&mut page).await,
        [
            "mouseover:go:60,100",
            "mousemove:go:60,100",
            "mousedown:go:60,100",
            "focus:go",
            "focusin:go",
            "mouseup:go:60,100",
            "click:go:60,100",
        ],
        "a press focuses the button between mousedown and mouseup, focus before focusin as Chromium"
    );
    assert_eq!(
        page.evaluate("document.getElementById('out').textContent")
            .await,
        "Hello, !"
    );
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typing_edits_the_focused_input_and_reports_every_event() {
    let (fx, mut page) = form().await;
    click(&mut page, 60.0, 35.0).await;
    assert_eq!(page.evaluate("document.activeElement.id").await, "name");
    log(&mut page).await;

    key(&mut page, "A", Some("A")).await;
    key(&mut page, "d", Some("d")).await;
    key(&mut page, "a", Some("a")).await;
    assert_eq!(
        page.evaluate("document.getElementById('name').value").await,
        "Ada"
    );
    assert_eq!(
        log(&mut page).await[..5],
        [
            "keydown:A",
            "keypress:A",
            "input:A:insertText",
            "keyup:A",
            "keydown:d"
        ]
    );

    key(&mut page, "Backspace", None).await;
    assert_eq!(
        page.evaluate("document.getElementById('name').value").await,
        "Ad"
    );
    page.result("Input.insertText", json!({ "text": "!" }))
        .await;
    assert_eq!(
        page.evaluate("document.getElementById('name').value").await,
        "Ad!"
    );

    // Caret movement and replacement: Home, then type before the text.
    key(&mut page, "Home", None).await;
    key(&mut page, "x", Some("x")).await;
    assert_eq!(
        page.evaluate("document.getElementById('name').value").await,
        "xAd!"
    );

    // Select all and replace.
    page.result(
        "Input.dispatchKeyEvent",
        json!({ "type": "rawKeyDown", "key": "a", "modifiers": 2 }),
    )
    .await;
    key(&mut page, "z", Some("z")).await;
    assert_eq!(
        page.evaluate("document.getElementById('name').value").await,
        "z"
    );

    // The button reads what was typed.
    click(&mut page, 60.0, 100.0).await;
    assert_eq!(
        page.evaluate("document.getElementById('out').textContent")
            .await,
        "Hello, z!"
    );
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn focus_moves_with_clicks_and_tab_and_enter_submits_the_form() {
    let (fx, mut page) = form().await;
    click(&mut page, 60.0, 35.0).await;
    log(&mut page).await;
    click(&mut page, 60.0, 100.0).await;
    let events = log(&mut page).await;
    let at = |name: &str| {
        events
            .iter()
            .position(|e| e == name)
            .unwrap_or_else(|| panic!("{name} in {events:?}"))
    };
    assert!(at("blur:name") < at("focus:go"), "{events:?}");
    assert_eq!(page.evaluate("document.activeElement.id").await, "go");

    key(&mut page, "Tab", None).await;
    assert_eq!(
        page.evaluate("document.activeElement.localName").await,
        "body"
    );

    click(&mut page, 60.0, 35.0).await;
    key(&mut page, "Enter", Some("\r")).await;
    let events = log(&mut page).await;
    assert!(events.contains(&"submit".to_owned()), "{events:?}");
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkbox_toggles_and_clicks_on_nothing_are_harmless() {
    let (fx, mut page) = form().await;
    click(&mut page, 310.0, 30.0).await;
    assert_eq!(
        page.evaluate("document.getElementById('agree').checked")
            .await,
        true
    );
    let events = log(&mut page).await;
    assert!(events.contains(&"change:true".to_owned()), "{events:?}");
    click(&mut page, 310.0, 30.0).await;
    assert_eq!(
        page.evaluate("document.getElementById('agree').checked")
            .await,
        false
    );

    // The wheel has nothing to scroll; touch and drag are named refusals.
    page.result(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseWheel", "x": 10, "y": 10, "deltaX": 0, "deltaY": 100 }),
    )
    .await;
    for method in ["Input.dispatchTouchEvent", "Input.dispatchDragEvent"] {
        let refused = page
            .call(method, json!({ "type": "touchStart", "touchPoints": [] }))
            .await;
        let (code, message) = error_of(&refused);
        assert_eq!(code, -32000, "{method}");
        assert!(
            message.starts_with(&format!("{method} is not supported")),
            "{message}"
        );
    }
    let unknown = page
        .call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseTeleported", "x": 1, "y": 1 }),
        )
        .await;
    assert_eq!(error_of(&unknown).0, -32602);
    fx.stop().await;
}

// ---- Emulation and Performance --------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_metrics_resize_the_viewport_layout_and_screenshot_until_cleared() {
    let (fx, mut page) = form().await;
    let before = page.result("Page.getLayoutMetrics", json!({})).await;
    let original = (
        before["cssLayoutViewport"]["clientWidth"].as_u64().unwrap(),
        before["cssLayoutViewport"]["clientHeight"]
            .as_u64()
            .unwrap(),
    );

    page.result(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": 320, "height": 200, "deviceScaleFactor": 2, "mobile": false }),
    )
    .await;
    let metrics = page.result("Page.getLayoutMetrics", json!({})).await;
    assert_eq!(metrics["cssLayoutViewport"]["clientWidth"], 320);
    assert_eq!(metrics["cssLayoutViewport"]["clientHeight"], 200);
    let png = page.screenshot(json!({})).await;
    let image = decode_png(&png);
    assert_eq!(
        (image.width, image.height),
        (640, 400),
        "CSS size times the scale factor"
    );

    page.result("Emulation.clearDeviceMetricsOverride", json!({}))
        .await;
    let after = page.result("Page.getLayoutMetrics", json!({})).await;
    assert_eq!(
        (
            after["cssLayoutViewport"]["clientWidth"].as_u64().unwrap(),
            after["cssLayoutViewport"]["clientHeight"].as_u64().unwrap()
        ),
        original
    );

    let invalid = page
        .call(
            "Emulation.setDeviceMetricsOverride",
            json!({ "width": -1, "height": 1, "deviceScaleFactor": 1, "mobile": false }),
        )
        .await;
    assert_eq!(error_of(&invalid).0, -32602);
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_override_ends_with_the_session_that_set_it() {
    let (fx, mut page) = form().await;
    let target = page.target.clone();
    page.result(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": 300, "height": 150, "deviceScaleFactor": 1, "mobile": true }),
    )
    .await;
    drop(page);
    let mut other = fx.attach(&target).await;
    // The first session's teardown races this connection; give it its turn.
    let mut width = 0;
    for _ in 0..50 {
        width = other.result("Page.getLayoutMetrics", json!({})).await["cssLayoutViewport"]
            ["clientWidth"]
            .as_u64()
            .unwrap();
        if width != 300 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_ne!(width, 300, "the viewport is back to the page's own");
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emulation_accepts_what_it_can_ignore_and_refuses_the_rest_by_name() {
    let (fx, mut page) = form().await;
    page.result(
        "Emulation.setTouchEmulationEnabled",
        json!({ "enabled": true }),
    )
    .await;
    page.result(
        "Emulation.setEmulatedMedia",
        json!({ "media": "print", "features": [{ "name": "prefers-color-scheme", "value": "dark" }] }),
    )
    .await;
    // The user agent header is overridable per page since the Network package;
    // `navigator.userAgent` is not.
    page.result(
        "Emulation.setUserAgentOverride",
        json!({ "userAgent": "x" }),
    )
    .await;
    for (method, params) in [
        (
            "setTimezoneOverride",
            json!({ "timezoneId": "Europe/Warsaw" }),
        ),
        ("setLocaleOverride", json!({ "locale": "pl-PL" })),
        ("setScriptExecutionDisabled", json!({ "value": true })),
        ("setGeolocationOverride", json!({})),
    ] {
        let refused = page.call(&format!("Emulation.{method}"), params).await;
        let (code, message) = error_of(&refused);
        assert_eq!(code, -32000, "{method}");
        assert!(
            message.starts_with(&format!("Emulation.{method} is not supported")),
            "{message}"
        );
    }
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn performance_enables_and_reports_a_timestamp() {
    let (fx, mut page) = form().await;
    page.result("Performance.enable", json!({})).await;
    let metrics = page.result("Performance.getMetrics", json!({})).await["metrics"].clone();
    let names: Vec<&str> = metrics
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Timestamp"));
    page.result("Performance.disable", json!({})).await;
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_screenshot_after_typing_shows_the_page_as_it_is() {
    let (fx, mut page) = form().await;
    click(&mut page, 60.0, 35.0).await;
    key(&mut page, "Q", Some("Q")).await;
    click(&mut page, 60.0, 100.0).await;
    let png = page.screenshot(json!({})).await;
    let image = decode_png(&png);
    // The button is green where the click landed.
    let [r, g, b, _] = image.pixel(60, 100);
    assert!(g > r && g > b, "button colour {:?}", (r, g, b));
    assert!(
        !base64::engine::general_purpose::STANDARD
            .encode(&png)
            .is_empty()
    );
    fx.stop().await;
}

// ---- the page-side surface the clients' injected functions rely on --------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_geometry_follows_the_layout_when_a_client_function_reads_it() {
    let (fx, mut page) = form().await;
    let rect = page
        .evaluate("JSON.stringify(document.getElementById('box').getBoundingClientRect())")
        .await;
    let rect: Value = serde_json::from_str(rect.as_str().unwrap()).unwrap();
    assert_eq!(
        (
            rect["x"].as_f64(),
            rect["y"].as_f64(),
            rect["width"].as_f64(),
            rect["height"].as_f64()
        ),
        (Some(30.0), Some(150.0), Some(114.0), Some(64.0))
    );
    assert_eq!(rect["right"], 144.0);
    assert_eq!(
        page.evaluate("document.getElementById('box').getClientRects().length")
            .await,
        1
    );
    assert_eq!(
        page.evaluate("document.getElementById('hidden').getClientRects().length")
            .await,
        0,
        "display: none has no box"
    );
    assert_eq!(
        page.evaluate("document.getElementById('box').offsetWidth")
            .await,
        114
    );
    // clientWidth is the padding box: 100 + 2 * 5.
    assert_eq!(
        page.evaluate("document.getElementById('box').clientWidth")
            .await,
        110
    );
    let viewport =
        page.result("Page.getLayoutMetrics", json!({})).await["cssLayoutViewport"].clone();
    assert_eq!(
        page.evaluate("document.documentElement.clientWidth").await,
        viewport["clientWidth"]
    );

    // A move the page makes itself shows at the next client read.
    page.evaluate("document.getElementById('box').style.left = '60px'")
        .await;
    assert_eq!(
        page.evaluate("document.getElementById('box').getBoundingClientRect().x")
            .await,
        60
    );
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observers_the_clients_wait_with_answer_without_native_support() {
    let (fx, mut page) = form().await;
    let ratio = page
        .result(
            "Runtime.evaluate",
            json!({ "awaitPromise": true, "returnByValue": true, "expression":
                "new Promise(function (resolve) { new IntersectionObserver(function (entries) { resolve(entries[0].intersectionRatio); }).observe(document.getElementById('box')); })" }),
        )
        .await;
    assert_eq!(ratio["result"]["value"], 1);

    let seen = page
        .result(
            "Runtime.evaluate",
            json!({ "awaitPromise": true, "returnByValue": true, "expression":
                "new Promise(function (resolve) { \
                   var observer = new MutationObserver(function () { \
                     if (document.getElementById('late')) { observer.disconnect(); resolve(true); } }); \
                   observer.observe(document, { childList: true, subtree: true }); \
                   setTimeout(function () { var p = document.createElement('p'); p.id = 'late'; document.body.appendChild(p); }, 30); })" }),
        )
        .await;
    assert_eq!(seen["result"]["value"], true);
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_execution_context_is_announced_before_the_page_loads() {
    let fx = Fixture::start(&[("/form", FORM)]).await;
    let mut page = fx.page().await;
    page.begin().await;
    page.result("Runtime.enable", json!({})).await;
    let first = page.event("Runtime.executionContextCreated").await;
    let blank = first["context"]["id"].as_u64().unwrap();
    let url = fx.site.url("/form");
    let loader = page.navigate(&url).await;
    let mut seen = Vec::new();
    loop {
        let event = page.client.next_event().await;
        let method = event["method"].as_str().unwrap_or_default().to_owned();
        let named = if method == "Page.lifecycleEvent" {
            if event["params"]["loaderId"] != loader.as_str() {
                continue;
            }
            format!(
                "lifecycle:{}",
                event["params"]["name"].as_str().unwrap_or_default()
            )
        } else {
            method
        };
        let done = named == "lifecycle:load";
        if named == "Runtime.executionContextCreated" {
            assert_ne!(event["params"]["context"]["id"].as_u64().unwrap(), blank);
            assert_eq!(event["params"]["context"]["origin"], fx.site.base.as_str());
        }
        seen.push(named);
        if done {
            break;
        }
    }
    let created = seen
        .iter()
        .position(|name| name == "Runtime.executionContextCreated")
        .unwrap_or_else(|| panic!("a context before load in {seen:?}"));
    assert!(
        created < seen.len() - 1,
        "the new context precedes the load lifecycle event: {seen:?}"
    );
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_start_up_domains_are_accepted() {
    let (fx, mut page) = form().await;
    for method in [
        "Network.enable",
        "Network.disable",
        "Audits.enable",
        "Audits.disable",
        "Log.enable",
        "Security.enable",
    ] {
        page.result(method, json!({})).await;
    }
    fx.stop().await;
}

async fn viewport_width(page: &mut Page) -> u64 {
    page.result("Page.getLayoutMetrics", json!({})).await["cssLayoutViewport"]["clientWidth"]
        .as_u64()
        .unwrap()
}

async fn width_settles_at(page: &mut Page, wanted: u64) {
    for _ in 0..100 {
        if viewport_width(page).await == wanted {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    assert_eq!(viewport_width(page).await, wanted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overrides_of_two_sessions_stack_and_unwind_in_any_order() {
    let (fx, mut watcher) = form().await;
    let target = watcher.target.clone();
    let pristine = viewport_width(&mut watcher).await;
    let set = |width: u32| json!({ "width": width, "height": 200, "deviceScaleFactor": 1, "mobile": false });
    let mut a = fx.attach(&target).await;
    let mut b = fx.attach(&target).await;
    a.result("Emulation.setDeviceMetricsOverride", set(300))
        .await;
    b.result("Emulation.setDeviceMetricsOverride", set(500))
        .await;
    assert_eq!(
        viewport_width(&mut watcher).await,
        500,
        "the newest is in force"
    );

    drop(a);
    width_settles_at(&mut watcher, 500).await;
    assert_eq!(
        viewport_width(&mut watcher).await,
        500,
        "A leaving keeps B's override"
    );
    drop(b);
    width_settles_at(&mut watcher, pristine).await;

    // The other order: the older override leaves last and the page is still
    // restored to the pristine viewport, not to the first override.
    let mut a = fx.attach(&target).await;
    let mut b = fx.attach(&target).await;
    a.result("Emulation.setDeviceMetricsOverride", set(300))
        .await;
    b.result("Emulation.setDeviceMetricsOverride", set(500))
        .await;
    drop(b);
    width_settles_at(&mut watcher, 300).await;
    drop(a);
    width_settles_at(&mut watcher, pristine).await;
    fx.stop().await;
}

const NAV: &str = "<!doctype html><html><body style='margin:0'>\
    <a id=l href='/dest' style='position:absolute;left:10px;top:10px;width:80px;height:30px;display:block'>go</a>\
    <a id=frag href='#x' style='position:absolute;left:10px;top:60px;width:80px;height:30px;display:block'>f</a>\
    <form id=g action='/dest' method=get>\
    <input name=q value='a b' style='position:absolute;left:10px;top:110px;width:100px;height:20px;margin:0'>\
    <input type=checkbox name=c checked style='position:absolute;left:150px;top:110px;width:20px;height:20px;margin:0'>\
    <button id=s type=submit name=go value=1 style='position:absolute;left:10px;top:150px;width:80px;height:30px;margin:0;padding:0'>s</button></form>\
    <form id=p action='/dest' method=post>\
    <button id=ps type=submit style='position:absolute;left:10px;top:200px;width:80px;height:30px;margin:0;padding:0'>p</button></form>\
    <a id=f1 href='file:///etc/hosts' style='position:absolute;left:300px;top:60px;width:80px;height:30px;display:block'>file</a>\
    <a id=j1 href='javascript:void(0)' style='position:absolute;left:300px;top:110px;width:80px;height:30px;display:block'>js</a>\
    <form action='file:///etc/hosts'><button id=ff type=submit style='position:absolute;left:300px;top:160px;width:80px;height:30px;margin:0;padding:0'>f</button></form>\
    <form action='javascript:void(0)'><button id=jf type=submit style='position:absolute;left:300px;top:210px;width:80px;height:30px;margin:0;padding:0'>j</button></form>\
    <form action='/dest'><input name=t value=\"a&b=c!(~)'\nx\"><input name=m value=m><select name=s multiple><option value=1 selected>1</option><option selected>two</option><option value=3>3</option></select>\
    <button id=fa type=submit formaction='/dest2' name=z value=z style='position:absolute;left:300px;top:260px;width:80px;height:30px;margin:0;padding:0'>fa</button></form>\
    <div id=ce contenteditable style='position:absolute;left:200px;top:10px;width:100px;height:30px'>x</div>\
    </body></html>";

async fn nav_fixture() -> (Fixture, Page) {
    let fx = Fixture::start(&[
        ("/nav", NAV),
        (
            "/dest",
            "<!doctype html><title>dest</title><body>arrived</body>",
        ),
    ])
    .await;
    let mut page = fx.page().await;
    page.begin().await;
    let url = fx.site.url("/nav");
    page.navigate(&url).await;
    page.event("Page.loadEventFired").await;
    (fx, page)
}

async fn navigated_to(page: &mut Page) -> String {
    loop {
        let event = page.event("Page.frameNavigated").await;
        let url = event["frame"]["url"].as_str().unwrap().to_owned();
        if url.contains("/dest") {
            return url;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clicking_a_link_or_submitting_a_get_form_navigates_and_the_rest_does_not() {
    let (fx, mut page) = nav_fixture().await;
    click(&mut page, 20.0, 20.0).await;
    assert!(navigated_to(&mut page).await.ends_with("/dest"));

    let (fx2, mut page) = nav_fixture().await;
    click(&mut page, 160.0, 120.0).await; // uncheck the checkbox
    click(&mut page, 30.0, 160.0).await; // the submit button
    let url = navigated_to(&mut page).await;
    assert!(url.ends_with("/dest?q=a+b&go=1"), "{url}");
    fx.stop().await;
    fx2.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_default_actions_leave_the_document_intact() {
    let (fx, mut page) = nav_fixture().await;
    page.evaluate("document.body.setAttribute('data-alive', 'yes')")
        .await;
    for (x, y) in [
        (310.0, 70.0),
        (310.0, 120.0),
        (310.0, 170.0),
        (310.0, 220.0),
    ] {
        click(&mut page, x, y).await;
    }
    // Give a navigation that wrongly started time to replace the document.
    for _ in 0..5 {
        page.result("Browser.getVersion", json!({})).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        page.evaluate("document.body.getAttribute('data-alive')")
            .await,
        "yes",
        "the document is still the page"
    );
    assert!(fx.server.backend().targets()[0].url().ends_with("/nav"));
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_forms_encode_like_the_html_serializer_and_honour_formaction() {
    let (fx, mut page) = nav_fixture().await;
    click(&mut page, 310.0, 270.0).await;
    let url = loop {
        let event = page.event("Page.frameNavigated").await;
        let url = event["frame"]["url"].as_str().unwrap().to_owned();
        if url.contains("/dest2") {
            break url;
        }
    };
    assert!(
        url.ends_with("/dest2?t=a%26b%3Dc%21%28%7E%29%27%0D%0Ax&m=m&s=1&s=two&z=z"),
        "{url}"
    );
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_post_form_is_submitted_by_the_page_itself() {
    // The engine builds the entity body and starts the navigation; the host
    // learns of it from the frame's events, as for any other navigation.
    let (fx, mut page) = nav_fixture().await;
    click(&mut page, 20.0, 215.0).await;
    assert!(navigated_to(&mut page).await.ends_with("/dest"));
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fragment_link_and_enter_in_a_get_form_behave() {
    let (fx, mut page) = nav_fixture().await;
    // A link into the document moves to the fragment and loads nothing.
    click(&mut page, 20.0, 70.0).await;
    page.result("Browser.getVersion", json!({})).await;
    assert_eq!(
        page.evaluate(
            "document.getElementsByTagName('title').length + '|' + location.hash + '|' + \
             history.length"
        )
        .await,
        "0|#x|2",
        "still on the first page, at the fragment"
    );
    // Enter in a text field of a GET form submits through its button.
    click(&mut page, 30.0, 120.0).await;
    key(&mut page, "Enter", Some("\r")).await;
    let url = navigated_to(&mut page).await;
    assert!(url.contains("/dest?q=a+b&c=on&go=1"), "{url}");
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typing_into_contenteditable_is_an_error_not_a_silent_no_op() {
    let (fx, mut page) = nav_fixture().await;
    click(&mut page, 210.0, 20.0).await;
    assert_eq!(page.evaluate("document.activeElement.id").await, "ce");
    let refused = page.call("Input.insertText", json!({ "text": "hi" })).await;
    let (code, message) = error_of(&refused);
    assert_eq!(code, -32000);
    assert!(message.contains("contenteditable"), "{message}");
    let key = page
        .call(
            "Input.dispatchKeyEvent",
            json!({ "type": "keyDown", "key": "a", "text": "a" }),
        )
        .await;
    assert_eq!(error_of(&key).0, -32000);
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutation_observers_get_records_only_for_real_changes() {
    let (fx, mut page) = form().await;
    let result = page
        .result(
            "Runtime.evaluate",
            json!({ "awaitPromise": true, "returnByValue": true, "expression":
                "new Promise(function (resolve) { \
                   var calls = 0; var seen = []; \
                   var observer = new MutationObserver(function (records) { \
                     calls += 1; \
                     records.forEach(function (r) { seen.push(r.type + ':' + (r.target.id || r.target.localName) + ':' + (r.attributeName || '') + ':' + r.addedNodes.length); }); }); \
                   observer.observe(document, { childList: true, attributes: true, subtree: true }); \
                   setTimeout(function () { \
                     var quiet = calls; \
                     document.getElementById('out').setAttribute('data-x', '1'); \
                     var p = document.createElement('p'); p.id = 'added'; document.body.appendChild(p); \
                     setTimeout(function () { observer.disconnect(); resolve({ quiet: quiet, seen: seen }); }, 200); \
                   }, 200); })" }),
        )
        .await;
    let value = &result["result"]["value"];
    assert_eq!(value["quiet"], 0, "no callback without a change");
    let seen: Vec<&str> = value["seen"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(seen.contains(&"attributes:out:data-x:0"), "{seen:?}");
    assert!(seen.contains(&"childList:body::1"), "{seen:?}");
    fx.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tree_walkers_visit_elements_in_document_order() {
    let (fx, mut page) = form().await;
    let names = page
        .evaluate(
            "(function () { var w = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT); \
               var out = [w.currentNode.localName]; var n; while ((n = w.nextNode())) out.push(n.localName); \
               return out.slice(0, 4).join(); })()",
        )
        .await;
    assert_eq!(names, "body,form,input,input");
    fx.stop().await;
}

/// Loads 16,000 list items with and without a page `MutationObserver` on the
/// whole document and returns the load time.
async fn load_big_page(observed: bool) -> std::time::Duration {
    let items: String = (0..16_000)
        .map(|i| format!("<li id=i{i}>{i}</li>"))
        .collect();
    let script = if observed {
        "new MutationObserver(function () {}).observe(document, { childList: true, subtree: true, attributes: true });"
    } else {
        ""
    };
    let html = format!("<!doctype html><body><ul>{items}</ul><script>{script}</script></body>");
    let fx = Fixture::start(&[("/big", html.as_str())]).await;
    let mut page = fx.page().await;
    page.begin().await;
    let started = std::time::Instant::now();
    let url = fx.site.url("/big");
    page.navigate(&url).await;
    page.event("Page.loadEventFired").await;
    let loaded = started.elapsed();
    let probe = std::time::Instant::now();
    for _ in 0..5 {
        page.evaluate("1").await;
    }
    assert!(
        probe.elapsed() / 5 < std::time::Duration::from_secs(1),
        "evaluate stays quick"
    );
    fx.stop().await;
    loaded
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observing_a_large_tree_does_not_slow_the_page_down() {
    let plain = load_big_page(false).await;
    let observed = load_big_page(true).await;
    assert!(
        observed < plain * 2 + std::time::Duration::from_secs(1),
        "observed load {observed:?} against {plain:?}"
    );
}
