#!/usr/bin/env python3
"""Reconstruct native30, optionally with StoreIC, outside live or cache trees.

This prepares source only. A successful run does not prove a native build,
artifact route, or browser behavior.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tempfile

RECIPE = Path(__file__).resolve().parent


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def checked(path: Path, expected: str) -> None:
    if not path.is_file() or digest(path) != expected:
        raise ValueError(f"missing or changed recipe input: {path}")


def relative(name: str) -> Path:
    path = PurePosixPath(name)
    if not name or path.is_absolute() or ".." in path.parts:
        raise ValueError(f"unsafe source path: {name!r}")
    return Path(*path.parts)


def apply_storeic(source: Path) -> None:
    delta_file = RECIPE / "storeic-delta.json"
    delta = json.loads(delta_file.read_text())
    patch = RECIPE / "symbol-storeic.patch"
    checked(patch, delta["patch_sha256"])
    for name, expected in delta["native30_recipe_inputs"].items():
        checked(RECIPE / relative(name), expected)
    before, after = delta["source_before"], delta["source_after"]
    if before.keys() != after.keys() or [
        name for name in before if before[name] != after[name]
    ] != ["v8/src/ic/ic.cc"]:
        raise ValueError("StoreIC must change only the pinned IC source")
    for name, expected in before.items():
        checked(source / relative(name), expected)
    subprocess.run(
        ["git", "apply", "--check", "--whitespace=error", str(patch)],
        cwd=source,
        check=True,
    )
    subprocess.run(["git", "apply", str(patch)], cwd=source, check=True)
    for name, expected in after.items():
        checked(source / relative(name), expected)
    (source / "TENTA-SOURCE-STOREIC.json").write_text(
        json.dumps(
            {"status": "verified source only", "delta_sha256": digest(delta_file)},
            indent=2,
            sort_keys=True,
        ) + "\n"
    )


def prepare(
    crate_archive: Path, inputs: Path, destination: Path, *, storeic: bool = False
) -> None:
    if destination.exists() or destination.is_symlink():
        raise ValueError(f"destination already exists: {destination}")
    delta_file = RECIPE / "native30-delta.json"
    delta = json.loads(delta_file.read_text())
    patch = RECIPE / "incremental-native30.patch"
    checked(patch, delta["incremental_patch_sha256"])
    for name, expected in delta["base_recipe_inputs"].items():
        checked(RECIPE / name, expected)
    if delta["base_recipe_inputs"]["web-idl-legacy-combined.patch"] != delta["base_recipe_patch_sha256"]:
        raise ValueError("base patch and recipe input pins disagree")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="tenta-native30-", dir=destination.parent) as temp:
        source = Path(temp) / "source"
        subprocess.run(
            [
                sys.executable,
                str(RECIPE / "prepare_source.py"),
                "--crate-archive", str(crate_archive),
                "--inputs", str(inputs),
                "--destination", str(source),
            ],
            check=True,
        )
        for item in delta["changed"]:
            path = source / relative(item["path"])
            expected = item["base_sha256"]
            if expected is None:
                if path.exists():
                    raise ValueError(f"expected absent source: {path}")
            else:
                checked(path, expected)
        subprocess.run(
            ["git", "apply", "--check", "--whitespace=error", str(patch)],
            cwd=source,
            check=True,
        )
        subprocess.run(["git", "apply", str(patch)], cwd=source, check=True)
        for item in delta["changed"]:
            path = source / relative(item["path"])
            checked(path, item["modified_sha256"])
            if path.stat().st_size != item["bytes"]:
                raise ValueError(f"source size mismatch: {path}")
        record = {
            "status": "verified source reconstruction only; native run evidence is separate",
            "native23_patch_sha256": delta["base_recipe_patch_sha256"],
            "incremental_patch_sha256": delta["incremental_patch_sha256"],
            "changed": delta["changed"],
        }
        (source / "TENTA-SOURCE-NATIVE30.json").write_text(
            json.dumps(record, indent=2, sort_keys=True) + "\n"
        )
        if storeic:
            apply_storeic(source)
        source.rename(destination)


def main() -> None:
    if sys.version_info < (3, 12):
        raise SystemExit("Python 3.12+ is required by the base source recipe")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--crate-archive", type=Path, required=True)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument(
        "--storeic", action="store_true",
        help="apply the reviewed public-Symbol StoreIC delta after native30",
    )
    args = parser.parse_args()
    prepare(
        args.crate_archive.resolve(), args.inputs.resolve(),
        args.destination.absolute(), storeic=args.storeic,
    )
    print(json.dumps({"destination": str(args.destination), "status": "verified source only"}))


if __name__ == "__main__":
    main()
