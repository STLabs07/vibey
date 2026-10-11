//! A project's terminal panel: VTE terminals on real PTYs, with tabs.
//! (Auto-naming and status dots come in phase 2.)

use std::cell::{Cell, RefCell};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use vte4::prelude::*;

use crate::icons::codicon;
use crate::theme::ThemeModel;

struct Term {
    term: vte4::Terminal,
    chip: gtk::Box,
    shell_pid: Cell<i32>,
}

impl Term {
    /// Name of the foreground job if it isn't the shell itself (e.g. "claude", "node").
    fn foreground_job(&self) -> Option<String> {
        let pty = self.term.pty()?;
        let pgid = unsafe { libc::tcgetpgrp(pty.fd().as_raw_fd()) };
        let shell = self.shell_pid.get();
        if pgid <= 0 || shell <= 0 || pgid == shell {
            return None;
        }
        let comm = std::fs::read_to_string(format!("/proc/{pgid}/comm")).ok()?;
        Some(comm.trim().to_string())
    }
}

pub struct TerminalPanel {
    pub widget: gtk::Box,
    cwd: PathBuf,
    header_tabs: gtk::Box,
    stack: gtk::Stack,
    terms: RefCell<Vec<Rc<Term>>>,
    active: Cell<usize>,
    scale: f64,
    me: RefCell<Weak<TerminalPanel>>,
}

fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
}

impl TerminalPanel {
    pub fn new(cwd: &Path, scale: f64) -> Rc<TerminalPanel> {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("panel-header");
        let title = gtk::Label::new(Some("Terminal"));
        title.add_css_class("panel-tab");
        title.add_css_class("active");
        header.append(&title);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        header.append(&spacer);
        let header_tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.append(&header_tabs);
        let add = gtk::Button::with_label(codicon::ADD);
        add.add_css_class("icon-button");
        add.add_css_class("codicon");
        add.set_tooltip_text(Some("New Terminal (Ctrl+Shift+`)"));
        add.set_focus_on_click(false);
        header.append(&add);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        let empty = gtk::Label::new(Some("No terminals. Press Ctrl+Shift+` to open one."));
        empty.add_css_class("dim");
        stack.add_named(&empty, Some("empty"));

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("panel");
        widget.append(&header);
        widget.append(&stack);

        let panel = Rc::new(TerminalPanel {
            widget,
            cwd: cwd.to_path_buf(),
            header_tabs,
            stack,
            terms: RefCell::new(Vec::new()),
            active: Cell::new(0),
            scale,
            me: RefCell::new(Weak::new()),
        });
        *panel.me.borrow_mut() = Rc::downgrade(&panel);
        let weak = Rc::downgrade(&panel);
        add.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.add_terminal(&crate::app::current_theme());
            }
        });
        panel
    }

    pub fn is_empty(&self) -> bool {
        self.terms.borrow().is_empty()
    }

    pub fn add_terminal(&self, theme: &ThemeModel) {
        let term = vte4::Terminal::new();
        term.set_hexpand(true);
        term.set_vexpand(true);
        // VS Code: editor font family, 14 px, scaled like the rest of the UI.
        let mut font = pango::FontDescription::from_string("Droid Sans Mono,monospace");
        font.set_absolute_size(14.0 * self.scale * pango::SCALE as f64);
        term.set_font(Some(&font));
        term.set_bold_is_bright(false);
        term.set_scrollback_lines(10_000);
        term.set_mouse_autohide(true);
        apply_colors(&term, theme);

        let n = self.terms.borrow().len() + 1;
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        chip.add_css_class("term-tab");
        let icon = gtk::Label::new(Some(codicon::TERMINAL));
        icon.add_css_class("codicon");
        let name = shell().rsplit('/').next().unwrap_or("sh").to_string();
        chip.append(&icon);
        chip.append(&gtk::Label::new(Some(&format!("{n}: {name}"))));
        self.header_tabs.append(&chip);
        self.stack.add_child(&term);

        let t = Rc::new(Term { term: term.clone(), chip: chip.clone(), shell_pid: Cell::new(0) });
        self.terms.borrow_mut().push(t.clone());

        let click = gtk::GestureClick::new();
        let (me, wt) = (self.me.borrow().clone(), Rc::downgrade(&t));
        click.connect_pressed(move |_, _, _, _| {
            if let (Some(p), Some(t)) = (me.upgrade(), wt.upgrade()) {
                if let Some(i) = p.terms.borrow().iter().position(|x| Rc::ptr_eq(x, &t)) {
                    p.activate(i);
                }
            }
        });
        chip.add_controller(click);

        let (me, wt) = (self.me.borrow().clone(), Rc::downgrade(&t));
        term.connect_child_exited(move |_, _| {
            if let (Some(p), Some(t)) = (me.upgrade(), wt.upgrade()) {
                p.remove(&t);
            }
        });

        let wt = Rc::downgrade(&t);
        term.spawn_async(
            vte4::PtyFlags::DEFAULT,
            self.cwd.to_str(),
            &[&shell()],
            &[],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&gio::Cancellable>,
            move |res| match res {
                Ok(pid) => {
                    if let Some(t) = wt.upgrade() {
                        t.shell_pid.set(pid.0);
                    }
                }
                Err(e) => eprintln!("vibey: failed to spawn shell: {e}"),
            },
        );
        self.activate(n - 1);
    }

    fn remove(&self, t: &Rc<Term>) {
        let Some(i) = self.terms.borrow().iter().position(|x| Rc::ptr_eq(x, t)) else { return };
        self.terms.borrow_mut().remove(i);
        self.header_tabs.remove(&t.chip);
        self.stack.remove(&t.term);
        let n = self.terms.borrow().len();
        if n == 0 {
            self.stack.set_visible_child_name("empty");
        } else {
            self.activate(i.min(n - 1));
        }
    }

    fn activate(&self, i: usize) {
        let terms = self.terms.borrow();
        let Some(t) = terms.get(i) else { return };
        for (j, x) in terms.iter().enumerate() {
            if j == i {
                x.chip.add_css_class("active");
            } else {
                x.chip.remove_css_class("active");
            }
        }
        self.active.set(i);
        self.stack.set_visible_child(&t.term);
        t.term.grab_focus();
    }

    pub fn focus(&self) {
        let terms = self.terms.borrow();
        if let Some(t) = terms.get(self.active.get()) {
            t.term.grab_focus();
        }
    }

    pub fn restyle(&self, theme: &ThemeModel) {
        for t in self.terms.borrow().iter() {
            apply_colors(&t.term, theme);
        }
    }

    /// Foreground jobs (other than the shell) running in this panel's terminals.
    pub fn running_jobs(&self) -> Vec<String> {
        self.terms.borrow().iter().filter_map(|t| t.foreground_job()).collect()
    }
}

fn apply_colors(term: &vte4::Terminal, theme: &ThemeModel) {
    let (fg, bg, palette) = theme.terminal_colors();
    let palette: Vec<gtk::gdk::RGBA> = palette.iter().map(|c| c.gdk()).collect();
    let refs: Vec<&gtk::gdk::RGBA> = palette.iter().collect();
    term.set_colors(Some(&fg.gdk()), Some(&bg.gdk()), &refs);
    term.set_color_cursor(Some(&theme.get("terminalCursor.foreground").gdk()));
}
