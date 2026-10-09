// Copyright 2026 TentaBrowser contributors. Apache-2.0.
// This guard selects and verifies the reviewed native artifact for this build.
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Configuration {
  pub target: String,
  pub features: Vec<String>,
  pub native_debug: bool,
  pub crt_static: bool,
}

/// Reviewed, source-built output identities. An empty table intentionally fails.
pub(crate) struct ArtifactPin {
  pub target: &'static str,
  pub features: &'static [&'static str],
  pub native_debug: bool,
  pub crt_static: bool,
  pub archive_sha256: &'static str,
  pub binding_sha256: &'static str,
  pub provenance_sha256: &'static str,
}

include!("artifact_pins.rs");

const FEATURES: &[(&str, &str)] = &[
  ("CARGO_FEATURE_SIMDUTF", "simdutf"),
  ("CARGO_FEATURE_USE_CUSTOM_LIBCXX", "use_custom_libcxx"),
  (
    "CARGO_FEATURE_V8_ENABLE_POINTER_COMPRESSION",
    "v8_enable_pointer_compression",
  ),
  ("CARGO_FEATURE_V8_ENABLE_SANDBOX", "v8_enable_sandbox"),
  ("CARGO_FEATURE_V8_ENABLE_V8_CHECKS", "v8_enable_v8_checks"),
];

const REJECTED_OVERRIDES: &[&str] = &[
  "V8_FROM_SOURCE",
  "RUSTY_V8_ARCHIVE",
  "RUSTY_V8_ARCHIVE_SHA256",
  "RUSTY_V8_SKIP_DOWNLOAD",
  "RUSTY_V8_SRC_BINDING_PATH",
  "RUSTY_V8_SRC_BINDING_URL",
  "RUSTY_V8_MIRROR",
  "RUSTY_V8_MIRROR_TAG",
  "RUSTY_V8_MIRROR_FALLBACK",
  "DENO_TRYBUILD",
  "GN_ARGS",
  "EXTRA_GN_ARGS",
  "CXXSTDLIB",
];

fn configuration() -> Result<Configuration, String> {
  for name in REJECTED_OVERRIDES {
    println!("cargo:rerun-if-env-changed={name}");
    if env::var_os(name).is_some() {
      return Err(format!(
        "{name} is not accepted by tenta-v8; select a reviewed bundle with TENTA_V8_ARTIFACT_DIR"
      ));
    }
  }
  let rustflags = env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
  if rustflags.contains("sanitizer") {
    return Err(
      "no instrumented native artifact has been accepted for this package"
        .into(),
    );
  }
  let features = FEATURES
    .iter()
    .filter(|(name, _)| env::var_os(name).is_some())
    .map(|(_, feature)| (*feature).to_string())
    .collect();
  let target = env::var("TARGET").map_err(|e| format!("TARGET: {e}"))?;
  let native_debug = super::prebuilt_profile() == "debug";
  let crt_static = env::var("CARGO_CFG_TARGET_FEATURE")
    .unwrap_or_default()
    .split(',')
    .any(|feature| feature == "crt-static");
  Ok(Configuration {
    target,
    features,
    native_debug,
    crt_static,
  })
}

fn select_pin<'a>(
  configuration: &Configuration,
  pins: &'a [ArtifactPin],
) -> Result<&'a ArtifactPin, String> {
  let matches: Vec<_> = pins
    .iter()
    .filter(|pin| {
      pin.target == configuration.target
        && pin
          .features
          .iter()
          .copied()
          .eq(configuration.features.iter().map(String::as_str))
        && pin.native_debug == configuration.native_debug
        && pin.crt_static == configuration.crt_static
    })
    .collect();
  match matches.as_slice() {
    [pin] => Ok(pin),
    [] => Err(format!(
      "no reviewed source-built V8 bundle matches {configuration:?}; upstream archive fallback is disabled"
    )),
    _ => Err(
      "duplicate native artifact configuration in the reviewed pin table"
        .into(),
    ),
  }
}

