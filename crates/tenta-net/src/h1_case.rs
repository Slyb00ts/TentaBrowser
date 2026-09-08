//! Writes the profile's own header casing and order on HTTP/1.1.
//!
//! Chrome sends `sec-ch-ua` lower-case and `User-Agent` title-cased in the
//! same request. hyper can do one or the other: its `title_case_headers`
//! capitalises after every `-`, and the per-header `HeaderCaseMap` extension
//! that would carry the exact spelling is `pub(crate)` in hyper 1.x, so it
//! cannot be set from here. It also writes the headers in `HeaderMap`
//! iteration order, which groups repeated names and is not promised to be the
//! order they went in. `docs/spec/03-network.md` §1 pins HTTP/1.1 to hyper, so
//! the casing and the order are both restored on the way out instead.
//!
//! The restoration is exact, not heuristic: before each request the connection
//! records the head hyper is *about to* write (its own title-cased spelling)
//! together with the head the profile asks for. The writer compares the
//! outgoing bytes with the recorded prediction byte for byte and substitutes
//! only on a complete match, so nothing else in the stream — least of all a
//! request body — can ever be rewritten. A prediction that does not match is
//! passed through untouched and logged; the conformance tests in
//! `tests/fingerprint.rs` compare the wire bytes with the Chrome capture, so
//! a hyper change that breaks the prediction fails there rather than silently
//! degrading the fingerprint.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use http::HeaderValue;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// One request head: what hyper will write, and what should go out instead.
#[derive(Debug)]
pub struct HeadRewrite {
    pub predicted: Vec<u8>,
    pub replacement: Vec<u8>,
}

/// The rewrites queued for one connection, shared with its writer.
pub type Rewrites = Rc<RefCell<VecDeque<HeadRewrite>>>;

pub fn rewrites() -> Rewrites {
    Rc::new(RefCell::new(VecDeque::new()))
}

/// The head hyper is about to write, and the head that should go out instead.
///
/// `predicted` mirrors `hyper::proto::h1::role::Client::encode`: the request
/// line, every header in `HeaderMap` iteration order with hyper's title
/// casing, then the blank line. It is derived from the very map handed to
/// hyper, so it cannot drift from what hyper iterates. The caller is
/// responsible for setting the framing header (`content-length` or
/// `transfer-encoding`) before this is built, so `Client::set_length` has
/// nothing left to insert.
///
/// When the two header lists are not the same set — `to_header_map` drops
/// names `http` rejects, so that is reachable — substituting one for the other
/// would change what the server sees, and the rewrite becomes an identity: the
/// head is recognised, so the queue stays in step with the connection, but the
/// bytes are hyper's own.
pub fn head_rewrite(
    method: &http::Method,
    target: &str,
    map: &http::HeaderMap,
    ordered: &[(String, HeaderValue)],
) -> HeadRewrite {
    let mut predicted = request_line(method, target);
    for (name, value) in map {
        title_case(&mut predicted, name.as_str().as_bytes());
        predicted.extend_from_slice(b": ");
        predicted.extend_from_slice(value.as_bytes());
        predicted.extend_from_slice(b"\r\n");
    }
    predicted.extend_from_slice(b"\r\n");

    if !same_headers(map, ordered) {
        tracing::debug!(
            "the http/1.1 header list does not match the request map; \
             sending hyper's own casing and order"
        );
        return HeadRewrite {
            replacement: predicted.clone(),
            predicted,
        };
    }

    let mut replacement = request_line(method, target);
    for (name, value) in ordered {
        replacement.extend_from_slice(name.as_bytes());
        replacement.extend_from_slice(b": ");
        replacement.extend_from_slice(value.as_bytes());
        replacement.extend_from_slice(b"\r\n");
    }
    replacement.extend_from_slice(b"\r\n");

    HeadRewrite {
        predicted,
        replacement,
    }
}

/// Whether the ordered list and the map carry the same name/value pairs.
fn same_headers(map: &http::HeaderMap, ordered: &[(String, HeaderValue)]) -> bool {
    if map.iter().count() != ordered.len() {
        return false;
    }
    let mut from_map: Vec<(String, &[u8])> = map
        .iter()
        .map(|(name, value)| (name.as_str().to_owned(), value.as_bytes()))
        .collect();
    let mut from_list: Vec<(String, &[u8])> = ordered
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.as_bytes()))
        .collect();
    from_map.sort_unstable();
    from_list.sort_unstable();
    from_map == from_list
}

fn request_line(method: &http::Method, target: &str) -> Vec<u8> {
    let mut line = Vec::with_capacity(256);
    line.extend_from_slice(method.as_str().as_bytes());
    line.push(b' ');
    line.extend_from_slice(target.as_bytes());
    line.extend_from_slice(b" HTTP/1.1\r\n");
    line
}

