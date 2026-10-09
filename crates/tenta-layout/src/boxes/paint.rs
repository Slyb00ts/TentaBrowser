//! Resolution of Stylo computed values into the Stylo-free [`PaintStyle`].

use std::sync::Arc;

use style::color::AbsoluteColor;
use style::properties::ComputedValues;
use style::values::computed::length_percentage::LengthPercentage;
use style::values::computed::{Color as ComputedColor, Length};
use style::values::generics::image::{GenericGradientItem, GenericImage};
use style::values::generics::length::GenericLengthPercentageOrAuto as LengthPercentageOrAuto;
use style::values::specified::box_::Overflow;
use style::values::specified::text::{TextDecorationLine, TextUnderlinePosition};

use crate::box_tree::BoxFlags;
use crate::types::{
    Affine, BackgroundContent, BackgroundLayer, BackgroundSize, BlendMode, BorderPaint,
    BorderStyle, BoxArea, Color, ColorStop, DecorationPaint, Edges, FilterOp, GradientPaint,
    ImageKey, ImageRendering, LengthPercent, LineStyle, MarkerKind, MarkerPaint, ObjectFit,
    OutlinePaint, OverflowKind, PaintStyle, PositionKind, Rect, RepeatKind, ShadowPaint,
    UnderlinePosition,
};

/// What resolving a style could not express or could not finish yet.
#[derive(Default)]
struct Report {
    /// A feature was left out of the `PaintStyle`.
    skipped: bool,
    /// A value needs the used box size (see `BoxFlags::PAINT_SIZE_DEPENDENT`).
    size_dependent: bool,
}

impl PaintStyle {
    /// Resolves everything paint needs from `style`. `ancestor_decoration` is
    /// the text decoration propagated from the nearest decorating ancestor:
    /// decorations are not inherited in CSS, yet they are drawn under the text
    /// of descendants, so the result already merges them.
    ///
    /// Values that depend on the box size are resolved against a zero-sized
    /// box here; see [`crate::BoxTree::paint_style_for`] for the per-fragment
    /// answer.
    pub fn from_computed(style: &ComputedValues, ancestor_decoration: &DecorationPaint) -> Self {
        paint_style(style, ancestor_decoration).0
    }
}

/// The paint style of a box plus the box flags describing how complete it is.
pub(super) fn paint_style(
    style: &ComputedValues,
    ancestor_decoration: &DecorationPaint,
) -> (PaintStyle, BoxFlags) {
    let mut report = Report::default();
    let paint = build_paint(style, ancestor_decoration, (0.0, 0.0), &mut report);
    let mut flags = BoxFlags::empty();
    flags.set(BoxFlags::PAINT_SKIPPED, report.skipped);
    flags.set(BoxFlags::PAINT_SIZE_DEPENDENT, report.size_dependent);
    (paint, flags)
}

/// Re-resolves the size-dependent parts of `paint` for a border box of the
/// given size.
pub(super) fn resolve_for_size(
    paint: &PaintStyle,
    style: &ComputedValues,
    border_box: Rect,
) -> PaintStyle {
    let mut report = Report::default();
    let current = style.get_inherited_text().color;
    let mut resolved = paint.clone();
    resolved.background =
        background_layers(style, &current, (border_box.w, border_box.h), &mut report);
    resolved.transform = resolve_transform(style, border_box);
    resolved
}

/// The matrix of `transform` and the individual `translate`, `rotate` and
/// `scale` properties for a border box of the given size, in the order CSS
/// composes them (translate, rotate, scale, transform). Origin handling is
/// left to paint (`PaintStyle::transform_origin`). Only the 2D part of 3D
/// functions is kept; `None` is the identity.
pub(crate) fn resolve_transform(style: &ComputedValues, border_box: Rect) -> Option<Affine> {
    transform_affine(style, border_box.w, border_box.h, &mut Report::default())
}

