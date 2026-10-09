//! Replaced images and image background layers.
//!
//! Both turn a loaded image into `Item::Image` draws. `<img>` content is sized
//! and placed by `object-fit` and `object-position` inside the content box and
//! clipped to it; a background layer is sized by `background-size`, positioned
//! by `background-position` and tiled by `background-repeat` inside its
//! positioning area and clipped to its painting area. An image that has not
//! loaded (or failed) draws nothing: its box keeps the size layout gave it.

use tenta_layout::{
    BackgroundLayer, BackgroundSize, BorderStyle, Color, Edges, Fragment, FragmentKind, ImageKey,
    ImageRendering, IntrinsicSize, LengthPercent, ObjectFit, Rect, RepeatKind, ReplacedKind,
};

use super::background::{MIN_TILE, contains, tile_spans};
use super::border::{inset_radii, inset_rect, resolve_radii};
use crate::PaintContext;
use crate::dl::{Item, Radii};
use crate::image::ImageQuality;

/// Most tiles one background layer may draw. A one-pixel tile over a large
/// area would otherwise emit millions of items.
const MAX_TILES: usize = 4096;

const EPSILON: f32 = 1e-3;

/// Pushes the content of a replaced fragment.
pub(crate) fn emit(frag: &Fragment, border_box: Rect, cx: &PaintContext, items: &mut Vec<Item>) {
    // A video shows its poster image until it plays.
    let FragmentKind::Replaced(kind @ (ReplacedKind::Image | ReplacedKind::Video)) = frag.kind
    else {
        return;
    };
    let drawable = cx
        .bindings
        .node_image(frag.node)
        .filter(|image| cx.images.get(image.id).is_some());
    let Some(image) = drawable else {
        if kind == ReplacedKind::Image && cx.bindings.node_broken(frag.node) {
            emit_broken(border_box, items);
        }
        return;
    };
    let padding_box = inset_rect(border_box, &frag.border);
    let content = inset_rect(padding_box, &frag.padding);
    if content.w <= 0.0 || content.h <= 0.0 {
        return;
    }
    let style = &frag.paint;
    let dest = object_rect(
        image.natural,
        content,
        style.object_fit,
        style.object_position,
    );
    if dest.w <= 0.0 || dest.h <= 0.0 {
        return;
    }
    let outer = resolve_radii(&style.border.radii, border_box);
    let radii = inset_radii(inset_radii(outer, &frag.border), &frag.padding);
    let rounded = radii.iter().any(|&r| r > 0.0);
    let clipped = rounded || !contains(&content, &dest);
    if clipped {
        items.push(Item::PushClip {
            rect: content,
            radii,
        });
    }
    items.push(Item::Image {
        dest,
        src: image.id,
        src_rect: None,
        quality: quality(style.image_rendering),
    });
    if clipped {
        items.push(Item::PopClip);
    }
}

/// The frame of a broken image that has a box: one light-grey pixel line, as
/// Chrome draws, without its icon. Text-less boxes under 3 px show nothing.
fn emit_broken(border_box: Rect, items: &mut Vec<Item>) {
    if border_box.w < 3.0 || border_box.h < 3.0 {
        return;
    }
    items.push(Item::Border {
        rect: border_box,
        widths: Edges::all(1.0),
        colors: [Color::from_rgba8(192, 192, 192, 255); 4],
        styles: [BorderStyle::Solid; 4],
        radii: [0.0; 8],
    });
}

