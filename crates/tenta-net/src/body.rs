//! Request and response bodies.
//!
//! Response bodies are `Bytes` chunks end to end (`docs/spec/02-architecture.md`
//! §7): the transport decodes `Content-Encoding` in place on the io thread and
//! hands the parser the same buffers the socket produced.

use std::io::Write;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use tokio::sync::mpsc;

use crate::error::{ErrorName, NetError};

/// A request body.
#[derive(Debug, Default)]
pub enum Body {
    /// No body at all (not even `Content-Length: 0`).
    #[default]
    Empty,
    /// A complete in-memory body.
    Bytes(Bytes),
    /// A body streamed from the caller; `None` closes it.
    Stream {
        chunks: mpsc::Receiver<Result<Bytes, NetError>>,
        /// Declared length, when the caller knows it.
        length: Option<u64>,
    },
}

impl Body {
    pub fn empty() -> Self {
        Body::Empty
    }

    /// A channel-backed streaming body and its sender.
    pub fn channel(length: Option<u64>) -> (mpsc::Sender<Result<Bytes, NetError>>, Self) {
        let (tx, chunks) = mpsc::channel(8);
        (tx, Body::Stream { chunks, length })
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Body::Empty => true,
            Body::Bytes(bytes) => bytes.is_empty(),
            Body::Stream { .. } => false,
        }
    }

    /// The body length when it is known before sending.
    pub fn length(&self) -> Option<u64> {
        match self {
            Body::Empty => Some(0),
            Body::Bytes(bytes) => Some(bytes.len() as u64),
            Body::Stream { length, .. } => *length,
        }
    }

    /// Splits off the in-memory bytes, if any, leaving the body empty.
    pub fn take_bytes(&mut self) -> Option<Bytes> {
        match std::mem::replace(self, Body::Empty) {
            Body::Bytes(bytes) => Some(bytes),
            other => {
                *self = other;
                None
            }
        }
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Self {
        Body::Bytes(bytes)
    }
}

impl From<Vec<u8>> for Body {
    fn from(bytes: Vec<u8>) -> Self {
        Body::Bytes(Bytes::from(bytes))
    }
}

impl From<&'static str> for Body {
    fn from(text: &'static str) -> Self {
        Body::Bytes(Bytes::from_static(text.as_bytes()))
    }
}

impl From<String> for Body {
    fn from(text: String) -> Self {
        Body::Bytes(Bytes::from(text))
    }
}

/// The request body as hyper sees it.
///
/// Only the HTTP/1.1 path uses this; the HTTP/2 path writes into `tenta-h2`'s
/// `SendStream` directly.
#[derive(Debug)]
pub struct OutgoingBody {
    inner: Body,
    finished: bool,
}

impl OutgoingBody {
    pub fn new(inner: Body) -> Self {
        Self {
            inner,
            finished: false,
        }
    }
}

impl HttpBody for OutgoingBody {
    type Data = Bytes;
    type Error = NetError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, NetError>>> {
        if self.finished {
            return Poll::Ready(None);
        }
        match &mut self.inner {
            Body::Empty => {
                self.finished = true;
                Poll::Ready(None)
            }
            Body::Bytes(bytes) => {
                let chunk = std::mem::take(bytes);
                self.finished = true;
                if chunk.is_empty() {
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(Ok(Frame::data(chunk))))
                }
            }
            Body::Stream { chunks, .. } => match chunks.poll_recv(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => {
                    self.finished = true;
                    Poll::Ready(None)
                }
                Poll::Ready(Some(Ok(chunk))) => Poll::Ready(Some(Ok(Frame::data(chunk)))),
                Poll::Ready(Some(Err(err))) => {
                    self.finished = true;
                    Poll::Ready(Some(Err(err)))
                }
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        self.finished || self.inner.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        match self.inner.length() {
            Some(length) => SizeHint::with_exact(length),
            None => SizeHint::default(),
        }
    }
}

/// A response body: decoded `Bytes` chunks in arrival order.
///
/// The stream is filled by the io thread and consumed anywhere, so a caller
/// on another thread can read the body while the connection stays on its
/// runtime.
#[derive(Debug)]
pub struct BodyStream {
    chunks: mpsc::Receiver<Result<Bytes, NetError>>,
}

impl BodyStream {
    pub(crate) fn new(chunks: mpsc::Receiver<Result<Bytes, NetError>>) -> Self {
        Self { chunks }
    }

    /// An already-complete body, used for cached and intercepted responses.
    pub fn once(bytes: Bytes) -> Self {
        let (tx, rx) = mpsc::channel(1);
        if !bytes.is_empty() {
            let _ = tx.try_send(Ok(bytes));
        }
        Self { chunks: rx }
    }

