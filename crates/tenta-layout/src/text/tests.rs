//! Behaviour of shaping, line breaking and measurement over the bundled fonts.

use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::GlyphMetrics;
use skrifa::{FontRef, MetadataProvider};

use super::shape::{ShapeOptions, shape_and_break};
use super::shaped::{LineEnd, ShapedLines};
use super::style::{
    FontFeatureSetting, LineBreak, OverflowWrap, Paragraph, TextSpan, TextStyle, WhiteSpace,
    WordBreak,
};
use super::transform::{TextTransform, apply};
use crate::embedded_fonts::EMBEDDED_FONTS;
use crate::font::{FontSystem, ResolvedFont};
use crate::types::Direction;

const SANS: &str = "Liberation Sans";
const SERIF: &str = "Liberation Serif";

fn style(family: &str, size: f32) -> TextStyle {
    TextStyle {
        family_list: family.to_owned(),
        size,
        ..TextStyle::default()
    }
}

fn shape_with(
    fonts: &mut FontSystem,
    text: &str,
    style: &TextStyle,
    direction: Direction,
    options: ShapeOptions,
) -> ShapedLines {
    let spans = [TextSpan {
        range: 0..text.len(),
        style,
    }];
    let paragraph = Paragraph {
        text,
        spans: &spans,
        direction,
    };
    shape_and_break(fonts, &paragraph, options)
}

fn shape_at(text: &str, style: &TextStyle, width: Option<f32>) -> ShapedLines {
    let mut fonts = FontSystem::new_embedded();
    shape_with(
        &mut fonts,
        text,
        style,
        Direction::Ltr,
        ShapeOptions {
            max_width: width,
            first_line_indent: 0.0,
        },
    )
}

fn line_texts<'a>(text: &'a str, shaped: &ShapedLines) -> Vec<&'a str> {
    shaped
        .lines
        .iter()
        .map(|line| &text[line.text_range.clone()])
        .collect()
}

fn embedded_bytes(file_name: &str) -> &'static [u8] {
    EMBEDDED_FONTS
        .iter()
        .find(|font| font.file_name == file_name)
        .expect("a bundled file")
        .bytes
}

/// The advance of each character summed straight from the font tables, with no
/// kerning and no ligatures.
fn skrifa_sum(bytes: &'static [u8], text: &str, size: f32) -> f32 {
    let font = FontRef::new(bytes).expect("a valid font");
    let charmap = font.charmap();
    let metrics = GlyphMetrics::new(&font, Size::new(size), LocationRef::default());
    text.chars()
        .map(|ch| {
            let glyph = charmap.map(ch).expect("a mapped character");
            metrics.advance_width(glyph).expect("an advance")
        })
        .sum()
}

fn no_kerning() -> Vec<FontFeatureSetting> {
    vec![
        FontFeatureSetting {
            tag: *b"kern",
            value: 0,
        },
        FontFeatureSetting {
            tag: *b"liga",
            value: 0,
        },
    ]
}

#[test]
fn hello_in_liberation_serif_matches_the_font_tables() {
    let mut serif = style(SERIF, 16.0);
    serif.features = no_kerning();
    let shaped = shape_at("Hello", &serif, None);
    assert_eq!(shaped.lines.len(), 1);
    let line = &shaped.lines[0];
    assert_eq!(line.runs.len(), 1);
    let run = &line.runs[0];
    assert_eq!(run.glyphs.glyphs.len(), 5);

    let expected = skrifa_sum(embedded_bytes("LiberationSerif-Regular.ttf"), "Hello", 16.0);
    assert!(
        (line.advance - expected).abs() < 0.01,
        "{} vs {expected}",
        line.advance
    );
    assert!((run.advance - expected).abs() < 0.01);

    let font = FontRef::new(embedded_bytes("LiberationSerif-Regular.ttf")).unwrap();
    let ids: Vec<u32> = "Hello"
        .chars()
        .map(|ch| font.charmap().map(ch).unwrap().to_u32())
        .collect();
    let shaped_ids: Vec<u32> = run.glyphs.glyphs.iter().map(|glyph| glyph.id).collect();
    assert_eq!(shaped_ids, ids);
    let xs: Vec<f32> = run.glyphs.glyphs.iter().map(|glyph| glyph.x).collect();
    assert_eq!(xs[0], 0.0);
    assert!(xs.windows(2).all(|pair| pair[1] > pair[0]));
    assert_eq!(run.glyphs.size, 16.0);
    assert!(!run.glyphs.synthetic_bold);
    assert_eq!(run.glyphs.skew, 0.0);
}

#[test]
fn sans_sentence_matches_the_font_tables_without_kerning() {
    let text = "The quick brown fox, 0123456789.";
    let mut sans = style(SANS, 16.0);
    sans.features = no_kerning();
    let shaped = shape_at(text, &sans, None);
    let expected = skrifa_sum(embedded_bytes("LiberationSans-Regular.ttf"), text, 16.0);
    assert_eq!(shaped.lines[0].runs[0].glyphs.glyphs.len(), text.len());
    assert!((shaped.width - expected).abs() < 0.01);
}

#[test]
fn kerning_is_on_by_default_and_a_feature_setting_turns_it_off() {
    let kerned = shape_at("AV", &style(SANS, 16.0), None).width;
    let mut plain = style(SANS, 16.0);
    plain.features = no_kerning();
    let unkerned = shape_at("AV", &plain, None).width;
    assert!(kerned < unkerned - 0.5, "{kerned} vs {unkerned}");
}

#[test]
fn letter_and_word_spacing_widen_the_text() {
    let base = shape_at("a b", &style(SANS, 16.0), None).width;
    let mut spaced = style(SANS, 16.0);
    spaced.letter_spacing = 2.0;
    assert!((shape_at("a b", &spaced, None).width - (base + 6.0)).abs() < 0.01);
    let mut words = style(SANS, 16.0);
    words.word_spacing = 5.0;
    assert!((shape_at("a b", &words, None).width - (base + 5.0)).abs() < 0.01);
}

