#!/usr/bin/env python3
"""Checks that flatpak/cargo-sources.json matches Cargo.lock.

The Flatpak builds offline from the crates listed in cargo-sources.json, so
every crates.io package in Cargo.lock must be there with the same checksum.
If this fails, regenerate the file with flatpak-cargo-generator (see README).
"""
import json
import sys
import tomllib
from pathlib import Path

root = Path(__file__).resolve().parent.parent
lock = tomllib.loads((root / "Cargo.lock").read_text())
sources = json.loads((root / "flatpak/cargo-sources.json").read_text())

vendored = {
    s["dest"].removeprefix("cargo/vendor/"): s["sha256"]
    for s in sources
    if s.get("type") == "archive"
}
problems = []
for pkg in lock["package"]:
    if "checksum" not in pkg:
        continue  # a workspace crate
    key = f"{pkg['name']}-{pkg['version']}"
    if key not in vendored:
        problems.append(f"missing: {key}")
    elif vendored[key] != pkg["checksum"]:
        problems.append(f"checksum differs: {key}")

if problems:
    print("flatpak/cargo-sources.json is out of date with Cargo.lock:", file=sys.stderr)
    for p in problems:
        print(f"  {p}", file=sys.stderr)
    sys.exit(1)
print(f"cargo-sources.json matches Cargo.lock ({len(vendored)} crates)")
