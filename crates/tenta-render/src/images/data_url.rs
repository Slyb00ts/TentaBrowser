//! `data:` URLs (RFC 2397 with the Fetch standard's forgiving base64).

/// The payload of a `data:` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataUrl {
    /// The media type, lower-cased and without parameters; `None` when the URL
    /// names none.
    pub mime: Option<String>,
    /// The decoded bytes.
    pub bytes: Vec<u8>,
}

/// Decodes a `data:` URL, refusing a payload over `limit` bytes.
pub fn decode_data_url(url: &str, limit: usize) -> Result<DataUrl, String> {
    let rest = url
        .strip_prefix("data:")
        .ok_or_else(|| "not a data: URL".to_owned())?;
    let rest = rest.split('#').next().unwrap_or(rest);
    let (meta, payload) = rest
        .split_once(',')
        .ok_or_else(|| "a data: URL has no payload".to_owned())?;
    let meta = meta.trim();
    let base64 = meta
        .rsplit(';')
        .next()
        .is_some_and(|last| last.trim().eq_ignore_ascii_case("base64"));
    // The size is bounded before the payload is expanded.
    if payload.len() > limit.saturating_mul(2).saturating_add(64) {
        return Err("the data: URL exceeds the size limit".to_owned());
    }
    let bytes = percent_decode(payload);
    let bytes = if base64 {
        decode_base64(&bytes).ok_or_else(|| "the data: URL's base64 is invalid".to_owned())?
    } else {
        bytes
    };
    if bytes.len() > limit {
        return Err("the data: URL exceeds the size limit".to_owned());
    }
    let mime = meta
        .split(';')
        .next()
        .map(|m| m.trim().to_ascii_lowercase())
        .filter(|m| m.contains('/'));
    Ok(DataUrl { mime, bytes })
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |byte: u8| char::from(byte).to_digit(16);
        match (bytes[index], bytes.get(index + 1), bytes.get(index + 2)) {
            (b'%', Some(&high), Some(&low)) if hex(high).is_some() && hex(low).is_some() => {
                decoded.push((hex(high).unwrap_or(0) * 16 + hex(low).unwrap_or(0)) as u8);
                index += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    decoded
}

/// The Fetch standard's forgiving base64 decode.
fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if data.len().is_multiple_of(4) {
        for _ in 0..2 {
            if data.last() == Some(&b'=') {
                data.pop();
            }
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let mut decoded = Vec::with_capacity(data.len() * 3 / 4);
    let (mut accumulator, mut bits) = (0u32, 0u32);
    for byte in data {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            decoded.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_percent_payloads_decode() {
        let url = decode_data_url("data:image/png;base64,aGk=", 100).unwrap();
        assert_eq!(url.mime.as_deref(), Some("image/png"));
        assert_eq!(url.bytes, b"hi");
        let url = decode_data_url("data:image/svg+xml;charset=utf-8,%3Csvg%2F%3E", 100).unwrap();
        assert_eq!(url.mime.as_deref(), Some("image/svg+xml"));
        assert_eq!(url.bytes, b"<svg/>");
        assert_eq!(decode_data_url("data:,x", 10).unwrap().mime, None);
    }

    #[test]
    fn bad_or_oversized_payloads_are_errors() {
        assert!(decode_data_url("data:image/png;base64,a", 10).is_err());
        assert!(decode_data_url("data:image/png", 10).is_err());
        assert!(decode_data_url("data:text/plain,0123456789abc", 10).is_err());
        assert!(decode_data_url("https://x/", 10).is_err());
    }
}
