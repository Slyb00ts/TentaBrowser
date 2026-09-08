//! Byte scanning for the tokenizer's text states.
//!
//! Most bytes of a page are text and never need the per-character state
//! machine (`docs/spec/04-dom-and-css.md` §3.1). Each text state has a small
//! set of bytes that end a run; [`find_next`] returns the offset of the first
//! such byte, using `memchr` (SSE2 on x86, NEON on aarch64, both selected at
//! runtime inside the crate) or a scalar reference loop.
//!
//! `docs/spec/13-performance.md` §5 requires the scalar reference to stay in
//! the tree and to be compared against the vectorised path by a differential
//! test; see `tests/simd_differential.rs` and [`ScanMode`].

/// Which implementation [`find_next`] uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScanMode {
    /// `memchr`-based vectorised scanning (the default).
    #[default]
    Vector,
    /// The scalar reference loop, kept for differential testing.
    Scalar,
}

/// The set of bytes that ends a text run in a given tokenizer state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CharSet {
    /// Data and RCDATA states: `<`, `&`, NUL, CR.
    DataOrRcdata,
    /// RAWTEXT and script data states: `<`, NUL, CR.
    Rawtext,
    /// Script data (double) escaped states: `-`, `<`, NUL, CR.
    ScriptEscaped,
    /// PLAINTEXT state: NUL, CR.
    Plaintext,
    /// Double-quoted attribute value: `"`, `&`, NUL, CR.
    AttrDoubleQuoted,
    /// Single-quoted attribute value: `'`, `&`, NUL, CR.
    AttrSingleQuoted,
    /// Unquoted attribute value: whitespace, `&`, `>`, NUL, CR.
    AttrUnquoted,
}

impl CharSet {
    #[inline]
    pub(crate) fn contains(self, b: u8) -> bool {
        match self {
            CharSet::DataOrRcdata => matches!(b, b'<' | b'&' | b'\0' | b'\r'),
            CharSet::Rawtext => matches!(b, b'<' | b'\0' | b'\r'),
            CharSet::ScriptEscaped => matches!(b, b'-' | b'<' | b'\0' | b'\r'),
            CharSet::Plaintext => matches!(b, b'\0' | b'\r'),
            CharSet::AttrDoubleQuoted => matches!(b, b'"' | b'&' | b'\0' | b'\r'),
            CharSet::AttrSingleQuoted => matches!(b, b'\'' | b'&' | b'\0' | b'\r'),
            CharSet::AttrUnquoted => {
                matches!(
                    b,
                    b'\t' | b'\n' | b'\x0c' | b' ' | b'&' | b'>' | b'\0' | b'\r'
                )
            }
        }
    }
}

/// Offset of the first byte of `haystack` that is in `set`, or
/// `haystack.len()` if there is none.
#[inline]
pub(crate) fn find_next(mode: ScanMode, haystack: &[u8], set: CharSet) -> usize {
    match mode {
        ScanMode::Scalar => find_next_scalar(haystack, set),
        ScanMode::Vector => find_next_vector(haystack, set),
    }
}

/// Scalar reference implementation (`docs/spec/13-performance.md` §5).
#[inline]
pub(crate) fn find_next_scalar(haystack: &[u8], set: CharSet) -> usize {
    haystack
        .iter()
        .position(|&b| set.contains(b))
        .unwrap_or(haystack.len())
}

/// First byte of `haystack` matched by either `three` or `one`.
///
/// `memchr` has no four-needle search, so a four-member set needs two passes.
/// Running either pass over the whole haystack costs the distance to *its*
/// own needle, which can be far past the answer: a `<pre>` block with a `\r`
/// on every line has no `<` until the closing tag, so a whole-haystack search
/// for `<` would cost the rest of the document once per line and make the
/// document quadratic in its own length.
///
/// Scanning a window that grows geometrically bounds the total work at a
/// constant times the answer, while still handing `memchr` windows large
/// enough to run at full width once a run really is long. Each window is
/// searched once: a window that contained no needle cannot contain the answer.
#[inline]
fn find_first_of(
    haystack: &[u8],
    three: impl Fn(&[u8]) -> Option<usize>,
    one: impl Fn(&[u8]) -> Option<usize>,
) -> usize {
    /// Big enough that a short run costs one pass, small enough that a run
    /// ending after a few bytes does not scan a page. Measured on the
    /// `<pre>` fixture, whose runs are 34 bytes: at 256 the wasted tail of the
    /// first window costs ten percent against the scalar loop, at 64 it costs
    /// nothing, and a long run still reaches full width after a few doublings.
    const FIRST_WINDOW: usize = 64;
    const GROWTH: usize = 4;

    let len = haystack.len();
    let mut start = 0usize;
    let mut end = FIRST_WINDOW.min(len);
    loop {
        let window = &haystack[start..end];
        let bound = three(window).unwrap_or(window.len());
        if let Some(offset) = one(&window[..bound]) {
            return start + offset;
        }
        if bound < window.len() {
            return start + bound;
        }
        if end == len {
            return len;
        }
        start = end;
        end = end.saturating_mul(GROWTH).min(len);
    }
}

