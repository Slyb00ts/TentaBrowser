//! Geometry queries over the session's fragment tree: the boxes of a node as
//! quads and the element under a point. Plain data in and out, so hosts
//! (the CDP `DOM` and `Input` domains, `elementFromPoint` later) need no
//! layout types.
//!
//! Coordinates are CSS pixels from the document's top-left corner. The engine
//! has no scrolling yet, so the viewport and the document share an origin.
//! 2D `transform`s are honoured; `perspective` and 3D transforms are not
//! modelled by layout and so not here either.

use tenta_core::NodeId;
use tenta_dom::Document;
use tenta_layout::{
    Affine, Edges, FragId, FragmentTree, LengthPercent, OverflowKind, PositionKind, Rect,
};

/// A quadrilateral, clockwise from the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad(pub [(f64, f64); 4]);

impl Quad {
    fn of_rect(rect: Rect) -> [(f32, f32); 4] {
        [
            (rect.x, rect.y),
            (rect.x + rect.w, rect.y),
            (rect.x + rect.w, rect.y + rect.h),
            (rect.x, rect.y + rect.h),
        ]
    }

    /// The smallest axis-aligned rectangle holding the quad, as `(x, y,
    /// width, height)`.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let xs = self.0.map(|point| point.0);
        let ys = self.0.map(|point| point.1);
        let (left, right) = (min_of(&xs), max_of(&xs));
        let (top, bottom) = (min_of(&ys), max_of(&ys));
        (left, top, right - left, bottom - top)
    }
}

fn min_of(values: &[f64; 4]) -> f64 {
    values.iter().copied().fold(f64::INFINITY, f64::min)
}

fn max_of(values: &[f64; 4]) -> f64 {
    values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

/// The four CSS boxes of one fragment of an element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxQuads {
    pub content: Quad,
    pub padding: Quad,
    pub border: Quad,
    pub margin: Quad,
}

/// The fragments of `node`'s own boxes: those its generated content
/// (`::before`, `::after`, `::marker`) also carries `node` for are left out.
pub(crate) fn own_fragments(tree: &FragmentTree, node: NodeId) -> Vec<FragId> {
    tree.fragments_of(node)
        .iter()
        .copied()
        .filter(|&id| !tree.get(id).paint.generated)
        .collect()
}

/// The boxes of every fragment `node` produced, in document order. Empty for
/// a node with no box (`display: none`, `display: contents`, not an element).
pub fn node_boxes(tree: &FragmentTree, node: NodeId) -> Vec<BoxQuads> {
    own_fragments(tree, node)
        .into_iter()
        .map(|id| fragment_boxes(tree, id))
        .collect()
}

/// What script geometry (`getClientRects`, `clientWidth`) needs of one
/// element.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementGeometry {
    /// `[x, y, width, height]` of the border box of each fragment.
    pub border: Vec<[f64; 4]>,
    /// Width and height of the padding box of the first fragment.
    pub client: [f64; 2],
    /// The computed `visibility` is `hidden` or `collapse` (inherited, so it
    /// is the value the element itself shows).
    pub visibility_hidden: bool,
    /// The computed `opacity` of the element, not multiplied by its ancestors'.
    pub opacity: f32,
}

/// The geometry of every element of the document, in document order: the
/// order `querySelectorAll('*')` lists them in. `None` for an element with no
/// box.
pub fn element_geometry(tree: &FragmentTree, doc: &Document) -> Vec<Option<ElementGeometry>> {
    doc.descendants(doc.document_node())
        .filter(|node| doc.is_element(*node))
        .map(|node| {
            let fragments = own_fragments(tree, node);
            let first = fragments.first()?;
            let border = fragments
                .iter()
                .map(|&id| {
                    let (x, y, width, height) = fragment_boxes(tree, id).border.bounds();
                    [x, y, width, height]
                })
                .collect();
            let (_, _, width, height) = fragment_boxes(tree, *first).padding.bounds();
            let paint = &tree.get(*first).paint;
            Some(ElementGeometry {
                border,
                client: [width, height],
                visibility_hidden: paint.visibility_hidden,
                opacity: paint.opacity,
            })
        })
        .collect()
}

