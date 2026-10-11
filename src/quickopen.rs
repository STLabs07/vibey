//! VS Code-style quick pick: a filter box over a list, used for "Go to File" (Ctrl+P)
//! and the theme picker. Providers supply items; the widget handles keys and layout.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;

pub struct Item {
    pub key: String,
    /// Pango markup.
    pub icon: String,
    pub label: String,
    pub detail: String,
    /// Group header shown above this item.
    pub group: Option<String>,
}

pub trait Provider {
    /// Produce items for `query`; call `done` (possibly later) with the result.
    fn query(&self, query: &str, done: Box<dyn Fn(Vec<Item>)>);
    fn accept(&self, key: &str);
    /// Selection moved (theme picker previews).
    fn selected(&self, _key: &str) {}
    fn cancel(&self) {}
    /// Extra key handling (e.g. Alt+I); return true when handled.
    fn key(&self, _key: gtk::gdk::Key, _mods: gtk::gdk::ModifierType) -> bool {
        false
    }
    fn footer(&self) -> String {
        String::new()
    }
    fn placeholder(&self) -> String;
    /// Initially selected item key (theme picker selects the current theme).
    fn initial(&self) -> Option<String> {
        None
    }
}

pub struct QuickPick {
    /// Add to a window-level overlay; covers the window (the scrim closes on click).
    pub widget: gtk::Overlay,
    entry: gtk::Entry,
    list: gtk::ListBox,
    scroll: gtk::ScrolledWindow,
    footer: gtk::Label,
    provider: RefCell<Option<Rc<dyn Provider>>>,
    keys: RefCell<Vec<String>>,
    generation: Cell<u64>,
    restore_focus: RefCell<Option<gtk::Widget>>,
    me: RefCell<Weak<QuickPick>>,
}

