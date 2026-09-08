//! HTTP/1.1 over our own connector (`docs/spec/03-network.md` §1).
//!
//! hyper's `client::conn::http1` codec, driven by our pool: hyper-util's
//! pooling client is not used, because the pool has to be keyed by profile
//! and context and has to answer questions (is this connection warm?) that it
//! does not expose.

use std::rc::Rc;

use http::{Request as HttpRequest, Response as HttpResponse};
use hyper::body::Incoming;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;

use crate::body::OutgoingBody;
use crate::error::{ErrorName, NetError};
use crate::h1_case::{self, CaseRestoringStream, Rewrites};
use crate::stream::NetStream;

/// One HTTP/1.1 connection.
#[derive(Debug)]
pub struct H1Handle {
    sender: http1::SendRequest<OutgoingBody>,
    /// Present only for browser profiles: the request heads whose casing and
    /// order the writer restores (`h1_case`).
    rewrites: Option<Rewrites>,
}

impl H1Handle {
    /// Whether the connection can still be used.
    pub fn is_usable(&self) -> bool {
        !self.sender.is_closed()
    }

    /// Sends one request. The response body is still streaming when this
    /// returns.
    ///
    /// `ordered` is the header list as the profile wants it on the wire, in
    /// its own casing and order; `request` carries the same headers in an
    /// `http::HeaderMap`, which keeps neither.
    pub async fn send(
        &mut self,
        request: HttpRequest<OutgoingBody>,
        ordered: &[(String, http::HeaderValue)],
    ) -> Result<HttpResponse<Incoming>, NetError> {
        self.sender
            .ready()
            .await
            .map_err(|err| hyper_error(&err, ErrorName::ConnectionClosed))?;
        if let Some(queue) = &self.rewrites {
            let rewrite = h1_case::head_rewrite(
                request.method(),
                &request.uri().to_string(),
                request.headers(),
                ordered,
            );
            queue.borrow_mut().push_back(rewrite);
        }
        self.sender
            .send_request(request)
            .await
            .map_err(|err| hyper_error(&err, ErrorName::ConnectionFailed))
    }
}

/// Performs the HTTP/1.1 handshake and spawns the connection task on the
/// current (io thread) runtime.
///
/// With `browser_profile`, hyper's title casing is turned on — it is the
/// casing `h1_case` predicts — and the stream is wrapped so the profile's own
/// casing and order reach the wire. Without it hyper writes the headers as it
/// always has and the wrapper is a straight pass-through.
pub async fn handshake(stream: NetStream, browser_profile: bool) -> Result<H1Handle, NetError> {
    let mut builder = http1::Builder::new();
    builder
        .title_case_headers(browser_profile)
        // Chrome does not pipeline and neither do we; one request in flight
        // per connection, six connections per host.
        // Response head limits match Chrome: up to 256 KiB total, no fixed header count limit.
        .max_buf_size(256 * 1024)
        .max_headers(1024);
    let queue = h1_case::rewrites();
    let stream = CaseRestoringStream::new(stream, Rc::clone(&queue));
    let (sender, connection) = builder
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|err| hyper_error(&err, ErrorName::ConnectionFailed))?;

    tokio::task::spawn_local(async move {
        if let Err(err) = connection.await {
            tracing::debug!(error = %err, "http/1.1 connection closed");
        }
    });

    Ok(H1Handle {
        sender,
        rewrites: browser_profile.then_some(queue),
    })
}

fn hyper_error(err: &hyper::Error, fallback: ErrorName) -> NetError {
    if err.is_timeout() {
        return NetError::new(ErrorName::TimedOut, err.to_string());
    }
    if err.is_parse_too_large() {
        return NetError::new(ErrorName::ResponseHeadersTooBig, err.to_string());
    }
    if err.is_parse() || err.is_parse_status() {
        return NetError::new(ErrorName::InvalidResponse, err.to_string());
    }
    if err.is_incomplete_message() {
        return NetError::new(ErrorName::EmptyResponse, err.to_string());
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
    while let Some(inner) = source {
        if let Some(io) = inner.downcast_ref::<std::io::Error>() {
            return NetError::from_stream_io(io);
        }
        source = inner.source();
    }
    NetError::new(fallback, err.to_string())
}
