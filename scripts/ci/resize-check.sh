#!/usr/bin/env bash
# CI check: open the sandbox under a virtual display with Vulkan validation and keep
# resizing its window. Every swapchain recreation must stay free of validation errors.
# Runs several times because frame timing differs between runs.
# Usage: scripts/ci/resize-check.sh <sandbox binary> [runs]
set -euo pipefail

BIN=$1
RUNS=${2:-5}
LOG_DIR=$(mktemp -d)

for run in $(seq 1 "$RUNS"); do
  log="$LOG_DIR/run$run.log"
  "$BIN" --validation --seconds 5 >"$log" 2>&1 &
  pid=$!
  win=""
  for _ in $(seq 1 50); do
    win=$(xdotool search --name ReyVec 2>/dev/null | head -1 || true)
    [ -n "$win" ] && break
    sleep 0.1
  done
  if [ -z "$win" ]; then
    echo "run $run: window did not appear"
    cat "$log"
    exit 1
  fi
  sleep 1
  for i in $(seq 1 25); do
    xdotool windowsize "$win" $((700 + (i % 10) * 60)) $((400 + (i % 7) * 40)) 2>/dev/null || true
    sleep 0.05
  done
  if ! wait "$pid"; then
    echo "run $run: sandbox failed"
    cat "$log"
    exit 1
  fi
  echo "run $run: ok ($(grep -o 'Готово.*' "$log" | head -1))"
done
