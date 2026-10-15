# Vibey — implementation plan

## Environment (detected 2026-10-11)
| Item | Value |
|---|---|
| OS / DE | Linux Mint 22 (Ubuntu 24.04 base), **Cinnamon on X11** (no Wayland) |
| GTK4 | 4.14.5 (dev headers installed) — gtk4-rs 0.11, feature `v4_14` |
| VTE | 0.76.0 GTK4 build (`libvte-2.91-gtk4-dev` installed) — vte4 0.10, feature `v0_76`; **verified builds & links** |
| Rust | 1.99.0 via rustup (installed to `~/.cargo`, minimal profile + clippy/rustfmt) |
| Git | 2.43.0. Vibey needs no git credentials: terminals inherit the environment. |
| Fonts | VS Code's default `monospace` stack falls back to **DejaVu Sans Mono**; UI font Ubuntu. UI scale follows Xft.dpi, which Electron also applies. |
| VS Code | 1.141, all default settings → theme **Dark 2026**, icons **vs-seti** |

## Architecture

One process, one GTK main thread. All GTK objects live on it. Workers only produce plain
data (no GObjects) and hand it back with `glib::MainContext::spawn_local` futures.

```
              ┌──────────────── UI thread (GTK main loop) ───────────────┐
              │ Workspace ─ Vec<Project> ─ active idx                     │
              │  Project: TreeModel, Tabs(CodeView…), Terminals(VTE…),    │
              │           GitState, Status                                │
              └──▲──────────────▲──────────────▲──────────────▲───────────┘
                 │ results      │ events       │ results      │ samples
       gio::spawn_blocking  GIO monitors    git worker     proc sampler
       (file read + diff +  (inotify, only  (1 thread,     (glib timeout →
        irosashi highlight)  open/expanded) debounced,     spawn_blocking,
                                            GIT_OPTIONAL_  only when visible)
                                            LOCKS=0)
```

- **Highlighting**: the file is read, diffed against the shown text and highlighted (irosashi,
  VS Code grammars) in `spawn_blocking`. The result is a list of `(line, byte_range, style_id)`
  that the UI thread turns into shared `GtkTextTag`s, 50k spans per idle chunk. The highlighter
  initializes once, lazily, off-thread (`OnceLock`).
- **Live reload**: GIO file monitor per open file → 60 ms debounce → Myers line diff off-thread →
  in-place buffer edits (scroll position kept) → changed lines flash, then fade over 1.5 s.
- **Code view**: `GtkTextView` (read-only, selectable) plus a custom gutter widget for line numbers
  and git bars. Line height is matched to VS Code's (`round(14×1.35)=19px` × UI scale).
- **Tree**: `GtkListView` + `GtkTreeListModel`, which is virtualized: only visible rows exist and
  directories are read on expand.
- **Terminals**: `vte4::Terminal`. State comes from `tcgetpgrp(pty)` vs the shell pid, plus
  contents-changed, bell and child-exited.
- **Theme**: VS Code JSON (JSONC, `include` chain) or irosashi's bundled themes → `ThemeModel` →
  GTK CSS (`@define-color` + sheet scaled by UI scale), VTE palette, and irosashi token rules.
  Duplicate selectors are merged so the later rule wins, as in vscode-textmate.
- **Single instance**: `GtkApplication` + `HANDLES_COMMAND_LINE` over D-Bus.

## Module layout
```
src/
  main.rs          CLI, config, renderer choice, startup measurement
  app.rs           window: rail, project pages, status bar, shortcuts, themes, close guard
  project.rs       per-project state (explorer, editor group, terminals), file index, Ctrl+P provider
  editor.rs        tab strip (preview semantics), breadcrumb, find widget, empty watermark
  codeview.rs      read-only text view, gutter, live reload + diff + change flash, find
  highlight.rs     irosashi wrapper, bracket pair colorization, language names
  theme.rs         theme catalog/model, VS Code defaults, CSS + terminal colors
  quickopen.rs     VS Code-style quick pick (files, themes)
  fuzzy.rs         fzf-style path scorer
  tree.rs          file tree (TreeListModel over dirlist.rs)
  dirlist.rs       per-directory ListStore: GIO listing + monitor, weak-ref async (not GtkDirectoryList)
  terminal.rs      terminal panel (VTE), foreground-job detection
  icons.rs         Seti + Codicon fonts and lookups
  config.rs        ~/.config/vibey/config.toml
  selftest.rs      scripted headless test harness (VIBEY_SELFTEST)
  git.rs           (phase 3)   procmon.rs (phase 4)   session.rs (phase 5)
assets/            VS Code themes, Seti + Codicon fonts, style.css
third_party/       license notices
scripts/           measure.sh, headless.sh, woff2ttf.py
```

## Dependencies (kept minimal)
| Crate | Why |
|---|---|
| gtk4, vte4 | the stack |
| irosashi | VS Code TextMate grammars + tokenizer (pure Rust, MIT) |
| similar | line diff for live-reload change highlighting (no deps) |
| serde, serde_json, toml | theme JSON, config |
| libc | tcgetpgrp, clock_gettime |

