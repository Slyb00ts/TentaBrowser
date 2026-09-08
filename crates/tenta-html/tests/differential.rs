//! The SIMD scanner must agree with the scalar reference
//! (`docs/spec/13-performance.md` §5), and neither may panic on hostile
//! input.

mod common;

use common::tree::{Tree, serialize};
use tenta_html::{Parser, ParserOptions, PreloadScanner, ScanMode};

/// SplitMix64: a deterministic generator, so a failure is reproducible from
/// the seed printed in the assertion.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

/// Bytes that make the tokenizer branch: the scanned delimiters, tag soup,
/// entities and multi-byte text.
const ALPHABET: &[&str] = &[
    "<",
    ">",
    "&",
    "\0",
    "\r",
    "\n",
    "\t",
    " ",
    "a",
    "/",
    "=",
    "\"",
    "'",
    "!",
    "-",
    "?",
    "<div>",
    "</div>",
    "<p",
    "<!--",
    "-->",
    "&amp;",
    "&#x41;",
    "&notit;",
    "<script>",
    "</script>",
    "<table>",
    "<td>",
    "<svg>",
    "<math>",
    "<template>",
    "<textarea>",
    "ą",
    "😀",
    "\u{feff}",
    "<!doctype html>",
    "<![CDATA[",
    "]]>",
    "<?pi ",
    "?>",
];

fn random_markup(rng: &mut Rng, pieces: usize) -> String {
    let mut text = String::new();
    for _ in 0..pieces {
        text.push_str(ALPHABET[rng.below(ALPHABET.len())]);
    }
    text
}

fn parse_with(text: &str, scan_mode: ScanMode) -> String {
    let options = ParserOptions {
        scan_mode,
        ..ParserOptions::default()
    };
    let mut parser = Parser::new(Tree::new(), options);
    parser.feed_str(text);
    serialize(&parser.finish(), 0, true)
}

#[test]
fn the_scanners_agree_on_random_markup() {
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    for case in 0..2_000 {
        let seed = rng.0;
        let pieces = 1 + rng.below(80);
        let text = random_markup(&mut rng, pieces);
        assert_eq!(
            parse_with(&text, ScanMode::Vector),
            parse_with(&text, ScanMode::Scalar),
            "scanners disagree on case {case} (seed {seed:#x}): {text:?}"
        );
    }
}

#[test]
fn the_scanners_agree_on_long_text_runs() {
    // Long runs are where the vector path takes over from the scalar loop.
    let mut rng = Rng(0xC0FF_EE00_1234_5678);
    for _ in 0..50 {
        let mut text = String::new();
        for _ in 0..8 {
            text.push_str(&"lorem ipsum dolor sit amet ".repeat(1 + rng.below(200)));
            text.push_str(ALPHABET[rng.below(ALPHABET.len())]);
        }
        assert_eq!(
            parse_with(&text, ScanMode::Vector),
            parse_with(&text, ScanMode::Scalar)
        );
    }
}

fn fixture_windows() -> Vec<Vec<u8>> {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let mut windows = Vec::new();
    for name in ["static-article", "tiny", "dynamic-scripts", "js-xhr"] {
        let Ok(bytes) = std::fs::read(fixtures.join(name).join("index.html")) else {
            continue;
        };
        let mut offset = 0;
        while offset < bytes.len() && windows.len() < 512 {
            let end = (offset + 2048).min(bytes.len());
            windows.push(bytes[offset..end].to_vec());
            offset = end;
        }
    }
    assert!(!windows.is_empty(), "no fixtures found");
    windows
}

#[test]
fn the_scanners_agree_on_the_fixture_windows() {
    for window in fixture_windows() {
        let text = String::from_utf8_lossy(&window);
        assert_eq!(
            parse_with(&text, ScanMode::Vector),
            parse_with(&text, ScanMode::Scalar)
        );
    }
}

