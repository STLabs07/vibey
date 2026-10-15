//! Right-click menus.
//!
//! Each menu is one popover, created once and parented to the window content for as long as
//! the window lives. Don't build one per click and `unparent()` it afterwards: GTK 4.14 can
//! make a just-closed popover the window's pointer-focus target (a late event on its hidden
//! surface) and drops that target only when the widget is *disposed*. An unparented popover
//! has no surface, so if anything still holds it (a clone captured by its own button's
//! closure is enough) the next layout pass calls `gdk_surface_request_motion(NULL)` and the
//! whole process, shells included, dies with SIGSEGV. Closures here hold weak references.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::graphene;
use gtk::prelude::*;

/// One row: label, shortcut hint, and whether a separator line goes above it.
pub type Entry = (&'static str, &'static str, bool);

/// Runs with the index of the row the user picked.
type Pick = Rc<dyn Fn(usize)>;

pub struct ContextMenu {
    popover: gtk::Popover,
    /// What the popover is parented to; pointer coordinates are translated into its space.
    parent: gtk::Widget,
    buttons: Vec<gtk::Button>,
    /// Replaced on every `show`.
    pick: RefCell<Option<Pick>>,
}

impl ContextMenu {
    pub fn new(parent: &impl IsA<gtk::Widget>, entries: &[Entry]) -> Rc<ContextMenu> {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_halign(gtk::Align::Start);
        popover.add_css_class("menu-popover");
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let menu = Rc::new_cyclic(|me: &Weak<ContextMenu>| {
            let buttons = entries
                .iter()
                .enumerate()
                .map(|(i, &(name, accel, separator))| {
                    if separator {
                        list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                    }
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 24);
                    let label = gtk::Label::new(Some(name));
                    label.set_hexpand(true);
                    label.set_xalign(0.0);
                    row.append(&label);
                    let keys = gtk::Label::new(Some(accel));
                    keys.add_css_class("accel");
                    row.append(&keys);
                    let button = gtk::Button::builder().child(&row).build();
                    button.add_css_class("flat");
                    button.add_css_class("menu-item");
                    let me = me.clone();
                    button.connect_clicked(move |_| {
                        if let Some(menu) = me.upgrade() {
                            menu.activate(i);
                        }
                    });
                    list.append(&button);
                    button
                })
                .collect();
            ContextMenu { popover: popover.clone(), parent: parent.as_ref().clone(), buttons, pick: RefCell::new(None) }
        });
        popover.set_child(Some(&list));
        popover.set_parent(parent);
        menu
    }

    /// Pop the menu up at (`x`, `y`) in `source`'s coordinates. `enabled` is per row (rows past
    /// its end stay enabled); `pick` gets the index of the row the user chooses.
    pub fn show(&self, source: &impl IsA<gtk::Widget>, x: f64, y: f64, enabled: &[bool], pick: impl Fn(usize) + 'static) {
        self.popover.popdown();
        let at = graphene::Point::new(x as f32, y as f32);
        let at = source.compute_point(&self.parent, &at).unwrap_or(at);
        self.popover.set_pointing_to(Some(&gdk::Rectangle::new(at.x() as i32, at.y() as i32, 1, 1)));
        for (i, button) in self.buttons.iter().enumerate() {
            button.set_sensitive(enabled.get(i).copied().unwrap_or(true));
        }
        *self.pick.borrow_mut() = Some(Rc::new(pick));
        self.popover.popup();
    }

    fn activate(&self, i: usize) {
        self.popover.popdown();
        let pick = self.pick.borrow().clone();
        if let Some(pick) = pick {
            pick(i);
        }
    }

    /// Self-test access.
    pub fn popover(&self) -> &gtk::Popover {
        &self.popover
    }

    /// Self-test: choose row `i` as if it were clicked.
    pub fn choose(&self, i: usize) {
        self.activate(i);
    }
}
