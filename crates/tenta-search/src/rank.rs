//! Merging several providers' results (`docs/spec/08-search.md` §3.7).
//!
//! Providers disagree about ranking, about `www.` and about trailing slashes,
//! so the merge normalises the URL, deduplicates on it and orders the union
//! by reciprocal rank fusion, the standard combination for ranked lists whose
//! scores are not comparable.

use std::collections::HashMap;

use url::Url;

use crate::model::SearchResult;

/// The `k` of reciprocal rank fusion. 60 is the value the original paper
/// (Cormack et al., 2009) fixed and every implementation since has kept: it
/// damps the difference between the top ranks enough that one provider's
/// first place cannot outvote two other providers' second places.
pub const RRF_K: f64 = 60.0;

/// Common tracking/analytics parameters that should not affect deduplication.
/// URLs that differ only in these parameters should be treated as the same.
const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_content",
    "utm_term",
    "fbclid",
    "gclid",
    "msclkid",
    "twclid",
    "mc_cid",
    "mc_eid",
];

/// Filter out tracking parameters from a query string.
fn filter_tracking_params(query: &str) -> String {
    query
        .split('&')
        .filter(|param| {
            if let Some((key, _)) = param.split_once('=') {
                !TRACKING_PARAMS.iter().any(|tp| tp == &key)
            } else {
                !TRACKING_PARAMS.iter().any(|tp| tp == param)
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// The identity two results are merged on: scheme-insensitive, `www.`
/// stripped, host lowercased, trailing slash and fragment dropped.
/// Tracking parameters are also removed to improve deduplication.
pub fn normalise_url(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let path = url.path();
    let path = path.strip_suffix('/').unwrap_or(path);
    let mut key = String::with_capacity(host.len() + path.len() + 8);
    key.push_str(host);
    if let Some(port) = url.port()
        && port != 80
        && port != 443
    {
        key.push(':');
        key.push_str(&port.to_string());
    }
    key.push_str(path);
    if let Some(query) = url.query()
        && !query.is_empty()
    {
        let filtered = filter_tracking_params(query);
        if !filtered.is_empty() {
            key.push('?');
            key.push_str(&filtered);
        }
    }
    key
}

/// One provider's contribution to the merge, in that provider's own order.
pub struct Ranked {
    /// Position of the provider in the request, used as the tie-break.
    pub provider_index: usize,
    pub results: Vec<SearchResult>,
}

/// Deduplicates and merges by reciprocal rank fusion, then re-ranks from 1
/// and cuts to `limit`.
pub fn fuse(inputs: Vec<Ranked>, limit: u32) -> Vec<SearchResult> {
    struct Entry {
        score: f64,
        best_rank: u32,
        best_provider: usize,
        result: SearchResult,
    }

    let mut order: Vec<String> = Vec::new();
    let mut entries: HashMap<String, Entry> = HashMap::new();

    for input in inputs {
        let mut seen_here: HashMap<String, ()> = HashMap::new();
        let mut rank = 0u32;
        for result in input.results {
            let key = normalise_url(&result.url);
            // A provider that lists the same page twice votes for it once.
            if seen_here.insert(key.clone(), ()).is_some() {
                continue;
            }
            rank += 1;
            let contribution = 1.0 / (RRF_K + f64::from(rank));
            match entries.get_mut(&key) {
                Some(entry) => {
                    entry.score += contribution;
                    if (rank, input.provider_index) < (entry.best_rank, entry.best_provider) {
                        entry.best_rank = rank;
                        entry.best_provider = input.provider_index;
                        entry.result = result;
                    }
                }
                None => {
                    order.push(key.clone());
                    entries.insert(
                        key,
                        Entry {
                            score: contribution,
                            best_rank: rank,
                            best_provider: input.provider_index,
                            result,
                        },
                    );
                }
            }
        }
    }

    let mut merged: Vec<Entry> = order
        .into_iter()
        .filter_map(|key| entries.remove(&key))
        .collect();
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.best_rank.cmp(&b.best_rank))
            .then(a.best_provider.cmp(&b.best_provider))
    });

    merged
        .into_iter()
        .take(limit as usize)
        .enumerate()
        .map(|(index, entry)| SearchResult {
            rank: index as u32 + 1,
            ..entry.result
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Provider};

    fn result(rank: u32, url: &str) -> SearchResult {
        SearchResult {
            rank,
            title: format!("result {rank} {url}"),
            url: Url::parse(url).expect("url"),
            display_url: String::new(),
            snippet: String::new(),
            published: None,
            sitelinks: Vec::new(),
            source: Provider::Bing,
            kind: Kind::Organic,
        }
    }

    #[test]
    fn the_merge_key_ignores_scheme_www_trailing_slash_and_fragment() {
        let variants = [
            "https://example.com/page",
            "http://example.com/page",
            "https://www.example.com/page/",
            "https://EXAMPLE.com/page#section",
            "https://example.com:443/page",
        ];
        for variant in variants {
            assert_eq!(
                normalise_url(&Url::parse(variant).expect("url")),
                "example.com/page",
                "{variant}"
            );
        }
        // A query is part of the page's identity.
        assert_eq!(
            normalise_url(&Url::parse("https://example.com/s?q=1").expect("url")),
            "example.com/s?q=1"
        );
        // A non-default port is not.
        assert_eq!(
            normalise_url(&Url::parse("http://example.com:8080/").expect("url")),
            "example.com:8080"
        );
    }

    #[test]
    fn one_provider_keeps_its_own_order_and_drops_repeats() {
        let merged = fuse(
            vec![Ranked {
                provider_index: 0,
                results: vec![
                    result(1, "https://a.test/"),
                    result(2, "https://b.test/"),
                    result(3, "https://www.a.test"),
                ],
            }],
            10,
        );

        let urls: Vec<&str> = merged.iter().map(|result| result.url.as_str()).collect();
        assert_eq!(urls, ["https://a.test/", "https://b.test/"]);
        assert_eq!(merged[1].rank, 2);
    }

    #[test]
    fn a_page_two_providers_agree_on_outranks_a_page_only_one_listed() {
        // Provider A: x is 5th. Provider B: x is 5th. y is A's 1st only.
        let merged = fuse(
            vec![
                Ranked {
                    provider_index: 0,
                    results: vec![
                        result(1, "https://y.test/"),
                        result(2, "https://p2.test/"),
                        result(3, "https://p3.test/"),
                        result(4, "https://p4.test/"),
                        result(5, "https://x.test/"),
                    ],
                },
                Ranked {
                    provider_index: 1,
                    results: vec![
                        result(1, "https://q1.test/"),
                        result(2, "https://q2.test/"),
                        result(3, "https://q3.test/"),
                        result(4, "https://q4.test/"),
                        result(5, "https://x.test/"),
                    ],
                },
            ],
            3,
        );

        assert_eq!(merged.len(), 3, "the limit cuts the merged list");
        assert_eq!(merged[0].url.as_str(), "https://x.test/");
        // 1/65 + 1/65 > 1/61: two fifth places beat one first place.
        assert_eq!(merged[0].rank, 1);
        assert!(
            merged[1..]
                .iter()
                .any(|result| result.url.as_str() == "https://y.test/")
        );
    }

    #[test]
    fn the_best_ranked_provider_owns_the_merged_result() {
        let mut second = result(1, "https://x.test/");
        second.source = Provider::Brave;
        second.snippet = "brave".into();
        let merged = fuse(
            vec![
                Ranked {
                    provider_index: 0,
                    results: vec![result(1, "https://a.test/"), result(2, "https://x.test/")],
                },
                Ranked {
                    provider_index: 1,
                    results: vec![second],
                },
            ],
            10,
        );

        let x = merged
            .iter()
            .find(|result| result.url.as_str() == "https://x.test/")
            .expect("x");
        assert_eq!(x.source, Provider::Brave, "Brave ranked it first");
        assert_eq!(x.snippet, "brave");
    }

    #[test]
    fn tracking_parameters_do_not_affect_deduplication() {
        // URLs that differ only in tracking parameters should be deduplicated
        let base = "https://example.com/page";
        let with_utm = "https://example.com/page?utm_source=google&utm_medium=cpc&other=keep";
        let with_fbclid = "https://example.com/page?fbclid=abc123&other=keep";
        let with_mixed = "https://example.com/page?gclid=xyz&utm_campaign=foo&other=keep";

        let base_key = normalise_url(&Url::parse(base).expect("url"));
        assert_eq!(base_key, "example.com/page");

        // All tracking params are stripped, but other params are kept
        let utm_key = normalise_url(&Url::parse(with_utm).expect("url"));
        assert_eq!(utm_key, "example.com/page?other=keep");

        let fbclid_key = normalise_url(&Url::parse(with_fbclid).expect("url"));
        assert_eq!(fbclid_key, "example.com/page?other=keep");

        let mixed_key = normalise_url(&Url::parse(with_mixed).expect("url"));
        assert_eq!(mixed_key, "example.com/page?other=keep");
    }
}
