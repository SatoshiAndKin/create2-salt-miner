"""Check that unusable profiles block Linux PGO optimization."""

import tempfile
import unittest
from pathlib import Path
from unittest import mock

import pgo


class ProfileTests(unittest.TestCase):
    def test_missing_and_empty_profiles_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "missing.profdata"
            with self.assertRaisesRegex(ValueError, "Missing or empty"):
                pgo.check_profile(path)
            path.touch()
            with self.assertRaisesRegex(ValueError, "Missing or empty"):
                pgo.check_profile(path)

    def test_profiles_require_executed_functions(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "profile.profdata"
            path.write_bytes(b"profile fixture")
            for functions, count in ((0, 0), (1, 0), (0, 1)):
                with (
                    self.subTest(functions=functions, count=count),
                    mock.patch.object(
                        pgo, "profdata", return_value=Path("llvm-profdata")
                    ),
                    mock.patch.object(
                        pgo,
                        "output",
                        return_value=(
                            f"Total functions: {functions}\n"
                            f"Maximum function count: {count}\n"
                        ),
                    ),
                ):
                    with self.assertRaisesRegex(ValueError, "no executed functions"):
                        pgo.check_profile(path)

    def test_executed_profile_is_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "profile.profdata"
            path.write_bytes(b"profile fixture")
            with (
                mock.patch.object(pgo, "profdata", return_value=Path("llvm-profdata")),
                mock.patch.object(
                    pgo,
                    "output",
                    return_value="Total functions: 3\nMaximum function count: 10\n",
                ),
            ):
                pgo.check_profile(path)


if __name__ == "__main__":
    unittest.main()
