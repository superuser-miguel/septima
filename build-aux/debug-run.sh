#!/usr/bin/env bash
#
# debug-run.sh — launch Septima (Devel) with the engine job trace enabled.
#
# Sets SEPTIMA_DEBUG=1 so the engine prints each 7zz job's lifecycle: the command
# it runs (passwords redacted), the pid, and — crucial for cancel bugs — exactly
# when a cancel flag is noticed and the process is killed. Output is filtered to
# the interesting lines, shown live, and tee'd to a timestamped log in
# Troubleshooting/ so you can share it.
#
#   build-aux/debug-run.sh [ARCHIVE]             # full trace (job + panics + GTK criticals)
#   build-aux/debug-run.sh --interact [ARCHIVE]  # job lifecycle only (drop GTK noise)
#
# Then reproduce the issue (e.g. create a big archive and hit Cancel) and watch
# the trace; close the app or Ctrl-C to stop. Needs the Devel Flatpak built from
# the current code (flatpak-builder ... Septima.Devel.json).
set -uo pipefail

APP=io.github.superuser_miguel.Septima.Devel
REPO="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
LOGDIR="${SEPTIMA_LOGDIR:-$REPO/Troubleshooting}"
mkdir -p "$LOGDIR"
LOG="$LOGDIR/septima-debug_$(date +%Y-%m-%d_%H-%M-%S).log"
RUNLOG="$(mktemp)"

MODE="full"
KEEP='\[septima\]|panic|CRITICAL|WARNING'
ARGS=()
for a in "$@"; do
  case "$a" in
    --interact) KEEP='\[septima\]|panic'; MODE="interact" ;;
    -h|--help)  sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)          ARGS+=("$a") ;;
  esac
done

echo "Septima debug trace  (mode: $MODE)"
echo "  app: $APP"
echo "  log: $LOG"
echo "  → reproduce the issue (create a big archive, hit Cancel); Ctrl-C or close to stop."
echo

{ echo; echo "===== run $(date)  mode=$MODE  args: ${ARGS[*]:-(none)} ====="; } >> "$LOG"

# Merge stderr, keep only the trace lines, show them live AND append to both logs.
flatpak run --env=SEPTIMA_DEBUG=1 "$APP" "${ARGS[@]}" 2>&1 \
  | grep --line-buffered -E "$KEEP" \
  | tee -a "$LOG" "$RUNLOG" || true

count() { grep -c -- "$1" "$RUNLOG" 2>/dev/null || echo 0; }

echo
echo "=== job summary (this run) ==="
printf "  7zz jobs started : %s\n" "$(count '7zz started')"
printf "  cancels noticed  : %s\n" "$(count 'cancel flag set')"
printf "  processes killed : %s\n" "$(count 'killed')"
printf "  panics           : %s\n" "$(count 'panic')"

# The whole point of the trace for the cancel bug: a cancel must be followed by a kill.
started=$(count 'cancel flag set'); killed=$(count 'killed')
if [ "$started" -gt "$killed" ]; then
  echo
  echo "  ⚠  A cancel was requested but no kill followed — the job may be stuck."
fi

echo "  (full log: $LOG — share it or let Claude read it)"
rm -f "$RUNLOG"