/// Pushes one image background layer clipped to `clip` with `clip_radii`;
/// `origin` is the positioning area.
pub(super) fn emit_background(
    layer: &BackgroundLayer,
    key: &ImageKey,
    origin: Rect,
    clip: Rect,
    clip_radii: Radii,
    cx: &PaintContext,
    items: &mut Vec<Item>,
) {
    let Some(image) = cx.bindings.url_image(key) else {
        return;
    };
    let Some((mut width, mut height)) = tile_size(layer.size, image.natural, origin) else {
        return;
    };
    // `round` fits a whole number of tiles; the other axis, when it is `auto`,
    // scales with it to keep the image's proportions.
    let (auto_w, auto_h) = match layer.size {
        BackgroundSize::Auto => (true, true),
        BackgroundSize::Explicit(w, h) => (w.is_none(), h.is_none()),
        _ => (false, false),
    };
    let fit = |extent: f32, tile: f32| {
        let count = (extent / tile).round().max(1.0);
        extent / count / tile
    };
    let (round_x, round_y) = (
        layer.repeat.0 == RepeatKind::Round,
        layer.repeat.1 == RepeatKind::Round,
    );
    if width >= MIN_TILE && height >= MIN_TILE {
        let (fx, fy) = (fit(origin.w, width), fit(origin.h, height));
        match (round_x, round_y) {
            (true, false) if auto_h => height *= fx,
            (false, true) if auto_w => width *= fy,
            _ => {}
        }
    }
    if !(width >= super::background::MIN_TILE && height >= super::background::MIN_TILE) {
        return;
    }
    let start_x = origin.x + layer.position.0.resolve(origin.w - width);
    let start_y = origin.y + layer.position.1.resolve(origin.h - height);
    // Many tiles along an axis that repeats evenly are one pattern fill, not an
    // item each (a 2 px stripe repeated down an 8000 px page).
    if let Some(item) = pattern_item(
        layer,
        (width, height),
        (start_x, start_y),
        (origin, clip),
        image.id,
    ) {
        let Some(item) = item else {
            return;
        };
        let rounded = clip_radii.iter().any(|&r| r > 0.0);
        if rounded {
            items.push(Item::PushClip {
                rect: clip,
                radii: clip_radii,
            });
        }
        items.push(item);
        if rounded {
            items.push(Item::PopClip);
        }
        return;
    }
    let columns = tile_spans(
        (origin.x, origin.w),
        (start_x, width),
        layer.repeat.0,
        (clip.x, clip.right()),
    );
    let rows = tile_spans(
        (origin.y, origin.h),
        (start_y, height),
        layer.repeat.1,
        (clip.y, clip.bottom()),
    );
    if columns.is_empty() || rows.is_empty() || columns.len().saturating_mul(rows.len()) > MAX_TILES
    {
        return;
    }
    let tiles: Vec<Rect> = rows
        .iter()
        .flat_map(|&(y, h)| columns.iter().map(move |&(x, w)| Rect::new(x, y, w, h)))
        .collect();
    // `Item::Image` has no corner radii, so a rounded layer is always clipped.
    let rounded = clip_radii.iter().any(|&r| r > 0.0);
    let clipped = rounded || !tiles.iter().all(|t| contains(&clip, t));
    if clipped {
        items.push(Item::PushClip {
            rect: clip,
            radii: clip_radii,
        });
    }
    for tile in tiles {
        items.push(Item::Image {
            dest: tile,
            src: image.id,
            src_rect: None,
            quality: ImageQuality::Smooth,
        });
    }
    if clipped {
        items.push(Item::PopClip);
    }
}

/// More tiles than this along an axis make a pattern fill.
const PATTERN_THRESHOLD: f32 = 64.0;

/// The pattern fill for a layer whose even repetition along an axis would need
/// many tiles, or `None` when per-tile items do.
fn pattern_item(
    layer: &BackgroundLayer,
    (width, height): (f32, f32),
    (start_x, start_y): (f32, f32),
    (origin, clip): (Rect, Rect),
    src: crate::image::ImageId,
) -> Option<Option<Item>> {
    let even = |kind| matches!(kind, RepeatKind::Repeat | RepeatKind::Round);
    let (rx, ry) = layer.repeat;
    if rx == RepeatKind::Space || ry == RepeatKind::Space {
        return None;
    }
    let step = |kind: RepeatKind, tile: f32, extent: f32| {
        if kind == RepeatKind::Round {
            extent / (extent / tile).round().max(1.0)
        } else {
            tile
        }
    };
    let (tw, th) = (step(rx, width, origin.w), step(ry, height, origin.h));
    let many = (even(rx) && clip.w / tw > PATTERN_THRESHOLD)
        || (even(ry) && clip.h / th > PATTERN_THRESHOLD);
    if !many {
        return None;
    }
    // An axis that does not repeat is cut to its one tile.
    let span = |kind: RepeatKind, start: f32, len: f32, clip: (f32, f32)| {
        if even(kind) {
            clip
        } else {
            (start.max(clip.0), (start + len).min(clip.1))
        }
    };
    let x = span(rx, start_x, tw, (clip.x, clip.right()));
    let y = span(ry, start_y, th, (clip.y, clip.bottom()));
    if !(x.1 > x.0 && y.1 > y.0) {
        // Nothing of the layer is visible.
        return Some(None);
    }
    Some(Some(Item::ImagePattern {
        area: Rect::new(x.0, y.0, x.1 - x.0, y.1 - y.0),
        origin: (start_x, start_y),
        tile: (tw, th),
        src,
        quality: ImageQuality::Smooth,
    }))
}

