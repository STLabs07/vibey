# Vibey

A lightweight, read-only, VS Code-shaped window for people who code through terminal AI
CLIs (Claude Code and similar). The AI edits the files. Vibey shows the project the way VS Code
does and hosts the terminals, at a fraction of the memory.

Linux only. Rust + GTK4 + VTE. See [SPEC.md](SPEC.md) for scope and [PLAN.md](PLAN.md) for
architecture and measurements.

> Status: **Phase 0 spike.** One project, one file tab, tree, 2 terminals. Not usable as a daily
> tool yet.

## Build

System packages (Ubuntu 24.04 / Mint 22 names):

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libvte-2.91-gtk4-dev
```

Rust (stable, via [rustup](https://rustup.rs)):

```sh
cargo build --release
```

## Run

```sh
./target/release/vibey /path/to/project                # opens README.md / src/main.rs if present
./target/release/vibey . --open src/main.rs --terminals 3
./target/release/vibey . --theme dark_plus             # or any theme JSON in ~/.config/vibey/themes/
```

Themes: drop any VS Code color theme JSON (JSONC and `include` work) into
`~/.config/vibey/themes/` and pass its file stem to `--theme`.

## Measure

```sh
scripts/measure.sh [project-dir] [file]   # startup ms + Vibey's own PSS per GSK renderer
cargo test --release -- --ignored --nocapture   # syntax-set load cost
```

`GSK_RENDERER=cairo` trades GPU rendering for about 25 MB less memory.

## Licensing

Vibey's own license is not chosen yet. Bundled VS Code themes, the Seti icons and Codicons
are third-party. See [third_party/NOTICES.md](third_party/NOTICES.md).
