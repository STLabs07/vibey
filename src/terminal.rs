//! A project's terminal panel: VTE terminals on real PTYs, with tabs named after their
//! foreground process and a status per terminal.
//!
//! Status is event-driven (no polling): VTE's `contents-changed` marks a terminal busy and
//! arms one quiet-timer; when output stops for `QUIET`, it's idle again. A busy stretch of at
//! least `SUSTAINED` (an AI CLI working, a long build) that ends while the user isn't
//! looking at that terminal leaves it "finished, unseen". So does a bell, or a long-running
//! foreground job exiting.

use std::cell::{Cell, RefCell};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use vte4::prelude::*;

use crate::icons::codicon;
use crate::menu::{self, ContextMenu};
use crate::procmon::process_name;
use crate::theme::ThemeModel;

/// No output for this long → idle.
const QUIET: Duration = Duration::from_millis(1500);
/// Busy for at least this long before going quiet → worth telling the user.
const SUSTAINED: Duration = Duration::from_secs(3);
/// Re-read the foreground process at most this often while output streams.
const JOB_CHECK: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Idle,
    Running,
    /// Finished (or rang the bell) while the user was looking elsewhere.
    Attention,
}

struct Term {
    term: vte4::Terminal,
    chip: gtk::Box,
    dot: gtk::Label,
    num: gtk::Label,
    name: gtk::Label,
    shell_pid: Cell<i32>,
    shell_name: String,
    job: RefCell<Option<String>>,
    job_since: Cell<Option<Instant>>,
    job_checked: Cell<Option<Instant>>,
    state: Cell<State>,
    last_output: Cell<Instant>,
    busy_since: Cell<Option<Instant>>,
    timer_armed: Cell<bool>,
}

impl Term {
    fn foreground_pgid(&self) -> Option<i32> {
        let pty = self.term.pty()?;
        let pgid = unsafe { libc::tcgetpgrp(pty.fd().as_raw_fd()) };
        let shell = self.shell_pid.get();
        (pgid > 0 && shell > 0 && pgid != shell).then_some(pgid)
    }

    fn label(&self) -> String {
        self.job.borrow().clone().unwrap_or_else(|| self.shell_name.clone())
    }
}

const CLI_CLASSES: [&str; 6] = ["cli-claude", "cli-codex", "cli-gemini", "cli-ai", "cli-proc", "cli-shell"];

/// Color class for a terminal by what runs in it: Claude orange, Codex green, Gemini blue,
/// other AI CLIs purple, other programs neutral-accent, plain shells gray.
pub fn cli_class(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.starts_with("claude") {
        "cli-claude"
    } else if n.starts_with("codex") {
        "cli-codex"
    } else if n.starts_with("gemini") {
        "cli-gemini"
    } else if ["aider", "opencode", "cursor", "amp", "goose", "crush", "qwen", "copilot", "cline", "kilo", "droid", "llm", "ollama"]
        .iter()
        .any(|a| n.starts_with(a))
    {
        "cli-ai"
    } else if ["bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "nu"].contains(&n.as_str()) {
        "cli-shell"
    } else {
        "cli-proc"
    }
}

/// Panel-level events for the window (rail badges, notifications).
pub enum Event {
    Changed,
    /// A terminal needs attention: (process name).
    Finished(String),
}

pub struct TerminalPanel {
    pub widget: gtk::Box,
    cwd: PathBuf,
    header_tabs: gtk::Box,
    stack: gtk::Stack,
    terms: RefCell<Vec<Rc<Term>>>,
    active: Cell<usize>,
    /// The panel's project is on screen and the window is focused.
    visible: Cell<bool>,
    scale: f64,
    on_event: RefCell<Option<Rc<dyn Fn(Event)>>>,
    /// Working directories to reopen terminals in (session restore).
    restore: RefCell<Vec<PathBuf>>,
    me: RefCell<Weak<TerminalPanel>>,
}

fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
}

