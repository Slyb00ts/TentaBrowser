use std::path::PathBuf;
use std::time::{Duration, Instant};

use tenta_layout::{BorderStyle, Edges};

use super::*;
use crate::dl::Radii;
use crate::encode::{PngProfile, encode_png};
use crate::image::ImageStore;

/// Golden comparisons allow this much difference per channel until the
/// cross-platform determinism gate proves byte-identical output.
const GOLDEN_TOLERANCE: u8 = 1;

const NO_RADII: Radii = [0.0; 8];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const CLEAR: [u8; 4] = [0; 4];

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

fn options(width: u32, height: u32) -> RasterOptions {
    RasterOptions {
        width,
        height,
        scale: 1.0,
        origin: (0.0, 0.0),
        background: None,
    }
}

fn run(items: Vec<Item>, opts: &RasterOptions) -> RasterOutput {
    let list = DisplayList {
        items,
        bounds: Rect::default(),
        ..DisplayList::default()
    };
    rasterize(&list, opts, &ImageStore::new()).expect("the output size is valid")
}

fn raster_with(items: Vec<Item>, opts: &RasterOptions) -> Pixmap {
    let out = run(items, opts);
    assert!(out.stats.is_complete(), "skipped {:?}", out.stats.skipped);
    out.pixmap
}

fn raster(items: Vec<Item>, width: u32, height: u32) -> Pixmap {
    raster_with(items, &options(width, height))
}

/// The premultiplied RGBA of one device pixel.
fn px(pix: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * pix.width() + x) * 4) as usize;
    pix.data()[offset..offset + 4]
        .try_into()
        .expect("four channels")
}

/// The pixels of `pix` as premultiplied RGBA quadruples.
fn pixels(pix: &Pixmap) -> &[[u8; 4]] {
    pix.data().as_chunks::<4>().0
}

fn alphas(pix: &Pixmap, y: u32, xs: std::ops::Range<u32>) -> Vec<u8> {
    xs.map(|x| px(pix, x, y)[3]).collect()
}

fn border(rect: Rect, width: f32, color: Color, style: BorderStyle) -> Item {
    Item::Border {
        rect,
        widths: Edges::all(width),
        colors: [color; 4],
        styles: [style; 4],
        radii: NO_RADII,
    }
}

fn fill(rect: Rect, color: Color) -> Item {
    Item::FillRect { rect, color }
}

fn clip(rect: Rect, radii: Radii) -> Item {
    Item::PushClip { rect, radii }
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

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/render-goldens")
        .join(format!("{name}.png"))
}

