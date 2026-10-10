//! The Network and Fetch domains against a real page engine, in the command
//! sequences Puppeteer sends: events of a navigation with a redirect and
//! subresources, bodies, overrides, cookies, interception.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use common::{Fixture, Page};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ---- fixture ------------------------------------------------------------

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[derive(Clone)]
struct Route {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn route(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Route {
    Route {
        status,
        headers: vec![("Content-Type".to_owned(), content_type.to_owned())],
        body: body.into(),
    }
}

/// Every request the site saw: path, then lower-cased header names and values.
type Seen = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;

struct NetSite {
    base: String,
    seen: Seen,
}

impl NetSite {
    async fn start(routes: Vec<(&str, Route)>) -> Self {
        let routes: Arc<HashMap<String, Route>> = Arc::new(
            routes
                .into_iter()
                .map(|(path, route)| (path.to_owned(), route))
                .collect(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Seen = Arc::default();
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let (routes, log) = (Arc::clone(&routes), Arc::clone(&log));
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut chunk = [0_u8; 2048];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => head.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let text = String::from_utf8_lossy(&head).into_owned();
                    let mut lines = text.split("\r\n");
                    let path = lines
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_owned();
                    let headers = lines
                        .filter_map(|line| line.split_once(':'))
                        .map(|(name, value)| {
                            (name.trim().to_ascii_lowercase(), value.trim().to_owned())
                        })
                        .collect();
                    log.lock().unwrap().push((path.clone(), headers));
                    let route = routes
                        .get(path.split('?').next().unwrap_or(&path))
                        .cloned()
                        .unwrap_or_else(|| route(404, "text/plain", "not found"));
                    let reason = match route.status {
                        200 => "OK",
                        302 => "Found",
                        404 => "Not Found",
                        _ => "Status",
                    };
                    let mut response = format!("HTTP/1.1 {} {reason}\r\n", route.status);
                    for (name, value) in &route.headers {
                        response.push_str(&format!("{name}: {value}\r\n"));
                    }
                    response.push_str(&format!(
                        "Content-Length: {}\r\nConnection: close\r\n\r\n",
                        route.body.len()
                    ));
                    let mut bytes = response.into_bytes();
                    bytes.extend_from_slice(&route.body);
                    let _ = stream.write_all(&bytes).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self { base, seen }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The headers of the newest request for `path`.
    fn headers_of(&self, path: &str) -> Option<HashMap<String, String>> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(seen, _)| seen == path)
            .map(|(_, headers)| headers.clone())
    }

    fn count(&self, path: &str) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, _)| seen == path)
            .count()
    }
}

const DOC: &str = "<!doctype html><html><head><title>Net</title>\
    <link rel=stylesheet href=/s.css><script src=/a.js></script></head>\
    <body><img src=/i.png width=1 height=1><div id=out>idle</div></body></html>";

fn standard_site() -> Vec<(&'static str, Route)> {
    let mut redirect = route(302, "text/plain", "");
    redirect
        .headers
        .push(("Location".to_owned(), "/doc".to_owned()));
    vec![
        ("/doc", route(200, "text/html; charset=utf-8", DOC)),
        ("/go", redirect),
        ("/s.css", route(200, "text/css", "body{margin:0}")),
        (
            "/a.js",
            route(200, "application/javascript", "window.ran = 1;"),
        ),
        (
            "/i.png",
            route(
                200,
                "image/png",
                base64::engine::general_purpose::STANDARD
                    .decode(PNG)
                    .unwrap(),
            ),
        ),
        (
            "/data.json",
            route(200, "application/json", "{\"ok\":true}"),
        ),
        (
            "/blank",
            route(200, "text/html", "<!doctype html><title>b</title>"),
        ),
    ]
}

async fn start(routes: Vec<(&str, Route)>) -> (Fixture, NetSite, Page) {
    let fx = Fixture::start(&[]).await;
    let site = NetSite::start(routes).await;
    let mut page = fx.page().await;
    page.begin().await;
    page.result("Network.enable", json!({})).await;
    (fx, site, page)
}

/// Every `Network.*` event up to the `networkIdle` lifecycle event of `loader`.
async fn network_events(page: &mut Page, loader: &str) -> Vec<Value> {
    let mut events = Vec::new();
    loop {
        let event = page.client.next_event().await;
        let method = event["method"].as_str().unwrap_or_default();
        if method == "Page.lifecycleEvent"
            && event["params"]["loaderId"] == loader
            && event["params"]["name"] == "networkIdle"
        {
            return events;
        }
        if method.starts_with("Network.") {
            events.push(event["params"].clone().tap_method(method));
        }
    }
}

