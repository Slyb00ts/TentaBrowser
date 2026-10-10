//! The `Network` domain: the requests, responses and bodies of one page, and
//! the per-page overrides a client applies to them.
//!
//! The page reports its traffic to the target's [`NetworkBridge`]; a session
//! that enabled the domain gets those events in order on its own queue. Every
//! request the page makes is covered: the navigation and its redirects,
//! parser-found scripts, stylesheets and images, and `fetch()`/XHR. Not
//! covered, and stated where a client might look for it: request headers are
//! the page's own plus the overrides (the transport's profile headers are not
//! reported), `encodedDataLength` counts decoded bytes, a redirect response
//! carries only its headers, there is no `requestWillBeSentExtraInfo`,
//! `requestServedFromCache`, WebSocket or EventSource event, and the user
//! agent override changes the request header but not `navigator.userAgent`.
//!
//! Response bodies are kept per target in a byte-bounded LRU
//! ([`bridge::BODY_BUDGET`]); a body larger than the page's retention limit is
//! reported and not kept, and `getResponseBody` then fails as Chromium does for
//! a body it dropped.

pub(crate) mod bridge;
mod cookies;
mod events;
mod overrides;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tenta_browser::network::NetworkEvent;

pub(crate) use self::bridge::NetworkBridge;
use self::bridge::{Outgoing, Subscriber};
pub(super) use self::events::{request_id, request_json, resource_type};
pub(crate) use self::overrides::NetOverrides;
use crate::backend::{Target, lock};
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;
use crate::session::Session;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Network);
}

struct Network;

/// What a session holds on a target's network: its subscription and the
/// overrides it set, all of which end with the session.
#[derive(Default)]
pub(super) struct NetState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    target: Option<Arc<Target>>,
    subscriber: Option<Arc<Subscriber>>,
    /// Identifies the session in the page's override stacks.
    owner: usize,
    overrides: bool,
}

impl NetState {
    /// The session's subscription, created with its queue task on first use.
    pub(super) fn subscriber(
        &self,
        session: &Arc<Session>,
        target: &Arc<Target>,
    ) -> Arc<Subscriber> {
        let mut inner = lock(&self.inner);
        if let Some(subscriber) = &inner.subscriber {
            return Arc::clone(subscriber);
        }
        let subscriber = target.network().subscribe();
        inner.target = Some(Arc::clone(target));
        inner.owner = Arc::as_ptr(session) as usize;
        inner.subscriber = Some(Arc::clone(&subscriber));
        let (task_session, task_target) = (Arc::clone(session), Arc::clone(target));
        let task_subscriber = Arc::clone(&subscriber);
        session.spawn(async move { drain(task_session, task_target, task_subscriber).await });
        subscriber
    }

    /// Changes a page-wide setting on behalf of the session, which undoes only
    /// what it set when it goes.
    fn set_override(
        &self,
        session: &Arc<Session>,
        target: &Arc<Target>,
        change: impl FnOnce(&mut overrides::NetOverrides, usize),
    ) {
        let owner = Arc::as_ptr(session) as usize;
        {
            let mut inner = lock(&self.inner);
            inner.target = Some(Arc::clone(target));
            inner.owner = owner;
            inner.overrides = true;
        }
        let mut stacks = target.net_overrides();
        change(&mut stacks, owner);
        stacks.apply(target.page().network());
    }
}

impl Drop for NetState {
    fn drop(&mut self) {
        let inner = self
            .inner
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(target) = inner.target.take() else {
            return;
        };
        if let Some(subscriber) = inner.subscriber.take() {
            target.network().unsubscribe(&subscriber);
        }
        if inner.overrides {
            let mut stacks = target.net_overrides();
            stacks.clear(inner.owner);
            stacks.apply(target.page().network());
        }
        target.sync_network();
    }
}

/// Ends the session's queue when its task does, however it ends.
struct Closing(Arc<Subscriber>);