#[test]
fn ascii_text_wraps_at_spaces_and_the_space_hangs() {
    let text = "alpha beta gamma delta";
    let sans = style(SANS, 16.0);
    let alpha_beta = shape_at("alpha beta", &sans, None).width;
    let gamma_delta = shape_at("gamma delta", &sans, None).width;
    assert!(gamma_delta > alpha_beta);
    let shaped = shape_at(text, &sans, Some(gamma_delta + 1.0));
    assert_eq!(
        line_texts(text, &shaped),
        ["alpha beta ", "gamma delta"],
        "the space stays on the line it ends"
    );
    let first = &shaped.lines[0];
    assert!((first.advance - alpha_beta).abs() < 0.01);
    assert!(first.hanging > 3.0);
    assert_eq!(first.end, LineEnd::Soft);
    assert_eq!(shaped.lines[1].end, LineEnd::Last);
    assert_eq!(shaped.lines[1].hanging, 0.0);
    assert!(
        shaped
            .lines
            .iter()
            .all(|line| line.advance <= gamma_delta + 1.0)
    );
}

#[test]
fn text_without_a_width_only_breaks_at_segment_breaks() {
    let text = "one two\nthree";
    let shaped = shape_at(text, &style(SANS, 16.0), None);
    assert_eq!(line_texts(text, &shaped), ["one two\n", "three"]);
    assert_eq!(shaped.lines[0].end, LineEnd::Forced);
}

#[test]
fn overflow_wrap_decides_whether_a_long_word_breaks() {
    let word = "Supercalifragilistic";
    let mut sans = style(SANS, 16.0);
    let full = shape_at(word, &sans, None).width;

    let kept = shape_at(word, &sans, Some(full / 3.0));
    assert_eq!(line_texts(word, &kept), [word], "normal overflows instead");

    for wrap in [OverflowWrap::Anywhere, OverflowWrap::BreakWord] {
        sans.overflow_wrap = wrap;
        let broken = shape_at(word, &sans, Some(full / 3.0));
        assert!(broken.lines.len() >= 3, "{wrap:?}");
        assert_eq!(line_texts(word, &broken).concat(), word);
        assert!(broken.lines.iter().all(|line| line.advance <= full / 3.0));
        assert!(
            broken
                .lines
                .iter()
                .take(broken.lines.len() - 1)
                .all(|line| line.end == LineEnd::Overflow)
        );
    }
}

#[test]
fn word_break_break_all_and_keep_all() {
    let text = "abcdefghij klmnop";
    let mut sans = style(SANS, 16.0);
    let word = shape_at("abcdefghij", &sans, None).width;
    sans.word_break = WordBreak::BreakAll;
    let broken = shape_at(text, &sans, Some(word - 5.0));
    assert!(line_texts(text, &broken)[0].len() < "abcdefghij ".len());

    // Without break-all the word does not fit and stays whole.
    let normal = shape_at(text, &style(SANS, 16.0), Some(word - 5.0));
    assert_eq!(line_texts(text, &normal), ["abcdefghij ", "klmnop"]);

    let cjk = "日本語日本語";
    let mut keep = style(SANS, 16.0);
    keep.word_break = WordBreak::KeepAll;
    let kept = shape_at(cjk, &keep, Some(20.0));
    assert_eq!(kept.lines.len(), 1, "keep-all does not break CJK runs");
    let free = shape_at(cjk, &style(SANS, 16.0), Some(20.0));
    assert!(free.lines.len() > 1);
}

#[test]
fn cjk_breaks_between_ideographs_but_not_before_closing_punctuation() {
    let text = "日本語、日本語、日本語";
    let sans = style(SANS, 16.0);
    let one = shape_at("日", &sans, None).width;
    for tenths in 12..60 {
        let width = one * tenths as f32 / 10.0;
        let shaped = shape_at(text, &sans, Some(width));
        let lines = line_texts(text, &shaped);
        assert_eq!(lines.concat(), text);
        assert!(lines.len() > 1, "width {width}");
        for line in &lines[1..] {
            assert!(
                !line.starts_with('、'),
                "a line starts with a closing mark at width {width}: {lines:?}"
            );
        }
        assert!(
            lines.iter().all(|line| line.chars().count() >= 1),
            "{lines:?}"
        );
    }
}

#[test]
fn line_break_anywhere_breaks_between_any_characters() {
    let text = "abc/def";
    let mut sans = style(SANS, 16.0);
    sans.line_break = LineBreak::Anywhere;
    let two_letters = shape_at("ab", &sans, None).width;
    let shaped = shape_at(text, &sans, Some(two_letters + 1.0));
    assert!(shaped.lines.len() >= 3);
    assert_eq!(line_texts(text, &shaped).concat(), text);
}

#[test]
fn url_breaks_after_slashes_and_not_inside_words() {
    let url = "https://example.com/path/to/page?x=1&y=2";
    let sans = style(SANS, 16.0);
    let full = shape_at(url, &sans, None).width;
    // Chromium's tailoring keeps slashes, `=` and `&` together and only
    // offers the position after `?`, so the URL has exactly one opportunity.
    for divisor in [2.0, 3.0, 5.0] {
        let shaped = shape_at(url, &sans, Some(full / divisor));
        assert_eq!(
            line_texts(url, &shaped),
            ["https://example.com/path/to/page?", "x=1&y=2"]
        );
    }
    let mut anywhere = sans.clone();
    anywhere.line_break = LineBreak::Anywhere;
    assert!(shape_at(url, &anywhere, Some(full / 5.0)).lines.len() >= 5);
}