fn checked_copy(
  source: &Path,
  destination: &Path,
  expected: &str,
) -> Result<(), String> {
  if expected.len() != 64
    || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
  {
    return Err("invalid reviewed SHA-256 pin".into());
  }
  println!("cargo:rerun-if-changed={}", source.display());
  // Do not copy source permissions or truncate an earlier accepted private
  // artifact. Old versions of the guard left read-only files in OUT_DIR.
  // A unique sibling lets us verify bytes before replacing that directory entry.
  static SEQUENCE: AtomicU64 = AtomicU64::new(0);
  let staging = destination.with_extension(format!(
    "tenta-copy-{}-{}",
    std::process::id(),
    SEQUENCE.fetch_add(1, Ordering::Relaxed),
  ));
  let mut output = fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(&staging)
    .map_err(|e| format!("could not create {}: {e}", staging.display()))?;
  let cleanup = StagedCopy(staging);
  let copied = fs::File::open(source)
    .and_then(|mut input| io::copy(&mut input, &mut output))
    .map_err(|e| format!("could not copy {}: {e}", source.display()));
  // Close before either the success rename or failure cleanup, also on hosts
  // where an open destination cannot be removed.
  drop(output);
  copied?;
  // Hash the exact private bytes that will be consumed, not merely the source.
  let actual = super::sha256_hex_of_file(&cleanup.0)?;
  if !actual.eq_ignore_ascii_case(expected) {
    return Err(format!(
      "native artifact SHA-256 mismatch: {} expected {expected}, got {actual}",
      source.display()
    ));
  }
  fs::rename(&cleanup.0, destination)
    .map_err(|e| format!("could not replace {}: {e}", destination.display()))?;
  Ok(())
}

struct StagedCopy(PathBuf);

impl Drop for StagedCopy {
  fn drop(&mut self) {
    // The path no longer exists after successful rename. On failure, retain the
    // prior accepted destination and discard only our newly created scratch.
    let _ = fs::remove_file(&self.0);
  }
}

pub(crate) fn library_directory(out: &Path) -> PathBuf {
  out.join("native")
}

pub(crate) struct PreparedArtifact {
  linked_archive: PathBuf,
  expected_archive_sha256: &'static str,
}

impl PreparedArtifact {
  pub(crate) fn verify_linked(&self) -> Result<(), String> {
    let actual = super::sha256_hex_of_file(&self.linked_archive)?;
    if !actual.eq_ignore_ascii_case(self.expected_archive_sha256) {
      return Err(format!(
        "final linked native archive SHA-256 mismatch: {} expected {}, got {actual}",
        self.linked_archive.display(),
        self.expected_archive_sha256,
      ));
    }
    Ok(())
  }
}

fn prepare_files(
  directory: &Path,
  out: &Path,
  archive_name: &str,
  pin: &ArtifactPin,
) -> Result<PreparedArtifact, String> {
  checked_copy(
    &directory.join(archive_name),
    &out.join(archive_name),
    pin.archive_sha256,
  )?;
  checked_copy(
    &directory.join("src_binding.rs"),
    &out.join("src_binding.rs"),
    pin.binding_sha256,
  )?;
  checked_copy(
    &directory.join("provenance.json"),
    &out.join("tenta-v8-provenance.json"),
    pin.provenance_sha256,
  )?;
  Ok(PreparedArtifact {
    linked_archive: library_directory(out).join(archive_name),
    expected_archive_sha256: pin.archive_sha256,
  })
}

