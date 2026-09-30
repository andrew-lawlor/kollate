#!/usr/bin/env bash
# Writes the files for the Flathub repository into target/flathub/: the
# manifest, building from a release tag on GitHub rather than this folder,
# the pinned Cargo sources, and flathub.json (x86_64 only: the handwriting
# engine is untested on aarch64).
#
#   scripts/flathub-manifest.sh v0.5.1
set -euo pipefail

cd "$(dirname "$0")/.."
TAG="${1:?usage: $0 <tag>}"
COMMIT="$(git rev-list -n 1 "$TAG")"
OUT=target/flathub
mkdir -p "$OUT"

python3 - "$TAG" "$COMMIT" "$OUT" <<'PY'
import sys
tag, commit, out = sys.argv[1:]
src = open("flatpak/io.github.andrew_lawlor.Kollate.yml").read()
local = """      - type: dir
        path: ..
        skip:
          - target
          - data
          - .flatpak-builder
          - flatpak/build-dir
          - flatpak/repo
"""
assert local in src, "the manifest's local source changed; update this script"
git = f"""      - type: git
        url: https://github.com/andrew-lawlor/kollate.git
        tag: {tag}
        commit: {commit}
"""
open(f"{out}/io.github.andrew_lawlor.Kollate.yml", "w").write(src.replace(local, git))
PY
cp flatpak/cargo-sources.json "$OUT/"
printf '{\n  "only-arches": ["x86_64"]\n}\n' > "$OUT/flathub.json"
echo "Wrote $OUT/ for $TAG ($COMMIT)"
