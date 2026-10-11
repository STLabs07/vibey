//! The window: project rail, project pages, status bar, quick pick, shortcuts, themes.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::codeview::{SharedStyle, ViewStyle};
use crate::config::{self, Config};
use crate::editor::Status;
use crate::icons::codicon;
use crate::project::{FileProvider, Project};
use crate::procmon::{self, Prev};
use crate::quickopen::{self, Item, Provider, QuickPick};
use crate::resources::{ProjectUsage, ResourcePanel};
use crate::session::{self, ProjectState, Session};
use crate::theme::{self, Kind, ThemeModel};

thread_local! {
    static THEME: RefCell<Option<Rc<ThemeModel>>> = const { RefCell::new(None) };
}

/// The theme currently applied (UI thread only).
pub fn current_theme() -> Rc<ThemeModel> {
    THEME.with(|t| t.borrow().clone()).expect("theme applied at startup")
}

const AVATAR_COLORS: usize = 8;

pub struct App {
    pub window: gtk::ApplicationWindow,
    rail_projects: gtk::Box,
    stack: gtk::Stack,
    status_project: gtk::Label,
    status_branch: gtk::Label,
    /// Title bar: total resources (top right) and the search box (center).
    res_label: gtk::Label,
    search_label: gtk::Label,
    status_right: gtk::Box,
    status_pos: gtk::Label,
    status_indent: gtk::Label,
    status_eol: gtk::Label,
    status_lang: gtk::Label,
    projects: RefCell<Vec<(Rc<Project>, gtk::Box)>>,
    active: Cell<Option<usize>>,
    style: SharedStyle,
    css: gtk::CssProvider,
    config: RefCell<Config>,
    quick: Rc<QuickPick>,
    resources: Rc<ResourcePanel>,
    prev_sample: RefCell<Prev>,
    sampling: Cell<bool>,
    scale: f64,
    chord: Cell<bool>,
    allow_close: Cell<bool>,
    /// Test/measurement runs: don't read or write the user's session.
    ephemeral: bool,
    save_pending: Cell<bool>,
    me: RefCell<Weak<App>>,
}

fn ui_scale() -> f64 {
    let dpi = gtk::Settings::default().map(|s| s.gtk_xft_dpi()).unwrap_or(-1);
    if dpi > 0 { dpi as f64 / 1024.0 / 96.0 } else { 1.0 }
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

/// CPU share with enough precision to never read as an ambiguous "0%".
fn fmt_pct(p: f32) -> String {
    if p < 10.0 { format!("{p:.1}%") } else { format!("{p:.0}%") }
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    if !class.is_empty() {
        l.add_css_class(class);
    }
    l
}

fn rail_button(glyph: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::with_label(glyph);
    b.add_css_class("rail-button");
    b.add_css_class("codicon");
    b.set_tooltip_text(Some(tooltip));
    b.set_focus_on_click(false);
    b.set_cursor_from_name(Some("pointer"));
    b
}

fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| c == '-' || c == '_' || c == ' ' || c == '.').filter(|w| !w.is_empty()).collect();
    let s: String = match words.as_slice() {
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
        [a] => a.chars().take(1).collect(),
        [] => "?".into(),
    };
    s.to_uppercase()
}