## Phase 0 spike (what's being measured)
A window built in the final layout shape: activity rail, Seti-iconed tree, a tab with breadcrumb,
a highlighted read-only code view with line numbers, a VTE panel with 2 terminals, and a status bar.
`scripts/measure.sh` reports startup ms and PSS for the GSK renderers `ngl` and `cairo` with 2 and
6 terminals, and the result is compared side by side with VS Code on the same file.

## Phase 0 results (2026-10-11)

Measured with `scripts/measure.sh` on a Linux Mint 22 desktop, 1400×900 window, opening `src/codeview.rs`.

| | ngl (GTK default) | cairo |
|---|---|---|
| Vibey PSS, 2 terminals | **72.8 MB** | 48.3 MB |
| Vibey PSS, 6 terminals | **73.7 MB** | 49.4 MB |
| Cost per extra terminal | ~0.2–0.7 MB | ~0.3 MB |
| Shells inside terminals (not Vibey) | 11.5 MB (2) / 33.9 MB (6) | same |

- **Startup**: 248 ms median to first painted frame, warm cache (target < 1 s).
- **Reference**: VS Code 1.141 on the same file with 1 terminal and *no extensions* totals **547 MB** PSS.
- **Where ngl's 73 MB goes**: heap 21 MB + anon 20 MB (GL buffers and glyph atlases), libLLVM 15 MB
  (Mesa's shader compiler, shared with other GL apps, so PSS splits it), libgtk 7 MB, Mesa 3 MB,
  Vibey 2.6 MB. The syntax set (220 grammars, lazy) is ~2 MB and loads in 3 ms.
- **Projection** for the target scenario (3 projects, 6 terminals): about 80–90 MB on `ngl`, under the
  120 MB cap and inside the 60–100 MB ideal. Phase 1 confirms this with real projects.

### Look vs VS Code (Dark 2026, same file, side by side)
Matching: theme colors, layout geometry (row heights, tab/breadcrumb/panel/status heights, gutter,
line pitch 21 px vs 20.8 px), Seti file icons, Codicon chevrons, bracket pair colorization,
terminal palette and font.
Differences:
1. ~~**Token colors in some languages.**~~ Fixed in phase 1 by switching to irosashi. Token colors now
   match pixels sampled from VS Code (`highlight::tests::matches_vscode_dark_2026`).
2. **Text rendering.** GTK 4.14 rounds glyph advances to whole pixels, so code text is ~2% narrower
   and slightly lighter than Chromium's. GTK 4.16+ adds `gtk-font-rendering` to fix this; Mint 22
   ships 4.14.
3. Not built yet: minimap, current-line highlight, VS Code's custom title bar (Vibey uses the
   window manager's title bar, like VS Code's `window.titleBarStyle: native`).

### Findings that change the plan
- Seti ships as **WOFF, which HarfBuzz/Pango can't load** (fontconfig can, which hides the problem).
  It is now bundled as TTF.
- Watching *only expanded* directories through GIO (inotify) is exactly the watching policy in the
  spec. `GtkDirectoryList` did this but crashes on GTK 4.14.5 (use-after-free of its queued monitor
  events when a folder is collapsed mid-event), so `dirlist.rs` does the same with weak refs. Proposal: use GIO file monitors for open files too and
  drop the `notify` crate. That means one less dependency and no extra thread. Needs approval
  (stack change).
- `TreeListRow::is_expandable()` must not be used: it creates (lists + monitors) collapsed folders' models.

## Phase 1 results (2026-10-11)

Done: project rail (avatars, Ctrl+1..9, add/close), per-project explorer, tabs with preview
semantics, live reload with change flash, Ctrl+P file finder, Ctrl+F find, theme picker
(66 themes, live preview), config.toml, close guard for running jobs.

Verified headlessly (`scripts/headless.sh`, Broadway backend, PNG snapshots). Nothing ran on the
user's desktop: quick open on a 737-file repo, find (124 matches), live reload (3 changed lines
flash, then fade), theme preview + Escape revert, project switching from inside a terminal.

**Memory (real display, `scripts/measure.sh`):**

| Scenario | GPU (ngl) | software (cairo) |
|---|---|---|
| 1 project, 2 terminals, 1 file | 81 MB | 56 MB |
| Spec target: 3 projects, 6 terminals, 6 files (this repo) | **95 MB** | 70 MB |
| Same with real projects (TSX, Prisma, Markdown, 7 files) | **107 MB** | 86 MB |

Startup is 277 ms median. The first real-project run was **188 MB**. Fixes:
- Markdown's grammar embeds ~60 fenced-code languages and compiled all of them (+46 MB for a 21-byte
  README). We now register VS Code's markdown grammar without them and highlight each fenced block
  with its own language, so only the languages actually present are loaded (6 MB, plus each used language).
- `M_ARENA_MAX=2` and `malloc_trim` after each load: worker-thread arenas were holding freed memory.

Per-grammar cost (RSS, first use): Rust 2 MB, TSX 6 MB, CSS 3 MB, Markdown 6 MB, JSON/HTML/Python ≈0–1 MB.
Under the 120 MB cap. GPU is slightly above the 60–100 MB "ideal" with real projects.

