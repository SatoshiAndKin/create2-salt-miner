"""Measure five alternating pairs without overlapping GPU workloads."""

import argparse
import hashlib
import json
import statistics
import subprocess
import time
from pathlib import Path


def measure(binary: Path, arguments: list[str]) -> dict:
    start = time.perf_counter()
    result = subprocess.run(
        [str(binary.resolve()), *arguments], capture_output=True, text=True, check=False
    )
    wall = time.perf_counter() - start
    if result.returncode not in (0, 2):
        raise RuntimeError(result.stderr)
    metrics = {}
    for line in result.stdout.splitlines():
        if line.startswith("METRIC "):
            name, value = line.removeprefix("METRIC ").split("=", 1)
            metrics[name] = float(value)
    return {
        "arguments": arguments,
        "wall_seconds": wall,
        "exit_code": result.returncode,
        "stdout": result.stdout,
        "stderr": result.stderr,
        "metrics": metrics,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    inputs = [
        "--factory",
        "0x0000000000FFe8B47B3e2130213B802212439497",
        "--caller",
        "0x0000000000000000000000000000000000000000",
        "--codehash",
        "0x64e604787cbf194841e7b68d7cd28786f6c9a0a3ab9f8b0a0e87cb4387ab0107",
        "--worksize",
        "71303168",
    ]
    binaries = {"baseline": args.baseline, "candidate": args.candidate}
    data = {
        "binaries": {
            name: {
                "path": str(path),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
            for name, path in binaries.items()
        },
        "pairs": [],
        "mining": {},
        "startup": {},
    }
    for pair in range(5):
        runs = {}
        for name in (
            ["baseline", "candidate"] if pair % 2 == 0 else ["candidate", "baseline"]
        ):
            runs[name] = measure(
                binaries[name],
                ["bench", *inputs, "--warmup-batches", "2", "--batches", "8"],
            )
        data["pairs"].append(runs)
    rates = {
        name: [pair[name]["metrics"]["attempts_per_sec"] for pair in data["pairs"]]
        for name in binaries
    }
    gains = [
        c / b - 1 for b, c in zip(rates["baseline"], rates["candidate"], strict=True)
    ]
    data["summary"] = {
        "baseline_median": statistics.median(rates["baseline"]),
        "candidate_median": statistics.median(rates["candidate"]),
        "median_paired_gain": statistics.median(gains),
        "baseline_range_fraction": (max(rates["baseline"]) - min(rates["baseline"]))
        / statistics.median(rates["baseline"]),
    }
    for name, binary in binaries.items():
        startup = measure(
            binary, ["bench", *inputs, "--warmup-batches", "0", "--batches", "1"]
        )
        startup["untimed_wall_seconds"] = (
            startup["wall_seconds"] - 71303168 / startup["metrics"]["attempts_per_sec"]
        )
        data["startup"][name] = startup
        data["mining"][name] = []
        for target in (1, 21):
            run = measure(
                binary,
                [
                    "mine",
                    *inputs,
                    "--zeros",
                    str(target),
                    "--min-runtime-secs",
                    "2",
                    "--max-runtime-secs",
                    "3",
                    "--abi",
                ],
            )
            expected_exit = 0 if target == 1 else 2
            if run["exit_code"] != expected_exit:
                raise RuntimeError(f"Incorrect mining exit code: {run}")
            if len(bytes.fromhex(run["stdout"].strip().removeprefix("0x"))) != 96:
                raise RuntimeError("Incorrect ABI output size")
            data["mining"][name].append(run)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(data, indent=2) + "\n")
    print(json.dumps(data["summary"]))


if __name__ == "__main__":
    main()