fn fragment_boxes(tree: &FragmentTree, id: FragId) -> BoxQuads {
    let fragment = tree.get(id);
    let border_box = tree.absolute_border_box(id);
    let padding_box = shrink(border_box, fragment.border);
    let content_box = shrink(padding_box, fragment.padding);
    let margin_box = grow(border_box, fragment.margin);
    let quad = |rect: Rect| {
        let mut points = Quad::of_rect(rect);
        place(tree, id, &mut points);
        Quad(points.map(|(x, y)| (f64::from(x), f64::from(y))))
    };
    BoxQuads {
        content: quad(content_box),
        padding: quad(padding_box),
        border: quad(border_box),
        margin: quad(margin_box),
    }
}

fn shrink(rect: Rect, edges: Edges) -> Rect {
    Rect::new(
        rect.x + edges.left,
        rect.y + edges.top,
        (rect.w - edges.left - edges.right).max(0.0),
        (rect.h - edges.top - edges.bottom).max(0.0),
    )
}

fn grow(rect: Rect, edges: Edges) -> Rect {
    Rect::new(
        rect.x - edges.left,
        rect.y - edges.top,
        rect.w + edges.left + edges.right,
        rect.h + edges.top + edges.bottom,
    )
}

/// Applies the transforms of the fragment and its ancestors to points given
/// in untransformed document coordinates, innermost first.
fn place(tree: &FragmentTree, id: FragId, points: &mut [(f32, f32)]) {
    let mut cursor = Some(id);
    while let Some(current) = cursor {
        let fragment = tree.get(current);
        if let Some(transform) = fragment.paint.transform {
            let border_box = tree.absolute_border_box(current);
            let (origin_x, origin_y) =
                transform_origin(fragment.paint.transform_origin, border_box.w, border_box.h);
            let (origin_x, origin_y) = (border_box.x + origin_x, border_box.y + origin_y);
            for point in points.iter_mut() {
                let (x, y) = transform.apply(point.0 - origin_x, point.1 - origin_y);
                *point = (origin_x + x, origin_y + y);
            }
        }
        cursor = tree.parent(current);
    }
}

fn transform_origin(origin: (LengthPercent, LengthPercent), width: f32, height: f32) -> (f32, f32) {
    (
        origin.0.px + origin.0.fraction * width,
        origin.1.px + origin.1.fraction * height,
    )
}

fn invert(transform: &Affine) -> Option<Affine> {
    let determinant = transform.a * transform.d - transform.b * transform.c;
    if !determinant.is_finite() || determinant.abs() < 1e-9 {
        return None;
    }
    let (a, b, c, d) = (
        transform.d / determinant,
        -transform.b / determinant,
        -transform.c / determinant,
        transform.a / determinant,
    );
    Some(Affine {
        a,
        b,
        c,
        d,
        e: -(a * transform.e + c * transform.f),
        f: -(b * transform.e + d * transform.f),
    })
}

/// The element under the point, in the order a browser would pick it: the
/// topmost painted box that takes pointer events, text resolved to its
/// parent element. `None` outside the document's extent.
///
/// Stacking is approximated by sorting siblings by `z-index` class (negative,
/// in flow, positioned or `z-index: 0`, positive); nested stacking contexts
/// are not separated, so a high `z-index` inside a low-ranked context can win
/// over a sibling context that paints above it in a browser.
pub fn hit_test(tree: &FragmentTree, doc: &Document, x: f32, y: f32) -> Option<NodeId> {
    let root = tree.root;
    let extent = tree.scroll_extent;
    let inside = x >= 0.0
        && y >= 0.0
        && x < tree.viewport.w.max(extent.right())
        && y < tree.viewport.h.max(extent.bottom());
    if !inside {
        return None;
    }
    // The root's rect is relative to nothing: its parent space is the document.
    hit_fragment(tree, doc, root, (x, y))
        .or_else(|| doc.document_element())
        .filter(|node| doc.is_connected(*node))
}

