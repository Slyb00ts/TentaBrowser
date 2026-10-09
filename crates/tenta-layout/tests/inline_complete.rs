//! Atomic inlines, `vertical-align`, bidirectional lines and `text-overflow`
//! through the whole pipeline: cascade, box tree, block and inline layout.
//!
//! Rectangles were recorded in headless Chrome 154 with the bundled faces
//! declared through `@font-face`, so metrics and advances match.

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "inline_complete/atomic_cases.rs"]
mod atomic_cases;
#[path = "inline_complete/atomic_more_cases.rs"]
mod atomic_more_cases;
#[path = "inline_complete/baseline_and_limit_cases.rs"]
mod baseline_and_limit_cases;
#[path = "inline_complete/bidi_box_cases.rs"]
mod bidi_box_cases;
#[path = "inline_complete/bidi_cases.rs"]
mod bidi_cases;
#[path = "inline_complete/bidi_class_cases.rs"]
mod bidi_class_cases;
#[path = "inline_complete/quirks_cases.rs"]
mod quirks_cases;
mod support;
#[path = "inline_complete/valign_cases.rs"]
mod valign_cases;

use support::{Case, check_cases};

#[test]
fn atomic_inlines_match_chrome() {
    check_cases(atomic_cases::CASES);
}

#[test]
fn vertical_align_matches_chrome() {
    check_cases(valign_cases::CASES);
}

#[test]
fn bidirectional_lines_match_chrome() {
    check_cases(bidi_cases::CASES);
}

#[test]
fn bidirectional_boxes_and_objects_match_chrome() {
    check_cases(bidi_box_cases::CASES);
}

/// Where Chrome puts the ellipsis. The DOM does not report truncation, so each
/// expectation is read off a Chrome 154 screenshot: the ink edge of the
/// ellipsis nearest the cut (right edge in left-to-right blocks, left edge in
/// right-to-left ones) less the offset of that edge from the origin of a lone
/// U+2026 in the same font. Pixels quantise the ink, hence the tolerance.
struct Ellipsis {
    name: &'static str,
    html: &'static str,
    x: f32,
}