trait TapMethod {
    fn tap_method(self, method: &str) -> Value;
}

impl TapMethod for Value {
    /// The params with the event name beside them, for the assertions below.
    fn tap_method(mut self, method: &str) -> Value {
        self["event"] = json!(method);
        self
    }
}

/// A header of a protocol headers object, whatever the name's case.
fn header(headers: &Value, name: &str) -> Value {
    headers
        .as_object()
        .and_then(|headers| {
            headers
                .iter()
                .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        })
        .map_or(Value::Null, |(_, value)| value.clone())
}

fn of_kind<'a>(events: &'a [Value], name: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["event"] == format!("Network.{name}"))
        .collect()
}

fn position(events: &[Value], name: &str, request_id: &str) -> usize {
    events
        .iter()
        .position(|event| {
            event["event"] == format!("Network.{name}") && event["requestId"] == request_id
        })
        .unwrap_or_else(|| panic!("no {name} for {request_id}"))
}

// ---- tests --------------------------------------------------------------

#[tokio::test]
async fn a_navigation_reports_its_redirect_chain_and_every_subresource() {
    let (fx, site, mut page) = start(standard_site()).await;
    let loader = page.navigate(&site.url("/go")).await;
    let events = network_events(&mut page, &loader).await;

    // The document: one request id (the loader id) across the redirect.
    let documents: Vec<_> = of_kind(&events, "requestWillBeSent")
        .into_iter()
        .filter(|event| event["type"] == "Document")
        .collect();
    assert_eq!(documents.len(), 2, "two hops: {events:#?}");
    let first = documents[0];
    assert_eq!(first["requestId"], loader.as_str());
    assert_eq!(first["loaderId"], loader.as_str());
    assert_eq!(first["frameId"], page.target.as_str());
    assert_eq!(first["request"]["url"], site.url("/go"));
    assert_eq!(first["request"]["method"], "GET");
    assert_eq!(first["initiator"]["type"], "other");
    assert!(first.get("redirectResponse").is_none());
    let second = documents[1];
    assert_eq!(second["requestId"], loader.as_str());
    assert_eq!(second["request"]["url"], site.url("/doc"));
    assert_eq!(second["redirectResponse"]["status"], 302);
    assert_eq!(second["redirectResponse"]["url"], site.url("/go"));
    let redirect_headers = second["redirectResponse"]["headers"].as_object().unwrap();
    assert!(
        redirect_headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("location") && value == "/doc"),
        "{redirect_headers:?}"
    );
    assert_eq!(second["documentURL"], site.url("/doc"));

    // The document's response and end, in order, with the body length.
    let received = position(&events, "responseReceived", &loader);
    let finished = position(&events, "loadingFinished", &loader);
    let second_sent = events
        .iter()
        .rposition(|event| {
            event["event"] == "Network.requestWillBeSent" && event["requestId"] == loader.as_str()
        })
        .unwrap();
    assert!(second_sent < received && received < finished);
    let response = &events[received]["response"];
    assert_eq!(response["status"], 200);
    assert_eq!(response["statusText"], "OK");
    assert_eq!(response["mimeType"], "text/html");
    assert_eq!(response["protocol"], "http/1.1");
    assert_eq!(response["url"], site.url("/doc"));
    assert_eq!(response["remoteIPAddress"], "127.0.0.1");
    assert_eq!(
        response["remotePort"].as_u64().unwrap().to_string(),
        site.base.rsplit(':').next().unwrap()
    );
    assert_eq!(
        header(&response["headers"], "content-type"),
        "text/html; charset=utf-8"
    );
    assert_eq!(response["fromDiskCache"], false);
    assert!(response["timing"]["requestTime"].as_f64().unwrap() > 0.0);
    assert_eq!(events[finished]["encodedDataLength"], DOC.len());

    // The subresources, each with its type and an ordered life.
    for (path, kind, initiator) in [("/a.js", "Script", "parser"), ("/i.png", "Image", "parser")] {
        let sent = events
            .iter()
            .find(|event| {
                event["event"] == "Network.requestWillBeSent"
                    && event["request"]["url"] == site.url(path)
            })
            .unwrap_or_else(|| panic!("{path} was not requested: {events:#?}"));
        assert_eq!(sent["type"], kind, "{path}");
        assert_eq!(sent["initiator"]["type"], initiator, "{path}");
        assert_eq!(sent["loaderId"], loader.as_str(), "{path}");
        assert_eq!(sent["documentURL"], site.url("/doc"), "{path}");
        let id = sent["requestId"].as_str().unwrap();
        assert_ne!(id, loader);
        let (a, b, c) = (
            position(&events, "requestWillBeSent", id),
            position(&events, "responseReceived", id),
            position(&events, "loadingFinished", id),
        );
        assert!(
            a < b && b < c,
            "{path}: sent {a}, received {b}, finished {c}"
        );
        assert_eq!(events[b]["response"]["status"], 200, "{path}");
        assert_eq!(events[b]["type"], kind, "{path}");
    }

    // Bodies: text comes back as text, an image as base64.
    let script_id = events
        .iter()
        .find(|event| event["event"] == "Network.requestWillBeSent" && event["type"] == "Script")
        .unwrap()["requestId"]
        .clone();
    let body = page
        .result("Network.getResponseBody", json!({ "requestId": script_id }))
        .await;
    assert_eq!(
        body,
        json!({ "body": "window.ran = 1;", "base64Encoded": false })
    );
    let image_id = events
        .iter()
        .find(|event| event["event"] == "Network.requestWillBeSent" && event["type"] == "Image")
        .unwrap()["requestId"]
        .clone();
    let image = page
        .result("Network.getResponseBody", json!({ "requestId": image_id }))
        .await;
    assert_eq!(image["base64Encoded"], true);
    assert_eq!(image["body"], PNG);
    let document = page
        .result("Network.getResponseBody", json!({ "requestId": loader }))
        .await;
    assert_eq!(document["body"], DOC);
    let missing = page
        .call("Network.getResponseBody", json!({ "requestId": "nope" }))
        .await;
    assert_eq!(missing["error"]["code"], -32000);

    fx.stop().await;
}

