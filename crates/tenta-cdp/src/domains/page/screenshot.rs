//! `Page.captureScreenshot` and the content size `Page.getLayoutMetrics`
//! reports, both of which need the render pipeline.

#[cfg(feature = "render")]
pub(super) use built::{capture, content_size};
#[cfg(not(feature = "render"))]
pub(super) use missing::{capture, content_size};

#[cfg(feature = "render")]
mod built {
    use base64::Engine;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use tenta_browser::{Capture, Clip, ContentSize, ImageFormat, ScreenshotOptions};

    use crate::backend::Target;
    use crate::domain::parse_params;
    use crate::error::CdpError;

    /// Chromium's default JPEG quality.
    const DEFAULT_JPEG_QUALITY: u8 = 80;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CaptureParams {
        format: Option<String>,
        quality: Option<u32>,
        clip: Option<ClipParams>,
        #[serde(default)]
        capture_beyond_viewport: bool,
    }

    #[derive(Deserialize)]
    struct ClipParams {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        scale: f64,
    }

    fn format_of(params: &CaptureParams) -> Result<ImageFormat, CdpError> {
        match params.format.as_deref().unwrap_or("png") {
            "png" => Ok(ImageFormat::Png),
            "jpeg" => {
                let quality = match params.quality {
                    None | Some(0) => DEFAULT_JPEG_QUALITY,
                    Some(quality @ 1..=100) => quality as u8,
                    Some(_) => {
                        return Err(CdpError::invalid_params("quality is 0 to 100"));
                    }
                };
                Ok(ImageFormat::Jpeg { quality })
            }
            "webp" => Err(CdpError::server("WebP screenshots are not supported")),
            other => Err(CdpError::invalid_params(format!(
                "unknown screenshot format {other:?}"
            ))),
        }
    }

    /// The clip in document coordinates. Without `captureBeyondViewport` a
    /// capture shows what the viewport shows, so the clip is cut to it.
    fn capture_of(
        target: &Target,
        params: &CaptureParams,
    ) -> Result<(Capture, Option<f32>), CdpError> {
        let Some(clip) = &params.clip else {
            return Ok((Capture::Viewport, None));
        };
        if !(clip.scale.is_finite() && clip.scale > 0.0) {
            return Err(CdpError::invalid_params("clip.scale must be positive"));
        }
        let (mut left, mut top) = (clip.x, clip.y);
        let (mut right, mut bottom) = (clip.x + clip.width, clip.y + clip.height);
        if !params.capture_beyond_viewport {
            let viewport = target.page().viewport();
            left = left.max(0.0);
            top = top.max(0.0);
            right = right.min(f64::from(viewport.width));
            bottom = bottom.min(f64::from(viewport.height));
        }
        let (width, height) = (right - left, bottom - top);
        if !(width.is_finite() && width > 0.0) {
            return Err(CdpError::server("Cannot take screenshot with 0 width."));
        }
        if !(height.is_finite() && height > 0.0) {
            return Err(CdpError::server("Cannot take screenshot with 0 height."));
        }
        let capture = Capture::Clip(Clip {
            x: left as f32,
            y: top as f32,
            width: width as f32,
            height: height as f32,
        });
        Ok((capture, Some(clip.scale as f32)))
    }

    pub(in crate::domains::page) async fn capture(
        target: &Target,
        params: Value,
    ) -> Result<Value, CdpError> {
        let params: CaptureParams = parse_params(params)?;
        let format = format_of(&params)?;
        let (capture, scale) = capture_of(target, &params)?;
        let shot = target
            .page()
            .screenshot(ScreenshotOptions {
                format,
                capture,
                scale,
                omit_background: false,
            })
            .await?;
        Ok(json!({ "data": base64::engine::general_purpose::STANDARD.encode(shot.bytes) }))
    }

    pub(in crate::domains::page) async fn content_size(
        target: &Target,
    ) -> Result<(f64, f64), CdpError> {
        let ContentSize { width, height } = target.page().content_size().await?;
        Ok((f64::from(width), f64::from(height)))
    }
}

#[cfg(not(feature = "render"))]
mod missing {
    use serde_json::Value;

    use crate::backend::Target;
    use crate::error::CdpError;

    fn unavailable() -> CdpError {
        CdpError::server("This build has no renderer; enable the render feature")
    }

    pub(in crate::domains::page) async fn capture(
        _target: &Target,
        _params: Value,
    ) -> Result<Value, CdpError> {
        Err(unavailable())
    }

    pub(in crate::domains::page) async fn content_size(
        _target: &Target,
    ) -> Result<(f64, f64), CdpError> {
        Err(unavailable())
    }
}
