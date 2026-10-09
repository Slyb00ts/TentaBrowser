//! Element rects of `checks.json` against our fragment tree.
//!
//! Chromium lays out in 1/64 px `LayoutUnit` and snaps; we use `f32`. Rects
//! whose reference values are all integers therefore have to match within
//! 0.02 px per axis, other rects within 0.05 px. Rects of elements that hold
//! inline text depend on font metrics we do not share with Chromium, so they
//! get 1 px for position and 2 px or 3% for size. They are reported, never
//! gated.

use serde_json::Value;
use tenta_dom::Document;
use tenta_render::RenderSession;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    IntegerRect,
    FractionalRect,
    TextDependent,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::IntegerRect => "integer",
            Class::FractionalRect => "fractional",
            Class::TextDependent => "text-dependent",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ElementResult {
    pub selector: String,
    pub class: Class,
    pub expected: [f64; 4],
    /// `None` when the selector matched nothing.
    pub actual: Option<[f64; 4]>,
    pub passed: bool,
}

fn within(class: Class, expected: [f64; 4], actual: [f64; 4]) -> bool {
    (0..4).all(|i| {
        let delta = (expected[i] - actual[i]).abs();
        match (class, i >= 2) {
            (Class::IntegerRect, _) => delta <= 0.02,
            (Class::FractionalRect, _) => delta <= 0.05,
            (Class::TextDependent, false) => delta <= 1.0,
            (Class::TextDependent, true) => delta <= 2.0f64.max(expected[i].abs() * 0.03),
        }
    })
}

fn holds_text(doc: &Document, node: tenta_core::NodeId) -> bool {
    doc.children(node)
        .filter_map(|child| doc.text_data(child))
        .any(|text| !text.trim().is_empty())
}

pub fn evaluate_element_checks(
    doc: &Document,
    session: &mut RenderSession,
    checks: &[Value],
) -> Vec<ElementResult> {
    let Some(root) = doc.document_element() else {
        return Vec::new();
    };
    let frags = session.layout(doc);
    checks
        .iter()
        .filter(|check| check["kind"] == "element")
        .map(|check| {
            let selector = check["selector"].as_str().expect("a selector").to_owned();
            let number = |key: &str| check[key].as_f64().expect("a number");
            let expected = [number("x"), number("y"), number("width"), number("height")];
            let node = doc.query_selector(root, &selector).ok().flatten();
            let class = if node.is_some_and(|node| holds_text(doc, node)) {
                Class::TextDependent
            } else if expected.iter().all(|v| v.fract() == 0.0) {
                Class::IntegerRect
            } else {
                Class::FractionalRect
            };
            let actual = node.map(|node| {
                let mut union: Option<[f64; 4]> = None;
                for &frag in frags.fragments_of(node) {
                    let r = frags.absolute_border_box(frag);
                    let b = [
                        f64::from(r.x),
                        f64::from(r.y),
                        f64::from(r.x + r.w),
                        f64::from(r.y + r.h),
                    ];
                    union = Some(union.map_or(b, |u| {
                        [
                            u[0].min(b[0]),
                            u[1].min(b[1]),
                            u[2].max(b[2]),
                            u[3].max(b[3]),
                        ]
                    }));
                }
                // An element without a box (`display: none`) reports an empty
                // rect at the origin, as `getBoundingClientRect` does.
                union.map_or([0.0; 4], |u| [u[0], u[1], u[2] - u[0], u[3] - u[1]])
            });
            ElementResult {
                passed: actual.is_some_and(|actual| within(class, expected, actual)),
                selector,
                class,
                expected,
                actual,
            }
        })
        .collect()
}