    /// An empty body.
    pub fn empty() -> Self {
        Self::once(Bytes::new())
    }

    /// The next chunk, or `None` at the end of the body.
    pub async fn next_chunk(&mut self) -> Option<Result<Bytes, NetError>> {
        self.chunks.recv().await
    }

    /// Reads the body to the end.
    pub async fn collect(mut self) -> Result<Bytes, NetError> {
        let mut chunks: Vec<Bytes> = Vec::new();
        let mut total = 0;
        while let Some(chunk) = self.next_chunk().await {
            let chunk = chunk?;
            total += chunk.len();
            chunks.push(chunk);
        }
        match chunks.len() {
            0 => Ok(Bytes::new()),
            1 => Ok(chunks.pop().expect("one chunk")),
            _ => {
                let mut out = Vec::with_capacity(total);
                for chunk in chunks {
                    out.extend_from_slice(&chunk);
                }
                Ok(Bytes::from(out))
            }
        }
    }

    /// Reads the body to the end as UTF-8, replacing invalid sequences.
    pub async fn text(self) -> Result<String, NetError> {
        Ok(String::from_utf8_lossy(&self.collect().await?).into_owned())
    }
}

/// Content codings the transport decodes transparently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentEncoding {
    Identity,
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

impl ContentEncoding {
    /// Parses a `Content-Encoding` value. Multiple codings are not decoded:
    /// they are rare and the raw bytes stay available to the caller.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.contains(',') {
            return None;
        }
        match value.to_ascii_lowercase().as_str() {
            "identity" | "" => Some(ContentEncoding::Identity),
            "gzip" | "x-gzip" => Some(ContentEncoding::Gzip),
            "deflate" => Some(ContentEncoding::Deflate),
            "br" => Some(ContentEncoding::Brotli),
            "zstd" => Some(ContentEncoding::Zstd),
            _ => None,
        }
    }
}

/// The decoded output of one body, refusing to grow past the request's limit.
///
/// The budget is enforced *inside* the decoder, on the sink every backend
/// writes to, so a 1 KB body that expands to a gigabyte stops after the first
/// `limit` bytes instead of after the whole chunk has been materialised
/// (`docs/spec/03-network.md` §8).
#[derive(Debug, Default)]
pub struct BoundedSink {
    buffer: Vec<u8>,
    written: u64,
    limit: Option<u64>,
}

impl BoundedSink {
    fn new(limit: Option<u64>) -> Self {
        Self {
            buffer: Vec::new(),
            written: 0,
            limit,
        }
    }

    /// Bytes the decoder has produced so far, over the whole body.
    pub fn written(&self) -> u64 {
        self.written
    }
}

impl Write for BoundedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(limit) = self.limit {
            let remaining = limit.saturating_sub(self.written);
            if buf.len() as u64 > remaining {
                let fits = usize::try_from(remaining).unwrap_or(usize::MAX);
                self.buffer.extend_from_slice(&buf[..fits]);
                self.written += fits as u64;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::FileTooLarge,
                    format!("decoded body is larger than the {limit}-byte limit"),
                ));
            }
        }
        self.buffer.extend_from_slice(buf);
        self.written += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Streaming decompressor for one response body.
pub struct Decoder {
    inner: DecoderInner,
}

enum DecoderInner {
    Identity { written: u64, limit: Option<u64> },
    Gzip(Box<flate2::write::GzDecoder<BoundedSink>>),
    Deflate(Box<flate2::write::ZlibDecoder<BoundedSink>>),
    Brotli(Box<brotli::DecompressorWriter<BoundedSink>>),
    Zstd(Box<zstd::stream::write::Decoder<'static, BoundedSink>>),
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.inner {
            DecoderInner::Identity { .. } => "identity",
            DecoderInner::Gzip(_) => "gzip",
            DecoderInner::Deflate(_) => "deflate",
            DecoderInner::Brotli(_) => "br",
            DecoderInner::Zstd(_) => "zstd",
        };
        write!(f, "Decoder({name})")
    }
}

impl Decoder {
    /// A decoder for `encoding` that never produces more than `limit` bytes.
    pub fn new(encoding: ContentEncoding, limit: Option<u64>) -> Result<Self, NetError> {
        let sink = || BoundedSink::new(limit);
        let inner = match encoding {
            ContentEncoding::Identity => DecoderInner::Identity { written: 0, limit },
            ContentEncoding::Gzip => {
                DecoderInner::Gzip(Box::new(flate2::write::GzDecoder::new(sink())))
            }
            ContentEncoding::Deflate => {
                DecoderInner::Deflate(Box::new(flate2::write::ZlibDecoder::new(sink())))
            }
            ContentEncoding::Brotli => {
                DecoderInner::Brotli(Box::new(brotli::DecompressorWriter::new(sink(), 8192)))
            }
            ContentEncoding::Zstd => DecoderInner::Zstd(Box::new(
                zstd::stream::write::Decoder::new(sink()).map_err(|err| {
                    NetError::new(ErrorName::InvalidResponse, format!("zstd decoder: {err}"))
                })?,
            )),
        };
        Ok(Self { inner })
    }

