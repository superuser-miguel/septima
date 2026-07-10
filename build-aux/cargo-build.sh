#!/bin/sh
# Wrapper so Meson can build the Rust binary with cargo and stage it as the
# custom_target output. Environment (CARGO_HOME, SEPTIMA_*) is provided by Meson.
#
# Args: <manifest-path> <target-dir> <profile-dir> <output> [extra cargo args...]
set -eu

manifest="$1"
target_dir="$2"
profile_dir="$3"
output="$4"
shift 4

cargo build --manifest-path "$manifest" --target-dir "$target_dir" --package septima-gtk "$@"
cp "$target_dir/$profile_dir/septima" "$output"