/// hyper's rule: capitalise the first character and every one after a `-`.
/// The names come from a `HeaderMap` and are already lower-case, so this does
/// not fold case the way a general-purpose title caser would.
fn title_case(dst: &mut Vec<u8>, name: &[u8]) {
    let mut previous = b'-';
    for &(mut byte) in name {
        if previous == b'-' {
            byte.make_ascii_uppercase();
        }
        dst.push(byte);
        previous = byte;
    }
}

enum State {
    /// Waiting for the next request head.
    Idle,
    /// Comparing outgoing bytes with a prediction.
    Matching {
        rewrite: HeadRewrite,
        matched: usize,
    },
    /// The prediction failed; everything goes through untouched until the
    /// next request is queued.
    PassThrough,
}

/// An `AsyncWrite` that substitutes the recorded request heads.
pub struct CaseRestoringStream<S> {
    inner: S,
    queue: Rewrites,
    state: State,
    /// Bytes accepted from the caller and not yet written to `inner`.
    out: Vec<u8>,
    written: usize,
}

impl<S> CaseRestoringStream<S> {
    pub fn new(inner: S, queue: Rewrites) -> Self {
        Self {
            inner,
            queue,
            state: State::Idle,
            out: Vec::new(),
            written: 0,
        }
    }

    /// Whether the next bytes can go straight to the socket.
    fn is_idle(&self) -> bool {
        match self.state {
            State::Idle => self.queue.borrow().is_empty(),
            State::PassThrough | State::Matching { .. } => false,
        }
    }

