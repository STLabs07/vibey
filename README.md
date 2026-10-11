# Vibey

A lightweight, read-only, VS Code-shaped window for people who code through terminal AI
CLIs (Claude Code and similar). The AI edits the files. Vibey shows your projects the way VS Code
does, with VS Code's own themes, icons and syntax colors, and hosts the terminals, at about a
seventh of the memory.

Linux only. Rust + GTK4 + VTE. MIT licensed. See [SPEC.md](SPEC.md) for scope and
[PLAN.md](PLAN.md) for architecture and measurements.

> Status: **v0.1, phases 0–5 done.** About 97 MB with 3 projects and 6 terminals
> (VS Code: 547 MB with one). Starts in about 0.3 s.

## Build

System packages (Ubuntu 24.04 / Mint 22 names):

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libvte-2.91-gtk4-dev
```

Rust (stable, via [rustup](https://rustup.rs)):

```sh
cargo build --release
```

## Install

```sh
scripts/install.sh          # builds, installs ~/.local/bin/vibey + app menu entry (no sudo)
scripts/package-deb.sh      # or build target/vibey_0.1.0_amd64.deb
```

## Run

```sh
vibey .                      # add/focus this folder in the running Vibey (starts it if needed)
vibey ~/code/app ~/code/api  # several projects
vibey . --open src/main.rs   # and open a file
vibey                        # just start: restores last session's projects, tabs, terminals
```

`vibey .` also works from a terminal inside Vibey. Open projects, pinned tabs and terminal
folders are restored on the next start.

## Keys

| Keys | Action |
|---|---|
| Ctrl+P (or the search box in the title bar) | Go to file: fuzzy search over the whole project. Space-separated terms narrow it (`comp nav`). Alt+I includes git-ignored files |
| Ctrl+F | Find in the open file (Enter / Shift+Enter: next / previous, Alt+C: match case). In a terminal or with no file open: project search |
| Ctrl+Shift+F | Project search: the box at the top of the Explorer searches file names and file contents; click a result to jump to the line |
| Alt+1 … 9 | Switch terminal (tabs are numbered) |
| Ctrl+1 … 9 | Switch project |
| Ctrl+W, Ctrl+Tab, Ctrl+PageUp/Down | Close tab, cycle tabs |
| Ctrl+` / Ctrl+Shift+` | Toggle focus editor ↔ terminal / new terminal |
| Ctrl+K Ctrl+T | Color theme picker (live preview, Escape reverts) |
| Ctrl+Shift+R | Resource panel: CPU / memory per project → terminal → process, with kill |

Focus in a terminal: only Ctrl+P, Ctrl+F, Ctrl+1…9, Alt+1…9 and Ctrl+` are taken. Every other key (Ctrl+W,
Ctrl+F, Ctrl+K, …) goes to the shell or CLI.

Terminal tabs are numbered and named after what's running in them, and colored by AI CLI:
Claude orange, Codex green, Gemini blue, other AI CLIs (aider, opencode, …) purple, shells gray.
The top right of the title bar shows **Resources used**: RAM and CPU for Vibey plus everything
running in all projects' terminals. CPU is a share of the whole machine (all cores = 100%). A project's rail avatar shows a pulsing
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