fn build_paint(
    style: &ComputedValues,
    ancestor_decoration: &DecorationPaint,
    size: (f32, f32),
    report: &mut Report,
) -> PaintStyle {
    let current = style.get_inherited_text().color;
    let font_box = style.get_box();
    let effects = style.get_effects();
    let ui = style.get_inherited_ui();
    let position = style.get_box().clone_position();

    PaintStyle {
        visibility_hidden: !matches!(
            style.get_inherited_box().visibility,
            style::properties::longhands::visibility::computed_value::T::Visible
        ),
        opacity: effects.opacity,
        pointer_events: !matches!(
            ui.pointer_events,
            style::properties::longhands::pointer_events::computed_value::T::None
        ),
        background: background_layers(style, &current, size, report),
        border: border_paint(style, &current),
        outline: outline_paint(style, &current),
        box_shadow: effects
            .box_shadow
            .0
            .iter()
            .map(|s| ShadowPaint {
                offset: (s.base.horizontal.px(), s.base.vertical.px()),
                blur: s.base.blur.px(),
                spread: s.spread.px(),
                color: resolve_color(&s.base.color, &current),
                inset: s.inset,
            })
            .collect(),
        text_shadow: style
            .get_inherited_text()
            .text_shadow
            .0
            .iter()
            .map(|s| simple_shadow(s, &current))
            .collect(),
        color: color_of(&current),
        text_decoration: decoration(style, &current, ancestor_decoration),
        overflow_x: overflow(font_box.overflow_x),
        overflow_y: overflow(font_box.overflow_y),
        position: position_kind(position),
        z_index: z_index(style),
        transform: transform_affine(style, size.0, size.1, report),
        transform_origin: (
            length_percent(&font_box.transform_origin.horizontal),
            length_percent(&font_box.transform_origin.vertical),
        ),
        filter: effects
            .filter
            .0
            .iter()
            .filter_map(|f| filter_op(f, &current, report))
            .collect(),
        blend: blend_mode(effects.mix_blend_mode),
        isolate: matches!(
            font_box.isolation,
            style::computed_values::isolation::T::Isolate
        ),
        list_marker: list_marker(style),
        object_fit: object_fit(style),
        object_position: {
            let p = &style.get_position().object_position;
            (length_percent(&p.horizontal), length_percent(&p.vertical))
        },
        image_rendering: match style.get_inherited_box().image_rendering {
            style::values::computed::ImageRendering::Auto => ImageRendering::Auto,
            _ => ImageRendering::Pixelated,
        },
    }
}

/// An absolute Stylo colour as a premultiplied paint colour.
fn color_of(c: &AbsoluteColor) -> Color {
    let srgb = c.into_srgb_legacy();
    let alpha = srgb.alpha.clamp(0.0, 1.0);
    let channel = |v: f32| {
        if v.is_nan() {
            0.0
        } else {
            v.clamp(0.0, 1.0) * alpha
        }
    };
    Color {
        r: channel(srgb.components.0),
        g: channel(srgb.components.1),
        b: channel(srgb.components.2),
        a: alpha,
    }
}

fn resolve_color(c: &ComputedColor, current: &AbsoluteColor) -> Color {
    color_of(&c.resolve_to_absolute(current))
}

/// A computed `<length-percentage>` as `px + fraction * basis`. Calc values
/// are sampled at two bases, which is exact for the linear ones.
fn length_percent(lp: &LengthPercentage) -> LengthPercent {
    if let Some(len) = lp.to_length() {
        return LengthPercent::px(len.px());
    }
    if let Some(pct) = lp.to_percentage() {
        return LengthPercent::fraction(pct.0);
    }
    const PROBE: f32 = 1024.0;
    let px = lp.resolve(Length::new(0.0)).px();
    let at_probe = lp.resolve(Length::new(PROBE)).px();
    LengthPercent {
        px,
        fraction: (at_probe - px) / PROBE,
    }
}

fn simple_shadow(
    s: &style::values::computed::effects::SimpleShadow,
    current: &AbsoluteColor,
) -> ShadowPaint {
    ShadowPaint {
        offset: (s.horizontal.px(), s.vertical.px()),
        blur: s.blur.px(),
        spread: 0.0,
        color: resolve_color(&s.color, current),
        inset: false,
    }
}

fn box_area_origin(
    o: style::properties::longhands::background_origin::single_value::computed_value::T,
) -> BoxArea {
    use style::properties::longhands::background_origin::single_value::computed_value::T as O;
    match o {
        O::BorderBox => BoxArea::BorderBox,
        O::PaddingBox => BoxArea::PaddingBox,
        O::ContentBox => BoxArea::ContentBox,
    }
}

