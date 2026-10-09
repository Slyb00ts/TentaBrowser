//! The text-affecting properties shaping and line breaking read, as plain values.

use std::ops::Range;

use crate::types::Direction;

/// The `white-space` shorthand as far as shaping cares: whether soft wrapping
/// is on. Collapsing and trimming spaces happens before the text reaches
/// shaping, so every mode keeps the text it is given, and a segment break in the
/// text is always a forced line break.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(crate) enum WhiteSpace {
    /// `normal`.
    #[default]
    Normal,
    /// `nowrap`.
    Nowrap,
    /// `pre`.
    Pre,
    /// `pre-wrap`.
    PreWrap,
    /// `pre-line`.
    PreLine,
    /// `break-spaces`. Wraps like `pre-wrap`: spaces still hang at a soft wrap,
    /// because the shaper has no way to let them break the line.
    BreakSpaces,
}

impl WhiteSpace {
    /// Whether the mode allows soft wrap opportunities to be taken.
    pub(crate) fn wraps(self) -> bool {
        !matches!(self, Self::Nowrap | Self::Pre)
    }
}

/// `line-break`. `loose`, `normal` and `strict` differ only in how small kana
/// and iteration marks break, which the segmenter data does not distinguish, so
/// they are all `Auto`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(crate) enum LineBreak {
    /// `auto`: UAX #14 with the browser tailoring.
    #[default]
    Auto,
    /// `anywhere`: a soft wrap opportunity after every typographic character
    /// unit. It applies to the whole paragraph if any span asks for it.
    Anywhere,
}

/// `word-break`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(crate) enum WordBreak {
    /// `normal`.
    #[default]
    Normal,
    /// `break-all`.
    BreakAll,
    /// `keep-all`.
    KeepAll,
}

/// `overflow-wrap`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(crate) enum OverflowWrap {
    /// `normal`.
    #[default]
    Normal,
    /// `anywhere`: breaks inside words when a line overflows, and counts for
    /// min-content.
    Anywhere,
    /// `break-word`: like `anywhere`, but ignored for min-content.
    BreakWord,
}

/// One `font-feature-settings` entry.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct FontFeatureSetting {
    /// Four-byte OpenType feature tag such as `*b"kern"`.
    pub(crate) tag: [u8; 4],
    /// Zero turns the feature off, one on; larger values select alternates.
    pub(crate) value: u16,
}

/// Everything shaping reads from the computed style of a stretch of text.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct TextStyle {
    /// The CSS `font-family` list, as written.
    pub(crate) family_list: String,
    /// `font-size` in CSS pixels.
    pub(crate) size: f32,
    /// `font-weight` (1 to 1000).
    pub(crate) weight: u16,
    /// Whether `font-style` is italic or oblique.
    pub(crate) italic: bool,
    /// `letter-spacing` in pixels, added after every character.
    pub(crate) letter_spacing: f32,
    /// `word-spacing` in pixels, added to every word separator.
    pub(crate) word_spacing: f32,
    /// `font-feature-settings`, in declaration order.
    pub(crate) features: Vec<FontFeatureSetting>,
    /// The `lang` of the text, a BCP 47 tag; an unparsable tag is ignored.
    pub(crate) lang: Option<String>,
    /// `white-space`.
    pub(crate) white_space: WhiteSpace,
    /// `line-break`.
    pub(crate) line_break: LineBreak,
    /// `word-break`.
    pub(crate) word_break: WordBreak,
    /// `overflow-wrap`.
    pub(crate) overflow_wrap: OverflowWrap,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            family_list: "sans-serif".to_owned(),
            size: 16.0,
            weight: 400,
            italic: false,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            features: Vec::new(),
            lang: None,
            white_space: WhiteSpace::default(),
            line_break: LineBreak::default(),
            word_break: WordBreak::default(),
            overflow_wrap: OverflowWrap::default(),
        }
    }
}

/// A byte range of the paragraph text and the style it is shaped with.
#[derive(Clone, Debug)]
pub(crate) struct TextSpan<'a> {
    /// Byte range into the paragraph text, on character boundaries.
    pub(crate) range: Range<usize>,
    /// The style of that range.
    pub(crate) style: &'a TextStyle,
}

/// Text to shape: the characters, the styles of their ranges and the inline base
/// direction.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Paragraph<'a> {
    /// The text after white-space processing and `text-transform`.
    pub(crate) text: &'a str,
    /// Contiguous spans that tile `text` exactly.
    pub(crate) spans: &'a [TextSpan<'a>],
    /// The inline base direction (`direction`); it fixes the paragraph level
    /// even when the text starts with a strong character of the other direction.
    pub(crate) direction: Direction,
}

impl Paragraph<'_> {
    /// Panics unless the spans tile the text on character boundaries, which is a
    /// caller bug rather than an input to recover from.
    pub(crate) fn assert_tiled(&self) {
        let mut end = 0;
        for span in self.spans {
            assert!(
                span.range.start == end && span.range.end >= span.range.start,
                "text spans must be contiguous"
            );
            assert!(
                self.text.is_char_boundary(span.range.end),
                "text spans must end on character boundaries"
            );
            end = span.range.end;
        }
        assert!(end == self.text.len(), "text spans must cover the text");
    }
}
