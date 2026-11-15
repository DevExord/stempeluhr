#!/bin/sh
# Runs the build-directory binary against the real session (needs `meson compile -C build`).
# Extra env: RUST_LOG, STEMPELUHR_SCREENSHOT=/path/prefix (debug builds).
root="$(cd "$(dirname "$0")/.." && pwd)"
GSETTINGS_SCHEMA_DIR="$root/build/data" \
STEMPELUHR_RESOURCES_FILE="$root/build/src/stempeluhr.gresource" \
STEMPELUHR_LOCALEDIR="${STEMPELUHR_LOCALEDIR:-$root/install/share/locale}" \
exec "$root/build/src/stempeluhr" "$@"