impl TerminalPanel {
    pub fn new(cwd: &Path, scale: f64) -> Rc<TerminalPanel> {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("panel-header");
        let title = gtk::Label::new(Some("Terminal"));
        title.add_css_class("panel-tab");
        title.add_css_class("active");
        header.append(&title);
        // Numbered terminal tabs sit right next to the "Terminal" label.
        let header_tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header_tabs.add_css_class("term-tabs");
        let tabs_scroll = gtk::ScrolledWindow::builder()
            .child(&header_tabs)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::External)
            .propagate_natural_width(true)
            .build();
        header.append(&tabs_scroll);
        let add = gtk::Button::with_label(codicon::ADD);
        add.add_css_class("icon-button");
        add.add_css_class("codicon");
        add.set_tooltip_text(Some("New Terminal (Ctrl+Shift+`)"));
        add.set_focus_on_click(false);
        header.append(&add);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        header.append(&spacer);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        let empty = gtk::Label::new(Some("No terminals. Press Ctrl+Shift+` to open one."));
        empty.add_css_class("dim");
        stack.add_named(&empty, Some("empty"));

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("panel");
        widget.append(&header);
        widget.append(&stack);

        let panel = Rc::new(TerminalPanel {
            widget,
            cwd: cwd.to_path_buf(),
            header_tabs,
            stack,
            terms: RefCell::new(Vec::new()),
            active: Cell::new(0),
            visible: Cell::new(false),
            scale,
            on_event: RefCell::new(None),
            restore: RefCell::new(Vec::new()),
            me: RefCell::new(Weak::new()),
        });
        *panel.me.borrow_mut() = Rc::downgrade(&panel);
        let weak = Rc::downgrade(&panel);
        add.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.add_terminal(&crate::app::current_theme(), None);
            }
        });
        panel
    }

    pub fn set_on_event(&self, f: Rc<dyn Fn(Event)>) {
        *self.on_event.borrow_mut() = Some(f);
    }

    fn emit(&self, e: Event) {
        let f = self.on_event.borrow().clone();
        if let Some(f) = f {
            f(e);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.borrow().is_empty()
    }

    /// Terminals to reopen (in these directories) when the panel first starts.
    pub fn set_restore(&self, cwds: Vec<PathBuf>) {
        *self.restore.borrow_mut() = cwds;
    }

    /// Open the panel's initial terminals: the restored ones, or one in the project root.
    pub fn start(&self, theme: &ThemeModel) {
        let cwds = std::mem::take(&mut *self.restore.borrow_mut());
        if cwds.is_empty() {
            self.add_terminal(theme, None);
        }
        for cwd in cwds {
            self.add_terminal(theme, Some(&cwd));
        }
        if !self.is_empty() {
            self.activate(0);
        }
    }

    /// Restore list not yet used (project never activated this session).
    pub fn pending_restore(&self) -> Vec<PathBuf> {
        self.restore.borrow().clone()
    }

    /// Current working directory of each terminal's shell.
    pub fn cwds(&self) -> Vec<PathBuf> {
        self.terms
            .borrow()
            .iter()
            .map(|t| std::fs::read_link(format!("/proc/{}/cwd", t.shell_pid.get())).unwrap_or_else(|_| self.cwd.clone()))
            .collect()
    }

    pub fn add_terminal(&self, theme: &ThemeModel, cwd: Option<&Path>) {
        let term = vte4::Terminal::new();
        term.set_hexpand(true);
        term.set_vexpand(true);
        // VS Code: editor font family, 14 px, scaled like the rest of the UI.
        let mut font = pango::FontDescription::from_string("Droid Sans Mono,monospace");
        font.set_absolute_size(14.0 * self.scale * pango::SCALE as f64);
        term.set_font(Some(&font));
        term.set_bold_is_bright(false);
        term.set_scrollback_lines(10_000);
        term.set_mouse_autohide(true);
        apply_colors(&term, theme);

        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        chip.add_css_class("term-tab");
        let dot = gtk::Label::new(None);
        dot.add_css_class("status-dot");
        let num = gtk::Label::new(None);
        num.add_css_class("term-num");
        let shell_name = shell().rsplit('/').next().unwrap_or("sh").to_string();
        let name = gtk::Label::new(Some(&shell_name));
        let close = gtk::Label::new(Some(codicon::CLOSE));
        close.add_css_class("codicon");
        close.add_css_class("close");
        close.set_tooltip_text(Some("Kill Terminal"));
        chip.add_css_class(cli_class(&shell_name));
        chip.append(&num);
        chip.append(&name);
        chip.append(&dot);
        chip.append(&close);
        self.header_tabs.append(&chip);
        self.stack.add_child(&term);

        let t = Rc::new(Term {
            term: term.clone(),
            chip: chip.clone(),
            dot,
            num,
            name,
            shell_pid: Cell::new(0),
            shell_name,
            job: RefCell::new(None),
            job_since: Cell::new(None),
            job_checked: Cell::new(None),
            state: Cell::new(State::Idle),
            last_output: Cell::new(Instant::now()),
            busy_since: Cell::new(None),
            timer_armed: Cell::new(false),
        });
        self.terms.borrow_mut().push(t.clone());

        let click = gtk::GestureClick::new();
        let (me, wt) = (self.me.borrow().clone(), Rc::downgrade(&t));
        let close_w = close.clone();
        click.connect_pressed(move |_, _, x, y| {
            let (Some(p), Some(t)) = (me.upgrade(), wt.upgrade()) else { return };
            let on_close = close_w.compute_point(&t.chip, &gtk::graphene::Point::new(0.0, 0.0)).is_some_and(|pt| {
                x as f32 >= pt.x() && y as f32 >= 0.0
            });
            if on_close {
                p.kill(&t);
            } else if let Some(i) = p.index_of(&t) {
                p.activate(i);
            }
        });
        chip.add_controller(click);
        install_clipboard(&t);

        let (me, wt) = (self.me.borrow().clone(), Rc::downgrade(&t));
        term.connect_child_exited(move |_, _| {
            if let (Some(p), Some(t)) = (me.upgrade(), wt.upgrade()) {
                p.remove(&t);
            }
        });
        let (me, wt) = (self.me.borrow().clone(), Rc::downgrade(&t));
        term.connect_contents_changed(move |_| {
            if let (Some(p), Some(t)) = (me.upgrade(), wt.upgrade()) {
                p.on_output(&t);
            }
        });
        let (me, wt) = (self.me.borrow().clone(), Rc::downgrade(&t));
        term.connect_bell(move |_| {
            if let (Some(p), Some(t)) = (me.upgrade(), wt.upgrade()) {
                if !p.is_seen(&t) {
                    p.set_state(&t, State::Attention);
                }
            }
        });
        let (me, wt) = (self.me.borrow().clone(), Rc::downgrade(&t));
        term.connect_window_title_changed(move |term| {
            if let (Some(_), Some(t)) = (me.upgrade(), wt.upgrade()) {
                if let Some(title) = term.window_title() {
                    t.chip.set_tooltip_text(Some(&format!("{} · {title}", t.label())));
                }
            }
        });

        let wt = Rc::downgrade(&t);
        let dir = cwd.filter(|d| d.is_dir()).unwrap_or(&self.cwd);
        term.spawn_async(
            vte4::PtyFlags::DEFAULT,
            dir.to_str(),
            &[&shell()],
            &[],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&gio::Cancellable>,
            move |res| match res {
                Ok(pid) => {
                    if let Some(t) = wt.upgrade() {
                        t.shell_pid.set(pid.0);
                    }
                }
                Err(e) => eprintln!("vibey: failed to spawn shell: {e}"),
            },
        );
        let n = self.terms.borrow().len();
        self.renumber();
        self.activate(n - 1);
    }

    /// Tabs are numbered 1…n in order (Alt+1…9 jumps to them).
    fn renumber(&self) {
        for (i, t) in self.terms.borrow().iter().enumerate() {
            t.num.set_text(&(i + 1).to_string());
            t.chip.set_tooltip_text(Some(&format!("Terminal {} (Alt+{}): {}", i + 1, i + 1, t.label())));
        }
    }

    /// Show terminal `i` (0-based) and focus it.
    pub fn activate_index(&self, i: usize) -> bool {
        if i < self.terms.borrow().len() {
            self.activate(i);
            true
        } else {
            false
        }
    }

    fn index_of(&self, t: &Rc<Term>) -> Option<usize> {
        self.terms.borrow().iter().position(|x| Rc::ptr_eq(x, t))
    }

    // ---- status ----

    fn on_output(&self, t: &Rc<Term>) {
        let now = Instant::now();
        t.last_output.set(now);
        if t.busy_since.get().is_none() {
            t.busy_since.set(Some(now));
        }
        if t.job_checked.get().is_none_or(|c| now - c >= JOB_CHECK) {
            self.update_job(t);
        }
        if t.state.get() != State::Running {
            self.set_state(t, State::Running);
        }
        if !t.timer_armed.replace(true) {
            self.arm_quiet_timer(t, QUIET);
        }
    }

    fn arm_quiet_timer(&self, t: &Rc<Term>, after: Duration) {
        let (me, wt) = (self.me.borrow().clone(), Rc::downgrade(t));
        glib::timeout_add_local_once(after, move || {
            let (Some(p), Some(t)) = (me.upgrade(), wt.upgrade()) else { return };
            let quiet_for = t.last_output.get().elapsed();
            if quiet_for < QUIET {
                p.arm_quiet_timer(&t, QUIET - quiet_for);
                return;
            }
            t.timer_armed.set(false);
            let busy = t.busy_since.take().map_or(Duration::ZERO, |b| t.last_output.get() - b);
            p.update_job(&t);
            let next = if busy >= SUSTAINED && !p.is_seen(&t) { State::Attention } else { State::Idle };
            if t.state.get() != State::Attention {
                p.set_state(&t, next);
            }
        });
    }

    /// Refresh the foreground job (tab name); a long job ending unseen needs attention.
    fn update_job(&self, t: &Rc<Term>) {
        t.job_checked.set(Some(Instant::now()));
        let job = t.foreground_pgid().and_then(process_name);
        if *t.job.borrow() == job {
            return;
        }
        let ended_long_job = job.is_none()
            && t.job_since.get().is_some_and(|s| s.elapsed() >= SUSTAINED)
            && !self.is_seen(t);
        t.job_since.set(job.as_ref().map(|_| Instant::now()));
        *t.job.borrow_mut() = job;
        t.name.set_text(&t.label());
        for c in CLI_CLASSES {
            t.chip.remove_css_class(c);
        }
        t.chip.add_css_class(cli_class(&t.label()));
        if ended_long_job {
            self.set_state(t, State::Attention);
        }
        self.emit(Event::Changed);
    }

    fn set_state(&self, t: &Rc<Term>, state: State) {
        let prev = t.state.replace(state);
        for c in ["running", "attention"] {
            t.chip.remove_css_class(c);
        }
        match state {
            State::Running => t.chip.add_css_class("running"),
            State::Attention => t.chip.add_css_class("attention"),
            State::Idle => {}
        }
        t.dot.set_text(match state {
            State::Idle => "",
            _ => codicon::CIRCLE_FILLED,
        });
        if state == State::Attention && prev != State::Attention {
            self.emit(Event::Finished(t.label()));
        } else if state != prev {
            self.emit(Event::Changed);
        }
    }

    fn is_seen(&self, t: &Rc<Term>) -> bool {
        self.visible.get() && self.terms.borrow().get(self.active.get()).is_some_and(|a| Rc::ptr_eq(a, t))
    }

    /// The window tells the panel whether its project is on screen and focused; the shown
    /// terminal's attention clears once seen.
    pub fn set_visible(&self, visible: bool) {
        self.visible.set(visible);
        self.clear_seen();
    }

    fn clear_seen(&self) {
        let active = self.terms.borrow().get(self.active.get()).cloned();
        if let Some(t) = active {
            if self.is_seen(&t) && t.state.get() == State::Attention {
                self.set_state(&t, State::Idle);
            }
        }
    }

    /// (any running, any needing attention, label of the first attention terminal)
    pub fn summary(&self) -> (bool, bool, Option<String>) {
        let terms = self.terms.borrow();
        let running = terms.iter().any(|t| t.state.get() == State::Running);
        let attention = terms.iter().find(|t| t.state.get() == State::Attention).map(|t| t.label());
        (running, attention.is_some(), attention)
    }

    // ---- tabs ----

    fn kill(&self, t: &Rc<Term>) {
        let job = t.foreground_pgid().and_then(process_name);
        let do_kill = {
            let (me, wt) = (self.me.borrow().clone(), Rc::downgrade(t));
            move || {
                if let (Some(p), Some(t)) = (me.upgrade(), wt.upgrade()) {
                    let pid = t.shell_pid.get();
                    if pid > 0 {
                        // The shell forwards SIGHUP to its jobs, like closing a terminal window.
                        unsafe { libc::kill(pid, libc::SIGHUP) };
                    }
                    p.remove(&t);
                }
            }
        };
        match job {
            None => do_kill(),
            Some(job) => {
                crate::dialog::confirm(
                    &format!("Kill terminal running {job}?"),
                    "The process will be stopped.",
                    "Kill Terminal",
                    do_kill,
                );
            }
        }
    }

    fn remove(&self, t: &Rc<Term>) {
        let Some(i) = self.index_of(t) else { return };
        self.terms.borrow_mut().remove(i);
        self.header_tabs.remove(&t.chip);
        self.stack.remove(&t.term);
        self.renumber();
        let n = self.terms.borrow().len();
        if n == 0 {
            self.stack.set_visible_child_name("empty");
        } else {
            self.activate(i.min(n - 1));
        }
        self.emit(Event::Changed);
    }

    fn activate(&self, i: usize) {
        let t = {
            let terms = self.terms.borrow();
            let Some(t) = terms.get(i).cloned() else { return };
            for (j, x) in terms.iter().enumerate() {
                if j == i {
                    x.chip.add_css_class("active");
                } else {
                    x.chip.remove_css_class("active");
                }
            }
            t
        };
        self.active.set(i);
        self.stack.set_visible_child(&t.term);
        t.term.grab_focus();
        self.clear_seen();
    }

    /// Send input to the shown terminal as if typed (self-test).
    pub fn feed(&self, text: &str) {
        let t = self.terms.borrow().get(self.active.get()).cloned();
        if let Some(t) = t {
            t.term.feed_child(text.as_bytes());
        }
    }

    /// Self-test hooks: `select`, `clear`, `copy` (prints the clipboard), `menu <png> [hovered row]` (prints the items).
    pub fn selftest(&self, cmd: &str) {
        let Some(t) = self.terms.borrow().get(self.active.get()).cloned() else { return };
        let (cmd, arg) = cmd.split_once(' ').unwrap_or((cmd, ""));
        match cmd {
            "select" => t.term.select_all(),
            "clear" => clear(&t.term),
            "copy" => {
                copy(&t.term);
                t.term.clipboard().read_text_async(None::<&gio::Cancellable>, |r| {
                    eprintln!("selftest: clipboard={:?}", r.ok().flatten().map(|s| s.trim().to_string()));
                });
            }
            "menu" => {
                let (path, hover) = arg.split_once(' ').unwrap_or((arg, ""));
                let (path, hover) = (path.to_string(), hover.parse::<usize>().ok());
                let Some(pop) = context_menu(&t, 40.0, 40.0).map(|m| m.popover().clone()) else { return };
                // Hover has to be in place before the popover's first frame.
                let mut row = 0;
                let mut b = pop.child().and_then(|c| c.first_child());
                while let Some(w) = b {
                    if w.is::<gtk::Button>() {
                        if hover == Some(row) {
                            w.set_state_flags(gtk::StateFlags::PRELIGHT, false);
                        }
                        row += 1;
                    }
                    b = w.next_sibling();
                }
                // The hover highlight fades in; give it time before the snapshot.
                glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                    let mut b = pop.child().and_then(|c| c.first_child());
                    while let Some(w) = b {
                        if let Some(btn) = w.downcast_ref::<gtk::Button>() {
                            let text = btn.child().and_then(|r| r.first_child()).and_downcast::<gtk::Label>().map(|l| l.text());
                            eprintln!("selftest: menu {:?} enabled={}", text.unwrap_or_default(), btn.is_sensitive());
                        }
                        b = w.next_sibling();
                    }
                    let (w, h) = (pop.width() as f64, pop.height() as f64);
                    let snap = gtk::Snapshot::new();
                    gtk::WidgetPaintable::new(Some(&pop)).snapshot(&snap, w, h);
                    if let Some(node) = snap.to_node() {
                        let r = gtk::gsk::CairoRenderer::new();
                        if r.realize_for_display(&WidgetExt::display(&pop)).is_ok() {
                            let tex = r.render_texture(&node, None);
                            let _ = tex.save_to_png(&path);
                            r.unrealize();
                        }
                    }
                    pop.popdown();
                    // The menu must outlive its use, parented and realized (see `menu`).
                    glib::timeout_add_local_once(Duration::from_millis(150), move || {
                        eprintln!("selftest: menu kept parent={} realized={}", pop.parent().is_some(), pop.is_realized());
                    });
                });
            }
            _ => eprintln!("selftest: unknown terminal command {cmd}"),
        }
    }

    pub fn focus(&self) {
        let t = self.terms.borrow().get(self.active.get()).cloned();
        if let Some(t) = t {
            t.term.grab_focus();
        }
    }

    pub fn restyle(&self, theme: &ThemeModel) {
        for t in self.terms.borrow().iter() {
            apply_colors(&t.term, theme);
        }
    }

    /// (tab label, shell pid) per terminal, for resource sampling.
    pub fn shells(&self) -> Vec<(String, i32)> {
        self.terms.borrow().iter().filter(|t| t.shell_pid.get() > 0).map(|t| (t.label(), t.shell_pid.get())).collect()
    }

    /// Foreground jobs (other than the shell) running in this panel's terminals.
    pub fn running_jobs(&self) -> Vec<String> {
        self.terms.borrow().iter().filter_map(|t| t.foreground_pgid().and_then(process_name)).collect()
    }
}

