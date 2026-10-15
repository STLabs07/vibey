//! Code view: GtkTextView + a custom gutter (line numbers, git bars).
//!
//! Editing is deliberately plain: type, paste, undo, save. It exists for things an AI CLI
//! can't do for you (pasting an API key into `.env`), not to compete with an IDE.
//!
//! - `dirty`: the buffer differs from `text`, the file as last read or written.
//! - Save writes atomically and keeps the file's permissions. It refuses (a *conflict*) when
//!   the file on disk is no longer the version the edits started from, unless forced.
//! - Files that can't round-trip through a text buffer (not UTF-8, NUL bytes) stay read-only.
//! - Clicking a Markdown task box (`- [ ]`) ticks or unticks it.
//!
//! Live reload: a GIO file monitor triggers a re-read; the new text is diffed against the
//! old off the UI thread, the buffer is edited in place (so the scroll position stays), and
//! changed lines are highlighted briefly, then fade out. A reload never touches a dirty
//! buffer; it flags a conflict instead.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib, graphene, pango};
use similar::{Algorithm, DiffTag};

use crate::highlight::{self, BracketColors, Highlighted, SpanStyle};
use crate::theme::Rgba;

/// Larger files get a notice instead of their contents.
const MAX_OPEN_BYTES: u64 = 8 << 20;
/// Spans applied per idle callback, so huge files don't block the UI.
const SPANS_PER_CHUNK: usize = 50_000;
const MAX_FIND_MATCHES: usize = 10_000;
/// Changed-line highlight: hold, then fade.
const CHANGE_HOLD: Duration = Duration::from_millis(1200);
const CHANGE_FADE: Duration = Duration::from_millis(1500);

/// Colors and theme shared by all code views; replaced when the theme changes.
pub struct ViewStyle {
    pub theme_id: String,
    pub brackets: Arc<BracketColors>,
    pub line_number: Rgba,
    pub changed: Rgba,
    pub find_match: Rgba,
    pub find_current: Rgba,
    pub git_added: Rgba,
    pub git_modified: Rgba,
    pub git_deleted: Rgba,
    pub scale: f64,
}

/// Gutter marker per line, vs the git index (what VS Code's quick diff shows).
pub const MARK_ADDED: u8 = 1;
pub const MARK_MODIFIED: u8 = 2;
/// Lines were deleted below this line.
pub const MARK_DELETED_BELOW: u8 = 3;
/// Lines were deleted above the first line.
pub const MARK_DELETED_ABOVE: u8 = 4;

