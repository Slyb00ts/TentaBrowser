//! The text, clip, effect and marker emitters driven through the public
//! builder and rasteriser, over hand-built fragment trees.

use std::path::PathBuf;
use std::sync::Arc;

use skrifa::instance::{LocationRef, Size};
use skrifa::{GlyphId, MetadataProvider};
use tenta_core::NodeId;
use tenta_layout::embedded_fonts::EMBEDDED_FONTS;
use tenta_layout::{
    FragFlags, FragId, Fragment, FragmentBuilder, FragmentKind, FragmentTree, TextFragment,
};
use tenta_paint::{
    Affine, BackgroundContent, BackgroundLayer, BackgroundSize, BorderStyle, BoxArea, Color,
    DecorationPaint, Edges, FontBlob, FontRef, Glyph, GlyphRun, ImageStore, Item, LengthPercent,
    LineStyle, MarkerKind, PaintContext, PaintStyle, Pixmap, PngProfile, RasterOptions,
    RasterOutput, Rect, RepeatKind, ShadowPaint, build, encode_png, rasterize,
};

const SANS: usize = 0;
const ASCENT_16: f32 = 14.0;

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

fn style(edit: impl FnOnce(&mut PaintStyle)) -> PaintStyle {
    let mut style = PaintStyle::default();
    edit(&mut style);
    style
}

fn filled(color: Color) -> PaintStyle {
    style(|s| {
        s.background = vec![BackgroundLayer {
            content: BackgroundContent::Color(color),
            repeat: (RepeatKind::Repeat, RepeatKind::Repeat),
            position: (LengthPercent::default(), LengthPercent::default()),
            size: BackgroundSize::Auto,
            origin: BoxArea::BorderBox,
            clip: BoxArea::BorderBox,
        }]
    })
}

fn sans() -> FontRef {
    FontRef {
        data: Arc::new(FontBlob::Static(EMBEDDED_FONTS[SANS].bytes)),
        index: 0,
        coords: Arc::from([]),
    }
}

/// Lays `text` out left to right with the font's own advances; returns the
/// run and its width.
fn shaped(text: &str, size: f32) -> (Arc<GlyphRun>, f32) {
    let font = sans();
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
    let run = GlyphRun {
        font,
        size,
        glyphs,
        synthetic_bold: false,
        skew: 0.0,
    };
    (Arc::new(run), x)
}

struct Scene {
    builder: FragmentBuilder,
    root: FragId,
    extent: Rect,
}

impl Scene {
    fn new(width: f32, height: f32, root_style: PaintStyle) -> Self {
        let extent = Rect::new(0.0, 0.0, width, height);
        let mut builder = FragmentBuilder::new();
        let root = builder.push(Fragment::new(
            NodeId(1),
            FragmentKind::Box,
            Arc::new(root_style),
            extent,
        ));
        Scene {
            builder,
            root,
            extent,
        }
    }

    fn add(
        &mut self,
        parent: FragId,
        kind: FragmentKind,
        rect: Rect,
        paint: PaintStyle,
        edit: impl FnOnce(&mut Fragment),
    ) -> FragId {
        let mut frag = Fragment::new(NodeId(2), kind, Arc::new(paint), rect);
        edit(&mut frag);
        let id = self.builder.push(frag);
        self.builder.add_child(parent, id);
        id
    }

    fn boxed(&mut self, parent: FragId, rect: Rect, paint: PaintStyle) -> FragId {
        self.add(parent, FragmentKind::Box, rect, paint, |_| {})
    }

    /// A line box at `y` holding one text fragment at `x`; returns the text.
    fn text(
        &mut self,
        parent: FragId,
        (x, y): (f32, f32),
        text: &str,
        size: f32,
        paint: PaintStyle,
    ) -> FragId {
        let (run, width) = shaped(text, size);
        let height = (size * 1.15).ceil();
        let baseline = (size * 0.905).round();
        let line = self.add(
            parent,
            FragmentKind::Line,
            Rect::new(x, y, width, height),
            PaintStyle::default(),
            |_| {},
        );
        let fragment = TextFragment {
            text_range: 0..text.len() as u32,
            runs: std::iter::once(run).collect(),
            baseline,
        };
        self.add(
            line,
            FragmentKind::Text(fragment),
            Rect::new(0.0, 0.0, width, height),
            paint,
            |_| {},
        )
    }

