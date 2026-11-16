//! The command line surface (`docs/spec/08-search.md` §5,
//! `docs/spec/19-security.md` §1, `docs/spec/20-platform-behaviour.md` §11).

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

/// The default fingerprint profile: Chrome 144 for the host operating system
/// (`docs/spec/21-project-operations.md` §2, O-3).
pub fn default_profile() -> &'static str {
    if cfg!(target_os = "windows") {
        "chrome_144_windows"
    } else if cfg!(any(target_os = "macos", target_os = "ios")) {
        "chrome_144_macos"
    } else {
        "chrome_144_linux"
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "tentabrowser",
    version,
    about = "TentaBrowser: fetch, search and scrape the web without a browser",
    long_about = None,
    disable_help_subcommand = true,
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,

    #[command(subcommand)]
    pub command: Command,
}

/// Flags that configure the engine and its context, whatever the subcommand.
#[derive(Debug, Args)]
#[command(next_help_heading = "Global options")]
pub struct Global {
    /// Fingerprint profile name or path to a profile TOML; `none` disables
    /// fingerprinting.
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,

    /// Overrides the profile's User-Agent.
    #[arg(long, global = true, value_name = "UA")]
    pub user_agent: Option<String>,

    /// Overrides the profile's Accept-Language.
    #[arg(long, global = true, value_name = "TAGS")]
    pub accept_language: Option<String>,

    /// Proxy URL: http://, https:// or socks5://, credentials included.
    #[arg(long, global = true, value_name = "URL")]
    pub proxy: Option<String>,

    /// Persist nothing: no session cache, no cookies between runs.
    #[arg(long, global = true)]
    pub ephemeral: bool,

    /// Allow loopback, RFC 1918, link-local and .local destinations.
    #[arg(long, global = true)]
    pub allow_private_network: bool,

    /// Directories the process may read (repeatable, comma separated).
    #[arg(long, global = true, value_name = "DIRS", value_delimiter = ',')]
    pub allow_read: Vec<PathBuf>,

    /// Directories the process may write (repeatable, comma separated).
    #[arg(long, global = true, value_name = "DIRS", value_delimiter = ',')]
    pub allow_write: Vec<PathBuf>,

    /// Directory downloads are accepted into; without it downloads are denied.
    /// Pre-existing directories are chmod-ed to 0700 for consistency (N-R2-2).
    #[arg(long, global = true, value_name = "DIR")]
    pub downloads: Option<PathBuf>,

    /// On-disk HTTP cache directory for this run.
    /// Pre-existing directories are chmod-ed to 0700 to protect cached credentials (N-R2-2).
    #[arg(long, global = true, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Per-navigation deadline in milliseconds. Also sets a ceiling for
    /// `Retry-After` delays (max of this timeout and 60 s); larger delays
    /// are treated as if the host is overwhelmed, and its remaining queued
    /// URLs are dropped with an error.
    #[arg(long, global = true, value_name = "MS", default_value_t = 30_000)]
    pub timeout: u64,

    /// Network io threads.
    #[arg(long, global = true, value_name = "N")]
    pub io_threads: Option<usize>,

    /// Page threads; the default is what the command needs.
    #[arg(long, global = true, value_name = "N")]
    pub page_threads: Option<usize>,

    /// Log level: error, warn, info, debug or trace. Logs go to stderr.
    #[arg(long, global = true, value_name = "LEVEL", default_value = "warn")]
    pub log: String,

    /// Do not redact URLs, cookies and credentials in logs (developer flag).
    #[arg(long, global = true)]
    pub log_unredacted: bool,

