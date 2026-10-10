//! Request interception and passive observation (`docs/spec/03-network.md`
//! §9).
//!
//! Cookies, the HTTP cache, the session cache and the destination policy are
//! not hooks: they are the types in [`crate::policy`], configured on
//! [`crate::client::ClientConfig`] and called directly by the client. What is
//! left here is what a CDP or MCP client plugs in at runtime.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use url::Url;

use crate::request::Request;

/// A boxed future with the lifetime of the borrowed request.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What an interceptor decided about a request.
pub enum Interception {
    /// Send the request (possibly modified in place).
    Continue,
    /// Answer it without touching the network.
    Fulfill(FulfilledResponse),
    /// Fail it with a `net::ERR_*` name.
    Fail(crate::error::NetError),
}

/// A response an interceptor produced.
#[derive(Clone, Debug)]
pub struct FulfilledResponse {
    pub status: StatusCode,
    pub headers: Vec<(String, http::HeaderValue)>,
    pub body: Bytes,
}

impl FulfilledResponse {
    pub fn new(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        if let Ok(value) = http::HeaderValue::from_str(value) {
            self.headers.push((name.to_owned(), value));
        }
        self
    }
}

/// A request interceptor. Interceptors run in registration order; the first
/// one that does not return [`Interception::Continue`] decides.
pub trait Interceptor: Send + Sync {
    fn intercept<'a>(&'a self, request: &'a mut Request) -> BoxFuture<'a, Interception>;
}

/// Passive observers. They are only consulted when at least one is
/// registered, which the client checks with an atomic flag, not a lock
/// (`docs/spec/03-network.md` §9).
pub trait Observer: Send + Sync {
    fn on_request(&self, _request: &Request) {}
    fn on_response(&self, _url: &Url, _status: StatusCode, _headers: &HeaderMap) {}
    fn on_body_chunk(&self, _url: &Url, _chunk: &Bytes) {}
}

/// A listener one request carries with it (`Request::tap`).
///
/// The context-wide [`Observer`] and [`Interceptor`] cannot tell which page a
/// request belongs to; a tap is attached by the page that issues the request,
/// so it can. A request without one pays a single `Option` check.
pub trait RequestTap: Send + Sync + std::fmt::Debug {
    /// Runs on every hop, after the context's interceptors and before the
    /// cache: the same place a context interceptor runs.
    fn intercept<'a>(&'a self, _request: &'a mut Request) -> BoxFuture<'a, Interception> {
        Box::pin(std::future::ready(Interception::Continue))
    }

    /// A 3xx answered the previous hop and `next` is the request that follows
    /// it. `headers` are the redirect response's.
    fn redirected(
        &self,
        _next: &Request,
        _hop: &crate::client::RedirectHop,
        _headers: &[(String, http::HeaderValue)],
    ) {
    }
}

/// Everything a client lets a driver plug in at runtime.
#[derive(Clone, Default)]
pub struct Hooks {
    pub interceptors: Vec<Arc<dyn Interceptor>>,
    pub observers: Vec<Arc<dyn Observer>>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks")
            .field("interceptors", &self.interceptors.len())
            .field("observers", &self.observers.len())
            .finish()
    }
}