    fn finish(self) -> FragmentTree {
        self.builder.finish(self.root, self.extent, self.extent)
    }
}

fn items(tree: &FragmentTree) -> Vec<Item> {
    let list = build(
        tree,
        &PaintContext {
            images: &ImageStore::new(),
        },
    );
    assert!(list.is_balanced());
    list.items
}

fn render(tree: &FragmentTree) -> RasterOutput {
    let list = build(
        tree,
        &PaintContext {
            images: &ImageStore::new(),
        },
    );
    let opts = RasterOptions {
        width: tree.viewport.w as u32,
        height: tree.viewport.h as u32,
        scale: 1.0,
        origin: (0.0, 0.0),
        background: None,
    };
    rasterize(&list, &opts, &ImageStore::new()).expect("valid size")
}

fn px(pix: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * pix.width() + x) * 4) as usize;
    pix.data()[offset..offset + 4]
        .try_into()
        .expect("four channels")
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

/// Compares with the blessed golden, within one unit per channel;
/// `UPDATE_GOLDEN=1` rewrites it.
fn assert_golden(name: &str, pix: &Pixmap) {
    let encoded = encode_png(pix, PngProfile::Fast);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/render-goldens")
        .join(format!("{name}.png"));
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
    assert!(worst <= 1, "golden {name} differs by {worst} per channel");
}

/// Short names for the shape of a list.
fn shape(items: &[Item]) -> Vec<&'static str> {
    items
        .iter()
        .map(|item| match item {
            Item::FillRect { .. } => "fill",
            Item::FillRRect { .. } => "rfill",
            Item::Border { .. } => "border",
            Item::Text { .. } => "text",
            Item::Decoration { .. } => "line",
            Item::PushClip { .. } => "clip{",
            Item::PopClip => "}clip",
            Item::PushOpacity(_) => "opacity{",
            Item::PopOpacity => "}opacity",
            Item::PushTransform(_) => "transform{",
            Item::PopTransform => "}transform",
            _ => "other",
        })
        .collect()
}

#[test]
fn an_overflow_clip_brackets_exactly_its_descendants() {
    let mut scene = Scene::new(200.0, 100.0, PaintStyle::default());
    let root = scene.root;
    let clipping = scene.add(
        root,
        FragmentKind::Box,
        Rect::new(10.0, 10.0, 80.0, 40.0),
        PaintStyle {
            overflow_x: tenta_paint::OverflowKind::Hidden,
            overflow_y: tenta_paint::OverflowKind::Hidden,
            ..filled(rgb(1, 0, 0))
        },
        |f| f.flags = FragFlags::CLIPS_OVERFLOW,
    );
    scene.boxed(
        clipping,
        Rect::new(0.0, 0.0, 200.0, 10.0),
        filled(rgb(2, 0, 0)),
    );
    scene.boxed(
        clipping,
        Rect::new(0.0, 20.0, 10.0, 10.0),
        filled(rgb(3, 0, 0)),
    );
    scene.boxed(
        root,
        Rect::new(100.0, 10.0, 10.0, 10.0),
        filled(rgb(4, 0, 0)),
    );
    let list = items(&scene.finish());
    assert_eq!(
        shape(&list),
        ["fill", "clip{", "fill", "fill", "}clip", "fill"]
    );
    assert_eq!(
        list[1],
        Item::PushClip {
            rect: Rect::new(10.0, 10.0, 80.0, 40.0),
            radii: [0.0; 8],
        }
    );
}

