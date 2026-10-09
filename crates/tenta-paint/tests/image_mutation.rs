//! Robustness: 10 MiB of seeded mutations of valid and corrupt samples must
//! never panic a decoder, never exceed the decode limits and never hold more
//! heap than the decode budget plus fixed decoder state. The same property is fuzzed open-endedly by `fuzz/`.

mod support;

use support::tracking::{Tracking, measure};
use support::{corpus, crc32, fixture};
use tenta_paint::{DecodeError, DecodeLimits, DecodeParams, ImageFormat, decode, decode_format};

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

/// SplitMix64: small, seedable and good enough to scatter edits.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Edits concentrate on the first bytes, where the headers and dimensions are.
fn position(rng: &mut Rng, len: usize) -> usize {
    if rng.below(2) == 0 {
        rng.below(len.min(80))
    } else {
        rng.below(len)
    }
}

fn mutate(rng: &mut Rng, sample: &[u8]) -> Vec<u8> {
    const INTERESTING: [u32; 8] = [
        0,
        1,
        0xFF,
        0x7FFF,
        0xFFFF,
        0x1_0000,
        0x7FFF_FFFF,
        0xFFFF_FFFF,
    ];
    let mut data = sample.to_vec();
    for _ in 0..1 + rng.below(6) {
        if data.is_empty() {
            break;
        }
        let at = position(rng, data.len());
        match rng.below(7) {
            0 => data[at] ^= 1 << rng.below(8),
            1 => data[at] = rng.next() as u8,
            2 => {
                let value = INTERESTING[rng.below(INTERESTING.len())].to_le_bytes();
                let width = if rng.below(2) == 0 { 2 } else { 4 };
                for (i, b) in value[..width].iter().enumerate() {
                    if let Some(slot) = data.get_mut(at + i) {
                        *slot = *b;
                    }
                }
            }
            3 => {
                let end = (at + 1 + rng.below(32)).min(data.len());
                data.drain(at..end);
            }
            4 => {
                let end = (at + 1 + rng.below(32)).min(data.len());
                let copy = data[at..end].to_vec();
                data.splice(at..at, copy);
            }
            5 => data.truncate(at.max(1)),
            _ => {
                let end = (at + 1 + rng.below(16)).min(data.len());
                data[at..end].fill(if rng.below(2) == 0 { 0 } else { 0xFF });
            }
        }
    }
    data
}

/// Rewrites every PNG chunk checksum that fits in the data, so that damage to
/// chunk contents reaches the inflater and unfilter instead of the CRC check.
fn fix_png_crcs(data: &mut [u8]) {
    let mut at = 8;
    while at + 12 <= data.len() {
        let len = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        let Some(end) = (at + 8).checked_add(len).filter(|&e| e + 4 <= data.len()) else {
            return;
        };
        let crc = crc32(&data[at + 4..end]).to_be_bytes();
        data[end..end + 4].copy_from_slice(&crc);
        at = end + 4;
    }
}

fn samples() -> Vec<(ImageFormat, Vec<u8>)> {
    let mut samples = corpus();
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/images/pngsuite");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("fixture directory")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".png"))
        .collect();
    names.sort();
    for name in names {
        samples.push((ImageFormat::Png, fixture(&format!("pngsuite/{name}"))));
    }
    samples
}

#[test]
fn ten_megabytes_of_mutations_never_panic_or_exceed_limits() {
    const TARGET_BYTES: usize = 10 * 1024 * 1024;
    /// The decode budget bounds the peak, so a mutant may use the 1 MiB budget
    /// plus the fixed decoder state (tables, inflater window) and no more.
    const PEAK_ALLOWANCE: usize = 1024 * 1024 + 2 * 1024 * 1024;

    let limits = DecodeLimits {
        max_pixels: 1 << 19,
        max_decoded_bytes: 1 << 20,
        max_transient_bytes: 1 << 20,
    };
    let samples = samples();
    let mut rng = Rng(0x7E47_AB20_2026_1009);
    let (mut fed, mut iteration) = (0usize, 0usize);
    let mut decoded_by_format: std::collections::HashMap<ImageFormat, usize> = Default::default();
    let mut worst_peak = 0usize;

    while fed < TARGET_BYTES {
        let (format, sample) = &samples[iteration % samples.len()];
        let mut bytes = mutate(&mut rng, sample);
        if *format == ImageFormat::Png && rng.below(4) != 0 {
            fix_png_crcs(&mut bytes);
        }
        fed += bytes.len();
        let params = DecodeParams {
            max_size: (iteration % 2 == 0).then_some((48, 48)),
        };

        let (result, peak) = measure(|| {
            if iteration % 5 == 0 {
                decode(&bytes, &params, &limits)
            } else {
                decode_format(*format, &bytes, &params, &limits)
            }
        });
        worst_peak = worst_peak.max(peak);
        assert!(
            peak <= PEAK_ALLOWANCE,
            "mutation {iteration} of a {format} sample held {peak} bytes ({:?})",
            result.as_ref().map(|i| (i.width, i.height))
        );

        match &result {
            Err(DecodeError::Panicked(f)) => {
                panic!("the {f} decoder panicked on mutation {iteration} of a {format} sample")
            }
            Err(_) => {}
            Ok(image) => {
                *decoded_by_format.entry(*format).or_default() += 1;
                let bytes = image.width as usize * image.height as usize * 4;
                assert_eq!(image.rgba.len(), bytes, "mutation {iteration}");
                assert!(bytes <= limits.max_decoded_bytes, "mutation {iteration}");
                assert!(
                    u64::from(image.source_width) * u64::from(image.source_height)
                        <= limits.max_pixels,
                    "mutation {iteration}"
                );
                if let Some((w, h)) = params.max_size {
                    assert!(image.width <= w.max(1) && image.height <= h.max(1));
                }
            }
        }
        iteration += 1;
    }

    assert!(worst_peak <= PEAK_ALLOWANCE, "peak heap {worst_peak} bytes");
    // Damage is not always fatal: every format decoded some mutants, so the
    // decoders ran past their headers.
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::WebP,
        ImageFormat::Gif,
        ImageFormat::Bmp,
        ImageFormat::Ico,
    ] {
        let count = decoded_by_format.get(&format).copied().unwrap_or(0);
        assert!(count >= 20, "only {count} {format} mutants decoded");
    }
}
