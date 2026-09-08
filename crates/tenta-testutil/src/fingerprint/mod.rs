//! Open-standard client fingerprint algorithms: JA3, JA4 and the Akamai HTTP/2
//! fingerprint.
//!
//! These are pure parsers and hashes over bytes we captured from a real client:
//! nothing here performs IO. `tests/fingerprint/` and the `tenta-net`
//! conformance test feed them a ClientHello and an HTTP/2 client preface taken
//! off our own transport and compare the result with the fingerprint profile in
//! `profiles/` (`docs/spec/03-network.md` §2). The `fp-capture` binary uses the
//! same code on bytes taken off a real browser.
//!
//! References: FoxIO's JA4 specification (`technical_details/JA4.md` in
//! <https://github.com/FoxIO-LLC/ja4>), Salesforce's JA3, Akamai's
//! "Passive Fingerprinting of HTTP/2 Clients" and RFC 7541 for HPACK.

mod h2;
mod hpack;
mod ja3;
mod ja4;
mod tls;

pub use h2::{
    FirstHeaders, H2Preface, HeadersPriority, PriorityFrame, akamai_h2, parse_h2_preface,
};
pub use hpack::{HpackDecoder, decode_header_block};
pub use ja3::{ja3, ja3_string};
pub use ja4::{Ja4, ja4};
pub use tls::{
    ApplicationSettings, ClientHello, Extension, ext, is_grease, parse_client_hello, version_label,
};

/// Everything that can go wrong while parsing captured wire bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The input ended in the middle of a field.
    Truncated {
        /// What was being read.
        what: &'static str,
        /// How many bytes were needed.
        need: usize,
        /// How many bytes were left.
        have: usize,
    },
    /// A length prefix did not match the bytes that followed it.
    BadLength {
        /// What was being read.
        what: &'static str,
        /// The offending value.
        value: usize,
    },
    /// A tag, type or magic value was not the one the format requires.
    Unexpected {
        /// What was being read.
        what: &'static str,
        /// The value found.
        found: u32,
    },
    /// Bytes that had to be UTF-8 were not.
    NotUtf8 {
        /// What was being read.
        what: &'static str,
    },
    /// The capture did not contain a frame the caller needs.
    Missing {
        /// What was expected.
        what: &'static str,
    },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated { what, need, have } => {
                write!(f, "truncated {what}: need {need} bytes, have {have}")
            }
            Self::BadLength { what, value } => write!(f, "bad length {value} for {what}"),
            Self::Unexpected { what, found } => write!(f, "unexpected {what}: {found:#x}"),
            Self::NotUtf8 { what } => write!(f, "{what} is not valid UTF-8"),
            Self::Missing { what } => write!(f, "capture contains no {what}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Result of parsing captured wire bytes.
pub type Result<T> = std::result::Result<T, ParseError>;

/// Big-endian cursor over a captured buffer.
///
/// Every read is checked; a short buffer produces [`ParseError::Truncated`]
/// rather than a panic, because the inputs are attacker-shaped by definition.
#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub(crate) fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(ParseError::Truncated {
                what,
                need: n,
                have: self.remaining(),
            });
        }
        let out = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub(crate) fn rest(&mut self) -> &'a [u8] {
        let out = &self.bytes[self.pos..];
        self.pos = self.bytes.len();
        out
    }

    pub(crate) fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    pub(crate) fn u16(&mut self, what: &'static str) -> Result<u16> {
        let b = self.take(2, what)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(crate) fn u24(&mut self, what: &'static str) -> Result<u32> {
        let b = self.take(3, what)?;
        Ok(u32::from_be_bytes([0, b[0], b[1], b[2]]))
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32> {
        let b = self.take(4, what)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Reads a vector introduced by a one-byte length.
    pub(crate) fn u8_prefixed(&mut self, what: &'static str) -> Result<&'a [u8]> {
        let n = self.u8(what)? as usize;
        self.take(n, what)
    }

    /// Reads a vector introduced by a two-byte length.
    pub(crate) fn u16_prefixed(&mut self, what: &'static str) -> Result<&'a [u8]> {
        let n = self.u16(what)? as usize;
        self.take(n, what)
    }
}

/// Lower-case hex of `bytes`, no separator.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

/// The first `chars` hex characters of the SHA-256 of `input`.
///
/// JA4's `b` and `c` components are this with `chars = 12`.
pub(crate) fn sha256_truncated(input: &str, chars: usize) -> String {
    use sha2::{Digest, Sha256};
    let mut full = hex(&Sha256::digest(input.as_bytes()));
    full.truncate(chars);
    full
}

/// Reads a list of `u16` from a byte slice whose length must be even.
pub(crate) fn u16_list(bytes: &[u8], what: &'static str) -> Result<Vec<u16>> {
    if !bytes.len().is_multiple_of(2) {
        return Err(ParseError::BadLength {
            what,
            value: bytes.len(),
        });
    }
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect())
}