#[tokio::test]
async fn stylesheets_are_requested_when_the_page_is_rendered() {
    let (fx, site, mut page) = start(standard_site()).await;
    let loader = page.navigate(&site.url("/doc")).await;
    let events = network_events(&mut page, &loader).await;
    assert!(
        of_kind(&events, "requestWillBeSent")
            .iter()
            .all(|event| event["type"] != "Stylesheet"),
        "the engine fetches sheets when it lays the page out"
    );
    page.screenshot(json!({})).await;
    let sent = page.event("Network.requestWillBeSent").await;
    assert_eq!(sent["type"], "Stylesheet");
    assert_eq!(sent["request"]["url"], site.url("/s.css"));
    assert_eq!(sent["initiator"]["type"], "parser");
    assert_eq!(sent["loaderId"], loader.as_str());
    let id = sent["requestId"].clone();
    let body = page
        .result("Network.getResponseBody", json!({ "requestId": id }))
        .await;
    assert_eq!(body["body"], "body{margin:0}");
    fx.stop().await;
}

#[tokio::test]
async fn nothing_is_reported_until_the_domain_is_enabled_and_after_it_is_disabled() {
    let fx = Fixture::start(&[]).await;
    let site = NetSite::start(standard_site()).await;
    let mut page = fx.page().await;
    page.begin().await;
    let loader = page.navigate(&site.url("/doc")).await;
    // Read the whole navigation without any network event.
    loop {
        let event = page.client.next_event().await;
        assert!(
            !event["method"]
                .as_str()
                .unwrap_or_default()
                .starts_with("Network."),
            "unexpected {event}"
        );
        if event["method"] == "Page.lifecycleEvent"
            && event["params"]["loaderId"] == loader.as_str()
            && event["params"]["name"] == "networkIdle"
        {
            break;
        }
    }
    page.result("Network.enable", json!({})).await;
    page.result("Network.disable", json!({})).await;
    let loader = page.navigate(&site.url("/blank")).await;
    loop {
        let event = page.client.next_event().await;
        assert!(
            !event["method"]
                .as_str()
                .unwrap_or_default()
                .starts_with("Network."),
            "unexpected {event}"
        );
        if event["method"] == "Page.lifecycleEvent"
            && event["params"]["loaderId"] == loader.as_str()
            && event["params"]["name"] == "networkIdle"
        {
            break;
        }
    }
    fx.stop().await;
}

