#!/usr/bin/env bash
# CI check: mouse camera in the sandbox window. Takes an F12 snapshot, zooms with the wheel,
# drags with the left button (static SVG scene), takes a second snapshot and expects two different PNGs and
# no Vulkan validation errors.
# Usage: scripts/ci/camera-check.sh <sandbox binary>
set -euo pipefail

BIN=$1
DIR="$(dirname "$BIN")/snapshots"
rm -rf "$DIR"
LOG=$(mktemp)
"$BIN" --validation --scene svg --seconds 9 >"$LOG" 2>&1 &
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
sleep 2
xdotool windowfocus "$win" 2>/dev/null || true
# Hide the panel so the mouse is over the scene, then take the first snapshot.
xdotool key F1
sleep 0.3
xdotool key F12
sleep 0.8
# Zoom in around a point right of centre: five wheel clicks up.
xdotool mousemove 900 300
for _ in 1 2 3 4 5; do
  xdotool click 4
  sleep 0.05
done
# Drag the scene left and up.
xdotool mousedown 1
for step in 1 2 3 4 5 6 7 8 9 10; do
  xdotool mousemove $((900 - step * 20)) $((300 - step * 10))
  sleep 0.03
done
xdotool mouseup 1
sleep 0.5
xdotool key F12
status=0
wait "$pid" || status=$?
if [ "$status" -ne 0 ]; then
  echo "sandbox exited with $status"
  cat "$LOG"
  exit 1
fi
if grep -aq "Validation Error" "$LOG"; then
  echo "Vulkan validation errors"
  grep -a "Validation Error" "$LOG" | head -5
  exit 1
fi
mapfile -t shots < <(ls "$DIR"/*.png 2>/dev/null)
if [ "${#shots[@]}" -lt 2 ]; then
  echo "expected two snapshots in $DIR, got ${#shots[@]}"
  cat "$LOG"
  exit 1
fi
if cmp -s "${shots[0]}" "${shots[1]}"; then
  echo "camera did not move: snapshots are identical"
  exit 1
fi
echo "OK: ${shots[0]} ${shots[1]}"
