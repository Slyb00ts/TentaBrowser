// Expected rectangles recorded from Chrome 154 in quirks mode (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "quirks_canvas_only",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><canvas id=c style='width:50px;height:30px'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 30.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_canvas_in_text",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>text<canvas id=c style='width:50px;height:30px'></canvas>text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("c", [25.797, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_ib_empty_only",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=a style='display:inline-block;background:#ccc;width:40px;height:30px'></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 30.0]),
            ("a", [0.0, 0.0, 40.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_ib_text",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;padding:4px;font-size:24px'>Big</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [8.0, 0.0, 42.688, 36.0]),
        ],
    },
    Case {
        name: "quirks_valign_top_only",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=a style='display:inline-block;background:#ccc;width:40px;height:30px;vertical-align:top'></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 30.0]),
            ("a", [0.0, 0.0, 40.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_span_valign_super",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:super;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 19.328]),
            ("s", [40.906, 0.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "quirks_empty_span_valign",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=s style='vertical-align:super;font-size:30px'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [8.0, -19.328, 0.0, 33.0]),
        ],
    },
    Case {
        name: "quirks_only_padded_empty_span",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=s style='padding:2px'></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [0.0, -2.0, 4.0, 21.0]),
        ],
    },
    Case {
        name: "quirks_atomic_and_padded_span",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><canvas id=c style='width:50px;height:30px'></canvas><span id=s style='padding:2px'></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
            ("s", [50.0, 14.0, 4.0, 21.0]),
        ],
    },
    Case {
        name: "quirks_br_only",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><br></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 800.0, 18.0])],
    },
    Case {
        name: "quirks_atomic_br",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><canvas id=c style='width:50px;height:30px'></canvas><br></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 30.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_atomic_then_space_text",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><canvas id=c style='width:50px;height:30px'></canvas> </div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 30.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_two_atomics_wrap",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:60px'><canvas id=c style='width:50px;height:30px'></canvas><canvas id=d style='width:50px;height:20px'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 60.0, 50.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
            ("d", [0.0, 30.0, 50.0, 20.0]),
        ],
    },
    Case {
        name: "quirks_atomic_line_then_text_line",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:60px'><canvas id=c style='width:50px;height:30px'></canvas>text more</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 60.0, 66.0]), ("c", [0.0, 0.0, 50.0, 30.0])],
    },
    Case {
        name: "quirks_line_height_20_atomic",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='line-height:20px'><canvas id=c style='width:50px;height:10px'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 10.0]),
            ("c", [0.0, 0.0, 50.0, 10.0]),
        ],
    },
    Case {
        name: "quirks_ib_with_text_only",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=a style='display:inline-block;background:#ccc;'>word</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("a", [0.0, 0.0, 34.688, 18.0]),
        ],
    },
    Case {
        name: "quirks_valign_middle_only_atomic",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><canvas id=c style='width:50px;height:30px;vertical-align:middle'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 30.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_atomic_space_atomic",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><canvas id=c style='width:50px;height:30px'></canvas> <canvas id=d style='width:50px;height:30px'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
            ("d", [54.453, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_atomic_nbsp",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><canvas id=c style='width:50px;height:30px'></canvas>&nbsp;</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_atomic_pre_space",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='white-space:pre'><canvas id=c style='width:50px;height:30px'></canvas> </div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_atomic_in_span_no_text",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=s><canvas id=c style='width:50px;height:30px'></canvas></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 30.0]),
            ("s", [0.0, 16.0, 50.0, 17.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_atomic_in_padded_span",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=s style='padding:1px'><canvas id=c style='width:50px;height:30px'></canvas></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [0.0, 15.0, 52.0, 19.0]),
            ("c", [1.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "quirks_atomic_in_span_with_text",
        viewport: (800.0, 600.0),
        html: r#"<html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=s>t<canvas id=c style='width:50px;height:30px'></canvas></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [0.0, 16.0, 54.453, 17.0]),
            ("c", [4.453, 0.0, 50.0, 30.0]),
        ],
    },
];