#[test]
fn an_opacity_group_and_a_transform_wrap_the_right_subtree() {
    let mut scene = Scene::new(200.0, 100.0, PaintStyle::default());
    let root = scene.root;
    let group = scene.boxed(
        root,
        Rect::new(10.0, 10.0, 40.0, 20.0),
        PaintStyle {
            opacity: 0.5,
            ..filled(rgb(1, 0, 0))
        },
    );
    scene.boxed(group, Rect::new(0.0, 0.0, 5.0, 5.0), filled(rgb(2, 0, 0)));
    let turned = scene.boxed(
        root,
        Rect::new(100.0, 10.0, 40.0, 20.0),
        PaintStyle {
            transform: Some(Affine::scale(2.0, 2.0)),
            ..filled(rgb(3, 0, 0))
        },
    );
    scene.boxed(turned, Rect::new(0.0, 0.0, 5.0, 5.0), filled(rgb(4, 0, 0)));
    scene.boxed(
        root,
        Rect::new(150.0, 10.0, 10.0, 10.0),
        filled(rgb(5, 0, 0)),
    );
    let list = items(&scene.finish());
    assert_eq!(
        shape(&list),
        [
            // The plain box belongs to the in-flow layer, which precedes the
            // z-index 0 contexts.
            "fill",
            "opacity{",
            "fill",
            "fill",
            "}opacity",
            "transform{",
            "fill",
            "fill",
            "}transform",
        ]
    );
    assert_eq!(list[1], Item::PushOpacity(0.5));
}

#[test]
fn hidden_text_is_skipped_while_visible_text_in_the_same_box_paints() {
    let mut scene = Scene::new(200.0, 100.0, PaintStyle::default());
    let root = scene.root;
    let block = scene.boxed(
        root,
        Rect::new(0.0, 0.0, 200.0, 40.0),
        PaintStyle::default(),
    );
    scene.text(
        block,
        (0.0, 0.0),
        "hidden",
        16.0,
        style(|s| s.visibility_hidden = true),
    );
    scene.text(block, (0.0, 20.0), "shown", 16.0, PaintStyle::default());
    let list = items(&scene.finish());
    assert_eq!(shape(&list), ["text"]);
    let Item::Text { origin, .. } = &list[0] else {
        panic!("expected text");
    };
    assert_eq!(*origin, (0.0, 20.0 + ASCENT_16));
}

#[test]
fn underlined_text_rasterises_to_the_golden() {
    let mut scene = Scene::new(210.0, 64.0, filled(rgb(255, 255, 255)));
    let root = scene.root;
    scene.text(
        root,
        (10.0, 6.0),
        "Hamburgefonstiv",
        20.0,
        style(|s| {
            s.color = rgb(20, 40, 160);
            s.text_decoration = DecorationPaint {
                underline: true,
                color: rgb(20, 40, 160),
                ..DecorationPaint::default()
            };
        }),
    );
    scene.text(
        root,
        (10.0, 36.0),
        "Wavy and struck",
        16.0,
        style(|s| {
            s.text_decoration = DecorationPaint {
                underline: true,
                line_through: true,
                style: LineStyle::Wavy,
                color: rgb(200, 0, 0),
                ..DecorationPaint::default()
            };
            s.text_shadow = vec![ShadowPaint {
                offset: (1.0, 1.0),
                blur: 0.0,
                spread: 0.0,
                color: rgb(190, 190, 190),
                inset: false,
            }];
        }),
    );
    let out = render(&scene.finish());
    // Skip-ink is not drawn and is reported once per underlined fragment.
    assert_eq!(
        out.stats.skipped,
        std::collections::BTreeMap::from([("TextDecorationSkipInk", 2)])
    );
    // The underline of the first line sits one pixel under its baseline
    // (6 + round(0.905 * 20) = 24): rows 25 and 26 hold the line, a pixel
    // thick plus the 0.17 px remainder.
    let under = px(&out.pixmap, 40, 25);
    assert!(under[0] < 60 && under[2] > 100, "blue line, got {under:?}");
    assert_eq!(px(&out.pixmap, 40, 29), [255, 255, 255, 255]);
    assert_golden("build-text-decoration", &out.pixmap);
}