    /// Machine-readable output: `fetch --dump json`, `search` as JSON lines,
    /// and `screenshot`'s summary on stderr as one JSON object. `scrape`
    /// already prints JSON lines and ignores it.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Fetch one URL and print the extraction you asked for.
    Fetch(FetchArgs),
    /// Query search providers through their public pages.
    Search(SearchArgs),
    /// Fetch many URLs politely and print one JSON object per page.
    Scrape(ScrapeArgs),
    /// Capture a page as a PNG or JPEG image. Needs a build with `--features render`.
    Screenshot(ScreenshotArgs),
    /// Serve the Chrome DevTools Protocol, for Puppeteer and Playwright
    /// (`puppeteer.connect({ browserWSEndpoint })`). Loopback only unless a
    /// token is set. On loopback there is no authentication by default: any
    /// local process, and any web page in a local browser that is allowed by
    /// `--allowed-origin`, can drive the browser; set a token on shared
    /// machines.
    Cdp(CdpArgs),
    /// Serve the Model Context Protocol, so agents can drive the browser with
    /// tools (`browser_navigate`, `browser_snapshot`, `browser_click`, ...).
    /// Speaks newline-delimited JSON-RPC on stdin and stdout by default;
    /// `--http` serves streamable HTTP on loopback instead. Logs go to
    /// stderr. Page scripts and screenshots need a build with `--features
    /// v8,render`.
    Mcp(McpArgs),
    /// Print the version.
    Version,
}

#[derive(Debug, Args)]
pub struct CdpArgs {
    /// Address to listen on.
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1")]
    pub host: std::net::IpAddr,

    /// Port to listen on; 0 picks a free one. The endpoint is printed on stderr.
    #[arg(long, value_name = "PORT", default_value_t = 9222)]
    pub port: u16,

    /// Require this token as the `token` query parameter of every request.
    /// Needed (16 bytes or more) to listen on a non-loopback address. Prefer
    /// the environment variable: a flag is visible in the process list.
    #[arg(
        long,
        value_name = "TOKEN",
        env = "TENTA_CDP_TOKEN",
        hide_env_values = true
    )]
    pub token: Option<String>,

    /// Accept WebSocket and HTTP requests that carry this `Origin`, matched
    /// exactly (repeatable). Without any, only clients that send no `Origin`
    /// (not web pages) are served.
    #[arg(long, value_name = "ORIGIN")]
    pub allowed_origin: Vec<String>,

    /// Pages that may be open at once.
    #[arg(long, value_name = "N", default_value_t = 16)]
    pub max_targets: usize,
}

#[derive(Debug, Args)]
pub struct McpArgs {
    /// Serve streamable HTTP instead of stdio. Without `--token` on loopback a
    /// random bearer token is generated and printed on stderr after the URL.
    /// Session-less (stateless, protocol 2026-07-28) clients of one server all
    /// share ONE browser session: cookies, page and refs. Use stdio, or one
    /// server per agent, when agents must not see each other.
    #[arg(long)]
    pub http: bool,

    /// Serve HTTP on loopback with no bearer token. Any local process, and any
    /// web page allowed by `--allowed-origin`, can then drive the browser.
    #[arg(long, requires = "http", conflicts_with = "token")]
    pub no_auth: bool,

    /// Address to listen on (with `--http`). A non-loopback address needs a
    /// token.
    #[arg(
        long,
        value_name = "ADDR",
        default_value = "127.0.0.1",
        requires = "http"
    )]
    pub host: std::net::IpAddr,

    /// Port to listen on (with `--http`); 0 picks a free one. The endpoint is
    /// printed on stderr.
    #[arg(long, value_name = "PORT", default_value_t = 8931, requires = "http")]
    pub port: u16,

    /// Require this bearer token (`Authorization: Bearer ...`) on every HTTP
    /// request. Needed (16 bytes or more) to listen on a non-loopback
    /// address. Prefer the environment variable: a flag is visible in the
    /// process list.
    #[arg(
        long,
        value_name = "TOKEN",
        env = "TENTA_MCP_TOKEN",
        hide_env_values = true,
        requires = "http"
    )]
    pub token: Option<String>,

    /// Accept HTTP requests that carry this `Origin`, matched exactly
    /// (repeatable). Without any, only clients that send no `Origin` (not web
    /// pages) are served.
    #[arg(long, value_name = "ORIGIN", requires = "http")]
    pub allowed_origin: Vec<String>,

    /// Accept this `Host` name besides IP addresses and `localhost`
    /// (repeatable), for a server behind a named proxy.
    #[arg(long, value_name = "HOST", requires = "http")]
    pub allowed_host: Vec<String>,

    /// All sessions share one browser context (cookies and storage) instead
    /// of one each.
    #[arg(long)]
    pub share_context: bool,

    /// Page viewport as WIDTHxHEIGHT in CSS pixels.
    #[arg(long, value_name = "WxH", default_value = "1280x800", value_parser = parse_viewport)]
    pub viewport: (u32, u32),

    /// Sessions that may be open at once.
    #[arg(long, value_name = "N", default_value_t = 16)]
    pub max_sessions: usize,

    /// Point a search provider's plan at another endpoint for
    /// `browser_web_search`: `<plan_id>=<url>`. Fixture replay; repeatable.
    #[arg(long = "search-endpoint", value_name = "PLAN=URL", hide = true)]
    pub search_endpoint: Vec<String>,

    /// Print the version of the tool list and its schemas, then exit.
    #[arg(long)]
    pub schema_version: bool,
}

