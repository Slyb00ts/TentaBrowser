// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "ib_empty_on_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:40px;height:30px'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("a", [8.0, 0.0, 40.0, 30.0]),
        ],
    },
    Case {
        name: "ib_text_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;padding:4px;border:2px solid;font-size:24px'>Big</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [8.0, 0.0, 46.688, 40.0]),
        ],
    },
    Case {
        name: "ib_two_lines_last_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:60px'>one<br>two three</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 54.0]),
            ("a", [8.0, 0.0, 60.0, 54.0]),
        ],
    },
    Case {
        name: "ib_overflow_hidden_bottom_edge",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:60px;overflow:hidden'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [8.0, 0.0, 60.0, 36.0]),
        ],
    },
    Case {
        name: "ib_margins",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:40px;height:20px;margin:7px 5px 3px 9px'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("a", [17.0, 7.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "ib_negative_margin_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:40px;height:20px;margin-bottom:-6px'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("a", [8.0, 0.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "ib_box_sizing_border",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;box-sizing:border-box;width:50px;height:30px;padding:5px;border:3px solid'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("a", [8.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "ib_shrink_to_fit_wraps",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:100px'><span id=a style='display:inline-block;background:#ccc;'>aaa bbb ccc ddd eee</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 100.0, 36.0]),
            ("a", [0.0, 0.0, 100.0, 36.0]),
        ],
    },
    Case {
        name: "ib_shrink_to_fit_short",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:300px'><span id=a style='display:inline-block;background:#ccc;'>aaa bbb</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 18.0]),
            ("a", [0.0, 0.0, 57.844, 18.0]),
        ],
    },
    Case {
        name: "ib_percent_width",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:200px'><span id=a style='display:inline-block;background:#ccc;width:50%;height:10px'></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 18.0]),
            ("a", [0.0, 4.0, 100.0, 10.0]),
        ],
    },
    Case {
        name: "ib_nested",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>a<span id=a style='display:inline-block;background:#ccc;padding:3px'>b<span id=b style='display:inline-block;background:#ccc;padding:2px;font-size:30px'>c</span>d</span>e</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 44.0]),
            ("a", [8.906, 0.0, 42.812, 44.0]),
            ("b", [20.812, 3.0, 19.0, 38.0]),
        ],
    },
    Case {
        name: "ib_in_stf_container",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='display:inline-block;background:#ccc;'><span id=a style='display:inline-block;background:#ccc;width:80px;padding:6px;border:2px solid;margin:0 4px'></span><span id=b style='display:inline-block;background:#ccc;padding:0 10px'>text</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 149.797, 20.0]),
            ("a", [4.0, 0.0, 96.0, 16.0]),
            ("b", [104.0, 2.0, 45.797, 18.0]),
        ],
    },
    Case {
        name: "ib_line_height_grows",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;height:50px;width:20px'></span><span id=b style='display:inline-block;background:#ccc;height:10px;width:20px'></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 54.0]),
            ("a", [8.0, 0.0, 20.0, 50.0]),
            ("b", [28.0, 40.0, 20.0, 10.0]),
        ],
    },
    Case {
        name: "ib_wrap_between_atomics",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:100px'><span id=a style='display:inline-block;background:#ccc;width:60px;height:10px'></span><span id=b style='display:inline-block;background:#ccc;width:60px;height:10px'></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 100.0, 36.0]),
            ("a", [0.0, 4.0, 60.0, 10.0]),
            ("b", [0.0, 22.0, 60.0, 10.0]),
        ],
    },
    Case {
        name: "canvas_in_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>text<canvas id=c style='width:50px;height:30px'></canvas>text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("c", [25.797, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "canvas_default_size",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>text<canvas id=c></canvas>text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 154.0]),
            ("c", [25.797, 0.0, 300.0, 150.0]),
        ],
    },
    Case {
        name: "canvas_margins_border",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>text<canvas id=c style='width:50px;height:30px;margin:4px 6px;border:2px solid;padding:3px'></canvas>text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 52.0]),
            ("c", [31.797, 4.0, 60.0, 40.0]),
        ],
    },
    Case {
        name: "canvas_only",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><canvas id=c style='width:50px;height:30px'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("c", [0.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "canvas_line_height_20",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='line-height:20px'>text<canvas id=c style='width:50px;height:30px'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 35.0]),
            ("c", [25.797, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "canvas_percent_width",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:200px'><canvas id=c style='width:50%;height:10px'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 18.0]),
            ("c", [0.0, 4.0, 100.0, 10.0]),
        ],
    },
    Case {
        name: "break_between_atomic_and_full_stop",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:60px'><canvas id=c style='width:58px;height:10px'></canvas>.</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 60.0, 36.0]), ("c", [0.0, 4.0, 58.0, 10.0])],
    },
    Case {
        name: "break_before_letter",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:60px'><canvas id=c style='width:58px;height:10px'></canvas>x</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 60.0, 36.0]), ("c", [0.0, 4.0, 58.0, 10.0])],
    },
    Case {
        name: "break_between_open_paren_and_atomic",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:60px'>(<canvas id=c style='width:58px;height:10px'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 60.0, 36.0]),
            ("c", [0.0, 22.0, 58.0, 10.0]),
        ],
    },
    Case {
        name: "break_after_space_before_atomic",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:60px'>aa <canvas id=c style='width:58px;height:10px'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 60.0, 36.0]),
            ("c", [0.0, 22.0, 58.0, 10.0]),
        ],
    },
    Case {
        name: "space_after_atomic",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:70px'><canvas id=c style='width:58px;height:10px'></canvas> yy</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 70.0, 36.0]), ("c", [0.0, 4.0, 58.0, 10.0])],
    },
    Case {
        name: "break_between_atomic_and_comma_in_span",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:60px'><canvas id=c style='width:58px;height:10px'></canvas><span id=s>,</span>z</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 60.0, 36.0]),
            ("c", [0.0, 4.0, 58.0, 10.0]),
            ("s", [0.0, 18.0, 4.453, 17.0]),
        ],
    },
    Case {
        name: "legacy_inline_block_in_p",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><p id=p><span id=a style='display:inline-block;background:#ccc;width:40px;height:40px;vertical-align:middle'></span> text</p></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 16.0, 800.0, 40.0]),
            ("a", [0.0, 16.0, 40.0, 40.0]),
        ],
    },
];
