//! Encoding sniffing and streaming decoding (`docs/spec/04-dom-and-css.md`
//! §3.4).
//!
//! UTF-8 and ASCII bodies are validated in place and handed to the tokenizer
//! as slices of the caller's buffer; every other encoding is transcoded once
//! with `encoding_rs` (`docs/spec/02-architecture.md` §7).

use encoding_rs::{DecoderResult, Encoding, UTF_8, WINDOWS_1250, WINDOWS_1252};

/// How sure we are about the encoding. A tentative encoding can still be
/// overridden by a late `<meta charset>`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confidence {
    /// From a BOM or the transport layer; a `<meta charset>` cannot change it.
    Certain,
    /// From the `<meta>` prescan or the locale default.
    Tentative,
}

/// The result of the encoding sniffing algorithm.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sniffed {
    /// The encoding to decode with.
    pub encoding: &'static Encoding,
    /// How sure we are.
    pub confidence: Confidence,
    /// Length of the byte order mark that must be skipped, in bytes.
    pub bom_length: usize,
}

/// How many bytes the prescan looks at.
pub const PRESCAN_LIMIT: usize = 1024;

/// The default encoding for a locale: `windows-1250` for Polish, else
/// `windows-1252` (`docs/spec/04-dom-and-css.md` §3.4).
pub fn default_encoding_for_locale(default_locale: &str) -> &'static Encoding {
    let language = default_locale
        .split(['-', '_'])
        .next()
        .unwrap_or(default_locale);
    if language.eq_ignore_ascii_case("pl") {
        WINDOWS_1250
    } else {
        WINDOWS_1252
    }
}

/// The BOM at the start of `bytes`, if any.
fn sniff_bom(bytes: &[u8]) -> Option<(&'static Encoding, usize)> {
    Encoding::for_bom(bytes)
}

/// The WHATWG encoding sniffing algorithm.
///
/// `transport_charset` is the `charset` parameter of the `Content-Type`
/// header, if the response had one. `default_locale` picks the fallback.
pub fn sniff(bytes: &[u8], transport_charset: Option<&str>, default_locale: &str) -> Sniffed {
    if let Some((encoding, bom_length)) = sniff_bom(bytes) {
        return Sniffed {
            encoding,
            confidence: Confidence::Certain,
            bom_length,
        };
    }
    if let Some(label) = transport_charset
        && let Some(encoding) = Encoding::for_label(label.trim().as_bytes())
    {
        return Sniffed {
            encoding,
            confidence: Confidence::Certain,
            bom_length: 0,
        };
    }
    if let Some(encoding) = prescan(&bytes[..bytes.len().min(PRESCAN_LIMIT)]) {
        return Sniffed {
            encoding,
            confidence: Confidence::Tentative,
            bom_length: 0,
        };
    }
    Sniffed {
        encoding: default_encoding_for_locale(default_locale),
        confidence: Confidence::Tentative,
        bom_length: 0,
    }
}

/// Encoding sniffing for XML: a BOM, then the `encoding` pseudo-attribute of
/// the XML declaration, then UTF-8
/// (<https://www.w3.org/TR/xml/#sec-guessing>).
pub fn sniff_xml(bytes: &[u8]) -> Sniffed {
    if let Some((encoding, bom_length)) = sniff_bom(bytes) {
        return Sniffed {
            encoding,
            confidence: Confidence::Certain,
            bom_length,
        };
    }
    if let Some(encoding) = xml_declaration_encoding(bytes) {
        return Sniffed {
            encoding,
            confidence: Confidence::Certain,
            bom_length: 0,
        };
    }
    Sniffed {
        encoding: UTF_8,
        confidence: Confidence::Tentative,
        bom_length: 0,
    }
}

