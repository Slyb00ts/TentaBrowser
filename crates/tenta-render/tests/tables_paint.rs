//! Painting of tables: background layers, collapsed borders, `empty-cells`
//! and captions, through the whole pipeline to pixels.

use std::io::Cursor;
use std::ops::Range;

use tenta_css::cascade::StyleEnv;
use tenta_css::cascade::test_util::style_html;
use tenta_render::{RenderConfig, RenderSession, ScreenshotOptions, Viewport};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const CYAN: [u8; 4] = [0, 255, 255, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];

struct Image {
    width: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let at = (y * self.width + x) as usize * 4;
        self.rgba[at..at + 4].try_into().expect("four channels")
    }

    fn count(&self, color: [u8; 4], x: Range<u32>, y: Range<u32>) -> usize {
        y.flat_map(|y| x.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| self.px(x, y) == color)
            .count()
    }
}

fn render(html: &str, width: u32, height: u32) -> Image {
    let (doc, engine) = style_html(html, StyleEnv::default());
    let cfg = RenderConfig {
        viewport: Viewport {
            width,
            height,
            device_pixel_ratio: 1.0,
        },
        ..RenderConfig::default()
    };
    let mut session = RenderSession::new(cfg);
    session.set_style_sources(engine.sources().to_vec());
    let shot = session
        .screenshot(&doc, &ScreenshotOptions::default())
        .expect("the screenshot succeeds");
    let mut reader = png::Decoder::new(Cursor::new(&shot.bytes))
        .read_info()
        .expect("valid PNG header");
    let mut rgba = vec![0; reader.output_buffer_size().expect("a frame fits in memory")];
    let info = reader.next_frame(&mut rgba).expect("valid PNG frame");
    rgba.truncate(info.buffer_size());
    Image {
        width: info.width,
        rgba,
    }
}

const PAGE: &str = "<!doctype html><style>body{margin:0}table{border-spacing:0}\
                    td{padding:0;width:50px;height:20px}";

/// One 100x40 table of 2x2 cells, each layer painting one colour.
fn layered(layers: &[&str]) -> String {
    let on = |name: &str| layers.contains(&name);
    let mut css = String::new();
    if on("table") {
        css += "table{background:#f00}";
    }
    if on("colgroup") {
        css += "colgroup{background:#0f0}";
    }
    if on("col") {
        css += "col{background:#00f}";
    }
    if on("rowgroup") {
        css += "tbody{background:#ff0}";
    }
    if on("row") {
        css += "tr{background:#0ff}";
    }
    if on("cell") {
        css += "td{background:#f0f}";
    }
    format!(
        "{PAGE}{css}</style><table><colgroup><col></colgroup>\
         <tbody><tr><td></td><td></td></tr><tr><td></td><td></td></tr></tbody></table>"
    )
}

#[test]
fn table_background_layers_paint_from_the_table_up_to_the_cell() {
    let order = ["table", "colgroup", "col", "rowgroup", "row", "cell"];
    let colours = [RED, GREEN, BLUE, YELLOW, CYAN, MAGENTA];
    for top in (0..order.len()).rev() {
        let img = render(&layered(&order[..=top]), 120, 60);
        // The first column holds the group and the column; the second column
        // only sees the layers above them.
        let first = colours[top];
        assert_eq!(
            img.px(10, 10),
            first,
            "first column with {:?}",
            &order[..=top]
        );
        let second_top = if top >= 3 { top } else { 0 };
        assert_eq!(
            img.px(75, 10),
            colours[second_top],
            "second column with {:?}",
            &order[..=top]
        );
    }
}

#[test]
fn a_row_background_covers_the_spacing_between_its_cells() {
    let html = "<!doctype html><style>body{margin:0}table{border-spacing:10px}\
                td{padding:0;width:30px;height:20px}tr{background:#f00}</style>\
                <table><tr><td></td><td></td></tr></table>";
    let img = render(html, 120, 60);
    // The gap between the cells (x 40..50) is inside the row.
    assert_eq!(img.px(45, 20), RED);
    // The table's own spacing outside the row stays white.
    assert_eq!(img.px(5, 20), WHITE);
    assert_eq!(img.px(45, 5), WHITE);
}

