# Vibey — product spec

This is the original brief with corrections from inspecting the target machine.
Changes from the brief are marked **[rev]**.

## Decisions log
- 2026-10-11, **license**: MIT ("open source, free forever").
- 2026-10-11, **search is in scope (read-only)**: Ctrl+P fuzzy file finder over the whole
  project, including git-ignored files on request. This fixes the VS Code weak spot the user named.
  Ctrl+F find-in-file. Still no replace.
- 2026-10-11, **themes**: the VS Code dark look stays the default (Dark 2026), with more options available:
  VS Code's built-ins, 60+ community themes and user JSON, through a live-preview picker.
- 2026-10-11, **highlighter: syntect → irosashi** (MIT). It runs VS Code's actual TextMate
  grammars with vscode-textmate semantics, so token colors match VS Code. It's also faster
  and needs no C regex build. Approved as "do what you think is best".
- 2026-10-11, **file watching: notify → GIO file monitors**, which are already in-process via GTK. One less
  dependency and no extra thread. Approved.
- 2026-10-11, **renderer**: GPU by default; `renderer = "software"` in config.toml saves ≈25 MB.

## What this is
A very lightweight, Linux-only, READ-ONLY "VS Code-shaped" window for people who code
entirely through terminal AI CLIs (Claude Code, GPT CLIs, etc.). The AI edits the files;
Vibey shows the project the way VS Code does and hosts the terminals. It is NOT an editor.

VS Code uses 200–500 MB+ before any terminals. Vibey exists to give the same look at a
fraction of the weight, with better multi-project / multi-terminal awareness.

## Hard scope
IN: file tree, read-only syntax-highlighted code view with tabs, embedded terminals,
multiple projects in one window, live file reload, git status display, resource monitoring,
`vibey .` launcher, file finder (Ctrl+P), find in file (Ctrl+F), theme picker.

OUT, permanently: editing, extensions/plugins, debugger, LSP, IntelliSense, search-and-replace.

**[rev] Allowed, because they don't edit anything:** selecting and copying text in the code view,
"Copy path" and "Copy relative path" in the tree, file finder, find in file. **Not decided yet:**
project-wide text search (Ctrl+Shift+F, read-only), "Open in $EDITOR". Ask before adding either.