fn parse_viewport(text: &str) -> Result<(u32, u32), String> {
    let (width, height) = text
        .split_once(['x', 'X'])
        .ok_or_else(|| "expected WIDTHxHEIGHT, such as 1280x800".to_owned())?;
    let parse = |part: &str| {
        part.parse::<u32>()
            .ok()
            .filter(|value| (1..=16384).contains(value))
            .ok_or_else(|| "each side must be a number from 1 to 16384".to_owned())
    };
    Ok((parse(width)?, parse(height)?))
}

#[derive(Debug, Args)]
pub struct FetchArgs {
    /// The URL to fetch.
    pub url: String,

    /// What to print. Defaults to text; the global `--json` wins over this flag.
    /// Cannot be combined with `--eval`.
    #[arg(long, value_enum)]
    pub dump: Option<Dump>,

    /// Evaluate a JavaScript expression after the requested wait and print its
    /// completion text. Requires a build with `--features v8`.
    #[arg(short = 'e', long, value_name = "EXPRESSION")]
    pub eval: Option<String>,

    /// Restrict `text` and `markdown` to the page's main content.
    #[arg(long)]
    pub main_content: bool,

    /// How long to wait before extracting.
    #[arg(long, value_enum, default_value_t = Wait::Load)]
    pub wait: Wait,

    /// Print the status line and response headers on stderr.
    #[arg(long)]
    pub headers: bool,

