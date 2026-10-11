//! Scripted self-test for headless runs (`GDK_BACKEND=broadway`): drives the app through
//! its own actions and saves screenshots rendered by GTK, so GUI behavior can be verified
//! without synthetic input on a real desktop.
//!
//! `VIBEY_SELFTEST="wait 800; key ctrl+p; type codev; shot /tmp/a.png; enter; quit"`
//!
//! Commands: `term TEXT` (typed into the shown terminal + Enter), `wait MS`, `key SPEC` (e.g. ctrl+shift+grave), `type TEXT` (quick pick or
//! find box), `enter`, `down`, `escape`, `find TEXT`, `project N`, `theme ID`,
//! `exec SHELL…`, `shot FILE`, `mem`, `quit`.

use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, gsk};

use crate::app::App;

pub fn run(app: Rc<App>, gapp: gtk::Application, script: String) {
    let steps: Vec<String> = script.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    step(app, gapp, steps, 0);
}

fn step(app: Rc<App>, gapp: gtk::Application, steps: Vec<String>, i: usize) {
    let Some(line) = steps.get(i).cloned() else { return };
    let (cmd, arg) = line.split_once(' ').map_or((line.as_str(), ""), |(c, a)| (c, a.trim()));
    let mut delay = 150;
    match cmd {
        "wait" => delay = arg.parse().unwrap_or(500),
        "key" => {
            let (key, mods) = parse_key(arg);
            let handled = app.handle_key(key, mods);
            eprintln!("selftest: key {arg} handled={handled}");
        }
        "type" => {
            if app.quick().is_open() {
                app.quick().set_query(arg);
            } else if let Some(p) = app.active_project() {
                p.editor.set_find_query(arg);
            }
        }
        "search" => {
            if let Some(s) = app.active_project().and_then(|p| p.search.get().cloned()) {
                s.focus();
                s.set_query(arg);
            }
        }
        "enter" => {
            if app.quick().is_open() {
                app.quick().accept_selected();
            } else if let Some(p) = app.active_project() {
                p.editor.find_step(true);
            }
        }
        "down" => app.quick().move_selection(1),
        "escape" => {
            if app.quick().is_open() {
                app.quick().cancel();
            } else if let Some(p) = app.active_project() {
                p.editor.close_find();
            }
        }
        "resources" => app.toggle_resources(),
        "term" => {
            if let Some(p) = app.active_project() {
                p.terminals.feed(&format!("{arg}\n"));
            }
        }
        "find" => {
            if let Some(p) = app.active_project() {
                p.editor.show_find();
                p.editor.set_find_query(arg);
            }
        }
        "project" => app.activate(arg.parse::<usize>().unwrap_or(1).saturating_sub(1)),
        "theme" => app.apply_theme(arg, false),
        "exec" => {
            let ok = std::process::Command::new("sh").args(["-c", arg]).status().is_ok_and(|s| s.success());
            eprintln!("selftest: exec ok={ok}");
        }
        "shot" => {
            // The first frames may not have been drawn yet; retry briefly.
            let (window, path) = (app.window.clone(), arg.to_string());
            let tries = std::cell::Cell::new(0);
            glib::timeout_add_local(Duration::from_millis(100), move || match shot(&window, &path) {
                Ok(()) => {
                    eprintln!("selftest: saved {path}");
                    glib::ControlFlow::Break
                }
                Err(e) if tries.get() >= 20 => {
                    eprintln!("selftest: shot failed: {e}");
                    glib::ControlFlow::Break
                }
                Err(_) => {
                    tries.set(tries.get() + 1);
                    glib::ControlFlow::Continue
                }
            });
            delay = 2200;
        }
        "trim" => {
            unsafe { libc::malloc_trim(0) };
        }
        "smaps" => {
            let mut agg: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
            let mut name = String::new();
            for line in std::fs::read_to_string("/proc/self/smaps").unwrap_or_default().lines() {
                let first = line.split_whitespace().next().unwrap_or("");
                if first.contains('-') && first.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                    let path = line.split_whitespace().nth(5).unwrap_or("[anon]");
                    name = path.rsplit('/').next().unwrap_or(path).to_string();
                    if name.starts_with("libLLVM") {
                        name = "libLLVM".into();
                    }
                } else if let Some(v) = line.strip_prefix("Pss:") {
                    *agg.entry(name.clone()).or_default() += v.trim().trim_end_matches(" kB").parse::<u64>().unwrap_or(0);
                }
            }
            let mut v: Vec<_> = agg.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            for (n, kb) in v.into_iter().take(8) {
                eprintln!("selftest:   {:6.1} MB {n}", kb as f64 / 1024.0);
            }
        }
        "mem" => {
            let s = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
            let pss = s.lines().find(|l| l.starts_with("Pss:")).unwrap_or("Pss: ?");
            eprintln!("selftest: {pss}");
        }
        "quit" => {
            gapp.quit();
            return;
        }
        other => eprintln!("selftest: unknown command {other}"),
    }
    glib::timeout_add_local_once(Duration::from_millis(delay), move || step(app, gapp, steps, i + 1));
}

fn parse_key(spec: &str) -> (gdk::Key, gdk::ModifierType) {
    let mut mods = gdk::ModifierType::empty();
    let mut key = gdk::Key::VoidSymbol;
    for part in spec.split('+') {
        match part {
            "ctrl" => mods |= gdk::ModifierType::CONTROL_MASK,
            "shift" => mods |= gdk::ModifierType::SHIFT_MASK,
            "alt" => mods |= gdk::ModifierType::ALT_MASK,
            name => key = gdk::Key::from_name(name).unwrap_or(gdk::Key::VoidSymbol),
        }
    }
    (key, mods)
}

/// Render the window (title bar + content) with a software renderer and save it as PNG.
fn shot(window: &gtk::ApplicationWindow, path: &str) -> Result<(), String> {
    // A toplevel can't be painted by WidgetPaintable; its title bar and child can.
    let child = window.child().ok_or("window has no child")?;
    let bar = window.titlebar();
    let (w, ch) = (child.width(), child.height());
    let th = bar.as_ref().map_or(0, |b| b.height());
    let snapshot = gtk::Snapshot::new();
    if let Some(bar) = &bar {
        gtk::WidgetPaintable::new(Some(bar)).snapshot(&snapshot, w as f64, th as f64);
    }
    snapshot.translate(&gtk::graphene::Point::new(0.0, th as f32));
    gtk::WidgetPaintable::new(Some(&child)).snapshot(&snapshot, w as f64, ch as f64);
    let node = snapshot
        .to_node()
        .ok_or_else(|| format!("nothing rendered (mapped={}, {w}x{ch})", window.is_mapped()))?;
    let renderer = gsk::CairoRenderer::new();
    renderer.realize_for_display(&WidgetExt::display(window)).map_err(|e| e.to_string())?;
    let texture = renderer.render_texture(&node, Some(&gtk::graphene::Rect::new(0.0, 0.0, w as f32, (th + ch) as f32)));
    renderer.unrealize();
    texture.save_to_png(path).map_err(|e| e.to_string())
}
