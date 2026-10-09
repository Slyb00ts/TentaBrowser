// Expected rectangles recorded from Chrome 153 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "rel_percent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:400px;height:200px"><div id=a style="position:relative;left:10%;top:25%;height:30px;width:50px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 400.0, 200.0]),
            ("a", [40.0, 50.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "rel_percent_right_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:400px;height:200px"><div id=a style="position:relative;right:10%;bottom:25%;height:30px;width:50px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 400.0, 200.0]),
            ("a", [-40.0, -50.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "rel_percent_top_indefinite",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:400px"><div id=a style="position:relative;top:25%;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 400.0, 30.0]),
            ("a", [0.0, 0.0, 400.0, 30.0]),
        ],
    },
    Case {
        name: "rel_calc",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=a style="position:relative;left:calc(10px + 5%);top:calc(100% - 10px);height:30px"></div>"#,
        scroll: (850.0, 600.0),
        expect: &[("a", [50.0, 0.0, 800.0, 30.0])],
    },
];