#[test]
fn collapsed_borders_are_shared_between_cells() {
    let html = "<!doctype html><style>body{margin:0}\
                table{border-collapse:collapse}\
                td{padding:0;width:20px;height:20px;border:4px solid #00f}</style>\
                <table><tr><td></td><td></td></tr></table>";
    let img = render(html, 80, 50);
    // Outer edges 4px, the shared line between the cells 4px as well.
    assert_eq!(img.px(0, 10), BLUE);
    assert_eq!(img.px(3, 10), BLUE);
    assert_eq!(img.px(4, 10), WHITE);
    assert_eq!(img.px(23, 10), WHITE);
    assert_eq!(img.px(24, 10), BLUE);
    assert_eq!(img.px(27, 10), BLUE);
    assert_eq!(img.px(28, 10), WHITE);
    assert_eq!(img.px(47, 10), WHITE);
    assert_eq!(img.px(48, 10), BLUE);
    assert_eq!(img.px(51, 10), BLUE);
    assert_eq!(img.px(52, 10), WHITE);
    // Top and bottom: 4px each.
    assert_eq!(img.px(10, 0), BLUE);
    assert_eq!(img.px(10, 3), BLUE);
    assert_eq!(img.px(10, 4), WHITE);
    assert_eq!(img.px(10, 27), BLUE);
    assert_eq!(img.px(10, 28), WHITE);
    // The shared line is one line, not two: 4 columns wide in total.
    assert_eq!(img.count(BLUE, 20..32, 10..11), 4);
}

#[test]
fn a_hidden_border_suppresses_the_shared_line() {
    let html = "<!doctype html><style>body{margin:0}\
                table{border-collapse:collapse}\
                td{padding:0;width:20px;height:20px;border:4px solid #00f}\
                td+td{border-left:4px hidden}</style>\
                <table><tr><td></td><td></td></tr></table>";
    let img = render(html, 80, 50);
    assert_eq!(img.count(BLUE, 20..32, 10..11), 0);
    // The outer edges stay.
    assert_eq!(img.px(0, 10), BLUE);
}

#[test]
fn a_wider_collapsed_border_wins_over_a_thinner_one() {
    let html = "<!doctype html><style>body{margin:0}\
                table{border-collapse:collapse}\
                td{padding:0;width:20px;height:20px}\
                td:first-child{border-right:2px solid #f00}\
                td+td{border-left:6px solid #00f}</style>\
                <table><tr><td></td><td></td></tr></table>";
    let img = render(html, 80, 50);
    assert_eq!(img.count(RED, 0..80, 10..11), 0);
    assert_eq!(img.count(BLUE, 0..80, 10..11), 6);
}

#[test]
fn empty_cells_hide_only_when_asked() {
    let html = |value: &str| {
        format!(
            "<!doctype html><style>body{{margin:0}}table{{border-spacing:0;empty-cells:{value}}}\
             td{{padding:0;width:40px;height:20px;background:#f00}}</style>\
             <table><tr><td></td><td>x</td></tr></table>"
        )
    };
    let shown = render(&html("show"), 100, 40);
    assert_eq!(shown.px(20, 10), RED);
    let hidden = render(&html("hide"), 100, 40);
    assert_eq!(hidden.px(20, 10), WHITE);
    // The cell with content still paints.
    assert_eq!(hidden.px(45, 2), RED);
}

#[test]
fn captions_sit_outside_the_table_background_and_border() {
    let html = "<!doctype html><style>body{margin:0}\
                table{border-spacing:0;border:4px solid #00f;background:#f00}\
                td{padding:0;width:40px;height:20px}\
                caption{height:20px;padding:0}</style>\
                <table><caption></caption><tr><td></td></tr></table>";
    let img = render(html, 100, 80);
    // The caption row is above the table's decoration.
    assert_eq!(img.px(20, 10), WHITE);
    assert_eq!(img.px(2, 10), WHITE);
    // The border starts under it.
    assert_eq!(img.px(20, 21), BLUE);
    assert_eq!(img.px(20, 30), RED);
    assert_eq!(img.px(2, 30), BLUE);
}
