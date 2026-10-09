//! The render half of `tentabrowser screenshot`: navigate, capture, write,
//! and say what the rasteriser left out.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::{Duration, Instant};

use serde_json::json;
use tentabrowser::{Goto, ImageFormat, Page, PageOptions, Screenshot, ScreenshotOptions, Viewport};
use url::Url;

use super::is_stdout;
use crate::cli::{Global, JsMode, ScreenshotArgs, ScreenshotFormat};
use crate::error::{CliError, CliResult};
use crate::fetch::settle;
use crate::logging;
use crate::output;
use crate::setup::{self, Session};

const DEFAULT_JPEG_QUALITY: u8 = 90;

pub async fn run(global: &Global, args: &ScreenshotArgs) -> CliResult<()> {
    let session = setup::open_render(global)?;
    let outcome = shoot(&session, global, args).await;
    session.close();
    outcome
}

async fn shoot(session: &Session, global: &Global, args: &ScreenshotArgs) -> CliResult<()> {
    if !is_stdout(&args.output) {
        session.engine.fs_policy().check_write(&args.output)?;
    }
    let started = Instant::now();
    let page = session.context.new_page(page_options(args)).await?;
    let outcome = navigate_and_capture(&page, global, args).await;
    let _ = page.close().await;
    let (shot, final_url) = outcome?;

    if is_stdout(&args.output) {
        output::write_stdout_exact(&shot.bytes)?;
    } else {
        output::write_file_exact(session.engine.fs_policy(), &args.output, &shot.bytes)?;
    }
    report(global, args, &shot, &final_url, started.elapsed());
    Ok(())
}

/// Scripts run before the capture, as `fetch --eval` runs them. `--js` decides;
/// without it they run only in a build with V8.
fn page_options(args: &ScreenshotArgs) -> PageOptions {
    PageOptions {
        javascript: runs_scripts(args),
        viewport: Viewport {
            width: args.width,
            height: args.height,
            device_scale_factor: args.dpr,
            mobile: false,
        },
        ..PageOptions::default()
    }
}

fn runs_scripts(args: &ScreenshotArgs) -> bool {
    match args.js {
        Some(JsMode::On) => true,
        Some(JsMode::Off) => false,
        None => cfg!(feature = "v8"),
    }
}

async fn navigate_and_capture(
    page: &Page,
    global: &Global,
    args: &ScreenshotArgs,
) -> CliResult<(Screenshot, Url)> {
    let goto = Goto::default()
        .wait_until(settle(args.wait))
        .timeout(Duration::from_millis(global.timeout));
    let navigation = page.goto(&args.url, goto).await?;
    if navigation.status >= 400 && !args.allow_http_errors {
        return Err(CliError::http(
            navigation.status,
            &logging::url(&navigation.final_url),
        ));
    }
    let shot = page.screenshot(options(args)).await?;
    Ok((shot, navigation.final_url))
}

fn options(args: &ScreenshotArgs) -> ScreenshotOptions {
    let area = if args.full_page {
        ScreenshotOptions::full_page()
    } else if let Some(selector) = &args.selector {
        ScreenshotOptions::element(selector.as_str())
    } else if let Some([x, y, width, height]) = args.clip {
        ScreenshotOptions::clip(x, y, width, height)
    } else {
        ScreenshotOptions::viewport()
    };
    ScreenshotOptions {
        format: image_format(args),
        ..area
    }
}

fn image_format(args: &ScreenshotArgs) -> ImageFormat {
    match args.format {
        ScreenshotFormat::Png => ImageFormat::Png,
        ScreenshotFormat::Jpeg => ImageFormat::Jpeg {
            quality: args.quality.unwrap_or(DEFAULT_JPEG_QUALITY),
        },
    }
}

/// One stderr line with the outcome, then one line per thing that is missing
/// from the image. `--json` replaces the text with one JSON object.
fn report(
    global: &Global,
    args: &ScreenshotArgs,
    shot: &Screenshot,
    final_url: &Url,
    elapsed: Duration,
) {
    let millis = elapsed.as_secs_f64() * 1000.0;
    let mut stderr = std::io::stderr().lock();
    if global.json {
        let line = json!({
            "url": logging::url(final_url),
            "format": format_name(args.format),
            "width": shot.width,
            "height": shot.height,
            "bytes": shot.bytes.len(),
            "ms": millis,
            "clamped": shot.clamped,
            "skipped": shot.stats.skipped,
            "approximated": shot.stats.approximated,
        });
        let _ = writeln!(stderr, "{line}");
        return;
    }

    let destination = if is_stdout(&args.output) {
        String::from("stdout")
    } else {
        args.output.display().to_string()
    };
    let _ = writeln!(
        stderr,
        "tentabrowser: wrote {destination} ({}x{} {}, {} bytes, {millis:.0} ms)",
        shot.width,
        shot.height,
        format_name(args.format),
        shot.bytes.len(),
    );
    if shot.clamped {
        let _ = writeln!(
            stderr,
            "tentabrowser: warning: the page is taller than the image; it was cut at the render size limits"
        );
    }
    if !shot.stats.is_complete() {
        let _ = writeln!(
            stderr,
            "tentabrowser: warning: not drawn: {}",
            kinds(&shot.stats.skipped)
        );
    }
    if !shot.stats.approximated.is_empty() {
        let _ = writeln!(
            stderr,
            "tentabrowser: note: drawn in a simpler form: {}",
            kinds(&shot.stats.approximated)
        );
    }
}

fn format_name(format: ScreenshotFormat) -> &'static str {
    match format {
        ScreenshotFormat::Png => "png",
        ScreenshotFormat::Jpeg => "jpeg",
    }
}

fn kinds(counts: &BTreeMap<&'static str, usize>) -> String {
    counts
        .iter()
        .map(|(kind, count)| format!("{kind} x{count}"))
        .collect::<Vec<_>>()
        .join(", ")
}
