"""Download verification controls; no network and no native compilation."""
import hashlib
import io
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

import bootstrap_source as bootstrap


def archive(members: list[tuple[str, bytes | str, int]]) -> bytes:
    """A tar.gz of (name, content or link target, mode); a str content is a symlink."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as output:
        for name, content, mode in members:
            info = tarfile.TarInfo(name)
            info.mode = mode
            if isinstance(content, str):
                info.type = tarfile.SYMTYPE
                info.linkname = content
                output.addfile(info)
            else:
                info.size = len(content)
                output.addfile(info, io.BytesIO(content))
    return buffer.getvalue()


MEMBERS = [
    ("README.md", b"readme\n", 0o644),
    ("run.sh", b"#!/bin/sh\n", 0o755),
    # Git orders the directory `a` as `a/`, after the file `a-b`.
    ("a/nested/deep.txt", b"deep\n", 0o644),
    ("a-b", b"sibling\n", 0o644),
    ("link", "README.md", 0o777),
]


class TreeIdentityTests(unittest.TestCase):
    def test_tree_id_is_the_one_git_writes(self):
        data = archive(MEMBERS)
        with tempfile.TemporaryDirectory() as tmp:
            with tarfile.open(fileobj=io.BytesIO(data)) as source:
                source.extractall(tmp, filter="tar")

            def git(*args: str) -> str:
                return subprocess.run(
                    ["git", "-c", "core.fileMode=true", "-c", "core.symlinks=true", *args],
                    cwd=tmp, check=True, capture_output=True, text=True,
                ).stdout.strip()

            git("init", "--quiet")
            git("add", "--all")
            self.assertEqual(bootstrap.git_tree_id(data), git("write-tree"))

    def test_tree_id_ignores_compression_and_member_order(self):
        self.assertEqual(
            bootstrap.git_tree_id(archive(MEMBERS)),
            bootstrap.git_tree_id(archive(list(reversed(MEMBERS)))),
        )

    def test_changed_content_mode_or_name_changes_the_tree_id(self):
        expected = bootstrap.git_tree_id(archive(MEMBERS))
        for index, changed in [
            (0, ("README.md", b"other\n", 0o644)),
            (1, ("run.sh", b"#!/bin/sh\n", 0o644)),
            (3, ("a-c", b"sibling\n", 0o644)),
        ]:
            members = list(MEMBERS)
            members[index] = changed
            self.assertNotEqual(bootstrap.git_tree_id(archive(members)), expected, changed)

    def test_members_outside_the_tree_and_special_files_are_refused(self):
        for name in ["../escape", "/absolute"]:
            with self.assertRaisesRegex(ValueError, "unsafe archive member"):
                bootstrap.git_tree_id(archive([(name, b"x", 0o644)]))
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as output:
            info = tarfile.TarInfo("fifo")
            info.type = tarfile.FIFOTYPE
            output.addfile(info)
        with self.assertRaisesRegex(ValueError, "unsupported archive member"):
            bootstrap.git_tree_id(buffer.getvalue())


class InputVerificationTests(unittest.TestCase):
    def test_archive_is_accepted_by_tree_and_file_by_checksum(self):
        data = archive(MEMBERS)
        tar = {"kind": "tar", "git_tree_sha1": bootstrap.git_tree_id(data)}
        self.assertEqual(bootstrap.verified_input(tar, data), data)
        self.assertIsNone(bootstrap.verified_input(tar, archive(MEMBERS[:-1])))
        self.assertIsNone(bootstrap.verified_input(tar, b"not an archive"))
        blob = {"kind": "file", "sha256": hashlib.sha256(b"icu").hexdigest()}
        self.assertEqual(bootstrap.verified_input(blob, b"icu"), b"icu")
        self.assertIsNone(bootstrap.verified_input(blob, b"other"))

    def test_cached_copy_is_reverified_and_described_by_its_own_bytes(self):
        data = archive(MEMBERS)
        item = {
            "name": "input.tar.gz", "kind": "tar", "url": "unused: the cache is valid",
            "git_tree_sha1": bootstrap.git_tree_id(data), "sha256": "pinned", "bytes": 1,
        }
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "input.tar.gz").write_bytes(data)
            entry = bootstrap.fetch_input(item, Path(tmp))
        self.assertEqual(entry["sha256"], hashlib.sha256(data).hexdigest())
        self.assertEqual(entry["bytes"], len(data))
        self.assertEqual(entry["git_tree_sha1"], item["git_tree_sha1"])

    def test_corrupt_cache_falls_through_to_the_download(self):
        data = archive(MEMBERS)
        item = {
            "name": "input.tar.gz", "kind": "tar", "url": "https://example.invalid/input",
            "git_tree_sha1": bootstrap.git_tree_id(data),
        }
        original = bootstrap.download
        requested = []
        bootstrap.download = lambda url: requested.append(url) or data
        try:
            with tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp) / "input.tar.gz"
                path.write_bytes(b"truncated")
                bootstrap.fetch_input(item, Path(tmp))
                self.assertEqual(path.read_bytes(), data)
        finally:
            bootstrap.download = original
        self.assertEqual(requested, [item["url"]])

    def test_download_that_is_not_the_pinned_tree_is_refused(self):
        item = {
            "name": "input.tar.gz", "kind": "tar", "url": "https://example.invalid/input",
            "git_tree_sha1": bootstrap.git_tree_id(archive(MEMBERS)),
        }
        original = bootstrap.download
        bootstrap.download = lambda url: archive(MEMBERS[:1])
        try:
            with tempfile.TemporaryDirectory() as tmp:
                with self.assertRaisesRegex(ValueError, "failed verification"):
                    bootstrap.fetch_input(item, Path(tmp))
                self.assertEqual(list(Path(tmp).iterdir()), [])
        finally:
            bootstrap.download = original


if __name__ == "__main__":
    unittest.main()