/// A cheap stand-in for the `cargo fuzz` targets in `fuzz/`, which CI cannot
/// run: random bytes and random mutations of the fixtures must not panic.
#[test]
fn random_input_never_panics() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF0);

    for _ in 0..10_000 {
        let length = rng.below(256);
        let bytes: Vec<u8> = (0..length).map(|_| (rng.next_u64() & 0xFF) as u8).collect();
        let mut parser = Parser::new(Tree::new(), ParserOptions::default());
        parser.feed(&bytes);
        let _ = parser.finish();
        let _ = PreloadScanner::scan(&String::from_utf8_lossy(&bytes), true);
    }

    let windows = fixture_windows();
    for _ in 0..10_000 {
        let mut bytes = windows[rng.below(windows.len())].clone();
        for _ in 0..1 + rng.below(8) {
            if bytes.is_empty() {
                break;
            }
            let at = rng.below(bytes.len());
            match rng.below(4) {
                0 => bytes[at] = (rng.next_u64() & 0xFF) as u8,
                1 => {
                    bytes.remove(at);
                }
                2 => bytes.insert(at, (rng.next_u64() & 0xFF) as u8),
                _ => {
                    let value = bytes[at];
                    bytes.insert(at, value);
                }
            }
        }
        let mut parser = Parser::new(Tree::new(), ParserOptions::default());
        parser.feed(&bytes);
        let _ = parser.finish();
    }
}

/// The XML parser must reject malformed input rather than panic.
#[test]
fn random_input_never_panics_in_the_xml_parser() {
    let mut rng = Rng(0xFEED_FACE_CAFE_0001);
    for _ in 0..10_000 {
        let pieces = 1 + rng.below(40);
        let text = random_markup(&mut rng, pieces);
        let mut parser =
            tenta_html::xml::Parser::new(Tree::new(), tenta_html::xml::XmlOptions::default());
        parser.feed_str(&text);
        let _ = parser.finish();
    }
}

/// Whole parsing must agree with chunked parsing at multiple chunk sizes.
/// R3-3 found a differential where chunking at certain boundaries caused
/// divergence in PI/comment content, so test all crash inputs at various
/// chunk boundaries.
#[test]
fn crash_inputs_must_parse_the_same_at_all_chunk_sizes() {
    // Load the crash inputs from fixtures.
    let crash_files = ["crash1.bin", "crash2.bin", "crash3.bin", "crash4.bin"];

    for filename in &crash_files {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(filename);
        let bytes =
            std::fs::read(&file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));

        let whole = parse_with_bytes(&bytes, ScanMode::Vector, &bytes);

        // Test at chunk sizes 1, 2, 3, 4, 7, 64, 500, and whole.
        for chunk_size in &[1, 2, 3, 4, 7, 64, 500, bytes.len()] {
            let chunked = parse_with_chunks_bytes(&bytes, *chunk_size, ScanMode::Vector);
            assert_eq!(
                whole, chunked,
                "{}: chunk size {} diverges from whole parse",
                filename, chunk_size
            );
        }
    }
}

/// Helper to parse bytes in whole mode.
fn parse_with_bytes(bytes: &[u8], scan_mode: ScanMode, _all_bytes: &[u8]) -> String {
    let options = ParserOptions {
        scan_mode,
        ..ParserOptions::default()
    };
    let mut parser = Parser::new(Tree::new(), options);
    parser.feed(bytes);
    serialize(&parser.finish(), 0, true)
}

/// Helper to parse bytes in chunks.
fn parse_with_chunks_bytes(bytes: &[u8], chunk_size: usize, scan_mode: ScanMode) -> String {
    let options = ParserOptions {
        scan_mode,
        ..ParserOptions::default()
    };
    let mut parser = Parser::new(Tree::new(), options);
    for chunk in bytes.chunks(chunk_size) {
        parser.feed(chunk);
    }
    serialize(&parser.finish(), 0, true)
}

