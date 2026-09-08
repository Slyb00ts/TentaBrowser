//! HTTP/2 through `tenta-h2` directly (`docs/spec/03-network.md` §1).
//!
//! hyper is not in this path: the fingerprint lives in the framing, and
//! `tenta-h2`'s client builder is where it is configured.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http::{Request as HttpRequest, Response as HttpResponse};
use tenta_h2::client::{self, ResponseFuture, SendRequest};
use tenta_h2::{RecvStream, SendStream};

use crate::error::{ErrorName, NetError};
use crate::profile::Profile;
use crate::stream::NetStream;

/// One HTTP/2 connection. Cloning it is how streams are multiplexed.
/// The connection task sets `alive` to false when it exits, so the pool
/// knows to evict a handle whose task has died (BLOCKING-1).
#[derive(Clone, Debug)]
pub struct H2Handle {
    sender: SendRequest<Bytes>,
    /// Shared flag set to false when the connection task exits.
    alive: Arc<AtomicBool>,
}

impl H2Handle {
    /// Whether the connection task is still running.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

impl H2Handle {
    /// Opens one stream. The response future and the send half are returned
    /// separately: a request body has to be written while the response is
    /// still pending, or a server that waits for the body deadlocks.
    pub async fn start(
        &mut self,
        request: HttpRequest<()>,
        end_of_stream: bool,
    ) -> Result<(ResponseFuture, SendStream<Bytes>), NetError> {
        let mut sender = self
            .sender
            .clone()
            .ready()
            .await
            .map_err(|err| h2_error(&err))?;
        let (response, send_stream) = sender
            .send_request(request, end_of_stream)
            .map_err(|err| h2_error(&err))?;
        self.sender = sender;
        Ok((response, send_stream))
    }

    /// Awaits a response head opened by [`H2Handle::start`].
    pub async fn response(response: ResponseFuture) -> Result<HttpResponse<RecvStream>, NetError> {
        response.await.map_err(|err| h2_error(&err))
    }
}

/// Performs the HTTP/2 handshake with the profile's fingerprint and spawns
/// the connection task, including its keep-alive pings.
pub async fn handshake(
    stream: NetStream,
    profile: &Profile,
    keep_alive: Option<Duration>,
) -> Result<H2Handle, NetError> {
    let mut builder = client::Builder::new();
    if let Some(client_profile) = profile.client_profile() {
        builder.fingerprint(client_profile.h2_fingerprint());
    }
    let (sender, connection) = builder
        .handshake::<_, Bytes>(stream)
        .await
        .map_err(|err| h2_error(&err))?;

    let mut ping_pong = None;
    let mut connection = connection;
    if keep_alive.is_some() {
        ping_pong = connection.ping_pong();
    }

    let alive = Arc::new(AtomicBool::new(true));
    let alive_for_task = Arc::clone(&alive);

    tokio::task::spawn_local(async move {
        if let Err(err) = connection.await {
            tracing::debug!(error = %err, "http/2 connection closed");
        }
        // Signal that the connection task has exited (BLOCKING-1).
        alive_for_task.store(false, Ordering::Release);
    });

    if let (Some(mut ping_pong), Some(interval)) = (ping_pong, keep_alive) {
        // Idle pings keep a warm pooled connection alive through NAT and
        // proxy idle timers (`docs/spec/03-network.md` §3). A peer that stops
        // answering the ping — a NAT that dropped the mapping, a wedged origin,
        // a middlebox black-holing an idle socket — never closes the TCP
        // connection, so the connection task never exits and the socket looks
        // alive forever. Chrome closes such a session when a PING goes
        // unanswered for 10 s; we do the same, marking the handle dead so the
        // pool opens a fresh connection instead of parking every later request
        // on a socket that will never answer (BLOCKING-2).
        let alive_for_ping = Arc::clone(&alive);
        tokio::task::spawn_local(async move {
            const PING_TIMEOUT: Duration = Duration::from_secs(10);
            loop {
                tokio::time::sleep(interval).await;
                match tokio::time::timeout(PING_TIMEOUT, ping_pong.ping(tenta_h2::Ping::opaque()))
                    .await
                {
                    Ok(Ok(_)) => {}
                    // Ping failed, or no PONG arrived within the timeout: the
                    // peer is gone. Mark the handle dead and stop pinging.
                    _ => {
                        alive_for_ping.store(false, Ordering::Release);
                        break;
                    }
                }
            }
        });
    }

    Ok(H2Handle { sender, alive })
}

/// Maps an HTTP/2 error to a Chromium name.
pub fn h2_error(err: &tenta_h2::Error) -> NetError {
    if let Some(io) = err.get_io() {
        return NetError::from_stream_io(io);
    }
    // Distinguish GOAWAY (connection-level) from stream-level errors.
    if err.is_go_away() {
        return NetError::new(
            ErrorName::Http2ProtocolError,
            format!("connection error received: {err}"),
        );
    }
    if err.is_reset() {
        // Stream-level error; connection is still valid.
        return NetError::new(
            ErrorName::Http2ProtocolError,
            format!("stream reset: {err}"),
        );
    }
    NetError::new(ErrorName::Http2ProtocolError, err.to_string())
}
