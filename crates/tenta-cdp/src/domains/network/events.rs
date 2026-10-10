//! The protocol shapes of a page's network events.

use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Map, Value, json};
use tenta_browser::network::{
    Initiator, NetworkEvent, PriorityClass, RedirectResponse, RequestInfo, RequestKey,
    ResourceType, ResponseInfo, Timing, reason_phrase,
};

use crate::backend::Target;
use crate::clock::{seconds_at, wall_seconds_at};

/// `Network.ResourceType`.
pub(crate) fn resource_type(kind: ResourceType) -> &'static str {
    match kind {
        ResourceType::Document => "Document",
        ResourceType::Stylesheet => "Stylesheet",
        ResourceType::Script | ResourceType::BlockingScript => "Script",
        ResourceType::Font => "Font",
        ResourceType::Image => "Image",
        ResourceType::Media => "Media",
        ResourceType::Xhr => "XHR",
        ResourceType::Fetch => "Fetch",
        ResourceType::Prefetch => "Prefetch",
        _ => "Other",
    }
}

fn priority(kind: ResourceType) -> &'static str {
    match kind.default_priority().class {
        PriorityClass::VeryLow => "VeryLow",
        PriorityClass::Low => "Low",
        PriorityClass::Medium => "Medium",
        PriorityClass::High => "High",
        PriorityClass::VeryHigh => "VeryHigh",
    }
}

/// The requestId a client sees. A document's equals its loader id, which is
/// how Puppeteer tells a navigation request from the rest.
pub(crate) fn request_id(target: &Target, key: RequestKey) -> String {
    if key.resource_type == ResourceType::Document {
        target.loader_id(key.loader)
    } else {
        format!("{}.{}", target.page().id().raw(), key.id)
    }
}

fn key_of(info: &RequestInfo) -> RequestKey {
    RequestKey {
        id: info.id,
        loader: info.loader,
        resource_type: info.resource_type,
    }
}

/// Header lines as the protocol's object: a repeated name joins with a
/// newline.
pub(crate) fn headers_object(headers: &[(String, String)]) -> Value {
    let mut object = Map::new();
    for (name, value) in headers {
        match object.get_mut(name) {
            Some(Value::String(existing)) => {
                existing.push('\n');
                existing.push_str(value);
            }
            _ => {
                object.insert(name.clone(), Value::String(value.clone()));
            }
        }
    }
    Value::Object(object)
}

fn without_fragment(url: &url::Url) -> (String, Option<String>) {
    let mut bare = url.clone();
    let fragment = bare.fragment().map(|fragment| format!("#{fragment}"));
    bare.set_fragment(None);
    (bare.into(), fragment)
}

pub(crate) fn request_json(info: &RequestInfo) -> Value {
    let (url, fragment) = without_fragment(&info.url);
    let mut request = json!({
        "url": url,
        "method": info.method,
        "headers": headers_object(&info.headers),
        "initialPriority": priority(info.resource_type),
        "referrerPolicy": "strict-origin-when-cross-origin",
        "mixedContentType": "none",
    });
    if let Some(fragment) = fragment {
        request["urlFragment"] = json!(fragment);
    }
    if let Some(post) = &info.post_data
        && !post.is_empty()
    {
        request["hasPostData"] = json!(true);
        match std::str::from_utf8(post) {
            Ok(text) => request["postData"] = json!(text),
            Err(_) => {
                request["postDataEntries"] =
                    json!([{ "bytes": base64::engine::general_purpose::STANDARD.encode(post) }]);
            }
        }
    }
    request
}

