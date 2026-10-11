//! Resource panel: project → terminal → process tree with CPU% and memory (PSS),
//! sortable, with a kill button per process. Vibey's own overhead is the first row.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::icons::codicon;
use crate::procmon::{self, Node, Report};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Memory,
    Cpu,
    Name,
}

/// What the panel shows: per project, per terminal, the process tree under the shell.
pub struct ProjectUsage {
    pub name: String,
    pub terminals: Vec<(String, Option<Node>)>,
}

impl ProjectUsage {
    pub fn totals(&self) -> (f32, u64) {
        self.terminals.iter().filter_map(|(_, n)| n.as_ref()).fold((0.0, 0), |(c, m), n| (c + n.total_cpu(), m + n.total_mem()))
    }
}

pub struct ResourcePanel {
    /// Add to the window overlay; covers the window (scrim closes on click).
    pub widget: gtk::Overlay,
    grid: gtk::Grid,
    footer: gtk::Label,
    sort: Cell<SortBy>,
    sort_buttons: Vec<(SortBy, gtk::ToggleButton)>,
    last: RefCell<Option<(Rc<Report>, Rc<Vec<ProjectUsage>>)>>,
    on_killed: RefCell<Option<Rc<dyn Fn()>>>,
    me: RefCell<Weak<ResourcePanel>>,
}

impl ResourcePanel {
    pub fn new(scale: f64) -> Rc<ResourcePanel> {
        let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scrim.add_css_class("scrim");
        scrim.set_hexpand(true);
        scrim.set_vexpand(true);

        let title = gtk::Label::new(Some("Resources"));
        title.add_css_class("res-title");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        header.add_css_class("res-header");
        header.append(&title);
        header.append(&gtk::Label::new(Some("Sort:")));
        let mut sort_buttons = Vec::new();
        let mut group: Option<gtk::ToggleButton> = None;
        for (s, label) in [(SortBy::Memory, "Memory"), (SortBy::Cpu, "CPU"), (SortBy::Name, "Name")] {
            let b = gtk::ToggleButton::with_label(label);
            b.add_css_class("flat");
            b.add_css_class("res-sort");
            if let Some(g) = &group {
                b.set_group(Some(g));
            } else {
                group = Some(b.clone());
            }
            b.set_active(s == SortBy::Memory);
            header.append(&b);
            sort_buttons.push((s, b));
        }
        let close = gtk::Button::with_label(codicon::CLOSE);
        close.add_css_class("icon-button");
        close.add_css_class("codicon");
        close.set_tooltip_text(Some("Close (Escape)"));
        header.append(&close);

        let grid = gtk::Grid::new();
        grid.add_css_class("res-grid");
        grid.set_column_spacing((12.0 * scale) as u32);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&grid)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height((520.0 * scale) as i32)
            .build();
        let footer = gtk::Label::new(None);
        footer.add_css_class("quick-footer");
        footer.set_xalign(0.0);

        let popup = gtk::Box::new(gtk::Orientation::Vertical, 0);
        popup.add_css_class("quick-input");
        popup.add_css_class("res-panel");
        popup.set_halign(gtk::Align::Center);
        popup.set_valign(gtk::Align::Start);
        popup.set_size_request((760.0 * scale) as i32, -1);
        popup.append(&header);
        popup.append(&scroll);
        popup.append(&footer);

        let widget = gtk::Overlay::new();
        widget.set_child(Some(&scrim));
        widget.add_overlay(&popup);
        widget.set_visible(false);

        let panel = Rc::new(ResourcePanel {
            widget,
            grid,
            footer,
            sort: Cell::new(SortBy::Memory),
            sort_buttons,
            last: RefCell::new(None),
            on_killed: RefCell::new(None),
            me: RefCell::new(Weak::new()),
        });
        *panel.me.borrow_mut() = Rc::downgrade(&panel);

