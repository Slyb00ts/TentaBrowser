// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "fill_between_insets",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:10px;right:20px;top:5px;bottom:15px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [30.0, 25.0, 270.0, 180.0]),
        ],
    },
    Case {
        name: "inset_zero_fills_padding_box",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;padding:7px;border:3px solid;width:300px;height:200px"><div id=a style="position:absolute;inset:0"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 320.0, 220.0]),
            ("a", [23.0, 23.0, 314.0, 214.0]),
        ],
    },
    Case {
        name: "inset_shorthand_four_values",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;inset:10px 20px 30px 40px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [60.0, 30.0, 240.0, 160.0]),
        ],
    },
    Case {
        name: "fill_with_margins_padding_border",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;inset:10px;margin:3px 4px 5px 6px;padding:2px 3px;border:1px solid"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [36.0, 33.0, 270.0, 172.0]),
        ],
    },
    Case {
        name: "fill_border_box_sizing",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;inset:10px;padding:5px;border:2px solid;box-sizing:border-box"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [30.0, 30.0, 280.0, 180.0]),
        ],
    },
    Case {
        name: "over_constrained_ltr_ignores_right",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:10px;right:10px;width:100px;height:20px;top:0"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [30.0, 20.0, 100.0, 20.0]),
        ],
    },
    Case {
        name: "over_constrained_rtl_ignores_left",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px;direction:rtl"><div id=a style="position:absolute;left:10px;right:10px;width:100px;height:20px;top:0"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [210.0, 20.0, 100.0, 20.0]),
        ],
    },
    Case {
        name: "over_constrained_vertical_ignores_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:10px;bottom:10px;height:40px;width:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 30.0, 20.0, 40.0]),
        ],
    },
    Case {
        name: "auto_margins_center_both_axes",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;inset:0;margin:auto;width:100px;height:50px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [120.0, 95.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "auto_margin_left_only",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;inset:0;margin-left:auto;width:100px;height:50px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [220.0, 20.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "auto_margin_right_only",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;inset:0;margin-right:auto;width:100px;height:50px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "auto_margin_top_only",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;inset:0;margin-top:auto;width:100px;height:50px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 170.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "auto_margins_negative_free_ltr",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:100px;height:100px"><div id=a style="position:absolute;inset:0;margin:auto;width:150px;height:130px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 100.0]),
            ("a", [20.0, 5.0, 150.0, 130.0]),
        ],
    },
    Case {
        name: "auto_margins_negative_free_rtl",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:100px;height:100px;direction:rtl"><div id=a style="position:absolute;inset:0;margin:auto;width:150px;height:130px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 100.0]),
            ("a", [-30.0, 5.0, 150.0, 130.0]),
        ],
    },
    Case {
        name: "auto_margins_with_fixed_margin_other_side",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;inset:0;margin:auto 30px auto auto;width:100px;height:50px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [190.0, 95.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "right_bottom_only",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:20px;width:60px;height:40px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [250.0, 160.0, 60.0, 40.0]),
        ],
    },
    Case {
        name: "negative_insets",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:50px;width:300px;height:200px"><div id=a style="position:absolute;left:-20px;top:-30px;width:60px;height:40px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [50.0, 50.0, 300.0, 200.0]),
            ("a", [30.0, 20.0, 60.0, 40.0]),
        ],
    },
    Case {
        name: "left_only_shrink_to_fit_blocks",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:10px;top:10px"><div style="width:120px;height:20px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [30.0, 30.0, 120.0, 20.0]),
        ],
    },
    Case {
        name: "right_only_shrink_to_fit",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;top:10px"><div style="width:120px;height:20px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [190.0, 30.0, 120.0, 20.0]),
        ],
    },
    Case {
        name: "shrink_to_fit_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:0">Hello world</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 79.1406, 18.0]),
        ],
    },
    Case {
        name: "shrink_to_fit_text_wraps_at_available",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:100px;height:200px"><div id=a style="position:absolute;left:0;top:0">Hello world and more words here</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 200.0]),
            ("a", [20.0, 20.0, 100.0, 54.0]),
        ],
    },
    Case {
        name: "shrink_to_fit_available_reduced_by_left",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:100px;height:200px"><div id=a style="position:absolute;left:60px;top:0">Hello world</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 200.0]),
            ("a", [80.0, 20.0, 40.0, 36.0]),
        ],
    },
    Case {
        name: "shrink_to_fit_with_padding_margin",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:0;padding:4px 6px;margin:3px 5px;border:2px solid">Hello</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [25.0, 23.0, 52.4688, 30.0]),
        ],
    },
    Case {
        name: "shrink_to_fit_inline_block_children",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:0"><span id=s style="display:inline-block;width:70px;height:10px"></span><span id=t style="display:inline-block;width:80px;height:10px"></span></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 150.0, 18.0]),
            ("s", [20.0, 24.0, 70.0, 10.0]),
            ("t", [90.0, 24.0, 80.0, 10.0]),
        ],
    },
    Case {
        name: "fit_content_keyword",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:0;width:fit-content"><div style="width:90px;height:10px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 90.0, 10.0]),
        ],
    },
    Case {
        name: "min_width_beats_fill",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:10px;right:10px;top:0;min-width:320px;height:10px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [30.0, 20.0, 320.0, 10.0]),
        ],
    },
    Case {
        name: "max_width_limits_fill_and_recenters_margins",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;right:0;top:0;max-width:100px;margin:0 auto;height:10px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [120.0, 20.0, 100.0, 10.0]),
        ],
    },
    Case {
        name: "max_height_limits_fill",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;width:10px;top:0;bottom:0;max-height:50px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 10.0, 50.0]),
        ],
    },
    Case {
        name: "min_height_beats_fill",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;width:10px;top:0;bottom:150px;min-height:80px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 10.0, 80.0]),
        ],
    },
    Case {
        name: "height_from_content",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:10px;width:100px"><div style="height:25px"></div><div style="height:35px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 30.0, 100.0, 60.0]),
        ],
    },
    Case {
        name: "height_from_content_bottom_anchored",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;bottom:10px;width:100px"><div style="height:25px"></div><div style="height:35px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 150.0, 100.0, 60.0]),
        ],
    },
    Case {
        name: "percent_height_child_uses_resolved_height",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:10px;bottom:20px;width:100px"><div id=c style="height:50%"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 30.0, 100.0, 170.0]),
            ("c", [20.0, 30.0, 100.0, 85.0]),
        ],
    },
    Case {
        name: "percent_height_of_abs_uses_cb_not_parent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div style="height:30px"><div id=a style="position:absolute;left:0;top:0;width:20px;height:50%"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 20.0, 100.0]),
        ],
    },
    Case {
        name: "child_margins_do_not_collapse_through_abs",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:0;width:100px"><div id=c style="margin:15px 0;height:10px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 100.0, 40.0]),
            ("c", [20.0, 35.0, 100.0, 10.0]),
        ],
    },
    Case {
        name: "abs_establishes_bfc_floatless",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:0;width:100px;padding:5px"><div id=c style="margin-top:12px;height:10px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [20.0, 20.0, 110.0, 32.0]),
            ("c", [25.0, 37.0, 100.0, 10.0]),
        ],
    },
    Case {
        name: "empty_abs_shrinks_to_zero",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:5px;top:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [25.0, 25.0, 0.0, 0.0]),
        ],
    },
    Case {
        name: "abs_flex_container_shrink",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:5px;top:5px;display:flex"><div id=c style="width:40px;height:10px"></div><div id=d style="width:60px;height:20px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [25.0, 25.0, 100.0, 20.0]),
            ("c", [25.0, 25.0, 40.0, 10.0]),
            ("d", [65.0, 25.0, 60.0, 20.0]),
        ],
    },
    Case {
        name: "abs_grid_container_fill",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;inset:10px;display:grid;grid-template-columns:1fr 2fr"><div id=c></div><div id=d></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [30.0, 30.0, 280.0, 180.0]),
            ("c", [30.0, 30.0, 93.3281, 180.0]),
            ("d", [123.3281, 30.0, 186.6719, 180.0]),
        ],
    },
];
