#!/usr/bin/env bash
# Build and install Vibey for the current user (no sudo): ~/.local/bin/vibey + menu entry.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release
install -Dm755 target/release/vibey "$HOME/.local/bin/vibey"
install -Dm644 packaging/dev.vibey.Vibey.desktop "$HOME/.local/share/applications/dev.vibey.Vibey.desktop"
install -Dm644 packaging/dev.vibey.Vibey.svg "$HOME/.local/share/icons/hicolor/scalable/apps/dev.vibey.Vibey.svg"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "Installed: $HOME/.local/bin/vibey  (run 'vibey .' in any project)"
