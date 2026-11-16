//! `btoa` and `atob`: the binary-string codecs of the HTML Standard
//! (<https://html.spec.whatwg.org/multipage/webappapis.html#atob>).
//!
//! Both answer `None` where the standard throws `InvalidCharacterError`; the
//! engine hosts turn that into the exception.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `btoa(data)`: every code point of `data` is one byte. A code point above
/// U+00FF has no byte, which includes the replacement of a lone surrogate.
pub fn btoa(data: &str) -> Option<String> {
    let bytes = data
        .chars()
        .map(|point| u8::try_from(u32::from(point)).ok())
        .collect::<Option<Vec<u8>>>()?;
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = u32::from(chunk[0]) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        let symbol = |shift: u32| char::from(ALPHABET[(group >> shift) as usize & 63]);
        encoded.push(symbol(18));
        encoded.push(symbol(12));
        encoded.push(if chunk.len() > 1 { symbol(6) } else { '=' });
        encoded.push(if chunk.len() > 2 { symbol(0) } else { '=' });
    }
    Some(encoded)
}

/// `atob(data)`: the Infra Standard's forgiving-base64 decode
/// (<https://infra.spec.whatwg.org/#forgiving-base64-decode>), each byte
/// returned as the code point of the same value.
pub fn atob(data: &str) -> Option<String> {
    let mut symbols: Vec<u8> = data
        .bytes()
        .filter(|byte| !matches!(byte, b'\t' | b'\n' | 0x0C | b'\r' | b' '))
        .collect();
    if symbols.len().is_multiple_of(4) {
        for _ in 0..2 {
            if symbols.last() == Some(&b'=') {
                symbols.pop();
            }
        }
    }
    if symbols.len() % 4 == 1 {
        return None;
    }
    let mut decoded = String::with_capacity(symbols.len() / 4 * 3 + 2);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for symbol in symbols {
        let value = match symbol {
            b'A'..=b'Z' => symbol - b'A',
            b'a'..=b'z' => symbol - b'a' + 26,
            b'0'..=b'9' => symbol - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            // Padding anywhere but the end, and every other code point.
            _ => return None,
        };
        buffer = buffer << 6 | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            decoded.push(char::from((buffer >> bits) as u8));
            // The bits left over at the end are discarded, as the standard says.
            buffer &= (1 << bits) - 1;
        }
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::{atob, btoa};

    #[test]
    fn btoa_encodes_latin1_and_refuses_everything_above_it() {
        // RFC 4648 §10.
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(btoa(plain).as_deref(), Some(encoded), "{plain}");
        }
        assert_eq!(btoa("\u{0}\u{ff}\u{fe}").as_deref(), Some("AP/+"));
        assert_eq!(btoa("\u{100}"), None);
        assert_eq!(btoa("a\u{fffd}"), None);
    }

    #[test]
    fn atob_is_forgiving_about_whitespace_and_padding_only() {
        for (encoded, plain) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zg", "f"),
            ("Zm8=", "fo"),
            ("Zm8", "fo"),
            ("Zm9v", "foo"),
            (" Zm9v\tYmF\ny\r\x0c", "foobar"),
            ("AP/+", "\u{0}\u{ff}\u{fe}"),
            // Leftover bits are dropped: `Zh` carries four that belong nowhere.
            ("Zh", "f"),
        ] {
            assert_eq!(atob(encoded).as_deref(), Some(plain), "{encoded:?}");
        }
        for refused in [
            "Z",     // one symbol is never a byte
            "Zm9vY", // length 5
            "Zg=",   // one `=` where the length then leaves 3 symbols and `=`
            "Zg===", // too much padding
            "Z=g=",  // padding inside
            "Zm9v=", // padding after a whole group
            "Zm-v",  // the URL-safe alphabet is not accepted
            "Zm9\u{e9}",
        ] {
            assert_eq!(atob(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn every_byte_round_trips() {
        let all: String = (0u8..=255).map(char::from).collect();
        let encoded = btoa(&all).expect("latin1");
        assert_eq!(atob(&encoded).as_deref(), Some(all.as_str()));
    }
}