impl QuickPick {
    pub fn new(scale: f64) -> Rc<QuickPick> {
        let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scrim.add_css_class("scrim");
        scrim.set_hexpand(true);
        scrim.set_vexpand(true);

        let entry = gtk::Entry::new();
        entry.add_css_class("quick-entry");
        let list = gtk::ListBox::new();
        list.add_css_class("quick-list");
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(true);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height((440.0 * scale) as i32)
            .build();
        let footer = gtk::Label::new(None);
        footer.add_css_class("quick-footer");
        footer.set_xalign(0.0);

        let popup = gtk::Box::new(gtk::Orientation::Vertical, 0);
        popup.add_css_class("quick-input");
        popup.set_halign(gtk::Align::Center);
        popup.set_valign(gtk::Align::Start);
        popup.set_size_request((600.0 * scale) as i32, -1);
        popup.append(&entry);
        popup.append(&scroll);
        popup.append(&footer);

        let widget = gtk::Overlay::new();
        widget.set_child(Some(&scrim));
        widget.add_overlay(&popup);
        widget.set_visible(false);

        let qp = Rc::new(QuickPick {
            widget,
            entry,
            list,
            scroll,
            footer,
            provider: RefCell::new(None),
            keys: RefCell::new(Vec::new()),
            generation: Cell::new(0),
            restore_focus: RefCell::new(None),
            me: RefCell::new(Weak::new()),
        });
        *qp.me.borrow_mut() = Rc::downgrade(&qp);

        let click = gtk::GestureClick::new();
        let w = Rc::downgrade(&qp);
        click.connect_pressed(move |_, _, _, _| {
            if let Some(q) = w.upgrade() {
                q.cancel();
            }
        });
        scrim.add_controller(click);

        let w = Rc::downgrade(&qp);
        qp.entry.connect_changed(move |_| {
            if let Some(q) = w.upgrade() {
                q.requery();
            }
        });
        let w = Rc::downgrade(&qp);
        qp.list.connect_row_activated(move |_, row| {
            if let Some(q) = w.upgrade() {
                q.accept_index(row.index());
            }
        });
        let w = Rc::downgrade(&qp);
        qp.list.connect_row_selected(move |_, row| {
            if let (Some(q), Some(row)) = (w.upgrade(), row) {
                let key = q.keys.borrow().get(row.index() as usize).cloned();
                let p = q.provider.borrow().clone();
                if let (Some(k), Some(p)) = (key, p) {
                    p.selected(&k);
                }
            }
        });

        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(&qp);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(q) = w.upgrade() else { return glib::Propagation::Proceed };
            use gtk::gdk::Key;
            match key {
                Key::Escape => q.cancel(),
                Key::Down => q.move_selection(1),
                Key::Up => q.move_selection(-1),
                Key::Page_Down => q.move_selection(10),
                Key::Page_Up => q.move_selection(-10),
                Key::Return | Key::KP_Enter => {
                    let i = q.list.selected_row().map_or(0, |r| r.index());
                    q.accept_index(i);
                }
                _ => {
                    let p = q.provider.borrow().clone();
                    if !p.is_some_and(|p| p.key(key, mods)) {
                        return glib::Propagation::Proceed;
                    }
                    q.requery();
                }
            }
            glib::Propagation::Stop
        });
        qp.entry.add_controller(keys);
        qp
    }

    pub fn is_open(&self) -> bool {
        self.widget.is_visible()
    }

    pub fn open(&self, provider: Rc<dyn Provider>, focus: Option<gtk::Widget>) {
        if self.is_open() {
            self.cancel();
        }
        *self.restore_focus.borrow_mut() = focus;
        self.entry.set_placeholder_text(Some(&provider.placeholder()));
        *self.provider.borrow_mut() = Some(provider);
        self.widget.set_visible(true);
        self.entry.set_text("");
        self.requery();
        self.entry.grab_focus();
    }

    /// Re-run the current query (e.g. after the provider's data changed).
    pub fn requery(&self) {
        let Some(p) = self.provider.borrow().clone() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.footer.set_text(&p.footer());
        self.footer.set_visible(!self.footer.text().is_empty());
        let me = self.me.borrow().clone();
        let initial = p.initial();
        p.query(
            &self.entry.text(),
            Box::new(move |items| {
                if let Some(q) = me.upgrade() {
                    if q.generation.get() == generation && q.is_open() {
                        q.show(items, initial.as_deref());
                    }
                }
            }),
        );
    }

    fn show(&self, items: Vec<Item>, initial: Option<&str>) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let mut keys = Vec::with_capacity(items.len());
        for item in &items {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.add_css_class("quick-row");
            if !item.icon.is_empty() {
                let icon = gtk::Label::new(None);
                icon.set_markup(&item.icon);
                icon.add_css_class("seti");
                row.append(&icon);
            }
            let label = gtk::Label::new(None);
            label.set_markup(&item.label);
            label.add_css_class("quick-label");
            row.append(&label);
            let detail = gtk::Label::new(None);
            detail.set_markup(&item.detail);
            detail.add_css_class("quick-detail");
            detail.set_ellipsize(gtk::pango::EllipsizeMode::Start);
            detail.set_hexpand(true);
            detail.set_xalign(0.0);
            row.append(&detail);
            if let Some(g) = &item.group {
                let gl = gtk::Label::new(Some(g));
                gl.add_css_class("quick-group");
                row.append(&gl);
            }
            self.list.append(&row);
            keys.push(item.key.clone());
        }
        let start = initial.and_then(|k| keys.iter().position(|x| x == k)).unwrap_or(0);
        *self.keys.borrow_mut() = keys;
        if let Some(row) = self.list.row_at_index(start as i32) {
            self.list.select_row(Some(&row));
            self.scroll_to(&row);
        }
    }

    pub fn move_selection(&self, delta: i32) {
        let n = self.keys.borrow().len() as i32;
        if n == 0 {
            return;
        }
        let cur = self.list.selected_row().map_or(0, |r| r.index());
        let next = (cur + delta).clamp(0, n - 1);
        if let Some(row) = self.list.row_at_index(next) {
            self.list.select_row(Some(&row));
            self.scroll_to(&row);
        }
    }

    fn scroll_to(&self, row: &gtk::ListBoxRow) {
        let row = row.clone();
        let adj = self.scroll.vadjustment();
        let list = self.list.clone();
        glib::idle_add_local_once(move || {
            let Some(b) = row.compute_bounds(&list) else { return };
            let (y, h) = (b.y() as f64, b.height() as f64);
            if y < adj.value() {
                adj.set_value(y);
            } else if y + h > adj.value() + adj.page_size() {
                adj.set_value(y + h - adj.page_size());
            }
        });
    }

    pub fn accept_selected(&self) {
        self.accept_index(self.list.selected_row().map_or(0, |r| r.index()));
    }

    /// Type into the filter box (self-test).
    pub fn set_query(&self, text: &str) {
        self.entry.set_text(text);
    }

    fn accept_index(&self, i: i32) {
        let key = self.keys.borrow().get(i.max(0) as usize).cloned();
        let p = self.provider.borrow_mut().take();
        self.close();
        if let (Some(k), Some(p)) = (key, p) {
            p.accept(&k);
        }
    }

    pub fn cancel(&self) {
        let p = self.provider.borrow_mut().take();
        self.close();
        if let Some(p) = p {
            p.cancel();
        }
    }

    fn close(&self) {
        self.widget.set_visible(false);
        self.generation.set(self.generation.get() + 1);
        if let Some(w) = self.restore_focus.borrow_mut().take() {
            w.grab_focus();
        }
    }
}

/// `text` with the bytes at `positions` (offsets into `text` + `base`) highlighted.
pub fn highlight_markup(text: &str, positions: &[usize], base: usize, color: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let mut in_hl = false;
    let flush = |out: &mut String, run: &mut String, hl: bool| {
        if run.is_empty() {
            return;
        }
        let esc = glib::markup_escape_text(run);
        if hl {
            out.push_str(&format!("<span foreground=\"{color}\" weight=\"bold\">{esc}</span>"));
        } else {
            out.push_str(&esc);
        }
        run.clear();
    };
    for (i, ch) in text.char_indices() {
        let hl = positions.binary_search(&(base + i)).is_ok();
        if hl != in_hl {
            flush(&mut out, &mut run, in_hl);
            in_hl = hl;
        }
        run.push(ch);
    }
    flush(&mut out, &mut run, in_hl);
    out
}
