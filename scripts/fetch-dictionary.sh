#!/usr/bin/env bash
# Downloads the English Wiktionary (compiled by reader.dict, CC BY-SA 4.0)
# and builds Kollate's bundled dictionary at data/dictionaries/wiktionary-en.db.
# The file is fetched from Kollate's own data release, because the upstream
# URL isn't versioned; the checksum pins the exact copy.
set -euo pipefail

cd "$(dirname "$0")/.."
URL="https://github.com/andrew-lawlor/kollate/releases/download/data-wiktionary-en-2026-09-09/dict-en-en.df.bz2"
SHA256="bd74da67652e66bc6a69a3321adb1de5922e7bbef9daf305fbc44bbc67b5db15"
SRC="data/cache/dict-en-en.df.bz2"
OUT="data/dictionaries/wiktionary-en.db"

mkdir -p data/cache data/dictionaries
if ! echo "$SHA256  $SRC" | sha256sum --check --status 2>/dev/null; then
    echo "Downloading $URL"
    curl --fail --location --output "$SRC.part" "$URL"
    echo "$SHA256  $SRC.part" | sha256sum --check
    mv "$SRC.part" "$SRC"
fi
cargo run --quiet --release -p kollate-cli -- dict build-dictfile "$SRC" "$OUT"
echo "Built $OUT"
