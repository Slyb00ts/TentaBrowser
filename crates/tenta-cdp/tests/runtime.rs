//! The Runtime domain against a real V8 page, in the command sequences
//! Puppeteer and Playwright send: contexts, evaluation, handles, properties,
//! promises, exceptions, console events and bindings.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tenta_browser::{Engine, Goto, JsEngine};
use tenta_cdp::{CdpOptions, CdpServer};
use tenta_core::Tier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

/// A bound for tests that would otherwise hang on a regression. It is never
/// waited out on a passing run.
const PATIENCE: Duration = Duration::from_secs(30);

const STALE_CONTEXT: &str = "Cannot find context with specified id";

// ---- fixture ------------------------------------------------------------

/// Serves fixed pages by path on a loopback port.
struct Site {
    base: String,
    /// `/slow` answers once this is notified.
    gate: Arc<Notify>,
}

impl Site {
    async fn start(pages: &[(&str, &str)]) -> Self {
        let pages: Arc<HashMap<String, String>> = Arc::new(
            pages
                .iter()
                .map(|(path, body)| ((*path).to_owned(), (*body).to_owned()))
                .collect(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let gate = Arc::new(Notify::new());
        let held = Arc::clone(&gate);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let pages = Arc::clone(&pages);
                let held = Arc::clone(&held);
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut chunk = [0_u8; 2048];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => head.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let head = String::from_utf8_lossy(&head);
                    let path = head.split_whitespace().nth(1).unwrap_or("/");
                    if path == "/slow" {
                        held.notified().await;
                    }
                    let (status, body) = match pages.get(path) {
                        Some(body) => ("200 OK", body.as_str()),
                        None => ("404 Not Found", "not found"),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self { base, gate }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

struct Fixture {
    server: CdpServer,
    engine: Engine,
    site: Site,
}

impl Fixture {
    async fn start(pages: &[(&str, &str)]) -> Self {
        let engine = Engine::builder()
            .js(JsEngine::V8)
            .page_threads(1)
            .ephemeral(true)
            .max_tier(Tier::Js)
            .build()
            .expect("engine");
        let mut options = CdpOptions {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..CdpOptions::default()
        };
        options.default_context.allow_private_network = true;
        let server = CdpServer::bind(engine.clone(), options)
            .await
            .expect("bind");
        Self {
            server,
            engine,
            site: Site::start(pages).await,
        }
    }

    /// A page session on a blank page, as a client holds it after
    /// `Target.attachToTarget`.
    async fn page(&self) -> Page {
        let mut client = Client::connect(&self.server.browser_ws_url()).await;
        let target = client
            .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
            .await["targetId"]
            .as_str()
            .unwrap()
            .to_owned();
        let session = client
            .result(
                None,
                "Target.attachToTarget",
                json!({ "targetId": target, "flatten": true }),
            )
            .await["sessionId"]
            .as_str()
            .unwrap()
            .to_owned();
        Page {
            client,
            session,
            target,
        }
    }

    /// Navigates the page the way the Page domain will: through the engine.
    async fn goto(&self, page: &Page, path: &str) {
        let target = self.server.backend().target(&page.target).expect("target");
        target
            .page()
            .goto(&self.site.url(path), Goto::default())
            .await
            .expect("goto");
    }

    async fn stop(self) {
        self.server.shutdown().await;
        self.engine.close();
    }
}

struct Client {
    ws: WebSocketStream<TcpStream>,
    next_id: u64,
    backlog: Vec<Value>,
}

impl Client {
    async fn connect(url: &str) -> Self {
        let request = url.into_client_request().expect("request");
        let authority = request.uri().authority().expect("authority").to_string();
        let stream = TcpStream::connect(authority).await.expect("tcp");
        let (ws, _) = client_async(request, stream).await.expect("handshake");
        Self {
            ws,
            next_id: 1,
            backlog: Vec::new(),
        }
    }

    async fn read_wire(&mut self) -> Value {
        loop {
            let frame = tokio::time::timeout(PATIENCE, self.ws.next())
                .await
                .expect("timed out waiting for a message")
                .expect("connection ended")
                .expect("frame");
            if let Message::Text(text) = frame {
                return serde_json::from_str(text.as_str()).expect("json");
            }
        }
    }

    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.ws
            .send(Message::text(message.to_string()))
            .await
            .expect("send");
        if let Some(index) = self.backlog.iter().position(|m| m["id"] == id) {
            return self.backlog.remove(index);
        }
        loop {
            let message = self.read_wire().await;
            if message["id"] == id {
                return message;
            }
            self.backlog.push(message);
        }
    }

    async fn result(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let response = self.call(session, method, params).await;
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response["result"].clone()
    }

    /// The next event named `method`, from the backlog first.
    async fn event(&mut self, method: &str) -> Value {
        if let Some(index) = self.backlog.iter().position(|m| m["method"] == method) {
            return self.backlog.remove(index)["params"].clone();
        }
        loop {
            let message = self.read_wire().await;
            if message["method"] == method {
                return message["params"].clone();
            }
            self.backlog.push(message);
        }
    }

    /// The methods of the events received so far, oldest first.
    fn events_so_far(&self) -> Vec<String> {
        self.backlog
            .iter()
            .filter_map(|m| m["method"].as_str().map(str::to_owned))
            .collect()
    }
}

/// A client attached to one page.
struct Page {
    client: Client,
    session: String,
    target: String,
}

impl Page {
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.client.call(Some(&session), method, params).await
    }

    async fn result(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.client.result(Some(&session), method, params).await
    }

    async fn event(&mut self, method: &str) -> Value {
        self.client.event(method).await
    }

    async fn evaluate(&mut self, expression: &str, extra: Value) -> Value {
        let mut params = json!({ "expression": expression });
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        self.result("Runtime.evaluate", params).await
    }

    /// `Runtime.enable` and the context it announces, the way both clients
    /// begin.
    async fn enable(&mut self) -> Value {
        self.result("Runtime.enable", json!({})).await;
        self.event("Runtime.executionContextCreated").await["context"].clone()
    }
}

const INDEX: &str = "<!doctype html><html><body><div id=\"m\" class=\"a b\">hello</div>\
    <script>console.log('from parse', 7);</script></body></html>";

fn error_of(response: &Value) -> (i64, &str) {
    (
        response["error"]["code"].as_i64().expect("an error"),
        response["error"]["message"].as_str().unwrap_or_default(),
    )
}

// ---- contexts -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contexts_follow_the_documents_of_the_page() {
    let fixture = Fixture::start(&[("/", INDEX), ("/two", "<p id=\"two\">two</p>")]).await;
    let mut page = fixture.page().await;

    // Enabled on the blank document; the first navigation replaces its context.
    let blank = page.enable().await;
    fixture.goto(&page, "/").await;
    let gone = page.event("Runtime.executionContextDestroyed").await;
    assert_eq!(gone["executionContextId"], blank["id"]);
    page.event("Runtime.executionContextsCleared").await;
    let first = page.event("Runtime.executionContextCreated").await["context"].clone();
    assert_eq!(first["auxData"]["isDefault"], true);
    assert_eq!(first["auxData"]["type"], "default");
    assert_eq!(first["auxData"]["frameId"], page.target.as_str());
    let first_id = first["id"].as_u64().expect("numeric id");
    let first_unique = first["uniqueId"].as_str().expect("uniqueId").to_owned();

    // A handle and a by-id evaluation in the first context.
    let handle = page
        .evaluate("({ n: 1 })", json!({ "contextId": first_id }))
        .await["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_owned();

    fixture.goto(&page, "/two").await;
    let destroyed = page.event("Runtime.executionContextDestroyed").await;
    assert_eq!(destroyed["executionContextId"], first_id);
    page.event("Runtime.executionContextsCleared").await;
    let second = page.event("Runtime.executionContextCreated").await["context"].clone();
    let second_id = second["id"].as_u64().unwrap();
    assert!(second_id > first_id);

    // Anything aimed at the old context is refused with Chromium's message.
    let stale_eval = page
        .call(
            "Runtime.evaluate",
            json!({ "expression": "1", "contextId": first_id }),
        )
        .await;
    assert_eq!(error_of(&stale_eval), (-32000, STALE_CONTEXT));
    let stale_unique = page
        .call(
            "Runtime.evaluate",
            json!({ "expression": "1", "uniqueContextId": first_unique }),
        )
        .await;
    assert_eq!(error_of(&stale_unique), (-32000, STALE_CONTEXT));
    let stale_call = page
        .call(
            "Runtime.callFunctionOn",
            json!({ "functionDeclaration": "function () { return 1; }", "objectId": handle }),
        )
        .await;
    assert_eq!(error_of(&stale_call), (-32000, STALE_CONTEXT));
    let stale_properties = page
        .call("Runtime.getProperties", json!({ "objectId": handle }))
        .await;
    assert_eq!(error_of(&stale_properties), (-32000, STALE_CONTEXT));

    let live = page
        .evaluate(
            "document.getElementById('two').textContent",
            json!({
                "contextId": second_id,
                "returnByValue": true,
            }),
        )
        .await;
    assert_eq!(live["result"], json!({ "type": "string", "value": "two" }));

    // Enabling twice is one enable; disabling clears the contexts and a later
    // enable announces the live one again.
    page.result("Runtime.enable", json!({})).await;
    page.result("Runtime.disable", json!({})).await;
    page.event("Runtime.executionContextsCleared").await;
    assert!(
        !page
            .client
            .events_so_far()
            .contains(&"Runtime.executionContextCreated".to_owned())
    );
    page.result("Runtime.enable", json!({})).await;
    let again = page.event("Runtime.executionContextCreated").await;
    assert_eq!(again["context"]["id"], second_id);

    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enable_after_navigation_answers_after_announcing_the_context() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;

    page.call("Runtime.enable", json!({})).await;
    // The context precedes the response in the stream, so a client that
    // starts evaluating once enable returns already knows it.
    assert!(
        page.client
            .events_so_far()
            .contains(&"Runtime.executionContextCreated".to_owned())
    );
    let context = page.event("Runtime.executionContextCreated").await["context"].clone();
    assert!(
        context["origin"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );
    fixture.stop().await;
}

// ---- evaluation ---------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn evaluate_returns_primitives_special_numbers_and_values() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;

    for (expression, expected) in [
        (
            "1 + 1",
            json!({ "type": "number", "value": 2, "description": "2" }),
        ),
        (
            "1.5",
            json!({ "type": "number", "value": 1.5, "description": "1.5" }),
        ),
        ("'text'", json!({ "type": "string", "value": "text" })),
        ("true", json!({ "type": "boolean", "value": true })),
        ("undefined", json!({ "type": "undefined" })),
        (
            "null",
            json!({ "type": "object", "subtype": "null", "value": null }),
        ),
        (
            "NaN",
            json!({ "type": "number", "unserializableValue": "NaN", "description": "NaN" }),
        ),
        (
            "-0",
            json!({ "type": "number", "unserializableValue": "-0", "description": "0" }),
        ),
        (
            "1 / 0",
            json!({ "type": "number", "unserializableValue": "Infinity", "description": "Infinity" }),
        ),
        (
            "-1 / 0",
            json!({ "type": "number", "unserializableValue": "-Infinity", "description": "-Infinity" }),
        ),
        (
            "123n",
            json!({ "type": "bigint", "unserializableValue": "123n", "description": "123n" }),
        ),
    ] {
        let result = page.evaluate(expression, json!({})).await;
        assert_eq!(result["result"], expected, "{expression}");
        assert!(result.get("exceptionDetails").is_none(), "{expression}");
    }

    let by_value = page
        .evaluate(
            "({ a: 1, b: [true, null, 'x'], c: { d: 2.5 }, f() {}, n: NaN })",
            json!({ "returnByValue": true }),
        )
        .await;
    assert_eq!(
        by_value["result"],
        json!({ "type": "object", "value": { "a": 1, "b": [true, null, "x"], "c": { "d": 2.5 }, "n": null } })
    );

    // A cycle cannot be copied: a command error, as in Chromium.
    let cycle = page
        .call(
            "Runtime.evaluate",
            json!({ "expression": "const o = {}; o.o = o; o", "returnByValue": true }),
        )
        .await;
    assert_eq!(error_of(&cycle).0, -32000);

    let unsupported = page
        .call(
            "Runtime.evaluate",
            json!({ "expression": "1", "throwOnSideEffect": true }),
        )
        .await;
    assert_eq!(error_of(&unsupported).0, -32000);

    // Objects stay in the page and describe themselves, with a preview.
    let object = page
        .evaluate(
            "({ a: 1, b: 'two', c: [1] })",
            json!({ "generatePreview": true, "objectGroup": "g" }),
        )
        .await;
    let result = &object["result"];
    assert_eq!(result["type"], "object");
    assert_eq!(result["className"], "Object");
    assert!(result["objectId"].as_str().unwrap().contains('.'));
    let preview = &result["preview"];
    assert_eq!(preview["type"], "object");
    assert_eq!(preview["overflow"], false);
    assert_eq!(
        preview["properties"][0],
        json!({ "name": "a", "type": "number", "value": "1" })
    );
    assert_eq!(preview["properties"][1]["value"], "two");
    assert_eq!(preview["properties"][2]["type"], "object");
    assert_eq!(preview["properties"][2]["subtype"], "array");

    let element = page
        .evaluate("document.getElementById('m')", json!({}))
        .await;
    assert_eq!(element["result"]["subtype"], "node");
    assert_eq!(element["result"]["className"], "HTMLDivElement");
    assert_eq!(element["result"]["description"], "div#m.a.b");

    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_function_on_binds_this_and_takes_every_argument_kind() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;

    let receiver = page.evaluate("({ n: 40 })", json!({})).await["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = page.evaluate("({ m: 100 })", json!({})).await["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_owned();

    let sum = page
        .result(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": "function (x, other) { return this.n + x + other.m; }",
                "objectId": receiver,
                "arguments": [{ "value": 2 }, { "objectId": other }],
                "returnByValue": true,
            }),
        )
        .await;
    assert_eq!(sum["result"]["value"], 142);

    // A handle result joins the group asked for and releases with it.
    let made = page
        .result(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": "function () { return { twice: this.n * 2 }; }",
                "objectId": receiver,
                "objectGroup": "made",
                "generatePreview": true,
            }),
        )
        .await;
    let made_id = made["result"]["objectId"].as_str().unwrap().to_owned();
    assert_eq!(made["result"]["preview"]["properties"][0]["value"], "80");
    page.result(
        "Runtime.releaseObjectGroup",
        json!({ "objectGroup": "made" }),
    )
    .await;
    let released = page
        .call(
            "Runtime.callFunctionOn",
            json!({ "functionDeclaration": "function () { return 1; }", "objectId": made_id }),
        )
        .await;
    assert_eq!(
        error_of(&released),
        (-32000, "Could not find object with given id")
    );

