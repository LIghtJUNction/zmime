#!/usr/bin/env python3
"""Run fixed-source differential checks and paired benchmarks, then record context."""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_hashes(root):
    """Include untracked new source files and the exact experiment protocol."""
    paths = [root / name for name in (
        "Cargo.toml", "Cargo.lock", "comparison/Cargo.toml", "comparison/Cargo.lock",
        "comparison/run.py", "comparison/summarize.py",
    )]
    for directory in ("src", "tests", "comparison/src", "comparison/tests"):
        paths.extend((root / directory).rglob("*.rs"))
    return {str(path.relative_to(root)): sha256(path) for path in sorted(paths)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("comparison/results/local"))
    parser.add_argument("--samples", type=int, default=9)
    parser.add_argument("--target-ms", type=float, default=20)
    parser.add_argument("--filter", default="")
    parser.add_argument("--portable", action="store_true")
    parser.add_argument("--cpu", type=int)
    args = parser.parse_args()
    directory = Path(__file__).resolve().parent
    root = directory.parent
    cargo = shutil.which("cargo")
    if not cargo:
        raise SystemExit("cargo must be on PATH")
    rustc = shutil.which("rustc")
    if not rustc:
        raise SystemExit("rustc must be on PATH")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source_before = source_hashes(root)
    common = ["--locked", "--manifest-path", str(directory / "Cargo.toml")]
    if args.portable:
        common.append("--no-default-features")
    subprocess.run([cargo, "test", *common], cwd=root, check=True)
    subprocess.run([cargo, "build", "--release", *common], cwd=root, check=True)
    if hasattr(os, "sched_getaffinity"):
        allowed = sorted(os.sched_getaffinity(0))
        selected = args.cpu if args.cpu is not None else allowed[0]
        if selected not in allowed:
            raise SystemExit(f"CPU {selected} is outside the allowed set {allowed}")
        os.sched_setaffinity(0, {selected})
    else:
        allowed, selected = None, None
        if args.cpu is not None:
            raise SystemExit("CPU affinity is unavailable on this platform")
    cpu_info = {}
    if Path("/proc/cpuinfo").exists():
        for line in Path("/proc/cpuinfo").read_text().split("\n\n", 1)[0].splitlines():
            if ":" in line:
                key, value = line.split(":", 1)
                if key.strip() in {"model name", "vendor_id", "flags", "cpu family", "model", "stepping"}:
                    cpu_info[key.strip()] = value.strip()
    def capture(command):
        return subprocess.check_output(command, cwd=root, text=True).strip()
    context = {
        "protocol_version": 2,
        "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "system": platform.system(),
        "kernel": platform.release(),
        "architecture": platform.machine(),
        "cpu": cpu_info,
        "allowed_cpus": allowed,
        "selected_cpu": selected,
        "rustc": capture([rustc, "-vV"]),
        "cargo": capture([cargo, "--version"]),
        "git_commit": capture(["git", "rev-parse", "HEAD"]),
        "tracked_modifications": capture(["git", "status", "--short", "--untracked-files=no"]),
        "source_sha256": source_before,
        "rustflags": os.environ.get("RUSTFLAGS", ""),
        "features": "portable (--no-default-features)" if args.portable else "default simd",
        "mail_parser": "1.0.1, default-features=false, transitive versions in Cargo.lock",
        "profile": "release; lto=thin; codegen-units=1; default target CPU",
        "samples": args.samples,
        "target_ms": args.target_ms,
        "filter": args.filter,
        "warmup_iterations_per_pair": 32,
        "sample_order": "alternating zmime/reference then reference/zmime",
        "timing_scope": "parse+consume every raw header/body view and supported MIME metadata including all unescaped parameter values; caller storage prepared outside timing; no transfer/charset decode during parse",
    }
    cargo_metadata = json.loads(capture([
        cargo, "metadata", "--format-version", "1", "--no-deps", "--offline",
        "--manifest-path", str(directory / "Cargo.toml"),
    ]))
    binary = Path(cargo_metadata["target_directory"]) / "release" / ("compare.exe" if os.name == "nt" else "compare")
    cold_binary = binary.with_name("cold.exe" if os.name == "nt" else "cold")
    context["binary_sha256"] = {path.name: sha256(path) for path in (binary, cold_binary)}
    with tempfile.TemporaryDirectory(prefix="zmime-corpus-") as temporary:
        subprocess.run([str(binary), "--dump-corpus", temporary], cwd=root, check=True)
        context["fixture_sha256"] = {
            path.name: {"bytes": path.stat().st_size, "sha256": sha256(path)}
            for path in sorted(Path(temporary).glob("*.eml"))
        }
    command = [str(binary), "--samples", str(args.samples), "--target-ms", str(args.target_ms)]
    if args.filter:
        command += ["--filter", args.filter]
    with (output / "raw.csv").open("w", newline="") as destination:
        subprocess.run(command, cwd=root, stdout=destination, check=True)
    with (output / "summary.csv").open("w", newline="") as destination:
        subprocess.run([sys.executable, str(directory / "summarize.py"), str(output / "raw.csv")], stdout=destination, check=True)
    with (output / "cold.csv").open("w", newline="") as destination:
        destination.write("sample,engine,input_bytes,first_call_ns,warmed_ns\n")
        for size in (1024, 65536, 1048576):
            for sample in range(args.samples):
                for engine in ("zmime", "mail-parser-1.0.1"):
                    observation = subprocess.check_output([str(cold_binary), engine, str(size)], cwd=root, text=True)
                    destination.write(f"{sample},{observation}")
    context["finished_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    context["source_unchanged"] = source_before == source_hashes(root)
    context["binaries_unchanged"] = context["binary_sha256"] == {
        path.name: sha256(path) for path in (binary, cold_binary)
    }
    (output / "environment.json").write_text(json.dumps(context, indent=2) + "\n", encoding="utf-8")
    if not context["source_unchanged"] or not context["binaries_unchanged"]:
        raise SystemExit("Source or benchmark binaries changed during collection; discard this run and repeat with a stable checkout")
    print(output)


if __name__ == "__main__":
    main()
