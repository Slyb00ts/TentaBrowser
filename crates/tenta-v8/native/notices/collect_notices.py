#!/usr/bin/env python3
"""Collect exact upstream notice bytes; never modify the native build tree."""

import argparse
import base64
import concurrent.futures
import hashlib
import json
from pathlib import Path
import urllib.request


def identity(data):
    return {
        "bytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
        "git_blob_sha1": hashlib.sha1(
            b"blob " + str(len(data)).encode() + b"\0" + data
        ).hexdigest(),
    }


def collect_remote(root, row):
    entries = []
    if "error" in row:
        raise ValueError(f"inventory fetch failed: {row}")
    for notice in row["notices"]:
        if notice["type"] != "blob":
            raise ValueError("a notice must be an ordinary file")
        name = notice["name"]
        if Path(name).name != name or name in (".", ".."):
            raise ValueError("invalid notice filename")
        prefix = row.get("subdirectory", "")
        path = f"{prefix}/{name}" if prefix else name
        source = f'{row["repository"]}/+/{row["revision"]}/{path}?format=TEXT'
        with urllib.request.urlopen(source, timeout=30) as response:
            encoded = response.read(2 * 1024 * 1024 + 1)
        if len(encoded) > 2 * 1024 * 1024:
            raise ValueError("notice response exceeds bound")
        data = base64.b64decode(encoded, validate=True)
        record = identity(data)
        if record["git_blob_sha1"] != notice["id"]:
            raise ValueError(f"notice blob differs from pinned tree: {source}")
        destination = root / "texts" / row["name"] / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        entries.append({
            "component": row["name"],
            "path": destination.relative_to(root).as_posix(),
            "source": source,
            "revision": row["revision"],
            "tree": row["tree"],
            **record,
        })
    return entries


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--reconstructed-source", type=Path, required=True)
    parser.add_argument("--rust-toolchain", type=Path, required=True)
    parser.add_argument("--rust-toolchain-provenance", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output
    rows = json.loads((root / "remote-root-inventory.json").read_text())
    rows += json.loads((root / "remote-nested-inventory.json").read_text())
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        chunks = list(pool.map(lambda row: collect_remote(root, row), rows))
    records = [record for chunk in chunks for record in chunk]
    vendor = args.reconstructed_source / "third_party/rust/chromium_crates_io/vendor"
    for path in sorted(vendor.rglob("*")):
        if not path.is_file() or not path.name.upper().startswith(
            ("LICENSE", "COPYING", "NOTICE")
        ):
            continue
        relative = path.relative_to(vendor)
        data = path.read_bytes()
        destination = root / "texts" / "rust-vendor" / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        records.append({
            "component": "rust-vendor/" + relative.parts[0],
            "path": destination.relative_to(root).as_posix(),
            "source": path.relative_to(args.reconstructed_source).as_posix(),
            "source_proof": "../source-reproduction-attempt12-result.json",
            **identity(data),
        })
    provenance = json.loads(args.rust_toolchain_provenance.read_text())
    relative = "share/doc/rustc/COPYRIGHT-library.html"
    data = (args.rust_toolchain / relative).read_bytes()
    destination = root / "texts" / "rust-stdlib" / "COPYRIGHT-library.html"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(data)
    records.append({
        "component": "rust-stdlib",
        "path": destination.relative_to(root).as_posix(),
        "source": relative,
        "source_archive": provenance["url"],
        "source_archive_sha256": provenance["sha256"],
        **identity(data),
    })
    manifest = {
        "status": "partial notice collection; not a complete linked-component audit",
        "scope": "Pinned native dependencies and reconstructed Rust vendor notice texts; some components may be build-only or unused in a target.",
        "remaining": [
            "Map all native/header-only and Rust standard-library components to notices using the final build graph.",
            "Resolve Chromium allocator notice attribution if its code is linked in the selected target.",
            "Combine binding MIT, V8 notices, local Apache-2.0 patch attribution and dependency notices for redistribution.",
        ],
        "files": sorted(records, key=lambda record: record["path"]),
    }
    (root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"files": len(records), "bytes": sum(r["bytes"] for r in records)}))


if __name__ == "__main__":
    main()
