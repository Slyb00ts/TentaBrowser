//! SVG documents drawn as images (`<img src=x.svg>`, `background-image`).
//!
//! An SVG used as an image is rendered to a bitmap at the size the page will
//! show it, never as a live document: scripts do not run, and the parser is
//! given no way to read a file or reach the network. `<image>` elements inside
//! the document resolve only `data:` URLs, and the font database is empty, so
//! text in an SVG image is not drawn (a known gap).

use std::sync::atomic::{AtomicUsize, Ordering};

use resvg::tiny_skia;
use resvg::usvg;

use super::DecodedImage;
use super::ctx::DecodeControl;
use super::error::DecodeError;
use super::format::ImageFormat;
use super::limits::DecodeLimits;

/// Longest side of a rendered SVG bitmap.
const MAX_SIDE: u32 = 8192;

/// Largest SVG document parsed, in bytes.
const MAX_INPUT_BYTES: usize = 2 << 20;

/// SVG renders in flight at once, process-wide. resvg cannot be interrupted, so
/// the number of threads a hostile document can pin is bounded instead; the
/// next render fails fast ("svg busy") rather than queueing.
const MAX_CONCURRENT_RENDERS: usize = 3;

static RENDERS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Most nodes a tree may hold, nested SVG images included.
const MAX_NODES: usize = 20_000;

const MAX_BLUR_DEVIATION: f32 = 100.0;
const MAX_MORPHOLOGY_RADIUS: f32 = 50.0;

/// Holds one of the render slots until dropped.
struct RenderSlot;

impl RenderSlot {
    fn take() -> Option<RenderSlot> {
        RENDERS_IN_FLIGHT
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_CONCURRENT_RENDERS).then_some(n + 1)
            })
            .ok()
            .map(|_| RenderSlot)
    }
}

impl Drop for RenderSlot {
    fn drop(&mut self) {
        RENDERS_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

/// What an SVG says about its own size, as the replaced-element sizing rules
/// read it: the `width` and `height` attributes when they are absolute lengths,
/// and the aspect ratio from them or from the `viewBox`.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct SvgNatural {
    /// Natural width in CSS pixels; `None` for a missing or relative `width`.
    pub width: Option<f32>,
    /// Natural height in CSS pixels; `None` for a missing or relative `height`.
    pub height: Option<f32>,
    /// Width over height.
    pub ratio: Option<f32>,
}

/// Renders `bytes` at `scale` bitmap pixels per CSS pixel of its natural size.
///
/// The bitmap is at most [`MAX_SIDE`] on a side and fits
/// `limits.max_decoded_bytes`: a larger request is reduced, not refused. An
/// SVG with neither a size nor a view box has no size to scale and is refused
/// as malformed.
pub fn decode_svg(
    bytes: &[u8],
    scale: f32,
    limits: &DecodeLimits,
    control: &DecodeControl,
) -> Result<(DecodedImage, SvgNatural), DecodeError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        render(bytes, scale, limits, control)
    }))
    .map_err(|_| DecodeError::Panicked(ImageFormat::Svg))?
}

