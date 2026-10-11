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
       gio::spawn_blocking  notify thread   git worker     proc sampler
       (file read +         (inotify, non-  (1 thread,     (glib timeout →
        syntect highlight)   recursive)     debounced,     spawn_blocking,
                                            GIT_OPTIONAL_  only when visible)
                                            LOCKS=0)
```

- **Highlighting**: the file is read and highlighted in `spawn_blocking`. The result is a list of
  `(line, byte_range, style_id)` that the UI thread turns into shared `GtkTextTag`s, in idle
  chunks with the visible region first. The SyntaxSet loads once, lazily, off-thread
  (`OnceLock`), so startup never waits for it.
- **Code view**: `GtkTextView` (read-only, selectable) plus a custom gutter widget for line numbers
  and git bars. Line height is matched to VS Code's (`round(14×1.35)=19px` × UI scale).
- **Tree**: `GtkListView` + `GtkTreeListModel`, which is virtualized: only visible rows exist and
  directories are read on expand.
- **Terminals**: `vte4::Terminal`. State comes from `tcgetpgrp(pty)` vs the shell pid, plus
  contents-changed, bell and child-exited.
- **Theme**: VS Code JSON (JSONC, `include` chain) → `ThemeModel` → syntect `Theme`, GTK CSS
  (`@define-color` + generated sheet scaled by UI scale) and the VTE palette.
- **Single instance**: `GtkApplication` + `HANDLES_COMMAND_LINE` over D-Bus.

## Module layout
```
src/
  main.rs          app setup, CLI (--measure-startup)
  theme.rs         VS Code theme loader → syntect theme, CSS, terminal palette
  highlight.rs     lazy SyntaxSet, background highlighting
  codeview.rs      read-only text view, gutter (line numbers, git bars)
  tree.rs          file tree (TreeListModel), Seti icons
  icons.rs         Seti font registration + filename→glyph mapping
  terminal.rs      VTE wrapper, naming, status        (phase 2)
  project.rs       per-project state, rail            (phase 1)
  fswatch.rs       notify integration                 (phase 1)
  git.rs           git worker, porcelain v2 parser    (phase 3)
  procmon.rs       /proc sampling, PSS                (phase 4)
  session.rs       config.toml / session.json         (phase 5)
assets/            bundled themes (Dark 2026 chain), seti.woff + mapping
third_party/       MIT notices for VS Code themes / Seti
scripts/measure.sh memory + startup measurement
```

## Dependencies (kept minimal)
| Crate | Why |
|---|---|
| gtk4, vte4 | the stack |
| syntect (no default features; `parsing`, `dump-load`, `regex-onig`) | highlighting |
| two-face (syntaxes only) | TS/TSX/TOML/Dockerfile/… grammars syntect lacks |
| serde_json | theme JSON |
| libc | tcgetpgrp, clock_gettime, kill |
| notify (phase 1), toml + serde (phase 5) | watching, config |

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
1. **Token colors in some languages.** Sublime grammars (syntect) scope fewer things than VS Code's
   TextMate grammars. Example: in Rust, VS Code colors `HashMap`, `Arc` and `u64` teal, while ours
   stay default or blue. Fix option: swap syntect for `irosashi` (MIT, claims byte-identical output
   to vscode-textmate) or `giallo` (EUPL-1.2). Needs approval because syntect is a decided stack item.
2. **Text rendering.** GTK 4.14 rounds glyph advances to whole pixels, so code text is ~2% narrower
   and slightly lighter than Chromium's. GTK 4.16+ adds `gtk-font-rendering` to fix this; Mint 22
   ships 4.14.
3. Not built yet: minimap, current-line highlight, VS Code's custom title bar (Vibey uses the
   window manager's title bar, like VS Code's `window.titleBarStyle: native`).

### Findings that change the plan
- Seti ships as **WOFF, which HarfBuzz/Pango can't load** (fontconfig can, which hides the problem).
  It is now bundled as TTF.
- `GtkDirectoryList` already watches *only expanded* directories through GIO (inotify), which is
  exactly the watching policy in the spec. Proposal: use GIO file monitors for open files too and
  drop the `notify` crate. That means one less dependency and no extra thread. Needs approval
  (stack change).
- `TreeListRow::is_expandable()` must not be used: it creates (lists + monitors) collapsed folders' models.
