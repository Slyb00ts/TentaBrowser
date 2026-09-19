//! Reclaiming detached subtrees while a settle call drives the page loop
//! (`docs/spec/04-dom-and-css.md` §1.1, P2 d-1b-6).
//!
//! The stub engine records what the document looked like at the moment the page
//! asked it to trim, which is the only vantage point a page test has on the
//! ordering: the sweep that follows belongs to the document, not to the engine.
//! A trim that saw a detached root which the turn then freed — and a mutation
//! count the turn then reset — is a trim that ran first.
//!
//! Nothing here can observe the `parse_in_flight` guard: `turn` is called only
//! from the settle loop, and the parse is always over by then
//! (`crate::page`), so that half is covered by the in-crate test in
//! `crates/tenta-page/src/runtime.rs`.

mod support;

use tenta_page::{Settle, SettleState};

use support::Harness;

fn body(inner: &str) -> String {
    format!("<!doctype html><html><head></head><body>{inner}</body></html>")
}

/// The engine's collection epoch moving is the trigger a test can pull
/// directly, and the one a real collector produces. The page must trim the
/// engine before it sweeps, and the detached subtree both are for must be gone
/// by the time the settle call returns.
#[tokio::test(flavor = "current_thread")]
async fn a_collection_epoch_change_reclaims_after_trimming_the_engine() {
    let harness = Harness::new(&body(
        "<div id=victim><span>gone</span></div>\
         <script>detach victim\ngc</script>\
         <p id=tail>after</p>",
    ));
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let trims = harness.record.lock().unwrap().trims.clone();
    assert_eq!(
        trims.len(),
        1,
        "the epoch moved once, so one trim was owed and the epoch was recorded"
    );
    assert_eq!(
        trims[0].detached_roots, 1,
        "the trim saw the detached root that the sweep after it is for"
    );
    assert!(
        trims[0].mutations_since_sweep > 0,
        "the trim saw a pending mutation count, which only a later sweep resets"
    );
    assert_eq!(
        harness.document(|document| document.mutations_since_sweep()),
        0,
        "the sweep ran in the same turn, after the trim"
    );
    assert_eq!(
        harness.document(|document| document.detached_root_count()),
        0,
        "the detached root was reclaimed"
    );
    assert!(!harness.has("#victim"), "the detached subtree is gone");
    assert!(harness.has("#tail"), "the rest of the page is untouched");
}

/// The other trigger is the page's own mutation count crossing the interval,
/// with no collection involved: a page that has rewritten enough of its tree
/// reclaims without waiting for the engine to report anything.
#[tokio::test(flavor = "current_thread")]
async fn a_page_past_the_mutation_interval_reclaims_without_a_collection() {
    // The parse alone crosses the interval the reclaim threshold names. The
    // script detaches a subtree but never reports a collection, so the epoch
    // trigger cannot be what fires here.
    let filler = "<div class=filler></div>".repeat(6_000);
    let harness = Harness::new(&body(&format!(
        "<div id=victim><span>gone</span></div>\
         <script>detach victim</script>\
         {filler}<p id=tail>after</p>"
    )));
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let trims = harness.record.lock().unwrap().trims.clone();
    assert!(
        !trims.is_empty(),
        "the mutation count crossed the reclaim interval, so the page trimmed its engine"
    );
    assert!(
        trims[0].mutations_since_sweep >= 4096,
        "the fixture is past the interval MUTATIONS_PER_SWEEP names: {} mutations",
        trims[0].mutations_since_sweep
    );
    assert_eq!(trims[0].detached_roots, 1);
    assert_eq!(
        harness.document(|document| document.mutations_since_sweep()),
        0
    );
    assert_eq!(
        harness.document(|document| document.detached_root_count()),
        0
    );
    assert!(!harness.has("#victim"));
}
