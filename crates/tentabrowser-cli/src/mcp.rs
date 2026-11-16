//! `tentabrowser mcp`: the Model Context Protocol server for agents
//! (`docs/spec/10-mcp.md`).
//!
//! Over stdio the process is one agent's browser and ends with its input.
//! With `--http` it serves streamable HTTP until interrupted. Pages reach the
//! render tier and run scripts when the build has the `render` and `v8`
//! features; otherwise the tools that need them answer `tier_denied`.

use tenta_mcp::{HttpOptions, HttpServer, McpOptions, McpServer, stdio};
use tentabrowser::{
    ContextOptions, Engine, EngineSearch, JsEngine, SearchContextOptions, SearchError,
    SearchRequest, SearchResponse, SearchSetup, Tier, Viewport,
};

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
    let search_setup = search_setup(global, args)?;
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
        search: match EngineSearcher::start(engine.clone(), search_setup) {
            Ok(searcher) => Some(tenta_mcp::SearchHandle(std::sync::Arc::new(searcher))),
            Err(error) => {
                tracing::warn!(%error, "browser_web_search is unavailable: its worker did not start");
                None
            }
        },
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

type SearchJob = (
    SearchRequest,
    tokio::sync::oneshot::Sender<Result<SearchResponse, SearchError>>,
);

/// `browser_web_search` through the engine's internal search context, which
/// has its own cookie jar and connections: a search shares nothing with the
/// pages an agent opens (`docs/spec/03-network.md` §4).
///
/// A search future is not `Send` (it holds a parser), while a tool call may
/// run on any thread of the server's runtime, so the searches run on one
/// thread of their own and the tool waits for the answer over a channel.
struct EngineSearcher {
    jobs: tokio::sync::mpsc::UnboundedSender<SearchJob>,
}

impl EngineSearcher {
    fn start(engine: Engine, setup: SearchSetup) -> std::io::Result<Self> {
        let (jobs, mut queue) = tokio::sync::mpsc::unbounded_channel::<SearchJob>();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        std::thread::Builder::new()
            .name("tenta-mcp-search".to_owned())
            .spawn(move || {
                let local = tokio::task::LocalSet::new();
                local.block_on(&runtime, async move {
                    // Ends when the server, which owns the sender, is dropped.
                    while let Some((request, reply)) = queue.recv().await {
                        let (engine, setup) = (engine.clone(), setup.clone());
                        tokio::task::spawn_local(async move {
                            // The caller may have been cancelled; nobody waits then.
                            let _ = reply.send(engine.search_with(&setup, request).await);
                        });
                    }
                });
            })?;
        Ok(Self { jobs })
    }
}

impl tenta_mcp::WebSearch for EngineSearcher {
    fn search(&self, request: SearchRequest) -> tenta_mcp::SearchFuture<'_> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        let sent = self.jobs.send((request, reply));
        Box::pin(async move {
            let stopped = || SearchError::NoProvider("the search worker has stopped".to_owned());
            sent.map_err(|_| stopped())?;
            answer.await.map_err(|_| stopped())?
        })
    }
}

/// Every loaded provider plan is allowed: the agent names the provider per
/// call, and the first call's setup is the one the subsystem keeps.
fn search_setup(global: &Global, args: &McpArgs) -> CliResult<SearchSetup> {
    let mut setup = SearchSetup {
        context: SearchContextOptions {
            allow_private_network: global.allow_private_network,
            proxy: global.proxy.clone(),
            user_agent: global.user_agent.clone(),
            accept_language: global.accept_language.clone(),
        },
        request_timeout: Some(std::time::Duration::from_millis(global.timeout)),
        ..SearchSetup::default()
    };
    for spec in &args.search_endpoint {
        let (plan, endpoint) = crate::search::parse_endpoint(spec)?;
        setup = setup.endpoint_override(plan, endpoint);
    }
    Ok(setup)
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
