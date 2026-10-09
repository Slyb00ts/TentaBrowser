//! `browser_screenshot`.

use serde::Deserialize;
use serde_json::json;

use super::{Tool, ToolOutput, handler};
use crate::error::ToolError;
use crate::session::BrowserSession;

pub fn tools() -> Vec<Tool> {
    vec![Tool::new(
        "browser_screenshot",
        "Capture a screenshot",
        "Renders the page to an image and returns it as image content: the viewport by default, \
         the whole scrollable page with `full_page`, or one element with `ref`. Use it to check \
         what the outline cannot say (layout, colour, images, an overlay in the way); read the \
         page with browser_snapshot first, as it is cheaper and exact. The image is PNG, or JPEG \
         with `type` jpeg. When the encoded image exceeds `max_bytes` (default 1500000) it is \
         re-encoded as JPEG and then scaled down until it fits, and the result says so; if even \
         the smallest does not fit the call fails with `budget_exceeded` and advises capturing an \
         element. A full-page capture taller than the engine's limit is cut and flagged \
         `clamped`. A page that could not draw everything (an unsupported image or glyph) is \
         flagged `incomplete`. Needs the render tier, else `tier_denied`. Screenshots do not \
         wait for scripts: settle the page or use browser_wait_for first.",
        json!({
            "type": "object",
            "properties": {
                "type": {"type": "string", "enum": ["png", "jpeg"], "description": "Image format. Default png."},
                "quality": {"type": "integer", "minimum": 1, "maximum": 100, "description": "JPEG quality. Default 80."},
                "full_page": {"type": "boolean", "description": "Capture the whole scrollable page."},
                "ref": {"type": "string", "description": "Capture only this element (ref from browser_snapshot)."},
                "max_bytes": {"type": "integer", "minimum": 20000, "maximum": 20000000, "description": "Largest encoded image to return. Default 1500000."}
            },
            "additionalProperties": false,
            "examples": [{}, {"full_page": true, "type": "jpeg", "quality": 70}, {"ref": "e7"}]
        }),
        screenshot_handler,
    )
    .read_only()]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(feature = "render"), allow(dead_code))]
struct ScreenshotArgs {
    #[serde(rename = "type")]
    format: Option<String>,
    quality: Option<u8>,
    #[serde(default)]
    full_page: bool,
    #[serde(rename = "ref")]
    reference: Option<String>,
    max_bytes: Option<usize>,
}

handler!(screenshot_handler, ScreenshotArgs, screenshot);

#[cfg(not(feature = "render"))]
async fn screenshot(
    _session: &mut BrowserSession,
    _args: ScreenshotArgs,
) -> Result<ToolOutput, ToolError> {
    Err(ToolError::tier_denied(
        "this build has no renderer; rebuild tentabrowser with --features render",
    ))
}

#[cfg(feature = "render")]
async fn screenshot(
    session: &mut BrowserSession,
    args: ScreenshotArgs,
) -> Result<ToolOutput, ToolError> {
    use base64::Engine as _;
    use tenta_browser::{Capture, Clip, ImageFormat, ScreenshotOptions};

    use super::dom;
    use crate::error::ErrorKind;

    if args.full_page && args.reference.is_some() {
        return Err(ToolError::invalid(
            "`full_page` and `ref` cannot be combined",
        ));
    }
    let page = session.page().await?;
    super::snapshot::after_navigation(session, &page).await;
    let capture = match &args.reference {
        Some(reference) => {
            let target = dom::resolve_ref(session, &page, reference).await?;
            let boxes = page.node_boxes(target.node).await?;
            let (x, y, width, height) = boxes
                .first()
                .map(|quads| quads.border.bounds())
                .filter(|(_, _, width, height)| *width > 0.0 && *height > 0.0)
                .ok_or_else(|| {
                    ToolError::element_not_found(format!(
                        "`{reference}` ({}) has no layout box to capture",
                        target.description
                    ))
                })?;
            Capture::Clip(Clip {
                x: x as f32,
                y: y as f32,
                width: width as f32,
                height: height as f32,
            })
        }
        None if args.full_page => Capture::FullPage,
        None => Capture::Viewport,
    };
    let max_bytes = args.max_bytes.unwrap_or(1_500_000);
    let quality = args.quality.unwrap_or(80);
    let jpeg_first = args.format.as_deref() == Some("jpeg");
    // Format and scale to try, in order, until the image fits.
    let mut attempts: Vec<(ImageFormat, f32)> = vec![if jpeg_first {
        (ImageFormat::Jpeg { quality }, 1.0)
    } else {
        (ImageFormat::Png, 1.0)
    }];
    if !jpeg_first {
        attempts.push((
            ImageFormat::Jpeg {
                quality: quality.min(75),
            },
            1.0,
        ));
    }
    for scale in [0.75, 0.5, 0.35, 0.25] {
        attempts.push((
            ImageFormat::Jpeg {
                quality: quality.min(70),
            },
            scale,
        ));
    }

    let mut smallest = usize::MAX;
    for (index, (format, scale)) in attempts.iter().enumerate() {
        let options = ScreenshotOptions {
            format: *format,
            capture: capture.clone(),
            scale: (*scale != 1.0).then_some(*scale),
            omit_background: false,
        };
        let shot = page.screenshot(options).await?;
        smallest = smallest.min(shot.bytes.len());
        if shot.bytes.len() > max_bytes {
            continue;
        }
        let (mime, name) = match shot.format {
            ImageFormat::Png => ("image/png", "png"),
            _ => ("image/jpeg", "jpeg"),
        };
        let downscaled = index > 0;
        let incomplete = !shot.stats.is_complete();
        let mut summary = format!(
            "Screenshot {}x{} px, {} bytes ({name}).",
            shot.width,
            shot.height,
            shot.bytes.len()
        );
        if downscaled {
            summary.push_str(&format!(
                " Re-encoded to fit max_bytes {max_bytes}: format {name}, scale {scale}."
            ));
        }
        if shot.clamped {
            summary.push_str(" The page is taller than the capture limit and was cut.");
        }
        if incomplete {
            summary.push_str(" Some content could not be drawn.");
        }
        let structured = json!({
            "width": shot.width,
            "height": shot.height,
            "format": name,
            "mime_type": mime,
            "bytes": shot.bytes.len(),
            "scale": scale,
            "downscaled": downscaled,
            "clamped": shot.clamped,
            "incomplete": incomplete,
        });
        let data = base64::engine::general_purpose::STANDARD.encode(&shot.bytes);
        return Ok(ToolOutput::text(summary, structured).with_image(data, mime));
    }
    Err(ToolError::new(
        ErrorKind::BudgetExceeded,
        format!(
            "the smallest encoding is {smallest} bytes, over max_bytes {max_bytes}; capture an element with `ref`, or raise max_bytes"
        ),
    ))
}
