//! `link-test` and `hello-sizes`: the single-libcrypto link test and the
//! "hello" size ladder of `tests/linktest` (docs/spec/03-network.md §1.1,
//! docs/spec/21-project-operations.md §4, docs/spec/14-memory.md §1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, ensure};

/// The feature ladder, in build order. Every rung is a Cargo feature of the
/// `hello` crate in `tests/linktest`.
const LADDER: [&str; 5] = ["base", "rtc", "codecs", "v8", "all"];

/// Number of `hello` runs per rung for the RSS median.
const RSS_RUNS: usize = 10;

/// Crates whose versions are recorded next to the measurements.
const RECORDED_CRATES: [&str; 14] = [
    "tokio",
    "hyper",
    "boring",
    "boring-sys",
    "hickory-resolver",
    "rcgen",
    "ring",
    "rustls",
    "str0m",
    "str0m-apple-crypto",
    "shiguredo_libvpx",
    "rav1d",
    "opusic-sys",
    "v8",
];

/// Unprefixed symbol families that only a libcrypto/libssl defines. Each name
/// must be defined at most once in the binary; `SSL_new` must be present.
const LIBCRYPTO_PREFIXES: [&str; 12] = [
    "SSL_", "EVP_", "CRYPTO_", "OPENSSL_", "BN_", "EC_", "RSA_", "X509_", "HMAC_", "SHA256_",
    "BIO_", "ERR_",
];

fn linktest_dir(root: &Path) -> PathBuf {
    root.join("tests/linktest")
}

fn build(root: &Path, profile: &str, features: &str) -> anyhow::Result<PathBuf> {
    let dir = linktest_dir(root);
    let status = Command::new(env!("CARGO"))
        .args(["build", "--profile", profile, "--features", features])
        .current_dir(&dir)
        .status()
        .with_context(|| format!("running cargo build --features {features}"))?;
    ensure!(status.success(), "cargo build --features {features} failed");
    let mut binary = dir.join("target").join(profile).join("hello");
    if !binary.exists() {
        binary.set_extension("exe");
    }
    ensure!(
        binary.exists(),
        "built binary not found at {}",
        binary.display()
    );
    Ok(binary)
}

fn run_capture(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program}"))?;
    ensure!(
        output.status.success(),
        "{program} {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `cargo xtask link-test`: build every rung with the unstripped `linktest`
/// profile and scan the binary for a second libcrypto.
pub fn link_test(root: &Path) -> anyhow::Result<()> {
    let mut failures = Vec::new();
    for features in LADDER {
        println!("== link-test: features = {features}");
        let binary = build(root, "linktest", features)?;
        match scan(root, &binary) {
            Ok(report) => println!("{report}"),
            Err(e) => {
                println!("FAIL ({features}): {e:#}");
                failures.push(features);
            }
        }
    }
    ensure!(
        failures.is_empty(),
        "link test failed for: {}",
        failures.join(", ")
    );
    println!("link-test: exactly one libcrypto (BoringSSL) in every rung");
    Ok(())
}

/// Evidence gathered from one binary.
struct ScanReport {
    dynamic_libs: Vec<String>,
    libcrypto_symbols: usize,
    ring_symbols: usize,
    version_strings: Vec<String>,
    crypto_archives: Vec<String>,
}

impl std::fmt::Display for ScanReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "  dynamic libraries: {}", self.dynamic_libs.join(", "))?;
        writeln!(
            f,
            "  libcrypto-family symbols (each defined once): {}",
            self.libcrypto_symbols
        )?;
        writeln!(
            f,
            "  ring symbols (prefixed ring_core_*): {}",
            self.ring_symbols
        )?;
        writeln!(
            f,
            "  crypto version strings: {}",
            self.version_strings.join(" | ")
        )?;
        write!(
            f,
            "  native crypto archives linked: {}",
            self.crypto_archives.join(", ")
        )
    }
}