fn initiator_json(initiator: &Initiator) -> Value {
    match initiator {
        Initiator::Other => json!({ "type": "other" }),
        Initiator::Parser(url) => json!({ "type": "parser", "url": url.as_str(), "lineNumber": 0 }),
        Initiator::Script(url) => json!({
            "type": "script",
            "url": url.as_str(),
            "stack": { "callFrames": [] },
        }),
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// `Network.ResourceTiming`; the phases a reused connection skipped are -1.
fn timing_json(timing: &Timing, requested_at: std::time::Instant) -> Value {
    let dns = ms(timing.dns);
    let connect = ms(timing.connect);
    let tls = ms(timing.tls);
    let after_dns = dns;
    let after_connect = dns + connect;
    let after_tls = after_connect + tls;
    let phase = |present: bool, value: f64| if present { value } else { -1.0 };
    let has_dns = !timing.dns.is_zero();
    let has_connect = !timing.connect.is_zero();
    let has_tls = !timing.tls.is_zero();
    json!({
        "requestTime": seconds_at(requested_at),
        "proxyStart": -1, "proxyEnd": -1,
        "dnsStart": phase(has_dns, 0.0),
        "dnsEnd": phase(has_dns, after_dns),
        "connectStart": phase(has_connect || has_tls, after_dns),
        "connectEnd": phase(has_connect || has_tls, after_tls),
        "sslStart": phase(has_tls, after_connect),
        "sslEnd": phase(has_tls, after_tls),
        "workerStart": -1, "workerReady": -1,
        "workerFetchStart": -1, "workerRespondWithSettled": -1,
        "sendStart": after_tls,
        "sendEnd": after_tls,
        "pushStart": 0, "pushEnd": 0,
        "receiveHeadersStart": ms(timing.ttfb),
        "receiveHeadersEnd": ms(timing.total),
    })
}

fn remote_address(remote: SocketAddr) -> String {
    match remote {
        SocketAddr::V4(v4) => v4.ip().to_string(),
        SocketAddr::V6(v6) => format!("[{}]", v6.ip()),
    }
}

fn header_bytes(headers: &[(String, String)]) -> usize {
    headers
        .iter()
        .map(|(name, value)| name.len() + value.len() + 4)
        .sum()
}

/// `Network.Response`.
pub(crate) fn response_json(info: &ResponseInfo) -> Value {
    let (url, _) = without_fragment(&info.url);
    let mut response = json!({
        "url": url,
        "status": info.status,
        "statusText": reason_phrase(info.status),
        "headers": headers_object(&info.headers),
        "mimeType": info.mime_type,
        "connectionReused": false,
        "connectionId": 0,
        "encodedDataLength": header_bytes(&info.headers),
        "fromDiskCache": info.from_cache,
        "fromServiceWorker": false,
        "fromPrefetchCache": false,
        "protocol": info.protocol,
        "securityState": if info.url.scheme() == "https" { "secure" } else { "insecure" },
    });
    if !info.charset.is_empty() {
        response["charset"] = json!(info.charset);
    }
    response["responseTime"] = json!(wall_seconds_at(info.at) * 1000.0);
    if !info.local {
        response["timing"] = timing_json(&info.timing, info.requested_at);
    }
    if let Some(remote) = info.remote {
        response["remoteIPAddress"] = json!(remote_address(remote));
        response["remotePort"] = json!(remote.port());
    }
    response
}

/// The 3xx answer a redirected request reports.
fn redirect_json(redirect: &RedirectResponse) -> Value {
    let (url, _) = without_fragment(&redirect.url);
    json!({
        "url": url,
        "status": redirect.status,
        "statusText": reason_phrase(redirect.status),
        "headers": headers_object(&redirect.headers),
        "mimeType": "",
        "connectionReused": false,
        "connectionId": 0,
        "encodedDataLength": header_bytes(&redirect.headers),
        "fromDiskCache": redirect.from_cache,
        "fromServiceWorker": false,
        "fromPrefetchCache": false,
        "securityState": if redirect.url.scheme() == "https" { "secure" } else { "insecure" },
    })
}

/// `Network.requestWillBeSent` for one hop.
pub(crate) fn request_will_be_sent(target: &Target, info: &RequestInfo) -> Value {
    let mut event = json!({
        "requestId": request_id(target, key_of(info)),
        "loaderId": target.loader_id(info.loader),
        "documentURL": without_fragment(&info.document_url).0,
        "request": request_json(info),
        "timestamp": seconds_at(info.at),
        "wallTime": wall_seconds_at(info.at),
        "initiator": initiator_json(&info.initiator),
        "redirectHasExtraInfo": false,
        "type": resource_type(info.resource_type),
        "frameId": target.id(),
        "hasUserGesture": info.user_gesture,
    });
    if let Some(redirect) = &info.redirect {
        event["redirectResponse"] = redirect_json(redirect);
    }
    event
}

/// The method and parameters of the protocol event `event` becomes, if any.
pub(crate) fn protocol_event(
    target: &Target,
    event: &NetworkEvent,
) -> Option<(&'static str, Value)> {
    Some(match event {
        NetworkEvent::Request(info) => (
            "Network.requestWillBeSent",
            request_will_be_sent(target, info),
        ),
        NetworkEvent::Response(info) => (
            "Network.responseReceived",
            json!({
                "requestId": request_id(target, RequestKey {
                    id: info.id,
                    loader: info.loader,
                    resource_type: info.resource_type,
                }),
                "loaderId": target.loader_id(info.loader),
                "timestamp": seconds_at(info.at),
                "type": resource_type(info.resource_type),
                "response": response_json(info),
                "hasExtraInfo": false,
                "frameId": target.id(),
            }),
        ),
        NetworkEvent::Data { key, length, at } => (
            "Network.dataReceived",
            json!({
                "requestId": request_id(target, *key),
                "timestamp": seconds_at(*at),
                "dataLength": length,
                "encodedDataLength": length,
            }),
        ),
        NetworkEvent::Finished {
            key,
            encoded_length,
            at,
        } => (
            "Network.loadingFinished",
            json!({
                "requestId": request_id(target, *key),
                "timestamp": seconds_at(*at),
                "encodedDataLength": encoded_length,
            }),
        ),
        NetworkEvent::Failed {
            key,
            error,
            canceled,
            blocked,
            at,
        } => {
            let mut failed = json!({
                "requestId": request_id(target, *key),
                "timestamp": seconds_at(*at),
                "type": resource_type(key.resource_type),
                "errorText": error,
                "canceled": canceled,
            });
            if let Some(reason) = blocked {
                failed["blockedReason"] = json!(reason);
            }
            ("Network.loadingFailed", failed)
        }
        _ => return None,
    })
}
