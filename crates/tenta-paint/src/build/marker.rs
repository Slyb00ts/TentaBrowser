//! List markers.

use tenta_layout::{Affine, BorderStyle, Color, Edges, Fragment, FragmentKind, MarkerKind, Rect};

use crate::dl::Item;

/// Pushes the shape of a list-marker fragment; does nothing for other kinds.
///
/// The fragment's border box is the bullet's own box, sized and placed by
/// layout from the marker font (Blink's bullet is a square a third of the
/// ascent wide, set a little below the middle of the line), so the shape fills
/// the box it is given: a disc or circle is the ellipse inscribed in it, and a
/// disclosure triangle spans it. A counter string (`MarkerKind::Text`) has no
/// shape; its text is a child fragment that the text emitter paints.
pub(super) fn emit(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
    let FragmentKind::Marker(kind) = frag.kind else {
        return;
    };
    let color = frag.paint.color;
    if color.is_transparent() || border_box.w <= 0.0 || border_box.h <= 0.0 {
        return;
    }
    match kind {
        MarkerKind::Disc => items.push(Item::FillRRect {
            rect: border_box,
            radii: ellipse_radii(border_box),
            color,
        }),
        MarkerKind::Square => items.push(Item::FillRect {
            rect: border_box,
            color,
        }),
        MarkerKind::Circle => {
            // Blink strokes the ring one pixel wide centred on the box edge, so
            // it reaches half a pixel outside the box.
            let ring = Rect::new(
                border_box.x - 0.5,
                border_box.y - 0.5,
                border_box.w + 1.0,
                border_box.h + 1.0,
            );
            items.push(Item::Border {
                rect: ring,
                widths: Edges::all(1.0),
                colors: [color; 4],
                styles: [BorderStyle::Solid; 4],
                radii: ellipse_radii(ring),
            });
        }
        MarkerKind::DisclosureOpen => triangle(items, border_box, Direction::Down, color),
        MarkerKind::DisclosureClosed => triangle(items, border_box, Direction::Right, color),
        MarkerKind::Text => {}
    }
}

fn ellipse_radii(rect: Rect) -> [f32; 8] {
    let (rx, ry) = (rect.w / 2.0, rect.h / 2.0);
    [rx, ry, rx, ry, rx, ry, rx, ry]
}

enum Direction {
    Right,
    Down,
}