const ELLIPSES: &[Ellipsis] = &[
    Ellipsis {
        name: "basic",
        html: "<div id=e class=e>The quick brown fox jumps over</div>",
        x: 83.0,
    },
    Ellipsis {
        name: "straddling_inline_box",
        html: "<div id=e class=e><span>The quick</span> brown fox jumps over</div>",
        x: 83.0,
    },
    Ellipsis {
        name: "right_to_left",
        html: "<div id=e class=e style='direction:rtl'>The quick brown fox jumps over</div>",
        x: 1.0,
    },
    Ellipsis {
        name: "narrower_than_the_ellipsis",
        html: "<div id=e class=e style='width:6px'>The quick brown fox jumps over</div>",
        x: 0.0,
    },
    Ellipsis {
        name: "straddling_atomic_inline_is_dropped",
        html: "<div id=e class=e>ab<span style='display:inline-block;width:60px;height:8px;background:#000'></span>cd efgh</div>",
        x: 78.0,
    },
    Ellipsis {
        name: "padding",
        html: "<div id=e class=e style='padding-left:10px'>The quick brown fox jumps over</div>",
        x: 93.0,
    },
    Ellipsis {
        name: "large_font",
        html: "<div id=e class=e style='font-size:24px'>The quick brown fox jumps over</div>",
        x: 75.0,
    },
    Ellipsis {
        name: "letter_spacing",
        html: "<div id=e class=e style='letter-spacing:2px'>The quick brown fox jumps over</div>",
        x: 77.0,
    },
    Ellipsis {
        name: "overflow_auto",
        html: "<div id=e class=e style='overflow:auto'>The quick brown fox jumps over</div>",
        x: 83.0,
    },
    Ellipsis {
        name: "text_indent",
        html: "<div id=e class=e style='text-indent:15px'>The quick brown fox jumps over</div>",
        x: 76.0,
    },
    Ellipsis {
        name: "nested_inline_boxes",
        html: "<div id=e class=e>The <b>quick <i>brown fox</i> jumps</b> over</div>",
        x: 78.0,
    },
    Ellipsis {
        name: "hebrew_at_the_end_of_a_left_to_right_line",
        html: "<div id=e class=e style=\"font-family:'DejaVu Sans'\">abc אבג דהו וזח אבג</div>",
        x: 82.0,
    },
    Ellipsis {
        name: "right_to_left_hebrew",
        html: "<div id=e class=e style=\"font-family:'DejaVu Sans';direction:rtl\">אבג דהו וזח אבג דהו וזח</div>",
        x: 3.0,
    },
    Ellipsis {
        name: "decomposed_accents_stay_whole",
        html: "<div id=e class=e>e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;</div>",
        x: 80.0,
    },
    Ellipsis {
        name: "decomposed_accents_with_letter_spacing",
        html: "<div id=e class=e style='letter-spacing:1px'>e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;</div>",
        x: 79.0,
    },
    Ellipsis {
        name: "right_to_left_decomposed_accents",
        html: "<div id=e class=e style='direction:rtl'>e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;e&#x301;</div>",
        x: 4.0,
    },
    Ellipsis {
        name: "right_to_left_straddling_atomic_inline_is_dropped",
        html: "<div id=e class=e style='direction:rtl'>ab<span style='display:inline-block;width:60px;height:8px;background:#000'></span>cd efgh</div>",
        x: 32.0,
    },
    Ellipsis {
        name: "preserved_white_space",
        html: "<div id=e class=e style='white-space:pre'>The  quick  brown  fox  jumps</div>",
        x: 83.0,
    },
];

/// The x of the generated ellipsis in document coordinates, if the line has one.
fn ellipsis_x(laid: &support::Laid) -> Option<f32> {
    (0..laid.frags.len() as u32).find_map(|i| {
        let id = tenta_layout::FragId(i);
        let f = laid.frags.get(id);
        let generated = matches!(&f.kind, tenta_layout::FragmentKind::Text(t) if t.text_range == (0..0) && !t.runs.is_empty());
        generated.then(|| laid.frags.absolute_border_box(id).x)
    })
}

#[test]
fn ellipsis_sits_where_chrome_puts_it() {
    let style = "<style>html,body{margin:0}body{font:16px 'Liberation Sans'}.e{width:100px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}</style>";
    let mut failures = Vec::new();
    for case in ELLIPSES {
        let html = format!("<!doctype html><meta charset=utf-8>{style}{}", case.html);
        let laid = support::lay_out(&html, (800.0, 600.0));
        match ellipsis_x(&laid) {
            Some(x) if (x - case.x).abs() <= 0.6 => {}
            other => failures.push(format!("{}: got {other:?}, Chrome {}", case.name, case.x)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn text_that_fits_gets_no_ellipsis() {
    let html = "<!doctype html><style>body{margin:0;font:16px 'Liberation Sans'}.e{width:100px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}</style><div id=e class=e>short</div><div id=v style='width:20px;white-space:nowrap;text-overflow:ellipsis'>not clipped at all</div>";
    let laid = support::lay_out(html, (800.0, 600.0));
    assert_eq!(ellipsis_x(&laid), None);
}

#[test]
fn quirks_mode_atomics_match_chrome() {
    check_cases(quirks_cases::CASES);
}

#[test]
fn atomic_inline_sizing_matches_chrome() {
    check_cases(atomic_more_cases::CASES);
}

#[test]
fn baselines_and_percentage_limits_match_chrome() {
    check_cases(baseline_and_limit_cases::CASES);
}

#[test]
fn brackets_digits_and_signs_match_chrome() {
    check_cases(bidi_class_cases::CASES);
}
