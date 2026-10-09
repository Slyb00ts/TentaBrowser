// Expected rectangles recorded from Chrome 153 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "rel_basic",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=a style="position:relative;left:10px;top:20px;height:30px"></div><div id=n style="height:5px"></div>"#,
        scroll: (810.0, 600.0),
        expect: &[
            ("a", [10.0, 20.0, 800.0, 30.0]),
            ("n", [0.0, 30.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "rel_right_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=a style="position:relative;right:10px;bottom:20px;height:30px"></div><div id=n style="height:5px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [-10.0, -20.0, 800.0, 30.0]),
            ("n", [0.0, 30.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "rel_both_ltr",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=a style="position:relative;left:10px;right:50px;top:7px;bottom:90px;height:30px"></div>"#,
        scroll: (810.0, 600.0),
        expect: &[("a", [10.0, 7.0, 800.0, 30.0])],
    },
    Case {
        name: "rel_both_rtl",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div style="direction:rtl"><div id=a style="position:relative;left:10px;right:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [-50.0, 0.0, 800.0, 30.0])],
    },
    Case {
        name: "rel_child_ltr_in_rtl_parent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div style="direction:rtl"><div id=a style="direction:ltr;position:relative;left:10px;right:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [-50.0, 0.0, 800.0, 30.0])],
    },
    Case {
        name: "rel_child_rtl_in_ltr_parent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div><div id=a style="direction:rtl;position:relative;left:10px;right:50px;height:30px"></div></div>"#,
        scroll: (810.0, 600.0),
        expect: &[("a", [10.0, 0.0, 800.0, 30.0])],
    },
    Case {
        name: "rel_does_not_move_siblings",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=a style="position:relative;top:50px;height:30px"></div><div id=b style="height:30px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [0.0, 50.0, 800.0, 30.0]),
            ("b", [0.0, 30.0, 800.0, 30.0]),
        ],
    },
    Case {
        name: "rel_child_follows_parent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="position:relative;left:15px;top:5px;height:50px"><div id=a style="height:10px"></div></div>"#,
        scroll: (815.0, 600.0),
        expect: &[
            ("p", [15.0, 5.0, 800.0, 50.0]),
            ("a", [15.0, 5.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "rel_nested",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="position:relative;left:15px;top:5px;height:50px"><div id=a style="position:relative;left:3px;top:4px;height:10px"></div></div>"#,
        scroll: (818.0, 600.0),
        expect: &[
            ("p", [15.0, 5.0, 800.0, 50.0]),
            ("a", [18.0, 9.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "rel_with_margins_padding",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=a style="position:relative;left:10px;top:-5px;margin:20px;padding:4px;border:3px solid;height:30px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [30.0, 15.0, 760.0, 44.0])],
    },
    Case {
        name: "rel_auto_margin_center",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=a style="position:relative;left:10px;width:200px;margin:0 auto;height:30px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [310.0, 0.0, 200.0, 30.0])],
    },
    Case {
        name: "rel_negative",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=a style="position:relative;left:-30px;top:-10px;height:30px;margin-top:20px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("a", [-30.0, 10.0, 800.0, 30.0])],
    },
    Case {
        name: "rel_scroll_extent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=a style="position:relative;left:900px;height:30px;width:50px"></div>"#,
        scroll: (950.0, 600.0),
        expect: &[("a", [900.0, 0.0, 50.0, 30.0])],
    },
];
