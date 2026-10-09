//! Content sniffing by magic bytes; neither the URL nor the declared type is consulted.

use std::fmt;

/// An image format this module can decode.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum ImageFormat {
    /// Portable Network Graphics.
    Png,
    /// JPEG, baseline or progressive.
    Jpeg,
    /// WebP, lossy or lossless, with or without alpha.
    WebP,
    /// GIF; only the first frame is decoded.
    Gif,
    /// Windows bitmap.
    Bmp,
    /// Windows icon or cursor.
    Ico,
    /// Scalable Vector Graphics. Never sniffed: a browser takes an SVG on its
    /// declared type, and [`decode_svg`](super::decode_svg) renders it.
    Svg,
}

impl fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
            Self::Gif => "GIF",
            Self::Bmp => "BMP",
            Self::Ico => "ICO",
            Self::Svg => "SVG",
        })
    }
}

pub(super) const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The format whose signature `bytes` starts with, following the WHATWG MIME
/// Sniffing image signatures.
pub fn sniff_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::WebP)
    } else if bytes.starts_with(&[0, 0, 1, 0]) || bytes.starts_with(&[0, 0, 2, 0]) {
        Some(ImageFormat::Ico)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_select_the_format() {
        assert_eq!(sniff_format(&PNG_SIGNATURE), Some(ImageFormat::Png));
        assert_eq!(
            sniff_format(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(sniff_format(b"GIF89a\x01\0"), Some(ImageFormat::Gif));
        assert_eq!(
            sniff_format(b"RIFF\x10\0\0\0WEBPVP8 "),
            Some(ImageFormat::WebP)
        );
        assert_eq!(sniff_format(&[0, 0, 1, 0, 1, 0]), Some(ImageFormat::Ico));
        assert_eq!(sniff_format(&[0, 0, 2, 0, 1, 0]), Some(ImageFormat::Ico));
        assert_eq!(sniff_format(b"BM\x1a\0"), Some(ImageFormat::Bmp));
    }

    #[test]
    fn near_misses_and_short_input_are_unknown() {
        assert_eq!(sniff_format(b""), None);
        assert_eq!(sniff_format(b"GIF90a"), None);
        assert_eq!(sniff_format(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(sniff_format(b"RIFF\x10\0\0\0WEB"), None);
        assert_eq!(
            sniff_format(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            None
        );
        assert_eq!(sniff_format(&PNG_SIGNATURE[..7]), None);
    }
}