/// A triangle spanning `rect`, as a square turned 45 degrees and scaled into a
/// diamond, of which the clip keeps one half. The display list has no polygon
/// item, and this keeps both slanted edges anti-aliased.
fn triangle(items: &mut Vec<Item>, rect: Rect, direction: Direction, color: Color) {
    // The diamond's centre sits on the edge opposite the apex; its half-axes
    // reach the apex and the two corners of that edge.
    let (centre, half) = match direction {
        Direction::Right => ((rect.x, rect.y + rect.h / 2.0), (rect.w, rect.h / 2.0)),
        Direction::Down => ((rect.x + rect.w / 2.0, rect.y), (rect.w / 2.0, rect.h)),
    };
    let turn = std::f32::consts::FRAC_1_SQRT_2;
    let diamond = Affine::translation(centre.0, centre.1)
        .after(&Affine::scale(half.0, half.1))
        .after(&Affine {
            a: turn,
            b: turn,
            c: -turn,
            d: turn,
            e: 0.0,
            f: 0.0,
        });
    // Half the side of a square whose corners reach distance 1 on the axes.
    let side = std::f32::consts::FRAC_1_SQRT_2;
    items.push(Item::PushClip {
        rect,
        radii: [0.0; 8],
    });
    items.push(Item::PushTransform(diamond.to_array()));
    items.push(Item::FillRect {
        rect: Rect::new(-side, -side, 2.0 * side, 2.0 * side),
        color,
    });
    items.push(Item::PopTransform);
    items.push(Item::PopClip);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tenta_core::NodeId;
    use tenta_layout::{Color, PaintStyle};

    use super::*;

    const RED: Color = Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };

    fn marker(kind: MarkerKind) -> Fragment {
        let style = PaintStyle {
            color: RED,
            ..PaintStyle::default()
        };
        Fragment::new(
            NodeId(2),
            FragmentKind::Marker(kind),
            Arc::new(style),
            Rect::new(0.0, 0.0, 6.0, 6.0),
        )
    }

    fn emitted(kind: MarkerKind, border_box: Rect) -> Vec<Item> {
        let mut items = Vec::new();
        emit(&marker(kind), border_box, &mut items);
        items
    }

    #[test]
    fn a_disc_is_the_ellipse_inscribed_in_its_box() {
        assert_eq!(
            emitted(MarkerKind::Disc, Rect::new(12.0, 30.0, 5.0, 5.0)),
            vec![Item::FillRRect {
                rect: Rect::new(12.0, 30.0, 5.0, 5.0),
                radii: [2.5; 8],
                color: RED,
            }]
        );
    }

    #[test]
    fn a_square_fills_its_box() {
        assert_eq!(
            emitted(MarkerKind::Square, Rect::new(12.0, 30.0, 5.0, 5.0)),
            vec![Item::FillRect {
                rect: Rect::new(12.0, 30.0, 5.0, 5.0),
                color: RED,
            }]
        );
    }

    #[test]
    fn a_circle_is_a_one_pixel_ring_centred_on_the_box_edge() {
        assert_eq!(
            emitted(MarkerKind::Circle, Rect::new(12.0, 30.0, 5.0, 5.0)),
            vec![Item::Border {
                rect: Rect::new(11.5, 29.5, 6.0, 6.0),
                widths: Edges::all(1.0),
                colors: [RED; 4],
                styles: [BorderStyle::Solid; 4],
                radii: [3.0; 8],
            }]
        );
    }

    #[test]
    fn counter_text_has_no_shape_of_its_own() {
        assert!(emitted(MarkerKind::Text, Rect::new(0.0, 0.0, 20.0, 18.0)).is_empty());
    }

    #[test]
    fn empty_boxes_and_other_fragments_draw_nothing() {
        assert!(emitted(MarkerKind::Disc, Rect::new(0.0, 0.0, 0.0, 5.0)).is_empty());
        let mut frag = marker(MarkerKind::Disc);
        frag.kind = FragmentKind::Box;
        let mut items = Vec::new();
        emit(&frag, Rect::new(0.0, 0.0, 5.0, 5.0), &mut items);
        assert!(items.is_empty());
    }

    /// The corners of the unit square the triangle fills, mapped through the
    /// transform the items carry.
    fn diamond(items: &[Item]) -> Vec<(f32, f32)> {
        let [
            Item::PushClip { .. },
            Item::PushTransform([a, b, c, d, e, f]),
            Item::FillRect { rect, .. },
            Item::PopTransform,
            Item::PopClip,
        ] = items
        else {
            panic!("expected a clipped, transformed fill, got {items:?}");
        };
        let m = Affine {
            a: *a,
            b: *b,
            c: *c,
            d: *d,
            e: *e,
            f: *f,
        };
        [
            (rect.x, rect.y),
            (rect.right(), rect.y),
            (rect.right(), rect.bottom()),
            (rect.x, rect.bottom()),
        ]
        .map(|(x, y)| m.apply(x, y))
        .to_vec()
    }

    fn has_vertex(vertices: &[(f32, f32)], x: f32, y: f32) -> bool {
        vertices
            .iter()
            .any(|v| (v.0 - x).abs() < 1e-3 && (v.1 - y).abs() < 1e-3)
    }

    #[test]
    fn a_closed_disclosure_points_right_and_is_clipped_to_its_box() {
        let bb = Rect::new(10.0, 20.0, 8.0, 12.0);
        let items = emitted(MarkerKind::DisclosureClosed, bb);
        assert_eq!(
            items[0],
            Item::PushClip {
                rect: bb,
                radii: [0.0; 8]
            }
        );
        // The clip keeps the right half of this diamond: the triangle with
        // its base on the left edge and its apex on the right.
        let vertices = diamond(&items);
        assert!(has_vertex(&vertices, 18.0, 26.0), "apex in {vertices:?}");
        assert!(has_vertex(&vertices, 10.0, 20.0), "top in {vertices:?}");
        assert!(has_vertex(&vertices, 10.0, 32.0), "bottom in {vertices:?}");
        assert!(
            has_vertex(&vertices, 2.0, 26.0),
            "clipped tip in {vertices:?}"
        );
    }

    #[test]
    fn an_open_disclosure_points_down() {
        let bb = Rect::new(10.0, 20.0, 8.0, 12.0);
        let vertices = diamond(&emitted(MarkerKind::DisclosureOpen, bb));
        assert!(has_vertex(&vertices, 14.0, 32.0), "apex in {vertices:?}");
        assert!(has_vertex(&vertices, 10.0, 20.0), "left in {vertices:?}");
        assert!(has_vertex(&vertices, 18.0, 20.0), "right in {vertices:?}");
        assert!(
            has_vertex(&vertices, 14.0, 8.0),
            "clipped tip in {vertices:?}"
        );
    }
}
