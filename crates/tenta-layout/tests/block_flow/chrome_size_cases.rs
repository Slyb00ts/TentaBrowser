// Expected rectangles recorded from Chrome 153 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "min_height_equal_to_content_margin_escapes",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="min-height:10px"><div id=c style="height:10px;margin-bottom:30px"></div></div><div id=n style="height:5px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 45.0]),
            ("body", [0.0, 0.0, 800.0, 45.0]),
            ("p", [0.0, 0.0, 800.0, 10.0]),
            ("c", [0.0, 0.0, 800.0, 10.0]),
            ("n", [0.0, 40.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "min_height_below_content_margin_escapes",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="min-height:5px"><div id=c style="height:10px;margin-bottom:30px"></div></div><div id=n style="height:5px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 45.0]),
            ("body", [0.0, 0.0, 800.0, 45.0]),
            ("p", [0.0, 0.0, 800.0, 10.0]),
            ("c", [0.0, 0.0, 800.0, 10.0]),
            ("n", [0.0, 40.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "min_height_above_content_margin_absorbed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="min-height:50px"><div id=c style="height:10px;margin-bottom:30px"></div></div><div id=n style="height:5px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 55.0]),
            ("body", [0.0, 0.0, 800.0, 55.0]),
            ("p", [0.0, 0.0, 800.0, 50.0]),
            ("c", [0.0, 0.0, 800.0, 10.0]),
            ("n", [0.0, 50.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "min_height_above_content_no_margin",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="min-height:10px"><div id=c style="height:5px"></div></div><div id=n style="height:5px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 15.0]),
            ("body", [0.0, 0.0, 800.0, 15.0]),
            ("p", [0.0, 0.0, 800.0, 10.0]),
            ("c", [0.0, 0.0, 800.0, 5.0]),
            ("n", [0.0, 10.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "max_height_clamps_margin_absorbed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="max-height:10px"><div id=c style="height:30px;margin-bottom:50px"></div></div><div id=n style="height:5px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 15.0]),
            ("body", [0.0, 0.0, 800.0, 15.0]),
            ("p", [0.0, 0.0, 800.0, 10.0]),
            ("c", [0.0, 0.0, 800.0, 30.0]),
            ("n", [0.0, 10.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "min_height_above_content_drops_margin",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="min-height:10px"><div id=c style="height:5px;margin-bottom:30px"></div></div><div id=n style="height:5px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 15.0]),
            ("body", [0.0, 0.0, 800.0, 15.0]),
            ("p", [0.0, 0.0, 800.0, 10.0]),
            ("c", [0.0, 0.0, 800.0, 5.0]),
            ("n", [0.0, 10.0, 800.0, 5.0]),
        ],
    },
];
