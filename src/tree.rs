//! File tree: a virtualized GtkListView over a lazily-expanded GtkTreeListModel.
//! Directories are only listed when expanded.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::gio;

use crate::icons::{self, codicon};

/// VS Code's default `files.exclude`.
const HIDDEN: &[&str] = &[".git", ".svn", ".hg", "CVS", ".DS_Store", "Thumbs.db"];

fn is_dir(info: &gio::FileInfo) -> bool {
    info.file_type() == gio::FileType::Directory
}

fn file_of(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_object("standard::file")?.downcast::<gio::File>().ok()
}

fn dir_model(dir: &gio::File) -> gio::ListModel {
    let list = gtk::DirectoryList::new(Some("standard::name,standard::display-name,standard::type"), Some(dir));
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

pub fn build(root: &Path, on_open: Rc<dyn Fn(PathBuf)>) -> gtk::ScrolledWindow {
    let tree = gtk::TreeListModel::new(dir_model(&gio::File::for_path(root)), false, false, |item| {
        let info = item.downcast_ref::<gio::FileInfo>()?;
        if !is_dir(info) {
            return None;
        }
        Some(dir_model(&file_of(info)?))
    });
    let selection = gtk::SingleSelection::new(Some(tree));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        // GTK's expander arrow is hidden; VS Code's codicon chevron is drawn instead.
        let expander = gtk::TreeExpander::new();
        expander.set_hide_expander(true);
        expander.set_indent_for_icon(false);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_hexpand(true);
        let twistie = gtk::Label::new(None);
        twistie.add_css_class("codicon");
        twistie.add_css_class("twistie");
        row.append(&twistie);
        let icon = gtk::Label::new(None);
        icon.add_css_class("seti");
        let name = gtk::Label::new(None);
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&icon);
        row.append(&name);
        expander.set_child(Some(&row));
        item.set_child(Some(&expander));

        // Clicking the name toggles folders / opens files (the arrow toggles by itself).
        let click = gtk::GestureClick::new();
        let on_open = on_open.clone();
        let exp = expander.clone();
        let tw = twistie.clone();
        click.connect_released(move |_, _, _, _| {
            let Some(row) = exp.list_row() else { return };
            let Some(info) = row.item().and_downcast::<gio::FileInfo>() else { return };
            if is_dir(&info) {
                row.set_expanded(!row.is_expanded());
                tw.set_text(twistie_glyph(&row));
            } else if let Some(path) = file_of(&info).and_then(|f| f.path()) {
                on_open(path);
            }
        });
        row.add_controller(click);
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = item.item().and_downcast::<gtk::TreeListRow>().unwrap();
        let expander = item.child().and_downcast::<gtk::TreeExpander>().unwrap();
        expander.set_list_row(Some(&row));
        let info = row.item().and_downcast::<gio::FileInfo>().unwrap();
        let hbox = expander.child().unwrap();
        let twistie = hbox.first_child().and_downcast::<gtk::Label>().unwrap();
        let icon = twistie.next_sibling().and_downcast::<gtk::Label>().unwrap();
        let name = icon.next_sibling().and_downcast::<gtk::Label>().unwrap();
        let display = info.display_name();
        name.set_text(&display);
        twistie.set_text(twistie_glyph(&row));
        // VS Code collapses the twistie of leaves: file icons sit where chevrons would.
        twistie.set_visible(is_dir(&info));
        if is_dir(&info) {
            icon.set_visible(false);
        } else {
            icon.set_visible(true);
            icon.set_markup(&icons::markup(icons::for_file(&display)));
        }
    });

    let list = gtk::ListView::new(Some(selection), Some(factory));
    list.add_css_class("tree");
    gtk::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build()
}
