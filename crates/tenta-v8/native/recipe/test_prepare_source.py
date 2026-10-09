"""Source preparation failure controls; no native compilation or network."""
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

import prepare_source as recipe


class SourcePreparationTests(unittest.TestCase):
    def test_corrupt_input_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "input"
            path.write_bytes(b"original")
            expected = recipe.sha256(path)
            recipe.checked_file(path, expected, 8)
            path.write_bytes(b"modified")
            with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                recipe.checked_file(path, expected, 8)

    def test_wrong_size_fails_before_hash_or_extraction(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "input"
            path.write_bytes(b"short")
            with self.assertRaisesRegex(ValueError, "size mismatch"):
                recipe.checked_file(path, "not a valid checksum", 50)

    def test_relative_paths_cannot_escape_the_new_source_tree(self):
        for path in ["", "/etc/passwd", "../outside", "nested/../../outside"]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                recipe.relative(path)
        self.assertEqual(recipe.relative("v8/src/objects/keys.cc"), Path("v8/src/objects/keys.cc"))

    def test_archive_traversal_never_writes_outside_destination(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = root / "hostile.tar"
            with tarfile.open(archive, "w") as output:
                member = tarfile.TarInfo("../outside")
                member.size = 4
                output.addfile(member, io.BytesIO(b"data"))
            with self.assertRaises(tarfile.FilterError):
                recipe.extract(archive, root / "destination")
            self.assertFalse((root / "outside").exists())

    def test_invalid_package_cannot_create_a_destination(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            package = root / "bad.crate"
            package.write_bytes(b"not the pinned crate")
            destination = root / "source"
            with self.assertRaisesRegex(ValueError, "size mismatch"):
                recipe.prepare(package, root, destination)
            self.assertFalse(destination.exists())


if __name__ == "__main__":
    unittest.main()
