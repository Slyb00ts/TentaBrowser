//! The property every decoder target checks.

use tenta_paint::{DecodeError, DecodeLimits, DecodeParams, ImageFormat, decode_format};

const LIMITS: DecodeLimits = DecodeLimits {
    max_pixels: 1 << 20,
    max_decoded_bytes: 4 << 20,
    max_transient_bytes: 4 << 20,
};

/// Decodes `data` as `format`, with and without a decode-time shrink.
///
/// `decode_format` contains decoder panics and reports them as
/// [`DecodeError::Panicked`]; the fuzzer must still see those as crashes.
pub fn check(format: ImageFormat, data: &[u8]) {
    for max_size in [None, Some((64, 64))] {
        let params = DecodeParams { max_size };
        match decode_format(format, data, &params, &LIMITS) {
            Err(DecodeError::Panicked(f)) => panic!("the {f} decoder panicked"),
            Err(_) => {}
            Ok(image) => {
                let bytes = image.width as usize * image.height as usize * 4;
                assert_eq!(image.rgba.len(), bytes);
                assert!(bytes <= LIMITS.max_decoded_bytes);
                assert!(
                    u64::from(image.source_width) * u64::from(image.source_height)
                        <= LIMITS.max_pixels
                );
            }
        }
    }
}