#[tokio::test]
async fn script_requests_are_typed_fetch_and_xhr_with_a_script_initiator() {
    let doc = "<!doctype html><title>x</title><div id=out></div><script>\
        window.done = 0;\
        fetch('/data.json?f=1').then(function (r) { return r.text(); }).then(function (t) { window.f = t; window.done++; });\
        var x = new XMLHttpRequest(); x.open('GET', '/data.json?x=1');\
        x.onload = function () { window.x = x.responseText; window.done++; }; x.send();\
        </script>";
    let mut routes = standard_site();
    routes.push(("/script", route(200, "text/html", doc)));
    let (fx, site, mut page) = start(routes).await;
    let loader = page.navigate(&site.url("/script")).await;
    let events = network_events(&mut page, &loader).await;
    for (query, kind) in [("f=1", "Fetch"), ("x=1", "XHR")] {
        let sent = events
            .iter()
            .find(|event| {
                event["event"] == "Network.requestWillBeSent"
                    && event["request"]["url"]
                        .as_str()
                        .is_some_and(|url| url.ends_with(query))
            })
            .unwrap_or_else(|| panic!("{query} was not requested: {events:#?}"));
        assert_eq!(sent["type"], kind);
        assert_eq!(sent["initiator"]["type"], "script");
        let id = sent["requestId"].as_str().unwrap();
        let received = position(&events, "responseReceived", id);
        assert_eq!(events[received]["response"]["mimeType"], "application/json");
        position(&events, "loadingFinished", id);
        let body = page
            .result("Network.getResponseBody", json!({ "requestId": id }))
            .await;
        assert_eq!(body["body"], "{\"ok\":true}");
    }
    assert_eq!(page.evaluate("window.done").await, json!(2));
    fx.stop().await;
}

#[tokio::test]
async fn a_failed_request_reports_its_net_error() {
    let (fx, _site, mut page) = start(standard_site()).await;
    // A port nothing listens on.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        format!("http://127.0.0.1:{port}/x")
    };
    let reply = page.result("Page.navigate", json!({ "url": closed })).await;
    assert_eq!(reply["errorText"], "net::ERR_CONNECTION_REFUSED");
    let params = page.event("Network.loadingFailed").await;
    assert_eq!(params["errorText"], "net::ERR_CONNECTION_REFUSED");
    assert_eq!(params["type"], "Document");
    assert_eq!(params["canceled"], false);
    assert_eq!(params["requestId"], reply["loaderId"]);
    fx.stop().await;
}

#[tokio::test]
async fn overrides_reach_the_server_and_end_with_the_session() {
    let (fx, site, mut page) = start(standard_site()).await;
    page.result(
        "Network.setExtraHTTPHeaders",
        json!({ "headers": { "X-Tenta-Test": "yes" } }),
    )
    .await;
    page.result(
        "Network.setUserAgentOverride",
        json!({ "userAgent": "TentaTest/1.0" }),
    )
    .await;
    let loader = page.navigate(&site.url("/doc")).await;
    let events = network_events(&mut page, &loader).await;
    for path in ["/doc", "/a.js"] {
        let headers = site
            .headers_of(path)
            .unwrap_or_else(|| panic!("{path} not served"));
        assert_eq!(
            headers.get("x-tenta-test").map(String::as_str),
            Some("yes"),
            "{path}"
        );
        assert_eq!(
            headers.get("user-agent").map(String::as_str),
            Some("TentaTest/1.0"),
            "{path}"
        );
    }
    // The client sees what it set on the request.
    let sent = &of_kind(&events, "requestWillBeSent")[0];
    assert_eq!(header(&sent["request"]["headers"], "x-tenta-test"), "yes");

    // Another session of the page sees no override once the first one is gone.
    drop(page);
    let mut again = fx.page().await;
    again.begin().await;
    let loader = again.navigate(&site.url("/blank")).await;
    again.events_through(&loader, "load").await;
    assert!(
        !site
            .headers_of("/blank")
            .unwrap()
            .contains_key("x-tenta-test"),
        "a new page starts clean"
    );
    fx.stop().await;
}

#[tokio::test]
async fn disabling_the_cache_sends_the_request_to_the_server_every_time() {
    let mut routes = standard_site();
    let mut cached = route(200, "text/html", "<!doctype html><title>c</title>");
    cached
        .headers
        .push(("Cache-Control".to_owned(), "max-age=3600".to_owned()));
    routes.push(("/cached", cached));
    let (fx, site, mut page) = start(routes).await;
    for _ in 0..2 {
        let loader = page.navigate(&site.url("/cached")).await;
        page.events_through(&loader, "load").await;
    }
    assert_eq!(
        site.count("/cached"),
        1,
        "the second load came from the cache"
    );
    page.result("Network.setCacheDisabled", json!({ "cacheDisabled": true }))
        .await;
    let loader = page.navigate(&site.url("/cached")).await;
    page.events_through(&loader, "load").await;
    assert_eq!(site.count("/cached"), 2);
    fx.stop().await;
}

