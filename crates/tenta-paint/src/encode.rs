//! PNG and JPEG encoding of rasterised pixmaps.

use crate::raster::Pixmap;

/// PNG speed/size trade-off.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum PngProfile {
    /// Fast compression, the default for screenshots.
    #[default]
    Fast,
    /// Smallest output.
    Best,
}

/// Encodes `pix` as a PNG, un-premultiplying once.
pub fn encode_png(pix: &Pixmap, profile: PngProfile) -> Vec<u8> {
    let _ = (pix, profile);
    todo!("P1")
}

/// Encodes `pix` as a JPEG at `quality` (1-100) over an opaque background.
pub fn encode_jpeg(pix: &Pixmap, quality: u8) -> Vec<u8> {
    let _ = (pix, quality);
    todo!("M3 JPEG encode")
}
