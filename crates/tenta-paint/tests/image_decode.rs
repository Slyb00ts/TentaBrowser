//! Decode conformance: PngSuite files by pixel hash, generated images against
//! independently computed pixels, and the limits and downscale contract.

mod support;

use support::*;
use tenta_paint::{DecodeError, DecodeLimits, DecodeParams, DecodedImage, ImageFormat};

fn px(image: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width + x) * 4) as usize;
    [
        image.rgba[at],
        image.rgba[at + 1],
        image.rgba[at + 2],
        image.rgba[at + 3],
    ]
}

fn pixels(image: &DecodedImage) -> Vec<[u8; 4]> {
    (0..image.height)
        .flat_map(|y| (0..image.width).map(move |x| (x, y)))
        .map(|(x, y)| px(image, x, y))
        .collect()
}

fn max_channel_diff(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0)
}

fn decode_with(
    bytes: &[u8],
    params: DecodeParams,
    limits: DecodeLimits,
) -> Result<DecodedImage, DecodeError> {
    tenta_paint::decode(bytes, &params, &limits)
}

// PNG ----------------------------------------------------------------------

/// Pixel hashes of the vendored PngSuite files. Files that depict the same
/// picture share a hash, which is itself part of the check.
const PNGSUITE_HASHES: &[(&str, u64)] = &[
    ("basn0g01", 0xbca65a8897f66bfd),
    ("basn0g02", 0xde031ad5e95f9fc5),
    ("basn0g04", 0xb2923940470fc9c5),
    ("basn0g08", 0x909747bdc68aefed),
    ("basn0g16", 0xfd5028c3679d815c),
    ("basn2c08", 0xeba2c1da252933c5),
    ("basn2c16", 0x97b41e996768d113),
    ("basn3p01", 0xad7fc3827a48dbc5),
    ("basn3p02", 0x22f6431cf9a97fc5),
    ("basn3p04", 0xd081f929144d3de5),
    ("basn3p08", 0x77599a0032eb9fc5),
    ("basn4a08", 0x111b3d14ce52e547),
    ("basn4a16", 0x63205e6ccfcd28e5),
    ("basn6a08", 0xb9309fb67361dd40),
    ("basn6a16", 0x80ecfee1f97be4f7),
    ("bgai4a08", 0x111b3d14ce52e547),
    ("ccwn2c08", 0xac342547550df7b8),
    ("cdfn2c08", 0x8a0624d990cd4869),
    ("cm0n0g04", 0x4a97101d4a5e2a2c),
    ("ctzn0g04", 0x4a97101d4a5e2a2c),
    ("exif2c08", 0xeec14cb9b73a68a9),
    ("f00n2c08", 0xc342a11c249e2922),
    ("f01n2c08", 0x209b9e16f52b579a),
    ("f02n2c08", 0xe3c5078b98dcb86c),
    ("f03n2c08", 0x67d1d8ac1eb67a4c),
    ("f04n2c08", 0x8bee6238aee60e26),
    ("f99n0g04", 0x27aadab6bdc90715),
    ("g10n3p04", 0x0255b0ba8c9d2426),
    ("g25n2c08", 0x9f6979e02d6f0d7e),
    ("oi2n0g16", 0xfd5028c3679d815c),
    ("pp0n2c16", 0x97b41e996768d113),
    ("pp0n6a08", 0x311aeb96657e161d),
    ("ps2n0g08", 0x909747bdc68aefed),
    ("s01i3p01", 0x7e2f2542cd3e897b),
    ("s02n3p01", 0x1ddb1c53ba7695c5),
    ("s07i3p02", 0x06e4a8f2e85e5358),
    ("s33n3p04", 0x5fe7478579b7216b),
    ("s39i3p04", 0xad9f79032fe1bf97),
    ("tbbn2c16", 0xc74a84d32964d24d),
    ("tbbn3p08", 0xd1730e0537fdd597),
    ("tbgn3p08", 0xd1730e0537fdd597),
    ("tbrn2c08", 0xc74a84d32964d24d),
    ("tbwn0g16", 0x8cc22eddd48811bc),
    ("tp0n3p08", 0x72fe4332b05acd9f),
    ("tp1n3p08", 0xd1730e0537fdd597),
    ("z00n2c08", 0x97b41e996768d113),
    ("z09n2c08", 0x97b41e996768d113),
];

fn suite(name: &str) -> DecodedImage {
    decoded(&fixture(&format!("pngsuite/{name}.png")))
}

#[test]
fn pngsuite_pixels_match_pinned_hashes() {
    for &(name, expected) in PNGSUITE_HASHES {
        let image = suite(name);
        assert_eq!(hash(&image), expected, "{name}");
        assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
    }
}

#[test]
fn pngsuite_interlaced_decodes_like_plain() {
    for kind in [
        "0g01", "0g02", "0g04", "0g08", "0g16", "2c08", "2c16", "3p01", "3p02", "3p04", "3p08",
        "4a08", "4a16", "6a08", "6a16",
    ] {
        assert_eq!(
            suite(&format!("basi{kind}")),
            suite(&format!("basn{kind}")),
            "{kind}"
        );
    }
}

