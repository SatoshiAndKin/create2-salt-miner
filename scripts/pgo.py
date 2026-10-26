"""Validate executed Linux PGO profiles (Python 3.9+)."""

import argparse
import subprocess
from pathlib import Path


def output(*args: str) -> str:
    return subprocess.check_output(args, text=True).strip()


def profdata() -> Path:
    tool = (
        Path(output("rustc", "--print", "target-libdir")).parent / "bin/llvm-profdata"
    )
    if not tool.is_file():
        raise ValueError("Install llvm-tools-preview for the pinned Rust toolchain")
    return tool


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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", type=Path)
    args = parser.parse_args()
    check_profile(args.path)


if __name__ == "__main__":
    main()
