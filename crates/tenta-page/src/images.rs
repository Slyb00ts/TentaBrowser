//! Fetching and decoding the images of a render (feature `render`,
//! crate-private).
//!
//! The render session lists what the document wants ([`ImagePlan`]); this
//! module fetches it and returns decoded pixels for the session to install.
//! Every request goes through the page's [`Client`], so the proxy, cookies,
//! `Referer` and the page's headers apply exactly as they do to scripts and
//! stylesheets, and the document is the initiator and the first party.
//!
//! Work is bounded four ways: at most [`MAX_CONCURRENT`] images are in flight,
//! each URL is fetched once (the plan is deduplicated and the session
//! remembers what it has), the bytes and requests of a navigation are charged
//! against the page's network budgets, and a decode runs under a deadline and a
//! cancel flag that is raised when the load is dropped (the screenshot was
//! abandoned or the page navigated). Decoding happens on the blocking pool, so
//! the page thread keeps serving timers and other calls meanwhile.
//!
//! A failed image is not an error of the render: the session marks it broken
//! and lays it out as a broken image.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tenta_core::{Budgets, FsPolicy};
use tenta_net::policy::cookies::Site;
use tenta_net::{Client, Request, ResourceType};
use tenta_render::{
    DecodeControl, DecodeError, DecodeSettings, Decoded, ImageKey, ImageKind, ImagePlan, ImageWant,
    Rerender, decode_data_url, decode_image,
};
use tokio::task::JoinSet;
use url::Url;

use crate::clock::Clock;
use crate::styles::read_local_file;

/// Images fetched at once from one host, and at once in all.
const MAX_PER_HOST: usize = 6;
const MAX_TOTAL: usize = 24;

/// Largest body of one image, before the page's remaining byte budget.
const MAX_IMAGE_BYTES: u64 = 32 << 20;

/// How long one decode may run.
const DECODE_TIMEOUT: Duration = Duration::from_secs(10);

/// The most a body may weigh and still be kept to decode again at another
/// size, by kind, and the most all kept bodies of a navigation may weigh.
const MAX_RETAINED_SVG: usize = 4 << 20;
const MAX_RETAINED_RASTER: usize = 8 << 20;
const MAX_RETAINED_TOTAL: usize = 32 << 20;

/// What the images of one navigation have cost so far, and the bodies kept to
/// decode them again at the size their boxes turn out to need (a viewport or
/// device ratio change, or a box much smaller than the first decode).
#[derive(Default)]
pub(crate) struct ImageLedger {
    /// Bytes of bodies that arrived.
    bytes: u64,
    /// Bytes promised to requests still in flight, so that concurrent
    /// requests cannot together overshoot the budget.
    reserved: u64,
    requests: u32,
    bodies: HashMap<ImageKey, (Bytes, ImageKind)>,
    retained: usize,
}

/// What fetching images needs from the page.
pub(crate) struct ImageContext<'a> {
    pub client: &'a Client,
    pub fs_policy: &'a FsPolicy,
    pub budgets: &'a Budgets,
    pub clock: &'a dyn Clock,
    pub extra_headers: &'a [(String, String)],
    /// The document: initiator of every request and the first party.
    pub document_url: &'a Url,
    /// A fetch that cannot finish before this is not started.
    pub deadline: Instant,
}

/// What one image task returns.
struct Finished {
    result: Result<Decoded, String>,
    /// Bytes the body cost.
    charged: u64,
    /// A body to keep, with its kind.
    retained: Option<(Bytes, ImageKind)>,
}

/// What the loop remembers about a running task until it ends.
struct Running {
    index: usize,
    host: Option<String>,
    reserved: u64,
}

/// The outcome of one wanted image.
pub(crate) struct Loaded {
    pub want: ImageWant,
    pub result: Result<Decoded, String>,
}