/// `None` for `background-clip: text` and other clips the rasteriser has no
/// shape for: painting such a layer unclipped would cover the whole box.
fn box_area_clip(c: style::values::specified::background::BackgroundClip) -> Option<BoxArea> {
    use style::values::specified::background::BackgroundClip as C;
    match c {
        C::BorderBox => Some(BoxArea::BorderBox),
        C::PaddingBox => Some(BoxArea::PaddingBox),
        C::ContentBox => Some(BoxArea::ContentBox),
        _ => None,
    }
}

fn repeat_kind(k: style::values::specified::background::BackgroundRepeatKeyword) -> RepeatKind {
    use style::values::specified::background::BackgroundRepeatKeyword as K;
    match k {
        K::Repeat => RepeatKind::Repeat,
        K::Space => RepeatKind::Space,
        K::Round => RepeatKind::Round,
        K::NoRepeat => RepeatKind::NoRepeat,
    }
}

fn background_layers(
    style: &ComputedValues,
    current: &AbsoluteColor,
    size: (f32, f32),
    report: &mut Report,
) -> Vec<BackgroundLayer> {
    let bg = style.get_background();
    let count = bg.background_image.0.len();
    let mut layers = Vec::new();
    let mut last_clip = BoxArea::BorderBox;
    for i in 0..count {
        // Property lists repeat cyclically to the length of the image list.
        let pick = |len: usize| i % len.max(1);
        let Some(clip) = box_area_clip(bg.background_clip.0[pick(bg.background_clip.0.len())])
        else {
            // `background-clip: text` and friends have no shape to clip to;
            // painting the layer unclipped would cover the whole box.
            report.skipped = true;
            continue;
        };
        last_clip = clip;
        let content = match &bg.background_image.0[i] {
            GenericImage::Url(url) => match url.url() {
                Some(u) => BackgroundContent::Image(ImageKey(Arc::from(u.as_str()))),
                None => {
                    report.skipped = true;
                    continue;
                }
            },
            GenericImage::Gradient(g) => match linear_gradient(g, current, size, report) {
                Some(g) => BackgroundContent::Gradient(g),
                None => {
                    report.skipped = true;
                    continue;
                }
            },
            GenericImage::None => continue,
            _ => {
                report.skipped = true;
                continue;
            }
        };
        let repeat = &bg.background_repeat.0[pick(bg.background_repeat.0.len())];
        let px = &bg.background_position_x.0[pick(bg.background_position_x.0.len())];
        let py = &bg.background_position_y.0[pick(bg.background_position_y.0.len())];
        layers.push(BackgroundLayer {
            content,
            repeat: (repeat_kind(repeat.0), repeat_kind(repeat.1)),
            position: (length_percent(px), length_percent(py)),
            size: background_size(&bg.background_size.0[pick(bg.background_size.0.len())]),
            origin: box_area_origin(bg.background_origin.0[pick(bg.background_origin.0.len())]),
            clip,
        });
    }
    let color = resolve_color(&bg.background_color, current);
    if !color.is_transparent() {
        layers.push(BackgroundLayer {
            content: BackgroundContent::Color(color),
            repeat: (RepeatKind::Repeat, RepeatKind::Repeat),
            position: (LengthPercent::default(), LengthPercent::default()),
            size: BackgroundSize::Auto,
            origin: BoxArea::PaddingBox,
            clip: last_clip,
        });
    }
    layers
}

fn background_size(size: &style::values::computed::BackgroundSize) -> BackgroundSize {
    use style::values::generics::background::GenericBackgroundSize as S;
    use style::values::generics::length::GenericLengthPercentageOrAuto as A;
    let axis = |a: &A<style::values::computed::NonNegativeLengthPercentage>| match a {
        A::LengthPercentage(lp) => Some(length_percent(&lp.0)),
        A::Auto => None,
    };
    match size {
        S::Cover => BackgroundSize::Cover,
        S::Contain => BackgroundSize::Contain,
        S::ExplicitSize { width, height } => match (axis(width), axis(height)) {
            (None, None) => BackgroundSize::Auto,
            (w, h) => BackgroundSize::Explicit(w, h),
        },
    }
}

