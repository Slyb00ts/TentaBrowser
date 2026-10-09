//! Pixel-level checks of the render corpus: exact-colour connected components
//! (the behaviour assertions of `color-checks.json`) and pixel-distance
//! metrics against the Chromium reference.
//!
//! The component logic mirrors `render-fixtures/tools/check.py` so a verdict
//! here and there can be compared one to one: 4-connected components of
//! pixels equal to the colour (alpha ignored), components under 20 pixels
//! ignored, bounding-box geometry compared within one pixel.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use serde_json::Value;

/// A decoded image without alpha.
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    data: Vec<[u8; 3]>,
}

impl Rgb {
    pub fn decode(png_bytes: &[u8]) -> Rgb {
        let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().expect("valid PNG header");
        let mut buffer = vec![0; reader.output_buffer_size().expect("a frame fits in memory")];
        let info = reader.next_frame(&mut buffer).expect("valid PNG frame");
        let (width, height) = (info.width as usize, info.height as usize);
        let channels = match info.color_type {
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            other => panic!("unsupported PNG colour type {other:?}"),
        };
        assert_eq!(info.bit_depth, png::BitDepth::Eight, "8-bit PNG expected");
        let data = buffer[..info.buffer_size()]
            .chunks_exact(channels)
            .map(|px| [px[0], px[1], px[2]])
            .collect();
        Rgb {
            width,
            height,
            data,
        }
    }

    fn px(&self, x: usize, y: usize) -> [u8; 3] {
        self.data[y * self.width + x]
    }

    fn luminance(&self, x: usize, y: usize) -> f32 {
        let [r, g, b] = self.px(x, y);
        f32::from(r) * 0.2126 + f32::from(g) * 0.7152 + f32::from(b) * 0.0722
    }
}

/// A solid-colour component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Component {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub area: usize,
}

/// Components below this many pixels are noise (anti-aliasing slivers).
const MIN_AREA: usize = 20;

pub fn parse_color(hex: &str) -> [u8; 3] {
    let hex = hex.trim_start_matches('#');
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).expect("a hex colour");
    [channel(0), channel(2), channel(4)]
}

/// Components of every wanted colour, found in one pass over the image: the
/// pixels of each wanted colour are bucketed first, so the cost does not grow
/// with the number of checks.
pub fn components(image: &Rgb, wanted: &BTreeSet<[u8; 3]>) -> BTreeMap<[u8; 3], Vec<Component>> {
    let (width, height) = (image.width, image.height);
    let mut buckets: BTreeMap<[u8; 3], Vec<usize>> =
        wanted.iter().map(|&color| (color, Vec::new())).collect();
    let mut first_channels = [false; 256];
    for color in wanted {
        first_channels[usize::from(color[0])] = true;
    }
    for (at, px) in image.data.iter().enumerate() {
        if !first_channels[usize::from(px[0])] {
            continue;
        }
        if let Some(bucket) = buckets.get_mut(px) {
            bucket.push(at);
        }
    }
    let mut seen = vec![false; width * height];
    let mut stack = Vec::new();
    buckets
        .into_iter()
        .map(|(color, starts)| {
            let mut found = Vec::new();
            for start in starts {
                if seen[start] {
                    continue;
                }
                seen[start] = true;
                stack.push(start);
                let (mut min_x, mut max_x) = (start % width, start % width);
                let (mut min_y, mut max_y) = (start / width, start / width);
                let mut area = 0;
                while let Some(at) = stack.pop() {
                    let (x, y) = (at % width, at / width);
                    area += 1;
                    (min_x, max_x) = (min_x.min(x), max_x.max(x));
                    (min_y, max_y) = (min_y.min(y), max_y.max(y));
                    let neighbours = [
                        (x > 0).then(|| at - 1),
                        (x + 1 < width).then(|| at + 1),
                        (y > 0).then(|| at - width),
                        (y + 1 < height).then(|| at + width),
                    ];
                    for next in neighbours.into_iter().flatten() {
                        if !seen[next] && image.data[next] == color {
                            seen[next] = true;
                            stack.push(next);
                        }
                    }
                }
                if area >= MIN_AREA {
                    found.push(Component {
                        x: min_x,
                        y: min_y,
                        width: max_x - min_x + 1,
                        height: max_y - min_y + 1,
                        area,
                    });
                }
            }
            (color, found)
        })
        .collect()
}