    // releaseObject frees one handle; the next use of it fails the same way.
    page.result("Runtime.releaseObject", json!({ "objectId": receiver }))
        .await;
    let gone = page
        .call(
            "Runtime.getProperties",
            json!({ "objectId": receiver, "ownProperties": true }),
        )
        .await;
    assert_eq!(
        error_of(&gone),
        (-32000, "Could not find object with given id")
    );

    // Neither a receiver nor a context.
    let nowhere = page
        .call(
            "Runtime.callFunctionOn",
            json!({ "functionDeclaration": "function () {}" }),
        )
        .await;
    assert_eq!(error_of(&nowhere).0, -32000);

    // Not a function: the exception comes back as details.
    let bad = page
        .result(
            "Runtime.callFunctionOn",
            json!({ "functionDeclaration": "42", "objectId": other }),
        )
        .await;
    assert!(bad["exceptionDetails"]["text"].is_string());

    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_function_on_a_context_runs_with_the_global_as_this() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;
    let context = page.enable().await;

    let result = page
        .result(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": "(a, b, c, d) => [1 / a, b, c, typeof d, this === globalThis]",
                "executionContextId": context["id"],
                "arguments": [
                    { "unserializableValue": "-0" },
                    { "unserializableValue": "7n" },
                    { "value": { "k": [1, "z"] } },
                    {},
                ],
                "returnByValue": true,
            }),
        )
        .await;
    assert_eq!(
        result["result"]["value"],
        json!([null, null, { "k": [1, "z"] }, "undefined", true])
    );
    fixture.stop().await;
}

