//! `tentabrowser search` (`docs/spec/08-search.md` §5).
//!
//! The query runs in the engine's internal search context, not in the CLI's
//! default one: it has its own cookie jar and its own session-cache entry, so
//! a `fetch` in the same profile shares nothing with a provider
//! (`docs/spec/03-network.md` §4).

use std::str::FromStr;

use tentabrowser::{
    EngineSearch, Freshness, Locale, Mode, Provider, ProviderId, SafeSearch, SearchContextOptions,
    SearchRequest, SearchResponse, SearchSetup,
};
use url::Url;

use crate::cli::{Fresh, Global, Safe, SearchArgs, SearchMode};
use crate::error::{CliError, CliResult};
use crate::output;
use crate::setup;

pub async fn run(global: &Global, args: &SearchArgs) -> CliResult<()> {
    let request = request(args)?;
    let setup = search_setup(global, args, &request)?;

    // A search needs no page thread: `tenta-search` parses the SERP itself.
    let engine = setup::build_engine(global, 1)?;
    if args.warm {
        let report = engine.warm_search(&setup).await?;
        for (provider, reason) in &report.failed {
            tracing::warn!(%provider, %reason, "warm pool: provider not reachable");
        }
    }
    let response = engine.search_with(&setup, request).await;
    engine.close();

    print(&response?, global.json)
}

fn print(response: &SearchResponse, json: bool) -> CliResult<()> {
    for meta in &response.providers {
        tracing::debug!(
            provider = %meta.provider,
            endpoint = %crate::logging::url(&meta.endpoint),
            results = meta.results,
            cache_hit = meta.cache_hit,
            "provider answered"
        );
    }
    if json {
        return output::write_stdout(response.to_json_lines().as_bytes());
    }
    output::write_stdout(table(response).as_bytes())
}