#[test]
fn pngsuite_transparency_becomes_alpha() {
    let has_alpha = |image: &DecodedImage| {
        let a: Vec<u8> = image.rgba.chunks(4).map(|p| p[3]).collect();
        (
            a.contains(&0),
            a.iter().any(|&v| v != 0 && v != 255),
            a.contains(&255),
        )
    };
    // tRNS colour key: some pixels fully transparent, the rest opaque.
    assert_eq!(has_alpha(&suite("tbrn2c08")), (true, false, true));
    assert_eq!(has_alpha(&suite("tbwn0g16")), (true, false, true));
    // Palette tRNS: one transparent entry, and several graded ones.
    assert_eq!(has_alpha(&suite("tbbn3p08")), (true, false, true));
    assert_eq!(has_alpha(&suite("tm3n3p02")), (true, true, true));
    // Alpha channels with graded values.
    assert_eq!(has_alpha(&suite("basn6a16")), (true, true, false));
    assert_eq!(has_alpha(&suite("basn4a08")), (true, true, true));
    // No alpha anywhere.
    assert_eq!(has_alpha(&suite("basn2c16")), (false, false, true));
    assert_eq!(has_alpha(&suite("basn3p08")), (false, false, true));
}

#[test]
fn pngsuite_corrupt_files_are_typed_errors() {
    for name in [
        "xc1n0g08", "xcsn0g01", "xd0n2c08", "xd3n2c08", "xd9n2c08", "xdtn0g01", "xhdn0g08",
        "xcrn0g04", "xlfn0g04", "xs1n0g01", "xs2n0g01", "xs4n0g01", "xs7n0g01",
    ] {
        let bytes = fixture(&format!("pngsuite/{name}.png"));
        let result = decode_with(&bytes, DecodeParams::default(), DecodeLimits::default());
        assert!(
            matches!(
                result,
                Err(DecodeError::UnknownFormat | DecodeError::Malformed { .. })
            ),
            "{name}: {result:?}"
        );
        let forced = tenta_paint::decode_format(
            ImageFormat::Png,
            &bytes,
            &DecodeParams::default(),
            &DecodeLimits::default(),
        );
        assert!(
            matches!(forced, Err(DecodeError::Malformed { .. })),
            "{name} forced: {forced:?}"
        );
    }
}

fn spec(color: png::ColorType, depth: png::BitDepth) -> PngSpec<'static> {
    PngSpec {
        width: 4,
        height: 2,
        color,
        depth,
        palette: None,
        trns: None,
        gamma: None,
    }
}

/// `round(v / 257)`, the exact 16- to 8-bit reduction.
fn narrow(v: u16) -> u8 {
    (f64::from(v) / 257.0).round() as u8
}