/// Compares `pix` with the blessed golden `name`. Both sides go through PNG
/// encoding, so the comparison is on the straight RGBA that a reviewer sees.
/// `UPDATE_GOLDEN=1` rewrites the golden instead of comparing.
fn assert_golden(name: &str, pix: &Pixmap) {
    let encoded = encode_png(pix, PngProfile::Fast);
    let path = golden_path(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("golden has a directory"))
            .expect("golden directory is writable");
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
fn pixmap_rejects_zero_dimensions_and_reports_its_size() {
    assert!(Pixmap::new(0, 4).is_none());
    let pix = Pixmap::new(3, 2).unwrap();
    assert_eq!((pix.width(), pix.height()), (3, 2));
    assert_eq!(pix.data().len(), 3 * 2 * 4);
    assert!(pix.data().iter().all(|&b| b == 0));
}

#[test]
fn a_zero_sized_target_is_reported_not_panicked() {
    let list = DisplayList::default();
    let error = rasterize(&list, &options(0, 4), &ImageStore::new()).unwrap_err();
    assert_eq!(
        error,
        PaintError::InvalidSize {
            width: 0,
            height: 4
        }
    );
}

#[test]
fn fill_rect_covers_exactly_its_device_pixels() {
    let pix = raster(
        vec![fill(Rect::new(2.0, 2.0, 4.0, 3.0), rgb(255, 0, 0))],
        10,
        10,
    );
    assert_eq!(px(&pix, 2, 2), RED);
    assert_eq!(px(&pix, 5, 4), RED);
    assert_eq!(px(&pix, 6, 4), CLEAR);
    assert_eq!(px(&pix, 2, 5), CLEAR);
}

#[test]
fn fractional_edges_round_to_whole_pixels_without_a_fringe() {
    // 10.3 rounds down to 10 and 14.3 to 14: columns 10..14.
    let down = raster(
        vec![fill(Rect::new(10.3, 0.0, 4.0, 1.0), rgb(0, 0, 0))],
        20,
        1,
    );
    assert_eq!(
        alphas(&down, 0, 8..16),
        [0, 0, 255, 255, 255, 255, 0, 0],
        "10.3 + 4"
    );
    // 10.6 rounds up to 11 and 14.6 to 15: columns 11..15.
    let up = raster(
        vec![fill(Rect::new(10.6, 0.0, 4.0, 1.0), rgb(0, 0, 0))],
        20,
        1,
    );
    assert_eq!(
        alphas(&up, 0, 8..16),
        [0, 0, 0, 255, 255, 255, 255, 0],
        "10.6 + 4"
    );
    // The size is the difference of the rounded edges: 10.3 -> 10, 14.6 -> 15.
    let wider = raster(
        vec![fill(Rect::new(10.3, 0.0, 4.3, 1.0), rgb(0, 0, 0))],
        20,
        1,
    );
    assert_eq!(
        alphas(&wider, 0, 9..16),
        [0, 255, 255, 255, 255, 255, 0],
        "10.3 + 4.3"
    );
}

#[test]
fn a_translucent_fractional_rect_is_uniform_up_to_its_edge() {
    let mut opts = options(10, 4);
    opts.background = Some(Color::WHITE);
    let pix = raster_with(
        vec![fill(
            Rect::new(2.4, 1.2, 4.0, 2.0),
            Color::from_rgba8(0, 0, 0, 128),
        )],
        &opts,
    );
    let inside = px(&pix, 3, 1);
    assert_ne!(inside, [255; 4]);
    for (x, y) in [(2, 1), (5, 2), (2, 2), (5, 1)] {
        assert_eq!(px(&pix, x, y), inside, "pixel ({x}, {y})");
    }
    assert_eq!(px(&pix, 6, 1), [255; 4]);
    assert_eq!(px(&pix, 1, 1), [255; 4]);
    assert_eq!(px(&pix, 3, 0), [255; 4]);
    assert_eq!(px(&pix, 3, 3), [255; 4]);
}

#[test]
fn rounded_corners_have_exact_anti_aliased_edge_pixels() {
    let item = Item::FillRRect {
        rect: Rect::new(0.0, 0.0, 10.0, 10.0),
        radii: [5.0; 8],
        color: rgb(0, 0, 255),
    };
    let pix = raster(vec![item], 10, 10);
    // The corner pixel lies wholly outside the arc, the centre wholly inside.
    assert_eq!(px(&pix, 0, 0), CLEAR);
    assert_eq!(px(&pix, 5, 5), BLUE);
    // tiny-skia samples coverage on a 4x4 grid, so edge pixels come in
    // sixteenths of 255. The radius-5 circle centred (5, 5) covers 0.547 of
    // pixel (1, 1), 0.317 of (2, 0) and 0.760 of (3, 0): 10, 5 and 12 samples.
    assert_eq!(px(&pix, 1, 1), [0, 0, 159, 159]);
    assert_eq!(px(&pix, 2, 0), [0, 0, 80, 80]);
    assert_eq!(px(&pix, 3, 0), [0, 0, 191, 191]);
    assert_eq!(px(&pix, 1, 0), CLEAR);
    // The sample grid is not mirror-symmetric, so the other corners differ
    // by a few samples around the same analytic 0.547.
    assert_eq!(px(&pix, 8, 1)[3], 128);
    assert_eq!(px(&pix, 1, 8)[3], 144);
    assert_eq!(px(&pix, 8, 8)[3], 128);
}

#[test]
fn clip_hides_what_is_outside_it() {
    let items = vec![
        clip(Rect::new(0.0, 0.0, 5.0, 5.0), NO_RADII),
        fill(Rect::new(0.0, 0.0, 10.0, 10.0), rgb(0, 255, 0)),
        Item::PopClip,
        fill(Rect::new(0.0, 0.0, 10.0, 10.0), rgb(255, 0, 0)),
    ];
    let pix = raster(items, 10, 10);
    assert_eq!(px(&pix, 2, 2), RED);
    assert_eq!(px(&pix, 7, 7), RED);
}

#[test]
fn a_fractional_rect_clip_snaps_to_whole_pixels() {
    let items = vec![
        clip(Rect::new(2.4, 2.4, 3.0, 3.0), NO_RADII),
        fill(Rect::new(0.0, 0.0, 10.0, 10.0), rgb(255, 0, 0)),
        Item::PopClip,
    ];
    let pix = raster(items, 10, 10);
    // 2.4 -> 2 and 5.4 -> 5: pixels 2, 3 and 4 on each axis, nothing partial.
    assert_eq!(alphas(&pix, 3, 0..8), [0, 0, 255, 255, 255, 0, 0, 0]);
    assert_eq!(px(&pix, 3, 1)[3], 0);
    assert_eq!(px(&pix, 3, 5)[3], 0);
}

#[test]
fn nested_clips_intersect() {
    let items = vec![
        clip(Rect::new(0.0, 0.0, 6.0, 10.0), NO_RADII),
        clip(Rect::new(4.0, 0.0, 6.0, 10.0), NO_RADII),
        fill(Rect::new(0.0, 0.0, 10.0, 10.0), rgb(0, 0, 255)),
        Item::PopClip,
        fill(Rect::new(0.0, 0.0, 10.0, 1.0), rgb(255, 0, 0)),
        Item::PopClip,
    ];
    let pix = raster(items, 10, 10);
    assert_eq!(px(&pix, 5, 5), BLUE);
    assert_eq!(px(&pix, 2, 5), CLEAR);
    assert_eq!(px(&pix, 8, 5), CLEAR);
    assert_eq!(px(&pix, 2, 0), RED);
}

#[test]
fn a_rounded_clip_matches_the_rounded_fill_of_the_same_shape() {
    let rounded = [5.0; 8];
    let clipped = raster(
        vec![
            clip(Rect::new(0.0, 0.0, 10.0, 10.0), rounded),
            fill(Rect::new(0.0, 0.0, 10.0, 10.0), rgb(0, 0, 255)),
            Item::PopClip,
        ],
        10,
        10,
    );
    assert_eq!(px(&clipped, 0, 0), CLEAR);
    assert_eq!(px(&clipped, 5, 5), BLUE);
    // The mask carries the same sixteenths of coverage as the direct fill.
    assert_eq!(px(&clipped, 1, 1), [0, 0, 159, 159]);
    assert_eq!(px(&clipped, 2, 0), [0, 0, 80, 80]);
    assert_eq!(px(&clipped, 8, 8)[3], 128);
}

#[test]
fn a_clip_under_a_scale_transform_snaps_in_device_pixels() {
    let items = vec![
        Item::PushTransform([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]),
        clip(Rect::new(1.2, 0.0, 2.0, 1.0), NO_RADII),
        fill(Rect::new(0.0, 0.0, 10.0, 10.0), rgb(255, 0, 0)),
        Item::PopClip,
        Item::PopTransform,
    ];
    let pix = raster(items, 12, 4);
    // Device x: 1.2 * 2 = 2.4 -> 2 and 3.2 * 2 = 6.4 -> 6.
    assert_eq!(alphas(&pix, 0, 0..8), [0, 0, 255, 255, 255, 255, 0, 0]);
    assert_eq!(px(&pix, 3, 2)[3], 0);
}

#[test]
fn a_rotated_clip_is_a_mask_with_anti_aliased_edges() {
    let (s, c) = (
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    );
    let items = vec![
        Item::PushTransform([c, s, -s, c, 10.0, 0.0]),
        clip(Rect::new(0.0, 0.0, 8.0, 8.0), NO_RADII),
        Item::PopTransform,
        // Drawn outside the transform, under the rotated clip.
        fill(Rect::new(0.0, 0.0, 24.0, 16.0), rgb(255, 0, 0)),
        Item::PopClip,
    ];
    let pix = raster(items, 24, 16);
    // The rotated square is a diamond centred at (10, 5.66) with half-diagonal 5.66.
    assert_eq!(px(&pix, 10, 6), RED);
    assert_eq!(px(&pix, 0, 0), CLEAR);
    assert_eq!(px(&pix, 22, 14), CLEAR);
    let edge = (0..24)
        .map(|x| px(&pix, x, 1)[3])
        .find(|&a| a > 0 && a < 255);
    assert!(edge.is_some(), "the diamond's slanted edge is anti-aliased");
}

#[test]
fn opacity_group_composites_at_half_strength() {
    let items = vec![
        Item::PushOpacity(0.5),
        fill(Rect::new(0.0, 0.0, 4.0, 4.0), rgb(255, 0, 0)),
        Item::PopOpacity,
    ];
    let pix = raster(items, 4, 4);
    let [r, g, b, a] = px(&pix, 1, 1);
    assert!(a.abs_diff(128) <= 1, "alpha {a}");
    assert!(r.abs_diff(128) <= 1, "red {r}");
    assert_eq!((g, b), (0, 0));
}

#[test]
fn opacity_one_is_a_no_op_and_zero_draws_nothing() {
    let content = || {
        vec![
            fill(Rect::new(1.0, 1.0, 5.0, 5.0), rgb(255, 0, 0)),
            fill(Rect::new(3.0, 3.0, 5.0, 5.0), rgb(0, 0, 255)),
        ]
    };
    let plain = raster(content(), 10, 10);
    let mut wrapped = vec![Item::PushOpacity(1.0)];
    wrapped.extend(content());
    wrapped.push(Item::PopOpacity);
    assert_eq!(raster(wrapped, 10, 10).data(), plain.data());

    let mut hidden = vec![Item::PushOpacity(0.0)];
    hidden.extend(content());
    hidden.push(Item::PopOpacity);
    assert!(raster(hidden, 10, 10).data().iter().all(|&b| b == 0));
}

#[test]
fn an_opacity_group_far_from_the_origin_composites_at_its_own_offset() {
    let items = vec![
        fill(Rect::new(40.0, 40.0, 8.0, 8.0), rgb(255, 255, 255)),
        Item::PushOpacity(0.5),
        fill(Rect::new(40.0, 40.0, 4.0, 4.0), rgb(0, 0, 0)),
        Item::PopOpacity,
    ];
    let pix = raster(items, 64, 64);
    let [r, _, _, a] = px(&pix, 41, 41);
    assert_eq!(a, 255);
    assert!(r.abs_diff(128) <= 1, "half black over white: {r}");
    assert_eq!(px(&pix, 45, 45), [255; 4]);
    assert_eq!(px(&pix, 10, 10), CLEAR);
}

#[test]
fn a_transform_inside_an_opacity_group_lands_in_the_right_place() {
    let items = vec![
        Item::PushOpacity(0.5),
        Item::PushTransform([1.0, 0.0, 0.0, 1.0, 30.0, 20.0]),
        fill(Rect::new(0.0, 0.0, 4.0, 4.0), rgb(255, 0, 0)),
        Item::PopTransform,
        Item::PopOpacity,
    ];
    let pix = raster(items, 64, 64);
    assert!(px(&pix, 31, 21)[3].abs_diff(128) <= 1);
    assert_eq!(px(&pix, 3, 3), CLEAR);
    assert_eq!(px(&pix, 34, 24), CLEAR);
}

#[test]
fn opacity_inside_a_clip_is_cut_by_the_clip() {
    let items = vec![
        clip(Rect::new(0.0, 0.0, 5.0, 5.0), NO_RADII),
        Item::PushOpacity(0.5),
        fill(Rect::new(0.0, 0.0, 10.0, 10.0), rgb(255, 0, 0)),
        Item::PopOpacity,
        Item::PopClip,
    ];
    let pix = raster(items, 10, 10);
    assert!(px(&pix, 2, 2)[3].abs_diff(128) <= 1);
    assert_eq!(px(&pix, 7, 7), CLEAR);
    assert_eq!(px(&pix, 2, 7), CLEAR);
}

#[test]
fn a_rounded_clip_pushed_before_an_opacity_group_shapes_the_group_once() {
    let rounded = [8.0; 8];
    let content = |opacity: Option<f32>| {
        let mut items = vec![clip(Rect::new(4.0, 4.0, 24.0, 24.0), rounded)];
        items.extend(opacity.map(Item::PushOpacity));
        items.push(fill(Rect::new(0.0, 0.0, 40.0, 40.0), rgb(255, 0, 0)));
        items.extend(opacity.map(|_| Item::PopOpacity));
        items.push(Item::PopClip);
        items
    };
    let opaque = raster(content(None), 40, 40);
    let faded = raster(content(Some(0.5)), 40, 40);
    for (index, (full, half)) in pixels(&opaque).iter().zip(pixels(&faded)).enumerate() {
        // Half of the clip's own coverage: the edge is shaped once, not twice.
        let expected = u16::from(full[3]).div_ceil(2);
        assert!(
            u16::from(half[3]).abs_diff(expected) <= 1,
            "pixel {index}: {half:?} vs {full:?}"
        );
    }
    assert_eq!(px(&faded, 4, 4), CLEAR);
}

#[test]
fn a_nested_opacity_group_multiplies() {
    let items = vec![
        Item::PushOpacity(0.5),
        Item::PushOpacity(0.5),
        fill(Rect::new(0.0, 0.0, 4.0, 4.0), rgb(255, 0, 0)),
        Item::PopOpacity,
        Item::PopOpacity,
    ];
    let pix = raster(items, 4, 4);
    assert!(px(&pix, 1, 1)[3].abs_diff(64) <= 1);
}

#[test]
fn group_extent_covers_the_items_between_push_and_pop() {
    let items = vec![
        Item::PushOpacity(0.5),
        fill(Rect::new(10.0, 20.0, 5.0, 5.0), rgb(0, 0, 0)),
        Item::PushTransform([1.0, 0.0, 0.0, 1.0, 100.0, 0.0]),
        fill(Rect::new(0.0, 0.0, 4.0, 4.0), rgb(0, 0, 0)),
        Item::PopTransform,
        Item::PopOpacity,
        fill(Rect::new(500.0, 500.0, 1.0, 1.0), rgb(0, 0, 0)),
    ];
    let (end, footprint) = group_extent(&items, 0, Transform::identity());
    assert_eq!(end, 5);
    let Footprint::Area(area) = footprint else {
        panic!("the group's content has known bounds");
    };
    // The first fill spans y 19..26 and the translated one reaches y -1..5.
    assert_eq!(area, IRect::new(9, -1, 105, 26));
}

#[test]
fn transform_offsets_the_items_inside_it() {
    let items = vec![
        Item::PushTransform([1.0, 0.0, 0.0, 1.0, 5.0, 0.0]),
        fill(Rect::new(0.0, 0.0, 2.0, 2.0), rgb(255, 0, 0)),
        Item::PopTransform,
    ];
    let pix = raster(items, 10, 4);
    assert_eq!(px(&pix, 5, 0), RED);
    assert_eq!(px(&pix, 0, 0), CLEAR);
}

#[test]
fn a_rotated_rect_keeps_exact_anti_aliased_geometry() {
    let (s, c) = (
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    );
    let items = vec![
        Item::PushTransform([c, s, -s, c, 10.0, 2.0]),
        fill(Rect::new(0.0, 0.0, 6.0, 6.0), rgb(255, 0, 0)),
        Item::PopTransform,
    ];
    let pix = raster(items, 20, 14);
    assert_eq!(px(&pix, 10, 6), RED);
    let partial = pixels(&pix)
        .iter()
        .filter(|p| p[3] > 0 && p[3] < 255)
        .count();
    assert!(partial > 8, "{partial} anti-aliased edge pixels");
}

#[test]
fn scale_and_origin_map_css_pixels_to_device_pixels() {
    let opts = RasterOptions {
        width: 10,
        height: 10,
        scale: 2.0,
        origin: (1.0, 1.0),
        background: None,
    };
    let pix = raster_with(
        vec![fill(Rect::new(1.0, 1.0, 2.0, 2.0), rgb(255, 0, 0))],
        &opts,
    );
    assert_eq!(px(&pix, 0, 0), RED);
    assert_eq!(px(&pix, 3, 3), RED);
    assert_eq!(px(&pix, 4, 4), CLEAR);
}

#[test]
fn background_fills_the_canvas_and_none_stays_transparent() {
    let mut opts = options(3, 3);
    opts.background = Some(rgb(0, 0, 255));
    let filled = raster_with(vec![], &opts);
    assert_eq!(px(&filled, 1, 1), BLUE);
    let transparent = raster_with(vec![], &options(3, 3));
    assert_eq!(px(&transparent, 1, 1), CLEAR);
}

fn image() -> Item {
    Item::Image {
        dest: Rect::new(0.0, 0.0, 4.0, 4.0),
        src: crate::image::ImageId(0),
        src_rect: None,
        quality: crate::image::ImageQuality::default(),
    }
}

#[test]
fn unsupported_items_are_skipped_and_reported_by_kind() {
    let items = vec![
        Item::BoxShadow {
            rect: Rect::new(0.0, 0.0, 4.0, 4.0),
            radii: NO_RADII,
            offset: (0.0, 0.0),
            blur: 0.0,
            spread: 0.0,
            color: rgb(0, 0, 0),
            inset: false,
        },
        Item::LinearGradient {
            rect: Rect::new(0.0, 0.0, 4.0, 4.0),
            radii: NO_RADII,
            angle: 0.0,
            stops: vec![(0.0, rgb(0, 0, 0)), (1.0, rgb(255, 255, 255))],
            repeat: false,
        },
        image(),
        fill(Rect::new(0.0, 0.0, 2.0, 2.0), rgb(255, 0, 0)),
        image(),
    ];
    let out = run(items, &options(4, 4));
    assert_eq!(px(&out.pixmap, 1, 1), RED, "supported items still draw");
    assert!(!out.stats.is_complete());
    // The gradient is drawn; an image the store does not hold is skipped.
    assert_eq!(out.stats.skipped_total(), 3);
    assert_eq!(
        out.stats.skipped.iter().collect::<Vec<_>>(),
        [(&"BoxShadow", &1), (&"Image", &2)]
    );
}

#[test]
fn approximated_features_are_reported_apart_from_skipped_ones() {
    let list = DisplayList {
        items: vec![fill(Rect::new(0.0, 0.0, 2.0, 2.0), rgb(255, 0, 0))],
        approximated: [("TextDecorationSkipInk", 3), ("Nothing", 0)].into(),
        ..DisplayList::default()
    };
    let out = rasterize(&list, &options(4, 4), &ImageStore::new()).expect("valid size");
    assert!(out.stats.is_complete(), "approximated pixels are drawn");
    assert!(!out.stats.is_exact());
    assert_eq!(out.stats.approximated_total(), 3);
    assert_eq!(out.stats.skipped_total(), 0);
    assert_eq!(
        out.stats.approximated.len(),
        1,
        "zero counts are not recorded"
    );
    assert!(RasterStats::default().is_exact());
}

#[test]
fn degenerate_geometry_draws_nothing_and_does_not_panic() {
    let items = vec![
        fill(Rect::new(f32::NAN, 0.0, 4.0, 4.0), rgb(255, 0, 0)),
        fill(Rect::new(2.0, 2.0, 0.0, 4.0), rgb(255, 0, 0)),
        fill(Rect::new(2.0, 2.0, -3.0, 4.0), rgb(255, 0, 0)),
        Item::FillRRect {
            rect: Rect::new(1.0e9, 1.0e9, 5.0, 5.0),
            radii: [2.0; 8],
            color: rgb(255, 0, 0),
        },
        border(
            Rect::new(0.0, 0.0, 0.0, 5.0),
            1.0,
            rgb(0, 0, 0),
            BorderStyle::Solid,
        ),
        Item::PopClip,
        Item::PopOpacity,
        Item::PopTransform,
    ];
    let out = run(items, &options(8, 8));
    assert!(out.stats.is_complete());
    assert!(out.pixmap.data().iter().all(|&b| b == 0));
}

#[test]
fn solid_border_paints_only_its_band() {
    let pix = raster(
        vec![border(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            2.0,
            rgb(0, 0, 0),
            BorderStyle::Solid,
        )],
        10,
        10,
    );
    assert_eq!(px(&pix, 0, 5), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 1, 5), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 2, 5), CLEAR);
    assert_eq!(px(&pix, 7, 5), CLEAR);
    assert_eq!(px(&pix, 9, 9), [0, 0, 0, 255]);
}