#[test]
fn white_space_modes_decide_wrapping_and_preserved_breaks() {
    let sans = style(SANS, 16.0);
    let text = "aaa bbb\nccc  ddd";
    let narrow = Some(30.0);

    let mut pre = sans.clone();
    pre.white_space = WhiteSpace::Pre;
    let shaped = shape_at(text, &pre, narrow);
    assert_eq!(line_texts(text, &shaped), ["aaa bbb\n", "ccc  ddd"]);

    let mut nowrap = sans.clone();
    nowrap.white_space = WhiteSpace::Nowrap;
    let flat = "aaa bbb ccc";
    assert_eq!(shape_at(flat, &nowrap, narrow).lines.len(), 1);
    assert!(shape_at(flat, &sans, narrow).lines.len() > 1);

    let mut pre_wrap = sans.clone();
    pre_wrap.white_space = WhiteSpace::PreWrap;
    let shaped = shape_at(text, &pre_wrap, narrow);
    assert_eq!(line_texts(text, &shaped).concat(), text);
    assert!(
        shaped.lines.len() > 2,
        "pre-wrap wraps as well as preserving"
    );
    assert_eq!(shaped.lines[0].end, LineEnd::Soft);

    // Preserved spaces count: two spaces are wider than one.
    let one_space = shape_at("a b", &pre_wrap, None).width;
    let two_spaces = shape_at("a  b", &pre_wrap, None).width;
    assert!(two_spaces > one_space + 3.0);
}

#[test]
fn trailing_spaces_hang_when_wrapping_and_count_in_pre() {
    let wrap = style(SANS, 16.0);
    let mut pre = wrap.clone();
    pre.white_space = WhiteSpace::Pre;
    let trailing_wrap = shape_at("ab   \nc", &wrap, None);
    let trailing_pre = shape_at("ab   \nc", &pre, None);
    let bare = shape_at("ab", &wrap, None).width;
    assert!((trailing_wrap.lines[0].advance - bare).abs() < 0.01);
    assert!(trailing_wrap.lines[0].hanging > 8.0);
    assert!(trailing_pre.lines[0].advance > bare + 8.0);
    assert_eq!(trailing_pre.lines[0].hanging, 0.0);
}

#[test]
fn a_segment_break_after_a_space_still_ends_the_line() {
    let text = "ab \ncd \n\n ef";
    for white_space in [WhiteSpace::Pre, WhiteSpace::PreWrap, WhiteSpace::PreLine] {
        let mut sans = style(SANS, 16.0);
        sans.white_space = white_space;
        let shaped = shape_at(text, &sans, Some(500.0));
        assert_eq!(
            line_texts(text, &shaped),
            ["ab \n", "cd \n", "\n", " ef"],
            "{white_space:?}"
        );
    }
}

#[test]
fn a_trailing_segment_break_does_not_open_another_line() {
    let text = "one\ntwo\n";
    let mut pre = style(SANS, 16.0);
    pre.white_space = WhiteSpace::Pre;
    let shaped = shape_at(text, &pre, None);
    assert_eq!(line_texts(text, &shaped), ["one\n", "two\n"]);
    assert_eq!(shaped.lines[1].end, LineEnd::Last);

    let blank = "a\n\nb";
    let shaped = shape_at(blank, &pre, None);
    assert_eq!(line_texts(blank, &shaped), ["a\n", "\n", "b"]);
}

#[test]
fn empty_text_has_no_lines_and_no_width() {
    let sans = style(SANS, 16.0);
    let shaped = shape_at("", &sans, Some(100.0));
    assert!(shaped.lines.is_empty());
    assert_eq!(shaped.width, 0.0);
}

#[test]
fn bidi_run_order_follows_the_paragraph_direction() {
    let text = "abc שלום def";
    let sans = style(SANS, 16.0);
    let mut fonts = FontSystem::new_embedded();
    let ltr = shape_with(
        &mut fonts,
        text,
        &sans,
        Direction::Ltr,
        ShapeOptions::default(),
    );
    let runs = &ltr.lines[0].runs;
    // The space after the Hebrew word may be shaped as a run of its own.
    let mut directions: Vec<bool> = runs.iter().map(|run| run.rtl).collect();
    directions.dedup();
    assert_eq!(directions, [false, true, false]);
    let heb = text.find('ש').unwrap();
    let after = text.find("def").unwrap();
    assert_eq!(runs[0].text_range, 0..heb);
    assert_eq!(runs[1].text_range, heb..after - 1);
    assert_eq!(runs[2].text_range.start, after - 1);
    // Right-to-left clusters run visually left to right in the run, so their
    // byte ranges descend.
    let hebrew_clusters: Vec<usize> = runs[1]
        .clusters
        .iter()
        .map(|cluster| cluster.text_range.start)
        .collect();
    assert!(hebrew_clusters.windows(2).all(|pair| pair[0] > pair[1]));
    assert!(
        runs[1]
            .clusters
            .windows(2)
            .all(|pair| pair[1].x > pair[0].x)
    );
    assert!(runs.windows(2).all(|pair| pair[1].x > pair[0].x));

    let rtl = shape_with(
        &mut fonts,
        text,
        &sans,
        Direction::Rtl,
        ShapeOptions::default(),
    );
    let runs = &rtl.lines[0].runs;
    let starts: Vec<usize> = runs.iter().map(|run| run.text_range.start).collect();
    assert!(
        starts.windows(2).all(|pair| pair[0] > pair[1]),
        "a right-to-left paragraph shows its first words on the right: {starts:?}"
    );
    assert!((rtl.width - ltr.width).abs() < 0.01);
}

