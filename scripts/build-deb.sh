#!/usr/bin/env bash
# Builds target/debian/kollate_<version>_<arch>.deb (app, CLI, English Wiktionary).
set -euo pipefail
cd "$(dirname "$0")/.."

[ -f data/dictionaries/wiktionary-en.db ] || ./scripts/fetch-dictionary.sh
cargo build --release --workspace
cargo deb -p kollate --no-build "$@"
