//! A decode-only HPACK implementation (RFC 7541), enough to read a browser's
//! first HEADERS frame.
//!
//! We do not use one of the published HPACK crates: `hpack` (2015),
//! `httlib-hpack` (2020), `fluke-hpack` (2024) and `loona-hpack` (2024) are all
//! unmaintained, and `h2`'s decoder is private. Decoding is a few hundred lines
//! and this crate must never grow a dependency it cannot vouch for, so the
//! static table, the dynamic table and the canonical Huffman code from
//! RFC 7541 Appendix B live here.
//!
//! Encoding is out of scope: `fp-capture` only reads headers.

use super::{ParseError, Result};

/// RFC 7541 Appendix A. Index `i` of the static table is `STATIC_TABLE[i - 1]`.
const STATIC_TABLE: [(&str, &str); 61] = [
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

/// A decoded header field.
pub type HeaderField = (String, String);

/// HPACK decoder state: the dynamic table and its size limit.
///
/// One decoder serves one direction of one connection; feed it header blocks in
/// the order the peer sent them, because the dynamic table carries over.
#[derive(Debug)]
pub struct HpackDecoder {
    /// Most recently inserted entry first, as RFC 7541 §2.3.3 indexes them.
    dynamic: Vec<HeaderField>,
    size: usize,
    max_size: usize,
    /// The largest size the peer may announce, from `SETTINGS_HEADER_TABLE_SIZE`.
    hard_max_size: usize,
}

impl Default for HpackDecoder {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl HpackDecoder {
    /// A decoder whose dynamic table starts at `max_size` octets, the value of
    /// `SETTINGS_HEADER_TABLE_SIZE` the decoder advertised (4096 by default).
    #[must_use]
    pub fn new(max_size: usize) -> Self {
        Self {
            dynamic: Vec::new(),
            size: 0,
            max_size,
            hard_max_size: max_size,
        }
    }

    /// Decodes one header block fragment sequence into fields, in wire order.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] on a truncated block, an out-of-range table index,
    /// an oversized dynamic table update, or an invalid Huffman string.
    pub fn decode(&mut self, block: &[u8]) -> Result<Vec<HeaderField>> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < block.len() {
            let first = block[pos];
            if first & 0x80 != 0 {
                // Indexed header field.
                let (index, next) = decode_integer(block, pos, 7)?;
                pos = next;
                out.push(self.entry(index)?);
            } else if first & 0xc0 == 0x40 {
                // Literal with incremental indexing.
                let (name, value, next) = self.literal(block, pos, 6)?;
                pos = next;
                self.insert(name.clone(), value.clone());
                out.push((name, value));
            } else if first & 0xe0 == 0x20 {
                // Dynamic table size update.
                let (size, next) = decode_integer(block, pos, 5)?;
                pos = next;
                let size = size as usize;
                if size > self.hard_max_size {
                    return Err(ParseError::BadLength {
                        what: "dynamic table size update",
                        value: size,
                    });
                }
                self.max_size = size;
                self.evict();
            } else {
                // Literal without indexing (0x00) or never indexed (0x10).
                let (name, value, next) = self.literal(block, pos, 4)?;
                pos = next;
                out.push((name, value));
            }
        }
        Ok(out)
    }

    /// Looks up a static or dynamic entry by HPACK index (1-based).
    fn entry(&self, index: u64) -> Result<HeaderField> {
        let index = usize::try_from(index).map_err(|_| ParseError::BadLength {
            what: "header index",
            value: usize::MAX,
        })?;
        if index == 0 {
            return Err(ParseError::Unexpected {
                what: "header index",
                found: 0,
            });
        }
        if let Some((name, value)) = STATIC_TABLE.get(index - 1) {
            return Ok(((*name).to_owned(), (*value).to_owned()));
        }
        self.dynamic
            .get(index - 1 - STATIC_TABLE.len())
            .cloned()
            .ok_or(ParseError::BadLength {
                what: "header index",
                value: index,
            })
    }

    /// Decodes a literal field: an optionally indexed name, then a string value.
    fn literal(&self, block: &[u8], pos: usize, prefix: u32) -> Result<(String, String, usize)> {
        let (index, mut pos) = decode_integer(block, pos, prefix)?;
        let name = if index == 0 {
            let (name, next) = decode_string(block, pos)?;
            pos = next;
            name
        } else {
            self.entry(index)?.0
        };
        let (value, pos) = decode_string(block, pos)?;
        Ok((name, value, pos))
    }

    /// Adds an entry to the dynamic table, evicting from the back (RFC 7541 §4.4).
    fn insert(&mut self, name: String, value: String) {
        let entry_size = name.len() + value.len() + 32;
        self.dynamic.insert(0, (name, value));
        self.size += entry_size;
        self.evict();
    }

    fn evict(&mut self) {
        while self.size > self.max_size {
            let Some((name, value)) = self.dynamic.pop() else {
                self.size = 0;
                return;
            };
            self.size -= name.len() + value.len() + 32;
        }
    }
}

/// Decodes one header block with a fresh dynamic table.
///
/// Convenient for a single captured HEADERS frame, which is all the fingerprint
/// tools need; a real connection must keep one [`HpackDecoder`] alive instead.
///
/// # Errors
///
/// See [`HpackDecoder::decode`].
pub fn decode_header_block(block: &[u8]) -> Result<Vec<HeaderField>> {
    HpackDecoder::default().decode(block)
}

/// RFC 7541 §5.1 integer, with `prefix` significant bits in the first octet.
fn decode_integer(block: &[u8], pos: usize, prefix: u32) -> Result<(u64, usize)> {
    let mask = (1u64 << prefix) - 1;
    let first = *block.get(pos).ok_or(ParseError::Truncated {
        what: "hpack integer",
        need: 1,
        have: 0,
    })?;
    let mut value = u64::from(first) & mask;
    let mut pos = pos + 1;
    if value < mask {
        return Ok((value, pos));
    }
    let mut shift = 0u32;
    loop {
        let byte = *block.get(pos).ok_or(ParseError::Truncated {
            what: "hpack integer continuation",
            need: 1,
            have: 0,
        })?;
        pos += 1;
        if shift > 56 {
            return Err(ParseError::BadLength {
                what: "hpack integer",
                value: shift as usize,
            });
        }
        value += u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, pos));
        }
        shift += 7;
    }
}