#[test]
fn the_declared_direction_wins_over_the_first_strong_character() {
    let text = "שלום abc";
    let sans = style(SANS, 16.0);
    let mut fonts = FontSystem::new_embedded();
    let ltr = shape_with(
        &mut fonts,
        text,
        &sans,
        Direction::Ltr,
        ShapeOptions::default(),
    );
    assert!(
        ltr.lines[0].runs[0].rtl,
        "Hebrew is shown first on the left"
    );
    assert_eq!(ltr.lines[0].runs[0].x, 0.0);
    assert_eq!(ltr.lines[0].text_range, 0..text.len());
    let rtl = shape_with(
        &mut fonts,
        text,
        &sans,
        Direction::Rtl,
        ShapeOptions::default(),
    );
    assert!(rtl.lines[0].runs.last().unwrap().rtl);
    assert!(!rtl.lines[0].runs[0].rtl);
}

#[test]
fn a_missing_glyph_falls_back_to_dejavu() {
    let text = "a\u{2713}b";
    let shaped = shape_at(text, &style(SANS, 16.0), None);
    let runs = &shaped.lines[0].runs;
    assert_eq!(runs.len(), 3);
    assert!(runs.iter().all(|run| run.span == 0));
    let dejavu = embedded_bytes("DejaVuSans.ttf");
    let sans = embedded_bytes("LiberationSans-Regular.ttf");
    assert_eq!(runs[0].glyphs.font.data.bytes().as_ptr(), sans.as_ptr());
    assert_eq!(runs[1].glyphs.font.data.bytes().as_ptr(), dejavu.as_ptr());
    assert_eq!(runs[2].glyphs.font, runs[0].glyphs.font);
    let check = FontRef::new(dejavu)
        .unwrap()
        .charmap()
        .map('\u{2713}')
        .unwrap();
    assert_eq!(runs[1].glyphs.glyphs[0].id, check.to_u32());
    assert_eq!(runs[1].text_range, 1..4);
    assert!(runs[1].advance > 0.0);
    let total: f32 = runs.iter().map(|run| run.advance).sum();
    assert!((shaped.lines[0].advance - total).abs() < 0.01);
    assert_eq!(runs[2].x, runs[0].advance + runs[1].advance);
}

#[test]
fn mixed_styles_split_runs_and_keep_their_own_sizes() {
    let mut fonts = FontSystem::new_embedded();
    let regular = style(SANS, 16.0);
    let mut bold = style(SANS, 24.0);
    bold.weight = 700;
    let text = "Hello World";
    let spans = [
        TextSpan {
            range: 0..6,
            style: &regular,
        },
        TextSpan {
            range: 6..11,
            style: &bold,
        },
    ];
    let paragraph = Paragraph {
        text,
        spans: &spans,
        direction: Direction::Ltr,
    };
    let shaped = shape_and_break(&mut fonts, &paragraph, ShapeOptions::default());
    let runs = &shaped.lines[0].runs;
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].span, runs[1].span), (0, 1));
    assert_eq!(runs[0].text_range, 0..6);
    assert_eq!(runs[1].text_range, 6..11);
    assert_eq!((runs[0].glyphs.size, runs[1].glyphs.size), (16.0, 24.0));
    assert_eq!(
        runs[1].glyphs.font.data.bytes().as_ptr(),
        embedded_bytes("LiberationSans-Bold.ttf").as_ptr()
    );
    assert!(!runs[1].glyphs.synthetic_bold, "a real bold face exists");
    assert_eq!(runs[1].x, runs[0].advance);
    let bold_world = skrifa_sum(embedded_bytes("LiberationSans-Bold.ttf"), "World", 24.0);
    assert!((runs[1].advance - bold_world).abs() < 0.5);
    assert!(runs[1].metrics.ascent > runs[0].metrics.ascent);
    assert_eq!(shaped.lines[0].metrics.ascent, runs[1].metrics.ascent);

    // Same font, two spans: the runs still split so each can carry its own box.
    let spans = [
        TextSpan {
            range: 0..6,
            style: &regular,
        },
        TextSpan {
            range: 6..11,
            style: &regular,
        },
    ];
    let paragraph = Paragraph {
        text,
        spans: &spans,
        direction: Direction::Ltr,
    };
    let split = shape_and_break(&mut fonts, &paragraph, ShapeOptions::default());
    assert_eq!(split.lines[0].runs.len(), 2);
    assert_eq!(
        split.lines[0].runs[0].glyphs.font,
        split.lines[0].runs[1].glyphs.font
    );
}

#[test]
fn a_style_boundary_inside_a_wrapped_paragraph_keeps_every_span_in_range() {
    let mut fonts = FontSystem::new_embedded();
    let regular = style(SANS, 16.0);
    let mut big = style(SANS, 32.0);
    big.weight = 700;
    let text = "alpha beta gamma delta";
    let spans = [
        TextSpan {
            range: 0..11,
            style: &regular,
        },
        TextSpan {
            range: 11..text.len(),
            style: &big,
        },
    ];
    let paragraph = Paragraph {
        text,
        spans: &spans,
        direction: Direction::Ltr,
    };
    let shaped = shape_and_break(
        &mut fonts,
        &paragraph,
        ShapeOptions {
            max_width: Some(150.0),
            first_line_indent: 0.0,
        },
    );
    assert!(shaped.lines.len() >= 2);
    for line in &shaped.lines {
        for run in &line.runs {
            let span = &spans[run.span];
            assert!(run.text_range.start >= span.range.start);
            assert!(run.text_range.end <= span.range.end);
        }
    }
}