fn render(
    bytes: &[u8],
    scale: f32,
    limits: &DecodeLimits,
    control: &DecodeControl,
) -> Result<(DecodedImage, SvgNatural), DecodeError> {
    control.check()?;
    let malformed =
        |detail: &dyn std::fmt::Display| DecodeError::malformed(ImageFormat::Svg, detail);
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(DecodeError::TooLarge {
            width: 0,
            height: 0,
        });
    }
    // usvg expands a pattern into every pattern that fills it while parsing, so
    // a chain of patterns costs exponential time before any tree exists to
    // inspect; the number of `<pattern>` elements bounds the chain.
    if count_occurrences(bytes, b"<pattern") > MAX_PATTERNS {
        return Err(DecodeError::Unsupported {
            format: ImageFormat::Svg,
            feature: "the SVG has too many patterns",
        });
    }
    if patterns_nest(bytes) {
        return Err(DecodeError::Unsupported {
            format: ImageFormat::Svg,
            feature: "SVG patterns that use other patterns are not rendered",
        });
    }
    let Some(_slot) = RenderSlot::take() else {
        return Err(DecodeError::Unsupported {
            format: ImageFormat::Svg,
            feature: "svg busy: too many SVG renders in progress",
        });
    };
    let tree = usvg::Tree::from_data(bytes, &safe_options()).map_err(|e| malformed(&e))?;
    control.check()?;

    let tree_size = tree.size();
    let (tree_w, tree_h) = (tree_size.width(), tree_size.height());
    if !(tree_w > 0.0 && tree_h > 0.0) {
        return Err(malformed(&"the document has no size"));
    }
    let natural = natural_of(bytes, (tree_w, tree_h));
    // Rendered size: the natural one, else the one the view box gives.
    let (css_w, css_h) = match (natural.width, natural.height) {
        (Some(w), Some(h)) => (w, h),
        _ => (tree_w, tree_h),
    };
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let (mut w, mut h) = (
        (css_w * scale).ceil().max(1.0),
        (css_h * scale).ceil().max(1.0),
    );
    let max_pixels = (limits.max_decoded_bytes / 4) as f32;
    let shrink = (MAX_SIDE as f32 / w)
        .min(MAX_SIDE as f32 / h)
        .min((max_pixels / (w * h)).sqrt())
        .min(1.0);
    if shrink < 1.0 {
        w = (w * shrink).floor().max(1.0);
        h = (h * shrink).floor().max(1.0);
    }
    let (width, height) = (w as u32, h as u32);
    let scale_x = width as f32 / tree_w;
    let scale_y = height as f32 / tree_h;
    let mut budget = Walk {
        sx: scale_x,
        sy: scale_y,
        canvas: f64::from(width) * f64::from(height),
        nodes: 0,
        work: 0.0,
    };
    if let Err(reason) = walk(tree.root(), &mut budget, 0) {
        return Err(DecodeError::Unsupported {
            format: ImageFormat::Svg,
            feature: reason,
        });
    }
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or(DecodeError::TooLarge { width, height })?;
    let transform = tiny_skia::Transform::from_scale(width as f32 / tree_w, height as f32 / tree_h);
    // One top-level child at a time, so the deadline and the cancel flag are
    // polled between them (resvg cannot be interrupted inside one).
    for node in tree.root().children() {
        control.check()?;
        // `render_node` draws a node at the pixmap's origin; put it back where
        // the document has it.
        let Some(bbox) = node.abs_layer_bounding_box() else {
            continue;
        };
        let at = transform.pre_translate(bbox.x(), bbox.y());
        resvg::render_node(node, at, &mut pixmap.as_mut());
    }
    control.check()?;
    let image = DecodedImage {
        width,
        height,
        rgba: pixmap.take().into(),
        source_width: width,
        source_height: height,
    };
    Ok((image, natural))
}

/// What one document may cost to render, summed over every element, in device
/// pixels of work: filter regions, layers (a canvas each), pattern tiles and
/// the area they fill, and dashed strokes. A hostile tree spreads its cost over
/// many cheap-looking elements, so the caps that matter are cumulative.
const MAX_TOTAL_WORK: f64 = 256.0e6;

/// Deepest `<pattern>` nesting; a pattern's tile is rendered once per use, so
/// depth multiplies the work.
const MAX_PATTERN_DEPTH: usize = 3;

/// Most `<pattern>` elements a document may declare.
const MAX_PATTERNS: usize = 256;

