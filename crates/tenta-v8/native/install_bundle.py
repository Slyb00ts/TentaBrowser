#!/usr/bin/env python3
"""Install a locally built native bundle and record its local pin.

Takes the Cargo target directory of a `V8_FROM_SOURCE=1` release build of the
reconstructed native30 source, copies the three files `BUILDING.md` names into
the durable cache and, with `--pin`, writes the local pin next to them. The
build guard keeps checking every byte it links: this command only records what
was built here.

A local pin is this machine's record of its own build, not the reviewed test
run behind `artifact_pins.rs`. It stays out of the repository, and a build
accepts it only when `TENTA_V8_LOCAL_PIN` names it; `env` prints that setting.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import subprocess
import sys

PACKAGE = Path(__file__).resolve().parent.parent
PINS = PACKAGE / "artifact_pins.rs"
VERSION = "152.2.0"
FEATURES = ["use_custom_libcxx"]
# The manifest `artifact_guard.rs` hashes too: a local pin names the source
# recipe it was built from and stops matching when that moves on.
SOURCE_MANIFEST = PACKAGE / "native" / "recipe" / "native30-delta.json"
LOCAL_PIN = "local-pin.toml"
HOSTS = {
    ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("Linux", "aarch64"): "aarch64-unknown-linux-gnu",
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Darwin", "x86_64"): "x86_64-apple-darwin",
}

ENTRY = re.compile(r"ArtifactPin \{(?P<body>[^}]*)\}")
FIELD = re.compile(r"(\w+): (&\[[^\]]*\]|\"[^\"]*\"|true|false),")
LINE = re.compile(r"(\w+) = (\"[^\"\\]*\"|true|false|\d+)")


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def archive_name(target: str) -> str:
    return "rusty_v8.lib" if "windows" in target else "librusty_v8.a"


def cache_root() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "tentabrowser" / "v8" / VERSION


def host_target() -> str:
    target = HOSTS.get((platform.system(), platform.machine()))
    if target is None:
        raise SystemExit("install_bundle: pass --target, this host has no default")
    return target


def local_pin_path(target: str) -> Path:
    return cache_root() / target / "default" / LOCAL_PIN


def first_line(command: list[str]) -> str:
    try:
        output = subprocess.run(command, capture_output=True, text=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return "unavailable"
    return output.splitlines()[0] if output else "unavailable"


def provenance(target: str, source: Path, release: Path, tests: str) -> dict:
    """What was built, without paths or times: equal inputs give equal bytes."""
    gn_out = release / "gn_out"
    args = (gn_out / "args.gn").read_text().replace(str(release), "<target>/release")
    record = {
        "schema": 1,
        "kind": "local source build; not the reviewed native acceptance run",
        "package": {"name": "v8", "version": VERSION},
        "configuration": {
            "target": target,
            "features": FEATURES,
            "native_debug": False,
            "crt_static": False,
            "gn_args": args,
        },
        "source": source_identity(source),
        "toolchain": {
            "clang": first_line([str(release / "clang" / "bin" / "clang"), "--version"]),
            "rustc": first_line(["rustc", "--version"]),
        },
        "native_tests": tests,
        "archive": describe(gn_out / "obj" / archive_name(target)),
        "binding": describe(gn_out / "src_binding.rs"),
    }
    return record


def source_identity(source: Path) -> dict:
    """The reviewed identities of the reconstructed tree, not hashes of its
    records: those name the downloaded archives, whose bytes differ per download."""
    base = json.loads((source / "TENTA-SOURCE.json").read_text())
    native30 = json.loads((source / "TENTA-SOURCE-NATIVE30.json").read_text())
    return {
        "package_sha256": base["package_sha256"],
        "native23_patch_sha256": base["patch_sha256"],
        "incremental_patch_sha256": native30["incremental_patch_sha256"],
        "Cargo.lock": sha256(source / "Cargo.lock"),
    }


def describe(path: Path) -> dict:
    return {"bytes": path.stat().st_size, "sha256": sha256(path)}


def install(
    path: Path, data: bytes | None = None, source: Path | None = None, replace: bool = False
) -> None:
    """Write `path` once. An existing file with other bytes is replaced only on request."""
    if data is None:
        data = source.read_bytes()
    if path.exists():
        if hashlib.sha256(data).hexdigest() == sha256(path):
            return
        if not replace:
            raise ValueError(f"refusing to replace a different file: {path}")
    staging = path.with_name(path.name + ".part")
    staging.write_bytes(data)
    staging.replace(path)


def read_pins(text: str) -> list[dict]:
    return [dict(FIELD.findall(entry.group("body"))) for entry in ENTRY.finditer(text)]


def pinned(target: str, pins_file: Path = PINS) -> dict | None:
    """The hashes the reviewed table accepts for the default configuration of `target`."""
    for entry in read_pins(pins_file.read_text()):
        if entry.get("target") == f'"{target}"' and entry.get("native_debug") == "false":
            return {
                "archive": entry["archive_sha256"].strip('"'),
                "src_binding.rs": entry["binding_sha256"].strip('"'),
                "provenance.json": entry["provenance_sha256"].strip('"'),
            }
    return None


def render_local_pin(target: str, hashes: dict, source_sha256: str) -> str:
    return (
        "# Local V8 pin written by crates/tenta-v8/native/install_bundle.py for a\n"
        "# bundle built on this machine. A build accepts it only when\n"
        "# TENTA_V8_LOCAL_PIN names this file.\n"
        "schema = 1\n"
        f'target = "{target}"\n'
        f'features = "{",".join(FEATURES)}"\n'
        "native_debug = false\n"
        "crt_static = false\n"
        f'source_sha256 = "{source_sha256}"\n'
        f'archive_sha256 = "{hashes["archive"]}"\n'
        f'binding_sha256 = "{hashes["src_binding.rs"]}"\n'
        f'provenance_sha256 = "{hashes["provenance.json"]}"\n'
    )


def locally_pinned(target: str, pin_file: Path, manifest: Path = SOURCE_MANIFEST) -> dict | None:
    """The hashes of this machine's own build, if its pin is for this source recipe."""
    if not pin_file.is_file():
        return None
    lines = [line for line in pin_file.read_text().splitlines() if line and line[0] != "#"]
    matches = [LINE.fullmatch(line) for line in lines]
    if not all(matches):
        return None
    fields = {match.group(1): match.group(2).strip('"') for match in matches}
    expected = {
        "schema": "1",
        "target": target,
        "features": ",".join(FEATURES),
        "native_debug": "false",
        "crt_static": "false",
        "source_sha256": sha256(manifest),
    }
    hashes = ("archive_sha256", "binding_sha256", "provenance_sha256")
    if len(fields) != len(lines) or set(fields) != set(expected) | set(hashes):
        return None
    if any(fields[name] != value for name, value in expected.items()):
        return None
    return {
        "archive": fields["archive_sha256"],
        "src_binding.rs": fields["binding_sha256"],
        "provenance.json": fields["provenance_sha256"],
    }


