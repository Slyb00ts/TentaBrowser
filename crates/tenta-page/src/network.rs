//! What a page tells a protocol host about its network traffic, and the
//! per-page overrides such a host applies to it (`docs/spec/09-cdp-compat.md`
//! §3, the CDP `Network` and `Fetch` domains).
//!
//! A [`NetworkHub`] belongs to one page. Until a host calls one of its setters
//! the hub is inactive and every request site pays one relaxed atomic load.
//! Once active, a request is announced to the host's [`NetworkSink`], carries a
//! [`RequestTap`] through the transport (so redirects and interception are
//! attributed to this page and this request), and reports its response, body
//! and ending through a [`Tracked`].
//!
//! Events are pushed synchronously from the thread that produces them and must
//! not block: a sink queues them. Request headers are the ones the page set
//! plus the overrides; the transport adds the profile's own headers
//! (`User-Agent` from the profile, `Accept`, `Cookie`, ...) later and does not
//! report them back.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

pub use bytes::Bytes;
use http::HeaderValue;
use parking_lot::RwLock;
use tenta_core::{FrameId, RequestId};
use tenta_net::hooks::{BoxFuture, Interception, RequestTap};
use tenta_net::request::ResourceType;
use tenta_net::{ErrorName, NetError, RedirectHop, Request, Response, Timing};
use url::Url;

/// The most bytes of one response body kept for `Network.getResponseBody`.
/// A larger body is reported (`Data`, `Finished`) and not retained.
pub const RETAINED_BODY_LIMIT: usize = 16 * 1024 * 1024;

/// What started a request, as DevTools reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Initiator {
    /// A navigation the host or the user started.
    Other,
    /// The HTML or CSS parser found a resource of this document.
    Parser(Url),
    /// Page script called `fetch()` or `XMLHttpRequest`.
    Script(Url),
}

/// The 3xx answer that sent a request to its current URL.
#[derive(Clone, Debug)]
pub struct RedirectResponse {
    pub url: Url,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub from_cache: bool,
}

/// What names a request in every event about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestKey {
    /// Stable across the hops of one request.
    pub id: u64,
    /// The navigation that loaded the document the request belongs to; for a
    /// document request, the navigation it is.
    pub loader: u64,
    pub resource_type: ResourceType,
}

/// The HTTP reason phrase for `status`, or an empty string.
pub fn reason_phrase(status: u16) -> &'static str {
    http::StatusCode::from_u16(status)
        .ok()
        .and_then(|status| status.canonical_reason())
        .unwrap_or("")
}

/// One hop of a request, about to leave.
#[derive(Clone, Debug)]
pub struct RequestInfo {
    /// Stable across the hops of one request.
    pub id: u64,
    /// The navigation that loaded the document this request belongs to; for a
    /// document request, the navigation it is.
    pub loader: u64,
    pub frame: FrameId,
    pub resource_type: ResourceType,
    pub url: Url,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub post_data: Option<Bytes>,
    pub document_url: Url,
    pub initiator: Initiator,
    /// Set from the second hop on.
    pub redirect: Option<RedirectResponse>,
    pub user_gesture: bool,
    pub at: Instant,
}

/// A response head.
#[derive(Clone, Debug)]
pub struct ResponseInfo {
    pub id: u64,
    pub loader: u64,
    pub frame: FrameId,
    pub resource_type: ResourceType,
    pub url: Url,
    pub status: u16,
    /// Exactly what the server sent, with a repeated name repeated.
    pub headers: Vec<(String, String)>,
    pub mime_type: String,
    /// The `charset` parameter of the content type, or empty.
    pub charset: String,
    pub protocol: &'static str,
    pub remote: Option<SocketAddr>,
    pub timing: Timing,
    pub from_cache: bool,
    /// Whether the transport answered without a connection (an interceptor).
    pub local: bool,
    pub requested_at: Instant,
    pub at: Instant,
}

/// One step in the life of a request.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum NetworkEvent {
    Request(Box<RequestInfo>),
    Response(Box<ResponseInfo>),
    Data {
        key: RequestKey,
        length: u64,
        at: Instant,
    },
    /// The complete body, sent just before `Finished` when it fits
    /// [`RETAINED_BODY_LIMIT`].
    Body {
        key: RequestKey,
        body: Bytes,
    },
    Finished {
        key: RequestKey,
        encoded_length: u64,
        at: Instant,
    },
    Failed {
        key: RequestKey,
        error: String,
        canceled: bool,
        blocked: Option<&'static str>,
        at: Instant,
    },
}

