//! Loading, validating and applying the parse plans in `providers/plans/`
//! (`docs/spec/08-search.md` §2, format in `providers/README.md`).

use std::collections::HashMap;

use tenta_search::{Freshness, Locale, PlanPath, PlanSet, SafeSearch, SearchError, SearchRequest};
use tenta_testutil::temp_dir;
use url::Url;

fn plans() -> PlanSet {
    PlanSet::embedded().expect("embedded plans")
}

fn params(url: &Url) -> HashMap<String, String> {
    url.query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[test]
fn every_shipped_plan_loads_and_validates() {
    let plans = plans();
    let ids: Vec<&str> = plans.ids().collect();
    assert_eq!(
        ids,
        [
            "bing",
            "brave",
            "duckduckgo_html",
            "duckduckgo_lite",
            "google_chromium",
            "google_nojs"
        ]
    );

    let bing = plans.get("bing").expect("bing");
    assert_eq!(bing.version, 1);
    assert_eq!(bing.path, PlanPath::Fast);
    assert!(bing.verified);
    assert_eq!(bing.results.item.as_str(), "li.b_algo");
    assert_eq!(bing.results.exclude.len(), 3);

    // Google's plans are the detection-only and the Full-path ones.
    let chromium = plans.get("google_chromium").expect("google_chromium");
    assert_eq!(chromium.path, PlanPath::Full);
    assert!(
        !chromium.verified,
        "no fixture with results was ever recorded"
    );
    assert!(
        plans
            .get("google_nojs")
            .expect("google_nojs")
            .expect_blocked
    );
}

#[test]
fn duckduckgo_fills_kl_from_the_locale_and_drops_it_without_a_region() {
    let plans = plans();
    let plan = plans.get("duckduckgo_html").expect("plan");

    let polish = plan
        .build_request(
            &SearchRequest::new("warszawa pogoda").locale(Locale::parse("pl-PL").expect("locale")),
            None,
        )
        .expect("request");
    let query = params(&polish.url);
    assert_eq!(query.get("q").map(String::as_str), Some("warszawa pogoda"));
    assert_eq!(query.get("kl").map(String::as_str), Some("pl-pl"));
    assert_eq!(
        header(&polish.headers, "Accept-Language"),
        Some("pl-PL,pl;q=0.9,en-US;q=0.8,en;q=0.7")
    );

    let american = plan
        .build_request(
            &SearchRequest::new("rust").locale(Locale::parse("en-US").expect("locale")),
            None,
        )
        .expect("request");
    assert_eq!(
        params(&american.url).get("kl").map(String::as_str),
        Some("us-en")
    );
    assert_eq!(
        header(&american.headers, "Accept-Language"),
        Some("en-US,en;q=0.9")
    );

    // No region: `kl` needs one, so the parameter is left out and the
    // provider answers with its own default (`wt-wt`).
    let neutral = plan
        .build_request(
            &SearchRequest::new("rust").locale(Locale::parse("en").expect("locale")),
            None,
        )
        .expect("request");
    assert!(!params(&neutral.url).contains_key("kl"));
    assert_eq!(header(&neutral.headers, "Accept-Language"), Some("en"));
}

#[test]
fn the_query_parameter_comes_first_so_the_url_reads_like_the_providers_own() {
    let plans = plans();
    let plan = plans.get("duckduckgo_html").expect("plan");
    let prepared = plan
        .build_request(&SearchRequest::new("rust browser engine"), None)
        .expect("request");
    assert!(
        prepared
            .url
            .query()
            .expect("query")
            .starts_with("q=rust+browser+engine"),
        "got {}",
        prepared.url
    );
    assert!(!prepared.post);
    assert!(prepared.body.is_none());
}

#[test]
fn bing_takes_safesearch_as_a_cookie_and_the_locale_as_parameters() {
    let plans = plans();
    let plan = plans.get("bing").expect("plan");

    let request = SearchRequest::new("rust")
        .locale(Locale::parse("pl-PL").expect("locale"))
        .safe(SafeSearch::Strict);
    let prepared = plan.build_request(&request, None).expect("request");

    let query = params(&prepared.url);
    assert_eq!(query.get("setlang").map(String::as_str), Some("pl"));
    assert_eq!(query.get("cc").map(String::as_str), Some("PL"));
    assert!(!query.contains_key("safe"), "Bing takes it as a cookie");
    assert_eq!(
        header(&prepared.headers, "Cookie"),
        Some("SRCHHPGUSR=ADLT=STRICT")
    );

    let off = plan
        .build_request(&SearchRequest::new("rust").safe(SafeSearch::Off), None)
        .expect("request");
    assert_eq!(header(&off.headers, "Cookie"), Some("SRCHHPGUSR=ADLT=OFF"));
}

#[test]
fn duckduckgo_takes_safesearch_and_freshness_as_parameters() {
    let plans = plans();
    let plan = plans.get("duckduckgo_html").expect("plan");

    let prepared = plan
        .build_request(
            &SearchRequest::new("rust")
                .safe(SafeSearch::Strict)
                .freshness(Freshness::Week),
            None,
        )
        .expect("request");
    let query = params(&prepared.url);
    assert_eq!(query.get("kp").map(String::as_str), Some("1"));
    assert_eq!(query.get("df").map(String::as_str), Some("w"));

    let moderate = plan
        .build_request(&SearchRequest::new("rust"), None)
        .expect("request");
    assert_eq!(
        params(&moderate.url).get("kp").map(String::as_str),
        Some("-1"),
        "moderate is the model's default"
    );
}

#[test]
fn page_two_becomes_the_plans_offset_parameter() {
    let plans = plans();

    let bing = plans
        .get("bing")
        .expect("plan")
        .build_request(&SearchRequest::new("rust").page(2), None)
        .expect("request");
    // Bing counts results from 1 in steps of 10.
    assert_eq!(
        params(&bing.url).get("first").map(String::as_str),
        Some("11")
    );

    let brave = plans
        .get("brave")
        .expect("plan")
        .build_request(&SearchRequest::new("rust").page(3), None)
        .expect("request");
    // Brave counts pages from 0 in steps of 1.
    assert_eq!(
        params(&brave.url).get("offset").map(String::as_str),
        Some("2")
    );

    let first_page = plans
        .get("bing")
        .expect("plan")
        .build_request(&SearchRequest::new("rust"), None)
        .expect("request");
    assert!(!params(&first_page.url).contains_key("first"));
}

#[test]
fn duckduckgo_reports_pagination_it_cannot_reach_on_the_fast_path() {
    let plans = plans();
    // DuckDuckGo paginates through a hidden POST form carrying `vqd`, which
    // needs page 1's document; the fast path says so instead of silently
    // returning page 1 again.
    let error = plans
        .get("duckduckgo_html")
        .expect("plan")
        .build_request(&SearchRequest::new("rust").page(2), None)
        .expect_err("no page parameter");

    match error {
        SearchError::Pagination { page, detail, .. } => {
            assert_eq!(page, 2);
            assert!(detail.contains("Form"), "{detail}");
        }
        other => panic!("expected a pagination error, got {other:?}"),
    }
}

#[test]
fn brave_carries_its_fixed_parameters_and_freshness() {
    let plans = plans();
    let prepared = plans
        .get("brave")
        .expect("plan")
        .build_request(
            &SearchRequest::new("rust")
                .locale(Locale::parse("pl-PL").expect("locale"))
                .freshness(Freshness::Month),
            None,
        )
        .expect("request");

    let query = params(&prepared.url);
    assert_eq!(query.get("source").map(String::as_str), Some("web"));
    assert_eq!(query.get("country").map(String::as_str), Some("pl"));
    assert_eq!(query.get("search_lang").map(String::as_str), Some("pl"));
    assert_eq!(query.get("ui_lang").map(String::as_str), Some("pl-pl"));
    assert_eq!(query.get("tf").map(String::as_str), Some("pm"));
    assert_eq!(
        query.get("safesearch").map(String::as_str),
        Some("moderate")
    );
}

#[test]
fn a_site_restriction_without_a_plan_parameter_is_folded_into_the_query() {
    let plans = plans();
    let prepared = plans
        .get("duckduckgo_html")
        .expect("plan")
        .build_request(&SearchRequest::new("rust").site("servo.org"), None)
        .expect("request");

    assert_eq!(
        params(&prepared.url).get("q").map(String::as_str),
        Some("site:servo.org rust")
    );
}

#[test]
fn an_endpoint_override_replaces_only_the_endpoint() {
    let plans = plans();
    let endpoint = Url::parse("http://127.0.0.1:9/html/").expect("url");
    let prepared = plans
        .get("duckduckgo_html")
        .expect("plan")
        .build_request(&SearchRequest::new("rust"), Some(&endpoint))
        .expect("request");

    assert_eq!(prepared.url.host_str(), Some("127.0.0.1"));
    assert_eq!(prepared.url.path(), "/html/");
    assert_eq!(
        params(&prepared.url).get("q").map(String::as_str),
        Some("rust")
    );
}

#[test]
fn a_post_plan_puts_its_parameters_in_the_body() {
    let directory = temp_dir("search-plan-post");
    directory.write(
        "form_provider.toml",
        r#"
provider = "form_provider"
version = 2
path = "fast"

[request]
endpoint = "https://example.test/search"
method = "POST"
query_param = "q"
params = { source = "web" }

[results]
container = "div#results"
item = "div.result"
title = "a"
url = "a"
"#,
    );

    let plans = PlanSet::embedded()
        .expect("embedded")
        .with_dir(directory.path())
        .expect("directory plans");
    let plan = plans.get("form_provider").expect("plan");
    assert_eq!(plan.version, 2);

    let prepared = plan
        .build_request(&SearchRequest::new("rust engine"), None)
        .expect("request");
    assert!(prepared.post);
    assert_eq!(prepared.url.query(), None);
    assert_eq!(
        prepared.body.as_deref(),
        Some("q=rust+engine&source=web"),
        "GET parameters become the form body"
    );
}

fn reject(body: &str) -> String {
    let directory = temp_dir("search-plan-reject");
    directory.write("broken.toml", body);
    PlanSet::from_dir(directory.path())
        .expect_err("the plan must be refused")
        .to_string()
}

const MINIMAL: &str = r#"
provider = "broken"
version = 1
[request]
endpoint = "https://example.test/search"
query_param = "q"
[results]
item = "div.result"
title = "a"
url = "a"
"#;

#[test]
fn a_plan_naming_a_vendor_api_is_refused() {
    // O-7: search only ever uses the providers' public pages, so the
    // placeholders that only an API key could fill are refused outright.
    let error = reject(&MINIMAL.replace(
        "[results]",
        "[request.headers]\nX-Key = \"{api_key}\"\n\n[results]",
    ));
    assert!(error.contains("api_key"), "{error}");
    assert!(error.contains("O-7"), "{error}");

    let error = reject(&MINIMAL.replace("path = \"fast\"", "").replace(
        "provider = \"broken\"",
        "provider = \"broken\"\npath = \"api\"",
    ));
    assert!(error.contains("public pages"), "{error}");
}

#[test]
fn plan_mistakes_are_refused_at_load_with_the_reason() {
    let unknown_placeholder = reject(&MINIMAL.replace(
        "query_param = \"q\"",
        "query_param = \"q\"\nlocale = { hl = \"{country}\" }",
    ));
    assert!(
        unknown_placeholder.contains("unknown placeholder"),
        "{unknown_placeholder}"
    );

    let bad_selector = reject(&MINIMAL.replace("item = \"div.result\"", "item = \"div..\""));
    assert!(
        bad_selector.contains("is not a valid selector"),
        "{bad_selector}"
    );

    let unknown_field = reject(&MINIMAL.replace("version = 1", "version = 1\nflavour = \"salty\""));
    assert!(unknown_field.contains("unknown field"), "{unknown_field}");

    let mismatched = reject(&MINIMAL.replace("provider = \"broken\"", "provider = \"other\""));
    assert!(
        mismatched.contains("does not match the file name"),
        "{mismatched}"
    );

    // The Python reference matches `[block] patterns` as regular expressions;
    // the fast path runs no regex, so a pattern that is not a literal is
    // refused rather than quietly matching something else.
    let regex_pattern = reject(&format!(
        "{MINIMAL}\n[block]\npatterns = [\"unusual (traffic)\"]\n"
    ));
    assert!(
        regex_pattern.contains("regular expressions"),
        "{regex_pattern}"
    );

    let regex_unwrap =
        reject(&MINIMAL.replace("url = \"a\"", "url = \"a\"\nurl_unwrap_regex = \"u=(.*)\""));
    assert!(regex_unwrap.contains("no regex"), "{regex_unwrap}");

    let following =
        reject(&MINIMAL.replace("url = \"a\"", "url = \"a\"\nfollowing = [\"snippet\"]"));
    assert!(following.contains("does not define"), "{following}");

    let decode =
        reject(&MINIMAL.replace("url = \"a\"", "url = \"a\"\nurl_unwrap_decode = \"rot13\""));
    assert!(decode.contains("url_unwrap_decode"), "{decode}");
}

#[test]
fn a_directory_plan_replaces_the_embedded_one_of_the_same_id() {
    let directory = temp_dir("search-plan-override");
    directory.write(
        "bing.toml",
        r#"
provider = "bing"
version = 99
path = "fast"

[request]
endpoint = "https://www.bing.com/search"
query_param = "q"

[results]
item = "li.b_algo_v2"
title = "h2 a"
url = "h2 a"
"#,
    );

    let plans = PlanSet::embedded()
        .expect("embedded")
        .with_dir(directory.path())
        .expect("override");
    let bing = plans.get("bing").expect("bing");
    assert_eq!(bing.version, 99);
    assert_eq!(bing.results.item.as_str(), "li.b_algo_v2");
    assert_eq!(plans.len(), 6, "the override replaced, not added");
}
