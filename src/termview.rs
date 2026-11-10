//! Built-in terminal: a shell running in a PTY, emulated by `alacritty_terminal`
//! (VT parser, grid, scrollback, selection) and drawn directly with egui.
//!
//! Rendering copies the visible cells out of the grid while holding the lock
//! briefly and paints them afterwards, so the PTY reader thread is never blocked
//! by drawing. Text is painted in runs of same-styled ASCII cells; other glyphs
//! are placed cell by cell so wide and fallback characters stay on the grid.
use crate::theme::Theme;
use alacritty_terminal::event::{Event as TEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AColor, CursorShape, NamedColor, Rgb};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontId, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const PAD: f32 = 6.0;

/// Grid size handed to `Term::new` / `Term::resize`.
struct Size {
    cols: usize,
    lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Forwards terminal events to the UI thread and wakes egui up.
#[derive(Clone)]
struct Listener {
    tx: Sender<TEvent>,
    ctx: egui::Context,
    dirty: Arc<AtomicBool>,
    /// The program wrote something (unlike `dirty`, not set for the first frame).
    output: Arc<AtomicBool>,
}

impl EventListener for Listener {
    fn send_event(&self, e: TEvent) {
        match e {
            TEvent::Wakeup => {
                self.dirty.store(true, Ordering::Relaxed);
                self.output.store(true, Ordering::Relaxed);
                self.ctx.request_repaint();
            }
            TEvent::MouseCursorDirty | TEvent::CursorBlinkingChange => {}
            e => {
                let _ = self.tx.send(e);
                self.ctx.request_repaint();
            }
        }
    }
}

/// Per-frame options from the app settings.
pub struct TermOpts<'a> {
    pub theme: &'a Theme,
    pub font_size: f32,
    pub copy_on_select: bool,
    /// Lines scrolled per row of trackpad travel, per mouse wheel notch.
    pub scroll: [f32; 2],
    /// The pane holding this terminal is the active one.
    pub active: bool,
    /// Allowed splits of its window: right, left, down, up.
    pub can_split: [bool; 4],
    /// The window shares its tab with other windows.
    pub in_split: bool,
    /// Radius of the window's bottom corners (the background follows them).
    pub radius: f32,
}

/// Background colour of a terminal drawn with theme `th`.
pub fn background(th: &Theme) -> Color32 {
    Palette::new(th).bg
}

/// Requests from the terminal widget to the app.
#[derive(Default)]
pub struct TermResponse {
    /// The user clicked into the terminal (focus its pane).
    pub focus: bool,
    pub close: bool,
    pub restart: bool,
    /// Open this folder in a file tab.
    pub open_dir: Option<PathBuf>,
    pub font_delta: f32,
    /// Split the window: direction, and `true` for a terminal (`false`: files).
    pub split: Option<(crate::layout::SplitDir, bool)>,
}

/// One visible cell copied out of the grid.
#[derive(Clone)]
struct VCell {
    c: char,
    zw: Option<Vec<char>>,
    fg: Color32,
    bg: Color32,
    flags: Flags,
    link: Option<Arc<str>>,
}

/// A link on screen: viewport row, column range (inclusive) and target.
#[derive(Clone, Debug, PartialEq)]
struct LinkSpan {
    row: usize,
    c0: usize,
    c1: usize,
    url: Arc<str>,
}

pub struct TermSession {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    rx: Receiver<TEvent>,
    dirty: Arc<AtomicBool>,
    pub id: egui::Id,
    /// Title set by the program (OSC 0/2).
    pub title: String,
    /// Current directory of the shell (tracked on Linux, else the start folder).
    pub cwd: PathBuf,
    cwd_at: Instant,
    #[cfg_attr(not(unix), allow(dead_code))]
    pid: Option<u32>,
    /// A second handle on the pty, to ask which process group is in the foreground.
    #[cfg(unix)]
    pty: Option<std::fs::File>,
    /// argv the session was started with (for restart).
    pub argv: Option<Vec<String>>,
    /// The profile's startup script, typed at the prompt line by line (kept for restart).
    pub startup: Vec<String>,
    /// Lines of `startup` not typed yet.
    pending: std::collections::VecDeque<String>,
    output: Arc<AtomicBool>,
    /// When the program last wrote something, and whether it has since the last
    /// startup line was typed (the shell answers each one with a new prompt).
    last_output: Instant,
    output_since_line: bool,
    line_at: Instant,
    pub exited: Option<i32>,
    /// New output while the tab was not visible.
    pub activity: bool,
    pub bell: Option<Instant>,
    cols: usize,
    lines: usize,
    cell: egui::Vec2,
    last_drawn: u64,
    scroll_acc: f32,
    /// Selection drag in progress.
    selecting: bool,
    /// Mouse button held while reporting to the program.
    report_button: Option<u8>,
    last_report_cell: Option<(usize, usize)>,
    skip_text: bool,
    /// Link under the right-click menu.
    menu_link: Option<Arc<str>>,
    pub want_focus: bool,
    /// Colour theme from the terminal profile (`None` = the app theme).
    pub theme: Option<&'static Theme>,
    /// Name of the profile it was started from: changes to that profile reach it.
    pub profile: Option<String>,
    /// The ANSI colours it was last drawn with (terminal programs can ask for them).
    ansi: &'static [u32; 16],
    url_re: regex::Regex,
    cells: Vec<VCell>,
    links: Vec<LinkSpan>,
    bold_font: FontId,
}

