//! The `Fetch` domain: hold a page's requests until the client decides.
//!
//! `Fetch.enable` makes the transport ask this session about every request of
//! the page that matches a pattern, at the request stage, before the cache.
//! The client then continues it (optionally changed), fulfills it with a
//! response of its own, or fails it. A redirect hop is held again, as in
//! Chromium. Only one session at a time may hold a target's requests; its
//! disappearance lets every held request go on unchanged.
//!
//! Not built, and answered with an error naming the gap: the response stage
//! (`requestStage: "Response"`), `continueWithAuth` (the engine has no
//! authentication challenges, so `Fetch.authRequired` is never sent) and
//! `getResponseBody`/`takeResponseBodyAsStream`.

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tenta_browser::network::{Bytes, Modifications, RequestInfo, Resolution};

use super::network::{NetState, bridge::Gate};
use crate::backend::Target;
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Fetch);
}

struct Fetch;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnableParams {
    patterns: Option<Vec<PatternParams>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatternParams {
    url_pattern: Option<String>,
    resource_type: Option<String>,
    request_stage: Option<String>,
}

#[derive(Deserialize)]
struct HeaderEntry {
    name: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContinueParams {
    request_id: String,
    url: Option<String>,
    method: Option<String>,
    post_data: Option<String>,
    headers: Option<Vec<HeaderEntry>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FulfillParams {
    request_id: String,
    response_code: u16,
    response_headers: Option<Vec<HeaderEntry>>,
    body: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FailParams {
    request_id: String,
    error_reason: String,
}

/// The `Network.ErrorReason` as a Chromium error name.
fn error_name(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "Failed" => "ERR_FAILED",
        "Aborted" => "ERR_ABORTED",
        "TimedOut" => "ERR_TIMED_OUT",
        "AccessDenied" => "ERR_ACCESS_DENIED",
        "ConnectionClosed" => "ERR_CONNECTION_CLOSED",
        "ConnectionReset" => "ERR_CONNECTION_RESET",
        "ConnectionRefused" => "ERR_CONNECTION_REFUSED",
        "ConnectionAborted" => "ERR_CONNECTION_ABORTED",
        "ConnectionFailed" => "ERR_CONNECTION_FAILED",
        "NameNotResolved" => "ERR_NAME_NOT_RESOLVED",
        "InternetDisconnected" => "ERR_INTERNET_DISCONNECTED",
        "AddressUnreachable" => "ERR_ADDRESS_UNREACHABLE",
        "BlockedByClient" => "ERR_BLOCKED_BY_CLIENT",
        "BlockedByResponse" => "ERR_BLOCKED_BY_RESPONSE",
        _ => return None,
    })
}

fn decode(text: &str) -> Result<Bytes, CdpError> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map(Bytes::from)
        .map_err(|_| CdpError::invalid_params("Invalid base64 encoding"))
}

fn lines(entries: Vec<HeaderEntry>) -> Vec<(String, String)> {
    entries
        .into_iter()
        .map(|entry| (entry.name, entry.value))
        .collect()
}

/// `Fetch.requestPaused` for a held request. `networkId` is the id the
/// `Network` domain reports for the same request, which is how clients pair
/// the two.
pub(super) fn paused_event(target: &Target, fetch_id: &str, request: &RequestInfo) -> Value {
    let key = tenta_browser::network::RequestKey {
        id: request.id,
        loader: request.loader,
        resource_type: request.resource_type,
    };
    json!({
        "requestId": fetch_id,
        "request": super::network::request_json(request),
        "frameId": target.id(),
        "resourceType": super::network::resource_type(request.resource_type),
        "networkId": super::network::request_id(target, key),
    })
}

fn resolve(cx: &CommandContext, fetch_id: &str, resolution: Resolution) -> Result<Value, CdpError> {
    if cx.target()?.network().resolve(fetch_id, resolution) {
        Ok(json!({}))
    } else {
        Err(CdpError::invalid_params("Invalid InterceptionId."))
    }
}

impl DomainHandler for Fetch {
    fn domain(&self) -> &'static str {
        "Fetch"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "enable",
            "disable",
            "continueRequest",
            "fulfillRequest",
            "failRequest",
            "continueWithAuth",
        ]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            let target = cx.target()?;
            match method {
                "enable" => {
                    let EnableParams { patterns } = parse_params(params)?;
                    let mut gate = Gate::default();
                    for pattern in patterns.unwrap_or_default() {
                        if pattern
                            .request_stage
                            .as_deref()
                            .is_some_and(|stage| stage != "Request")
                        {
                            return Err(CdpError::server(
                                "Fetch.enable: only the Request stage is supported",
                            ));
                        }
                        gate.patterns.push((
                            pattern.url_pattern.unwrap_or_else(|| "*".to_owned()),
                            pattern.resource_type,
                        ));
                    }
                    let subscriber = cx
                        .session()
                        .state::<NetState>()
                        .subscriber(cx.session(), target);
                    if target.network().gate_taken_by_other(&subscriber) {
                        return Err(CdpError::server(
                            "Another session already intercepts this target's requests",
                        ));
                    }
                    subscriber.set_gate(Some(gate));
                    target.sync_network();
                    Ok(json!({}))
                }
                "disable" => {
                    let subscriber = cx
                        .session()
                        .state::<NetState>()
                        .subscriber(cx.session(), target);
                    subscriber.set_gate(None);
                    target.network().release(&subscriber);
                    target.sync_network();
                    Ok(json!({}))
                }
                "continueRequest" => {
                    let ContinueParams {
                        request_id,
                        url,
                        method,
                        post_data,
                        headers,
                    } = parse_params(params)?;
                    let url = url
                        .map(|url| {
                            url::Url::parse(&url)
                                .map_err(|_| CdpError::invalid_params("Invalid url"))
                        })
                        .transpose()?;
                    let post_data = post_data.as_deref().map(decode).transpose()?;
                    let headers = headers.map(lines);
                    if let Some(headers) = &headers {
                        tenta_browser::network::check_headers(headers)
                            .map_err(CdpError::invalid_params)?;
                    }
                    let changes = Modifications {
                        url,
                        method,
                        headers,
                        post_data,
                    };
                    resolve(&cx, &request_id, Resolution::Continue(changes))
                }
                "fulfillRequest" => {
                    let FulfillParams {
                        request_id,
                        response_code,
                        response_headers,
                        body,
                    } = parse_params(params)?;
                    let body = body.as_deref().map(decode).transpose()?.unwrap_or_default();
                    let headers = response_headers.map(lines).unwrap_or_default();
                    tenta_browser::network::check_headers(&headers)
                        .map_err(CdpError::invalid_params)?;
                    let Some(resolution) = Resolution::fulfill(response_code, headers, body) else {
                        return Err(CdpError::invalid_params("Invalid http status code"));
                    };
                    resolve(&cx, &request_id, resolution)
                }
                "failRequest" => {
                    let FailParams {
                        request_id,
                        error_reason,
                    } = parse_params(params)?;
                    let resolution = error_name(&error_reason)
                        .and_then(Resolution::fail)
                        .ok_or_else(|| CdpError::invalid_params("Invalid net error reason"))?;
                    resolve(&cx, &request_id, resolution)
                }
                "continueWithAuth" => Err(CdpError::server(
                    "Fetch.continueWithAuth is not supported: the engine raises no authentication challenges",
                )),
                other => Err(CdpError::method_not_found(&format!("Fetch.{other}"))),
            }
        })
    }
}
