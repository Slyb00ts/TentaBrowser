//! Sample images shared by the decode conformance and mutation tests. Every
//! image is built here from first principles, so expectations are computed
//! independently of the decoders under test.

#![allow(dead_code)]

pub mod tracking;

use std::path::PathBuf;

use tenta_paint::{DecodeLimits, DecodeParams, DecodedImage, ImageFormat};

pub const W: u32 = 32;
pub const H: u32 = 24;

/// The straight-alpha gradient every encoder test starts from.
pub fn gradient(x: u32, y: u32) -> [u8; 4] {
    [
        (x * 8) as u8,
        (y * 10) as u8,
        ((x + y) * 4) as u8,
        (255 - x * 8) as u8,
    ]
}

/// CRC-32 (IEEE), as PNG chunks use it.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (!(crc & 1)).wrapping_add(1));
        }
    }
    !crc
}

pub fn gradient_rgba() -> Vec<u8> {
    (0..H)
        .flat_map(|y| (0..W).flat_map(move |x| gradient(x, y)))
        .collect()
}

pub fn gradient_rgb() -> Vec<u8> {
    gradient_rgba()
        .chunks(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect()
}

/// `round(c * a / 255)` for each colour channel.
pub fn premultiply(px: [u8; 4]) -> [u8; 4] {
    let a = u32::from(px[3]);
    let m = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
    [m(px[0]), m(px[1]), m(px[2]), px[3]]
}

pub fn premultiplied(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks(4)
        .flat_map(|p| premultiply([p[0], p[1], p[2], p[3]]))
        .collect()
}

/// FNV-1a over the dimensions and the pixels.
pub fn hash(image: &DecodedImage) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let dims = [image.width.to_le_bytes(), image.height.to_le_bytes()].concat();
    for &b in dims.iter().chain(image.rgba.iter()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn decoded(bytes: &[u8]) -> DecodedImage {
    tenta_paint::decode(bytes, &DecodeParams::default(), &DecodeLimits::default())
        .expect("sample decodes")
}

pub fn fixture(relative: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/images")
        .join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub struct PngSpec<'a> {
    pub width: u32,
    pub height: u32,
    pub color: png::ColorType,
    pub depth: png::BitDepth,
    pub palette: Option<&'a [u8]>,
    pub trns: Option<&'a [u8]>,
    pub gamma: Option<f32>,
}

pub fn encode_png(spec: &PngSpec, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, spec.width, spec.height);
    encoder.set_color(spec.color);
    encoder.set_depth(spec.depth);
    if let Some(p) = spec.palette {
        encoder.set_palette(p.to_vec());
    }
    if let Some(t) = spec.trns {
        encoder.set_trns(t.to_vec());
    }
    if let Some(g) = spec.gamma {
        encoder.set_source_gamma(png::ScaledFloat::new(g));
    }
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(data).expect("png data");
    writer.finish().expect("png finish");
    out
}

pub fn png_rgba8() -> Vec<u8> {
    encode_png(
        &PngSpec {
            width: W,
            height: H,
            color: png::ColorType::Rgba,
            depth: png::BitDepth::Eight,
            palette: None,
            trns: None,
            gamma: None,
        },
        &gradient_rgba(),
    )
}

pub fn jpeg(
    color: jpeg_encoder::ColorType,
    data: &[u8],
    configure: impl FnOnce(&mut jpeg_encoder::Encoder<&mut Vec<u8>>),
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, 92);
    configure(&mut encoder);
    encoder
        .encode(data, W as u16, H as u16, color)
        .expect("jpeg encode");
    out
}

/// A minimal EXIF block whose only entry is the orientation.
pub fn exif_orientation(value: u16) -> Vec<u8> {
    let mut e = b"MM\0*".to_vec();
    e.extend_from_slice(&8u32.to_be_bytes());
    e.extend_from_slice(&1u16.to_be_bytes());
    e.extend_from_slice(&0x0112u16.to_be_bytes());
    e.extend_from_slice(&3u16.to_be_bytes());
    e.extend_from_slice(&1u32.to_be_bytes());
    e.extend_from_slice(&value.to_be_bytes());
    e.extend_from_slice(&[0, 0]);
    e.extend_from_slice(&0u32.to_be_bytes());
    e
}