Known gaps carried forward: the tree doesn't auto-reveal the active file yet. Files over 4 MB show
without highlighting, and files over 8 MB show a notice.

irosashi upstream bug to report: with two rules for the same selector, the first wins
(vscode-textmate: the last wins). Worked around in `theme::dedupe_rules`.

## Phase 2 results (2026-10-11)

Terminal tabs are named after the foreground process (`claude`, `cargo`, `sleep`…). For
interpreters (`node script`) the script name is used. Status: running (output in the last 1.5 s,
pulsing dot), idle, or finished-unseen. The last means a busy stretch ≥ 3 s ended, a long
foreground job exited, or the bell rang while that terminal wasn't on screen with the window
focused. It rolls up to a rail badge (pulsing ring = working, blue dot = finished) and a `●`
prefix in the window title. When Vibey isn't focused it also sends a desktop notification
(`notifications = false` turns that off). Kill-terminal × on each tab asks first if a job is running.

All of it is event-driven: VTE `contents-changed` / `bell` / `child-exited`, plus one one-shot timer
armed by output. An idle terminal costs no wakeups.

Verified headlessly: busy script → running badge, switch away → "finished" dot on that project,
switch back → cleared, `sleep` shows as the tab name.
Memory, target scenario: 96.6 MB GPU / 73.3 MB software. Startup is 293 ms median.

## Phase 3 results (2026-10-11)

`git.rs` runs `git --no-optional-locks status --porcelain=v2 --branch -z --ignored=matching`
off the UI thread, debounced 300 ms, one at a time. Refresh triggers are events only (no timers):
- anything in `.git/` changes (commit, stage, checkout; `*.lock` churn is ignored)
- an open file reloads
- a terminal starts or stops being busy
- an expanded folder's contents change
- the window regains focus

Decorations:
- **Explorer:** VS Code colors plus M/U/A/D/R letters. Folders take their loudest child and
  show `•`. Ignored paths are dimmed (`node_modules/` comes back as one entry, so it's cheap).
- **Tabs:** title colored by status.
- **Gutter:** added/modified bars and deleted wedges, diffed against the *index* like VS Code's
  quick diff (staging clears them). The index copy is cached by `.git/index` mtime.
- **Status bar:** `⎇ main*` plus `↓n ↑n`.
- **Rail:** a dirty dot on the avatar; branch and change count in the tooltip.

Verified headlessly on a scratch repo: gutter bars on changed lines, tree colors, `main*`, rail
dot, tab color; `git add -A` cleared the gutter about 1.5 s later via the `.git` watch.
Memory, target scenario: 97.6 MB GPU / 73.3 MB software.

## Phase 4 results (2026-10-11)

`procmon.rs` reads `/proc` directly. It walks only Vibey's own process tree via
`/proc/<pid>/task/*/children`, never all of /proc. CPU% comes from utime+stime deltas (100% = one
core) and memory is PSS from `smaps_rollup`, falling back to RSS. Sampling runs off the UI thread
every 2 s (1 s while the panel is open) and **only while the window is focused**. Unfocused, it stops.

- Status bar: `Vibey + terminals: 123 MB · 104%`. Click it or press Ctrl+Shift+R for the panel.
- Rail: memory chip under each avatar (`89M 100%`).
- Panel: Vibey's own row, then project → terminal → process tree. Sort by memory / CPU / name.
  A kill button per process asks first, then sends SIGTERM. Footer shows system load and RAM.

Verified headlessly with an 80 MB / 100% CPU test process. Memory, target scenario: 98.3 MB GPU /
73.9 MB software. Startup is 302 ms. Idle CPU with the window focused is 0.5% of one core
(cursor blink + sampler).

## Phase 5 results (2026-10-11)

- **Single instance:** GtkApplication + `HANDLES_COMMAND_LINE` over D-Bus. `vibey DIR…` from any
  terminal, including one inside Vibey, adds or focuses projects in the running window. A
  forwarding call exits in ~25 ms without initializing GTK. Relative paths resolve against the
  caller's directory.
- **Session** (`~/.config/vibey/session.json`, debounced 1 s, atomic): projects, pinned tabs,
  active file, window size/maximized, and each terminal's working directory (read from
  `/proc/<shell>/cwd`). Terminals reopen as fresh shells there. `--new-instance`, measurements and
  self-tests never read or write the session.
- **Packaging:** `scripts/install.sh` (no sudo: `~/.local/bin`, menu entry, icon) and
  `scripts/package-deb.sh` (plain `dpkg-deb`, depends on libgtk-4-1, libvte-2.91-gtk4-0, git).
  Release builds are stripped with thin LTO. Binary is 12.7 MB, most of it embedded grammars.

Verified on the real display with an isolated `XDG_CONFIG_HOME`: forwarding, session save,
restore after restart (projects, tab, gutter, terminal cwd).
Final numbers: startup 293 ms median. Target scenario 96.9 MB GPU / 74.5 MB software
(VS Code with one file and one terminal: 547 MB). Idle CPU 0.5% of one core while focused,
and no sampling while unfocused.
