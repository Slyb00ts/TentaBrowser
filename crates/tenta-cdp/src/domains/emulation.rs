//! The `Emulation` domain: what automation clients set up before a script
//! runs.
//!
//! `setDeviceMetricsOverride` changes the viewport the page lays out and
//! renders at, device scale factor included, until `clearDeviceMetricsOverride`
//! or until the session that set it goes away. Window metrics a script reads
//! (`innerWidth`) keep the viewport the document's engine was built with.
//! `setTouchEmulationEnabled` and `setEmulatedMedia` are accepted and have no
//! effect yet: there is no touch input, and the style system takes no media
//! overrides. Everything else is refused by name rather than answered `{}`,
//! so a client does not believe a user agent or a locale took hold.

use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};
use tenta_browser::{Page, Viewport};

use crate::backend::{Target, lock};
use crate::domain::{CommandContext, CommandFuture, DomainHandler, DomainRegistry, parse_params};
use crate::error::CdpError;

pub(super) fn register(registry: &mut DomainRegistry) {
    registry.register(Emulation);
}

struct Emulation;

/// Chromium's bound on an overridden dimension.
const MAX_DIMENSION: u32 = 10_000_000;
const MAX_SCALE_FACTOR: f64 = 16.0;

/// What a refused command says is missing.
const REFUSED: &[(&str, &str)] = &[
    (
        "setUserAgentOverride",
        "the user agent is fixed per browser context; set it when the context is created",
    ),
    ("setLocaleOverride", "locale overrides are not built"),
    ("setTimezoneOverride", "timezone overrides are not built"),
    (
        "setScriptExecutionDisabled",
        "scripts cannot be switched off for a live page",
    ),
    ("setGeolocationOverride", "geolocation is not built"),
    ("setCPUThrottlingRate", "CPU throttling is not built"),
    (
        "setDefaultBackgroundColorOverride",
        "background color overrides are not built",
    ),
    (
        "setEmulatedVisionDeficiency",
        "vision deficiencies are not built",
    ),
    ("setFocusEmulationEnabled", "focus emulation is not built"),
    ("setIdleOverride", "idle detection is not built"),
    ("clearIdleOverride", "idle detection is not built"),
    ("addScreen", "multi-screen emulation is not built"),
    ("removeScreen", "multi-screen emulation is not built"),
    ("getScreenInfos", "multi-screen emulation is not built"),
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceMetrics {
    width: u32,
    height: u32,
    device_scale_factor: f64,
    mobile: bool,
}

/// The override one session holds on its page; ends with the session.
#[derive(Default)]
struct Override {
    held: Mutex<Option<(Arc<Target>, usize)>>,
}

impl Override {
    fn restore(&self) {
        let Some((target, owner)) = lock(&self.held).take() else {
            return;
        };
        let next = target.viewport_overrides().clear(owner);
        if let Some(viewport) = next {
            set_viewport(target.page(), viewport);
        }
    }
}

impl Drop for Override {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(feature = "render")]
fn set_viewport(page: &Page, viewport: Viewport) {
    page.set_viewport(viewport);
}

#[cfg(not(feature = "render"))]
fn set_viewport(_page: &Page, _viewport: Viewport) {}

fn apply(cx: &CommandContext, metrics: DeviceMetrics) -> Result<(), CdpError> {
    if !cfg!(feature = "render") {
        return Err(CdpError::server(
            "This build has no renderer; enable the render feature",
        ));
    }
    let DeviceMetrics {
        width,
        height,
        device_scale_factor,
        mobile,
    } = metrics;
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(CdpError::invalid_params(
            "Width and height values must be positive, not greater than 10000000",
        ));
    }
    if !(device_scale_factor.is_finite() && (0.0..=MAX_SCALE_FACTOR).contains(&device_scale_factor))
    {
        return Err(CdpError::invalid_params(
            "deviceScaleFactor must be between 0 and 16",
        ));
    }
    let target = cx.target()?;
    let owner = std::ptr::from_ref::<crate::session::Session>(cx.session()) as usize;
    let page = target.page();
    let mut overrides = target.viewport_overrides();
    // A zero dimension or scale factor leaves that part as the page had it
    // before any override.
    let original = overrides.pristine_or(page.viewport());
    let viewport = Viewport {
        width: if width == 0 { original.width } else { width },
        height: if height == 0 { original.height } else { height },
        device_scale_factor: if device_scale_factor == 0.0 {
            original.device_scale_factor
        } else {
            device_scale_factor as f32
        },
        mobile,
    };
    overrides.set(owner, page.viewport(), viewport);
    drop(overrides);
    set_viewport(page, viewport);
    *lock(&cx.session().state::<Override>().held) = Some((Arc::clone(target), owner));
    Ok(())
}

impl DomainHandler for Emulation {
    fn domain(&self) -> &'static str {
        "Emulation"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "setDeviceMetricsOverride",
            "clearDeviceMetricsOverride",
            "setTouchEmulationEnabled",
            "setEmulatedMedia",
            "setUserAgentOverride",
            "setLocaleOverride",
            "setTimezoneOverride",
            "setScriptExecutionDisabled",
            "setGeolocationOverride",
            "setCPUThrottlingRate",
            "setDefaultBackgroundColorOverride",
            "setEmulatedVisionDeficiency",
            "setFocusEmulationEnabled",
            "setIdleOverride",
            "clearIdleOverride",
            "addScreen",
            "removeScreen",
            "getScreenInfos",
        ]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                "setDeviceMetricsOverride" => {
                    apply(&cx, parse_params(params)?)?;
                    Ok(json!({}))
                }
                "clearDeviceMetricsOverride" => {
                    cx.target()?;
                    cx.session().state::<Override>().restore();
                    Ok(json!({}))
                }
                "setTouchEmulationEnabled" | "setEmulatedMedia" => {
                    cx.target()?;
                    Ok(json!({}))
                }
                other => match REFUSED.iter().find(|(name, _)| *name == other) {
                    Some((name, reason)) => Err(CdpError::server(format!(
                        "Emulation.{name} is not supported: {reason}"
                    ))),
                    None => Err(CdpError::method_not_found(&format!("Emulation.{other}"))),
                },
            }
        })
    }
}
