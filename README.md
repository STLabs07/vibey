# Vibey

A lightweight, read-only, VS Code-shaped window for people who code through terminal AI
CLIs (Claude Code and similar). The AI edits the files. Vibey shows your projects the way VS Code
does, with VS Code's own themes, icons and syntax colors, and hosts the terminals, at about a
seventh of the memory.

Linux only. Rust + GTK4 + VTE. MIT licensed. See [SPEC.md](SPEC.md) for scope and
[PLAN.md](PLAN.md) for architecture and measurements.

> Status: **Phases 1–3 done** (projects, tabs, live reload, file finder, find, themes,
> terminal naming + "finished" badges, git decorations). Coming next: resource panel,
> `vibey .` single instance, session restore, packaging.

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
./target/release/vibey ~/code/app ~/code/api          # open several projects
./target/release/vibey . --open src/main.rs            # open a file in the first project
```

## Keys

| Keys | Action |
|---|---|
| Ctrl+P | Go to file: fuzzy search over the whole project. Space-separated terms narrow it (`comp nav`). Alt+I includes git-ignored files |
| Ctrl+F | Find in file (Enter / Shift+Enter: next / previous, Alt+C: match case) |
| Ctrl+1 … 9 | Switch project |
| Ctrl+W, Ctrl+Tab, Ctrl+PageUp/Down | Close tab, cycle tabs |
| Ctrl+` / Ctrl+Shift+` | Toggle focus editor ↔ terminal / new terminal |
| Ctrl+K Ctrl+T | Color theme picker (live preview, Escape reverts) |

Focus in a terminal: only Ctrl+P, Ctrl+1…9 and Ctrl+` are taken. Every other key (Ctrl+W,
Ctrl+F, Ctrl+K, …) goes to the shell or CLI.

Terminal tabs are named after what's running in them. A project's rail avatar shows a pulsing
ring while one of its terminals is working, and a blue dot when one finished while you were
looking elsewhere, e.g. "Claude finished in project B" while you're in project A. When Vibey
isn't focused, you also get a desktop notification.

Git status shows the way VS Code shows it: colored tree entries with M/U/A letters, gutter
bars against the staged version, `main*` and ↓/↑ in the status bar, and a dirty dot on the
project's rail avatar. Vibey only ever runs `git --no-optional-locks …`, so it never blocks
your AI CLI's commits.

Click in the tree opens a preview tab (italic, replaced by the next click). Double-click pins it.
When a file changes on disk, its tab updates in place and the changed lines flash, then fade.

## Configure

`~/.config/vibey/config.toml`:

```toml
theme = "dark-2026"   # any id from the theme picker, e.g. "one-dark-pro", "user:mytheme"
renderer = "gpu"      # or "software": ≈25 MB less memory, more CPU when scrolling (restart)
notifications = true  # desktop notification when a terminal finishes while Vibey is unfocused
```

Themes: 6 VS Code built-ins, 60+ community themes (One Dark Pro, Dracula, Tokyo Night,
Catppuccin, GitHub, Nord, …), and any VS Code theme JSON you drop into
`~/.config/vibey/themes/` (JSONC and `include` work).

## Test and measure

```sh
cargo test --release
scripts/measure.sh [project-dir] [file]       # startup ms + Vibey's own PSS per GSK renderer
scripts/headless.sh "wait 800; key ctrl+p; type main; shot /tmp/q.png; quit" .
```

`scripts/headless.sh` runs Vibey off-screen on GTK's Broadway backend with a self-test
script (see `src/selftest.rs`). It needs `gtk4-broadwayd` (package `libgtk-4-bin`).

## Licensing

MIT, see [LICENSE](LICENSE). Bundled VS Code themes, Seti icons, Codicons and the grammars
compiled in through `irosashi` are third-party, see [third_party/NOTICES.md](third_party/NOTICES.md).