/// The verdict of one `color-checks.json` entry.
#[derive(Clone, Debug)]
pub struct ColorResult {
    pub name: String,
    pub passed: bool,
    pub expected_count: usize,
    pub expected: Value,
    pub matches: Vec<Component>,
    /// Components of the colour that have the wrong geometry, nearest first
    /// by summed edge distance, so a failure report shows what was drawn.
    pub nearest: Vec<Component>,
}

pub fn evaluate_color_checks(image: &Rgb, checks: &[Value]) -> Vec<ColorResult> {
    let colors = checks
        .iter()
        .map(|check| parse_color(check["color"].as_str().expect("a colour")))
        .collect();
    let by_color = components(image, &colors);
    checks
        .iter()
        .map(|check| {
            let color = parse_color(check["color"].as_str().expect("a colour"));
            let expected_count = check["count"].as_u64().unwrap_or(1) as usize;
            let geometry = |key: &str| {
                check.get(key).map(|value| {
                    value.as_f64().unwrap_or_else(|| {
                        panic!("check {:?}: {key} is not a number: {value}", check["name"])
                    })
                })
            };
            let wanted = [
                geometry("x"),
                geometry("y"),
                geometry("width"),
                geometry("height"),
            ];
            let (matches, mut misses): (Vec<_>, Vec<_>) =
                by_color[&color].iter().copied().partition(|c| {
                    let actual = [c.x, c.y, c.width, c.height];
                    wanted
                        .iter()
                        .zip(actual)
                        .all(|(want, got)| want.is_none_or(|want| (got as f64 - want).abs() <= 1.0))
                });
            let distance = |c: &Component| -> f64 {
                let actual = [c.x, c.y, c.width, c.height];
                wanted
                    .iter()
                    .zip(actual)
                    .map(|(want, got)| want.map_or(0.0, |want| (got as f64 - want).abs()))
                    .sum()
            };
            misses.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
            misses.truncate(3);
            ColorResult {
                name: check["name"].as_str().expect("a check name").to_owned(),
                passed: matches.len() >= expected_count,
                expected_count,
                expected: serde_json::json!({
                    "color": check["color"], "x": check.get("x"), "y": check.get("y"),
                    "width": check.get("width"), "height": check.get("height"),
                }),
                matches,
                nearest: misses,
            }
        })
        .collect()
}

/// Distance between an engine capture and the reference, with the text
/// regions of the reference reported apart from the rest. The text figures
/// are what text contrast and positioning calibration reads.
#[derive(Clone, Debug, Default)]
pub struct PixelMetrics {
    pub size_mismatch: Option<([usize; 2], [usize; 2])>,
    pub rgb_mae: f64,
    pub pixels_gt_10: f64,
    pub pixels_gt_50: f64,
    pub text_mae: Option<f64>,
    pub non_text_mae: f64,
    pub text_pixels: usize,
    /// Darkness (255 - luminance) summed over the text regions, ours over the
    /// reference. Below 1 means our glyphs are lighter (thinner or less
    /// contrasty) than Chromium's.
    pub text_ink_ratio: Option<f64>,
    /// Mean offset of the ink centroid per text rect, ours minus reference,
    /// in pixels: positive x is to the right, positive y is lower.
    pub text_centroid_dx: Option<f64>,
    pub text_centroid_dy: Option<f64>,
    pub text_centroid_abs_dy: Option<f64>,
    pub text_rects: usize,
}

/// Padding around a reference text rect, so glyph overhang and sub-pixel
/// offsets stay inside the text region.
const TEXT_PAD: i64 = 1;

/// Search margin of the centroid comparison, wide enough to hold a text line
/// that sits a few pixels off.
const CENTROID_PAD: i64 = 4;

