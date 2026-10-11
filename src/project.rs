//! One open project: its explorer, editor group, terminals and file index.
//! Everything stays alive while the project is in the background.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::codeview::SharedStyle;
use crate::editor::EditorGroup;
use crate::icons::{self, codicon};
use crate::quickopen::{self, Item, Provider, QuickPick};
use crate::terminal::TerminalPanel;
use crate::{fuzzy, tree};

/// Directories skipped when indexing a folder that isn't a git repository.
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".venv", "venv", "__pycache__", ".cache", ".next", "dist"];
const MAX_INDEX: usize = 300_000;
const MAX_RECENT: usize = 30;

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub page: gtk::Paned,
    pub editor: Rc<EditorGroup>,
    pub terminals: Rc<TerminalPanel>,
    pub terminals_started: Cell<bool>,
    index: RefCell<Arc<Vec<String>>>,
    ignored: RefCell<Arc<Vec<String>>>,
    indexing: Cell<bool>,
    recent: RefCell<Vec<PathBuf>>,
}

impl Project {
    pub fn new(root: &Path, style: SharedStyle, scale: f64) -> Rc<Project> {
        let name = root.file_name().map_or_else(|| root.to_string_lossy().to_string(), |n| n.to_string_lossy().to_string());
        let editor = EditorGroup::new(root, style);
        let terminals = TerminalPanel::new(root, scale);

        let vpaned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&editor.widget)
            .end_child(&terminals.widget)
            .resize_end_child(false)
            .shrink_end_child(false)
            .position((470.0 * scale) as i32)
            .build();
        let page = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .end_child(&vpaned)
            .resize_start_child(false)
            .shrink_start_child(false)
            .position((300.0 * scale) as i32)
            .hexpand(true)
            .vexpand(true)
            .build();

        let project = Rc::new(Project {
            root: root.to_path_buf(),
            name,
            page,
            editor,
            terminals,
            terminals_started: Cell::new(false),
            index: RefCell::new(Arc::new(Vec::new())),
            ignored: RefCell::new(Arc::new(Vec::new())),
            indexing: Cell::new(false),
            recent: RefCell::new(Vec::new()),
        });

        // Explorer
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.add_css_class("sidebar");
        let title = gtk::Label::new(Some("Explorer"));
        title.add_css_class("title");
        title.set_xalign(0.0);
        sidebar.append(&title);
        let section = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        section.add_css_class("section");
        let chevron = gtk::Label::new(Some(codicon::CHEVRON_DOWN));
        chevron.add_css_class("codicon");
        section.append(&chevron);
        section.append(&gtk::Label::new(Some(&project.name)));
        sidebar.append(&section);
        let weak = Rc::downgrade(&project);
        sidebar.append(&tree::build(
            root,
            Rc::new(move |path, pin| {
                if let Some(p) = weak.upgrade() {
                    p.open_file(&path, pin);
                }
            }),
        ));
        project.page.set_start_child(Some(&sidebar));
        project
    }

    pub fn open_file(&self, path: &Path, pin: bool) {
        self.editor.open(path, pin);
        let mut recent = self.recent.borrow_mut();
        recent.retain(|p| p != path);
        recent.insert(0, path.to_path_buf());
        recent.truncate(MAX_RECENT);
    }

    /// Rebuild the file index in the background, then call `then`.
    pub fn refresh_index(self: &Rc<Self>, include_ignored: bool, then: Rc<dyn Fn()>) {
        if self.indexing.replace(true) {
            return;
        }
        let root = self.root.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || {
                let normal = build_index(&root, false);
                let ignored = if include_ignored { Some(build_index(&root, true)) } else { None };
                (normal, ignored)
            })
            .await;
            let Some(p) = weak.upgrade() else { return };
            p.indexing.set(false);
            if let Ok((normal, ignored)) = res {
                *p.index.borrow_mut() = Arc::new(normal);
                if let Some(ig) = ignored {
                    *p.ignored.borrow_mut() = Arc::new(ig);
                }
                then();
            }
        });
    }

    pub fn running_jobs(&self) -> Vec<String> {
        self.terminals.running_jobs()
    }
}

/// Project files relative to `root`. Uses git when possible (fast, respects .gitignore);
/// `ignored = true` lists only the files git ignores.
pub fn build_index(root: &Path, ignored: bool) -> Vec<String> {
    let args: &[&str] = if ignored {
        &["--no-optional-locks", "ls-files", "-z", "--others", "--ignored", "--exclude-standard"]
    } else {
        &["--no-optional-locks", "ls-files", "-z", "--cached", "--others", "--exclude-standard"]
    };
    let git = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stderr(std::process::Stdio::null())
        .output();
    if let Ok(out) = git {
        if out.status.success() {
            let mut files: Vec<String> = out
                .stdout
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .take(MAX_INDEX)
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect();
            files.sort_unstable();
            files.dedup();
            return files;
        }
    }
    if ignored {
        return Vec::new();
    }
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name();
            if ft.is_dir() {
                if !SKIP_DIRS.iter().any(|s| name == *s) {
                    stack.push(e.path());
                }
            } else if let Ok(rel) = e.path().strip_prefix(root) {
                files.push(rel.to_string_lossy().into_owned());
                if files.len() >= MAX_INDEX {
                    return files;
                }
            }
        }
    }
    files.sort_unstable();
    files
}