/// Copy and paste like GNOME Terminal (Ctrl+Shift+C / Ctrl+Shift+V), Ctrl+C copies instead of
/// interrupting while text is selected (like Windows Terminal), and a right-click menu.
fn install_clipboard(t: &Rc<Term>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let term = t.term.downgrade();
    keys.connect_key_pressed(move |_, key, _, mods| {
        use gtk::gdk::{Key, ModifierType as M};
        let Some(term) = term.upgrade() else { return glib::Propagation::Proceed };
        if !mods.contains(M::CONTROL_MASK) || mods.contains(M::ALT_MASK) {
            return glib::Propagation::Proceed;
        }
        let shift = mods.contains(M::SHIFT_MASK);
        match key.to_lower() {
            Key::c if shift || term.has_selection() => copy(&term),
            Key::v if shift => term.paste_clipboard(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    t.term.add_controller(keys);

    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let wt = Rc::downgrade(t);
    click.connect_pressed(move |g, _, x, y| {
        let Some(t) = wt.upgrade() else { return };
        g.set_state(gtk::EventSequenceState::Claimed);
        t.term.grab_focus();
        context_menu(&t, x, y);
    });
    t.term.add_controller(click);
}

/// Copy the selection as plain text and drop it, so the next Ctrl+C interrupts again.
fn copy(term: &vte4::Terminal) {
    if term.has_selection() {
        term.copy_clipboard_format(vte4::Format::Text);
        term.unselect_all();
    }
}

/// Clear the screen and scrollback, then have the shell redraw its prompt (Ctrl+L).
fn clear(term: &vte4::Terminal) {
    term.reset(true, true);
    term.feed_child(b"\x0c");
}

/// The right-click menu's rows; `context_menu` enables them in this order.
const MENU: [menu::Entry; 4] = [
    ("Copy", "Ctrl+Shift+C", false),
    ("Paste", "Ctrl+Shift+V", false),
    ("Select All", "", false),
    ("Clear", "", true),
];

thread_local! {
    static CONTEXT_MENU: RefCell<Option<Rc<ContextMenu>>> = const { RefCell::new(None) };
}

/// Create the terminals' right-click menu, parented to `parent` (the window content) for the
/// life of the window. It is shared by every terminal and never unparented; see `menu`.
pub fn install_menu(parent: &impl IsA<gtk::Widget>) {
    CONTEXT_MENU.with(|m| *m.borrow_mut() = Some(ContextMenu::new(parent, &MENU)));
}

fn context_menu(t: &Rc<Term>, x: f64, y: f64) -> Option<Rc<ContextMenu>> {
    let menu = CONTEXT_MENU.with(|m| m.borrow().clone())?;
    // A reset under a running program (an AI CLI, vim) would scramble its screen, so Clear
    // is only offered at a shell prompt.
    let at_prompt = t.foreground_pgid().is_none();
    let enabled = [t.term.has_selection(), true, true, at_prompt];
    let actions: [fn(&vte4::Terminal); 4] = [copy, |t| t.paste_clipboard(), |t| t.select_all(), clear];
    let term = t.term.downgrade();
    menu.show(&t.term, x, y, &enabled, move |i| {
        if let Some(term) = term.upgrade() {
            actions[i](&term);
            term.grab_focus();
        }
    });
    Some(menu)
}

fn apply_colors(term: &vte4::Terminal, theme: &ThemeModel) {
    let (fg, bg, palette) = theme.terminal_colors();
    let palette: Vec<gtk::gdk::RGBA> = palette.iter().map(|c| c.gdk()).collect();
    let refs: Vec<&gtk::gdk::RGBA> = palette.iter().collect();
    term.set_colors(Some(&fg.gdk()), Some(&bg.gdk()), &refs);
    term.set_color_cursor(Some(&theme.get("terminalCursor.foreground").gdk()));
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_clis() {
        assert_eq!(cli_class("claude"), "cli-claude");
        assert_eq!(cli_class("codex"), "cli-codex");
        assert_eq!(cli_class("gemini"), "cli-gemini");
        assert_eq!(cli_class("aider"), "cli-ai");
        assert_eq!(cli_class("bash"), "cli-shell");
        assert_eq!(cli_class("cargo"), "cli-proc");
    }
}