#[test]
fn border_widths_floor_to_device_pixels_with_a_one_pixel_minimum() {
    let at = |width: f32| {
        raster(
            vec![border(
                Rect::new(0.0, 0.0, 12.0, 12.0),
                width,
                rgb(0, 0, 0),
                BorderStyle::Solid,
            )],
            12,
            12,
        )
    };
    // 1.9 floors to 1, and 0.4 is raised to the one-pixel minimum.
    for width in [1.9, 0.4] {
        let pix = at(width);
        assert_eq!(alphas(&pix, 6, 0..4), [255, 0, 0, 0], "width {width}");
    }
    assert_eq!(alphas(&at(2.9), 6, 0..4), [255, 255, 0, 0]);
    // A 1.5 px border on a 2x device is 3 device pixels.
    let mut opts = options(24, 24);
    opts.scale = 2.0;
    let pix = raster_with(
        vec![border(
            Rect::new(0.0, 0.0, 12.0, 12.0),
            1.5,
            rgb(0, 0, 0),
            BorderStyle::Solid,
        )],
        &opts,
    );
    assert_eq!(alphas(&pix, 12, 0..5), [255, 255, 255, 0, 0]);
}

#[test]
fn fractional_boxes_get_crisp_borders() {
    for x in [10.3, 10.6] {
        let pix = raster(
            vec![border(
                Rect::new(x, 4.3, 20.0, 12.0),
                2.0,
                rgb(0, 0, 0),
                BorderStyle::Solid,
            )],
            40,
            24,
        );
        assert!(
            pixels(&pix).iter().all(|p| p[3] == 0 || p[3] == 255),
            "x = {x} left a soft pixel"
        );
    }
    // 10.3 -> 10, 30.3 -> 30: the left border is columns 10 and 11.
    let low = raster(
        vec![border(
            Rect::new(10.3, 4.3, 20.0, 12.0),
            2.0,
            rgb(0, 0, 0),
            BorderStyle::Solid,
        )],
        40,
        24,
    );
    assert_eq!(alphas(&low, 10, 8..14), [0, 0, 255, 255, 0, 0]);
    // 10.6 -> 11, 30.6 -> 31.
    let high = raster(
        vec![border(
            Rect::new(10.6, 4.3, 20.0, 12.0),
            2.0,
            rgb(0, 0, 0),
            BorderStyle::Solid,
        )],
        40,
        24,
    );
    assert_eq!(alphas(&high, 10, 8..14), [0, 0, 0, 255, 255, 0]);
}

