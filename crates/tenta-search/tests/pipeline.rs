//! The query pipeline: cache and coalescing, fan-out and merge, the JSON the
//! CLI prints, the early stop, the warm pool and proxy rotation
//! (`docs/spec/08-search.md` §3, §4, §5).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{Harness, custom};
use tenta_net::hooks::{Hooks, Observer};
use tenta_net::proxy::Proxy;
use tenta_net::request::Request;
use tenta_search::{BlockClass, Kind, Locale, Provider, SearchError, SearchOptions, SearchRequest};

#[tokio::test]
async fn an_identical_query_is_served_from_the_cache() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("bing", "rust-browser-engine");
    let search = harness.search();
    let request = || SearchRequest::new("rust browser engine").provider(Provider::Bing);

    let first = search.query(request()).await.expect("first");
    let second = search.query(request()).await.expect("second");

    assert_eq!(harness.requests_served(), 1, "the second query never left");
    assert!(!first.providers[0].cache_hit);
    assert!(second.providers[0].cache_hit);
    assert_eq!(first.results, second.results);
    let stats = search.cache_stats();
    assert_eq!((stats.hits, stats.misses, stats.entries), (1, 1, 1));

    // A cache hit costs no network time, so its whole wall time is overhead.
    assert_eq!(second.timing.engine_overhead, second.timing.total);
}

#[tokio::test]
async fn the_cache_key_covers_the_locale_the_page_and_safesearch() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("bing", "rust-browser-engine");
    let search = harness.search();

    let base = || SearchRequest::new("Rust  Browser Engine").provider(Provider::Bing);
    search.query(base()).await.expect("first");
    // Whitespace and case are normalised away: same key.
    search
        .query(SearchRequest::new("rust browser engine").provider(Provider::Bing))
        .await
        .expect("normalised");
    assert_eq!(harness.requests_served(), 1);

    search
        .query(base().locale(Locale::parse("pl-PL").expect("locale")))
        .await
        .expect("other locale");
    assert_eq!(harness.requests_served(), 2);

    search
        .query(base().safe(tenta_search::SafeSearch::Strict))
        .await
        .expect("other safesearch");
    assert_eq!(harness.requests_served(), 3);
}

#[tokio::test]
async fn identical_concurrent_queries_coalesce_on_one_request() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("duckduckgo_html", "rust-browser-engine");
    let search = harness.search();

    let queries: Vec<_> = (0..8)
        .map(|_| {
            search.query(SearchRequest::new("rust browser engine").provider(Provider::DuckDuckGo))
        })
        .collect();
    let responses = futures_join_all(queries).await;

    assert_eq!(
        harness.requests_served(),
        1,
        "one request for eight queries"
    );
    for response in &responses {
        assert_eq!(response.as_ref().expect("query").results.len(), 10);
    }
    let stats = search.cache_stats();
    assert_eq!(stats.misses, 1);
    assert_eq!(stats.coalesced, 7);
    let served_from_cache = responses
        .iter()
        .filter(|response| response.as_ref().expect("query").providers[0].cache_hit)
        .count();
    assert_eq!(served_from_cache, 7);
}