// ---- getProperties ------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_properties_lists_an_array_the_way_puppeteer_reads_handles() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;

    let array = page
        .evaluate(
            "[document.getElementById('m'), 'two', { x: 3 }]",
            json!({ "objectGroup": "list" }),
        )
        .await;
    assert_eq!(array["result"]["subtype"], "array");
    let id = array["result"]["objectId"].as_str().unwrap().to_owned();

    let listed = page
        .result(
            "Runtime.getProperties",
            json!({ "objectId": id, "ownProperties": true, "generatePreview": true }),
        )
        .await;
    let properties = listed["result"].as_array().unwrap();
    let names: Vec<&str> = properties
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["0", "1", "2", "length", "__proto__"]);

    // What Puppeteer keeps: enumerable members that have a value.
    let kept: Vec<&Value> = properties
        .iter()
        .filter(|p| p["enumerable"] == true && p.get("value").is_some())
        .collect();
    assert_eq!(kept.len(), 3);
    assert_eq!(kept[0]["value"]["subtype"], "node");
    assert_eq!(kept[0]["value"]["description"], "div#m.a.b");
    assert_eq!(
        kept[1]["value"],
        json!({ "type": "string", "value": "two" })
    );
    assert_eq!(kept[2]["value"]["preview"]["properties"][0]["value"], "3");
    assert!(
        kept.iter()
            .all(|p| p["isOwn"] == true && p["writable"] == true)
    );

    // The element handle from the list is usable on its own.
    let element = kept[0]["value"]["objectId"].as_str().unwrap();
    let text = page
        .result(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": "function () { return this.textContent; }",
                "objectId": element,
                "returnByValue": true,
            }),
        )
        .await;
    assert_eq!(text["result"]["value"], "hello");

    // The members the list made belong to the array's group.
    page.result(
        "Runtime.releaseObjectGroup",
        json!({ "objectGroup": "list" }),
    )
    .await;
    let released = page
        .call(
            "Runtime.getProperties",
            json!({ "objectId": element, "ownProperties": true }),
        )
        .await;
    assert_eq!(error_of(&released).1, "Could not find object with given id");

    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_properties_walks_the_prototype_chain_of_an_element() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;

    let element = page
        .evaluate("document.getElementById('m')", json!({}))
        .await["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_owned();

    let own = page
        .result(
            "Runtime.getProperties",
            json!({ "objectId": element, "ownProperties": true }),
        )
        .await;
    assert!(
        own["result"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["isOwn"] == true)
    );

    let inherited = page
        .result("Runtime.getProperties", json!({ "objectId": element }))
        .await;
    let id = inherited["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "id")
        .expect("Element.prototype.id");
    assert_eq!(id["isOwn"], false);
    assert_eq!(id["get"]["type"], "function");
    assert!(id.get("value").is_none());

    let accessors = page
        .result(
            "Runtime.getProperties",
            json!({ "objectId": element, "accessorPropertiesOnly": true }),
        )
        .await;
    let accessors = accessors["result"].as_array().unwrap();
    assert!(!accessors.is_empty());
    assert!(
        accessors
            .iter()
            .all(|p| p.get("get").is_some() || p.get("set").is_some())
    );

    fixture.stop().await;
}

// ---- promises and exceptions --------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn await_promise_waits_for_the_settlement() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;

    let resolved = page
        .evaluate(
            "new Promise((resolve) => setTimeout(() => resolve({ ok: 1 }), 20))",
            json!({ "awaitPromise": true, "returnByValue": true }),
        )
        .await;
    assert_eq!(resolved["result"]["value"], json!({ "ok": 1 }));

    // Without awaiting, the promise itself comes back.
    let promise = page.evaluate("Promise.resolve(1)", json!({})).await;
    assert_eq!(promise["result"]["subtype"], "promise");

    let rejected = page
        .evaluate(
            "Promise.reject(new Error('nope'))",
            json!({ "awaitPromise": true }),
        )
        .await;
    assert!(
        rejected["exceptionDetails"]["exception"]["description"]
            .as_str()
            .unwrap()
            .starts_with("Error: nope")
    );

    let context = page.enable().await["id"].clone();
    let via_function = page
        .result(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": "async function (ms) { await new Promise((r) => setTimeout(r, ms)); return 'done'; }",
                "executionContextId": context,
                "arguments": [{ "value": 10 }],
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await;
    assert_eq!(via_function["result"]["value"], "done");

    // The deadline ends a wait that never settles.
    let timed_out = page
        .evaluate(
            "new Promise(() => {})",
            json!({ "awaitPromise": true, "timeout": 50 }),
        )
        .await;
    assert_eq!(
        timed_out["exceptionDetails"]["exception"]["description"],
        "Error: Execution was terminated"
    );

    // A navigation ends the realm a wait belongs to.
    let session = page.session.clone();
    let id = page.client.next_id;
    page.client
        .ws
        .send(Message::text(
            json!({
                "id": id,
                "sessionId": session,
                "method": "Runtime.evaluate",
                "params": { "expression": "new Promise(() => {})", "awaitPromise": true },
            })
            .to_string(),
        ))
        .await
        .unwrap();
    page.client.next_id += 1;
    fixture.goto(&page, "/").await;
    loop {
        let message = page.client.read_wire().await;
        if message["id"] == id {
            assert_eq!(
                error_of(&message),
                (-32000, "Execution context was destroyed.")
            );
            break;
        }
        page.client.backlog.push(message);
    }

    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thrown_error_is_exception_details_not_a_protocol_error() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;
    let context = page.enable().await["id"].clone();

    let thrown = page
        .evaluate(
            "function boom() { throw new TypeError('bad'); }\nboom();",
            json!({}),
        )
        .await;
    let details = &thrown["exceptionDetails"];
    assert_eq!(details["text"], "Uncaught");
    assert_eq!(details["executionContextId"], context);
    assert_eq!(details["lineNumber"], 0);
    assert!(details["exceptionId"].as_u64().unwrap() >= 1);
    assert_eq!(details["exception"]["subtype"], "error");
    assert_eq!(details["exception"]["className"], "TypeError");
    assert!(
        details["exception"]["description"]
            .as_str()
            .unwrap()
            .starts_with("TypeError: bad\n")
    );
    assert_eq!(
        details["stackTrace"]["callFrames"][0]["functionName"],
        "boom"
    );
    // The thrown value is also the result, as Chromium reports it.
    assert_eq!(thrown["result"], details["exception"]);

    let primitive = page.evaluate("throw 'plain'", json!({})).await;
    assert_eq!(
        primitive["exceptionDetails"]["exception"],
        json!({ "type": "string", "value": "plain" })
    );

    let syntax = page.evaluate("let = = 1", json!({})).await;
    assert_eq!(syntax["result"]["className"], "SyntaxError");
    assert!(syntax["exceptionDetails"].is_object());

    fixture.stop().await;
}

// ---- console, exceptions and bindings ------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_calls_arrive_as_console_api_called() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    page.enable().await;

    // The script of the document runs before the navigation commits; its
    // context is announced before its first message.
    fixture.goto(&page, "/").await;
    let created = page.event("Runtime.executionContextCreated").await["context"].clone();
    let parsed = page.event("Runtime.consoleAPICalled").await;
    assert_eq!(parsed["type"], "log");
    assert_eq!(parsed["executionContextId"], created["id"]);
    assert_eq!(
        parsed["args"][0],
        json!({ "type": "string", "value": "from parse" })
    );
    assert_eq!(parsed["args"][1]["value"], 7);
    assert!(parsed["timestamp"].as_f64().unwrap() > 1.0e12);
    assert!(parsed["stackTrace"]["callFrames"].is_array());

    page.evaluate(
        "console.warn('careful'); console.error({ a: 1 }, [1, 2]); console.debug('d'); \
         console.info('i'); console.assert(false, 'nope'); console.count(); console.trace('t')",
        json!({}),
    )
    .await;
    let mut seen = Vec::new();
    for _ in 0..7 {
        let call = page.event("Runtime.consoleAPICalled").await;
        seen.push(call["type"].as_str().unwrap().to_owned());
        if call["type"] == "error" {
            assert_eq!(call["args"][0]["type"], "object");
            assert_eq!(call["args"][0]["preview"]["properties"][0]["name"], "a");
            assert_eq!(call["args"][1]["subtype"], "array");
            assert_eq!(call["args"][1]["description"], "Array(2)");
        }
    }
    assert_eq!(
        seen,
        [
            "warning", "error", "debug", "info", "assert", "count", "trace"
        ]
    );

    // While disabled nothing is forwarded.
    page.result("Runtime.disable", json!({})).await;
    page.evaluate("console.log('silent')", json!({})).await;
    page.result("Runtime.enable", json!({})).await;
    page.evaluate("console.log('loud')", json!({})).await;
    let loud = loop {
        let call = page.event("Runtime.consoleAPICalled").await;
        if call["args"][0]["value"] == "loud" {
            break call;
        }
    };
    assert_eq!(loud["args"][0]["value"], "loud");

    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncaught_errors_arrive_as_exception_thrown() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;
    let context = page.enable().await["id"].clone();

    page.evaluate(
        "setTimeout(() => { throw new RangeError('late'); }, 0)",
        json!({}),
    )
    .await;
    let thrown = page.event("Runtime.exceptionThrown").await;
    assert!(thrown["timestamp"].as_f64().unwrap() > 1.0e12);
    let details = &thrown["exceptionDetails"];
    assert_eq!(details["text"], "Uncaught");
    assert_eq!(details["executionContextId"], context);
    assert_eq!(details["exception"]["className"], "RangeError");
    assert!(
        details["exception"]["description"]
            .as_str()
            .unwrap()
            .starts_with("RangeError: late")
    );

    page.evaluate("Promise.reject(new Error('unhandled'))", json!({}))
        .await;
    let rejection = page.event("Runtime.exceptionThrown").await;
    assert_eq!(
        rejection["exceptionDetails"]["text"],
        "Uncaught (in promise)"
    );

    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bindings_report_calls_and_survive_navigation() {
    let fixture = Fixture::start(&[("/", INDEX), ("/two", "<p>two</p>")]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;
    let context = page.enable().await["id"].clone();

    page.result(
        "Runtime.addBinding",
        json!({ "name": "report", "executionContextName": "ignored" }),
    )
    .await;
    page.evaluate("report('first')", json!({})).await;
    let called = page.event("Runtime.bindingCalled").await;
    assert_eq!(
        called,
        json!({ "name": "report", "payload": "first", "executionContextId": context })
    );

    fixture.goto(&page, "/two").await;
    let next = page.event("Runtime.executionContextCreated").await["context"].clone();
    page.evaluate("report('second')", json!({})).await;
    let again = page.event("Runtime.bindingCalled").await;
    assert_eq!(again["payload"], "second");
    assert_eq!(again["executionContextId"], next["id"]);

    page.result("Runtime.removeBinding", json!({ "name": "report" }))
        .await;
    let removed = page
        .evaluate("typeof report", json!({ "returnByValue": true }))
        .await;
    assert_eq!(removed["result"]["value"], "undefined");

    let invalid = page
        .call("Runtime.addBinding", json!({ "name": "not valid" }))
        .await;
    assert_eq!(error_of(&invalid).0, -32000);

    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn small_commands_accept_what_the_clients_send() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;
    page.result("Runtime.discardConsoleEntries", json!({}))
        .await;
    page.result("Runtime.runIfWaitingForDebugger", json!({}))
        .await;
    let missing = page.call("Runtime.evaluate", json!({})).await;
    assert_eq!(error_of(&missing).0, -32602);
    let unknown = page.call("Runtime.queryObjects", json!({})).await;
    assert_eq!(error_of(&unknown).0, -32601);
    fixture.stop().await;
}

// ---- the initial document -------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_page_has_a_blank_document_with_a_context() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;

    let context = page.enable().await;
    assert_eq!(context["origin"], "://");
    assert_eq!(context["auxData"]["isDefault"], true);
    assert_eq!(context["auxData"]["type"], "default");
    assert_eq!(context["auxData"]["frameId"], page.target.as_str());

    let sum = page
        .evaluate(
            "1 + 1",
            json!({ "contextId": context["id"], "returnByValue": true }),
        )
        .await;
    assert_eq!(sum["result"]["value"], 2);

    // The blank document is a real one, with a body to write into.
    // (`document.open/write/close` are not implemented by the engine yet.)
    page.evaluate(
        "document.body.innerHTML = '<p id=\\\"w\\\">written</p>'",
        json!({}),
    )
    .await;
    let body = page
        .evaluate("document.body.innerHTML", json!({ "returnByValue": true }))
        .await;
    assert_eq!(body["result"]["value"], "<p id=\"w\">written</p>");

    // The first navigation replaces it, with a new context.
    fixture.goto(&page, "/").await;
    page.event("Runtime.executionContextDestroyed").await;
    let next = page.event("Runtime.executionContextCreated").await["context"].clone();
    assert_ne!(next["id"], context["id"]);
    fixture.stop().await;
}

// ---- navigation in flight -------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enable_and_later_commands_do_not_wait_for_a_slow_navigation() {
    let fixture = Fixture::start(&[("/slow", INDEX)]).await;
    let mut page = fixture.page().await;
    let target = fixture
        .server
        .backend()
        .target(&page.target)
        .expect("target");
    let navigating = target.page().clone();
    let url = fixture.site.url("/slow");
    let navigation = tokio::spawn(async move { navigating.goto(&url, Goto::default()).await });
    while !target.page().navigation_in_flight() {
        tokio::task::yield_now().await;
    }

    // Both answers arrive while the document is still on its way.
    page.result("Runtime.enable", json!({})).await;
    page.result("Runtime.releaseObjectGroup", json!({ "objectGroup": "g" }))
        .await;
    assert!(target.page().navigation_in_flight());

    // A command that needs the script waits, without holding back the next.
    let waiting = page.client.next_id;
    let session = page.session.clone();
    page.client
        .ws
        .send(Message::text(
            json!({
                "id": waiting,
                "sessionId": session,
                "method": "Runtime.evaluate",
                "params": { "expression": "document.title", "returnByValue": true },
            })
            .to_string(),
        ))
        .await
        .unwrap();
    page.client.next_id += 1;
    page.result("Runtime.releaseObjectGroup", json!({ "objectGroup": "g" }))
        .await;
    assert!(target.page().navigation_in_flight());

    fixture.site.gate.notify_one();
    navigation.await.unwrap().expect("navigation");
    let answer = loop {
        let message = page.client.read_wire().await;
        if message["id"] == waiting {
            break message;
        }
        page.client.backlog.push(message);
    };
    assert!(answer.get("error").is_none(), "{answer}");
    page.event("Runtime.executionContextCreated").await;
    fixture.stop().await;
}

