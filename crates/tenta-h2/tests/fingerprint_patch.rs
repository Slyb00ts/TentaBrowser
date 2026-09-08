//! Wire-image tests for the TentaBrowser fingerprint patch (not upstream).
//!
//! They read the raw bytes a client writes to its socket and assert the frame
//! sequence, the SETTINGS payload, the connection WINDOW_UPDATE, the HEADERS
//! priority fields and the pseudo-header order byte for byte.

use bytes::Bytes;
use http::Request;
use tenta_h2::client;
use tenta_h2::fingerprint::{H2Fingerprint, HeadersPriority, PseudoId};
use tokio::io::{AsyncReadExt, DuplexStream};

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

#[derive(Debug, PartialEq, Eq)]
struct RawFrame {
    kind: u8,
    flags: u8,
    stream_id: u32,
    payload: Vec<u8>,
}

async fn read_exact(io: &mut DuplexStream, len: usize) -> Vec<u8> {
    let mut buf = vec![0; len];
    io.read_exact(&mut buf).await.expect("read");
    buf
}

async fn read_frame(io: &mut DuplexStream) -> RawFrame {
    let head = read_exact(io, 9).await;
    let len = u32::from_be_bytes([0, head[0], head[1], head[2]]) as usize;
    let stream_id = u32::from_be_bytes([head[5], head[6], head[7], head[8]]) & !(1 << 31);
    RawFrame {
        kind: head[3],
        flags: head[4],
        stream_id,
        payload: read_exact(io, len).await,
    }
}

/// Runs one GET through a client built by `configure` and returns the bytes it
/// wrote, as (preface, frames).
async fn client_wire_image(
    configure: impl FnOnce(&mut client::Builder) -> &mut client::Builder,
) -> Vec<RawFrame> {
    let (client_io, mut server_io) = tokio::io::duplex(64 * 1024);
    let mut builder = client::Builder::new();
    configure(&mut builder);

    let handshake = builder.handshake::<_, Bytes>(client_io);
    let driver = tokio::spawn(async move {
        let (mut send_request, connection) = handshake.await.expect("handshake");
        let conn = tokio::spawn(async move {
            let _ = connection.await;
        });
        let request = Request::get("https://example.com/")
            .body(())
            .expect("request");
        let (response, _) = send_request
            .send_request(request, true)
            .expect("send_request");
        let _ = response.await;
        let _ = conn.await;
    });

    assert_eq!(read_exact(&mut server_io, PREFACE.len()).await, PREFACE);
    let mut frames = Vec::new();
    while frames.len() < 3 {
        frames.push(read_frame(&mut server_io).await);
        if frames.last().map(|f| f.kind) == Some(0x1) {
            break;
        }
    }
    driver.abort();
    frames
}

#[tokio::test]
async fn chrome_fingerprint_writes_chromes_frame_sequence() {
    let frames = client_wire_image(|b| b.fingerprint(H2Fingerprint::chrome())).await;

    // SETTINGS: identifiers in the profile's order, values verbatim.
    assert_eq!(frames[0].kind, 0x4);
    assert_eq!(frames[0].stream_id, 0);
    assert_eq!(
        frames[0].payload,
        vec![
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, // 1 = 65536
            0x00, 0x02, 0x00, 0x00, 0x00, 0x00, // 2 = 0
            0x00, 0x04, 0x00, 0x60, 0x00, 0x00, // 4 = 6291456
            0x00, 0x06, 0x00, 0x04, 0x00, 0x00, // 6 = 262144
        ]
    );

    // Connection-level WINDOW_UPDATE right after the preface.
    assert_eq!(frames[1].kind, 0x8);
    assert_eq!(frames[1].stream_id, 0);
    assert_eq!(frames[1].payload, 15_663_105u32.to_be_bytes().to_vec());

    // HEADERS with the PRIORITY flag, exclusive dependency on stream 0,
    // wire weight 255, then `:method :authority :scheme :path`.
    let headers = &frames[2];
    assert_eq!(headers.kind, 0x1);
    assert_eq!(headers.stream_id, 1);
    assert_eq!(headers.flags & 0x20, 0x20, "PRIORITY flag");
    assert_eq!(&headers.payload[..5], &[0x80, 0x00, 0x00, 0x00, 0xff]);

    let block = &headers.payload[5..];
    assert_eq!(block[0], 0x82, ":method GET (static index 2)");
    assert_eq!(
        block[1], 0x41,
        ":authority, literal with incremental indexing"
    );
    // Length octet: the high bit is the Huffman flag, the rest is the length.
    let after_authority = 3 + (block[2] & 0x7f) as usize;
    assert_eq!(block[after_authority], 0x87, ":scheme https (index 7)");
    assert_eq!(block[after_authority + 1], 0x84, ":path / (index 4)");
}