def from_this_recipe(identity: dict, manifest: Path = SOURCE_MANIFEST) -> bool:
    """Whether a reconstructed tree is the one this checkout's recipe describes."""
    recipe = json.loads(manifest.read_text())
    return (
        identity["native23_patch_sha256"] == recipe["base_recipe_patch_sha256"]
        and identity["incremental_patch_sha256"] == recipe["incremental_patch_sha256"]
    )


def matches(directory: Path, target: str, wanted: dict | None) -> bool:
    if wanted is None:
        return False
    files = {
        "archive": directory / archive_name(target),
        "src_binding.rs": directory / "src_binding.rs",
        "provenance.json": directory / "provenance.json",
    }
    return all(path.is_file() for path in files.values()) and all(
        sha256(path) == wanted[name] for name, path in files.items()
    )


def find(
    target: str,
    directory: Path | None,
    pins_file: Path = PINS,
    pin_file: Path | None = None,
    manifest: Path = SOURCE_MANIFEST,
) -> tuple[Path, Path | None] | None:
    """A bundle directory a build accepts, and the local pin that makes it so.

    The reviewed table goes first; its bundles need no local pin.
    """
    pin_file = pin_file or local_pin_path(target)
    candidates = [directory] if directory else sorted((cache_root() / target / "default").glob("*/"))
    accepted = ((pinned(target, pins_file), None), (locally_pinned(target, pin_file, manifest), pin_file))
    for wanted, source in accepted:
        for candidate in candidates:
            if matches(candidate, target, wanted):
                return candidate, source
    return None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    locate = commands.add_parser("find", help="print the bundle directory a build accepts")
    settings = commands.add_parser("env", help="print the shell settings that select that bundle")
    for command in (locate, settings):
        command.add_argument("--target", help="default: this host")
        command.add_argument("--directory", type=Path, help="check this directory only")
    build = commands.add_parser("install", help="install a finished source build")
    build.add_argument("--target", required=True)
    build.add_argument("--source", type=Path, required=True, help="reconstructed native30 tree")
    build.add_argument("--release", type=Path, required=True, help="<cargo target dir>/release")
    build.add_argument("--native-tests", default="not run")
    build.add_argument("--pin", action="store_true", help="write the local pin for this bundle")
    build.add_argument(
        "--replace-provenance",
        action="store_true",
        help="describe an already installed archive anew, e.g. after a toolchain change",
    )
    args = parser.parse_args()

    if args.command in ("find", "env"):
        found = find(args.target or host_target(), args.directory)
        if found is None:
            raise SystemExit(1)
        directory, pin_file = found
        if args.command == "find":
            print(directory)
        else:
            # An empty TENTA_V8_LOCAL_PIN is the same as none: the reviewed table decides.
            print(f"export TENTA_V8_ARTIFACT_DIR={shlex.quote(str(directory))}")
            print(f"export TENTA_V8_LOCAL_PIN={shlex.quote(str(pin_file or ''))}")
        return

    release = args.release.resolve()
    gn_out = release / "gn_out"
    archive = gn_out / "obj" / archive_name(args.target)
    binding = gn_out / "src_binding.rs"
    for path in (archive, binding, gn_out / "args.gn"):
        if not path.is_file():
            raise SystemExit(f"install_bundle: not a finished source build, missing {path}")
    record = provenance(args.target, args.source.resolve(), release, args.native_tests)
    if not from_this_recipe(record["source"]):
        raise SystemExit(
            f"install_bundle: {args.source} is not the source {SOURCE_MANIFEST.name} describes"
        )
    record_bytes = (json.dumps(record, indent=2, sort_keys=True) + "\n").encode()
    hashes = {
        "archive": record["archive"]["sha256"],
        "src_binding.rs": record["binding"]["sha256"],
        "provenance.json": hashlib.sha256(record_bytes).hexdigest(),
    }
    directory = cache_root() / args.target / "default" / hashes["archive"][:16]
    if matches(directory, args.target, pinned(args.target)):
        # The build reproduced the reviewed archive. Its directory keeps the
        # reviewed provenance, and the reviewed table already accepts it.
        print(json.dumps({"directory": str(directory), "pin": "reviewed bundle, left as it is"}))
        return
    directory.mkdir(parents=True, exist_ok=True)
    outcome = "not pinned"
    try:
        install(directory / archive_name(args.target), source=archive)
        install(directory / "src_binding.rs", source=binding)
        # The directory is named after the archive, so a rebuild that reproduces
        # the archive may describe it anew; the archive and binding never change.
        install(directory / "provenance.json", data=record_bytes, replace=args.replace_provenance)
        if args.pin:
            pin_file = local_pin_path(args.target)
            text = render_local_pin(args.target, hashes, sha256(SOURCE_MANIFEST))
            install(pin_file, data=text.encode(), replace=True)
            outcome = str(pin_file)
    except ValueError as error:
        raise SystemExit(f"install_bundle: {error}") from error
    print(json.dumps({"directory": str(directory), "pin": outcome, **hashes}))


if __name__ == "__main__":
    if sys.version_info < (3, 11):
        raise SystemExit("Python 3.11+ is required")
    main()