impl Drop for Closing {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Sends a session what its target's page reports, in the order it happened.
///
/// A request the session never saw start (the domain was enabled while it was
/// in flight) is not reported from the middle: its redirects, response and end
/// are dropped, as the client has no request to attach them to.
async fn drain(session: Arc<Session>, target: Arc<Target>, subscriber: Arc<Subscriber>) {
    let _closing = Closing(Arc::clone(&subscriber));
    let mut started: HashSet<u64> = HashSet::new();
    loop {
        match subscriber.next().await {
            Outgoing::Event(event) => {
                if !admit(&mut started, &event) {
                    continue;
                }
                if let Some((method, params)) = events::protocol_event(&target, &event) {
                    session.emit(method, params).await;
                }
            }
            Outgoing::Paused { fetch_id, request } => {
                let params = super::fetch::paused_event(&target, &fetch_id, &request);
                session.emit("Fetch.requestPaused", params).await;
            }
        }
    }
}

/// Whether the session has the request `event` is about, tracking starts and
/// ends.
fn admit(started: &mut HashSet<u64>, event: &NetworkEvent) -> bool {
    match event {
        NetworkEvent::Request(info) if info.redirect.is_none() => {
            started.insert(info.id);
            true
        }
        NetworkEvent::Request(info) => started.contains(&info.id),
        NetworkEvent::Response(info) => started.contains(&info.id),
        NetworkEvent::Data { key, .. } => started.contains(&key.id),
        NetworkEvent::Finished { key, .. } | NetworkEvent::Failed { key, .. } => {
            started.remove(&key.id)
        }
        _ => false,
    }
}

/// Applies the user agent override of `Network` and `Emulation`.
pub(super) fn override_user_agent(
    cx: &CommandContext,
    user_agent: &str,
    accept_language: Option<&str>,
) -> Result<(), CdpError> {
    let target = cx.target()?;
    let user_agent = (!user_agent.is_empty()).then(|| user_agent.to_owned());
    let language = accept_language
        .filter(|language| !language.is_empty())
        .map(str::to_owned);
    cx.session()
        .state::<NetState>()
        .set_override(cx.session(), target, |stacks, owner| {
            stacks.set_agent(owner, (user_agent, language));
        });
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadersParams {
    headers: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheParams {
    cache_disabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentParams {
    user_agent: String,
    accept_language: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConditionParams {
    offline: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestIdParams {
    request_id: String,
}

/// The page's request number named by a client's `requestId`, if it names one
/// of this target's.
fn parse_request_id(target: &Target, request_id: &str) -> Option<u64> {
    let prefix = format!("{}.", target.page().id().raw());
    if let Some(number) = request_id.strip_prefix(&prefix) {
        return number.parse().ok();
    }
    // A document's request id is its loader id: the page, then the navigation.
    if request_id.len() == 32 && request_id.is_ascii() {
        let (page, navigation) = request_id.split_at(16);
        let page = u64::from_str_radix(page, 16).ok()?;
        if page == target.page().id().raw() {
            return Some(bridge::DOCUMENT_BIT | u64::from_str_radix(navigation, 16).ok()?);
        }
    }
    None
}

impl DomainHandler for Network {
    fn domain(&self) -> &'static str {
        "Network"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "enable",
            "disable",
            "setExtraHTTPHeaders",
            "setCacheDisabled",
            "setUserAgentOverride",
            "setBypassServiceWorker",
            "emulateNetworkConditions",
            "clearBrowserCache",
            "getResponseBody",
            "getRequestPostData",
            "getCookies",
            "getAllCookies",
            "setCookie",
            "setCookies",
            "deleteCookies",
            "clearBrowserCookies",
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
                    let subscriber = cx
                        .session()
                        .state::<NetState>()
                        .subscriber(cx.session(), target);
                    subscriber.set_events(true);
                    target.sync_network();
                    Ok(json!({}))
                }
                "disable" => {
                    let state = cx.session().state::<NetState>();
                    if lock(&state.inner).subscriber.is_some() {
                        state.subscriber(cx.session(), target).set_events(false);
                        target.sync_network();
                    }
                    Ok(json!({}))
                }
                "setExtraHTTPHeaders" => {
                    let HeadersParams { headers } = parse_params(params)?;
                    let headers = headers
                        .into_iter()
                        .map(|(name, value)| {
                            let value = match value {
                                Value::String(text) => text,
                                other => other.to_string(),
                            };
                            (name, value)
                        })
                        .collect::<Vec<(String, String)>>();
                    tenta_browser::network::check_headers(&headers)
                        .map_err(CdpError::invalid_params)?;
                    cx.session().state::<NetState>().set_override(
                        cx.session(),
                        target,
                        |stacks, owner| stacks.set_headers(owner, headers),
                    );
                    Ok(json!({}))
                }
                "setCacheDisabled" => {
                    let CacheParams { cache_disabled } = parse_params(params)?;
                    cx.session().state::<NetState>().set_override(
                        cx.session(),
                        target,
                        |stacks, owner| stacks.set_cache_disabled(owner, cache_disabled),
                    );
                    Ok(json!({}))
                }
                "setUserAgentOverride" => {
                    let AgentParams {
                        user_agent,
                        accept_language,
                    } = parse_params(params)?;
                    override_user_agent(&cx, &user_agent, accept_language.as_deref())?;
                    Ok(json!({}))
                }
                // There are no service workers.
                "setBypassServiceWorker" => Ok(json!({})),
                // Only `offline` is honoured; the engine has no throttling.
                "emulateNetworkConditions" => {
                    let ConditionParams { offline } = parse_params(params)?;
                    cx.session().state::<NetState>().set_override(
                        cx.session(),
                        target,
                        |stacks, owner| stacks.set_offline(owner, offline),
                    );
                    Ok(json!({}))
                }
                "clearBrowserCache" => {
                    let (_, context) =
                        super::target::find_context(cx.backend(), Some(target.context_id()))?;
                    context.cache().clear();
                    Ok(json!({}))
                }
                "getResponseBody" => {
                    let RequestIdParams { request_id } = parse_params(params)?;
                    let kept = parse_request_id(target, &request_id)
                        .and_then(|id| target.network().response_body(id));
                    let Some((body, text)) = kept else {
                        return Err(CdpError::server("No resource with given identifier found"));
                    };
                    Ok(match (text, std::str::from_utf8(&body)) {
                        (true, Ok(text)) => json!({ "body": text, "base64Encoded": false }),
                        _ => json!({
                            "body": base64::engine::general_purpose::STANDARD.encode(&body),
                            "base64Encoded": true,
                        }),
                    })
                }
                "getRequestPostData" => {
                    let RequestIdParams { request_id } = parse_params(params)?;
                    let kept = parse_request_id(target, &request_id)
                        .and_then(|id| target.network().post_data(id));
                    match kept {
                        Some(post) => Ok(json!({
                            "postData": String::from_utf8_lossy(&post),
                        })),
                        None => Err(CdpError::server("No post data available for the request")),
                    }
                }
                "getCookies"
                | "getAllCookies"
                | "setCookie"
                | "setCookies"
                | "deleteCookies"
                | "clearBrowserCookies" => cookies::handle(&cx, method, params),
                other => Err(CdpError::method_not_found(&format!("Network.{other}"))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use tenta_browser::network::{
        Initiator, RedirectResponse, RequestInfo, RequestKey, ResourceType,
    };

    use super::*;

    fn info(id: u64, redirect: bool) -> RequestInfo {
        let url = url::Url::parse("http://a.test/").unwrap();
        RequestInfo {
            id,
            loader: 1,
            frame: tenta_core::FrameId(1),
            resource_type: ResourceType::Fetch,
            url: url.clone(),
            method: "GET".to_owned(),
            headers: Vec::new(),
            post_data: None,
            document_url: url.clone(),
            initiator: Initiator::Other,
            redirect: redirect.then(|| RedirectResponse {
                url,
                status: 302,
                headers: Vec::new(),
                from_cache: false,
            }),
            user_gesture: false,
            at: Instant::now(),
        }
    }

    #[test]
    fn a_request_the_session_never_saw_start_is_not_reported_from_the_middle() {
        let key = |id| RequestKey {
            id,
            loader: 1,
            resource_type: ResourceType::Fetch,
        };
        let mut started = HashSet::new();
        // Request 1 began before the domain was enabled.
        assert!(!admit(
            &mut started,
            &NetworkEvent::Request(Box::new(info(1, true)))
        ));
        assert!(!admit(
            &mut started,
            &NetworkEvent::Finished {
                key: key(1),
                encoded_length: 0,
                at: Instant::now()
            }
        ));
        // Request 2 is seen whole, redirect included.
        assert!(admit(
            &mut started,
            &NetworkEvent::Request(Box::new(info(2, false)))
        ));
        assert!(admit(
            &mut started,
            &NetworkEvent::Request(Box::new(info(2, true)))
        ));
        assert!(admit(
            &mut started,
            &NetworkEvent::Finished {
                key: key(2),
                encoded_length: 0,
                at: Instant::now()
            }
        ));
        assert!(started.is_empty(), "an ended request is forgotten");
    }
}
