//! Interception and observation, registered per context
//! (`docs/spec/03-network.md` §9).
//!
//! The transport takes its hooks once, when the client is built, so a context
//! installs one hub of each kind and adds to it afterwards. Registration order
//! is call order: the first interceptor that does not continue decides.

use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use parking_lot::RwLock;
use tenta_net::hooks::{BoxFuture, Interception, Interceptor, Observer};
use tenta_net::request::Request;
use url::Url;

/// Every interceptor of one context.
#[derive(Default)]
pub(crate) struct InterceptHub {
    interceptors: RwLock<Vec<Arc<dyn Interceptor>>>,
}

impl InterceptHub {
    pub(crate) fn push(&self, interceptor: Arc<dyn Interceptor>) {
        self.interceptors.write().push(interceptor);
    }

    pub(crate) fn len(&self) -> usize {
        self.interceptors.read().len()
    }
}

impl Interceptor for InterceptHub {
    fn intercept<'a>(&'a self, request: &'a mut Request) -> BoxFuture<'a, Interception> {
        Box::pin(async move {
            // The registrations are copied out before the first await so the
            // lock is never held across one.
            let interceptors: Vec<Arc<dyn Interceptor>> = self.interceptors.read().clone();
            for interceptor in interceptors {
                match interceptor.intercept(request).await {
                    Interception::Continue => {}
                    decided => return decided,
                }
            }
            Interception::Continue
        })
    }
}

/// Every passive observer of one context.
#[derive(Default)]
pub(crate) struct ObserverHub {
    observers: RwLock<Vec<Arc<dyn Observer>>>,
}

impl ObserverHub {
    pub(crate) fn push(&self, observer: Arc<dyn Observer>) {
        self.observers.write().push(observer);
    }

    pub(crate) fn len(&self) -> usize {
        self.observers.read().len()
    }
}

impl Observer for ObserverHub {
    fn on_request(&self, request: &Request) {
        for observer in self.observers.read().iter() {
            observer.on_request(request);
        }
    }

    fn on_response(&self, url: &Url, status: StatusCode, headers: &HeaderMap) {
        for observer in self.observers.read().iter() {
            observer.on_response(url, status, headers);
        }
    }

    fn on_body_chunk(&self, url: &Url, chunk: &Bytes) {
        for observer in self.observers.read().iter() {
            observer.on_body_chunk(url, chunk);
        }
    }
}
