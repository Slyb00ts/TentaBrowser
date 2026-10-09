#!/usr/bin/env python3
"""Reconstruct the native candidate offline; never edit registry/live sources.

This command prepares source only. A successful result is not build/test proof.
All downloads must already be present and match the checked input records.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile

RECIPE = Path(__file__).resolve().parent
PACKAGE_SHA256 = "a10fe1a92da5c32c7c7f838ce36c0ccfcfd5edf0865b58bdde820aa64cea9888"
PATCH_SHA256 = "2091d2806100fb4ea488312f88fa31ecee871fa773ecd431b4f6da26561a32cd"


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def checked_file(path: Path, expected: str, size: int | None = None) -> None:
    if not path.is_file():
        raise ValueError(f"Required input is not a file: {path}")
    if size is not None and path.stat().st_size != size:
        raise ValueError(f"Input size mismatch: {path}")
    actual = sha256(path)
    if actual != expected:
        raise ValueError(f"SHA-256 mismatch: {path}: {actual} != {expected}")


def relative(value: str) -> Path:
    path = PurePosixPath(value)
    if not value or path.is_absolute() or ".." in path.parts:
        raise ValueError(f"Unsafe relative source path: {value!r}")
    return Path(*path.parts)


def extract(archive: Path, destination: Path, expected: str | None = None, size: int | None = None) -> None:
    if expected is None:
        # Used only by the hostile-archive extraction control. Production callers
        # always pass a reviewed checksum and bounded compressed size.
        stream = archive.open("rb")
    else:
        if size is None:
            raise ValueError("Verified extraction requires the pinned compressed size")
        with archive.open("rb") as source:
            data = source.read(size + 1)
        if len(data) != size or hashlib.sha256(data).hexdigest() != expected:
            raise ValueError(f"Archive changed or failed verification: {archive}")
        # Extract exactly the bytes verified above, even if a writable cache is
        # replaced concurrently. This package plus vendor inputs is <40 MiB.
        stream = io.BytesIO(data)
    destination.mkdir(parents=True, exist_ok=True)
    with stream, tarfile.open(fileobj=stream, mode="r:*") as source:
        source.extractall(destination, filter="data")


def prepare(package: Path, inputs: Path, destination: Path) -> dict:
    patch = RECIPE / "web-idl-legacy-combined.patch"
    changes = json.loads((RECIPE / "changed-files.json").read_text())
    supplemental = json.loads((RECIPE / "supplemental-inputs.lock.json").read_text())["inputs"]
    checked_file(package, PACKAGE_SHA256, 36569477)
    checked_file(patch, PATCH_SHA256)
    if changes["patch_sha256"] != PATCH_SHA256:
        raise ValueError("Changed-file record describes another native patch")
    for item in supplemental:
        checked_file(inputs / relative(item["name"]), item["sha256"], item["bytes"])
    if destination.exists():
        raise ValueError("Destination already exists; refusing to overwrite a source tree")
    destination.parent.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(destination.parent).free < 1024**3:
        raise ValueError("Need at least 1 GiB free for source-only reconstruction")
    # Build in a fresh sibling and atomically publish only after verification.
    with tempfile.TemporaryDirectory(prefix="tenta-v8-source-", dir=destination.parent) as tmp:
        staging = Path(tmp)
        extract(package, staging, PACKAGE_SHA256, 36569477)
        source = staging / "v8-152.2.0"
        if not source.is_dir():
            raise ValueError("Verified package has an unexpected top-level directory")
        for item in changes["files"]:
            path = source / relative(item["path"])
            expected = item["upstream_sha256"]
            if expected is None:
                if path.exists():
                    raise ValueError(f"Expected an absent new source file: {path}")
            else:
                checked_file(path, expected)
        subprocess.run(
            ["patch", "--batch", "--fuzz=0", "-p1", "-i", str(patch)],
            cwd=source,
            check=True,
        )
        for item in changes["files"]:
            checked_file(source / relative(item["path"]), item["candidate_sha256"])
        for item in supplemental:
            target = source / relative(item["path"])
            if target.exists():
                raise ValueError(f"Supplemental input would replace a packaged path: {target}")
            archive = inputs / relative(item["name"])
            if item["kind"] == "tar":
                extract(archive, target, item["sha256"], item["bytes"])
            elif item["kind"] == "file":
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(archive, target)
                checked_file(target, item["sha256"])
            else:
                raise ValueError(f"Unknown supplemental input kind: {item['kind']}")
        record = {
            "status": "verified source only; native build and tests not run by this command",
            "package_sha256": PACKAGE_SHA256,
            "patch_sha256": PATCH_SHA256,
            "changed_files": changes["files"],
            "supplemental_inputs": supplemental,
            "recipe_sha256": sha256(Path(__file__)),
        }
        (source / "TENTA-SOURCE.json").write_text(json.dumps(record, indent=2) + "\n")
        source.rename(destination)
    return record


def main() -> None:
    if sys.version_info < (3, 12):
        raise SystemExit("Python 3.12+ is required for explicit safe archive extraction")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--crate-archive", type=Path, required=True)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    record = prepare(args.crate_archive.resolve(), args.inputs.resolve(), args.destination.absolute())
    print(json.dumps({"destination": str(args.destination), "status": record["status"]}))


if __name__ == "__main__":
    main()
