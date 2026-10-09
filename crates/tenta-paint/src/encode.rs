//! PNG and JPEG encoding of rasterised pixmaps.

use std::io::Write;

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
    write_png(pix, profile).expect("encoding a valid pixmap into memory cannot fail")
}

fn write_png(pix: &Pixmap, profile: PngProfile) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, pix.width(), pix.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    match profile {
        PngProfile::Fast => {
            encoder.set_compression(png::Compression::Fast);
            encoder.set_filter(png::Filter::Sub);
        }
        PngProfile::Best => {
            encoder.set_compression(png::Compression::High);
            encoder.set_filter(png::Filter::Adaptive);
        }
    }
    let mut writer = encoder.write_header()?;
    // Rows go through one reused buffer, so the straight-alpha copy of the
    // image never exists as a whole.
    let mut stream = writer.stream_writer()?;
    let mut row = vec![0; pix.width() as usize * 4];
    for premultiplied in pix.data().chunks_exact(row.len()) {
        unpremultiply_row(premultiplied, &mut row);
        stream.write_all(&row)?;
    }
    stream.finish()?;
    writer.finish()?;
    Ok(out)
}

/// Straight RGBA8 from premultiplied RGBA8. Fully transparent pixels become
/// zero, since their colour is unrecoverable.
fn unpremultiply_row(premultiplied: &[u8], straight: &mut [u8]) {
    let (source, _) = premultiplied.as_chunks::<4>();
    let (target, _) = straight.as_chunks_mut::<4>();
    for (pixel, out) in source.iter().zip(target) {
        *out = match pixel[3] {
            0 => [0; 4],
            255 => *pixel,
            alpha => {
                let alpha = u32::from(alpha);
                let channel = |c: u8| ((u32::from(c) * 255 + alpha / 2) / alpha).min(255) as u8;
                [
                    channel(pixel[0]),
                    channel(pixel[1]),
                    channel(pixel[2]),
                    pixel[3],
                ]
            }
        };
    }
}

/// Encodes `pix` as a JPEG at `quality` (1-100) over an opaque background.
pub fn encode_jpeg(pix: &Pixmap, quality: u8) -> Vec<u8> {
    let _ = (pix, quality);
    todo!("M3 JPEG encode")
}
