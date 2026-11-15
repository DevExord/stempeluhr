#!/bin/sh
# Regenerates cargo-sources.json (vendored crates for the offline Flatpak
# build) from Cargo.lock, using flatpak-cargo-generator from
# https://github.com/flatpak/flatpak-builder-tools (cargo/).
set -e
cd "$(dirname "$0")/.."
gen="${FLATPAK_CARGO_GENERATOR:-flatpak-cargo-generator.py}"
python3 "$gen" Cargo.lock -o build-aux/cargo-sources.json
