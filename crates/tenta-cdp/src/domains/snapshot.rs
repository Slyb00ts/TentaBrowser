//! Script geometry for the clients' injected functions.
//!
//! `getClientRects`, `getBoundingClientRect` and `clientWidth` have no native
//! implementation in the engine yet (G1b), and Puppeteer's click reads them
//! from the page. `interaction.js` defines them over a layout snapshot, and
//! this module supplies the snapshot: before a client-initiated evaluation
//! whose source names one of those members, the current layout is pushed into
//! the page. A snapshot is therefore as old as that evaluation; geometry a
//! page's own scripts read between two such evaluations may be stale.
//! Without the `render` feature nothing is pushed and the members answer as a
//! box-less element does.

use serde_json::{Value, json};

use super::geometry::{self, Snapshot};
use super::nodes;
use crate::backend::Target;
use crate::domain::CommandContext;

/// Members of script geometry the page-side definitions provide.
const MEMBERS: &[&str] = &[
    "getClientRects",
    "getBoundingClientRect",
    "clientWidth",
    "clientHeight",
    "offsetWidth",
    "offsetHeight",
];

fn payload(snapshot: &Snapshot) -> Value {
    let boxes: Vec<Value> = snapshot
        .elements
        .iter()
        .map(|element| match element {
            Some(element) => json!({ "r": element.rects, "c": element.client }),
            None => Value::Null,
        })
        .collect();
    json!({ "viewport": [snapshot.viewport.0, snapshot.viewport.1], "boxes": boxes })
}

/// Pushes the current layout into the page when `source` is about to read
/// geometry. Best effort: a page that cannot lay out keeps its last snapshot.
pub(super) async fn refresh_for(cx: &CommandContext, target: &Target, source: &str) {
    if !MEMBERS.iter().any(|member| source.contains(member)) {
        return;
    }
    let Ok(snapshot) = geometry::snapshot(target).await else {
        return;
    };
    let expression = format!("globalThis.__tentaCdp.setLayout({})", payload(&snapshot));
    let _ = nodes::run(cx, target, &expression).await;
}
