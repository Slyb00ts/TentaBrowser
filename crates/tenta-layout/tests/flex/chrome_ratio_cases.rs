// Expected rectangles recorded from Chrome 153 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "float_ignored_on_items",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:flex;width:300px"><div id=a style="float:left;width:100px;height:10px"></div><div id=b style="flex:1;height:10px"></div></div><div id=after style="height:5px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [0.0, 0.0, 100.0, 10.0]),
            ("b", [100.0, 0.0, 200.0, 10.0]),
            ("after", [0.0, 10.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "float_wide_item_in_flex",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:flex;width:900px;margin-top:10px"><div id=m style="float:left;width:100%;height:20px"><div id=mm style="height:10px"></div></div><div id=s style="width:100px;margin-left:-100%;height:15px"></div></div></body></html>"#,
        scroll: (900.0, 600.0),
        expect: &[
            ("c", [0.0, 10.0, 900.0, 20.0]),
            ("m", [0.0, 10.0, 900.0, 20.0]),
            ("mm", [0.0, 10.0, 900.0, 10.0]),
            ("s", [0.0, 10.0, 100.0, 15.0]),
        ],
    },
    Case {
        name: "ratio_flex_media",
        viewport: (900.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=stack style="display:flex;flex-flow:column;gap:150px;width:1080px;overflow:hidden"><div id=row style="display:flex;align-items:flex-start;gap:40px;width:1080px;overflow:hidden"><div id=media style="flex:1 0 0;width:1px;aspect-ratio:1.72;overflow:hidden"></div><div id=copy style="flex:1 0 0;width:1px;height:150px"></div></div><div id=after style="height:20px"></div></div></body></html>"#,
        scroll: (1080.0, 600.0),
        expect: &[
            ("stack", [0.0, 0.0, 1080.0, 472.312]),
            ("row", [0.0, 0.0, 1080.0, 302.312]),
            ("media", [0.0, 0.0, 520.0, 302.312]),
            ("copy", [560.0, 0.0, 520.0, 150.0]),
            ("after", [0.0, 452.312, 1080.0, 20.0]),
        ],
    },
    Case {
        name: "ratio_height_definite",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:flex;width:600px"><div id=a style="height:100px;aspect-ratio:2"></div><div id=b style="width:100px;aspect-ratio:1/2"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 600.0, 200.0]),
            ("a", [0.0, 0.0, 200.0, 100.0]),
            ("b", [200.0, 0.0, 100.0, 200.0]),
        ],
    },
    Case {
        name: "ratio_content_box_padding",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:flex;width:600px;align-items:flex-start"><div id=a style="width:100px;padding:10px;aspect-ratio:2"></div><div id=b style="width:100px;padding:10px;aspect-ratio:2;box-sizing:border-box"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 600.0, 70.0]),
            ("a", [0.0, 0.0, 120.0, 70.0]),
            ("b", [120.0, 0.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "ratio_content_taller",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:flex;width:600px;align-items:flex-start"><div id=a style="width:100px;aspect-ratio:2">The quick brown fox jumps over the lazy dog and keeps running far away</div><div id=b style="width:100px;aspect-ratio:2;overflow:hidden">The quick brown fox jumps over the lazy dog and keeps running far away</div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 600.0, 126.0]),
            ("a", [0.0, 0.0, 100.0, 126.0]),
            ("b", [100.0, 0.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "ratio_stretch_column",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:flex;flex-direction:column;width:200px;align-items:stretch"><div id=a style="aspect-ratio:2"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 200.0, 100.0]),
            ("a", [0.0, 0.0, 200.0, 100.0]),
        ],
    },
];
