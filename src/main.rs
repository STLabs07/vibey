mod codeview;
mod highlight;
mod icons;
mod terminal;
mod theme;
mod tree;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};

use theme::ThemeModel;

const APP_ID: &str = "dev.vibey.Vibey";

pub fn config_dir() -> PathBuf {
    glib::user_config_dir().join("vibey")
}

pub fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("vibey")
}

struct Args {
    root: PathBuf,
    open: Option<PathBuf>,
    measure_startup: bool,
    exit_after_startup: bool,
    terminals: usize,
    theme: String,
}

fn parse_args() -> Args {
    let mut args = Args {
        root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        open: None,
        measure_startup: false,
        exit_after_startup: false,
        terminals: 2,
        theme: theme::DEFAULT_THEME.to_string(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--measure-startup" => args.measure_startup = true,
            "--exit-after-startup" => args.exit_after_startup = true,
            "--open" => args.open = it.next().map(PathBuf::from),
            "--terminals" => args.terminals = it.next().and_then(|n| n.parse().ok()).unwrap_or(2),
            "--theme" => args.theme = it.next().unwrap_or(args.theme),
            _ => args.root = PathBuf::from(a),
        }
    }
    args.root = args.root.canonicalize().unwrap_or(args.root);
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

fn ui_scale() -> f64 {
    let dpi = gtk::Settings::default().map(|s| s.gtk_xft_dpi()).unwrap_or(-1);
    if dpi > 0 {
        dpi as f64 / 1024.0 / 96.0
    } else {
        1.0
    }
}

/// Expand `{{n}}` (rounded) and `{{~n}}` (fractional) placeholders to scaled px.
fn scale_css(src: &str, scale: f64) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find("}}") else { break };
        let inner = rest[i + 2..i + j].trim();
        let (exact, num) = match inner.strip_prefix('~') {
            Some(n) => (true, n),
            None => (false, inner),
        };
        let px = num.parse::<f64>().unwrap_or(0.0) * scale;
        out.push_str(&if exact { format!("{px:.2}px") } else { format!("{}px", px.round()) });
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    if !class.is_empty() {
        l.add_css_class(class);
    }
    l
}

fn icon_label(name: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.add_css_class("seti");
    l.set_markup(&icons::markup(icons::for_file(name)));
    l
}

