#![no_main]

use libfuzzer_sys::fuzz_target;
use tenta_html::{PreloadScanner, ScanMode, sniff};

// The tokenizer is not public, so it is driven through the two consumers that
// only tokenize: encoding sniffing and the preload scanner. Both scanner
// implementations must agree (docs/spec/13-performance.md §5).
fuzz_target!(|data: &[u8]| {
    let _ = sniff(data, None, "pl");
    let _ = sniff(data, Some("iso-8859-2"), "en");

    let text = String::from_utf8_lossy(data);
    let vector = PreloadScanner::scan(&text, true);
    let scalar = {
        let mut scanner = PreloadScanner::with_scan_mode(true, ScanMode::Scalar);
        let mut hints = scanner.feed(&text);
        hints.extend(scanner.finish());
        hints
    };
    assert_eq!(vector, scalar, "scanners disagree on {text:?}");
});
