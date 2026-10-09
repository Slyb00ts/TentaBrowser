//! Turning the global flags into an engine and a context
//! (`docs/spec/19-security.md` §1, `docs/spec/03-network.md` §4).

use std::path::{Path, PathBuf};

use tentabrowser::{
    BrowserContext, ClientProfile, ContextOptions, DownloadPolicy, Engine, FsPolicy, JsEngine,
    Profile, SettleConfig, Tier,
};

use crate::cli::{Global, default_profile};
use crate::error::{CliError, CliResult};

/// The persistence id of the CLI's default context: a cold process reaches a
/// known host in one RTT because its TLS ticket survived the last run
/// (`docs/spec/03-network.md` §4).
pub const CLI_PERSISTENCE_ID: &str = "cli-default";

/// The engine and the context a subcommand works with.
pub struct Session {
    pub engine: Engine,
    pub context: BrowserContext,
}

impl Session {
    pub fn close(&self) {
        self.context.close();
        self.engine.close();
    }
}

/// Builds the engine `global` describes. `page_threads` is what this command
/// needs when the caller did not pin a number: a `fetch` wants one thread,
/// not one per core, because every thread it never uses is cold-start cost
/// (`docs/spec/13-performance.md` §2).
pub fn build_engine(global: &Global, page_threads: usize) -> CliResult<Engine> {
    build_engine_for_tier(global, page_threads, Tier::Dom)
}

fn build_engine_for_tier(global: &Global, page_threads: usize, tier: Tier) -> CliResult<Engine> {
    let js = if tier == Tier::Js {
        JsEngine::V8
    } else {
        JsEngine::Auto
    };
    build_engine_with(global, page_threads, tier, js)
}

/// [`build_engine`] with the tier ceiling and the script backend named.
pub fn build_engine_with(
    global: &Global,
    page_threads: usize,
    tier: Tier,
    js: JsEngine,
) -> CliResult<Engine> {
    build_engine_settled(global, page_threads, tier, js, None)
}

/// [`build_engine_with`] with the settle tunables of the pages named; `None`
/// keeps the library's.
pub fn build_engine_settled(
    global: &Global,
    page_threads: usize,
    tier: Tier,
    js: JsEngine,
    settle: Option<SettleConfig>,
) -> CliResult<Engine> {
    let profile = resolve_profile(global.profile.as_deref())?;
    let session_dir = session_cache_dir(global);
    let policy = fs_policy(global, session_dir.as_deref())?;

    let mut builder = Engine::builder()
        .max_tier(tier)
        .js(js)
        .profile(profile)
        .fs_policy(policy)
        .ephemeral(global.ephemeral)
        .io_threads(global.io_threads.unwrap_or(1))
        .page_threads(global.page_threads.unwrap_or(page_threads));
    if let Some(dir) = &session_dir {
        builder = builder.session_cache_dir(dir);
    }
    if let Some(settle) = settle {
        builder = builder.settle(settle);
    }
    builder.build().map_err(CliError::from)
}

/// The context every subcommand fetches through.
pub fn build_context(engine: &Engine, global: &Global) -> CliResult<BrowserContext> {
    let persistence_id = (!global.ephemeral).then(|| CLI_PERSISTENCE_ID.to_owned());
    engine
        .new_context(ContextOptions {
            proxy: global.proxy.clone(),
            allow_private_network: global.allow_private_network,
            cache_dir: global.cache_dir.clone(),
            persistence_id,
            user_agent: global.user_agent.clone(),
            accept_language: global.accept_language.clone(),
            ..ContextOptions::default()
        })
        .map_err(CliError::from)
}

pub fn open(global: &Global, page_threads: usize) -> CliResult<Session> {
    let engine = build_engine(global, page_threads)?;
    let context = build_context(&engine, global)?;
    Ok(Session { engine, context })
}

/// Opens the JavaScript page path for an explicit `fetch --eval` request.
pub fn open_eval(global: &Global) -> CliResult<Session> {
    let engine = build_engine_for_tier(global, 1, Tier::Js)?;
    let context = build_context(&engine, global)?;
    Ok(Session { engine, context })
}

/// Opens the engine a `screenshot` renders with: pages may reach Tier 3.
#[cfg(feature = "render")]
pub fn open_render(global: &Global) -> CliResult<Session> {
    let engine = build_engine_for_tier(global, 1, Tier::Render)?;
    let context = build_context(&engine, global)?;
    Ok(Session { engine, context })
}

/// The filesystem policy of the process.
///
/// `--allow-read`, `--allow-write` and `--downloads` are the only thing that
/// opens the host filesystem to page content. The engine's own session cache
/// directory is added to the write roots as well: it is engine state, not
/// user content, and `docs/spec/03-network.md` §4 requires the CLI's default
/// context to persist there. `--ephemeral` leaves it out, and so the process
/// then writes nothing at all.
pub fn fs_policy(global: &Global, session_dir: Option<&Path>) -> CliResult<FsPolicy> {
    let mut policy = FsPolicy::none();
    for dir in &global.allow_read {
        policy = policy.allow_read(existing_dir(dir)?);
    }
    for dir in &global.allow_write {
        policy = policy.allow_write(existing_dir(dir)?);
    }
    if let Some(dir) = &global.downloads {
        let dir = existing_dir(dir)?;
        policy = policy.allow_write(dir.clone());
        policy.downloads = DownloadPolicy::Accept { dir };
    }
    if let Some(dir) = &global.cache_dir {
        std::fs::create_dir_all(dir)
            .map_err(|err| CliError::usage(format!("--cache-dir {}: {err}", dir.display())))?;
        policy = policy.allow_write(existing_dir(dir)?);
    }
    if let Some(dir) = session_dir {
        policy = policy.allow_write(dir);
    }
    Ok(policy)
}

