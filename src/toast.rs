//! A small self-dismissing message at the bottom of the window ("Copied to clipboard"). It
//! lives in the window's overlay and never takes input, so it can't get in the way of clicks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::icons::codicon;

/// How long a toast stays up.
const SHOWN: Duration = Duration::from_millis(1600);

struct Toast {
    widget: gtk::Box,
    text: gtk::Label,
    /// Bumped on every show, so an older toast's timer can't hide a newer one.
    serial: Cell<u32>,
}

thread_local! {
    static TOAST: RefCell<Option<Rc<Toast>>> = const { RefCell::new(None) };
}

/// Add the toast to the window's overlay. Call once, before the first `show`.
pub fn install(overlay: &gtk::Overlay) {
    let icon = gtk::Label::new(Some(codicon::CHECK));
    icon.add_css_class("codicon");
    let text = gtk::Label::new(None);
    let widget = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    widget.append(&icon);
    widget.append(&text);
    widget.add_css_class("toast");
    widget.set_halign(gtk::Align::Center);
    widget.set_valign(gtk::Align::End);
    widget.set_can_target(false);
    widget.set_can_focus(false);
    overlay.add_overlay(&widget);
    TOAST.with(|t| *t.borrow_mut() = Some(Rc::new(Toast { widget, text, serial: Cell::new(0) })));
}

/// Show `text` for a moment; a toast already up is replaced.
pub fn show(text: &str) {
    let Some(t) = TOAST.with(|t| t.borrow().clone()) else { return };
    t.text.set_text(text);
    t.widget.add_css_class("shown");
    let serial = t.serial.get().wrapping_add(1);
    t.serial.set(serial);
    let me = Rc::downgrade(&t);
    glib::timeout_add_local_once(SHOWN, move || {
        if let Some(t) = me.upgrade().filter(|t| t.serial.get() == serial) {
            t.widget.remove_css_class("shown");
        }
    });
}