#[test]
fn per_side_colours_meet_on_the_diagonal_without_a_seam() {
    let item = Item::Border {
        rect: Rect::new(0.0, 0.0, 10.0, 10.0),
        widths: Edges::all(2.0),
        colors: [
            rgb(255, 0, 0),
            rgb(0, 0, 255),
            rgb(0, 0, 255),
            rgb(0, 0, 255),
        ],
        styles: [BorderStyle::Solid; 4],
        radii: NO_RADII,
    };
    let pix = raster(vec![item], 10, 10);
    assert_eq!(px(&pix, 5, 0), RED);
    assert_eq!(px(&pix, 5, 1), RED);
    assert_eq!(px(&pix, 5, 9), BLUE);
    assert_eq!(px(&pix, 0, 5), BLUE);
    assert_eq!(px(&pix, 5, 4), CLEAR);
    // Top-right corner: the diagonal runs from (10, 0) to (8, 2).
    assert_eq!(px(&pix, 8, 0), RED);
    assert_eq!(px(&pix, 9, 1), BLUE);
    for (x, y) in [(9, 0), (8, 1)] {
        let [r, _, b, a] = px(&pix, x, y);
        // The red side was drawn across the corner first, so the blue side's
        // anti-aliased edge mixes with red instead of with the background.
        assert_eq!(a, 255, "pixel ({x}, {y}) is opaque");
        assert!(
            r > 40 && b > 40 && r.abs_diff(255 - b) <= 1,
            "({x}, {y}): {r} {b}"
        );
    }
}

#[test]
fn a_dashed_side_does_not_overdraw_the_mitre_of_its_solid_neighbour() {
    let item = Item::Border {
        rect: Rect::new(0.0, 0.0, 40.0, 20.0),
        widths: Edges::all(4.0),
        colors: [rgb(255, 0, 0), rgb(0, 0, 0), rgb(0, 0, 0), rgb(0, 0, 255)],
        styles: [
            BorderStyle::Dashed,
            BorderStyle::None,
            BorderStyle::None,
            BorderStyle::Solid,
        ],
        radii: NO_RADII,
    };
    let pix = raster(vec![item], 40, 20);
    // Top-left corner square is 4x4 with its diagonal from (0, 0) to (4, 4):
    // red above it (the dash), blue below it (the solid left side).
    assert_eq!(px(&pix, 3, 0), RED);
    assert_eq!(px(&pix, 2, 0), RED);
    assert_eq!(px(&pix, 0, 3), BLUE);
    assert_eq!(px(&pix, 0, 2), BLUE);
    assert_eq!(px(&pix, 0, 10), BLUE);
}

#[test]
fn rounded_inner_corners_leave_no_gap_in_the_frame() {
    // The inner arc of a 4px border with an 8px radius is centred at (8, 8) with
    // radius 4, so the pixel at (4, 4) lies wholly in the inner corner square.
    let corner = Item::Border {
        rect: Rect::new(0.0, 0.0, 20.0, 20.0),
        widths: Edges::all(4.0),
        colors: [
            rgb(255, 0, 0),
            rgb(0, 0, 255),
            rgb(0, 0, 255),
            rgb(0, 0, 255),
        ],
        styles: [BorderStyle::Solid; 4],
        radii: [8.0; 8],
    };
    let uniform = Item::Border {
        rect: Rect::new(0.0, 0.0, 20.0, 20.0),
        widths: Edges::all(4.0),
        colors: [rgb(0, 0, 255); 4],
        styles: [BorderStyle::Solid; 4],
        radii: [8.0; 8],
    };
    for item in [corner, uniform] {
        let pix = raster(vec![item], 20, 20);
        assert_eq!(px(&pix, 4, 4)[3], 255);
        assert_eq!(px(&pix, 10, 10), CLEAR);
        assert_eq!(px(&pix, 0, 0), CLEAR, "outside the outer arc");
    }
}

#[test]
fn a_multi_colour_rounded_border_has_each_colour_on_its_side() {
    let item = Item::Border {
        rect: Rect::new(0.0, 0.0, 40.0, 40.0),
        widths: Edges::all(4.0),
        colors: [
            rgb(255, 0, 0),
            rgb(0, 255, 0),
            rgb(0, 0, 255),
            rgb(255, 255, 0),
        ],
        styles: [BorderStyle::Solid; 4],
        radii: [12.0; 8],
    };
    let pix = raster(vec![item], 40, 40);
    assert_eq!(px(&pix, 20, 1), RED);
    assert_eq!(px(&pix, 38, 20), [0, 255, 0, 255]);
    assert_eq!(px(&pix, 20, 38), BLUE);
    assert_eq!(px(&pix, 1, 20), [255, 255, 0, 255]);
    assert_eq!(px(&pix, 20, 20), CLEAR);
}

#[test]
fn double_border_splits_the_width_into_rounded_thirds() {
    let row = |width: f32, size: u32| {
        let pix = raster(
            vec![border(
                Rect::new(0.0, 0.0, size as f32, size as f32),
                width,
                rgb(0, 0, 0),
                BorderStyle::Double,
            )],
            size,
            size,
        );
        alphas(&pix, size / 2, 0..width as u32)
    };
    // (w + 1) / 3: 3 -> 1, 5 -> 2, 7 -> 2, 8 -> 3.
    assert_eq!(row(3.0, 20), [255, 0, 255]);
    assert_eq!(row(5.0, 20), [255, 255, 0, 255, 255]);
    assert_eq!(row(7.0, 24), [255, 255, 0, 0, 0, 255, 255]);
    assert_eq!(row(8.0, 24), [255, 255, 255, 0, 0, 255, 255, 255]);
    // Below three pixels a double border is solid.
    assert_eq!(row(2.0, 20), [255, 255]);
}

#[test]
fn dashes_are_fitted_so_both_ends_of_a_side_hold_a_dash() {
    let pix = raster(
        vec![Item::Border {
            rect: Rect::new(0.0, 0.0, 60.0, 10.0),
            widths: Edges {
                top: 1.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            },
            colors: [rgb(0, 0, 0); 4],
            styles: [BorderStyle::Dashed; 4],
            radii: NO_RADII,
        }],
        60,
        10,
    );
    // 1 px dashes are 3 long with a gap of 2, fitted to 12 dashes and a gap of
    // 24 / 11 over 60 px: the first dash is pixels 0..3, the second starts at
    // 5.18 (12 of 16 coverage samples in pixel 5) and the last is 57..60.
    assert_eq!(alphas(&pix, 0, 0..6), [255, 255, 255, 0, 0, 192]);
    assert_eq!(alphas(&pix, 0, 54..60), [192, 0, 0, 255, 255, 255]);
}

