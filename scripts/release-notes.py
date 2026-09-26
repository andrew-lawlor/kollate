#!/usr/bin/env python3
"""Prints the GitHub release notes for a version, from the metainfo file.

Fails unless the version matches Cargo.toml and has an entry at the top of
the metainfo <releases>, so a tag can't be released with stale metadata.

Usage: scripts/release-notes.py 0.1.5
"""
import sys
import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path

root = Path(__file__).resolve().parent.parent
version = sys.argv[1].removeprefix("v")


def fail(message):
    print(f"release-notes: {message}", file=sys.stderr)
    sys.exit(1)


cargo = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
if cargo != version:
    fail(f"Cargo.toml says {cargo}, not {version}")

metainfo = ET.parse(root / "crates/kollate/data/io.github.andrew_lawlor.Kollate.metainfo.xml")
release = metainfo.find("releases/release")
if release is None or release.get("version") != version:
    fail(f"the newest metainfo release isn't {version}")


def text(el):
    return " ".join("".join(el.itertext()).split())


notes = []
for el in release.find("description"):
    if el.tag == "p":
        notes += [text(el), ""]
    elif el.tag == "ul":
        notes += [f"- {text(li)}" for li in el.findall("li")] + [""]

deb = f"kollate_{version}-1_amd64.deb"
print(f"""## What's new

{chr(10).join(notes).strip()}

## Downloads

| File | For |
|---|---|
| `kollate.flatpak` | Any distribution: `flatpak install --user kollate.flatpak`. Read-only access to your Kobo, no network. |
| `{deb}` | Debian 13 or Ubuntu 24.04 and newer: `sudo apt install ./{deb}` |
| `SHA256SUMS` | Checksums |

Upgrading keeps your library.

Built by [GitHub Actions](https://github.com/andrew-lawlor/kollate/actions) from the tagged commit. To check a download came from this repository: `gh attestation verify <file> --repo andrew-lawlor/kollate`.""")
