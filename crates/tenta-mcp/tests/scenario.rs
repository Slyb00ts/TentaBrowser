//! The render tier through MCP: style-aware snapshots, clicks by layout,
//! screenshots, and a scripted agent session from navigation to capture.

mod common;

use std::io::Cursor;

use common::{Harness, find_ref};
use serde_json::json;
use tenta_core::Tier;

struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Decoded {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        self.rgba[at..at + 4].try_into().unwrap()
    }
}

fn decode_png(bytes: &[u8]) -> Decoded {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "a PNG signature");
    let mut reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .expect("a PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("a sized buffer")];
    let info = reader.next_frame(&mut rgba).expect("a frame");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    Decoded {
        width: info.width,
        height: info.height,
        rgba,
    }
}

async fn rendered() -> (Harness, common::Client) {
    let harness = Harness::start(Tier::Render).await;
    let client = harness.client();
    (harness, client)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_snapshot_leaves_out_what_the_stylesheet_hides() {
    let (harness, client) = rendered().await;
    client.navigate(&harness.url("/index.html")).await;
    let snapshot = client.tool("browser_snapshot", json!({})).await;
    snapshot.ok();
    assert_eq!(snapshot.structured()["layout_used"], true);
    let text = snapshot.text();
    assert!(
        text.contains("heading \"Welcome to the fixture\""),
        "{text}"
    );
    for hidden in [
        "hidden by a stylesheet",
        "Hidden by the attribute",
        "Hidden inline",
    ] {
        assert!(!text.contains(hidden), "{hidden} leaked:\n{text}");
    }
    assert!(snapshot.structured()["hidden_omitted"].as_u64().unwrap() >= 1);

    // The page's own handler reveals the note; the next snapshot shows it.
    let toggle = find_ref(&text, &["button", "Toggle details"]);
    client
        .tool("browser_click", json!({"ref": toggle}))
        .await
        .ok();
    let after = client.tool("browser_snapshot", json!({})).await.text();
    assert!(after.contains("The note is now visible."), "{after}");

    // A wrapper that is display: none takes its link with it.
    client.navigate(&harness.url("/cover.html")).await;
    let cover = client.tool("browser_snapshot", json!({})).await.text();
    assert!(!cover.contains("Never shown"), "{cover}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_computed_visibility_and_opacity_of_a_linked_sheet_are_seen() {
    let (harness, client) = rendered().await;
    client.navigate(&harness.url("/sheetvis.html")).await;
    let text = client.tool("browser_snapshot", json!({})).await.text();
    assert!(text.contains("Visible text"), "{text}");
    assert!(
        text.contains("Shown child by sheet"),
        "a child may turn visibility back on: {text}"
    );
    assert!(
        !text.contains("Invisible by sheet") && !text.contains("Hidden sibling by sheet"),
        "{text}"
    );
    let ghost = text
        .lines()
        .find(|line| line.contains("Ghost by sheet"))
        .unwrap_or_else(|| panic!("opacity 0 stays in the tree: {text}"));
    assert!(ghost.contains("opacity-0"), "{ghost}");
}

#[tokio::test(flavor = "multi_thread")]
async fn boxes_are_reported_when_asked_for() {
    let (harness, client) = rendered().await;
    client.navigate(&harness.url("/cover.html")).await;
    let snapshot = client
        .tool("browser_snapshot", json!({"include_boxes": true}))
        .await
        .text();
    let line = snapshot
        .lines()
        .find(|line| line.contains("Covered"))
        .expect("the button");
    assert!(line.contains("[box=50,50,100,40]"), "{line}");
    let plain = client.tool("browser_snapshot", json!({})).await.text();
    assert!(!plain.contains("[box="));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_click_blocked_by_another_element_names_it() {
    let (harness, client) = rendered().await;
    client.navigate(&harness.url("/cover.html")).await;
    let covered = client.ref_of(&["button", "Covered"]).await;
    let blocked = client.tool("browser_click", json!({"ref": covered})).await;
    assert_eq!(blocked.error_type(), "blocked", "{}", blocked.0);
    assert!(blocked.text().contains("<div#cover>"), "{}", blocked.text());
    // The user can see it is the page's overlay, so the same click by point lands on it.
    client.evaluate("window.hits = 0; document.getElementById('cover').addEventListener('click', () => hits++)").await;
    client
        .tool("browser_click", json!({"x": 60, "y": 60}))
        .await
        .ok();
    assert_eq!(client.evaluate("hits").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn clicks_by_coordinates_hit_what_is_drawn_there() {
    let (harness, client) = rendered().await;
    client.navigate(&harness.url("/form.html")).await;
    let name = client.ref_of(&["textbox", "Your name"]).await;
    client
        .tool("browser_fill", json!({"ref": name, "value": "Ada"}))
        .await
        .ok();
    // The Greet button sits at (20, 80) with a size of 120 by 40.
    let clicked = client
        .tool("browser_click", json!({"x": 60, "y": 100}))
        .await;
    clicked.ok();
    assert_eq!(
        clicked.structured()["point"],
        json!({"x": 60.0, "y": 100.0})
    );
    assert_eq!(
        client
            .evaluate("document.getElementById('out').textContent")
            .await,
        "Hello, Ada!"
    );
    let empty = client
        .tool("browser_click", json!({"x": -50, "y": -50}))
        .await;
    assert_eq!(empty.error_type(), "element_not_found", "{}", empty.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn screenshots_cover_the_viewport_the_page_and_an_element() {
    let (harness, client) = rendered().await;
    client.navigate(&harness.url("/form.html")).await;

    let viewport = client.tool("browser_screenshot", json!({})).await;
    viewport.ok();
    let images = viewport.images();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].0, "image/png");
    let picture = decode_png(&images[0].1);
    assert_eq!((picture.width, picture.height), (1280, 800));
    assert_eq!(
        picture.pixel(60, 100),
        [0x2a, 0x9d, 0x4a, 255],
        "the green Greet button"
    );
    assert_eq!(viewport.structured()["format"], "png");
    assert_eq!(viewport.structured()["downscaled"], false);
    assert!(
        viewport.text().starts_with("Screenshot 1280x800 px"),
        "{}",
        viewport.text()
    );

    let send = client.ref_of(&["button", "Greet"]).await;
    let element = client
        .tool("browser_screenshot", json!({"ref": send}))
        .await;
    element.ok();
    let button = decode_png(&element.images()[0].1);
    assert_eq!((button.width, button.height), (120, 40));
    assert_eq!(button.pixel(5, 5), [0x2a, 0x9d, 0x4a, 255]);

    client.navigate(&harness.url("/long.html")).await;
    // Text on 5,000 pixels of page is more than the default 1.5 MB as a PNG.
    let full = client
        .tool(
            "browser_screenshot",
            json!({"full_page": true, "max_bytes": 20_000_000}),
        )
        .await;
    full.ok();
    assert_eq!(full.structured()["format"], "png");
    let page = decode_png(&full.images()[0].1);
    assert_eq!(page.width, 1280);
    assert!(
        page.height > 800,
        "the long page is taller than the viewport: {}",
        page.height
    );

    let reencoded = client
        .tool("browser_screenshot", json!({"full_page": true}))
        .await;
    reencoded.ok();
    assert_eq!(
        reencoded.structured()["downscaled"],
        true,
        "{}",
        reencoded.0
    );
    assert_eq!(reencoded.images()[0].0, "image/jpeg");

    let jpeg = client
        .tool("browser_screenshot", json!({"type": "jpeg", "quality": 60}))
        .await;
    jpeg.ok();
    let (mime, bytes) = &jpeg.images()[0];
    assert_eq!(mime, "image/jpeg");
    assert_eq!(&bytes[..3], &[0xff, 0xd8, 0xff]);
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_screenshots_are_re_encoded_or_refused() {
    let (harness, client) = rendered().await;
    client.navigate(&harness.url("/long.html")).await;
    let plain = client
        .tool("browser_screenshot", json!({"full_page": true}))
        .await;
    plain.ok();
    let size = plain.structured()["bytes"].as_u64().unwrap();

    // Ask for less than the PNG takes: the tool re-encodes until it fits.
    let limit = (size / 2).max(20_000);
    let squeezed = client
        .tool(
            "browser_screenshot",
            json!({"full_page": true, "max_bytes": limit}),
        )
        .await;
    if size / 2 >= 20_000 {
        squeezed.ok();
        assert_eq!(squeezed.structured()["downscaled"], true, "{}", squeezed.0);
        assert!(squeezed.structured()["bytes"].as_u64().unwrap() <= limit);
        assert!(
            squeezed.text().contains("Re-encoded to fit"),
            "{}",
            squeezed.text()
        );
    } else {
        assert!(!squeezed.is_error() || squeezed.error_type() == "budget_exceeded");
    }

    let impossible = client
        .tool(
            "browser_screenshot",
            json!({"full_page": true, "max_bytes": 20_000}),
        )
        .await;
    if impossible.is_error() {
        assert_eq!(impossible.error_type(), "budget_exceeded");
        assert!(
            impossible.text().contains("capture an element"),
            "{}",
            impossible.text()
        );
    } else {
        assert!(impossible.structured()["bytes"].as_u64().unwrap() <= 20_000);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn screenshot_arguments_are_checked() {
    let (harness, client) = rendered().await;
    client.navigate(&harness.url("/cover.html")).await;
    let both = client
        .tool(
            "browser_screenshot",
            json!({"full_page": true, "ref": "e1"}),
        )
        .await;
    assert_eq!(both.error_type(), "invalid_arguments");
    let stale = client
        .tool("browser_screenshot", json!({"ref": "e500"}))
        .await;
    assert_eq!(stale.error_type(), "element_not_found");
}

/// What an agent does: open a site, read it, follow a link by ref, fill a
/// form by ref, click a button by ref, wait for the page's reply and look at
/// the result.
#[tokio::test(flavor = "multi_thread")]
async fn a_scripted_agent_session_runs_end_to_end() {
    let (harness, client) = rendered().await;
    let init = client.initialize().await;
    assert_eq!(init["serverInfo"]["name"], "tentabrowser");

    // 1. Open the site and read it.
    let opened = client.navigate(&harness.url("/index.html")).await;
    assert_eq!(opened.structured()["title"], "Fixture Home");
    let home = client.tool("browser_snapshot", json!({})).await;
    home.ok();
    let link = find_ref(&home.text(), &["link", "Contact form"]);

    // 2. Follow the link by ref.
    let followed = client.tool("browser_click", json!({"ref": link})).await;
    followed.ok();
    assert_eq!(
        followed.structured()["navigation"]["url"],
        harness.url("/form.html")
    );

    // 3. The old refs are gone; the new page is read afresh.
    let stale = client.tool("browser_click", json!({"ref": link})).await;
    assert_eq!(stale.error_type(), "element_not_found");
    let form = client.tool("browser_snapshot", json!({})).await;
    form.ok();
    let text = form.text();
    assert!(text.contains("Page Title: Contact"), "{text}");
    let name = find_ref(&text, &["textbox", "Your name"]);
    let greet = find_ref(&text, &["button", "Greet"]);
    // A ref from the first page does not name an element of this one, even
    // though this snapshot issued refs of its own.
    let retired = client.tool("browser_click", json!({"ref": link})).await;
    assert_eq!(retired.error_type(), "element_not_found", "{}", retired.0);
    assert!(retired.text().contains("stale"), "{}", retired.text());

    // 4. Type into the field, click the button, wait for the reply.
    client
        .tool("browser_type", json!({"ref": name, "text": "Ada Lovelace"}))
        .await
        .ok();
    client
        .tool("browser_click", json!({"ref": greet}))
        .await
        .ok();
    let waited = client
        .tool(
            "browser_wait_for",
            json!({"text": "Hello, Ada Lovelace!", "timeout_ms": 5000}),
        )
        .await;
    waited.ok();

    // 5. Look at the result.
    let shot = client.tool("browser_screenshot", json!({})).await;
    shot.ok();
    let picture = decode_png(&shot.images()[0].1);
    assert_eq!((picture.width, picture.height), (1280, 800));
    assert_eq!(picture.pixel(60, 100), [0x2a, 0x9d, 0x4a, 255]);
    let outline = client.tool("browser_snapshot", json!({})).await.text();
    assert!(outline.contains("Hello, Ada Lovelace!"), "{outline}");

    // 6. Submit the form: the key press navigates, and the tool says where.
    let submitted = client
        .tool("browser_press_key", json!({"key": "Enter", "ref": name}))
        .await;
    submitted.ok();
    assert!(
        submitted.structured()["navigation"]["url"]
            .as_str()
            .unwrap()
            .contains("name=Ada")
    );
    assert!(
        client
            .tool("browser_snapshot", json!({}))
            .await
            .text()
            .contains("Submitted")
    );

    let closed = client.tool("browser_close", json!({})).await;
    assert_eq!(closed.structured()["closed"], true);
}