impl Drop for TermSession {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

impl TermSession {
    /// Start `argv` (or the default shell) in `dir`.
    pub fn spawn(ctx: &egui::Context, dir: &Path, argv: Option<Vec<String>>, scrollback: usize) -> Result<Self, String> {
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "MatrixFinder".to_string());
        env.insert("TERM_PROGRAM_VERSION".to_string(), env!("CARGO_PKG_VERSION").to_string());
        // Apps started from Finder / the Dock (or a bare session) often have no locale,
        // and zsh themes then fail on their UTF-8 symbols ("character not in range").
        // Like Terminal.app and iTerm2, give the shell a UTF-8 locale when none is set.
        let has_locale = ["LC_ALL", "LC_CTYPE", "LANG"].iter().any(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()));
        if !has_locale {
            let lang = if cfg!(target_os = "macos") { "en_US.UTF-8" } else { "C.UTF-8" };
            env.insert("LANG".to_string(), lang.to_string());
        }
        let shell = argv.as_ref().filter(|v| !v.is_empty()).map(|v| tty::Shell::new(v[0].clone(), v[1..].to_vec()));
        let shell = shell.or_else(|| {
            // Unix: start the user's shell as a login shell, like most terminals do.
            (!cfg!(windows)).then(|| tty::Shell::new(crate::terminal::default_shell(), vec!["-l".into()]))
        });
        let opts = tty::Options {
            shell,
            working_directory: Some(dir.to_path_buf()),
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };
        let ws = WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 16 };
        let pty = tty::new(&opts, ws, 0).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        let pid = Some(pty.child().id());
        #[cfg(not(unix))]
        let pid = None;
        #[cfg(unix)]
        let pty_file = pty.file().try_clone().ok();
        let (tx, rx) = channel();
        let dirty = Arc::new(AtomicBool::new(true));
        let output = Arc::new(AtomicBool::new(false));
        let listener = Listener { tx, ctx: ctx.clone(), dirty: dirty.clone(), output: output.clone() };
        let config = Config { scrolling_history: scrollback.clamp(100, 200_000), kitty_keyboard: false, ..Default::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &Size { cols: 80, lines: 24 }, listener.clone())));
        let ev = EventLoop::new(term.clone(), listener, pty, true, false).map_err(|e| e.to_string())?;
        let sender = ev.channel();
        ev.spawn();
        Ok(Self {
            term,
            sender,
            rx,
            dirty,
            id: egui::Id::new(("terminal", next_id())),
            title: String::new(),
            cwd: dir.to_path_buf(),
            cwd_at: Instant::now(),
            pid,
            #[cfg(unix)]
            pty: pty_file,
            argv,
            startup: Vec::new(),
            pending: Default::default(),
            output,
            last_output: Instant::now(),
            output_since_line: false,
            line_at: Instant::now(),
            exited: None,
            activity: false,
            bell: None,
            cols: 80,
            lines: 24,
            cell: vec2(8.0, 16.0),
            last_drawn: 0,
            scroll_acc: 0.0,
            selecting: false,
            report_button: None,
            last_report_cell: None,
            skip_text: false,
            menu_link: None,
            want_focus: true,
            theme: None,
            profile: None,
            ansi: &DARK_ANSI,
            url_re: regex::Regex::new(r#"(?:https?|ftp|file)://[^\s<>"'`\x00-\x1f]+|www\.[A-Za-z0-9-]+\.[^\s<>"'`\x00-\x1f]+"#).unwrap(),
            cells: Vec::new(),
            links: Vec::new(),
            bold_font: FontId::monospace(14.0),
        })
    }

    /// Type `lines` (a profile's startup script) at the prompt once the shell has
    /// started: after its startup files ran, so what they set up is there to use.
    pub fn run_at_start(&mut self, lines: Vec<String>) {
        self.pending = lines.iter().cloned().collect();
        self.startup = lines;
        self.line_at = Instant::now();
    }

    /// Type the next startup line when the shell waits at its prompt: it has written
    /// something (the prompt) since the last line, has been quiet for a moment, and
    /// no program has the terminal. Lines typed a little early are not lost: the
    /// terminal keeps them until the shell reads its input.
    fn type_startup(&mut self, ctx: &egui::Context) {
        if self.pending.is_empty() || self.exited.is_some() {
            return;
        }
        const QUIET: Duration = Duration::from_millis(250);
        // A shell that never prints a prompt still gets its script.
        const NO_PROMPT: Duration = Duration::from_secs(5);
        let prompt = self.output_since_line || self.line_at.elapsed() > NO_PROMPT;
        if prompt && self.last_output.elapsed() >= QUIET && self.is_idle() {
            let line = self.pending.pop_front().unwrap_or_default();
            if self.run_line(&line) {
                self.output_since_line = false;
                self.line_at = Instant::now();
            } else {
                self.pending.push_front(line);
            }
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let _ = self.sender.send(Msg::Input(bytes.into()));
    }

    /// Type text into the terminal as if pasted (bracketed when the program asks).
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let s = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.write(s.into_bytes());
        self.scroll_to_bottom();
    }

    fn scroll_to_bottom(&self) {
        let mut t = self.term.lock();
        if t.grid().display_offset() != 0 {
            t.scroll_display(Scroll::Bottom);
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    pub fn select_all(&self) {
        let mut t = self.term.lock();
        let mut sel = Selection::new(SelectionType::Lines, Point::new(t.topmost_line(), Column(0)), Side::Left);
        sel.update(Point::new(t.bottommost_line(), t.last_column()), Side::Right);
        t.selection = Some(sel);
    }

    /// Clear the scrollback and the screen (like Ctrl+L plus history).
    pub fn clear(&self) {
        {
            let mut t = self.term.lock();
            t.grid_mut().clear_history();
            t.selection = None;
        }
        self.write(&b"\x0c"[..]);
    }

    /// Make the shell change to `dir`: clear what is typed on the prompt line, then
    /// run the shell's own `cd`. False (nothing sent) when the shell has exited or a
    /// full-screen program (vim, less, …) has the terminal.
    pub fn cd(&mut self, dir: &Path) -> bool {
        let d = dir.to_string_lossy();
        let line = match self.shell_name().to_lowercase().as_str() {
            "cmd" => format!("cd /d \"{d}\""),
            "pwsh" | "powershell" => format!("Set-Location -LiteralPath '{}'", d.replace('\'', "''")),
            "nu" | "nushell" => format!("cd \"{}\"", d.replace('\\', "\\\\").replace('"', "\\\"")),
            _ => format!("cd {}", crate::terminal::sh_quote(&d)),
        };
        if !self.run_line(&line) {
            return false;
        }
        self.cwd = dir.to_path_buf();
        true
    }

    /// Start `program` (a command the shell finds, or a full path) at the shell's
    /// prompt. False (nothing sent) while another program runs in the foreground,
    /// or when the shell has exited.
    pub fn run_program(&mut self, program: &str) -> bool {
        if !self.is_idle() {
            return false;
        }
        let plain = !program.is_empty() && program.chars().all(|c| c.is_ascii_alphanumeric() || "/\\._-+:".contains(c));
        let line = match self.shell_name().to_lowercase().as_str() {
            _ if plain => program.to_string(),
            "cmd" => format!("\"{program}\""),
            "pwsh" | "powershell" => format!("& '{}'", program.replace('\'', "''")),
            "nu" | "nushell" => format!("^\"{}\"", program.replace('\\', "\\\\").replace('"', "\\\"")),
            _ => crate::terminal::sh_quote(program),
        };
        self.run_line(&line)
    }

    /// The shell waits at its prompt: it runs, and no other program has the terminal
    /// (none in the foreground, which only Unix can tell, and no full-screen one).
    pub fn is_idle(&self) -> bool {
        self.exited.is_none() && self.foreground_pid().is_none() && !self.term.lock().mode().contains(TermMode::ALT_SCREEN)
    }

    /// Clear what is typed on the prompt line, type `line` and press Enter. False
    /// (nothing sent) when the shell has exited or a full-screen program (vim,
    /// less, …) has the terminal.
    fn run_line(&mut self, line: &str) -> bool {
        if self.exited.is_some() || self.term.lock().mode().contains(TermMode::ALT_SCREEN) {
            return false;
        }
        // Esc clears the prompt line in cmd and PowerShell (PSReadLine); Ctrl+E
        // Ctrl+U in POSIX shells, fish and nu.
        let clear = match self.shell_name().to_lowercase().as_str() {
            "cmd" | "pwsh" | "powershell" => "\x1b",
            _ => "\x05\x15",
        };
        self.write(format!("{clear}{line}\r").into_bytes());
        self.scroll_to_bottom();
        true
    }

    /// Name of the shell (or program) the session runs, e.g. "zsh".
    pub fn shell_name(&self) -> String {
        let shell = self.argv.as_ref().and_then(|v| v.first().cloned()).unwrap_or_else(crate::terminal::default_shell);
        shell.rsplit(['/', '\\']).next().unwrap_or("").trim_end_matches(".exe").to_string()
    }

    /// What runs right now: the program's own title (e.g. "vim main.rs") when it
    /// set one that is more than the usual "user@host: folder", else the shell.
    pub fn activity_label(&self) -> String {
        let title = self.title.trim();
        let pathish = title.starts_with('~') || title.starts_with('/') || title.contains('@') || title.contains(":\\");
        if title.is_empty() || pathish {
            self.shell_name()
        } else {
            title.to_string()
        }
    }

    /// The program running in the foreground in place of the shell (e.g. "vim"),
    /// or `None` at the shell's prompt. Unix only: elsewhere always `None`.
    pub fn running_program(&self) -> Option<String> {
        let pid = self.foreground_pid()?;
        Some(process_name(pid).unwrap_or_else(|| self.activity_label()))
    }

    /// The foreground process group of the terminal when it is not the shell's.
    /// The shell leads its own session, so at its prompt the two are the same.
    pub fn foreground_pid(&self) -> Option<u32> {
        if self.exited.is_some() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let fd = self.pty.as_ref()?.as_raw_fd();
            // SAFETY: `fd` is a pty descriptor this session owns and keeps open.
            let pgrp = unsafe { libc::tcgetpgrp(fd) };
            if pgrp > 0 && Some(pgrp as u32) != self.pid {
                return Some(pgrp as u32);
            }
        }
        None
    }

    /// A short label for the tab.
    pub fn label(&self) -> String {
        if !self.title.trim().is_empty() {
            return self.title.trim().to_string();
        }
        let dir = self.cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| self.cwd.to_string_lossy().to_string());
        let shell = self
            .argv
            .as_ref()
            .and_then(|v| v.first().cloned())
            .unwrap_or_else(crate::terminal::default_shell);
        let shell = shell.rsplit(['/', '\\']).next().unwrap_or("").trim_end_matches(".exe").to_string();
        format!("{shell}: {dir}")
    }

    /// Handle terminal events (call every frame, also for hidden tabs).
    pub fn poll(&mut self, ctx: &egui::Context) {
        let frame = ctx.cumulative_pass_nr();
        let woke = self.dirty.swap(false, Ordering::Relaxed);
        if woke && self.last_drawn + 1 < frame {
            self.activity = true;
        }
        while let Ok(e) = self.rx.try_recv() {
            match e {
                TEvent::Title(t) => self.title = t,
                TEvent::ResetTitle => self.title.clear(),
                TEvent::ClipboardStore(_, s) => ctx.copy_text(s),
                TEvent::ClipboardLoad(_, f) => {
                    if let Some(text) = clipboard_get(false) {
                        self.write(f(&text).into_bytes());
                    }
                }
                TEvent::ColorRequest(i, f) => {
                    let c = palette_color(i, self.ansi);
                    self.write(f(Rgb { r: c.r(), g: c.g(), b: c.b() }).into_bytes());
                }
                TEvent::PtyWrite(s) => self.write(s.into_bytes()),
                TEvent::TextAreaSizeRequest(f) => {
                    let ws = self.window_size();
                    self.write(f(ws).into_bytes());
                }
                TEvent::Bell => self.bell = Some(Instant::now()),
                TEvent::ChildExit(status) => self.exited = Some(status.code().unwrap_or(-1)),
                TEvent::Exit if self.exited.is_none() => self.exited = Some(0),
                _ => {}
            }
        }
        if self.output.swap(false, Ordering::Relaxed) {
            self.last_output = Instant::now();
            self.output_since_line = true;
        }
        self.type_startup(ctx);
        // The shell prints a prompt after `cd`, so new output is the moment to look.
        if woke || self.cwd_at.elapsed() > Duration::from_secs(3) {
            self.cwd_at = Instant::now();
            #[cfg(target_os = "linux")]
            if let Some(pid) = self.pid {
                if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
                    self.cwd = p;
                }
            }
        }
    }

    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.lines as u16,
            num_cols: self.cols as u16,
            cell_width: self.cell.x.round().max(1.0) as u16,
            cell_height: self.cell.y.round().max(1.0) as u16,
        }
    }

    fn resize(&mut self, cols: usize, lines: usize) {
        if cols == self.cols && lines == self.lines {
            return;
        }
        self.cols = cols;
        self.lines = lines;
        self.term.lock().resize(Size { cols, lines });
        let _ = self.sender.send(Msg::Resize(self.window_size()));
    }

    // ------------------------------------------------------------ widget

    pub fn ui(&mut self, ui: &mut egui::Ui, o: &TermOpts) -> TermResponse {
        let mut out = TermResponse::default();
        let th = o.theme;
        self.poll(ui.ctx());
        self.last_drawn = ui.ctx().cumulative_pass_nr();
        self.activity = false;

        let rect = ui.available_rect_before_wrap();
        let resp = ui.interact(rect, self.id, Sense::click_and_drag());
        let pal = Palette::new(th);
        self.ansi = pal.ansi;
        // Square at the top (the info bar is above), rounded like the window below.
        let r = o.radius.clamp(0.0, 255.0) as u8;
        let corners = egui::CornerRadius { nw: 0, ne: 0, sw: r, se: r };
        ui.painter().rect_filled(rect, corners, pal.bg);

        // Focus: the terminal takes the keyboard while its pane is active.
        if !o.active {
            if resp.has_focus() {
                ui.memory_mut(|m| m.surrender_focus(self.id));
            }
        } else if self.want_focus || ui.memory(|m| m.focused().is_none()) {
            resp.request_focus();
            self.want_focus = false;
        }
        if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
            resp.request_focus();
            out.focus = true;
        }
        let focused = resp.has_focus() || ui.memory(|m| m.has_focus(self.id));
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(self.id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
            });
        }

        // Cell metrics and grid size.
        let size = o.font_size.clamp(6.0, 48.0);
        let font = FontId::new(size, egui::FontFamily::Name("term".into()));
        self.bold_font = FontId::new(size, egui::FontFamily::Name("term-bold".into()));
        // Measure the advance the way runs are laid out (glyph positions are
        // pixel-rounded), so long runs stay aligned with the cell grid.
        let (cw, ch) = ui.fonts(|f| (f.layout_no_wrap("M".repeat(100), font.clone(), Color32::WHITE).size().x / 100.0, f.row_height(&font)));
        self.cell = vec2(cw.max(1.0), ch.ceil().max(1.0));
        let origin = rect.min + vec2(PAD, PAD / 2.0);
        let cols = (((rect.width() - 2.0 * PAD) / self.cell.x).floor() as usize).max(2);
        let lines = (((rect.height() - PAD) / self.cell.y).floor() as usize).max(1);
        self.resize(cols, lines);

        if focused && self.exited.is_none() {
            self.keyboard(ui);
        } else if focused {
            // After the process exited: Enter restarts, Esc closes.
            ui.input(|i| {
                if i.key_pressed(Key::Enter) {
                    out.restart = true;
                }
                if i.key_pressed(Key::Escape) {
                    out.close = true;
                }
            });
        }
        let hover = resp.hover_pos().filter(|p| rect.contains(*p));
        self.mouse(ui, &resp, rect, origin, o, &mut out);

        // ---- copy the visible cells out of the grid (short lock)
        let (cursor, offset, history) = {
            let t = self.term.lock();
            let content = t.renderable_content();
            let offset = content.display_offset;
            let colors = content.colors;
            let sel = content.selection;
            let n = self.cols * self.lines;
            self.cells.clear();
            self.cells.resize(n, VCell { c: ' ', zw: None, fg: pal.fg, bg: pal.bg, flags: Flags::empty(), link: None });
            for ic in content.display_iter {
                let row = ic.point.line.0 + offset as i32;
                if row < 0 || row as usize >= self.lines || ic.point.column.0 >= self.cols {
                    continue;
                }
                let cell = ic.cell;
                let mut fg = pal.resolve(cell.fg, colors, true);
                let mut bg = pal.resolve(cell.bg, colors, false);
                if cell.flags.contains(Flags::DIM) {
                    fg = blend(fg, bg, 0.4);
                }
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.flags.contains(Flags::HIDDEN) {
                    fg = bg;
                }
                if sel.is_some_and(|s| s.contains_cell(&ic, content.cursor.point, content.cursor.shape)) {
                    bg = pal.selection;
                    if fg == pal.selection {
                        fg = pal.fg;
                    }
                }
                let link = cell.hyperlink().map(|h| Arc::<str>::from(h.uri()));
                let zw = cell.zerowidth().map(|z| z.to_vec());
                self.cells[row as usize * self.cols + ic.point.column.0] = VCell { c: cell.c, zw, fg, bg, flags: cell.flags, link };
            }
            let cur = content.cursor;
            let cursor = (cur.point.line.0 + offset as i32, cur.point.column.0, cur.shape);
            (cursor, offset, t.grid().history_size())
        };
        self.detect_links();

        // ---- paint
        let painter = ui.painter_at(rect);
        let (cw, chh) = (self.cell.x, self.cell.y);
        let hover_link = hover.and_then(|p| self.link_at(p, origin)).cloned();
        let link_mod = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        // Non-ASCII characters on screen, checked against the fonts after painting.
        let mut others: std::collections::HashSet<char> = std::collections::HashSet::new();
        for row in 0..self.lines {
            let y = origin.y + row as f32 * chh;
            let base = row * self.cols;
            // Backgrounds, merged into runs.
            let mut col = 0;
            while col < self.cols {
                let bg = self.cells[base + col].bg;
                let start = col;
                while col < self.cols && self.cells[base + col].bg == bg {
                    col += 1;
                }
                if bg != pal.bg {
                    let r = Rect::from_min_max(pos2(origin.x + start as f32 * cw, y), pos2(origin.x + col as f32 * cw, y + chh));
                    painter.rect_filled(r, 0.0, bg);
                }
            }
            // Text runs.
            let mut col = 0;
            let mut run = String::new();
            while col < self.cols {
                let c = &self.cells[base + col];
                if c.flags.contains(Flags::WIDE_CHAR_SPACER) || c.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                    col += 1;
                    continue;
                }
                if c.c == ' ' && c.zw.is_none() && !c.flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT) {
                    col += 1;
                    continue;
                }
                let x = origin.x + col as f32 * cw;
                if c.c.is_ascii() && c.zw.is_none() {
                    // Extend a run of same-styled ASCII.
                    let (fg, fl) = (c.fg, c.flags);
                    let start = col;
                    run.clear();
                    while col < self.cols {
                        let d = &self.cells[base + col];
                        if !d.c.is_ascii() || d.zw.is_some() || d.fg != fg || d.flags != fl {
                            break;
                        }
                        run.push(if d.c == '\t' { ' ' } else { d.c });
                        col += 1;
                    }
                    let trimmed = run.trim_end();
                    self.paint_text(&painter, pos2(x, y), trimmed, &font, fg, fl, (col - start) as f32 * cw);
                } else {
                    let mut s = String::new();
                    s.push(c.c);
                    if let Some(z) = &c.zw {
                        s.extend(z.iter());
                    }
                    others.extend(s.chars());
                    let w = if c.flags.contains(Flags::WIDE_CHAR) { 2.0 } else { 1.0 } * cw;
                    self.paint_text(&painter, pos2(x, y), &s, &font, c.fg, c.flags, w);
                    col += 1;
                }
            }
        }
        // Characters no loaded font has are drawn as boxes this frame; look for a
        // system font that has them (the frame is repainted once one is added).
        if !others.is_empty() {
            let bold = self.bold_font.clone();
            let missing: Vec<char> = ui.fonts(|f| others.into_iter().filter(|&c| !c.is_whitespace() && (!f.has_glyph(&font, c) || !f.has_glyph(&bold, c))).collect());
            if !missing.is_empty() {
                crate::fontfallback::request(ui.ctx(), missing);
            }
        }
        // Links: a faint underline for every link, a solid one under the hovered one.
        for l in &self.links {
            let hovered = hover_link.as_ref() == Some(l);
            let y = origin.y + (l.row as f32 + 1.0) * chh - 1.5;
            let x0 = origin.x + l.c0 as f32 * cw;
            let x1 = origin.x + (l.c1 + 1) as f32 * cw;
            let color = if hovered { th.accent } else { th.accent.gamma_multiply(0.45) };
            painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(if hovered { 1.4_f32 } else { 1.0 }, color));
        }
        if let Some(l) = &hover_link {
            if link_mod {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            let hint = if cfg!(target_os = "macos") { "⌘+Click" } else { "Ctrl+Click" };
            resp.clone().on_hover_text_at_pointer(format!("{}\n{hint}", l.url));
        } else if hover.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }

        // Cursor (only when the live screen is shown).
        let (crow, ccol, shape) = cursor;
        if offset == 0 && crow >= 0 && (crow as usize) < self.lines && ccol < self.cols && self.exited.is_none() {
            let wide = self.cells[crow as usize * self.cols + ccol].flags.contains(Flags::WIDE_CHAR);
            let r = Rect::from_min_size(pos2(origin.x + ccol as f32 * cw, origin.y + crow as f32 * chh), vec2(cw * if wide { 2.0 } else { 1.0 }, chh));
            let cc = pal.cursor;
            match (shape, focused) {
                (CursorShape::Hidden, _) => {}
                (CursorShape::Block, true) => {
                    painter.rect_filled(r, 1.0, cc);
                    let cell = &self.cells[crow as usize * self.cols + ccol];
                    if cell.c != ' ' {
                        let s: String = std::iter::once(cell.c).chain(cell.zw.iter().flatten().copied()).collect();
                        self.paint_text(&painter, r.min, &s, &font, pal.bg, cell.flags, r.width());
                    }
                }
                (CursorShape::Beam, _) => {
                    painter.rect_filled(Rect::from_min_size(r.min, vec2(2.0, chh)), 0.0, cc);
                }
                (CursorShape::Underline, _) => {
                    painter.rect_filled(Rect::from_min_max(pos2(r.left(), r.bottom() - 2.0), r.max), 0.0, cc);
                }
                _ => {
                    painter.rect_stroke(r.shrink(0.5), 1.0, Stroke::new(1.0_f32, cc), egui::StrokeKind::Inside);
                }
            }
        }

        // Scrollback position indicator.
        if offset > 0 && history > 0 {
            let total = (history + self.lines) as f32;
            let h = (rect.height() * self.lines as f32 / total).max(18.0);
            let top = rect.top() + (rect.height() - h) * (1.0 - offset as f32 / history as f32);
            let bar = Rect::from_min_size(pos2(rect.right() - 5.0, top), vec2(3.0, h));
            painter.rect_filled(bar, 1.5, th.weak.gamma_multiply(0.6));
            let label = format!("↑ {offset}");
            let g = painter.layout_no_wrap(label, FontId::proportional(11.0), th.on_accent());
            let pill = Rect::from_min_size(pos2(rect.right() - g.size().x - 22.0, rect.top() + 6.0), g.size() + vec2(12.0, 4.0));
            painter.rect_filled(pill, 8.0, th.accent.gamma_multiply(0.85));
            painter.galley(pill.min + vec2(6.0, 2.0), g, th.on_accent());
        }

        // Visual bell.
        if let Some(at) = self.bell {
            let e = at.elapsed().as_secs_f32();
            if e < 0.25 {
                painter.rect_filled(rect, corners, th.accent.gamma_multiply(0.12 * (1.0 - e / 0.25)));
                ui.ctx().request_repaint();
            } else if o.active {
                self.bell = None;
            }
        }

        // Exit banner.
        if let Some(code) = self.exited {
            let text = format!(
                "{}  ·  {}",
                crate::i18n::t("Process exited with code") + &format!(" {code}"),
                crate::i18n::t("Enter: restart · Esc: close tab")
            );
            let g = painter.layout_no_wrap(text, FontId::proportional(12.5), th.text);
            let r = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 26.0), g.size() + vec2(24.0, 12.0));
            painter.rect_filled(r, 8.0, th.surface);
            painter.rect_stroke(r, 8.0, Stroke::new(1.0_f32, th.border), egui::StrokeKind::Inside);
            painter.galley(r.min + vec2(12.0, 6.0), g, th.text);
        }

        // Right-click menu.
        if resp.secondary_clicked() {
            self.menu_link = hover_link.as_ref().map(|l| l.url.clone());
        }
        resp.context_menu(|ui| self.context_menu(ui, o, &mut out));
        ui.advance_cursor_after_rect(rect);
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_text(&self, p: &egui::Painter, pos: Pos2, s: &str, font: &FontId, fg: Color32, fl: Flags, width: f32) {
        if !s.is_empty() {
            let f = if fl.contains(Flags::BOLD) { &self.bold_font } else { font };
            p.text(pos, Align2::LEFT_TOP, s, f.clone(), fg);

        }
        let h = self.cell.y;
        if fl.intersects(Flags::ALL_UNDERLINES) {
            let y = pos.y + h - 1.5;
            p.line_segment([pos2(pos.x, y), pos2(pos.x + width, y)], Stroke::new(1.0_f32, fg));
            if fl.contains(Flags::DOUBLE_UNDERLINE) {
                p.line_segment([pos2(pos.x, y - 2.0), pos2(pos.x + width, y - 2.0)], Stroke::new(1.0_f32, fg));
            }
        }
        if fl.contains(Flags::STRIKEOUT) {
            let y = pos.y + h * 0.52;
            p.line_segment([pos2(pos.x, y), pos2(pos.x + width, y)], Stroke::new(1.0_f32, fg));
        }
    }

    /// Find URLs (regex, also across wrapped lines) and OSC 8 hyperlinks on screen.
    fn detect_links(&mut self) {
        self.links.clear();
        let (cols, lines) = (self.cols, self.lines);
        let mut row = 0;
        while row < lines {
            // Join soft-wrapped rows into one logical line.
            let mut text = String::new();
            let mut map: Vec<(usize, usize)> = Vec::new(); // byte offset -> (row, col)
            let mut r = row;
            loop {
                for c in 0..cols {
                    let cell = &self.cells[r * cols + c];
                    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        continue;
                    }
                    let ch = if cell.c == '\0' { ' ' } else { cell.c };
                    for _ in 0..ch.len_utf8() {
                        map.push((r, c));
                    }
                    text.push(ch);
                }
                let wrapped = self.cells[r * cols + cols - 1].flags.contains(Flags::WRAPLINE);
                r += 1;
                if !wrapped || r >= lines {
                    break;
                }
            }
            for m in self.url_re.find_iter(&text) {
                let mut s = m.as_str();
                // Trim trailing punctuation and unbalanced closing brackets.
                while let Some(last) = s.chars().last() {
                    let unbalanced = |o: char, c: char| last == c && s.matches(o).count() < s.matches(c).count();
                    if ".,;:!?'\"".contains(last) || unbalanced('(', ')') || unbalanced('[', ']') || unbalanced('{', '}') || last == '>' {
                        s = &s[..s.len() - last.len_utf8()];
                    } else {
                        break;
                    }
                }
                if s.len() < 8 {
                    continue;
                }
                let url: Arc<str> = if s.starts_with("www.") { format!("https://{s}").into() } else { s.into() };
                let (b0, b1) = (m.start(), m.start() + s.len() - 1);
                // Split the match into one span per screen row.
                let mut i = b0;
                while i <= b1 {
                    let (sr, sc) = map[i];
                    let mut j = i;
                    while j < b1 && map[j + 1].0 == sr {
                        j += 1;
                    }
                    self.links.push(LinkSpan { row: sr, c0: sc, c1: map[j].1, url: url.clone() });
                    i = j + 1;
                }
            }
            row = r;
        }
        // OSC 8 hyperlinks.
        for r in 0..lines {
            let mut c = 0;
            while c < cols {
                if let Some(url) = self.cells[r * cols + c].link.clone() {
                    let start = c;
                    while c + 1 < cols && self.cells[r * cols + c + 1].link.as_deref() == Some(&*url) {
                        c += 1;
                    }
                    if !self.links.iter().any(|l| l.row == r && l.c0 <= start && l.c1 >= start) {
                        self.links.push(LinkSpan { row: r, c0: start, c1: c, url });
                    }
                }
                c += 1;
            }
        }
    }

    fn grid_pos(&self, p: Pos2, origin: Pos2) -> (usize, usize, Side) {
        let x = (p.x - origin.x) / self.cell.x;
        let y = (p.y - origin.y) / self.cell.y;
        let col = (x.floor().max(0.0) as usize).min(self.cols - 1);
        let row = (y.floor().max(0.0) as usize).min(self.lines - 1);
        let side = if x - x.floor() < 0.5 { Side::Left } else { Side::Right };
        (row, col, side)
    }

    fn link_at(&self, p: Pos2, origin: Pos2) -> Option<&LinkSpan> {
        let (row, col, _) = self.grid_pos(p, origin);
        self.links.iter().find(|l| l.row == row && l.c0 <= col && col <= l.c1)
    }

    fn point(&self, row: usize, col: usize, offset: usize) -> Point {
        Point::new(Line(row as i32 - offset as i32), Column(col))
    }

    // ------------------------------------------------------------ input

    fn keyboard(&mut self, ui: &egui::Ui) {
        let (events, mods) = ui.input(|i| (i.events.clone(), i.modifiers));
        let mode = *self.term.lock().mode();
        let mac = cfg!(target_os = "macos");
        let mut typed = false;
        for e in events {
            match e {
                egui::Event::Text(t) => {
                    if std::mem::take(&mut self.skip_text) {
                        continue;
                    }
                    self.write(t.into_bytes());
                    typed = true;
                }
                egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                    self.write(t.into_bytes());
                    typed = true;
                }
                egui::Event::Copy => {
                    // Cmd+C / Ctrl+Shift+C copy. Plain Ctrl+C copies too while text is
                    // selected (and clears the selection), otherwise it interrupts.
                    // (Shift is not reliable here: a quick chord can be released
                    // before the frame that sees the event.)
                    match self.selection_text() {
                        Some(s) => {
                            ui.ctx().copy_text(s);
                            self.term.lock().selection = None;
                        }
                        None if mac => {}
                        None => {
                            self.write(&b"\x03"[..]);
                            typed = true;
                        }
                    }
                }
                egui::Event::Cut => {
                    if mac {
                        if let Some(s) = self.selection_text() {
                            ui.ctx().copy_text(s);
                        }
                    } else {
                        self.write(&b"\x18"[..]);
                        typed = true;
                    }
                }
                egui::Event::Paste(s) => {
                    // Ctrl+V pastes too, except in full-screen programs (vim's
                    // block mode) where plain Ctrl+V is passed through.
                    if mac || mods.shift || !mode.contains(TermMode::ALT_SCREEN) {
                        self.paste(&s);
                    } else {
                        self.write(&b"\x16"[..]);
                        typed = true;
                    }
                }
                egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                    // Scrollback navigation stays local unless a full-screen app runs.
                    if m.shift && !m.ctrl && !mode.contains(TermMode::ALT_SCREEN) {
                        let s = match key {
                            Key::PageUp => Some(Scroll::PageUp),
                            Key::PageDown => Some(Scroll::PageDown),
                            Key::Home => Some(Scroll::Top),
                            Key::End => Some(Scroll::Bottom),
                            _ => None,
                        };
                        if let Some(s) = s {
                            self.term.lock().scroll_display(s);
                            continue;
                        }
                    }
                    if mac && m.mac_cmd {
                        continue; // app shortcuts
                    }
                    if let Some(bytes) = key_bytes(key, m, mode, mac) {
                        if m.alt && !m.ctrl && !mac && is_text_key(key) {
                            self.skip_text = true;
                        }
                        self.write(bytes);
                        typed = true;
                    }
                }
                _ => {}
            }
        }
        if typed {
            self.scroll_to_bottom();
        }
    }

    fn mouse(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: Rect, origin: Pos2, o: &TermOpts, out: &mut TermResponse) {
        let mode = *self.term.lock().mode();
        let (mods, zoom, events, hover) = ui.input(|i| (i.modifiers, i.zoom_delta(), i.events.clone(), i.pointer.hover_pos()));
        // Scrolled lines, like Ghostty: a row of trackpad travel and a mouse wheel notch
        // each scroll their own number of lines (1 and 3 by default). With Shift held
        // egui reports the wheel as horizontal, so take x then.
        let scroll: f32 = events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { unit, delta, modifiers } => {
                    let d = if delta.y == 0.0 && modifiers.shift { delta.x } else { delta.y };
                    match unit {
                        egui::MouseWheelUnit::Point => d / self.cell.y.max(1.0) * o.scroll[0],
                        egui::MouseWheelUnit::Line => d * o.scroll[1],
                        egui::MouseWheelUnit::Page => d * self.lines as f32,
                    }
                }
                _ => 0.0,
            })
            .sum();
        let hovered = hover.is_some_and(|p| rect.contains(p)) && resp.hovered();
        let offset = self.term.lock().grid().display_offset();
        let reporting = mode.intersects(TermMode::MOUSE_MODE) && !mods.shift;

        // Ctrl+wheel (pinch) changes the font size.
        if hovered && zoom != 1.0 {
            out.font_delta += if zoom > 1.0 { 1.0 } else { -1.0 };
            return;
        }

        if hovered && scroll != 0.0 {
            self.scroll_acc += scroll;
            let n = self.scroll_acc.trunc() as i32;
            if n != 0 {
                self.scroll_acc -= n as f32;
                if reporting {
                    let (row, col, _) = self.grid_pos(hover.unwrap_or(origin), origin);
                    for _ in 0..n.unsigned_abs().min(10) {
                        self.report(if n > 0 { 64 } else { 65 }, row, col, true, mode, mods);
                    }
                } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
                    let seq: &[u8] = match (n > 0, mode.contains(TermMode::APP_CURSOR)) {
                        (true, true) => b"\x1bOA",
                        (true, false) => b"\x1b[A",
                        (false, true) => b"\x1bOB",
                        (false, false) => b"\x1b[B",
                    };
                    let mut v = Vec::new();
                    for _ in 0..n.unsigned_abs().min(30) {
                        v.extend_from_slice(seq);
                    }
                    self.write(v);
                } else {
                    self.term.lock().scroll_display(Scroll::Delta(n));
                }
            }
        }

        // Mouse reporting to programs (vim, htop, tmux…). Shift bypasses it.
        if reporting {
            for e in &events {
                match e {
                    egui::Event::PointerButton { pos, button, pressed, .. } if rect.contains(*pos) || self.report_button.is_some() => {
                        let b = match button {
                            PointerButton::Primary => 0,
                            PointerButton::Middle => 1,
                            PointerButton::Secondary => 2,
                            _ => continue,
                        };
                        let (row, col, _) = self.grid_pos(*pos, origin);
                        self.report(b, row, col, *pressed, mode, mods);
                        self.report_button = pressed.then_some(b);
                    }
                    egui::Event::PointerMoved(pos) if rect.contains(*pos) => {
                        let (row, col, _) = self.grid_pos(*pos, origin);
                        if self.last_report_cell == Some((row, col)) {
                            continue;
                        }
                        self.last_report_cell = Some((row, col));
                        if let Some(b) = self.report_button {
                            if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
                                self.report(b + 32, row, col, true, mode, mods);
                            }
                        } else if mode.contains(TermMode::MOUSE_MOTION) {
                            self.report(35, row, col, true, mode, mods);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }

        // Local selection.
        let link_mod = mods.command || mods.ctrl;
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if link_mod {
                    if let Some(l) = self.link_at(p, origin) {
                        let url = l.url.to_string();
                        open_link(&url, out);
                        return;
                    }
                }
                let (row, col, side) = self.grid_pos(p, origin);
                let point = self.point(row, col, offset);
                let mut t = self.term.lock();
                if mods.shift {
                    if let Some(sel) = t.selection.as_mut() {
                        sel.update(point, side);
                    }
                } else {
                    t.selection = None;
                }
            }
        }
        if resp.double_clicked() || resp.triple_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (row, col, side) = self.grid_pos(p, origin);
                let point = self.point(row, col, offset);
                let ty = if resp.triple_clicked() { SelectionType::Lines } else { SelectionType::Semantic };
                self.term.lock().selection = Some(Selection::new(ty, point, side));
                self.after_select(ui, o);
            }
        }
        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                let (row, col, side) = self.grid_pos(p, origin);
                let point = self.point(row, col, offset);
                let ty = if mods.alt { SelectionType::Block } else { SelectionType::Simple };
                self.term.lock().selection = Some(Selection::new(ty, point, side));
                self.selecting = true;
            }
        }
        if self.selecting {
            if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                // Auto-scroll when dragging past the top or bottom edge.
                if p.y < rect.top() {
                    self.term.lock().scroll_display(Scroll::Delta(1));
                    ui.ctx().request_repaint_after(Duration::from_millis(30));
                } else if p.y > rect.bottom() {
                    self.term.lock().scroll_display(Scroll::Delta(-1));
                    ui.ctx().request_repaint_after(Duration::from_millis(30));
                }
                let offset = self.term.lock().grid().display_offset();
                let (row, col, side) = self.grid_pos(p, origin);
                let point = self.point(row, col, offset);
                if let Some(sel) = self.term.lock().selection.as_mut() {
                    sel.update(point, side);
                }
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                self.selecting = false;
                self.after_select(ui, o);
            }
        }
        // Middle click pastes (the primary selection on Linux).
        if resp.middle_clicked() {
            if let Some(text) = clipboard_get(cfg!(target_os = "linux")).or_else(|| clipboard_get(false)) {
                self.paste(&text);
            }
        }
    }

    fn after_select(&self, ui: &egui::Ui, o: &TermOpts) {
        let Some(text) = self.selection_text() else { return };
        if cfg!(target_os = "linux") {
            clipboard_set_primary(&text);
        }
        if o.copy_on_select {
            ui.ctx().copy_text(text);
        }
    }

    fn report(&self, button: u8, row: usize, col: usize, pressed: bool, mode: TermMode, m: Modifiers) {
        let mut b = button;
        if m.shift {
            b += 4;
        }
        if m.alt {
            b += 8;
        }
        if m.ctrl {
            b += 16;
        }
        if mode.contains(TermMode::SGR_MOUSE) {
            let s = format!("\x1b[<{};{};{}{}", b, col + 1, row + 1, if pressed { 'M' } else { 'm' });
            self.write(s.into_bytes());
        } else {
            let b = if pressed { b } else { 3 + (b & !3) };
            let enc = |v: usize| (32 + 1 + v).min(255) as u8;
            self.write(vec![0x1b, b'[', b'M', 32 + b, enc(col), enc(row)]);
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, o: &TermOpts, out: &mut TermResponse) {
        use crate::i18n::t;
        let mac = cfg!(target_os = "macos");
        let k = |linux: &str, macos: &str| if mac { macos.to_string() } else { linux.to_string() };
        if let Some(url) = self.menu_link.clone() {
            if ui.button(t("Open link")).clicked() {
                open_link(&url, out);
                ui.close_menu();
            }
            if ui.button(t("Copy link")).clicked() {
                ui.ctx().copy_text(url.to_string());
                ui.close_menu();
            }
            ui.separator();
        }
        let has_sel = self.selection_text().is_some();
        if ui.add_enabled(has_sel, egui::Button::new(t("Copy")).shortcut_text(k("Ctrl+Shift+C", "⌘C"))).clicked() {
            if let Some(s) = self.selection_text() {
                ui.ctx().copy_text(s);
            }
            ui.close_menu();
        }
        if ui.add(egui::Button::new(t("Paste")).shortcut_text(k("Ctrl+Shift+V", "⌘V"))).clicked() {
            if let Some(s) = clipboard_get(false) {
                self.paste(&s);
            }
            ui.close_menu();
        }
        if ui.button(t("Select all")).clicked() {
            self.select_all();
            ui.close_menu();
        }
        ui.separator();
        if ui.button(t("Clear")).clicked() {
            self.clear();
            ui.close_menu();
        }
        if ui.button(t("Open folder in a file tab")).clicked() {
            out.open_dir = Some(self.cwd.clone());
            ui.close_menu();
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(t("Font size"));
            if ui.small_button("−").clicked() {
                out.font_delta -= 1.0;
            }
            if ui.small_button("+").clicked() {
                out.font_delta += 1.0;
            }
        });
        ui.separator();
        if o.can_split.iter().any(|&b| b) {
            use crate::layout::SplitDir as D;
            ui.menu_button(t("Layout"), |ui| {
                use crate::shortcuts::{terminal_hint, Action as K};
                for (k, (d, label, key)) in [(D::Right, t("Split right"), K::SplitRight), (D::Left, t("Split left"), K::SplitLeft), (D::Down, t("Split down"), K::SplitDown), (D::Up, t("Split up"), K::SplitUp)].into_iter().enumerate() {
                    if o.can_split[k] && ui.add(egui::Button::new(label).shortcut_text(terminal_hint(key))).clicked() {
                        out.split = Some((d, true));
                        ui.close_menu();
                    }
                }
                ui.separator();
                if o.can_split[0] && ui.button(t("Files to the right")).clicked() {
                    out.split = Some((D::Right, false));
                    ui.close_menu();
                }
                if o.can_split[2] && ui.button(t("Files below")).clicked() {
                    out.split = Some((D::Down, false));
                    ui.close_menu();
                }
            });
        }
        let close = if o.in_split { t("Close window") } else { t("Close tab") };
        if ui.add(egui::Button::new(close).shortcut_text(crate::shortcuts::terminal_hint(crate::shortcuts::Action::CloseWindow))).clicked() {
            out.close = true;
            ui.close_menu();
        }
    }
}

