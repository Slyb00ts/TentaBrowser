//! Block detection over the recorded block pages
//! (`docs/spec/08-search.md` §4, `providers/SURVEY.md`).

mod common;

use common::{fixture, parse};
use tenta_search::extract::{contains_ignore_ascii_case, detect_block};
use tenta_search::{BlockClass, PlanSet};

fn classify(plan_id: &str, slug: &str, status: u16) -> (BlockClass, String) {
    let plans = PlanSet::embedded().expect("plans");
    let plan = plans.get(plan_id).expect("plan");
    let body = fixture(plan_id, slug);
    let document = parse(&body);
    let signal = detect_block(plan, &document, &body, status)
        .unwrap_or_else(|| panic!("{plan_id}/{slug} is a block page"));
    (signal.class, signal.detail)
}

#[test]
fn googles_sorry_page_is_a_captcha() {
    // Both recorded Chromium runs were sent to /sorry/index with a reCAPTCHA.
    for slug in ["rust-browser-engine", "warszawa-pogoda"] {
        let (class, detail) = classify("google_chromium", slug, 200);
        assert_eq!(class, BlockClass::Captcha, "{slug}: {detail}");
        assert_eq!(detail, "selector form#captcha-form");
    }
}

#[test]
fn googles_no_javascript_page_is_js_required() {
    for slug in ["rust-browser-engine", "warszawa-pogoda"] {
        let (class, detail) = classify("google_nojs", slug, 200);
        assert_eq!(class, BlockClass::JsRequired, "{slug}");
        assert!(detail.contains("enablejs"), "{detail}");
    }
}

#[test]
fn braves_suspicious_traffic_page_is_rate_limiting() {
    let (class, detail) = classify("brave", "warszawa-pogoda", 200);
    assert_eq!(class, BlockClass::RateLimited);
    assert!(detail.contains("flagged as being suspicious"), "{detail}");
}

#[test]
fn a_serp_with_results_is_not_a_block() {
    let plans = PlanSet::embedded().expect("plans");
    for (plan_id, slug) in [
        ("bing", "rust-browser-engine"),
        ("brave", "rust-browser-engine"),
        ("duckduckgo_html", "warszawa-pogoda"),
        ("duckduckgo_lite", "warszawa-pogoda"),
    ] {
        let plan = plans.get(plan_id).expect("plan");
        let body = fixture(plan_id, slug);
        let document = parse(&body);
        assert!(
            detect_block(plan, &document, &body, 200).is_none(),
            "{plan_id}/{slug} must not look like a block page"
        );
    }
}

#[test]
fn a_status_the_plan_lists_is_rate_limiting_when_the_page_says_nothing() {
    let plans = PlanSet::embedded().expect("plans");
    let plan = plans.get("bing").expect("plan");
    let body = b"<html><body>nothing to see</body></html>";
    let document = parse(body);

    let signal = detect_block(plan, &document, body, 429).expect("429 is a block");
    assert_eq!(signal.class, BlockClass::RateLimited);
    assert_eq!(signal.detail, "HTTP 429");

    assert!(
        detect_block(plan, &document, body, 200).is_none(),
        "the same page with a 200 is a layout problem, not a block"
    );
}

#[test]
fn duckduckgos_anomaly_page_is_a_captcha() {
    let plans = PlanSet::embedded().expect("plans");
    let plan = plans.get("duckduckgo_html").expect("plan");
    let body = b"<html><body><div class=\"anomaly-modal\">\
        Unfortunately, bots use DuckDuckGo too.</div></body></html>";
    let document = parse(body);

    let signal = detect_block(plan, &document, body, 200).expect("anomaly page");
    assert_eq!(signal.class, BlockClass::Captcha);
    assert_eq!(signal.detail, "selector div.anomaly-modal");
}

#[test]
fn patterns_match_case_insensitively_and_only_inside_the_sample() {
    assert!(contains_ignore_ascii_case(
        b"Unusual Traffic",
        b"unusual traffic"
    ));
    assert!(contains_ignore_ascii_case(
        b"xxUNUSUAL TRAFFICxx",
        b"unusual traffic"
    ));
    assert!(!contains_ignore_ascii_case(b"unusual", b"unusual traffic"));
    assert!(contains_ignore_ascii_case(b"anything", b""));
    assert!(!contains_ignore_ascii_case(b"", b"a"));

    // The scan stops after the first 200 KB, like the Python reference.
    let plans = PlanSet::embedded().expect("plans");
    let plan = plans.get("duckduckgo_html").expect("plan");
    let mut body = vec![b' '; 200_000];
    body.extend_from_slice(b"Unfortunately, bots use DuckDuckGo too");
    let document = parse(&body);
    assert!(detect_block(plan, &document, &body, 200).is_none());
}

