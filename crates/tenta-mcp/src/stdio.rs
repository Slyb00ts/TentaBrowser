//! The stdio transport: newline-delimited JSON-RPC over a byte stream.
//!
//! Requests run concurrently, so a `notifications/cancelled` can end one in
//! flight; a cancelled request gets no response, as the protocol says. At
//! end of input the requests still running are answered before returning, so
//! `printf '...' | tentabrowser mcp` prints its replies.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};

use crate::jsonrpc::{self, INTERNAL_ERROR, RpcError};
use crate::server::McpServer;

/// The longest message accepted on one line.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Serves one client until `reader` ends, then closes its browser.
pub async fn serve<R, W>(server: &McpServer, reader: R, writer: W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let connection = server
        .connect()
        .map_err(|error| io::Error::other(error.message))?;
    let (outgoing, mut lines) = mpsc::unbounded_channel::<String>();
    let writer_task = tokio::spawn(async move {
        let mut writer = writer;
        while let Some(line) = lines.recv().await {
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
        io::Result::Ok(())
    });
    let in_flight: Arc<Mutex<HashMap<String, AbortHandle>>> = Arc::default();
    let mut tasks = JoinSet::new();
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();

    let outcome = loop {
        buffer.clear();
        let read = (&mut reader)
            .take(MAX_MESSAGE_BYTES as u64 + 1)
            .read_until(b'\n', &mut buffer)
            .await;
        match read {
            Ok(0) => break Ok(()),
            Ok(_) if buffer.len() > MAX_MESSAGE_BYTES => {
                let _ = outgoing.send(
                    jsonrpc::failure(
                        &Value::Null,
                        &RpcError::new(jsonrpc::INVALID_REQUEST, "message too large"),
                    )
                    .to_string(),
                );
                break Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "message too large",
                ));
            }
            Ok(_) => {}
            Err(error) => break Err(error),
        }
        let text = String::from_utf8_lossy(&buffer);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(text) {
            Ok(message) => message,
            Err(error) => {
                let _ = outgoing.send(jsonrpc::parse_failure(&error.to_string()).to_string());
                continue;
            }
        };
        if message.get("method").and_then(Value::as_str) == Some("notifications/cancelled") {
            if let Some(id) = message.pointer("/params/requestId") {
                let aborted = lock(&in_flight).remove(&id.to_string());
                if let Some(handle) = aborted {
                    handle.abort();
                }
            }
            continue;
        }
        let request_id = message
            .get("id")
            .filter(|_| message.get("method").is_some())
            .cloned();
        let connection = Arc::clone(&connection);
        let task = tokio::spawn(async move { connection.handle(message).await });
        let key = request_id.as_ref().map(Value::to_string);
        if let Some(key) = &key {
            lock(&in_flight).insert(key.clone(), task.abort_handle());
        }
        let outgoing = outgoing.clone();
        let in_flight = Arc::clone(&in_flight);
        tasks.spawn(async move {
            let reply = match task.await {
                Ok(reply) => reply,
                Err(error) if error.is_panic() => Some(jsonrpc::failure(
                    request_id.as_ref().unwrap_or(&Value::Null),
                    &RpcError::new(INTERNAL_ERROR, "the request failed internally"),
                )),
                // Cancelled by the client: no response.
                Err(_) => None,
            };
            if let Some(key) = key {
                lock(&in_flight).remove(&key);
            }
            if let Some(reply) = reply {
                let _ = outgoing.send(reply.to_string());
            }
        });
    };

    while tasks.join_next().await.is_some() {}
    connection.close().await;
    drop(outgoing);
    let written = writer_task.await.unwrap_or(Ok(()));
    outcome.and(written)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Serves the process's standard input and output.
pub async fn serve_stdio(server: &McpServer) -> io::Result<()> {
    serve(server, tokio::io::stdin(), tokio::io::stdout()).await
}