fn open_link(url: &str, out: &mut TermResponse) {
    if let Some(p) = url.strip_prefix("file://") {
        let path = PathBuf::from(p.trim_start_matches("localhost"));
        if path.is_dir() {
            out.open_dir = Some(path);
            return;
        }
    }
    let _ = open::that_detached(url);
}

/// The command name of process `pid`, if the system tells.
fn process_name(pid: u32) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer is valid for its length; proc_name writes at most that much.
        let n = unsafe { libc::proc_name(pid as libc::c_int, buf.as_mut_ptr().cast(), buf.len() as u32) };
        (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

fn next_id() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}


fn is_text_key(k: Key) -> bool {
    letter(k).is_some() || digit(k).is_some() || matches!(k, Key::Space | Key::Minus | Key::Period | Key::Comma | Key::Slash | Key::Semicolon | Key::Equals | Key::OpenBracket | Key::CloseBracket | Key::Backslash | Key::Backtick | Key::Quote)
}

fn letter(k: Key) -> Option<u8> {
    let n = k.name();
    (n.len() == 1 && n.as_bytes()[0].is_ascii_uppercase()).then(|| n.as_bytes()[0])
}

fn digit(k: Key) -> Option<u8> {
    use Key::*;
    Some(match k {
        Num0 => b'0',
        Num1 => b'1',
        Num2 => b'2',
        Num3 => b'3',
        Num4 => b'4',
        Num5 => b'5',
        Num6 => b'6',
        Num7 => b'7',
        Num8 => b'8',
        Num9 => b'9',
        _ => return None,
    })
}