/// The angle in degrees (0 up, clockwise) a gradient line points at. The
/// `to <corner>` forms aim the line so that the 50% colour passes through the
/// two corners the line does not point at, which depends on the box shape.
pub(super) fn resolve_gradient_angle(
    direction: &style::values::computed::image::LineDirection,
    size: (f32, f32),
) -> f32 {
    use style::values::computed::image::LineDirection;
    use style::values::specified::position::{
        HorizontalPositionKeyword as H, VerticalPositionKeyword as V,
    };
    match direction {
        LineDirection::Angle(a) => a.degrees(),
        LineDirection::Horizontal(H::Right) => 90.0,
        LineDirection::Horizontal(H::Left) => 270.0,
        LineDirection::Vertical(V::Top) => 0.0,
        LineDirection::Vertical(V::Bottom) => 180.0,
        LineDirection::Corner(h, v) => {
            // A degenerate box falls back to the square diagonal.
            let (w, h_px) = if size.0 > 0.0 && size.1 > 0.0 {
                size
            } else {
                (1.0, 1.0)
            };
            let x = if matches!(h, H::Right) { h_px } else { -h_px };
            let y = if matches!(v, V::Top) { -w } else { w };
            // atan2(x, -y): clockwise from "up" for a y-down vector.
            x.atan2(-y).to_degrees().rem_euclid(360.0)
        }
    }
}

fn linear_gradient(
    g: &style::values::computed::Gradient,
    current: &AbsoluteColor,
    size: (f32, f32),
    report: &mut Report,
) -> Option<GradientPaint> {
    use style::values::computed::image::LineDirection;
    use style::values::generics::image::GenericGradient;
    let GenericGradient::Linear {
        direction,
        items,
        flags,
        ..
    } = g
    else {
        return None;
    };
    let repeating = flags.contains(style::values::generics::image::GradientFlags::REPEATING);
    if matches!(direction, LineDirection::Corner(..)) {
        report.size_dependent = true;
    }
    let angle_deg = resolve_gradient_angle(direction, size);

    let mut stops: Vec<(Color, Option<LengthPercent>)> = Vec::with_capacity(items.len());
    for item in items.iter() {
        match item {
            GenericGradientItem::SimpleColorStop(c) => {
                stops.push((resolve_color(c, current), None))
            }
            GenericGradientItem::ComplexColorStop { color, position } => stops.push((
                resolve_color(color, current),
                Some(length_percent(position)),
            )),
            // A hint bends the interpolation between two stops; without it the
            // gradient is drawn with the midpoint at the default position.
            GenericGradientItem::InterpolationHint(_) => report.skipped = true,
        }
    }
    if stops.len() < 2 {
        return None;
    }
    let last = stops.len() - 1;
    if stops[0].1.is_none() {
        stops[0].1 = Some(LengthPercent::fraction(0.0));
    }
    if stops[last].1.is_none() {
        stops[last].1 = Some(LengthPercent::fraction(1.0));
    }
    let mut i = 1;
    while i < last {
        if stops[i].1.is_some() {
            i += 1;
            continue;
        }
        let run_start = i - 1;
        let mut run_end = i;
        while stops[run_end].1.is_none() {
            run_end += 1;
        }
        let a = stops[run_start].1.unwrap_or_default();
        let b = stops[run_end].1.unwrap_or_default();
        let span = (run_end - run_start) as f32;
        for (n, stop) in stops[i..run_end].iter_mut().enumerate() {
            let t = (n + 1) as f32 / span;
            stop.1 = Some(LengthPercent {
                px: a.px + (b.px - a.px) * t,
                fraction: a.fraction + (b.fraction - a.fraction) * t,
            });
        }
        i = run_end + 1;
    }
    Some(GradientPaint {
        angle_deg,
        stops: stops
            .into_iter()
            .map(|(color, position)| ColorStop {
                position: position.unwrap_or_default(),
                color,
            })
            .collect(),
        repeating,
    })
}