/// The session-cache directory this run may use, or `None` when it has none:
/// `--ephemeral`, no home directory, or a home that cannot be written to.
fn session_cache_dir(global: &Global) -> Option<PathBuf> {
    if global.ephemeral {
        return None;
    }
    let dir = tentabrowser::net::SessionCache::default_dir()?;
    match std::fs::create_dir_all(&dir) {
        Ok(()) => Some(dir),
        Err(error) => {
            tracing::warn!(
                dir = %dir.display(),
                %error,
                "session cache is unusable; this run resumes no TLS session"
            );
            None
        }
    }
}

fn existing_dir(dir: &Path) -> CliResult<PathBuf> {
    if !dir.is_dir() {
        return Err(CliError::usage(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    Ok(dir.to_path_buf())
}

/// `--profile`: a built-in name, a path to a profile TOML, or `none`.
pub fn resolve_profile(name: Option<&str>) -> CliResult<Profile> {
    let name = name.unwrap_or(default_profile());
    if name.eq_ignore_ascii_case("none") {
        return Ok(Profile::None);
    }
    let candidate = Path::new(name);
    let profile = if candidate.extension().is_some_and(|ext| ext == "toml") {
        ClientProfile::load(candidate)
    } else {
        ClientProfile::builtin(name)
    };
    profile
        .map(Profile::from)
        .map_err(|error| CliError::usage(format!("--profile {name}: {error}")))
}

/// The `User-Agent` requests carry: the override, else the profile's
/// (`docs/spec/21-project-operations.md` §2, O-3).
pub fn user_agent(global: &Global, profile: &Profile) -> String {
    if let Some(user_agent) = &global.user_agent {
        return user_agent.clone();
    }
    profile
        .client_profile()
        .map(|profile| profile.headers.user_agent.clone())
        .unwrap_or_else(|| format!("tentabrowser/{}", env!("CARGO_PKG_VERSION")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn global(args: &[&str]) -> Global {
        crate::cli::Cli::parse_from(
            std::iter::once("tentabrowser")
                .chain(args.iter().copied())
                .chain(["fetch", "http://example.test/"]),
        )
        .global
    }

    #[test]
    fn the_default_policy_denies_every_path_but_the_session_cache() {
        let policy = fs_policy(&global(&[]), None).expect("policy");
        assert!(policy.read_roots.is_empty());
        assert!(policy.write_roots.is_empty());
        assert_eq!(policy.downloads, DownloadPolicy::Deny);
        assert!(policy.check_write(Path::new("/tmp/x")).is_err());
    }

    #[test]
    fn allow_write_opens_exactly_the_named_roots() {
        let dir = tenta_testutil::temp_dir("cli-policy");
        let policy = fs_policy(
            &global(&["--allow-write", dir.path().to_str().expect("utf-8")]),
            None,
        )
        .expect("policy");
        assert_eq!(policy.write_roots.len(), 1);
        assert!(policy.check_write(&dir.path().join("out.txt")).is_ok());
        assert!(policy.check_write(Path::new("/tmp")).is_err());
        assert!(policy.check_read(dir.path()).is_err(), "write is not read");
    }

    #[test]
    fn a_nonexistent_root_is_a_usage_error() {
        let error = fs_policy(&global(&["--allow-read", "/no/such/dir/here"]), None)
            .expect_err("missing directory");
        assert_eq!(error.exit_code(), 2, "{error}");
    }

    #[test]
    fn the_session_cache_directory_is_writable_unless_ephemeral() {
        let dir = tenta_testutil::temp_dir("cli-session");
        let session = dir.path().join("session");
        std::fs::create_dir_all(&session).expect("session dir");
        let policy = fs_policy(&global(&[]), Some(&session)).expect("policy");
        assert!(policy.check_write(&session.join("x.jsonl")).is_ok());
        assert!(policy.read_roots.is_empty(), "nothing becomes readable");
    }

    #[test]
    fn the_profile_flag_takes_a_builtin_a_path_or_none() {
        assert_eq!(resolve_profile(Some("none")).expect("none").name(), "none");
        assert_eq!(
            resolve_profile(None).expect("default").name(),
            default_profile()
        );
        let error = resolve_profile(Some("chrome_1")).expect_err("unknown");
        assert_eq!(error.exit_code(), 2, "{error}");
    }

    #[test]
    fn the_user_agent_is_the_profiles_unless_overridden() {
        let profile = resolve_profile(None).expect("profile");
        let from_profile = user_agent(&global(&[]), &profile);
        assert!(
            from_profile.contains("Chrome/144"),
            "O-3 sends a plain Chrome UA: {from_profile}"
        );
        assert!(!from_profile.contains("Headless"), "{from_profile}");
        assert_eq!(
            user_agent(&global(&["--user-agent", "acme/1"]), &profile),
            "acme/1"
        );
    }
}
