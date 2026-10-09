use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use skrifa::instance::{LocationRef, Size};
use skrifa::{GlyphId, MetadataProvider};
use tenta_layout::embedded_fonts::EMBEDDED_FONTS;
use tenta_layout::{Color, FontBlob, FontRef, Glyph, GlyphRun, LineStyle, Rect};
use tiny_skia::{Mask, Pixmap, Transform};

use super::{cache, draw_decoration, draw_text};
use crate::dl::{DisplayList, Item};
use crate::image::ImageStore;
use crate::raster::{RasterOptions, rasterize};

/// Golden comparisons allow this much difference per channel until the
/// cross-platform determinism gate proves byte-identical output.
const GOLDEN_TOLERANCE: u8 = 1;

const SANS: usize = 0;
const SERIF: usize = 2;
const MONO: usize = 4;

fn font(index: usize) -> FontRef {
    FontRef {
        data: Arc::new(FontBlob::Static(EMBEDDED_FONTS[index].bytes)),
        index: 0,
        coords: Arc::from([]),
    }
}

/// Lays `text` out left to right with the font's own advances, no kerning.
fn run(text: &str, font: &FontRef, size: f32) -> GlyphRun {
    let parsed = skrifa::FontRef::from_index(font.data.bytes(), 0).expect("embedded font parses");
    let charmap = parsed.charmap();
    let metrics = parsed.glyph_metrics(Size::new(size), LocationRef::default());
    let mut x = 0.0;
    let glyphs = text
        .chars()
        .map(|ch| {
            let id = charmap.map(ch).unwrap_or(GlyphId::NOTDEF);
            let glyph = Glyph {
                id: id.to_u32(),
                x,
                y: 0.0,
            };
            x += metrics.advance_width(id).unwrap_or(0.0);
            glyph
        })
        .collect();
    GlyphRun {
        font: font.clone(),
        size,
        glyphs,
        synthetic_bold: false,
        skew: 0.0,
    }
}

fn white_canvas(width: u32, height: u32) -> Pixmap {
    let mut canvas = Pixmap::new(width, height).expect("canvas allocates");
    canvas.fill(tiny_skia::Color::WHITE);
    canvas
}

fn draw(canvas: &mut Pixmap, origin: (f32, f32), run: &GlyphRun) -> usize {
    draw_text(
        canvas,
        Transform::identity(),
        origin,
        run,
        Color::BLACK,
        None,
    )
}

fn alpha_at(canvas: &Pixmap, x: u32, y: u32) -> u8 {
    canvas.pixels()[(y * canvas.width() + x) as usize].alpha()
}

/// Darkness of the red channel on a white canvas, 0 (white) to 255 (black).
fn ink(canvas: &Pixmap, x: u32, y: u32) -> u32 {
    255 - u32::from(canvas.pixels()[(y * canvas.width() + x) as usize].red())
}

fn total_ink(canvas: &Pixmap) -> u32 {
    (0..canvas.height())
        .flat_map(|y| (0..canvas.width()).map(move |x| (x, y)))
        .map(|(x, y)| ink(canvas, x, y))
        .sum()
}

fn column_ink(canvas: &Pixmap, x: u32) -> u32 {
    (0..canvas.height()).map(|y| ink(canvas, x, y)).sum()
}

fn row_ink(canvas: &Pixmap, y: u32) -> u32 {
    (0..canvas.width()).map(|x| ink(canvas, x, y)).sum()
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/render-goldens")
        .join(format!("{name}.png"))
}

fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .expect("valid PNG header");
    let mut buffer = vec![0; reader.output_buffer_size().expect("a frame fits in memory")];
    let info = reader.next_frame(&mut buffer).expect("valid PNG frame");
    buffer.truncate(info.buffer_size());
    (info.width, info.height, buffer)
}

/// Compares `canvas` with the blessed golden `name`; `UPDATE_GOLDEN=1`
/// rewrites it instead.
fn assert_golden(name: &str, canvas: &Pixmap) {
    let encoded = canvas.encode_png().expect("canvas encodes");
    let path = golden_path(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &encoded).expect("golden is writable");
        return;
    }
    let golden = std::fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "missing golden {}: {err}; bless it with UPDATE_GOLDEN=1",
            path.display()
        )
    });
    let (w, h, expected) = decode_png(&golden);
    let (actual_w, actual_h, actual) = decode_png(&encoded);
    assert_eq!(
        (actual_w, actual_h),
        (w, h),
        "golden {name} has another size"
    );
    let worst = expected
        .iter()
        .zip(&actual)
        .map(|(e, a)| e.abs_diff(*a))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= GOLDEN_TOLERANCE,
        "golden {name} differs by {worst} per channel"
    );
}

