"""Measure two independent sets of alternating pairs with a paired gain gate."""

import argparse
import hashlib
import json
import random
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


def paired_gains(pairs: list[dict]) -> list[float]:
    return [
        pair["candidate"]["metrics"]["attempts_per_sec"]
        / pair["baseline"]["metrics"]["attempts_per_sec"]
        - 1
        for pair in pairs
    ]


def gain_gate(sets: list[list[float]]) -> dict:
    if len(sets) != 2 or any(len(values) < 5 for values in sets):
        raise ValueError(
            "The gain gate requires two sets with at least five pairs each"
        )
    # Resample whole baseline/candidate pairs within each independent set.
    # Preserve equal set representation, including the reversed starting order.
    rng = random.Random(71303168)
    bootstraps = sorted(
        statistics.median(
            [value for values in sets for value in rng.choices(values, k=len(values))]
        )
        for _ in range(20_000)
    )
    low, high = bootstraps[500], bootstraps[19_499]
    medians = [statistics.median(values) for values in sets]
    return {
        "set_median_paired_gains": medians,
        "median_paired_gain": statistics.median([v for values in sets for v in values]),
        "paired_bootstrap_95_interval": [low, high],
        "bootstrap_samples": 20_000,
        "bootstrap_seed": 71303168,
        "passes_gain_gate": all(value > 0 for value in medians) and low > 0,
    }


def save(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--pairs", type=int, default=5)
    parser.add_argument("--warmup-batches", type=int, default=8)
    parser.add_argument("--batches", type=int, default=32)
    args = parser.parse_args()
    if args.pairs < 5 or args.warmup_batches < 8 or args.batches < 32:
        parser.error(
            "Use at least five pairs, eight warmup batches, and 32 timed batches"
        )
    if args.output.exists():
        parser.error("Output already exists; preserve the earlier measurements")
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
        "protocol": {
            "sets": 2,
            "pairs_per_set": args.pairs,
            "warmup_batches": args.warmup_batches,
            "timed_batches": args.batches,
            "completed_hashes_per_measurement": 71303168 * args.batches,
            "preconditioning_warmup_batches": 64,
            "order": "AB/BA alternation; reverse the starting order in set two",
            "acceptance": "Both set medians positive and pooled paired bootstrap 95% lower bound above zero",
        },
        "sets": [],
        "mining": {},
        "startup": {},
    }
    for set_index in range(2):
        runs = {
            "preconditioning": measure(
                args.baseline,
                ["bench", *inputs, "--warmup-batches", "64", "--batches", "32"],
            ),
            "pairs": [],
        }
        data["sets"].append(runs)
        save(args.output, data)
        for pair in range(args.pairs):
            results = {}
            order = (
                ["baseline", "candidate"]
                if (pair + set_index) % 2 == 0
                else ["candidate", "baseline"]
            )
            for name in order:
                results[name] = measure(
                    binaries[name],
                    [
                        "bench",
                        *inputs,
                        "--warmup-batches",
                        str(args.warmup_batches),
                        "--batches",
                        str(args.batches),
                    ],
                )
            runs["pairs"].append(results)
            save(args.output, data)
            print(
                json.dumps(
                    {
                        "set": set_index + 1,
                        "pair": pair + 1,
                        "gain": paired_gains([results])[0],
                    }
                ),
                flush=True,
            )
        print(
            json.dumps(
                {
                    "set": set_index + 1,
                    "median_gain": statistics.median(paired_gains(runs["pairs"])),
                }
            ),
            flush=True,
        )
    data["summary"] = gain_gate([paired_gains(runs["pairs"]) for runs in data["sets"]])
    save(args.output, data)
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
    data["completed"] = True
    save(args.output, data)
    print(json.dumps(data["summary"]), flush=True)


if __name__ == "__main__":
    main()