    pub fn is_identity(&self) -> bool {
        matches!(self.inner, DecoderInner::Identity { .. })
    }

    /// Total bytes decoded so far across all chunks.
    pub fn written(&self) -> u64 {
        match &self.inner {
            DecoderInner::Identity { written, .. } => *written,
            DecoderInner::Gzip(decoder) => decoder.get_ref().written(),
            DecoderInner::Deflate(decoder) => decoder.get_ref().written(),
            DecoderInner::Brotli(decoder) => decoder.get_ref().written(),
            DecoderInner::Zstd(decoder) => decoder.get_ref().written(),
        }
    }

    /// Feeds one compressed chunk and returns whatever decoded out of it.
    pub fn decode(&mut self, chunk: Bytes) -> Result<Bytes, NetError> {
        match &mut self.inner {
            DecoderInner::Identity { written, limit } => {
                *written += chunk.len() as u64;
                if let Some(limit) = limit
                    && *written > *limit
                {
                    return Err(NetError::new(
                        ErrorName::FileTooBig,
                        format!("response body is larger than the {limit}-byte limit"),
                    ));
                }
                Ok(chunk)
            }
            DecoderInner::Gzip(decoder) => {
                decoder.write_all(&chunk).map_err(decode_error("gzip"))?;
                Ok(take_output(decoder.get_mut()))
            }
            DecoderInner::Deflate(decoder) => {
                decoder.write_all(&chunk).map_err(decode_error("deflate"))?;
                Ok(take_output(decoder.get_mut()))
            }
            DecoderInner::Brotli(decoder) => {
                decoder.write_all(&chunk).map_err(decode_error("br"))?;
                decoder.flush().map_err(decode_error("br"))?;
                Ok(take_output(decoder.get_mut()))
            }
            DecoderInner::Zstd(decoder) => {
                decoder.write_all(&chunk).map_err(decode_error("zstd"))?;
                decoder.flush().map_err(decode_error("zstd"))?;
                Ok(take_output(decoder.get_mut()))
            }
        }
    }

    /// Flushes the decoder at the end of the body.
    pub fn finish(self) -> Result<Bytes, NetError> {
        match self.inner {
            DecoderInner::Identity { .. } => Ok(Bytes::new()),
            DecoderInner::Gzip(decoder) => {
                let mut sink = decoder.finish().map_err(decode_error("gzip"))?;
                Ok(take_output(&mut sink))
            }
            DecoderInner::Deflate(decoder) => {
                let mut sink = decoder.finish().map_err(decode_error("deflate"))?;
                Ok(take_output(&mut sink))
            }
            DecoderInner::Brotli(mut decoder) => {
                decoder.flush().map_err(decode_error("br"))?;
                Ok(take_output(decoder.get_mut()))
            }
            DecoderInner::Zstd(mut decoder) => {
                decoder.flush().map_err(decode_error("zstd"))?;
                Ok(take_output(decoder.get_mut()))
            }
        }
    }
}

fn take_output(sink: &mut BoundedSink) -> Bytes {
    if sink.buffer.is_empty() {
        Bytes::new()
    } else {
        Bytes::from(std::mem::take(&mut sink.buffer))
    }
}