/// Test chunking divergence for exact chunk boundaries inside multi-character sequences.
/// Critical: chunk boundary between `?` and `>` in PI, or `--` and `>` in comment.
/// The tokenizer must not emit PI/comment differently based on where the boundary falls.
#[test]
fn chunk_boundary_inside_terminator_sequences() {
    // Helper to test with chunk boundary at exact position
    let make_test = |prefix: &str, char_at_boundary: char, suffix: &str| -> String {
        let mut html = String::with_capacity(prefix.len() + 1 + suffix.len());
        html.push_str(prefix);
        html.push(char_at_boundary);
        html.push_str(suffix);
        html
    };

    // Test cases: (prefix, char_at_boundary, suffix, description)
    // The boundary falls AFTER the char, splitting the terminator
    let test_cases = vec![
        // PI: boundary between `?` and `>` in `<?a?>`
        ("a<?a", '?', ">", "PI: boundary between ? and >"),
        // PI: `??>` keeps one `?` as data whichever side of the boundary it is on
        ("a<?a?", '?', ">", "PI: boundary between ? and ? in ??>"),
        ("a<?a", '?', "?>", "PI: boundary before ?? in ??>"),
        // Comment: boundary between first `-` and second `-` in `<!--x-->`
        ("a<!---", '-', "x-->", "comment: boundary between - and -"),
        // PI with filler to reach 1024-byte boundary
        ("", '?', ">", "PI at prescan: ? at 1023, > at 1024"),
    ];

    for (prefix, boundary_char, suffix, desc) in test_cases {
        let html = make_test(prefix, boundary_char, suffix);
        let bytes = html.as_bytes();

        let whole = parse_with_bytes(bytes, ScanMode::Vector, bytes);

        // Test with boundary-aligned chunk size (chunk1 ends with boundary_char)
        let chunk_size = prefix.len() + 1; // Put char_at_boundary at end of first chunk
        if chunk_size > 0 && chunk_size < bytes.len() {
            let chunked = parse_with_chunks_bytes(bytes, chunk_size, ScanMode::Vector);
            assert_eq!(
                whole, chunked,
                "{}: chunk_size={} diverges from whole parse",
                desc, chunk_size
            );
        }

        // Also test with chunk size 1 for thoroughness
        let chunked_1byte = parse_with_chunks_bytes(bytes, 1, ScanMode::Vector);
        assert_eq!(
            whole, chunked_1byte,
            "{}: 1-byte chunks diverge from whole parse",
            desc
        );
    }
}

/// Test chunking divergence around the 1024-byte prescan boundary for various tokenizer states.
/// This tests for "pending byte" bugs where a character consumed by the tokenizer is lost at
/// chunk boundaries when it was in the middle of a two-character sequence check (e.g., `?>`).
#[test]
fn tokenizer_state_pending_bytes_at_prescan_boundary() {
    // Helper to create a test input with filler bytes and a tail sequence.
    let make_boundary_test = |filler_len: usize, tail: &str| {
        let mut html = String::with_capacity(filler_len + tail.len());
        for _ in 0..filler_len {
            html.push('a');
        }
        html.push_str(tail);
        html
    };

    // Test cases: (filler_length, tail_sequence, description)
    let test_cases = vec![
        // Bogus comment starting with `<!`
        (1019, "<!--x--!", "bogus comment <!"),
        // Processing instruction with `?>`
        (1019, "<?x?>", "processing instruction ?>"),
        (1020, "<?x?>", "PI at boundary+1"),
        (1018, "<?x?>", "PI at boundary-1"),
        // Entity reference with `&`
        (1019, "&amp;", "entity reference &"),
        (1020, "&amp;", "entity at boundary+1"),
        // CR normalization
        (1022, "\r\n", "CR+LF at boundary"),
        (1023, "\r\n", "CR+LF at boundary+1"),
        (1021, "\r\n", "CR+LF at boundary-1"),
    ];

    for (filler_len, tail, desc) in test_cases {
        let html = make_boundary_test(filler_len, tail);
        let bytes = html.as_bytes();

        let whole = parse_with_bytes(bytes, ScanMode::Vector, bytes);

        // Test at various chunk sizes around the boundary
        for chunk_size in &[1, 2, 3, 4, 7, 64, 500, bytes.len()] {
            let chunked = parse_with_chunks_bytes(bytes, *chunk_size, ScanMode::Vector);
            assert_eq!(
                whole, chunked,
                "{}: chunk size {} diverges from whole parse (html: {} filler + '{}')",
                desc, chunk_size, filler_len, tail
            );
        }
    }
}