pub fn jpeg_rgb(configure: impl FnOnce(&mut jpeg_encoder::Encoder<&mut Vec<u8>>)) -> Vec<u8> {
    jpeg(jpeg_encoder::ColorType::Rgb, &gradient_rgb(), configure)
}

pub fn webp_lossless(alpha: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let (data, color) = if alpha {
        (gradient_rgba(), image_webp::ColorType::Rgba8)
    } else {
        (gradient_rgb(), image_webp::ColorType::Rgb8)
    };
    image_webp::WebPEncoder::new(&mut out)
        .encode(&data, W, H, color)
        .expect("webp encode");
    out
}

/// One GIF frame placed on the logical screen.
pub struct GifFrame {
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
    /// Palette indices, row-major.
    pub pixels: Vec<u8>,
    pub transparent: Option<u8>,
}

impl GifFrame {
    /// An opaque frame of palette indices at `(left, top)`.
    pub fn new(left: u16, top: u16, width: u16, height: u16, pixels: Vec<u8>) -> Self {
        Self {
            left,
            top,
            width,
            height,
            pixels,
            transparent: None,
        }
    }

    pub fn transparent(self, index: u8) -> Self {
        Self {
            transparent: Some(index),
            ..self
        }
    }
}

/// A GIF with a 3-colour global palette (red, blue, green) and the given frames.
pub fn gif(screen: (u16, u16), frames: &[GifFrame]) -> Vec<u8> {
    let mut out = Vec::new();
    let palette = [255, 0, 0, 0, 0, 255, 0, 255, 0];
    let mut encoder =
        gif::Encoder::new(&mut out, screen.0, screen.1, &palette).expect("gif header");
    for f in frames {
        let frame = gif::Frame {
            left: f.left,
            top: f.top,
            width: f.width,
            height: f.height,
            buffer: f.pixels.clone().into(),
            transparent: f.transparent,
            ..gif::Frame::default()
        };
        encoder.write_frame(&frame).expect("gif frame");
    }
    drop(encoder);
    out
}

/// A BMP with a 40-byte header. `rows` is top-down and already packed;
/// `height` is negative for a top-down file.
pub fn bmp(
    width: i32,
    height: i32,
    bpp: u16,
    compression: u32,
    masks: &[u32],
    palette: &[[u8; 3]],
    pixels: &[u8],
) -> Vec<u8> {
    let mut dib = Vec::new();
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&width.to_le_bytes());
    dib.extend_from_slice(&height.to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&bpp.to_le_bytes());
    dib.extend_from_slice(&compression.to_le_bytes());
    dib.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
    dib.extend_from_slice(&[0; 8]);
    dib.extend_from_slice(&(palette.len() as u32).to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    for m in masks {
        dib.extend_from_slice(&m.to_le_bytes());
    }
    for [r, g, b] in palette {
        dib.extend_from_slice(&[*b, *g, *r, 0]);
    }
    let offset = 14 + dib.len();
    let mut file = b"BM".to_vec();
    file.extend_from_slice(&((offset + pixels.len()) as u32).to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&(offset as u32).to_le_bytes());
    file.extend_from_slice(&dib);
    file.extend_from_slice(pixels);
    file
}

/// Packs `indices` (one row per entry, top-down) at `bpp` bits, 4-byte aligned,
/// stored bottom-up as a default BMP is.
pub fn pack_indices(rows: &[Vec<u8>], bpp: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for row in rows.iter().rev() {
        let mut bytes = vec![0u8; (row.len() * bpp).div_ceil(32) * 4];
        for (x, &index) in row.iter().enumerate() {
            let bit = x * bpp;
            bytes[bit / 8] |= index << (8 - bpp - bit % 8);
        }
        out.extend(bytes);
    }
    out
}