/// `point` is relative to the parent fragment's border box.
fn hit_fragment(
    tree: &FragmentTree,
    doc: &Document,
    id: FragId,
    point: (f32, f32),
) -> Option<NodeId> {
    let fragment = tree.get(id);
    let mut point = point;
    if let Some(transform) = fragment.paint.transform {
        let (origin_x, origin_y) = transform_origin(
            fragment.paint.transform_origin,
            fragment.rect.w,
            fragment.rect.h,
        );
        let (origin_x, origin_y) = (fragment.rect.x + origin_x, fragment.rect.y + origin_y);
        let inverse = invert(&transform)?;
        let (x, y) = inverse.apply(point.0 - origin_x, point.1 - origin_y);
        point = (origin_x + x, origin_y + y);
    }
    let local = (point.0 - fragment.rect.x, point.1 - fragment.rect.y);
    let within =
        local.0 >= 0.0 && local.1 >= 0.0 && local.0 < fragment.rect.w && local.1 < fragment.rect.h;
    let clips = !matches!(fragment.paint.overflow_x, OverflowKind::Visible)
        || !matches!(fragment.paint.overflow_y, OverflowKind::Visible);
    if clips && !within {
        return None;
    }

    let mut children: Vec<((u8, i32), usize, FragId)> = fragment
        .children
        .iter()
        .enumerate()
        .map(|(index, &child)| (stack_class(tree, child), index, child))
        .collect();
    // Topmost first: higher class, then later in tree order.
    children.sort_by_key(|child| std::cmp::Reverse((child.0, child.1)));
    for (_, _, child) in children {
        if let Some(hit) = hit_fragment(tree, doc, child, local) {
            return Some(hit);
        }
    }

    if within && !fragment.paint.visibility_hidden && fragment.paint.pointer_events {
        return element_of(doc, fragment.node);
    }
    None
}

/// The paint layer of a child among its siblings, then its `z-index` within
/// the layer.
fn stack_class(tree: &FragmentTree, id: FragId) -> (u8, i32) {
    let paint = &tree.get(id).paint;
    match paint.z_index {
        Some(z) if z < 0 => (0, z),
        Some(z) if z > 0 => (3, z),
        Some(_) => (2, 0),
        None if paint.position != PositionKind::Static => (2, 0),
        None => (1, 0),
    }
}

/// The element a hit on `node` reports: the node itself, or the nearest
/// element ancestor of a text node.
fn element_of(doc: &Document, node: NodeId) -> Option<NodeId> {
    if node == NodeId::NONE || !doc.is_live(node) {
        return None;
    }
    let mut cursor = node;
    while cursor != NodeId::NONE {
        if doc.is_element(cursor) {
            return Some(cursor);
        }
        cursor = doc.parent(cursor);
    }
    None
}

#[cfg(test)]
mod tests {
    use tenta_html::{Parser, ParserOptions};

    use super::*;
    use crate::{RenderConfig, RenderSession, Viewport};

    fn parse(html: &str) -> Document {
        let mut parser = Parser::new(Document::new(), ParserOptions::default());
        parser.feed_last(html.as_bytes());
        parser.finish()
    }

    fn session() -> RenderSession {
        RenderSession::new(RenderConfig {
            viewport: Viewport {
                width: 400,
                height: 300,
                device_pixel_ratio: 1.0,
            },
            ..RenderConfig::default()
        })
    }

    fn id_of(doc: &Document, id: &str) -> NodeId {
        doc.get_element_by_id(id).expect("fixture id")
    }

    #[test]
    fn box_quads_nest_margin_border_padding_content() {
        let doc = parse(
            "<!doctype html><body style='margin:0'><div id=b style='margin:10px;border:2px solid;\
             padding:5px;width:100px;height:40px'></div>",
        );
        let mut session = session();
        let tree = session.layout(&doc);
        let boxes = node_boxes(tree, id_of(&doc, "b"));
        assert_eq!(boxes.len(), 1);
        let b = boxes[0];
        assert_eq!(b.margin.bounds(), (0.0, 0.0, 400.0, 74.0));
        assert_eq!(b.border.bounds(), (10.0, 10.0, 114.0, 54.0));
        assert_eq!(b.padding.bounds(), (12.0, 12.0, 110.0, 50.0));
        assert_eq!(b.content.bounds(), (17.0, 17.0, 100.0, 40.0));
        assert_eq!(b.content.0[0], (17.0, 17.0), "clockwise from the top-left");
        assert_eq!(b.content.0[2], (117.0, 57.0));
    }

