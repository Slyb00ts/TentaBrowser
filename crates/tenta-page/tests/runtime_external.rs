//! A parser-blocking external classic script: the fetch the parse waits on
//! (`docs/spec/05-javascript.md` §6, P2 d-2a).
//!
//! A `<script src>` with no `defer`/`async` pauses the parse, is fetched and
//! run in document order, and the parse resumes past it. `defer` and `async`
//! are not this sub-phase's: a deferred script is still skipped, and the tests
//! below pin that boundary where it is easy to cross it by accident.

mod support;

use std::time::Duration;

use bytes::Bytes;
use tenta_page::page::Goto;
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

/// The URL a script's `src` resolved to, which is what proves the fetch was
/// issued for the script rather than for the document.
#[tokio::test(flavor = "current_thread")]
async fn an_external_script_runs_between_the_inline_scripts_around_it() {
    let harness = Harness::builder(&body(
        "<script>first</script><script src=\"middle.js\"></script>\
         <script>last</script><p id=tail>after</p>",
    ))
    .route("middle.js", "middle")
    .build();
    let navigation = harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["first".to_owned(), "middle".to_owned(), "last".to_owned()],
        "the fetched script ran in document order, between the two inline ones"
    );
    assert!(
        !saw_tail(&harness.record, 1),
        "it ran while the parse was still paused on it, before <p> was in the tree"
    );
    assert_eq!(
        harness.requests(),
        vec!["/".to_owned(), "/middle.js".to_owned()],
        "the document and the one script were the only requests"
    );
    assert_eq!(tail(&harness), Some("after".to_owned()));
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });
}

#[tokio::test(flavor = "current_thread")]
async fn an_external_scripts_src_resolves_against_the_documents_base() {
    // The `<base href>` is in the head, where the parser applies it before the
    // body's script is reached.
    let page = "<!doctype html><html><head>\
                <base href=\"http://fixture.test/assets/\"></head><body>\
                <script src=\"nested.js\"></script><p id=tail>after</p>\
                </body></html>";
    let harness = Harness::builder(page)
        .route("/assets/nested.js", "nested")
        .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["nested".to_owned()],
        "the script was fetched from the base the document declares"
    );
    assert_eq!(
        harness.requests(),
        vec!["/".to_owned(), "/assets/nested.js".to_owned()],
        "`nested.js` resolved against `<base href>`, not against the document URL"
    );
    assert_eq!(tail(&harness), Some("after".to_owned()));
}

/// A fetch that fails is the script's own failure: the parse resumption is not
/// conditional on it, and the request does not stay in the settle machine.
///
/// `NetworkIdle` is what makes the second half of that observable: a request
/// record left behind by the failed fetch would keep the network sources
/// non-empty, and the settle would run out the deadline instead of settling.
/// The deadline is shortened so a regression fails in seconds rather than in
/// the default thirty.
#[tokio::test(flavor = "current_thread")]
async fn a_script_whose_fetch_fails_does_not_abort_the_parse_or_hang_the_settle() {
    let harness = Harness::builder(&body(
        "<script src=\"gone.js\"></script><p id=tail>after</p>",
    ))
    .failing_route("gone.js")
    .build();
    let navigation = harness
        .goto(
            Goto::new()
                .wait_until(Settle::NetworkIdle)
                .timeout(Duration::from_secs(5)),
        )
        .await
        .expect("a failed script fetch does not fail the navigation");

    assert!(
        sources(&harness.record).is_empty(),
        "nothing ran, and nothing was invented to run"
    );
    assert_eq!(tail(&harness), Some("after".to_owned()));
    assert_eq!(
        harness.requests(),
        vec!["/".to_owned(), "/gone.js".to_owned()],
        "the fetch was issued once and not retried"
    );
    assert_eq!(
        navigation.settle,
        SettleState::Settled { busy: false },
        "the failed request left the settle machine"
    );
}

