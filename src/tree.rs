//! File tree: a virtualized GtkListView over a lazily-expanded GtkTreeListModel.
//! Directories are only listed (and watched, via GtkDirectoryList) while expanded.
//! Rows carry VS Code's git decorations: color + a letter badge.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::git;
use crate::icons::{self, codicon};

/// VS Code's default `files.exclude`.
const HIDDEN: &[&str] = &[".git", ".svn", ".hg", "CVS", ".DS_Store", "Thumbs.db"];

fn is_dir(info: &gio::FileInfo) -> bool {
    info.file_type() == gio::FileType::Directory
}

fn file_of(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_object("standard::file")?.downcast::<gio::File>().ok()
}

pub struct Tree {
    pub widget: gtk::ScrolledWindow,
    status: RefCell<Rc<git::Status>>,
    /// Every row widget the factory created (rows are recycled, so this stays small).
    rows: RefCell<Vec<glib::WeakRef<gtk::TreeExpander>>>,
    /// Called when an expanded directory's contents change (files created/deleted).
    on_dir_changed: Rc<dyn Fn()>,
}

impl Tree {
    /// `on_open(path, pin)`: single click opens a preview tab, double click pins it.
    pub fn new(root: &Path, on_open: Rc<dyn Fn(PathBuf, bool)>, on_dir_changed: Rc<dyn Fn()>) -> Rc<Tree> {
        let tree = Rc::new(Tree {
            widget: gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build(),
            status: RefCell::new(Rc::new(git::Status::default())),
            rows: RefCell::new(Vec::new()),
            on_dir_changed,
        });
        let model_tree = Rc::downgrade(&tree);
        let root_model = tree.dir_model(&gio::File::for_path(root));
        let model = gtk::TreeListModel::new(root_model, false, false, move |item| {
            let info = item.downcast_ref::<gio::FileInfo>()?;
            if !is_dir(info) {
                return None;
            }
            Some(model_tree.upgrade()?.dir_model(&file_of(info)?))
        });
        let selection = gtk::SingleSelection::new(Some(model));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);

        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(&tree);
        factory.connect_setup(move |_, item| {
            let Some(t) = weak.upgrade() else { return };
            t.setup_row(item.downcast_ref::<gtk::ListItem>().unwrap(), on_open.clone());
        });
        let weak = Rc::downgrade(&tree);
        factory.connect_bind(move |_, item| {
            let Some(t) = weak.upgrade() else { return };
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let row = item.item().and_downcast::<gtk::TreeListRow>().unwrap();
            let expander = item.child().and_downcast::<gtk::TreeExpander>().unwrap();
            expander.set_list_row(Some(&row));
            t.decorate(&expander);
        });