// ---- console replay -------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn messages_logged_before_enable_are_replayed_after_the_context() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;
    // The page logged 'from parse' while nobody was listening; the record is
    // kept by a task of the server, so ask until it has caught up.
    let mut replayed = None;
    for _ in 0..200 {
        page.result("Runtime.enable", json!({})).await;
        let methods = page.client.events_so_far();
        if methods.iter().any(|m| m == "Runtime.consoleAPICalled") {
            replayed = Some(methods);
            break;
        }
        page.result("Runtime.disable", json!({})).await;
        page.client.backlog.clear();
        tokio::task::yield_now().await;
    }
    let methods = replayed.expect("the earlier message is replayed");
    let created = methods
        .iter()
        .position(|m| m == "Runtime.executionContextCreated")
        .expect("created");
    let logged = methods
        .iter()
        .position(|m| m == "Runtime.consoleAPICalled")
        .unwrap();
    assert!(created < logged, "{methods:?}");
    let call = page.event("Runtime.consoleAPICalled").await;
    assert_eq!(call["args"][0]["value"], "from parse");
    let context = page.event("Runtime.executionContextCreated").await;
    assert_eq!(call["executionContextId"], context["context"]["id"]);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_function_on_refuses_a_receiver_together_with_a_context() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    fixture.goto(&page, "/").await;
    let context = page.enable().await["id"].clone();
    let object = page.evaluate("({})", json!({})).await["result"]["objectId"].clone();
    let both = page
        .call(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": "function () {}",
                "objectId": object,
                "executionContextId": context,
            }),
        )
        .await;
    assert_eq!(
        error_of(&both),
        (
            -32000,
            "ObjectId must not be specified together with executionContextId"
        )
    );
    let foreign = page
        .call(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": "function (x) {}",
                "objectId": object,
                "arguments": [{ "objectId": "9999.1" }],
            }),
        )
        .await;
    assert_eq!(
        error_of(&foreign),
        (
            -32000,
            "Argument should belong to the same JavaScript world as target object"
        )
    );
    fixture.stop().await;
}