#[test]
fn narrow_dotted_sides_hold_whole_dots_at_both_ends() {
    let pix = raster(
        vec![Item::Border {
            rect: Rect::new(0.0, 0.0, 60.0, 10.0),
            widths: Edges {
                top: 1.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            },
            colors: [rgb(0, 0, 0); 4],
            styles: [BorderStyle::Dotted; 4],
            radii: NO_RADII,
        }],
        60,
        10,
    );
    // An even length with 1 px dots grows the first dot to 2 px, then dots
    // fall on odd pixels up to the last pixel.
    let row = alphas(&pix, 0, 0..10);
    assert_eq!(row, [255, 255, 0, 255, 0, 255, 0, 255, 0, 255]);
    assert_eq!(px(&pix, 59, 0)[3], 255);
    assert_eq!(px(&pix, 58, 0)[3], 0);
}

#[test]
fn thick_dots_are_round() {
    let pix = raster(
        vec![Item::Border {
            rect: Rect::new(0.0, 0.0, 60.0, 16.0),
            widths: Edges {
                top: 8.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            },
            colors: [rgb(0, 0, 0); 4],
            styles: [BorderStyle::Dotted; 4],
            radii: NO_RADII,
        }],
        60,
        16,
    );
    // The first dot is centred at (4, 4) with radius 4: its middle is solid and
    // the square's corner pixel, at distance 4.2 from the centre, is empty.
    assert_eq!(px(&pix, 4, 4)[3], 255);
    assert_eq!(px(&pix, 0, 0)[3], 0);
    // Four dots span 60 px with gaps of 28 / 3, the last one centred 4 px from the end.
    assert_eq!(px(&pix, 56, 4)[3], 255);
    assert_eq!(px(&pix, 12, 4)[3], 0, "between the first two dots");
}

#[test]
fn dashed_and_dotted_borders_follow_a_rounded_outline() {
    for style in [BorderStyle::Dashed, BorderStyle::Dotted] {
        let item = Item::Border {
            rect: Rect::new(0.0, 0.0, 60.0, 40.0),
            widths: Edges::all(2.0),
            colors: [rgb(0, 0, 0); 4],
            styles: [style; 4],
            radii: [10.0; 8],
        };
        let pix = raster(vec![item], 60, 40);
        assert_eq!(px(&pix, 0, 0)[3], 0, "{style:?}: outside the arc");
        let top = alphas(&pix, 0, 12..48);
        assert!(top.contains(&255) && top.contains(&0), "{style:?}: {top:?}");
        assert_eq!(px(&pix, 30, 20)[3], 0, "{style:?}: inside is empty");
    }
}

/// A 20x20 box with 4 px borders of one style in `rgb(120, 160, 210)`, whose
/// shades are the Blink values `DARK` and `LIT`.
fn shaded_box(style: BorderStyle, width: f32) -> Pixmap {
    raster(
        vec![border(
            Rect::new(0.0, 0.0, 20.0, 20.0),
            width,
            rgb(120, 160, 210),
            style,
        )],
        20,
        20,
    )
}

const DARK: [u8; 4] = [72, 96, 126, 255];
const LIT: [u8; 4] = [146, 194, 255, 255];

#[test]
fn inset_and_outset_shade_opposite_sides_with_blinks_colours() {
    let inset = shaded_box(BorderStyle::Inset, 4.0);
    assert_eq!(px(&inset, 10, 1), DARK, "top");
    assert_eq!(px(&inset, 1, 10), DARK, "left");
    assert_eq!(px(&inset, 10, 18), LIT, "bottom");
    assert_eq!(px(&inset, 18, 10), LIT, "right");
    let outset = shaded_box(BorderStyle::Outset, 4.0);
    assert_eq!(px(&outset, 10, 1), LIT, "top");
    assert_eq!(px(&outset, 1, 10), LIT, "left");
    assert_eq!(px(&outset, 10, 18), DARK, "bottom");
    assert_eq!(px(&outset, 18, 10), DARK, "right");
}

#[test]
fn groove_is_dark_then_lit_from_the_outside_on_top_and_left() {
    let groove = shaded_box(BorderStyle::Groove, 4.0);
    let column: Vec<[u8; 4]> = (0..4).map(|y| px(&groove, 10, y)).collect();
    assert_eq!(column, [DARK, DARK, LIT, LIT], "top, outside in");
    let row: Vec<[u8; 4]> = (0..4).map(|x| px(&groove, x, 10)).collect();
    assert_eq!(row, [DARK, DARK, LIT, LIT], "left, outside in");
    let bottom: Vec<[u8; 4]> = (16..20).map(|y| px(&groove, 10, y)).collect();
    assert_eq!(bottom, [DARK, DARK, LIT, LIT], "bottom, inside out");
    let right: Vec<[u8; 4]> = (16..20).map(|x| px(&groove, x, 10)).collect();
    assert_eq!(right, [DARK, DARK, LIT, LIT], "right, inside out");
}

#[test]
fn ridge_is_the_mirror_of_groove() {
    let ridge = shaded_box(BorderStyle::Ridge, 4.0);
    let column: Vec<[u8; 4]> = (0..4).map(|y| px(&ridge, 10, y)).collect();
    assert_eq!(column, [LIT, LIT, DARK, DARK], "top, outside in");
    let bottom: Vec<[u8; 4]> = (16..20).map(|y| px(&ridge, 10, y)).collect();
    assert_eq!(bottom, [LIT, LIT, DARK, DARK], "bottom, inside out");
}

#[test]
fn an_odd_groove_gives_its_extra_pixel_to_the_first_half_in_reading_order() {
    let groove = shaded_box(BorderStyle::Groove, 3.0);
    let top: Vec<[u8; 4]> = (0..3).map(|y| px(&groove, 10, y)).collect();
    assert_eq!(top, [DARK, DARK, LIT], "top: outer half 2, inner half 1");
    let bottom: Vec<[u8; 4]> = (17..20).map(|y| px(&groove, 10, y)).collect();
    assert_eq!(
        bottom,
        [DARK, DARK, LIT],
        "bottom: inner half 2, outer half 1"
    );
}

#[test]
fn a_one_pixel_groove_is_a_solid_line() {
    let groove = shaded_box(BorderStyle::Groove, 1.0);
    assert_eq!(px(&groove, 10, 0), [120, 160, 210, 255]);
    assert_eq!(px(&groove, 10, 19), [120, 160, 210, 255]);
}

#[test]
fn black_inset_borders_stay_visible() {
    let inset = shaded_box_with(BorderStyle::Inset, rgb(0, 0, 0));
    assert_eq!(px(&inset, 10, 1), [84, 84, 84, 255]);
    assert_eq!(px(&inset, 10, 18), [168, 168, 168, 255]);
}

fn shaded_box_with(style: BorderStyle, color: Color) -> Pixmap {
    raster(
        vec![border(Rect::new(0.0, 0.0, 20.0, 20.0), 4.0, color, style)],
        20,
        20,
    )
}

#[test]
fn rasterising_twice_and_on_another_thread_is_byte_identical() {
    let items = || {
        vec![
            clip(Rect::new(3.0, 3.0, 40.0, 30.0), [6.0; 8]),
            Item::PushOpacity(0.7),
            fill(Rect::new(0.0, 0.0, 50.0, 40.0), rgb(10, 200, 30)),
            border(
                Rect::new(5.0, 5.0, 30.0, 20.0),
                3.0,
                rgb(0, 0, 0),
                BorderStyle::Groove,
            ),
            Item::PopOpacity,
            Item::PopClip,
        ]
    };
    let first = raster(items(), 50, 40);
    let second = raster(items(), 50, 40);
    let threaded = std::thread::spawn(move || raster(items(), 50, 40))
        .join()
        .expect("the rasteriser does not panic");
    assert_eq!(first.data(), second.data());
    assert_eq!(first.data(), threaded.data());
}

#[test]
fn png_round_trip_reproduces_straight_pixels() {
    let items = vec![
        Item::PushOpacity(0.5),
        fill(Rect::new(0.0, 0.0, 4.0, 4.0), rgb(255, 0, 0)),
        Item::PopOpacity,
    ];
    let pix = raster(items, 4, 4);
    for profile in [PngProfile::Fast, PngProfile::Best] {
        let (w, h, decoded) = decode_png(&encode_png(&pix, profile));
        assert_eq!((w, h), (4, 4));
        let [r, _, _, a] = px(&pix, 2, 2);
        assert_eq!(&decoded[..4], &[255, 0, 0, a], "{profile:?}");
        assert!(r.abs_diff(128) <= 1);
    }
}

