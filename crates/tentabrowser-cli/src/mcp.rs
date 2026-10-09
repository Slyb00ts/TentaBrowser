//! `tentabrowser mcp`: the Model Context Protocol server for agents
//! (`docs/spec/10-mcp.md`).
//!
//! Over stdio the process is one agent's browser and ends with its input.
//! With `--http` it serves streamable HTTP until interrupted. Pages reach the
//! render tier and run scripts when the build has the `render` and `v8`
//! features; otherwise the tools that need them answer `tier_denied`.

use tenta_mcp::{HttpOptions, HttpServer, McpOptions, McpServer, stdio};
use tentabrowser::{ContextOptions, Engine, JsEngine, Tier, Viewport};

use crate::cli::{Global, McpArgs};
use crate::error::{CliError, CliResult};
use crate::setup;

pub async fn run(global: &Global, args: &McpArgs) -> CliResult<()> {
    if args.schema_version {
        println!("{}", tenta_mcp::SCHEMA_VERSION);
        return Ok(());
    }
    if args.max_sessions == 0 {
        return Err(CliError::usage("--max-sessions must be at least 1"));
    }
    let engine = build_engine(global, args)?;
    let (width, height) = args.viewport;
    let options = McpOptions {
        context: ContextOptions {
            proxy: global.proxy.clone(),
            allow_private_network: global.allow_private_network,
            cache_dir: global.cache_dir.clone(),
            user_agent: global.user_agent.clone(),
            accept_language: global.accept_language.clone(),
            ..ContextOptions::default()
        },
        share_context: args.share_context,
        viewport: Viewport {
            width,
            height,
            ..Viewport::default()
        },
        max_sessions: args.max_sessions,
        ..McpOptions::default()
    };
    let server = McpServer::new(engine.clone(), options);
    let outcome = if args.http {
        serve_http(&server, args).await
    } else {
        serve_stdio(&server).await
    };
    server.close_all().await;
    engine.close();
    outcome
}

async fn serve_stdio(server: &McpServer) -> CliResult<()> {
    tokio::select! {
        served = stdio::serve_stdio(server) => {
            served.map_err(|error| CliError::new(1, format!("mcp: {error}")))
        }
        interrupted = tokio::signal::ctrl_c() => {
            interrupted.map_err(|error| CliError::new(1, format!("waiting for the interrupt: {error}")))
        }
    }
}

async fn serve_http(server: &McpServer, args: &McpArgs) -> CliResult<()> {
    let given = args.token.clone().filter(|token| !token.is_empty());
    // Loopback is reachable by every local process and, through a browser, by
    // web pages, so it gets a token unless the operator opts out.
    let generated = given.is_none() && !args.no_auth && args.host.is_loopback();
    let token = if generated {
        Some(tenta_mcp::generate_token())
    } else {
        given
    };
    let options = HttpOptions {
        bind: (args.host, args.port).into(),
        token: token.clone(),
        allowed_origins: args.allowed_origin.clone(),
        allowed_hosts: args.allowed_host.clone(),
        ..HttpOptions::default()
    };
    let http = HttpServer::bind(server.clone(), options)
        .await
        .map_err(|error| CliError::usage(format!("mcp: {error}")))?;
    eprintln!("MCP listening on {}", http.url());
    if generated && let Some(token) = &token {
        eprintln!("MCP bearer token: {token}");
    }
    let waited = tokio::signal::ctrl_c().await;
    http.shutdown().await;
    waited.map_err(|error| CliError::new(1, format!("waiting for the interrupt: {error}")))
}

fn build_engine(global: &Global, args: &McpArgs) -> CliResult<Engine> {
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
    setup::build_engine_settled(
        global,
        args.max_sessions.min(4),
        tier,
        js,
        Some(tenta_mcp::settle_config()),
    )
}