fn border_style(s: style::values::computed::BorderStyle) -> BorderStyle {
    use style::values::computed::BorderStyle as S;
    match s {
        S::None => BorderStyle::None,
        S::Hidden => BorderStyle::Hidden,
        S::Solid => BorderStyle::Solid,
        S::Dashed => BorderStyle::Dashed,
        S::Dotted => BorderStyle::Dotted,
        S::Double => BorderStyle::Double,
        S::Groove => BorderStyle::Groove,
        S::Ridge => BorderStyle::Ridge,
        S::Inset => BorderStyle::Inset,
        S::Outset => BorderStyle::Outset,
    }
}

fn border_paint(style: &ComputedValues, current: &AbsoluteColor) -> BorderPaint {
    let b = style.get_border();
    let styles = [
        border_style(b.border_top_style),
        border_style(b.border_right_style),
        border_style(b.border_bottom_style),
        border_style(b.border_left_style),
    ];
    let width = |w: &style::values::computed::BorderSideWidth, s: BorderStyle| {
        if matches!(s, BorderStyle::None | BorderStyle::Hidden) {
            0.0
        } else {
            w.0.to_f32_px()
        }
    };
    let corner = |r: &style::values::computed::BorderCornerRadius| {
        [length_percent(&r.0.width.0), length_percent(&r.0.height.0)]
    };
    let [tl_x, tl_y] = corner(&b.border_top_left_radius);
    let [tr_x, tr_y] = corner(&b.border_top_right_radius);
    let [br_x, br_y] = corner(&b.border_bottom_right_radius);
    let [bl_x, bl_y] = corner(&b.border_bottom_left_radius);
    BorderPaint {
        widths: Edges {
            top: width(&b.border_top_width, styles[0]),
            right: width(&b.border_right_width, styles[1]),
            bottom: width(&b.border_bottom_width, styles[2]),
            left: width(&b.border_left_width, styles[3]),
        },
        colors: [
            resolve_color(&b.border_top_color, current),
            resolve_color(&b.border_right_color, current),
            resolve_color(&b.border_bottom_color, current),
            resolve_color(&b.border_left_color, current),
        ],
        styles,
        radii: [tl_x, tl_y, tr_x, tr_y, br_x, br_y, bl_x, bl_y],
    }
}

fn outline_paint(style: &ComputedValues, current: &AbsoluteColor) -> Option<OutlinePaint> {
    use style::values::computed::OutlineStyle;
    let o = style.get_outline();
    let line = match o.outline_style {
        OutlineStyle::Auto => BorderStyle::Solid,
        OutlineStyle::BorderStyle(s) => border_style(s),
    };
    let width = o.outline_width.0.to_f32_px();
    if matches!(line, BorderStyle::None | BorderStyle::Hidden) || width <= 0.0 {
        return None;
    }
    Some(OutlinePaint {
        width,
        style: line,
        color: resolve_color(&o.outline_color, current),
        offset: o.outline_offset.to_f32_px(),
    })
}

fn decoration(
    style: &ComputedValues,
    current: &AbsoluteColor,
    ancestor: &DecorationPaint,
) -> DecorationPaint {
    use style::computed_values::text_decoration_style::T as S;
    use style::values::computed::text::TextDecorationLength;
    let text = style.get_text();
    let inherited_text = style.get_inherited_text();
    let line = text.text_decoration_line;
    if line.is_empty() {
        return *ancestor;
    }
    DecorationPaint {
        underline: ancestor.underline || line.contains(TextDecorationLine::UNDERLINE),
        overline: ancestor.overline || line.contains(TextDecorationLine::OVERLINE),
        line_through: ancestor.line_through || line.contains(TextDecorationLine::LINE_THROUGH),
        style: match text.text_decoration_style {
            S::Solid => LineStyle::Solid,
            S::Double => LineStyle::Double,
            S::Dotted => LineStyle::Dotted,
            S::Dashed => LineStyle::Dashed,
            S::Wavy => LineStyle::Wavy,
            _ => LineStyle::Solid,
        },
        color: resolve_color(&text.text_decoration_color, current),
        thickness: match &text.text_decoration_thickness {
            TextDecorationLength::LengthPercentage(lp) => lp.to_length().map(|l| l.px()),
            _ => None,
        },
        underline_offset: match &inherited_text.text_underline_offset {
            LengthPercentageOrAuto::LengthPercentage(lp) => Some(length_percent(lp)),
            LengthPercentageOrAuto::Auto => None,
        },
        underline_position: {
            let position = inherited_text.text_underline_position;
            if position.contains(TextUnderlinePosition::FROM_FONT) {
                UnderlinePosition::FromFont
            } else if position.contains(TextUnderlinePosition::UNDER) {
                UnderlinePosition::Under
            } else {
                UnderlinePosition::Auto
            }
        },
        // Stylo's servo flavour has no `text-decoration-skip-ink` longhand, so
        // the initial value (`auto`) always applies.
        skip_ink: true,
    }
}