/// Raises the cancel flag of every decode of a load when the load ends.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// The origin an HTTP image is counted against; `None` for `data:` and `file:`,
/// which make no request.
fn host_of(url: &Url) -> Option<String> {
    matches!(url.scheme(), "http" | "https").then(|| {
        format!(
            "{}:{}",
            url.host_str().unwrap_or(""),
            url.port_or_known_default().unwrap_or(0)
        )
    })
}

/// What admitting an image decided.
enum Admission {
    /// Start it, with this body limit (`reserved` of it held in the ledger).
    Go {
        byte_limit: u64,
        reserved: u64,
    },
    /// The budget is held by requests in flight; try again when one ends.
    Defer,
    Fail(String),
}

/// Fetches and decodes every image of `plan`: at most [`MAX_PER_HOST`] at once
/// from one host and [`MAX_TOTAL`] in all. Returns the outcomes in the plan's
/// order. Dropping the future aborts the fetches and cancels the decodes.
pub(crate) async fn load_images(
    plan: ImagePlan,
    ctx: &ImageContext<'_>,
    ledger: &mut ImageLedger,
) -> Vec<Loaded> {
    let cancel = CancelOnDrop(Arc::new(AtomicBool::new(false)));
    let ImagePlan { wants, decode } = plan;
    let mut slots: Vec<Option<Result<Decoded, String>>> = (0..wants.len()).map(|_| None).collect();
    let mut running: JoinSet<Finished> = JoinSet::new();
    let mut tasks: HashMap<tokio::task::Id, Running> = HashMap::new();
    let mut per_host: HashMap<String, usize> = HashMap::new();
    let mut pending: VecDeque<usize> = (0..wants.len()).collect();
    while !pending.is_empty() || !running.is_empty() {
        let mut at = 0;
        while at < pending.len() && running.len() < MAX_TOTAL {
            let index = pending[at];
            let want = &wants[index];
            let host = host_of(&want.url);
            if host
                .as_ref()
                .is_some_and(|h| per_host.get(h).copied().unwrap_or(0) >= MAX_PER_HOST)
            {
                at += 1;
                continue;
            }
            // A decode at a new size reads the body kept from the first fetch
            // and costs no request.
            let kept = want.rerender.map(|_| ledger.bodies.get(&want.key).cloned());
            let admission = match &kept {
                Some(Some(_)) => Admission::Go {
                    byte_limit: 0,
                    reserved: 0,
                },
                Some(None) => Admission::Fail("the body was not kept to decode again".to_owned()),
                None => admit(ctx, ledger),
            };
            let (byte_limit, reserved) = match admission {
                Admission::Fail(reason) => {
                    slots[index] = Some(Err(reason));
                    pending.remove(at);
                    continue;
                }
                Admission::Defer => {
                    at += 1;
                    continue;
                }
                Admission::Go {
                    byte_limit,
                    reserved,
                } => (byte_limit, reserved),
            };
            pending.remove(at);
            ledger.reserved += reserved;
            if kept.is_none() {
                ledger.requests += 1;
            }
            let mut settings = decode;
            match want.rerender {
                Some(Rerender::Svg { scale }) => settings.device_scale = scale,
                Some(Rerender::Raster { max_width }) => settings.max_size.0 = max_width.max(1),
                None => {}
            }
            let job = Job {
                want: want.clone(),
                kept: kept.flatten(),
                client: ctx.client.clone(),
                fs_policy: ctx.fs_policy.clone(),
                extra_headers: ctx.extra_headers.to_vec(),
                document_url: ctx.document_url.clone(),
                remaining: ctx.deadline.saturating_duration_since(ctx.clock.now()),
                byte_limit,
                decode: settings,
                cancel: Arc::clone(&cancel.0),
            };
            if let Some(h) = &host {
                *per_host.entry(h.clone()).or_default() += 1;
            }
            let handle = running.spawn_local(job.run());
            tasks.insert(
                handle.id(),
                Running {
                    index,
                    host,
                    reserved,
                },
            );
        }
        if running.is_empty() {
            // Nothing is running that could free the budget the rest wait on.
            for index in pending.drain(..) {
                slots[index] = Some(Err("the page's byte budget is spent".to_owned()));
            }
            break;
        }
        let Some(joined) = running.join_next_with_id().await else {
            continue;
        };
        let (id, finished) = match joined {
            Ok((id, finished)) => (id, Some(finished)),
            // A task that panicked loses only its own image.
            Err(error) => {
                tracing::warn!(%error, "an image task ended abnormally");
                (error.id(), None)
            }
        };
        let Some(task) = tasks.remove(&id) else {
            continue;
        };
        ledger.reserved = ledger.reserved.saturating_sub(task.reserved);
        if let Some(h) = &task.host
            && let Some(count) = per_host.get_mut(h)
        {
            *count = count.saturating_sub(1);
        }
        if let Some(finished) = finished {
            ledger.bytes += finished.charged;
            if let Some((body, kind)) = finished.retained {
                let cap = if kind == ImageKind::Svg {
                    MAX_RETAINED_SVG
                } else {
                    MAX_RETAINED_RASTER
                };
                if body.len() <= cap && ledger.retained + body.len() <= MAX_RETAINED_TOTAL {
                    ledger.retained += body.len();
                    ledger
                        .bodies
                        .insert(wants[task.index].key.clone(), (body, kind));
                }
            }
            slots[task.index] = Some(finished.result);
        }
    }
    wants
        .into_iter()
        .zip(slots)
        .map(|(want, result)| Loaded {
            want,
            result: result.unwrap_or_else(|| Err("the image task ended abnormally".to_owned())),
        })
        .collect()
}