fn scan(root: &Path, binary: &Path) -> anyhow::Result<ScanReport> {
    let binary_str = binary.to_str().context("binary path is not UTF-8")?;

    // 1. Dynamic dependencies: no libssl/libcrypto from the system.
    let dynamic_libs: Vec<String> = if cfg!(target_os = "macos") {
        run_capture("otool", &["-L", binary_str])?
            .lines()
            .skip(1)
            .filter_map(|l| l.split_whitespace().next().map(str::to_string))
            .collect()
    } else if cfg!(target_os = "linux") {
        run_capture("ldd", &[binary_str])?
            .lines()
            .filter_map(|l| l.split_whitespace().next().map(str::to_string))
            .collect()
    } else {
        Vec::new()
    };
    for lib in &dynamic_libs {
        let lower = lib.to_ascii_lowercase();
        ensure!(
            !lower.contains("libssl") && !lower.contains("libcrypto"),
            "dynamic libcrypto/libssl linked: {lib}"
        );
    }

    // 2. Defined symbols: no aws-lc prefix, every libcrypto symbol defined once.
    let nm = run_capture("nm", &["--defined-only", binary_str])?;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ring_symbols = 0usize;
    for line in nm.lines() {
        let Some(name) = line.split_whitespace().nth(2) else {
            continue;
        };
        let name = name.strip_prefix('_').unwrap_or(name);
        ensure!(
            !name.contains("aws_lc_") && !name.starts_with("AWS_LC"),
            "aws-lc symbol present: {name}"
        );
        if name.starts_with("ring_core_") {
            ring_symbols += 1;
        }
        if LIBCRYPTO_PREFIXES.iter().any(|p| name.starts_with(p)) {
            *counts.entry(name).or_default() += 1;
        }
    }
    ensure!(
        counts.contains_key("SSL_new") && counts.contains_key("EVP_aead_aes_128_gcm"),
        "BoringSSL symbols missing from the binary"
    );
    let duplicates: Vec<&str> = counts
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(name, _)| *name)
        .collect();
    ensure!(
        duplicates.is_empty(),
        "libcrypto symbols defined more than once (two libcrypto copies): {}",
        duplicates.join(", ")
    );

    // 3. Version strings: BoringSSL identifies itself; OpenSSL and AWS-LC do too.
    let bytes = std::fs::read(binary)?;
    let mut version_strings = Vec::new();
    for needle in [
        "BoringSSL",
        "OpenSSL 1.",
        "OpenSSL 3.",
        "OpenSSL 4.",
        "AWS-LC",
    ] {
        if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
            version_strings.push(needle.to_string());
        }
    }
    ensure!(
        version_strings.iter().any(|s| s == "BoringSSL"),
        "no BoringSSL version string in the binary"
    );
    let foreign: Vec<&String> = version_strings
        .iter()
        .filter(|s| s.starts_with("OpenSSL") || s.starts_with("AWS-LC"))
        .collect();
    ensure!(
        foreign.is_empty(),
        "foreign libcrypto version strings present: {foreign:?}"
    );

    // 4. Native libcrypto/libssl archives produced by build scripts for this
    //    profile (exact names; `libcryptokit.a` from the Apple backend is not one).
    let profile_dir = binary.parent().context("binary has no parent")?;
    let build_dir = profile_dir.join("build");
    let mut crypto_archives = Vec::new();
    collect_crypto_archives(&build_dir, &build_dir, &mut crypto_archives)?;
    let _ = root;
    let non_boring: Vec<&String> = crypto_archives
        .iter()
        .filter(|a| !a.contains("boring-sys"))
        .collect();
    ensure!(
        non_boring.is_empty(),
        "crypto archives from a crate other than boring-sys: {non_boring:?}"
    );

    Ok(ScanReport {
        dynamic_libs,
        libcrypto_symbols: counts.len(),
        ring_symbols,
        version_strings,
        crypto_archives,
    })
}

fn collect_crypto_archives(
    build_dir: &Path,
    dir: &Path,
    out: &mut Vec<String>,
) -> anyhow::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_crypto_archives(build_dir, &path, out)?;
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".a") || lower.ends_with(".lib")) {
            continue;
        }
        let is_libcrypto = matches!(
            lower.as_str(),
            "libcrypto.a" | "libssl.a" | "crypto.lib" | "ssl.lib" | "libcrypto.lib" | "libssl.lib"
        ) || lower.contains("aws_lc")
            || lower.contains("aws-lc")
            || lower.contains("openssl");
        if is_libcrypto {
            // Report relative to `target/<profile>/build` so the owning crate is visible.
            let rel = path.strip_prefix(build_dir).unwrap_or(&path);
            out.push(rel.display().to_string());
        }
    }
    Ok(())
}