#[test]
fn png_profiles_encode_the_same_pixels() {
    let pix = raster(
        vec![border(
            Rect::new(0.0, 0.0, 64.0, 48.0),
            3.0,
            rgb(9, 99, 199),
            BorderStyle::Ridge,
        )],
        64,
        48,
    );
    let fast = decode_png(&encode_png(&pix, PngProfile::Fast));
    let best = decode_png(&encode_png(&pix, PngProfile::Best));
    assert_eq!(fast.2, best.2);
}

#[test]
fn png_decodes_to_the_unpremultiplied_pixels_of_every_alpha() {
    let mut opts = options(256, 1);
    opts.background = None;
    let items = (0..=255_u8)
        .map(|alpha| {
            fill(
                Rect::new(f32::from(alpha), 0.0, 1.0, 1.0),
                Color::from_rgba8(200, 100, 50, alpha),
            )
        })
        .collect();
    let pix = raster_with(items, &opts);
    let (_, _, decoded) = decode_png(&encode_png(&pix, PngProfile::Fast));
    for (x, pixel) in decoded.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!(pixel[3], x as u8);
        match x {
            0 => assert_eq!(*pixel, [0, 0, 0, 0]),
            // Premultiplying to 8 bits loses precision for faint pixels; the
            // round trip stays within the quantisation of one premultiplied step.
            _ => {
                let step = (255.0 / x as f32).ceil() as u8;
                for (got, want) in pixel[..3].iter().zip([200_u8, 100, 50]) {
                    assert!(got.abs_diff(want) <= step, "alpha {x}: {pixel:?}");
                }
            }
        }
    }
}

#[test]
fn golden_fractional_rect() {
    let mut opts = options(40, 30);
    opts.background = Some(Color::WHITE);
    let items = vec![fill(
        Rect::new(10.3, 8.6, 12.5, 9.25),
        Color::from_rgba8(0, 128, 255, 200),
    )];
    assert_golden("fractional-rect", &raster_with(items, &opts));
}

#[test]
fn golden_border_styles_at_two_and_six_pixels() {
    let styles = [
        BorderStyle::Solid,
        BorderStyle::Dashed,
        BorderStyle::Dotted,
        BorderStyle::Double,
        BorderStyle::Groove,
        BorderStyle::Ridge,
        BorderStyle::Inset,
        BorderStyle::Outset,
        BorderStyle::None,
    ];
    let mut opts = options(9 * 64 + 8, 2 * 48 + 8);
    opts.background = Some(Color::WHITE);
    let mut items = Vec::new();
    for (column, style) in styles.into_iter().enumerate() {
        for (row, width) in [2.0_f32, 6.0].into_iter().enumerate() {
            let x = 8.0 + column as f32 * 64.0;
            let y = 8.0 + row as f32 * 48.0;
            items.push(border(
                Rect::new(x, y, 52.0, 36.0),
                width,
                rgb(120, 160, 210),
                style,
            ));
        }
    }
    assert_golden("border-styles", &raster_with(items, &opts));
}

#[test]
fn golden_mixed_side_borders() {
    let mut opts = options(3 * 70 + 10, 2 * 56 + 10);
    opts.background = Some(Color::WHITE);
    let sides = |top: BorderStyle, right: BorderStyle, bottom: BorderStyle, left: BorderStyle| {
        [top, right, bottom, left]
    };
    let cases = [
        (
            sides(
                BorderStyle::Dashed,
                BorderStyle::Solid,
                BorderStyle::Dashed,
                BorderStyle::Solid,
            ),
            [rgb(200, 40, 40); 4],
        ),
        (
            sides(
                BorderStyle::Solid,
                BorderStyle::Groove,
                BorderStyle::Dotted,
                BorderStyle::Double,
            ),
            [rgb(40, 120, 60); 4],
        ),
        (
            [BorderStyle::Solid; 4],
            [
                rgb(200, 40, 40),
                rgb(40, 140, 60),
                rgb(40, 60, 200),
                rgb(220, 160, 20),
            ],
        ),
        ([BorderStyle::Inset; 4], [rgb(150, 150, 200); 4]),
        (
            sides(
                BorderStyle::Dotted,
                BorderStyle::Dashed,
                BorderStyle::Dotted,
                BorderStyle::Dashed,
            ),
            [rgb(60, 60, 60); 4],
        ),
        (
            [BorderStyle::Solid; 4],
            [
                Color::from_rgba8(200, 40, 40, 128),
                Color::from_rgba8(200, 40, 40, 128),
                Color::from_rgba8(40, 40, 200, 128),
                Color::from_rgba8(40, 40, 200, 128),
            ],
        ),
    ];
    let mut items = vec![fill(Rect::new(0.0, 0.0, 400.0, 200.0), rgb(235, 235, 235))];
    for (index, (styles, colors)) in cases.into_iter().enumerate() {
        let x = 10.0 + (index % 3) as f32 * 70.0;
        let y = 10.0 + (index / 3) as f32 * 56.0;
        items.push(Item::Border {
            rect: Rect::new(x, y, 56.0, 40.0),
            widths: Edges::all(6.0),
            colors,
            styles,
            radii: NO_RADII,
        });
    }
    assert_golden("border-mixed-sides", &raster_with(items, &opts));
}

#[test]
fn golden_rounded_border_with_differing_radii() {
    let mut opts = options(120, 80);
    opts.background = Some(Color::WHITE);
    let items = vec![
        Item::FillRRect {
            rect: Rect::new(10.0, 10.0, 100.0, 60.0),
            radii: [20.0, 20.0, 4.0, 4.0, 30.0, 30.0, 0.0, 0.0],
            color: rgb(235, 235, 235),
        },
        Item::Border {
            rect: Rect::new(10.0, 10.0, 100.0, 60.0),
            widths: Edges::all(6.0),
            colors: [
                rgb(220, 30, 30),
                rgb(30, 160, 60),
                rgb(30, 60, 220),
                rgb(240, 150, 20),
            ],
            styles: [BorderStyle::Solid; 4],
            radii: [20.0, 20.0, 4.0, 4.0, 30.0, 30.0, 0.0, 0.0],
        },
    ];
    assert_golden("rounded-border", &raster_with(items, &opts));
}

#[test]
fn golden_rounded_border_styles() {
    let styles = [
        BorderStyle::Dashed,
        BorderStyle::Dotted,
        BorderStyle::Double,
        BorderStyle::Groove,
        BorderStyle::Ridge,
        BorderStyle::Inset,
    ];
    let mut opts = options(3 * 76 + 8, 2 * 56 + 8);
    opts.background = Some(Color::WHITE);
    let mut items = Vec::new();
    for (index, style) in styles.into_iter().enumerate() {
        let x = 8.0 + (index % 3) as f32 * 76.0;
        let y = 8.0 + (index / 3) as f32 * 56.0;
        items.push(Item::Border {
            rect: Rect::new(x, y, 64.0, 44.0),
            widths: Edges::all(5.0),
            colors: [rgb(120, 160, 210); 4],
            styles: [style; 4],
            radii: [14.0; 8],
        });
    }
    assert_golden("rounded-border-styles", &raster_with(items, &opts));
}

#[test]
fn golden_nested_clips() {
    let mut opts = options(60, 40);
    opts.background = Some(Color::WHITE);
    let items = vec![
        clip(Rect::new(5.0, 5.0, 50.0, 30.0), NO_RADII),
        clip(Rect::new(10.0, 8.0, 40.0, 24.0), [8.0; 8]),
        fill(Rect::new(0.0, 0.0, 60.0, 40.0), rgb(40, 90, 200)),
        Item::PopClip,
        fill(Rect::new(0.0, 0.0, 60.0, 6.0), rgb(20, 160, 60)),
        Item::PopClip,
        fill(Rect::new(56.0, 36.0, 4.0, 4.0), rgb(220, 30, 30)),
    ];
    assert_golden("nested-clips", &raster_with(items, &opts));
}

#[test]
fn golden_opacity_group_at_half_strength() {
    let mut opts = options(40, 30);
    opts.background = Some(Color::WHITE);
    let items = vec![
        Item::PushOpacity(0.5),
        fill(Rect::new(5.0, 5.0, 20.0, 20.0), rgb(220, 30, 30)),
        fill(Rect::new(10.0, 10.0, 10.0, 10.0), rgb(30, 60, 220)),
        Item::PopOpacity,
    ];
    assert_golden("opacity-group-50", &raster_with(items, &opts));
}

