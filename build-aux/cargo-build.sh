#!/bin/sh
# Usage: cargo-build.sh <cargo> <manifest> <target-dir> <debug|release> <offline|online> <output>
set -e
cargo="$1"; manifest="$2"; target="$3"; profile="$4"; mode="$5"; output="$6"
args="build --manifest-path $manifest --target-dir $target --bin stempeluhr"
[ "$profile" = release ] && args="$args --release"
[ "$mode" = offline ] && args="$args --offline"
# shellcheck disable=SC2086
"$cargo" $args
cp "$target/$profile/stempeluhr" "$output"
