// Expected rectangles recorded from Chrome 153 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "one_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=a>x</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 20.0]),
            ("body", [0.0, 0.0, 800.0, 20.0]),
            ("a", [0.0, 0.0, 800.0, 20.0]),
        ],
    },
    Case {
        name: "three_forced_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=a>a<br>b<br>c</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 60.0]),
            ("body", [0.0, 0.0, 800.0, 60.0]),
            ("a", [0.0, 0.0, 800.0, 60.0]),
        ],
    },
    Case {
        name: "pre_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=a style="white-space:pre">a
b
c</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 60.0]),
            ("body", [0.0, 0.0, 800.0, 60.0]),
            ("a", [0.0, 0.0, 800.0, 60.0]),
        ],
    },
    Case {
        name: "line_between_blocks",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=a style="height:10px;margin-bottom:5px"></div><div id=b style="margin-top:30px">x</div><div id=c style="margin-top:3px;height:10px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 73.0]),
            ("body", [0.0, 0.0, 800.0, 73.0]),
            ("a", [0.0, 0.0, 800.0, 10.0]),
            ("b", [0.0, 40.0, 800.0, 20.0]),
            ("c", [0.0, 63.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "anonymous_blocks_around_block",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=p>one<div id=c style="height:10px"></div>two</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 50.0]),
            ("body", [0.0, 0.0, 800.0, 50.0]),
            ("p", [0.0, 0.0, 800.0, 50.0]),
            ("c", [0.0, 20.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "first_child_with_text_collapses_with_parent",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=p style="margin-top:10px"><div id=c style="margin-top:30px">x</div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 50.0]),
            ("body", [0.0, 30.0, 800.0, 20.0]),
            ("p", [0.0, 30.0, 800.0, 20.0]),
            ("c", [0.0, 30.0, 800.0, 20.0]),
        ],
    },
    Case {
        name: "text_last_child_margin_collapses",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=p style="margin-bottom:10px"><div id=c style="margin-bottom:30px">x</div></div><div id=n style="height:5px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 55.0]),
            ("body", [0.0, 0.0, 800.0, 55.0]),
            ("p", [0.0, 0.0, 800.0, 20.0]),
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("n", [0.0, 50.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "text_directly_in_parent_blocks_collapse",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=p style="margin-top:10px">x<div id=c style="margin-top:30px;height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 70.0]),
            ("body", [0.0, 10.0, 800.0, 60.0]),
            ("p", [0.0, 10.0, 800.0, 60.0]),
            ("c", [0.0, 60.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "whitespace_only_block_is_empty",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=a style="height:10px;margin-bottom:20px"></div><div id=e style="margin:30px 0">   
  </div><div id=b style="height:10px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 50.0]),
            ("body", [0.0, 0.0, 800.0, 50.0]),
            ("a", [0.0, 0.0, 800.0, 10.0]),
            ("e", [0.0, 40.0, 800.0, 0.0]),
            ("b", [0.0, 40.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "br_block_is_not_empty",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=a style="height:10px;margin-bottom:20px"></div><div id=e style="margin:30px 0"><br></div><div id=b style="height:10px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 100.0]),
            ("body", [0.0, 0.0, 800.0, 100.0]),
            ("a", [0.0, 0.0, 800.0, 10.0]),
            ("e", [0.0, 40.0, 800.0, 20.0]),
            ("b", [0.0, 90.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "default_paragraph_margins",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><p id=a>x</p><p id=b>y</p></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 88.0]),
            ("body", [8.0, 16.0, 784.0, 56.0]),
            ("a", [8.0, 16.0, 784.0, 20.0]),
            ("b", [8.0, 52.0, 784.0, 20.0]),
        ],
    },
    Case {
        name: "body_margin_with_paragraph",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><p id=a>x</p></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 52.0]),
            ("body", [8.0, 16.0, 784.0, 20.0]),
            ("a", [8.0, 16.0, 784.0, 20.0]),
        ],
    },
    Case {
        name: "padding_border_around_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=a style="padding:3px 5px;border:2px solid">x<br>y</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 50.0]),
            ("body", [0.0, 0.0, 800.0, 50.0]),
            ("a", [0.0, 0.0, 800.0, 50.0]),
        ],
    },
    Case {
        name: "block_in_inline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=p><span>a<div id=d style="height:10px"></div>b</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 50.0]),
            ("body", [0.0, 0.0, 800.0, 50.0]),
            ("p", [0.0, 0.0, 800.0, 50.0]),
            ("d", [0.0, 20.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_block_with_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=p style="direction:rtl;width:300px"><div id=a style="width:50px">x</div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 20.0]),
            ("body", [0.0, 0.0, 800.0, 20.0]),
            ("p", [0.0, 0.0, 300.0, 20.0]),
            ("a", [250.0, 0.0, 50.0, 20.0]),
        ],
    },
    Case {
        name: "fixed_height_clips_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=a style="height:30px;overflow:hidden">a<br>b<br>c<br>d</div><div id=n style="height:5px"></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 35.0]),
            ("body", [0.0, 0.0, 800.0, 35.0]),
            ("a", [0.0, 0.0, 800.0, 30.0]),
            ("n", [0.0, 30.0, 800.0, 5.0]),
        ],
    },
    Case {
        name: "auto_height_with_child_margins_and_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=p style="padding:1px"><div id=a style="margin:10px 0">x</div><div id=b style="margin:15px 0">y</div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 82.0]),
            ("body", [0.0, 0.0, 800.0, 82.0]),
            ("p", [0.0, 0.0, 800.0, 82.0]),
            ("a", [1.0, 11.0, 798.0, 20.0]),
            ("b", [1.0, 46.0, 798.0, 20.0]),
        ],
    },
    Case {
        name: "fixed_width_block_with_text",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;}html,body{line-height:20px;font-size:16px} div{display:block}</style></head><body id=body><div id=a style="width:200px">x</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 20.0]),
            ("body", [0.0, 0.0, 800.0, 20.0]),
            ("a", [0.0, 0.0, 200.0, 20.0]),
        ],
    },
];