#[test]
fn golden_hg_in_liberation_sans_16() {
    let mut canvas = white_canvas(40, 28);
    assert_eq!(
        draw(&mut canvas, (4.0, 20.0), &run("Hg", &font(SANS), 16.0)),
        0
    );
    assert_golden("glyph-hg-liberation-sans-16", &canvas);
}

#[test]
fn golden_hamburgefonstiv_in_three_families_and_three_sizes() {
    let mut canvas = white_canvas(420, 250);
    let mut baseline = 0.0;
    for family in [SANS, SERIF, MONO] {
        for size in [12.0, 16.0, 32.0] {
            baseline += size * 1.3;
            let text = run("Hamburgefonstiv 0123", &font(family), size);
            assert_eq!(draw(&mut canvas, (4.0, baseline), &text), 0);
        }
    }
    assert_golden("glyph-hamburgefonstiv-families", &canvas);
}

#[test]
fn golden_synthetic_bold_and_oblique() {
    let mut canvas = white_canvas(300, 130);
    let regular = run("Hamburg", &font(SANS), 32.0);
    let mut bold = regular.clone();
    bold.synthetic_bold = true;
    let mut oblique = regular.clone();
    oblique.skew = 0.25;
    let mut both = bold.clone();
    both.skew = 0.25;
    assert_eq!(draw(&mut canvas, (4.0, 32.0), &regular), 0);
    assert_eq!(draw(&mut canvas, (4.0, 64.0), &bold), 0);
    assert_eq!(draw(&mut canvas, (4.0, 96.0), &oblique), 0);
    assert_eq!(draw(&mut canvas, (4.0, 124.0), &both), 0);
    assert_golden("glyph-synthetic-styles", &canvas);
}

#[test]
fn quarter_pixel_positions_differ_and_repeat() {
    let letter = run("l", &font(SANS), 16.0);
    let render = |x: f32| {
        let mut canvas = white_canvas(24, 24);
        draw(&mut canvas, (x, 18.0), &letter);
        canvas.data().to_vec()
    };
    let positions = [10.0, 10.25, 10.5, 10.75].map(render);
    for (i, a) in positions.iter().enumerate() {
        for b in &positions[i + 1..] {
            assert_ne!(a, b, "quarter positions must render differently");
        }
    }
    assert!(positions[0] == render(10.0), "a position repeats");
    assert!(
        positions[0] == render(10.1),
        "10.1 rounds down to the .0 step"
    );
    assert!(positions[1] == render(10.2), "10.2 rounds to the .25 step");
    let shifted = {
        let mut canvas = white_canvas(24, 24);
        draw(&mut canvas, (11.0, 18.0), &letter);
        canvas
    };
    let base = {
        let mut canvas = white_canvas(24, 24);
        draw(&mut canvas, (10.0, 18.0), &letter);
        canvas
    };
    for y in 0..24 {
        for x in 0..23 {
            assert_eq!(ink(&base, x, y), ink(&shifted, x + 1, y));
        }
    }
}

#[test]
fn baseline_snaps_to_a_whole_pixel() {
    let letter = run("H", &font(SANS), 16.0);
    let at = |y: f32| {
        let mut canvas = white_canvas(24, 24);
        draw(&mut canvas, (4.0, y), &letter);
        canvas.data().to_vec()
    };
    assert_eq!(at(18.0), at(18.3));
    assert_ne!(at(18.0), at(18.6));
}