/// RFC 7541 §5.2 string literal, Huffman-coded or raw.
fn decode_string(block: &[u8], pos: usize) -> Result<(String, usize)> {
    let huffman = block.get(pos).is_some_and(|b| b & 0x80 != 0);
    let (len, pos) = decode_integer(block, pos, 7)?;
    let len = len as usize;
    let end = pos.checked_add(len).ok_or(ParseError::BadLength {
        what: "hpack string",
        value: len,
    })?;
    let raw = block.get(pos..end).ok_or(ParseError::Truncated {
        what: "hpack string",
        need: len,
        have: block.len().saturating_sub(pos),
    })?;
    let bytes = if huffman {
        huffman_decode(raw)?
    } else {
        raw.to_vec()
    };
    let text = String::from_utf8(bytes).map_err(|_| ParseError::NotUtf8 {
        what: "hpack string",
    })?;
    Ok((text, end))
}

include!("huffman_table.rs");

/// Decodes an RFC 7541 Appendix B Huffman string.
///
/// The code is canonical, so the decoder walks the input bit by bit and, at each
/// length, checks whether the accumulated code falls in that length's range.
fn huffman_decode(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len() * 8 / 5);
    let mut code: u32 = 0;
    let mut len: usize = 0;
    for byte in bytes {
        for bit in (0..8).rev() {
            code = (code << 1) | u32::from((byte >> bit) & 1);
            len += 1;
            if len >= HUFFMAN_COUNT.len() {
                return Err(ParseError::BadLength {
                    what: "huffman code",
                    value: len,
                });
            }
            let count = u32::from(HUFFMAN_COUNT[len]);
            if count == 0 {
                continue;
            }
            let min = HUFFMAN_MIN_CODE[len];
            if code < min || code >= min + count {
                continue;
            }
            let symbol = HUFFMAN_SYMBOLS[HUFFMAN_OFFSET[len] as usize + (code - min) as usize];
            if symbol == 256 {
                return Err(ParseError::Unexpected {
                    what: "huffman EOS in string",
                    found: 256,
                });
            }
            out.push(symbol as u8);
            code = 0;
            len = 0;
        }
    }
    // The tail must be the most significant bits of EOS, i.e. fewer than eight
    // bits that are all ones (RFC 7541 §5.2).
    if len >= 8 || code != (1u32 << len) - 1 {
        return Err(ParseError::BadLength {
            what: "huffman padding",
            value: len,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_rfc7541_c3_request_sequence() {
        // RFC 7541 C.3: three requests without Huffman coding, sharing one
        // dynamic table.
        let mut decoder = HpackDecoder::default();
        let first = decoder
            .decode(&[
                0x82, 0x86, 0x84, 0x41, 0x0f, b'w', b'w', b'w', b'.', b'e', b'x', b'a', b'm', b'p',
                b'l', b'e', b'.', b'c', b'o', b'm',
            ])
            .expect("first block");
        assert_eq!(
            first,
            vec![
                (":method".to_owned(), "GET".to_owned()),
                (":scheme".to_owned(), "http".to_owned()),
                (":path".to_owned(), "/".to_owned()),
                (":authority".to_owned(), "www.example.com".to_owned()),
            ]
        );

        let second = decoder
            .decode(&[
                0x82, 0x86, 0x84, 0xbe, 0x58, 0x08, b'n', b'o', b'-', b'c', b'a', b'c', b'h', b'e',
            ])
            .expect("second block");
        assert_eq!(
            second,
            vec![
                (":method".to_owned(), "GET".to_owned()),
                (":scheme".to_owned(), "http".to_owned()),
                (":path".to_owned(), "/".to_owned()),
                (":authority".to_owned(), "www.example.com".to_owned()),
                ("cache-control".to_owned(), "no-cache".to_owned()),
            ]
        );

        let third = decoder
            .decode(&[
                0x82, 0x87, 0x85, 0xbf, 0x40, 0x0a, b'c', b'u', b's', b't', b'o', b'm', b'-', b'k',
                b'e', b'y', 0x0c, b'c', b'u', b's', b't', b'o', b'm', b'-', b'v', b'a', b'l', b'u',
                b'e',
            ])
            .expect("third block");
        assert_eq!(
            third,
            vec![
                (":method".to_owned(), "GET".to_owned()),
                (":scheme".to_owned(), "https".to_owned()),
                (":path".to_owned(), "/index.html".to_owned()),
                (":authority".to_owned(), "www.example.com".to_owned()),
                ("custom-key".to_owned(), "custom-value".to_owned()),
            ]
        );
    }

    #[test]
    fn decodes_rfc7541_c4_huffman_request_sequence() {
        // RFC 7541 C.4: the same three requests, Huffman coded.
        let mut decoder = HpackDecoder::default();
        let first = decoder
            .decode(&[
                0x82, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab,
                0x90, 0xf4, 0xff,
            ])
            .expect("first block");
        assert_eq!(
            first[3],
            (":authority".to_owned(), "www.example.com".to_owned())
        );

        let second = decoder
            .decode(&[
                0x82, 0x86, 0x84, 0xbe, 0x58, 0x86, 0xa8, 0xeb, 0x10, 0x64, 0x9c, 0xbf,
            ])
            .expect("second block");
        assert_eq!(
            second[4],
            ("cache-control".to_owned(), "no-cache".to_owned())
        );

        let third = decoder
            .decode(&[
                0x82, 0x87, 0x85, 0xbf, 0x40, 0x88, 0x25, 0xa8, 0x49, 0xe9, 0x5b, 0xa9, 0x7d, 0x7f,
                0x89, 0x25, 0xa8, 0x49, 0xe9, 0x5b, 0xb8, 0xe8, 0xb4, 0xbf,
            ])
            .expect("third block");
        assert_eq!(
            third[4],
            ("custom-key".to_owned(), "custom-value".to_owned())
        );
    }

    #[test]
    fn huffman_rejects_eos_and_bad_padding() {
        // 30 one-bits is EOS; it must never appear inside a string.
        assert!(huffman_decode(&[0xff, 0xff, 0xff, 0xff, 0xff]).is_err());
        // Padding must be all ones: a trailing zero bit is invalid.
        assert!(huffman_decode(&[0x00]).is_err());
    }

    #[test]
    fn integers_use_the_rfc7541_c1_encodings() {
        assert_eq!(decode_integer(&[0x0a], 0, 5).unwrap(), (10, 1));
        assert_eq!(
            decode_integer(&[0x1f, 0x9a, 0x0a], 0, 5).unwrap(),
            (1337, 3)
        );
        assert_eq!(decode_integer(&[0x2a], 0, 8).unwrap(), (42, 1));
    }

    #[test]
    fn dynamic_table_eviction_respects_the_size_update() {
        let mut decoder = HpackDecoder::new(4096);
        // Insert one entry, then shrink the table to zero: it must be evicted.
        decoder
            .decode(&[0x40, 0x01, b'a', 0x01, b'b'])
            .expect("insert");
        assert_eq!(decoder.dynamic.len(), 1);
        decoder.decode(&[0x20]).expect("size update to 0");
        assert!(decoder.dynamic.is_empty());
        assert_eq!(decoder.size, 0);
    }
}
