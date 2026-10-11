//! In-window confirmation, like VS Code's custom dialogs: a dimmed scrim over the window
//! and a fixed-width card. It lives in the window's overlay, so there is no separate
//! toplevel for the window manager to place and re-place (GTK's AlertDialog visibly
//! jittered under Cinnamon), and it matches the theme.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::icons::codicon;

pub struct Confirm {
    /// Add to the window overlay; covers the whole window while open.
    pub widget: gtk::Overlay,
    /// The window content behind the scrim; made unfocusable while open so Tab stays here.
    behind: gtk::Widget,
    heading: gtk::Label,
    detail: gtk::Label,
    action: gtk::Button,
    cancel: gtk::Button,
    then: RefCell<Option<Box<dyn Fn()>>>,
    restore_focus: RefCell<Option<glib::WeakRef<gtk::Widget>>>,
}

thread_local! {
    static CONFIRM: RefCell<Weak<Confirm>> = const { RefCell::new(Weak::new()) };
}

/// Ask before something destructive; `then` runs only if the user picks `action`.
/// Cancel is focused, so a stray Enter never confirms.
pub fn confirm(heading: &str, body: &str, action: &str, then: impl Fn() + 'static) {
    if let Some(c) = CONFIRM.with(|c| c.borrow().upgrade()) {
        c.ask(heading, body, action, then);
    }
}

/// Self-test: press the action (`true`) or Cancel.
pub fn answer(ok: bool) {
    if let Some(c) = CONFIRM.with(|c| c.borrow().upgrade()).filter(|c| c.widget.is_visible()) {
        c.finish(ok);
    }
}

pub fn is_open() -> bool {
    CONFIRM.with(|c| c.borrow().upgrade()).is_some_and(|c| c.widget.is_visible())
}

impl Confirm {
    pub fn new(scale: f64, behind: &impl IsA<gtk::Widget>) -> Rc<Confirm> {
        let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scrim.add_css_class("dialog-scrim");
        scrim.set_hexpand(true);
        scrim.set_vexpand(true);
        // Swallow clicks so nothing behind reacts (it's modal; clicking outside does nothing).
        scrim.add_controller(gtk::GestureClick::new());

        let icon = gtk::Label::new(Some(codicon::WARNING));
        icon.add_css_class("codicon");
        icon.add_css_class("dialog-icon");
        icon.set_valign(gtk::Align::Start);
        let heading = gtk::Label::new(None);
        heading.add_css_class("dialog-heading");
        let detail = gtk::Label::new(None);
        detail.add_css_class("dialog-detail");
        for l in [&heading, &detail] {
            l.set_xalign(0.0);
            l.set_wrap(true);
            l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            l.set_selectable(false);
        }
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        text.append(&heading);
        text.append(&detail);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.append(&icon);
        top.append(&text);

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("dialog-button");
        let action = gtk::Button::new();
        action.add_css_class("dialog-button");
        action.add_css_class("primary");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        buttons.add_css_class("dialog-buttons");
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&action);

        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("dialog-card");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Center);
        // Fixed width: the card never changes size as text wraps.
        card.set_size_request((460.0 * scale) as i32, -1);
        card.append(&top);
        card.append(&buttons);

        let widget = gtk::Overlay::new();
        widget.set_child(Some(&scrim));
        widget.add_overlay(&card);
        widget.set_visible(false);

        let c = Rc::new(Confirm {
            widget,
            behind: behind.clone().upcast(),
            heading,
            detail,
            action,
            cancel,
            then: RefCell::new(None),
            restore_focus: RefCell::new(None),
        });
        CONFIRM.with(|slot| *slot.borrow_mut() = Rc::downgrade(&c));

        let w = Rc::downgrade(&c);
        c.cancel.connect_clicked(move |_| {
            if let Some(c) = w.upgrade() {
                c.finish(false);
            }
        });
        let w = Rc::downgrade(&c);
        c.action.connect_clicked(move |_| {
            if let Some(c) = w.upgrade() {
                c.finish(true);
            }
        });
        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(&c);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(c) = w.upgrade() else { return glib::Propagation::Proceed };
            match key {
                gdk::Key::Escape => {
                    c.finish(false);
                    glib::Propagation::Stop
                }
                // Left/Right move between the two buttons, like a native dialog.
                gdk::Key::Left | gdk::Key::Right => {
                    let to = if c.cancel.has_focus() { &c.action } else { &c.cancel };
                    to.grab_focus();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        c.widget.add_controller(keys);
        c
    }

    pub fn ask(&self, heading: &str, body: &str, action: &str, then: impl Fn() + 'static) {
        self.heading.set_text(heading);
        self.detail.set_text(body);
        self.detail.set_visible(!body.is_empty());
        self.action.set_label(action);
        *self.then.borrow_mut() = Some(Box::new(then));
        if !self.widget.is_visible() {
            let root = self.widget.root();
            let focus = root.as_ref().and_then(|r| r.focus());
            *self.restore_focus.borrow_mut() = focus.map(|f| f.downgrade());
            self.behind.set_can_focus(false);
            self.widget.set_visible(true);
        }
        self.cancel.grab_focus();
    }

    fn finish(&self, ok: bool) {
        self.widget.set_visible(false);
        self.behind.set_can_focus(true);
        if let Some(f) = self.restore_focus.take().and_then(|w| w.upgrade()) {
            f.grab_focus();
        }
        let then = self.then.take();
        if ok {
            if let Some(f) = then {
                f();
            }
        }
    }
}