pub type SharedStyle = Rc<RefCell<ViewStyle>>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Gutter {
        pub view: glib::WeakRef<gtk::TextView>,
        pub width: Cell<i32>,
        pub number_right: Cell<i32>,
        pub text_top: Cell<i32>,
        pub color: RefCell<Option<gtk::gdk::RGBA>>,
        pub markers: RefCell<Vec<u8>>,
        /// added, modified, deleted
        pub marker_colors: RefCell<Vec<gtk::gdk::RGBA>>,
        pub marker_x: Cell<f32>,
        pub marker_w: Cell<f32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Gutter {
        const NAME: &'static str = "VibeyGutter";
        type Type = super::Gutter;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("gutter");
        }
    }

    impl ObjectImpl for Gutter {}

    impl WidgetImpl for Gutter {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Horizontal {
                let w = self.width.get();
                (w, w, -1, -1)
            } else {
                (0, 0, -1, -1)
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(view) = self.view.upgrade() else { return };
            let color = self.color.borrow().unwrap_or(gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 1.0));
            let layout = self.obj().create_pango_layout(None);
            let visible = view.visible_rect();
            let (mut iter, _) = view.line_at_y(visible.y());
            let bottom = visible.y() + visible.height();
            let right = self.number_right.get();
            let markers = self.markers.borrow();
            let mc = self.marker_colors.borrow();
            let (mx, mw) = (self.marker_x.get(), self.marker_w.get());
            let has_colors = mc.len() == 3;
            loop {
                let (y, h) = view.line_yrange(&iter);
                if y > bottom {
                    break;
                }
                let (_, wy) = view.buffer_to_window_coords(gtk::TextWindowType::Left, 0, y);
                let (fy, fh) = (wy as f32, h as f32);
                match markers.get(iter.line() as usize).copied().filter(|_| has_colors).unwrap_or(0) {
                    MARK_ADDED => snapshot.append_color(&mc[0], &graphene::Rect::new(mx, fy, mw, fh)),
                    MARK_MODIFIED => snapshot.append_color(&mc[1], &graphene::Rect::new(mx, fy, mw, fh)),
                    MARK_DELETED_BELOW => {
                        snapshot.append_color(&mc[2], &graphene::Rect::new(mx, fy + fh - mw, mw * 2.5, mw * 1.5))
                    }
                    MARK_DELETED_ABOVE => snapshot.append_color(&mc[2], &graphene::Rect::new(mx, fy, mw * 2.5, mw * 1.5)),
                    _ => {}
                }
                layout.set_text(&(iter.line() + 1).to_string());
                let (_, logical) = layout.pixel_extents();
                snapshot.save();
                snapshot.translate(&graphene::Point::new(
                    (right - logical.width()) as f32,
                    (wy + self.text_top.get()) as f32,
                ));
                snapshot.append_layout(&layout, &color);
                snapshot.restore();
                if !iter.forward_line() {
                    break;
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct Gutter(ObjectSubclass<imp::Gutter>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

enum Content {
    Text(Arc<str>),
    /// Text with bytes that aren't UTF-8, shown with replacement characters. Viewable but
    /// never saved: writing it back would corrupt the file.
    Lossy(Arc<str>),
    Binary,
    TooLarge(u64),
    Missing,
    Error(String),
}

/// Markdown task boxes (`- [ ]`, `* [x]`, `1. [ ]`, or a bare `[ ]` starting the line):
/// (line, byte column of the `[`).
fn task_boxes(text: &str) -> Vec<(usize, usize)> {
    text.split('\n')
        .enumerate()
        .filter_map(|(i, line)| {
            let body = line.trim_start();
            let digits = body.bytes().take_while(u8::is_ascii_digit).count();
            let rest = if let Some(r) = body.strip_prefix(['-', '*', '+']) {
                r.strip_prefix(' ')?
            } else if digits > 0 {
                body[digits..].strip_prefix(['.', ')'])?.strip_prefix(' ')?
            } else {
                body
            };
            let b = rest.as_bytes();
            let is_box = b.len() >= 3
                && b[0] == b'['
                && matches!(b[1], b' ' | b'x' | b'X')
                && b[2] == b']'
                && b.get(3).is_none_or(|c| c.is_ascii_whitespace());
            is_box.then(|| (i, line.len() - rest.len()))
        })
        .collect()
}

fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// The file's index version, cached until the index changes.
type Base = Option<(std::time::SystemTime, Arc<str>)>;

struct Loaded {
    content: Content,
    /// Not valid UTF-8: shown, but read-only.
    lossy: bool,
    hl: Option<Highlighted>,
    markers: Vec<u8>,
    base: Base,
    /// Line diff old → new: (tag, old lines, new lines), for reloads.
    ops: Vec<(DiffTag, Range<usize>, Range<usize>)>,
}

fn read(path: &Path) -> Content {
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Content::Missing,
        Err(e) => return Content::Error(e.to_string()),
    };
    if size > MAX_OPEN_BYTES {
        return Content::TooLarge(size);
    }
    match std::fs::read(path) {
        // A text buffer can't hold a NUL, so such a file couldn't be shown (or saved) intact.
        Ok(b) if b.contains(&0) => Content::Binary,
        Ok(b) => match String::from_utf8(b) {
            Ok(s) => Content::Text(s.into()),
            Err(e) => Content::Lossy(String::from_utf8_lossy(e.as_bytes()).into()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Content::Missing,
        Err(e) => Content::Error(e.to_string()),
    }
}

/// `text` as it should be written over a file whose last-known content is `disk`: a file
/// that uses CRLF throughout keeps it, so a line the user adds (a bare `\n`) doesn't leave
/// the file with mixed endings.
fn with_file_eol(disk: &str, text: &str) -> String {
    let crlf = disk.contains("\r\n") && !disk.replace("\r\n", "").contains('\n');
    if !crlf {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut prev = '\0';
    for c in text.chars() {
        if c == '\n' && prev != '\r' {
            out.push('\r');
        }
        out.push(c);
        prev = c;
    }
    out
}

/// Replace `path`'s content: write a sibling temp file, then rename it over the target, so a
/// crash can't leave a half-written file. Keeps the file's permissions (it's often a secrets
/// file: the temp file is owner-only until they're copied) and writes through symlinks. If
/// the directory isn't writable, falls back to rewriting the file in place.
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let meta = std::fs::metadata(&target).ok();
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = target.with_file_name(format!(".{name}.vibey-{}.tmp", std::process::id()));
    let atomic = || -> std::io::Result<()> {
        // An existing file's mode is copied after writing; a new file gets the umask default.
        let mode = if meta.is_some() { 0o600 } else { 0o666 };
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
        f.write_all(bytes)?;
        if let Some(m) = &meta {
            f.set_permissions(m.permissions())?;
        }
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &target)
    };
    match atomic() {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(&target, bytes)
        }
    }
}

/// Line markers of `text` vs `base` (both whole files).
fn line_markers(base: &str, text: &str) -> Vec<u8> {
    let a: Vec<&str> = base.split_inclusive('\n').collect();
    let b: Vec<&str> = text.split_inclusive('\n').collect();
    let mut marks = vec![0u8; b.len().max(1)];
    let deadline = Instant::now() + Duration::from_millis(150);
    for op in similar::capture_diff_slices_deadline(Algorithm::Myers, &a, &b, Some(deadline)) {
        let (tag, _old, new) = op.as_tag_tuple();
        match tag {
            DiffTag::Insert => marks[new.clone()].fill(MARK_ADDED),
            DiffTag::Replace => marks[new.clone()].fill(MARK_MODIFIED),
            DiffTag::Delete if new.start == 0 => marks[0] = marks[0].max(MARK_DELETED_ABOVE),
            DiffTag::Delete => {
                let i = (new.start - 1).min(marks.len() - 1);
                if marks[i] == 0 {
                    marks[i] = MARK_DELETED_BELOW;
                }
            }
            _ => {}
        }
    }
    marks
}

/// The index version of `path`, reusing `cached` while the index is unchanged.
fn index_base(path: &Path, cached: Base) -> Base {
    let stamp = crate::git::index_stamp(path)?;
    match cached {
        Some((t, text)) if t == stamp => Some((t, text)),
        _ => crate::git::index_content(path).map(|t| (stamp, t.into())),
    }
}

/// Read + highlight (+ diff against `old`, + git markers), on a worker thread.
fn load(path: &Path, old: Option<Arc<str>>, theme_id: &str, brackets: &BracketColors, base: Base) -> Loaded {
    let (content, lossy) = match read(path) {
        Content::Lossy(t) => (Content::Text(t), true),
        c => (c, false),
    };
    let Content::Text(text) = &content else {
        return Loaded { content, lossy, hl: None, markers: Vec::new(), base, ops: Vec::new() };
    };
    let base = index_base(path, base);
    let markers = base.as_ref().map(|(_, b)| line_markers(b, text)).unwrap_or_default();
    let ops = match old {
        Some(old) if *old != **text => {
            let a: Vec<&str> = old.split_inclusive('\n').collect();
            let b: Vec<&str> = text.split_inclusive('\n').collect();
            let deadline = Instant::now() + Duration::from_millis(150);
            similar::capture_diff_slices_deadline(Algorithm::Myers, &a, &b, Some(deadline))
                .into_iter()
                .map(|op| op.as_tag_tuple())
                .filter(|(tag, _, _)| *tag != DiffTag::Equal)
                .collect()
        }
        _ => Vec::new(),
    };
    let hl = highlight::highlight(text, path, theme_id, brackets);
    Loaded { content, lossy, hl: Some(hl), markers, base, ops }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_task_boxes() {
        let text = "# Plan\n- [ ] a\n  * [x] b\n12. [X] c\n[ ] d\n- [ ]\n- [y] no\n-[ ] no\nsee [ ] no\n- [ ]x no";
        assert_eq!(task_boxes(text), vec![(1, 2), (2, 4), (3, 4), (4, 0), (5, 2)]);
    }

    #[test]
    fn crlf_files_stay_crlf() {
        assert_eq!(with_file_eol("a\r\nb\r\n", "a\r\nX\nb\r\n"), "a\r\nX\r\nb\r\n");
        assert_eq!(with_file_eol("a\r\nb\r\n", "a\r\nb\r\n"), "a\r\nb\r\n");
        // LF files, mixed files and files with no newline are written as they are.
        assert_eq!(with_file_eol("a\nb\n", "a\nX\n"), "a\nX\n");
        assert_eq!(with_file_eol("a\r\nb\n", "a\nX\n"), "a\nX\n");
        assert_eq!(with_file_eol("KEY=", "KEY=1\n"), "KEY=1\n");
    }

    #[test]
    fn write_keeps_permissions_and_links() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = std::env::temp_dir().join(format!("vibey-write-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(".env");
        std::fs::write(&file, "OLD=1\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_file(&file, b"NEW=2\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "NEW=2\n");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);

        // Through a symlink: the link stays a link, the target changes.
        let link = dir.join("link.env");
        symlink(&file, &link).unwrap();
        write_file(&link, b"VIA=link\n").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "VIA=link\n");

        // A new file; no temp files are left behind.
        write_file(&dir.join("fresh.txt"), b"x").unwrap();
        let mut names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
        names.sort();
        assert_eq!(names, [".env", "fresh.txt", "link.env"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn markers_like_vscode() {
        let base = "a\nb\nc\nd\n";
        let text = "a\nB\nc\nnew\n"; // b modified, d deleted then a line added in its place
        let m = line_markers(base, text);
        assert_eq!(m[0], 0);
        assert_eq!(m[1], MARK_MODIFIED);
        assert_eq!(m[2], 0);
        assert_eq!(m[3], MARK_MODIFIED);
        let m = line_markers("a\nb\n", "a\nb\nc\n");
        assert_eq!(m, vec![0, 0, MARK_ADDED]);
        let m = line_markers("a\nb\nc\n", "a\nc\n");
        assert_eq!(m, vec![MARK_DELETED_BELOW, 0]);
        let m = line_markers("x\na\n", "a\n");
        assert_eq!(m, vec![MARK_DELETED_ABOVE]);
    }
}

#[derive(Default)]
struct Find {
    query: String,
    case_sensitive: bool,
    /// Char offsets of matches.
    matches: Vec<(i32, i32)>,
    current: Option<usize>,
}

pub struct CodeView {
    pub widget: gtk::ScrolledWindow,
    view: gtk::TextView,
    gutter: Gutter,
    style: SharedStyle,
    path: RefCell<PathBuf>,
    text: RefCell<Option<Arc<str>>>,
    language: RefCell<String>,
    deleted: Cell<bool>,
    load_id: Cell<u64>,
    tags: RefCell<HashMap<SpanStyle, gtk::TextTag>>,
    changed_tag: gtk::TextTag,
    find_tag: gtk::TextTag,
    find_current_tag: gtk::TextTag,
    task_tag: gtk::TextTag,
    /// Markdown task boxes in the shown text: (line, byte column).
    tasks: RefCell<Vec<(usize, usize)>>,
    over_task: Cell<bool>,
    find: RefCell<Find>,
    fade: RefCell<Option<glib::SourceId>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    reload_pending: Cell<bool>,
    notify: RefCell<Option<Rc<dyn Fn()>>>,
    on_reload: RefCell<Option<Rc<dyn Fn()>>>,
    base: RefCell<Base>,
    /// (0-based line, byte column, byte length) to select once the text is loaded.
    pending_reveal: Cell<Option<(i32, i32, i32)>>,
    /// The buffer has edits that aren't on disk.
    dirty: Cell<bool>,
    /// The file changed on disk while there were unsaved edits.
    conflict: Cell<bool>,
    /// The file can't be edited here (binary, not UTF-8, a notice is showing, …).
    read_only: Cell<bool>,
    /// Vibey itself is replacing the buffer's text (load, reload); not a user edit.
    applying: Cell<bool>,
    /// Bumped on every user edit; results computed for older text are dropped.
    edit_gen: Cell<u64>,
    /// Pending re-highlight after typing stops.
    settle: RefCell<Option<glib::SourceId>>,
}

/// Save when focus leaves a file or the window (see `config.auto_save`).
static AUTO_SAVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_auto_save(on: bool) {
    AUTO_SAVE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn auto_save() -> bool {
    AUTO_SAVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// How a save ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
    Done,
    /// Nothing to write: the buffer already matches the file.
    Unchanged,
    /// The file on disk changed since the edits began; nothing was written.
    Conflict,
    /// Not editable here.
    ReadOnly,
    Failed(String),
}

impl CodeView {
    pub fn new(style: SharedStyle) -> Rc<CodeView> {
        let view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::None)
            .build();
        view.add_css_class("code");
        let gutter: Gutter = glib::Object::new();
        gutter.imp().view.set(Some(&view));
        view.set_gutter(gtk::TextWindowType::Left, Some(&gutter));

        let widget = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hexpand(true).build();
        let g = gutter.clone();
        widget.vadjustment().connect_value_changed(move |_| g.queue_draw());

        let table = view.buffer().tag_table();
        let changed_tag = gtk::TextTag::new(None);
        let find_tag = gtk::TextTag::new(None);
        let find_current_tag = gtk::TextTag::new(None);
        let task_tag = gtk::TextTag::new(None);
        task_tag.set_weight(700);
        for t in [&changed_tag, &find_tag, &find_current_tag, &task_tag] {
            table.add(t);
        }

        let cv = Rc::new(CodeView {
            widget,
            view,
            gutter,
            style,
            path: RefCell::new(PathBuf::new()),
            text: RefCell::new(None),
            language: RefCell::new(String::new()),
            deleted: Cell::new(false),
            load_id: Cell::new(0),
            tags: RefCell::new(HashMap::new()),
            changed_tag,
            find_tag,
            find_current_tag,
            task_tag,
            tasks: RefCell::new(Vec::new()),
            over_task: Cell::new(false),
            find: RefCell::new(Find::default()),
            fade: RefCell::new(None),
            monitor: RefCell::new(None),
            reload_pending: Cell::new(false),
            notify: RefCell::new(None),
            on_reload: RefCell::new(None),
            base: RefCell::new(None),
            pending_reveal: Cell::new(None),
            dirty: Cell::new(false),
            conflict: Cell::new(false),
            read_only: Cell::new(true),
            applying: Cell::new(false),
            edit_gen: Cell::new(0),
            settle: RefCell::new(None),
        });
        cv.apply_colors();
        let weak = Rc::downgrade(&cv);
        cv.view.connect_realize(move |_| {
            if let Some(cv) = weak.upgrade() {
                cv.apply_metrics();
            }
        });
        let weak = Rc::downgrade(&cv);
        cv.view.buffer().connect_cursor_position_notify(move |_| {
            if let Some(cv) = weak.upgrade() {
                cv.emit();
            }
        });
        let weak = Rc::downgrade(&cv);
        cv.view.buffer().connect_changed(move |_| {
            if let Some(cv) = weak.upgrade() {
                cv.on_user_edit();
            }
        });
        // Leaving the view (another tab, the terminal, a different window) saves.
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(&cv);
        focus.connect_leave(move |_| {
            if let Some(cv) = weak.upgrade() {
                cv.autosave();
            }
        });
        cv.view.add_controller(focus);
        cv.install_task_clicks();
        cv
    }

    // ---- editing ----

    /// Run `f` with the buffer marked as being changed by Vibey, not the user. The undo
    /// history is dropped: undoing past a reload would resurrect text the file no longer has.
    fn programmatic<R>(&self, f: impl FnOnce(&gtk::TextBuffer) -> R) -> R {
        let buffer = self.view.buffer();
        self.applying.set(true);
        buffer.begin_irreversible_action();
        let r = f(&buffer);
        buffer.end_irreversible_action();
        self.applying.set(false);
        r
    }

    fn buffer_text(&self) -> String {
        let b = self.view.buffer();
        b.text(&b.start_iter(), &b.end_iter(), false).to_string()
    }

    /// The text as it would be written to the file.
    fn rendered(&self, buffer_text: &str) -> String {
        match self.text.borrow().as_deref() {
            Some(disk) => with_file_eol(disk, buffer_text),
            None => buffer_text.to_string(),
        }
    }

    /// What the file should look like now: the buffer, which may hold unsaved edits.
    fn current_text(&self) -> Option<Arc<str>> {
        if self.dirty.get() {
            Some(self.rendered(&self.buffer_text()).into())
        } else {
            self.text.borrow().clone()
        }
    }

    fn on_user_edit(self: &Rc<Self>) {
        if self.applying.get() || self.read_only.get() {
            return;
        }
        self.dirty.set(true);
        self.edit_gen.set(self.edit_gen.get() + 1);
        // Re-highlight and re-diff once typing pauses.
        if let Some(id) = self.settle.borrow_mut().take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_millis(350), move || {
            if let Some(cv) = weak.upgrade() {
                cv.settle.borrow_mut().take();
                cv.refresh_derived();
            }
        });
        *self.settle.borrow_mut() = Some(id);
        self.emit();
    }

    /// Recompute everything derived from the text after edits: is it still different from
    /// the file, syntax colors, git gutter, task boxes, find matches.
    fn refresh_derived(self: &Rc<Self>) {
        let Some(disk) = self.text.borrow().clone() else { return };
        let text = self.buffer_text();
        let rendered = self.rendered(&text);
        let was_dirty = self.dirty.replace(*disk != *rendered);
        let generation = self.edit_gen.get();
        let (path, base) = (self.path(), self.base.borrow().clone());
        let (theme_id, brackets) = {
            let s = self.style.borrow();
            (s.theme_id.clone(), s.brackets.clone())
        };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let text2 = text.clone();
            let res = gio::spawn_blocking(move || {
                let markers = base.as_ref().map(|(_, b)| line_markers(b, &text2)).unwrap_or_default();
                let hl = highlight::highlight(&text2, &path, &theme_id, &brackets);
                (markers, hl)
            })
            .await;
            if this.edit_gen.get() != generation || this.applying.get() {
                return;
            }
            if let Ok((markers, hl)) = res {
                *this.gutter.imp().markers.borrow_mut() = markers;
                this.gutter.queue_draw();
                this.mark_tasks(&text);
                this.apply_highlight(hl);
                let (q, case) = {
                    let f = this.find.borrow();
                    (f.query.clone(), f.case_sensitive)
                };
                if !q.is_empty() {
                    this.find(&q, case);
                }
                this.apply_metrics();
            }
        });
        if was_dirty != self.dirty.get() {
            self.emit();
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    /// The file changed on disk while there were unsaved edits.
    pub fn has_conflict(&self) -> bool {
        self.conflict.get()
    }

    /// Write the buffer to the file. Without `overwrite`, a file that changed on disk since
    /// the edits began is left alone and reported as a conflict.
    pub fn save(self: &Rc<Self>, overwrite: bool) -> Saved {
        let Some(disk) = self.text.borrow().clone() else { return Saved::ReadOnly };
        if self.read_only.get() {
            return Saved::ReadOnly;
        }
        let rendered = self.rendered(&self.buffer_text());
        if rendered == *disk {
            // Edited back to what the file has.
            if self.dirty.replace(false) {
                self.emit();
            }
            return Saved::Unchanged;
        }
        let path = self.path();
        if !overwrite {
            match read(&path) {
                Content::Text(d) | Content::Lossy(d) if *d != *disk => {
                    if !self.conflict.replace(true) {
                        self.emit();
                    }
                    return Saved::Conflict;
                }
                // Deleted since: saving recreates it.
                Content::Text(_) | Content::Lossy(_) | Content::Missing => {}
                Content::Binary => return Saved::Failed("the file on disk is now binary".into()),
                Content::TooLarge(_) => return Saved::Failed("the file on disk is now too large".into()),
                Content::Error(e) => return Saved::Failed(e),
            }
        }
        if let Err(e) = write_file(&path, rendered.as_bytes()) {
            return Saved::Failed(e.to_string());
        }
        *self.text.borrow_mut() = Some(rendered.into());
        self.dirty.set(false);
        self.conflict.set(false);
        self.deleted.set(false);
        self.refresh_derived();
        self.emit();
        let f = self.on_reload.borrow().clone();
        if let Some(f) = f {
            f();
        }
        Saved::Done
    }

    /// Save without asking, when `auto_save` is on. Conflicts and failures leave the edits in
    /// place, flagged, for the user to resolve.
    pub fn autosave(self: &Rc<Self>) {
        if self.dirty.get() && auto_save() {
            let _ = self.save(false);
        }
    }

    /// Self-test: insert `text` at the end of 1-based `line`, as typing would.
    pub fn selftest_edit(&self, line: i32, text: &str) {
        if self.read_only.get() {
            eprintln!("selftest: edit refused (read-only)");
            return;
        }
        let buffer = self.view.buffer();
        let mut it = buffer.iter_at_line(line - 1).unwrap_or_else(|| buffer.end_iter());
        if !it.ends_line() {
            it.forward_to_line_end();
        }
        buffer.insert(&mut it, text);
        eprintln!("selftest: edit line {line} dirty={}", self.dirty.get());
    }

    pub fn selftest_undo(&self) {
        let b = self.view.buffer();
        eprintln!("selftest: undo can_undo={} can_redo={}", b.can_undo(), b.can_redo());
        b.undo();
    }

    pub fn selftest_state(&self) -> String {
        format!(
            "dirty={} conflict={} read_only={} editable={}",
            self.dirty.get(),
            self.conflict.get(),
            self.read_only.get(),
            self.view.is_editable()
        )
    }

    /// Click a task box to tick/untick it; the pointer becomes a hand over one.
    fn install_task_clicks(self: &Rc<Self>) {
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        // Capture phase: the text view's own gesture would otherwise start a selection.
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |g, n, x, y| {
            let Some(cv) = weak.upgrade() else { return };
            if let (1, Some(task)) = (n, cv.task_at(x, y)) {
                g.set_state(gtk::EventSequenceState::Claimed);
                cv.toggle_task(task);
            }
        });
        self.view.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            let Some(cv) = weak.upgrade() else { return };
            let over = cv.task_at(x, y).is_some();
            if cv.over_task.replace(over) != over {
                cv.view.set_cursor_from_name(Some(if over { "pointer" } else { "text" }));
            }
        });
        self.view.add_controller(motion);
    }

    /// The task box under widget point (x, y), if any.
    fn task_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        if self.tasks.borrow().is_empty() || self.deleted.get() {
            return None;
        }
        let (bx, by) = self.view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let (iter, _) = self.view.iter_at_position(bx, by)?;
        // Past the end of a line this is still the last character; require the point on it.
        let r = self.view.iter_location(&iter);
        if bx < r.x() || bx >= r.x() + r.width() || !iter.has_tag(&self.task_tag) {
            return None;
        }
        let (line, index) = (iter.line() as usize, iter.line_index() as usize);
        self.tasks.borrow().iter().copied().find(|&(l, c)| l == line && (c..c + 3).contains(&index))
    }

    /// Flip `[ ]` ↔ `[x]` in the file on disk; the file monitor then reloads the view. Only if
    /// that line on disk is still what's shown (the AI may have just rewritten the file).
    fn toggle_task(&self, (line, col): (usize, usize)) {
        if self.dirty.get() {
            // Unsaved edits: what's on disk isn't what's shown, so the tick is one more edit.
            let buffer = self.view.buffer();
            let at = |c: usize| buffer.iter_at_line_index(line as i32, c as i32);
            let (Some(open), Some(mut a), Some(mut b)) = (at(col), at(col + 1), at(col + 2)) else { return };
            let mut close = b;
            close.forward_char();
            let shown = buffer.text(&open, &close, false);
            if !matches!(shown.as_str(), "[ ]" | "[x]" | "[X]") {
                return;
            }
            let ticked = if shown == "[ ]" { "x" } else { " " };
            buffer.begin_user_action();
            buffer.delete(&mut a, &mut b);
            buffer.insert(&mut a, ticked);
            buffer.end_user_action();
            return;
        }
        let path = self.path();
        let Some(shown) = self.text.borrow().as_deref().and_then(|t| t.split('\n').nth(line)).map(str::to_owned) else {
            return;
        };
        let Ok(mut bytes) = std::fs::read(&path) else { return };
        let start: usize = bytes.split(|&b| b == b'\n').take(line).map(|l| l.len() + 1).sum();
        if bytes.get(start..).and_then(|rest| rest.split(|&b| b == b'\n').next()) != Some(shown.as_bytes()) {
            return;
        }
        let i = start + col + 1;
        bytes[i] = if bytes[i] == b' ' { b'x' } else { b' ' };
        if let Err(e) = std::fs::write(&path, &bytes) {
            eprintln!("vibey: cannot update {}: {e}", path.display());
        }
    }

    /// Self-test: click the middle of task box `n` (through the same hit test as the mouse).
    pub fn selftest_tick(&self, n: usize) {
        let Some((line, col)) = self.tasks.borrow().get(n).copied() else {
            eprintln!("selftest: no task box {n}");
            return;
        };
        let buffer = self.view.buffer();
        let Some(iter) = buffer.iter_at_line_index(line as i32, col as i32 + 1) else { return };
        let r = self.view.iter_location(&iter);
        let (x, y) = self.view.buffer_to_window_coords(gtk::TextWindowType::Widget, r.x() + r.width() / 2, r.y() + r.height() / 2);
        let hit = self.task_at(x as f64, y as f64);
        eprintln!("selftest: tick {n} at ({x},{y}) hit={hit:?}");
        if let Some(task) = hit {
            self.toggle_task(task);
        }
    }

    fn mark_tasks(&self, text: &str) {
        let buffer = self.view.buffer();
        buffer.remove_tag(&self.task_tag, &buffer.start_iter(), &buffer.end_iter());
        let tasks = if is_markdown(&self.path()) { task_boxes(text) } else { Vec::new() };
        for &(line, col) in &tasks {
            if let (Some(a), Some(b)) =
                (buffer.iter_at_line_index(line as i32, col as i32), buffer.iter_at_line_index(line as i32, col as i32 + 3))
            {
                buffer.apply_tag(&self.task_tag, &a, &b);
            }
        }
        *self.tasks.borrow_mut() = tasks;
    }

    /// Called after (re)loads, cursor moves and deleted-state changes.
    pub fn set_notify(&self, f: Rc<dyn Fn()>) {
        *self.notify.borrow_mut() = Some(f);
    }

    fn emit(&self) {
        let f = self.notify.borrow().clone();
        if let Some(f) = f {
            f();
        }
    }

    /// Select a match and scroll it into view (now, or once the file has loaded).
    pub fn reveal(&self, line: i32, col: i32, len: i32) {
        self.pending_reveal.set(Some((line, col, len)));
        if self.text.borrow().is_some() {
            self.apply_reveal();
        }
    }

    fn apply_reveal(&self) {
        let Some((line, col, len)) = self.pending_reveal.take() else { return };
        let buffer = self.view.buffer();
        let Some(start) = buffer.iter_at_line_index(line, col).or_else(|| buffer.iter_at_line(line)) else { return };
        let end = buffer.iter_at_line_index(line, col + len).unwrap_or(start);
        buffer.select_range(&start, &end);
        let mark = buffer.create_mark(None, &start, true);
        self.view.scroll_to_mark(&mark, 0.0, true, 0.0, 0.3);
        buffer.delete_mark(&mark);
    }

    /// Called after the file changed on disk and was reloaded.
    pub fn set_on_reload(&self, f: Rc<dyn Fn()>) {
        *self.on_reload.borrow_mut() = Some(f);
    }

    pub fn path(&self) -> PathBuf {
        self.path.borrow().clone()
    }

    pub fn language(&self) -> String {
        self.language.borrow().clone()
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted.get()
    }

    /// 1-based (line, column) of the cursor.
    pub fn cursor(&self) -> (i32, i32) {
        let buffer = self.view.buffer();
        let it = buffer.iter_at_mark(&buffer.get_insert());
        (it.line() + 1, it.line_offset() + 1)
    }

    pub fn eol(&self) -> &'static str {
        match self.text.borrow().as_deref() {
            Some(t) if t.contains("\r\n") => "CRLF",
            _ => "LF",
        }
    }

    /// VS Code-style indentation guess for the status bar.
    pub fn indentation(&self) -> String {
        let text = self.text.borrow();
        let Some(t) = text.as_deref() else { return "Spaces: 4".into() };
        let (mut tabs, mut spaces, mut min_spaces) = (0, 0, usize::MAX);
        for line in t.lines().take(500) {
            if line.starts_with('\t') {
                tabs += 1;
            } else if line.starts_with(' ') {
                let n = line.len() - line.trim_start_matches(' ').len();
                if line.trim().is_empty() {
                    continue;
                }
                spaces += 1;
                if n % 2 == 0 {
                    min_spaces = min_spaces.min(n);
                }
            }
        }
        if tabs > spaces {
            "Tab Size: 4".into()
        } else if min_spaces == 2 {
            "Spaces: 2".into()
        } else {
            "Spaces: 4".into()
        }
    }

    pub fn grab_focus(&self) {
        self.view.grab_focus();
    }

    fn apply_colors(&self) {
        let s = self.style.borrow();
        *self.gutter.imp().color.borrow_mut() = Some(s.line_number.gdk());
        *self.gutter.imp().marker_colors.borrow_mut() = vec![s.git_added.gdk(), s.git_modified.gdk(), s.git_deleted.gdk()];
        self.find_tag.set_background_rgba(Some(&s.find_match.gdk()));
        self.find_current_tag.set_background_rgba(Some(&s.find_current.gdk()));
        self.gutter.queue_draw();
    }

    /// Match VS Code's line height (round(14 × 1.35) = 19 px, × UI scale), gutter and tabs.
    fn apply_metrics(&self) {
        let scale = self.style.borrow().scale;
        let ctx = self.view.pango_context();
        let metrics = ctx.metrics(None, None);
        // Measure a real layout: hinting can make lines taller than the font metrics say.
        let natural = self.view.create_pango_layout(Some("Xg")).pixel_size().1;
        let target = (19.0 * scale).round() as i32;
        let extra = (target - natural).max(0);
        self.view.set_pixels_above_lines(extra / 2);
        self.view.set_pixels_below_lines(extra - extra / 2);

        let digit = metrics.approximate_digit_width() as f64 / pango::SCALE as f64;
        let mut tabs = pango::TabArray::new(1, true);
        tabs.set_tab(0, pango::TabAlign::Left, (4.0 * digit).round() as i32);
        self.view.set_tabs(&tabs);

        let lines = self.view.buffer().line_count().max(1);
        let digits = lines.to_string().len().max(5) as f64;
        let gi = self.gutter.imp();
        // VS Code: line numbers (min 5 digits) | decorations + folding (26).
        let numbers = digits * digit;
        gi.number_right.set(numbers.round() as i32);
        // VS Code puts the quick-diff bar just right of the line numbers.
        gi.marker_x.set((numbers + 5.0 * scale) as f32);
        gi.marker_w.set((3.0 * scale) as f32);
        gi.width.set((numbers + 26.0 * scale).round() as i32);
        gi.text_top.set(extra / 2);
        self.gutter.queue_resize();
    }

    /// Open `path` and start watching it.
    pub fn open(self: &Rc<Self>, path: PathBuf) {
        *self.path.borrow_mut() = path.clone();
        *self.text.borrow_mut() = None;
        self.watch(&path);
        self.start_load(false);
    }

    fn watch(self: &Rc<Self>, path: &Path) {
        let Ok(monitor) =
            gio::File::for_path(path).monitor_file(gio::FileMonitorFlags::WATCH_MOVES, None::<&gio::Cancellable>)
        else {
            return;
        };
        monitor.set_rate_limit(50);
        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, _| {
            let Some(cv) = weak.upgrade() else { return };
            // Coalesce bursts (truncate + write + close) into one reload.
            if !cv.reload_pending.replace(true) {
                let weak = Rc::downgrade(&cv);
                glib::timeout_add_local_once(Duration::from_millis(60), move || {
                    if let Some(cv) = weak.upgrade() {
                        cv.reload_pending.set(false);
                        cv.start_load(true);
                    }
                });
            }
        });
        *self.monitor.borrow_mut() = Some(monitor);
    }

    /// Recompute gutter markers (the git index changed: stage, commit, checkout).
    pub fn refresh_git(self: &Rc<Self>) {
        let Some(text) = self.current_text() else { return };
        let path = self.path();
        let cached = self.base.borrow().clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || {
                let base = index_base(&path, cached);
                let markers = base.as_ref().map(|(_, b)| line_markers(b, &text)).unwrap_or_default();
                (base, markers)
            })
            .await;
            if let Ok((base, markers)) = res {
                *this.base.borrow_mut() = base;
                *this.gutter.imp().markers.borrow_mut() = markers;
                this.gutter.queue_draw();
            }
        });
    }

    /// Re-highlight with the current style (after a theme change).
    pub fn restyle(self: &Rc<Self>) {
        let table = self.view.buffer().tag_table();
        for (_, tag) in self.tags.borrow_mut().drain() {
            table.remove(&tag);
        }
        self.apply_colors();
        if self.dirty.get() {
            // Re-reading the file would throw the edits away; color the buffer instead.
            self.refresh_derived();
        } else if self.text.borrow().is_some() {
            self.start_load(false);
        }
    }

    fn start_load(self: &Rc<Self>, is_reload: bool) {
        let id = self.load_id.get() + 1;
        self.load_id.set(id);
        let path = self.path();
        let old = if is_reload { self.text.borrow().clone() } else { None };
        let (theme_id, brackets) = {
            let s = self.style.borrow();
            (s.theme_id.clone(), s.brackets.clone())
        };
        let base = self.base.borrow().clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let loaded = gio::spawn_blocking(move || load(&path, old, &theme_id, &brackets, base))
                .await
                .unwrap_or_else(|_| Loaded {
                    content: Content::Error("loader panicked".into()),
                    lossy: false,
                    hl: None,
                    markers: vec![],
                    base: None,
                    ops: vec![],
                });
            if this.load_id.get() == id {
                this.finish_load(loaded, is_reload);
            }
        });
    }

    fn finish_load(self: &Rc<Self>, loaded: Loaded, is_reload: bool) {
        let buffer = self.view.buffer();
        let was_deleted = self.deleted.replace(matches!(loaded.content, Content::Missing));
        let text = match loaded.content {
            Content::Text(t) => t,
            Content::Missing if is_reload => {
                // Keep showing the last content; the tab shows it as deleted.
                if !was_deleted {
                    self.emit();
                }
                return;
            }
            other => {
                if is_reload && self.dirty.get() {
                    // Keep the edits; saving will say the file changed.
                    self.conflict.set(true);
                    self.emit();
                    return;
                }
                let msg = match other {
                    Content::Binary => "This file is binary and is not displayed.".into(),
                    Content::TooLarge(n) => format!("This file is {:.1} MB, too large to display.", n as f64 / (1 << 20) as f64),
                    Content::Missing => "This file does not exist.".into(),
                    Content::Error(e) => format!("Cannot open file: {e}"),
                    Content::Text(_) | Content::Lossy(_) => unreachable!(),
                };
                *self.text.borrow_mut() = None;
                self.tasks.borrow_mut().clear();
                self.dirty.set(false);
                self.conflict.set(false);
                self.read_only.set(true);
                self.view.set_editable(false);
                self.view.set_cursor_visible(false);
                self.programmatic(|b| b.set_text(&msg));
                self.emit();
                return;
            }
        };
        if is_reload && self.dirty.get() {
            // Unsaved edits are never replaced by a reload. If the file moved on from the
            // version they were made against, saving will ask first.
            if self.text.borrow().as_deref() != Some(&*text) {
                self.conflict.set(true);
            }
            *self.base.borrow_mut() = loaded.base;
            self.emit();
            let f = self.on_reload.borrow().clone();
            if let Some(f) = f {
                f();
            }
            return;
        }
        self.conflict.set(false);
        self.dirty.set(false);
        self.read_only.set(loaded.lossy);
        self.view.set_editable(!loaded.lossy);
        self.view.set_cursor_visible(true);

        *self.base.borrow_mut() = loaded.base;
        *self.gutter.imp().markers.borrow_mut() = loaded.markers;
        self.gutter.queue_draw();
        let old = self.text.replace(Some(text.clone()));
        let unchanged = old.as_deref() == Some(&*text);
        let mut changed_lines = Vec::new();
        if is_reload && old.is_some() && !unchanged {
            changed_lines = self.programmatic(|_| self.apply_diff(&text, &loaded.ops));
        } else if !unchanged || !is_reload {
            self.programmatic(|b| b.set_text(&text));
            if !is_reload {
                buffer.place_cursor(&buffer.start_iter());
            }
        }
        if !(is_reload && unchanged) {
            self.mark_tasks(&text);
        }
        if let Some(hl) = loaded.hl {
            *self.language.borrow_mut() = hl.language.clone();
            if !(is_reload && unchanged) {
                self.apply_highlight(hl);
            }
        }
        if !changed_lines.is_empty() {
            self.flash_changes(&changed_lines);
        }
        if !self.find.borrow().query.is_empty() {
            let (q, case) = {
                let f = self.find.borrow();
                (f.query.clone(), f.case_sensitive)
            };
            self.find(&q, case);
        }
        self.apply_metrics();
        self.apply_reveal();
        self.emit();
        if is_reload {
            let f = self.on_reload.borrow().clone();
            if let Some(f) = f {
                f();
            }
        }
        // Highlighting allocates a lot transiently; give it back to the OS.
        glib::idle_add_local_once(|| unsafe {
            libc::malloc_trim(0);
        });
    }

    /// Edit the buffer in place, bottom-up, and return the new line ranges that changed.
    fn apply_diff(&self, new_text: &str, ops: &[(DiffTag, Range<usize>, Range<usize>)]) -> Vec<Range<usize>> {
        let buffer = self.view.buffer();
        let new_lines: Vec<&str> = new_text.split_inclusive('\n').collect();
        let line_iter = |line: usize| buffer.iter_at_line(line as i32).unwrap_or_else(|| buffer.end_iter());
        for (_, old, new) in ops.iter().rev() {
            let mut start = line_iter(old.start);
            if old.end > old.start {
                let mut end = line_iter(old.end);
                buffer.delete(&mut start, &mut end);
            }
            if new.end > new.start {
                buffer.insert(&mut start, &new_lines[new.clone()].concat());
            }
        }
        if buffer.char_count() as usize != new_text.chars().count() {
            // Should not happen; fall back to replacing everything.
            buffer.set_text(new_text);
            return Vec::new();
        }
        ops.iter().filter(|(_, _, new)| new.end > new.start).map(|(_, _, new)| new.clone()).collect()
    }

    fn flash_changes(self: &Rc<Self>, lines: &[Range<usize>]) {
        let buffer = self.view.buffer();
        if let Some(id) = self.fade.borrow_mut().take() {
            id.remove();
        }
        buffer.remove_tag(&self.changed_tag, &buffer.start_iter(), &buffer.end_iter());
        let base = self.style.borrow().changed;
        self.changed_tag.set_paragraph_background_rgba(Some(&base.gdk()));
        for r in lines {
            let start = buffer.iter_at_line(r.start as i32).unwrap_or_else(|| buffer.end_iter());
            let end = buffer.iter_at_line(r.end as i32).unwrap_or_else(|| buffer.end_iter());
            buffer.apply_tag(&self.changed_tag, &start, &end);
        }
        let started = Instant::now();
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local(Duration::from_millis(40), move || {
            let Some(cv) = weak.upgrade() else { return glib::ControlFlow::Break };
            let t = started.elapsed();
            if t < CHANGE_HOLD {
                return glib::ControlFlow::Continue;
            }
            let k = ((t - CHANGE_HOLD).as_secs_f64() / CHANGE_FADE.as_secs_f64()).min(1.0);
            if k >= 1.0 {
                let b = cv.view.buffer();
                b.remove_tag(&cv.changed_tag, &b.start_iter(), &b.end_iter());
                cv.fade.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            let c = base.gdk();
            let faded = gtk::gdk::RGBA::new(c.red(), c.green(), c.blue(), c.alpha() * (1.0 - k as f32));
            cv.changed_tag.set_paragraph_background_rgba(Some(&faded));
            glib::ControlFlow::Continue
        });
        *self.fade.borrow_mut() = Some(id);
    }

    fn tag_for(&self, style: &SpanStyle) -> gtk::TextTag {
        if let Some(t) = self.tags.borrow().get(style) {
            return t.clone();
        }
        let (r, g, b, a) = style.fg;
        let tag = gtk::TextTag::new(None);
        tag.set_foreground_rgba(Some(&gtk::gdk::RGBA::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        )));
        if style.bold {
            tag.set_weight(700);
        }
        if style.italic {
            tag.set_style(pango::Style::Italic);
        }
        if style.underline {
            tag.set_underline(pango::Underline::Single);
        }
        if style.strikethrough {
            tag.set_strikethrough(true);
        }
        let table = self.view.buffer().tag_table();
        table.add(&tag);
        // Keep find highlights above syntax colors.
        tag.set_priority(0);
        self.tags.borrow_mut().insert(*style, tag.clone());
        tag
    }

    fn apply_highlight(self: &Rc<Self>, hl: Highlighted) {
        let buffer = self.view.buffer();
        for tag in self.tags.borrow().values() {
            buffer.remove_tag(tag, &buffer.start_iter(), &buffer.end_iter());
        }
        let tags: Vec<gtk::TextTag> = hl.styles.iter().map(|s| self.tag_for(s)).collect();
        let spans = hl.spans;
        let id = self.load_id.get();
        let mut next = 0usize;
        let this = self.clone();
        let mut step = move || {
            if this.load_id.get() != id {
                return glib::ControlFlow::Break;
            }
            let buffer = this.view.buffer();
            let end = (next + SPANS_PER_CHUNK).min(spans.len());
            for s in &spans[next..end] {
                let (Some(a), Some(b)) = (
                    buffer.iter_at_line_index(s.line as i32, s.start as i32),
                    buffer.iter_at_line_index(s.line as i32, s.end as i32),
                ) else {
                    continue;
                };
                buffer.apply_tag(&tags[s.style as usize], &a, &b);
            }
            next = end;
            if end >= spans.len() { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        };
        // First chunk now (no flash of uncolored text), the rest when idle.
        if step() == glib::ControlFlow::Continue {
            glib::idle_add_local(step);
        }
    }

    /// Highlight all matches; returns the match count.
    pub fn find(&self, query: &str, case_sensitive: bool) -> usize {
        let buffer = self.view.buffer();
        buffer.remove_tag(&self.find_tag, &buffer.start_iter(), &buffer.end_iter());
        buffer.remove_tag(&self.find_current_tag, &buffer.start_iter(), &buffer.end_iter());
        let mut f = self.find.borrow_mut();
        f.query = query.to_string();
        f.case_sensitive = case_sensitive;
        f.matches.clear();
        f.current = None;
        if query.is_empty() || self.text.borrow().is_none() {
            return 0;
        }
        let mut flags = gtk::TextSearchFlags::TEXT_ONLY;
        if !case_sensitive {
            flags |= gtk::TextSearchFlags::CASE_INSENSITIVE;
        }
        let mut it = buffer.start_iter();
        while let Some((a, b)) = it.forward_search(query, flags, None) {
            buffer.apply_tag(&self.find_tag, &a, &b);
            f.matches.push((a.offset(), b.offset()));
            if f.matches.len() >= MAX_FIND_MATCHES {
                break;
            }
            it = b;
        }
        f.matches.len()
    }

    /// Move to the next/previous match (from the cursor); returns (1-based index, count).
    pub fn find_step(&self, forward: bool) -> (usize, usize) {
        let buffer = self.view.buffer();
        let mut f = self.find.borrow_mut();
        let n = f.matches.len();
        if n == 0 {
            return (0, 0);
        }
        let next = match f.current {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => {
                let cursor = buffer.iter_at_mark(&buffer.get_insert()).offset();
                let first_after = f.matches.iter().position(|(a, _)| *a >= cursor).unwrap_or(0);
                if forward { first_after } else { (first_after + n - 1) % n }
            }
        };
        if let Some(i) = f.current {
            let (a, b) = f.matches[i];
            buffer.remove_tag(&self.find_current_tag, &buffer.iter_at_offset(a), &buffer.iter_at_offset(b));
        }
        let (a, b) = f.matches[next];
        let (ia, ib) = (buffer.iter_at_offset(a), buffer.iter_at_offset(b));
        buffer.apply_tag(&self.find_current_tag, &ia, &ib);
        buffer.place_cursor(&ia);
        let mark = buffer.create_mark(None, &ia, true);
        self.view.scroll_to_mark(&mark, 0.15, false, 0.0, 0.0);
        buffer.delete_mark(&mark);
        f.current = Some(next);
        (next + 1, n)
    }

    pub fn clear_find(&self) {
        self.find("", false);
    }
}