/// Whether any `<pattern>` fills or strokes with another pattern: its content
/// holds `url(#id)` for the id of a pattern. Found on the text, since usvg
/// expands the chain while parsing, before there is a tree to inspect.
fn patterns_nest(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let mut ids = Vec::new();
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("<pattern") {
        let start = from + at;
        let end = text[start..]
            .find("</pattern")
            .map_or(text.len(), |e| start + e);
        let tag_end = text[start..].find('>').map_or(end, |t| start + t);
        let tag = &text[start..tag_end.min(end)];
        if let Some(i) = tag.find("id=") {
            let rest = &tag[i + 3..];
            if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'')
                && let Some(close) = rest[1..].find(quote)
            {
                ids.push(rest[1..1 + close].to_owned());
            }
        }
        spans.push((tag_end.min(end), end));
        from = start + "<pattern".len();
    }
    spans.iter().any(|&(a, b)| {
        let content = &text[a..b];
        ids.iter().any(|id| {
            content.contains(&format!("url(#{id})"))
                || content.contains(&format!("url('#{id}')"))
                || content.contains(&format!("url(\"#{id}\")"))
        })
    })
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

/// Largest convolution matrix side and most turbulence octaves accepted.
const MAX_CONVOLVE_ORDER: u32 = 9;
const MAX_OCTAVES: u32 = 8;

/// Longest `stroke-dasharray` accepted.
const MAX_DASHES: usize = 1000;

/// The running total of one walk over a tree.
struct Walk {
    sx: f32,
    sy: f32,
    /// Pixels of the canvas, which bound a layer.
    canvas: f64,
    nodes: usize,
    work: f64,
}

impl Walk {
    fn charge(&mut self, work: f64) -> Result<(), &'static str> {
        self.work += work;
        if self.work.is_nan() || self.work > MAX_TOTAL_WORK {
            return Err("the SVG is too expensive to render");
        }
        Ok(())
    }

    fn area(&self, w: f32, h: f32) -> f64 {
        f64::from(w * self.sx) * f64::from(h * self.sy)
    }
}

/// Walks `group` (and nested images and patterns) charging the document's
/// budget; `Err` says why it cannot be bounded.
fn walk(group: &usvg::Group, w: &mut Walk, depth: usize) -> Result<(), &'static str> {
    use usvg::filter::Kind;
    for filter in group.filters() {
        let region = filter.rect();
        let area = w.area(region.width(), region.height());
        // Each primitive costs its region times a weight for how much work a
        // pixel takes; the region charge above counted one unit each.
        for primitive in filter.primitives() {
            let (sx, sy) = (w.sx, w.sy);
            let weight = match primitive.kind() {
                Kind::GaussianBlur(b) => {
                    let dev = (b.std_dev_x().get() * sx).max(b.std_dev_y().get() * sy);
                    if dev.is_nan() || dev > MAX_BLUR_DEVIATION {
                        return Err("an SVG filter radius is too large to render");
                    }
                    1.0 + f64::from(dev) / 2.0
                }
                Kind::DropShadow(b) => {
                    let dev = (b.std_dev_x().get() * sx).max(b.std_dev_y().get() * sy);
                    if dev.is_nan() || dev > MAX_BLUR_DEVIATION {
                        return Err("an SVG filter radius is too large to render");
                    }
                    1.0 + f64::from(dev) / 2.0
                }
                Kind::Morphology(m) => {
                    let r = (m.radius_x().get() * sx).max(m.radius_y().get() * sy);
                    if r.is_nan() || r > MAX_MORPHOLOGY_RADIUS {
                        return Err("an SVG filter radius is too large to render");
                    }
                    1.0 + f64::from(r) * f64::from(r) / 4.0
                }
                Kind::ConvolveMatrix(c) => {
                    let (cols, rows) = (c.matrix().columns(), c.matrix().rows());
                    if cols > MAX_CONVOLVE_ORDER || rows > MAX_CONVOLVE_ORDER {
                        return Err("an SVG convolution matrix is too large");
                    }
                    f64::from(cols) * f64::from(rows)
                }
                Kind::Turbulence(t) => {
                    if t.num_octaves() > MAX_OCTAVES {
                        return Err("an SVG turbulence has too many octaves");
                    }
                    f64::from(t.num_octaves().max(1)) * 4.0
                }
                Kind::DiffuseLighting(_) | Kind::SpecularLighting(_) => 10.0,
                Kind::DisplacementMap(_) => 3.0,
                Kind::Offset(_)
                | Kind::Flood(_)
                | Kind::Merge(_)
                | Kind::Blend(_)
                | Kind::Composite(_)
                | Kind::ColorMatrix(_)
                | Kind::ComponentTransfer(_)
                | Kind::Tile(_)
                | Kind::Image(_) => 1.0,
            };
            w.charge(area * weight)?;
        }
    }
    // A group with opacity, a mask, a clip path or a filter draws into a canvas
    // of its own.
    if group.opacity().get() < 1.0
        || group.mask().is_some()
        || group.clip_path().is_some()
        || group.should_isolate()
    {
        let layer = group.abs_layer_bounding_box();
        let area = w.area(layer.width(), layer.height());
        w.charge(area.min(w.canvas))?;
    }
    for node in group.children() {
        w.nodes += 1;
        if w.nodes > MAX_NODES {
            return Err("the SVG has too many elements");
        }
        match node {
            usvg::Node::Group(g) => walk(g, w, depth)?,
            usvg::Node::Image(image) => {
                if let usvg::ImageKind::SVG(tree) = image.kind() {
                    walk(tree.root(), w, depth)?;
                }
            }
            usvg::Node::Path(path) => walk_path(path, w, depth)?,
            usvg::Node::Text(_) => {}
        }
    }
    Ok(())
}

