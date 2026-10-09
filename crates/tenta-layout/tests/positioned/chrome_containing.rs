// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "icb_basic",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=a style="position:absolute;top:10px;left:20px;width:50px;height:30px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [20.0, 10.0, 50.0, 30.0])],
    },
    Case {
        name: "icb_ignores_static_body_margin",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:40px;padding:20px"><div id=a style="position:absolute;top:10px;left:20px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [40.0, 40.0, 720.0, 40.0]),
            ("a", [20.0, 10.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "relative_ancestor",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;top:5px;left:6px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [36.0, 35.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "padding_box_of_bordered_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:30px;padding:10px;border:4px solid;width:300px;height:200px"><div id=a style="position:absolute;top:0;left:0;right:0;bottom:0"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 328.0, 228.0]),
            ("a", [34.0, 34.0, 320.0, 220.0]),
        ],
    },
    Case {
        name: "static_between_cb_and_child",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:30px;width:300px;height:200px"><div id=m style="margin:20px;padding:10px"><div id=a style="position:absolute;top:5px;left:6px;width:50px;height:30px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("m", [50.0, 30.0, 260.0, 20.0]),
            ("a", [36.0, 35.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_abs",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:absolute;left:100px;top:50px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [100.0, 50.0, 300.0, 200.0]),
            ("a", [340.0, 210.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_fixed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:fixed;left:100px;top:50px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [100.0, 50.0, 300.0, 200.0]),
            ("a", [340.0, 210.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_sticky",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:sticky;top:0;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [260.0, 180.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_transform",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="transform:translate(0,0);margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [270.0, 190.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_rotate_prop",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="rotate:0deg;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [270.0, 190.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_filter",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="filter:blur(0px);margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [270.0, 190.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_will_change_transform",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="will-change:transform;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [270.0, 190.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_will_change_opacity",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="will-change:opacity;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [740.0, 560.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_contain_layout",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="contain:layout;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [270.0, 190.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_contain_paint",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="contain:paint;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [270.0, 190.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_contain_size_is_not_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="contain:size;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [740.0, 560.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_perspective",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="perspective:500px;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [270.0, 190.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_opacity_is_not_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="opacity:.5;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [740.0, 560.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "abs_inside_overflow_hidden_is_not_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="overflow:hidden;margin:30px;width:300px;height:200px"><div id=a style="position:absolute;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [740.0, 560.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "fixed_viewport",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:30px;height:2000px"><div id=a style="position:fixed;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 2060.0),
        expect: &[
            ("p", [30.0, 30.0, 740.0, 2000.0]),
            ("a", [740.0, 560.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "fixed_ignores_relative_ancestor",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:30px;width:300px;height:200px"><div id=a style="position:fixed;left:10px;top:20px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [10.0, 20.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "fixed_inside_transform",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="transform:translate(0,0);margin:30px;width:300px;height:200px"><div id=a style="position:fixed;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [270.0, 190.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "fixed_inside_filter",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="filter:grayscale(1);margin:30px;width:300px;height:200px"><div id=a style="position:fixed;right:10px;bottom:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 300.0, 200.0]),
            ("a", [270.0, 190.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "fixed_inside_abs_is_viewport",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:absolute;left:100px;top:100px;width:300px;height:200px"><div id=a style="position:fixed;left:10px;top:20px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [100.0, 100.0, 300.0, 200.0]),
            ("a", [10.0, 20.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "relative_offset_moves_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;left:15px;top:25px;width:300px;height:200px"><div id=a style="position:absolute;left:5px;top:6px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [15.0, 25.0, 300.0, 200.0]),
            ("a", [20.0, 31.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "percent_insets_and_size",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;border:5px solid;padding:10px;width:300px;height:200px"><div id=a style="position:absolute;left:10%;top:10%;width:50%;height:25%"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 330.0, 230.0]),
            ("a", [57.0, 47.0, 160.0, 55.0]),
        ],
    },
    Case {
        name: "percent_margins_and_padding_use_cb_width",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;left:0;top:0;width:50px;height:30px;margin:10% 5%;padding:2%"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [35.0, 50.0, 62.0, 42.0]),
        ],
    },
    Case {
        name: "percent_inset_right_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:300px;height:200px"><div id=a style="position:absolute;right:10%;bottom:20%;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 200.0]),
            ("a", [240.0, 150.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "html_positioned_is_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}html{position:relative;margin:10px;height:300px}</style></head><body id=body><div id=p style="margin:40px;height:100px"><div id=a style="position:absolute;left:5px;top:5px;width:20px;height:20px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [50.0, 50.0, 700.0, 100.0]),
            ("a", [15.0, 15.0, 20.0, 20.0]),
        ],
    },
    Case {
        name: "body_positioned_is_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}body{position:relative;margin:30px;height:300px}</style></head><body id=body><div id=a style="position:absolute;left:5px;bottom:5px;width:20px;height:20px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [35.0, 305.0, 20.0, 20.0])],
    },
];