#[test]
fn png_color_types_and_depths_decode_exactly() {
    use png::{BitDepth::*, ColorType::*};
    let gray8 = [0u8, 1, 127, 255, 10, 20, 30, 40];
    let image = decoded(&encode_png(&spec(Grayscale, Eight), &gray8));
    let expected: Vec<[u8; 4]> = gray8.iter().map(|&g| [g, g, g, 255]).collect();
    assert_eq!(pixels(&image), expected);

    let gray16: [u16; 8] = [0, 1, 128, 0x1234, 0x7fff, 0x8000, 0xfffe, 0xffff];
    let bytes: Vec<u8> = gray16.iter().flat_map(|v| v.to_be_bytes()).collect();
    let image = decoded(&encode_png(&spec(Grayscale, Sixteen), &bytes));
    let expected: Vec<[u8; 4]> = gray16
        .iter()
        .map(|&v| [narrow(v); 3])
        .map(|[g, _, _]| [g, g, g, 255])
        .collect();
    assert_eq!(pixels(&image), expected);

    // Sub-byte grays widen to the full range.
    for (depth, bits, max) in [(One, 1u8, 1u8), (Two, 2, 3), (Four, 4, 15)] {
        let values: Vec<u8> = (0..8).map(|i| i % (max + 1)).collect();
        // PNG rows are MSB-first and padded to a whole byte.
        let mut packed = Vec::new();
        for row in values.chunks(4) {
            let mut bytes = vec![0u8; (4 * usize::from(bits)).div_ceil(8)];
            for (i, &v) in row.iter().enumerate() {
                let bit = i * usize::from(bits);
                bytes[bit / 8] |= v << (8 - usize::from(bits) - bit % 8);
            }
            packed.extend(bytes);
        }
        let image = decoded(&encode_png(&spec(Grayscale, depth), &packed));
        let expected: Vec<[u8; 4]> = values
            .iter()
            .map(|&v| {
                let g = (u32::from(v) * 255 / u32::from(max)) as u8;
                [g, g, g, 255]
            })
            .collect();
        assert_eq!(pixels(&image), expected, "gray {bits}-bit");
    }

    let rgb: Vec<u8> = (0..8u8).flat_map(|i| [i * 30, 255 - i * 30, i]).collect();
    let image = decoded(&encode_png(&spec(Rgb, Eight), &rgb));
    let expected: Vec<[u8; 4]> = rgb.chunks(3).map(|p| [p[0], p[1], p[2], 255]).collect();
    assert_eq!(pixels(&image), expected);

    let rgb16: Vec<u16> = (0..8u16)
        .flat_map(|i| [i * 8000, 65535 - i * 9000, 0x00ff * i])
        .collect();
    let bytes: Vec<u8> = rgb16.iter().flat_map(|v| v.to_be_bytes()).collect();
    let image = decoded(&encode_png(&spec(Rgb, Sixteen), &bytes));
    let expected: Vec<[u8; 4]> = rgb16
        .chunks(3)
        .map(|p| [narrow(p[0]), narrow(p[1]), narrow(p[2]), 255])
        .collect();
    assert_eq!(pixels(&image), expected);

    let ga: Vec<u8> = (0..8u8).flat_map(|i| [i * 30, 255 - i * 36]).collect();
    let image = decoded(&encode_png(&spec(GrayscaleAlpha, Eight), &ga));
    let expected: Vec<[u8; 4]> = ga
        .chunks(2)
        .map(|p| premultiply([p[0], p[0], p[0], p[1]]))
        .collect();
    assert_eq!(pixels(&image), expected);

    let ga16: Vec<u16> = (0..8u16)
        .flat_map(|i| [i * 9000, 65535 - i * 8000])
        .collect();
    let bytes: Vec<u8> = ga16.iter().flat_map(|v| v.to_be_bytes()).collect();
    let image = decoded(&encode_png(&spec(GrayscaleAlpha, Sixteen), &bytes));
    let expected: Vec<[u8; 4]> = ga16
        .chunks(2)
        .map(|p| premultiply([narrow(p[0]), narrow(p[0]), narrow(p[0]), narrow(p[1])]))
        .collect();
    assert_eq!(pixels(&image), expected);

    let rgba: Vec<u8> = (0..8u8)
        .flat_map(|i| [i * 30, 200, 255 - i * 30, i * 36])
        .collect();
    let image = decoded(&encode_png(&spec(Rgba, Eight), &rgba));
    let expected: Vec<[u8; 4]> = rgba
        .chunks(4)
        .map(|p| premultiply([p[0], p[1], p[2], p[3]]))
        .collect();
    assert_eq!(pixels(&image), expected);

    let rgba16: Vec<u16> = (0..8u16)
        .flat_map(|i| [i * 8000, 40000, 65535 - i * 8000, i * 9000])
        .collect();
    let bytes: Vec<u8> = rgba16.iter().flat_map(|v| v.to_be_bytes()).collect();
    let image = decoded(&encode_png(&spec(Rgba, Sixteen), &bytes));
    let expected: Vec<[u8; 4]> = rgba16
        .chunks(4)
        .map(|p| premultiply([narrow(p[0]), narrow(p[1]), narrow(p[2]), narrow(p[3])]))
        .collect();
    assert_eq!(pixels(&image), expected);
}

#[test]
fn png_palette_and_transparency_chunks_apply() {
    use png::{BitDepth::*, ColorType::*};
    let palette = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0];
    // The tRNS chunk is shorter than the palette: the rest stays opaque.
    let image = decoded(&encode_png(
        &PngSpec {
            palette: Some(&palette),
            trns: Some(&[0, 128]),
            ..spec(Indexed, Two)
        },
        &[0b00_01_10_11, 0b11_10_01_00],
    ));
    assert_eq!(
        pixels(&image),
        [
            [0, 0, 0, 0],
            premultiply([0, 255, 0, 128]),
            [0, 0, 255, 255],
            [255, 255, 0, 255],
            [255, 255, 0, 255],
            [0, 0, 255, 255],
            premultiply([0, 255, 0, 128]),
            [0, 0, 0, 0],
        ]
    );

    // tRNS colour key on gray 8-bit and on RGB 8-bit.
    let image = decoded(&encode_png(
        &PngSpec {
            trns: Some(&[0, 77]),
            ..spec(Grayscale, Eight)
        },
        &[77, 78, 77, 0, 1, 2, 3, 77],
    ));
    let alphas: Vec<u8> = pixels(&image).iter().map(|p| p[3]).collect();
    assert_eq!(alphas, [0, 255, 0, 255, 255, 255, 255, 0]);

    let rgb = [
        1, 2, 3, 1, 2, 4, 1, 2, 3, 9, 9, 9, 1, 2, 3, 0, 0, 0, 1, 2, 3, 1, 2, 3,
    ];
    let image = decoded(&encode_png(
        &PngSpec {
            trns: Some(&[0, 1, 0, 2, 0, 3]),
            ..spec(Rgb, Eight)
        },
        &rgb,
    ));
    let alphas: Vec<u8> = pixels(&image).iter().map(|p| p[3]).collect();
    assert_eq!(alphas, [0, 255, 0, 255, 0, 255, 0, 0]);
}

