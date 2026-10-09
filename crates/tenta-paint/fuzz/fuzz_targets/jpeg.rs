#![no_main]

use libfuzzer_sys::fuzz_target;
use tenta_paint::ImageFormat;

// Image bytes are attacker-controlled. Decoding must return a typed error or an
// image inside the limits, and never panic, even inside the contained backstop.
fuzz_target!(|data: &[u8]| tenta_paint_fuzz::check(ImageFormat::Jpeg, data));