impl App {
    pub fn new(app: &gtk::Application, cfg: Config, ephemeral: bool) -> Rc<App> {
        let scale = ui_scale();
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Vibey")
            .default_width(1400)
            .default_height(900)
            .build();
        window.add_css_class("vibey");

        // Title bar (VS Code style): search box in the middle, total resources on the right.
        let header = gtk::HeaderBar::new();
        header.add_css_class("titlebar-vibey");
        let search_btn = gtk::Button::new();
        search_btn.add_css_class("command-center");
        search_btn.set_focus_on_click(false);
        search_btn.set_tooltip_text(Some("Go to File (Ctrl+P). To search inside files, use the box in the Explorer (Ctrl+Shift+F)."));
        let sb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let search_icon = label(codicon::SEARCH, "codicon");
        let search_label = label("Search files", "cc-label");
        search_label.set_hexpand(true);
        search_label.set_xalign(0.0);
        search_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let search_keys = label("Ctrl+P", "cc-keys");
        sb.append(&search_icon);
        sb.append(&search_label);
        sb.append(&search_keys);
        search_btn.set_child(Some(&sb));
        search_btn.set_size_request((480.0 * scale) as i32, -1);
        header.set_title_widget(Some(&search_btn));
        let res_button = gtk::Button::new();
        res_button.add_css_class("res-button");
        res_button.set_focus_on_click(false);
        let res_label = label("Resources used: …", "");
        res_button.set_child(Some(&res_label));
        header.pack_end(&res_button);
        window.set_titlebar(Some(&header));

        let css = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().expect("display"),
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // Rail
        let rail = gtk::Box::new(gtk::Orientation::Vertical, 0);
        rail.add_css_class("rail");
        let rail_projects = gtk::Box::new(gtk::Orientation::Vertical, 0);
        rail.append(&rail_projects);
        let add = rail_button(codicon::ADD, "Open Folder as Project…");
        add.add_css_class("add");
        if let Some(c) = add.child() {
            // Keep the dashed slot square instead of stretching to the rail width.
            c.set_halign(gtk::Align::Center);
            c.set_valign(gtk::Align::Center);
        }
        rail.append(&add);
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        rail.append(&spacer);
        let gear = gtk::MenuButton::builder().tooltip_text("Settings").build();
        let gear_icon = label(codicon::SETTINGS_GEAR, "codicon");
        gear.set_child(Some(&gear_icon));
        gear.add_css_class("rail-menu");
        gear.set_cursor_from_name(Some("pointer"));
        rail.append(&gear);

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        let welcome = gtk::Box::new(gtk::Orientation::Vertical, 8);
        welcome.add_css_class("watermark");
        welcome.set_valign(gtk::Align::Center);
        welcome.set_halign(gtk::Align::Center);
        welcome.append(&label("No project open", "what"));
        let open_btn = gtk::Button::with_label("Open Folder…");
        open_btn.add_css_class("primary");
        welcome.append(&open_btn);
        stack.add_named(&welcome, Some("welcome"));

        // Status bar
        let status = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        status.add_css_class("statusbar");
        let status_project = label("", "");
        let status_branch = label("", "");
        status.append(&status_project);
        status.append(&status_branch);
        let sp = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        sp.set_hexpand(true);
        status.append(&sp);
        let status_right = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let (status_pos, status_indent, status_eol, status_lang) = (label("", ""), label("", ""), label("", ""), label("", ""));
        for l in [&status_pos, &status_indent, &label("UTF-8", ""), &status_eol, &status_lang] {
            status_right.append(l);
        }
        status.append(&status_right);

        let main = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        main.set_vexpand(true);
        main.append(&rail);
        main.append(&stack);
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.append(&main);
        outer.append(&status);

        let quick = QuickPick::new(scale);
        let resources = ResourcePanel::new(scale);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&outer));
        overlay.add_overlay(&resources.widget);
        overlay.add_overlay(&quick.widget);
        window.set_child(Some(&overlay));

        let style = Rc::new(RefCell::new(ViewStyle {
            theme_id: String::new(),
            brackets: Arc::new(crate::highlight::BracketColors { cycle: [(0, 0, 0, 0); 3], unexpected: (0, 0, 0, 0) }),
            line_number: theme::Rgba { r: 0, g: 0, b: 0, a: 0 },
            changed: theme::Rgba { r: 0, g: 0, b: 0, a: 0 },
            find_match: theme::Rgba { r: 0, g: 0, b: 0, a: 0 },
            find_current: theme::Rgba { r: 0, g: 0, b: 0, a: 0 },
            git_added: theme::Rgba { r: 0, g: 0, b: 0, a: 0 },
            git_modified: theme::Rgba { r: 0, g: 0, b: 0, a: 0 },
            git_deleted: theme::Rgba { r: 0, g: 0, b: 0, a: 0 },
            scale,
        }));

        let app_rc = Rc::new(App {
            window,
            rail_projects,
            stack,
            status_project,
            status_branch,
            res_label,
            search_label,
            status_right,
            status_pos,
            status_indent,
            status_eol,
            status_lang,
            projects: RefCell::new(Vec::new()),
            active: Cell::new(None),
            style,
            css,
            config: RefCell::new(cfg),
            quick,
            resources,
            prev_sample: RefCell::new(Prev::default()),
            sampling: Cell::new(false),
            scale,
            chord: Cell::new(false),
            allow_close: Cell::new(false),
            ephemeral,
            save_pending: Cell::new(false),
            me: RefCell::new(Weak::new()),
        });
        *app_rc.me.borrow_mut() = Rc::downgrade(&app_rc);
        let theme_id = app_rc.config.borrow().theme.clone();
        app_rc.apply_theme(&theme_id, false);

        let me = Rc::downgrade(&app_rc);
        add.connect_clicked(move |_| {
            if let Some(a) = me.upgrade() {
                a.pick_folder();
            }
        });
        let me = Rc::downgrade(&app_rc);
        open_btn.connect_clicked(move |_| {
            if let Some(a) = me.upgrade() {
                a.pick_folder();
            }
        });
        gear.set_popover(Some(&app_rc.settings_popover()));
        let me = Rc::downgrade(&app_rc);
        app_rc.window.connect_is_active_notify(move |w| {
            if let Some(a) = me.upgrade() {
                a.update_visibility();
                if w.is_active() {
                    if let Some(p) = a.active_project() {
                        p.refresh_git();
                    }
                    a.start_sampling();
                }
            }
        });
        let me = Rc::downgrade(&app_rc);
        res_button.connect_clicked(move |_| {
            if let Some(a) = me.upgrade() {
                a.toggle_resources();
            }
        });
        let me = Rc::downgrade(&app_rc);
        search_btn.connect_clicked(move |_| {
            if let Some(a) = me.upgrade() {
                a.open_file_search();
            }
        });
        let me = Rc::downgrade(&app_rc);
        app_rc.resources.set_on_killed(Rc::new(move || {
            if let Some(a) = me.upgrade() {
                a.sample_once();
            }
        }));
        app_rc.install_shortcuts();
        app_rc.install_close_guard();
        app_rc.update_status();
        app_rc
    }

    fn settings_popover(self: &Rc<Self>) -> gtk::Popover {
        let pop = gtk::Popover::new();
        pop.set_position(gtk::PositionType::Right);
        pop.add_css_class("menu-popover");
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let theme_btn = gtk::Button::with_label("Color Theme…   Ctrl+K Ctrl+T");
        theme_btn.add_css_class("flat");
        let me = Rc::downgrade(self);
        let p = pop.clone();
        theme_btn.connect_clicked(move |_| {
            p.popdown();
            if let Some(a) = me.upgrade() {
                a.open_theme_picker();
            }
        });
        b.append(&theme_btn);
        b.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        b.append(&label("Renderer (applies on restart)", "dim"));
        let gpu = gtk::CheckButton::with_label("GPU (smoother)");
        let sw = gtk::CheckButton::with_label("Software (≈25 MB less memory)");
        sw.set_group(Some(&gpu));
        if self.config.borrow().renderer == "software" { sw.set_active(true) } else { gpu.set_active(true) }
        for (btn, value) in [(&gpu, "gpu"), (&sw, "software")] {
            let me = Rc::downgrade(self);
            btn.connect_toggled(move |b| {
                if let (true, Some(a)) = (b.is_active(), me.upgrade()) {
                    a.config.borrow_mut().renderer = value.into();
                    config::save(&a.config.borrow());
                }
            });
            b.append(btn);
        }
        pop.set_child(Some(&b));
        pop
    }

    // ---- projects ----

    pub fn add_project(self: &Rc<Self>, root: &Path) -> Option<Rc<Project>> {
        let root = root.canonicalize().ok().filter(|p| p.is_dir())?;
        if let Some(i) = self.projects.borrow().iter().position(|(p, _)| p.root == root) {
            self.activate(i);
            return Some(self.projects.borrow()[i].0.clone());
        }
        let project = Project::new(&root, self.style.clone(), self.scale);
        let me = Rc::downgrade(self);
        let weak_p = Rc::downgrade(&project);
        project.editor.set_on_status(Rc::new(move |st| {
            if let (Some(a), Some(p)) = (me.upgrade(), weak_p.upgrade()) {
                if a.active_project().is_some_and(|ap| Rc::ptr_eq(&ap, &p)) {
                    a.show_status(st);
                }
            }
        }));
        self.stack.add_child(&project.page);
        if let Some(s) = project.search.get() {
            s.set_highlight_color(current_theme().get("list.highlightForeground").hex());
        }
        let me = Rc::downgrade(self);
        let weak_p = Rc::downgrade(&project);
        project.terminals.set_on_event(Rc::new(move |ev| {
            if let (Some(a), Some(p)) = (me.upgrade(), weak_p.upgrade()) {
                a.on_terminal_event(&p, ev);
            }
        }));
        let me = Rc::downgrade(self);
        project.editor.set_on_tabs_changed(Rc::new(move || {
            if let Some(a) = me.upgrade() {
                a.schedule_save();
            }
        }));
        let me = Rc::downgrade(self);
        let weak_p = Rc::downgrade(&project);
        project.set_on_git(Rc::new(move || {
            if let (Some(a), Some(p)) = (me.upgrade(), weak_p.upgrade()) {
                a.update_badges();
                if a.active_project().is_some_and(|ap| Rc::ptr_eq(&ap, &p)) {
                    a.show_branch(&p);
                }
            }
        }));

        let n = self.projects.borrow().len();
        let item = gtk::Box::new(gtk::Orientation::Vertical, 0);
        item.add_css_class("project");
        item.set_tooltip_text(Some(&format!("{}\n{}", project.name, root.display())));
        item.set_cursor_from_name(Some("pointer"));
        let avatar = label(&initials(&project.name), "avatar");
        let hash = project.name.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
        avatar.add_css_class(&format!("c{}", hash % AVATAR_COLORS));
        let badge = label("", "badge");
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::Start);
        badge.set_can_target(false);
        let dirty = label("", "dirty");
        dirty.set_halign(gtk::Align::End);
        dirty.set_valign(gtk::Align::End);
        dirty.set_can_target(false);
        let holder = gtk::Overlay::new();
        holder.set_child(Some(&avatar));
        holder.add_overlay(&badge);
        holder.add_overlay(&dirty);
        holder.set_halign(gtk::Align::Center);
        holder.set_valign(gtk::Align::Center);
        holder.set_margin_top((10.0 * self.scale) as i32);
        holder.set_margin_bottom((10.0 * self.scale) as i32);
        item.set_valign(gtk::Align::Start);
        item.append(&holder);
        let chip = label("", "res-chip");
        chip.set_visible(false);
        item.append(&chip);
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let me = Rc::downgrade(self);
        let weak_p = Rc::downgrade(&project);
        let it = item.clone();
        click.connect_pressed(move |g, _, x, y| {
            // A Box has no :active state of its own; set it for the press feedback.
            it.set_state_flags(gtk::StateFlags::ACTIVE, false);
            let (Some(a), Some(p)) = (me.upgrade(), weak_p.upgrade()) else { return };
            let Some(i) = a.index_of(&p) else { return };
            if g.current_button() == 3 {
                a.project_menu(i, &it, x, y);
            } else {
                a.activate(i);
            }
        });
        let it = item.clone();
        click.connect_released(move |_, _, _, _| it.unset_state_flags(gtk::StateFlags::ACTIVE));
        let it = item.clone();
        click.connect_stopped(move |_| it.unset_state_flags(gtk::StateFlags::ACTIVE));
        item.add_controller(click);
        self.rail_projects.append(&item);
        self.projects.borrow_mut().push((project.clone(), item));
        self.activate(n);
        Some(project)
    }

    fn index_of(&self, p: &Rc<Project>) -> Option<usize> {
        self.projects.borrow().iter().position(|(x, _)| Rc::ptr_eq(x, p))
    }

    pub fn active_project(&self) -> Option<Rc<Project>> {
        self.active.get().and_then(|i| self.projects.borrow().get(i).map(|(p, _)| p.clone()))
    }

    pub fn activate(self: &Rc<Self>, i: usize) {
        let Some((project, item)) = self.projects.borrow().get(i).cloned() else { return };
        if let Some(prev) = self.active.get().and_then(|j| self.projects.borrow().get(j).map(|(_, it)| it.clone())) {
            prev.remove_css_class("active");
        }
        item.add_css_class("active");
        self.active.set(Some(i));
        self.stack.set_visible_child(&project.page);
        self.search_label.set_text(&format!("Search files in {}", project.name));
        self.update_visibility();
        // Terminals start once the window is realized (see `on_realized`).
        if self.window.is_realized() && !project.terminals_started.replace(true) {
            project.terminals.start(&current_theme());
        }
        self.schedule_save();
        self.update_status();
        self.show_branch(&project);
        project.refresh_git();
    }

    /// Start the active project's terminal once the window exists (see main: GSK_RENDERER).
    pub fn on_realized(self: &Rc<Self>) {
        self.start_sampling();
        if let Some(p) = self.active_project() {
            if !p.terminals_started.replace(true) {
                p.terminals.start(&current_theme());
            }
        }
    }

    fn project_menu(self: &Rc<Self>, i: usize, anchor: &gtk::Box, x: f64, y: f64) {
        let pop = gtk::Popover::new();
        pop.set_parent(anchor);
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.set_position(gtk::PositionType::Right);
        let b = gtk::Button::with_label("Close Project");
        b.add_css_class("flat");
        let me = Rc::downgrade(self);
        let p = pop.clone();
        b.connect_clicked(move |_| {
            p.popdown();
            if let Some(a) = me.upgrade() {
                a.close_project(i);
            }
        });
        pop.set_child(Some(&b));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    fn close_project(self: &Rc<Self>, i: usize) {
        let Some((project, _)) = self.projects.borrow().get(i).cloned() else { return };
        let jobs = project.running_jobs();
        if jobs.is_empty() {
            self.remove_project(i);
            return;
        }
        let me = Rc::downgrade(self);
        self.confirm(
            &format!("Close {}?", project.name),
            &format!("Still running: {}. Closing the project stops them.", jobs.join(", ")),
            "Close Project",
            move || {
                if let Some(a) = me.upgrade() {
                    a.remove_project(i);
                }
            },
        );
    }

    fn remove_project(self: &Rc<Self>, i: usize) {
        if i >= self.projects.borrow().len() {
            return;
        }
        let (project, item) = self.projects.borrow_mut().remove(i);
        self.rail_projects.remove(&item);
        self.stack.remove(&project.page);
        let n = self.projects.borrow().len();
        self.active.set(None);
        self.schedule_save();
        if n == 0 {
            self.stack.set_visible_child_name("welcome");
            self.update_title();
            self.update_status();
        } else {
            self.activate(i.min(n - 1));
        }
    }

    fn pick_folder(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("Open Folder as Project").modal(true).build();
        let me = Rc::downgrade(self);
        dialog.select_folder(Some(&self.window), None::<&gio::Cancellable>, move |res| {
            if let (Ok(folder), Some(a)) = (res, me.upgrade()) {
                if let Some(path) = folder.path() {
                    a.add_project(&path);
                }
            }
        });
    }

    fn confirm(&self, heading: &str, body: &str, action: &str, then: impl Fn() + 'static) {
        let dialog = gtk::AlertDialog::builder()
            .message(heading)
            .detail(body)
            .buttons(["Cancel", action])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        dialog.choose(Some(&self.window), None::<&gio::Cancellable>, move |res| {
            if res == Ok(1) {
                then();
            }
        });
    }

    fn install_close_guard(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        self.window.connect_close_request(move |w| {
            let Some(a) = me.upgrade() else { return glib::Propagation::Proceed };
            if a.allow_close.get() {
                a.save_session();
                return glib::Propagation::Proceed;
            }
            let jobs: Vec<String> = a
                .projects
                .borrow()
                .iter()
                .flat_map(|(p, _)| p.running_jobs().into_iter().map(|j| format!("{j} ({})", p.name)).collect::<Vec<_>>())
                .collect();
            if jobs.is_empty() {
                a.save_session();
                return glib::Propagation::Proceed;
            }
            let w = w.clone();
            let me2 = Rc::downgrade(&a);
            a.confirm(
                "Close Vibey?",
                &format!("Still running: {}. Closing Vibey stops them.", jobs.join(", ")),
                "Close Anyway",
                move || {
                    if let Some(a) = me2.upgrade() {
                        a.allow_close.set(true);
                    }
                    w.close();
                },
            );
            glib::Propagation::Stop
        });
    }

    // ---- terminal status → rail badges, title, notifications ----

    fn on_terminal_event(self: &Rc<Self>, project: &Rc<Project>, ev: crate::terminal::Event) {
        // Terminal activity (an AI CLI working, a command finishing) usually means files changed.
        project.refresh_git();
        self.schedule_save();
        if let crate::terminal::Event::Finished(job) = &ev {
            let unfocused = !self.window.is_active();
            if unfocused && self.config.borrow().notifications {
                let n = gio::Notification::new(&format!("{job} finished in {}", project.name));
                n.set_body(Some("Vibey: a terminal went quiet while you were away."));
                if let Some(app) = self.window.application() {
                    app.send_notification(Some(&format!("finished-{}", project.name)), &n);
                }
            }
        }
        self.update_badges();
    }

    /// Only the active project's panel is "visible", and only while the window is focused.
    pub fn update_visibility(&self) {
        let active = self.active_project();
        let focused = self.window.is_active();
        for (p, _) in self.projects.borrow().iter() {
            let on_screen = active.as_ref().is_some_and(|a| Rc::ptr_eq(a, p));
            p.terminals.set_visible(on_screen && focused);
        }
        self.update_badges();
    }

    fn update_badges(&self) {
        let mut attention_anywhere = false;
        for (p, item) in self.projects.borrow().iter() {
            let (running, attention, job) = p.terminals.summary();
            attention_anywhere |= attention;
            // holder overlay: avatar, badge, dirty
            let Some(holder) = item.first_child().and_downcast::<gtk::Overlay>() else { continue };
            let Some(badge) = holder.first_child().and_then(|a| a.next_sibling()).and_downcast::<gtk::Label>() else {
                continue;
            };
            if let Some(dirty) = badge.next_sibling() {
                let n = p.git.borrow().changed();
                if n > 0 { dirty.add_css_class("on") } else { dirty.remove_css_class("on") }
            }
            for c in ["running", "attention"] {
                badge.remove_css_class(c);
            }
            let mut tip = format!("{}\n{}", p.name, p.root.display());
            {
                let st = p.git.borrow();
                if let Some(b) = &st.branch {
                    tip.push_str(&format!("\n⎇ {b}"));
                    if st.changed() > 0 {
                        tip.push_str(&format!(" · {} changed", st.changed()));
                    }
                }
            }
            if attention {
                badge.add_css_class("attention");
                tip.push_str(&format!("\n● {} finished", job.unwrap_or_default()));
            } else if running {
                badge.add_css_class("running");
                tip.push_str("\n◌ working…");
            }
            item.set_tooltip_text(Some(&tip));
        }
        self.update_title_with(attention_anywhere);
    }

    fn update_title(&self) {
        let any = self.projects.borrow().iter().any(|(p, _)| p.terminals.summary().1);
        self.update_title_with(any);
    }

    fn update_title_with(&self, attention: bool) {
        let name = self.active_project().map(|p| format!("{} — Vibey", p.name)).unwrap_or_else(|| "Vibey".into());
        let title = if attention { format!("● {name}") } else { name };
        self.window.set_title(Some(&title));
    }

    // ---- session ----

    /// Reopen projects, files and terminals from the last session.
    pub fn restore(self: &Rc<Self>, s: &Session) {
        if s.width > 0 && s.height > 0 {
            self.window.set_default_size(s.width, s.height);
        }
        if s.maximized {
            self.window.maximize();
        }
        for ps in &s.projects {
            let Some(p) = self.add_project(&ps.root) else { continue };
            p.terminals.set_restore(ps.terminals.clone());
            for f in ps.files.iter().filter(|f| f.is_file()) {
                p.open_file(f, true);
            }
            if let Some(f) = ps.active_file.as_ref().filter(|f| f.is_file()) {
                p.open_file(f, true);
            }
        }
        if !s.projects.is_empty() {
            self.activate(s.active.min(self.projects.borrow().len().saturating_sub(1)));
        }
    }

    fn snapshot(&self) -> Session {
        let projects = self
            .projects
            .borrow()
            .iter()
            .map(|(p, _)| {
                let (files, active_file) = p.editor.session();
                let terminals = if p.terminals_started.get() { p.terminals.cwds() } else { p.terminals.pending_restore() };
                ProjectState { root: p.root.clone(), files, active_file, terminals }
            })
            .collect();
        let (width, height) = self.window.default_size();
        Session { projects, active: self.active.get().unwrap_or(0), width, height, maximized: self.window.is_maximized() }
    }

    pub fn save_session(&self) {
        if !self.ephemeral {
            session::save(&self.snapshot());
        }
    }

    /// Debounced save after projects, tabs or terminals change.
    fn schedule_save(self: &Rc<Self>) {
        if self.ephemeral || self.save_pending.replace(true) {
            return;
        }
        let me = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(1000), move || {
            if let Some(a) = me.upgrade() {
                a.save_pending.set(false);
                a.save_session();
            }
        });
    }

    // ---- resource sampling (only while the window is focused) ----

    pub fn start_sampling(self: &Rc<Self>) {
        if !self.sampling.replace(true) {
            self.sample_once();
        }
    }

    fn sample_once(self: &Rc<Self>) {
        let projects: Vec<(String, Vec<(String, i32)>)> =
            self.projects.borrow().iter().map(|(p, _)| (p.name.clone(), p.terminals.shells())).collect();
        let pids: Vec<i32> = projects.iter().flat_map(|(_, t)| t.iter().map(|(_, pid)| *pid)).collect();
        let prev = self.prev_sample.borrow().clone();
        let me = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || procmon::sample(&pids, &prev)).await;
            let Some(a) = me.upgrade() else { return };
            if let Ok((report, next)) = res {
                *a.prev_sample.borrow_mut() = next;
                a.show_usage(report, projects);
            }
            // Keep going only while someone can see it.
            if a.window.is_active() && a.window.is_visible() {
                let interval = if a.resources.is_open() { 1000 } else { 2000 };
                let me = Rc::downgrade(&a);
                glib::timeout_add_local_once(Duration::from_millis(interval), move || {
                    if let Some(a) = me.upgrade() {
                        a.sample_once();
                    }
                });
            } else {
                a.sampling.set(false);
            }
        });
    }

    fn show_usage(&self, report: procmon::Report, projects: Vec<(String, Vec<(String, i32)>)>) {
        let mut roots = report.roots.iter();
        let usage: Vec<ProjectUsage> = projects
            .into_iter()
            .map(|(name, terms)| ProjectUsage {
                name,
                terminals: terms.into_iter().map(|(label, _)| (label, roots.next().cloned().flatten())).collect(),
            })
            .collect();
        // One unambiguous number: everything Vibey and all projects' terminals use, with CPU
        // as a share of the whole machine (all cores = 100%).
        let cores = report.cpus.max(1) as f32;
        let machine_cpu = report.total_cpu / cores;
        let ram_share = report.total_mem_kb as f64 / report.mem_total_kb.max(1) as f64 * 100.0;
        self.res_label.set_markup(&format!(
            "<span font_family=\"codicon\">{}</span>  Resources used:  <b>{}</b> RAM  ·  <b>{}</b> CPU",
            codicon::PULSE,
            procmon::fmt_mem(report.total_mem_kb),
            fmt_pct(machine_cpu)
        ));
        let vibey_kb = report.vibey.mem_kb;
        if let Some(b) = self.res_label.parent() {
            b.set_tooltip_text(Some(&format!(
                "Total for Vibey and everything running in every project's terminals.\n\n\
                 RAM: {} ({:.1}% of your {} RAM)\n  Vibey itself: {}\n  Terminals and their programs: {}\n\n\
                 CPU: {} of the whole machine ({} cores = 100%)\n\nClick for the per-project breakdown (Ctrl+Shift+R).",
                procmon::fmt_mem(report.total_mem_kb),
                ram_share,
                procmon::fmt_mem(report.mem_total_kb),
                procmon::fmt_mem(vibey_kb),
                procmon::fmt_mem(report.total_mem_kb.saturating_sub(vibey_kb)),
                fmt_pct(machine_cpu),
                report.cpus,
            )));
        }
        for ((_, item), u) in self.projects.borrow().iter().zip(&usage) {
            let (_, mem) = u.totals();
            if let Some(chip) = item.last_child().and_downcast::<gtk::Label>() {
                chip.set_visible(mem > 0);
                chip.set_text(&procmon::fmt_mem_short(mem));
            }
        }
        self.resources.update(Rc::new(report), Rc::new(usage));
    }

    /// Ctrl+P / the title-bar search box.
    pub fn open_file_search(self: &Rc<Self>) {
        if let Some(p) = self.active_project() {
            let hl = current_theme().get("list.highlightForeground").hex();
            FileProvider::open(p, &self.quick, hl, gtk::prelude::GtkWindowExt::focus(&self.window));
        }
    }

    /// Self-test: show rail item `i` (1-based; 0 = the "+" button) as hovered.
    pub fn hover_rail(&self, i: usize) {
        let w: Option<gtk::Widget> = match i {
            0 => self.rail_projects.next_sibling(),
            i => self.projects.borrow().get(i - 1).map(|(_, b)| b.clone().upcast()),
        };
        if let Some(w) = w {
            w.set_state_flags(gtk::StateFlags::PRELIGHT, false);
        }
    }

    pub fn toggle_resources(self: &Rc<Self>) {
        if self.resources.is_open() {
            self.resources.close();
        } else {
            self.resources.open();
            self.start_sampling();
        }
    }

    // ---- status bar ----

    fn update_status(&self) {
        match self.active_project() {
            Some(p) => {
                self.status_project.set_markup(&format!(
                    "<span font_family=\"codicon\">{}</span> {}",
                    codicon::FOLDER_OPENED,
                    glib::markup_escape_text(&p.name)
                ));
                self.show_status(p.editor.status());
            }
            None => {
                self.status_project.set_text("");
                self.status_branch.set_text("");
                self.show_status(None);
            }
        }
    }

    fn show_status(&self, st: Option<Status>) {
        self.status_right.set_visible(st.is_some());
        if let Some(st) = st {
            self.status_pos.set_text(&format!("Ln {}, Col {}", st.line, st.col));
            self.status_indent.set_text(&st.indentation);
            self.status_eol.set_text(st.eol);
            self.status_lang.set_text(&st.language);
        }
    }

    /// VS Code's status bar: `⎇ main*` (dirty) and `↓1 ↑2` (behind/ahead of upstream).
    fn show_branch(&self, project: &Rc<Project>) {
        let st = project.git.borrow().clone();
        let Some(branch) = st.branch.clone().or_else(|| st.is_repo().then(|| "(detached)".to_string())) else {
            self.status_branch.set_text("");
            return;
        };
        let dirty = if st.changed() > 0 { "*" } else { "" };
        let mut sync = String::new();
        if st.behind > 0 {
            sync.push_str(&format!(" ↓{}", st.behind));
        }
        if st.ahead > 0 {
            sync.push_str(&format!(" ↑{}", st.ahead));
        }
        self.status_branch.set_markup(&format!(
            "<span font_family=\"codicon\">{}</span> {}{dirty}{sync}",
            codicon::GIT_BRANCH,
            glib::markup_escape_text(&branch)
        ));
    }

    // ---- themes ----

    pub fn apply_theme(&self, id: &str, save: bool) {
        let model = Rc::new(ThemeModel::load(id));
        let static_css = scale_css(include_str!("../assets/style.css"), self.scale);
        self.css.load_from_string(&(model.css_colors() + &static_css));
        if let Some(s) = gtk::Settings::default() {
            s.set_gtk_application_prefer_dark_theme(model.kind == Kind::Dark);
        }
        {
            let mut st = self.style.borrow_mut();
            st.theme_id = model.id.clone();
            st.brackets = Arc::new(model.bracket_colors());
            st.line_number = model.get("editorLineNumber.foreground");
            st.changed = model.get("diffEditor.insertedLineBackground");
            st.find_match = model.get("editor.findMatchHighlightBackground");
            st.find_current = model.get("editor.findMatchBackground");
            st.git_added = model.get("editorGutter.addedBackground");
            st.git_modified = model.get("editorGutter.modifiedBackground");
            st.git_deleted = model.get("editorGutter.deletedBackground");
        }
        THEME.with(|t| *t.borrow_mut() = Some(model.clone()));
        let hl = model.get("list.highlightForeground").hex();
        for (p, _) in self.projects.borrow().iter() {
            if let Some(s) = p.search.get() {
                s.set_highlight_color(hl.clone());
            }
            p.editor.restyle();
            p.terminals.restyle(&model);
        }
        if save {
            self.config.borrow_mut().theme = model.id.clone();
            config::save(&self.config.borrow());
        }
    }

    pub fn quick(&self) -> &Rc<QuickPick> {
        &self.quick
    }

    pub fn open_theme_picker(self: &Rc<Self>) {
        let provider = Rc::new(ThemeProvider {
            app: Rc::downgrade(self),
            original: current_theme().id.clone(),
            entries: theme::catalog(),
            pending: Rc::new(RefCell::new(None)),
        });
        self.quick.open(provider, gtk::prelude::GtkWindowExt::focus(&self.window));
    }

    // ---- shortcuts ----

    fn install_shortcuts(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(a) = me.upgrade() else { return glib::Propagation::Proceed };
            if a.handle_key(key, mods) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        self.window.add_controller(keys);
    }

    pub fn handle_key(self: &Rc<Self>, key: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> bool {
        use gtk::gdk::{Key, ModifierType as M};
        if self.quick.is_open() {
            return false;
        }
        if self.resources.is_open() && key == Key::Escape {
            self.resources.close();
            return true;
        }
        let ctrl = mods.contains(M::CONTROL_MASK);
        let shift = mods.contains(M::SHIFT_MASK);
        let alt = mods.contains(M::ALT_MASK);
        // In a terminal, only a few keys are taken (like VS Code's commandsToSkipShell);
        // everything else (Ctrl+W, Ctrl+F, …) belongs to the shell / CLI.
        let in_terminal = gtk::prelude::GtkWindowExt::focus(&self.window).is_some_and(|w| {
            w.is::<vte4::Terminal>() || w.ancestor(vte4::Terminal::static_type()).is_some()
        });
        let k = key.to_lower();

        if self.chord.replace(false) && ctrl && k == Key::t {
            self.open_theme_picker();
            return true;
        }
        // Alt+1…9: switch terminal in the current project (works inside terminals too).
        if alt && !ctrl {
            if let Some(n) = key.to_unicode().and_then(|c| c.to_digit(10)).filter(|n| (1..=9).contains(n)) {
                return self.active_project().is_some_and(|p| p.terminals.activate_index(n as usize - 1));
            }
            return false;
        }
        if !ctrl || alt {
            return false;
        }
        if let Some(n) = key.to_unicode().and_then(|c| c.to_digit(10)).filter(|n| (1..=9).contains(n)) {
            self.activate(n as usize - 1);
            return true;
        }
        let project = self.active_project();
        match k {
            Key::p if !shift => {
                self.open_file_search();
                true
            }
            // Ctrl+F always does something: find in the open file, or (in a terminal / with
            // no file open) the Explorer's project search. Ctrl+Shift+F: project search.
            Key::f => {
                if let Some(p) = project {
                    if !shift && !in_terminal && p.editor.has_active() {
                        p.editor.show_find();
                    } else if let Some(s) = p.search.get() {
                        s.focus();
                    }
                }
                true
            }
            Key::grave | Key::dead_grave => {
                if let Some(p) = project {
                    if shift {
                        p.terminals.add_terminal(&current_theme(), None);
                    } else if in_terminal {
                        p.editor.focus();
                    } else if p.terminals.is_empty() {
                        p.terminals.add_terminal(&current_theme(), None);
                    } else {
                        p.terminals.focus();
                    }
                }
                true
            }
            _ if in_terminal => false,
            Key::r if shift => {
                self.toggle_resources();
                true
            }
            Key::k => {
                self.chord.set(true);
                true
            }
            Key::w => {
                if let Some(p) = project {
                    p.editor.close_active();
                }
                true
            }
            Key::Tab | Key::ISO_Left_Tab => {
                if let Some(p) = project {
                    p.editor.cycle(!shift && key != Key::ISO_Left_Tab);
                }
                true
            }
            Key::Page_Down | Key::Page_Up => {
                if let Some(p) = project {
                    p.editor.cycle(k == Key::Page_Down);
                }
                true
            }
            _ => false,
        }
    }
}