/// How a paused request goes on.
pub enum Resolution {
    /// Send it, with the changes the host made.
    Continue(Modifications),
    /// Answer it without the network.
    Fulfill(tenta_net::hooks::FulfilledResponse),
    /// Fail it.
    Fail(NetError),
}

/// `Err` names the first header that cannot go on the wire.
pub fn check_headers(headers: &[(String, String)]) -> Result<(), String> {
    for (name, value) in headers {
        if http::HeaderName::try_from(name.as_str()).is_err() {
            return Err(format!("Invalid header name: {name:?}"));
        }
        if HeaderValue::from_str(value).is_err() {
            return Err(format!("Invalid value for header {name:?}"));
        }
    }
    Ok(())
}

impl Resolution {
    /// Answers the request with `status`, `headers` and `body`. `None` when
    /// the status is not an HTTP status or a header is invalid
    /// ([`check_headers`] says which).
    pub fn fulfill(status: u16, headers: Vec<(String, String)>, body: Bytes) -> Option<Self> {
        check_headers(&headers).ok()?;
        let status = http::StatusCode::from_u16(status).ok()?;
        let mut response = tenta_net::hooks::FulfilledResponse::new(status, body);
        for (name, value) in headers {
            response = response.header(&name, &value);
        }
        Some(Resolution::Fulfill(response))
    }

    /// Fails the request with a Chromium error name (`net::ERR_FAILED` or
    /// `ERR_FAILED`). `None` when the name is unknown.
    pub fn fail(name: &str) -> Option<Self> {
        let name = ErrorName::parse(name)?;
        Some(Resolution::Fail(NetError::new(
            name,
            "the request was failed by the protocol host",
        )))
    }
}

/// What a host changed on a request it held.
#[derive(Default)]
pub struct Modifications {
    pub url: Option<Url>,
    pub method: Option<String>,
    /// Replaces every header the request carried.
    pub headers: Option<Vec<(String, String)>>,
    pub post_data: Option<Bytes>,
}

/// The host side of a page's network traffic.
pub trait NetworkSink: Send + Sync {
    /// Called on whichever thread produced the event; must not block.
    fn event(&self, event: NetworkEvent);

    /// Holds `request` until the host decides. Only called while interception
    /// is on. Dropping the future lets the request continue unchanged.
    fn pause<'a>(&'a self, request: &'a RequestInfo) -> BoxFuture<'a, Resolution>;
}

#[derive(Default)]
struct Settings {
    sink: Option<Arc<dyn NetworkSink>>,
    events: bool,
    intercept: bool,
    extra_headers: Vec<(String, String)>,
    user_agent: Option<String>,
    accept_language: Option<String>,
    cache_disabled: bool,
    offline: bool,
    discard_bodies: bool,
}

impl Settings {
    fn active(&self) -> bool {
        self.sink.is_some() && (self.events || self.intercept)
            || self.offline
            || self.cache_disabled
            || self.user_agent.is_some()
            || self.accept_language.is_some()
            || !self.extra_headers.is_empty()
    }
}

#[derive(Default)]
struct DocumentRef {
    loader: u64,
    url: Option<Url>,
}

#[derive(Default)]
struct HubInner {
    active: AtomicBool,
    frame: AtomicU64,
    settings: RwLock<Settings>,
    document: RwLock<DocumentRef>,
}

/// One page's network observation point and overrides.
#[derive(Clone, Default)]
pub struct NetworkHub {
    inner: Arc<HubInner>,
}

impl std::fmt::Debug for NetworkHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkHub")
            .field("active", &self.inner.active.load(Ordering::Relaxed))
            .finish()
    }
}

impl NetworkHub {
    fn update(&self, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.inner.settings.write();
        change(&mut settings);
        self.inner
            .active
            .store(settings.active(), Ordering::Relaxed);
    }

    /// Installs the host that receives events and decides paused requests.
    pub fn set_sink(&self, sink: Option<Arc<dyn NetworkSink>>) {
        self.update(|settings| settings.sink = sink);
    }

