//! Fixture replay for the search tests.
//!
//! The recorded SERPs in `providers/fixtures/` are served by
//! `tenta_testutil::FixtureServer` from a temporary directory whose layout
//! mirrors the plans' own endpoint paths (`/html/` for DuckDuckGo, `/search`
//! for Bing and Brave), so the only thing a test changes about a plan is the
//! endpoint's host and port.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tenta_net::Client;
use tenta_net::client::{ClientConfig, NetRuntime};
use tenta_net::dns::Resolver;
use tenta_net::hooks::Hooks;
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::policy::session_cache::SessionCache;
use tenta_search::{PlanSet, Provider, ProviderId, Search, SearchOptions};
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir, temp_dir};
use url::Url;

/// `providers/fixtures/`.
pub fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("providers")
        .join("fixtures")
}

/// A recorded SERP served on the loopback fixture server.
pub struct Harness {
    pub server: FixtureServer,
    pub runtime: Arc<NetRuntime>,
    pub plans: PlanSet,
    temp: TempDir,
    endpoints: HashMap<ProviderId, Url>,
}

impl Harness {
    pub async fn start() -> Harness {
        let temp = temp_dir("search-fixtures");
        let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
            .await
            .expect("fixture server");
        Harness {
            server,
            runtime: NetRuntime::new(1).expect("io runtime"),
            plans: PlanSet::embedded().expect("embedded plans"),
            temp,
            endpoints: HashMap::new(),
        }
    }

    /// Serves `providers/fixtures/<plan>/<slug>.html` at the plan's own
    /// endpoint path and points the plan's endpoint at it.
    pub fn serve_fixture(&mut self, plan_id: &str, slug: &str) -> Url {
        let path = fixtures_root().join(plan_id).join(format!("{slug}.html"));
        let html =
            std::fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
        self.serve_bytes(plan_id, &html)
    }

    /// Serves `html` at the plan's endpoint path.
    pub fn serve_html(&mut self, plan_id: &str, html: &str) -> Url {
        self.serve_bytes(plan_id, html.as_bytes())
    }

    fn serve_bytes(&mut self, plan_id: &str, body: &[u8]) -> Url {
        let plan = self
            .plans
            .get(plan_id)
            .unwrap_or_else(|| panic!("no plan {plan_id}"));
        let endpoint_path = plan.request.endpoint.path().trim_matches('/').to_owned();
        let relative = format!("{plan_id}/{endpoint_path}/index.html");
        self.temp.write(&relative, body);
        let url = Url::parse(&self.server.url(&format!("/{plan_id}/{endpoint_path}")))
            .expect("fixture url");
        self.endpoints
            .insert(ProviderId::new(plan_id).expect("plan id"), url.clone());
        url
    }

    /// Installs a plan from TOML and serves `html` at its endpoint.
    pub fn serve_plan(&mut self, plan_id: &str, plan_toml: &str, html: &str) -> Url {
        let directory = self.plans_dir();
        std::fs::write(directory.join(format!("{plan_id}.toml")), plan_toml).expect("write plan");
        self.plans = PlanSet::embedded()
            .expect("embedded")
            .with_dir(&directory)
            .expect("plan directory");
        self.serve_html(plan_id, html)
    }

    /// Directory the runtime-loaded plans live in.
    pub fn plans_dir(&self) -> PathBuf {
        let directory = self.temp.path().join("plans");
        std::fs::create_dir_all(&directory).expect("plans dir");
        directory
    }

    /// A client that resolves only literals and never negotiates HTTP/2, like
    /// the transport's own tests.
    pub fn client(&self) -> Client {
        self.client_with(Hooks::default())
    }

    pub fn client_with(&self, hooks: Hooks) -> Client {
        let config = ClientConfig {
            hooks,
            resolver: Some(Arc::new(Resolver::literals_only(SessionCache::ephemeral()))),
            destination: DestinationPolicy::permissive(),
            allow_h2: false,
            request_timeout: Some(Duration::from_secs(5)),
            ..ClientConfig::default()
        };
        self.runtime.client(config).expect("client")
    }

    /// A `Search` over the served fixtures. Warming is off: it would reach
    /// the providers' real hosts.
    pub fn search(&self) -> Search {
        self.search_with(SearchOptions::default())
    }

    pub fn search_with(&self, options: SearchOptions) -> Search {
        self.search_from(self.client(), options)
    }

    pub fn search_from(&self, client: Client, mut options: SearchOptions) -> Search {
        // Warming an unrestricted `Search` would reach the providers' real
        // hosts; with an explicit provider list every endpoint is overridden
        // to the fixture server, so the option can be exercised.
        if options.providers_enabled.is_empty() {
            options.warm_on_start = false;
        }
        for (id, url) in &self.endpoints {
            options
                .endpoint_override
                .entry(id.clone())
                .or_insert_with(|| url.clone());
        }
        if options.plans_dir.is_none() && self.temp.path().join("plans").is_dir() {
            options.plans_dir = Some(self.temp.path().join("plans"));
        }
        Search::new(client, options).expect("search")
    }

    pub fn requests_served(&self) -> u64 {
        self.server.requests_served()
    }
}

/// Parses HTML the way the fast path does: Tier 1, no scripting, UTF-8.
pub fn parse(html: &[u8]) -> tenta_dom::Document {
    let mut parser = tenta_html::Parser::new(
        tenta_dom::Document::new(),
        tenta_html::ParserOptions {
            scripting_enabled: false,
            encoding: tenta_html::Encoding::for_label(b"utf-8"),
            ..tenta_html::ParserOptions::default()
        },
    );
    parser.feed(html);
    parser.finish()
}

/// Reads a recorded SERP.
pub fn fixture(plan_id: &str, slug: &str) -> Vec<u8> {
    let path = fixtures_root().join(plan_id).join(format!("{slug}.html"));
    std::fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

/// `Provider::Custom(id)`, for the plans that have no named variant.
pub fn custom(id: &str) -> Provider {
    Provider::Custom(ProviderId::new(id).expect("provider id"))
}