#[tokio::test]
async fn cookies_are_the_contexts_jar() {
    let mut routes = standard_site();
    let mut setter = route(200, "text/html", "<!doctype html><title>s</title>");
    setter
        .headers
        .push(("Set-Cookie".to_owned(), "from_server=1; Path=/".to_owned()));
    routes.push(("/set", setter));
    let (fx, site, mut page) = start(routes).await;
    let loader = page.navigate(&site.url("/set")).await;
    page.events_through(&loader, "load").await;
    let cookies = page.result("Network.getCookies", json!({})).await;
    let names: Vec<_> = cookies["cookies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cookie| cookie["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["from_server"]);

    let set = page
        .result(
            "Network.setCookie",
            json!({ "name": "mine", "value": "2", "url": site.url("/") }),
        )
        .await;
    assert_eq!(set["success"], true);
    page.result(
        "Network.setCookies",
        json!({ "cookies": [{ "name": "other", "value": "3", "url": "https://elsewhere.test/" }] }),
    )
    .await;
    let here = page
        .result("Network.getCookies", json!({ "urls": [site.url("/")] }))
        .await;
    assert_eq!(here["cookies"].as_array().unwrap().len(), 2);
    let all = page.result("Network.getAllCookies", json!({})).await;
    assert_eq!(all["cookies"].as_array().unwrap().len(), 3);

    let loader = page.navigate(&site.url("/blank")).await;
    page.events_through(&loader, "load").await;
    let cookie = site.headers_of("/blank").unwrap()["cookie"].clone();
    assert!(
        cookie.contains("mine=2") && cookie.contains("from_server=1"),
        "{cookie}"
    );
    assert!(!cookie.contains("other"), "{cookie}");

    page.result(
        "Network.deleteCookies",
        json!({ "name": "mine", "url": site.url("/") }),
    )
    .await;
    let left = page.result("Network.getAllCookies", json!({})).await;
    assert_eq!(left["cookies"].as_array().unwrap().len(), 2);
    page.result("Network.clearBrowserCookies", json!({})).await;
    let none = page.result("Network.getAllCookies", json!({})).await;
    assert!(none["cookies"].as_array().unwrap().is_empty());
    fx.stop().await;
}

#[tokio::test]
async fn offline_fails_requests_and_online_restores_them() {
    let (fx, site, mut page) = start(standard_site()).await;
    page.result(
        "Network.emulateNetworkConditions",
        json!({ "offline": true, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1 }),
    )
    .await;
    let reply = page
        .result("Page.navigate", json!({ "url": site.url("/blank") }))
        .await;
    assert_eq!(reply["errorText"], "net::ERR_INTERNET_DISCONNECTED");
    assert_eq!(site.count("/blank"), 0);
    page.result(
        "Network.emulateNetworkConditions",
        json!({ "offline": false, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1 }),
    )
    .await;
    let loader = page.navigate(&site.url("/blank")).await;
    page.events_through(&loader, "load").await;
    assert_eq!(site.count("/blank"), 1);
    fx.stop().await;
}

// ---- Fetch ----------------------------------------------------------------

