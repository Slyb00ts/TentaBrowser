//! The HTTP/2 client connection preface and the Akamai HTTP/2 fingerprint.
//!
//! Akamai's "Passive Fingerprinting of HTTP/2 Clients" keys on what a client
//! says before it says anything about the request: the SETTINGS values and their
//! order, the connection-level WINDOW_UPDATE, any PRIORITY frames, and the order
//! of the pseudo-headers in the first HEADERS frame. Chrome sends `m,a,s,p` and
//! no PRIORITY frames, which is what `tenta-h2` reproduces
//! (`docs/spec/03-network.md` §1).

use super::hpack::HpackDecoder;
use super::{ParseError, Reader, Result};

/// The 24-byte client connection preface of RFC 9113 §3.4.
pub const CLIENT_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

const FRAME_HEADERS: u8 = 0x1;
const FRAME_PRIORITY: u8 = 0x2;
const FRAME_SETTINGS: u8 = 0x4;
const FRAME_WINDOW_UPDATE: u8 = 0x8;
const FRAME_CONTINUATION: u8 = 0x9;

const FLAG_END_HEADERS: u8 = 0x4;
const FLAG_PADDED: u8 = 0x8;
const FLAG_PRIORITY: u8 = 0x20;
const FLAG_ACK: u8 = 0x1;

/// A standalone PRIORITY frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriorityFrame {
    /// The stream the frame prioritises.
    pub stream_id: u32,
    /// The exclusive bit of the dependency.
    pub exclusive: bool,
    /// The stream this one depends on.
    pub dependency: u32,
    /// The weight, 1..=256 (the wire carries weight minus one).
    pub weight: u16,
}

/// The priority information carried inside a HEADERS frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadersPriority {
    /// The exclusive bit of the dependency.
    pub exclusive: bool,
    /// The stream this one depends on.
    pub dependency: u32,
    /// The weight, 1..=256.
    pub weight: u16,
}

/// The first HEADERS frame of the connection, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstHeaders {
    /// The stream the request opened.
    pub stream_id: u32,
    /// Pseudo-header initials in wire order, e.g. `m,a,s,p` for Chrome.
    pub pseudo_header_order: String,
    /// The priority the HEADERS frame carried, if it had the PRIORITY flag.
    pub priority: Option<HeadersPriority>,
    /// Every header field in wire order, pseudo-headers included.
    pub headers: Vec<(String, String)>,
}

impl FirstHeaders {
    /// Regular header names in wire order, lower-cased, pseudo-headers dropped.
    ///
    /// This is what a profile's `[headers] order` records.
    #[must_use]
    pub fn header_order(&self) -> Vec<String> {
        self.headers
            .iter()
            .filter(|(name, _)| !name.starts_with(':'))
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect()
    }

    /// The value of a header, matched case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Everything a client says before its first request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H2Preface {
    /// SETTINGS parameters in wire order, from the first non-ACK SETTINGS frame.
    pub settings: Vec<(u16, u32)>,
    /// The increment of the first connection-level (stream 0) WINDOW_UPDATE.
    pub window_update: Option<u32>,
    /// Standalone PRIORITY frames in wire order.
    pub priority_frames: Vec<PriorityFrame>,
    /// The first HEADERS frame, if the capture reached one.
    pub first_headers: Option<FirstHeaders>,
}

/// Parses a captured client connection preface up to and including the first
/// HEADERS frame.
///
/// `bytes` may start with the `PRI * HTTP/2.0` magic or directly with a frame
/// header; anything after the first complete HEADERS frame is ignored, and a
/// capture that stops early simply yields the fields it reached.
///
/// # Errors
///
/// Returns [`ParseError`] when a frame header is truncated, when the magic is
/// present but wrong, or when the HEADERS block is not decodable HPACK.
pub fn parse_h2_preface(bytes: &[u8]) -> Result<H2Preface> {
    let mut r = Reader::new(bytes);
    if bytes.len() >= CLIENT_PREFACE.len() && bytes.starts_with(&CLIENT_PREFACE[..3]) {
        let magic = r.take(CLIENT_PREFACE.len(), "client preface")?;
        if magic != CLIENT_PREFACE {
            return Err(ParseError::Unexpected {
                what: "client preface",
                found: u32::from_be_bytes([magic[0], magic[1], magic[2], magic[3]]),
            });
        }
    }

    let mut preface = H2Preface {
        settings: Vec::new(),
        window_update: None,
        priority_frames: Vec::new(),
        first_headers: None,
    };
    let mut settings_seen = false;
    let mut decoder = HpackDecoder::default();

    while r.remaining() >= 9 {
        let length = r.u24("frame length")? as usize;
        let kind = r.u8("frame type")?;
        let flags = r.u8("frame flags")?;
        let stream_id = r.u32("frame stream id")? & 0x7fff_ffff;
        let payload = r.take(length, "frame payload")?;

        match kind {
            FRAME_SETTINGS if flags & FLAG_ACK == 0 && !settings_seen => {
                settings_seen = true;
                preface.settings = parse_settings(payload)?;
            }
            FRAME_WINDOW_UPDATE if stream_id == 0 && preface.window_update.is_none() => {
                let mut p = Reader::new(payload);
                preface.window_update = Some(p.u32("window update")? & 0x7fff_ffff);
            }
            FRAME_PRIORITY => {
                let (exclusive, dependency, weight) = parse_priority(payload)?;
                preface.priority_frames.push(PriorityFrame {
                    stream_id,
                    exclusive,
                    dependency,
                    weight,
                });
            }
            FRAME_HEADERS => {
                let (priority, mut block) = split_headers_payload(payload, flags)?;
                let mut continued = flags & FLAG_END_HEADERS == 0;
                let mut fragment = block.to_vec();
                while continued && r.remaining() >= 9 {
                    let length = r.u24("continuation length")? as usize;
                    let kind = r.u8("continuation type")?;
                    let flags = r.u8("continuation flags")?;
                    let _stream = r.u32("continuation stream id")?;
                    let payload = r.take(length, "continuation payload")?;
                    if kind != FRAME_CONTINUATION {
                        return Err(ParseError::Unexpected {
                            what: "frame type after HEADERS without END_HEADERS",
                            found: u32::from(kind),
                        });
                    }
                    fragment.extend_from_slice(payload);
                    continued = flags & FLAG_END_HEADERS == 0;
                }
                block = &fragment;
                let headers = decoder.decode(block)?;
                preface.first_headers = Some(FirstHeaders {
                    stream_id,
                    pseudo_header_order: pseudo_header_order(&headers),
                    priority,
                    headers,
                });
                break;
            }
            _ => {}
        }
    }

    Ok(preface)
}

