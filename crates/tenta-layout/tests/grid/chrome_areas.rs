// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "holy_grail",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-areas:'h h h' 'n m s' 'f f f';grid-template-columns:100px 1fr 80px;grid-template-rows:30px 1fr 20px;width:500px;height:300px"><div id=h style="grid-area:h"></div><div id=n style="grid-area:n"></div><div id=m style="grid-area:m"></div><div id=s style="grid-area:s"></div><div id=f style="grid-area:f"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 500.0, 300.0]),
            ("h", [0.0, 0.0, 500.0, 30.0]),
            ("n", [0.0, 30.0, 100.0, 250.0]),
            ("m", [100.0, 30.0, 320.0, 250.0]),
            ("s", [420.0, 30.0, 80.0, 250.0]),
            ("f", [0.0, 280.0, 500.0, 20.0]),
        ],
    },
    Case {
        name: "areas_with_gaps",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-areas:'a a b' 'c . b';grid-template-columns:50px 50px 50px;grid-template-rows:20px 20px;gap:5px"><div id=a style="grid-area:a"></div><div id=b style="grid-area:b"></div><div id=d style="grid-area:c"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 45.0]),
            ("a", [0.0, 0.0, 105.0, 20.0]),
            ("b", [110.0, 0.0, 50.0, 45.0]),
            ("d", [0.0, 25.0, 50.0, 20.0]),
        ],
    },
    Case {
        name: "areas_without_tracks_implicit",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-areas:'x y' 'z w';width:200px"><div id=a style="grid-area:x;height:10px"></div><div id=b style="grid-area:w;height:20px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 200.0, 30.0]),
            ("a", [0.0, 0.0, 100.0, 10.0]),
            ("b", [100.0, 10.0, 100.0, 20.0]),
        ],
    },
    Case {
        name: "areas_rtl",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-areas:'a b';grid-template-columns:60px 90px;direction:rtl;width:300px"><div id=a style="grid-area:a;height:10px"></div><div id=b style="grid-area:b;height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [240.0, 0.0, 60.0, 10.0]),
            ("b", [150.0, 0.0, 90.0, 10.0]),
        ],
    },
    Case {
        name: "areas_auto_flow_fills_rest",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-areas:'a b';grid-template-columns:60px 90px"><div id=a style="grid-area:b;height:10px"></div><div id=b style="height:10px"></div><div id=d style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [60.0, 0.0, 90.0, 10.0]),
            ("b", [0.0, 0.0, 60.0, 10.0]),
            ("d", [0.0, 10.0, 60.0, 10.0]),
        ],
    },
    Case {
        name: "area_name_as_line_span",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-areas:'. a a .' '. a a .';grid-template-columns:repeat(4,40px);grid-template-rows:20px 20px"><div id=a style="grid-area:a"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 40.0]),
            ("a", [40.0, 0.0, 80.0, 40.0]),
        ],
    },
];