/// The crate's own concurrent-join helper is private; the tests need the same
/// behaviour and no new dependency.
async fn futures_join_all<F: std::future::Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut pending: Vec<std::pin::Pin<Box<F>>> = futures.into_iter().map(Box::pin).collect();
    let mut outputs: Vec<Option<F::Output>> = std::iter::repeat_with(|| None)
        .take(pending.len())
        .collect();
    let mut remaining = pending.len();
    std::future::poll_fn(move |cx| {
        for (index, future) in pending.iter_mut().enumerate() {
            if outputs[index].is_none()
                && let std::task::Poll::Ready(value) = future.as_mut().poll(cx)
            {
                outputs[index] = Some(value);
                remaining -= 1;
            }
        }
        if remaining == 0 {
            std::task::Poll::Ready(
                std::mem::take(&mut outputs)
                    .into_iter()
                    .map(|output| output.expect("done"))
                    .collect(),
            )
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}

#[tokio::test]
async fn an_expired_entry_is_fetched_again() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("bing", "rust-browser-engine");
    let search = harness.search_with(SearchOptions {
        cache_ttl: Duration::ZERO,
        ..SearchOptions::default()
    });

    for _ in 0..3 {
        search
            .query(SearchRequest::new("rust").provider(Provider::Bing))
            .await
            .expect("query");
    }

    assert_eq!(harness.requests_served(), 3);
    assert_eq!(search.cache_stats().hits, 0);
}

#[tokio::test]
async fn a_fan_out_merges_two_providers_by_reciprocal_rank_fusion() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("duckduckgo_html", "rust-browser-engine");
    harness.serve_fixture("brave", "rust-browser-engine");
    let search = harness.search();

    let response = search
        .query(
            SearchRequest::new("rust browser engine")
                .providers([Provider::DuckDuckGo, Provider::Brave])
                .limit(50),
        )
        .await
        .expect("query");

    assert_eq!(
        harness.requests_served(),
        2,
        "the providers ran concurrently"
    );
    assert_eq!(response.providers.len(), 2);
    assert_eq!(response.providers[0].results, 10);
    assert_eq!(response.providers[1].results, 20);

    // 10 + 20 with three pages in common.
    assert_eq!(response.results.len(), 27);
    assert_eq!(response.results[0].url.as_str(), "https://servo.org/");
    assert_eq!(
        response.results[1].url.as_str(),
        "https://github.com/servo/servo"
    );
    // Both providers list this page (9th and 20th); RRF puts it above pages
    // only one provider listed at rank 3.
    assert!(
        response.results[2]
            .url
            .as_str()
            .starts_with("https://medium.com/rust-revolution/"),
        "got {}",
        response.results[2].url
    );
    // Every URL appears once, however the providers spelled it.
    let mut keys: Vec<String> = response
        .results
        .iter()
        .map(|result| tenta_search::normalise_url(&result.url))
        .collect();
    keys.sort();
    let count = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), count);

    // The network time of a fan-out is the slowest provider's, not the sum.
    assert!(response.timing.engine_overhead <= response.timing.total);
}

#[tokio::test]
async fn a_fan_out_keeps_going_when_one_provider_is_blocked() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("duckduckgo_html", "rust-browser-engine");
    harness.serve_fixture("brave", "warszawa-pogoda"); // Brave's 429 page.
    let search = harness.search();

    let response = search
        .query(
            SearchRequest::new("rust browser engine")
                .providers([Provider::DuckDuckGo, Provider::Brave])
                .limit(50),
        )
        .await
        .expect("DuckDuckGo answered");

    assert_eq!(response.results.len(), 10);
    let brave = response
        .providers
        .iter()
        .find(|meta| meta.provider == Provider::Brave)
        .expect("brave meta");
    assert_eq!(brave.results, 0);
    assert!(
        brave
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("rate-limited"),
        "the meta says why: {:?}",
        brave.error
    );
}

#[tokio::test]
async fn the_json_lines_are_one_object_per_result_and_a_trailing_meta_object() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("duckduckgo_html", "rust-browser-engine");
    let search = harness.search();

    let response = search
        .query(
            SearchRequest::new("rust browser engine")
                .provider(Provider::DuckDuckGo)
                .limit(3),
        )
        .await
        .expect("query");

    let json_lines = response.to_json_lines();
    let lines: Vec<&str> = json_lines.lines().map(str::trim).collect();
    assert_eq!(lines.len(), 4);

    let first: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
    assert_eq!(first["rank"], 1);
    assert_eq!(first["url"], "https://servo.org/");
    assert_eq!(first["source"], "duckduckgo");
    assert_eq!(first["kind"], "organic");
    assert!(first["title"].is_string());
    assert!(first["snippet"].is_string());
    assert!(first["display_url"].is_string());
    assert!(first.get("published").is_none(), "omitted when unknown");
    assert!(first.get("sitelinks").is_none(), "omitted when empty");

    let meta: serde_json::Value = serde_json::from_str(lines[3]).expect("json");
    let meta = &meta["meta"];
    assert_eq!(meta["results"], 3);
    assert!(meta["timing"]["engine_overhead_ms"].is_number());
    assert!(meta["timing"]["total_ms"].is_number());
    assert_eq!(meta["providers"][0]["provider"], "duckduckgo");
    assert_eq!(meta["providers"][0]["plan_version"], 1);
    assert_eq!(meta["providers"][0]["mode_used"], "fast");
    assert_eq!(meta["providers"][0]["cache_hit"], false);
    assert!(
        meta["providers"][0]["endpoint"]
            .as_str()
            .expect("endpoint")
            .contains("q=rust+browser+engine")
    );

    // The whole-object form carries the same fields.
    let whole: serde_json::Value = serde_json::from_str(&response.to_json()).expect("json");
    assert_eq!(whole["results"].as_array().expect("results").len(), 3);
    assert!(whole["related"].is_array());
    assert!(whole["timing"]["ttfb_ms"].is_number());
}