    #[test]
    fn element_geometry_lists_elements_in_document_order() {
        let doc = parse(
            "<!doctype html><body style='margin:0'><div id=a style='width:50px;height:20px;padding:3px'></div>\
             <div id=gone style='display:none'></div><span id=s>text</span>",
        );
        let mut session = session();
        let tree = session.layout(&doc);
        let geometry = element_geometry(tree, &doc);
        let tags: Vec<&str> = doc
            .descendants(doc.document_node())
            .filter(|n| doc.is_element(*n))
            .map(|n| doc.tag(n).map_or("", |t| t.as_str()))
            .collect();
        assert_eq!(geometry.len(), tags.len());
        let at = |tag: &str| tags.iter().position(|t| *t == tag).unwrap();
        let a = geometry[tags.iter().position(|t| *t == "div").unwrap()]
            .as_ref()
            .unwrap();
        assert_eq!(a.border, [[0.0, 0.0, 56.0, 26.0]]);
        assert_eq!(a.client, [56.0, 26.0]);
        assert!(geometry[at("html")].is_some());
        assert!(
            geometry.iter().filter(|g| g.is_none()).count() >= 1,
            "head and the hidden div have no box"
        );
    }

    #[test]
    fn a_node_without_a_box_has_no_quads() {
        let doc = parse("<!doctype html><body><div id=n style='display:none'>x</div>");
        let mut session = session();
        let tree = session.layout(&doc);
        assert!(node_boxes(tree, id_of(&doc, "n")).is_empty());
    }

    #[test]
    fn a_transform_moves_the_quads_about_the_origin() {
        let doc = parse(
            "<!doctype html><body style='margin:0'><div id=t style='width:100px;height:50px;\
             transform:translate(30px,20px)'></div>",
        );
        let mut session = session();
        let tree = session.layout(&doc);
        let quads = node_boxes(tree, id_of(&doc, "t"));
        assert_eq!(quads[0].border.bounds(), (30.0, 20.0, 100.0, 50.0));
        let node = id_of(&doc, "t");
        assert_eq!(hit_test(tree, &doc, 35.0, 25.0), Some(node));
        assert_ne!(hit_test(tree, &doc, 5.0, 5.0), Some(node));
    }

    #[test]
    fn the_topmost_box_wins_and_text_reports_its_element() {
        let doc = parse(
            "<!doctype html><body style='margin:0'>\
             <p id=p style='margin:0'>hello</p>\
             <div id=top style='position:absolute;left:0;top:40px;width:50px;height:50px;z-index:3'></div>\
             <div id=under style='position:absolute;left:0;top:40px;width:50px;height:50px;z-index:1'></div>",
        );
        let mut session = session();
        let tree = session.layout(&doc);
        assert_eq!(hit_test(tree, &doc, 3.0, 3.0), Some(id_of(&doc, "p")));
        assert_eq!(hit_test(tree, &doc, 10.0, 50.0), Some(id_of(&doc, "top")));
    }

    #[test]
    fn pointer_events_none_and_hidden_boxes_let_the_hit_through() {
        let doc = parse(
            "<!doctype html><body style='margin:0'>\
             <div id=back style='width:100px;height:100px'></div>\
             <div id=ghost style='position:absolute;left:0;top:0;width:100px;height:100px;pointer-events:none'></div>\
             <div id=veil style='position:absolute;left:0;top:0;width:100px;height:100px;visibility:hidden'></div>",
        );
        let mut session = session();
        let tree = session.layout(&doc);
        assert_eq!(hit_test(tree, &doc, 10.0, 10.0), Some(id_of(&doc, "back")));
    }

    #[test]
    fn a_point_outside_the_document_hits_nothing() {
        let doc = parse("<!doctype html><body>x");
        let mut session = session();
        let tree = session.layout(&doc);
        assert_eq!(hit_test(tree, &doc, -1.0, 5.0), None);
        assert_eq!(hit_test(tree, &doc, 5.0, 10_000.0), None);
        assert!(hit_test(tree, &doc, 399.0, 299.0).is_some());
    }

    #[test]
    fn overflow_hidden_clips_the_hit_of_its_descendants() {
        let doc = parse(
            "<!doctype html><body style='margin:0'>\
             <div id=clip style='width:50px;height:50px;overflow:hidden'>\
             <div id=big style='width:200px;height:200px'></div></div>",
        );
        let mut session = session();
        let tree = session.layout(&doc);
        assert_eq!(hit_test(tree, &doc, 10.0, 10.0), Some(id_of(&doc, "big")));
        assert_ne!(hit_test(tree, &doc, 100.0, 100.0), Some(id_of(&doc, "big")));
    }
}