    /// Feeds `buf` through the rewriter into `out`.
    fn absorb(&mut self, buf: &[u8]) {
        let mut rest = buf;
        while !rest.is_empty() {
            match &mut self.state {
                State::PassThrough | State::Idle => {
                    // If a new rewrite arrived while in PassThrough, reset to Idle
                    // so the next request can be matched.
                    if matches!(self.state, State::PassThrough) {
                        if self.queue.borrow().is_empty() {
                            self.out.extend_from_slice(rest);
                            return;
                        } else {
                            self.state = State::Idle;
                            continue;
                        }
                    }
                    if let Some(rewrite) = self.queue.borrow_mut().pop_front() {
                        self.state = State::Matching {
                            rewrite,
                            matched: 0,
                        };
                        continue;
                    }
                    self.out.extend_from_slice(rest);
                    return;
                }
                State::Matching { rewrite, matched } => {
                    let wanted = &rewrite.predicted[*matched..];
                    let take = wanted.len().min(rest.len());
                    if wanted[..take] != rest[..take] {
                        // Not the head we predicted: emit what was held back
                        // and stop touching this connection's stream.
                        tracing::debug!(
                            "the predicted HTTP/1.1 request head did not match; \
                             sending hyper's own casing"
                        );
                        let held = rewrite.predicted[..*matched].to_vec();
                        self.out.extend_from_slice(&held);
                        self.out.extend_from_slice(rest);
                        self.state = State::PassThrough;
                        self.queue.borrow_mut().clear();
                        return;
                    }
                    *matched += take;
                    rest = &rest[take..];
                    if *matched == rewrite.predicted.len() {
                        self.out.extend_from_slice(&rewrite.replacement);
                        self.state = State::Idle;
                    }
                }
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> CaseRestoringStream<S> {
    /// Writes as much of `out` as the socket takes.
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.written < self.out.len() {
            let n = std::task::ready!(
                Pin::new(&mut self.inner).poll_write(cx, &self.out[self.written..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "the connection accepted no bytes",
                )));
            }
            self.written += n;
        }
        self.out.clear();
        self.written = 0;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CaseRestoringStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CaseRestoringStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Everything already accepted has to reach the socket before more is
        // taken, otherwise a caller could queue an unbounded amount.
        std::task::ready!(self.as_mut().poll_drain(cx))?;
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if self.is_idle() {
            // Nothing to substitute: response bodies, request bodies and every
            // connection without a browser profile go straight to the socket.
            return Pin::new(&mut self.inner).poll_write(cx, buf);
        }
        self.absorb(buf);
        // The bytes are ours now; failure to drain surfaces on the next call.
        let _ = self.as_mut().poll_drain(cx)?;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        std::task::ready!(self.as_mut().poll_drain(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        std::task::ready!(self.as_mut().poll_drain(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn header(name: &str, value: &str) -> (String, HeaderValue) {
        (
            name.to_owned(),
            HeaderValue::from_str(value).expect("value"),
        )
    }

    fn map_of(headers: &[(String, HeaderValue)]) -> http::HeaderMap {
        let mut map = http::HeaderMap::new();
        for (name, value) in headers {
            let name = http::header::HeaderName::from_bytes(name.as_bytes()).expect("name");
            map.append(name, value.clone());
        }
        map
    }

    #[test]
    fn the_prediction_uses_hypers_title_case() {
        let headers = [header("sec-ch-ua-mobile", "?0")];
        let rewrite = head_rewrite(&http::Method::GET, "/", &map_of(&headers), &headers);
        assert_eq!(
            String::from_utf8(rewrite.predicted).expect("utf-8"),
            "GET / HTTP/1.1\r\nSec-Ch-Ua-Mobile: ?0\r\n\r\n"
        );
        assert_eq!(
            String::from_utf8(rewrite.replacement).expect("utf-8"),
            "GET / HTTP/1.1\r\nsec-ch-ua-mobile: ?0\r\n\r\n",
            "the replacement keeps the profile's casing"
        );
    }

    #[test]
    fn the_replacement_keeps_the_profile_order_when_the_map_reorders() {
        // The profile order comes from the ordered list, never from the map,
        // which groups repeated names and promises no order of its own.
        let headers: Vec<_> = [
            ("Host", "example.com"),
            ("Connection", "keep-alive"),
            ("sec-ch-ua", "\"Chromium\";v=\"144\""),
            ("sec-ch-ua-mobile", "?0"),
            ("sec-ch-ua-platform", "\"macOS\""),
            ("Upgrade-Insecure-Requests", "1"),
            ("User-Agent", "test/1.0"),
            ("Accept", "text/html"),
            ("Accept-Encoding", "gzip, deflate, br, zstd"),
            ("Accept-Language", "en-US,en;q=0.9"),
        ]
        .into_iter()
        .map(|(name, value)| header(name, value))
        .collect();
        let rewrite = head_rewrite(&http::Method::GET, "/", &map_of(&headers), &headers);

        let replacement = String::from_utf8(rewrite.replacement).expect("utf-8");
        let names: Vec<&str> = replacement
            .lines()
            .skip(1)
            .filter(|line| !line.is_empty())
            .map(|line| line.split(':').next().expect("name"))
            .collect();
        let wanted: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, wanted);
    }

    #[tokio::test]
    async fn a_matching_head_is_replaced_and_the_body_is_untouched() {
        let queue = rewrites();
        let headers = [
            header("Host", "example.com"),
            header("sec-ch-ua", "\"Chromium\";v=\"144\""),
        ];
        let rewrite = head_rewrite(&http::Method::POST, "/x", &map_of(&headers), &headers);
        let predicted = rewrite.predicted.clone();
        queue.borrow_mut().push_back(rewrite);

        let mut stream = CaseRestoringStream::new(Vec::new(), queue);
        // hyper writes the head first, then the body, in separate calls.
        stream.write_all(&predicted).await.expect("head");
        // A body that happens to look like a request head must survive.
        stream.write_all(&predicted).await.expect("body");
        stream.flush().await.expect("flush");

        let written = String::from_utf8(stream.inner.clone()).expect("utf-8");
        assert!(
            written.starts_with("POST /x HTTP/1.1\r\nHost: example.com\r\nsec-ch-ua: "),
            "the head kept the profile's casing: {written:?}"
        );
        assert_eq!(
            written.matches("Sec-Ch-Ua: ").count(),
            1,
            "only the head was rewritten, the body was not: {written:?}"
        );
    }

    #[tokio::test]
    async fn an_unexpected_head_is_passed_through_unchanged() {
        let queue = rewrites();
        let headers = [header("Host", "example.com")];
        let mut rewrite =
            head_rewrite(&http::Method::GET, "/expected", &map_of(&headers), &headers);
        rewrite.replacement = b"REWRITTEN".to_vec();
        queue.borrow_mut().push_back(rewrite);

        let mut stream = CaseRestoringStream::new(Vec::new(), queue);
        let actual =
            head_rewrite(&http::Method::GET, "/other", &map_of(&headers), &headers).predicted;
        stream.write_all(&actual).await.expect("head");
        stream.flush().await.expect("flush");

        assert_eq!(stream.inner, actual, "nothing was substituted");
    }

    #[tokio::test]
    async fn a_head_split_across_writes_is_still_matched() {
        let queue = rewrites();
        let headers = [header("sec-ch-ua-mobile", "?0")];
        let rewrite = head_rewrite(&http::Method::GET, "/", &map_of(&headers), &headers);
        let predicted = rewrite.predicted.clone();
        queue.borrow_mut().push_back(rewrite);

        let mut stream = CaseRestoringStream::new(Vec::new(), queue);
        for chunk in predicted.chunks(7) {
            stream.write_all(chunk).await.expect("chunk");
        }
        stream.flush().await.expect("flush");
        let written = String::from_utf8(stream.inner.clone()).expect("utf-8");
        assert!(
            written.contains("sec-ch-ua-mobile: ?0"),
            "the profile casing survived a fragmented write: {written:?}"
        );
    }
}