/// `cargo xtask hello-sizes`: release build (stripped) of every rung, binary
/// size and idle RSS (median of `RSS_RUNS`), written to
/// `bench/baselines/hello-sizes.json`.
pub fn hello_sizes(root: &Path) -> anyhow::Result<()> {
    let mut steps = Vec::new();
    for features in LADDER {
        println!("== hello-sizes: features = {features}");
        let binary = build(root, "release", features)?;
        let size = std::fs::metadata(&binary)?.len();
        let mut rss = Vec::with_capacity(RSS_RUNS);
        let mut startup = Vec::with_capacity(RSS_RUNS);
        let mut v8_platform = Vec::new();
        let mut v8_isolate = Vec::new();
        for _ in 0..RSS_RUNS {
            let run = measure_run(&binary)?;
            rss.push(run.max_rss_bytes);
            startup.push(run.startup_ms);
            if let Some((p, i)) = run.v8_ms {
                v8_platform.push(p);
                v8_isolate.push(i);
            }
        }
        let mut step = serde_json::json!({
            "features": features,
            "binary_size_bytes": size,
            "max_rss_bytes_median": median_u64(&mut rss.clone()),
            "max_rss_bytes_runs": rss,
            "startup_ms_median": median_f64(&mut startup.clone()),
        });
        if !v8_platform.is_empty() {
            step["v8_platform_init_ms_median"] = median_f64(&mut v8_platform).into();
            step["v8_first_isolate_ms_median"] = median_f64(&mut v8_isolate).into();
        }
        println!(
            "  size {:.2} MB, max RSS median {:.2} MB",
            size as f64 / 1_048_576.0,
            step["max_rss_bytes_median"].as_u64().unwrap_or(0) as f64 / 1_048_576.0
        );
        steps.push(step);
    }

    let report = serde_json::json!({
        "machine": machine_info(),
        "date": run_capture("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])?.trim(),
        "toolchain": run_capture("rustc", &["-V"])?.trim(),
        "target": std::env::var("TARGET").unwrap_or_else(|_| host_triple()),
        "profile": {
            "opt-level": 3, "lto": "thin", "codegen-units": 16,
            "panic": "unwind", "strip": "symbols", "debug": false
        },
        "method": {
            "size": "stripped release binary size in bytes (std::fs::metadata)",
            "rss": format!("/usr/bin/time -l ./hello, maximum resident set size in bytes, median of {RSS_RUNS} runs, process exits right after initialisation"),
            "rss_runs": RSS_RUNS,
        },
        "crate_versions": crate_versions(root)?,
        "steps": steps,
    });
    let out = root.join("bench/baselines/hello-sizes.json");
    std::fs::create_dir_all(out.parent().context("baseline dir")?)?;
    std::fs::write(
        &out,
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    println!("wrote {}", out.display());
    Ok(())
}

struct Run {
    max_rss_bytes: u64,
    startup_ms: f64,
    v8_ms: Option<(f64, f64)>,
}

fn measure_run(binary: &Path) -> anyhow::Result<Run> {
    let (time_program, time_args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("/usr/bin/time", &["-l"])
    } else {
        ("/usr/bin/time", &["-v"])
    };
    let output = Command::new(time_program)
        .args(time_args)
        .arg(binary)
        .output()
        .context("running /usr/bin/time")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.success() && stdout.lines().any(|l| l == "ok"),
        "hello did not print ok:\n{stdout}\n{stderr}"
    );
    let rss_line = stderr
        .lines()
        .find(|l| l.to_ascii_lowercase().contains("maximum resident set size"))
        .context("no maximum resident set size line from /usr/bin/time")?;
    let number: u64 = rss_line
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .context("no number in RSS line")?
        .parse()?;
    let max_rss_bytes = if rss_line.contains("kbytes") {
        number * 1024
    } else {
        number
    };
    let startup_ms = stdout
        .lines()
        .find_map(|l| l.strip_prefix("startup: "))
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .context("no startup line")?;
    let v8_ms = stdout
        .lines()
        .find(|l| l.starts_with("v8: "))
        .and_then(|l| {
            let platform = l
                .split("platform init ")
                .nth(1)?
                .split(' ')
                .next()?
                .parse()
                .ok()?;
            let isolate = l
                .split("first isolate ")
                .nth(1)?
                .split(' ')
                .next()?
                .parse()
                .ok()?;
            Some((platform, isolate))
        });
    Ok(Run {
        max_rss_bytes,
        startup_ms,
        v8_ms,
    })
}

fn median_u64(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        values[n / 2 - 1].midpoint(values[n / 2])
    }
}

fn median_f64(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn host_triple() -> String {
    run_capture("rustc", &["-vV"])
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        })
        .unwrap_or_default()
}

fn machine_info() -> serde_json::Value {
    let sysctl = |key: &str| {
        run_capture("sysctl", &["-n", key])
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    if cfg!(target_os = "macos") {
        serde_json::json!({
            "os": format!("macOS {}", run_capture("sw_vers", &["-productVersion"]).map(|s| s.trim().to_string()).unwrap_or_default()),
            "model": sysctl("hw.model"),
            "cpu": sysctl("machdep.cpu.brand_string"),
            "memory_bytes": sysctl("hw.memsize").parse::<u64>().unwrap_or(0),
        })
    } else {
        serde_json::json!({
            "os": run_capture("uname", &["-sr"]).map(|s| s.trim().to_string()).unwrap_or_default(),
        })
    }
}

fn crate_versions(root: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let lock = std::fs::read_to_string(linktest_dir(root).join("Cargo.lock"))?;
    let mut versions = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in lock.lines() {
        if let Some(name) = line.strip_prefix("name = ") {
            current = Some(name.trim_matches('"').to_string());
        } else if let Some(version) = line.strip_prefix("version = ")
            && let Some(name) = current.take()
            && RECORDED_CRATES.contains(&name.as_str())
        {
            versions.insert(name, version.trim_matches('"').to_string());
        }
    }
    Ok(versions)
}