fn parse_settings(payload: &[u8]) -> Result<Vec<(u16, u32)>> {
    if !payload.len().is_multiple_of(6) {
        return Err(ParseError::BadLength {
            what: "SETTINGS payload",
            value: payload.len(),
        });
    }
    let mut r = Reader::new(payload);
    let mut out = Vec::with_capacity(payload.len() / 6);
    while !r.is_empty() {
        let id = r.u16("setting id")?;
        let value = r.u32("setting value")?;
        out.push((id, value));
    }
    Ok(out)
}

fn parse_priority(payload: &[u8]) -> Result<(bool, u32, u16)> {
    let mut r = Reader::new(payload);
    let dependency = r.u32("priority dependency")?;
    let weight = r.u8("priority weight")?;
    Ok((
        dependency & 0x8000_0000 != 0,
        dependency & 0x7fff_ffff,
        u16::from(weight) + 1,
    ))
}

/// Strips padding and the optional priority block from a HEADERS payload.
fn split_headers_payload(payload: &[u8], flags: u8) -> Result<(Option<HeadersPriority>, &[u8])> {
    let mut r = Reader::new(payload);
    let pad_len = if flags & FLAG_PADDED != 0 {
        r.u8("headers pad length")? as usize
    } else {
        0
    };
    let priority = if flags & FLAG_PRIORITY != 0 {
        let block = r.take(5, "headers priority")?;
        let (exclusive, dependency, weight) = parse_priority(block)?;
        Some(HeadersPriority {
            exclusive,
            dependency,
            weight,
        })
    } else {
        None
    };
    let rest = r.rest();
    let block = rest
        .get(
            ..rest
                .len()
                .checked_sub(pad_len)
                .ok_or(ParseError::BadLength {
                    what: "headers padding",
                    value: pad_len,
                })?,
        )
        .ok_or(ParseError::BadLength {
            what: "headers padding",
            value: pad_len,
        })?;
    Ok((priority, block))
}