#[test]
fn text_is_cut_by_a_rounded_clip() {
    let big = run("HH", &font(SANS), 48.0);
    let text = Item::Text {
        origin: (2.0, 52.0),
        run: Arc::new(big),
        color: Color::BLACK,
    };
    let render = |items: Vec<Item>| {
        let list = DisplayList {
            items,
            bounds: Rect::default(),
            ..DisplayList::default()
        };
        let options = RasterOptions {
            width: 80,
            height: 64,
            scale: 1.0,
            origin: (0.0, 0.0),
            background: Some(Color::WHITE),
        };
        let out = rasterize(&list, &options, &ImageStore::new()).expect("valid size");
        assert!(out.stats.is_complete());
        out.pixmap.data().to_vec()
    };
    let free = render(vec![text.clone()]);
    // A circle of radius 20 centred at (30, 30): the left stem of the first
    // H reaches far outside it.
    let clipped = render(vec![
        Item::PushClip {
            rect: Rect::new(10.0, 10.0, 40.0, 40.0),
            radii: [20.0; 8],
        },
        text,
        Item::PopClip,
    ]);
    let pixel = |data: &[u8], x: usize, y: usize| data[(y * 80 + x) * 4];
    let mut inside_ink = 0;
    for y in 0..64 {
        for x in 0..80 {
            let (dx, dy) = (x as f32 + 0.5 - 30.0, y as f32 + 0.5 - 30.0);
            let radius = (dx * dx + dy * dy).sqrt();
            if radius > 21.0 {
                assert_eq!(
                    pixel(&clipped, x, y),
                    255,
                    "ink outside the circle at {x},{y}"
                );
            } else if radius < 19.0 {
                assert_eq!(
                    pixel(&clipped, x, y),
                    pixel(&free, x, y),
                    "inside at {x},{y}"
                );
                inside_ink += u32::from(pixel(&clipped, x, y) < 128);
            }
        }
    }
    assert!(inside_ink > 50, "the clip must still show some glyph");
    assert_ne!(free, clipped);
}

#[test]
fn clip_mask_cuts_the_blit() {
    let mut mask = Mask::new(24, 24).expect("mask allocates");
    for y in 0..24usize {
        for x in 12..24usize {
            mask.data_mut()[y * 24 + x] = 255;
        }
    }
    let letters = run("HH", &font(SANS), 16.0);
    let mut canvas = white_canvas(24, 24);
    draw_text(
        &mut canvas,
        Transform::identity(),
        (1.0, 18.0),
        &letters,
        Color::BLACK,
        Some(&mask),
    );
    assert!((0..12).all(|x| column_ink(&canvas, x) == 0));
    assert!((12..24).any(|x| column_ink(&canvas, x) > 0));
}

#[test]
fn synthetic_bold_widens_and_oblique_leans() {
    let regular = run("H", &font(SANS), 32.0);
    let mut bold = regular.clone();
    bold.synthetic_bold = true;
    let mut oblique = regular.clone();
    oblique.skew = 0.25;
    let render = |run: &GlyphRun| {
        let mut canvas = white_canvas(48, 48);
        draw(&mut canvas, (6.0, 40.0), run);
        canvas
    };
    let (plain, heavy, slanted) = (render(&regular), render(&bold), render(&oblique));
    // 32 / 24 = 1.33 px more ink across each of the two stems.
    let (plain_ink, heavy_ink) = (total_ink(&plain), total_ink(&heavy));
    assert!(
        heavy_ink > plain_ink + plain_ink / 10,
        "{plain_ink} {heavy_ink}"
    );
    let centroid = |canvas: &Pixmap, rows: std::ops::Range<u32>| {
        let (mut sum, mut weight) = (0.0, 0.0);
        for y in rows {
            for x in 0..48 {
                let i = ink(canvas, x, y) as f32;
                sum += i * (x as f32 + 0.5);
                weight += i;
            }
        }
        sum / weight
    };
    let lean = |canvas: &Pixmap| centroid(canvas, 14..22) - centroid(canvas, 32..40);
    assert!(
        lean(&plain).abs() < 0.01,
        "upright H leans {}",
        lean(&plain)
    );
    // The rows sit about 16 px apart, so a 0.25 shear shifts them 4 px.
    assert!(lean(&slanted) > 3.0, "oblique H leans {}", lean(&slanted));
}

#[test]
fn cache_hits_after_the_first_draw_of_each_key() {
    let letters = run("aaaa", &font(SANS), 16.0);
    let mut canvas = white_canvas(80, 24);
    // Advances are fractional, so the four a's land on a few quarters; clear
    // the guess by drawing at whole pixels.
    let single = GlyphRun {
        glyphs: (0..4)
            .map(|i| Glyph {
                id: letters.glyphs[0].id,
                x: i as f32 * 10.0,
                y: 0.0,
            })
            .collect(),
        ..letters
    };
    draw(&mut canvas, (2.0, 18.0), &single);
    assert_eq!(cache::stats(), (3, 1));
    draw(&mut canvas, (2.0, 18.0), &single);
    assert_eq!(cache::stats(), (7, 1));
    draw(&mut canvas, (2.25, 18.0), &single);
    assert_eq!(cache::stats(), (10, 2), "a new quarter is a new key");
    let mut bold = single.clone();
    bold.synthetic_bold = true;
    draw(&mut canvas, (2.0, 18.0), &bold);
    assert_eq!(cache::stats(), (13, 3), "synthetic bold is part of the key");
}

