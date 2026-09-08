//! The recorded SERPs in `providers/fixtures/` replayed through the loopback
//! fixture server (`docs/spec/15-testing-and-benchmarks.md` §1).
//!
//! The expectations are pinned against the reference implementation of the
//! plan format, `providers/check_plans.py`: a provider layout change is a
//! fixture diff, and a change in these numbers means the Rust extraction
//! drifted from the reference.

mod common;

use common::{Harness, custom};
use tenta_search::{BlockClass, Kind, Provider, SearchError, SearchRequest, SearchResponse};

async fn replay(plan_id: &str, slug: &str, provider: Provider) -> SearchResponse {
    let mut harness = Harness::start().await;
    harness.serve_fixture(plan_id, slug);
    let search = harness.search();
    search
        .query(
            SearchRequest::new(slug.replace('-', " "))
                .provider(provider)
                .limit(50),
        )
        .await
        .expect("query")
}

async fn replay_error(plan_id: &str, slug: &str, provider: Provider) -> SearchError {
    let mut harness = Harness::start().await;
    harness.serve_fixture(plan_id, slug);
    let search = harness.search();
    search
        .query(SearchRequest::new("query").provider(provider).limit(50))
        .await
        .expect_err("the fixture is a block page")
}

#[tokio::test]
async fn duckduckgo_html_extracts_ten_results_with_unwrapped_urls() {
    let response = replay(
        "duckduckgo_html",
        "rust-browser-engine",
        Provider::DuckDuckGo,
    )
    .await;

    assert_eq!(response.results.len(), 10);
    let first = &response.results[0];
    assert_eq!(first.rank, 1);
    assert_eq!(
        first.title,
        "Servo aims to empower developers with a lightweight, high-performance ..."
    );
    // The href is a `//duckduckgo.com/l/?uddg=…` redirect wrapper; the plan
    // unwraps it, so no result may point back at the provider.
    assert_eq!(first.url.as_str(), "https://servo.org/");
    assert_eq!(first.display_url, "servo.org");
    assert!(
        first
            .snippet
            .starts_with("Servo is a web browser rendering engine written in Rust")
    );
    assert_eq!(first.kind, Kind::Organic);
    assert_eq!(first.source, Provider::DuckDuckGo);
    assert_eq!(
        response.results[1].url.as_str(),
        "https://github.com/servo/servo"
    );
    assert!(response.results.iter().all(|result| {
        !result
            .url
            .host_str()
            .unwrap_or_default()
            .contains("duckduckgo")
    }));
    assert!(
        response
            .results
            .iter()
            .all(|result| !result.title.is_empty())
    );

    let meta = &response.providers[0];
    assert_eq!(meta.provider, Provider::DuckDuckGo);
    assert_eq!(meta.plan_version, 1);
    assert!(!meta.cache_hit);
    assert_eq!(meta.results, 10);
}

#[tokio::test]
async fn duckduckgo_html_extracts_the_polish_serp() {
    let response = replay("duckduckgo_html", "warszawa-pogoda", Provider::DuckDuckGo).await;

    assert_eq!(response.results.len(), 10);
    assert_eq!(
        response.results[0].title,
        "Pogoda długoterminowa Warszawa na 16, 25 i 45 dni - INTERIA.PL"
    );
    assert_eq!(
        response.results[0].url.as_str(),
        "https://pogoda.interia.pl/prognoza-dlugoterminowa-warszawa,cId,36917"
    );
    assert_eq!(
        response.results[1].url.as_str(),
        "https://pogoda.onet.pl/prognoza-pogody/warszawa-357732"
    );
}

#[tokio::test]
async fn duckduckgo_lite_takes_snippet_and_display_url_from_the_following_rows() {
    let response = replay(
        "duckduckgo_lite",
        "rust-browser-engine",
        custom("duckduckgo_lite"),
    )
    .await;

    assert_eq!(response.results.len(), 10);
    let first = &response.results[0];
    assert_eq!(
        first.title,
        "Servo aims to empower developers with a lightweight, high-performance ..."
    );
    assert_eq!(first.url.as_str(), "https://servo.org/");
    // Both fields live in <tr>s after the one carrying the link.
    assert_eq!(first.display_url, "servo.org");
    assert!(
        first
            .snippet
            .starts_with("Servo is a web browser rendering engine written in Rust")
    );
    assert!(
        response.results[1]
            .snippet
            .starts_with("Servo is a prototype web browser engine")
    );
    assert!(
        response
            .results
            .iter()
            .all(|result| !result.snippet.is_empty())
    );
}

#[tokio::test]
async fn bing_decodes_its_base64_click_wrappers() {
    let response = replay("bing", "rust-browser-engine", Provider::Bing).await;

    assert_eq!(response.results.len(), 10);
    let first = &response.results[0];
    assert_eq!(first.title, "Rust — Explore, Build and Survive");
    // The href is https://www.bing.com/ck/a?…&u=a1<base64url>.
    assert_eq!(first.url.as_str(), "https://rust.facepunch.com/");
    assert_eq!(first.display_url, "https://rust.facepunch.com");
    assert_eq!(response.results[1].url.as_str(), "https://rust-lang.org/");
    assert!(
        response
            .results
            .iter()
            .all(|result| !result.url.as_str().contains("bing.com/ck/a"))
    );
}

