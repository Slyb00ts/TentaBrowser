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

/// Encodes `pix` as a JPEG at `quality` (1-100) over an opaque white
/// background, since the format has no alpha channel.
///
/// # Panics
///
/// Panics if `quality` is outside 1-100 or a side exceeds 65535 pixels; the
/// render session rejects both before it gets here.
pub fn encode_jpeg(pix: &Pixmap, quality: u8) -> Vec<u8> {
    assert!((1..=100).contains(&quality), "JPEG quality is 1 to 100");
    let (width, height) = (
        u16::try_from(pix.width()).expect("a JPEG side fits 16 bits"),
        u16::try_from(pix.height()).expect("a JPEG side fits 16 bits"),
    );
    let mut rgb = Vec::with_capacity(pix.data().len() / 4 * 3);
    for pixel in pix.data().as_chunks::<4>().0 {
        // Premultiplied over white: the missing coverage is white.
        let uncovered = 255 - pixel[3];
        rgb.extend(pixel[..3].iter().map(|&c| c.saturating_add(uncovered)));
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, quality)
        .encode(&rgb, width, height, jpeg_encoder::ColorType::Rgb)
        .expect("encoding a valid pixmap into memory cannot fail");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jpeg_flattens_transparency_onto_white() {
        let pix = Pixmap::new(16, 8).expect("a pixmap");
        let bytes = encode_jpeg(&pix, 90);
        assert_eq!(&bytes[..2], [0xff, 0xd8], "a JPEG start marker");
        let mut decoder =
            zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(&bytes));
        let rgb = decoder.decode().expect("a decodable JPEG");
        assert_eq!(decoder.dimensions(), Some((16, 8)));
        assert!(rgb.iter().all(|&c| c >= 253), "white, within codec error");
    }

    #[test]
    #[should_panic(expected = "JPEG quality")]
    fn a_quality_of_zero_is_a_caller_bug() {
        let _ = encode_jpeg(&Pixmap::new(1, 1).expect("a pixmap"), 0);
    }
}
