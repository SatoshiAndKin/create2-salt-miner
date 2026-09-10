"""Package, validate, and merge native Windows training data (Python 3.9+)."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import zipfile
from pathlib import Path


def output(*args: str) -> str:
    return subprocess.check_output(args, text=True).strip()


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def compiler() -> dict[str, str]:
    details = dict(
        line.split(": ", 1) for line in output("rustc", "-vV").splitlines()[1:]
    )
    if details["release"] != "1.98.0":
        raise ValueError("Windows PGO requires Rust 1.98.0")
    return {key: details[key] for key in ("release", "commit-hash", "LLVM version")}


def profdata() -> Path:
    tool = (
        Path(output("rustc", "--print", "target-libdir")).parent / "bin/llvm-profdata"
    )
    if not tool.is_file():
        raise ValueError("Install llvm-tools-preview for the pinned Rust toolchain")
    return tool


def expected() -> dict:
    files = sorted(
        [
            Path(name)
            for name in (
                "Cargo.toml",
                "rust-toolchain.toml",
                "Cross.toml",
                "Dockerfile.cross",
                ".cargo/config.toml",
                "Justfile",
            )
        ]
        + list(Path("src").rglob("*.rs"))
        + list(Path("src").rglob("*.cl"))
        + [Path("scripts/pgo.py"), Path("scripts/windows-pgo-train.ps1")]
    )
    source = hashlib.sha256()
    for path in files:
        source.update(path.as_posix().encode() + b"\0" + path.read_bytes() + b"\0")
    return {
        "schema": 1,
        "target": "x86_64-pc-windows-gnu",
        "compiler": compiler(),
        "rustflags": os.environ["WINDOWS_RUSTFLAGS"],
        "source_sha256": source.hexdigest(),
        "lockfile_sha256": digest(Path("Cargo.lock")),
        "training": {
            key.lower(): os.environ[f"TRAIN_{key}"]
            for key in (
                "FACTORY",
                "CALLER",
                "CODEHASH",
                "WORKSIZE",
                "ZEROS",
                "MIN_RUNTIME_SECS",
            )
        },
    }


def verify(metadata: dict, current: dict) -> None:
    for key, value in current.items():
        if metadata.get(key) != value:
            raise ValueError(
                f"Missing, stale, or incompatible Windows PGO metadata: {key}"
            )


def check_profile(path: Path) -> None:
    if not path.is_file() or not path.stat().st_size:
        raise ValueError(f"Missing or empty PGO profile: {path}")
    result = output(str(profdata()), "show", str(path))
    print(result)
    counts = dict(
        line.strip().split(": ", 1) for line in result.splitlines() if ": " in line
    )
    if (
        int(counts.get("Total functions", "0")) == 0
        or int(counts.get("Maximum function count", "0")) == 0
    ):
        raise ValueError("PGO profile contains no executed functions")


def bundle(destination: Path) -> None:
    metadata = expected()
    cargo = json.loads(
        output("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1")
    )
    executable = (
        Path(cargo["target_directory"]) / metadata["target"] / "release/salty.exe"
    )
    metadata["executable_sha256"] = digest(executable)
    metadata["source_commit"] = output("git", "rev-parse", "HEAD")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.write(executable, "salty.exe")
        archive.writestr("metadata.json", json.dumps(metadata, indent=2) + "\n")
        archive.write("scripts/windows-pgo-train.ps1", "windows-pgo-train.ps1")
        archive.writestr(
            "Justfile",
            "windows-pgo-train bundle:\n    powershell.exe -NoProfile -File windows-pgo-train.ps1 {{quote(bundle)}}\n",
        )
    print(destination)


def import_results(results: Path) -> None:
    with (
        zipfile.ZipFile(results) as archive,
        tempfile.TemporaryDirectory() as directory,
    ):
        metadata = json.loads(archive.read("metadata.json").decode("utf-8-sig"))
        verify(metadata, expected())
        training = json.loads(archive.read("training.json").decode("utf-8-sig"))
        if training.get("platform") != "Windows" or training.get("exit_code") != 0:
            raise ValueError("Training did not finish successfully on native Windows")
        executable_hash = metadata.get("executable_sha256")
        if not isinstance(executable_hash, str) or len(executable_hash) != 64:
            raise ValueError("Missing executable checksum in Windows bundle")
        if training.get("executable_sha256") != executable_hash:
            raise ValueError("Training executable does not match the bundle")
        raw_files = []
        for index, name in enumerate(archive.namelist()):
            if name.endswith(".profraw"):
                path = Path(directory) / f"{index}.profraw"
                path.write_bytes(archive.read(name))
                raw_files.append(str(path))
        if not raw_files:
            raise ValueError("No raw profiles in Windows training results")
        merged = Path(directory) / "merged.profdata"
        subprocess.run(
            [str(profdata()), "merge", "-o", str(merged), *raw_files], check=True
        )
        check_profile(merged)
        destination = Path(".pgo/salty-windows-x86_64.profdata")
        destination.parent.mkdir(parents=True, exist_ok=True)
        metadata["profile_sha256"] = digest(merged)
        metadata["training_result"] = training
        shutil.copyfile(merged, destination)
        destination.with_suffix(".json").write_text(
            json.dumps(metadata, indent=2) + "\n"
        )
        print(destination)


def verify_release() -> None:
    path = Path(".pgo/salty-windows-x86_64.profdata")
    metadata = json.loads(path.with_suffix(".json").read_text())
    verify(metadata, expected())
    if digest(path) != metadata.get("profile_sha256"):
        raise ValueError("Windows PGO profile checksum mismatch")
    check_profile(path)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("bundle", "import", "verify", "check"))
    parser.add_argument("path", nargs="?", type=Path)
    args = parser.parse_args()
    if args.action == "verify":
        verify_release()
    elif args.path is None:
        parser.error("path is required")
    elif args.action == "bundle":
        bundle(args.path)
    elif args.action == "import":
        import_results(args.path)
    else:
        check_profile(args.path)


if __name__ == "__main__":
    main()
