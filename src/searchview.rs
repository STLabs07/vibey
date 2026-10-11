//! The always-visible search box at the top of the Explorer. Typing searches file names
//! and file contents across the project; the results replace the tree until cleared.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::icons::{self, codicon};
use crate::quickopen::highlight_markup;
use crate::{fuzzy, search};

/// Hit rows rendered at most (results beyond are summarized).
const MAX_ROWS: usize = 600;
const MAX_NAME_MATCHES: usize = 8;

enum Action {
    OpenFile(String),
    OpenHit { rel: String, line: u32, col: usize },
    None,
}

pub struct Callbacks {
    /// Current file index (relative paths).
    pub index: Rc<dyn Fn() -> Arc<Vec<String>>>,
    /// Search box focused: a chance to refresh the index.
    pub on_focus: Rc<dyn Fn()>,
    pub open_file: Rc<dyn Fn(PathBuf)>,
    /// (path, 0-based line, byte column, match length)
    pub open_hit: Rc<dyn Fn(PathBuf, u32, usize, usize)>,
}

pub struct SearchView {
    /// Search box + (tree | results).
    pub widget: gtk::Box,
    entry: gtk::Entry,
    case: gtk::ToggleButton,
    stack: gtk::Stack,
    list: gtk::ListBox,
    summary: gtk::Label,
    root: PathBuf,
    cb: Callbacks,
    actions: RefCell<Vec<Action>>,
    generation: Cell<u64>,
    highlight: RefCell<String>,
    me: RefCell<Weak<SearchView>>,
}

impl SearchView {
    pub fn new(root: &Path, tree_page: &impl IsA<gtk::Widget>, cb: Callbacks) -> Rc<SearchView> {
        let entry = gtk::Entry::builder().placeholder_text("Search files and text").hexpand(true).build();
        entry.set_primary_icon_name(Some("system-search-symbolic"));
        entry.add_css_class("side-search");
        let case = gtk::ToggleButton::with_label(codicon::CASE_SENSITIVE);
        case.add_css_class("icon-button");
        case.add_css_class("codicon");
        case.set_tooltip_text(Some("Match Case"));
        case.set_focus_on_click(false);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.add_css_class("side-search-bar");
        bar.append(&entry);
        bar.append(&case);

        let summary = gtk::Label::new(None);
        summary.add_css_class("search-summary");
        summary.set_xalign(0.0);
        summary.set_wrap(true);
        let list = gtk::ListBox::new();
        list.add_css_class("search-results");
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(true);
        let scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let results = gtk::Box::new(gtk::Orientation::Vertical, 0);
        results.append(&summary);
        results.append(&scroll);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.add_named(tree_page, Some("tree"));
        stack.add_named(&results, Some("results"));

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&bar);
        widget.append(&stack);

        let view = Rc::new(SearchView {
            widget,
            entry,
            case,
            stack,
            list,
            summary,
            root: root.to_path_buf(),
            cb,
            actions: RefCell::new(Vec::new()),
            generation: Cell::new(0),
            highlight: RefCell::new("#2aaaff".into()),
            me: RefCell::new(Weak::new()),
        });
        *view.me.borrow_mut() = Rc::downgrade(&view);