    /// Write the dump to this file instead of stdout; needs --allow-write.
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Exit 0 even when the server answered 4xx or 5xx.
    #[arg(long)]
    pub allow_http_errors: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Dump {
    Html,
    Text,
    Markdown,
    Links,
    Json,
    Metadata,
    Forms,
    Snapshot,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Wait {
    /// The `load` event.
    Load,
    /// `DOMContentLoaded`.
    #[value(name = "domcontent")]
    DomContent,
    /// Nothing left on the network.
    #[value(name = "networkidle")]
    NetworkIdle,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// The query.
    pub query: String,

    /// Providers to query: ddg, ddg-lite, bing, brave, google.
    #[arg(long, value_name = "LIST", value_delimiter = ',', default_values_t = [String::from("ddg")])]
    pub provider: Vec<String>,

    /// Results after merging, 1..=50.
    #[arg(long, default_value_t = 10)]
    pub limit: u32,

    /// 1-based result page.
    #[arg(long, default_value_t = 1)]
    pub page: u32,

    /// Language and optional region, e.g. pl-PL.
    #[arg(long, value_name = "TAG")]
    pub locale: Option<String>,

    /// Adult-content filter.
    #[arg(long, value_enum, default_value_t = Safe::Moderate)]
    pub safe: Safe,

    /// Restrict results by age.
    #[arg(long, value_enum)]
    pub freshness: Option<Fresh>,

    /// Restrict results to one host.
    #[arg(long, value_name = "HOST")]
    pub site: Option<String>,

    /// Which path the query takes; `full` needs JavaScript, which this
    /// release does not run.
    #[arg(long, value_enum, default_value_t = SearchMode::Auto)]
    pub mode: SearchMode,

    /// Pre-connect to the providers before querying.
    #[arg(long)]
    pub warm: bool,

    /// Point a provider's plan at another endpoint: `<plan_id>=<url>`.
    /// Fixture replay; repeatable.
    #[arg(long = "search-endpoint", value_name = "PLAN=URL", hide = true)]
    pub search_endpoint: Vec<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Safe {
    Off,
    Moderate,
    Strict,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Fresh {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum SearchMode {
    Fast,
    Full,
    Auto,
}

#[derive(Debug, Args)]
pub struct ScrapeArgs {
    /// URLs to fetch.
    pub urls: Vec<String>,

    /// Read URLs, one per line, from a file or `-` for stdin.
    /// The lookahead window starts at `--concurrency × 4` and grows (up to 4096 entries)
    /// while every ready host is at its `--per-host` concurrency limit, ensuring
    /// good host diversity on host-grouped lists.
    #[arg(long, value_name = "FILE")]
    pub from: Option<String>,

    /// What each JSON line carries.
    #[arg(long, value_enum, default_value_t = ScrapeDump::Text)]
    pub dump: ScrapeDump,

    /// Pages fetched at the same time across all hosts.
    #[arg(long, default_value_t = 4)]
    pub concurrency: usize,

    /// Requests in flight to one host.
    #[arg(long, default_value_t = 2)]
    pub per_host: usize,

    /// Minimum spacing between two requests to the same host, milliseconds.
    #[arg(long, value_name = "MS", default_value_t = 0)]
    pub delay: u64,

    /// Fetch pages robots.txt disallows. Off by default, and the reason it
    /// is off is that the file says what the site asked for.
    #[arg(long)]
    pub ignore_robots: bool,

    /// Stop after this many pages.
    #[arg(long, value_name = "N")]
    pub max_pages: Option<usize>,

    /// Follow links found in the fetched pages.
    #[arg(long)]
    pub follow_links: bool,

    /// Only follow links to the host they were found on.
    #[arg(long)]
    pub same_host: bool,

    /// How many link hops to follow; 0 is the seed pages only.
    #[arg(long, default_value_t = 1)]
    pub depth: usize,

    /// Print the lines in the order the URLs were given.
    #[arg(long)]
    pub ordered: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ScrapeDump {
    Text,
    Markdown,
    Links,
    Json,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("area").args(["full_page", "selector", "clip"])))]
pub struct ScreenshotArgs {
    /// The URL to capture.
    pub url: String,

    /// Image file to write; `-` writes to stdout, which a terminal refuses.
    /// A file needs --allow-write.
    #[arg(
        short = 'o',
        long = "output",
        value_name = "FILE",
        default_value = "screenshot.png"
    )]
    pub output: PathBuf,

    /// Capture the whole scrollable page, not the viewport.
    #[arg(long)]
    pub full_page: bool,

    /// Capture the border box of the first element matching this CSS selector.
    #[arg(long, value_name = "CSS")]
    pub selector: Option<String>,

    /// Capture a rectangle of the document, in CSS pixels, widened to whole
    /// device pixels.
    #[arg(long, value_name = "X,Y,W,H", value_parser = parse_clip)]
    pub clip: Option<[f32; 4]>,

    /// Image encoding.
    #[arg(long, value_enum, default_value_t = ScreenshotFormat::Png)]
    pub format: ScreenshotFormat,

    /// JPEG quality, 1 to 100 (default 90). Needs `--format jpeg`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=100))]
    pub quality: Option<u8>,

    /// Viewport width in CSS pixels.
    #[arg(long, value_name = "PX", default_value_t = 1280, value_parser = clap::value_parser!(u32).range(1..))]
    pub width: u32,

    /// Viewport height in CSS pixels.
    #[arg(long, value_name = "PX", default_value_t = 800, value_parser = clap::value_parser!(u32).range(1..))]
    pub height: u32,

    /// Device pixels per CSS pixel.
    #[arg(long, value_name = "N", default_value_t = 1.0, value_parser = parse_dpr)]
    pub dpr: f32,

    /// How long to wait before capturing.
    #[arg(long, value_enum, default_value_t = Wait::Load)]
    pub wait: Wait,

    /// Run the page's scripts. The default is `on` in a build with `v8` and
    /// `off` otherwise; `on` needs a build with `v8`.
    #[arg(long, value_enum, value_name = "on|off")]
    pub js: Option<JsMode>,

    /// Capture a page that answered 4xx or 5xx.
    #[arg(long)]
    pub allow_http_errors: bool,

    /// Draw text with the bundled fonts only. By default scripts they lack
    /// (CJK, Thai, ...) fall back to the system fonts, so the same page can
    /// look different on another machine.
    #[arg(long)]
    pub bundled_fonts: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ScreenshotFormat {
    Png,
    Jpeg,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum JsMode {
    On,
    Off,
}

/// `X,Y,W,H` in CSS pixels; the size must be positive.
fn parse_clip(text: &str) -> Result<[f32; 4], String> {
    let values = text
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<f32>()
                .map_err(|err| format!("{part:?} is not a number: {err}"))
        })
        .collect::<Result<Vec<f32>, _>>()?;
    let [x, y, width, height]: [f32; 4] = values
        .try_into()
        .map_err(|_| String::from("expected four numbers: X,Y,W,H"))?;
    if ![x, y, width, height].iter().all(|value| value.is_finite()) {
        return Err(String::from("every value must be a finite number"));
    }
    if width <= 0.0 || height <= 0.0 {
        return Err(String::from("the width and height must be positive"));
    }
    Ok([x, y, width, height])
}

fn parse_dpr(text: &str) -> Result<f32, String> {
    let value: f32 = text
        .parse()
        .map_err(|err| format!("{text:?} is not a number: {err}"))?;
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(format!("{text} must be a positive number"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn fetch_defaults_to_text_on_stdout() {
        let cli = Cli::parse_from(["tentabrowser", "fetch", "https://example.com/"]);
        let Command::Fetch(args) = cli.command else {
            panic!("expected fetch");
        };
        assert_eq!(args.dump, None);
        assert_eq!(args.eval, None);
        assert_eq!(args.wait, Wait::Load);
        assert!(args.output.is_none());
        assert!(!cli.global.allow_private_network);
        assert_eq!(cli.global.timeout, 30_000);
    }

    #[test]
    fn global_flags_are_accepted_after_the_subcommand() {
        let cli = Cli::parse_from([
            "tentabrowser",
            "fetch",
            "http://127.0.0.1/",
            "--allow-private-network",
            "--allow-write",
            "/tmp,/var/tmp",
            "--json",
        ]);
        assert!(cli.global.allow_private_network);
        assert!(cli.global.json);
        assert_eq!(cli.global.allow_write.len(), 2, "the list is comma split");
    }

    #[test]
    fn search_takes_a_provider_list() {
        let cli = Cli::parse_from([
            "tentabrowser",
            "search",
            "rust browser",
            "--provider",
            "ddg,brave",
            "--limit",
            "5",
        ]);
        let Command::Search(args) = cli.command else {
            panic!("expected search");
        };
        assert_eq!(args.provider, ["ddg", "brave"]);
        assert_eq!(args.limit, 5);
        assert_eq!(args.mode, SearchMode::Auto);
    }

    #[test]
    fn screenshot_takes_one_area_and_a_quality_in_range() {
        let cli = Cli::parse_from([
            "tentabrowser",
            "screenshot",
            "https://example.com/",
            "--clip",
            "1,2,30,40",
        ]);
        let Command::Screenshot(args) = cli.command else {
            panic!("expected screenshot");
        };
        assert_eq!(args.clip, Some([1.0, 2.0, 30.0, 40.0]));
        assert_eq!(args.output, PathBuf::from("screenshot.png"));
        assert_eq!((args.width, args.height), (1280, 800));

        let base = ["tentabrowser", "screenshot", "https://example.com/"];
        let with = |extra: &[&str]| {
            Cli::try_parse_from(base.iter().chain(extra.iter()).copied()).map(|_| ())
        };
        assert!(with(&["--full-page", "--selector", "#a"]).is_err());
        assert!(with(&["--clip", "1,2,0,40"]).is_err(), "a zero width");
        assert!(with(&["--clip", "1,2,3"]).is_err(), "three numbers");
        assert!(with(&["--quality", "0"]).is_err());
        assert!(with(&["--quality", "101"]).is_err());
        assert!(with(&["--dpr", "-1"]).is_err());
        assert!(with(&["--format", "jpeg", "--quality", "100"]).is_ok());
    }

    #[test]
    fn the_default_profile_names_a_built_in() {
        assert!(default_profile().starts_with("chrome_144_"));
    }
}
