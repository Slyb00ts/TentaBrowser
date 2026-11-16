"""Pin lookup and bundle installation controls; no native build."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import install_bundle as bundle

# The layout of the tracked table: one reviewed entry opened on the table's line.
TABLE = """\
// Reviewed bundle.
const ARTIFACT_PINS: &[ArtifactPin] = &[ArtifactPin {
  target: "aarch64-apple-darwin",
  features: &["use_custom_libcxx"],
  native_debug: false,
  crt_static: false,
  archive_sha256: "aaaa",
  binding_sha256: "bbbb",
  provenance_sha256: "cccc",
}];
"""
LINUX = "x86_64-unknown-linux-gnu"
MAC = "aarch64-apple-darwin"
FILES = {"librusty_v8.a": b"archive", "src_binding.rs": b"binding", "provenance.json": b"{}"}
HASHES = {
    "archive": hashlib.sha256(FILES["librusty_v8.a"]).hexdigest(),
    "src_binding.rs": hashlib.sha256(FILES["src_binding.rs"]).hexdigest(),
    "provenance.json": hashlib.sha256(FILES["provenance.json"]).hexdigest(),
}


class Workspace(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.pins = self.root / "artifact_pins.rs"
        self.pins.write_text(TABLE)
        self.manifest = self.root / "native30-delta.json"
        self.manifest.write_text(
            json.dumps({"base_recipe_patch_sha256": "base", "incremental_patch_sha256": "delta"})
        )
        self.pin_file = self.root / "local-pin.toml"

    def bundle(self, name: str, files: dict = FILES) -> Path:
        directory = self.root / name
        directory.mkdir()
        for file, data in files.items():
            (directory / file).write_bytes(data)
        return directory

    def write_pin(self, target: str = LINUX, hashes: dict = HASHES) -> None:
        source = bundle.sha256(self.manifest)
        self.pin_file.write_text(bundle.render_local_pin(target, hashes, source))

    def find(self, target: str, directory: Path):
        return bundle.find(target, directory, self.pins, self.pin_file, self.manifest)


class LocalPinTests(Workspace):
    def test_the_reviewed_table_is_read_and_has_no_entry_for_other_targets(self):
        self.assertEqual(
            bundle.pinned(MAC, self.pins),
            {"archive": "aaaa", "src_binding.rs": "bbbb", "provenance.json": "cccc"},
        )
        self.assertIsNone(bundle.pinned(LINUX, self.pins))

    def test_a_local_pin_round_trips_for_its_own_target_only(self):
        self.assertIsNone(bundle.locally_pinned(LINUX, self.pin_file, self.manifest))
        self.write_pin()
        self.assertEqual(bundle.locally_pinned(LINUX, self.pin_file, self.manifest), HASHES)
        self.assertIsNone(bundle.locally_pinned(MAC, self.pin_file, self.manifest))

    def test_a_local_pin_stops_matching_when_the_source_recipe_changes(self):
        self.write_pin()
        self.manifest.write_text(self.manifest.read_text() + "\n")
        self.assertIsNone(bundle.locally_pinned(LINUX, self.pin_file, self.manifest))

    def test_a_damaged_local_pin_is_not_read(self):
        self.write_pin()
        text = self.pin_file.read_text()
        for damaged in (
            text.replace("schema = 1", "schema = 2"),
            text.replace("crt_static = false\n", ""),
            text + "reviewed = true\n",
            text + "crt_static = false\n",
            text.replace('archive_sha256 = "', "archive_sha256 = "),
        ):
            self.assertNotEqual(damaged, text)
            self.pin_file.write_text(damaged)
            self.assertIsNone(bundle.locally_pinned(LINUX, self.pin_file, self.manifest))

    def test_find_accepts_only_a_directory_matching_every_local_hash(self):
        good = self.bundle("good")
        self.assertIsNone(self.find(LINUX, good))
        self.write_pin()
        self.assertEqual(self.find(LINUX, good), (good, self.pin_file))
        (good / "src_binding.rs").write_bytes(b"other binding")
        self.assertIsNone(self.find(LINUX, good))
        self.assertIsNone(self.find(LINUX, self.root / "absent"))
        self.assertIsNone(self.find("riscv64gc-unknown-linux-gnu", good))

    def test_a_reviewed_bundle_needs_no_local_pin_and_goes_first(self):
        reviewed = self.bundle("reviewed")
        self.pins.write_text(
            TABLE.replace("aaaa", HASHES["archive"])
            .replace("bbbb", HASHES["src_binding.rs"])
            .replace("cccc", HASHES["provenance.json"])
        )
        self.assertEqual(self.find(MAC, reviewed), (reviewed, None))
        self.write_pin(MAC)
        self.assertEqual(self.find(MAC, reviewed), (reviewed, None))

    def test_only_a_tree_of_this_recipe_may_be_pinned(self):
        identity = {"native23_patch_sha256": "base", "incremental_patch_sha256": "delta"}
        self.assertTrue(bundle.from_this_recipe(identity, self.manifest))
        for name in identity:
            self.assertFalse(bundle.from_this_recipe({**identity, name: "other"}, self.manifest))


class InstallTests(unittest.TestCase):
    def test_an_installed_file_is_replaced_by_other_bytes_only_on_request(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "librusty_v8.a"
            bundle.install(path, data=b"first")
            bundle.install(path, data=b"first")
            with self.assertRaisesRegex(ValueError, "refusing to replace"):
                bundle.install(path, data=b"second")
            self.assertEqual(path.read_bytes(), b"first")
            bundle.install(path, data=b"second", replace=True)
            self.assertEqual(path.read_bytes(), b"second")
            self.assertEqual(sorted(p.name for p in Path(tmp).iterdir()), ["librusty_v8.a"])


class TrackedFilesTests(unittest.TestCase):
    def test_the_tracked_table_and_manifest_are_readable(self):
        self.assertIsNotNone(bundle.pinned(MAC))
        recipe = json.loads(bundle.SOURCE_MANIFEST.read_text())
        self.assertIn("base_recipe_patch_sha256", recipe)
        self.assertIn("incremental_patch_sha256", recipe)


if __name__ == "__main__":
    unittest.main()