        let w = Rc::downgrade(&view);
        view.entry.connect_changed(move |_| {
            if let Some(v) = w.upgrade() {
                v.schedule();
            }
        });
        let w = Rc::downgrade(&view);
        view.case.connect_toggled(move |_| {
            if let Some(v) = w.upgrade() {
                v.schedule();
            }
        });
        let w = Rc::downgrade(&view);
        view.list.connect_row_activated(move |_, row| {
            if let Some(v) = w.upgrade() {
                v.activate(row.index() as usize);
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let w = Rc::downgrade(&view);
        focus.connect_enter(move |_| {
            if let Some(v) = w.upgrade() {
                (v.cb.on_focus)();
            }
        });
        view.entry.add_controller(focus);
        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(&view);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(v) = w.upgrade() else { return glib::Propagation::Proceed };
            match key {
                gtk::gdk::Key::Escape => v.entry.set_text(""),
                gtk::gdk::Key::Down => {
                    if let Some(r) = v.list.row_at_index(0) {
                        v.list.select_row(Some(&r));
                        r.grab_focus();
                    }
                }
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => v.activate(0),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        view.entry.add_controller(keys);
        view
    }

    pub fn focus(&self) {
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    /// Set the query (self-test).
    pub fn set_query(&self, q: &str) {
        self.entry.set_text(q);
    }

    pub fn set_highlight_color(&self, hex: String) {
        *self.highlight.borrow_mut() = hex;
    }

    fn schedule(&self) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let q = self.entry.text().to_string();
        if q.trim().is_empty() {
            self.stack.set_visible_child_name("tree");
            return;
        }
        self.stack.set_visible_child_name("results");
        let me = self.me.borrow().clone();
        // Debounce typing.
        glib::timeout_add_local_once(Duration::from_millis(180), move || {
            if let Some(v) = me.upgrade() {
                if v.generation.get() == generation {
                    v.run(q, generation);
                }
            }
        });
    }

    fn run(&self, query: String, generation: u64) {
        let index = (self.cb.index)();
        let root = self.root.clone();
        let case = self.case.is_active();
        let me = self.me.borrow().clone();
        self.summary.set_text("Searching…");
        glib::spawn_future_local(async move {
            let q = query.clone();
            let res = gio::spawn_blocking(move || {
                let names: Vec<(String, fuzzy::Match)> =
                    fuzzy::top(&q, &index, MAX_NAME_MATCHES).into_iter().map(|(p, m)| (p.to_string(), m)).collect();
                let text = if q.trim().chars().count() >= 2 { Some(search::search(&root, q.trim(), case, &index)) } else { None };
                (names, text)
            })
            .await;
            let Some(v) = me.upgrade() else { return };
            if v.generation.get() != generation {
                return;
            }
            if let Ok((names, text)) = res {
                v.show(&query, names, text);
            }
        });
    }

    fn show(&self, query: &str, names: Vec<(String, fuzzy::Match)>, text: Option<search::Results>) {
        while let Some(r) = self.list.row_at_index(0) {
            self.list.remove(&r);
        }
        let mut actions = Vec::new();
        let hl = self.highlight.borrow().clone();

        if !names.is_empty() {
            self.list.append(&group_header(&format!("FILES ({})", names.len())));
            actions.push(Action::None);
            for (rel, m) in &names {
                let split = rel.rfind('/').map_or(0, |i| i + 1);
                let (dir, name) = rel.split_at(split);
                self.list.append(&file_row(
                    name,
                    &highlight_markup(name, &m.positions, split, &hl),
                    &highlight_markup(dir.trim_end_matches('/'), &m.positions, 0, &hl),
                    None,
                ));
                actions.push(Action::OpenFile(rel.clone()));
            }
        }

        let mut rows = 0;
        match &text {
            Some(r) if r.total > 0 => {
                self.list.append(&group_header(&format!(
                    "IN FILES ({}{} in {} file{})",
                    r.total,
                    if r.truncated { "+" } else { "" },
                    r.files.len(),
                    if r.files.len() == 1 { "" } else { "s" }
                )));
                actions.push(Action::None);
                'files: for f in &r.files {
                    let split = f.rel.rfind('/').map_or(0, |i| i + 1);
                    let (dir, name) = f.rel.split_at(split);
                    self.list.append(&file_row(
                        name,
                        &glib::markup_escape_text(name),
                        &glib::markup_escape_text(dir.trim_end_matches('/')),
                        Some(f.hits.len()),
                    ));
                    actions.push(Action::OpenFile(f.rel.clone()));
                    for h in &f.hits {
                        if rows >= MAX_ROWS {
                            break 'files;
                        }
                        rows += 1;
                        self.list.append(&hit_row(h, &hl));
                        actions.push(Action::OpenHit { rel: f.rel.clone(), line: h.line, col: h.col });
                    }
                }
            }
            _ => {}
        }
        let summary = match &text {
            None if names.is_empty() => "No files match.".to_string(),
            None => "Type 2+ characters to search inside files.".to_string(),
            Some(r) if r.total == 0 && names.is_empty() => format!("No results for “{query}”."),
            Some(r) if r.truncated || rows >= MAX_ROWS => {
                format!("Showing the first {} matches. Refine the search to see more.", rows.min(r.total))
            }
            Some(_) => String::new(),
        };
        self.summary.set_text(&summary);
        self.summary.set_visible(!summary.is_empty());
        *self.actions.borrow_mut() = actions;
    }

    fn activate(&self, i: usize) {
        let action = match self.actions.borrow().get(i) {
            Some(Action::OpenFile(rel)) => Some((rel.clone(), None)),
            Some(Action::OpenHit { rel, line, col }) => Some((rel.clone(), Some((*line, *col)))),
            _ => None,
        };
        let Some((rel, at)) = action else {
            // Clicked a header: open the first item instead.
            if i == 0 && self.actions.borrow().len() > 1 {
                self.activate(1);
            }
            return;
        };
        let path = self.root.join(&rel);
        match at {
            None => (self.cb.open_file)(path),
            Some((line, col)) => {
                let len = self.entry.text().trim().len();
                (self.cb.open_hit)(path, line.saturating_sub(1), col, len)
            }
        }
    }
}

fn group_header(text: &str) -> gtk::ListBoxRow {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("search-group");
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&l));
    row.set_activatable(false);
    row.set_selectable(false);
    row
}

fn file_row(name: &str, name_markup: &str, dir_markup: &str, count: Option<usize>) -> gtk::ListBoxRow {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("search-file");
    let icon = gtk::Label::new(None);
    icon.add_css_class("seti");
    icon.set_markup(&icons::markup(icons::for_file(name)));
    let n = gtk::Label::new(None);
    n.set_markup(name_markup);
    let d = gtk::Label::new(None);
    d.set_markup(dir_markup);
    d.add_css_class("quick-detail");
    d.set_hexpand(true);
    d.set_xalign(0.0);
    d.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    for w in [&icon, &n, &d] {
        b.append(w);
    }
    if let Some(c) = count {
        let badge = gtk::Label::new(Some(&c.to_string()));
        badge.add_css_class("count-badge");
        b.append(&badge);
    }
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&b));
    row
}

fn hit_row(h: &search::Hit, hl: &str) -> gtk::ListBoxRow {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("search-hit");
    let no = gtk::Label::new(Some(&h.line.to_string()));
    no.add_css_class("hit-line");
    no.set_xalign(1.0);
    let mut positions = Vec::new();
    for (a, z) in &h.ranges {
        positions.extend(*a..*z);
    }
    let t = gtk::Label::new(None);
    t.set_markup(&highlight_markup(&h.text, &positions, 0, hl));
    t.set_xalign(0.0);
    t.set_hexpand(true);
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    b.append(&no);
    b.append(&t);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&b));
    row
}
