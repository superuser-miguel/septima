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

# @OUTPUT@ may be relative to the build dir (the current CWD), so resolve it to
# an absolute path before changing directory.
output="$(realpath -m "$output")"

# Run from the manifest's directory so the vendored-sources `directory =
# "cargo/vendor"` in $CARGO_HOME/config resolves correctly during the Flatpak
# (offline) build. manifest/target_dir are absolute, so this is safe on the host
# too, where cargo just uses the default registry.
cd "$(dirname "$manifest")"

cargo build --manifest-path "$manifest" --target-dir "$target_dir" --package septima-gtk "$@"
cp "$target_dir/$profile_dir/septima" "$output"
