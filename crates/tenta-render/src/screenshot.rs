//! Screenshot request and result types.

use tenta_core::NodeId;
use tenta_layout::Rect;
use tenta_paint::PaintError;

/// Output encoding.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPEG at a quality of 1 to 100.
    Jpeg {
        /// Quality.
        quality: u8,
    },
    /// WebP (lossless).
    Webp,
}

/// What part of the page to capture.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Capture {
    /// The current viewport.
    Viewport,
    /// The whole scrollable document.
    FullPage,
    /// A rectangle in document coordinates.
    Clip(Rect),
    /// The border box of an element.
    Element(NodeId),
}

/// Options of one screenshot.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScreenshotOptions {
    /// Encoding.
    pub format: ImageFormat,
    /// Area.
    pub capture: Capture,
    /// Device scale override; `None` uses the viewport's.
    pub scale: Option<f32>,
    /// Leave the page background transparent.
    pub omit_background: bool,
}

impl Default for ScreenshotOptions {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            capture: Capture::Viewport,
            scale: None,
            omit_background: false,
        }
    }
}

/// An encoded screenshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screenshot {
    /// Encoded bytes.
    pub bytes: Vec<u8>,
    /// Encoding of `bytes`.
    pub format: ImageFormat,
    /// Width in device pixels.
    pub width: u32,
    /// Height in device pixels.
    pub height: u32,
}

/// Why a render failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The capture exceeds the pixel or memory cap.
    TooLarge {
        /// Requested width in device pixels.
        width: u32,
        /// Requested height in device pixels.
        height: u32,
    },
    /// The document has no root element to render.
    NoRoot,
    /// Rasterisation failed.
    Paint(PaintError),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::TooLarge { width, height } => {
                write!(f, "a {width}x{height} capture exceeds the render limits")
            }
            RenderError::NoRoot => f.write_str("the document has no root element"),
            RenderError::Paint(err) => write!(f, "paint failed: {err}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RenderError::Paint(err) => Some(err),
            _ => None,
        }
    }
}

impl From<PaintError> for RenderError {
    fn from(err: PaintError) -> Self {
        RenderError::Paint(err)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn paint_errors_convert_and_chain() {
        let err: RenderError = PaintError::Unsupported("Image").into();
        assert!(err.source().is_some());
        assert!(err.to_string().contains("unsupported display item"));
        assert!(RenderError::NoRoot.source().is_none());
    }

    #[test]
    fn default_options_capture_the_viewport_as_png() {
        let opts = ScreenshotOptions::default();
        assert_eq!(opts.format, ImageFormat::Png);
        assert_eq!(opts.capture, Capture::Viewport);
        assert!(!opts.omit_background);
    }
}