        let list = gtk::ListView::new(Some(selection), Some(factory));
        list.add_css_class("tree");
        tree.widget.set_child(Some(&list));
        tree
    }

    fn dir_model(&self, dir: &gio::File) -> gio::ListModel {
        let list = gtk::DirectoryList::new(Some("standard::name,standard::display-name,standard::type"), Some(dir));
        let cb = self.on_dir_changed.clone();
        list.connect_items_changed(move |l, _, removed, added| {
            // Ignore the initial listing; react to later creations/deletions.
            if !l.is_loading() && (removed > 0 || added > 0) {
                cb();
            }
        });
        let filter = gtk::CustomFilter::new(|obj| {
            let info = obj.downcast_ref::<gio::FileInfo>().unwrap();
            !HIDDEN.iter().any(|h| info.name().as_os_str() == *h)
        });
        // VS Code's default sort: folders first, then case-insensitive name.
        let sorter = gtk::CustomSorter::new(|a, b| {
            let a = a.downcast_ref::<gio::FileInfo>().unwrap();
            let b = b.downcast_ref::<gio::FileInfo>().unwrap();
            is_dir(b)
                .cmp(&is_dir(a))
                .then_with(|| a.display_name().to_lowercase().cmp(&b.display_name().to_lowercase()))
                .into()
        });
        let filtered = gtk::FilterListModel::new(Some(list), Some(filter));
        gtk::SortListModel::new(Some(filtered), Some(sorter)).upcast()
    }

    fn setup_row(&self, item: &gtk::ListItem, on_open: Rc<dyn Fn(PathBuf, bool)>) {
        // GTK's expander arrow is hidden; VS Code's codicon chevron is drawn instead.
        let expander = gtk::TreeExpander::new();
        expander.set_hide_expander(true);
        expander.set_indent_for_icon(false);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_hexpand(true);
        let twistie = gtk::Label::new(None);
        twistie.add_css_class("codicon");
        twistie.add_css_class("twistie");
        let icon = gtk::Label::new(None);
        icon.add_css_class("seti");
        let name = gtk::Label::new(None);
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let badge = gtk::Label::new(None);
        badge.add_css_class("git-badge");
        for w in [&twistie, &icon, &name, &badge] {
            row.append(w);
        }
        expander.set_child(Some(&row));
        item.set_child(Some(&expander));
        self.rows.borrow_mut().push(expander.downgrade());

        // Clicking the name toggles folders / opens files (the arrow toggles by itself).
        let click = gtk::GestureClick::new();
        let exp = expander.clone();
        click.connect_released(move |_, n_press, _, _| {
            let Some(row) = exp.list_row() else { return };
            let Some(info) = row.item().and_downcast::<gio::FileInfo>() else { return };
            if is_dir(&info) {
                if n_press == 1 {
                    row.set_expanded(!row.is_expanded());
                    if let Some(tw) = exp.child().and_then(|c| c.first_child()).and_downcast::<gtk::Label>() {
                        tw.set_text(twistie_glyph(&row));
                    }
                }
            } else if let Some(path) = file_of(&info).and_then(|f| f.path()) {
                on_open(path, n_press >= 2);
            }
        });
        row.add_controller(click);
    }

    /// Name, icon, chevron and git decoration of a bound row.
    fn decorate(&self, expander: &gtk::TreeExpander) {
        let Some(row) = expander.list_row() else { return };
        let Some(info) = row.item().and_downcast::<gio::FileInfo>() else { return };
        let Some(hbox) = expander.child() else { return };
        let twistie = hbox.first_child().and_downcast::<gtk::Label>().unwrap();
        let icon = twistie.next_sibling().and_downcast::<gtk::Label>().unwrap();
        let name = icon.next_sibling().and_downcast::<gtk::Label>().unwrap();
        let badge = name.next_sibling().and_downcast::<gtk::Label>().unwrap();
        let display = info.display_name();
        let dir = is_dir(&info);
        name.set_text(&display);
        twistie.set_text(twistie_glyph(&row));
        // VS Code collapses the twistie of leaves: file icons sit where chevrons would.
        twistie.set_visible(dir);
        icon.set_visible(!dir);
        if !dir {
            icon.set_markup(&icons::markup(icons::for_file(&display)));
        }

        let kind = file_of(&info).and_then(|f| f.path()).and_then(|p| self.status.borrow().kind(&p, dir));
        for c in git::Kind::ALL_CSS {
            hbox.remove_css_class(c);
        }
        if let Some(k) = kind {
            hbox.add_css_class(k.css());
        }
        // Files show the letter; folders show a dot when something inside changed.
        badge.set_text(match kind {
            Some(git::Kind::Ignored) | None => "",
            Some(_) if dir => "•",
            Some(k) => k.letter(),
        });
    }

    pub fn set_status(&self, status: Rc<git::Status>) {
        *self.status.borrow_mut() = status;
        self.rows.borrow_mut().retain(|w| w.upgrade().is_some());
        for w in self.rows.borrow().iter() {
            if let Some(e) = w.upgrade() {
                self.decorate(&e);
            }
        }
    }
}

/// Chevron for a row. Uses the file type rather than `TreeListRow::is_expandable()`, which
/// would create (and start listing + monitoring) the child model of every collapsed folder.
fn twistie_glyph(row: &gtk::TreeListRow) -> &'static str {
    let dir = row.item().and_downcast::<gio::FileInfo>().is_some_and(|i| is_dir(&i));
    match dir {
        true if row.is_expanded() => codicon::CHEVRON_DOWN,
        true => codicon::CHEVRON_RIGHT,
        false => "",
    }
}