#[tokio::test]
async fn fetch_fulfills_fails_and_continues_requests_the_page_makes() {
    let doc = "<!doctype html><title>x</title><script>\
        window.out = {};\
        function go(name, url, init) {\
          fetch(url, init).then(function (r) { return r.text().then(function (t) { window.out[name] = r.status + ':' + t; }); },\
                             function (e) { window.out[name] = 'failed'; });\
        }\
        </script>";
    let mut routes = standard_site();
    routes.push(("/app", route(200, "text/html", doc)));
    routes.push(("/api/real", route(200, "text/plain", "real")));
    let (fx, site, mut page) = start(routes).await;
    let loader = page.navigate(&site.url("/app")).await;
    network_events(&mut page, &loader).await;

    page.result(
        "Fetch.enable",
        json!({ "patterns": [{ "urlPattern": "*/api/*" }] }),
    )
    .await;
    // A second session may not take the same target's requests.
    let mut rival = fx.attach(&page.target).await;
    let refused = rival.call("Fetch.enable", json!({})).await;
    assert_eq!(refused["error"]["code"], -32000);

    // Fulfill: the page gets the client's response and the server sees nothing.
    page.evaluate("go('a', '/api/fulfilled'); 0").await;
    let paused = page.event("Fetch.requestPaused").await;
    assert_eq!(paused["request"]["url"], site.url("/api/fulfilled"));
    assert_eq!(paused["resourceType"], "Fetch");
    assert_eq!(paused["frameId"], page.target.as_str());
    let sent = page.event("Network.requestWillBeSent").await;
    assert_eq!(
        sent["requestId"], paused["networkId"],
        "the two domains name one request"
    );
    page.result(
        "Fetch.fulfillRequest",
        json!({
            "requestId": paused["requestId"],
            "responseCode": 201,
            "responseHeaders": [{ "name": "Content-Type", "value": "text/plain" }],
            "body": base64::engine::general_purpose::STANDARD.encode("from the client"),
        }),
    )
    .await;
    let received = page.event("Network.responseReceived").await;
    assert_eq!(received["response"]["status"], 201);
    page.client.event("Network.loadingFinished").await;
    assert_eq!(site.count("/api/fulfilled"), 0);

    // Fail: the page's fetch rejects and the client sees the error.
    page.evaluate("go('b', '/api/failed'); 0").await;
    let paused = page.event("Fetch.requestPaused").await;
    page.result(
        "Fetch.failRequest",
        json!({ "requestId": paused["requestId"], "errorReason": "BlockedByClient" }),
    )
    .await;
    let failed = page.event("Network.loadingFailed").await;
    assert_eq!(failed["errorText"], "net::ERR_BLOCKED_BY_CLIENT");
    assert_eq!(failed["blockedReason"], "inspector");
    assert_eq!(site.count("/api/failed"), 0);

    // Continue, changed: another URL and an extra header reach the server.
    page.evaluate("go('c', '/api/original'); 0").await;
    let paused = page.event("Fetch.requestPaused").await;
    page.result(
        "Fetch.continueRequest",
        json!({
            "requestId": paused["requestId"],
            "url": site.url("/api/real"),
            "headers": [{ "name": "X-Changed", "value": "1" }],
        }),
    )
    .await;
    page.client.event("Network.loadingFinished").await;
    assert_eq!(site.count("/api/original"), 0);
    assert_eq!(
        site.headers_of("/api/real")
            .unwrap()
            .get("x-changed")
            .map(String::as_str),
        Some("1")
    );

    // A request the pattern does not match is not held.
    page.evaluate("go('d', '/data.json'); 0").await;
    loop {
        let event = page.client.next_event().await;
        assert_ne!(event["method"], "Fetch.requestPaused", "held: {event}");
        if event["method"] == "Network.loadingFinished" {
            break;
        }
    }

    // Unknown request ids are refused.
    let unknown = page
        .call(
            "Fetch.continueRequest",
            json!({ "requestId": "interception-job-999.0" }),
        )
        .await;
    assert_eq!(unknown["error"]["code"], -32602);

    // Disabling lets nothing stay held, and the other session may take over.
    page.result("Fetch.disable", json!({})).await;
    rival.result("Fetch.enable", json!({})).await;
    fx.stop().await;
}

#[tokio::test]
async fn a_held_navigation_waits_for_the_client_and_redirect_hops_are_held_again() {
    let (fx, site, mut page) = start(standard_site()).await;
    page.result(
        "Fetch.enable",
        json!({ "patterns": [{ "urlPattern": "*" }] }),
    )
    .await;
    let navigate = page
        .client
        .send(
            Some(&page.session.clone()),
            "Page.navigate",
            json!({ "url": site.url("/go") }),
        )
        .await;
    let first = page.event("Fetch.requestPaused").await;
    assert_eq!(first["resourceType"], "Document");
    assert_eq!(first["request"]["url"], site.url("/go"));
    assert_eq!(site.count("/go"), 0, "held before it left");
    page.result(
        "Fetch.continueRequest",
        json!({ "requestId": first["requestId"] }),
    )
    .await;
    let second = page.event("Fetch.requestPaused").await;
    assert_eq!(second["request"]["url"], site.url("/doc"));
    assert_eq!(second["networkId"], first["networkId"]);
    page.result(
        "Fetch.continueRequest",
        json!({ "requestId": second["requestId"] }),
    )
    .await;
    // The subresources are held too; release them as they come.
    page.result("Fetch.disable", json!({})).await;
    let reply = page.client.response(navigate).await;
    assert_eq!(reply["result"]["loaderId"], first["networkId"]);
    fx.stop().await;
}

