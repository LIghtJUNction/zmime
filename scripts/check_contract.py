#!/usr/bin/env python3
"""Check the published crate's dependency and no_std contract using Cargo metadata."""

import json
from pathlib import Path
import subprocess


def main():
    root = Path(__file__).resolve().parents[1]
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--no-deps"],
        cwd=root, text=True,
    ))
    packages = metadata["packages"]
    if len(packages) != 1 or packages[0]["name"] != "zmime":
        raise SystemExit("The publication workspace must contain only zmime")
    package = packages[0]
    if package["dependencies"]:
        raise SystemExit("No dependencies are allowed, including dev/build/optional/target dependencies")
    if package["rust_version"] != "1.74":
        raise SystemExit("Update the MSRV matrix and contract together with rust-version")
    if (root / "build.rs").exists() or any(
        "custom-build" in target["kind"] for target in package["targets"]
    ):
        raise SystemExit("Build scripts are prohibited")
    if "#![no_std]" not in (root / "src/lib.rs").read_text():
        raise SystemExit("The published library must unconditionally declare #![no_std]")
    files = subprocess.check_output(
        ["cargo", "package", "--list", "--locked", "--allow-dirty"],
        cwd=root, text=True,
    ).splitlines()
    forbidden = [name for name in files if name.startswith(("comparison/", "scripts/", ".github/"))]
    if forbidden:
        raise SystemExit("Development-only files leaked into the package: " + ", ".join(forbidden))
    print("Verified: zero dependencies, no build script, no_std, declared MSRV, isolated comparison project")


if __name__ == "__main__":
    main()
