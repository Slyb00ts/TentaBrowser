// Expected rectangles recorded from Chrome 154: the union of the first line's getClientRects(), in document coordinates.

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "ib_min_width",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=a style='display:inline-block;background:#ccc;min-width:120px'>ab</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("a", [0.0, 0.0, 120.0, 18.0]),
        ],
    },
    Case {
        name: "ib_max_width_shrinks_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=a style='display:inline-block;background:#ccc;max-width:60px'>aaa bbb ccc ddd</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [0.0, 0.0, 60.0, 36.0]),
        ],
    },
    Case {
        name: "ib_unbreakable_word_overflows_container",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:50px'><span id=a style='display:inline-block;background:#ccc;'>supercalifragilistic</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 50.0, 18.0]),
            ("a", [0.0, 0.0, 126.266, 18.0]),
        ],
    },
    Case {
        name: "ib_nested_stf_padding",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=a style='display:inline-block;background:#ccc;padding:5px;border:2px solid'><span id=b style='display:inline-block;background:#ccc;padding:3px;margin:0 4px'>inner text</span></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 38.0]),
            ("a", [0.0, 0.0, 93.812, 38.0]),
            ("b", [11.0, 7.0, 71.812, 24.0]),
        ],
    },
    Case {
        name: "ib_block_child_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:80px'><div style='height:20px'></div><div id=d style='font-size:24px'>two</div></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 48.0]),
            ("a", [8.0, 0.0, 80.0, 48.0]),
            ("d", [8.0, 20.0, 80.0, 28.0]),
        ],
    },
    Case {
        name: "ib_nested_inline_block_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;padding:4px'>a<span id=b style='display:inline-block;background:#ccc;font-size:30px'>b</span></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 42.0]),
            ("a", [8.0, 0.0, 33.594, 42.0]),
            ("b", [20.906, 4.0, 16.688, 34.0]),
        ],
    },
    Case {
        name: "ib_border_only_empty",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;border:5px solid'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("a", [8.0, 4.0, 10.0, 10.0]),
        ],
    },
    Case {
        name: "ib_line_height_own",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;line-height:40px'>two<br>lines</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 80.0]),
            ("a", [8.0, 0.0, 32.906, 80.0]),
        ],
    },
    Case {
        name: "ib_text_align_center_inside",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:300px;text-align:center'>x<span id=a style='display:inline-block;background:#ccc;width:100px;text-align:right'>r</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 18.0]),
            ("a", [100.0, 0.0, 100.0, 18.0]),
        ],
    },
    Case {
        name: "ib_inherits_text_align_center_shrinks",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:300px;text-align:center'><span id=a style='display:inline-block;background:#ccc;'>one two<br>three</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 36.0]),
            ("a", [121.969, 0.0, 56.047, 36.0]),
        ],
    },
    Case {
        name: "ib_height_percent_of_definite",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='height:100px'><span id=a style='display:inline-block;background:#ccc;height:50%;width:10px'></span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 100.0]),
            ("a", [0.0, 0.0, 10.0, 50.0]),
        ],
    },
    Case {
        name: "ib_height_percent_of_auto",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=a style='display:inline-block;background:#ccc;height:50%;width:10px'>x</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("a", [0.0, 0.0, 10.0, 18.0]),
        ],
    },
    Case {
        name: "ib_overflow_auto_bottom_edge",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;overflow:auto;width:50px;height:30px'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("a", [8.0, 0.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "ib_vertical_align_top_two_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;vertical-align:top'>one<br>two</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [8.0, 0.0, 26.703, 36.0]),
        ],
    },
    Case {
        name: "ib_relative_offset",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:30px;height:20px;position:relative;top:5px;left:7px'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.0]),
            ("a", [15.0, 5.0, 30.0, 20.0]),
        ],
    },
    Case {
        name: "img_attrs_size",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>text<img id=i width=40 height=20>text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.0]),
            ("i", [25.797, 0.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "img_only_width_attr",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>text<img id=i width=40>text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("i", [25.797, 14.0, 40.0, 0.0]),
        ],
    },
    Case {
        name: "ib_rtl_margins",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:300px;direction:rtl'>x<span id=a style='display:inline-block;background:#ccc;width:40px;height:10px;margin:0 3px 0 9px'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 18.0]),
            ("a", [249.0, 4.0, 40.0, 10.0]),
        ],
    },
    Case {
        name: "ib_rtl_stf",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:300px;direction:rtl'><span id=a style='display:inline-block;background:#ccc;'>aaa bbb</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 18.0]),
            ("a", [242.156, 0.0, 57.844, 18.0]),
        ],
    },
    Case {
        name: "ib_inline_flex_like_block_child_margins",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;'><div style='margin:5px 0;height:10px'></div></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 800.0, 24.0]), ("a", [8.0, 0.0, 0.0, 20.0])],
    },
    Case {
        name: "ib_white_space_nowrap_container",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:50px;white-space:nowrap'><span id=a style='display:inline-block;background:#ccc;'>aaa bbb ccc</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 50.0, 18.0]),
            ("a", [0.0, 0.0, 86.281, 18.0]),
        ],
    },
    Case {
        name: "ib_in_inline_with_padding",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>a<span id=o style='padding:6px;border:1px solid'>b<span id=a style='display:inline-block;background:#ccc;width:20px;height:20px'></span>c</span>d</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.0]),
            ("o", [8.906, -1.0, 50.906, 31.0]),
            ("a", [24.812, 0.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "ib_space_collapsing_around",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>a <span id=a style='display:inline-block;background:#ccc;width:20px;height:10px'></span> <span id=b style='display:inline-block;background:#ccc;width:20px;height:10px'></span> d</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("a", [13.344, 4.0, 20.0, 10.0]),
            ("b", [37.797, 4.0, 20.0, 10.0]),
        ],
    },
    Case {
        name: "relative_inline_percent_top_definite_height",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='height:100px;width:200px'>x<span id=s style='position:relative;top:10%;left:25%'>rel</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 100.0]),
            ("s", [58.0, 10.0, 17.781, 17.0]),
        ],
    },
    Case {
        name: "relative_inline_percent_top_auto_height",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:200px'>x<span id=s style='position:relative;top:10%;left:25%'>rel</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 18.0]),
            ("s", [58.0, 0.0, 17.781, 17.0]),
        ],
    },
    Case {
        name: "relative_inline_px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=s style='position:relative;top:3px;left:-2px'>rel</span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [6.0, 3.0, 17.781, 17.0]),
        ],
    },
];