#[tokio::test]
async fn disabling_fetch_releases_a_request_the_client_was_holding() {
    let doc = "<!doctype html><title>x</title><script>\
        window.result = 'pending';\
        function go(url) { fetch(url).then(function (r) { return r.text(); }).then(function (t) { window.result = t; }); }\
        </script>";
    let mut routes = standard_site();
    routes.push(("/app", route(200, "text/html", doc)));
    routes.push(("/api/held", route(200, "text/plain", "released")));
    let (fx, site, mut page) = start(routes).await;
    let loader = page.navigate(&site.url("/app")).await;
    network_events(&mut page, &loader).await;
    page.result(
        "Fetch.enable",
        json!({ "patterns": [{ "urlPattern": "*/api/*" }] }),
    )
    .await;
    page.evaluate("go('/api/held'); 0").await;
    page.event("Fetch.requestPaused").await;
    assert_eq!(site.count("/api/held"), 0, "held while the client decides");
    page.result("Fetch.disable", json!({})).await;
    page.client.event("Network.loadingFinished").await;
    assert_eq!(site.count("/api/held"), 1);
    // A session that simply goes away releases its held requests as well.
    page.result(
        "Fetch.enable",
        json!({ "patterns": [{ "urlPattern": "*/api/*" }] }),
    )
    .await;
    page.evaluate("go('/api/held'); 0").await;
    page.event("Fetch.requestPaused").await;
    let Page { client, .. } = page;
    drop(client);
    let mut watcher = fx.page().await;
    for _ in 0..500 {
        if site.count("/api/held") == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        site.count("/api/held"),
        2,
        "the second request was released too"
    );
    watcher.begin().await;
    fx.stop().await;
}

