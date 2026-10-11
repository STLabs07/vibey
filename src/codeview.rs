//! Read-only code view: GtkTextView + a custom gutter (line numbers; git bars later).
//!
//! Live reload: a GIO file monitor triggers a re-read; the new text is diffed against the
//! old off the UI thread, the buffer is edited in place (so the scroll position stays), and
//! changed lines are highlighted briefly, then fade out.

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
    pub scale: f64,
}

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
            loop {
                let (y, _h) = view.line_yrange(&iter);
                if y > bottom {
                    break;
                }
                let (_, wy) = view.buffer_to_window_coords(gtk::TextWindowType::Left, 0, y);
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
    Binary,
    TooLarge(u64),
    Missing,
    Error(String),
}

struct Loaded {
    content: Content,
    hl: Option<Highlighted>,
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
        Ok(b) if b[..b.len().min(8192)].contains(&0) => Content::Binary,
        Ok(b) => Content::Text(String::from_utf8_lossy(&b).into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Content::Missing,
        Err(e) => Content::Error(e.to_string()),
    }
}

/// Read + highlight (+ diff against `old`), on a worker thread.
fn load(path: &Path, old: Option<Arc<str>>, theme_id: &str, brackets: &BracketColors) -> Loaded {
    let content = read(path);
    let Content::Text(text) = &content else { return Loaded { content, hl: None, ops: Vec::new() } };
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
    Loaded { content, hl: Some(hl), ops }
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
    find: RefCell<Find>,
    fade: RefCell<Option<glib::SourceId>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    reload_pending: Cell<bool>,
    notify: RefCell<Option<Rc<dyn Fn()>>>,
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
        for t in [&changed_tag, &find_tag, &find_current_tag] {
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
            find: RefCell::new(Find::default()),
            fade: RefCell::new(None),
            monitor: RefCell::new(None),
            reload_pending: Cell::new(false),
            notify: RefCell::new(None),
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
        cv
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

    /// Re-highlight with the current style (after a theme change).
    pub fn restyle(self: &Rc<Self>) {
        let table = self.view.buffer().tag_table();
        for (_, tag) in self.tags.borrow_mut().drain() {
            table.remove(&tag);
        }
        self.apply_colors();
        if self.text.borrow().is_some() {
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
        let this = self.clone();
        glib::spawn_future_local(async move {
            let loaded = gio::spawn_blocking(move || load(&path, old, &theme_id, &brackets))
                .await
                .unwrap_or_else(|_| Loaded { content: Content::Error("loader panicked".into()), hl: None, ops: vec![] });
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
                let msg = match other {
                    Content::Binary => "This file is binary and is not displayed.".into(),
                    Content::TooLarge(n) => format!("This file is {:.1} MB, too large to display.", n as f64 / (1 << 20) as f64),
                    Content::Missing => "This file does not exist.".into(),
                    Content::Error(e) => format!("Cannot open file: {e}"),
                    Content::Text(_) => unreachable!(),
                };
                *self.text.borrow_mut() = None;
                buffer.set_text(&msg);
                self.emit();
                return;
            }
        };

        let old = self.text.replace(Some(text.clone()));
        let unchanged = old.as_deref() == Some(&*text);
        let mut changed_lines = Vec::new();
        if is_reload && old.is_some() && !unchanged {
            changed_lines = self.apply_diff(&text, &loaded.ops);
        } else if !unchanged || !is_reload {
            buffer.set_text(&text);
            if !is_reload {
                buffer.place_cursor(&buffer.start_iter());
            }
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
        self.emit();
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