    /// Whether requests are announced to the sink.
    pub fn set_events(&self, on: bool) {
        self.update(|settings| settings.events = on);
    }

    /// Whether a response body is kept and handed to the sink as
    /// [`NetworkEvent::Body`], which is the default. A host that only lists
    /// requests turns it off, and then no body is copied for it.
    pub fn set_body_retention(&self, retain: bool) {
        self.update(|settings| settings.discard_bodies = !retain);
    }

    /// Whether every request is held for the sink's decision.
    pub fn set_intercepting(&self, on: bool) {
        self.update(|settings| settings.intercept = on);
    }

    /// Headers added to every request of the page, replacing a header of the
    /// same name the page set.
    pub fn set_extra_headers(&self, headers: Vec<(String, String)>) {
        self.update(|settings| settings.extra_headers = headers);
    }

    /// Replaces the `User-Agent` (and, when given, the `Accept-Language`) of
    /// every request of the page. Script still reads the context's
    /// `navigator.userAgent`.
    pub fn set_user_agent(&self, user_agent: Option<String>, accept_language: Option<String>) {
        self.update(|settings| {
            settings.user_agent = user_agent;
            settings.accept_language = accept_language;
        });
    }

    /// Sends every request to the network instead of the HTTP cache.
    pub fn set_cache_disabled(&self, disabled: bool) {
        self.update(|settings| settings.cache_disabled = disabled);
    }

    /// Fails every request with `net::ERR_INTERNET_DISCONNECTED`.
    pub fn set_offline(&self, offline: bool) {
        self.update(|settings| settings.offline = offline);
    }

    pub(crate) fn bind_frame(&self, frame: FrameId) {
        self.inner.frame.store(frame.raw(), Ordering::Relaxed);
    }

    /// The document the page now shows, which is the loader and the document
    /// URL of every subresource request that follows.
    pub(crate) fn note_document(&self, loader: u64, url: &Url) {
        let mut document = self.inner.document.write();
        document.loader = loader;
        document.url = Some(url.clone());
    }

    /// Prepares `request` and announces it. `None` when the hub is inactive or
    /// nobody listens, in which case there is nothing more to do.
    ///
    /// `loader` is the navigation id of a document request; a subresource
    /// takes the loader of the document it belongs to.
    pub(crate) fn begin(
        &self,
        request: &mut Request,
        id: RequestId,
        loader: Option<u64>,
    ) -> Option<Tracked> {
        if !self.inner.active.load(Ordering::Relaxed) {
            return None;
        }
        let settings = self.inner.settings.read();
        apply_overrides(request, &settings);
        let sink = settings.sink.clone();
        let (events, intercept, offline) = (settings.events, settings.intercept, settings.offline);
        let discard_bodies = settings.discard_bodies;
        drop(settings);
        let sink = sink.filter(|_| events || intercept);
        if sink.is_none() && !offline {
            return None;
        }
        let document = self.inner.document.read();
        let tap = Arc::new(Tap {
            hub: self.clone(),
            id: id.raw(),
            loader: loader.unwrap_or(document.loader),
            frame: FrameId(self.inner.frame.load(Ordering::Relaxed)),
            resource_type: request.resource_type,
            document_url: document.url.clone().unwrap_or_else(|| request.url.clone()),
            initiator: match request.resource_type {
                ResourceType::Document => Initiator::Other,
                ResourceType::Xhr | ResourceType::Fetch => Initiator::Script(
                    request
                        .initiator
                        .clone()
                        .unwrap_or_else(|| request.url.clone()),
                ),
                _ => Initiator::Parser(
                    request
                        .initiator
                        .clone()
                        .unwrap_or_else(|| request.url.clone()),
                ),
            },
        });
        drop(document);
        request.tap = Some(Arc::clone(&tap) as Arc<dyn RequestTap>);
        let sink = sink?;
        if !events {
            return None;
        }
        sink.event(NetworkEvent::Request(Box::new(tap.describe(request, None))));
        Some(Tracked {
            sink,
            tap,
            started: Instant::now(),
            total: 0,
            parts: Vec::new(),
            // A body nobody asked to keep is one that has already overflowed.
            overflow: discard_bodies,
            whole: None,
            ended: false,
        })
    }
}