#[test]
fn synthetic_styles_are_reported_per_run() {
    let mut dejavu = style("DejaVu Sans", 16.0);
    dejavu.weight = 700;
    let shaped = shape_at("Bold", &dejavu, None);
    assert!(shaped.lines[0].runs[0].glyphs.synthetic_bold);

    let mut italic = style(SANS, 16.0);
    italic.italic = true;
    let run = &shape_at("Slanted", &italic, None).lines[0].runs[0];
    assert_eq!(run.glyphs.skew, 0.25);
    assert!(!run.glyphs.synthetic_bold);

    let mut medium = style(SANS, 16.0);
    medium.weight = 500;
    let run = &shape_at("Medium", &medium, None).lines[0].runs[0];
    assert!(!run.glyphs.synthetic_bold, "500 stays regular");
    assert_eq!(
        run.glyphs.font.data.bytes().as_ptr(),
        embedded_bytes("LiberationSans-Regular.ttf").as_ptr()
    );

    let mut semibold = style("Liberation Mono", 16.0);
    semibold.weight = 600;
    let run = &shape_at("Semi", &semibold, None).lines[0].runs[0];
    assert!(!run.glyphs.synthetic_bold);
    assert_eq!(
        run.glyphs.font.data.bytes().as_ptr(),
        embedded_bytes("LiberationMono-Bold.ttf").as_ptr()
    );
}

#[test]
fn run_and_line_metrics_match_the_font_system() {
    let shaped = shape_at("Metrics", &style(SANS, 16.0), None);
    let expected = ResolvedFont::resolve(SANS, 400, false).vertical_metrics(16.0);
    let line = &shaped.lines[0];
    assert_eq!(line.runs[0].metrics, expected);
    assert_eq!(line.metrics, expected);
}

#[test]
fn clusters_map_text_offsets_to_positions() {
    let shaped = shape_at("abc", &style(SANS, 16.0), None);
    let run = &shaped.lines[0].runs[0];
    let ranges: Vec<_> = run
        .clusters
        .iter()
        .map(|cluster| cluster.text_range.clone())
        .collect();
    assert_eq!(ranges, [0..1, 1..2, 2..3]);
    let mut x = 0.0;
    for (cluster, glyph) in run.clusters.iter().zip(&run.glyphs.glyphs) {
        assert!((cluster.x - x).abs() < 0.001);
        assert!((cluster.x - glyph.x).abs() < 0.001);
        x += cluster.advance;
    }
    assert!((x - run.advance).abs() < 0.001);
}

#[test]
fn multi_byte_clusters_report_byte_ranges() {
    let text = "né→";
    let shaped = shape_at(text, &style(SANS, 16.0), None);
    let ranges: Vec<_> = shaped.lines[0]
        .runs
        .iter()
        .flat_map(|run| {
            run.clusters
                .iter()
                .map(|cluster| cluster.text_range.clone())
        })
        .collect();
    assert_eq!(ranges, [0..1, 1..3, 3..6]);
}

#[test]
fn first_line_indent_narrows_only_the_first_line() {
    let sans = style(SANS, 16.0);
    let text = "alpha beta gamma delta";
    let width = shape_at("alpha beta gamma", &sans, None).width + 1.0;
    let plain = shape_at(text, &sans, Some(width));
    let mut fonts = FontSystem::new_embedded();
    let indented = shape_with(
        &mut fonts,
        text,
        &sans,
        Direction::Ltr,
        ShapeOptions {
            max_width: Some(width),
            first_line_indent: 30.0,
        },
    );
    assert_eq!(line_texts(text, &plain), ["alpha beta gamma ", "delta"]);
    assert_eq!(line_texts(text, &indented)[0], "alpha beta ");
    assert_eq!(indented.lines[1].runs[0].x, 0.0);
}

#[test]
fn results_do_not_depend_on_the_font_system_instance() {
    let sans = style(SANS, 16.0);
    let text = "Deterministic output: same input, same glyphs. \u{2713} שלום";
    let a = shape_at(text, &sans, Some(180.0));
    let b = shape_at(text, &sans, Some(180.0));
    assert_eq!(a, b);
    assert!(a.lines.len() > 1);
}

#[test]
fn degenerate_sizes_and_widths_do_not_panic() {
    let zero = style(SANS, 0.0);
    let shaped = shape_at("zero", &zero, Some(10.0));
    assert!(!shaped.lines.is_empty());
    assert_eq!(shaped.width, 0.0);
    let nan = style(SANS, f32::NAN);
    shape_at("nan", &nan, None);
    shape_at("negative width", &style(SANS, 16.0), Some(-5.0));
    assert!(
        !shape_at("tab\tseparated", &style(SANS, 16.0), Some(f32::NAN))
            .lines
            .is_empty()
    );
}

#[test]
fn text_transform_maps_case() {
    assert_eq!(apply("keep", TextTransform::None, &mut false), "keep");
    assert_eq!(
        apply("Straße", TextTransform::Uppercase, &mut false),
        "STRASSE"
    );
    assert_eq!(
        apply("MiXed", TextTransform::Lowercase, &mut false),
        "mixed"
    );
    assert_eq!(
        apply(
            "hello wORLD-wide web 3d don't",
            TextTransform::Capitalize,
            &mut false
        ),
        "Hello WORLD-Wide Web 3d Don't"
    );
}

/// Widths Chromium reported for single-line text in the render corpus, as
/// (family, text, width). The fixtures that supply them set no font size, so the
/// text is 16px; `sans-serif` and the default serif resolve to the Liberation
/// faces on the reference machine.
const CORPUS_WIDTHS: &[(&str, &str, f32)] = &[
    (SANS, "short text beside the float.", 183.22),
    (SANS, "CLEARED: full width below", 192.97),
    (SANS, "after content flows normally.", 198.02),
    (SANS, "content-box", 83.61),
    (SANS, "border-box", 77.38),
    (SANS, "50% content-box", 120.08),
    (SANS, "50% border-box", 113.84),
    (SANS, "inherited border-box", 143.2),
    (
        SANS,
        "Every bar must be green and about 320px wide.",
        340.66,
    ),
    (SANS, "stylesheet important beats later normal", 275.69),
    (SANS, "stylesheet important beats inline normal", 281.92),
    (SANS, "inline important beats stylesheet important", 299.72),
    (SANS, "inline important beats later inline normal", 282.81),
    (SANS, "important custom property wins", 222.31),
    (SANS, "author CSS beats a presentational hint", 274.84),
    (SANS, "block child: must span the full 400px", 257.03),
    (SANS, "trailing inline text after the block child", 261.47),
    (SANS, "contentless wrapper w/ abs fill", 213.44),
    (SANS, "hydration contracts passed", 191.22),
    (SANS, "streamed hydration passed", 193.0),
    (SERIF, "A rel(auto-width)+BLOCK, abs inset:0:", 255.22),
    (SERIF, "B rel+FLEX, abs inset:0:", 161.02),
    (SERIF, "C rel+GRID, abs inset:0:", 160.13),
    (SERIF, "D abs width:100%/height:100%:", 209.77),
    (SERIF, "E padding-hack ratio + abs fill IMG:", 233.88),
];