/// The human form: one line of head per result, one indented snippet.
fn table(response: &SearchResponse) -> String {
    if response.results.is_empty() {
        return "no results".to_owned();
    }
    let mut out = String::with_capacity(160 * response.results.len());
    if let Some(answer) = &response.answer {
        out.push_str(&output::truncate(&answer.text, 400));
        out.push_str("\n\n");
    }
    for result in &response.results {
        out.push_str(&format!(
            "{:>2}. {}\n    {}\n",
            result.rank,
            output::truncate(&result.title, 96),
            result.url
        ));
        let snippet = output::truncate(&result.snippet, 160);
        if !snippet.is_empty() {
            out.push_str(&format!("    {snippet}\n"));
        }
    }
    if !response.related.is_empty() {
        out.push_str(&format!(
            "\nrelated: {}\n",
            response
                .related
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

fn request(args: &SearchArgs) -> CliResult<SearchRequest> {
    let mut providers = Vec::with_capacity(args.provider.len());
    for name in &args.provider {
        providers.push(
            Provider::from_str(name)
                .map_err(|error| CliError::usage(format!("--provider {name}: {error}")))?,
        );
    }
    let mut request = SearchRequest::new(args.query.clone())
        .providers(providers)
        .limit(args.limit)
        .page(args.page)
        .safe(safe(args.safe))
        .mode(mode(args.mode));
    if let Some(locale) = &args.locale {
        request = request.locale(
            Locale::parse(locale)
                .map_err(|error| CliError::usage(format!("--locale {locale}: {error}")))?,
        );
    }
    if let Some(freshness) = args.freshness {
        request = request.freshness(fresh(freshness));
    }
    if let Some(site) = &args.site {
        request = request.site(site.clone());
    }
    request.validate().map_err(CliError::from)?;
    Ok(request)
}

fn search_setup(
    global: &Global,
    args: &SearchArgs,
    request: &SearchRequest,
) -> CliResult<SearchSetup> {
    let mut setup = SearchSetup::for_request(request);
    setup.context = SearchContextOptions {
        allow_private_network: global.allow_private_network,
        proxy: global.proxy.clone(),
        user_agent: global.user_agent.clone(),
        accept_language: global.accept_language.clone(),
    };
    setup.request_timeout = Some(std::time::Duration::from_millis(global.timeout));
    for spec in &args.search_endpoint {
        let (plan, endpoint) = parse_endpoint(spec)?;
        setup = setup.endpoint_override(plan, endpoint);
    }
    Ok(setup)
}

/// `--search-endpoint duckduckgo_html=http://127.0.0.1:8080/serp.html`.
pub(crate) fn parse_endpoint(spec: &str) -> CliResult<(ProviderId, Url)> {
    let (plan, url) = spec.split_once('=').ok_or_else(|| {
        CliError::usage(format!(
            "--search-endpoint {spec:?}: expected <plan_id>=<url>"
        ))
    })?;
    let plan = ProviderId::new(plan)
        .map_err(|error| CliError::usage(format!("--search-endpoint {spec:?}: {error}")))?;
    let url = Url::parse(url)
        .map_err(|error| CliError::usage(format!("--search-endpoint {spec:?}: {error}")))?;
    Ok((plan, url))
}

fn safe(value: Safe) -> SafeSearch {
    match value {
        Safe::Off => SafeSearch::Off,
        Safe::Moderate => SafeSearch::Moderate,
        Safe::Strict => SafeSearch::Strict,
    }
}

fn fresh(value: Fresh) -> Freshness {
    match value {
        Fresh::Day => Freshness::Day,
        Fresh::Week => Freshness::Week,
        Fresh::Month => Freshness::Month,
        Fresh::Year => Freshness::Year,
    }
}

fn mode(value: SearchMode) -> Mode {
    match value {
        SearchMode::Fast => Mode::Fast,
        SearchMode::Full => Mode::Full,
        SearchMode::Auto => Mode::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args(extra: &[&str]) -> (Global, SearchArgs) {
        let cli = crate::cli::Cli::parse_from(
            ["tentabrowser", "search", "rust browser engine"]
                .into_iter()
                .chain(extra.iter().copied()),
        );
        let crate::cli::Command::Search(args) = cli.command else {
            panic!("expected search");
        };
        (cli.global, args)
    }

    #[test]
    fn the_request_carries_every_flag() {
        let (_, args) = args(&[
            "--provider",
            "ddg,brave",
            "--limit",
            "7",
            "--page",
            "2",
            "--locale",
            "pl-PL",
            "--safe",
            "strict",
            "--freshness",
            "week",
            "--site",
            "servo.org",
            "--mode",
            "fast",
        ]);
        let request = request(&args).expect("request");
        assert_eq!(request.query, "rust browser engine");
        assert_eq!(request.providers, [Provider::DuckDuckGo, Provider::Brave]);
        assert_eq!(request.limit, 7);
        assert_eq!(request.page, 2);
        assert_eq!(request.locale.tag(), "pl-PL");
        assert_eq!(request.safe, SafeSearch::Strict);
        assert_eq!(request.freshness, Some(Freshness::Week));
        assert_eq!(request.site.as_deref(), Some("servo.org"));
        assert_eq!(request.mode, Mode::Fast);
    }

    #[test]
    fn an_out_of_range_limit_is_a_usage_error() {
        let (_, too_many) = args(&["--limit", "500"]);
        let error = request(&too_many).expect_err("50 is the maximum");
        assert_eq!(error.exit_code(), crate::error::EXIT_USAGE, "{error}");

        let (_, bad_provider) = args(&["--provider", "Not A Provider"]);
        assert_eq!(
            request(&bad_provider)
                .expect_err("bad provider")
                .exit_code(),
            crate::error::EXIT_USAGE
        );
    }

    #[test]
    fn an_endpoint_override_names_a_plan_and_a_url() {
        let (global, args) = args(&[
            "--search-endpoint",
            "duckduckgo_html=http://127.0.0.1:9/serp.html",
        ]);
        let request = request(&args).expect("request");
        let setup = search_setup(&global, &args, &request).expect("setup");
        let plan = ProviderId::new("duckduckgo_html").expect("id");
        assert_eq!(
            setup.endpoint_override.get(&plan).map(Url::as_str),
            Some("http://127.0.0.1:9/serp.html")
        );
        assert!(parse_endpoint("no-equals-sign").is_err());
        assert!(parse_endpoint("bing=not a url").is_err());
    }

    #[test]
    fn the_table_prints_rank_title_url_and_snippet() {
        let response = SearchResponse {
            results: vec![tentabrowser::SearchResult {
                rank: 1,
                title: "Servo".into(),
                url: Url::parse("https://servo.org/").expect("url"),
                display_url: "servo.org".into(),
                snippet: "A web\nengine".into(),
                published: None,
                sitelinks: Vec::new(),
                source: Provider::DuckDuckGo,
                kind: tentabrowser::Kind::Organic,
            }],
            related: vec!["servo layout".into()],
            answer: None,
            timing: Default::default(),
            providers: Vec::new(),
        };
        let rendered = table(&response);
        assert!(rendered.contains(" 1. Servo"), "{rendered}");
        assert!(rendered.contains("https://servo.org/"), "{rendered}");
        assert!(rendered.contains("A web engine"), "{rendered}");
        assert!(rendered.contains("related: servo layout"), "{rendered}");

        let empty = SearchResponse {
            results: Vec::new(),
            ..response
        };
        assert_eq!(table(&empty), "no results");
    }
}
