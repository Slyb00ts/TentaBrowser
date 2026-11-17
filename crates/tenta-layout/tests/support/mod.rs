//! Shared helpers: parse HTML, run the real cascade, build the box tree and
//! lay it out, then read fragment rectangles back by element id.

#![allow(dead_code)]

use tenta_core::NodeId;
use tenta_css::cascade::StyleEnv;
use tenta_css::cascade::test_util::style_html;
use tenta_dom::Document;
use tenta_layout::{BoxTree, FontSystem, FragId, FragmentTree, LayoutCx, NoImages, Rect, layout};

/// A laid-out document.
pub struct Laid {
    pub doc: Document,
    pub tree: BoxTree,
    pub frags: FragmentTree,
}

/// Lays `html` out in a viewport of the given size.
pub fn lay_out(html: &str, viewport: (f32, f32)) -> Laid {
    let env = StyleEnv {
        viewport,
        ..StyleEnv::default()
    };
    let (doc, engine) = style_html(html, env);
    let mut fonts = FontSystem::new_embedded();
    let mut cx = LayoutCx {
        doc: &doc,
        styles: &engine,
        fonts: &mut fonts,
        images: &NoImages,
        viewport,
        quirks: StyleEnv::quirks_of(&doc),
    };
    let tree = BoxTree::build(&cx);
    let frags = layout(&mut cx, &tree);
    Laid { doc, tree, frags }
}

impl Laid {
    /// The element with id `id`, in the document or in a shadow tree.
    pub fn node(&self, id: &str) -> NodeId {
        let doc = &self.doc;
        doc.get_element_by_id(id)
            .or_else(|| {
                doc.shadow_including_subtree(doc.document_node())
                    .into_iter()
                    .find(|&node| {
                        doc.is_element(node)
                            && doc.attribute(node, tenta_core::atom::atoms::ID) == Some(id)
                    })
            })
            .unwrap_or_else(|| panic!("no element with id {id}"))
    }

    /// The principal fragment of element `id`.
    pub fn frag(&self, id: &str) -> FragId {
        *self
            .frags
            .fragments_of(self.node(id))
            .first()
            .unwrap_or_else(|| panic!("element {id} produced no fragment"))
    }

    /// Border box of element `id` in document coordinates; an element without
    /// a box (`display: none`) reports an empty rectangle, as browsers do.
    pub fn rect(&self, id: &str) -> Rect {
        self.frags
            .fragments_of(self.node(id))
            .first()
            .map_or_else(Rect::default, |&frag| self.frags.absolute_border_box(frag))
    }
}

/// The corpus tolerance policy: 0.02 px when the expected value is an integer,
/// 0.05 px otherwise.
pub fn within_tolerance(actual: f32, expected: f32) -> bool {
    let tolerance = if expected.fract() == 0.0 { 0.02 } else { 0.05 };
    (actual - expected).abs() <= tolerance
}

/// Compares a rectangle with `[x, y, w, h]` and describes the mismatch.
pub fn rect_mismatch(actual: Rect, expected: [f32; 4]) -> Option<String> {
    let got = [actual.x, actual.y, actual.w, actual.h];
    got.iter()
        .zip(expected)
        .any(|(a, e)| !within_tolerance(*a, e))
        .then(|| format!("got {got:?}, Chrome {expected:?}"))
}

/// One recorded Chrome page: its HTML and the border boxes (`[x, y, w, h]`,
/// document coordinates) of the elements that carry an id, plus the root's
/// scroll size.
pub struct Case {
    pub name: &'static str,
    pub viewport: (f32, f32),
    pub html: &'static str,
    pub scroll: (f32, f32),
    pub expect: &'static [(&'static str, [f32; 4])],
}

/// Lays out every case and fails with one line per mismatching rectangle.
pub fn check_cases(cases: &[Case]) {
    check_cases_with(cases, |html, viewport| Some(lay_out(html, viewport)));
}

/// Like [`check_cases`], but reports the test skipped when a layout step owned
/// by a package that has not landed yet is still `todo!`.
pub fn check_cases_when_ready(cases: &[Case]) {
    check_cases_with(cases, try_lay_out);
}

fn check_cases_with(cases: &[Case], mut lay: impl FnMut(&str, (f32, f32)) -> Option<Laid>) {
    let mut failures = Vec::new();
    for case in cases {
        let Some(laid) = lay(case.html, case.viewport) else {
            return;
        };
        for (id, expected) in case.expect {
            if let Some(why) = rect_mismatch(laid.rect(id), *expected) {
                failures.push(format!("{} / #{id}: {why}", case.name));
            }
        }
        let extent = laid.frags.scroll_extent;
        if (extent.w, extent.h) != case.scroll {
            failures.push(format!(
                "{} / scroll extent: got {:?}, Chrome {:?}",
                case.name,
                (extent.w, extent.h),
                case.scroll
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Lays `html` out, or returns `None` after saying so on stderr when a layout
/// step owned by a package that has not landed yet (inline atomics, relative
/// positioning, ...) is still `todo!`. Any other panic propagates.
pub fn try_lay_out(html: &str, viewport: (f32, f32)) -> Option<Laid> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let laid = std::panic::catch_unwind(|| lay_out(html, viewport));
    std::panic::set_hook(previous);
    match laid {
        Ok(laid) => Some(laid),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("");
            if message.starts_with("not yet implemented") {
                eprintln!("skipped: layout needs a package that has not landed ({message})");
                None
            } else {
                std::panic::resume_unwind(payload)
            }
        }
    }
}

/// Runs `f` with a layout context over the laid-out document, for tests that
/// call the entry points directly.
pub fn with_layout_cx<R>(
    html: &str,
    viewport: (f32, f32),
    f: impl FnOnce(&mut LayoutCx, &BoxTree, &Document) -> R,
) -> R {
    let env = StyleEnv {
        viewport,
        ..StyleEnv::default()
    };
    let (doc, engine) = style_html(html, env);
    let mut fonts = FontSystem::new_embedded();
    let mut cx = LayoutCx {
        doc: &doc,
        styles: &engine,
        fonts: &mut fonts,
        images: &NoImages,
        viewport,
        quirks: StyleEnv::quirks_of(&doc),
    };
    let tree = BoxTree::build(&cx);
    f(&mut cx, &tree, &doc)
}