/// Applies the page-wide overrides to a request about to be sent.
fn apply_overrides(request: &mut Request, settings: &Settings) {
    let set = |request: &mut Request, name: &str, value: &str| {
        if let Ok(value) = HeaderValue::from_str(value) {
            request
                .headers
                .retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
            request.headers.push((name.to_owned(), value));
        }
    };
    for (name, value) in &settings.extra_headers {
        set(request, name, value);
    }
    if let Some(user_agent) = &settings.user_agent {
        set(request, "User-Agent", user_agent);
    }
    if let Some(language) = &settings.accept_language {
        set(request, "Accept-Language", language);
    }
    if settings.cache_disabled {
        request.bypass_cache_lookup = true;
        set(request, "Cache-Control", "no-cache");
        set(request, "Pragma", "no-cache");
    }
}

/// Rewrites the headers of a request as a host asked, keeping only the ones
/// the transport accepts.
fn replace_headers(request: &mut Request, headers: Vec<(String, String)>) {
    request.headers = headers
        .into_iter()
        .filter_map(|(name, value)| {
            http::HeaderName::try_from(name.as_str()).ok()?;
            Some((name, HeaderValue::from_str(&value).ok()?))
        })
        .collect();
}

fn strings(headers: &[(String, HeaderValue)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect()
}

/// The listener one request carries through the transport.
#[derive(Debug)]
struct Tap {
    hub: NetworkHub,
    id: u64,
    loader: u64,
    frame: FrameId,
    resource_type: ResourceType,
    document_url: Url,
    initiator: Initiator,
}

impl Tap {
    fn key(&self) -> RequestKey {
        RequestKey {
            id: self.id,
            loader: self.loader,
            resource_type: self.resource_type,
        }
    }

    fn describe(&self, request: &Request, redirect: Option<RedirectResponse>) -> RequestInfo {
        RequestInfo {
            id: self.id,
            loader: self.loader,
            frame: self.frame,
            resource_type: self.resource_type,
            url: request.url.clone(),
            method: request.method.as_str().to_owned(),
            headers: strings(&request.headers),
            post_data: match &request.body {
                tenta_net::Body::Bytes(bytes) => Some(bytes.clone()),
                _ => None,
            },
            // A navigation's document is the URL it is asking for, hop by hop.
            document_url: if self.resource_type == ResourceType::Document {
                request.url.clone()
            } else {
                self.document_url.clone()
            },
            initiator: self.initiator.clone(),
            redirect,
            user_gesture: request.user_activated,
            at: Instant::now(),
        }
    }

    fn sink(&self) -> Option<Arc<dyn NetworkSink>> {
        self.hub.inner.settings.read().sink.clone()
    }
}

impl RequestTap for Tap {
    fn intercept<'a>(&'a self, request: &'a mut Request) -> BoxFuture<'a, Interception> {
        Box::pin(async move {
            let (sink, intercept, offline) = {
                let settings = self.hub.inner.settings.read();
                (settings.sink.clone(), settings.intercept, settings.offline)
            };
            if offline {
                return Interception::Fail(NetError::new(
                    ErrorName::InternetDisconnected,
                    "the page is emulating an offline network",
                ));
            }
            let Some(sink) = sink.filter(|_| intercept) else {
                return Interception::Continue;
            };
            let info = self.describe(request, None);
            match sink.pause(&info).await {
                Resolution::Continue(changes) => {
                    if let Some(url) = changes.url {
                        request.url = url;
                    }
                    if let Some(method) = changes
                        .method
                        .and_then(|method| http::Method::from_bytes(method.as_bytes()).ok())
                    {
                        request.method = method;
                    }
                    if let Some(headers) = changes.headers {
                        replace_headers(request, headers);
                    }
                    if let Some(body) = changes.post_data {
                        request.body = body.into();
                    }
                    Interception::Continue
                }
                Resolution::Fulfill(response) => Interception::Fulfill(response),
                Resolution::Fail(error) => Interception::Fail(error),
            }
        })
    }

    fn redirected(&self, next: &Request, hop: &RedirectHop, headers: &[(String, HeaderValue)]) {
        let Some(sink) = self.sink() else { return };
        let redirect = RedirectResponse {
            url: hop.url.clone(),
            status: hop.status,
            headers: strings(headers),
            from_cache: hop.from_cache,
        };
        sink.event(NetworkEvent::Request(Box::new(
            self.describe(next, Some(redirect)),
        )));
    }
}

/// The reporting half of one announced request.
///
/// A request that is dropped before it ended is reported as cancelled, so a
/// navigation that a newer one replaced, or a task that was aborted, never
/// leaves the host waiting for a finish.
pub(crate) struct Tracked {
    sink: Arc<dyn NetworkSink>,
    tap: Arc<Tap>,
    started: Instant,
    total: u64,
    parts: Vec<u8>,
    overflow: bool,
    whole: Option<Bytes>,
    ended: bool,
}

impl Tracked {
    /// The response head arrived. Redirect hops were reported as they
    /// happened.
    pub(crate) fn response(&mut self, response: &Response) {
        let content_type = response.header("content-type").unwrap_or_default();
        let mime_type = content_type
            .split(';')
            .next()
            .map(|essence| essence.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let charset = content_type
            .split(';')
            .skip(1)
            .filter_map(|parameter| parameter.split_once('='))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("charset"))
            .map(|(_, value)| value.trim().trim_matches('"').to_owned())
            .unwrap_or_default();
        self.sink
            .event(NetworkEvent::Response(Box::new(ResponseInfo {
                id: self.tap.id,
                loader: self.tap.loader,
                frame: self.tap.frame,
                resource_type: self.tap.resource_type,
                url: response.url.clone(),
                status: response.status.as_u16(),
                headers: strings(&response.raw_headers),
                mime_type,
                charset,
                protocol: response.protocol.as_str(),
                remote: response.remote_addr,
                timing: response.timing,
                from_cache: response.from_cache,
                local: matches!(response.protocol, tenta_net::Protocol::Local),
                requested_at: self.started,
                at: Instant::now(),
            })));
    }

    /// A piece of the body arrived.
    pub(crate) fn chunk(&mut self, chunk: &Bytes) {
        self.total += chunk.len() as u64;
        if !self.overflow {
            if self.parts.len() + chunk.len() > RETAINED_BODY_LIMIT {
                self.overflow = true;
                self.parts = Vec::new();
            } else {
                self.parts.extend_from_slice(chunk);
            }
        }
        self.sink.event(NetworkEvent::Data {
            key: self.tap.key(),
            length: chunk.len() as u64,
            at: Instant::now(),
        });
    }

    /// The whole body is `body`; the request ends.
    pub(crate) fn complete(&mut self, body: &Bytes) {
        if !body.is_empty() {
            self.total += body.len() as u64;
            self.sink.event(NetworkEvent::Data {
                key: self.tap.key(),
                length: body.len() as u64,
                at: Instant::now(),
            });
        }
        if self.overflow {
            // Nothing is kept: the limit was passed, or the host keeps none.
        } else if body.len() <= RETAINED_BODY_LIMIT {
            self.whole = Some(body.clone());
        } else {
            self.overflow = true;
        }
        self.finish();
    }

    /// The body ended.
    pub(crate) fn finish(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        if !self.overflow {
            let body = self
                .whole
                .take()
                .unwrap_or_else(|| Bytes::from(std::mem::take(&mut self.parts)));
            self.sink.event(NetworkEvent::Body {
                key: self.tap.key(),
                body,
            });
        }
        self.sink.event(NetworkEvent::Finished {
            key: self.tap.key(),
            encoded_length: self.total,
            at: Instant::now(),
        });
    }

    /// The transport failed the request.
    pub(crate) fn failed(&mut self, error: &NetError) {
        let blocked = match error.name {
            ErrorName::BlockedByClient => Some("inspector"),
            _ => None,
        };
        self.fail(
            error.error_text(),
            matches!(error.name, ErrorName::Aborted),
            blocked,
        );
    }

    /// The request failed for a reason the page decided.
    pub(crate) fn failed_with(&mut self, error: &str) {
        self.fail(error.to_owned(), error == "net::ERR_ABORTED", None);
    }

    fn fail(&mut self, error: String, canceled: bool, blocked: Option<&'static str>) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.sink.event(NetworkEvent::Failed {
            key: self.tap.key(),
            error,
            canceled,
            blocked,
            at: Instant::now(),
        });
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.fail("net::ERR_ABORTED".to_owned(), true, None);
    }
}
