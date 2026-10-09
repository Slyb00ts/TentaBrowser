// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "scroll_extent_grows_for_abs",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=a style="position:absolute;left:900px;top:700px;width:50px;height:30px"></div>"#,
        scroll: (950.0, 730.0),
        expect: &[("a", [900.0, 700.0, 50.0, 30.0])],
    },
    Case {
        name: "scroll_extent_ignores_fixed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=a style="position:fixed;left:900px;top:700px;width:50px;height:30px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [900.0, 700.0, 50.0, 30.0])],
    },
    Case {
        name: "scroll_extent_ignores_negative",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=a style="position:absolute;left:-200px;top:-200px;width:50px;height:30px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [-200.0, -200.0, 50.0, 30.0])],
    },
    Case {
        name: "scroll_extent_clipped_by_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;overflow:hidden;width:100px;height:100px"><div id=a style="position:absolute;left:300px;top:300px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 100.0, 100.0]),
            ("a", [300.0, 300.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "scroll_extent_escapes_non_cb_clip",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="overflow:hidden;width:100px;height:100px"><div id=a style="position:absolute;left:900px;top:300px;width:50px;height:30px"></div></div>"#,
        scroll: (950.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 100.0, 100.0]),
            ("a", [900.0, 300.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "scroll_extent_abs_in_flow_cb",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:10px;width:100px;height:100px"><div id=a style="position:absolute;right:-1000px;bottom:-800px;width:50px;height:30px"></div></div>"#,
        scroll: (1110.0, 910.0),
        expect: &[
            ("p", [10.0, 10.0, 100.0, 100.0]),
            ("a", [1060.0, 880.0, 50.0, 30.0]),
        ],
    },
];
