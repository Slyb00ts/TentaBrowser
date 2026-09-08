//! JA4 for TLS clients, per FoxIO's `technical_details/JA4.md`.
//!
//! The fingerprint is `a_b_c`:
//!
//! * `a` — transport, TLS version, whether SNI was sent, cipher and extension
//!   counts, and the first and last character of the first ALPN value;
//! * `b` — truncated SHA-256 of the cipher list sorted by hex value;
//! * `c` — truncated SHA-256 of the extension list sorted by hex value (without
//!   SNI and ALPN) joined by `_` to the signature algorithms in wire order.
//!
//! GREASE is excluded everywhere. Sorting `b` and `c` is what makes JA4 survive
//! Chrome's per-connection extension permutation, which is why profiles store
//! JA4 and not JA3 (`docs/spec/03-network.md` §2).

use super::tls::{ClientHello, ext, is_grease, version_label};
use super::{hex, sha256_truncated};

/// The JA4 fingerprint of a ClientHello and the raw lists behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ja4 {
    /// The unhashed prefix, e.g. `t13d1516h2`.
    pub a: String,
    /// The 12-character cipher hash.
    pub b: String,
    /// The 12-character extension and signature-algorithm hash.
    pub c: String,
    /// `a_b_c`, the fingerprint a profile stores as `ja4`.
    pub full: String,
    /// The sorted cipher list that `b` hashes.
    pub raw_b: String,
    /// The sorted extension list and signature algorithms that `c` hashes.
    pub raw_c: String,
}

impl Ja4 {
    /// `JA4_r`: the same fingerprint with `b` and `c` left unhashed. A profile
    /// stores it as `ja4_r`; it is what tells you *why* two JA4s differ.
    #[must_use]
    pub fn raw(&self) -> String {
        format!("{}_{}_{}", self.a, self.raw_b, self.raw_c)
    }
}

impl std::fmt::Display for Ja4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.full)
    }
}

/// Computes the JA4 fingerprint of a TLS-over-TCP ClientHello.
#[must_use]
pub fn ja4(hello: &ClientHello) -> Ja4 {
    let ciphers = hello.ciphers_without_grease();
    let extensions = hello.extensions_without_grease();

    let sni = if hello.has_extension(ext::SERVER_NAME) {
        'd'
    } else {
        'i'
    };
    let a = format!(
        "t{}{}{}{}{}",
        version_label(hello.effective_version()),
        sni,
        two_digits(ciphers.len()),
        two_digits(extensions.len()),
        alpn_marker(hello),
    );

    let mut sorted_ciphers = ciphers;
    sorted_ciphers.sort_unstable();
    let raw_b = join_hex(&sorted_ciphers);
    let b = if sorted_ciphers.is_empty() {
        "000000000000".to_owned()
    } else {
        sha256_truncated(&raw_b, 12)
    };

    let mut sorted_extensions: Vec<u16> = extensions
        .into_iter()
        .filter(|k| *k != ext::SERVER_NAME && *k != ext::ALPN)
        .collect();
    sorted_extensions.sort_unstable();
    let sigalgs: Vec<u16> = hello
        .signature_algorithms()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !is_grease(*s))
        .collect();
    let raw_c = if sigalgs.is_empty() {
        join_hex(&sorted_extensions)
    } else {
        format!("{}_{}", join_hex(&sorted_extensions), join_hex(&sigalgs))
    };
    let c = if sorted_extensions.is_empty() {
        "000000000000".to_owned()
    } else {
        sha256_truncated(&raw_c, 12)
    };

    let full = format!("{a}_{b}_{c}");
    Ja4 {
        a,
        b,
        c,
        full,
        raw_b,
        raw_c,
    }
}

/// Two decimal digits, saturating at 99 as the spec requires.
fn two_digits(n: usize) -> String {
    format!("{:02}", n.min(99))
}

fn join_hex(values: &[u16]) -> String {
    values
        .iter()
        .map(|v| format!("{v:04x}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The two ALPN characters of JA4's `a` component.
///
/// The first ALPN value's first and last bytes are used when both are ASCII
/// alphanumeric; otherwise the first and last characters of the value's hex
/// encoding are used, which is how GREASE and binary ALPNs stay printable.
fn alpn_marker(hello: &ClientHello) -> String {
    let Some(first) = hello.alpn().and_then(|list| list.into_iter().next()) else {
        return "00".to_owned();
    };
    alpn_marker_for(&first)
}

fn alpn_marker_for(value: &[u8]) -> String {
    let (Some(first), Some(last)) = (value.first().copied(), value.last().copied()) else {
        return "00".to_owned();
    };
    if is_ascii_alphanumeric(first) && is_ascii_alphanumeric(last) {
        return String::from_utf8_lossy(&[first, last]).into_owned();
    }
    let encoded = hex(value);
    let bytes = encoded.as_bytes();
    // `hex` of a non-empty slice always has at least two characters.
    format!("{}{}", bytes[0] as char, bytes[bytes.len() - 1] as char)
}

fn is_ascii_alphanumeric(b: u8) -> bool {
    b.is_ascii_digit() || b.is_ascii_uppercase() || b.is_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpn_marker_follows_the_spec_examples() {
        // The eight worked examples from JA4.md, "ALPN Extension Value".
        assert_eq!(alpn_marker_for(&[0xab]), "ab");
        assert_eq!(alpn_marker_for(&[0x20]), "20");
        assert_eq!(alpn_marker_for(&[0xab, 0xcd]), "ad");
        assert_eq!(alpn_marker_for(&[0x20, 0x61]), "21");
        assert_eq!(alpn_marker_for(&[0x30, 0xab]), "3b");
        assert_eq!(alpn_marker_for(&[0x61, 0x20]), "60");
        assert_eq!(alpn_marker_for(&[0x30, 0x31, 0xab, 0xcd]), "3d");
        assert_eq!(alpn_marker_for(&[0x30, 0xab, 0xcd, 0x31]), "01");
        // Alphanumeric values, including the single-character rule.
        assert_eq!(alpn_marker_for(b"h2"), "h2");
        assert_eq!(alpn_marker_for(b"http/1.1"), "h1");
        assert_eq!(alpn_marker_for(b"q"), "qq");
        assert_eq!(alpn_marker_for(b""), "00");
    }

    #[test]
    fn cipher_count_saturates_at_99() {
        assert_eq!(two_digits(0), "00");
        assert_eq!(two_digits(5), "05");
        assert_eq!(two_digits(99), "99");
        assert_eq!(two_digits(140), "99");
    }
}