#[test]
fn png_gamma_chunk_does_not_change_pixels() {
    use png::{BitDepth::Eight, ColorType::Rgb};
    let data: Vec<u8> = (0..24).map(|i| i * 10).collect();
    let plain = decoded(&encode_png(&spec(Rgb, Eight), &data));
    for gamma in [0.2, 1.0, 2.2, 5.0] {
        let tagged = decoded(&encode_png(
            &PngSpec {
                gamma: Some(gamma),
                ..spec(Rgb, Eight)
            },
            &data,
        ));
        assert_eq!(tagged, plain, "gamma {gamma}");
    }
}

// JPEG ---------------------------------------------------------------------

#[test]
fn jpeg_baseline_and_progressive_agree_and_track_the_source() {
    let baseline = decoded(&jpeg_rgb(|_| {}));
    let progressive = decoded(&jpeg_rgb(|e| e.set_progressive(true)));
    assert_eq!((baseline.width, baseline.height), (W, H));
    assert_eq!(baseline, progressive, "same coefficients, same pixels");
    let expected: Vec<u8> = gradient_rgb()
        .chunks(3)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    assert!(max_channel_diff(&baseline.rgba, &expected) <= 12);
    assert_eq!(hash(&baseline), JPEG_GRADIENT_HASH);
}

/// Within 2 levels of libjpeg-turbo's output for the same file.
const JPEG_GRADIENT_HASH: u64 = 0x3f4b8f53ec811fdd;

#[test]
fn jpeg_exif_orientation_is_applied() {
    let upright = decoded(&jpeg_rgb(|_| {}));
    for orientation in 1u16..=8 {
        let image = decoded(&jpeg_rgb(|e| {
            e.add_exif_metadata(&exif_orientation(orientation))
                .expect("exif");
        }));
        let (w, h) = (upright.width, upright.height);
        let swapped = (5..=8).contains(&orientation);
        let (ow, oh) = if swapped { (h, w) } else { (w, h) };
        assert_eq!(
            (image.width, image.height),
            (ow, oh),
            "orientation {orientation}"
        );
        // EXIF tables: where the stored pixel (sx, sy) ends up.
        for sy in 0..h {
            for sx in 0..w {
                let (dx, dy) = match orientation {
                    1 => (sx, sy),
                    2 => (w - 1 - sx, sy),
                    3 => (w - 1 - sx, h - 1 - sy),
                    4 => (sx, h - 1 - sy),
                    5 => (sy, sx),
                    6 => (h - 1 - sy, sx),
                    7 => (h - 1 - sy, w - 1 - sx),
                    _ => (sy, w - 1 - sx),
                };
                assert_eq!(
                    px(&image, dx, dy),
                    px(&upright, sx, sy),
                    "orientation {orientation} at {sx},{sy}"
                );
            }
        }
        assert_eq!((image.source_width, image.source_height), (ow, oh));
    }
}

#[test]
fn jpeg_orientation_out_of_range_is_upright() {
    let upright = decoded(&jpeg_rgb(|_| {}));
    for value in [0u16, 9, 65535] {
        let image = decoded(&jpeg_rgb(|e| {
            e.add_exif_metadata(&exif_orientation(value)).expect("exif");
        }));
        assert_eq!(image, upright, "value {value}");
    }
}

#[test]
fn jpeg_grayscale_decodes_to_gray() {
    let luma: Vec<u8> = (0..H)
        .flat_map(|y| (0..W).map(move |x| (x * 4 + y * 3) as u8))
        .collect();
    let image = decoded(&jpeg(jpeg_encoder::ColorType::Luma, &luma, |_| {}));
    for (p, &l) in image.rgba.chunks(4).zip(&luma) {
        assert_eq!((p[0], p[1], p[3]), (p[2], p[2], 255));
        assert!(p[0].abs_diff(l) <= 6);
    }
}

#[test]
fn jpeg_cmyk_and_ycck_are_refused_with_a_typed_error() {
    let data = vec![40u8; (W * H * 4) as usize];
    for color in [jpeg_encoder::ColorType::Cmyk, jpeg_encoder::ColorType::Ycck] {
        let bytes = jpeg(color, &data, |_| {});
        let result = decode_with(&bytes, DecodeParams::default(), DecodeLimits::default());
        assert!(
            matches!(
                result,
                Err(DecodeError::Unsupported {
                    format: ImageFormat::Jpeg,
                    ..
                })
            ),
            "{result:?}"
        );
    }
}

#[test]
fn jpeg_truncation_is_an_error_not_a_panic() {
    let bytes = jpeg_rgb(|e| e.set_progressive(true));
    for cut in [2, 20, 100, bytes.len() / 2] {
        let result = decode_with(
            &bytes[..cut],
            DecodeParams::default(),
            DecodeLimits::default(),
        );
        assert!(
            !matches!(result, Err(DecodeError::Panicked(_))),
            "cut {cut}: {result:?}"
        );
    }
}

// WebP ---------------------------------------------------------------------

