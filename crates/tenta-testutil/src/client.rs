//! A deliberately tiny HTTP/1.1 client for tests: one request per connection,
//! `Connection: close`, and timestamps for when the head and the body arrived
//! so latency simulation can be asserted without a real HTTP client crate.

use std::io;
use std::net::SocketAddr;
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// When the request was written.
    pub sent_at: Instant,
    /// When the end of the response head (`\r\n\r\n`) had been received.
    pub head_at: Instant,
    /// When the connection closed after the body.
    pub body_at: Instant,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A response whose head has been parsed while the body is still streaming.
#[derive(Debug)]
pub struct StreamingResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub sent_at: Instant,
    pub head_at: Instant,
    /// Body bytes that arrived together with the head.
    pub buffered: Vec<u8>,
    pub stream: TcpStream,
}

impl StreamingResponse {
    /// Reads until at least `min_len` body bytes are buffered or the connection closes.
    pub async fn read_body_until(&mut self, min_len: usize) -> io::Result<&[u8]> {
        let mut chunk = [0u8; 4096];
        while self.buffered.len() < min_len {
            let n = self.stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            self.buffered.extend_from_slice(&chunk[..n]);
        }
        Ok(&self.buffered)
    }

    /// Reads until the connection closes.
    pub async fn finish(mut self) -> io::Result<Response> {
        let mut rest = Vec::new();
        self.stream.read_to_end(&mut rest).await?;
        self.buffered.extend_from_slice(&rest);
        let body_at = Instant::now();
        Ok(Response {
            status: self.status,
            headers: self.headers,
            body: self.buffered,
            sent_at: self.sent_at,
            head_at: self.head_at,
            body_at,
        })
    }
}

pub async fn get(addr: SocketAddr, path: &str) -> io::Result<Response> {
    request(addr, "GET", path, &[], &[]).await?.finish().await
}

pub async fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> io::Result<StreamingResponse> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() || method == "POST" || method == "PUT" {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let sent_at = Instant::now();
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;

    let mut buffered = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = find_head_end(&buffered) {
            break pos;
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before response head",
            ));
        }
        buffered.extend_from_slice(&chunk[..n]);
    };
    let head_at = Instant::now();
    let head_text = String::from_utf8_lossy(&buffered[..head_end]).into_owned();
    let body_start = head_end + 4;
    let rest = buffered.split_off(body_start);
    let mut lines = head_text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad status line"))?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Ok(StreamingResponse {
        status,
        headers,
        sent_at,
        head_at,
        buffered: rest,
        stream,
    })
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
