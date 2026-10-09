use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::StatusCode;
use tenta_core::Tier;
use tenta_dom::Mutation;
use tenta_html::{ParseStatus, Parser, ParserOptions, QualName};
use tenta_net::hooks::{BoxFuture, FulfilledResponse, Hooks, Interception, Interceptor};
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::request::Request;
use tenta_net::{ClientConfig, ErrorName, NetRuntime};

use super::*;
use crate::clock::system_clock;

const DOC_URL: &str = "http://fixture.test/dir/page.html";

/// What the fixture saw of one request.
#[derive(Clone, Debug)]
struct Seen {
    url: String,
    initiator: Option<Url>,
    resource_type: ResourceType,
}

type Answer = (StatusCode, Vec<(&'static str, &'static str)>, Vec<u8>);

struct Fixture {
    routes: HashMap<String, Answer>,
    /// Answers any URL the table lacks, for trees too wide to list.
    generated: Option<fn(&str) -> Option<String>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Interceptor for Fixture {
    fn intercept<'a>(&'a self, request: &'a mut Request) -> BoxFuture<'a, Interception> {
        self.seen.lock().unwrap().push(Seen {
            url: request.url.to_string(),
            initiator: request.initiator.clone(),
            resource_type: request.resource_type,
        });
        let answer = self.routes.get(request.url.as_str()).cloned().or_else(|| {
            let body = self.generated?(request.url.as_str())?;
            Some((
                StatusCode::OK,
                vec![("content-type", "text/css")],
                body.into_bytes(),
            ))
        });
        Box::pin(async move {
            match answer {
                Some((status, headers, body)) => {
                    let mut response = FulfilledResponse::new(status, Bytes::from(body));
                    for (name, value) in headers {
                        response = response.header(name, value);
                    }
                    Interception::Fulfill(response)
                }
                None => Interception::Fail(tenta_net::NetError::new(
                    ErrorName::ConnectionRefused,
                    "no route",
                )),
            }
        })
    }
}

struct Env {
    _net: Arc<NetRuntime>,
    client: Client,
    seen: Arc<Mutex<Vec<Seen>>>,
    fs_policy: FsPolicy,
    budgets: Budgets,
    clock: Arc<dyn Clock>,
    document_url: Url,
    tier: Tier,
}

impl Env {
    fn new(routes: &[(&str, &'static str, &str)]) -> Env {
        Env::with_routes(
            routes
                .iter()
                .map(|(url, content_type, body)| {
                    (
                        (*url).to_owned(),
                        (
                            StatusCode::OK,
                            vec![("content-type", *content_type)],
                            body.as_bytes().to_vec(),
                        ),
                    )
                })
                .collect(),
        )
    }

    fn with_routes(routes: HashMap<String, Answer>) -> Env {
        Env::build(routes, None)
    }

    fn generated(generated: fn(&str) -> Option<String>) -> Env {
        Env::build(HashMap::new(), Some(generated))
    }

    fn build(
        routes: HashMap<String, Answer>,
        generated: Option<fn(&str) -> Option<String>>,
    ) -> Env {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let net = NetRuntime::new(1).expect("io runtime");
        let client = net
            .client(ClientConfig {
                hooks: Hooks {
                    interceptors: vec![Arc::new(Fixture {
                        routes,
                        generated,
                        seen: Arc::clone(&seen),
                    })],
                    observers: Vec::new(),
                },
                destination: DestinationPolicy::permissive(),
                ..ClientConfig::default()
            })
            .expect("client");
        Env {
            _net: net,
            client,
            seen,
            fs_policy: FsPolicy::none(),
            budgets: Budgets::default(),
            clock: system_clock(),
            document_url: Url::parse(DOC_URL).unwrap(),
            tier: Tier::Render,
        }
    }

    fn ctx(&self) -> StyleContext<'_> {
        StyleContext {
            client: &self.client,
            fs_policy: &self.fs_policy,
            budgets: &self.budgets,
            clock: &*self.clock,
            extra_headers: &[],
            document_url: &self.document_url,
            deadline: Instant::now() + Duration::from_secs(60),
            fetch_policy: FetchPolicy::for_tier(self.tier),
        }
    }

    fn requested(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.url.clone())
            .collect()
    }
}

fn parse(html: &str, url: &str) -> Document {
    let mut parser = Parser::new(
        Document::with_url(Url::parse(url).unwrap()),
        ParserOptions::default(),
    );
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    parser.finish()
}

fn base(doc: &Document) -> Url {
    doc.base_url().cloned().expect("a base URL")
}

fn texts(sources: &[StyleSource]) -> Vec<&str> {
    sources.iter().map(|s| s.text.as_str()).collect()
}

async fn collect(doc: &Document, env: &Env, state: &mut StyleState) -> Vec<StyleSource> {
    collect_style_sources(doc, &base(doc), &env.ctx(), state).await
}

#[tokio::test(flavor = "current_thread")]
async fn inline_and_linked_sheets_arrive_in_document_order() {
    let env = Env::new(&[
        ("http://fixture.test/dir/rel.css", "text/css", "rel{}"),
        ("http://other.test/abs.css", "text/css", "abs{}"),
    ]);
    let doc = parse(
        "<style>one{}</style><link rel=stylesheet href=rel.css>\
         <style media=screen>two{}</style>\
         <link rel='Stylesheet' href='http://other.test/abs.css'>",
        DOC_URL,
    );
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;

    assert_eq!(texts(&sources), ["one{}", "rel{}", "two{}", "abs{}"]);
    assert_eq!(sources[2].media.as_deref(), Some("screen"));
    assert_eq!(sources[1].url.as_str(), "http://fixture.test/dir/rel.css");
    assert!(sources.iter().all(|s| s.owner.is_some() && !s.disabled));
}

#[tokio::test(flavor = "current_thread")]
async fn base_href_resolves_links_and_is_the_base_of_inline_sheets() {
    let env = Env::new(&[("http://fixture.test/assets/site.css", "text/css", "a{}")]);
    let doc = parse(
        "<base href='http://fixture.test/assets/'><link rel=stylesheet href=site.css><style>b{}</style>",
        DOC_URL,
    );
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;

    assert_eq!(texts(&sources), ["a{}", "b{}"]);
    assert_eq!(sources[1].url.as_str(), "http://fixture.test/assets/");
}

#[tokio::test(flavor = "current_thread")]
async fn elements_that_are_not_applicable_sheets_are_skipped() {
    let env = Env::new(&[("http://fixture.test/dir/a.css", "text/css", "a{}")]);
    let doc = parse(
        "<link rel='alternate stylesheet' href=alt.css>\
         <link rel=icon href=icon.css>\
         <link rel=stylesheet type='text/plain' href=plain.css>\
         <style type='text/x-other'>x{}</style>\
         <link rel=stylesheet href=a.css media=print>\
         <svg><style>svg{}</style></svg>",
        DOC_URL,
    );
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;

    assert_eq!(texts(&sources), ["a{}"]);
    assert_eq!(
        sources[0].media.as_deref(),
        Some("print"),
        "media is the engine's to evaluate against its environment"
    );
    assert_eq!(env.requested(), ["http://fixture.test/dir/a.css"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_disabled_link_is_listed_disabled_and_not_fetched() {
    let env = Env::new(&[]);
    let doc = parse("<link rel=stylesheet href=a.css disabled>", DOC_URL);
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;

    assert_eq!(sources.len(), 1);
    assert!(sources[0].disabled && sources[0].text.is_empty());
    assert!(env.requested().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn imports_ride_on_their_source_and_a_cycle_terminates() {
    let env = Env::new(&[
        (
            "http://fixture.test/dir/a.css",
            "text/css",
            "@import 'b.css'; a{}",
        ),
        (
            "http://fixture.test/dir/b.css",
            "text/css",
            "@import url(a.css) print; b{}",
        ),
    ]);
    let doc = parse(
        "<link rel=stylesheet href=a.css><style>@import 'b.css#frag'; s{}</style>",
        DOC_URL,
    );
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;

    assert_eq!(
        sources.len(),
        2,
        "one source per element, imports not flattened"
    );
    assert_eq!(sources[0].text, "@import 'b.css'; a{}");
    let urls: Vec<_> = sources[0].imports.iter().map(|i| i.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "http://fixture.test/dir/b.css",
            "http://fixture.test/dir/a.css"
        ]
    );
    assert_eq!(
        sources[1].imports.len(),
        2,
        "an inline sheet's imports are fetched too"
    );
    let mut requested = env.requested();
    requested.sort();
    assert_eq!(
        requested,
        [
            "http://fixture.test/dir/a.css",
            "http://fixture.test/dir/b.css"
        ],
        "each URL once, and a fragment does not split the cache"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn import_depth_is_bounded_and_reported_once() {
    let env = Env::generated(|url| {
        let n: u32 = url.rsplit('/').next()?.strip_suffix(".css")?.parse().ok()?;
        Some(format!("@import '{}.css'; s{n}{{}}", n + 1))
    });
    let doc = parse("<link rel=stylesheet href=0.css>", DOC_URL);
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;
    collect(&doc, &env, &mut state).await;

    assert_eq!(sources[0].imports.len(), usize::from(MAX_IMPORT_DEPTH));
    let failures = state.take_failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].reason.contains("deeper"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_wide_deep_import_tree_stays_bounded() {
    // Every sheet imports 50 children with names unique to their path, so the
    // tree has 50^4 distinct sheets.
    let env = Env::generated(|url| {
        let stem = url.rsplit('/').next()?.strip_suffix(".css")?;
        let imports: String = (0..50)
            .map(|i| format!("@import '{stem}x{i}.css';"))
            .collect();
        Some(format!("{imports} s{{}}"))
    });
    let doc = parse("<link rel=stylesheet href=r.css>", DOC_URL);
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;

    assert_eq!(sources[0].imports.len(), MAX_IMPORTS_PER_SOURCE);
    assert!(
        env.requested().len() <= MAX_IMPORTS_PER_SOURCE + 2,
        "{} requests",
        env.requested().len()
    );
    assert!(
        state
            .take_failures()
            .iter()
            .any(|f| f.reason.contains("imported sheets"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_sheet_is_absent_reported_once_and_not_retried() {
    let env = Env::new(&[]);
    let doc = parse(
        "<style>kept{}</style><link rel=stylesheet href=gone.css>",
        DOC_URL,
    );
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;
    let again = collect(&doc, &env, &mut state).await;

    assert_eq!(texts(&sources), ["kept{}"]);
    assert_eq!(texts(&again), ["kept{}"]);
    let failures = state.take_failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].url.as_str(), "http://fixture.test/dir/gone.css");
    assert_eq!(
        env.requested().len(),
        1,
        "the failure is cached for the navigation"
    );
    assert!(state.take_failures().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_non_ok_status_is_a_failure() {
    let env = Env::with_routes(HashMap::from([(
        "http://fixture.test/dir/a.css".to_owned(),
        (StatusCode::NOT_FOUND, vec![], b"a{}".to_vec()),
    )]));
    let doc = parse("<link rel=stylesheet href=a.css>", DOC_URL);
    let mut state = StyleState::default();

    assert!(collect(&doc, &env, &mut state).await.is_empty());
    assert!(state.take_failures()[0].reason.contains("404"));
}

#[tokio::test(flavor = "current_thread")]
async fn requests_carry_the_document_as_initiator_and_the_stylesheet_type() {
    let env = Env::new(&[("http://fixture.test/dir/a.css", "text/css", "a{}")]);
    let doc = parse("<link rel=stylesheet href=a.css>", DOC_URL);
    collect(&doc, &env, &mut StyleState::default()).await;

    let seen = env.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].initiator.as_ref().map(Url::as_str), Some(DOC_URL));
    assert_eq!(seen[0].resource_type, ResourceType::Stylesheet);
}

#[tokio::test(flavor = "current_thread")]
async fn a_tier_that_only_records_style_hints_fetches_nothing() {
    let mut env = Env::new(&[("http://fixture.test/dir/a.css", "text/css", "a{}")]);
    env.tier = Tier::Dom;
    let doc = parse(
        "<style>i{}</style><link rel=stylesheet href=a.css>",
        DOC_URL,
    );
    let sources = collect(&doc, &env, &mut StyleState::default()).await;

    assert_eq!(texts(&sources), ["i{}"]);
    assert!(env.requested().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_pending_link_is_absent_until_its_body_is_cached() {
    let env = Env::new(&[("http://fixture.test/dir/a.css", "text/css", "a{}")]);
    let doc = parse(
        "<style>i{}</style><link rel=stylesheet href=a.css>",
        DOC_URL,
    );
    let mut state = StyleState::default();
    let before = state.signature(&doc);

    assert_eq!(texts(&state.cached_sources(&doc, &base(&doc))), ["i{}"]);
    collect(&doc, &env, &mut state).await;

    assert_eq!(state.completed_fetches(), 1);
    assert_ne!(
        state.signature(&doc),
        before,
        "an arrival forces a re-collect"
    );
    assert_eq!(
        texts(&state.cached_sources(&doc, &base(&doc))),
        ["i{}", "a{}"]
    );
}

fn set_attribute(doc: &mut Document, element: NodeId, name: tenta_core::atom::Atom, value: &str) {
    doc.mutate(Mutation::SetAttribute {
        element,
        name: QualName::new(atoms::EMPTY, name),
        value: value.to_owned(),
    })
    .expect("set the attribute");
}

#[test]
fn the_signature_tracks_exactly_what_changes_the_sheet_list() {
    let mut doc = parse(
        "<div id=d></div><style id=s>a{}</style><link id=l rel=stylesheet href=a.css>\
         <link id=icon rel=icon href=i.png>",
        DOC_URL,
    );
    let id = |doc: &Document, name: &str| doc.get_element_by_id(name).expect(name);
    let (div, style, link, icon) = (
        id(&doc, "d"),
        id(&doc, "s"),
        id(&doc, "l"),
        id(&doc, "icon"),
    );
    let sig = |doc: &Document| StyleSignature::of(doc, 0);
    let initial = sig(&doc);

    set_attribute(&mut doc, div, atoms::CLASS, "x");
    set_attribute(&mut doc, icon, atoms::HREF, "j.png");
    assert_eq!(sig(&doc), initial, "unrelated mutations leave it alone");

    doc.set_inner_html(style, "b{}").unwrap();
    let after_text = sig(&doc);
    assert_ne!(after_text, initial, "a <style> text change");

    set_attribute(&mut doc, link, atoms::MEDIA, "print");
    let after_media = sig(&doc);
    assert_ne!(after_media, after_text, "a media change");

    set_attribute(&mut doc, link, atoms::HREF, "b.css");
    assert_ne!(sig(&doc), after_media, "an href change");

    let added = doc.create_html_element(atoms::LINK);
    let head = doc.body().unwrap();
    doc.mutate(Mutation::Append {
        parent: head,
        child: added,
    })
    .unwrap();
    let before_rel = sig(&doc);
    set_attribute(&mut doc, added, atoms::REL, "stylesheet");
    assert_ne!(sig(&doc), before_rel, "a new stylesheet link");

    assert_ne!(StyleSignature::of(&doc, 1), sig(&doc), "a completed fetch");
}

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tenta-styles-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test(flavor = "current_thread")]
async fn a_file_stylesheet_is_read_only_inside_the_read_roots() {
    let allowed = scratch_dir("allowed");
    let denied = scratch_dir("denied");
    std::fs::write(allowed.join("ok.css"), "ok{}").unwrap();
    std::fs::write(denied.join("secret.css"), "secret{}").unwrap();

    let document_url = Url::from_file_path(allowed.join("index.html")).unwrap();
    let mut env = Env::new(&[]);
    env.fs_policy = FsPolicy::none().allow_read(&allowed);
    env.document_url = document_url.clone();
    let html = format!(
        "<link rel=stylesheet href=ok.css><link rel=stylesheet href='{}'>",
        Url::from_file_path(denied.join("secret.css")).unwrap(),
    );
    let doc = parse(&html, document_url.as_str());
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;

    assert_eq!(texts(&sources), ["ok{}"]);
    let failures = state.take_failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].reason.contains("policy"), "{failures:?}");
    assert!(
        env.requested().is_empty(),
        "file: never reaches the network"
    );
    let _ = std::fs::remove_dir_all(&allowed);
    let _ = std::fs::remove_dir_all(&denied);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_symlink_out_of_the_read_roots_is_refused() {
    let allowed = scratch_dir("link-allowed");
    let denied = scratch_dir("link-denied");
    std::fs::write(denied.join("secret.css"), "secret{}").unwrap();
    std::os::unix::fs::symlink(denied.join("secret.css"), allowed.join("alias.css")).unwrap();

    let document_url = Url::from_file_path(allowed.join("index.html")).unwrap();
    let mut env = Env::new(&[]);
    env.fs_policy = FsPolicy::none().allow_read(&allowed);
    env.document_url = document_url.clone();
    let doc = parse(
        "<link rel=stylesheet href=alias.css>",
        document_url.as_str(),
    );

    assert!(
        collect(&doc, &env, &mut StyleState::default())
            .await
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(&allowed);
    let _ = std::fs::remove_dir_all(&denied);
}

#[tokio::test(flavor = "current_thread")]
async fn a_network_document_cannot_load_a_file_stylesheet() {
    let dir = scratch_dir("net-doc");
    std::fs::write(dir.join("a.css"), "a{}").unwrap();
    let mut env = Env::new(&[]);
    env.fs_policy = FsPolicy::none().allow_read(&dir);
    let doc = parse(
        &format!(
            "<link rel=stylesheet href='{}'>",
            Url::from_file_path(dir.join("a.css")).unwrap()
        ),
        DOC_URL,
    );
    let mut state = StyleState::default();

    assert!(collect(&doc, &env, &mut state).await.is_empty());
    assert!(state.take_failures()[0].reason.contains("file: document"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stylesheets_decode_by_bom_then_transport_then_at_charset_then_fallback() {
    let latin1 = Encoding::for_label(b"iso-8859-1").unwrap();
    let utf8 = utf8();
    let decode = |body: &[u8], transport: Option<&str>, fallback| {
        decode_stylesheet(body, transport, fallback).0
    };
    // 0xE9 is "é" in Latin-1 and invalid on its own in UTF-8.
    assert_eq!(decode(b"a\xE9", Some("iso-8859-1"), utf8), "a\u{e9}");
    assert_eq!(decode(b"a\xE9", None, latin1), "a\u{e9}");
    assert_eq!(
        decode(b"@charset \"iso-8859-1\"; a\xE9", None, utf8),
        "@charset \"iso-8859-1\"; a\u{e9}"
    );
    assert_eq!(
        decode(b"\xEF\xBB\xBF\xC3\xA9", Some("iso-8859-1"), latin1),
        "\u{e9}",
        "a byte-order mark beats the transport"
    );
    assert!(
        decode(b"@charset \"utf-8\";a\xE9", Some("iso-8859-1"), utf8).ends_with('\u{e9}'),
        "the transport beats @charset"
    );
    assert_eq!(
        charset_param("text/css; Charset=\"ISO-8859-1\""),
        Some("ISO-8859-1".to_owned())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_import_inherits_its_importers_encoding() {
    let env = Env::with_routes(HashMap::from([
        (
            "http://fixture.test/dir/a.css".to_owned(),
            (
                StatusCode::OK,
                vec![("content-type", "text/css; charset=iso-8859-1")],
                b"@import 'b.css';".to_vec(),
            ),
        ),
        (
            "http://fixture.test/dir/b.css".to_owned(),
            (
                StatusCode::OK,
                vec![("content-type", "text/css")],
                b"b\xE9{}".to_vec(),
            ),
        ),
    ]));
    let doc = parse("<link rel=stylesheet href=a.css>", DOC_URL);
    let sources = collect(&doc, &env, &mut StyleState::default()).await;

    assert_eq!(sources[0].imports[0].text, "b\u{e9}{}");
}

#[test]
fn data_urls_decode_percent_and_base64_payloads() {
    let read = |url: &str| read_data(&Url::parse(url).unwrap(), 100);
    assert_eq!(read("data:text/css,a%7Bb%7D").unwrap().0, b"a{b}");
    assert_eq!(
        read("data:text/css;charset=iso-8859-1;base64,YXtifQ").unwrap(),
        (b"a{b}".to_vec(), Some("iso-8859-1".to_owned()))
    );
    assert!(read("data:text/css;base64,a").is_err());
    assert!(read("data:text/css").is_err());
    assert!(read_data(&Url::parse("data:text/css,aaaa").unwrap(), 3).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn a_linked_sheet_is_decoded_with_its_responses_charset() {
    let env = Env::with_routes(HashMap::from([(
        "http://fixture.test/dir/a.css".to_owned(),
        (
            StatusCode::OK,
            vec![("content-type", "text/css; charset=iso-8859-1")],
            b"a\xE9".to_vec(),
        ),
    )]));
    let doc = parse("<link rel=stylesheet href=a.css>", DOC_URL);
    let sources = collect(&doc, &env, &mut StyleState::default()).await;

    assert_eq!(texts(&sources), ["a\u{e9}"]);
}

#[tokio::test(flavor = "current_thread")]
async fn only_http_https_file_and_data_sheets_are_loaded() {
    let env = Env::new(&[]);
    let doc = parse(
        "<link rel=stylesheet href='javascript:alert(1)'>\
         <link rel=stylesheet href='blob:http://fixture.test/x'>\
         <link rel=stylesheet href='ftp://fixture.test/x.css'>\
         <link rel=stylesheet href='data:text/css,d%7Bx%7D'>",
        DOC_URL,
    );
    let mut state = StyleState::default();
    let sources = collect(&doc, &env, &mut state).await;

    assert_eq!(texts(&sources), ["d{x}"]);
    let failures = state.take_failures();
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(failures.iter().all(|f| f.reason.contains("scheme")));
    assert!(env.requested().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn standards_mode_wants_text_css_and_nosniff_forbids_the_quirks_tolerance() {
    let env = Env::with_routes(HashMap::from([
        (
            "http://fixture.test/dir/plain.css".to_owned(),
            (
                StatusCode::OK,
                vec![("content-type", "text/plain")],
                b"p{}".to_vec(),
            ),
        ),
        (
            "http://fixture.test/dir/sniff.css".to_owned(),
            (
                StatusCode::OK,
                vec![
                    ("content-type", "text/plain"),
                    ("x-content-type-options", "nosniff"),
                ],
                b"n{}".to_vec(),
            ),
        ),
    ]));
    let links = "<link rel=stylesheet href=plain.css><link rel=stylesheet href=sniff.css>";

    let standards = parse(&format!("<!doctype html>{links}"), DOC_URL);
    assert!(
        collect(&standards, &env, &mut StyleState::default())
            .await
            .is_empty()
    );

    let quirks = parse(links, DOC_URL);
    let sources = collect(&quirks, &env, &mut StyleState::default()).await;
    assert_eq!(
        texts(&sources),
        ["p{}"],
        "quirks mode tolerates the type, nosniff does not"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_exhausted_budget_is_not_a_permanent_failure() {
    let mut env = Env::new(&[
        ("http://fixture.test/dir/a.css", "text/css", "a{}"),
        ("http://fixture.test/dir/b.css", "text/css", "b{}"),
    ]);
    env.budgets.network_requests = 1;
    let doc = parse(
        "<link rel=stylesheet href=a.css><link rel=stylesheet href=b.css>",
        DOC_URL,
    );
    let mut state = StyleState::default();

    assert_eq!(texts(&collect(&doc, &env, &mut state).await), ["a{}"]);
    assert!(state.take_failures()[0].reason.contains("budget"));
    collect(&doc, &env, &mut state).await;
    assert!(
        state.take_failures().is_empty(),
        "the same refusal is reported once"
    );

    env.budgets.network_requests = 10;
    assert_eq!(
        texts(&collect(&doc, &env, &mut state).await),
        ["a{}", "b{}"]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_byte_budget_is_charged_across_sheets() {
    let mut env = Env::new(&[
        ("http://fixture.test/dir/a.css", "text/css", "aaaa"),
        ("http://fixture.test/dir/b.css", "text/css", "bbbb"),
    ]);
    env.budgets.network_bytes = 5;
    let doc = parse(
        "<link rel=stylesheet href=a.css><link rel=stylesheet href=b.css>",
        DOC_URL,
    );
    let sources = collect(&doc, &env, &mut StyleState::default()).await;

    assert_eq!(
        texts(&sources),
        ["aaaa"],
        "the second sheet only had one byte left"
    );
}

#[test]
fn the_signature_follows_the_document_encoding() {
    let mut doc = parse("<style>a{}</style>", DOC_URL);
    let before = StyleSignature::of(&doc, 0);
    doc.set_encoding("windows-1252");
    assert_ne!(StyleSignature::of(&doc, 0), before);
}

#[tokio::test(flavor = "current_thread")]
async fn an_oversize_file_is_refused_not_truncated() {
    let dir = scratch_dir("oversize");
    std::fs::write(dir.join("big.css"), "0123456789").unwrap();
    let document_url = Url::from_file_path(dir.join("index.html")).unwrap();
    let mut env = Env::new(&[]);
    env.fs_policy = FsPolicy::none().allow_read(&dir);
    env.document_url = document_url.clone();
    env.budgets.network_bytes = 5;
    let doc = parse("<link rel=stylesheet href=big.css>", document_url.as_str());
    let mut state = StyleState::default();

    assert!(collect(&doc, &env, &mut state).await.is_empty());
    assert!(state.take_failures()[0].reason.contains("size limit"));
    let _ = std::fs::remove_dir_all(&dir);
}