#[test]
fn webp_lossless_is_exact() {
    let image = decoded(&webp_lossless(true));
    assert_eq!(
        image.rgba.as_ref(),
        premultiplied(&gradient_rgba()).as_slice()
    );
    let opaque = decoded(&webp_lossless(false));
    let expected: Vec<u8> = gradient_rgb()
        .chunks(3)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    assert_eq!(opaque.rgba.as_ref(), expected.as_slice());
}

#[test]
fn webp_lossy_tracks_the_source() {
    let image = decoded(&fixture("lossy.webp"));
    assert_eq!((image.width, image.height), (W, H));
    let expected: Vec<u8> = gradient_rgb()
        .chunks(3)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    assert!(max_channel_diff(&image.rgba, &expected) <= 14);
    assert_eq!(hash(&image), WEBP_LOSSY_HASH);
}

/// Bit-identical to libwebp's `dwebp` output for the same file.
const WEBP_LOSSY_HASH: u64 = 0xb29f26644ec5725a;
/// Bit-identical to libwebp's `dwebp` output for the same file, premultiplied.
const WEBP_LOSSY_ALPHA_HASH: u64 = 0x49d896ba11b2dfc0;

#[test]
fn webp_lossy_with_alpha_keeps_the_alpha_plane() {
    let image = decoded(&fixture("lossy-alpha.webp"));
    assert_eq!((image.width, image.height), (W, H));
    let expected = premultiplied(&gradient_rgba());
    for (got, want) in image.rgba.chunks(4).zip(expected.chunks(4)) {
        assert!(got[3].abs_diff(want[3]) <= 1, "alpha {got:?} vs {want:?}");
    }
    assert!(max_channel_diff(&image.rgba, &expected) <= 20);
    assert_eq!(hash(&image), WEBP_LOSSY_ALPHA_HASH);
}

#[test]
fn webp_animation_yields_the_first_frame() {
    let image = decoded(&fixture("animated.webp"));
    assert_eq!((image.width, image.height), (W, H));
    let red = [200u8, 20, 20, 255];
    for p in pixels(&image) {
        assert!(
            p[..3].iter().zip(&red).all(|(a, b)| a.abs_diff(*b) <= 14) && p[3] == 255,
            "{p:?}"
        );
    }
}

// GIF ----------------------------------------------------------------------

#[test]
fn gif_first_frame_is_composed_on_a_transparent_screen() {
    // Palette: 0 red, 1 blue, 2 green. Index 2 is transparent in the frame.
    let first = vec![0, 1, 2, 2, 1, 0];
    let second = vec![1; 6];
    let bytes = gif(
        (5, 4),
        &[
            GifFrame::new(1, 1, 3, 2, first).transparent(2),
            GifFrame::new(0, 0, 5, 4, vec![1; 20]),
            GifFrame::new(0, 0, 3, 2, second),
        ],
    );
    let image = decoded(&bytes);
    assert_eq!((image.width, image.height), (5, 4));
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 255];
    let clear = [0, 0, 0, 0];
    let mut expected = vec![clear; 20];
    expected[5 + 1] = red;
    expected[5 + 2] = blue;
    expected[2 * 5 + 2] = blue;
    expected[2 * 5 + 3] = red;
    assert_eq!(pixels(&image), expected);
}

#[test]
fn gif_frame_overhanging_the_screen_is_clipped() {
    let bytes = gif((3, 2), &[GifFrame::new(2, 1, 3, 2, vec![0; 6])]);
    let image = decoded(&bytes);
    assert_eq!((image.width, image.height), (3, 2));
    let red = [255, 0, 0, 255];
    assert_eq!(px(&image, 2, 1), red);
    assert_eq!(px(&image, 1, 1), [0, 0, 0, 0]);
}

// BMP ----------------------------------------------------------------------

fn palette_rgba(indices: &[Vec<u8>]) -> Vec<[u8; 4]> {
    let palette = bmp_palette();
    indices
        .iter()
        .flatten()
        .map(|&i| {
            let [r, g, b] = palette[usize::from(i)];
            [r, g, b, 255]
        })
        .collect()
}

#[test]
fn bmp_indexed_depths_decode_bottom_up() {
    for (bpp, colours) in [(1usize, 2u8), (4, 6), (8, 6)] {
        let rows: Vec<Vec<u8>> = (0..3u8)
            .map(|y| (0..5u8).map(|x| (x + 2 * y) % colours).collect())
            .collect();
        let file = bmp(
            5,
            3,
            bpp as u16,
            0,
            &[],
            &bmp_palette(),
            &pack_indices(&rows, bpp),
        );
        let image = decoded(&file);
        assert_eq!((image.width, image.height), (5, 3));
        assert_eq!(pixels(&image), palette_rgba(&rows), "{bpp} bpp");
    }
}

#[test]
fn bmp_top_down_flag_is_honoured() {
    let rows: Vec<Vec<u8>> = vec![vec![0, 1, 2], vec![3, 4, 5]];
    // Top-down files store the first row first.
    let mut data = Vec::new();
    for row in &rows {
        let mut bytes = row.clone();
        bytes.resize(4, 0);
        data.extend(bytes);
    }
    let image = decoded(&bmp(3, -2, 8, 0, &[], &bmp_palette(), &data));
    assert_eq!(pixels(&image), palette_rgba(&rows));
}

