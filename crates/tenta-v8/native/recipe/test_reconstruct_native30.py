"""Destination refusal controls; no native compilation or external inputs."""
from pathlib import Path
import tempfile
import unittest

import reconstruct_native30 as recipe


class Native30DestinationTests(unittest.TestCase):
    def test_existing_destinations_are_preserved_before_reading_inputs(self):
        for storeic in (False, True):
            for kind in ("file", "directory", "dangling_symlink"):
                with self.subTest(storeic=storeic, kind=kind):
                    with tempfile.TemporaryDirectory() as tmp:
                        root = Path(tmp)
                        destination = root / "source"
                        if kind == "file":
                            destination.write_bytes(b"preserve me")
                        elif kind == "directory":
                            destination.mkdir()
                            (destination / "sentinel").write_bytes(b"preserve me")
                        else:
                            destination.symlink_to(root / "missing")
                        with self.assertRaisesRegex(ValueError, "destination already exists"):
                            recipe.prepare(
                                root / "absent.crate", root / "absent-inputs",
                                destination, storeic=storeic,
                            )
                        if kind == "dangling_symlink":
                            self.assertTrue(destination.is_symlink())
                            self.assertEqual(destination.readlink(), root / "missing")
                            self.assertFalse((root / "missing").exists())
                        else:
                            sentinel = destination if kind == "file" else destination / "sentinel"
                            self.assertEqual(sentinel.read_bytes(), b"preserve me")


if __name__ == "__main__":
    unittest.main()