#[test]
fn corpus_text_widths_match_chromium() {
    let mut worst = 0.0_f32;
    for &(family, text, expected) in CORPUS_WIDTHS {
        let width = shape_at(text, &style(family, 16.0), None).width;
        let error = (width - expected).abs() / expected;
        worst = worst.max(error);
        let bound = if text.contains("fill") {
            // See `chromium_ligates_fi_where_the_bundled_serif_cannot`.
            0.004
        } else {
            0.0005
        };
        assert!(
            error < bound,
            "{family} {text:?}: {width} vs {expected} ({:.4}%)",
            error * 100.0
        );
    }
    eprintln!(
        "max relative width error over {} strings: {:.4}%",
        CORPUS_WIDTHS.len(),
        worst * 100.0
    );
}

#[test]
fn chromium_ligates_fi_where_the_bundled_serif_cannot() {
    use skrifa::raw::TableProvider;
    let bytes = embedded_bytes("LiberationSerif-Regular.ttf");
    let font = FontRef::new(bytes).unwrap();
    let tags: Vec<_> = font
        .gsub()
        .unwrap()
        .feature_list()
        .unwrap()
        .feature_records()
        .iter()
        .map(|record| record.feature_tag().to_string())
        .collect();
    assert!(
        !tags.iter().any(|tag| tag == "liga" || tag == "clig"),
        "the file has no standard ligature feature: {tags:?}"
    );
    // So "fi" stays two glyphs, as the file dictates.
    for (text, glyphs) in [("fill", 4), ("office", 6), ("fi", 2)] {
        let shaped = shape_at(text, &style(SERIF, 16.0), None);
        assert_eq!(
            shaped.lines[0].runs[0].glyphs.glyphs.len(),
            glyphs,
            "{text}"
        );
    }
    // Chromium's 0.86 px narrower "fill" matches replacing f+i by the file's
    // U+FB01 glyph (0.875 px narrower at 16 px), so the reference machine
    // ligated through a different Liberation Serif build.
    let metrics = GlyphMetrics::new(&font, Size::new(16.0), LocationRef::default());
    let advance = |ch: char| {
        metrics
            .advance_width(font.charmap().map(ch).unwrap())
            .unwrap()
    };
    let saved = advance('f') + advance('i') - advance('\u{FB01}');
    let ours = shape_at(
        "E padding-hack ratio + abs fill IMG:",
        &style(SERIF, 16.0),
        None,
    )
    .width;
    assert!((ours - saved - 233.88).abs() < 0.05, "{ours} - {saved}");
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

/// A timing probe for a ~5 KB paragraph. Run it in release with
/// `cargo nextest run -p tenta-layout --release --run-ignored only text::tests::paragraph_timing`.
#[test]
#[ignore = "timing probe, meaningful only in release"]
fn paragraph_timing() {
    use std::time::Instant;
    const SENTENCES: [&str; 6] = [
        "The quick brown fox jumps over the lazy dog while the rain keeps falling on the roof. ",
        "Layout engines spend most of their time shaping text, so the cache matters more than the breaker. ",
        "Some words are considerably longer, like internationalization and counterrevolutionaries. ",
        "Numbers such as 3.14159, 2,718 and 1,000,000 appear, as do URLs like https://example.com/a/b?c=d. ",
        "Short ones: a, an, of, to, in. ",
        "Quotes \u{201C}curly\u{201D} and dashes \u{2014} are also here, with caf\u{e9} and na\u{ef}ve. ",
    ];
    let mut paragraph_text = String::new();
    while paragraph_text.len() < 5000 {
        paragraph_text.push_str(SENTENCES[paragraph_text.len() % SENTENCES.len()]);
    }
    let sans = style(SANS, 16.0);
    let mut fonts = FontSystem::new_embedded();
    let run = |fonts: &mut FontSystem, text: &str, width: f32| {
        let spans = [TextSpan {
            range: 0..text.len(),
            style: &sans,
        }];
        let paragraph = Paragraph {
            text,
            spans: &spans,
            direction: Direction::Ltr,
        };
        let start = Instant::now();
        let shaped = shape_and_break(
            fonts,
            &paragraph,
            ShapeOptions {
                max_width: Some(width),
                first_line_indent: 0.0,
            },
        );
        (start.elapsed().as_secs_f64() * 1000.0, shaped.lines.len())
    };
    // The first call pays for loading the fonts and building shaping data.
    run(&mut fonts, &paragraph_text, 600.0);

    let mut cold = Vec::new();
    let mut warm = Vec::new();
    let mut lines = 0;
    for index in 0..40 {
        // A different prefix per run defeats the paragraph cache.
        let text = format!("{index} {paragraph_text}");
        let (millis, count) = run(&mut fonts, &text, 600.0);
        if index >= 10 {
            cold.push(millis);
            lines = count;
        }
        let (millis, _) = run(&mut fonts, &text, 400.0 + index as f32);
        if index >= 10 {
            warm.push(millis);
        }
    }
    for samples in [&mut cold, &mut warm] {
        samples.sort_by(f64::total_cmp);
    }
    eprintln!(
        "{} bytes, {lines} lines at 600px; shape+break p50 {:.3} ms p95 {:.3} ms; re-break (cached) p50 {:.3} ms p95 {:.3} ms ({} samples each)",
        paragraph_text.len(),
        percentile(&cold, 0.5),
        percentile(&cold, 0.95),
        percentile(&warm, 0.5),
        percentile(&warm, 0.95),
        cold.len()
    );
}

#[test]
fn break_all_applies_only_to_the_span_that_asks() {
    let mut fonts = FontSystem::new_embedded();
    let normal = style(SANS, 16.0);
    let mut break_all = style(SANS, 16.0);
    break_all.word_break = WordBreak::BreakAll;
    let text = "abcdefghij klmnopqrstuvwx";
    let narrow = shape_at("abcdefghij", &normal, None).width + 3.0;
    let spans = [
        TextSpan {
            range: 0..11,
            style: &normal,
        },
        TextSpan {
            range: 11..text.len(),
            style: &break_all,
        },
    ];
    let paragraph = Paragraph {
        text,
        spans: &spans,
        direction: Direction::Ltr,
    };
    let options = ShapeOptions {
        max_width: Some(narrow),
        first_line_indent: 0.0,
    };
    let mixed = shape_and_break(&mut fonts, &paragraph, options);
    let lines = line_texts(text, &mixed);
    assert_eq!(lines[0], "abcdefghij ", "the normal word stays whole");
    assert!(lines.len() >= 3, "the break-all span splits: {lines:?}");
    assert_eq!(lines.concat(), text);

    // Swapped: break-all text first, then a normal word that must not split.
    let spans = [
        TextSpan {
            range: 0..11,
            style: &break_all,
        },
        TextSpan {
            range: 11..text.len(),
            style: &normal,
        },
    ];
    let paragraph = Paragraph {
        text,
        spans: &spans,
        direction: Direction::Ltr,
    };
    let swapped = shape_and_break(&mut fonts, &paragraph, options);
    let lines = line_texts(text, &swapped);
    assert_eq!(lines.concat(), text);
    assert_eq!(lines.last().copied(), Some("klmnopqrstuvwx"));
}

#[test]
fn line_break_anywhere_applies_only_to_the_span_that_asks() {
    let mut fonts = FontSystem::new_embedded();
    let normal = style(SANS, 16.0);
    let mut anywhere = style(SANS, 16.0);
    anywhere.line_break = LineBreak::Anywhere;
    let text = "wholeword brokenapart";
    let spans = [
        TextSpan {
            range: 0..10,
            style: &normal,
        },
        TextSpan {
            range: 10..text.len(),
            style: &anywhere,
        },
    ];
    let paragraph = Paragraph {
        text,
        spans: &spans,
        direction: Direction::Ltr,
    };
    let shaped = shape_and_break(
        &mut fonts,
        &paragraph,
        ShapeOptions {
            max_width: Some(shape_at("wholeword", &normal, None).width + 2.0),
            first_line_indent: 0.0,
        },
    );
    let lines = line_texts(text, &shaped);
    assert_eq!(lines[0], "wholeword ");
    assert!(lines.len() >= 3, "{lines:?}");
}

#[test]
fn line_break_anywhere_never_leaves_an_empty_line() {
    let mut anywhere = style(SANS, 16.0);
    anywhere.line_break = LineBreak::Anywhere;
    let mut fonts = FontSystem::new_embedded();
    for (text, direction) in [
        ("abc def", Direction::Ltr),
        ("abc def", Direction::Rtl),
        ("日本語", Direction::Ltr),
    ] {
        let shaped = shape_with(
            &mut fonts,
            text,
            &anywhere,
            direction,
            ShapeOptions {
                max_width: Some(1.0),
                first_line_indent: 0.0,
            },
        );
        assert!(
            shaped.lines.iter().all(|line| !line.text_range.is_empty()),
            "{text:?} {direction:?}"
        );
        assert_eq!(line_texts(text, &shaped).concat(), text);
        assert!(shaped.lines.len() >= text.chars().filter(|ch| !ch.is_whitespace()).count());
    }
}

#[test]
fn right_to_left_lines_put_the_hanging_spaces_at_the_left() {
    let text = "שלום עולם שלום עולם שלום";
    let sans = style(SANS, 16.0);
    let mut fonts = FontSystem::new_embedded();
    let word = shape_at("שלום", &sans, None).width;
    let shaped = shape_with(
        &mut fonts,
        text,
        &sans,
        Direction::Rtl,
        ShapeOptions {
            max_width: Some(word * 2.3),
            first_line_indent: 0.0,
        },
    );
    assert_eq!(shaped.lines.len(), 3);
    let space = shape_at(" ", &sans, None).lines[0].hanging;
    let mut chars = 0;
    for line in &shaped.lines[..2] {
        assert!((line.hanging - space).abs() < 0.01, "{}", line.hanging);
        assert_eq!(line.content_x, line.hanging);
        let total: f32 = line.runs.iter().map(|run| run.advance).sum();
        assert!((total - (line.advance + line.hanging)).abs() < 0.01);
        assert_eq!(line.runs[0].x, 0.0);
        for pair in line.runs.windows(2) {
            assert!((pair[1].x - (pair[0].x + pair[0].advance)).abs() < 0.001);
        }
        // The trailing space is the leftmost cluster; the content follows it.
        let clusters: Vec<_> = line.runs.iter().flat_map(|run| &run.clusters).collect();
        assert_eq!(clusters[0].x, 0.0);
        assert_eq!(&text[clusters[0].text_range.clone()], " ");
        assert!((clusters[1].x - line.content_x).abs() < 0.01);
        assert!(line.advance <= word * 2.3);
    }
    for line in &shaped.lines {
        for run in &line.runs {
            assert!(run.rtl);
            // Hebrew without ligatures: one glyph per cluster.
            assert_eq!(run.glyphs.glyphs.len(), run.clusters.len());
            chars += run.clusters.len();
        }
    }
    assert_eq!(chars, text.chars().count());
    let last = &shaped.lines[2];
    assert_eq!((last.hanging, last.content_x), (0.0, 0.0));
}

#[test]
fn left_to_right_lines_keep_content_at_the_left() {
    let sans = style(SANS, 16.0);
    let text = "alpha beta gamma";
    let shaped = shape_at(
        text,
        &sans,
        Some(shape_at("alpha beta", &sans, None).width + 1.0),
    );
    let first = &shaped.lines[0];
    assert!(first.hanging > 0.0);
    assert_eq!(first.content_x, 0.0);
    let clusters: Vec<_> = first.runs.iter().flat_map(|run| &run.clusters).collect();
    let space = clusters.last().unwrap();
    assert_eq!(&text[space.text_range.clone()], " ");
    assert!((space.x - first.advance).abs() < 0.01);
}

#[test]
fn glyph_y_is_positive_down_from_the_baseline() {
    let serif = style(SERIF, 32.0);
    let y_of_mark = |text: &str| {
        let shaped = shape_at(text, &serif, None);
        let glyphs = &shaped.lines[0].runs[0].glyphs.glyphs;
        assert_eq!(glyphs.len(), 2, "{text:?} keeps a separate mark glyph");
        assert_eq!(glyphs[0].y, 0.0, "base glyphs sit on the baseline");
        glyphs[1].y
    };
    let below = y_of_mark("q\u{323}");
    let above_tall = y_of_mark("b\u{302}");
    let above_short = y_of_mark("x\u{301}");
    assert!(below > 0.0, "a dot below the base moves down: {below}");
    assert!(
        above_tall < 0.0,
        "a circumflex over an ascender moves up: {above_tall}"
    );
    assert!(
        above_tall < above_short,
        "taller bases lift their marks higher"
    );
}

fn chunk_of(
    fonts: &mut FontSystem,
    text: &str,
    style: &TextStyle,
) -> std::rc::Rc<super::chunk::ShapedChunk> {
    let spans = [TextSpan {
        range: 0..text.len(),
        style,
    }];
    let paragraph = Paragraph {
        text,
        spans: &spans,
        direction: Direction::Ltr,
    };
    super::shape::shape_chunk(fonts, &paragraph)
}

#[test]
fn chunk_pieces_match_the_reference_line_breaker() {
    let mut fonts = FontSystem::new_embedded();
    let sans = style(SANS, 16.0);
    let text = "The quick brown fox jumps over the lazy dog, then rests.";
    let chunk = chunk_of(&mut fonts, text, &sans);
    let reference = shape_at(text, &sans, Some(0.0));
    let mut start = 0;
    let mut ends: Vec<u32> = chunk.breaks.iter().map(|b| b.end).collect();
    ends.push(text.len() as u32);
    assert_eq!(ends.len(), reference.lines.len());
    for (end, line) in ends.iter().zip(&reference.lines) {
        assert_eq!(line.text_range, start as usize..*end as usize);
        let clusters = chunk.cluster_range(start, *end);
        let width = chunk.width_of(&clusters);
        assert!(
            (width - (line.advance + line.hanging)).abs() < 0.01,
            "{width}"
        );
        start = *end;
    }
    assert!((chunk.width - reference_width(text, &sans)).abs() < 0.01);
}

fn reference_width(text: &str, style: &TextStyle) -> f32 {
    shape_at(text, style, None).width
}

#[test]
fn shaping_a_chunk_again_hits_the_cache() {
    let mut fonts = FontSystem::new_embedded();
    let sans = style(SANS, 16.0);
    let before = super::chunk::stats();
    let first = chunk_of(&mut fonts, "cache me if you can", &sans);
    let second = chunk_of(&mut fonts, "cache me if you can", &sans);
    let after = super::chunk::stats();
    assert!(std::rc::Rc::ptr_eq(&first, &second));
    assert_eq!(after.misses, before.misses + 1);
    assert_eq!(after.hits, before.hits + 1);
    let mut bold = sans.clone();
    bold.weight = 700;
    let third = chunk_of(&mut fonts, "cache me if you can", &bold);
    assert!(!std::rc::Rc::ptr_eq(&first, &third));
}

#[test]
fn the_cache_evicts_the_least_recently_used_chunk() {
    let mut fonts = FontSystem::new_embedded();
    let sans = style(SANS, 16.0);
    let favourite = chunk_of(&mut fonts, "always needed", &sans);
    for i in 0..300 {
        chunk_of(&mut fonts, &format!("filler number {i}"), &sans);
        let again = chunk_of(&mut fonts, "always needed", &sans);
        assert!(std::rc::Rc::ptr_eq(&favourite, &again), "evicted at {i}");
    }
    assert!(super::chunk::stats().entries <= 64);
}

#[test]
fn a_nan_size_shapes_without_filling_the_cache() {
    let mut fonts = FontSystem::new_embedded();
    let mut broken = style(SANS, 16.0);
    broken.size = f32::NAN;
    let before = super::chunk::stats();
    let chunk = chunk_of(&mut fonts, "not a number", &broken);
    assert!(chunk.glyphs.is_empty() || chunk.width == 0.0);
    assert_eq!(super::chunk::stats().entries, before.entries);
}

#[test]
fn capitalize_continues_a_word_across_calls() {
    let mut in_word = false;
    assert_eq!(
        apply("hello f", TextTransform::Capitalize, &mut in_word),
        "Hello F"
    );
    assert!(in_word);
    assert_eq!(
        apply("oo wor", TextTransform::Capitalize, &mut in_word),
        "oo Wor"
    );
    assert_eq!(apply("ld", TextTransform::Capitalize, &mut in_word), "ld");
    assert_eq!(apply(" x", TextTransform::Capitalize, &mut in_word), " X");
}