#[test]
fn bmp_true_colour_and_bitfields() {
    // 24-bit BGR, rows padded to 4 bytes (3 pixels = 9 bytes -> 12).
    let colours = [[10u8, 20, 30], [40, 50, 60], [70, 80, 90]];
    let mut row = Vec::new();
    for [r, g, b] in colours {
        row.extend_from_slice(&[b, g, r]);
    }
    row.resize(12, 0);
    let image = decoded(&bmp(3, 1, 24, 0, &[], &[], &row));
    assert_eq!(
        pixels(&image),
        colours.map(|[r, g, b]| [r, g, b, 255]).to_vec()
    );

    // 32-bit BI_RGB: the fourth byte is alpha when any pixel uses it.
    let bgra = [30, 20, 10, 255, 60, 50, 40, 128, 90, 80, 70, 0];
    let image = decoded(&bmp(3, 1, 32, 0, &[], &[], &bgra));
    assert_eq!(
        pixels(&image),
        [
            [10, 20, 30, 255],
            premultiply([40, 50, 60, 128]),
            [0, 0, 0, 0]
        ]
    );

    // ...and is ignored when it is zero everywhere.
    let bgrx = [30, 20, 10, 0, 60, 50, 40, 0];
    let image = decoded(&bmp(2, 1, 32, 0, &[], &[], &bgrx));
    assert_eq!(pixels(&image), [[10, 20, 30, 255], [40, 50, 60, 255]]);

    // 16-bit RGB565 through BI_BITFIELDS.
    let v = |r: u16, g: u16, b: u16| ((r << 11) | (g << 5) | b).to_le_bytes();
    let mut data = Vec::new();
    data.extend(v(31, 63, 31));
    data.extend(v(0, 0, 0));
    data.extend(v(16, 32, 8));
    data.extend([0, 0]);
    let image = decoded(&bmp(3, 1, 16, 3, &[0xF800, 0x07E0, 0x001F], &[], &data));
    let scale = |v: u32, max: u32| ((v * 255 + max / 2) / max) as u8;
    assert_eq!(
        pixels(&image),
        [
            [255, 255, 255, 255],
            [0, 0, 0, 255],
            [scale(16, 31), scale(32, 63), scale(8, 31), 255]
        ]
    );

    // 16-bit default is 5-5-5.
    let image = decoded(&bmp(1, 1, 16, 0, &[], &[], &[0x1F, 0x00, 0, 0]));
    assert_eq!(pixels(&image), [[0, 0, 255, 255]]);
}

#[test]
fn bmp_rle8_leaves_skipped_pixels_transparent() {
    // 5x3, rows from the bottom: a run, a literal run plus a run, a delta.
    let data = [
        5, 1, 0, 0, // bottom row: five of index 1, end of line
        0, 3, 2, 3, 4, 0, 2, 1, 0, 0, // literal 2,3,4 (+pad), two of index 1, end of line
        0, 2, 2, 0, 3, 2, 0, 1, // skip two pixels, three of index 2, end of bitmap
    ];
    let image = decoded(&bmp(5, 3, 8, 1, &[], &bmp_palette(), &data));
    let c = |i: usize| {
        let [r, g, b] = bmp_palette()[i];
        [r, g, b, 255]
    };
    let clear = [0, 0, 0, 0];
    assert_eq!(
        pixels(&image),
        [
            clear,
            clear,
            c(2),
            c(2),
            c(2), // top
            c(2),
            c(3),
            c(4),
            c(1),
            c(1), // middle
            c(1),
            c(1),
            c(1),
            c(1),
            c(1), // bottom
        ]
    );
}

#[test]
fn bmp_rle4_expands_nibbles() {
    // 5x1: an encoded run of five alternating 1,2 then end of bitmap, then
    // nothing; and a literal run of three nibbles in a second image.
    let image = decoded(&bmp(5, 1, 4, 2, &[], &bmp_palette(), &[5, 0x12, 0, 1]));
    let c = |i: usize| {
        let [r, g, b] = bmp_palette()[i];
        [r, g, b, 255]
    };
    assert_eq!(pixels(&image), [c(1), c(2), c(1), c(2), c(1)]);

    let image = decoded(&bmp(
        3,
        1,
        4,
        2,
        &[],
        &bmp_palette(),
        &[0, 3, 0x34, 0x50, 0, 1],
    ));
    assert_eq!(pixels(&image), [c(3), c(4), c(5)]);
}

