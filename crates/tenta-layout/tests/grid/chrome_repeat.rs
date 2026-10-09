// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "auto_fill_px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(auto-fill,100px);width:350px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div><div id=d style="height:10px"></div><div id=e style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 350.0, 20.0]),
            ("a", [0.0, 0.0, 100.0, 10.0]),
            ("b", [100.0, 0.0, 100.0, 10.0]),
            ("d", [200.0, 0.0, 100.0, 10.0]),
            ("e", [0.0, 10.0, 100.0, 10.0]),
        ],
    },
    Case {
        name: "auto_fit_collapses",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(auto-fit,100px);width:450px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 450.0, 10.0]),
            ("a", [0.0, 0.0, 100.0, 10.0]),
            ("b", [100.0, 0.0, 100.0, 10.0]),
        ],
    },
    Case {
        name: "auto_fit_minmax_stretch",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(auto-fit,minmax(100px,1fr));width:450px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 450.0, 10.0]),
            ("a", [0.0, 0.0, 225.0, 10.0]),
            ("b", [225.0, 0.0, 225.0, 10.0]),
        ],
    },
    Case {
        name: "auto_fill_minmax_keeps_empty",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(auto-fill,minmax(100px,1fr));width:450px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 450.0, 10.0]),
            ("a", [0.0, 0.0, 112.5, 10.0]),
            ("b", [112.5, 0.0, 112.5, 10.0]),
        ],
    },
    Case {
        name: "auto_fill_with_gap",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(auto-fill,80px);gap:10px;width:350px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div><div id=d style="height:10px"></div><div id=e style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 350.0, 10.0]),
            ("a", [0.0, 0.0, 80.0, 10.0]),
            ("b", [90.0, 0.0, 80.0, 10.0]),
            ("d", [180.0, 0.0, 80.0, 10.0]),
            ("e", [270.0, 0.0, 80.0, 10.0]),
        ],
    },
    Case {
        name: "auto_fill_after_fixed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px repeat(auto-fill,100px);width:400px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div><div id=d style="height:10px"></div><div id=e style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 400.0, 10.0]),
            ("a", [0.0, 0.0, 50.0, 10.0]),
            ("b", [50.0, 0.0, 100.0, 10.0]),
            ("d", [150.0, 0.0, 100.0, 10.0]),
            ("e", [250.0, 0.0, 100.0, 10.0]),
        ],
    },
    Case {
        name: "auto_fill_rows",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-rows:repeat(auto-fill,40px);grid-template-columns:100px;height:130px"><div id=a></div><div id=b></div><div id=d></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 130.0]),
            ("a", [0.0, 0.0, 100.0, 40.0]),
            ("b", [0.0, 40.0, 100.0, 40.0]),
            ("d", [0.0, 80.0, 100.0, 40.0]),
        ],
    },
    Case {
        name: "auto_fit_wraps_items",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));width:480px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div><div id=d style="height:10px"></div><div id=e style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 480.0, 20.0]),
            ("a", [0.0, 0.0, 160.0, 10.0]),
            ("b", [160.0, 0.0, 160.0, 10.0]),
            ("d", [320.0, 0.0, 160.0, 10.0]),
            ("e", [0.0, 10.0, 160.0, 10.0]),
        ],
    },
];
