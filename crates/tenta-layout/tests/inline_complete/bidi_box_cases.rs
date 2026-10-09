// Expected rectangles recorded from Chrome 154: the union of the first line's getClientRects(), in document coordinates.

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "ltr_span_hebrew_then_latin",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px'><span id=s class=b>אבג דהו abc</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("s", [3.0, -1.0, 101.016, 21.0]),
        ],
    },
    Case {
        name: "rtl_span_latin_then_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p class=r style='width:300px'><span id=s class=b>abc אבג דהו</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("s", [195.969, -1.0, 101.031, 21.0]),
        ],
    },
    Case {
        name: "ltr_nested_boxes",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px'><span id=o class=b>וזח <span id=i class=b>abc</span> וזח</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("o", [3.0, -1.0, 109.672, 21.0]),
            ("i", [37.453, -1.0, 40.766, 21.0]),
        ],
    },
    Case {
        name: "rtl_nested_boxes",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p class=r style='width:300px'><span id=o class=b>abc <span id=i class=b>וזח</span> def</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("o", [182.047, -1.0, 114.953, 21.0]),
            ("i", [221.781, -1.0, 32.359, 21.0]),
        ],
    },
    Case {
        name: "ltr_inline_block_hebrew_inside",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px'>abc <span id=i style='display:inline-block;background:#ccc'>אבג דהו</span> def</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("i", [33.844, 0.0, 55.172, 19.0]),
        ],
    },
    Case {
        name: "rtl_inline_block_in_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p class=r style='width:300px'>וזח <span id=i style='display:inline-block;background:#ccc;padding:2px'>abc def</span> וזח</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 23.0]),
            ("i", [211.078, 0.0, 63.484, 23.0]),
        ],
    },
    Case {
        name: "rtl_block_text_align_left",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p class=r style='width:300px;text-align:left'><span id=a>אבג דהו</span> <span id=b>abc</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [33.859, 0.0, 55.172, 19.0]),
            ("b", [0.0, 0.0, 28.766, 19.0]),
        ],
    },
    Case {
        name: "ltr_justify_mixed_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:150px;text-align:justify'><span id=a>abc def</span> <span id=h>אבג דהו וזח</span> <span id=b>ghi jkl mno</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 150.0, 38.0]),
            ("a", [0.0, 0.0, 60.688, 19.0]),
            ("h", [66.984, 0.0, 83.016, 19.0]),
            ("b", [0.0, 19.0, 88.594, 19.0]),
        ],
    },
    Case {
        name: "rtl_justify_mixed_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p class=r style='width:150px;text-align:justify'><span id=h>אבג דהו וזח</span> <span id=b>ghi jkl mno</span> <span id=a>abc def</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 150.0, 38.0]),
            ("h", [61.234, 0.0, 88.766, 19.0]),
            ("b", [0.0, 0.0, 52.062, 19.0]),
            ("a", [90.516, 19.0, 59.484, 19.0]),
        ],
    },
    Case {
        name: "ltr_img_hebrew_wrap",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:100px'><span id=h>אבג דהו וזח</span><canvas id=c style='width:40px;height:10px'></canvas><span id=a>אבג דהו</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 100.0, 38.0]),
            ("h", [0.0, 0.0, 80.609, 19.0]),
            ("c", [55.172, 24.0, 40.0, 10.0]),
            ("a", [0.0, 19.0, 55.172, 19.0]),
        ],
    },
    Case {
        name: "rtl_two_blocks_stacked",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p class=r style='width:200px'><div id=a>אבג דהו</div><div id=b>abc</div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 200.0, 38.0]),
            ("a", [0.0, 0.0, 200.0, 19.0]),
            ("b", [0.0, 19.0, 200.0, 19.0]),
        ],
    },
    Case {
        name: "ltr_numbers_with_punctuation_in_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px'><span id=h>וזח 1,234.5 וזח</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("h", [0.0, 0.0, 111.953, 19.0]),
        ],
    },
    Case {
        name: "rtl_percent_and_currency",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p class=r style='width:300px'><span id=a>50%</span> <span id=b>$20</span> <span id=c>וזח</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [264.438, 0.0, 35.562, 19.0]),
            ("b", [228.797, 0.0, 30.547, 19.0]),
            ("c", [203.344, 0.0, 20.359, 19.0]),
        ],
    },
    Case {
        name: "ltr_quote_around_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px'>abc "<span id=h>אבג דהו</span>" def</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("h", [41.203, 0.0, 55.172, 19.0]),
        ],
    },
    Case {
        name: "rtl_trailing_spaces_before_br",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p class=r style='width:300px'><span id=a>אבג דהו </span><br><span id=b>abc </span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 38.0]),
            ("a", [244.828, 0.0, 55.172, 19.0]),
            ("b", [271.234, 19.0, 28.766, 19.0]),
        ],
    },
    Case {
        name: "ltr_br_mixed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px'>abc <span id=a>אבג דהו</span><br><span id=b>וזח</span> def</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 38.0]),
            ("a", [33.844, 0.0, 55.172, 19.0]),
            ("b", [0.0, 19.0, 20.359, 19.0]),
        ],
    },
    Case {
        name: "ltr_letter_spacing_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px;letter-spacing:2px'>abc <span id=a>אבג דהו</span> def</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [41.844, 0.0, 69.172, 19.0]),
        ],
    },
    Case {
        name: "ltr_word_spacing_mixed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px;word-spacing:4px'>abc <span id=a>אבג דהו</span> def</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [37.844, 0.0, 59.172, 19.0]),
        ],
    },
    Case {
        name: "ltr_text_indent_mixed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px;text-indent:20px'>abc <span id=a>אבג דהו</span> def</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [53.844, 0.0, 55.172, 19.0]),
        ],
    },
    Case {
        name: "rtl_text_indent_mixed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p class=r style='width:300px;text-indent:20px'>abc <span id=a>אבג דהו</span> def</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [190.969, 0.0, 55.172, 19.0]),
        ],
    },
    Case {
        name: "ltr_font_size_mix_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px'>abc <span id=a style='font-size:30px'>וזח</span> <span id=b style='font-size:10px'>abc</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 35.0]),
            ("a", [33.844, 0.0, 38.172, 35.0]),
            ("b", [77.109, 19.0, 17.984, 11.0]),
        ],
    },
    Case {
        name: "ltr_vertical_align_in_bidi_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl} .b{padding:0 5px;border:1px solid;margin:0 3px}</style></head><body id=body><div id=p style='width:300px'>abc <span id=a style='vertical-align:super;font-size:12px'>וזח</span> def</div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 21.328]),
            ("a", [33.844, 0.0, 15.266, 14.0]),
        ],
    },
];