fn clamp_rect(rect: [f64; 4], pad: i64, image: &Rgb) -> (usize, usize, usize, usize) {
    let x0 = (rect[0].floor() as i64 - pad).clamp(0, image.width as i64) as usize;
    let y0 = (rect[1].floor() as i64 - pad).clamp(0, image.height as i64) as usize;
    let x1 = ((rect[0] + rect[2]).ceil() as i64 + pad).clamp(0, image.width as i64) as usize;
    let y1 = ((rect[1] + rect[3]).ceil() as i64 + pad).clamp(0, image.height as i64) as usize;
    (x0, y0, x1, y1)
}

fn ink_centroid(
    image: &Rgb,
    (x0, y0, x1, y1): (usize, usize, usize, usize),
) -> Option<(f64, f64, f64)> {
    let (mut sum, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let ink = f64::from((255.0 - image.luminance(x, y)).max(0.0));
            sum += ink;
            sx += ink * (x as f64 + 0.5);
            sy += ink * (y as f64 + 0.5);
        }
    }
    (sum > 0.0).then(|| (sx / sum, sy / sum, sum))
}

/// `text_rects` are `[x, y, width, height]` boxes of the reference's text.
pub fn pixel_metrics(ours: &Rgb, reference: &Rgb, text_rects: &[[f64; 4]]) -> PixelMetrics {
    if (ours.width, ours.height) != (reference.width, reference.height) {
        return PixelMetrics {
            size_mismatch: Some((
                [ours.width, ours.height],
                [reference.width, reference.height],
            )),
            ..PixelMetrics::default()
        };
    }
    let (width, height) = (ours.width, ours.height);
    let mut in_text = vec![false; width * height];
    for &rect in text_rects {
        let (x0, y0, x1, y1) = clamp_rect(rect, TEXT_PAD, ours);
        for y in y0..y1 {
            in_text[y * width..][x0..x1].fill(true);
        }
    }

    let (mut total, mut text_total, mut non_text_total) = (0u64, 0u64, 0u64);
    let (mut gt10, mut gt50, mut text_px) = (0usize, 0usize, 0usize);
    for (i, (a, b)) in ours.data.iter().zip(&reference.data).enumerate() {
        if a == b {
            text_px += usize::from(in_text[i]);
            continue;
        }
        let deltas = [0, 1, 2].map(|c| u64::from(a[c].abs_diff(b[c])));
        let sum: u64 = deltas.iter().sum();
        let max = deltas.into_iter().max().unwrap_or(0);
        total += sum;
        gt10 += usize::from(max > 10);
        gt50 += usize::from(max > 50);
        if in_text[i] {
            text_total += sum;
            text_px += 1;
        } else {
            non_text_total += sum;
        }
    }
    let pixels = (width * height) as f64;
    let mae = |sum: u64, count: usize| sum as f64 / (count as f64 * 3.0 * 255.0);

    let (mut ours_ink, mut ref_ink) = (0.0, 0.0);
    let (mut dx, mut dy, mut abs_dy, mut measured) = (0.0, 0.0, 0.0, 0usize);
    for &rect in text_rects {
        let region = clamp_rect(rect, CENTROID_PAD, ours);
        let (Some((ox, oy, o_sum)), Some((rx, ry, r_sum))) =
            (ink_centroid(ours, region), ink_centroid(reference, region))
        else {
            continue;
        };
        ours_ink += o_sum;
        ref_ink += r_sum;
        dx += ox - rx;
        dy += oy - ry;
        abs_dy += (oy - ry).abs();
        measured += 1;
    }
    let mean = |sum: f64| (measured > 0).then(|| sum / measured as f64);
    PixelMetrics {
        size_mismatch: None,
        rgb_mae: total as f64 / (pixels * 3.0 * 255.0),
        pixels_gt_10: gt10 as f64 / pixels,
        pixels_gt_50: gt50 as f64 / pixels,
        text_mae: (text_px > 0).then(|| mae(text_total, text_px)),
        non_text_mae: mae(non_text_total, width * height - text_px),
        text_pixels: text_px,
        text_ink_ratio: (ref_ink > 0.0).then(|| ours_ink / ref_ink),
        text_centroid_dx: mean(dx),
        text_centroid_dy: mean(dy),
        text_centroid_abs_dy: mean(abs_dy),
        text_rects: measured,
    }
}