#[tokio::test]
async fn bing_extracts_the_polish_serp() {
    let response = replay("bing", "warszawa-pogoda", Provider::Bing).await;

    assert_eq!(response.results.len(), 10);
    assert_eq!(
        response.results[0].url.as_str(),
        "https://pogoda.interia.pl/prognoza-dlugoterminowa-warszawa,cId,36917"
    );
    assert_eq!(
        response.results[1].title,
        "Pogoda Warszawa - Prognoza pogody godzinowa na dziś i jutro"
    );
}

#[tokio::test]
async fn brave_extracts_twenty_direct_results() {
    let response = replay("brave", "rust-browser-engine", Provider::Brave).await;

    assert_eq!(response.results.len(), 20);
    assert_eq!(response.results[0].url.as_str(), "https://servo.org/");
    assert_eq!(
        response.results[1].title,
        "The Servo Parallel Browser Engine Project"
    );
    assert_eq!(
        response.results[1].url.as_str(),
        "https://github.com/servo/servo"
    );
    assert_eq!(
        response.results[1].display_url,
        "GitHub github.com › servo › servo"
    );
    assert_eq!(response.results[19].rank, 20);
}

#[tokio::test]
async fn a_limit_cuts_the_result_list_and_re_ranks() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("brave", "rust-browser-engine");
    let search = harness.search();

    let response = search
        .query(
            SearchRequest::new("rust browser engine")
                .provider(Provider::Brave)
                .limit(5),
        )
        .await
        .expect("query");

    assert_eq!(response.results.len(), 5);
    assert_eq!(response.results[4].rank, 5);
    // The provider still reports everything it found.
    assert_eq!(response.providers[0].results, 20);
}

#[tokio::test]
async fn braves_rate_limit_page_is_a_rate_limited_block() {
    let error = replay_error("brave", "warszawa-pogoda", Provider::Brave).await;

    assert_eq!(error.block_class(), Some(BlockClass::RateLimited));
    match &error {
        SearchError::Blocked {
            provider, detail, ..
        } => {
            assert_eq!(provider, &Provider::Brave);
            assert!(
                detail.contains("flagged as being suspicious"),
                "detail names the signal that fired: {detail}"
            );
        }
        other => panic!("expected a block, got {other:?}"),
    }
}

#[tokio::test]
async fn googles_no_javascript_page_is_a_js_required_block() {
    let error = replay_error("google_nojs", "rust-browser-engine", custom("google_nojs")).await;

    assert_eq!(error.block_class(), Some(BlockClass::JsRequired));
    assert!(
        error.to_string().contains("enablejs"),
        "the detail names the meta refresh: {error}"
    );
}

#[tokio::test]
async fn googles_sorry_page_is_a_captcha_block() {
    // The Full path is P2, so the plan is reached through its own id rather
    // than through `Provider::Google`, which reports `Unsupported` instead.
    let error = replay_error("google_nojs", "rust-browser-engine", custom("google_nojs")).await;
    assert!(matches!(error, SearchError::Blocked { .. }));

    let mut harness = Harness::start().await;
    harness.serve_fixture("google_nojs", "warszawa-pogoda");
    let search = harness.search();
    let error = search
        .query(SearchRequest::new("warszawa pogoda").provider(custom("google_nojs")))
        .await
        .expect_err("block page");
    assert_eq!(error.block_class(), Some(BlockClass::JsRequired));
}

#[tokio::test]
async fn google_needs_full_mode_which_is_not_in_this_build() {
    let harness = Harness::start().await;
    let search = harness.search();

    let error = search
        .query(SearchRequest::new("rust").provider(Provider::Google))
        .await
        .expect_err("Google is Full-only");

    assert!(
        matches!(
            error,
            SearchError::Unsupported {
                provider: Provider::Google,
                mode: tenta_search::Mode::Full
            }
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_page_the_plan_does_not_recognise_is_a_layout_block() {
    let mut harness = Harness::start().await;
    harness.serve_html(
        "bing",
        "<!doctype html><html><body><ol id=b_results></ol></body></html>",
    );
    let search = harness.search();

    let error = search
        .query(SearchRequest::new("rust").provider(Provider::Bing))
        .await
        .expect_err("no results and no block signal");

    assert_eq!(error.block_class(), Some(BlockClass::Layout));
    let detail = error.to_string();
    assert!(
        detail.contains("li.b_algo"),
        "names the item selector: {detail}"
    );
    assert!(
        detail.contains("providers/fixtures/bing/"),
        "hints at the fixture diff: {detail}"
    );
    assert!(detail.contains("check_plans.py"), "{detail}");
}