#[test]
fn an_overflow_hidden_rounded_box_clips_its_text_and_children() {
    let mut scene = Scene::new(170.0, 90.0, filled(rgb(255, 255, 255)));
    let root = scene.root;
    let card = scene.add(
        root,
        FragmentKind::Box,
        Rect::new(10.0, 10.0, 150.0, 70.0),
        PaintStyle {
            border: tenta_paint::BorderPaint {
                widths: Edges::all(3.0),
                colors: [rgb(30, 30, 80); 4],
                styles: [BorderStyle::Solid; 4],
                radii: [LengthPercent::px(26.0); 8],
            },
            overflow_x: tenta_paint::OverflowKind::Hidden,
            overflow_y: tenta_paint::OverflowKind::Hidden,
            ..filled(rgb(225, 235, 255))
        },
        |f| {
            f.flags = FragFlags::CLIPS_OVERFLOW;
            f.border = Edges::all(3.0);
        },
    );
    // A swatch hanging out of the top-left corner and a headline running out
    // of the right edge and the bottom.
    scene.boxed(
        card,
        Rect::new(-10.0, -10.0, 50.0, 40.0),
        filled(rgb(230, 80, 60)),
    );
    scene.text(
        card,
        (30.0, 30.0),
        "Overflowing headline text",
        30.0,
        style(|s| s.color = rgb(10, 10, 10)),
    );
    let tree = scene.finish();
    let list = items(&tree);
    assert_eq!(
        shape(&list),
        ["fill", "rfill", "border", "clip{", "fill", "text", "}clip"]
    );
    // Padding box (13, 13, 144, 64) with the radii less the 3 px border.
    assert_eq!(
        list[3],
        Item::PushClip {
            rect: Rect::new(13.0, 13.0, 144.0, 64.0),
            radii: [23.0; 8],
        }
    );
    let out = render(&tree);
    assert!(out.stats.is_complete(), "{:?}", out.stats.skipped);
    // Outside the rounded corner the red swatch must not show.
    assert_eq!(px(&out.pixmap, 12, 12), [255, 255, 255, 255]);
    // Past the right edge the headline is cut off.
    assert_eq!(px(&out.pixmap, 163, 40), [255, 255, 255, 255]);
    assert_golden("build-overflow-rounded-clip", &out.pixmap);
}

#[test]
fn an_opacity_group_blends_as_one_surface() {
    let mut scene = Scene::new(120.0, 70.0, filled(rgb(255, 255, 255)));
    let root = scene.root;
    scene.boxed(
        root,
        Rect::new(10.0, 10.0, 60.0, 50.0),
        filled(rgb(0, 0, 255)),
    );
    let group = scene.boxed(
        root,
        Rect::new(40.0, 20.0, 70.0, 40.0),
        PaintStyle {
            opacity: 0.5,
            ..filled(rgb(255, 0, 0))
        },
    );
    // A child overlapping its parent inside the group does not double-blend.
    scene.boxed(
        group,
        Rect::new(10.0, 10.0, 30.0, 20.0),
        filled(rgb(0, 160, 0)),
    );
    scene.text(
        group,
        (36.0, 4.0),
        "50%",
        16.0,
        style(|s| s.color = rgb(0, 0, 0)),
    );
    let out = render(&scene.finish());
    assert!(out.stats.is_complete(), "{:?}", out.stats.skipped);
    // Red over white at one half, red over blue at one half, and the green
    // child over the red parent inside the group, then the group at one half.
    assert_eq!(px(&out.pixmap, 100, 55), [255, 128, 128, 255]);
    assert_eq!(px(&out.pixmap, 45, 25), [128, 0, 128, 255]);
    assert_eq!(px(&out.pixmap, 75, 40), [128, 208, 128, 255]);
    assert_golden("build-opacity-group", &out.pixmap);
}

