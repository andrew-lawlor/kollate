#!/usr/bin/env bash
# Builds and installs the Flatpak for the current user, and writes a
# single-file bundle to target/flatpak/kollate.flatpak.
# Needs: flatpak install --user flathub org.gnome.Sdk//51 \
#          org.freedesktop.Sdk.Extension.rust-stable//26.08 org.flatpak.Builder
set -euo pipefail
cd "$(dirname "$0")/.."

APP_ID=io.github.andrew_lawlor.Kollate
flatpak run org.flatpak.Builder --user --install --force-clean --install-deps-from=flathub \
    --state-dir=.flatpak-builder --repo=flatpak/repo flatpak/build-dir "flatpak/$APP_ID.yml"
mkdir -p target/flatpak
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    flatpak/repo target/flatpak/kollate.flatpak "$APP_ID"
echo "Bundle: target/flatpak/kollate.flatpak"
