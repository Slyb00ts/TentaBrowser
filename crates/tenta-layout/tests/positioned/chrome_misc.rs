// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "abs_replaced_canvas_sizes",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><canvas id=a style="width:60px;height:30px;position:absolute;left:10px;right:10px;top:5px"></canvas></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [30.0, 25.0, 60.0, 30.0]),
        ],
    },
    Case {
        name: "abs_display_none_sibling_no_box",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="display:none;position:absolute;left:5px;top:5px;width:20px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [0.0, 0.0, 0.0, 0.0]),
        ],
    },
    Case {
        name: "abs_with_float_computes_none",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;float:right;left:5px;top:5px;width:20px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [25.0, 25.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "abs_inline_display_blockified",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><span id=a style="position:absolute;display:inline;left:5px;top:5px;width:20px;height:20px"></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [25.0, 25.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "sticky_behaves_as_static",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=s style="height:30px"></div><div id=a style="position:sticky;top:0;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 50.0]),
            ("s", [20.0, 20.0, 760.0, 30.0]),
            ("a", [20.0, 50.0, 760.0, 20.0]),
        ],
    },
    Case {
        name: "relative_still_works_with_abs_sibling",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=r style="position:relative;left:10px;top:5px;height:20px"></div><div id=a style="position:absolute;left:0;top:50px;width:10px;height:10px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 20.0]),
            ("r", [30.0, 25.0, 760.0, 20.0]),
            ("a", [0.0, 50.0, 10.0, 10.0]),
        ],
    },
    Case {
        name: "two_abs_siblings",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:0;width:10px;height:10px"></div><div id=b style="position:absolute;right:0;bottom:0;width:20px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 10.0, 10.0]),
            ("b", [300.0, 200.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "abs_deep_nesting_chain",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=q style="position:absolute;left:10px;top:10px;width:200px;height:100px"><div id=r style="position:absolute;left:10px;top:10px;width:100px;height:50px"><div id=a style="position:absolute;right:0;bottom:0;width:20px;height:20px"></div></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("q", [30.0, 30.0, 200.0, 100.0]),
            ("r", [40.0, 40.0, 100.0, 50.0]),
            ("a", [120.0, 70.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "abs_inside_abs_static_nested",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=q style="position:absolute;left:10px;top:10px;width:200px"><div id=s style="height:30px"></div><div id=a style="position:absolute;width:20px;height:20px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("q", [30.0, 30.0, 200.0, 30.0]),
            ("s", [30.0, 30.0, 200.0, 30.0]),
            ("a", [30.0, 60.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "abs_in_relative_in_abs_ancestors",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:absolute;left:50px;top:50px;width:300px;height:200px"><div id=q style="position:relative;left:10px;top:10px;width:100px;height:100px"><div id=a style="position:absolute;left:5px;top:5px;width:20px;height:20px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [50.0, 50.0, 300.0, 200.0]),
            ("q", [60.0, 60.0, 100.0, 100.0]),
            ("a", [65.0, 65.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "vh_units_in_abs",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=a style="position:absolute;left:0;top:0;width:50vw;height:25vh"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [0.0, 0.0, 400.0, 150.0])],
    },
    Case {
        name: "fixed_full_viewport",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=a style="position:fixed;inset:0"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [0.0, 0.0, 800.0, 600.0])],
    },
    Case {
        name: "fixed_centered_with_margin_auto",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=a style="position:fixed;inset:0;margin:auto;width:200px;height:100px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [300.0, 250.0, 200.0, 100.0])],
    },
    Case {
        name: "fixed_static_position",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div style="height:50px"></div><div id=a style="position:fixed;width:40px;height:20px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [0.0, 50.0, 40.0, 20.0])],
    },
    Case {
        name: "fixed_bottom_with_page_taller_than_viewport",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div style="height:2000px"></div><div id=a style="position:fixed;left:0;right:0;bottom:0;height:40px"></div>"#,
        scroll: (800.0, 2000.0),
        expect: &[("a", [0.0, 560.0, 800.0, 40.0])],
    },
    Case {
        name: "abs_in_small_viewport",
        viewport: (600.0, 240.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div>"#,
        scroll: (600.0, 240.0),
        expect: &[("a", [540.0, 200.0, 50.0, 30.0])],
    },
    Case {
        name: "display_contents_wrapper_keeps_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div style="display:contents"><div id=a style="position:absolute;left:5px;top:6px;width:20px;height:20px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [25.0, 26.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "list_item_static",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><ul style="margin:20px;padding-left:30px"><li id=l style="height:20px"></li><li id=m><div id=a style="position:absolute;width:20px;height:10px"></div></li></ul>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("l", [50.0, 20.0, 730.0, 20.0]),
            ("m", [50.0, 40.0, 730.0, 18.0]),
            ("a", [50.0, 40.0, 20.0, 10.0]),
        ],
    },
    Case {
        name: "anonymous_block_between_blocks_static",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px"><div style="height:10px"></div>text<div id=a style="position:absolute;width:20px;height:10px"></div>more<div style="height:10px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 38.0]),
            ("a", [20.0, 48.0, 20.0, 10.0]),
        ],
    },
    Case {
        name: "text_indent_static",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;text-indent:25px">Hi<span id=a style="position:absolute;width:20px;height:10px"></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [60.1094, 20.0, 20.0, 10.0]),
        ],
    },
    Case {
        name: "overflow_scroll_cb_padding_box",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;overflow:scroll;margin:20px;border:4px solid;padding:6px;width:200px;height:100px"><div id=a style="position:absolute;inset:0"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 220.0, 120.0]),
            ("a", [24.0, 24.0, 212.0, 112.0]),
        ],
    },
    Case {
        name: "zero_size_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:0;height:0"><div id=a style="position:absolute;left:5px;top:5px;right:5px;bottom:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [20.0, 20.0, 0.0, 0.0]), ("a", [25.0, 25.0, 0.0, 0.0])],
    },
    Case {
        name: "cb_with_negative_margin",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;margin-left:-10px;width:200px;height:100px"><div id=a style="position:absolute;right:0;bottom:0;width:20px;height:10px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [-10.0, 20.0, 200.0, 100.0]),
            ("a", [170.0, 110.0, 20.0, 10.0]),
        ],
    },
    Case {
        name: "abs_cb_is_flex_container_child_static_row",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="display:flex;margin:20px;width:300px;height:100px"><div id=q style="position:relative;flex:1"><div style="height:30px"></div><div id=a style="position:absolute;width:20px;height:10px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 100.0]),
            ("q", [20.0, 20.0, 300.0, 100.0]),
            ("a", [20.0, 50.0, 20.0, 10.0]),
        ],
    },
    Case {
        name: "abs_text_wraps_in_fixed_width",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:0;width:60px">Hello world again</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 60.0, 54.0]),
        ],
    },
    Case {
        name: "abs_fill_text_height",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:10px;right:10px;top:10px">Hello world</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [30.0, 30.0, 280.0, 18.0]),
        ],
    },
    Case {
        name: "abs_rtl_own_direction_static",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px"><div id=a style="position:absolute;direction:rtl;width:50px;height:10px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 0.0]),
            ("a", [20.0, 20.0, 50.0, 10.0]),
        ],
    },
    Case {
        name: "abs_rtl_cb_right_inset_shrink",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;direction:rtl;margin:20px;width:300px;height:100px"><div id=a style="position:absolute;right:10px;top:0"><div style="width:70px;height:10px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 100.0]),
            ("a", [240.0, 20.0, 70.0, 10.0]),
        ],
    },
    Case {
        name: "abs_rtl_cb_both_auto_inset_shrink",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;direction:rtl;margin:20px;width:300px;height:100px"><div id=s style="height:10px"></div><div id=a style="position:absolute"><div style="width:70px;height:10px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 100.0]),
            ("s", [20.0, 20.0, 300.0, 10.0]),
            ("a", [250.0, 30.0, 70.0, 10.0]),
        ],
    },
];