/// Bytes sent for a key press (xterm conventions). `None` = handled as text.
fn key_bytes(key: Key, m: Modifiers, mode: TermMode, mac: bool) -> Option<Vec<u8>> {
    let modn = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let cursor = |f: char| -> Vec<u8> {
        if modn > 1 {
            format!("\x1b[1;{modn}{f}")
        } else if app_cursor {
            format!("\x1bO{f}")
        } else {
            format!("\x1b[{f}")
        }
        .into_bytes()
    };
    let tilde = |n: u8| -> Vec<u8> { if modn > 1 { format!("\x1b[{n};{modn}~") } else { format!("\x1b[{n}~") }.into_bytes() };
    let ss3 = |f: char| -> Vec<u8> { if modn > 1 { format!("\x1b[1;{modn}{f}") } else { format!("\x1bO{f}") }.into_bytes() };
    let esc = |b: &[u8]| -> Vec<u8> { if m.alt { [&[0x1b], b].concat() } else { b.to_vec() } };
    Some(match key {
        Key::ArrowUp => cursor('A'),
        Key::ArrowDown => cursor('B'),
        Key::ArrowRight => cursor('C'),
        Key::ArrowLeft => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => ss3('P'),
        Key::F2 => ss3('Q'),
        Key::F3 => ss3('R'),
        Key::F4 => ss3('S'),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        Key::Enter => esc(b"\r"),
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => esc(b"\t"),
        Key::Backspace if m.ctrl => esc(b"\x08"),
        Key::Backspace => esc(b"\x7f"),
        Key::Escape => b"\x1b".to_vec(),
        _ if m.ctrl => {
            let c = if let Some(l) = letter(key) {
                l - b'A' + 1
            } else {
                match key {
                    Key::Space | Key::Num2 => 0,
                    Key::OpenBracket | Key::Num3 => 0x1b,
                    Key::Backslash | Key::Num4 => 0x1c,
                    Key::CloseBracket | Key::Num5 => 0x1d,
                    Key::Num6 => 0x1e,
                    Key::Minus | Key::Slash | Key::Num7 => 0x1f,
                    Key::Num8 => 0x7f,
                    _ => return None,
                }
            };
            esc(&[c])
        }
        _ if m.alt && !mac => {
            let c = if let Some(l) = letter(key) {
                if m.shift { l } else { l.to_ascii_lowercase() }
            } else if let Some(d) = digit(key) {
                d
            } else {
                match key {
                    Key::Period => b'.',
                    Key::Comma => b',',
                    Key::Minus => b'-',
                    Key::Slash => b'/',
                    Key::Semicolon => b';',
                    Key::Equals => b'=',
                    Key::Backtick => b'`',
                    Key::Quote => b'\'',
                    Key::OpenBracket => b'[',
                    Key::CloseBracket => b']',
                    Key::Backslash => b'\\',
                    Key::Space => b' ',
                    _ => return None,
                }
            };
            vec![0x1b, c]
        }
        _ => return None,
    })
}

