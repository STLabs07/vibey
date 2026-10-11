//! One project's editor area: tab strip, breadcrumb, code views, find widget.
//!
//! Tabs follow VS Code's preview semantics: a single click in the tree opens a *preview*
//! tab (italic) that the next single click replaces; double-click pins.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::glib;

use crate::codeview::{CodeView, SharedStyle};
use crate::icons::{self, codicon};

pub struct Tab {
    view: Rc<CodeView>,
    widget: gtk::Box,
    icon: gtk::Label,
    label: gtk::Label,
    preview: Cell<bool>,
}

/// What the status bar shows for the active tab.
pub struct Status {
    pub line: i32,
    pub col: i32,
    pub indentation: String,
    pub eol: &'static str,
    pub language: String,
}

pub struct EditorGroup {
    pub widget: gtk::Box,
    root: PathBuf,
    style: SharedStyle,
    strip: gtk::Box,
    tabs_bar: gtk::Box,
    breadcrumb: gtk::Box,
    stack: gtk::Stack,
    find_bar: gtk::Box,
    find_entry: gtk::Entry,
    find_count: gtk::Label,
    find_case: gtk::ToggleButton,
    tabs: RefCell<Vec<Rc<Tab>>>,
    active: Cell<Option<usize>>,
    on_status: RefCell<Option<Rc<dyn Fn(Option<Status>)>>>,
    on_reload: RefCell<Option<Rc<dyn Fn()>>>,
    on_tabs_changed: RefCell<Option<Rc<dyn Fn()>>>,
    git: RefCell<Rc<crate::git::Status>>,
    me: RefCell<Weak<EditorGroup>>,
}

fn icon_button(glyph: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::with_label(glyph);
    b.add_css_class("icon-button");
    b.add_css_class("codicon");
    b.set_tooltip_text(Some(tooltip));
    b.set_focus_on_click(false);
    b
}

impl EditorGroup {
    pub fn new(root: &Path, style: SharedStyle) -> Rc<EditorGroup> {
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let strip_scroll = gtk::ScrolledWindow::builder()
            .child(&strip)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::External)
            .hexpand(true)
            .build();
        let tabs_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs_bar.add_css_class("tabs");
        tabs_bar.append(&strip_scroll);

        let breadcrumb = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        breadcrumb.add_css_class("breadcrumb");

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.add_named(&empty_hint(), Some("empty"));

