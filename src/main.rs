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
mod search;
mod searchview;
mod selftest;
mod session;
mod terminal;
mod theme;
mod tree;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

const APP_ID: &str = "dev.vibey.Vibey";

const USAGE: &str = "usage: vibey [DIR…] [--open FILE] [--new-instance]

Opens each DIR as a project (or focuses it) in the running Vibey window,
starting Vibey if needed. With no DIR, restores the last session.

  --open FILE        open FILE in the first given project
  --new-instance     separate window, no session restore/save (testing)
  --theme ID         theme for this run
  --terminals N      terminals to open in the active project
  --measure-startup  print time to first frame";

pub fn config_dir() -> PathBuf {
    glib::user_config_dir().join("vibey")
}

pub fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("vibey")
}

#[derive(Default)]
struct Args {
    roots: Vec<PathBuf>,
    open: Option<PathBuf>,
    measure_startup: bool,
    exit_after_startup: bool,
    new_instance: bool,
    help: bool,
    terminals: usize,
    theme: Option<String>,
}

/// Parse a command line; relative paths resolve against `cwd` (the caller's directory,
/// which differs from ours when another `vibey` forwarded its arguments).
fn parse_args(argv: &[String], cwd: &Path) -> Args {
    let mut args = Args { terminals: 1, ..Default::default() };
    let abs = |p: &str| {
        let p = PathBuf::from(p);
        if p.is_absolute() { p } else { cwd.join(p) }
    };
    let mut it = argv.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--measure-startup" => args.measure_startup = true,
            "--exit-after-startup" => args.exit_after_startup = true,
            "--new-instance" => args.new_instance = true,
            "-h" | "--help" => args.help = true,
            "--open" => args.open = it.next().map(|p| p.into()),
            "--terminals" => args.terminals = it.next().and_then(|n| n.parse().ok()).unwrap_or(1),
            "--theme" => args.theme = it.next().cloned(),
            _ => args.roots.push(abs(a)),
        }
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

thread_local! {
    static APP: RefCell<Option<Rc<app::App>>> = const { RefCell::new(None) };
}

fn main() -> glib::ExitCode {
    // Worker threads (highlighting, indexing) would otherwise each get a malloc arena that
    // keeps freed memory resident.
    unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
    let argv: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let local = parse_args(&argv, &cwd);
    if local.help {
        println!("{USAGE}");
        return glib::ExitCode::SUCCESS;
    }
    // Tests and measurements get their own window and never touch the user's session.
    let ephemeral = local.new_instance || local.measure_startup || std::env::var_os("VIBEY_SELFTEST").is_some();

    let mut cfg = config::load();
    if let Some(t) = &local.theme {
        cfg.theme = t.clone();
    }
    // The renderer is chosen when the window is realized; the variable is removed again
    // afterwards so programs started in the terminals don't inherit it.
    let set_renderer = cfg.renderer == "software" && std::env::var_os("GSK_RENDERER").is_none();
    if set_renderer {
        // SAFETY: single-threaded at this point.
        unsafe { std::env::set_var("GSK_RENDERER", "cairo") };
    }

    let mut flags = gio::ApplicationFlags::HANDLES_COMMAND_LINE;
    if ephemeral {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let gapp = gtk::Application::builder().application_id(APP_ID).flags(flags).build();
    gapp.connect_startup(|_| {
        // Only the primary instance gets here; a forwarding `vibey .` exits without GTK.
        highlight::preload();
        icons::register_fonts();
        if let Some(s) = gtk::Settings::default() {
            // GTK's built-in theme, so the user's GTK theme can't leak into ours.
            s.set_gtk_theme_name(Some("Default"));
            // Fractional font sizes + subpixel positioning, closer to Chromium's text.
            s.set_gtk_hint_font_metrics(false);
        }
    });
    let cfg = Rc::new(cfg);
    gapp.connect_command_line(move |gapp, cmdline| {
        let argv: Vec<String> = cmdline.arguments().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let cwd = cmdline.cwd().unwrap_or_else(|| PathBuf::from("/"));
        let args = parse_args(&argv, &cwd);
        match APP.with(|a| a.borrow().clone()) {
            Some(app) => {
                // A later `vibey DIR` (often from a terminal inside Vibey).
                open_args(&app, &args);
                app.window.present();
            }
            None => first_launch(gapp, &cfg, args, ephemeral, set_renderer),
        }
        glib::ExitCode::SUCCESS
    });
    gapp.run_with_args(&argv)
}

fn open_args(app: &Rc<app::App>, args: &Args) {
    let mut first = None;
    for root in &args.roots {
        match app.add_project(root) {
            Some(p) => {
                first.get_or_insert(p);
            }
            None => eprintln!("vibey: not a directory: {}", root.display()),
        }
    }
    if let (Some(file), Some(p)) = (&args.open, first.or_else(|| app.active_project())) {
        let path = if file.is_absolute() { file.clone() } else { p.root.join(file) };
        p.open_file(&path, true);
    }
}

fn first_launch(gapp: &gtk::Application, cfg: &config::Config, args: Args, ephemeral: bool, set_renderer: bool) {
    let app = app::App::new(gapp, cfg.clone(), ephemeral);
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));
    if !ephemeral {
        app.restore(&session::load());
    }
    open_args(&app, &args);

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
                p.terminals.add_terminal(&app::current_theme(), None);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_resolve_against_caller_cwd() {
        let argv: Vec<String> = ["vibey", ".", "../other", "/abs", "--open", "src/main.rs"].map(String::from).to_vec();
        let a = parse_args(&argv, Path::new("/home/u/proj"));
        assert_eq!(a.roots, vec![PathBuf::from("/home/u/proj/."), "/home/u/proj/../other".into(), "/abs".into()]);
        assert_eq!(a.open, Some(PathBuf::from("src/main.rs")));
    }
}