#[tokio::test]
async fn the_tree_builder_stops_when_the_results_container_closes() {
    let mut harness = Harness::start().await;
    let html = format!(
        "<!doctype html><html><body><div id=links>{}</div>\
         <div id=after-the-container>never parsed</div></body></html>",
        (0..3)
            .map(|index| format!(
                "<div class=result><h2 class=result__title>\
                 <a class=result__a href='https://example.test/{index}'>Result {index}</a></h2>\
                 <a class=result__snippet>Snippet {index}</a></div>"
            ))
            .collect::<String>()
    );
    harness.serve_html("duckduckgo_html", &html);
    let search = harness.search();

    // The fixture titles are "Result N": the query must echo in them or the
    // soft-block gate classifies the page as a block.
    let response = search
        .query(SearchRequest::new("result").provider(Provider::DuckDuckGo))
        .await
        .expect("query");
    assert_eq!(response.results.len(), 3);

    // Same page, parsed through the same path: the marker after the
    // container never reaches the DOM.
    let plans = tenta_search::PlanSet::embedded().expect("plans");
    let plan = plans.get("duckduckgo_html").expect("plan");
    let prepared = plan
        .build_request(
            &SearchRequest::new("result"),
            Some(&harness.serve_html("duckduckgo_html", &html)),
        )
        .expect("request");
    let fetched = tenta_search::fetch::fetch_and_parse(
        &harness.client(),
        plan,
        &prepared,
        tenta_search::fetch::FetchOptions::default(),
    )
    .await
    .expect("fetch");

    assert!(fetched.stopped_early, "the stop predicate fired");
    let root = fetched.document.document_node();
    assert!(
        fetched
            .document
            .query_selector(root, "div#after-the-container")
            .expect("selector")
            .is_none(),
        "markup after the container is never built"
    );
    assert!(
        fetched
            .document
            .query_selector(root, "div#links")
            .expect("selector")
            .is_some()
    );
}

#[tokio::test]
async fn warm_preconnects_every_enabled_provider() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("duckduckgo_html", "rust-browser-engine");
    harness.serve_fixture("bing", "rust-browser-engine");
    let search = harness.search_with(SearchOptions {
        providers_enabled: vec![Provider::DuckDuckGo, Provider::Bing],
        ..SearchOptions::default()
    });

    let report = search.warm().await;

    assert!(report.is_complete(), "failed: {:?}", report.failed);
    assert_eq!(report.warmed.len(), 2);
    assert_eq!(
        harness.requests_served(),
        0,
        "warming opens connections, it does not query"
    );

    // The connection the warm pool opened is the one the query uses.
    let response = search
        .query(SearchRequest::new("rust browser engine").provider(Provider::Bing))
        .await
        .expect("query");
    assert_eq!(response.results.len(), 10);
    assert_eq!(response.timing.connect, Duration::ZERO, "pooled connection");
}

#[tokio::test]
async fn warm_on_start_opens_the_connection_before_the_first_query() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("bing", "rust-browser-engine");

    let cold = harness.search_with(SearchOptions {
        providers_enabled: vec![Provider::Bing],
        warm_on_start: false,
        ..SearchOptions::default()
    });
    let first = cold
        .query(SearchRequest::new("rust").provider(Provider::Bing))
        .await
        .expect("query");
    assert!(
        first.timing.connect > Duration::ZERO,
        "the first query opened the connection itself"
    );

    let warm = harness.search_with(SearchOptions {
        providers_enabled: vec![Provider::Bing],
        warm_on_start: true,
        ..SearchOptions::default()
    });
    let first = warm
        .query(SearchRequest::new("rust").provider(Provider::Bing))
        .await
        .expect("query");
    assert_eq!(
        first.timing.connect,
        Duration::ZERO,
        "the warm pool had the connection open"
    );
}

