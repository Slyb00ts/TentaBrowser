// Expected rectangles recorded from Chrome 154: the union of the first line's getClientRects(), in document coordinates.

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "ib_overflow_clip_keeps_last_line_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:50px;overflow:clip'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [8.0, 0.0, 50.0, 36.0]),
        ],
    },
    Case {
        name: "ib_overflow_x_clip_keeps_last_line_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:50px;overflow-x:clip'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [8.0, 0.0, 50.0, 36.0]),
        ],
    },
    Case {
        name: "ib_overflow_hidden_x_only",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:50px;overflow-x:hidden'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [8.0, 0.0, 50.0, 36.0]),
        ],
    },
    Case {
        name: "ib_contain_layout_bottom_edge",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:50px;contain:layout'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [8.0, 0.0, 50.0, 36.0]),
        ],
    },
    Case {
        name: "ib_contain_paint_bottom_edge",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:50px;contain:paint'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [8.0, 0.0, 50.0, 36.0]),
        ],
    },
    Case {
        name: "ib_contain_content_bottom_edge",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:50px;contain:content'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [8.0, 0.0, 50.0, 36.0]),
        ],
    },
    Case {
        name: "ib_contain_strict_bottom_edge",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:50px;height:40px;contain:strict'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 44.0]),
            ("a", [8.0, 0.0, 50.0, 40.0]),
        ],
    },
    Case {
        name: "ib_contain_size_bottom_edge",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;contain:size;width:50px;height:40px'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [8.0, 0.0, 50.0, 40.0]),
        ],
    },
    Case {
        name: "ib_contain_style_keeps_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:50px;contain:style'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [8.0, 0.0, 50.0, 36.0]),
        ],
    },
    Case {
        name: "ib_percent_min_width",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:200px'><span id=a style='display:inline-block;background:#ccc;min-width:50%'>ab</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 18.0]),
            ("a", [0.0, 0.0, 100.0, 18.0]),
        ],
    },
    Case {
        name: "ib_percent_max_width",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:200px'><span id=a style='display:inline-block;background:#ccc;max-width:50%'>aaa bbb ccc ddd eee fff ggg hhh</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 54.0]),
            ("a", [0.0, 0.0, 100.0, 54.0]),
        ],
    },
    Case {
        name: "ib_percent_max_width_short_content",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:200px'><span id=a style='display:inline-block;background:#ccc;max-width:50%'>ab</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 18.0]),
            ("a", [0.0, 0.0, 17.797, 18.0]),
        ],
    },
    Case {
        name: "ib_percent_min_over_max",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:200px'><span id=a style='display:inline-block;background:#ccc;min-width:60%;max-width:30%'>ab</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 18.0]),
            ("a", [0.0, 0.0, 120.0, 18.0]),
        ],
    },
    Case {
        name: "ib_percent_min_width_border_box",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:200px'><span id=a style='display:inline-block;background:#ccc;box-sizing:border-box;min-width:50%;padding:10px;border:2px solid'>ab</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 42.0]),
            ("a", [0.0, 0.0, 100.0, 42.0]),
        ],
    },
    Case {
        name: "ib_percent_min_width_in_stf_container",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='display:inline-block;background:#ccc;'><span id=a style='display:inline-block;background:#ccc;min-width:50%'>ab</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 17.797, 18.0]),
            ("a", [0.0, 0.0, 17.797, 18.0]),
        ],
    },
    Case {
        name: "ib_percent_max_width_in_stf_container",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='display:inline-block;background:#ccc;'><span id=a style='display:inline-block;background:#ccc;max-width:50%'>aaa bbb ccc ddd</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 117.422, 36.0]),
            ("a", [0.0, 0.0, 58.703, 36.0]),
        ],
    },
    Case {
        name: "canvas_percent_max_width",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:200px'><canvas id=c style='width:300px;height:30px;max-width:50%'></canvas></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 34.0]),
            ("c", [0.0, 0.0, 100.0, 30.0]),
        ],
    },
    Case {
        name: "ib_percent_height_min",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='height:100px'><span id=a style='display:inline-block;background:#ccc;min-height:50%;width:10px'></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 100.0]),
            ("a", [0.0, 0.0, 10.0, 50.0]),
        ],
    },
];