// ------------------------------------------------------------ colours

struct Palette {
    fg: Color32,
    bg: Color32,
    cursor: Color32,
    selection: Color32,
    ansi: &'static [u32; 16],
}

const DARK_ANSI: [u32; 16] = [
    0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, //
    0x6c7086, 0xf7a1b8, 0xb9eab5, 0xfbe9c0, 0xa5c5fb, 0xf8d2ec, 0xabeadf, 0xdee2f0,
];
const LIGHT_ANSI: [u32; 16] = [
    0x24292f, 0xcf222e, 0x116329, 0x9a6700, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781, //
    0x57606a, 0xa40e26, 0x1a7f37, 0x7d4e00, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
];

fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Colour of palette entry `i` (0..=255).
fn palette_color(i: usize, ansi: &[u32; 16]) -> Color32 {
    match i {
        0..=15 => hex(ansi[i]),
        16..=231 => {
            let i = i - 16;
            let v = |x: usize| if x == 0 { 0 } else { (55 + 40 * x) as u8 };
            Color32::from_rgb(v(i / 36), v((i / 6) % 6), v(i % 6))
        }
        232..=255 => {
            let g = (8 + 10 * (i - 232)) as u8;
            Color32::from_rgb(g, g, g)
        }
        _ => Color32::GRAY,
    }
}