#[test]
fn golden_scale_two() {
    let opts = RasterOptions {
        width: 80,
        height: 60,
        scale: 2.0,
        origin: (0.0, 0.0),
        background: Some(Color::WHITE),
    };
    let items = vec![
        Item::Border {
            rect: Rect::new(4.0, 4.0, 30.0, 20.0),
            widths: Edges::all(2.0),
            colors: [rgb(240, 150, 20); 4],
            styles: [BorderStyle::Solid; 4],
            radii: [6.0; 8],
        },
        fill(Rect::new(10.0, 10.0, 8.0, 6.0), rgb(20, 150, 150)),
    ];
    assert_golden("scale-2x", &raster_with(items, &opts));
}

/// A representative 1280x800 page-like list: backgrounds, borders, rounded
/// cards, clipped regions and one opacity group. Timed by the ignored test.
fn representative_list() -> Vec<Item> {
    let mut items = vec![fill(Rect::new(0.0, 0.0, 1280.0, 800.0), rgb(246, 246, 246))];
    for row in 0..16 {
        for column in 0..10 {
            let x = 12.0 + column as f32 * 126.0;
            let y = 12.0 + row as f32 * 48.0;
            items.push(Item::FillRRect {
                rect: Rect::new(x, y, 118.0, 40.0),
                radii: [6.0; 8],
                color: rgb(255, 255, 255),
            });
            items.push(Item::Border {
                rect: Rect::new(x, y, 118.0, 40.0),
                widths: Edges::all(1.0),
                colors: [rgb(200, 200, 210); 4],
                styles: [BorderStyle::Solid; 4],
                radii: [6.0; 8],
            });
        }
    }
    items.push(clip(Rect::new(100.0, 100.0, 600.0, 400.0), [12.0; 8]));
    items.push(Item::PushOpacity(0.8));
    items.push(border(
        Rect::new(120.0, 120.0, 400.0, 200.0),
        3.0,
        rgb(30, 60, 220),
        BorderStyle::Dashed,
    ));
    items.push(fill(
        Rect::new(300.0, 300.0, 200.0, 100.0),
        rgb(20, 160, 60),
    ));
    items.push(Item::PopOpacity);
    items.push(Item::PopClip);
    items
}

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

/// Prints raster and PNG-encode timings for the 1280x800 budget. Run in release:
/// `cargo nextest run -p tenta-paint --release --run-ignored only --no-capture timing`.
#[test]
#[ignore = "timing report, run in release with --run-ignored only"]
fn timing_report_1280x800() {
    const RUNS: usize = 30;
    let list = DisplayList {
        items: representative_list(),
        bounds: Rect::default(),
        ..DisplayList::default()
    };
    let opts = options(1280, 800);
    let images = ImageStore::new();
    let mut raster_times = Vec::with_capacity(RUNS);
    let mut encode_times = Vec::with_capacity(RUNS);
    let mut last = None;
    for _ in 0..RUNS {
        let start = Instant::now();
        let out = rasterize(&list, &opts, &images).expect("supported items");
        raster_times.push(start.elapsed());
        let start = Instant::now();
        let encoded = encode_png(&out.pixmap, PngProfile::Fast);
        encode_times.push(start.elapsed());
        last = Some((out, encoded));
    }
    raster_times.sort();
    encode_times.sort();
    let (out, encoded) = last.expect("at least one run");
    assert!(out.stats.is_complete());
    eprintln!(
        "raster 1280x800 ({} items): p50 {:?} p95 {:?}",
        list.items.len(),
        percentile(&raster_times, 0.5),
        percentile(&raster_times, 0.95)
    );
    eprintln!(
        "png fast 1280x800 ({} bytes): p50 {:?} p95 {:?}",
        encoded.len(),
        percentile(&encode_times, 0.5),
        percentile(&encode_times, 0.95)
    );
    assert_eq!((out.pixmap.width(), out.pixmap.height()), (1280, 800));
}

// ------------------------------------------------------------------ images

mod images {
    use super::*;
    use crate::image::{DecodeParams, DecodedImage, ImageId, ImageQuality, StoreKey};
    use tenta_layout::ImageKey;

    /// A store holding a `w x h` image whose pixel `(x, y)` is `pixel(x, y)`
    /// (premultiplied RGBA).
    fn store_with(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> (ImageStore, ImageId) {
        let rgba: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| pixel(x, y))
            .collect();
        let mut store = ImageStore::new();
        let key = StoreKey {
            url: ImageKey("test:raster".into()),
            params: DecodeParams::default(),
        };
        let image = DecodedImage {
            width: w,
            height: h,
            rgba: rgba.into(),
            source_width: w,
            source_height: h,
        };
        let id = store.insert(key, image, 1.0).expect("fits");
        (store, id)
    }

    fn draw(store: &ImageStore, items: Vec<Item>, opts: &RasterOptions) -> RasterOutput {
        let list = DisplayList {
            items,
            ..DisplayList::default()
        };
        rasterize(&list, opts, store).expect("the output size is valid")
    }

    fn image(id: ImageId, dest: Rect, quality: ImageQuality) -> Item {
        Item::Image {
            dest,
            src: id,
            src_rect: None,
            quality,
        }
    }

    /// Red left half, blue right half, 2x1.
    fn halves(x: u32, _y: u32) -> [u8; 4] {
        if x == 0 { RED } else { BLUE }
    }

    #[test]
    fn an_image_at_its_own_size_is_copied_exactly() {
        let (store, id) = store_with(2, 1, halves);
        let out = draw(
            &store,
            vec![image(
                id,
                Rect::new(1.0, 1.0, 2.0, 1.0),
                ImageQuality::Smooth,
            )],
            &options(4, 3),
        );
        assert!(out.stats.is_complete());
        assert_eq!(px(&out.pixmap, 1, 1), RED);
        assert_eq!(px(&out.pixmap, 2, 1), BLUE);
        assert_eq!(px(&out.pixmap, 0, 1), CLEAR);
        assert_eq!(px(&out.pixmap, 3, 1), CLEAR);
        assert_eq!(px(&out.pixmap, 1, 0), CLEAR);
    }

    #[test]
    fn pixelated_scaling_keeps_hard_edges_and_smooth_scaling_blends_them() {
        let (store, id) = store_with(2, 1, halves);
        let dest = Rect::new(0.0, 0.0, 20.0, 10.0);
        let pixelated = draw(
            &store,
            vec![image(id, dest, ImageQuality::Pixelated)],
            &options(20, 10),
        );
        assert_eq!(px(&pixelated.pixmap, 9, 5), RED);
        assert_eq!(px(&pixelated.pixmap, 10, 5), BLUE);
        let smooth = draw(
            &store,
            vec![image(id, dest, ImageQuality::Smooth)],
            &options(20, 10),
        );
        let edge = px(&smooth.pixmap, 10, 5);
        assert!(edge[0] > 0 && edge[2] > 0, "the seam is blended: {edge:?}");
        assert_eq!(
            px(&smooth.pixmap, 2, 5),
            RED,
            "far from the seam it is flat"
        );
        assert_eq!(px(&smooth.pixmap, 17, 5), BLUE);
    }

    #[test]
    fn a_source_rectangle_selects_part_of_the_image() {
        let (store, id) = store_with(2, 1, halves);
        let item = Item::Image {
            dest: Rect::new(0.0, 0.0, 4.0, 4.0),
            src: id,
            src_rect: Some(Rect::new(1.0, 0.0, 1.0, 1.0)),
            quality: ImageQuality::Pixelated,
        };
        let out = draw(&store, vec![item], &options(4, 4));
        assert_eq!(px(&out.pixmap, 0, 0), BLUE);
        assert_eq!(px(&out.pixmap, 3, 3), BLUE);
    }

    #[test]
    fn a_large_downscale_averages_instead_of_sampling() {
        // A 64x64 checkerboard of 1 px squares drawn at 8x8 averages to grey,
        // where plain bilinear sampling would pick one colour.
        let (store, id) = store_with(64, 64, |x, y| {
            if (x + y) % 2 == 0 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        });
        let out = draw(
            &store,
            vec![image(
                id,
                Rect::new(0.0, 0.0, 8.0, 8.0),
                ImageQuality::Smooth,
            )],
            &options(8, 8),
        );
        for (x, y) in [(1, 1), (4, 4), (6, 2)] {
            let p = px(&out.pixmap, x, y);
            assert!(p[0].abs_diff(128) <= 4, "grey at {x},{y}: {p:?}");
        }
    }

