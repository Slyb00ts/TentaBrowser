//! Memory: at the default desktop limits, every decoder either refuses an image
//! before allocating for it or stays within the decode budget at its **peak**,
//! not merely in its output. Measured with a counting allocator.

mod support;

use support::tracking::{Tracking, measure};
use support::{GifFrame, bmp, bmp_palette, crc32, fixture, gif, webp_lossless};
use tenta_paint::{DecodeError, DecodeLimits, DecodeParams, ImageFormat, decode_format};

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

const MIB: usize = 1024 * 1024;
/// Fixed decoder state (tables, I/O buffers) that does not scale with pixels.
const SLACK: usize = MIB;
/// What a decoder holds regardless of the image: the PNG inflater's window,
/// JPEG and WebP tables. The per-pixel estimate does not include it.
const FIXED: usize = MIB / 2;

fn limits() -> DecodeLimits {
    DecodeLimits::default()
}

/// The peak-heap budget of a decode (the retained image has its own, smaller one).
fn budget() -> usize {
    limits().max_transient_bytes
}

/// Limits whose retained and transient budgets are both `bytes`.
fn capped(bytes: usize) -> DecodeLimits {
    DecodeLimits {
        max_decoded_bytes: bytes,
        max_transient_bytes: bytes,
        ..limits()
    }
}

// Builders -------------------------------------------------------------------

/// Deterministic incompressible bytes.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// A zlib stream of stored (uncompressed) blocks.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none()));
        out.extend_from_slice(&(block.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[name.as_slice(), data].concat()).to_be_bytes());
}

const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

