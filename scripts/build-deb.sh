#!/usr/bin/env bash
# Builds target/debian/kollate_<version>_<arch>.deb (app, CLI, WordNet).
set -euo pipefail
cd "$(dirname "$0")/.."

[ -f data/dictionaries/oewn-2025.db ] || ./scripts/fetch-wordnet.sh
cargo build --release --workspace
cargo deb -p kollate --no-build "$@"