fn quality(rendering: ImageRendering) -> ImageQuality {
    match rendering {
        ImageRendering::Auto => ImageQuality::Smooth,
        ImageRendering::Pixelated => ImageQuality::Pixelated,
    }
}

/// The natural aspect ratio: the stated one, else the one of the size.
fn ratio_of(natural: IntrinsicSize) -> Option<f32> {
    natural
        .ratio
        .or(match (natural.width, natural.height) {
            (Some(w), Some(h)) if h > 0.0 => Some(w / h),
            _ => None,
        })
        .filter(|r| r.is_finite() && *r > 0.0)
}

/// The largest box of `ratio` inside (`contain`) or the smallest outside
/// (`cover`) a `w x h` box.
fn fit_ratio(ratio: f32, w: f32, h: f32, contain: bool) -> (f32, f32) {
    let wide = w / h.max(EPSILON) > ratio;
    if wide == contain {
        (h * ratio, h)
    } else {
        (w, w / ratio)
    }
}

/// The size of the image under `object-fit` in a content box (css-images-3,
/// 5.5): the concrete object size.
fn object_size(natural: IntrinsicSize, content: Rect, fit: ObjectFit) -> (f32, f32) {
    let (cw, ch) = (content.w, content.h);
    let ratio = ratio_of(natural);
    // The size `none` gives: the natural one, completed through the ratio, and
    // the contain constraint when there is nothing natural to complete.
    let none = || match (natural.width, natural.height, ratio) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(r)) => (w, w / r),
        (None, Some(h), Some(r)) => (h * r, h),
        (None, None, Some(r)) => fit_ratio(r, cw, ch, true),
        (Some(w), None, None) => (w, ch),
        (None, Some(h), None) => (cw, h),
        (None, None, None) => (cw, ch),
    };
    match fit {
        ObjectFit::Fill => (cw, ch),
        ObjectFit::None => none(),
        ObjectFit::Contain => ratio.map_or((cw, ch), |r| fit_ratio(r, cw, ch, true)),
        ObjectFit::Cover => ratio.map_or((cw, ch), |r| fit_ratio(r, cw, ch, false)),
        ObjectFit::ScaleDown => {
            let natural_size = none();
            let contained = ratio.map_or((cw, ch), |r| fit_ratio(r, cw, ch, true));
            if natural_size.0 <= contained.0 && natural_size.1 <= contained.1 {
                natural_size
            } else {
                contained
            }
        }
    }
}

/// Where the image is drawn: the concrete object size placed by
/// `object-position` in the content box.
pub(super) fn object_rect(
    natural: IntrinsicSize,
    content: Rect,
    fit: ObjectFit,
    position: (LengthPercent, LengthPercent),
) -> Rect {
    let (w, h) = object_size(natural, content, fit);
    Rect::new(
        content.x + position.0.resolve(content.w - w),
        content.y + position.1.resolve(content.h - h),
        w,
        h,
    )
}