## Stack (decided)
- Rust + GTK4 (gtk4-rs), plain GTK4 with custom CSS, no libadwaita.
- Terminals: VTE GTK4 build (`vte4` crate) on a real PTY.
- Highlighting: ~~`syntect`~~ **irosashi** (see the decisions log) + a VS Code theme JSON → GTK CSS converter.
  - **[rev] Default theme is "Dark 2026".** That is VS Code 1.141's actual default, and it's what
    a default VS Code install shows, since `settings.json` sets no theme. Dark Modern and Dark+
    ship too. All three are an `include` chain
    (`2026-dark → dark_modern → dark_plus → dark_vs`), so the loader must resolve includes.
    It must also accept JSONC (comments and trailing commas), which is the format of themes
    users drop in.
  - **[rev] Extra grammars come from `two-face`** (bat's syntax set). syntect's built-in set has no
    TypeScript/TSX, TOML, Dockerfile, etc., which are the languages AI CLIs touch most.
  - **[rev] Honest limit:** syntect uses Sublime grammars, not VS Code's TextMate JSON grammars, and
    VS Code layers *semantic* highlighting from its language servers on top. Colors will be
    very close but not pixel-identical, especially in TS/Python.
- **[rev] File icons:** VS Code's default icon theme is Seti, a font plus a JSON mapping (MIT).
  Bundle it and register the font through fontconfig, so no extra crate is needed.
- File watching: ~~`notify`~~ **GIO file monitors** (inotify), see the decisions log.
- Git: shell out to system `git`, debounced, off the UI thread, no libgit2.
  - **[rev] Always run with `GIT_OPTIONAL_LOCKS=0` (`--no-optional-locks`).** A background
    `git status` otherwise briefly takes `.git/index.lock`, and the AI CLI's own `git commit`
    then fails with "index.lock exists". That would be a real bug for the target user.
- Resource monitoring: read /proc directly.
- Config/state: `~/.config/vibey/` (`config.toml`, `session.json`), themes in
  `~/.config/vibey/themes/*.json`.

## Layout (match VS Code closely)
- Far-left PROJECT RAIL: one avatar per open project (2–5 typical, ~10 max). Each shows its
  branch, a git-dirty marker, a resource chip ("1.2 GB · 14%"), and an attention dot.
- File TREE: Seti icons, indentation guides, expand/collapse, git colors (modified,
  untracked, added, deleted; ignored files dimmed).
- Main area: tabbed read-only code view with line numbers, git gutter (added, modified and
  deleted bars), breadcrumb, and smooth scrolling. On a disk change it reloads, keeps the scroll
  position, and briefly highlights the changed lines, which then fade out.
- Terminal DOCK: several terminal tabs per project, splits optional. The cwd is the project
  root. Each tab is named after its foreground process, falling back to the shell name.
- Status bar: project and branch, Vibey + children CPU/RAM, system load. Clicking it opens the resource panel.
- Each project keeps its files, scroll positions and terminals alive in the background.
  Ctrl+1..9 or a rail click swaps the whole set.
- Optional, low priority: pop a project out into its own window.

## Terminal status dots
States: running (foreground job active or recent output), idle (at the prompt), and
finished-unseen (exited or went idle while the user was elsewhere). These roll up to the rail.
**[rev] Detection on VTE 0.76:** that version has no termprops or shell-integration API, so:
- compare `tcgetpgrp(pty_fd)` with the shell's pid, then read `/proc/<pgid>/comm` for the name
- `contents-changed` timestamps for "recent output"
- `bell` and `child-exited` signals

Claude Code rings the bell or sends OSC notifications when it finishes, which covers the main case.
**[rev] Closing the window** while any terminal has a foreground job running asks for confirmation first.

## Resource panel
Grouped project → terminal → process tree, with CPU% and RAM (PSS from `smaps_rollup`), sortable,
kill button per process (confirm, SIGTERM). Vibey's own overhead is a row. Poll every 1–2 s,
only while the panel or status bar is visible and the window is not hidden.

## Launcher / single instance
`vibey .` / `vibey /path` adds or focuses the project in the running instance, or starts one.
**[rev] Use GtkApplication's built-in D-Bus uniqueness** (`HANDLES_COMMAND_LINE`) rather than a custom
socket. It costs zero extra code or dependencies. Open projects are restored on start.
**[rev]** Restore brings back terminals *with their cwd*. Processes themselves can't be restored.

## Git and GitHub
v1 uses the user's system git and credentials only. Vibey stores nothing, and terminals inherit
the environment (`SSH_AUTH_SOCK` etc.). v1.5 GitHub sign-in is out of scope for now.

## Performance targets (requirements)
**[rev] Each target now says how it's measured, so the numbers are reproducible (`scripts/measure.sh`):**
- **Cold start < 1 s**: from exec to the first frame painted, warm page cache, reported by
  `vibey --measure-startup`.
- **Memory**: with 3 projects and 6 terminals, Vibey's own PSS (its pid only, read from
  `/proc/<pid>/smaps_rollup` 5 s after settle) is < 120 MB, ideally 60–100 MB. Each phase also
  reports the incremental cost per terminal and per open file, so the target can be extrapolated.
- **[rev] GTK renderer is part of the memory budget.** GTK's GL renderer loads Mesa's
  LLVM shader compiler, so `ngl` and `cairo` both get measured and the one used is a deliberate choice.
- **Idle CPU ≈ 0%**: no timers while unfocused or hidden.
- **Large files**: over 1 MB or 20k lines, highlight lazily (visible region first) or show
  plain text. Over 20 MB, or binary (NUL byte in the first 8 KB), show a notice instead of the contents.
- **[rev] File watching** never recursively watches the whole tree. It watches only expanded
  directories, open files and `.git/{HEAD,index,refs}`, because recursive watches on
  `node_modules`/`target` exhaust inotify and memory.

## Process
1. Inspect the environment and report it. (done, see PLAN.md)
2. PLAN.md: architecture, modules, threading.
3. Phase 0 measurement spike, then stop and report memory, startup and look honestly.
4. Phases 1–5, checking in after each and reporting memory each time. Fix regressions before moving on.
5. Small commits and a README.

Name: Vibey. Binary: `vibey`. License: **MIT**. Bundled VS Code themes and the Seti
font are MIT, and their notices must ship in `third_party/`.
