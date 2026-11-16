//! Script-visible geometry and `getComputedStyle` against headless Chrome.
//!
//! `tests/script_api/chrome.json` holds the cases of
//! `render-fixtures/tools/script_api_cases.py` with the values Chrome returned
//! from each page's `collect()` (recorded by `record_script_api.py`; Chrome is a
//! reference only). The test loads every page in this engine, calls the same
//! `collect()` and compares the answers value by value: numbers within a
//! hundredth of a pixel, everything else exactly. A difference that is not in
//! [`KNOWN`] fails, and so does a [`KNOWN`] entry that no longer differs, so the
//! list only shrinks.

#![cfg(all(feature = "render", feature = "v8"))]

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;
use tenta_browser::{ContextOptions, Engine, Goto, JsEngine, PageOptions, Settle, Viewport};
use tenta_core::{FsPolicy, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig};

const PATIENCE: Duration = Duration::from_secs(60);
const CASES: &str = include_str!("script_api/chrome.json");

/// Known differences: `case` and a JSON path prefix, with the reason. Each is a
/// place this engine answers differently from Chrome today.
const KNOWN: &[(&str, &str, &str)] = &[
    (
        "g-pseudo-block",
        ".b.width",
        "pseudo-elements report computed values: their used width is not resolved",
    ),
    (
        "g-scroll-offsets",
        ".t",
        "elements do not scroll yet (M5): scrollTop reads 0 after a write",
    ),
    (
        "s-property-access",
        ".proto",
        "CSSOM names the class CSSStyleProperties; Chrome 154 still says CSSStyleDeclaration",
    ),
];

fn compare(path: &str, expected: &Value, actual: &Value, out: &mut Vec<(String, String)>) {
    match (expected, actual) {
        (Value::Number(e), Value::Number(a)) => {
            let (e, a) = (
                e.as_f64().unwrap_or(f64::NAN),
                a.as_f64().unwrap_or(f64::NAN),
            );
            if (e - a).abs() > 0.011 {
                out.push((path.to_owned(), format!("chrome {e}, here {a}")));
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            if e.len() != a.len() {
                out.push((
                    path.to_owned(),
                    format!("chrome has {} items, here {}", e.len(), a.len()),
                ));
            }
            for (index, (e, a)) in e.iter().zip(a).enumerate() {
                compare(&format!("{path}[{index}]"), e, a, out);
            }
        }
        (Value::Object(e), Value::Object(a)) => {
            for (key, e) in e {
                match a.get(key) {
                    Some(a) => compare(&format!("{path}.{key}"), e, a, out),
                    None => out.push((format!("{path}.{key}"), "missing here".to_owned())),
                }
            }
            for key in a.keys().filter(|key| !e.contains_key(*key)) {
                out.push((format!("{path}.{key}"), "extra here".to_owned()));
            }
        }
        (e, a) if e == a => {}
        (e, a) => out.push((path.to_owned(), format!("chrome {e}, here {a}"))),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_visible_geometry_and_styles_match_chrome() {
    let cases: Vec<Value> = serde_json::from_str(CASES).expect("chrome.json");
    let temp = tenta_testutil::temp_dir("script-api");
    for case in &cases {
        let name = case["name"].as_str().expect("name");
        temp.write(
            &format!("{name}.html"),
            case["html"].as_str().expect("html"),
        );
    }
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
        .await
        .expect("fixture server");
    let engine = Engine::builder()
        .max_tier(Tier::Render)
        .io_threads(1)
        .page_threads(1)
        .fs_policy(FsPolicy::none().allow_read(temp.path()))
        .js(JsEngine::V8)
        .ephemeral(true)
        .build()
        .expect("engine");
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context");

    let mut differences: BTreeMap<(String, String), String> = BTreeMap::new();
    for case in &cases {
        let name = case["name"].as_str().expect("name");
        let size = case["viewport"].as_array().expect("viewport");
        let page = context
            .new_page(PageOptions {
                javascript: true,
                viewport: Viewport {
                    width: size[0].as_u64().expect("width") as u32,
                    height: size[1].as_u64().expect("height") as u32,
                    device_scale_factor: 1.0,
                    mobile: false,
                },
                ..PageOptions::default()
            })
            .await
            .expect("page");
        page.goto(
            &server.url(&format!("/{name}.html")),
            Goto::default().wait_until(Settle::Load).timeout(PATIENCE),
        )
        .await
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let answer = page
            .evaluate("JSON.stringify(collect())")
            .await
            .unwrap_or_else(|error| panic!("{name}: {error:?}"))
            .into_value::<String>()
            .unwrap_or_else(|error| panic!("{name}: collect() gave no string: {error:?}"));
        let actual: Value = serde_json::from_str(&answer).expect("collect() is JSON");
        let mut found = Vec::new();
        compare("", &case["expect"], &actual, &mut found);
        for (path, detail) in found {
            differences.insert((name.to_owned(), path), detail);
        }
        page.close().await.expect("close");
    }
    context.close();
    engine.close();

    let known = |case: &str, path: &str| {
        KNOWN
            .iter()
            .any(|(k_case, prefix, _)| *k_case == case && path.starts_with(prefix))
    };
    let unexpected: Vec<String> = differences
        .iter()
        .filter(|((case, path), _)| !known(case, path))
        .map(|((case, path), detail)| format!("{case}{path}: {detail}"))
        .collect();
    let stale: Vec<String> = KNOWN
        .iter()
        .filter(|(case, prefix, _)| {
            !differences
                .keys()
                .any(|(c, p)| c == case && p.starts_with(prefix))
        })
        .map(|(case, prefix, _)| format!("{case}{prefix}"))
        .collect();
    assert!(
        unexpected.is_empty() && stale.is_empty(),
        "{} unexpected differences, {} stale KNOWN entries\n{}\nstale: {stale:?}",
        unexpected.len(),
        stale.len(),
        unexpected.join("\n")
    );
}