/// The `encoding="…"` pseudo-attribute of an XML declaration. The declaration
/// is ASCII-compatible by construction, so the bytes can be read directly.
fn xml_declaration_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    let head = &bytes[..bytes.len().min(PRESCAN_LIMIT)];
    let declaration_end = head.windows(2).position(|pair| pair == b"?>")?;
    let declaration = head.get(..declaration_end)?;
    if !declaration.starts_with(b"<?xml") {
        return None;
    }
    let start = declaration
        .windows("encoding".len())
        .position(|window| window == b"encoding")?
        + "encoding".len();
    let rest = &declaration[start..];
    let equals = rest.iter().position(|&byte| byte == b'=')?;
    let rest = &rest[equals + 1..];
    let quote = *rest.iter().find(|byte| !byte.is_ascii_whitespace())?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let value_start = rest.iter().position(|&byte| byte == quote)? + 1;
    let rest = &rest[value_start..];
    let value_end = rest.iter().position(|&byte| byte == quote)?;
    Encoding::for_label(&rest[..value_end])
}

/// Maps a label to an encoding the way the `<meta>` rules require: UTF-16
/// becomes UTF-8 and `x-user-defined` becomes `windows-1252`.
pub fn encoding_from_meta_label(label: &str) -> Option<&'static Encoding> {
    let encoding = Encoding::for_label(label.trim().as_bytes())?;
    if encoding == encoding_rs::UTF_16BE || encoding == encoding_rs::UTF_16LE {
        return Some(UTF_8);
    }
    if encoding == encoding_rs::X_USER_DEFINED {
        return Some(WINDOWS_1252);
    }
    Some(encoding)
}

/// <https://html.spec.whatwg.org/multipage/#algorithm-for-extracting-a-character-encoding-from-a-meta-element>
pub(crate) fn extract_a_character_encoding_from_a_meta_element(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut position = 0;
    let value_start = loop {
        // Find the next ASCII case-insensitive "charset".
        loop {
            let candidate = bytes.get(position..position + "charset".len())?;
            if candidate.eq_ignore_ascii_case(b"charset") {
                break;
            }
            position += 1;
        }
        position += "charset".len();
        position += bytes[position..]
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();
        if bytes.get(position) == Some(&b'=') {
            position += 1;
            position += bytes[position..]
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
            break position;
        }
    };

    match bytes.get(value_start)? {
        quote @ (b'"' | b'\'') => {
            let length = bytes[value_start + 1..]
                .iter()
                .position(|byte| byte == quote)?;
            Some(input[value_start + 1..value_start + 1 + length].to_owned())
        }
        _ => {
            let length = bytes[value_start..]
                .iter()
                .position(|byte| byte.is_ascii_whitespace() || *byte == b';')
                .unwrap_or(bytes.len() - value_start);
            Some(input[value_start..value_start + length].to_owned())
        }
    }
}

/// <https://html.spec.whatwg.org/multipage/#prescan-a-byte-stream-to-determine-its-encoding>
pub fn prescan(bytes: &[u8]) -> Option<&'static Encoding> {
    let mut position = 0usize;
    while position < bytes.len() {
        if bytes[position..].starts_with(b"<!--") {
            position = find(bytes, position + 2, b"-->")? + 2;
        } else if starts_with_ci(&bytes[position..], b"<meta")
            && matches!(
                bytes.get(position + 5),
                Some(b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'/')
            )
        {
            position += 5;
            if let Some(result) = prescan_meta(bytes, &mut position) {
                return Some(result);
            }
            continue;
        } else if is_tag_start(bytes, position) {
            position += if bytes.get(position + 1) == Some(&b'/') {
                2
            } else {
                1
            };
            while position < bytes.len()
                && !matches!(
                    bytes[position],
                    b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'>'
                )
            {
                position += 1;
            }
            while get_attribute(bytes, &mut position).is_some() {}
            continue;
        } else if matches!(
            (bytes.get(position), bytes.get(position + 1)),
            (Some(b'<'), Some(b'!' | b'/' | b'?'))
        ) {
            position += bytes[position..].iter().position(|&b| b == b'>')?;
        }
        position += 1;
    }
    None
}