#[tokio::test]
async fn a_disabled_provider_is_never_queried() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("bing", "rust-browser-engine");
    let search = harness.search_with(SearchOptions {
        providers_enabled: vec![Provider::Bing],
        ..SearchOptions::default()
    });

    let error = search
        .query(SearchRequest::new("rust").provider(Provider::Brave))
        .await
        .expect_err("brave is not enabled");

    assert!(matches!(error, SearchError::NoProvider(_)), "{error:?}");
    assert_eq!(harness.requests_served(), 0);
}

#[tokio::test]
async fn a_rate_limited_provider_is_retried_through_the_next_proxy() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("brave", "warszawa-pogoda"); // Brave's 429 page.

    // A proxy that is not listening: the ladder's step 3 is observable
    // because the failure changes from the block to the proxy's refusal.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
    let port = dead.local_addr().expect("addr").port();
    drop(dead);
    let search = harness.search_with(SearchOptions {
        proxies: vec![Proxy::parse(&format!("http://127.0.0.1:{port}")).expect("proxy")],
        ..SearchOptions::default()
    });

    let error = search
        .query(SearchRequest::new("warszawa pogoda").provider(Provider::Brave))
        .await
        .expect_err("no proxy works");

    match &error {
        SearchError::Net(net) => assert!(
            net.to_string().contains("CONNECTION"),
            "the retry went through the proxy: {net}"
        ),
        other => panic!("expected the proxy attempt to fail, got {other:?}"),
    }

    // Without a proxy the same page is simply reported as rate limiting.
    let plain = harness.search();
    let error = plain
        .query(SearchRequest::new("warszawa pogoda").provider(Provider::Brave))
        .await
        .expect_err("blocked");
    assert_eq!(error.block_class(), Some(BlockClass::RateLimited));
}

/// One request as the transport sent it: the URL and its caller headers.
type RecordedRequest = (String, Vec<(String, String)>);

#[derive(Default)]
struct RecordingObserver {
    requests: std::sync::Mutex<Vec<RecordedRequest>>,
    count: AtomicUsize,
}

impl Observer for RecordingObserver {
    fn on_request(&self, request: &Request) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.requests.lock().expect("lock").push((
            request.url.to_string(),
            request
                .headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        String::from_utf8_lossy(value.as_bytes()).into_owned(),
                    )
                })
                .collect(),
        ));
    }
}

#[tokio::test]
async fn the_plans_headers_and_cookies_go_out_on_the_wire() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("bing", "warszawa-pogoda");
    let observer = Arc::new(RecordingObserver::default());
    let hooks = Hooks {
        observers: vec![observer.clone()],
        ..Hooks::default()
    };
    let search = harness.search_from(harness.client_with(hooks), SearchOptions::default());

    let _ = search
        .query(
            SearchRequest::new("warszawa pogoda")
                .provider(Provider::Bing)
                .locale(Locale::parse("pl-PL").expect("locale"))
                .safe(tenta_search::SafeSearch::Strict),
        )
        .await;

    let requests = observer.requests.lock().expect("lock").clone();
    assert_eq!(requests.len(), 1);
    let (url, headers) = &requests[0];
    assert!(url.contains("setlang=pl"), "{url}");
    assert!(url.contains("cc=PL"), "{url}");
    let find = |name: &str| {
        headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(
        find("Accept-Language"),
        Some("pl-PL,pl;q=0.9,en-US;q=0.8,en;q=0.7")
    );
    assert_eq!(find("Cookie"), Some("SRCHHPGUSR=ADLT=STRICT"));
}

const DEMO_PLAN: &str = r#"
provider = "demo"
version = 4
path = "fast"

[request]
endpoint = "https://demo.test/search"
query_param = "q"

[results]
container = "div#results"
item = "article.result"
exclude = ["article.ad"]
title = "h3"
url = "a.link"
display_url = "span.cite"
snippet = "p.snippet"
date = "time"
date_attr = "datetime"
sitelinks = "nav.sitelinks a"
drop_params = ["utm_source", "utm_campaign"]
kind = { news = "span.badge-news", video = "span.badge-video" }

