// Expected rectangles recorded from Chrome 153 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "stf_max_content_blocks",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:max-content"><div id=a style="width:100px;padding:5px;height:5px"></div><div id=b style="width:50px;height:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 110.0, 20.0]),
            ("a", [0.0, 0.0, 110.0, 15.0]),
            ("b", [0.0, 15.0, 50.0, 5.0]),
        ],
    },
    Case {
        name: "stf_min_content_blocks",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:min-content"><div id=a style="width:100px;padding:5px;height:5px"></div><div id=b style="width:50px;height:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 110.0, 20.0]),
            ("a", [0.0, 0.0, 110.0, 15.0]),
            ("b", [0.0, 15.0, 50.0, 5.0]),
        ],
    },
    Case {
        name: "stf_fit_content_keyword",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:fit-content"><div id=a style="width:900px;height:5px"></div></div>"#,
        scroll: (900.0, 600.0),
        expect: &[("p", [0.0, 0.0, 900.0, 5.0]), ("a", [0.0, 0.0, 900.0, 5.0])],
    },
    Case {
        name: "stf_fit_content_small",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:fit-content"><div id=a style="width:100px;margin:0 7px 0 3px;height:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 110.0, 5.0]), ("a", [3.0, 0.0, 100.0, 5.0])],
    },
    Case {
        name: "stf_nested_percent_is_auto",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:max-content"><div id=a style="width:50%;height:5px"></div><div id=b style="width:30px;height:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 30.0, 10.0]),
            ("a", [0.0, 0.0, 15.0, 5.0]),
            ("b", [0.0, 5.0, 30.0, 5.0]),
        ],
    },
    Case {
        name: "stf_min_max_clamp_child",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:max-content"><div id=a style="width:300px;max-width:120px;height:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 120.0, 5.0]), ("a", [0.0, 0.0, 120.0, 5.0])],
    },
    Case {
        name: "stf_min_width_child",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:max-content"><div id=a style="min-width:90px;height:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 90.0, 5.0]), ("a", [0.0, 0.0, 90.0, 5.0])],
    },
    Case {
        name: "stf_border_box_child",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:max-content"><div id=a style="box-sizing:border-box;width:100px;padding:0 10px;height:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 100.0, 5.0]), ("a", [0.0, 0.0, 100.0, 5.0])],
    },
    Case {
        name: "stf_negative_margin_child",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:max-content"><div id=a style="width:100px;margin:0 -10px 0 -20px;height:5px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 70.0, 5.0]),
            ("a", [-20.0, 0.0, 100.0, 5.0]),
        ],
    },
    Case {
        name: "stf_canvas",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:max-content"><canvas id=a style="display:block"></canvas></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 150.0]),
            ("a", [0.0, 0.0, 300.0, 150.0]),
        ],
    },
    Case {
        name: "stf_canvas_min_content_percent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:min-content"><canvas id=a style="display:block;max-width:100%"></canvas></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 0.0, 0.0]), ("a", [0.0, 0.0, 0.0, 0.0])],
    },
    Case {
        name: "stf_canvas_width_percent_min_content",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:min-content"><canvas id=a style="display:block;width:100%;height:20px"></canvas></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 0.0, 20.0]), ("a", [0.0, 0.0, 0.0, 20.0])],
    },
    Case {
        name: "stf_text_max",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:max-content;font:16px LS"><div id=a>aaa bbb</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 49.3125, 18.0]),
            ("a", [0.0, 0.0, 49.3125, 18.0]),
        ],
    },
    Case {
        name: "stf_text_min",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:min-content;font:16px LS;line-height:20px"><div id=a>aa bbbbbb c</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 48.0, 60.0]), ("a", [0.0, 0.0, 48.0, 60.0])],
    },
    Case {
        name: "stf_text_fit_wraps",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:fit-content;font:16px LS;line-height:20px;max-width:60px"><div id=a>aa bbbbbb c</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 60.0, 40.0]), ("a", [0.0, 0.0, 60.0, 40.0])],
    },
    Case {
        name: "stf_text_padding",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:max-content;padding:4px 6px;font:16px LS;line-height:20px"><div id=a>aaa</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 33.3125, 28.0]),
            ("a", [6.0, 4.0, 21.3125, 20.0]),
        ],
    },
    Case {
        name: "stf_min_width_keyword",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="width:20px;min-width:max-content;font:16px LS;line-height:20px"><div id=a>aaa bbb</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 49.3125, 20.0]),
            ("a", [0.0, 0.0, 49.3125, 20.0]),
        ],
    },
    Case {
        name: "stf_max_width_keyword",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} div{display:block}</style></head><body id=body><div id=p style="max-width:min-content;font:16px LS;line-height:20px"><div id=a>aa bbbbbb c</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[("p", [0.0, 0.0, 48.0, 60.0]), ("a", [0.0, 0.0, 48.0, 60.0])],
    },
];
