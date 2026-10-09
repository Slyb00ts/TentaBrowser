//! Image decoding and the decoded-image store.
//!
//! Every decoder produces straight RGBA8; `decode` then bounds it by the
//! [`DecodeLimits`], applies EXIF orientation, shrinks it to the requested
//! size and premultiplies it, which is what `tiny-skia` composites.
//!
//! Memory is bounded by the **peak** of a decode, not by its output: every
//! decoder estimates what it holds at its largest (see `Ctx::admit`) and
//! refuses images that do not fit `DecodeLimits::max_transient_bytes`, while the
//! retained image must fit `max_decoded_bytes`. A decode
//! can also be stopped by a deadline or a cancel flag ([`DecodeControl`]).
//!
//! Images are kept as one contiguous pixmap rather than 256x256 tiles, and a
//! GIF or animated WebP yields its first frame only (D-035). The raster path
//! draws an image through a single `tiny-skia` pattern shader, so tiles would
//! turn one draw into one draw per tile and need seam handling for bilinear
//! sampling at every tile edge. Decode-time shrinking to the largest drawn size
//! already bounds memory, which is what tiling is for; revisit it when the GPU
//! path needs tile-granular upload.

mod bmp;
mod ctx;
mod decode;
mod error;
mod format;
mod gif;
mod ico;
mod jpeg;
mod limits;
mod orient;
mod pixels;
mod png;
mod resize;
mod store;
mod svg;
mod webp;

use std::sync::Arc;

pub use ctx::DecodeControl;
pub use decode::{decode, decode_format, decode_format_with_control, decode_with_control};
pub use error::DecodeError;
pub use format::{ImageFormat, sniff_format};
pub use limits::{DecodeLimits, DecodeParams};
pub use store::{ImageInfo, ImageStore, StoreError, StoreKey};
pub use svg::{SvgNatural, decode_svg};

/// Handle to an image in an [`ImageStore`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct ImageId(pub u32);

/// Resampling quality for a drawn image.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum ImageQuality {
    /// Smooth resampling.
    #[default]
    Smooth,
    /// Nearest neighbour.
    Pixelated,
}

/// A decoded image: premultiplied RGBA8, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Arc<[u8]>,
    /// Width of the image as stored in the file, after orientation; the
    /// pixels above may be a shrunken copy of it.
    pub source_width: u32,
    /// Height of the image as stored in the file, after orientation.
    pub source_height: u32,
}