/// Charges one request, reserving its body limit against the byte budget.
fn admit(ctx: &ImageContext<'_>, ledger: &ImageLedger) -> Admission {
    if ledger.requests >= ctx.budgets.network_requests {
        return Admission::Fail("the page's request budget is spent".to_owned());
    }
    if ctx.deadline <= ctx.clock.now() {
        return Admission::Fail("the image wait ended before this image was requested".to_owned());
    }
    let committed = ledger.bytes.saturating_add(ledger.reserved);
    let left = ctx.budgets.network_bytes.saturating_sub(committed);
    if left == 0 {
        return if ledger.reserved > 0 {
            Admission::Defer
        } else {
            Admission::Fail("the page's byte budget is spent".to_owned())
        };
    }
    let limit = left.min(MAX_IMAGE_BYTES);
    Admission::Go {
        byte_limit: limit,
        reserved: limit,
    }
}

struct Job {
    want: ImageWant,
    /// The retained body to decode again, instead of a fetch.
    kept: Option<(Bytes, ImageKind)>,
    client: Client,
    fs_policy: FsPolicy,
    extra_headers: Vec<(String, String)>,
    document_url: Url,
    remaining: Duration,
    byte_limit: u64,
    decode: DecodeSettings,
    cancel: Arc<AtomicBool>,
}

impl Job {
    /// Fetches (or reads the kept body) and decodes.
    async fn run(self) -> Finished {
        let (body, kind, charged) = match self.kept.clone() {
            Some((body, kind)) => (body, kind, 0),
            None => {
                let (body, content_type) = match self.fetch().await {
                    Ok(fetched) => fetched,
                    Err(reason) => {
                        return Finished {
                            result: Err(reason),
                            charged: 0,
                            retained: None,
                        };
                    }
                };
                let kind = match (self.want.url.scheme(), content_type.as_deref()) {
                    ("file", None)
                        if self.want.url.path().to_ascii_lowercase().ends_with(".svg") =>
                    {
                        ImageKind::Svg
                    }
                    (_, declared) => ImageKind::of(declared),
                };
                let charged = body.len() as u64;
                (body, kind, charged)
            }
        };
        let control = DecodeControl::new()
            .with_timeout(DECODE_TIMEOUT.min(self.remaining.max(Duration::from_secs(1))))
            .with_cancel(Arc::clone(&self.cancel));
        let settings = self.decode;
        let source = body.clone();
        let decoded =
            tokio::task::spawn_blocking(move || decode_image(&source, kind, &settings, &control))
                .await;
        let result = match decoded {
            Ok(Ok(decoded)) => Ok(decoded),
            Ok(Err(error)) => Err(describe(&error)),
            Err(_) => Err("the decode task ended abnormally".to_owned()),
        };
        let retained = (self.kept.is_none() && result.is_ok()).then_some((body, kind));
        Finished {
            result,
            charged,
            retained,
        }
    }