#[tokio::test]
async fn default_builder_keeps_the_upstream_wire_image() {
    let frames = client_wire_image(|b| b).await;

    assert_eq!(frames[0].kind, 0x4);
    assert!(
        frames[0].payload.is_empty(),
        "stock h2 sends an empty SETTINGS frame"
    );

    let headers = &frames[1];
    assert_eq!(headers.kind, 0x1);
    assert_eq!(headers.flags & 0x20, 0, "no PRIORITY flag upstream");
    assert_eq!(headers.payload[0], 0x82, ":method GET");
    assert_eq!(headers.payload[1], 0x87, ":scheme https before :authority");
}

#[tokio::test]
async fn pseudo_order_and_priority_are_configurable() {
    let fingerprint = H2Fingerprint::upstream()
        .with_pseudo_order([
            PseudoId::Method,
            PseudoId::Path,
            PseudoId::Authority,
            PseudoId::Scheme,
        ])
        .with_headers_priority(Some(HeadersPriority {
            exclusive: false,
            dependency: 3,
            weight: 41,
        }))
        .with_window_update(Some(1024));
    let frames = client_wire_image(|b| b.fingerprint(fingerprint)).await;

    assert_eq!(frames[1].kind, 0x8);
    assert_eq!(frames[1].payload, 1024u32.to_be_bytes().to_vec());

    let headers = &frames[2];
    assert_eq!(headers.flags & 0x20, 0x20);
    assert_eq!(&headers.payload[..5], &[0x00, 0x00, 0x00, 0x03, 41]);
    assert_eq!(headers.payload[5], 0x82, ":method GET");
    assert_eq!(headers.payload[6], 0x84, ":path / directly after :method");
}

/// The representation prefix of every field in an HPACK block, in order.
///
/// Names and values are skipped by their length prefix, so no Huffman decoder
/// is needed: only the representation byte matters here.
fn field_prefixes(block: &[u8]) -> Vec<u8> {
    fn skip_string(block: &[u8], pos: &mut usize) {
        let len = (block[*pos] & 0x7f) as usize;
        assert!(len < 127, "multi-octet string lengths are not used here");
        *pos += 1 + len;
    }
    fn skip_int(block: &[u8], pos: &mut usize, prefix_bits: u8) {
        let mask = (1u16 << prefix_bits) - 1;
        if u16::from(block[*pos]) & mask == mask {
            loop {
                *pos += 1;
                if block[*pos] & 0x80 == 0 {
                    break;
                }
            }
        }
        *pos += 1;
    }

    let mut prefixes = Vec::new();
    let mut pos = 0;
    while pos < block.len() {
        let byte = block[pos];
        let (prefix_bits, has_name) = if byte & 0x80 != 0 {
            (7, false)
        } else if byte & 0xc0 == 0x40 {
            (6, true)
        } else if byte & 0xe0 == 0x20 {
            (5, false)
        } else {
            (4, true)
        };
        let index_is_zero = byte & ((1 << prefix_bits) - 1) == 0;
        prefixes.push(byte & !((1u8 << prefix_bits) - 1));
        skip_int(block, &mut pos, prefix_bits);
        if has_name {
            if index_is_zero {
                skip_string(block, &mut pos);
            }
            skip_string(block, &mut pos);
        }
    }
    prefixes
}

async fn custom_header_prefix(fingerprint: H2Fingerprint) -> u8 {
    let (client_io, mut server_io) = tokio::io::duplex(64 * 1024);
    let mut builder = client::Builder::new();
    builder.fingerprint(fingerprint);
    let handshake = builder.handshake::<_, Bytes>(client_io);
    let driver = tokio::spawn(async move {
        let (mut send_request, connection) = handshake.await.expect("handshake");
        let conn = tokio::spawn(async move {
            let _ = connection.await;
        });
        let request = Request::get("https://example.com/")
            .header("x-secret", "s3cret")
            .body(())
            .expect("request");
        let (response, _) = send_request
            .send_request(request, true)
            .expect("send_request");
        let _ = response.await;
        let _ = conn.await;
    });

    assert_eq!(read_exact(&mut server_io, PREFACE.len()).await, PREFACE);
    let _settings = read_frame(&mut server_io).await;
    let headers = read_frame(&mut server_io).await;
    driver.abort();

    // `:method :scheme :authority :path` then the one request header.
    let prefixes = field_prefixes(&headers.payload);
    assert_eq!(prefixes.len(), 5, "prefixes: {prefixes:02x?}");
    prefixes[4]
}

#[tokio::test]
async fn never_indexed_names_are_literals() {
    // 0x40: literal with incremental indexing. 0x10: never indexed.
    assert_eq!(custom_header_prefix(H2Fingerprint::upstream()).await, 0x40);
    assert_eq!(
        custom_header_prefix(
            H2Fingerprint::upstream()
                .with_never_index(vec![http::header::HeaderName::from_static("x-secret")])
        )
        .await,
        0x10
    );
}