fn build_ui(app: &gtk::Application, args: &Args) {
    let scale = ui_scale();
    let model = ThemeModel::load(&args.theme).unwrap_or_else(|e| {
        eprintln!("vibey: {e}; using built-in default theme");
        ThemeModel::load(theme::DEFAULT_THEME).expect("built-in theme")
    });
    let css = gtk::CssProvider::new();
    css.load_from_string(&(model.css_colors() + &scale_css(include_str!("../assets/style.css"), scale)));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().unwrap(),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let syn_theme = Arc::new(model.to_syntect());
    let brackets = Arc::new(model.bracket_colors());

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(format!("{} — Vibey", args.root.file_name().unwrap_or_default().to_string_lossy()))
        .default_width(1400)
        .default_height(900)
        .build();
    window.add_css_class("vibey");

    // Rail
    let rail = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rail.add_css_class("rail");
    let project_btn = gtk::Box::new(gtk::Orientation::Vertical, 0);
    project_btn.add_css_class("project");
    project_btn.add_css_class("active");
    let initial = args.root.file_name().and_then(|n| n.to_str()).and_then(|n| n.chars().next()).unwrap_or('?');
    let avatar = label(&initial.to_uppercase().to_string(), "avatar");
    avatar.set_valign(gtk::Align::Center);
    avatar.set_halign(gtk::Align::Center);
    avatar.set_vexpand(true);
    project_btn.set_vexpand(false);
    project_btn.append(&avatar);
    rail.append(&project_btn);

    // Code view + tab + breadcrumb
    let code = codeview::CodeView::new(syn_theme, brackets, &model, scale);
    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tabs.add_css_class("tabs");
    let tab = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tab.add_css_class("tab");
    tab.add_css_class("active");
    let tab_icon = icon_label("");
    let tab_name = label("", "");
    tab.append(&tab_icon);
    tab.append(&tab_name);
    let close = label(icons::codicon::CLOSE, "close");
    close.add_css_class("codicon");
    tab.append(&close);
    tabs.append(&tab);
    let breadcrumb = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    breadcrumb.add_css_class("breadcrumb");
    let editor = gtk::Box::new(gtk::Orientation::Vertical, 0);
    editor.append(&tabs);
    editor.append(&breadcrumb);
    editor.append(&code.widget);

    // Status bar
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status.add_css_class("statusbar");
    let branch = label("", "");
    status.append(&branch);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    status.append(&spacer);
    for s in ["Ln 1, Col 1", "Spaces: 4", "UTF-8", "LF"] {
        status.append(&label(s, ""));
    }
    let lang = label("", "");
    status.append(&lang);

    let root = args.root.clone();
    let open: Rc<dyn Fn(PathBuf)> = {
        let code = code.clone();
        let (tab_icon, tab_name, breadcrumb, lang) = (tab_icon.clone(), tab_name.clone(), breadcrumb.clone(), lang.clone());
        Rc::new(move |path: PathBuf| {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            tab_name.set_text(&name);
            tab_icon.set_markup(&icons::markup(icons::for_file(&name)));
            while let Some(c) = breadcrumb.first_child() {
                breadcrumb.remove(&c);
            }
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            let parts: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
            for (i, p) in parts.iter().enumerate() {
                if i + 1 == parts.len() {
                    breadcrumb.append(&icon_label(p));
                }
                breadcrumb.append(&label(p, ""));
                if i + 1 < parts.len() {
                    let sep = label(icons::codicon::CHEVRON_RIGHT, "sep");
                    sep.add_css_class("codicon");
                    breadcrumb.append(&sep);
                }
            }
            let lang = lang.clone();
            code.load(path, move |syntax| lang.set_text(syntax));
        })
    };

    // Sidebar
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.add_css_class("sidebar");
    let title = label("Explorer", "title");
    title.set_xalign(0.0);
    sidebar.append(&title);
    let section = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    section.add_css_class("section");
    let chevron = label(icons::codicon::CHEVRON_DOWN, "codicon");
    section.append(&chevron);
    section.append(&label(&args.root.file_name().unwrap_or_default().to_string_lossy(), ""));
    sidebar.append(&section);
    sidebar.append(&tree::build(&args.root, open.clone()));

    // Terminal panel
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add_css_class("panel");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.add_css_class("panel-header");
    let term_tab = label("Terminal", "panel-tab");
    term_tab.add_css_class("active");
    header.append(&term_tab);
    let hspacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hspacer.set_hexpand(true);
    header.append(&hspacer);
    let stack = gtk::Stack::new();
    let shell_name = std::env::var("SHELL").ok().and_then(|s| s.rsplit('/').next().map(str::to_string)).unwrap_or("sh".into());
    for i in 0..args.terminals {
        let t = terminal::new_terminal(&args.root, &model, scale);
        stack.add_named(&t, Some(&i.to_string()));
        let chip = label(&format!("{}: {shell_name}", i + 1), "term-tab");
        if i == 0 {
            chip.add_css_class("active");
        }
        header.append(&chip);
    }
    panel.append(&header);
    panel.append(&stack);

    let vpaned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Vertical)
        .start_child(&editor)
        .end_child(&panel)
        .resize_end_child(false)
        .shrink_end_child(false)
        .position(560)
        .build();
    let hpaned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&sidebar)
        .end_child(&vpaned)
        .resize_start_child(false)
        .shrink_start_child(false)
        .position((300.0 * scale) as i32)
        .build();
    hpaned.set_hexpand(true);
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    main.set_vexpand(true);
    main.append(&rail);
    main.append(&hpaned);
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.append(&main);
    outer.append(&status);
    window.set_child(Some(&outer));

    // Branch, off the UI thread (git is only shelled out; never holds index.lock).
    let root = args.root.clone();
    glib::spawn_future_local(async move {
        let out = gio::spawn_blocking(move || {
            std::process::Command::new("git")
                .args(["--no-optional-locks", "rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(&root)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .await
        .ok()
        .flatten();
        if let Some(b) = out {
            branch.set_markup(&format!(
                "<span font_family=\"codicon\">{}</span> {}",
                icons::codicon::GIT_BRANCH,
                glib::markup_escape_text(&b)
            ));
        }
    });

    let initial = args.open.clone().or_else(|| {
        ["README.md", "src/main.rs", "main.py", "index.ts", "package.json"]
            .iter()
            .map(|f| args.root.join(f))
            .find(|p| p.is_file())
    });
    if let Some(p) = initial {
        open(if p.is_absolute() { p } else { args.root.join(p) });
    }

    if args.measure_startup {
        let exit = args.exit_after_startup;
        let app = app.clone();
        window.add_tick_callback(move |_, clock| {
            let done = Rc::new(Cell::new(false));
            let app = app.clone();
            clock.connect_after_paint(move |_| {
                if !done.replace(true) {
                    eprintln!("vibey: first frame after {:.0} ms", ms_since_exec());
                    if exit {
                        let app = app.clone();
                        glib::idle_add_local_once(move || app.quit());
                    }
                }
            });
            glib::ControlFlow::Break
        });
    }
    window.present();
}

fn main() -> glib::ExitCode {
    let args = parse_args();
    highlight::preload();
    icons::register_fonts();

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let args = Rc::new(args);
    app.connect_startup(|_| {
        // Use GTK's built-in theme so the user's GTK theme can't leak into ours.
        if let Some(s) = gtk::Settings::default() {
            s.set_gtk_theme_name(Some("Default"));
            s.set_gtk_application_prefer_dark_theme(true);
            // Fractional font sizes + subpixel positioning, closer to Chromium's text.
            s.set_gtk_hint_font_metrics(false);
        }
    });
    app.connect_activate(move |app| build_ui(app, &args));
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}
