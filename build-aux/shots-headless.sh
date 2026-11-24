#!/bin/sh
# Render Septima's main views to PNGs on a display nobody is looking at.
#
# Builds the app with the dev-only `shots` feature (src/shots.rs), then runs it
# on a headless mutter with one virtual monitor and a private session bus, so
# no window ever appears on the real screen. The app renders each view itself,
# in light and dark, and quits.
#
# Usage:  build-aux/shots-headless.sh [OUT_DIR]     (default: target/shots)
# Copied from Foresight's selftest-headless.sh; same cleanup gotchas apply.
set -eu

HERE="$(cd "$(dirname "$0")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$HERE/target}"
OUT="${1:-$TARGET_DIR/shots}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

(cd "$HERE" && cargo build --quiet -p septima-gtk --features shots)
BIN="$TARGET_DIR/debug/septima"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }
command -v mutter >/dev/null || { echo "mutter not found" >&2; exit 1; }

WORK="$(mktemp -d "$TARGET_DIR/shots-run.XXXXXX")"
# The runtime dir holds the compositor's socket, and a Unix socket path must fit
# in 108 bytes, so it gets a short path of its own rather than one under $WORK.
RUN="$(mktemp -d /tmp/septima-shots.XXXXXX)"
mkdir -p "$WORK/cfg" "$WORK/data"
chmod 700 "$RUN"
cleanup() {
    for mount in "$RUN/doc" "$RUN/gvfs"; do
        fusermount3 -uz "$mount" 2>/dev/null || true
    done
    n=0
    until rm -rf "$WORK" "$RUN" 2>/dev/null || [ "$n" -ge 30 ]; do
        n=$((n + 1)); sleep 0.1
    done
}
trap cleanup EXIT

export XDG_RUNTIME_DIR="$RUN" XDG_CONFIG_HOME="$WORK/cfg" XDG_DATA_HOME="$WORK/data"
# A small archive to open, so the file list is in the shots.
mkdir -p "$WORK/sample/photos" "$WORK/sample/notes"
printf 'hello\n' >"$WORK/sample/notes/readme.txt"
printf 'x%.0s' $(seq 1 2000) >"$WORK/sample/photos/beach.jpg"
printf 'y%.0s' $(seq 1 900) >"$WORK/sample/photos/sunset.jpg"
(cd "$WORK/sample" && 7zz a -bso0 -bsp0 -t7z -m0=zstd "$WORK/sample.7z" photos notes)
export SEPTIMA_SHOTS_ARCHIVE="$WORK/sample.7z"
# A folder that already holds the archive's files, for the extract-clash shots.
mkdir -p "$WORK/Downloads"
7zz x -bso0 -bsp0 -y -o"$WORK/Downloads" "$WORK/sample.7z"
export SEPTIMA_SHOTS_DEST="$WORK/Downloads"
# Sixteen archives that each take a second or two to extract, for the batch
# shots. One real archive under sixteen names (hard links), so it costs the
# disk space of one.
mkdir -p "$WORK/batch" "$WORK/batch-src"
head -c 110000000 /dev/urandom | base64 >"$WORK/batch-src/payload.txt"
7zz a -bso0 -bsp0 -t7z -m0=lzma2 -mx=1 "$WORK/batch/holiday-photos-01.7z" "$WORK/batch-src/payload.txt"
rm -rf "$WORK/batch-src"
for n in 02 03 04 05 06 07 08 09 10 11 12 13 14 15 16; do
    ln "$WORK/batch/holiday-photos-01.7z" "$WORK/batch/holiday-photos-$n.7z"
done
export SEPTIMA_SHOTS_BATCH="$WORK/batch"
export SEPTIMA_SHOTS_DIR="$OUT" SEPTIMA_BIN="$BIN" GTK_A11Y=none
unset DISPLAY WAYLAND_DISPLAY

status=0
# shellcheck disable=SC2016  # expanded by the inner shell, on purpose
dbus-run-session -- sh -c '
    mutter --headless --no-x11 --wayland-display=septima-shots \
        --virtual-monitor 1280x800 >"$XDG_RUNTIME_DIR/mutter.log" 2>&1 &
    compositor=$!
    n=0
    while [ ! -S "$XDG_RUNTIME_DIR/septima-shots" ]; do
        n=$((n + 1))
        if [ "$n" -gt 100 ] || ! kill -0 "$compositor" 2>/dev/null; then
            echo "the headless compositor did not start:" >&2
            cat "$XDG_RUNTIME_DIR/mutter.log" >&2
            exit 1
        fi
        sleep 0.1
    done
    status=0
    GDK_BACKEND=wayland WAYLAND_DISPLAY=septima-shots timeout 60 "$SEPTIMA_BIN" || status=$?
    kill "$compositor" 2>/dev/null
    wait "$compositor" 2>/dev/null
    exit "$status"
' || status=$?
echo "shots in $OUT"
exit "$status"