impl Palette {
    fn new(th: &Theme) -> Self {
        Self {
            fg: th.text,
            bg: th.bg,
            cursor: th.accent,
            selection: blend(th.bg, th.accent, if th.dark { 0.35 } else { 0.28 }),
            ansi: th.ansi.unwrap_or(if th.dark { &DARK_ANSI } else { &LIGHT_ANSI }),
        }
    }

    fn resolve(&self, c: AColor, colors: &Colors, is_fg: bool) -> Color32 {
        let rgb = |r: Rgb| Color32::from_rgb(r.r, r.g, r.b);
        match c {
            AColor::Spec(r) => rgb(r),
            AColor::Indexed(i) => colors[i as usize].map(rgb).unwrap_or_else(|| palette_color(i as usize, self.ansi)),
            AColor::Named(n) => {
                if let Some(r) = colors[n] {
                    return rgb(r);
                }
                match n {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                    NamedColor::Background => self.bg,
                    NamedColor::Cursor => self.cursor,
                    NamedColor::DimForeground => blend(self.fg, self.bg, 0.4),
                    n => {
                        let i = n as usize;
                        if i < 16 {
                            palette_color(i, self.ansi)
                        } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) {
                            blend(palette_color(i - NamedColor::DimBlack as usize, self.ansi), self.bg, 0.35)
                        } else if is_fg {
                            self.fg
                        } else {
                            self.bg
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------ previews

/// A small made-up terminal session in the colours of `th`, with the terminal font.
pub fn preview(ui: &mut egui::Ui, th: &Theme, font_size: f32, width: f32) -> egui::Response {
    use egui::{pos2, CornerRadius, FontFamily};
    const RED: usize = 1;
    const GREEN: usize = 2;
    const YELLOW: usize = 3;
    const BLUE: usize = 4;
    let pal = Palette::new(th);
    let ansi = |i: usize| hex(pal.ansi[i]);
    let font = FontId::new(font_size, FontFamily::Name("term".into()));
    let bold = FontId::new(font_size, FontFamily::Name("term-bold".into()));
    let lines: [&[(&str, Color32, bool)]; 6] = [
        &[("~/projects ", ansi(BLUE), true), ("$ ", ansi(GREEN), true), ("ls -l", pal.fg, false)],
        &[("drwxr-xr-x  ", pal.fg, false), ("assets", ansi(BLUE), true)],
        &[("-rwxr-xr-x  ", pal.fg, false), ("build.sh", ansi(GREEN), true)],
        &[("-rw-r--r--  ", pal.fg, false), ("README.md", pal.fg, false)],
        &[("[ok] ", ansi(GREEN), false), ("build  ", pal.fg, false), ("[!] ", ansi(YELLOW), false), ("2 warnings  ", pal.fg, false), ("[x] ", ansi(RED), false), ("0 errors", pal.fg, false)],
        &[("~/projects ", ansi(BLUE), true), ("$ ", ansi(GREEN), true)],
    ];
    let (cell_w, row_h) = ui.fonts(|f| (f.layout_no_wrap("M".repeat(100), font.clone(), pal.fg).size().x / 100.0, f.row_height(&font).ceil()));
    let swatch = (font_size * 0.9).max(10.0);
    let height = row_h * lines.len() as f32 + swatch * 2.0 + 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(8), pal.bg);
    let origin = rect.min + vec2(12.0, 10.0);
    for (y, line) in lines.iter().enumerate() {
        let mut col = 0usize;
        for (text, color, is_bold) in line.iter() {
            let at = pos2(origin.x + col as f32 * cell_w, origin.y + y as f32 * row_h);
            p.text(at, egui::Align2::LEFT_TOP, text, if *is_bold { bold.clone() } else { font.clone() }, *color);
            col += text.chars().count();
        }
        if y == lines.len() - 1 {
            let at = pos2(origin.x + col as f32 * cell_w, origin.y + y as f32 * row_h);
            p.rect_filled(egui::Rect::from_min_size(at, vec2(cell_w, row_h)), 0.0, pal.cursor);
        }
    }
    // The 16 colours, as a terminal's palette table shows them.
    let top = origin.y + lines.len() as f32 * row_h + 8.0;
    for (i, c) in pal.ansi.iter().enumerate() {
        let at = pos2(origin.x + (i % 8) as f32 * (swatch + 4.0), top + (i / 8) as f32 * (swatch + 2.0));
        p.rect_filled(egui::Rect::from_min_size(at, vec2(swatch, swatch)), 2.0, hex(*c));
    }
    resp
}

// ------------------------------------------------------------ clipboard

fn clipboard() -> &'static Mutex<Option<arboard::Clipboard>> {
    // Kept alive for the whole run: on X11 the selection is served from it.
    static CB: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
    CB.get_or_init(|| Mutex::new(arboard::Clipboard::new().ok()))
}

/// Read the clipboard (or the primary selection on Linux).
pub fn clipboard_get(primary: bool) -> Option<String> {
    let mut g = clipboard().lock().ok()?;
    let cb = g.as_mut()?;
    #[cfg(target_os = "linux")]
    if primary {
        use arboard::{GetExtLinux, LinuxClipboardKind};
        return cb.get().clipboard(LinuxClipboardKind::Primary).text().ok().filter(|s| !s.is_empty());
    }
    let _ = primary;
    cb.get_text().ok().filter(|s| !s.is_empty())
}

fn clipboard_set_primary(_text: &str) {
    #[cfg(target_os = "linux")]
    {
        use arboard::{LinuxClipboardKind, SetExtLinux};
        if let Ok(mut g) = clipboard().lock() {
            if let Some(cb) = g.as_mut() {
                let _ = cb.set().clipboard(LinuxClipboardKind::Primary).text(_text.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let none = TermMode::empty();
        let ctrl = Modifiers { ctrl: true, command: true, ..Default::default() };
        let alt = Modifiers { alt: true, ..Default::default() };
        assert_eq!(key_bytes(Key::C, ctrl, none, false), Some(vec![3]));
        assert_eq!(key_bytes(Key::ArrowUp, Modifiers::NONE, none, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(Key::ArrowUp, Modifiers::NONE, TermMode::APP_CURSOR, false), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes(Key::ArrowLeft, ctrl, none, false), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(key_bytes(Key::B, alt, none, false), Some(b"\x1bb".to_vec()));
        assert_eq!(key_bytes(Key::A, Modifiers::NONE, none, false), None, "plain letters arrive as text");
        assert_eq!(key_bytes(Key::F5, Modifiers::NONE, none, false), Some(b"\x1b[15~".to_vec()));
        assert_eq!(key_bytes(Key::Tab, Modifiers::SHIFT, none, false), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn palette() {
        assert_eq!(palette_color(1, &DARK_ANSI), hex(DARK_ANSI[1]));
        assert_eq!(palette_color(16, &DARK_ANSI), Color32::from_rgb(0, 0, 0));
        assert_eq!(palette_color(231, &DARK_ANSI), Color32::from_rgb(255, 255, 255));
        assert_eq!(palette_color(232, &DARK_ANSI), Color32::from_rgb(8, 8, 8));
    }
}
