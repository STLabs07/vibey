//! Embedded terminals (VTE on a real PTY).

use std::path::Path;

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use vte4::prelude::*;

use crate::theme::ThemeModel;

pub fn new_terminal(cwd: &Path, theme: &ThemeModel, scale: f64) -> vte4::Terminal {
    let term = vte4::Terminal::new();
    term.set_hexpand(true);
    term.set_vexpand(true);

    // VS Code: editor font family, 14 px, scaled like the rest of the UI.
    let mut font = pango::FontDescription::from_string("Droid Sans Mono,monospace");
    font.set_absolute_size(14.0 * scale * pango::SCALE as f64);
    term.set_font(Some(&font));

    let (fg, bg, palette) = theme.terminal_colors();
    let palette: Vec<gtk::gdk::RGBA> = palette.iter().map(|c| c.gdk()).collect();
    let refs: Vec<&gtk::gdk::RGBA> = palette.iter().collect();
    term.set_colors(Some(&fg.gdk()), Some(&bg.gdk()), &refs);
    if let Some(c) = theme.color("terminalCursor.foreground") {
        term.set_color_cursor(Some(&c.gdk()));
    }
    term.set_bold_is_bright(false);
    term.set_scrollback_lines(10_000);
    term.set_mouse_autohide(true);

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    term.spawn_async(
        vte4::PtyFlags::DEFAULT,
        cwd.to_str(),
        &[&shell],
        &[],
        glib::SpawnFlags::DEFAULT,
        || {},
        -1,
        None::<&gio::Cancellable>,
        |res| {
            if let Err(e) = res {
                eprintln!("vibey: failed to spawn shell: {e}");
            }
        },
    );
    term
}