#[test]
fn a_warm_cache_draws_the_bytes_a_cold_cache_drew() {
    let text = run("Hamburgefonstiv 0123", &font(SERIF), 17.0);
    let render = || {
        let mut canvas = white_canvas(220, 30);
        draw(&mut canvas, (3.3, 22.0), &text);
        canvas.data().to_vec()
    };
    let cold = render();
    let (hits, misses) = cache::stats();
    assert!(misses > 0, "the first draw must rasterise");
    let warm = render();
    let (warm_hits, warm_misses) = cache::stats();
    assert_eq!(warm_misses, misses, "the second draw must not rasterise");
    assert_eq!(warm_hits - hits, text.glyphs.len() as u64);
    assert!(cold == warm, "a hit and a miss must draw the same bytes");
}

fn max_difference(a: &Pixmap, b: &Pixmap) -> u8 {
    a.data()
        .iter()
        .zip(b.data())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[test]
fn a_cached_mask_equals_the_direct_fill_at_the_same_offset() {
    for bold in [false, true] {
        for skew in [0.0, 0.25] {
            for x in [10.0, 10.25, 10.5, 10.75] {
                let mut text = run("Rag&g", &font(SANS), 20.0);
                // Whole-pixel advances keep every glyph on the quarter step
                // under test; the direct fill does not quantise.
                for (i, glyph) in text.glyphs.iter_mut().enumerate() {
                    glyph.x = i as f32 * 14.0;
                }
                text.synthetic_bold = bold;
                text.skew = skew;
                let mut cached = white_canvas(120, 40);
                draw(&mut cached, (x, 28.0), &text);
                let mut direct = white_canvas(120, 40);
                super::draw_direct(
                    &mut direct,
                    Transform::identity(),
                    (x, 28.0),
                    &text,
                    Color::BLACK,
                    None,
                    &mut None,
                );
                let worst = max_difference(&cached, &direct);
                assert!(
                    worst <= GOLDEN_TOLERANCE,
                    "bold {bold} skew {skew} x {x}: differs by {worst}"
                );
            }
        }
    }
}

#[test]
fn synthetic_bold_adds_exactly_the_embolden_width_without_edge_darkening() {
    let regular = run("l", &font(SANS), 48.0);
    let mut bold = regular.clone();
    bold.synthetic_bold = true;
    let middle_row = |run: &GlyphRun, x: f32| {
        let mut canvas = white_canvas(40, 60);
        draw(&mut canvas, (x, 48.0), run);
        row_ink(&canvas, 24) as f32 / 255.0
    };
    for x in [4.0, 4.25, 4.5, 4.75] {
        let grown = middle_row(&bold, x) - middle_row(&regular, x);
        let expected = cache::embolden_width(48.0);
        // Source-over of the stroke on the fill would add up to a quarter
        // pixel per edge.
        assert!(
            (grown - expected).abs() < 0.1,
            "x {x}: grew {grown}, expected {expected}"
        );
    }
}

#[test]
fn the_cache_never_keeps_font_bytes_alive() {
    let bytes = EMBEDDED_FONTS[SANS].bytes;
    let owned = FontRef {
        data: Arc::new(FontBlob::Owned(bytes.into())),
        index: 0,
        coords: Arc::from([]),
    };
    let mut canvas = white_canvas(40, 40);
    draw(&mut canvas, (4.0, 30.0), &run("Hg", &owned, 16.0));
    assert_eq!(Arc::strong_count(&owned.data), 1);
    assert_eq!(cache::sizes().2, 1);
    let weak = Arc::downgrade(&owned.data);
    drop(owned);
    assert!(weak.upgrade().is_none(), "the blob must be freed");
}

#[test]
fn fonts_that_were_dropped_leave_the_cache_when_it_rotates() {
    cache::reset(4000);
    let owned = FontRef {
        data: Arc::new(FontBlob::Owned(EMBEDDED_FONTS[SANS].bytes.into())),
        index: 0,
        coords: Arc::from([]),
    };
    let mut canvas = white_canvas(40, 40);
    draw(&mut canvas, (4.0, 30.0), &run("Hg", &owned, 16.0));
    drop(owned);
    let survivor = run("a", &font(SERIF), 16.0);
    for size in 10..80 {
        let mut text = survivor.clone();
        text.size = size as f32;
        draw(&mut canvas, (4.0, 30.0), &text);
    }
    assert_eq!(cache::sizes().2, 1, "only the live font remains");
}

#[test]
fn two_generations_keep_a_working_set_and_drop_what_goes_unused() {
    let budget = 4000;
    cache::reset(budget);
    let mut canvas = white_canvas(40, 40);
    let working = run("ab", &font(SANS), 16.0);
    let filler = |size: u32| {
        let mut text = run("a", &font(SANS), 16.0);
        text.size = 20.0 + size as f32 * 0.5;
        text
    };
    draw(&mut canvas, (4.0, 30.0), &working);
    let mut next = 0;
    while cache::sizes().1 == 0 {
        draw(&mut canvas, (4.0, 30.0), &filler(next));
        next += 1;
    }
    let (_, misses) = cache::stats();
    draw(&mut canvas, (4.0, 30.0), &working);
    assert_eq!(
        cache::stats().1,
        misses,
        "a rotation keeps the previous generation"
    );
    for _ in 0..200 {
        draw(&mut canvas, (4.0, 30.0), &filler(next));
        next += 1;
    }
    let (current, previous, _) = cache::sizes();
    assert!(current + previous < 60, "{current} + {previous} entries");
    let (_, misses) = cache::stats();
    draw(&mut canvas, (4.0, 30.0), &working);
    assert_eq!(cache::stats().1, misses + 2, "unused glyphs are evicted");
}

#[test]
fn scaling_the_transform_matches_a_larger_run() {
    let small = run("Hag", &font(SANS), 12.0);
    let large = run("Hag", &font(SANS), 24.0);
    let mut scaled = white_canvas(100, 50);
    draw_text(
        &mut scaled,
        Transform::from_scale(2.0, 2.0),
        (2.0, 20.0),
        &small,
        Color::BLACK,
        None,
    );
    let mut direct = white_canvas(100, 50);
    draw(&mut direct, (4.0, 40.0), &large);
    assert_eq!(scaled.data(), direct.data());
}

#[test]
fn rotated_text_is_drawn_with_exact_geometry() {
    let letters = run("Hi", &font(SANS), 24.0);
    let mut canvas = white_canvas(60, 60);
    let rotate = Transform::from_rotate_at(90.0, 30.0, 30.0);
    let undrawn = draw_text(
        &mut canvas,
        rotate,
        (10.0, 30.0),
        &letters,
        Color::BLACK,
        None,
    );
    assert_eq!(undrawn, 0);
    assert!(total_ink(&canvas) > 1000);
    // Rotated a quarter turn the glyphs run downward: ink spreads over rows.
    let rows = (0..60).filter(|y| row_ink(&canvas, *y) > 0).count();
    assert!(rows > 12, "{rows} rows inked");
}

#[test]
fn large_glyphs_are_filled_directly() {
    let big = run("O", &font(SANS), 300.0);
    let mut canvas = white_canvas(260, 320);
    assert_eq!(draw(&mut canvas, (10.0, 300.0), &big), 0);
    assert!(total_ink(&canvas) > 10_000);
    assert_eq!(cache::stats(), (0, 0), "oversized glyphs bypass the cache");
}

#[test]
fn a_missing_character_draws_the_font_notdef_box() {
    let font = font(SANS);
    // U+E000 is private use and absent from Liberation Sans.
    let missing = run("\u{e000}", &font, 24.0);
    assert_eq!(missing.glyphs[0].id, 0);
    let mut canvas = white_canvas(40, 40);
    assert_eq!(draw(&mut canvas, (4.0, 30.0), &missing), 0);
    let ink_rows: Vec<u32> = (0..40).filter(|y| row_ink(&canvas, *y) > 0).collect();
    assert!(ink_rows.len() >= 10, "notdef should be a visible box");
    // A box outline: the first inked row is a long horizontal bar.
    assert!(row_ink(&canvas, ink_rows[0]) > 8 * 127);
}

#[test]
fn spaces_draw_nothing_and_unknown_glyph_ids_are_undrawn() {
    let font = font(SANS);
    let mut canvas = white_canvas(40, 40);
    assert_eq!(draw(&mut canvas, (4.0, 30.0), &run("  ", &font, 16.0)), 0);
    assert_eq!(total_ink(&canvas), 0);
    let mut invalid = run("a", &font, 16.0);
    invalid.glyphs[0].id = 60_000;
    assert_eq!(draw(&mut canvas, (4.0, 30.0), &invalid), 1);
    assert_eq!(total_ink(&canvas), 0);
}

#[test]
fn translucent_text_blends_over_the_canvas() {
    let letters = run("l", &font(SANS), 32.0);
    let mut canvas = white_canvas(24, 40);
    let half = Color::from_rgba8(0, 0, 0, 128);
    draw_text(
        &mut canvas,
        Transform::identity(),
        (4.0, 32.0),
        &letters,
        half,
        None,
    );
    let core = (0..24)
        .map(|x| ink(&canvas, x, 16))
        .max()
        .expect("row has pixels");
    assert!((126..=129).contains(&core), "stem ink {core}");
}

fn decoration(style: LineStyle, rect: Rect) -> Pixmap {
    let mut canvas = white_canvas(60, 20);
    let undrawn = draw_decoration(
        &mut canvas,
        Transform::identity(),
        rect,
        Color::BLACK,
        style,
        None,
    );
    assert_eq!(undrawn, 0);
    canvas
}

#[test]
fn solid_decoration_fills_its_rows_exactly() {
    let canvas = decoration(LineStyle::Solid, Rect::new(5.0, 10.0, 30.0, 2.0));
    for y in 0..20 {
        let expected = if (10..12).contains(&y) { 30 * 255 } else { 0 };
        assert_eq!(row_ink(&canvas, y), expected, "row {y}");
    }
    assert_eq!(ink(&canvas, 4, 10), 0);
    assert_eq!(ink(&canvas, 5, 10), 255);
    assert_eq!(ink(&canvas, 34, 10), 255);
    assert_eq!(ink(&canvas, 35, 10), 0);
}

#[test]
fn fractional_decoration_snaps_to_whole_rows_with_a_minimum_of_one() {
    let thin = decoration(LineStyle::Solid, Rect::new(0.0, 10.4, 20.0, 0.2));
    assert_eq!(
        (9..13).map(|y| row_ink(&thin, y)).collect::<Vec<_>>(),
        [0, 20 * 255, 0, 0]
    );
    let moved = decoration(LineStyle::Solid, Rect::new(0.0, 10.4, 20.0, 1.2));
    assert_eq!(row_ink(&moved, 10), 20 * 255);
    assert_eq!(row_ink(&moved, 11), 20 * 255);
    assert_eq!(row_ink(&moved, 12), 0);
}

#[test]
fn double_decoration_leaves_a_gap_between_two_lines() {
    let canvas = decoration(LineStyle::Double, Rect::new(5.0, 6.0, 30.0, 3.0));
    let rows: Vec<bool> = (5..10).map(|y| row_ink(&canvas, y) > 0).collect();
    assert_eq!(rows, [false, true, false, true, false]);
}

#[test]
fn dashed_and_dotted_decorations_alternate_ink_and_gaps() {
    let dashed = decoration(LineStyle::Dashed, Rect::new(0.0, 8.0, 50.0, 2.0));
    let runs = |canvas: &Pixmap, y: u32| {
        let mut count = 0;
        let mut previous = false;
        for x in 0..canvas.width() {
            let inked = ink(canvas, x, y) > 127;
            count += usize::from(inked && !previous);
            previous = inked;
        }
        count
    };
    assert!(runs(&dashed, 8) >= 4, "{} dashes", runs(&dashed, 8));
    assert_eq!(ink(&dashed, 0, 8), 255, "starts on a dash");
    assert_eq!(ink(&dashed, 49, 8), 255, "ends on a dash");
    assert_eq!(row_ink(&dashed, 7), 0);
    assert_eq!(row_ink(&dashed, 10), 0);
    let dotted = decoration(LineStyle::Dotted, Rect::new(0.0, 8.0, 50.0, 3.0));
    assert!(runs(&dotted, 9) >= 6, "{} dots", runs(&dotted, 9));
    assert_eq!(row_ink(&dotted, 7), 0);
    assert_eq!(row_ink(&dotted, 11), 0);
}

#[test]
fn wavy_decoration_stays_inside_its_band_and_leaves_both_edges() {
    let rect = Rect::new(2.0, 6.0, 48.0, 6.0);
    let canvas = decoration(LineStyle::Wavy, rect);
    for y in 0..20 {
        let inside = (6..12).contains(&y);
        assert_eq!(row_ink(&canvas, y) > 0, inside, "row {y}");
    }
    let tops = (2..50).filter(|x| ink(&canvas, *x, 6) > 64).count();
    let bottoms = (2..50).filter(|x| ink(&canvas, *x, 11) > 64).count();
    assert!(tops > 0 && bottoms > 0, "wave must reach both edges");
}

#[test]
fn a_decoration_is_cut_by_the_clip_mask() {
    let mut mask = Mask::new(60, 20).expect("mask allocates");
    for y in 0..20usize {
        for x in 20..60usize {
            mask.data_mut()[y * 60 + x] = 255;
        }
    }
    let mut canvas = white_canvas(60, 20);
    draw_decoration(
        &mut canvas,
        Transform::identity(),
        Rect::new(0.0, 5.0, 60.0, 2.0),
        Color::BLACK,
        LineStyle::Solid,
        Some(&mask),
    );
    assert!((0..20).all(|x| column_ink(&canvas, x) == 0));
    assert!((20..60).all(|x| column_ink(&canvas, x) == 2 * 255));
}

#[test]
fn transparent_text_and_lines_draw_nothing() {
    let mut canvas = white_canvas(40, 40);
    let clear = Color::TRANSPARENT;
    let letters = run("Hg", &font(SANS), 16.0);
    draw_text(
        &mut canvas,
        Transform::identity(),
        (4.0, 20.0),
        &letters,
        clear,
        None,
    );
    draw_decoration(
        &mut canvas,
        Transform::identity(),
        Rect::new(0.0, 30.0, 40.0, 2.0),
        clear,
        LineStyle::Solid,
        None,
    );
    assert_eq!(total_ink(&canvas), 0);
    assert_eq!(alpha_at(&canvas, 0, 0), 255);
}

/// Times a 2,000-glyph paragraph three ways: warm cache, cold cache (every
/// mask rasterised) and uncached direct fills. Run with
/// `cargo nextest run --release -p tenta-paint --run-ignored only glyph_throughput --no-capture`.
#[test]
#[ignore = "timing report, meaningful only in release"]
fn glyph_throughput() {
    let line = "Hamburgefonstiv the quick brown fox jumps over a lazy dog 0123456789 ";
    let mut paragraph = Vec::new();
    for row in 0..24 {
        let mut laid = run(&line.repeat(2)[..84], &font(SANS), 14.0);
        for glyph in &mut laid.glyphs {
            glyph.y = row as f32 * 18.0;
        }
        paragraph.push(laid);
    }
    let glyph_count: usize = paragraph.iter().map(|r| r.glyphs.len()).sum();
    assert!(glyph_count >= 2000, "{glyph_count} glyphs");
    let measure = |label: &str, setup: &dyn Fn(), paint_run: &dyn Fn(&mut Pixmap, &GlyphRun)| {
        let mut micros = Vec::new();
        for _ in 0..30 {
            setup();
            let mut canvas = white_canvas(900, 460);
            let start = Instant::now();
            for laid in &paragraph {
                paint_run(&mut canvas, laid);
            }
            micros.push(start.elapsed().as_micros());
        }
        micros.sort_unstable();
        eprintln!(
            "{label}: {glyph_count} glyphs, p50 {} us, p95 {} us over {} runs",
            micros[micros.len() / 2],
            micros[micros.len() * 95 / 100],
            micros.len()
        );
    };
    let cached = |canvas: &mut Pixmap, laid: &GlyphRun| {
        draw(canvas, (4.0, 16.0), laid);
    };
    measure("warm cache", &|| {}, &cached);
    measure("cold cache", &|| cache::reset(1 << 23), &cached);
    measure("direct fills, no cache", &|| {}, &|canvas, laid| {
        super::draw_direct(
            canvas,
            Transform::identity(),
            (4.0, 16.0),
            laid,
            Color::BLACK,
            None,
            &mut None,
        );
    });
}
