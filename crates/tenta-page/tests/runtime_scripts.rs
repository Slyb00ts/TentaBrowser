//! Driving a page's parse through the scripts it pauses on
//! (`docs/spec/05-javascript.md` §6, P2 d-1b-3).
//!
//! Every test here reads the tree the way a script would see it *at the
//! moment the parse is paused on `</script>`*: `tail_parsed` is what the stub
//! engine observed, and a `#tail` element after the navigation is what the
//! parse produced once it was resumed.

mod support;

use tenta_page::{Settle, SettleState};

use support::{Harness, Recorder};

fn body(inner: &str) -> String {
    format!("<!doctype html><html><head></head><body>{inner}</body></html>")
}

/// The scripts the engine ran, in order.
fn sources(record: &Recorder) -> Vec<String> {
    record.lock().unwrap().sources()
}

/// Whether the script at `index` could already see the markup that follows it.
fn saw_tail(record: &Recorder, index: usize) -> bool {
    record.lock().unwrap().scripts[index].tail_parsed
}

/// The text of the tail element, which is only there if the parse resumed.
fn tail(harness: &Harness) -> Option<String> {
    harness.document(|document| support::text_of(document, "#tail"))
}

#[tokio::test(flavor = "current_thread")]
async fn engine_initialization_failure_does_not_report_a_scriptless_success() {
    let harness = Harness::builder(&body(
        "<script>first</script><script>second</script><p id=tail>after</p>",
    ))
    .failing_engine("runtime initialization failed")
    .build();
    let error = harness
        .settle(Settle::Load)
        .await
        .expect_err("navigation fails");
    assert!(matches!(error, tenta_core::Error::Unsupported { reason }
        if reason == "runtime initialization failed"));
    let record = harness.record.lock().unwrap();
    assert_eq!(record.engine_creations, 1, "do not retry at later scripts");
    assert!(record.scripts.is_empty());
    assert!(
        harness.page.navigation().is_none(),
        "no successful navigation is published"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_static_navigation_does_not_require_engine_initialization() {
    let harness = Harness::builder(&body("<p id=tail>static</p>"))
        .failing_engine("runtime initialization failed")
        .build();
    harness
        .settle(Settle::Load)
        .await
        .expect("static navigation");
    assert_eq!(tail(&harness), Some("static".to_owned()));
    assert_eq!(harness.record.lock().unwrap().engine_creations, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn an_inline_script_runs_at_the_pause_and_sees_only_what_came_before_it() {
    let harness = Harness::new(&body(
        "<div id=before>before</div><script>first</script><p id=tail>after</p>",
    ));
    let navigation = harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(sources(&harness.record), vec!["first".to_owned()]);
    assert!(
        !saw_tail(&harness.record, 0),
        "the script runs with the parser paused on it, before <p> is in the tree"
    );
    assert_eq!(
        tail(&harness),
        Some("after".to_owned()),
        "the rest of the document is parsed once the script has run"
    );
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_does_not_swallow_the_markup_that_follows_it() {
    let harness = Harness::new(&body("<script>first</script><p id=tail>after</p>"));
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(sources(&harness.record), vec!["first".to_owned()]);
    assert_eq!(
        tail(&harness),
        Some("after".to_owned()),
        "the buffered tail the pause left behind is still parsed"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn scripts_run_in_document_order() {
    let harness = Harness::new(&body(
        "<script>first</script><p id=mid>mid</p><script>second</script><p id=tail>after</p>",
    ));
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["first".to_owned(), "second".to_owned()],
        "the second script is handed out only after the first has run"
    );
    assert!(
        !saw_tail(&harness.record, 0),
        "the first script runs before the markup between the two is parsed"
    );
    assert!(
        saw_tail(&harness.record, 1),
        "the second script runs after the markup between the two is parsed"
    );
    assert_eq!(tail(&harness), Some("after".to_owned()));
}

#[tokio::test(flavor = "current_thread")]
async fn two_adjacent_scripts_both_run() {
    let harness = Harness::new(&body(
        "<script>first</script><script>second</script><p id=tail>x</p>",
    ));
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["first".to_owned(), "second".to_owned()],
        "resuming reports the second pause of the same chunk"
    );
    assert_eq!(tail(&harness), Some("x".to_owned()));
}

#[tokio::test(flavor = "current_thread")]
async fn a_short_document_still_runs_its_scripts() {
    let page = body("<script>first</script><p id=tail>after</p>");
    let length = page.len();
    assert!(
        length < 1024,
        "this test is about a body shorter than the sniff window, and this one is {length} bytes"
    );
    let harness = Harness::new(&page);
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["first".to_owned()],
        "a script of a body shorter than the sniff window is reported from feed_last"
    );
    assert_eq!(tail(&harness), Some("after".to_owned()));
}

#[tokio::test(flavor = "current_thread")]
async fn javascript_off_never_runs_a_script() {
    let harness = Harness::builder(&body("<script>first</script><p id=tail>after</p>"))
        .javascript(false)
        .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert!(
        sources(&harness.record).is_empty(),
        "a page with javascript off never enters an engine"
    );
    assert!(harness.has("#tail"), "the parse still ran to the end");
}

#[tokio::test(flavor = "current_thread")]
async fn an_external_script_is_fetched_and_run() {
    let harness = Harness::builder(&body(
        "<script src=\"x.js\">this must not run</script><p id=tail>after</p>",
    ))
    .route("x.js", "external")
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["external".to_owned()],
        "the fetched resource ran, and the element's own text did not (P2 d-2a)"
    );
    assert!(harness.has("#tail"), "the parse resumed past it");
}

#[tokio::test(flavor = "current_thread")]
async fn a_non_classic_script_type_is_not_run() {
    let harness = Harness::new(&body(
        "<script type=\"text/template\">this must not run</script>\
         <script type=\"module\">neither must this</script>\
         <p id=tail>after</p>",
    ));
    harness.settle(Settle::Load).await.expect("navigation");

    assert!(
        sources(&harness.record).is_empty(),
        "a template and a module are not classic scripts"
    );
    assert!(harness.has("#tail"), "the parse resumed past both");
}

/// `defer` and `async` are timing attributes of an *external* script: on an
/// inline one HTML ignores both, and `nomodule` is a gate that is open while
/// there is no module support. So all three of these run where they stand.
#[tokio::test(flavor = "current_thread")]
async fn inline_timing_attributes_do_not_change_when_a_script_runs() {
    let harness = Harness::new(&body(
        "<script defer>a</script><script async>b</script><script nomodule>c</script>\
         <p id=tail>after</p>",
    ));
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
        "an inline script runs at the parse point whatever timing it carries (P2 d-2b)"
    );
    assert!(
        !saw_tail(&harness.record, 0),
        "and it runs there, before the markup that follows it is parsed"
    );
    assert!(harness.has("#tail"), "the parse resumed past all three");
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_script_runs_nothing() {
    let harness = Harness::new(&body(
        "<script></script><script>   </script><p id=tail>after</p>",
    ));
    harness.settle(Settle::Load).await.expect("navigation");

    assert!(
        sources(&harness.record).is_empty(),
        "an empty element has nothing to run"
    );
    assert!(harness.has("#tail"), "the parse resumed past both");
}

/// A page with no declared charset whose first non-ASCII byte lies past the
/// 1 KiB prescan is still decoded as UTF-8, because the parser looks further;
/// `characterSet` must say what the text was decoded with.
#[tokio::test(flavor = "current_thread")]
async fn a_utf8_page_with_late_non_ascii_text_reports_utf8() {
    let padding = "x".repeat(1500);
    let page = format!(
        "<!doctype html><html><head><title>t</title></head><body>\
         <div data-pad=\"{padding}\"></div><p id=\"t\">za\u{17c}\u{f3}\u{142}\u{107}</p></body></html>"
    );
    let harness = Harness::new(&page);
    let navigation = harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(navigation.encoding, "UTF-8");
    let text = harness.document(|document| support::text_of(document, "#t"));
    assert_eq!(text.as_deref(), Some("za\u{17c}\u{f3}\u{142}\u{107}"));
}

/// A declaration inside the prescan window still decides the encoding.
#[tokio::test(flavor = "current_thread")]
async fn a_declared_windows_1250_page_still_reports_windows_1250() {
    let harness = Harness::new(
        "<!doctype html><html><head><meta charset=\"windows-1250\"></head>\
         <body><p id=\"t\">plain</p></body></html>",
    );
    let navigation = harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(navigation.encoding, "windows-1250");
}

/// A `<meta charset>` past the sniff window would restart the parse — and run
/// every script a second time — if the encoding were still tentative when the
/// script was handed out. Handing a script out makes it certain, so the
/// declaration is dropped instead.
#[tokio::test(flavor = "current_thread")]
async fn a_late_meta_charset_after_the_first_script_never_restarts() {
    // The padding pushes the declaration past the 1024-byte sniff window, so
    // the sniff is tentative when the parse pauses on the script.
    let padding = "x".repeat(1100);
    let page = format!(
        "<!doctype html><html><head></head><body><script>first</script>\
         <div data-pad=\"{padding}\"></div>\
         <meta charset=\"windows-1250\"><p id=tail>after</p></body></html>"
    );
    let harness = Harness::new(&page);
    let navigation = harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["first".to_owned()],
        "a restart would run the script a second time"
    );
    assert_eq!(
        navigation.encoding, "windows-1252",
        "the sniffed fallback for an en-US locale stands"
    );
    assert!(
        harness.has("#tail"),
        "the parse resumed past the declaration"
    );
}
