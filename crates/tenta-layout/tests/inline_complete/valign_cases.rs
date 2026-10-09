// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "va_small_sub",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:sub;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 20.188]),
            ("s", [40.906, 9.188, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_sub",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:sub;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_super",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:super;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 19.328]),
            ("s", [40.906, 0.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_super",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:super;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 37.328]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_textmtop",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:text-top;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [40.906, 0.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_textmtop",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:text-top;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_textmbottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:text-bottom;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [40.906, 6.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_textmbottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:text-bottom;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 35.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_middle",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:middle;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [40.906, 4.266, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_middle",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:middle;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_top",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:top;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [40.906, 0.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_top",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:top;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:bottom;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [40.906, 7.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:bottom;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:baseline;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [40.906, 5.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:baseline;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_4px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:4px;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [40.906, 1.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_4px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:4px;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 35.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_m5px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:-5px;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 21.0]),
            ("s", [40.906, 10.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_m5px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:-5px;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_50pct",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:50%;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.5]),
            ("s", [40.906, 0.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_50pct",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:50%;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 48.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_m120pct",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:-120%;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 29.188]),
            ("s", [40.906, 18.188, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_m120pct",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:-120%;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 61.797]),
            ("s", [40.906, 27.797, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_small_2_5px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:2.5px;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("s", [40.906, 2.5, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_big_2_5px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:2.5px;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [40.906, 0.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_ib_sub",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;width:30px;height:40px;vertical-align:sub'></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [40.906, 0.0, 30.0, 40.0]),
        ],
    },
    Case {
        name: "va_ib_text_sub",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;vertical-align:sub'>in<br>side</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [40.906, 0.0, 29.359, 36.0]),
        ],
    },
    Case {
        name: "va_canvas_sub",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <canvas id=c style='width:30px;height:24px;vertical-align:sub'></canvas> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.0]),
            ("c", [40.906, 0.0, 30.0, 24.0]),
        ],
    },
    Case {
        name: "va_ib_super",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;width:30px;height:40px;vertical-align:super'></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 50.328]),
            ("a", [40.906, 0.0, 30.0, 40.0]),
        ],
    },
    Case {
        name: "va_ib_text_super",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;vertical-align:super'>in<br>side</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 42.328]),
            ("a", [40.906, 0.0, 29.359, 36.0]),
        ],
    },
    Case {
        name: "va_canvas_super",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <canvas id=c style='width:30px;height:24px;vertical-align:super'></canvas> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.328]),
            ("c", [40.906, 0.0, 30.0, 24.0]),
        ],
    },
    Case {
        name: "va_ib_textmtop",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;width:30px;height:40px;vertical-align:text-top'></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [40.906, 0.0, 30.0, 40.0]),
        ],
    },
    Case {
        name: "va_ib_text_textmtop",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;vertical-align:text-top'>in<br>side</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [40.906, 0.0, 29.359, 36.0]),
        ],
    },
    Case {
        name: "va_canvas_textmtop",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <canvas id=c style='width:30px;height:24px;vertical-align:text-top'></canvas> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.0]),
            ("c", [40.906, 0.0, 30.0, 24.0]),
        ],
    },
    Case {
        name: "va_ib_textmbottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;width:30px;height:40px;vertical-align:text-bottom'></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 41.0]),
            ("a", [40.906, 0.0, 30.0, 40.0]),
        ],
    },
    Case {
        name: "va_ib_text_textmbottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;vertical-align:text-bottom'>in<br>side</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 37.0]),
            ("a", [40.906, 0.0, 29.359, 36.0]),
        ],
    },
    Case {
        name: "va_canvas_textmbottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <canvas id=c style='width:30px;height:24px;vertical-align:text-bottom'></canvas> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 25.0]),
            ("c", [40.906, 0.0, 30.0, 24.0]),
        ],
    },
    Case {
        name: "va_ib_middle",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;width:30px;height:40px;vertical-align:middle'></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [40.906, 0.0, 30.0, 40.0]),
        ],
    },
    Case {
        name: "va_ib_text_middle",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;vertical-align:middle'>in<br>side</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [40.906, 0.0, 29.359, 36.0]),
        ],
    },
    Case {
        name: "va_canvas_middle",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <canvas id=c style='width:30px;height:24px;vertical-align:middle'></canvas> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.0]),
            ("c", [40.906, 0.0, 30.0, 24.0]),
        ],
    },
    Case {
        name: "va_ib_top",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;width:30px;height:40px;vertical-align:top'></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [40.906, 0.0, 30.0, 40.0]),
        ],
    },
    Case {
        name: "va_ib_text_top",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;vertical-align:top'>in<br>side</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [40.906, 0.0, 29.359, 36.0]),
        ],
    },
    Case {
        name: "va_canvas_top",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <canvas id=c style='width:30px;height:24px;vertical-align:top'></canvas> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.0]),
            ("c", [40.906, 0.0, 30.0, 24.0]),
        ],
    },
    Case {
        name: "va_ib_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;width:30px;height:40px;vertical-align:bottom'></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.0]),
            ("a", [40.906, 0.0, 30.0, 40.0]),
        ],
    },
    Case {
        name: "va_ib_text_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;vertical-align:bottom'>in<br>side</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("a", [40.906, 0.0, 29.359, 36.0]),
        ],
    },
    Case {
        name: "va_canvas_bottom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <canvas id=c style='width:30px;height:24px;vertical-align:bottom'></canvas> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.0]),
            ("c", [40.906, 0.0, 30.0, 24.0]),
        ],
    },
    Case {
        name: "va_ib_6px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;width:30px;height:40px;vertical-align:6px'></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 50.0]),
            ("a", [40.906, 0.0, 30.0, 40.0]),
        ],
    },
    Case {
        name: "va_ib_text_6px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;vertical-align:6px'>in<br>side</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 42.0]),
            ("a", [40.906, 0.0, 29.359, 36.0]),
        ],
    },
    Case {
        name: "va_canvas_6px",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <canvas id=c style='width:30px;height:24px;vertical-align:6px'></canvas> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("c", [40.906, 0.0, 30.0, 24.0]),
        ],
    },
    Case {
        name: "va_ib_25pct",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;width:30px;height:40px;vertical-align:25%'></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 48.5]),
            ("a", [40.906, 0.0, 30.0, 40.0]),
        ],
    },
    Case {
        name: "va_ib_text_25pct",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=a style='display:inline-block;background:#ccc;vertical-align:25%'>in<br>side</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 40.5]),
            ("a", [40.906, 0.0, 29.359, 36.0]),
        ],
    },
    Case {
        name: "va_canvas_25pct",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <canvas id=c style='width:30px;height:24px;vertical-align:25%'></canvas> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 32.5]),
            ("c", [40.906, 0.0, 30.0, 24.0]),
        ],
    },
    Case {
        name: "va_middle_dejavu",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:middle;font:30px "DejaVu Sans"'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 35.0]),
            ("s", [40.906, 0.0, 47.969, 35.0]),
        ],
    },
    Case {
        name: "va_middle_parent_dejavu",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='font-family:"DejaVu Sans"'>Hello <span id=s style='vertical-align:middle;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 35.0]),
            ("s", [45.641, 0.0, 47.969, 35.0]),
        ],
    },
    Case {
        name: "va_sub_nested_size",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='font-size:20px'>Hello <span id=s style='vertical-align:sub;font-size:10px'>small</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 25.0]),
            ("s", [51.141, 14.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_sub_nested_in_span",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=o style='font-size:30px'>big <span id=s style='vertical-align:sub;font-size:10px'>small</span></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 36.0]),
            ("o", [40.906, 0.0, 71.719, 33.0]),
            ("s", [89.281, 25.0, 23.344, 11.0]),
        ],
    },
    Case {
        name: "va_nested_shifts",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=o style='vertical-align:super;font-size:12px'>up <span id=s style='vertical-align:super;font-size:10px'>more</span></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.328]),
            ("o", [40.906, 3.0, 39.484, 14.0]),
            ("s", [57.594, 0.0, 22.797, 11.0]),
        ],
    },
    Case {
        name: "va_nested_in_shifted_grows_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=o style='vertical-align:20px'>up <span id=s style='vertical-align:20px;font-size:30px'>more</span></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 71.0]),
            ("o", [40.906, 33.0, 90.609, 17.0]),
            ("s", [63.156, 0.0, 68.359, 33.0]),
        ],
    },
    Case {
        name: "va_top_and_bottom_both",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=t style='vertical-align:top;font-size:40px'>T</span> <span id=b style='vertical-align:bottom;font-size:50px'>B</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 58.0]),
            ("t", [40.906, 0.0, 24.438, 44.0]),
            ("b", [69.797, 1.0, 33.359, 56.0]),
        ],
    },
    Case {
        name: "va_top_taller_than_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=t style='vertical-align:top;font-size:50px'>T</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 58.0]),
            ("t", [40.906, 1.0, 30.547, 56.0]),
        ],
    },
    Case {
        name: "va_bottom_taller_than_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=t style='vertical-align:bottom;font-size:50px'>T</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 58.0]),
            ("t", [40.906, 1.0, 30.547, 56.0]),
        ],
    },
    Case {
        name: "va_top_inside_span",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=o style='font-size:12px'>x <span id=s style='vertical-align:top;font-size:40px'>T</span></span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 45.0]),
            ("o", [40.906, 3.0, 33.781, 14.0]),
            ("s", [50.25, 0.0, 24.438, 44.0]),
        ],
    },
    Case {
        name: "va_line_height_20",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='line-height:20px'>Hello <span id=s style='vertical-align:middle;font-size:30px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 20.766]),
            ("s", [40.906, -6.234, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_line_height_20_span_lh",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='line-height:20px'>Hello <span id=s style='vertical-align:super;font-size:30px;line-height:40px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 41.328]),
            ("s", [40.906, 3.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_percent_uses_own_line_height",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>Hello <span id=s style='vertical-align:50%;font-size:30px;line-height:60px'>Big</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 74.0]),
            ("s", [40.906, 13.0, 43.359, 33.0]),
        ],
    },
    Case {
        name: "va_mixed_sizes_middle",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>a<span id=a style='font-size:10px;vertical-align:middle'>b</span><span id=b style='font-size:24px;vertical-align:middle'>c</span><span id=c style='font-size:40px;vertical-align:middle'>d</span>e</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 45.0]),
            ("a", [8.906, 17.0, 5.562, 11.0]),
            ("b", [14.469, 8.5, 12.0, 27.0]),
            ("c", [26.469, 0.0, 22.25, 44.0]),
        ],
    },
    Case {
        name: "va_middle_empty_ib",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=a style='display:inline-block;background:#ccc;width:20px;height:10px;vertical-align:middle'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 18.0]),
            ("a", [8.0, 4.766, 20.0, 10.0]),
        ],
    },
    Case {
        name: "va_text_top_two_sizes",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='font-size:30px'>Big <span id=s style='vertical-align:text-top;font-size:10px'>tt</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [51.703, 0.0, 5.562, 11.0]),
        ],
    },
    Case {
        name: "va_text_bottom_two_sizes",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='font-size:30px'>Big <span id=s style='vertical-align:text-bottom;font-size:10px'>tb</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 34.0]),
            ("s", [51.703, 22.0, 8.344, 11.0]),
        ],
    },
    Case {
        name: "va_only_valign_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p><span id=s style='vertical-align:top;font-size:20px'>only</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 23.0]),
            ("s", [0.0, 0.0, 36.703, 22.0]),
        ],
    },
    Case {
        name: "va_sub_in_quirks_free_empty_span",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p>x<span id=s style='vertical-align:super;padding:0 2px'></span>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 800.0, 24.328]),
            ("s", [8.0, 0.0, 4.0, 17.0]),
        ],
    },
    Case {
        name: "va_two_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:90px'>Hello <span id=a style='vertical-align:super;font-size:30px'>Big</span> world again <span id=b style='vertical-align:sub;font-size:20px'>low</span> text</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 90.0, 78.516]),
            ("a", [40.906, 0.0, 43.359, 33.0]),
            ("b", [0.0, 55.516, 30.016, 22.0]),
        ],
    },
    Case {
        name: "va_span_continues_onto_second_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:100px'>aa <span id=s style='vertical-align:super;font-size:20px'>bbb ccc ddd eee fff</span> gg</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 100.0, 74.656]),
            ("s", [22.25, 0.0, 68.938, 22.0]),
        ],
    },
    Case {
        name: "va_top_span_continues_onto_second_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:100px'>aa <span id=s style='vertical-align:top;font-size:20px'>bbb ccc ddd eee fff</span> gg</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 100.0, 64.0]),
            ("s", [22.25, 0.0, 68.938, 22.0]),
        ],
    },
    Case {
        name: "va_atomic_wraps_before_valign_span",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style='width:100px'><span id=a style='display:inline-block;width:90px;height:10px;vertical-align:text-bottom'></span><span id=s style='vertical-align:sub;font-size:12px'>tail text</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 100.0, 39.188]),
            ("a", [0.0, 7.0, 90.0, 10.0]),
            ("s", [0.0, 25.188, 38.016, 14.0]),
        ],
    },
];