fn walk_path(path: &usvg::Path, w: &mut Walk, depth: usize) -> Result<(), &'static str> {
    let bounds = path.abs_bounding_box();
    let covered = w.area(bounds.width(), bounds.height()).min(w.canvas);
    let paints = [
        path.fill().map(usvg::Fill::paint),
        path.stroke().map(usvg::Stroke::paint),
    ];
    for paint in paints.into_iter().flatten() {
        if let usvg::Paint::Pattern(pattern) = paint {
            if depth >= MAX_PATTERN_DEPTH {
                return Err("SVG patterns are nested too deeply");
            }
            let tile = pattern.rect();
            // The tile is rendered once per use, then repeated over the area.
            w.charge(w.area(tile.width(), tile.height()).min(w.canvas) + covered)?;
            walk(pattern.root(), w, depth + 1)?;
        }
    }
    if let Some(stroke) = path.stroke()
        && let Some(dashes) = stroke.dasharray()
    {
        if dashes.len() > MAX_DASHES {
            return Err("an SVG stroke has too many dashes");
        }
        // Every segment is cut by every dash.
        w.charge(path.data().len() as f64 * dashes.len() as f64 * 16.0)?;
    }
    Ok(())
}

/// Options that read nothing from the machine: no resource directory, no
/// fonts, and `<image>` hrefs resolve only when they are `data:` URLs.
fn safe_options() -> usvg::Options<'static> {
    usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..usvg::ImageHrefResolver::default()
        },
        ..usvg::Options::default()
    }
}

/// The natural size and ratio from the root element's attributes, falling back
/// to the size usvg computed for the ratio.
fn natural_of(bytes: &[u8], tree_size: (f32, f32)) -> SvgNatural {
    let attrs = root_attributes(bytes);
    let length = |name: &str| {
        attrs
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| parse_length(v))
    };
    let (width, height) = (length("width"), length("height"));
    let view_ratio = attrs
        .iter()
        .find(|(n, _)| n == "viewBox")
        .and_then(|(_, v)| {
            let nums: Vec<f32> = v
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            (nums.len() == 4 && nums[2] > 0.0 && nums[3] > 0.0).then(|| nums[2] / nums[3])
        });
    let ratio = match (width, height) {
        (Some(w), Some(h)) if h > 0.0 => Some(w / h),
        _ => view_ratio,
    }
    .or_else(|| (tree_size.1 > 0.0).then(|| tree_size.0 / tree_size.1));
    let (width, height) = match (width, height, ratio) {
        (Some(w), None, Some(r)) if view_ratio.is_some() => (Some(w), Some(w / r)),
        (None, Some(h), Some(r)) if view_ratio.is_some() => (Some(h * r), Some(h)),
        other => (other.0, other.1),
    };
    SvgNatural {
        width,
        height,
        ratio,
    }
}