fn overflow(o: Overflow) -> OverflowKind {
    match o {
        Overflow::Visible => OverflowKind::Visible,
        Overflow::Hidden => OverflowKind::Hidden,
        Overflow::Scroll => OverflowKind::Scroll,
        Overflow::Auto => OverflowKind::Auto,
        Overflow::Clip => OverflowKind::Clip,
    }
}

fn position_kind(p: style::computed_values::position::T) -> PositionKind {
    use style::computed_values::position::T;
    match p {
        T::Static => PositionKind::Static,
        T::Relative => PositionKind::Relative,
        T::Absolute => PositionKind::Absolute,
        T::Fixed => PositionKind::Fixed,
        T::Sticky => PositionKind::Sticky,
    }
}

fn z_index(style: &ComputedValues) -> Option<i32> {
    use style::values::generics::position::GenericZIndex;
    match style.get_position().z_index {
        GenericZIndex::Integer(i) => Some(i),
        GenericZIndex::Auto => None,
    }
}

fn rotation(radians: f32) -> Affine {
    let (sin, cos) = radians.sin_cos();
    Affine {
        a: cos,
        b: sin,
        c: -sin,
        d: cos,
        e: 0.0,
        f: 0.0,
    }
}

/// One `transform` function as a 2D matrix. Percentage translations resolve
/// against the border box; everything else goes through Stylo's own matrix.
fn operation_affine(
    op: &style::values::computed::transform::TransformOperation,
    (w, h): (f32, f32),
    report: &mut Report,
) -> Option<Affine> {
    use style::values::computed::transform::TransformOperation as Op;
    use style::values::generics::transform::ToMatrix;
    let mut length = |lp: &LengthPercentage, basis: f32| {
        let v = length_percent(lp);
        report.size_dependent |= v.fraction != 0.0;
        v.resolve(basis)
    };
    match op {
        Op::Translate(x, y) | Op::Translate3D(x, y, _) => {
            Some(Affine::translation(length(x, w), length(y, h)))
        }
        Op::TranslateX(x) => Some(Affine::translation(length(x, w), 0.0)),
        Op::TranslateY(y) => Some(Affine::translation(0.0, length(y, h))),
        _ => {
            // The 2D part of a 3D matrix; depth and perspective are dropped.
            let m = op.to_3d_matrix(None).ok()?;
            Some(Affine {
                a: m.m11 as f32,
                b: m.m12 as f32,
                c: m.m21 as f32,
                d: m.m22 as f32,
                e: m.m41 as f32,
                f: m.m42 as f32,
            })
        }
    }
}

fn transform_affine(style: &ComputedValues, w: f32, h: f32, report: &mut Report) -> Option<Affine> {
    use style::values::generics::transform::{GenericRotate, GenericScale, GenericTranslate};
    let b = style.get_box();
    let mut total = Affine::IDENTITY;
    if let GenericTranslate::Translate(x, y, _) = &b.translate {
        let (x, y) = (length_percent(x), length_percent(y));
        report.size_dependent |= x.fraction != 0.0 || y.fraction != 0.0;
        total = total.after(&Affine::translation(x.resolve(w), y.resolve(h)));
    }
    match &b.rotate {
        GenericRotate::None => {}
        GenericRotate::Rotate(angle) => total = total.after(&rotation(angle.radians())),
        // A rotation about the z axis is the 2D rotation; other axes tilt the
        // box out of the plane, which a 2D matrix cannot express.
        GenericRotate::Rotate3D(x, y, z, angle) if *x == 0.0 && *y == 0.0 && *z != 0.0 => {
            total = total.after(&rotation(angle.radians() * z.signum()));
        }
        GenericRotate::Rotate3D(..) => report.skipped = true,
    }
    if let GenericScale::Scale(x, y, _) = &b.scale {
        total = total.after(&Affine::scale(*x, *y));
    }
    for op in b.transform.0.iter() {
        match operation_affine(op, (w, h), report) {
            Some(m) => total = total.after(&m),
            None => report.skipped = true,
        }
    }
    (!total.is_identity()).then_some(total)
}

