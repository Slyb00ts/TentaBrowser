//! `tentabrowser cdp`: the DevTools protocol endpoint for browser automation
//! clients (`docs/spec/09-cdp-compat.md`).
//!
//! The server lives until the process is interrupted. Pages reach the render
//! tier and run scripts when the build has the `render` and `v8` features;
//! otherwise the matching commands answer an error naming the missing feature.

use tenta_cdp::{CdpOptions, CdpServer};
use tentabrowser::{ContextOptions, Engine, JsEngine, Tier};

use crate::cli::{CdpArgs, Global};
use crate::error::{CliError, CliResult};
use crate::setup;

pub async fn run(global: &Global, args: &CdpArgs) -> CliResult<()> {
    let engine = build_engine(global, args)?;
    let options = CdpOptions {
        bind: (args.host, args.port).into(),
        allow_non_loopback: !args.host.is_loopback(),
        token: args.token.clone().filter(|token| !token.is_empty()),
        allowed_origins: args.allowed_origin.clone(),
        max_targets: args.max_targets,
        default_context: ContextOptions {
            proxy: global.proxy.clone(),
            allow_private_network: global.allow_private_network,
            cache_dir: global.cache_dir.clone(),
            user_agent: global.user_agent.clone(),
            accept_language: global.accept_language.clone(),
            ..ContextOptions::default()
        },
        ..CdpOptions::default()
    };
    let server = match CdpServer::bind(engine.clone(), options).await {
        Ok(server) => server,
        Err(error) => {
            engine.close();
            return Err(CliError::usage(format!("cdp: {error}")));
        }
    };
    // The line Chromium prints and launchers wait for.
    eprintln!("DevTools listening on {}", server.browser_ws_url());
    let waited = tokio::signal::ctrl_c().await;
    server.shutdown().await;
    engine.close();
    waited.map_err(|error| CliError::new(1, format!("waiting for the interrupt: {error}")))
}

fn build_engine(global: &Global, args: &CdpArgs) -> CliResult<Engine> {
    let tier = if cfg!(feature = "render") {
        Tier::Render
    } else if cfg!(feature = "v8") {
        Tier::Js
    } else {
        Tier::Dom
    };
    let js = if cfg!(feature = "v8") {
        JsEngine::V8
    } else {
        JsEngine::Auto
    };
    setup::build_engine_with(global, args.max_targets.min(4), tier, js)
}