/// The Akamai pseudo-header order string: the initial of each pseudo-header in
/// wire order, comma separated (`m,a,s,p` for Chrome, `m,p,a,s` for Firefox).
fn pseudo_header_order(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .filter_map(|(name, _)| {
            let rest = name.strip_prefix(':')?;
            rest.chars().next()
        })
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Formats the Akamai HTTP/2 fingerprint of a captured preface.
///
/// `SETTINGS|WINDOW_UPDATE|PRIORITY|PSEUDO_HEADER_ORDER`, where SETTINGS is
/// `id:value` pairs joined by `;`, an absent WINDOW_UPDATE is `00`, an absent
/// PRIORITY list is `0`, and PRIORITY entries are
/// `stream:exclusive:dependency:weight` joined by `,`.
#[must_use]
pub fn akamai_h2(preface: &H2Preface) -> String {
    let settings = preface
        .settings
        .iter()
        .map(|(id, value)| format!("{id}:{value}"))
        .collect::<Vec<_>>()
        .join(";");
    let window = preface
        .window_update
        .map_or_else(|| "00".to_owned(), |w| w.to_string());
    let priorities = if preface.priority_frames.is_empty() {
        "0".to_owned()
    } else {
        preface
            .priority_frames
            .iter()
            .map(|p| {
                format!(
                    "{}:{}:{}:{}",
                    p.stream_id,
                    u8::from(p.exclusive),
                    p.dependency,
                    p.weight
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    let pseudo = preface
        .first_headers
        .as_ref()
        .map_or("", |h| h.pseudo_header_order.as_str());
    format!("{settings}|{window}|{priorities}|{pseudo}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a frame with the given type, flags, stream and payload.
    fn frame(kind: u8, flags: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let len = payload.len() as u32;
        out.extend_from_slice(&len.to_be_bytes()[1..]);
        out.push(kind);
        out.push(flags);
        out.extend_from_slice(&stream_id.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn settings_payload(entries: &[(u16, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (id, value) in entries {
            out.extend_from_slice(&id.to_be_bytes());
            out.extend_from_slice(&value.to_be_bytes());
        }
        out
    }

    #[test]
    fn parses_a_chrome_shaped_preface() {
        let mut bytes = CLIENT_PREFACE.to_vec();
        bytes.extend(frame(
            FRAME_SETTINGS,
            0,
            0,
            &settings_payload(&[(1, 65536), (2, 0), (4, 6291456), (6, 262144)]),
        ));
        bytes.extend(frame(FRAME_WINDOW_UPDATE, 0, 0, &15663105u32.to_be_bytes()));
        // :method GET, :authority (literal, indexed name 1), :scheme https, :path /
        bytes.extend(frame(
            FRAME_HEADERS,
            FLAG_END_HEADERS,
            1,
            &[0x82, 0x41, 0x01, b'x', 0x87, 0x84],
        ));

        let preface = parse_h2_preface(&bytes).expect("parse");
        assert_eq!(
            preface.settings,
            vec![(1, 65536), (2, 0), (4, 6291456), (6, 262144)]
        );
        assert_eq!(preface.window_update, Some(15663105));
        assert!(preface.priority_frames.is_empty());
        let headers = preface.first_headers.as_ref().expect("headers");
        assert_eq!(headers.pseudo_header_order, "m,a,s,p");
        assert_eq!(headers.priority, None);
        assert_eq!(
            akamai_h2(&preface),
            "1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p"
        );
    }

    #[test]
    fn parses_a_firefox_shaped_preface_with_priority_frames() {
        let mut bytes = CLIENT_PREFACE.to_vec();
        bytes.extend(frame(
            FRAME_SETTINGS,
            0,
            0,
            &settings_payload(&[(1, 65536), (4, 131072), (5, 16384)]),
        ));
        bytes.extend(frame(FRAME_WINDOW_UPDATE, 0, 0, &12517377u32.to_be_bytes()));
        // Stream 3 depends on 0 with weight 201 (wire 200), not exclusive.
        bytes.extend(frame(FRAME_PRIORITY, 0, 3, &[0, 0, 0, 0, 200]));
        // Stream 5 depends exclusively on 3 with weight 101 (wire 100).
        bytes.extend(frame(FRAME_PRIORITY, 0, 5, &[0x80, 0, 0, 3, 100]));
        // :method GET, :path /, :authority (literal, indexed name 1), :scheme https
        bytes.extend(frame(
            FRAME_HEADERS,
            FLAG_END_HEADERS,
            13,
            &[0x82, 0x84, 0x41, 0x01, b'x', 0x87],
        ));

        let preface = parse_h2_preface(&bytes).expect("parse");
        assert_eq!(
            preface.priority_frames,
            vec![
                PriorityFrame {
                    stream_id: 3,
                    exclusive: false,
                    dependency: 0,
                    weight: 201
                },
                PriorityFrame {
                    stream_id: 5,
                    exclusive: true,
                    dependency: 3,
                    weight: 101
                },
            ]
        );
        assert_eq!(
            akamai_h2(&preface),
            "1:65536;4:131072;5:16384|12517377|3:0:0:201,5:1:3:101|m,p,a,s"
        );
    }

    #[test]
    fn reads_headers_priority_padding_and_continuation() {
        let mut bytes = Vec::new();
        // PADDED | PRIORITY, split across HEADERS and CONTINUATION.
        let mut payload = vec![2u8]; // pad length
        payload.extend_from_slice(&[0x80, 0, 0, 0, 0xfe]); // exclusive, dep 0, weight 255
        payload.push(0x82); // :method GET
        payload.extend_from_slice(&[0, 0]); // padding
        bytes.extend(frame(
            FRAME_HEADERS,
            FLAG_PADDED | FLAG_PRIORITY,
            1,
            &payload,
        ));
        bytes.extend(frame(
            FRAME_CONTINUATION,
            FLAG_END_HEADERS,
            1,
            &[0x87, 0x84],
        ));

        let preface = parse_h2_preface(&bytes).expect("parse");
        let headers = preface.first_headers.expect("headers");
        assert_eq!(
            headers.priority,
            Some(HeadersPriority {
                exclusive: true,
                dependency: 0,
                weight: 255
            })
        );
        assert_eq!(headers.pseudo_header_order, "m,s,p");
    }

    #[test]
    fn absent_window_update_and_priorities_use_the_spec_placeholders() {
        let preface = H2Preface {
            settings: vec![(1, 4096)],
            window_update: None,
            priority_frames: Vec::new(),
            first_headers: None,
        };
        assert_eq!(akamai_h2(&preface), "1:4096|00|0|");
    }
}