#[tokio::test]
async fn a_fulfilled_redirect_is_followed_and_a_fulfilled_cookie_is_stored() {
    let doc = "<!doctype html><title>x</title><script>\
        window.result = 'pending';\
        fetch('/api/start').then(function (r) { return r.text(); }).then(function (t) { window.result = t; });\
        </script>";
    let mut routes = standard_site();
    routes.push(("/app", route(200, "text/html", doc)));
    routes.push(("/api/end", route(200, "text/plain", "at the end")));
    let (fx, site, mut page) = start(routes).await;
    page.result(
        "Fetch.enable",
        json!({ "patterns": [{ "urlPattern": "*/api/start" }] }),
    )
    .await;
    let loader = page.navigate(&site.url("/app")).await;
    let _ = loader;
    let paused = page.event("Fetch.requestPaused").await;
    page.result(
        "Fetch.fulfillRequest",
        json!({
            "requestId": paused["requestId"],
            "responseCode": 302,
            "responseHeaders": [
                { "name": "Location", "value": "/api/end" },
                { "name": "Set-Cookie", "value": "fulfilled=1; Path=/" },
            ],
        }),
    )
    .await;
    for _ in 0..500 {
        if page.evaluate("window.result").await != json!("pending") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(page.evaluate("window.result").await, json!("at the end"));
    assert_eq!(site.count("/api/end"), 1);
    let cookies = page.result("Network.getCookies", json!({})).await;
    assert!(
        cookies["cookies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cookie| cookie["name"] == "fulfilled")
    );
    fx.stop().await;
}

#[tokio::test]
async fn headers_that_cannot_go_on_the_wire_are_an_error() {
    let (fx, site, mut page) = start(standard_site()).await;
    let bad = page
        .call(
            "Network.setExtraHTTPHeaders",
            json!({ "headers": { "bad name": "1" } }),
        )
        .await;
    assert_eq!(bad["error"]["code"], -32602);
    page.result("Fetch.enable", json!({})).await;
    let navigate = page
        .client
        .send(
            Some(&page.session.clone()),
            "Page.navigate",
            json!({ "url": site.url("/blank") }),
        )
        .await;
    let paused = page.event("Fetch.requestPaused").await;
    for (method, extra) in [
        ("continueRequest", json!({})),
        ("fulfillRequest", json!({ "responseCode": 200 })),
    ] {
        let mut params = extra;
        params["requestId"] = paused["requestId"].clone();
        params[if method == "continueRequest" {
            "headers"
        } else {
            "responseHeaders"
        }] = json!([{ "name": "x", "value": "line\nbreak" }]);
        let refused = page.call(&format!("Fetch.{method}"), params).await;
        assert_eq!(refused["error"]["code"], -32602, "{method}");
    }
    // The request is still held and can be continued properly.
    page.result(
        "Fetch.continueRequest",
        json!({ "requestId": paused["requestId"] }),
    )
    .await;
    page.result("Fetch.disable", json!({})).await;
    page.client.response(navigate).await;
    fx.stop().await;
}

#[tokio::test]
async fn set_cookie_refuses_what_chromium_refuses() {
    let (fx, site, mut page) = start(standard_site()).await;
    let url = site.url("/");
    for (label, cookie) in [
        (
            "SameSite=None without Secure",
            json!({ "name": "a", "value": "1", "url": url, "sameSite": "None" }),
        ),
        (
            "__Secure- without Secure",
            json!({ "name": "__Secure-a", "value": "1", "url": url }),
        ),
        (
            "__Host- with a domain",
            json!({ "name": "__Host-a", "value": "1", "url": "https://127.0.0.1/", "secure": true, "domain": "127.0.0.1" }),
        ),
        (
            "a domain that does not cover the host",
            json!({ "name": "d", "value": "1", "url": url, "domain": "elsewhere.test" }),
        ),
    ] {
        let reply = page.result("Network.setCookie", cookie).await;
        assert_eq!(reply["success"], false, "{label}");
    }
    let ok = page
        .result(
            "Network.setCookie",
            json!({ "name": "__Host-ok", "value": "1", "url": "https://127.0.0.1/", "secure": true }),
        )
        .await;
    assert_eq!(ok["success"], true);
    let batch = page
        .call(
            "Network.setCookies",
            json!({ "cookies": [{ "name": "a", "value": "1", "url": url, "sameSite": "None" }] }),
        )
        .await;
    assert_eq!(batch["error"]["message"], "Invalid cookie fields");
    fx.stop().await;
}

#[tokio::test]
async fn delete_cookies_tells_host_only_from_domain_cookies() {
    let (fx, _site, mut page) = start(standard_site()).await;
    page.result(
        "Network.setCookies",
        json!({ "cookies": [
            { "name": "same", "value": "1", "url": "http://shop.test/" },
            { "name": "same", "value": "2", "domain": "shop.test", "path": "/wide" },
        ] }),
    )
    .await;
    let before = page.result("Network.getAllCookies", json!({})).await;
    let domains: Vec<_> = before["cookies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cookie| cookie["domain"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        domains.contains(&"shop.test".to_owned()) && domains.contains(&".shop.test".to_owned())
    );
    page.result(
        "Network.deleteCookies",
        json!({ "name": "same", "domain": ".shop.test" }),
    )
    .await;
    let after = page.result("Network.getAllCookies", json!({})).await;
    let left = after["cookies"].as_array().unwrap();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(
        left[0]["domain"], "shop.test",
        "only the domain cookie went"
    );
    fx.stop().await;
}

// ---- cost ----------------------------------------------------------------

/// A page of 300 requests, loaded with the domain off and on, interleaved.
/// Run with `--run-ignored only`; it prints p50 and p95 per mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a measurement, not a check"]
async fn three_hundred_requests_cost_the_same_with_the_domain_off() {
    const RUNS: usize = 15;
    const SCRIPTS: usize = 300;
    // Parser-found scripts are awaited before `load`, so the load is the work.
    let mut doc = String::from("<!doctype html><title>300</title>");
    for n in 0..SCRIPTS {
        doc.push_str(&format!("<script src=/a.js?n={n}></script>"));
    }
    let mut routes = standard_site();
    routes.push(("/many", route(200, "text/html", doc)));
    let fx = Fixture::start(&[]).await;
    let site = NetSite::start(routes).await;
    let mut timings: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    for run in 0..RUNS * 2 {
        let enabled = run % 2 == 1;
        let mut page = fx.page().await;
        page.begin().await;
        if enabled {
            page.result("Network.enable", json!({})).await;
        }
        let started = std::time::Instant::now();
        let loader = page.navigate(&site.url("/many")).await;
        page.events_through(&loader, "load").await;
        timings[usize::from(enabled)].push(started.elapsed().as_secs_f64() * 1000.0);
        let target = page.target.clone();
        page.client
            .result(None, "Target.closeTarget", json!({ "targetId": target }))
            .await;
    }
    for (label, samples) in ["off", "on"].iter().zip(&mut timings) {
        samples.sort_by(f64::total_cmp);
        println!(
            "network {label:>3}: p50 {:.1} ms, p95 {:.1} ms over {} runs",
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100],
            samples.len()
        );
    }
    fx.stop().await;
}