#[test]
fn bmp_damage_is_a_typed_error() {
    let rows = vec![vec![0u8; 5]; 3];
    let good = bmp(5, 3, 8, 0, &[], &bmp_palette(), &pack_indices(&rows, 8));
    let truncated = &good[..good.len() - 4];
    assert!(matches!(
        decode_with(truncated, DecodeParams::default(), DecodeLimits::default()),
        Err(DecodeError::Malformed {
            format: ImageFormat::Bmp,
            ..
        })
    ));
    let embedded = bmp(5, 3, 24, 4, &[], &[], &[0; 64]);
    assert!(matches!(
        decode_with(&embedded, DecodeParams::default(), DecodeLimits::default()),
        Err(DecodeError::Unsupported {
            format: ImageFormat::Bmp,
            ..
        })
    ));
    let zero = bmp(0, 3, 24, 0, &[], &[], &[0; 64]);
    assert!(matches!(
        decode_with(&zero, DecodeParams::default(), DecodeLimits::default()),
        Err(DecodeError::Malformed { .. })
    ));
}

// ICO ----------------------------------------------------------------------

#[test]
fn ico_picks_the_largest_entry() {
    let small = icon_dib(8, 32, |_, _| [1, 2, 3, 255], |_, _| false);
    let large = icon_dib(
        16,
        32,
        |x, y| [x as u8 * 10, y as u8 * 10, 7, 255],
        |_, _| false,
    );
    let bytes = ico(&[
        IconEntry {
            width: 8,
            height: 8,
            bpp: 32,
            data: small,
        },
        IconEntry {
            width: 16,
            height: 16,
            bpp: 32,
            data: large,
        },
    ]);
    let image = decoded(&bytes);
    assert_eq!((image.width, image.height), (16, 16));
    assert_eq!(px(&image, 3, 5), [30, 50, 7, 255]);
}

#[test]
fn ico_prefers_more_bits_at_equal_size() {
    let low = icon_dib(8, 24, |_, _| [1, 1, 1, 255], |_, _| false);
    let high = icon_dib(8, 32, |_, _| [200, 100, 50, 255], |_, _| false);
    let bytes = ico(&[
        IconEntry {
            width: 8,
            height: 8,
            bpp: 24,
            data: low,
        },
        IconEntry {
            width: 8,
            height: 8,
            bpp: 32,
            data: high,
        },
    ]);
    assert_eq!(px(&decoded(&bytes), 0, 0), [200, 100, 50, 255]);
}

#[test]
fn ico_mask_makes_pixels_transparent() {
    let dib = icon_dib(8, 24, |_, _| [9, 8, 7, 0], |x, y| x == y);
    let bytes = ico(&[IconEntry {
        width: 8,
        height: 8,
        bpp: 24,
        data: dib,
    }]);
    let image = decoded(&bytes);
    assert_eq!(px(&image, 2, 2), [0, 0, 0, 0]);
    assert_eq!(px(&image, 2, 3), [9, 8, 7, 255]);
}

#[test]
fn ico_alpha_channel_wins_over_the_mask() {
    let dib = icon_dib(8, 32, |_, _| [9, 8, 7, 100], |_, _| true);
    let bytes = ico(&[IconEntry {
        width: 8,
        height: 8,
        bpp: 32,
        data: dib,
    }]);
    assert_eq!(px(&decoded(&bytes), 4, 4), premultiply([9, 8, 7, 100]));
}

#[test]
fn ico_may_embed_a_png() {
    let bytes = ico(&[
        IconEntry {
            width: 0,
            height: 0,
            bpp: 32,
            data: png_rgba8(),
        },
        IconEntry {
            width: 8,
            height: 8,
            bpp: 32,
            data: icon_dib(8, 32, |_, _| [1, 2, 3, 255], |_, _| false),
        },
    ]);
    let image = decoded(&bytes);
    assert_eq!((image.width, image.height), (W, H));
    assert_eq!(
        image.rgba.as_ref(),
        premultiplied(&gradient_rgba()).as_slice()
    );
}

#[test]
fn ico_falls_back_when_the_best_entry_is_damaged() {
    let bytes = ico(&[
        IconEntry {
            width: 32,
            height: 32,
            bpp: 32,
            data: vec![0xAA; 40],
        },
        IconEntry {
            width: 8,
            height: 8,
            bpp: 32,
            data: icon_dib(8, 32, |_, _| [1, 2, 3, 255], |_, _| false),
        },
    ]);
    let image = decoded(&bytes);
    assert_eq!((image.width, image.height), (8, 8));
}

#[test]
fn ico_without_entries_is_malformed() {
    let result = decode_with(
        &[0, 0, 1, 0, 0, 0],
        DecodeParams::default(),
        DecodeLimits::default(),
    );
    assert!(matches!(
        result,
        Err(DecodeError::Malformed {
            format: ImageFormat::Ico,
            ..
        })
    ));
}

// Sniffing, limits and downscale --------------------------------------------

#[test]
fn the_decoder_is_chosen_by_content() {
    let png = png_rgba8();
    assert_eq!(decoded(&png).width, W);
    let result = decode_with(
        b"<html>not an image</html>",
        DecodeParams::default(),
        DecodeLimits::default(),
    );
    assert_eq!(result, Err(DecodeError::UnknownFormat));
    let wrong = tenta_paint::decode_format(
        ImageFormat::Jpeg,
        &png,
        &DecodeParams::default(),
        &DecodeLimits::default(),
    );
    assert!(
        matches!(
            wrong,
            Err(DecodeError::Malformed {
                format: ImageFormat::Jpeg,
                ..
            })
        ),
        "{wrong:?}"
    );
}