pub fn bmp_palette() -> Vec<[u8; 3]> {
    vec![
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [0, 255, 255],
        [10, 20, 30],
    ]
}

/// The entry of an icon directory.
pub struct IconEntry {
    pub width: u8,
    pub height: u8,
    pub bpp: u16,
    pub data: Vec<u8>,
}

pub fn ico(entries: &[IconEntry]) -> Vec<u8> {
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len();
    for e in entries {
        out.extend_from_slice(&[e.width, e.height, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&e.bpp.to_le_bytes());
        out.extend_from_slice(&(e.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += e.data.len();
    }
    for e in entries {
        out.extend_from_slice(&e.data);
    }
    out
}

/// A headerless 24- or 32-bit icon bitmap of `side x side` pixels whose
/// colour is `colour(x, y)` (top-down), followed by the 1-bit mask.
pub fn icon_dib(
    side: u32,
    bpp: u16,
    colour: impl Fn(u32, u32) -> [u8; 4],
    masked: impl Fn(u32, u32) -> bool,
) -> Vec<u8> {
    let mut dib = Vec::new();
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&(side as i32).to_le_bytes());
    dib.extend_from_slice(&(side as i32 * 2).to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&bpp.to_le_bytes());
    dib.extend_from_slice(&[0; 24]);
    let bytes = usize::from(bpp / 8);
    for y in (0..side).rev() {
        let mut row = Vec::new();
        for x in 0..side {
            let [r, g, b, a] = colour(x, y);
            row.extend_from_slice(&[b, g, r, a][..bytes]);
        }
        row.resize((side as usize * usize::from(bpp)).div_ceil(32) * 4, 0);
        dib.extend(row);
    }
    let stride = (side as usize).div_ceil(32) * 4;
    for y in (0..side).rev() {
        let mut row = vec![0u8; stride];
        for x in 0..side {
            if masked(x, y) {
                row[x as usize / 8] |= 0x80 >> (x % 8);
            }
        }
        dib.extend(row);
    }
    dib
}

/// Every format at least once, for the mutation test.
pub fn corpus() -> Vec<(ImageFormat, Vec<u8>)> {
    let mut corpus = vec![
        (ImageFormat::Png, png_rgba8()),
        (ImageFormat::Jpeg, jpeg_rgb(|_| {})),
        (ImageFormat::Jpeg, jpeg_rgb(|e| e.set_progressive(true))),
        (ImageFormat::WebP, webp_lossless(true)),
        (ImageFormat::WebP, fixture("lossy.webp")),
        (ImageFormat::WebP, fixture("lossy-alpha.webp")),
        (ImageFormat::WebP, fixture("animated.webp")),
        (
            ImageFormat::Gif,
            gif(
                (8, 6),
                &[
                    GifFrame::new(2, 1, 4, 3, vec![0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2])
                        .transparent(2),
                ],
            ),
        ),
    ];
    let rows: Vec<Vec<u8>> = (0..3u8)
        .map(|y| (0..5).map(|x| (x + y) % 4).collect())
        .collect();
    corpus.push((
        ImageFormat::Bmp,
        bmp(5, 3, 4, 0, &[], &bmp_palette(), &pack_indices(&rows, 4)),
    ));
    corpus.push((
        ImageFormat::Bmp,
        bmp(
            5,
            3,
            8,
            1,
            &[],
            &bmp_palette(),
            &[5, 1, 0, 0, 3, 0, 1, 2, 3, 0, 0, 2, 4, 0, 1],
        ),
    ));
    let dib = icon_dib(
        8,
        32,
        |x, y| [x as u8 * 30, y as u8 * 30, 9, 200],
        |_, _| false,
    );
    corpus.push((
        ImageFormat::Ico,
        ico(&[
            IconEntry {
                width: 8,
                height: 8,
                bpp: 32,
                data: dib,
            },
            IconEntry {
                width: 32,
                height: 24,
                bpp: 32,
                data: png_rgba8(),
            },
        ]),
    ));
    corpus
}