/// Run before upstream's docs/trybuild/download/source-build branches.
pub(crate) fn prepare() -> Result<PreparedArtifact, String> {
  println!("cargo:rerun-if-changed=artifact_pins.rs");
  println!("cargo:rerun-if-changed=artifact_guard.rs");
  for name in [
    "TENTA_V8_ARTIFACT_DIR",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_CFG_TARGET_FEATURE",
    "V8_FORCE_DEBUG",
  ] {
    println!("cargo:rerun-if-env-changed={name}");
  }
  let configuration = configuration()?;
  let pin = select_pin(&configuration, ARTIFACT_PINS)?;
  let directory = PathBuf::from(
        env::var_os("TENTA_V8_ARTIFACT_DIR")
            .ok_or("set TENTA_V8_ARTIFACT_DIR to the prepared, reviewed native artifact bundle")?,
    );
  let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is missing")?);
  let archive_name = if configuration.target.contains("windows") {
    "rusty_v8.lib"
  } else {
    "librusty_v8.a"
  };
  let archive = out.join(archive_name);
  let binding = out.join("src_binding.rs");
  let artifact = prepare_files(&directory, &out, archive_name, pin)?;
  // SAFETY: the Cargo build-script main thread has not started other threads or
  // called upstream main yet. These variables select only the checked private
  // files above; the runtime library never mutates its process environment.
  unsafe {
    env::set_var("RUSTY_V8_ARCHIVE", &archive);
    env::set_var("RUSTY_V8_ARCHIVE_SHA256", pin.archive_sha256);
    env::set_var("RUSTY_V8_SRC_BINDING_PATH", &binding);
  }
  Ok(artifact)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn configuration() -> Configuration {
    Configuration {
      target: "aarch64-apple-darwin".into(),
      features: vec!["use_custom_libcxx".into()],
      native_debug: false,
      crt_static: false,
    }
  }

  fn pin() -> ArtifactPin {
    ArtifactPin {
      target: "aarch64-apple-darwin",
      features: &["use_custom_libcxx"],
      native_debug: false,
      crt_static: false,
      archive_sha256: "not used by selection controls",
      binding_sha256: "not used by selection controls",
      provenance_sha256: "not used by selection controls",
    }
  }

  #[test]
  fn a_pin_cannot_be_reused_for_a_different_abi_configuration() {
    let pins = [pin()];
    let wanted = configuration();
    assert!(select_pin(&wanted, &pins).is_ok());
    let mut wrong_target = wanted.clone();
    wrong_target.target = "x86_64-unknown-linux-gnu".into();
    let mut compressed = wanted.clone();
    compressed
      .features
      .push("v8_enable_pointer_compression".into());
    let mut checks = wanted.clone();
    checks.features.push("v8_enable_v8_checks".into());
    let mut system_libcxx = wanted.clone();
    system_libcxx.features.clear();
    let mut debug = wanted.clone();
    debug.native_debug = true;
    let mut static_crt = wanted;
    static_crt.crt_static = true;
    for mismatched in [
      wrong_target,
      compressed,
      checks,
      system_libcxx,
      debug,
      static_crt,
    ] {
      assert!(select_pin(&mismatched, &pins).is_err(), "{mismatched:?}");
    }
  }

  #[test]
  fn missing_or_ambiguous_artifact_configuration_is_rejected() {
    let wanted = configuration();
    assert!(select_pin(&wanted, &[]).is_err());
    assert!(select_pin(&wanted, &[pin(), pin()]).is_err());
  }

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      static SEQUENCE: AtomicU64 = AtomicU64::new(0);
      let path = env::temp_dir().join(format!(
        "tenta-v8-artifact-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
      ));
      fs::create_dir(&path).expect("fresh temporary test directory");
      Self(path)
    }
  }

  impl Drop for TempDirectory {
    fn drop(&mut self) {
      let _ = fs::remove_dir_all(&self.0);
    }
  }

  #[test]
  fn corrupted_archive_or_binding_copy_is_never_selected() {
    let directory = TempDirectory::new();
    let source = directory.0.join("source");
    let destination = directory.0.join("private-copy");
    let original = b"reviewed native artifact bytes";
    fs::write(&source, original).unwrap();
    let expected = super::super::sha256_hex(original);
    checked_copy(&source, &destination, &expected).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), original);
    fs::write(&source, b"a corrupted cache entry").unwrap();
    assert!(checked_copy(&source, &destination, &expected).is_err());
    assert_eq!(fs::read(&destination).unwrap(), original);
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
  }

  #[cfg(unix)]
  #[test]
  fn repeated_bundle_preparation_replaces_read_only_private_outputs() {
    let directory = TempDirectory::new();
    let out = directory.0.join("out");
    fs::create_dir(&out).unwrap();
    let bundles = [
      (
        "first",
        ["abc", "def", "ghi"],
        ArtifactPin {
          archive_sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
          binding_sha256: "cb8379ac2098aa165029e3938a51da0bcecfc008fd6795f401178647f96c5b34",
          provenance_sha256: "50ae61e841fac4e8f9e40baf2ad36ec868922ea48368c18f9535e47db56dd7fb",
          ..pin()
        },
      ),
      (
        "changed",
        ["new archive", "new binding", "new provenance"],
        ArtifactPin {
          archive_sha256: "ad91a108cf009c7f0a19cdca877fb489735ec99b0e9bda04b0d05dccc5f1237a",
          binding_sha256: "67535942bd2ffe682d576ae311f65c9a8939e8f1f4450b1379605aea27879d82",
          provenance_sha256: "b21b866e4f50f143bf761c9e1c63bcf4e4fca6181698fbf81eecf079dc090a25",
          ..pin()
        },
      ),
    ];
    let names = ["librusty_v8.a", "src_binding.rs", "provenance.json"];
    let outputs = [
      "librusty_v8.a",
      "src_binding.rs",
      "tenta-v8-provenance.json",
    ];
    for (name, bytes, pin) in bundles {
      let source = directory.0.join(name);
      fs::create_dir(&source).unwrap();
      for (file, contents) in names.iter().zip(bytes) {
        let path = source.join(file);
        fs::write(&path, contents).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
      }
      for _ in 0..2 {
        prepare_files(&source, &out, names[0], &pin).unwrap();
        for ((file, original), contents) in outputs.iter().zip(names).zip(bytes)
        {
          let path = out.join(file);
          assert_eq!(fs::read(&path).unwrap(), contents.as_bytes());
          assert!(
            fs::metadata(source.join(original))
              .unwrap()
              .permissions()
              .readonly()
          );
          // Reproduce the private state left by the previous fs::copy guard.
          let mut permissions = fs::metadata(&path).unwrap().permissions();
          permissions.set_readonly(true);
          fs::set_permissions(&path, permissions).unwrap();
        }
        assert_eq!(fs::read_dir(&out).unwrap().count(), 3);
      }
    }
  }

  #[test]
  fn configurations_keep_final_link_archives_separate() {
    let directory = TempDirectory::new();
    let common = directory.0.join("target/release/build");
    let first_out = common.join("tenta-v8-first/out");
    let second_out = common.join("tenta-v8-second/out");
    let first = library_directory(&first_out);
    let second = library_directory(&second_out);
    assert!(first.starts_with(&first_out));
    assert!(second.starts_with(&second_out));
    assert_ne!(first, second);
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let source = directory.0.join("source");
    fs::write(&source, b"first configuration").unwrap();
    checked_copy(
      &source,
      &first.join("librusty_v8.a"),
      &super::super::sha256_hex(b"first configuration"),
    )
    .unwrap();
    fs::write(&source, b"second configuration").unwrap();
    checked_copy(
      &source,
      &second.join("librusty_v8.a"),
      &super::super::sha256_hex(b"second configuration"),
    )
    .unwrap();
    assert_eq!(
      fs::read(first.join("librusty_v8.a")).unwrap(),
      b"first configuration"
    );
    assert_eq!(
      fs::read(second.join("librusty_v8.a")).unwrap(),
      b"second configuration"
    );
  }

  #[test]
  fn corruption_at_the_final_link_path_is_rejected() {
    let directory = TempDirectory::new();
    let linked_archive = directory.0.join("librusty_v8.a");
    // SHA-256("abc"), also covered by upstream's published test vectors.
    let artifact = PreparedArtifact {
      linked_archive: linked_archive.clone(),
      expected_archive_sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    };
    fs::write(&linked_archive, b"abc").unwrap();
    artifact.verify_linked().unwrap();
    let uppercase_pin = PreparedArtifact {
      linked_archive: linked_archive.clone(),
      expected_archive_sha256: "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
    };
    uppercase_pin.verify_linked().unwrap();
    fs::write(&linked_archive, b"corrupt final copy").unwrap();
    assert!(artifact.verify_linked().is_err());
  }

  #[test]
  fn malformed_pin_cannot_authorize_even_matching_payload() {
    let directory = TempDirectory::new();
    let source = directory.0.join("source");
    let destination = directory.0.join("private-copy");
    fs::write(&source, b"payload").unwrap();
    for invalid in [
      "",
      "1234",
      "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    ] {
      assert!(checked_copy(&source, &destination, invalid).is_err());
      assert!(!destination.exists());
    }
  }
}
