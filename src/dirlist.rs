//! A directory's entries as a `gio::ListStore` of `FileInfo`, listed and kept current with GIO.
//!
//! This replaces `GtkDirectoryList`. In GTK 4.14.5 its monitor events are queued as raw
//! pointers to the list and to heap events, and each pending `g_file_query_info_async` keeps
//! those pointers; collapsing a folder (or closing a project) while a file was being created
//! disposes the list and frees the events, and the cancelled query's callback then runs on
//! freed memory (SIGSEGV in `g_file_query_info_finish`). Here every async step holds only a
//! weak reference to the store and stops when it is gone, so nothing can outlive it.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

const ATTRS: &str = "standard::name,standard::display-name,standard::type";
const BATCH: i32 = 256;

enum Event {
    /// Created, moved in, or changed: (re)query the file and insert or replace its entry.
    Upsert(gio::File),
    Remove(gio::File),
}

struct State {
    dir: gio::File,
    /// Monitor events, applied one at a time in arrival order (after the initial listing).
    queue: RefCell<VecDeque<Event>>,
    /// Whether the worker task is running.
    busy: Cell<bool>,
    /// Whether the initial listing is in.
    loaded: Cell<bool>,
    on_change: Rc<dyn Fn()>,
}

/// List `dir` (asynchronously) and keep the store current while it lives. `on_change` runs
/// after each later creation/deletion, not for the initial listing.
pub fn open(dir: &gio::File, on_change: Rc<dyn Fn()>) -> gio::ListStore {
    let store = gio::ListStore::new::<gio::FileInfo>();
    let state = Rc::new(State { dir: dir.clone(), queue: RefCell::new(VecDeque::new()), busy: Cell::new(false), loaded: Cell::new(false), on_change });

    // Started before listing so nothing created meanwhile is missed; the worker applies these
    // only once the listing is in (an entry seen by both is simply replaced).
    if let Ok(monitor) = dir.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
        let (weak, st) = (store.downgrade(), state.clone());
        monitor.connect_changed(move |_, file, other, event| {
            use gio::FileMonitorEvent as E;
            let mut q = st.queue.borrow_mut();
            match event {
                E::Created | E::MovedIn | E::AttributeChanged => q.push_back(Event::Upsert(file.clone())),
                E::Deleted | E::MovedOut => q.push_back(Event::Remove(file.clone())),
                E::Renamed => {
                    q.push_back(Event::Remove(file.clone()));
                    if let Some(to) = other {
                        q.push_back(Event::Upsert(to.clone()));
                    }
                }
                _ => return,
            }
            drop(q);
            kick(&weak, &st);
        });
        // Dies with the store, which cancels the monitor.
        unsafe { store.set_data("vibey-dir-monitor", monitor) };
    }
    kick(&store.downgrade(), &state);
    store
}

fn kick(store: &glib::WeakRef<gio::ListStore>, state: &Rc<State>) {
    if !state.busy.replace(true) {
        glib::spawn_future_local(work(store.clone(), state.clone()));
    }
}

/// The worker: the initial listing, then queued monitor events until none are left. Only one
/// runs at a time, so events apply in order even though each needs an async query.
async fn work(store: glib::WeakRef<gio::ListStore>, state: Rc<State>) {
    if !state.loaded.get() {
        if !list(&store, &state.dir).await {
            state.busy.set(false);
            return;
        }
        state.loaded.set(true);
    }
    loop {
        let Some(event) = state.queue.borrow_mut().pop_front() else {
            state.busy.set(false);
            return;
        };
        let changed = match event {
            Event::Upsert(file) => {
                let info = file.query_info_future(ATTRS, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await;
                let (Ok(info), Some(store)) = (info, store.upgrade()) else { continue };
                info.set_attribute_object("standard::file", &file);
                match position(&store, &file) {
                    Some(i) => store.splice(i, 1, &[info]),
                    None => store.append(&info),
                }
                true
            }
            Event::Remove(file) => match store.upgrade().and_then(|s| position(&s, &file).map(|i| (s, i))) {
                Some((store, i)) => {
                    store.remove(i);
                    true
                }
                None => false,
            },
        };
        if changed {
            (state.on_change)();
        }
    }
}

/// Initial listing, in batches. False if the store went away (or the directory is unreadable).
async fn list(store: &glib::WeakRef<gio::ListStore>, dir: &gio::File) -> bool {
    let Ok(en) = dir
        .enumerate_children_future(ATTRS, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT)
        .await
    else {
        return store.upgrade().is_some();
    };
    loop {
        let Ok(batch) = en.next_files_future(BATCH, glib::Priority::DEFAULT).await else { return true };
        let Some(store) = store.upgrade() else { return false };
        if batch.is_empty() {
            return true;
        }
        for info in &batch {
            info.set_attribute_object("standard::file", &dir.child(info.name()));
        }
        store.splice(store.n_items(), 0, &batch);
    }
}

fn position(store: &gio::ListStore, file: &gio::File) -> Option<u32> {
    let name = file.basename()?;
    (0..store.n_items()).find(|&i| store.item(i).and_downcast::<gio::FileInfo>().is_some_and(|f| f.name() == name))
}