    #[test]
    fn images_obey_clips_opacity_and_scale() {
        let (store, id) = store_with(2, 1, halves);
        let items = vec![
            clip(Rect::new(0.0, 0.0, 5.0, 10.0), NO_RADII),
            Item::PushOpacity(0.5),
            image(id, Rect::new(0.0, 0.0, 20.0, 10.0), ImageQuality::Pixelated),
            Item::PopOpacity,
            Item::PopClip,
        ];
        let out = draw(&store, items, &options(20, 10));
        let inside = px(&out.pixmap, 2, 5);
        assert!(inside[3].abs_diff(128) <= 1, "half opaque: {inside:?}");
        assert_eq!(px(&out.pixmap, 6, 5), CLEAR, "clipped");
        // At a device scale of 2 the image covers twice the pixels.
        let mut scaled = options(8, 4);
        scaled.scale = 2.0;
        let out = draw(
            &store,
            vec![image(
                id,
                Rect::new(0.0, 0.0, 2.0, 1.0),
                ImageQuality::Pixelated,
            )],
            &scaled,
        );
        assert_eq!(px(&out.pixmap, 1, 1), RED);
        assert_eq!(px(&out.pixmap, 2, 1), BLUE);
        assert_eq!(px(&out.pixmap, 4, 1), CLEAR);
    }

    #[test]
    fn a_rotated_image_still_draws() {
        let (store, id) = store_with(2, 2, |_, _| RED);
        let items = vec![
            Item::PushTransform([0.0, 1.0, -1.0, 0.0, 10.0, 0.0]),
            image(id, Rect::new(0.0, 0.0, 8.0, 4.0), ImageQuality::Smooth),
            Item::PopTransform,
        ];
        let out = draw(&store, items, &options(12, 12));
        // A quarter turn maps the 8x4 box to x in 6..10, y in 0..8.
        assert_eq!(px(&out.pixmap, 8, 4), RED);
        assert_eq!(px(&out.pixmap, 2, 4), CLEAR);
    }

    #[test]
    fn an_id_the_store_lacks_is_skipped_and_counted() {
        let (store, id) = store_with(1, 1, |_, _| RED);
        let missing = ImageId(id.0 + 7);
        let out = draw(
            &store,
            vec![image(
                missing,
                Rect::new(0.0, 0.0, 2.0, 2.0),
                ImageQuality::Smooth,
            )],
            &options(2, 2),
        );
        assert_eq!(out.stats.skipped.get("Image"), Some(&1));
    }

    #[test]
    fn an_opacity_group_holding_only_an_image_keeps_its_pixels() {
        let (store, id) = store_with(1, 1, |_, _| [255, 0, 0, 255]);
        let items = vec![
            Item::PushOpacity(0.5),
            image(id, Rect::new(2.0, 2.0, 4.0, 4.0), ImageQuality::Smooth),
            Item::PopOpacity,
        ];
        let out = draw(&store, items, &options(8, 8));
        assert!(px(&out.pixmap, 3, 3)[3] > 100);
    }

    // ------------------------------------------------------------ gradients

    fn gradient(angle: f32, stops: Vec<(f32, Color)>, repeat: bool, radii: Radii) -> Item {
        Item::LinearGradient {
            rect: Rect::new(0.0, 0.0, 100.0, 10.0),
            radii,
            angle,
            stops,
            repeat,
        }
    }

    #[test]
    fn a_gradient_runs_along_its_angle() {
        let stops = vec![(0.0, rgb(255, 0, 0)), (1.0, rgb(0, 0, 255))];
        let out = raster(
            vec![gradient(90.0, stops.clone(), false, NO_RADII)],
            100,
            10,
        );
        let (left, mid, right) = (px(&out, 1, 5), px(&out, 50, 5), px(&out, 98, 5));
        assert!(left[0] > 245 && left[2] < 10, "{left:?}");
        assert!(
            mid[0].abs_diff(127) < 6 && mid[2].abs_diff(127) < 6,
            "{mid:?}"
        );
        assert!(right[2] > 245 && right[0] < 10, "{right:?}");
        // Pointing up, the colour changes along y and not along x.
        let up = raster(
            vec![Item::LinearGradient {
                rect: Rect::new(0.0, 0.0, 10.0, 100.0),
                radii: NO_RADII,
                angle: 0.0,
                stops,
                repeat: false,
            }],
            10,
            100,
        );
        assert!(px(&up, 5, 98)[0] > 245, "red at the bottom");
        assert!(px(&up, 5, 1)[2] > 245, "blue at the top");
        assert_eq!(px(&up, 0, 50), px(&up, 9, 50));
    }

    #[test]
    fn a_repeating_gradient_restarts_each_period() {
        let stops = vec![(0.0, rgb(255, 0, 0)), (0.2, rgb(0, 0, 255))];
        let out = raster(vec![gradient(90.0, stops, true, NO_RADII)], 100, 10);
        // The period is 20 px: red restarts at 20, 40, ...
        for start in [0u32, 20, 40, 60] {
            let p = px(&out, start + 1, 5);
            assert!(p[0] > 200, "red near {start}: {p:?}");
            let q = px(&out, start + 18, 5);
            assert!(
                q[2] > 200,
                "blue near the end of the period at {start}: {q:?}"
            );
        }
    }

    #[test]
    fn fading_to_transparent_keeps_the_colour_of_the_visible_stop() {
        let stops = vec![(0.0, rgb(255, 0, 0)), (1.0, Color::TRANSPARENT)];
        let out = raster_with(
            vec![
                fill(Rect::new(0.0, 0.0, 100.0, 10.0), rgb(255, 255, 255)),
                gradient(90.0, stops, false, NO_RADII),
            ],
            &options(100, 10),
        );
        // Over white, half way is half red: (255, 128, 128), not a grey one.
        let mid = px(&out, 50, 5);
        assert_eq!(mid[0], 255);
        assert!(
            mid[1].abs_diff(128) <= 4 && mid[2].abs_diff(128) <= 4,
            "{mid:?}"
        );
    }

    #[test]
    fn a_rounded_gradient_leaves_the_corners_clear() {
        let stops = vec![(0.0, rgb(255, 0, 0)), (1.0, rgb(255, 0, 0))];
        let out = raster(vec![gradient(90.0, stops, false, [5.0; 8])], 100, 10);
        assert_eq!(px(&out, 0, 0), CLEAR);
        assert_eq!(px(&out, 50, 5), RED);
    }

    #[test]
    fn stops_outside_the_line_still_shape_the_colours_inside_it() {
        // Red at -0.5 and blue at 0.5: the line starts half way between them.
        let stops = vec![(-0.5, rgb(255, 0, 0)), (0.5, rgb(0, 0, 255))];
        let out = raster(vec![gradient(90.0, stops, false, NO_RADII)], 100, 10);
        let (start, end) = (px(&out, 0, 5), px(&out, 99, 5));
        assert!(
            start[0].abs_diff(127) < 8 && start[2].abs_diff(127) < 8,
            "{start:?}"
        );
        assert!(end[2] > 245 && end[0] < 8, "{end:?}");
    }

    #[test]
    fn a_semi_transparent_stop_interpolates_premultiplied() {
        // Opaque red to half-transparent blue: premultiplied, the mid point
        // is (127, 0, 64) at alpha 0.75, not a straight blend of the colours.
        let blue_half = Color::from_rgba8(0, 0, 255, 128);
        let stops = vec![(0.0, rgb(255, 0, 0)), (1.0, blue_half)];
        let out = raster(vec![gradient(90.0, stops, false, NO_RADII)], 100, 10);
        let mid = px(&out, 50, 5);
        assert!(mid[3].abs_diff(191) < 6, "alpha {mid:?}");
        assert!(
            mid[0].abs_diff(127) < 8 && mid[2].abs_diff(64) < 8,
            "{mid:?}"
        );
    }

    #[test]
    fn a_pattern_repeats_a_tile_over_thousands_of_rows() {
        // A 1x2 tile: red over blue, 1 px wide and 2 px tall, down 8000 px.
        let (store, id) = store_with(1, 2, |_, y| if y == 0 { RED } else { BLUE });
        let item = Item::ImagePattern {
            area: Rect::new(2.0, 0.0, 3.0, 8000.0),
            origin: (2.0, 0.0),
            tile: (1.0, 2.0),
            src: id,
            quality: ImageQuality::Pixelated,
        };
        let out = draw(&store, vec![item], &options(8, 8000));
        assert!(out.stats.is_complete());
        for row in [0u32, 1, 2, 3999 * 2, 3999 * 2 + 1, 7998, 7999] {
            let want = if row % 2 == 0 { RED } else { BLUE };
            assert_eq!(px(&out.pixmap, 3, row), want, "row {row}");
        }
        assert_eq!(px(&out.pixmap, 1, 10), CLEAR, "left of the area");
        assert_eq!(px(&out.pixmap, 5, 10), CLEAR, "right of the area");
    }
}