#[test]
fn a_rotated_box_with_text_rasterises_to_the_golden() {
    let mut scene = Scene::new(120.0, 90.0, filled(rgb(255, 255, 255)));
    let root = scene.root;
    let turned = scene.boxed(
        root,
        Rect::new(30.0, 25.0, 60.0, 30.0),
        PaintStyle {
            transform: Some(Affine {
                a: 0.866_025_4,
                b: 0.5,
                c: -0.5,
                d: 0.866_025_4,
                e: 0.0,
                f: 0.0,
            }),
            ..filled(rgb(255, 235, 150))
        },
    );
    scene.text(
        turned,
        (6.0, 6.0),
        "Rotated",
        16.0,
        style(|s| s.color = rgb(20, 20, 20)),
    );
    let out = render(&scene.finish());
    assert!(out.stats.is_complete(), "{:?}", out.stats.skipped);
    // The centre of the box is the fixed point of the default origin.
    assert_eq!(px(&out.pixmap, 60, 40)[3], 255);
    assert_golden("build-rotated-box-text", &out.pixmap);
}

#[test]
fn list_markers_rasterise_to_the_golden() {
    let mut scene = Scene::new(150.0, 100.0, filled(rgb(255, 255, 255)));
    let root = scene.root;
    let kinds = [
        (MarkerKind::Disc, "Disc marker"),
        (MarkerKind::Circle, "Circle marker"),
        (MarkerKind::Square, "Square marker"),
    ];
    for (row, (kind, label)) in kinds.into_iter().enumerate() {
        let top = 4.0 + 22.0 * row as f32;
        let item = scene.boxed(
            root,
            Rect::new(0.0, top, 150.0, 18.0),
            PaintStyle::default(),
        );
        // Blink's bullet at 16 px: a square (14 * 2 / 3 + 1) / 2 = 5 wide, set
        // 3 * (14 - 14 * 2 / 3) / 2 = 7 below the line's top.
        scene.add(
            item,
            FragmentKind::Marker(kind),
            Rect::new(9.0, 7.0, 5.0, 5.0),
            style(|s| s.color = rgb(160, 30, 30)),
            |_| {},
        );
        scene.text(item, (24.0, 0.0), label, 16.0, PaintStyle::default());
    }
    // A decimal marker is a text child of its marker fragment.
    let item = scene.boxed(
        root,
        Rect::new(0.0, 70.0, 150.0, 18.0),
        PaintStyle::default(),
    );
    let marker = scene.add(
        item,
        FragmentKind::Marker(MarkerKind::Text),
        Rect::new(0.0, 0.0, 20.0, 18.0),
        PaintStyle::default(),
        |_| {},
    );
    scene.text(marker, (4.0, 0.0), "1.", 16.0, PaintStyle::default());
    scene.text(
        item,
        (24.0, 0.0),
        "Decimal marker",
        16.0,
        PaintStyle::default(),
    );
    let tree = scene.finish();
    let list = items(&tree);
    assert_eq!(
        shape(&list),
        [
            "fill", "rfill", "text", "border", "text", "fill", "text", "text", "text"
        ]
    );
    let out = render(&tree);
    assert!(out.stats.is_complete(), "{:?}", out.stats.skipped);
    // The disc is solid at its centre and absent at its box corner.
    assert_eq!(px(&out.pixmap, 11, 15), [160, 30, 30, 255]);
    assert_golden("build-list-markers", &out.pixmap);
}

#[test]
fn filters_make_the_rasterisation_incomplete() {
    let mut scene = Scene::new(40.0, 40.0, PaintStyle::default());
    let root = scene.root;
    scene.boxed(
        root,
        Rect::new(5.0, 5.0, 10.0, 10.0),
        PaintStyle {
            filter: vec![tenta_paint::FilterOp::Blur(2.0)],
            ..filled(rgb(1, 0, 0))
        },
    );
    let out = render(&scene.finish());
    assert!(!out.stats.is_complete());
    assert_eq!(
        out.stats.skipped,
        std::collections::BTreeMap::from([("Filter", 1)])
    );
}
