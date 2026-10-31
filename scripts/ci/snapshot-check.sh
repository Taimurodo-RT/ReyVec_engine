#!/usr/bin/env bash
# CI check: open the sandbox window, press F12 and expect a PNG in snapshots/ next to the binary.
# Usage: scripts/ci/snapshot-check.sh <sandbox binary>
set -euo pipefail

BIN=$1
DIR="$(dirname "$BIN")/snapshots"
rm -rf "$DIR"
LOG=$(mktemp)
"$BIN" --validation --seconds 6 >"$LOG" 2>&1 &
pid=$!
win=""
for _ in $(seq 1 50); do
  win=$(xdotool search --name ReyVec 2>/dev/null | head -1 || true)
  [ -n "$win" ] && break
  sleep 0.1
done
if [ -z "$win" ]; then
  echo "window did not appear"
  cat "$LOG"
  exit 1
fi
sleep 1.5
xdotool windowfocus "$win" 2>/dev/null || true
xdotool key F12
status=0
wait "$pid" || status=$?
if [ "$status" -ne 0 ]; then
  echo "sandbox exited with $status"
  cat "$LOG"
  exit 1
fi
shot=$(ls "$DIR"/*.png 2>/dev/null | head -1 || true)
if [ -z "$shot" ] || [ ! -s "$shot" ]; then
  echo "no snapshot in $DIR"
  cat "$LOG"
  exit 1
fi
file "$shot" 2>/dev/null || true
grep -a "Снимок кадра" "$LOG" || true
echo "OK: $shot"