fn filter_op(
    f: &style::values::computed::Filter,
    current: &AbsoluteColor,
    report: &mut Report,
) -> Option<FilterOp> {
    use style::values::generics::effects::Filter as F;
    Some(match f {
        F::Blur(l) => FilterOp::Blur(l.0.px()),
        F::Brightness(v) => FilterOp::Brightness(v.0),
        F::Contrast(v) => FilterOp::Contrast(v.0),
        F::Grayscale(v) => FilterOp::Grayscale(v.0),
        F::HueRotate(a) => FilterOp::HueRotate(a.degrees()),
        F::Invert(v) => FilterOp::Invert(v.0),
        F::Opacity(v) => FilterOp::Opacity(v.0),
        F::Saturate(v) => FilterOp::Saturate(v.0),
        F::Sepia(v) => FilterOp::Sepia(v.0),
        F::DropShadow(s) => FilterOp::DropShadow(simple_shadow(s, current)),
        F::Url(_) => {
            report.skipped = true;
            return None;
        }
    })
}

fn blend_mode(m: style::computed_values::mix_blend_mode::T) -> BlendMode {
    use style::computed_values::mix_blend_mode::T;
    match m {
        T::Normal => BlendMode::Normal,
        T::Multiply => BlendMode::Multiply,
        T::Screen => BlendMode::Screen,
        T::Overlay => BlendMode::Overlay,
        T::Darken => BlendMode::Darken,
        T::Lighten => BlendMode::Lighten,
        T::ColorDodge => BlendMode::ColorDodge,
        T::ColorBurn => BlendMode::ColorBurn,
        T::HardLight => BlendMode::HardLight,
        T::SoftLight => BlendMode::SoftLight,
        T::Difference => BlendMode::Difference,
        T::Exclusion => BlendMode::Exclusion,
        T::Hue => BlendMode::Hue,
        T::Saturation => BlendMode::Saturation,
        T::Color => BlendMode::Color,
        T::Luminosity => BlendMode::Luminosity,
        T::PlusLighter => BlendMode::Screen,
    }
}

fn object_fit(style: &ComputedValues) -> ObjectFit {
    use style::computed_values::object_fit::T;
    match style.get_position().object_fit {
        T::Fill => ObjectFit::Fill,
        T::Contain => ObjectFit::Contain,
        T::Cover => ObjectFit::Cover,
        T::None => ObjectFit::None,
        T::ScaleDown => ObjectFit::ScaleDown,
    }
}

/// The marker a list item draws, or `None` for `list-style-type: none` and
/// for boxes that are not list items.
fn list_marker(style: &ComputedValues) -> Option<MarkerPaint> {
    if !style.get_box().display.is_list_item() {
        return None;
    }
    let list = style.get_list();
    let kind = marker_kind(&list.list_style_type.0)?;
    let inside = matches!(
        list.list_style_position,
        style::computed_values::list_style_position::T::Inside
    );
    Some(MarkerPaint { kind, inside })
}

/// How a counter style is drawn as a marker: a shape for bullets, `Text`
/// for everything that is a counter string, `None` for `none`.
fn marker_kind(counter_style: &style::counter_style::CounterStyle) -> Option<MarkerKind> {
    use style::counter_style::CounterStyle;
    match counter_style {
        CounterStyle::None => None,
        CounterStyle::Name(name) => Some(match &*name.0 {
            "disc" => MarkerKind::Disc,
            "circle" => MarkerKind::Circle,
            "square" => MarkerKind::Square,
            "disclosure-open" => MarkerKind::DisclosureOpen,
            "disclosure-closed" => MarkerKind::DisclosureClosed,
            _ => MarkerKind::Text,
        }),
        _ => Some(MarkerKind::Text),
    }
}
