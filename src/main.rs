mod app;
mod codeview;
mod config;
mod editor;
mod fuzzy;
mod git;
mod highlight;
mod icons;
mod procmon;
mod project;
mod quickopen;
mod resources;
mod selftest;
mod terminal;
mod theme;
mod tree;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

const APP_ID: &str = "dev.vibey.Vibey";

pub fn config_dir() -> PathBuf {
    glib::user_config_dir().join("vibey")
}

pub fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("vibey")
}

struct Args {
    roots: Vec<PathBuf>,
    open: Option<PathBuf>,
    measure_startup: bool,
    exit_after_startup: bool,
    terminals: usize,
    theme: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        roots: Vec::new(),
        open: None,
        measure_startup: false,
        exit_after_startup: false,
        terminals: 1,
        theme: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--measure-startup" => args.measure_startup = true,
            "--exit-after-startup" => args.exit_after_startup = true,
            "--open" => args.open = it.next().map(PathBuf::from),
            "--terminals" => args.terminals = it.next().and_then(|n| n.parse().ok()).unwrap_or(1),
            "--theme" => args.theme = it.next(),
            _ => args.roots.push(PathBuf::from(a)),
        }
    }
    if args.roots.is_empty() {
        args.roots.push(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
    }
    args
}

/// Milliseconds since this process was exec'd (10 ms resolution, from /proc).
fn ms_since_exec() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields: Vec<&str> = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or("").split_whitespace().collect();
    let start_ticks: f64 = fields.get(19).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    let now = ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9;
    (now - start_ticks / hz) * 1000.0
}

fn main() -> glib::ExitCode {
    // Worker threads (highlighting, indexing) would otherwise each get a malloc arena that
    // keeps freed memory resident.
    unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
    let args = parse_args();
    let mut cfg = config::load();
    if let Some(t) = &args.theme {
        cfg.theme = t.clone();
    }
    // The renderer is chosen when the window is realized; the variable is removed again
    // afterwards so programs started in the terminals don't inherit it.
    let set_renderer = cfg.renderer == "software" && std::env::var_os("GSK_RENDERER").is_none();
    if set_renderer {
        // SAFETY: single-threaded at this point.
        unsafe { std::env::set_var("GSK_RENDERER", "cairo") };
    }
    highlight::preload();
    icons::register_fonts();

    let app = gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|_| {
        if let Some(s) = gtk::Settings::default() {
            // GTK's built-in theme, so the user's GTK theme can't leak into ours.
            s.set_gtk_theme_name(Some("Default"));
            // Fractional font sizes + subpixel positioning, closer to Chromium's text.
            s.set_gtk_hint_font_metrics(false);
        }
    });
    let args = Rc::new(args);
    let cfg = Rc::new(cfg);
    app.connect_activate(move |gapp| {
        let app = app::App::new(gapp, (*cfg).clone());
        for root in &args.roots {
            if app.add_project(root).is_none() {
                eprintln!("vibey: not a directory: {}", root.display());
            }
        }
        if let (Some(file), Some(p)) = (&args.open, app.active_project()) {
            let path = if file.is_absolute() { file.clone() } else { p.root.join(file) };
            p.open_file(&path, true);
        }
        let terminals = args.terminals;
        let a = app.clone();
        app.window.connect_realize(move |_| {
            if set_renderer {
                // SAFETY: GTK is single-threaded here and no other thread reads the env.
                unsafe { std::env::remove_var("GSK_RENDERER") };
            }
            a.on_realized();
            if let Some(p) = a.active_project() {
                for _ in 1..terminals {
                    p.terminals.add_terminal(&app::current_theme());
                }
            }
        });

        if args.measure_startup {
            let exit = args.exit_after_startup;
            let gapp = gapp.clone();
            app.window.add_tick_callback(move |_, clock| {
                let done = Rc::new(Cell::new(false));
                let gapp = gapp.clone();
                clock.connect_after_paint(move |_| {
                    if !done.replace(true) {
                        eprintln!("vibey: first frame after {:.0} ms", ms_since_exec());
                        if exit {
                            let gapp = gapp.clone();
                            glib::idle_add_local_once(move || gapp.quit());
                        }
                    }
                });
                glib::ControlFlow::Break
            });
        }
        app.window.present();
        if let Ok(script) = std::env::var("VIBEY_SELFTEST") {
            selftest::run(app.clone(), gapp.clone(), script);
        }
        // Keep the App alive for the lifetime of the window.
        let keep = app.clone();
        app.window.connect_destroy(move |_| {
            let _ = &keep;
        });
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}