/// An origin that answers with a status that is not ok has not served a script:
/// HTML's "fetch a classic script" fails the fetch on any non-ok status, so the
/// body — an error page, in the general case — is not run as JavaScript. The
/// parse still resumes, and the fetch was still issued, which is what tells
/// this apart from a script that was skipped without being asked for.
#[tokio::test(flavor = "current_thread")]
async fn a_script_the_origin_does_not_serve_does_not_run() {
    // No route is registered for `missing.js`, so the fixture answers 404 with
    // a body that is not JavaScript.
    let harness = Harness::builder(&body(
        "<script src=\"missing.js\"></script><p id=tail>after</p>",
    ))
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert!(
        sources(&harness.record).is_empty(),
        "the 404 body was not run as the script the element named"
    );
    assert_eq!(
        harness.requests(),
        vec!["/".to_owned(), "/missing.js".to_owned()],
        "the script was fetched; it was the response that was not a script"
    );
    assert_eq!(tail(&harness), Some("after".to_owned()));
}

/// `defer` and `async` change when a script runs, which is d-2b: until then an
/// external one is skipped exactly as it was, and skipping it means the fetch
/// is never issued.
#[tokio::test(flavor = "current_thread")]
async fn a_deferred_or_async_external_script_is_not_fetched() {
    let harness = Harness::builder(&body(
        "<script src=\"deferred.js\" defer></script>\
         <script src=\"asynced.js\" async></script>\
         <script src=\"nomodule.js\" nomodule></script>\
         <p id=tail>after</p>",
    ))
    .route("deferred.js", "deferred")
    .route("asynced.js", "asynced")
    .route("nomodule.js", "nomodule")
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert!(
        sources(&harness.record).is_empty(),
        "none of the three ran (P2 d-2b)"
    );
    assert_eq!(
        harness.requests(),
        vec!["/".to_owned()],
        "and none of them was even fetched"
    );
    assert!(harness.has("#tail"), "the parse resumed past all three");
}

/// A `src` naming a module is not a classic script's text: it is skipped
/// rather than fetched and run in the wrong scope (P2 d-2c).
#[tokio::test(flavor = "current_thread")]
async fn a_module_script_is_not_fetched_as_a_classic_one() {
    let harness = Harness::builder(&body(
        "<script src=\"app.js\" type=\"module\"></script><p id=tail>after</p>",
    ))
    .route("app.js", "asynced")
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert!(sources(&harness.record).is_empty(), "a module did not run");
    assert_eq!(
        harness.requests(),
        vec!["/".to_owned()],
        "and a module script was not fetched as a classic one"
    );
    assert!(harness.has("#tail"), "the parse resumed past it");
}

/// A `src` the document's base cannot resolve is a script that does not run:
/// the element cannot name a request, and the parse still resumes.
#[tokio::test(flavor = "current_thread")]
async fn an_unresolvable_src_does_not_run_and_does_not_abort_the_parse() {
    let harness = Harness::builder(&body(
        "<script src=\"http://\"></script><p id=tail>after</p>",
    ))
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert!(sources(&harness.record).is_empty());
    assert_eq!(
        harness.requests(),
        vec!["/".to_owned()],
        "no request was issued for a URL that does not resolve"
    );
    assert!(harness.has("#tail"));
}

/// The document's encoding decodes the script, because a classic script has no
/// encoding of its own: `n\xe3o` is `não` only in windows-1252, and mojibake in
/// UTF-8.
#[tokio::test(flavor = "current_thread")]
async fn an_external_script_is_decoded_with_the_documents_encoding() {
    let harness = Harness::builder(&body(
        "<script src=\"latin.js\"></script><p id=tail>after</p>",
    ))
    .content_type("text/html; charset=windows-1252")
    .route("latin.js", Bytes::from_static(b"n\xe3o"))
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["não".to_owned()],
        "the script's bytes were decoded with the document's windows-1252"
    );
}