fn is_tag_start(bytes: &[u8], position: usize) -> bool {
    if bytes.get(position) != Some(&b'<') {
        return false;
    }
    match bytes.get(position + 1) {
        Some(b) if b.is_ascii_alphabetic() => true,
        Some(b'/') => bytes.get(position + 2).is_some_and(u8::is_ascii_alphabetic),
        _ => false,
    }
}

fn starts_with_ci(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|index| from + index)
}

/// The `<meta>` branch of the prescan.
fn prescan_meta(bytes: &[u8], position: &mut usize) -> Option<&'static Encoding> {
    let mut seen_names: Vec<Vec<u8>> = Vec::new();
    let mut got_pragma = false;
    let mut need_pragma: Option<bool> = None;
    let mut charset: Option<&'static Encoding> = None;

    while let Some((name, value)) = get_attribute(bytes, position) {
        if seen_names.contains(&name) {
            continue;
        }
        match name.as_slice() {
            b"http-equiv" => {
                if value == b"content-type" {
                    got_pragma = true;
                }
            }
            b"content" => {
                if charset.is_none()
                    && let Ok(text) = std::str::from_utf8(&value)
                    && let Some(label) = extract_a_character_encoding_from_a_meta_element(text)
                    && let Some(encoding) = encoding_from_meta_label(&label)
                {
                    charset = Some(encoding);
                    need_pragma = Some(true);
                }
            }
            b"charset" => {
                if let Ok(label) = std::str::from_utf8(&value)
                    && let Some(encoding) = encoding_from_meta_label(label)
                {
                    charset = Some(encoding);
                    need_pragma = Some(false);
                }
            }
            _ => {}
        }
        seen_names.push(name);
    }

    match need_pragma {
        None => None,
        Some(true) if !got_pragma => None,
        _ => charset,
    }
}

/// <https://html.spec.whatwg.org/multipage/#concept-get-attributes-when-sniffing>
fn get_attribute(bytes: &[u8], position: &mut usize) -> Option<(Vec<u8>, Vec<u8>)> {
    let is_space = |b: u8| matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ');

    while matches!(bytes.get(*position), Some(&b) if is_space(b) || b == b'/') {
        *position += 1;
    }
    if bytes.get(*position) == Some(&b'>') || *position >= bytes.len() {
        return None;
    }

    let mut name = Vec::new();
    let mut value = Vec::new();

    loop {
        match bytes.get(*position) {
            None => return None,
            Some(b'=') if !name.is_empty() => {
                *position += 1;
                break;
            }
            Some(&b) if is_space(b) => {
                while matches!(bytes.get(*position), Some(&b) if is_space(b)) {
                    *position += 1;
                }
                if bytes.get(*position) != Some(&b'=') {
                    return Some((name, value));
                }
                *position += 1;
                break;
            }
            Some(b'/') | Some(b'>') => return Some((name, value)),
            Some(&b) => {
                name.push(b.to_ascii_lowercase());
                *position += 1;
            }
        }
    }

    while matches!(bytes.get(*position), Some(&b) if is_space(b)) {
        *position += 1;
    }

    match bytes.get(*position) {
        None => None,
        Some(&quote @ (b'"' | b'\'')) => {
            *position += 1;
            loop {
                match bytes.get(*position) {
                    None => return None,
                    Some(&b) if b == quote => {
                        *position += 1;
                        return Some((name, value));
                    }
                    Some(&b) => {
                        value.push(b.to_ascii_lowercase());
                        *position += 1;
                    }
                }
            }
        }
        Some(b'>') => Some((name, value)),
        Some(&b) => {
            value.push(b.to_ascii_lowercase());
            *position += 1;
            loop {
                match bytes.get(*position) {
                    None => return None,
                    Some(&b) if is_space(b) || b == b'>' => return Some((name, value)),
                    Some(&b) => {
                        value.push(b.to_ascii_lowercase());
                        *position += 1;
                    }
                }
            }
        }
    }
}

