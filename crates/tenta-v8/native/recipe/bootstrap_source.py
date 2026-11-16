#!/usr/bin/env python3
"""Download the pinned native30 inputs and reconstruct the source tree.

`prepare_source.py` and `reconstruct_native30.py` work offline, from archives
whose compressed bytes match the lock. Gitiles regenerates its tar.gz archives
on request, so a fresh download carries the pinned tree but not the pinned
bytes. This command verifies every downloaded archive by the Git tree id the
lock records (the ICU data file by its SHA-256), and then runs the same checked
steps as the offline recipe: package checksum, patch checksums and the
before/after hash of every changed file.

Source only: nothing is compiled, and a successful run is not build evidence.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request

import prepare_source as base
import reconstruct_native30 as native30

RECIPE = Path(__file__).resolve().parent
CRATE_URL = "https://static.crates.io/crates/v8/v8-152.2.0.crate"
CRATE_NAME = "v8-152.2.0.crate"
CRATE_BYTES = 36569477
# No pinned input is larger than the ICU data file, which Gitiles serves as
# base64 (4/3 of 10.4 MiB).
DOWNLOAD_LIMIT = 64 * 1024 * 1024
ATTEMPTS = 4


def download(url: str) -> bytes:
    """Fetch `url` with a size bound, retrying transient failures."""
    for attempt in range(1, ATTEMPTS + 1):
        try:
            with urllib.request.urlopen(url, timeout=120) as response:
                data = response.read(DOWNLOAD_LIMIT + 1)
            if len(data) > DOWNLOAD_LIMIT:
                raise ValueError(f"download exceeds {DOWNLOAD_LIMIT} bytes: {url}")
            return data
        except (urllib.error.URLError, TimeoutError, ConnectionError) as error:
            if attempt == ATTEMPTS:
                raise ValueError(f"download failed: {url}: {error}") from error
            time.sleep(2**attempt)
    raise AssertionError("unreachable")


def git_tree_id(archive: bytes) -> str:
    """The Git tree id of the files in a tar archive, as `git write-tree` names it."""
    root: dict = {}
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:*") as source:
        for member in source:
            if member.isdir():
                continue
            if member.isfile():
                mode = b"100755" if member.mode & 0o100 else b"100644"
                content = source.extractfile(member).read()
            elif member.issym():
                mode = b"120000"
                content = member.linkname.encode()
            else:
                raise ValueError(f"unsupported archive member: {member.name!r}")
            parts = PurePosixPath(member.name).parts
            if not parts or ".." in parts or PurePosixPath(member.name).is_absolute():
                raise ValueError(f"unsafe archive member: {member.name!r}")
            node = root
            for part in parts[:-1]:
                node = node.setdefault(part, {})
                if not isinstance(node, dict):
                    raise ValueError(f"archive path is both file and directory: {member.name!r}")
            blob = hashlib.sha1(b"blob %d\0" % len(content) + content).digest()
            node[parts[-1]] = (mode, blob)
    return _tree(root).hex()


def _tree(node: dict) -> bytes:
    entries = []
    for name, value in node.items():
        raw = name.encode()
        if isinstance(value, dict):
            # Git orders a directory as if its name ended in a slash.
            entries.append((raw + b"/", b"40000", raw, _tree(value)))
        else:
            entries.append((raw, value[0], raw, value[1]))
    entries.sort(key=lambda entry: entry[0])
    body = b"".join(mode + b" " + raw + b"\0" + digest for _, mode, raw, digest in entries)
    return hashlib.sha1(b"tree %d\0" % len(body) + body).digest()


def verified_input(item: dict, data: bytes) -> bytes | None:
    """The bytes to store for a lock entry, or None when `data` is not that input."""
    if item["kind"] == "tar":
        try:
            return data if git_tree_id(data) == item["git_tree_sha1"] else None
        except (tarfile.TarError, ValueError, EOFError):
            return None
    if item["kind"] == "file":
        return data if hashlib.sha256(data).hexdigest() == item["sha256"] else None
    raise ValueError(f"Unknown supplemental input kind: {item['kind']}")


def fetch_input(item: dict, downloads: Path) -> dict:
    """Ensure a verified copy of one lock entry; return the entry for this copy."""
    path = downloads / base.relative(item["name"])
    stored = None
    if path.is_file() and path.stat().st_size <= DOWNLOAD_LIMIT:
        stored = verified_input(item, path.read_bytes())
    if stored is None:
        data = download(item["url"])
        if item["kind"] == "file":
            # Gitiles serves a blob as base64 with `?format=TEXT`.
            data = base64.b64decode(data, validate=True)
        stored = verified_input(item, data)
        if stored is None:
            raise ValueError(f"downloaded input failed verification: {item['name']}")
        path.parent.mkdir(parents=True, exist_ok=True)
        staging = path.with_name(path.name + ".part")
        staging.write_bytes(stored)
        staging.replace(path)
    # The offline recipe checks compressed bytes; describe the copy verified above.
    return {**item, "sha256": hashlib.sha256(stored).hexdigest(), "bytes": len(stored)}


def fetch_crate(downloads: Path) -> Path:
    path = downloads / CRATE_NAME
    try:
        base.checked_file(path, base.PACKAGE_SHA256, CRATE_BYTES)
    except ValueError:
        data = download(CRATE_URL)
        if len(data) != CRATE_BYTES or hashlib.sha256(data).hexdigest() != base.PACKAGE_SHA256:
            raise ValueError("downloaded v8 crate failed verification") from None
        path.parent.mkdir(parents=True, exist_ok=True)
        staging = path.with_name(path.name + ".part")
        staging.write_bytes(data)
        staging.replace(path)
    return path


def apply_native30(source: Path, delta: dict, patch: Path) -> None:
    """The increment `reconstruct_native30.py` applies after the base recipe."""
    for item in delta["changed"]:
        path = source / native30.relative(item["path"])
        if item["base_sha256"] is None:
            if path.exists():
                raise ValueError(f"expected absent source: {path}")
        else:
            native30.checked(path, item["base_sha256"])
    subprocess.run(
        ["git", "apply", "--check", "--whitespace=error", str(patch)], cwd=source, check=True
    )
    subprocess.run(["git", "apply", str(patch)], cwd=source, check=True)
    for item in delta["changed"]:
        path = source / native30.relative(item["path"])
        native30.checked(path, item["modified_sha256"])
        if path.stat().st_size != item["bytes"]:
            raise ValueError(f"source size mismatch: {path}")


def bootstrap(downloads: Path, destination: Path) -> dict:
    if destination.exists() or destination.is_symlink():
        raise ValueError(f"destination already exists: {destination}")
    delta = json.loads((RECIPE / "native30-delta.json").read_text())
    patch = RECIPE / "incremental-native30.patch"
    native30.checked(patch, delta["incremental_patch_sha256"])
    # The lock whose tree ids are trusted below is itself one of these pins.
    for name, expected in delta["base_recipe_inputs"].items():
        native30.checked(RECIPE / name, expected)
    lock = json.loads((RECIPE / "supplemental-inputs.lock.json").read_text())

    crate = fetch_crate(downloads)
    inputs = downloads / "inputs"
    fetched = [fetch_input(item, inputs) for item in lock["inputs"]]

    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="tenta-bootstrap-", dir=destination.parent) as temp:
        # `prepare_source.prepare` reads its lock next to the patch. Hand it the
        # reviewed patch and changed-file record with the lock of these copies.
        recipe = Path(temp) / "recipe"
        recipe.mkdir()
        for name in ("web-idl-legacy-combined.patch", "changed-files.json"):
            shutil.copyfile(RECIPE / name, recipe / name)
        (recipe / "supplemental-inputs.lock.json").write_text(
            json.dumps({"scope": lock["scope"], "inputs": fetched})
        )
        source = Path(temp) / "source"
        base.RECIPE = recipe
        try:
            base.prepare(crate, inputs, source)
        finally:
            base.RECIPE = RECIPE
        apply_native30(source, delta, patch)
        record = {
            "status": "verified source reconstruction only; native run evidence is separate",
            "verification": "supplemental archives by Git tree id, files by SHA-256",
            "native23_patch_sha256": delta["base_recipe_patch_sha256"],
            "incremental_patch_sha256": delta["incremental_patch_sha256"],
            "changed": delta["changed"],
        }
        (source / "TENTA-SOURCE-NATIVE30.json").write_text(
            json.dumps(record, indent=2, sort_keys=True) + "\n"
        )
        source.rename(destination)
    return record


def main() -> None:
    if sys.version_info < (3, 12):
        raise SystemExit("Python 3.12+ is required by the base source recipe")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--downloads", type=Path, required=True,
                        help="cache for the crate archive and the supplemental inputs")
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        bootstrap(args.downloads.resolve(), args.destination.absolute())
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"bootstrap_source: {error}") from error
    print(json.dumps({"destination": str(args.destination), "status": "verified source only"}))


if __name__ == "__main__":
    main()
