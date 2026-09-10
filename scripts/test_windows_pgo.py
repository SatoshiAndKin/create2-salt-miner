"""Check that profile identity failures block the release build."""

import copy
import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "windows_pgo", Path(__file__).with_name("pgo.py")
)
assert spec is not None and spec.loader is not None
pgo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pgo)


class ProfileIdentityTests(unittest.TestCase):
    def test_changed_or_missing_identity_is_rejected(self):
        current = {
            "schema": 1,
            "target": "x86_64-pc-windows-gnu",
            "compiler": {"release": "1.98.0", "LLVM version": "22.1.8"},
            "rustflags": "-C target-cpu=x86-64",
            "source_sha256": "source",
            "lockfile_sha256": "lock",
            "training": {"worksize": "71303168"},
        }
        pgo.verify(copy.deepcopy(current), current)
        for key in current:
            with self.subTest(key=key):
                changed = copy.deepcopy(current)
                changed[key] = "incompatible"
                with self.assertRaisesRegex(ValueError, key):
                    pgo.verify(changed, current)
                del changed[key]
                with self.assertRaisesRegex(ValueError, key):
                    pgo.verify(changed, current)

    def test_missing_and_empty_profiles_are_rejected(self):
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "missing.profdata"
            with self.assertRaisesRegex(ValueError, "Missing or empty"):
                pgo.check_profile(path)
            path.touch()
            with self.assertRaisesRegex(ValueError, "Missing or empty"):
                pgo.check_profile(path)


if __name__ == "__main__":
    unittest.main()