/// Length of a trailing byte sequence that could still become a valid
/// character once more input arrives.
fn incomplete_tail_len(bytes: &[u8]) -> usize {
    let len = bytes.len();
    for back in 1..=3.min(len) {
        let byte = bytes[len - back];
        if byte < 0x80 {
            return 0;
        }
        if byte >= 0xC0 {
            let needed = if byte < 0xE0 {
                2
            } else if byte < 0xF0 {
                3
            } else {
                4
            };
            return if back < needed { back } else { 0 };
        }
    }
    0
}

/// A streaming decoder.
///
/// For UTF-8 (and therefore ASCII) input the bulk of every chunk is validated
/// in place and returned as a borrowed slice; only the few bytes of a
/// character that straddles a chunk boundary are copied. Other encodings are
/// transcoded once into an internal buffer.
pub struct Decoder {
    encoding: &'static Encoding,
    /// Bytes held back from the previous chunk.
    pending: Vec<u8>,
    /// Owned output for the transcoding path and for straddling characters.
    buffer: String,
    transcoder: Option<encoding_rs::Decoder>,
}

impl Decoder {
    /// A decoder for `encoding`.
    pub fn new(encoding: &'static Encoding) -> Decoder {
        let transcoder = if encoding == UTF_8 {
            None
        } else {
            Some(encoding.new_decoder_without_bom_handling())
        };
        Decoder {
            encoding,
            pending: Vec::new(),
            buffer: String::new(),
            transcoder,
        }
    }

