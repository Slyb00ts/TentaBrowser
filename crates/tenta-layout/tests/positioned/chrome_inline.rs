// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "inline_block_is_positioned_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="display:inline-block;position:relative;width:100px;height:50px;margin:10px"><div id=a style="position:absolute;left:5px;top:5px;width:20px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [10.0, 10.0, 100.0, 50.0]),
            ("a", [15.0, 15.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_after_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hello <span id=a style="position:absolute;width:40px;height:20px"></span> world</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [60.9062, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_first_in_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px"><span id=a style="position:absolute;width:40px;height:20px"></span>Hello</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [20.0, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_in_second_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:100px">Hello world and more <span id=a style="position:absolute;width:40px;height:20px"></span>words</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 54.0]),
            ("a", [87.5938, 38.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_div_after_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hello<div id=a style="position:absolute;width:40px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [20.0, 38.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_rtl",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;direction:rtl">Hello <span id=a style="position:absolute;width:40px;height:20px"></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [243.5312, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_text_align_center",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;text-align:center">Hi<span id=a style="position:absolute;width:40px;height:20px"></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [177.5469, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_inside_inline_box",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hello <u>big <span id=a style="position:absolute;width:40px;height:20px"></span>box</u></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [86.7031, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_with_inline_block_before",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px"><span style="display:inline-block;width:80px;height:50px"></span><span id=a style="position:absolute;width:40px;height:20px"></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 54.0]),
            ("a", [100.0, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_cb_single_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hello <span id=s style="position:relative">big<span id=a style="position:absolute;left:0;top:100%;width:40px;height:20px"></span></span> world</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("s", [60.9062, 20.0, 21.3594, 17.0]),
            ("a", [60.9062, 37.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_cb_right_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hello <span id=s style="position:relative">big</span> world<span id=a style="position:absolute;width:40px;height:20px"></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("s", [60.9062, 20.0, 21.3594, 17.0]),
            ("a", [124.9531, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_cb_padding_box",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hello <span id=s style="position:relative;padding:5px 7px;border:2px solid">big<span id=a style="position:absolute;right:0;bottom:0;width:40px;height:20px"></span></span> world</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("s", [60.9062, 13.0, 39.3594, 31.0]),
            ("a", [58.2656, 22.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_cb_percent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hi <span id=s style="position:relative;padding:0 10px">word<span id=a style="position:absolute;left:50%;top:0;width:10%;height:50%"></span></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("s", [39.5625, 20.0, 54.6875, 17.0]),
            ("a", [66.9062, 20.0, 5.4688, 8.5]),
        ],
    },
    Case {
        name: "inline_cb_fill_insets",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hi <span id=s style="position:relative">word<span id=a style="position:absolute;inset:0"></span></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("s", [39.5625, 20.0, 34.6875, 17.0]),
            ("a", [39.5625, 20.0, 34.6875, 17.0]),
        ],
    },
    Case {
        name: "inline_static_block_level_first",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px"><div id=a style="position:absolute;width:40px;height:20px"></div>Hello</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [20.0, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_block_level_between_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hello<div id=a style="position:absolute;width:40px;height:20px"></div>World</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [20.0, 38.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_block_level_after_br",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hello<br><div id=a style="position:absolute;width:40px;height:20px"></div>World</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 36.0]),
            ("a", [20.0, 38.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_block_level_after_inline_box",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px"><span>x</span><div id=a style="position:absolute;width:40px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [20.0, 38.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_block_level_after_wrapped_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:100px">Hello world more<div id=a style="position:absolute;width:40px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 36.0]),
            ("a", [20.0, 56.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_block_level_rtl",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;direction:rtl">Hello<div id=a style="position:absolute;width:40px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [280.0, 38.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_block_level_text_align_center",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;text-align:center">Hello<div id=a style="position:absolute;width:40px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [20.0, 38.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_only_child_takes_no_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px"><span id=a style="position:absolute;width:40px;height:20px"></span></div><div id=t style="height:10px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 0.0]),
            ("a", [20.0, 20.0, 40.0, 20.0]),
            ("t", [0.0, 20.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "inline_cb_wrapped_ltr",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:100px">aa <span id=s style="position:relative">bbbb cccc dddd eeee<span id=a style="position:absolute;inset:0"></span></span> ff</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 36.0]),
            ("a", [42.25, 20.0, 53.3906, 35.0]),
        ],
    },
    Case {
        name: "inline_cb_wrapped_rtl",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:100px;direction:rtl">aa <span id=s style="position:relative">bbbb cccc dddd eeee<span id=a style="position:absolute;inset:0"></span></span> ff</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 36.0]),
            ("a", [31.3125, 20.0, 88.6875, 35.0]),
        ],
    },
    Case {
        name: "inline_cb_wrapped_three_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:60px">aa <span id=s style="position:relative">bbbb cccc dddd eeee<span id=a style="position:absolute;left:0;top:0;width:10px;height:10px"></span></span> ff</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 60.0, 72.0]),
            ("a", [42.25, 20.0, 10.0, 10.0]),
        ],
    },
    Case {
        name: "inline_cb_wrapped_with_padding_border",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:100px">aa <span id=s style="position:relative;padding:3px 5px;border:2px solid">bbbb cccc dddd eeee<span id=a style="position:absolute;right:0;bottom:0;width:10px;height:10px"></span></span> ff</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 54.0]),
            ("a", [50.5938, 66.0, 10.0, 10.0]),
        ],
    },
    Case {
        name: "inline_cb_innermost_positioned_wins",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hi <span id=s style="position:relative;padding-left:30px">one <span id=t style="position:relative">two<span id=a style="position:absolute;left:0;top:0;width:10px;height:10px"></span></span></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("s", [39.5625, 20.0, 86.0469, 17.0]),
            ("t", [100.7031, 20.0, 24.9062, 17.0]),
            ("a", [100.7031, 20.0, 10.0, 10.0]),
        ],
    },
    Case {
        name: "inline_cb_non_positioned_inline_is_skipped",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;position:relative">Hi <span id=s>one<span id=a style="position:absolute;left:0;top:0;width:10px;height:10px"></span></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("s", [39.5625, 20.0, 26.7031, 17.0]),
            ("a", [20.0, 20.0, 10.0, 10.0]),
        ],
    },
    Case {
        name: "inline_cb_static_position_inside",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hi <span id=s style="position:relative">one<span id=a style="position:absolute;width:10px;height:10px"></span></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("s", [39.5625, 20.0, 26.7031, 17.0]),
            ("a", [66.2656, 20.0, 10.0, 10.0]),
        ],
    },
    Case {
        name: "inline_flex_container_static",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">x<span id=f style="display:inline-flex;width:100px;height:30px;justify-content:center;align-items:center"><div id=a style="position:absolute;width:20px;height:10px"></div></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 34.0]),
            ("f", [28.0, 20.0, 100.0, 30.0]),
            ("a", [68.0, 30.0, 20.0, 10.0]),
        ],
    },
    Case {
        name: "inline_flex_container_is_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">x<span id=f style="display:inline-flex;position:relative;width:100px;height:30px"><div id=a style="position:absolute;right:0;bottom:0;width:20px;height:10px"></div></span></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 34.0]),
            ("f", [28.0, 20.0, 100.0, 30.0]),
            ("a", [108.0, 40.0, 20.0, 10.0]),
        ],
    },
    Case {
        name: "inline_static_rtl_between_spaces",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;direction:rtl">Hello <span id=a style="position:absolute;width:40px;height:20px"></span> world</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [281.7656, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_ltr_between_spaces",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px">Hello <span id=a style="position:absolute;width:40px;height:20px"></span> world</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [60.9062, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_rtl_before_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;direction:rtl"><span id=a style="position:absolute;width:40px;height:20px"></span> world</div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 18.0]),
            ("a", [280.0, 20.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_rtl_hebrew_between_collapsed_spaces",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div style="margin:20px"><p id=p dir=rtl style="margin:0;width:100px;background:#cfc;direction:rtl">אבג <span id=a style="position:absolute;top:60px;width:20px;height:20px">x</span> דהו</p></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 100.0, 18.0]),
            ("a", [69.1719, 60.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_rtl_hebrew_in_positioned_root",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}html{position:absolute;left:20px;top:20px;width:150px}body{margin:0;position:absolute;left:10px;top:10px}</style></head><body id=body><div style="height:100px"></div><p id=p dir=rtl style="margin:0;width:100px;background:#cfc;direction:rtl">אבג <span id=a style="position:absolute;top:60px;width:20px;height:20px">x</span> דהו</p>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 130.0, 100.0, 18.0]),
            ("a", [79.1719, 90.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "inline_static_rtl_hebrew_critic_probe",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}html{position:absolute;left:20px;top:20px;width:150px;font-family:'Liberation Serif'}body{margin:0;position:absolute;left:10px;top:10px;font-family:'Liberation Serif'}</style></head><body id=body><div style="position:absolute;right:0;bottom:0;width:30px;height:30px"></div><div style="height:100px"></div><div style="display:none"><div style="position:absolute;width:50px;height:50px"></div></div>
<p id=p dir=rtl style="margin:0;width:100px;background:#cfc;direction:rtl">אבג <span id=a style="position:absolute;top:60px;width:20px;height:20px">x</span> דהו</p>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 130.0, 100.0, 18.0]),
            ("a", [83.4062, 90.0, 20.0, 20.0]),
        ],
    },
];