/// A PNG of `width x height` pixels of `channels` samples of `depth_bytes`
/// bytes each. `rows` limits how much pixel data is really present; `None`
/// writes the whole picture as noise.
fn png_stored(
    width: u32,
    height: u32,
    color_type: u8,
    channels: usize,
    depth_bytes: usize,
    interlaced: bool,
    full: bool,
) -> Vec<u8> {
    let px = channels * depth_bytes;
    let mut raw = Vec::new();
    if full {
        let passes: Vec<(usize, usize, usize, usize)> = if interlaced {
            ADAM7.to_vec()
        } else {
            vec![(0, 0, 1, 1)]
        };
        for (x0, y0, dx, dy) in passes {
            let (w, h) = (
                (width as usize).saturating_sub(x0).div_ceil(dx),
                (height as usize).saturating_sub(y0).div_ceil(dy),
            );
            if w == 0 || h == 0 {
                continue;
            }
            for row in 0..h {
                raw.push(0);
                raw.extend(noise(w * px, (row + w) as u64 + 7));
            }
        }
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[
        (depth_bytes * 8) as u8,
        color_type,
        0,
        0,
        u8::from(interlaced),
    ]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn rgba16(w: u32, h: u32, interlaced: bool, full: bool) -> Vec<u8> {
    png_stored(w, h, 6, 4, 2, interlaced, full)
}

fn rgb8(w: u32, h: u32, interlaced: bool, full: bool) -> Vec<u8> {
    png_stored(w, h, 2, 3, 1, interlaced, full)
}

fn jpeg_noise(side: u16, sampling: jpeg_encoder::SamplingFactor, progressive: bool) -> Vec<u8> {
    let data = noise(usize::from(side) * usize::from(side) * 3, 3);
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, 80);
    encoder.set_sampling_factor(sampling);
    encoder.set_progressive(progressive);
    encoder
        .encode(&data, side, side, jpeg_encoder::ColorType::Rgb)
        .expect("jpeg encode");
    out
}

/// Rewrites the frame size in the first SOF segment of a JPEG.
fn jpeg_with_size(mut bytes: Vec<u8>, width: u16, height: u16) -> Vec<u8> {
    let sof = bytes
        .windows(2)
        .position(|w| w[0] == 0xFF && matches!(w[1], 0xC0 | 0xC2))
        .expect("SOF marker");
    bytes[sof + 5..sof + 7].copy_from_slice(&height.to_be_bytes());
    bytes[sof + 7..sof + 9].copy_from_slice(&width.to_be_bytes());
    bytes
}

/// A simple lossless WebP whose VP8L header claims `width x height`.
fn webp_lossless_with_size(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = webp_lossless(true);
    assert_eq!(&bytes[12..16], b"VP8L");
    let packed = (width - 1) | ((height - 1) << 14);
    let old = u32::from_le_bytes(bytes[21..25].try_into().expect("header"));
    let new = (old & !0x0FFF_FFFF) | packed;
    bytes[21..25].copy_from_slice(&new.to_le_bytes());
    bytes
}

/// A simple lossy WebP whose VP8 frame header claims `width x height`.
fn webp_lossy_with_size(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = fixture("lossy.webp");
    assert_eq!(&bytes[12..16], b"VP8 ");
    bytes[26..28].copy_from_slice(&width.to_le_bytes());
    bytes[28..30].copy_from_slice(&height.to_le_bytes());
    bytes
}

/// A bottom-up RLE8 bitmap of single-pixel runs.
fn bmp_rle8(side: i32) -> Vec<u8> {
    let indices = noise(side as usize * side as usize, 11);
    let mut data = Vec::new();
    for row in indices.chunks(side as usize) {
        for &i in row {
            data.extend_from_slice(&[1, i % 6]);
        }
        data.extend_from_slice(&[0, 0]);
    }
    data.extend_from_slice(&[0, 1]);
    bmp(side, side, 8, 1, &[], &bmp_palette(), &data)
}

/// A 24-bit bitmap header for `width x height` followed by little data.
fn bmp_hostile(width: i32, height: i32) -> Vec<u8> {
    bmp(width, height, 8, 1, &[], &bmp_palette(), &[0, 1])
}

fn gif_noise(side: u16, frame: u16, at: u16) -> Vec<u8> {
    let pixels: Vec<u8> = noise(usize::from(frame) * usize::from(frame), 9)
        .into_iter()
        .map(|b| b % 3)
        .collect();
    gif((side, side), &[GifFrame::new(at, at, frame, frame, pixels)])
}

// Measurement ----------------------------------------------------------------

type Outcome = (Result<tenta_paint::DecodedImage, DecodeError>, usize);

fn run(format: ImageFormat, bytes: &[u8], params: DecodeParams, limits: DecodeLimits) -> Outcome {
    measure(|| decode_format(format, bytes, &params, &limits))
}

fn shrink() -> DecodeParams {
    DecodeParams {
        max_size: Some((256, 256)),
    }
}

// Tests ----------------------------------------------------------------------

/// One real 1 Mpx image; `shrunk` also runs it with a decode-time shrink, which
/// is format independent, so one sample per format is enough.
type Sample = (&'static str, ImageFormat, Vec<u8>, bool);

/// For each sample: the admission estimate must cover the measured peak (a
/// budget just under the peak is refused) and must not be absurdly pessimistic
/// (three times the peak is accepted).
fn assert_estimate_covers(samples: Vec<Sample>) {
    for (name, format, bytes, shrunk) in &samples {
        let mut cases = vec![("full size", DecodeParams::default())];
        if *shrunk {
            cases.push(("shrunk", shrink()));
        }
        for (case, params) in cases {
            let (result, peak) = run(*format, bytes, params, limits());
            result.unwrap_or_else(|e| panic!("{name} {case}: {e}"));
            let output = if params.max_size.is_some() {
                256 * 256 * 4
            } else {
                1024 * 1024 * 4
            };
            eprintln!(
                "{name:24} {case:9} peak {:6.2} MiB = {:5.2} x output",
                peak as f64 / MIB as f64,
                peak as f64 / output as f64
            );

            // The estimate plus the fixed state covers the peak, so a budget
            // below the peak minus that state must be refused. (Where the peak
            // is mostly fixed state there is nothing to check.)
            if let Some(under) = (peak - peak / 20).checked_sub(FIXED).filter(|&b| b > 0) {
                let tight = capped(under);
                let (refused, _) = run(*format, bytes, params, tight);
                assert!(
                    matches!(refused, Err(DecodeError::TooLarge { .. })),
                    "{name} {case}: a budget of {under} under the measured peak {peak} must be refused, got {:?}",
                    refused.map(|i| (i.width, i.height))
                );
            }
            if params.max_size.is_none() {
                let roomy = capped(peak * 3 + FIXED);
                let (accepted, _) = run(*format, bytes, params, roomy);
                assert!(
                    accepted.is_ok(),
                    "{name} {case}: three times the peak ({peak}) is still refused: {:?}",
                    accepted.err()
                );
            }
        }
    }
}

#[test]
fn png_estimate_covers_the_measured_peak() {
    assert_estimate_covers(vec![
        (
            "png rgba16",
            ImageFormat::Png,
            rgba16(1024, 1024, false, true),
            true,
        ),
        (
            "png rgb8",
            ImageFormat::Png,
            rgb8(1024, 1024, false, true),
            false,
        ),
        (
            "png rgba16 interlaced",
            ImageFormat::Png,
            rgba16(1024, 1024, true, true),
            true,
        ),
    ]);
}

#[test]
fn jpeg_estimate_covers_the_measured_peak() {
    use jpeg_encoder::SamplingFactor::{F_1_1, F_2_2};
    assert_estimate_covers(vec![
        (
            "jpeg baseline 444",
            ImageFormat::Jpeg,
            jpeg_noise(1024, F_1_1, false),
            true,
        ),
        (
            "jpeg baseline 420",
            ImageFormat::Jpeg,
            jpeg_noise(1024, F_2_2, false),
            false,
        ),
        (
            "jpeg progressive 444",
            ImageFormat::Jpeg,
            fixture("progressive-444-1mpx.jpg"),
            true,
        ),
        (
            "jpeg progressive 420",
            ImageFormat::Jpeg,
            fixture("progressive-420-1mpx.jpg"),
            false,
        ),
    ]);
}

#[test]
fn lossy_webp_estimate_covers_the_measured_peak() {
    assert_estimate_covers(vec![
        (
            "webp lossy",
            ImageFormat::WebP,
            fixture("lossy-1mpx.webp"),
            true,
        ),
        (
            "webp lossy alpha",
            ImageFormat::WebP,
            fixture("lossy-alpha-1mpx.webp"),
            false,
        ),
    ]);
}

#[test]
fn lossless_webp_estimate_covers_the_measured_peak() {
    assert_estimate_covers(vec![
        (
            "webp lossless",
            ImageFormat::WebP,
            fixture("lossless-1mpx.webp"),
            true,
        ),
        (
            "webp lossless alpha",
            ImageFormat::WebP,
            fixture("lossless-alpha-1mpx.webp"),
            false,
        ),
        (
            "webp lossless palette",
            ImageFormat::WebP,
            fixture("lossless-palette-1mpx.webp"),
            false,
        ),
    ]);
}

#[test]
fn gif_and_bmp_estimates_cover_the_measured_peak() {
    assert_estimate_covers(vec![
        (
            "gif full frame",
            ImageFormat::Gif,
            gif_noise(1024, 1024, 0),
            true,
        ),
        (
            "gif offset frame",
            ImageFormat::Gif,
            gif_noise(1024, 1000, 10),
            false,
        ),
        ("bmp rle8", ImageFormat::Bmp, bmp_rle8(1024), true),
        (
            "bmp 24-bit",
            ImageFormat::Bmp,
            bmp(1024, 1024, 24, 0, &[], &[], &noise(1024 * 1024 * 3, 2)),
            false,
        ),
    ]);
}

/// An ICO entry that is a large PNG is bounded like a PNG.
#[test]
fn an_icon_holding_a_large_png_is_bounded() {
    let entry = rgba16(1024, 1024, false, true);
    let ico = support::ico(&[support::IconEntry {
        width: 0,
        height: 0,
        bpp: 32,
        data: entry,
    }]);
    let (result, peak) = run(ImageFormat::Ico, &ico, DecodeParams::default(), limits());
    result.expect("decodes");
    assert!(peak <= budget() + SLACK, "peak {peak}");
    let tight = capped(peak - peak / 10 - FIXED);
    let (refused, _) = run(ImageFormat::Ico, &ico, DecodeParams::default(), tight);
    assert!(
        matches!(refused, Err(DecodeError::TooLarge { .. })),
        "{refused:?}"
    );
}

/// Headers that claim huge pictures with (almost) no data: refused up front,
/// without allocating for them.
#[test]
fn hostile_headers_are_refused_before_allocation() {
    let jpeg = jpeg_noise(16, jpeg_encoder::SamplingFactor::F_1_1, false);
    let jpeg_prog = jpeg_noise(16, jpeg_encoder::SamplingFactor::F_1_1, true);
    let cases: Vec<(&str, ImageFormat, Vec<u8>)> = vec![
        (
            "png 30000x30000 rgba16",
            ImageFormat::Png,
            rgba16(30000, 30000, false, false),
        ),
        (
            "png 6000x6000 rgba8",
            ImageFormat::Png,
            png_stored(6000, 6000, 6, 4, 1, false, false),
        ),
        (
            "png 6000x6000 rgba8 interlaced",
            ImageFormat::Png,
            png_stored(6000, 6000, 6, 4, 1, true, false),
        ),
        (
            "png 6000x6000 rgb8 interlaced",
            ImageFormat::Png,
            rgb8(6000, 6000, true, false),
        ),
        (
            "jpeg 65535x65535",
            ImageFormat::Jpeg,
            jpeg_with_size(jpeg.clone(), 65535, 65535),
        ),
        (
            "jpeg 6000x6000 baseline",
            ImageFormat::Jpeg,
            jpeg_with_size(jpeg.clone(), 6000, 6000),
        ),
        (
            "jpeg 6000x6000 progressive",
            ImageFormat::Jpeg,
            jpeg_with_size(jpeg_prog, 6000, 6000),
        ),
        (
            "webp lossless 16383x16383",
            ImageFormat::WebP,
            webp_lossless_with_size(16383, 16383),
        ),
        (
            "webp lossless 6000x6000",
            ImageFormat::WebP,
            webp_lossless_with_size(6000, 6000),
        ),
        (
            "webp lossy 16383x16383",
            ImageFormat::WebP,
            webp_lossy_with_size(16383, 16383),
        ),
        (
            "webp lossy 6000x6000",
            ImageFormat::WebP,
            webp_lossy_with_size(6000, 6000),
        ),
        (
            "gif 16383x16383 screen",
            ImageFormat::Gif,
            gif((16383, 16383), &[GifFrame::new(0, 0, 1, 1, vec![0])]),
        ),
        (
            "bmp 30000x30000 rle8",
            ImageFormat::Bmp,
            bmp_hostile(30000, 30000),
        ),
    ];
    for (name, format, bytes) in &cases {
        let (result, peak) = run(*format, bytes, DecodeParams::default(), limits());
        assert!(
            matches!(result, Err(DecodeError::TooLarge { .. })),
            "{name}: expected TooLarge, got {:?}",
            result.map(|i| (i.width, i.height))
        );
        assert!(peak < MIB, "{name}: allocated {peak} bytes before refusing");
    }
}

/// Headers at the edge of what the estimates admit: whatever the decoder then
/// allocates for them (before failing on the missing data, or succeeding on a
/// blank picture) must still fit the budget.
fn assert_within_budget(cases: Vec<(&str, ImageFormat, Vec<u8>)>, with_shrink: bool) {
    for (name, format, bytes) in &cases {
        let mut all_params = vec![DecodeParams::default()];
        if with_shrink {
            all_params.push(shrink());
        }
        for params in all_params {
            let (result, peak) = run(*format, bytes, params, limits());
            eprintln!(
                "{name:30} shrink={:5} -> {:?} peak {:6.2} MiB",
                params.max_size.is_some(),
                result
                    .as_ref()
                    .map(|i| (i.width, i.height))
                    .map_err(|e| e.to_string()),
                peak as f64 / MIB as f64
            );
            assert!(
                peak <= budget() + SLACK,
                "{name}: peak {peak} exceeds the {} byte budget",
                budget()
            );
        }
    }
}

#[test]
fn png_near_the_limit_stays_within_the_budget() {
    assert_within_budget(
        vec![
            (
                "png 4000x4000 rgba8",
                ImageFormat::Png,
                png_stored(4000, 4000, 6, 4, 1, false, false),
            ),
            (
                "png 4000x4000 rgba8 interlaced",
                ImageFormat::Png,
                png_stored(4000, 4000, 6, 4, 1, true, false),
            ),
        ],
        true,
    );
}

#[test]
fn jpeg_near_the_limit_stays_within_the_budget() {
    use jpeg_encoder::SamplingFactor::F_1_1;
    assert_within_budget(
        vec![
            (
                "jpeg 3600x3600 baseline",
                ImageFormat::Jpeg,
                jpeg_with_size(jpeg_noise(16, F_1_1, false), 3600, 3600),
            ),
            (
                "jpeg 2300x2300 progressive",
                ImageFormat::Jpeg,
                jpeg_with_size(jpeg_noise(16, F_1_1, true), 2300, 2300),
            ),
        ],
        false,
    );
}

#[test]
fn webp_gif_and_bmp_near_the_limit_stay_within_the_budget() {
    assert_within_budget(
        vec![
            (
                "webp lossless alpha 3300x3300",
                ImageFormat::WebP,
                webp_lossless_with_size(3300, 3300),
            ),
            (
                "webp lossy 3000x3000",
                ImageFormat::WebP,
                webp_lossy_with_size(3000, 3000),
            ),
            (
                "gif 4000x4000 screen",
                ImageFormat::Gif,
                gif((4000, 4000), &[GifFrame::new(0, 0, 1, 1, vec![0])]),
            ),
            (
                "bmp 4000x4000 rle8",
                ImageFormat::Bmp,
                bmp_hostile(4000, 4000),
            ),
        ],
        true,
    );
}

/// With the budget as the only constraint, the pixel ceiling shrinks for the
/// formats that need more working memory per pixel.
#[test]
fn costlier_formats_are_admitted_at_fewer_pixels() {
    let side = |bytes: Vec<u8>, format: ImageFormat| {
        let (result, _) = run(format, &bytes, DecodeParams::default(), limits());
        !matches!(result, Err(DecodeError::TooLarge { .. }))
    };
    // Against the 128 MiB transient budget (the 64 MiB retained budget caps
    // the unshrunk output at 16 Mpx).
    let baseline = jpeg_noise(16, jpeg_encoder::SamplingFactor::F_1_1, false);
    let progressive = jpeg_noise(16, jpeg_encoder::SamplingFactor::F_1_1, true);
    assert!(side(
        png_stored(4000, 4000, 6, 4, 1, false, false),
        ImageFormat::Png
    ));
    assert!(side(
        jpeg_with_size(baseline, 4000, 4000),
        ImageFormat::Jpeg
    ));
    // 4:4:4 progressive needs 12 bytes a pixel: 108 MB at 3000x3000, 192 MB at 4000x4000.
    assert!(side(
        jpeg_with_size(progressive.clone(), 3000, 3000),
        ImageFormat::Jpeg
    ));
    assert!(!side(
        jpeg_with_size(progressive, 4000, 4000),
        ImageFormat::Jpeg
    ));
    assert!(side(webp_lossy_with_size(3500, 3500), ImageFormat::WebP));
    assert!(!side(webp_lossy_with_size(5000, 5000), ImageFormat::WebP));
}

/// A 12 Mpx phone photo (3000x4000) is admitted when it is shrunk to the size it
/// is drawn at: the retained image is small and the transient peak fits.
#[test]
fn twelve_megapixel_photos_decode_with_a_max_size() {
    use jpeg_encoder::SamplingFactor::F_2_2;
    let params = DecodeParams {
        max_size: Some((1280, 800)),
    };
    let cases: Vec<(&str, ImageFormat, Vec<u8>)> = vec![
        (
            "jpeg progressive 4:2:0",
            ImageFormat::Jpeg,
            jpeg_with_size(jpeg_noise(16, F_2_2, true), 3000, 4000),
        ),
        (
            "jpeg baseline",
            ImageFormat::Jpeg,
            jpeg_with_size(jpeg_noise(16, F_2_2, false), 3000, 4000),
        ),
        (
            "webp lossy",
            ImageFormat::WebP,
            webp_lossy_with_size(3000, 4000),
        ),
        (
            "webp lossless",
            ImageFormat::WebP,
            webp_lossless_with_size(3000, 4000),
        ),
    ];
    for (name, format, bytes) in &cases {
        let (result, peak) = run(*format, bytes, params, limits());
        // The patched headers have no matching data, so WebP ends in a decode
        // error after allocating; what matters is that it was admitted.
        assert!(
            !matches!(result, Err(DecodeError::TooLarge { .. })),
            "{name}: refused: {result:?}"
        );
        if let Ok(image) = &result {
            assert!(image.rgba.len() <= limits().max_decoded_bytes, "{name}");
            assert_eq!((image.source_width, image.source_height), (3000, 4000));
        }
        assert!(
            peak <= budget() + SLACK,
            "{name}: peak {peak} over the transient budget"
        );
    }
}

/// The transient budget does not make everything admissible.
#[test]
fn thirty_megapixel_photos_are_still_refused() {
    use jpeg_encoder::SamplingFactor::{F_1_1, F_2_2};
    let params = DecodeParams {
        max_size: Some((1280, 800)),
    };
    let cases: Vec<(&str, ImageFormat, Vec<u8>)> = vec![
        (
            "jpeg baseline",
            ImageFormat::Jpeg,
            jpeg_with_size(jpeg_noise(16, F_1_1, false), 5477, 5477),
        ),
        (
            "jpeg progressive 4:2:0",
            ImageFormat::Jpeg,
            jpeg_with_size(jpeg_noise(16, F_2_2, true), 5477, 5477),
        ),
        (
            "webp lossy",
            ImageFormat::WebP,
            webp_lossy_with_size(5477, 5477),
        ),
        (
            "webp lossless",
            ImageFormat::WebP,
            webp_lossless_with_size(5477, 5477),
        ),
    ];
    for (name, format, bytes) in &cases {
        let (result, peak) = run(*format, bytes, params, limits());
        assert!(
            matches!(result, Err(DecodeError::TooLarge { .. })),
            "{name}: {:?}",
            result.map(|i| (i.width, i.height))
        );
        assert!(peak < MIB, "{name}: allocated {peak} bytes before refusing");
    }
}

/// `image-webp` allocates a chunk from its declared size before reading it, so
/// a few bytes of header could otherwise request 4 GiB.
#[test]
fn a_webp_chunk_claiming_more_than_the_file_is_refused_without_allocating() {
    for mut bytes in [fixture("lossy.webp"), webp_lossless(true)] {
        bytes[16..20].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        let (result, peak) = run(ImageFormat::WebP, &bytes, DecodeParams::default(), limits());
        assert!(
            matches!(result, Err(DecodeError::Malformed { .. })),
            "{result:?}"
        );
        assert!(peak < MIB, "allocated {peak} bytes before refusing");
    }
}