/// "Go to File" (Ctrl+P) for one project.
pub struct FileProvider {
    project: Rc<Project>,
    quick: Weak<QuickPick>,
    include_ignored: Cell<bool>,
    highlight: String,
}

impl FileProvider {
    pub fn open(project: Rc<Project>, quick: &Rc<QuickPick>, highlight: String, focus: Option<gtk::Widget>) {
        let provider = Rc::new(FileProvider {
            project: project.clone(),
            quick: Rc::downgrade(quick),
            include_ignored: Cell::new(false),
            highlight,
        });
        quick.open(provider.clone(), focus);
        provider.reindex();
    }

    fn reindex(&self) {
        let q = self.quick.clone();
        self.project.refresh_index(
            self.include_ignored.get(),
            Rc::new(move || {
                if let Some(q) = q.upgrade() {
                    q.requery();
                }
            }),
        );
    }

    fn item(&self, rel: &str, positions: &[usize], ignored: bool) -> Item {
        let split = rel.rfind('/').map_or(0, |i| i + 1);
        let (dir, name) = rel.split_at(split);
        Item {
            key: rel.to_string(),
            icon: icons::markup(icons::for_file(name)),
            label: quickopen::highlight_markup(name, positions, split, &self.highlight),
            detail: quickopen::highlight_markup(dir.trim_end_matches('/'), positions, 0, &self.highlight),
            group: ignored.then(|| "ignored".to_string()),
        }
    }
}

impl Provider for FileProvider {
    fn query(&self, query: &str, done: Box<dyn Fn(Vec<Item>)>) {
        let normal = self.project.index.borrow().clone();
        let ignored = if self.include_ignored.get() { self.project.ignored.borrow().clone() } else { Arc::new(Vec::new()) };
        if query.trim().is_empty() {
            let root = &self.project.root;
            let mut items: Vec<Item> = self
                .project
                .recent
                .borrow()
                .iter()
                .filter(|p| p.exists())
                .filter_map(|p| p.strip_prefix(root).ok())
                .map(|rel| self.item(&rel.to_string_lossy(), &[], false))
                .collect();
            if let Some(first) = items.first_mut() {
                first.group = Some("recently opened".into());
            }
            done(items);
            return;
        }
        let q = query.to_string();
        let me = FileProvider {
            project: self.project.clone(),
            quick: self.quick.clone(),
            include_ignored: Cell::new(self.include_ignored.get()),
            highlight: self.highlight.clone(),
        };
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || {
                let mut all: Vec<(String, fuzzy::Match, bool)> = fuzzy::top(&q, &normal, 60)
                    .into_iter()
                    .map(|(p, m)| (p.to_string(), m, false))
                    .collect();
                all.extend(fuzzy::top(&q, &ignored, 60).into_iter().map(|(p, m)| (p.to_string(), m, true)));
                // Ignored files rank below equally good regular ones.
                all.sort_by(|a, b| (b.1.score - 20 * b.2 as i32).cmp(&(a.1.score - 20 * a.2 as i32)));
                all.truncate(60);
                all
            })
            .await;
            if let Ok(results) = res {
                done(results.iter().map(|(p, m, ig)| me.item(p, &m.positions, *ig)).collect());
            }
        });
    }

    fn accept(&self, key: &str) {
        self.project.open_file(&self.project.root.join(key), true);
        self.project.editor.focus();
    }

    fn key(&self, key: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> bool {
        if key.to_lower() == gtk::gdk::Key::i && mods.contains(gtk::gdk::ModifierType::ALT_MASK) {
            self.include_ignored.set(!self.include_ignored.get());
            if self.include_ignored.get() {
                self.reindex();
            }
            return true;
        }
        false
    }

    fn footer(&self) -> String {
        let n = self.project.index.borrow().len();
        if self.include_ignored.get() {
            let m = self.project.ignored.borrow().len();
            format!("{n} files + {m} ignored  ·  Alt+I: hide ignored files")
        } else {
            format!("{n} files  ·  Alt+I: include ignored files (.env, build output, node_modules…)")
        }
    }

    fn placeholder(&self) -> String {
        format!("Go to file in {} (space-separated terms narrow the search)", self.project.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_this_repo() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let files = build_index(root, false);
        assert!(files.iter().any(|f| f == "src/main.rs"));
        assert!(!files.iter().any(|f| f.starts_with("target/")));
    }
}
