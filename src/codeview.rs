//! Read-only code view: GtkTextView + a custom gutter (line numbers; git bars later).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib, graphene, pango};
use syntect::highlighting::Theme;

use crate::highlight::{self, BracketColors, Highlighted, SpanStyle};

/// Larger files get a notice instead of their contents.
const MAX_OPEN_BYTES: u64 = 20 << 20;
/// Spans applied per idle callback, so huge files don't block the UI.
const SPANS_PER_CHUNK: usize = 20_000;

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

pub enum Loaded {
    Text { text: String, hl: Highlighted },
    Binary,
    TooLarge(u64),
    Error(String),
}

fn load_file(path: &Path, theme: &Theme, brackets: &BracketColors) -> Loaded {
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => return Loaded::Error(e.to_string()),
    };
    if size > MAX_OPEN_BYTES {
        return Loaded::TooLarge(size);
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return Loaded::Error(e.to_string()),
    };
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Loaded::Binary;
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let hl = highlight::highlight(&text, path, theme, brackets);
    Loaded::Text { text, hl }
}

pub struct CodeView {
    pub widget: gtk::ScrolledWindow,
    view: gtk::TextView,
    gutter: Gutter,
    theme: Arc<Theme>,
    brackets: Arc<BracketColors>,
    tags: RefCell<HashMap<SpanStyle, gtk::TextTag>>,
    generation: Cell<u64>,
    scale: f64,
}

impl CodeView {
    pub fn new(
        theme: Arc<Theme>,
        brackets: Arc<BracketColors>,
        model: &crate::theme::ThemeModel,
        scale: f64,
    ) -> Rc<CodeView> {
        let view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::None)
            .left_margin(0)
            .build();
        view.add_css_class("code");
        let gutter: Gutter = glib::Object::new();
        gutter.imp().view.set(Some(&view));
        *gutter.imp().color.borrow_mut() = model.color("editorLineNumber.foreground").map(|c| c.gdk());
        view.set_gutter(gtk::TextWindowType::Left, Some(&gutter));

        let widget = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hexpand(true).build();
        let g = gutter.clone();
        widget.vadjustment().connect_value_changed(move |_| g.queue_draw());

        let cv = Rc::new(CodeView {
            widget,
            view,
            gutter,
            theme,
            brackets,
            tags: RefCell::new(HashMap::new()),
            generation: Cell::new(0),
            scale,
        });
        let weak = Rc::downgrade(&cv);
        cv.view.connect_realize(move |_| {
            if let Some(cv) = weak.upgrade() {
                cv.apply_metrics();
            }
        });
        cv
    }

    /// Match VS Code's line height (round(14 × 1.35) = 19 px, × UI scale) and gutter layout.
    fn apply_metrics(&self) {
        let ctx = self.view.pango_context();
        let metrics = ctx.metrics(None, None);
        // Measure a real layout: hinting can make lines taller than the font metrics say.
        let natural = self.view.create_pango_layout(Some("Xg")).pixel_size().1;
        let target = (19.0 * self.scale).round() as i32;
        let extra = (target - natural).max(0);
        self.view.set_pixels_above_lines(extra / 2);
        self.view.set_pixels_below_lines(extra - extra / 2);

        let digit = metrics.approximate_digit_width() as f64 / pango::SCALE as f64;
        let lines = self.view.buffer().line_count().max(1);
        let digits = lines.to_string().len().max(5) as f64;
        let gi = self.gutter.imp();
        // VS Code: line numbers (min 5 digits) | decorations + folding (26). The glyph
        // margin has no width while nothing is drawn in it.
        let left = 0.0;
        let numbers = digits * digit;
        let deco = 26.0 * self.scale;
        gi.number_right.set((left + numbers).round() as i32);
        gi.width.set((left + numbers + deco).round() as i32);
        gi.text_top.set(extra / 2);
        self.gutter.queue_resize();
    }

    pub fn load(self: &Rc<Self>, path: PathBuf, on_loaded: impl Fn(&str) + 'static) {
        let load_id = self.generation.get() + 1;
        self.generation.set(load_id);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let (theme, brackets) = (this.theme.clone(), this.brackets.clone());
            let p = path.clone();
            let loaded = gio::spawn_blocking(move || load_file(&p, &theme, &brackets))
                .await
                .unwrap_or_else(|_| Loaded::Error("loader panicked".into()));
            if this.generation.get() != load_id {
                return;
            }
            let buffer = this.view.buffer();
            match loaded {
                Loaded::Text { text, hl } => {
                    buffer.set_text(&text);
                    this.apply_metrics();
                    on_loaded(&hl.syntax_name);
                    this.apply_highlight(hl, load_id);
                }
                Loaded::Binary => buffer.set_text("This file is binary and is not displayed."),
                Loaded::TooLarge(n) => buffer.set_text(&format!(
                    "This file is {:.1} MB, too large to display.",
                    n as f64 / (1 << 20) as f64
                )),
                Loaded::Error(e) => buffer.set_text(&format!("Cannot open file: {e}")),
            }
        });
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
        self.view.buffer().tag_table().add(&tag);
        self.tags.borrow_mut().insert(*style, tag.clone());
        tag
    }

    fn apply_highlight(self: &Rc<Self>, hl: Highlighted, load_id: u64) {
        let tags: Vec<gtk::TextTag> = hl.styles.iter().map(|s| self.tag_for(s)).collect();
        let spans = Rc::new(hl.spans);
        let next = Cell::new(0usize);
        let this = self.clone();
        glib::idle_add_local(move || {
            if this.generation.get() != load_id {
                return glib::ControlFlow::Break;
            }
            let buffer = this.view.buffer();
            let start = next.get();
            let end = (start + SPANS_PER_CHUNK).min(spans.len());
            for s in &spans[start..end] {
                let (Some(a), Some(b)) = (
                    buffer.iter_at_line_index(s.line as i32, s.start as i32),
                    buffer.iter_at_line_index(s.line as i32, s.end as i32),
                ) else {
                    continue;
                };
                buffer.apply_tag(&tags[s.style as usize], &a, &b);
            }
            next.set(end);
            if end >= spans.len() {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}