/// An absolute SVG length in CSS pixels; percentages and `auto` are `None`.
fn parse_length(value: &str) -> Option<f32> {
    let value = value.trim();
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(end);
    let number: f32 = number.parse().ok()?;
    let factor = match unit.trim() {
        "" | "px" => 1.0,
        "pt" => 4.0 / 3.0,
        "pc" => 16.0,
        "mm" => 96.0 / 25.4,
        "cm" => 96.0 / 2.54,
        "in" => 96.0,
        "em" => 16.0,
        "ex" => 8.0,
        _ => return None,
    };
    let px = number * factor;
    (px.is_finite() && px > 0.0).then_some(px)
}

/// The attributes of the first `<svg` start tag, as `(name, value)` pairs.
fn root_attributes(bytes: &[u8]) -> Vec<(String, String)> {
    let head = &bytes[..bytes.len().min(16 * 1024)];
    let text = String::from_utf8_lossy(head);
    let mut search = 0;
    let start = loop {
        let Some(at) = text[search..].find("<svg") else {
            return Vec::new();
        };
        let at = search + at;
        match text[at + 4..].chars().next() {
            Some(c) if c.is_whitespace() || c == '>' || c == '/' => break at + 4,
            _ => search = at + 4,
        }
    };
    let mut attrs = Vec::new();
    let mut rest = &text[start..];
    loop {
        rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with('>') || rest.starts_with("/>") {
            break;
        }
        let name_end = rest
            .find(|c: char| c == '=' || c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(rest.len());
        let name = &rest[..name_end];
        rest = rest[name_end..].trim_start();
        if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let Some(close) = after[1..].find(quote) else {
                break;
            };
            attrs.push((name.to_owned(), after[1..1 + close].to_owned()));
            rest = &after[close + 2..];
        } else if name.is_empty() {
            break;
        }
    }
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20"><rect width="40" height="20" fill="#ff0000"/></svg>"##;

    fn decode(svg: &str, scale: f32) -> Result<(DecodedImage, SvgNatural), DecodeError> {
        decode_svg(
            svg.as_bytes(),
            scale,
            &DecodeLimits::default(),
            &DecodeControl::new(),
        )
    }

    #[test]
    fn a_sized_svg_renders_at_its_size_times_the_scale() {
        let (image, natural) = decode(SVG, 2.0).expect("renders");
        assert_eq!((image.width, image.height), (80, 40));
        assert_eq!(natural.width, Some(40.0));
        assert_eq!(natural.height, Some(20.0));
        assert_eq!(natural.ratio, Some(2.0));
        assert_eq!(&image.rgba[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn a_view_box_alone_gives_a_ratio_and_no_size() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 10"><rect width="30" height="10"/></svg>"#;
        let (image, natural) = decode(svg, 1.0).expect("renders");
        assert_eq!(natural.width, None);
        assert_eq!(natural.height, None);
        assert_eq!(natural.ratio, Some(3.0));
        assert_eq!((image.width, image.height), (30, 10));
    }

    #[test]
    fn one_side_and_a_view_box_complete_the_other() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="60" viewBox="0 0 30 10"/>"#;
        let (_, natural) = decode(svg, 1.0).expect("renders");
        assert_eq!(natural.width, Some(60.0));
        assert_eq!(natural.height, Some(20.0));
    }

    #[test]
    fn external_images_inside_an_svg_are_not_read() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image xlink:href="/etc/hosts" width="4" height="4"/></svg>"#;
        let (image, _) = decode(svg, 1.0).expect("renders without the image");
        assert!(image.rgba.iter().all(|&b| b == 0));
    }

    /// Runs `decode` on a thread and fails the test when it takes longer than
    /// `limit`: a hostile document must be refused, not rendered.
    fn refused_quickly(svg: String) -> DecodeError {
        let started = std::time::Instant::now();
        let handle = std::thread::spawn(move || decode(&svg, 1.0));
        let result = handle.join().expect("no panic");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        result.expect_err("refused")
    }

    #[test]
    fn a_huge_blur_and_morphology_region_is_refused_not_rendered() {
        let err = refused_quickly(
            "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>\
             <filter id='f' x='-100000' y='-100000' width='200000' height='200000'>\
             <feGaussianBlur stdDeviation='5000'/><feMorphology radius='500'/></filter>\
             <rect width='50' height='50' filter='url(#f)'/></svg>"
                .into(),
        );
        assert!(matches!(err, DecodeError::Unsupported { .. }), "{err:?}");
    }

    #[test]
    fn a_large_deviation_on_a_small_region_is_refused_too() {
        let err = refused_quickly(
            "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>\
             <filter id='f'><feGaussianBlur stdDeviation='900'/></filter>\
             <rect width='50' height='50' filter='url(#f)'/></svg>"
                .into(),
        );
        assert!(matches!(err, DecodeError::Unsupported { .. }), "{err:?}");
    }

    #[test]
    fn many_cheap_looking_filtered_elements_exhaust_the_cumulative_budget() {
        let mut svg = String::from(
            "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>\
             <filter id='f' x='0' y='0' width='4000' height='4000'>\
             <feGaussianBlur stdDeviation='5'/><feMorphology radius='5'/></filter>",
        );
        for _ in 0..3000 {
            svg.push_str("<rect width='1' height='1' filter='url(#f)'/>");
        }
        svg.push_str("</svg>");
        let err = refused_quickly(svg);
        assert!(matches!(err, DecodeError::Unsupported { .. }), "{err:?}");
    }

    #[test]
    fn deeply_nested_patterns_are_refused_not_expanded() {
        let mut svg = String::from(
            "<svg xmlns='http://www.w3.org/2000/svg' width='2000' height='2000'><defs>\
             <pattern id='p0' width='1' height='1' patternUnits='userSpaceOnUse'><rect width='1' height='1'/></pattern>",
        );
        for i in 1..30 {
            let below = i - 1;
            svg.push_str(&format!(
                "<pattern id='p{i}' width='2' height='2' patternUnits='userSpaceOnUse'>\
                 <rect width='2' height='2' fill='url(#p{below})'/>\
                 <rect width='1' height='1' fill='url(#p{below})'/></pattern>"
            ));
        }
        svg.push_str("</defs><rect width='2000' height='2000' fill='url(#p29)'/></svg>");
        let err = refused_quickly(svg);
        assert!(matches!(err, DecodeError::Unsupported { .. }), "{err:?}");
    }

    #[test]
    fn a_huge_convolution_and_turbulence_are_refused_fast() {
        for primitive in [
            "<feConvolveMatrix order='50' kernelMatrix='1'/>",
            "<feTurbulence baseFrequency='0.05' numOctaves='10'/>",
        ] {
            let err = refused_quickly(format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>\
                 <filter id='f' x='0' y='0' width='4000' height='4000'>{primitive}</filter>\
                 <rect width='50' height='50' filter='url(#f)'/></svg>"
            ));
            assert!(
                matches!(err, DecodeError::Unsupported { .. }),
                "{primitive}: {err:?}"
            );
        }
    }

    #[test]
    fn expensive_primitives_cost_more_than_simple_ones_over_the_same_region() {
        // The same 4000x4000 region: a flood passes, lighting at weight 10 over
        // many elements does not.
        let one = |primitive: &str, copies: usize| {
            let mut svg = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>\
                 <filter id='f' x='0' y='0' width='3000' height='3000'>{primitive}</filter>"
            );
            for _ in 0..copies {
                svg.push_str("<rect width='1' height='1' filter='url(#f)'/>");
            }
            svg.push_str("</svg>");
            decode(&svg, 1.0)
        };
        assert!(one("<feFlood flood-color='red'/>", 20).is_ok());
        assert!(
            one(
                "<feDisplacementMap in='SourceGraphic' in2='SourceGraphic' scale='5'/>",
                20
            )
            .is_err()
        );
    }

    #[test]
    fn flat_patterns_render_and_nested_ones_are_refused() {
        let mut flat =
            String::from("<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20'><defs>");
        for i in 0..9 {
            flat.push_str(&format!(
                "<pattern id='p{i}' width='2' height='2' patternUnits='userSpaceOnUse'><rect width='1' height='1' fill='#f00'/></pattern>"
            ));
        }
        flat.push_str("</defs><rect width='20' height='20' fill='url(#p3)'/></svg>");
        assert!(decode(&flat, 1.0).is_ok(), "nine flat patterns");
        let nested = "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20'><defs>\
            <pattern id='a' width='2' height='2' patternUnits='userSpaceOnUse'><rect width='1' height='1'/></pattern>\
            <pattern id='b' width='4' height='4' patternUnits='userSpaceOnUse'><rect width='4' height='4' fill='url(#a)'/></pattern>\
            </defs><rect width='20' height='20' fill='url(#b)'/></svg>";
        let err = refused_quickly(nested.into());
        assert!(matches!(err, DecodeError::Unsupported { .. }), "{err:?}");
    }

    #[test]
    fn a_stroke_with_thousands_of_dashes_is_refused() {
        let dashes = vec!["1"; 2000].join(" ");
        let err = refused_quickly(format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='50' height='50'>\
             <path d='M0 0 L50 50' stroke='black' stroke-dasharray='{dashes}'/></svg>"
        ));
        assert!(matches!(err, DecodeError::Unsupported { .. }), "{err:?}");
    }

    #[test]
    fn deep_and_wide_documents_are_refused_by_node_count() {
        let mut svg =
            String::from("<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>");
        for _ in 0..(MAX_NODES + 10) {
            svg.push_str("<rect width='1' height='1'/>");
        }
        svg.push_str("</svg>");
        let err = refused_quickly(svg);
        assert!(matches!(err, DecodeError::Unsupported { .. }), "{err:?}");
    }

    #[test]
    fn input_over_two_mebibytes_is_refused_before_parsing() {
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'><!--{}--></svg>",
            "x".repeat(MAX_INPUT_BYTES)
        );
        let err = refused_quickly(svg);
        assert!(matches!(err, DecodeError::TooLarge { .. }), "{err:?}");
    }

    #[test]
    fn render_slots_are_bounded_and_released() {
        let held: Vec<_> = (0..MAX_CONCURRENT_RENDERS)
            .map(|_| RenderSlot::take().expect("a slot"))
            .collect();
        assert!(RenderSlot::take().is_none());
        let busy = decode(SVG, 1.0).expect_err("busy");
        assert!(
            matches!(busy, DecodeError::Unsupported { feature, .. } if feature.contains("busy"))
        );
        drop(held);
        assert!(decode(SVG, 1.0).is_ok());
    }

    #[test]
    fn garbage_is_malformed_not_a_panic() {
        assert!(matches!(
            decode("not an svg", 1.0),
            Err(DecodeError::Malformed { .. })
        ));
    }

    #[test]
    fn huge_requests_are_reduced_to_the_limits() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100000"/>"#;
        let (image, natural) = decode(svg, 1.0).expect("reduced, not refused");
        assert!(image.width <= MAX_SIDE && image.height <= MAX_SIDE);
        assert_eq!(natural.width, Some(100_000.0));
    }
}
