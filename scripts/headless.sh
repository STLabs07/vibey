#!/usr/bin/env bash
# Run Vibey off-screen on GTK's Broadway backend with a self-test script, so GUI behavior
# can be checked without opening windows or sending input on a real desktop.
#
#   scripts/headless.sh "wait 800; key ctrl+p; type main; shot /tmp/q.png; quit" [projects…]
#
# Needs gtk4-broadwayd (package libgtk-4-bin, or set BROADWAYD=/path/to/gtk4-broadwayd).
set -euo pipefail
SCRIPT="$1"; shift
BIN="${BIN:-$(dirname "$0")/../target/release/vibey}"
BROADWAYD="${BROADWAYD:-$(command -v gtk4-broadwayd || true)}"
DISPLAY_NUM="${BROADWAY_NUM:-17}"
[ -x "$BROADWAYD" ] || { echo "gtk4-broadwayd not found (apt install libgtk-4-bin, or set BROADWAYD)"; exit 1; }
if ! pgrep -x gtk4-broadwayd >/dev/null; then
  "$BROADWAYD" --address 127.0.0.1 ":$DISPLAY_NUM" >/dev/null 2>&1 &
  sleep 0.5
fi
env -u DISPLAY -u WAYLAND_DISPLAY GDK_BACKEND=broadway BROADWAY_DISPLAY=":$DISPLAY_NUM" \
  VIBEY_SELFTEST="$SCRIPT" timeout 120 "$BIN" "$@"
