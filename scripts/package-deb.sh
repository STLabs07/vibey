#!/usr/bin/env bash
# Build target/vibey_<version>_amd64.deb with plain dpkg-deb (no extra tooling).
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
ROOT=target/deb/vibey_${VERSION}_amd64
rm -rf "$ROOT"
install -Dm755 target/release/vibey "$ROOT/usr/bin/vibey"
install -Dm644 packaging/dev.vibey.Vibey.desktop "$ROOT/usr/share/applications/dev.vibey.Vibey.desktop"
install -Dm644 packaging/dev.vibey.Vibey.svg "$ROOT/usr/share/icons/hicolor/scalable/apps/dev.vibey.Vibey.svg"
install -Dm644 LICENSE "$ROOT/usr/share/doc/vibey/copyright"
install -Dm644 third_party/NOTICES.md "$ROOT/usr/share/doc/vibey/NOTICES.md"
SIZE=$(du -sk "$ROOT/usr" | cut -f1)
mkdir -p "$ROOT/DEBIAN"
cat > "$ROOT/DEBIAN/control" <<CONTROL
Package: vibey
Version: $VERSION
Architecture: amd64
Maintainer: Vibey contributors <noreply@users.noreply.github.com>
Installed-Size: $SIZE
Depends: libgtk-4-1 (>= 4.14), libvte-2.91-gtk4-0 (>= 0.76), git
Section: devel
Priority: optional
Description: Read-only, VS Code-shaped window for terminal AI CLIs
 Shows projects the way VS Code does (themes, icons, syntax colors, git
 decorations) and hosts terminals with "finished" notifications, using a
 fraction of VS Code's memory. It is not an editor: the AI edits the files.
CONTROL
dpkg-deb --build --root-owner-group "$ROOT" "target/vibey_${VERSION}_amd64.deb" >/dev/null
echo "Built target/vibey_${VERSION}_amd64.deb"
