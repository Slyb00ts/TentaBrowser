//! Display lists, CPU and GPU raster, images, SVG, fonts and the glyph cache.
//!
//! The paint-value types (`Color`, `Rect`, `BorderStyle`, ...) are defined in
//! `tenta-layout`, because fragments carry them and layout cannot depend on this
//! crate; they are re-exported here. This crate defines only what layout never
//! mentions: the display list, rasterisation and images. Dependency direction
//! is enforced by `cargo xtask lint-deps` (`docs/spec/02-architecture.md` §2).

mod build;
mod dl;
mod encode;
mod glyph;
mod image;
mod raster;

use std::fmt;

pub use build::{DrawableImage, ImageBindings, NoBindings, PaintContext, build};
pub use dl::{DisplayList, Item, Radii};
pub use encode::{PngProfile, encode_jpeg, encode_png};
pub use image::{
    DecodeControl, DecodeError, DecodeLimits, DecodeParams, DecodedImage, ImageFormat, ImageId,
    ImageInfo, ImageQuality, ImageStore, StoreError, StoreKey, SvgNatural, decode, decode_format,
    decode_format_with_control, decode_svg, decode_with_control, sniff_format,
};
pub use raster::{Pixmap, RasterOptions, RasterOutput, RasterStats, rasterize};
pub use tenta_layout::types::{
    Affine, BackgroundContent, BackgroundLayer, BackgroundSize, BlendMode, BorderPaint,
    BorderStyle, BoxArea, Color, ColorStop, DecorationPaint, Edges, FilterOp, FontBlob, FontRef,
    Glyph, GlyphRun, GradientPaint, ImageKey, ImageRendering, LengthPercent, LineStyle, MarkerKind,
    MarkerPaint, ObjectFit, OutlinePaint, OverflowKind, PaintStyle, PositionKind, Rect, RepeatKind,
    ShadowPaint,
};

/// Why rasterisation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaintError {
    /// The requested output size is zero or too large to allocate.
    InvalidSize {
        /// Requested width in device pixels.
        width: u32,
        /// Requested height in device pixels.
        height: u32,
    },
}

impl fmt::Display for PaintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaintError::InvalidSize { width, height } => {
                write!(f, "cannot rasterise a {width}x{height} image")
            }
        }
    }
}

impl std::error::Error for PaintError {}

#[cfg(test)]
mod tests {
    use super::PaintError;

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(
            PaintError::InvalidSize {
                width: 0,
                height: 5
            }
            .to_string(),
            "cannot rasterise a 0x5 image"
        );
    }
}