/// Theme picker with live preview; Escape restores the original theme.
struct ThemeProvider {
    app: Weak<App>,
    original: String,
    entries: Vec<theme::ThemeEntry>,
    /// Debounced preview; cleared when it fires so it's never removed twice.
    pending: Rc<RefCell<Option<glib::SourceId>>>,
}

impl ThemeProvider {
    fn cancel_pending(&self) {
        if let Some(id) = self.pending.borrow_mut().take() {
            id.remove();
        }
    }
}

impl Provider for ThemeProvider {
    fn query(&self, query: &str, done: Box<dyn Fn(Vec<Item>)>) {
        let hl = current_theme().get("list.highlightForeground").hex();
        let mut last_group = "";
        let items = self
            .entries
            .iter()
            .filter_map(|e| {
                let m = crate::fuzzy::score(query, &e.label)?;
                Some((e, m))
            })
            .map(|(e, m)| {
                let group = (e.group != last_group).then(|| e.group.to_string());
                last_group = e.group;
                Item {
                    key: e.id.clone(),
                    icon: String::new(),
                    label: quickopen::highlight_markup(&e.label, &m.positions, 0, &hl),
                    detail: if e.id == self.original { "current".into() } else { String::new() },
                    group,
                }
            })
            .collect();
        done(items);
    }

    fn selected(&self, key: &str) {
        // Debounce previews while arrowing through the list.
        self.cancel_pending();
        let (app, key, slot) = (self.app.clone(), key.to_string(), self.pending.clone());
        let pending = glib::timeout_add_local_once(Duration::from_millis(120), move || {
            slot.borrow_mut().take();
            if let Some(a) = app.upgrade() {
                if current_theme().id != key {
                    a.apply_theme(&key, false);
                }
            }
        });
        *self.pending.borrow_mut() = Some(pending);
    }

    fn accept(&self, key: &str) {
        self.cancel_pending();
        if let Some(a) = self.app.upgrade() {
            a.apply_theme(key, true);
        }
    }

    fn cancel(&self) {
        self.cancel_pending();
        if let Some(a) = self.app.upgrade() {
            if current_theme().id != self.original {
                a.apply_theme(&self.original, false);
            }
        }
    }

    fn placeholder(&self) -> String {
        "Select Color Theme (Up/Down to preview)".into()
    }

    fn initial(&self) -> Option<String> {
        Some(self.original.clone())
    }
}