    /// The body of the image and its declared `Content-Type`.
    async fn fetch(&self) -> Result<(Bytes, Option<String>), String> {
        let url = &self.want.url;
        match url.scheme() {
            "data" => {
                let data = decode_data_url(url.as_str(), self.byte_limit as usize)?;
                Ok((Bytes::from(data.bytes), data.mime))
            }
            "file" => read_local_file(
                &self.document_url,
                &self.fs_policy,
                url,
                self.byte_limit,
                "image",
            )
            .map(|body| (Bytes::from(body), None)),
            "http" | "https" => self.fetch_remote().await,
            other => Err(format!("images are not loaded from {other}: URLs")),
        }
    }

    async fn fetch_remote(&self) -> Result<(Bytes, Option<String>), String> {
        let mut request = Request::new(http::Method::GET, self.want.url.clone())
            .resource_type(ResourceType::Image)
            .initiator(self.document_url.clone())
            .timeout(self.remaining)
            .max_body_bytes(self.byte_limit);
        request.first_party = Site::from_url(&self.document_url);
        for (name, value) in &self.extra_headers {
            request = request
                .header(name, value)
                .map_err(|error| format!("a page header cannot go on the wire: {error:?}"))?;
        }
        // The client enforces the timeout on the wire; the same bound here
        // covers an interceptor and a body that stalls after its head.
        let fetched = tokio::time::timeout(self.remaining, async {
            let response = self
                .client
                .send(request)
                .await
                .map_err(|error| format!("{error:?}"))?;
            self.read(response).await
        })
        .await;
        fetched.unwrap_or_else(|_| Err("the image did not arrive before the wait ended".to_owned()))
    }

    /// Checks a response and reads its body, without copying it.
    async fn read(&self, response: tenta_net::Response) -> Result<(Bytes, Option<String>), String> {
        if !matches!(response.url.scheme(), "http" | "https") {
            return Err(format!(
                "the image was served from a {}: URL",
                response.url.scheme()
            ));
        }
        if !response.status.is_success() {
            return Err(format!("the origin answered {}", response.status.as_u16()));
        }
        let content_type = response.header("content-type").map(str::to_owned);
        let body = response
            .body
            .collect()
            .await
            .map_err(|error| format!("{error:?}"))?;
        if body.len() as u64 > self.byte_limit {
            return Err("the response exceeds the image size limit".to_owned());
        }
        Ok((body, content_type))
    }
}

fn describe(error: &DecodeError) -> String {
    error.to_string()
}

/// Reasons that say the image may load next time: the render ran out of time
/// or was cancelled, or the process had no SVG slot free. They are reported but
/// not remembered as the image's failure, so a later render tries again.
const TRANSIENT_REASONS: [&str; 5] = [
    "svg busy",
    "decode deadline passed",
    "decode was cancelled",
    "before the wait ended",
    "wait ended before",
];

/// Whether a failure is worth retrying on a later render.
pub(crate) fn is_transient(reason: &str) -> bool {
    TRANSIENT_REASONS.iter().any(|t| reason.contains(t))
}

#[cfg(test)]
mod tests;