        let click = gtk::GestureClick::new();
        let w = Rc::downgrade(&panel);
        click.connect_pressed(move |_, _, _, _| {
            if let Some(p) = w.upgrade() {
                p.close();
            }
        });
        scrim.add_controller(click);
        let w = Rc::downgrade(&panel);
        close.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.close();
            }
        });
        for (s, b) in &panel.sort_buttons {
            let (w, s) = (Rc::downgrade(&panel), *s);
            b.connect_toggled(move |b| {
                if let (true, Some(p)) = (b.is_active(), w.upgrade()) {
                    p.sort.set(s);
                    p.render();
                }
            });
        }
        panel
    }

    pub fn is_open(&self) -> bool {
        self.widget.is_visible()
    }

    pub fn open(&self) {
        self.widget.set_visible(true);
        self.render();
    }

    pub fn close(&self) {
        self.widget.set_visible(false);
    }

    /// Called after a process was sent SIGTERM (to resample sooner).
    pub fn set_on_killed(&self, f: Rc<dyn Fn()>) {
        *self.on_killed.borrow_mut() = Some(f);
    }

    pub fn update(&self, report: Rc<Report>, projects: Rc<Vec<ProjectUsage>>) {
        *self.last.borrow_mut() = Some((report, projects));
        if self.is_open() {
            self.render();
        }
    }

    fn render(&self) {
        while let Some(c) = self.grid.first_child() {
            self.grid.remove(&c);
        }
        let Some((report, projects)) = self.last.borrow().clone() else {
            self.footer.set_text("Sampling…");
            return;
        };
        let mut row = 0;
        for (col, text) in ["", "PID", "CPU", "Memory", ""].iter().enumerate() {
            let l = gtk::Label::new(Some(text));
            l.add_css_class("res-colhead");
            l.set_xalign(if col == 0 { 0.0 } else { 1.0 });
            self.grid.attach(&l, col as i32, row, 1, 1);
        }
        row += 1;
        let v = &report.vibey;
        self.add_row(&mut row, 0, "Vibey (this window)", Some(v.pid), v.cpu, v.mem_kb, false, "res-vibey");

        let sort = self.sort.get();
        let mut projects: Vec<&ProjectUsage> = projects.iter().collect();
        projects.sort_by(|a, b| {
            let (ta, tb) = (a.totals(), b.totals());
            match sort {
                SortBy::Memory => tb.1.cmp(&ta.1),
                SortBy::Cpu => tb.0.total_cmp(&ta.0),
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });
        for p in projects {
            let (cpu, mem) = p.totals();
            self.add_row(&mut row, 0, &glib::markup_escape_text(&p.name), None, cpu, mem, false, "res-project");
            let mut terms: Vec<&(String, Option<Node>)> = p.terminals.iter().collect();
            terms.sort_by(|a, b| node_cmp(sort, a.1.as_ref(), b.1.as_ref(), &a.0, &b.0));
            for (label, node) in terms {
                let (cpu, mem) = node.as_ref().map_or((0.0, 0), |n| (n.total_cpu(), n.total_mem()));
                let markup = format!(
                    "<span font_family=\"codicon\">{}</span> {}",
                    codicon::TERMINAL,
                    glib::markup_escape_text(label)
                );
                self.add_row(&mut row, 1, &markup, None, cpu, mem, false, "res-terminal");
                if let Some(n) = node {
                    self.add_tree(&mut row, 2, n, sort);
                }
            }
        }
        let used = report.mem_total_kb.saturating_sub(report.mem_available_kb);
        self.footer.set_text(&format!(
            "Vibey + terminals: {} · {:.0}% CPU    System: load {:.2} on {} cores · {} of {} RAM used",
            procmon::fmt_mem(report.total_mem_kb),
            report.total_cpu,
            report.load1,
            report.cpus,
            procmon::fmt_mem(used),
            procmon::fmt_mem(report.mem_total_kb),
        ));
    }

    fn add_tree(&self, row: &mut i32, depth: i32, n: &Node, sort: SortBy) {
        self.add_row(row, depth, &glib::markup_escape_text(&n.name), Some(n.pid), n.cpu, n.mem_kb, true, "res-proc");
        let mut kids: Vec<&Node> = n.children.iter().collect();
        kids.sort_by(|a, b| node_cmp(sort, Some(a), Some(b), &a.name, &b.name));
        for k in kids {
            self.add_tree(row, depth + 1, k, sort);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_row(&self, row: &mut i32, depth: i32, name: &str, pid: Option<i32>, cpu: f32, mem_kb: u64, killable: bool, class: &str) {
        // `name` is Pango markup (callers escape plain names).
        let name_l = gtk::Label::new(None);
        name_l.set_markup(name);
        name_l.set_xalign(0.0);
        name_l.set_hexpand(true);
        name_l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name_l.set_margin_start(depth * 16);
        name_l.add_css_class(class);
        let pid_l = gtk::Label::new(Some(&pid.map(|p| p.to_string()).unwrap_or_default()));
        let cpu_l = gtk::Label::new(Some(&format!("{cpu:.1}%")));
        let mem_l = gtk::Label::new(Some(&procmon::fmt_mem(mem_kb)));
        for (i, l) in [&name_l, &pid_l, &cpu_l, &mem_l].into_iter().enumerate() {
            if i > 0 {
                l.set_xalign(1.0);
                l.add_css_class("res-num");
            }
            self.grid.attach(l, i as i32, *row, 1, 1);
        }
        if let (true, Some(pid)) = (killable, pid) {
            let kill = gtk::Button::with_label(codicon::CLOSE);
            kill.add_css_class("icon-button");
            kill.add_css_class("codicon");
            kill.set_tooltip_text(Some("Stop process (SIGTERM)"));
            let (me, name) = (self.me.borrow().clone(), name_l.text().to_string());
            kill.connect_clicked(move |b| {
                if let Some(p) = me.upgrade() {
                    p.confirm_kill(b, pid, &name);
                }
            });
            self.grid.attach(&kill, 4, *row, 1, 1);
        }
        *row += 1;
    }

    fn confirm_kill(&self, anchor: &gtk::Button, pid: i32, name: &str) {
        let dialog = gtk::AlertDialog::builder()
            .message(format!("Stop {name} (PID {pid})?"))
            .detail("Sends SIGTERM, which lets the process shut down cleanly.")
            .buttons(["Cancel", "Stop Process"])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let root = anchor.root().and_downcast::<gtk::Window>();
        let me = self.me.borrow().clone();
        dialog.choose(root.as_ref(), None::<&gio::Cancellable>, move |res| {
            if res == Ok(1) {
                unsafe { libc::kill(pid, libc::SIGTERM) };
                if let Some(f) = me.upgrade().and_then(|p| p.on_killed.borrow().clone()) {
                    glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || f());
                }
            }
        });
    }
}

fn node_cmp(sort: SortBy, a: Option<&Node>, b: Option<&Node>, an: &str, bn: &str) -> std::cmp::Ordering {
    let m = |n: Option<&Node>| n.map_or(0, |n| n.total_mem());
    let c = |n: Option<&Node>| n.map_or(0.0, |n| n.total_cpu());
    match sort {
        SortBy::Memory => m(b).cmp(&m(a)),
        SortBy::Cpu => c(b).total_cmp(&c(a)),
        SortBy::Name => an.to_lowercase().cmp(&bn.to_lowercase()),
    }
}
