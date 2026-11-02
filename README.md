# Vibey

**A lightweight, VS Code-shaped window for coding through terminal AI CLIs** (Claude Code,
Codex, Gemini, aider, and the like).

If the AI does the editing and you spend your day in a terminal, you mostly need VS Code for
the way it lays a project out, not for its editor. Vibey keeps that part: your projects, files,
tabs, git status and terminals in one window, with VS Code's own themes, icons and syntax
colors, at about a seventh of the memory.

- **Several projects, one window.** Each project has its own file tree, tabs and terminals. A
  rail icon per project pulses while a terminal is working and shows a dot when one finished
  while you were looking elsewhere.
- **Terminals that tell you when they're done.** A notification when a job finishes, when the
  CLI rings the bell asking for you, or when it stops after sustained work (done, or blocked on
  a prompt or safeguard). One per terminal until you look at it, so nothing spams.
- **VS Code's look.** Its themes (6 built-in, 60+ community, or drop in your own JSON), Seti
  file icons, TextMate-grammar syntax colors, git decorations.
- **Fast to get around.** Ctrl+P file finder, find in file, project-wide search.
- **Plain-text editing for the moments you need it.** When the AI asks you to paste an API key
  into `.env`, click in the file, type or paste, and press Ctrl+S. No more editor than that.
- **Claude's memory, visible.** The notes Claude Code keeps for a project live in a hidden
  folder outside it; Vibey shows them in a Claude Memory section of the Explorer.
- **Resource panel.** CPU and memory per project, terminal and process, with kill.

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

Pasting text works with nothing extra. When a CLI uses the clipboard itself, such as pasting an
image or `agy`'s own copy and paste, it needs a clipboard tool. Without one, `agy` says "no
supported clipboard tool found".

```sh
sudo apt install xclip          # X11
sudo apt install wl-clipboard   # Wayland
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
| Ctrl+S | Save the open file. Files also save when you leave them (another tab, a terminal, another window) |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo in the open file |
| Ctrl+W, Ctrl+Tab, Ctrl+PageUp/Down | Close tab, cycle tabs |
| Ctrl+` / Ctrl+Shift+` | Toggle focus editor ↔ terminal / new terminal |
| Ctrl+K Ctrl+T | Color theme picker (live preview, Escape reverts) |
| Ctrl+Shift+R | Resource panel: CPU / memory per project → terminal → process, with kill |
| Ctrl+Shift+C / Ctrl+Shift+V | Copy / paste in a terminal. Ctrl+C also copies while text is selected (otherwise it interrupts as usual). Ctrl+V pastes text too, except in editors, pagers and multiplexers (vim, nano, less, tmux, ssh…), which keep the key; with no text on the clipboard it goes through to the CLI, which is how image paste works (that needs `xclip`, or `wl-clipboard` on Wayland). Right-click: Copy, Paste, Select All, Clear. A copy shows a brief "Copied to clipboard" note |

Focus in a terminal: only Ctrl+P, Ctrl+F, Ctrl+1…9, Alt+1…9 and Ctrl+` are taken. Every other key (Ctrl+W,
Ctrl+F, Ctrl+K, …) goes to the shell or CLI.

Terminal tabs are numbered and named after what's running in them, and colored by AI CLI:
Claude orange, Codex green, Gemini blue, other AI CLIs (aider, opencode, …) purple, shells gray.
The top right of the title bar shows **Resources used**: RAM and CPU for Vibey plus everything
running in all projects' terminals. CPU is a share of the whole machine (all cores = 100%). A project's rail avatar shows a pulsing
ring while one of its terminals is working, and a blue dot when one finished while you were
looking elsewhere, e.g. "Claude finished in project B" while you're in project A. When Vibey
isn't focused, you also get a desktop notification, once per terminal until you look at it:
"finished" (a long job ended), "needs you" (the CLI rang the bell or its title asks for input),
or "is waiting" (an AI CLI stopped working: its turn ended or it's at a prompt). Pauses in the
middle of a turn don't alert: Claude Code and Gemini CLI report their status in the terminal
title, and other AI CLIs have to be silent for 15 seconds.

Git status shows the way VS Code shows it: colored tree entries with M/U/A letters, gutter
bars against the staged version, `main*` and ↓/↑ in the status bar, and a dirty dot on the
project's rail avatar. Vibey only ever runs `git --no-optional-locks …`, so it never blocks
your AI CLI's commits.

Editing is deliberately plain: type, paste, undo, save. A dot on the tab means unsaved edits;
the status bar says so too. Vibey writes atomically and keeps the file's permissions (a `.env`
stays private). If your AI CLI changed the file after you started editing, Vibey doesn't overwrite it
silently: the tab turns orange, auto-save skips it, and Ctrl+S asks first. Files that aren't UTF-8
or are binary stay read-only. Turn off automatic saving in the gear menu or with `auto_save = false`
in `~/.config/vibey/config.toml`.

Click in the tree opens a preview tab (italic, replaced by the next click). Double-click pins it.
When a file changes on disk, its tab updates in place and the changed lines flash, then fade.

Vibey only changes a file when you type in it and save, or in one other case: in a Markdown
file, click a task box (`- [ ]`) to tick it off (`- [x]`), and click again to untick. Handy when
the AI writes a task list and asks you to mark things done.

**Claude Memory.** Claude Code keeps per-project notes in
`~/.claude/projects/<project>/memory/` (or under `$CLAUDE_CONFIG_DIR`), a hidden folder outside
the project. When it exists, the Explorer shows it as a collapsible Claude Memory section under
the project tree. Click a file to read it, or to fix something the AI remembered wrong. It
appears when the project is opened, so reopen the project if Claude creates the folder later.

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
