// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "gap_both",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:100px 100px;gap:10px 20px"><div id=a style="height:30px"></div><div id=b style="height:30px"></div><div id=d style="height:30px"></div><div id=e style="height:30px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 70.0]),
            ("a", [0.0, 0.0, 100.0, 30.0]),
            ("b", [120.0, 0.0, 100.0, 30.0]),
            ("d", [0.0, 40.0, 100.0, 30.0]),
            ("e", [120.0, 40.0, 100.0, 30.0]),
        ],
    },
    Case {
        name: "gap_with_fr",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:1fr 1fr 1fr;gap:15px;width:400px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div><div id=d style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 400.0, 10.0]),
            ("a", [0.0, 0.0, 123.3281, 10.0]),
            ("b", [138.3281, 0.0, 123.3281, 10.0]),
            ("d", [276.6562, 0.0, 123.3438, 10.0]),
        ],
    },
    Case {
        name: "gap_percent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:1fr 1fr;column-gap:10%;row-gap:5%;width:400px;height:200px"><div id=a></div><div id=b></div><div id=d></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 400.0, 200.0]),
            ("a", [0.0, 0.0, 180.0, 95.0]),
            ("b", [220.0, 0.0, 180.0, 95.0]),
            ("d", [0.0, 105.0, 180.0, 95.0]),
        ],
    },
    Case {
        name: "gap_span_includes_gap",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(3,50px);gap:10px;grid-template-rows:20px 20px"><div id=a style="grid-column:1/4"></div><div id=b style="grid-column:1/3"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 50.0]),
            ("a", [0.0, 0.0, 170.0, 20.0]),
            ("b", [0.0, 30.0, 110.0, 20.0]),
        ],
    },
    Case {
        name: "gap_with_auto_fit_collapse",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(auto-fit,50px);gap:10px;width:300px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [0.0, 0.0, 50.0, 10.0]),
            ("b", [60.0, 0.0, 50.0, 10.0]),
        ],
    },
    Case {
        name: "gap_content_distribution",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px 50px;gap:10px;justify-content:space-between;width:300px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [0.0, 0.0, 50.0, 10.0]),
            ("b", [250.0, 0.0, 50.0, 10.0]),
        ],
    },
    Case {
        name: "gap_calc_em",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px 50px;gap:calc(1em + 2px);font-size:10px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div><div id=d style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 32.0]),
            ("a", [0.0, 0.0, 50.0, 10.0]),
            ("b", [62.0, 0.0, 50.0, 10.0]),
            ("d", [0.0, 22.0, 50.0, 10.0]),
        ],
    },
];