        // Find widget, top-right like VS Code's.
        let find_entry = gtk::Entry::builder().placeholder_text("Find").width_chars(22).build();
        let find_case = gtk::ToggleButton::with_label(codicon::CASE_SENSITIVE);
        find_case.add_css_class("icon-button");
        find_case.add_css_class("codicon");
        find_case.set_tooltip_text(Some("Match Case (Alt+C)"));
        find_case.set_focus_on_click(false);
        let find_count = gtk::Label::new(Some("No results"));
        find_count.add_css_class("find-count");
        let prev = icon_button(codicon::ARROW_UP, "Previous Match (Shift+Enter)");
        let next = icon_button(codicon::ARROW_DOWN, "Next Match (Enter)");
        let close = icon_button(codicon::CLOSE, "Close (Escape)");
        let find_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        find_bar.add_css_class("find-widget");
        find_bar.set_halign(gtk::Align::End);
        find_bar.set_valign(gtk::Align::Start);
        for w in [find_entry.upcast_ref::<gtk::Widget>(), find_case.upcast_ref(), find_count.upcast_ref(), prev.upcast_ref(), next.upcast_ref(), close.upcast_ref()] {
            find_bar.append(w);
        }
        find_bar.set_visible(false);
        let overlay = gtk::Overlay::new();
        overlay.add_css_class("editor-area");
        overlay.set_child(Some(&stack));
        overlay.add_overlay(&find_bar);

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&tabs_bar);
        widget.append(&breadcrumb);
        widget.append(&overlay);

        let group = Rc::new(EditorGroup {
            widget,
            root: root.to_path_buf(),
            style,
            strip,
            tabs_bar,
            breadcrumb,
            stack,
            find_bar,
            find_entry,
            find_count,
            find_case,
            tabs: RefCell::new(Vec::new()),
            active: Cell::new(None),
            on_status: RefCell::new(None),
            on_reload: RefCell::new(None),
            on_tabs_changed: RefCell::new(None),
            git: RefCell::new(Rc::new(crate::git::Status::default())),
            me: RefCell::new(Weak::new()),
        });
        *group.me.borrow_mut() = Rc::downgrade(&group);
        group.update_chrome();

        let g = Rc::downgrade(&group);
        group.find_entry.connect_changed(move |_| {
            if let Some(g) = g.upgrade() {
                g.run_find();
            }
        });
        let g = Rc::downgrade(&group);
        group.find_case.connect_toggled(move |_| {
            if let Some(g) = g.upgrade() {
                g.run_find();
            }
        });
        let g = Rc::downgrade(&group);
        group.find_entry.connect_activate(move |_| {
            if let Some(g) = g.upgrade() {
                g.find_step(true);
            }
        });
        let g = Rc::downgrade(&group);
        prev.connect_clicked(move |_| {
            if let Some(g) = g.upgrade() {
                g.find_step(false);
            }
        });
        let g = Rc::downgrade(&group);
        next.connect_clicked(move |_| {
            if let Some(g) = g.upgrade() {
                g.find_step(true);
            }
        });
        let g = Rc::downgrade(&group);
        close.connect_clicked(move |_| {
            if let Some(g) = g.upgrade() {
                g.close_find();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let g = Rc::downgrade(&group);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(g) = g.upgrade() else { return glib::Propagation::Proceed };
            match key {
                gtk::gdk::Key::Escape => g.close_find(),
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter if mods.contains(gtk::gdk::ModifierType::SHIFT_MASK) => {
                    g.find_step(false)
                }
                gtk::gdk::Key::c if mods.contains(gtk::gdk::ModifierType::ALT_MASK) => {
                    g.find_case.set_active(!g.find_case.is_active())
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        group.find_entry.add_controller(keys);
        group
    }

    pub fn set_on_status(&self, f: Rc<dyn Fn(Option<Status>)>) {
        *self.on_status.borrow_mut() = Some(f);
    }

    /// Called when an open file changed on disk and was reloaded.
    pub fn set_on_reload(&self, f: Rc<dyn Fn()>) {
        *self.on_reload.borrow_mut() = Some(f);
    }

    /// Called when tabs are opened, closed or switched (session saving).
    pub fn set_on_tabs_changed(&self, f: Rc<dyn Fn()>) {
        *self.on_tabs_changed.borrow_mut() = Some(f);
    }

    fn tabs_changed(&self) {
        let f = self.on_tabs_changed.borrow().clone();
        if let Some(f) = f {
            f();
        }
    }

    /// Paths of pinned tabs (preview tabs are transient), and the active one.
    pub fn session(&self) -> (Vec<PathBuf>, Option<PathBuf>) {
        let files = self.tabs.borrow().iter().filter(|t| !t.preview.get()).map(|t| t.view.path()).collect();
        (files, self.active_tab().map(|t| t.view.path()))
    }

    /// Git decoration colors on tab titles, like VS Code.
    pub fn set_git(&self, st: &Rc<crate::git::Status>) {
        *self.git.borrow_mut() = st.clone();
        for t in self.tabs.borrow().iter() {
            self.decorate_tab(t);
        }
    }

    fn decorate_tab(&self, t: &Tab) {
        for c in crate::git::Kind::ALL_CSS {
            t.widget.remove_css_class(c);
        }
        if let Some(k) = self.git.borrow().kind(&t.view.path(), false) {
            t.widget.add_css_class(k.css());
        }
    }

    pub fn refresh_git_markers(&self) {
        for v in self.views() {
            v.refresh_git();
        }
    }

    pub fn status(&self) -> Option<Status> {
        let tab = self.active_tab()?;
        let (line, col) = tab.view.cursor();
        Some(Status {
            line,
            col,
            indentation: tab.view.indentation(),
            eol: tab.view.eol(),
            language: tab.view.language(),
        })
    }

    fn emit_status(&self) {
        let f = self.on_status.borrow().clone();
        if let Some(f) = f {
            f(self.status());
        }
    }

    fn active_tab(&self) -> Option<Rc<Tab>> {
        self.active.get().and_then(|i| self.tabs.borrow().get(i).cloned())
    }

    pub fn views(&self) -> Vec<Rc<CodeView>> {
        self.tabs.borrow().iter().map(|t| t.view.clone()).collect()
    }

    pub fn has_active(&self) -> bool {
        self.active_tab().is_some()
    }

    /// Open `path` pinned and select a match at (0-based line, byte column, byte length).
    pub fn reveal(&self, path: &Path, line: u32, col: usize, len: usize) {
        self.open(path, true);
        if let Some(t) = self.active_tab() {
            t.view.reveal(line as i32, col as i32, len as i32);
        }
    }

    /// Open (or focus) `path`. `pin = false` opens a preview tab.
    pub fn open(&self, path: &Path, pin: bool) {
        let existing = self.tabs.borrow().iter().position(|t| t.view.path() == path);
        if let Some(i) = existing {
            if pin {
                self.pin(i);
            }
            self.activate(i);
            return;
        }
        let preview_idx = if pin { None } else { self.tabs.borrow().iter().position(|t| t.preview.get()) };
        let tab = self.make_tab(path, !pin);
        let idx = match preview_idx {
            Some(i) => {
                // Replace the preview tab in place.
                let old = std::mem::replace(&mut self.tabs.borrow_mut()[i], tab.clone());
                self.strip.insert_child_after(&tab.widget, Some(&old.widget));
                self.strip.remove(&old.widget);
                self.stack.remove(&old.view.widget);
                i
            }
            None => {
                let after = self.active.get().and_then(|i| self.tabs.borrow().get(i).map(|t| t.widget.clone()));
                let i = self.active.get().map_or(self.tabs.borrow().len(), |i| i + 1);
                self.strip.insert_child_after(&tab.widget, after.as_ref());
                self.tabs.borrow_mut().insert(i, tab.clone());
                i
            }
        };
        self.stack.add_child(&tab.view.widget);
        tab.view.open(path.to_path_buf());
        self.decorate_tab(&tab);
        self.activate(idx);
    }

    fn make_tab(&self, path: &Path, preview: bool) -> Rc<Tab> {
        let view = CodeView::new(self.style.clone());
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.add_css_class("tab");
        if preview {
            widget.add_css_class("preview");
        }
        let icon = gtk::Label::new(None);
        icon.add_css_class("seti");
        icon.set_markup(&icons::markup(icons::for_file(&name)));
        let label = gtk::Label::new(Some(&name));
        label.add_css_class("tab-label");
        let close = gtk::Label::new(Some(codicon::CLOSE));
        close.add_css_class("codicon");
        close.add_css_class("close");
        widget.append(&icon);
        widget.append(&label);
        widget.append(&close);
        widget.set_tooltip_text(Some(&path.to_string_lossy()));
        let tab = Rc::new(Tab { view, widget, icon, label, preview: Cell::new(preview) });

        let me = self.me.borrow().clone();
        let weak_tab = Rc::downgrade(&tab);
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.connect_pressed(move |gesture, n_press, x, _| {
            let (Some(g), Some(tab)) = (me.upgrade(), weak_tab.upgrade()) else { return };
            let Some(i) = g.index_of(&tab) else { return };
            let on_close = tab.widget.last_child().is_some_and(|c| {
                let x0 = c.compute_point(&tab.widget, &gtk::graphene::Point::new(0.0, 0.0)).map_or(f32::MAX, |p| p.x());
                x as f32 >= x0
            });
            match gesture.current_button() {
                2 => g.close(i),
                1 if on_close => g.close(i),
                1 if n_press == 2 => g.pin(i),
                1 => g.activate(i),
                _ => {}
            }
        });
        tab.widget.add_controller(click);

        let me = self.me.borrow().clone();
        tab.view.set_on_reload(Rc::new(move || {
            if let Some(f) = me.upgrade().and_then(|g| g.on_reload.borrow().clone()) {
                f();
            }
        }));
        let me = self.me.borrow().clone();
        let weak_tab = Rc::downgrade(&tab);
        tab.view.set_notify(Rc::new(move || {
            let (Some(g), Some(tab)) = (me.upgrade(), weak_tab.upgrade()) else { return };
            if tab.view.is_deleted() {
                tab.widget.add_css_class("deleted");
            } else {
                tab.widget.remove_css_class("deleted");
            }
            if g.active_tab().is_some_and(|a| Rc::ptr_eq(&a, &tab)) {
                g.emit_status();
            }
        }));
        tab
    }

    fn index_of(&self, tab: &Rc<Tab>) -> Option<usize> {
        self.tabs.borrow().iter().position(|t| Rc::ptr_eq(t, tab))
    }

    fn pin(&self, i: usize) {
        if let Some(t) = self.tabs.borrow().get(i) {
            t.preview.set(false);
            t.widget.remove_css_class("preview");
        }
        self.tabs_changed();
    }

    pub fn activate(&self, i: usize) {
        let Some(tab) = self.tabs.borrow().get(i).cloned() else { return };
        if let Some(prev) = self.active_tab() {
            prev.widget.remove_css_class("active");
        }
        self.active.set(Some(i));
        tab.widget.add_css_class("active");
        self.stack.set_visible_child(&tab.view.widget);
        self.update_chrome();
        if self.find_bar.is_visible() {
            self.run_find();
        }
        self.emit_status();
        self.tabs_changed();
    }

    pub fn close(&self, i: usize) {
        if i >= self.tabs.borrow().len() {
            return;
        }
        let tab = self.tabs.borrow_mut().remove(i);
        self.strip.remove(&tab.widget);
        self.stack.remove(&tab.view.widget);
        let n = self.tabs.borrow().len();
        let active = self.active.get();
        self.active.set(None);
        if n == 0 {
            self.update_chrome();
            self.emit_status();
            self.tabs_changed();
        } else {
            // VS Code focuses the tab to the right (or the new last one).
            let next = match active {
                Some(a) if a > i => a - 1,
                Some(a) if a == i => i.min(n - 1),
                Some(a) => a,
                None => 0,
            };
            self.activate(next);
        }
    }

    pub fn close_active(&self) {
        if let Some(i) = self.active.get() {
            self.close(i);
        }
    }

    pub fn cycle(&self, forward: bool) {
        let n = self.tabs.borrow().len();
        if let (Some(i), true) = (self.active.get(), n > 0) {
            self.activate(if forward { (i + 1) % n } else { (i + n - 1) % n });
        }
    }

    /// Tab strip state, breadcrumb, empty hint.
    fn update_chrome(&self) {
        while let Some(c) = self.breadcrumb.first_child() {
            self.breadcrumb.remove(&c);
        }
        let Some(tab) = self.active_tab() else {
            self.stack.set_visible_child_name("empty");
            self.breadcrumb.set_visible(false);
            self.tabs_bar.set_visible(false);
            return;
        };
        self.breadcrumb.set_visible(true);
        self.tabs_bar.set_visible(true);
        let path = tab.view.path();
        let rel = path.strip_prefix(&self.root).unwrap_or(&path);
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        for (i, p) in parts.iter().enumerate() {
            if i + 1 == parts.len() {
                let icon = gtk::Label::new(None);
                icon.add_css_class("seti");
                icon.set_markup(&icons::markup(icons::for_file(p)));
                self.breadcrumb.append(&icon);
            }
            self.breadcrumb.append(&gtk::Label::new(Some(p)));
            if i + 1 < parts.len() {
                let sep = gtk::Label::new(Some(codicon::CHEVRON_RIGHT));
                sep.add_css_class("codicon");
                sep.add_css_class("sep");
                self.breadcrumb.append(&sep);
            }
        }
        // Keep the active tab visible in the strip.
        let w = tab.widget.clone();
        glib::idle_add_local_once(move || {
            if let Some(p) = w.parent() {
                if let Some(sw) = p.parent().and_then(|v| v.parent()).and_downcast::<gtk::ScrolledWindow>() {
                    let adj = sw.hadjustment();
                    let x = w.compute_point(&p, &gtk::graphene::Point::new(0.0, 0.0)).map_or(0.0, |p| p.x() as f64);
                    let wd = w.width() as f64;
                    if x < adj.value() {
                        adj.set_value(x);
                    } else if x + wd > adj.value() + adj.page_size() {
                        adj.set_value(x + wd - adj.page_size());
                    }
                }
            }
        });
        let _ = (&tab.icon, &tab.label);
    }

    pub fn restyle(&self) {
        for v in self.views() {
            v.restyle();
        }
    }

    pub fn focus(&self) {
        if let Some(t) = self.active_tab() {
            t.view.grab_focus();
        }
    }

    // ---- find ----

    /// Set the find query (self-test).
    pub fn set_find_query(&self, q: &str) {
        self.find_entry.set_text(q);
    }

    pub fn show_find(&self) {
        if self.active_tab().is_none() {
            return;
        }
        self.find_bar.set_visible(true);
        self.find_entry.grab_focus();
        self.find_entry.select_region(0, -1);
        self.run_find();
    }

    pub fn close_find(&self) {
        self.find_bar.set_visible(false);
        for v in self.views() {
            v.clear_find();
        }
        self.focus();
    }

    fn run_find(&self) {
        let Some(tab) = self.active_tab() else { return };
        let q = self.find_entry.text();
        let n = tab.view.find(&q, self.find_case.is_active());
        self.find_count.set_text(&match n {
            0 if q.is_empty() => String::new(),
            0 => "No results".into(),
            _ => {
                let (i, n) = tab.view.find_step(true);
                format!("{i} of {n}")
            }
        });
        self.find_count.set_visible(!q.is_empty());
    }

    pub fn find_step(&self, forward: bool) {
        let Some(tab) = self.active_tab() else { return };
        let (i, n) = tab.view.find_step(forward);
        if n > 0 {
            self.find_count.set_text(&format!("{i} of {n}"));
        }
    }
}

/// VS Code's empty editor watermark, with Vibey's shortcuts.
fn empty_hint() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("watermark");
    b.set_valign(gtk::Align::Center);
    b.set_halign(gtk::Align::Center);
    for (what, keys) in [
        ("Go to File", "Ctrl + P"),
        ("Find in File", "Ctrl + F"),
        ("Switch Project", "Ctrl + 1…9"),
        ("Close Tab", "Ctrl + W"),
        ("Toggle Terminal Focus", "Ctrl + `"),
    ] {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let l = gtk::Label::new(Some(what));
        l.set_xalign(1.0);
        l.set_width_chars(22);
        l.add_css_class("what");
        let k = gtk::Label::new(Some(keys));
        k.set_xalign(0.0);
        k.add_css_class("keys");
        row.append(&l);
        row.append(&k);
        b.append(&row);
    }
    b
}