fn decode_error(coding: &'static str) -> impl Fn(std::io::Error) -> NetError {
    move |err| {
        // The sink stops the decoder when the request's budget is spent; that
        // is a size failure, not a malformed body.
        if err.kind() == std::io::ErrorKind::FileTooLarge {
            return NetError::new(ErrorName::FileTooBig, format!("{coding}: {err}"));
        }
        // Decode failures use Chromium's ContentDecodingFailed name.
        NetError::new(
            ErrorName::ContentDecodingFailed,
            format!("{coding} decoding failed: {err}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).expect("gzip");
        encoder.finish().expect("gzip")
    }

    fn brotli(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
            writer.write_all(data).expect("br");
        }
        out
    }

    fn zstd(data: &[u8]) -> Vec<u8> {
        zstd::stream::encode_all(data, 3).expect("zstd")
    }

    fn decode_all(encoding: ContentEncoding, compressed: &[u8], chunk_size: usize) -> Vec<u8> {
        let mut decoder = Decoder::new(encoding, None).expect("decoder");
        let mut out = Vec::new();
        for chunk in compressed.chunks(chunk_size) {
            out.extend_from_slice(
                &decoder
                    .decode(Bytes::copy_from_slice(chunk))
                    .expect("decode"),
            );
        }
        out.extend_from_slice(&decoder.finish().expect("finish"));
        out
    }

    #[test]
    fn every_content_encoding_round_trips_chunk_by_chunk() {
        let payload: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
        for (encoding, compressed) in [
            (ContentEncoding::Gzip, gzip(&payload)),
            (ContentEncoding::Brotli, brotli(&payload)),
            (ContentEncoding::Zstd, zstd(&payload)),
        ] {
            for chunk_size in [1024, 7, compressed.len().max(1)] {
                assert_eq!(
                    decode_all(encoding, &compressed, chunk_size),
                    payload,
                    "{encoding:?} in {chunk_size}-byte chunks"
                );
            }
        }
    }

    #[test]
    fn content_encoding_names_are_parsed_case_insensitively() {
        assert_eq!(ContentEncoding::parse("GZIP"), Some(ContentEncoding::Gzip));
        assert_eq!(
            ContentEncoding::parse(" br "),
            Some(ContentEncoding::Brotli)
        );
        assert_eq!(
            ContentEncoding::parse("identity"),
            Some(ContentEncoding::Identity)
        );
        assert_eq!(ContentEncoding::parse("gzip, br"), None);
        assert_eq!(ContentEncoding::parse("exotic"), None);
    }

    #[test]
    fn a_truncated_body_is_a_content_decoding_failed_not_a_panic() {
        let compressed = gzip(b"hello");
        let mut decoder = Decoder::new(ContentEncoding::Gzip, None).expect("decoder");
        let _ = decoder.decode(Bytes::copy_from_slice(&compressed[..compressed.len() / 2]));
        let err = decoder.finish().expect_err("truncated gzip must fail");
        assert_eq!(err.name, ErrorName::ContentDecodingFailed);
    }

    #[test]
    fn a_decompression_bomb_never_materialises_past_the_limit() {
        // 4 MiB of zeros in a few kilobytes. The decoder must stop inside the
        // chunk: what it hands back, plus what it kept, stays within the
        // budget instead of the expanded size.
        const LIMIT: u64 = 16 * 1024;
        let compressed = gzip(&vec![0u8; 4 * 1024 * 1024]);
        assert!(
            compressed.len() < 64 * 1024,
            "the fixture really is a bomb: {} bytes",
            compressed.len()
        );

        let mut decoder = Decoder::new(ContentEncoding::Gzip, Some(LIMIT)).expect("decoder");
        let mut produced = 0u64;
        let mut failed = false;
        for chunk in compressed.chunks(4096) {
            match decoder.decode(Bytes::copy_from_slice(chunk)) {
                Ok(decoded) => produced += decoded.len() as u64,
                Err(err) => {
                    assert_eq!(err.name, ErrorName::FileTooBig);
                    failed = true;
                    break;
                }
            }
        }
        assert!(failed, "the bomb was decoded in full");
        assert!(
            produced <= LIMIT,
            "the decoder produced {produced} bytes for a {LIMIT}-byte budget"
        );
    }

    #[test]
    fn an_undeclared_body_over_the_limit_fails_too() {
        // The identity path counts as well, so a plain 10 MB body cannot slip
        // past a 1 KB budget.
        let mut decoder = Decoder::new(ContentEncoding::Identity, Some(1024)).expect("decoder");
        assert!(decoder.decode(Bytes::from(vec![0u8; 512])).is_ok());
        let err = decoder
            .decode(Bytes::from(vec![0u8; 1024]))
            .expect_err("over the limit");
        assert_eq!(err.name, ErrorName::FileTooBig);
    }

    #[tokio::test]
    async fn body_streams_collect_in_order() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(Ok(Bytes::from_static(b"one ")))
            .await
            .expect("send");
        tx.send(Ok(Bytes::from_static(b"two"))).await.expect("send");
        drop(tx);
        let body = BodyStream::new(rx);
        assert_eq!(body.collect().await.expect("collect"), "one two");
        assert!(
            BodyStream::empty()
                .collect()
                .await
                .expect("empty")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_stream_error_reaches_the_reader() {
        let (tx, rx) = mpsc::channel(2);
        tx.send(Ok(Bytes::from_static(b"partial")))
            .await
            .expect("send");
        tx.send(Err(NetError::new(ErrorName::ConnectionReset, "reset")))
            .await
            .expect("send");
        drop(tx);
        let err = BodyStream::new(rx).collect().await.expect_err("must fail");
        assert_eq!(err.name, ErrorName::ConnectionReset);
    }
}