#[test]
fn soft_block_detector_requires_page_echo_check() {
    use tenta_search::extract::detect_soft_block;
    use tenta_search::model::SearchResult;
    use url::Url;

    // Test: Polish inflected query "koty" with results that only have inflected forms.
    // Without Unicode lowercase, this would fail to match.
    // Without the page echo check, this would also fail incorrectly.
    let query = "koty";
    let base_url = Url::parse("http://example.com/").unwrap();

    // Create results with Polish inflected words (not matching "koty" exactly)
    let results = vec![
        SearchResult {
            rank: 1,
            title: "Kot domowy – Wikipedia".to_string(),
            url: base_url.clone(),
            display_url: "example.com".to_string(),
            snippet: "Information about domestic cats".to_string(),
            published: None,
            sitelinks: vec![],
            source: tenta_search::Provider::DuckDuckGo,
            kind: tenta_search::model::Kind::Organic,
        },
        SearchResult {
            rank: 2,
            title: "Rasy kotów – przewodnik".to_string(),
            url: base_url.clone(),
            display_url: "example.com".to_string(),
            snippet: "Guide to cat breeds".to_string(),
            published: None,
            sitelinks: vec![],
            source: tenta_search::Provider::DuckDuckGo,
            kind: tenta_search::model::Kind::Organic,
        },
    ];

    // Create a test page without the query in the title or search box.
    // The detect_soft_block should NOT report this as soft blocked because
    // the results do contain the query terms (after prefix matching).
    let body = b"<html><head><title>Search Results</title></head>\
                   <body><input name='q' value=''/>\
                   <!-- results --></body></html>";
    let document = common::parse(body);

    let signal = detect_soft_block(query, &results, &document);
    // The page echoes nothing, but "kotów" and "kot" carry the stem of "koty".
    assert!(
        signal.is_none(),
        "results with inflected forms should match the query term by stem"
    );
    assert!(
        detect_soft_block("KOTÓW", &results, &document).is_none(),
        "upper-case non-ASCII queries fold before matching"
    );
}

#[test]
fn soft_block_detector_passes_when_page_echoes_query() {
    use tenta_search::extract::detect_soft_block;
    use tenta_search::model::SearchResult;
    use url::Url;

    let query = "rust browser engine";
    let base_url = Url::parse("http://example.com/").unwrap();

    // Create results that don't echo the query at all
    let results = vec![SearchResult {
        rank: 1,
        title: "First unrelated result".to_string(),
        url: base_url.clone(),
        display_url: "example.com".to_string(),
        snippet: "Something about cats".to_string(),
        published: None,
        sitelinks: vec![],
        source: tenta_search::Provider::DuckDuckGo,
        kind: tenta_search::model::Kind::Organic,
    }];

    // Create a page that echoes the query in the title (like real search engines do)
    let body = b"<html><head><title>Search Results for rust browser engine</title></head>\
                   <body><input name='q' value='rust browser engine'/>\
                   <!-- results --></body></html>";
    let document = common::parse(body);

    let signal = detect_soft_block(query, &results, &document);
    // Page echoes the query in title and search box, so should NOT be soft-blocked
    // even though results don't echo it
    assert!(
        signal.is_none(),
        "page that echoes query should not be soft-blocked"
    );
}

#[test]
fn soft_block_detector_fires_when_page_and_results_dont_echo() {
    use tenta_search::extract::detect_soft_block;
    use tenta_search::model::SearchResult;
    use url::Url;

    let query = "rust browser engine";
    let base_url = Url::parse("http://example.com/").unwrap();

    // Create results that don't echo the query
    let results = vec![SearchResult {
        rank: 1,
        title: "First unrelated result".to_string(),
        url: base_url.clone(),
        display_url: "example.com".to_string(),
        snippet: "Something about cats".to_string(),
        published: None,
        sitelinks: vec![],
        source: tenta_search::Provider::DuckDuckGo,
        kind: tenta_search::model::Kind::Organic,
    }];

    // Create a page that doesn't echo the query
    let body = b"<html><head><title>Search Results</title></head>\
                   <body><input name='q' value=''/>\
                   <!-- results --></body></html>";
    let document = common::parse(body);

    let signal = detect_soft_block(query, &results, &document);
    // Page and results don't echo query, so should be soft-blocked
    assert!(
        signal.is_some(),
        "page and results that don't echo query should be soft-blocked"
    );
    assert_eq!(signal.unwrap().class, BlockClass::Soft);
}

#[test]
fn soft_block_detector_ignores_stop_words() {
    use tenta_search::extract::detect_soft_block;
    use tenta_search::model::SearchResult;
    use url::Url;

    let query = "the a and to"; // All stop-words
    let base_url = Url::parse("http://example.com/").unwrap();

    let results = vec![SearchResult {
        rank: 1,
        title: "First result".to_string(),
        url: base_url.clone(),
        display_url: "example.com".to_string(),
        snippet: "No query terms here".to_string(),
        published: None,
        sitelinks: vec![],
        source: tenta_search::Provider::DuckDuckGo,
        kind: tenta_search::model::Kind::Organic,
    }];

    let body = b"<html><head><title>Search Results</title></head>\
                   <body><input name='q' value=''/>\
                   <!-- results --></body></html>";
    let document = common::parse(body);

    let signal = detect_soft_block(query, &results, &document);
    // Query is all stop-words, so should not be soft-blocked (no terms to check)
    assert!(
        signal.is_none(),
        "query with only stop-words should not be soft-blocked"
    );
}