/// `memchr`-based implementation. Sets with more than three members are
/// scanned in two passes over a growing window; see [`find_first_of`].
#[inline]
fn find_next_vector(haystack: &[u8], set: CharSet) -> usize {
    let len = haystack.len();
    match set {
        CharSet::DataOrRcdata => find_first_of(
            haystack,
            |window| memchr::memchr3(b'<', b'&', b'\0', window),
            |window| memchr::memchr(b'\r', window),
        ),
        CharSet::Rawtext => memchr::memchr3(b'<', b'\0', b'\r', haystack).unwrap_or(len),
        CharSet::ScriptEscaped => find_first_of(
            haystack,
            |window| memchr::memchr3(b'-', b'<', b'\0', window),
            |window| memchr::memchr(b'\r', window),
        ),
        CharSet::Plaintext => memchr::memchr2(b'\0', b'\r', haystack).unwrap_or(len),
        CharSet::AttrDoubleQuoted => find_first_of(
            haystack,
            |window| memchr::memchr3(b'"', b'&', b'\0', window),
            |window| memchr::memchr(b'\r', window),
        ),
        CharSet::AttrSingleQuoted => find_first_of(
            haystack,
            |window| memchr::memchr3(b'\'', b'&', b'\0', window),
            |window| memchr::memchr(b'\r', window),
        ),
        // Eight members; attribute values are short, so the scalar loop wins.
        CharSet::AttrUnquoted => find_next_scalar(haystack, set),
    }
}

/// Number of `\n` bytes in `haystack`, for line tracking.
#[inline]
pub(crate) fn count_newlines(mode: ScanMode, haystack: &[u8]) -> u64 {
    match mode {
        ScanMode::Scalar => haystack.iter().filter(|&&b| b == b'\n').count() as u64,
        ScanMode::Vector => memchr::memchr_iter(b'\n', haystack).count() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETS: &[CharSet] = &[
        CharSet::DataOrRcdata,
        CharSet::Rawtext,
        CharSet::ScriptEscaped,
        CharSet::Plaintext,
        CharSet::AttrDoubleQuoted,
        CharSet::AttrSingleQuoted,
        CharSet::AttrUnquoted,
    ];

    #[test]
    fn vector_and_scalar_agree_on_edge_inputs() {
        let inputs: &[&[u8]] = &[
            b"",
            b"a",
            b"<",
            b"aaaa<bbbb",
            b"\0",
            b"\r\n",
            b"------------------------------<",
            b"0123456789abcdef0123456789abcdef&",
            b"0123456789abcdef0123456789abcde\r<",
        ];
        for set in SETS {
            for input in inputs {
                assert_eq!(
                    find_next_scalar(input, *set),
                    find_next(ScanMode::Vector, input, *set),
                    "set {set:?} input {input:?}"
                );
            }
        }
    }

    #[test]
    fn vector_and_scalar_agree_across_the_window_boundaries() {
        // The windowed search grows 256, 1024, 4096, …; a needle just before,
        // on and just after each boundary must still be found at the same
        // offset as the scalar loop finds it.
        for boundary in [0usize, 1, 255, 256, 257, 1023, 1024, 1025, 4095, 4096, 4097] {
            for needle in *b"<&\0\r-\"'" {
                let mut input = vec![b'x'; boundary + 8192];
                input[boundary] = needle;
                for set in SETS {
                    assert_eq!(
                        find_next_scalar(&input, *set),
                        find_next(ScanMode::Vector, &input, *set),
                        "set {set:?} needle {needle:?} at {boundary}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_near_needle_does_not_scan_the_whole_haystack() {
        // The regression this guards: a `\r` a few bytes in used to cost a
        // scan all the way to the far `<`, so a text-heavy document was
        // quadratic in its own length. Measured as work, not time: the search
        // must not look at more than a bounded multiple of the answer.
        let mut input = vec![b'x'; 4 << 20];
        input[10] = b'\r';
        let far = input.len() - 1;
        input[far] = b'<';
        assert_eq!(
            find_next(ScanMode::Vector, &input, CharSet::DataOrRcdata),
            10
        );

        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            std::hint::black_box(find_next(
                std::hint::black_box(ScanMode::Vector),
                std::hint::black_box(&input),
                CharSet::DataOrRcdata,
            ));
        }
        let elapsed = start.elapsed();
        println!("10 000 near-needle scans of a 4 MiB haystack: {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "10 000 scans took {elapsed:?}; the search is reading past the needle"
        );
    }
}
