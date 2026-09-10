"""Check that profile identity failures block the release build."""

import copy
import hashlib
import importlib.util
import json
import os
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

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


class BundlePathTests(unittest.TestCase):
    def test_bundle_uses_cargo_target_directory(self):
        for setting in ("default", "relative", "absolute", "config", "override"):
            with self.subTest(setting=setting):
                self.check_bundle(setting)

    def test_missing_configured_executable_does_not_use_stale_default(self):
        self.check_bundle("relative", missing=True)

    def check_bundle(self, setting, missing=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            (root / "src").mkdir()
            (root / "src/main.rs").write_text("fn main() {}\n")
            (root / "Cargo.toml").write_text(
                '[package]\nname = "bundle-fixture"\nversion = "0.1.0"\n'
                'edition = "2021"\n[workspace]\n'
            )
            (root / "scripts").mkdir()
            (root / "scripts/windows-pgo-train.ps1").write_text("# training\n")
            target = root / ("target" if setting == "default" else "build output")
            environment = dict(os.environ)
            for key in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"):
                environment.pop(key, None)
            # Isolate the fixture from user-level Cargo configuration.
            environment["CARGO_HOME"] = str(root / "cargo-home")
            if setting in ("config", "override"):
                (root / ".cargo").mkdir()
                configured = (
                    "ignored output" if setting == "override" else "build output"
                )
                (root / ".cargo/config.toml").write_text(
                    f'[build]\ntarget-dir = "{configured}"\n'
                )
            if setting in ("relative", "override"):
                environment["CARGO_TARGET_DIR"] = "build output"
            elif setting == "absolute":
                environment["CARGO_TARGET_DIR"] = str(target)
            suffix = Path("x86_64-pc-windows-gnu/release/salty.exe")
            stale = root / "target" / suffix
            stale.parent.mkdir(parents=True)
            stale.write_bytes(b"stale executable")
            executable = target / suffix
            if not missing:
                executable.parent.mkdir(parents=True, exist_ok=True)
                executable.write_bytes(b"configured executable")
            real_output = pgo.output

            def command_output(*args):
                if args == ("git", "rev-parse", "HEAD"):
                    return "fixture-commit"
                return real_output(*args)

            previous = Path.cwd()
            try:
                os.chdir(root)
                with (
                    mock.patch.dict(os.environ, environment, clear=True),
                    mock.patch.object(
                        pgo, "expected", return_value={"target": suffix.parts[0]}
                    ),
                    mock.patch.object(pgo, "output", side_effect=command_output),
                ):
                    destination = root / "bundle.zip"
                    if missing:
                        with self.assertRaises(FileNotFoundError):
                            pgo.bundle(destination)
                        self.assertFalse(destination.exists())
                    else:
                        pgo.bundle(destination)
                        with zipfile.ZipFile(destination) as archive:
                            self.assertEqual(
                                archive.read("salty.exe"), b"configured executable"
                            )
                            metadata = json.loads(archive.read("metadata.json"))
                            self.assertEqual(
                                metadata["executable_sha256"],
                                hashlib.sha256(b"configured executable").hexdigest(),
                            )
                            self.assertEqual(
                                metadata["source_commit"], "fixture-commit"
                            )
            finally:
                os.chdir(previous)


if __name__ == "__main__":
    unittest.main()