#[test]
fn output_is_premultiplied() {
    let image = decoded(&png_rgba8());
    assert_eq!(px(&image, 31, 0), premultiply(gradient(31, 0)));
    assert_eq!(px(&image, 31, 0)[3], 7);
}

#[test]
fn the_pixel_limit_rejects_before_decoding() {
    let limits = DecodeLimits {
        max_pixels: 100,
        ..DecodeLimits::default()
    };
    for bytes in [
        png_rgba8(),
        jpeg_rgb(|_| {}),
        webp_lossless(true),
        fixture("lossy.webp"),
    ] {
        let result = decode_with(&bytes, DecodeParams::default(), limits);
        assert_eq!(
            result,
            Err(DecodeError::TooLarge {
                width: W,
                height: H
            })
        );
    }
    let gif = gif((32, 24), &[GifFrame::new(0, 0, 32, 24, vec![0; 32 * 24])]);
    assert_eq!(
        decode_with(&gif, DecodeParams::default(), limits),
        Err(DecodeError::TooLarge {
            width: 32,
            height: 24
        })
    );
    let bmp_file = bmp(32, 24, 24, 0, &[], &[], &vec![0; 96 * 24]);
    assert_eq!(
        decode_with(&bmp_file, DecodeParams::default(), limits),
        Err(DecodeError::TooLarge {
            width: 32,
            height: 24
        })
    );
}

#[test]
fn the_byte_limit_applies_to_the_decoded_size() {
    // The limit bounds the peak, which for a PNG is the result plus a few rows:
    // the 32x24 image (3072 bytes) does not fit, its 16x12 shrink (768) does.
    let limits = DecodeLimits {
        max_decoded_bytes: 4096,
        max_transient_bytes: 4096,
        ..DecodeLimits::default()
    };
    let bytes = png_rgba8();
    assert!(matches!(
        decode_with(&bytes, DecodeParams::default(), limits),
        Err(DecodeError::TooLarge { .. })
    ));
    // The same image fits once it is shrunk to the size it will be drawn at.
    let params = DecodeParams {
        max_size: Some((16, 12)),
    };
    let image = decode_with(&bytes, params, limits).expect("fits after the shrink");
    assert_eq!((image.width, image.height), (16, 12));
    assert!(image.rgba.len() <= limits.max_decoded_bytes);
}

#[test]
fn downscale_keeps_aspect_and_reports_the_source_size() {
    let bytes = png_rgba8();
    let params = DecodeParams {
        max_size: Some((16, 16)),
    };
    let image = decode_with(&bytes, params, DecodeLimits::default()).expect("decodes");
    assert_eq!((image.width, image.height), (16, 12));
    assert_eq!((image.source_width, image.source_height), (W, H));
    assert_eq!(image.rgba.len(), 16 * 12 * 4);

    // Never enlarged.
    let params = DecodeParams {
        max_size: Some((500, 500)),
    };
    let image = decode_with(&bytes, params, DecodeLimits::default()).expect("decodes");
    assert_eq!((image.width, image.height), (W, H));
}

#[test]
fn downscaling_averages_the_covered_pixels() {
    // 4x2 opaque, shrunk to 2x1: each output is the mean of a 2x2 block.
    let data = [
        0, 0, 0, 100, 0, 0, 200, 0, 0, 40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let png = encode_png(
        &PngSpec {
            color: png::ColorType::Rgb,
            depth: png::BitDepth::Eight,
            ..spec(png::ColorType::Rgb, png::BitDepth::Eight)
        },
        &data,
    );
    let params = DecodeParams {
        max_size: Some((2, 1)),
    };
    let image = decode_with(&png, params, DecodeLimits::default()).expect("decodes");
    assert_eq!((image.width, image.height), (2, 1));
    assert_eq!(pixels(&image), [[25, 0, 0, 255], [60, 0, 0, 255]]);
}

#[test]
fn downscale_with_rotation_fits_the_displayed_size() {
    let bytes = jpeg_rgb(|e| e.add_exif_metadata(&exif_orientation(6)).expect("exif"));
    let params = DecodeParams {
        max_size: Some((12, 12)),
    };
    let image = decode_with(&bytes, params, DecodeLimits::default()).expect("decodes");
    // Displayed 24x32 shrinks to 9x12.
    assert_eq!((image.width, image.height), (9, 12));
    assert_eq!((image.source_width, image.source_height), (H, W));
}

#[test]
fn downscaled_colour_never_exceeds_alpha() {
    let image = decode_with(
        &png_rgba8(),
        DecodeParams {
            max_size: Some((5, 5)),
        },
        DecodeLimits::default(),
    )
    .expect("decodes");
    for p in image.rgba.chunks(4) {
        assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "{p:?}");
    }
}