[related]
item = "ul#related li"

[answer]
container = "div#answer"
text = "p.answer-text"
source = "a.answer-source"
"#;

const DEMO_HTML: &str = r#"<!doctype html>
<html><body>
<div id="answer"><p class="answer-text">42</p>
  <a class="answer-source" href="https://source.test/why">why</a></div>
<div id="results">
  <article class="ad"><h3>Advertisement</h3><a class="link" href="https://ad.test/">ad</a></article>
  <article class="result">
    <h3>First   result</h3>
    <a class="link" href="https://first.test/page?utm_source=serp&amp;id=7&amp;utm_campaign=x">first</a>
    <span class="cite">first.test › page</span>
    <p class="snippet">The first snippet.</p>
    <time datetime="2026-08-31T09:00:00Z">31 August 2026</time>
    <nav class="sitelinks"><a class="link" href="https://first.test/a">Alpha</a>
      <a class="link" href="https://first.test/b">Beta</a></nav>
  </article>
  <article class="result">
    <h3>Second result</h3><span class="badge-news">News</span>
    <a class="link" href="https://second.test/">second</a>
    <p class="snippet">The second snippet.</p>
  </article>
</div>
<ul id="related"><li>rust engine</li><li>servo</li><li>rust engine</li></ul>
</body></html>"#;

#[tokio::test]
async fn a_plan_can_extract_answers_related_searches_sitelinks_kinds_and_dates() {
    let mut harness = Harness::start().await;
    harness.serve_plan("demo", DEMO_PLAN, DEMO_HTML);
    let search = harness.search();

    let response = search
        .query(SearchRequest::new("first result").provider(custom("demo")))
        .await
        .expect("query");

    assert_eq!(response.results.len(), 2, "the ad is excluded");
    let first = &response.results[0];
    assert_eq!(first.title, "First result", "whitespace is collapsed");
    // The tracking parameters the plan names are dropped, the rest stay.
    assert_eq!(first.url.as_str(), "https://first.test/page?id=7");
    assert_eq!(first.display_url, "first.test › page");
    assert_eq!(first.snippet, "The first snippet.");
    assert_eq!(
        first.published,
        Some(time::Date::from_calendar_date(2026, time::Month::August, 31).expect("date"))
    );
    assert_eq!(first.sitelinks.len(), 2);
    assert_eq!(first.sitelinks[0].title, "Alpha");
    assert_eq!(first.sitelinks[0].url.as_str(), "https://first.test/a");
    assert_eq!(first.kind, Kind::Organic);

    let second = &response.results[1];
    assert_eq!(second.kind, Kind::News);
    assert!(second.published.is_none());
    assert!(second.sitelinks.is_empty());

    assert_eq!(response.related, ["rust engine", "servo"], "deduplicated");
    let answer = response.answer.clone().expect("answer");
    assert_eq!(answer.text, "42");
    assert_eq!(
        answer.source.map(|url| url.to_string()),
        Some("https://source.test/why".to_owned())
    );

    // The JSON carries the optional fields when they are present.
    let json: serde_json::Value =
        serde_json::from_str(response.to_json_lines().lines().next().expect("line")).expect("json");
    assert_eq!(json["published"], "2026-08-31");
    assert_eq!(json["sitelinks"][0]["title"], "Alpha");
}

#[tokio::test]
async fn a_query_the_model_refuses_never_reaches_the_network() {
    let harness = Harness::start().await;
    let search = harness.search();

    for (request, expected) in [
        (SearchRequest::new("  "), "query is empty"),
        (SearchRequest::new("rust").limit(0), "limit 0"),
        (SearchRequest::new("rust").limit(51), "limit 51"),
        (SearchRequest::new("rust").page(0), "page is 1-based"),
        (
            SearchRequest::new("rust").site("two words"),
            "is not a host",
        ),
    ] {
        let error = search.query(request).await.expect_err("refused");
        assert!(
            error.to_string().contains(expected),
            "{error} does not mention {expected}"
        );
    }
    assert_eq!(harness.requests_served(), 0);
}
