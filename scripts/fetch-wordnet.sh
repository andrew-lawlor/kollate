#!/usr/bin/env bash
# Downloads Open English WordNet 2025 (CC BY 4.0) and builds Kollate's
# bundled dictionary at data/dictionaries/oewn-2025.db.
set -euo pipefail

cd "$(dirname "$0")/.."
URL="https://github.com/globalwordnet/english-wordnet/releases/download/2025-edition/english-wordnet-2025.xml.gz"
SHA256="9ca6d1dcb75f822fdd66617f7d9da48142ace38dd544d6ad5e2feca1674ad3fe"
SRC="data/cache/english-wordnet-2025.xml.gz"
OUT="data/dictionaries/oewn-2025.db"

mkdir -p data/cache data/dictionaries
if ! echo "$SHA256  $SRC" | sha256sum --check --status 2>/dev/null; then
    echo "Downloading $URL"
    curl --fail --location --output "$SRC.part" "$URL"
    echo "$SHA256  $SRC.part" | sha256sum --check
    mv "$SRC.part" "$SRC"
fi
cargo run --quiet --release -p kollate-cli -- dict build-wordnet "$SRC" "$OUT"
echo "Built $OUT"
