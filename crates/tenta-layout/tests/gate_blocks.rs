//! The block-only entries of the M1 corpus gate (`render-fixtures/M1-GATE.toml`):
//! the coloured box of each check is the border box of one element, so its
//! rectangle is compared with the fixture's `color-checks.json` values.

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

mod support;

use support::{lay_out, rect_mismatch};

/// `(selector, [x, y, w, h])`.
type Check = (&'static str, [f32; 4]);

/// The checks of each fixture, from the fixture's colour checks.
const FIXTURES: &[(&str, &[Check])] = &[
    (
        "background-shorthand-reset",
        &[
            ("#underlay", [0.0, 0.0, 300.0, 200.0]),
            ("#content", [50.0, 50.0, 200.0, 100.0]),
        ],
    ),
    (
        "block-auto-margins",
        &[
            (".center-fixed", [300.0, 20.0, 300.0, 40.0]),
            (".center-capped", [200.0, 100.0, 500.0, 40.0]),
            (".push-end", [650.0, 180.0, 200.0, 40.0]),
            (".push-start", [50.0, 260.0, 200.0, 40.0]),
            (".auto-width", [0.0, 340.0, 900.0, 40.0]),
        ],
    ),
    (
        "contextual-css-math",
        &[
            (".min-width", [0.0, 0.0, 225.0, 40.0]),
            (".min-margin", [225.0, 60.0, 100.0, 40.0]),
            (".clamped", [0.0, 120.0, 270.0, 40.0]),
            (".calculated", [122.0, 180.0, 100.0, 40.0]),
            (".inner", [60.0, 240.0, 20.0, 40.0]),
            (".rounded", [0.0, 300.0, 250.0, 40.0]),
        ],
    ),
    (
        "font-shorthand",
        &[
            ("#hero", [0.0, 0.0, 200.0, 80.0]),
            ("#reset", [0.0, 80.0, 200.0, 40.0]),
        ],
    ),
    (
        "relative-box-edges",
        &[
            (".vh-margin", [0.0, 150.0, 100.0, 40.0]),
            (".vw-margin", [90.0, 200.0, 100.0, 40.0]),
            (".em-inner", [40.0, 280.0, 20.0, 40.0]),
            (".rem-inner", [36.0, 360.0, 20.0, 40.0]),
            (".vmin-margin", [90.0, 440.0, 100.0, 40.0]),
            (".vmax-margin", [100.0, 520.0, 100.0, 40.0]),
        ],
    ),
    (
        "direction-rtl",
        &[(".block > div", [850.0, 120.0, 50.0, 40.0])],
    ),
    (
        "block-whitespace-margin-collapse",
        &[(".first", [0.0, 0.0, 900.0, 20.0])],
    ),
];

#[test]
fn block_only_gate_checks_match_the_corpus() {
    let corpus = concat!(env!("CARGO_MANIFEST_DIR"), "/../../render-fixtures");
    let mut failures = Vec::new();
    for (fixture, checks) in FIXTURES {
        let html = std::fs::read_to_string(format!("{corpus}/{fixture}/index.html"))
            .unwrap_or_else(|e| panic!("fixture {fixture}: {e}"));
        let laid = lay_out(&html, (900.0, 1000.0));
        for (selector, expected) in *checks {
            let node = laid
                .doc
                .query_selector(laid.doc.document_node(), selector)
                .unwrap_or_else(|e| panic!("{fixture}: selector {selector}: {e:?}"))
                .unwrap_or_else(|| panic!("{fixture}: nothing matches {selector}"));
            let rect = laid
                .frags
                .fragments_of(node)
                .first()
                .map(|&f| laid.frags.absolute_border_box(f))
                .unwrap_or_default();
            if let Some(why) = rect_mismatch(rect, *expected) {
                failures.push(format!("{fixture} {selector}: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
