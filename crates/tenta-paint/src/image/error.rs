//! Typed decode failures.

use std::fmt;

use super::format::ImageFormat;

/// Why a byte stream could not become an image.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The leading bytes match no supported format.
    UnknownFormat,
    /// The data is damaged or violates the format.
    Malformed {
        /// Format being decoded.
        format: ImageFormat,
        /// What went wrong, as reported by the decoder.
        detail: String,
    },
    /// The data is valid but uses a feature this engine does not decode.
    Unsupported {
        /// Format being decoded.
        format: ImageFormat,
        /// The refused feature.
        feature: &'static str,
    },
    /// The image exceeds a decode limit.
    TooLarge {
        /// Source width in pixels.
        width: u32,
        /// Source height in pixels.
        height: u32,
    },
    /// The decode deadline passed before the image was complete.
    TimedOut,
    /// The caller's cancel flag was raised before the image was complete.
    Cancelled,
    /// A buffer the decode needs could not be allocated.
    OutOfMemory,
    /// A third-party decoder panicked; the panic was contained.
    Panicked(ImageFormat),
}

impl DecodeError {
    pub(super) fn malformed(format: ImageFormat, detail: impl fmt::Display) -> Self {
        Self::Malformed {
            format,
            detail: detail.to_string(),
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFormat => f.write_str("unrecognised image format"),
            Self::Malformed { format, detail } => write!(f, "malformed {format} image: {detail}"),
            Self::Unsupported { format, feature } => {
                write!(f, "unsupported {format} feature: {feature}")
            }
            Self::TooLarge { width, height } => {
                write!(f, "a {width}x{height} image exceeds the decode limits")
            }
            Self::TimedOut => f.write_str("the image decode deadline passed"),
            Self::Cancelled => f.write_str("the image decode was cancelled"),
            Self::OutOfMemory => f.write_str("out of memory while decoding an image"),
            Self::Panicked(format) => write!(f, "the {format} decoder panicked"),
        }
    }
}

impl std::error::Error for DecodeError {}
