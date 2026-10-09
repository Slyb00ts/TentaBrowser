// Bullet boxes measured in Chrome 153 screenshots: the bounding box of the painted pixels
// (a circle's one-pixel stroke removed) is the box Blink gives the bullet.

pub(super) const CASES: &[(&str, &str, [f32; 4])] = &[
    (
        "disc_10px",
        r#"<!doctype html><style>body{margin:0;font:10px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [87.0, 4.0, 3.0, 3.0],
    ),
    (
        "circle_10px",
        r#"<!doctype html><style>body{margin:0;font:10px LS} ul{margin:0;padding-left:100px;list-style:circle;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [87.0, 4.0, 3.0, 3.0],
    ),
    (
        "square_10px",
        r#"<!doctype html><style>body{margin:0;font:10px LS} ul{margin:0;padding-left:100px;list-style:square;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [87.0, 4.0, 3.0, 3.0],
    ),
    (
        "disc_13px",
        r#"<!doctype html><style>body{margin:0;font:13px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [85.0, 6.0, 4.0, 4.0],
    ),
    (
        "circle_13px",
        r#"<!doctype html><style>body{margin:0;font:13px LS} ul{margin:0;padding-left:100px;list-style:circle;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [85.0, 6.0, 4.0, 4.0],
    ),
    (
        "square_13px",
        r#"<!doctype html><style>body{margin:0;font:13px LS} ul{margin:0;padding-left:100px;list-style:square;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [85.0, 6.0, 4.0, 4.0],
    ),
    (
        "disc_16px",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [84.0, 7.0, 5.0, 5.0],
    ),
    (
        "circle_16px",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:circle;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [84.0, 7.0, 5.0, 5.0],
    ),
    (
        "square_16px",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:square;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [84.0, 7.0, 5.0, 5.0],
    ),
    (
        "disc_24px",
        r#"<!doctype html><style>body{margin:0;font:24px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [79.0, 10.0, 7.0, 7.0],
    ),
    (
        "circle_24px",
        r#"<!doctype html><style>body{margin:0;font:24px LS} ul{margin:0;padding-left:100px;list-style:circle;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [79.0, 10.0, 7.0, 7.0],
    ),
    (
        "square_24px",
        r#"<!doctype html><style>body{margin:0;font:24px LS} ul{margin:0;padding-left:100px;list-style:square;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [79.0, 10.0, 7.0, 7.0],
    ),
    (
        "disc_32px",
        r#"<!doctype html><style>body{margin:0;font:32px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [74.0, 15.0, 10.0, 10.0],
    ),
    (
        "circle_32px",
        r#"<!doctype html><style>body{margin:0;font:32px LS} ul{margin:0;padding-left:100px;list-style:circle;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [74.0, 15.0, 10.0, 10.0],
    ),
    (
        "square_32px",
        r#"<!doctype html><style>body{margin:0;font:32px LS} ul{margin:0;padding-left:100px;list-style:square;} li{} </style><ul><li id=a>Hello</li></ul>"#,
        [74.0, 15.0, 10.0, 10.0],
    ),
    (
        "disc_line_height_40",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{line-height:40px} </style><ul><li id=a>Hello</li></ul>"#,
        [84.0, 18.0, 5.0, 5.0],
    ),
    (
        "disc_item_padding_and_border_top",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{padding-top:20px;border-top:3px solid transparent} </style><ul><li id=a>Hello</li></ul>"#,
        [84.0, 30.0, 5.0, 5.0],
    ),
    (
        "disc_after_block_child_margin",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{} </style><ul><li id=a><div style="margin-top:30px">Hello</div></li></ul>"#,
        [84.0, 37.0, 5.0, 5.0],
    ),
    (
        "disc_item_padding_left",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{padding-left:30px;border-left:7px solid red} </style><ul><li id=a>Hello</li></ul>"#,
        [84.0, 7.0, 5.0, 5.0],
    ),
    (
        "disc_first_child_font_size",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{} </style><ul><li id=a><h1 style="margin:0;font-size:40px">Hello</h1></li></ul>"#,
        [84.0, 30.0, 5.0, 5.0],
    ),
    (
        "square_text_indent",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:square;} li{text-indent:30px} </style><ul><li id=a>Hello</li></ul>"#,
        [84.0, 7.0, 5.0, 5.0],
    ),
    (
        "disc_font_size_on_item",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding-left:100px;list-style:disc;} li{font-size:32px} </style><ul><li id=a>Hello</li></ul>"#,
        [74.0, 15.0, 10.0, 10.0],
    ),
    (
        "disc_in_an_rtl_item",
        r#"<!doctype html><style>body{margin:0;font:16px LS} ul{margin:0;padding:0;width:300px;list-style:disc} li{direction:rtl} </style><ul><li id=a>Hello</li></ul>"#,
        [311.0, 7.0, 5.0, 5.0],
    ),
];