/// The size of one tile (css-backgrounds-3, 3.9), or `None` for an unusable
/// image.
fn tile_size(size: BackgroundSize, natural: IntrinsicSize, area: Rect) -> Option<(f32, f32)> {
    let ratio = ratio_of(natural);
    let both_auto = || match (natural.width, natural.height, ratio) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(r)) => (w, w / r),
        (None, Some(h), Some(r)) => (h * r, h),
        (None, None, Some(r)) => fit_ratio(r, area.w, area.h, true),
        (Some(w), None, None) => (w, area.h),
        (None, Some(h), None) => (area.w, h),
        (None, None, None) => (area.w, area.h),
    };
    let (w, h) = match size {
        BackgroundSize::Auto => both_auto(),
        BackgroundSize::Contain | BackgroundSize::Cover => match ratio {
            Some(r) => fit_ratio(r, area.w, area.h, size == BackgroundSize::Contain),
            None => both_auto(),
        },
        BackgroundSize::Explicit(w, h) => {
            let w = w.map(|w| w.resolve(area.w).max(0.0));
            let h = h.map(|h| h.resolve(area.h).max(0.0));
            match (w, h) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => (w, ratio.map(|r| w / r).or(natural.height).unwrap_or(area.h)),
                (None, Some(h)) => (ratio.map(|r| h * r).or(natural.width).unwrap_or(area.w), h),
                (None, None) => both_auto(),
            }
        }
    };
    (w.is_finite() && h.is_finite()).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn natural(w: f32, h: f32) -> IntrinsicSize {
        IntrinsicSize {
            width: Some(w),
            height: Some(h),
            ratio: Some(w / h),
        }
    }

    const CENTER: (LengthPercent, LengthPercent) =
        (LengthPercent::fraction(0.5), LengthPercent::fraction(0.5));

    #[test]
    fn object_fit_sizes_follow_css_images() {
        let content = Rect::new(10.0, 20.0, 200.0, 100.0);
        let image = natural(100.0, 100.0);
        let at = |fit| object_rect(image, content, fit, CENTER);
        assert_eq!(at(ObjectFit::Fill), content);
        assert_eq!(at(ObjectFit::Contain), Rect::new(60.0, 20.0, 100.0, 100.0));
        assert_eq!(at(ObjectFit::Cover), Rect::new(10.0, -30.0, 200.0, 200.0));
        assert_eq!(at(ObjectFit::None), Rect::new(60.0, 20.0, 100.0, 100.0));
        // scale-down never enlarges.
        let small = natural(20.0, 10.0);
        assert_eq!(
            object_rect(small, content, ObjectFit::ScaleDown, CENTER),
            Rect::new(100.0, 65.0, 20.0, 10.0)
        );
        assert_eq!(
            object_rect(natural(400.0, 400.0), content, ObjectFit::ScaleDown, CENTER),
            Rect::new(60.0, 20.0, 100.0, 100.0)
        );
    }

    #[test]
    fn object_position_places_the_object() {
        let content = Rect::new(0.0, 0.0, 200.0, 100.0);
        let corner = (LengthPercent::px(0.0), LengthPercent::px(0.0));
        assert_eq!(
            object_rect(natural(50.0, 50.0), content, ObjectFit::None, corner),
            Rect::new(0.0, 0.0, 50.0, 50.0)
        );
        let right = (LengthPercent::fraction(1.0), LengthPercent::fraction(1.0));
        assert_eq!(
            object_rect(natural(50.0, 50.0), content, ObjectFit::None, right),
            Rect::new(150.0, 50.0, 50.0, 50.0)
        );
    }

    #[test]
    fn background_sizes_follow_css_backgrounds() {
        let area = Rect::new(0.0, 0.0, 300.0, 100.0);
        let image = natural(40.0, 20.0);
        assert_eq!(
            tile_size(BackgroundSize::Auto, image, area),
            Some((40.0, 20.0))
        );
        assert_eq!(
            tile_size(BackgroundSize::Contain, image, area),
            Some((200.0, 100.0))
        );
        assert_eq!(
            tile_size(BackgroundSize::Cover, image, area),
            Some((300.0, 150.0))
        );
        let explicit = BackgroundSize::Explicit(Some(LengthPercent::fraction(0.5)), None);
        assert_eq!(tile_size(explicit, image, area), Some((150.0, 75.0)));
        let ratio_only = IntrinsicSize {
            width: None,
            height: None,
            ratio: Some(2.0),
        };
        assert_eq!(
            tile_size(BackgroundSize::Auto, ratio_only, area),
            Some((200.0, 100.0))
        );
    }
}
