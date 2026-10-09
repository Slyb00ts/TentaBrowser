//! Fetching and decoding images against an interceptor that stands in for the
//! network: what is requested, how many at once, and what fails how.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::StatusCode;
use tenta_net::hooks::{BoxFuture, FulfilledResponse, Hooks, Interception, Interceptor};
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::request::Request;
use tenta_net::{ClientConfig, ErrorName, NetRuntime};
use tenta_render::{ImageKey, RenderConfig, RenderSession, Rerender, WantOrigin};

use super::*;
use crate::clock::system_clock;

const DOC_URL: &str = "http://fixture.test/dir/page.html";

fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("header");
    let data: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
    writer.write_image_data(&data).expect("pixels");
    drop(writer);
    out
}

#[derive(Clone, Debug)]
struct Seen {
    url: String,
    initiator: Option<Url>,
    resource_type: ResourceType,
}

type Answer = (StatusCode, &'static str, Vec<u8>);

struct Fixture {
    routes: HashMap<String, Answer>,
    /// How long every answer takes, so requests overlap.
    delay: Duration,
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// Incremented when a request's future is dropped before it answered.
    abandoned: Arc<AtomicUsize>,
}

struct Guard {
    in_flight: Arc<AtomicUsize>,
    abandoned: Arc<AtomicUsize>,
    done: bool,
}

impl Guard {
    /// The request answered: dropping it now is not an abandonment.
    fn finish(mut self) {
        self.done = true;
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        if !self.done {
            self.abandoned.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Interceptor for Fixture {
    fn intercept<'a>(&'a self, request: &'a mut Request) -> BoxFuture<'a, Interception> {
        self.seen.lock().unwrap().push(Seen {
            url: request.url.to_string(),
            initiator: request.initiator.clone(),
            resource_type: request.resource_type,
        });
        let answer = self.routes.get(request.url.as_str()).cloned();
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let guard = Guard {
            in_flight: Arc::clone(&self.in_flight),
            abandoned: Arc::clone(&self.abandoned),
            done: false,
        };
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            guard.finish();
            match answer {
                Some((status, content_type, body)) => Interception::Fulfill(
                    FulfilledResponse::new(status, Bytes::from(body))
                        .header("content-type", content_type),
                ),
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
    peak: Arc<AtomicUsize>,
    abandoned: Arc<AtomicUsize>,
    fs_policy: FsPolicy,
    budgets: Budgets,
    clock: Arc<dyn Clock>,
    document_url: Url,
    settings: DecodeSettings,
}

impl Env {
    fn new(routes: Vec<(&str, Answer)>, delay: Duration) -> Env {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let peak = Arc::new(AtomicUsize::new(0));
        let abandoned = Arc::new(AtomicUsize::new(0));
        let net = NetRuntime::new(1).expect("io runtime");
        let client = net
            .client(ClientConfig {
                hooks: Hooks {
                    interceptors: vec![Arc::new(Fixture {
                        routes: routes
                            .into_iter()
                            .map(|(url, answer)| (url.to_owned(), answer))
                            .collect(),
                        delay,
                        in_flight: Arc::new(AtomicUsize::new(0)),
                        peak: Arc::clone(&peak),
                        seen: Arc::clone(&seen),
                        abandoned: Arc::clone(&abandoned),
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
            peak,
            abandoned,
            fs_policy: FsPolicy::none(),
            budgets: Budgets::default(),
            clock: system_clock(),
            document_url: Url::parse(DOC_URL).unwrap(),
            settings: RenderSession::new(RenderConfig::default()).decode_settings(),
        }
    }

    fn ctx(&self, wait: Duration) -> ImageContext<'_> {
        ImageContext {
            client: &self.client,
            fs_policy: &self.fs_policy,
            budgets: &self.budgets,
            clock: &*self.clock,
            extra_headers: &[],
            document_url: &self.document_url,
            deadline: Instant::now() + wait,
            rolling: None,
        }
    }

    fn plan(&self, urls: &[&str]) -> ImagePlan {
        ImagePlan {
            wants: urls.iter().map(|u| want(u)).collect(),
            decode: self.settings,
        }
    }
}

fn want(url: &str) -> ImageWant {
    let url = Url::parse(url).unwrap();
    ImageWant {
        key: ImageKey(url.as_str().into()),
        url,
        origin: WantOrigin::Element,
        rerender: None,
    }
}

const OK: StatusCode = StatusCode::OK;

#[tokio::test(flavor = "current_thread")]
async fn images_are_requested_as_images_with_the_document_as_initiator() {
    let env = Env::new(
        vec![(
            "http://fixture.test/a.png",
            (OK, "image/png", png(3, 2, [255, 0, 0, 255])),
        )],
        Duration::ZERO,
    );
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(async {
            load_images(
                env.plan(&["http://fixture.test/a.png"]),
                &env.ctx(Duration::from_secs(30)),
                &mut ImageLedger::default(),
            )
            .await
        })
        .await;
    let decoded = loaded[0].result.as_ref().expect("decoded");
    assert_eq!((decoded.image.width, decoded.image.height), (3, 2));
    let seen = env.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url, "http://fixture.test/a.png");
    assert_eq!(seen[0].resource_type, ResourceType::Image);
    assert_eq!(seen[0].initiator.as_ref().map(Url::as_str), Some(DOC_URL));
}

#[tokio::test(flavor = "current_thread")]
async fn a_streaming_load_takes_wants_that_arrive_while_it_waits_and_reports_each_as_it_ends() {
    let env = Env::new(
        vec![
            (
                "http://fixture.test/a.png",
                (OK, "image/png", png(2, 2, [1, 0, 0, 255])),
            ),
            (
                "http://fixture.test/b.png",
                (OK, "image/png", png(3, 3, [2, 0, 0, 255])),
            ),
        ],
        Duration::from_millis(80),
    );
    let feed = std::cell::RefCell::new(vec![want("http://fixture.test/a.png")]);
    let wake = tokio::sync::Notify::new();
    let ended = std::cell::RefCell::new(Vec::new());
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let late = async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                feed.borrow_mut().push(want("http://fixture.test/b.png"));
                wake.notify_one();
            };
            let ctx = ImageContext {
                rolling: Some(Duration::from_secs(30)),
                ..env.ctx(Duration::from_secs(30))
            };
            let mut ledger = ImageLedger::default();
            let driving = drive(
                env.settings,
                &ctx,
                &mut ledger,
                Some(&wake),
                || std::mem::take(&mut *feed.borrow_mut()),
                |index, loaded| {
                    ended
                        .borrow_mut()
                        .push((index, loaded.result.map(|d| d.image.width)));
                },
            );
            tokio::join!(late, driving);
        })
        .await;
    let ended = ended.into_inner();
    assert_eq!(ended.len(), 2, "{ended:?}");
    assert!(
        ended.contains(&(0, Ok(2))) && ended.contains(&(1, Ok(3))),
        "{ended:?}"
    );
    assert_eq!(
        env.peak.load(Ordering::SeqCst),
        2,
        "the late want ran beside the first, not after it"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn at_most_six_images_from_one_host_are_in_flight() {
    let routes: Vec<_> = (0..20)
        .map(|i| {
            (
                format!("http://fixture.test/{i}.png"),
                (OK, "image/png", png(2, 2, [i as u8, 0, 0, 255])),
            )
        })
        .collect();
    let urls: Vec<String> = routes.iter().map(|(u, _)| u.clone()).collect();
    let env = Env::new(
        routes
            .iter()
            .map(|(u, a)| (u.as_str(), a.clone()))
            .collect(),
        Duration::from_millis(40),
    );
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            env.plan(&refs),
            &env.ctx(Duration::from_secs(30)),
            &mut ImageLedger::default(),
        ))
        .await;
    assert_eq!(loaded.len(), 20);
    assert!(loaded.iter().all(|l| l.result.is_ok()));
    let peak = env.peak.load(Ordering::SeqCst);
    assert!(
        peak <= MAX_PER_HOST && peak > 1,
        "peak in flight was {peak}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn each_failure_is_that_images_own_outcome() {
    let env = Env::new(
        vec![
            (
                "http://fixture.test/ok.png",
                (OK, "image/png", png(2, 2, [0, 255, 0, 255])),
            ),
            (
                "http://fixture.test/gone.png",
                (StatusCode::NOT_FOUND, "text/html", b"no".to_vec()),
            ),
            (
                "http://fixture.test/bad.png",
                (OK, "image/png", b"\x89PNG\r\n\x1a\nxx".to_vec()),
            ),
            (
                "http://fixture.test/html.png",
                (OK, "text/html", b"<html>".to_vec()),
            ),
        ],
        Duration::ZERO,
    );
    let data = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
    let plan = env.plan(&[
        "http://fixture.test/ok.png",
        "http://fixture.test/gone.png",
        "http://fixture.test/bad.png",
        "http://fixture.test/html.png",
        "http://fixture.test/unrouted.png",
        data,
        "ftp://fixture.test/x.png",
    ]);
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            plan,
            &env.ctx(Duration::from_secs(30)),
            &mut ImageLedger::default(),
        ))
        .await;
    let why = |i: usize| loaded[i].result.as_ref().err().cloned().unwrap_or_default();
    assert!(loaded[0].result.is_ok());
    assert!(why(1).contains("404"), "{}", why(1));
    assert!(why(2).contains("PNG"), "{}", why(2));
    assert!(!why(3).is_empty(), "an HTML body is not an image");
    assert!(!why(4).is_empty(), "no route");
    assert!(
        loaded[5].result.is_ok(),
        "a data: URL needs no request: {}",
        why(5)
    );
    assert!(why(6).contains("ftp"), "{}", why(6));
    assert_eq!(
        env.seen.lock().unwrap().len(),
        5,
        "the data: URL made no request"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_request_budget_of_a_navigation_is_charged() {
    let mut env = Env::new(
        vec![
            (
                "http://fixture.test/1.png",
                (OK, "image/png", png(2, 2, [1, 0, 0, 255])),
            ),
            (
                "http://fixture.test/2.png",
                (OK, "image/png", png(2, 2, [2, 0, 0, 255])),
            ),
            (
                "http://fixture.test/3.png",
                (OK, "image/png", png(2, 2, [3, 0, 0, 255])),
            ),
        ],
        Duration::ZERO,
    );
    env.budgets.network_requests = 2;
    let mut ledger = ImageLedger::default();
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            env.plan(&[
                "http://fixture.test/1.png",
                "http://fixture.test/2.png",
                "http://fixture.test/3.png",
            ]),
            &env.ctx(Duration::from_secs(30)),
            &mut ledger,
        ))
        .await;
    assert!(loaded[0].result.is_ok() && loaded[1].result.is_ok());
    assert!(
        loaded[2]
            .result
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("request budget"))
    );
    assert!(ledger.bytes > 0, "bytes were charged");

    // A byte budget below one image refuses the body.
    env.budgets.network_requests = 100;
    env.budgets.network_bytes = 20;
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            env.plan(&["http://fixture.test/1.png"]),
            &env.ctx(Duration::from_secs(30)),
            &mut ImageLedger::default(),
        ))
        .await;
    assert!(
        loaded[0].result.is_err(),
        "a body over the byte budget fails"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_image_that_outlasts_the_wait_fails_and_the_wait_is_not_exceeded() {
    let env = Env::new(
        vec![(
            "http://fixture.test/slow.png",
            (OK, "image/png", png(2, 2, [1, 0, 0, 255])),
        )],
        Duration::from_secs(60),
    );
    let started = Instant::now();
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            env.plan(&["http://fixture.test/slow.png"]),
            &env.ctx(Duration::from_millis(300)),
            &mut ImageLedger::default(),
        ))
        .await;
    assert!(loaded[0].result.is_err());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_the_load_abandons_the_requests() {
    let env = Env::new(
        vec![
            (
                "http://fixture.test/a.png",
                (OK, "image/png", png(2, 2, [1, 0, 0, 255])),
            ),
            (
                "http://fixture.test/b.png",
                (OK, "image/png", png(2, 2, [2, 0, 0, 255])),
            ),
        ],
        Duration::from_secs(60),
    );
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut ledger = ImageLedger::default();
            let ctx = env.ctx(Duration::from_secs(120));
            let load = load_images(
                env.plan(&["http://fixture.test/a.png", "http://fixture.test/b.png"]),
                &ctx,
                &mut ledger,
            );
            let outcome = tokio::time::timeout(Duration::from_millis(150), load).await;
            assert!(outcome.is_err(), "still loading");
            // Let the aborted tasks unwind.
            tokio::time::sleep(Duration::from_millis(100)).await;
        })
        .await;
    assert_eq!(
        env.abandoned.load(Ordering::SeqCst),
        2,
        "both requests were dropped"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn files_load_only_for_a_file_document_and_inside_the_read_roots() {
    let dir = tenta_testutil::temp_dir("page-images");
    dir.write("a.png", png(2, 2, [9, 9, 9, 255]));
    let file_url = Url::from_file_path(dir.path().join("a.png")).unwrap();
    let mut env = Env::new(Vec::new(), Duration::ZERO);
    env.fs_policy = FsPolicy::none().allow_read(dir.path());
    let local = tokio::task::LocalSet::new();

    // An http document is refused a local file.
    let loaded = local
        .run_until(load_images(
            env.plan(&[file_url.as_str()]),
            &env.ctx(Duration::from_secs(30)),
            &mut ImageLedger::default(),
        ))
        .await;
    assert!(
        loaded[0]
            .result
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("file: document"))
    );

    // A file document may read inside its roots, and only there.
    env.document_url = Url::from_file_path(dir.path().join("page.html")).unwrap();
    let outside = Url::from_file_path("/etc/hosts").unwrap();
    let loaded = local
        .run_until(load_images(
            env.plan(&[file_url.as_str(), outside.as_str()]),
            &env.ctx(Duration::from_secs(30)),
            &mut ImageLedger::default(),
        ))
        .await;
    assert!(
        loaded[0].result.is_ok(),
        "{:?}",
        loaded[0].result.as_ref().err()
    );
    assert!(loaded[1].result.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn an_svg_is_kept_and_rendered_again_without_another_request() {
    let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><rect width='10' height='10' fill='red'/></svg>";
    let env = Env::new(
        vec![(
            "http://fixture.test/i.svg",
            (OK, "image/svg+xml", svg.as_bytes().to_vec()),
        )],
        Duration::ZERO,
    );
    let mut ledger = ImageLedger::default();
    let local = tokio::task::LocalSet::new();
    let first = local
        .run_until(load_images(
            env.plan(&["http://fixture.test/i.svg"]),
            &env.ctx(Duration::from_secs(30)),
            &mut ledger,
        ))
        .await;
    assert_eq!(first[0].result.as_ref().expect("renders").image.width, 10);
    let mut again = env.plan(&["http://fixture.test/i.svg"]);
    again.wants[0].rerender = Some(Rerender::Svg { scale: 4.0 });
    let second = local
        .run_until(load_images(
            again,
            &env.ctx(Duration::from_secs(30)),
            &mut ledger,
        ))
        .await;
    assert_eq!(second[0].result.as_ref().expect("renders").image.width, 40);
    assert_eq!(
        env.seen.lock().unwrap().len(),
        1,
        "one request for two renderings"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn many_hosts_share_a_total_of_twenty_four() {
    let routes: Vec<_> = (0..60)
        .map(|i| {
            (
                format!("http://host{}.test/{i}.png", i % 10),
                (OK, "image/png", png(2, 2, [i as u8, 0, 0, 255])),
            )
        })
        .collect();
    let urls: Vec<String> = routes.iter().map(|(u, _)| u.clone()).collect();
    let env = Env::new(
        routes
            .iter()
            .map(|(u, a)| (u.as_str(), a.clone()))
            .collect(),
        Duration::from_millis(60),
    );
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            env.plan(&refs),
            &env.ctx(Duration::from_secs(30)),
            &mut ImageLedger::default(),
        ))
        .await;
    assert!(loaded.iter().all(|l| l.result.is_ok()));
    let peak = env.peak.load(Ordering::SeqCst);
    assert!(
        peak > MAX_PER_HOST && peak <= MAX_TOTAL,
        "peak in flight was {peak}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_requests_reserve_their_bytes_so_the_budget_is_not_overshot() {
    let png_bytes = png(2, 2, [1, 0, 0, 255]);
    let urls: Vec<String> = (0..6)
        .map(|i| format!("http://fixture.test/{i}.png"))
        .collect();
    let mut env = Env::new(
        urls.iter()
            .map(|u| (u.as_str(), (OK, "image/png", png_bytes.clone())))
            .collect(),
        Duration::from_millis(30),
    );
    // Room for one body limit at a time: the others wait for it to settle
    // instead of each assuming the whole budget is theirs.
    env.budgets.network_bytes = 4096;
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    let mut ledger = ImageLedger::default();
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            env.plan(&refs),
            &env.ctx(Duration::from_secs(30)),
            &mut ledger,
        ))
        .await;
    assert!(
        loaded.iter().all(|l| l.result.is_ok()),
        "deferred, not failed"
    );
    assert_eq!(
        env.peak.load(Ordering::SeqCst),
        1,
        "one reservation held the budget at a time"
    );
    assert_eq!(ledger.reserved, 0, "every reservation was settled");
    assert_eq!(ledger.bytes, 6 * png_bytes.len() as u64);
}

#[tokio::test(flavor = "current_thread")]
async fn blob_javascript_and_about_images_are_refused() {
    let env = Env::new(Vec::new(), Duration::ZERO);
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            env.plan(&[
                "blob:http://fixture.test/uuid",
                "javascript:alert(1)",
                "about:blank",
                "ws://fixture.test/x",
            ]),
            &env.ctx(Duration::from_secs(30)),
            &mut ImageLedger::default(),
        ))
        .await;
    for l in &loaded {
        let reason = l.result.as_ref().expect_err("refused");
        assert!(reason.contains("not loaded from"), "{reason}");
    }
    assert!(
        env.seen.lock().unwrap().is_empty(),
        "nothing went to the network"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_default_destination_policy_blocks_loopback_and_metadata_addresses() {
    let net = NetRuntime::new(1).expect("io runtime");
    let client = net.client(ClientConfig::default()).expect("client");
    let mut env = Env::new(Vec::new(), Duration::ZERO);
    env.client = client;
    env._net = net;
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            env.plan(&[
                "http://127.0.0.1:9/a.png",
                "http://169.254.169.254/latest/meta-data/x.png",
            ]),
            &env.ctx(Duration::from_secs(10)),
            &mut ImageLedger::default(),
        ))
        .await;
    for l in &loaded {
        assert!(l.result.is_err(), "{} must not load", l.want.url);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_body_is_sniffed_but_an_svg_needs_its_declared_type() {
    // Cross-origin images are not origin-clean: when canvas drawImage lands, a
    // flag on the decoded image must record that (IM4); nothing here reads it.
    let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'/>";
    let env = Env::new(
        vec![
            (
                "http://fixture.test/png-as-text",
                (OK, "text/plain", png(2, 2, [1, 2, 3, 255])),
            ),
            (
                "http://fixture.test/svg-as-text",
                (OK, "text/plain", svg.as_bytes().to_vec()),
            ),
            (
                "http://fixture.test/svg-ok",
                (OK, "image/svg+xml; charset=utf-8", svg.as_bytes().to_vec()),
            ),
        ],
        Duration::ZERO,
    );
    let local = tokio::task::LocalSet::new();
    let loaded = local
        .run_until(load_images(
            env.plan(&[
                "http://fixture.test/png-as-text",
                "http://fixture.test/svg-as-text",
                "http://fixture.test/svg-ok",
            ]),
            &env.ctx(Duration::from_secs(30)),
            &mut ImageLedger::default(),
        ))
        .await;
    assert!(
        loaded[0].result.is_ok(),
        "a PNG is a PNG whatever it is served as"
    );
    assert!(
        loaded[1].result.is_err(),
        "an SVG served as text is not an image"
    );
    assert!(loaded[2].result.is_ok());
}