    /// The encoding this decoder decodes.
    pub fn encoding(&self) -> &'static Encoding {
        self.encoding
    }

    /// Decodes one chunk.
    ///
    /// Returns `(owned, borrowed)`: `owned` holds text that had to be copied
    /// (a straddling character, or all of it for a transcoded encoding) and
    /// comes first; `borrowed` is a slice of `bytes`. Either may be empty.
    pub fn decode<'d, 'b>(&'d mut self, bytes: &'b [u8], last: bool) -> (&'d str, &'b str) {
        self.buffer.clear();
        if let Some(transcoder) = &mut self.transcoder {
            // `decode_to_string` stops at `OutputFull` when the buffer cannot
            // take the whole chunk, so the loop runs until the input is
            // consumed rather than trusting one `reserve` to be large enough:
            // the replacement-emitting path can need more than
            // `max_utf8_buffer_length_without_replacement` says.
            let mut rest = bytes;
            loop {
                let capacity = transcoder
                    .max_utf8_buffer_length(rest.len())
                    .unwrap_or(rest.len().saturating_mul(4))
                    .max(4);
                self.buffer.reserve(capacity);
                let (result, read) =
                    transcoder.decode_to_string_without_replacement(rest, &mut self.buffer, last);
                rest = &rest[read..];
                match result {
                    DecoderResult::InputEmpty => break,
                    DecoderResult::OutputFull => continue,
                    DecoderResult::Malformed(..) => self.buffer.push('\u{FFFD}'),
                }
            }
            return (&self.buffer, "");
        }

        if self.pending.is_empty() {
            match std::str::from_utf8(bytes) {
                Ok(text) => return ("", text),
                Err(error) if error.error_len().is_none() && !last => {
                    let valid = error.valid_up_to();
                    self.pending.extend_from_slice(&bytes[valid..]);
                    let text = std::str::from_utf8(&bytes[..valid])
                        .expect("prefix validated by from_utf8");
                    return ("", text);
                }
                Err(_) => {}
            }
        }

        // Slow path: a straddling character or malformed input.
        let mut combined = std::mem::take(&mut self.pending);
        combined.extend_from_slice(bytes);
        let split = if last {
            combined.len()
        } else {
            combined.len() - incomplete_tail_len(&combined)
        };
        self.pending.clear();
        self.pending.extend_from_slice(&combined[split..]);
        self.buffer
            .push_str(&String::from_utf8_lossy(&combined[..split]));
        (&self.buffer, "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_wins_over_everything() {
        let sniffed = sniff(b"\xef\xbb\xbf<html>", Some("windows-1252"), "pl");
        assert_eq!(sniffed.encoding, UTF_8);
        assert_eq!(sniffed.confidence, Confidence::Certain);
        assert_eq!(sniffed.bom_length, 3);
    }

    #[test]
    fn transport_charset_beats_the_prescan() {
        let sniffed = sniff(b"<meta charset=iso-8859-2>", Some("windows-1252"), "pl");
        assert_eq!(sniffed.encoding, WINDOWS_1252);
        assert_eq!(sniffed.confidence, Confidence::Certain);
    }

    #[test]
    fn prescan_finds_meta_charset_and_content_type() {
        assert_eq!(
            prescan(b"<html><head><meta charset=\"iso-8859-2\">"),
            Some(encoding_rs::ISO_8859_2)
        );
        assert_eq!(
            prescan(
                b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1250\">"
            ),
            Some(WINDOWS_1250)
        );
        // A `content` charset without the pragma is ignored.
        assert_eq!(
            prescan(b"<meta content=\"text/html; charset=windows-1250\">"),
            None
        );
        // Comments are skipped.
        assert_eq!(
            prescan(b"<!-- <meta charset=big5> --><meta charset=gbk>"),
            Some(encoding_rs::GBK)
        );
    }

    #[test]
    fn locale_default_is_windows_1250_for_polish() {
        assert_eq!(default_encoding_for_locale("pl"), WINDOWS_1250);
        assert_eq!(default_encoding_for_locale("pl-PL"), WINDOWS_1250);
        assert_eq!(default_encoding_for_locale("en-GB"), WINDOWS_1252);
        assert_eq!(sniff(b"<html>", None, "pl").encoding, WINDOWS_1250);
    }

    #[test]
    fn utf8_is_decoded_without_copying() {
        let mut decoder = Decoder::new(UTF_8);
        let bytes = "zażółć gęślą jaźń".as_bytes();
        let (owned, borrowed) = decoder.decode(bytes, true);
        assert_eq!(owned, "");
        assert_eq!(borrowed, "zażółć gęślą jaźń");
        assert!(std::ptr::eq(borrowed.as_ptr(), bytes.as_ptr()));
    }

    #[test]
    fn utf8_character_straddling_a_chunk_boundary() {
        let text = "aął";
        let bytes = text.as_bytes();
        let mut decoder = Decoder::new(UTF_8);
        let mut out = String::new();
        for split in 1..bytes.len() {
            let mut decoder2 = Decoder::new(UTF_8);
            let mut result = String::new();
            let (owned, borrowed) = decoder2.decode(&bytes[..split], false);
            result.push_str(owned);
            result.push_str(borrowed);
            let (owned, borrowed) = decoder2.decode(&bytes[split..], true);
            result.push_str(owned);
            result.push_str(borrowed);
            assert_eq!(result, text, "split at {split}");
        }
        let (owned, borrowed) = decoder.decode(bytes, true);
        out.push_str(owned);
        out.push_str(borrowed);
        assert_eq!(out, text);
    }

    #[test]
    fn windows_1250_is_transcoded() {
        let mut decoder = Decoder::new(WINDOWS_1250);
        // 0x9C is U+015B (ś) in windows-1250.
        let (owned, borrowed) = decoder.decode(b"go\x9c\xe6", true);
        assert_eq!(owned, "gość");
        assert_eq!(borrowed, "");
    }

    #[test]
    fn meta_charset_extraction() {
        assert_eq!(
            extract_a_character_encoding_from_a_meta_element("text/html; charset=utf-8"),
            Some("utf-8".to_owned())
        );
        assert_eq!(
            extract_a_character_encoding_from_a_meta_element("cHarSet \t=\t'utf8'"),
            Some("utf8".to_owned())
        );
        assert_eq!(
            extract_a_character_encoding_from_a_meta_element("charset utf8"),
            None
        );
        assert_eq!(
            extract_a_character_encoding_from_a_meta_element("charset="),
            None
        );
    }
}
