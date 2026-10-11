#!/usr/bin/env bash
# Measure Vibey's startup time and its own memory (PSS of the vibey pid only; shells and
# other processes inside its terminals are reported separately).
#
# usage: scripts/measure.sh [project-dir] [file-to-open]
set -euo pipefail

BIN="${BIN:-$(dirname "$0")/../target/release/vibey}"
DIR="${1:-$(dirname "$0")/..}"
OPEN="${2:-src/codeview.rs}"
SETTLE="${SETTLE:-6}"

pss_kb() { awk '/^Pss:/ {print $2}' "/proc/$1/smaps_rollup" 2>/dev/null || echo 0; }
rss_kb() { awk '/^Rss:/ {print $2}' "/proc/$1/smaps_rollup" 2>/dev/null || echo 0; }
children_pss_kb() {
  local total=0 pid
  for pid in $(pgrep -P "$1" || true); do
    total=$(( total + $(pss_kb "$pid") ))
  done
  echo "$total"
}

echo "== startup (first frame painted, warm cache, 5 runs) =="
for _ in 1 2 3 4 5; do
  "$BIN" "$DIR" --open "$OPEN" --measure-startup --exit-after-startup 2>&1 | grep -o '[0-9]* ms'
done | sort -n | awk '{a[NR]=$1} END {printf "min %d ms, median %d ms, max %d ms\n", a[1], a[3], a[5]}'

echo
echo "== memory after ${SETTLE}s settle =="
printf "%-8s %-6s %10s %10s %14s\n" renderer terms "PSS MB" "RSS MB" "shells PSS MB"
for renderer in ngl gl cairo; do
  for terms in 2 6; do
    GSK_RENDERER=$renderer "$BIN" "$DIR" --open "$OPEN" --terminals "$terms" >/dev/null 2>&1 &
    pid=$!
    sleep "$SETTLE"
    printf "%-8s %-6s %10.1f %10.1f %14.1f\n" "$renderer" "$terms" \
      "$(echo "$(pss_kb $pid) / 1024" | bc -l)" \
      "$(echo "$(rss_kb $pid) / 1024" | bc -l)" \
      "$(echo "$(children_pss_kb $pid) / 1024" | bc -l)"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
done
