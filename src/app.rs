//! The main application: layout, keyboard handling, dialogs and background jobs.
use crate::archive::{self, ArchiveFormat};
use crate::backup::{self, BackupAction, BackupUi};
use crate::config::{Bookmark, ConnectionProfile, Protocol, Settings};
use crate::diskusage::DuAction;
use crate::i18n::{self, t};
use crate::icons;
use crate::palette::{self, Act, PalItem, Palette};
use crate::ops::{self, Conflict, Endpoint, Job};
use crate::layout::{Layout, SplitDir};
use crate::pane::{pane_ui, Cmd, ListCtx, Pane, PaneEvent, PaneUiCtx, Tab};
use crate::tabs::{Strip, TabEvent, TabInfo, Workspace};
use crate::termview::TermSession;
use crate::preview::{self, Preview};
use crate::shortcuts::{self, hint, Action, Chord};
use crate::rename::{self as ren, BatchRename, RenameAction};
use crate::remote::{self, RemoteFs, SharedRemote};
use crate::terminal::{self, TerminalKind};
use crate::theme::{self, Theme};
use crate::util::{es, human_size, join_slash, Res};
use crate::vfs::{Entry, Location};
use egui::{Key, Modifiers, RichText};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Build a `ListCtx` from disjoint fields so that panes can be borrowed mutably.
macro_rules! lc {
    ($s:expr) => {
        ListCtx { remotes: &$s.remotes, show_hidden: $s.cfg.show_hidden, dirs_first: $s.cfg.dirs_first }
    };
}

/// Every window of every tab: the shown tab's and the parked ones in the strip.
macro_rules! all_panes {
    ($s:expr) => {
        $s.panes.iter_mut().chain($s.strip.tabs.iter_mut().flat_map(|w| w.panes.iter_mut()))
    };
}

enum Dialog {
    Transfer { src: Location, mv: bool, items: Vec<Entry>, dst: String, policy: Conflict },
    /// `loc`: the folder it works in, and `view` the window it was asked from (kept, so
    /// that the dialog acts there even if the window shows another folder meanwhile).
    NewItem { name: String, file: bool, loc: Location, view: u64 },
    Rename { old: String, new: String, loc: Location, view: u64 },
    Delete { items: Vec<Entry>, trash: bool, loc: Location, view: u64 },
    Pack { items: Vec<Entry>, src_dir: PathBuf, name: String, dest_dir: String, format: ArchiveFormat, level: u32 },
    Unpack { archive: PathBuf, dest: String, selection: Vec<String>, strip: String, policy: Conflict },
    OpenWith { path: PathBuf, program: String, always: bool },
    /// Asks for an archive's password, tries it in the background, then goes on with `then`.
    ArchivePassword { file: PathBuf, password: String, wrong: bool, checking: Option<Receiver<bool>>, then: AfterPassword },
    Connect { profile: ConnectionProfile, save: bool, save_password: bool },
    /// An SFTP server whose host key is not known yet: trust it, or do not connect.
    TrustHostKey { profile: ConnectionProfile, fingerprint: String },
    Message { title: String, text: String },
    /// Replace every setting with those read from `path`.
    ImportSettings { path: PathBuf, settings: Box<Settings> },
    /// Closing terminals that are running programs (named in `programs`).
    CloseTerminals { target: CloseTarget, programs: Vec<String> },
}

/// What a confirmed `Dialog::CloseTerminals` closes.
enum CloseTarget {
    /// A window of the shown tab, by its view uid.
    Window(u64),
    Tabs(Vec<u64>),
}

enum After {
    Refresh,
    OpenFile(PathBuf),
    OpenWithDialog(PathBuf),
    /// The changes of temporary copy (id) were uploaded, as they were then.
    Uploaded(u64, crate::tempcopy::Stamp),
}

struct RunningJob {
    job: Job,
    after: After,
    dest: Option<Location>,
    /// Local files and folders the job reads or writes (a drive in use is not ejected).
    paths: Vec<PathBuf>,
    /// Smoothed transfer speed in bytes/s and the last (time, bytes) sample.
    speed: f64,
    sample: (Instant, u64),
    /// Time spent paused or queued, excluded from the elapsed time.
    started_running: Option<Instant>,
}

/// Where quitting stands while transfers are running.
#[derive(Clone, Copy, PartialEq)]
enum Quit {
    No,
    /// The user tried to quit: the confirmation is showing.
    Asking,
    /// "Stop and quit": waiting since then for the cancelled jobs to clean up.
    Stopping(Instant),
    /// Nothing holds it back any more.
    Go,
}

/// How long cancelled jobs get to clean up before quitting goes ahead without them.
const QUIT_WAIT: Duration = Duration::from_secs(10);

struct FinishedJob {
    title: String,
    ok: bool,
    /// Stopped by the user: neither done nor failed.
    cancelled: bool,
    msg: String,
    /// Bytes done, or items for a job that counts items (`items`).
    bytes: u64,
    items: bool,
    secs: f32,
    dest: Option<Location>,
    at: String,
}

/// A drive being ejected: mount point, name, and the result to come.
type Ejecting = (PathBuf, String, Receiver<Result<(), String>>);

struct Toast {
    text: String,
    error: bool,
    at: Instant,
}

type ConnectResult = Result<(Box<dyn RemoteFs>, String), remote::ConnectError>;

/// A closed tab that Ctrl+Shift+T can bring back.
enum ClosedView {
    Files(Location),
    Terminal(PathBuf),
    /// A disk usage window: folder and depth.
    DiskUsage(PathBuf, usize),
    Settings,
    Transfers,
}

impl ClosedView {
    fn of(p: &Pane) -> Self {
        let tab = p.tab();
        match (&tab.term, &tab.du) {
            (Some(t), _) => ClosedView::Terminal(t.cwd.clone()),
            (_, Some(d)) => ClosedView::DiskUsage(d.path.clone(), d.depth),
            _ if tab.settings => ClosedView::Settings,
            _ if tab.transfers => ClosedView::Transfers,
            // A Checksum or Batch rename tool comes back as the folder it worked in.
            _ => ClosedView::Files(tab.loc.clone()),
        }
    }
}

/// A closed tab (or window) for "Reopen closed tab".
struct ClosedTab {
    views: Vec<ClosedView>,
    layout: Layout,
    title: Option<String>,
}

pub struct App {
    cfg: Settings,
    theme: &'static Theme,
    /// The tab bar. The shown tab's windows are not in it but in `panes`, `layout`
    /// and `active` below (see `sync_tab`).
    strip: Strip,
    /// Uid of the tab whose windows are in `panes`.
    live: u64,
    /// Windows of the shown tab.
    panes: Vec<Pane>,
    layout: Layout,
    /// The previously focused pane (target of "other panel" commands).
    prev_active: usize,
    known_shells: Option<Vec<String>>,
    confirm_reset: bool,
    last_active: usize,
    active: usize,
    remotes: HashMap<u64, (String, SharedRemote)>,
    next_remote: u64,
    drives: Vec<crate::drives::Drive>,
    /// Keeps `drives` up to date off the UI thread.
    drive_watch: crate::drives::Watcher,
    ejecting: Vec<Ejecting>,
    jobs: Vec<RunningJob>,
    dialog: Option<Dialog>,
    dialog_focus: bool,
    /// Dialogs that came up while another was open (and whether their field takes the
    /// keyboard): each shows when the one before it closes.
    pending_dialogs: std::collections::VecDeque<(Dialog, bool)>,
    /// Settings changed by a drag or typing, saved once that is over (since when).
    cfg_dirty: Option<Instant>,
    quick: Option<Preview>,
    /// When the terminal text size last changed (pinch, buttons): saved once it settles.
    term_font_saved_at: Option<Instant>,
    viewer: Option<Preview>,
    backup: BackupUi,
    show_help: bool,
    show_about: bool,
    /// An update check under way, and whether the user asked for it (the menu).
    update_check: Option<(std::sync::mpsc::Receiver<crate::util::Res<crate::update::Outcome>>, bool)>,
    /// A newer release found, with its notes ready to show.
    update_found: Option<(crate::update::Release, Vec<crate::preview::MdBlock>)>,
    toasts: Vec<Toast>,
    connecting: Option<(ConnectionProfile, bool, Receiver<ConnectResult>)>,
    languages: Vec<(String, String)>,
    title: String,
    max_texture: usize,
    copy_text: Option<String>,
    palette: Palette,
    history: Vec<FinishedJob>,
    quit: Quit,
    /// Valid drop target under the pointer this frame (for the drag ghost).
    drag_target: Option<Location>,
    shift_down: bool,
    pane_rects: Vec<(usize, egui::Rect)>,
    /// The drives shown in the sidebar: mount point, name, and where its eject button was drawn.
    drive_rows: Vec<(PathBuf, String, Option<egui::Rect>)>,
    /// Exact destination of a drag & drop awaiting confirmation.
    dialog_dst: Option<Location>,
    /// The suggestions and folder tree of a dialog's folder field (Copy / Move to,
    /// Compress's Save to, Extract to).
    dialog_path: crate::pathfield::PathField,
    logo_large: Option<egui::TextureHandle>,
    icon_key: String,
    icon_previews: HashMap<String, icons::IconSet>,
    icon_themes: Option<(Vec<String>, Option<String>)>,
    egui_ctx: egui::Context,
    closed_tabs: Vec<ClosedTab>,
    /// AI coding agents found on this computer (`None`: not looked for yet), and
    /// the search under way.
    agents: Option<Vec<crate::agents::Found>>,
    agents_rx: Option<Receiver<Vec<crate::agents::Found>>>,
    /// Live filter while typing in the search box: (pane, tab uid, filter before, the
    /// cursor's entry before).
    pal_filter: Option<(usize, u64, String, Option<String>)>,
    /// Typing in a file window: a small search box that filters it as you type.
    type_search: Option<TypeSearch>,
    settings_page: SettingsPage,
    /// File types added on the Default apps page that have no app yet.
    default_app_types: Vec<String>,
    /// Text in that page's "add a file type" field.
    default_app_new: String,
    /// Suggestions and the folder tree of the bookmarks' Location fields.
    bookmark_paths: crate::pathfield::PathField,
    /// That page is asking whether to reset all its choices.
    confirm_reset_apps: bool,
    /// The shortcut Settings waits for a key press for: the action, and which of its keys to replace.
    shortcut_capture: Option<(Action, Option<usize>)>,
    /// What came of the last key press there.
    shortcut_note: Option<String>,
    /// Text in the Shortcuts page's search field.
    shortcut_search: String,
    /// That page is asking whether to reset all shortcuts.
    confirm_reset_shortcuts: bool,
    /// The tab shown before this one.
    last_used_tab: Option<u64>,
    /// The terminal profile being edited in Settings.
    profile_sel: usize,
    /// Text in the font dropdown's search field.
    font_search: String,
    /// The font settings in use (see `apply_fonts`).
    fonts_applied: crate::fonts::FontPrefs,
    chrome: crate::chrome::Chrome,
    /// Width of the toolbar's right-hand group last frame (keeps the search box clear of it).
    toolbar_right_w: f32,
    /// Listens for later launches (single instance); `None` when another Matrix Finder does.
    instance: Option<crate::instance::Server>,
    /// The settings file: saves merge with what another Matrix Finder saved meanwhile.
    store: crate::config::Store,
    /// Files from servers and archives opened in other apps through a temporary copy.
    temp_copies: crate::tempcopy::Copies,
    /// Edited copies (ids) asked about: upload the changes, or keep the copy.
    copy_prompts: Vec<u64>,
}

/// What to go on with when an archive's password has been accepted.
enum AfterPassword {
    /// Show the files in it.
    Browse(PathBuf),
    /// Quick View of what the cursor is on.
    Preview,
    Command(Cmd),
    /// A dialog that was confirmed before the password was asked for.
    Confirm(Box<Dialog>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SettingsPage {
    Appearance,
    Fonts,
    Language,
    Files,
    Bookmarks,
    QuickFilters,
    DefaultApps,
    Shortcuts,
    Terminal,
    Backup,
}

impl App {
    /// `open`: a folder given on the command line, shown first.
    pub fn new(cc: &eframe::CreationContext, chrome: crate::chrome::Chrome, open: Option<String>) -> Self {
        let (cfg, store) = crate::config::Store::open();
        i18n::set_language(&cfg.language);
        // Previews' temporary folders left by a Matrix Finder that crashed.
        crate::util::clean_stale_temp_dirs(&[crate::media::TEMP_PREFIX]);
        let theme = theme::by_name(&cfg.theme);
        let mut cfg = cfg;
        if cfg.theme != theme::SYSTEM {
            cfg.theme = theme.name.to_string();
        }
        shortcuts::install(&cfg.shortcuts);
        // A font that is no longer installed leaves the bundled default in its place.
        let _ = crate::fonts::apply(&cc.egui_ctx, &crate::fonts::FontPrefs::of(&cfg));
        theme::apply(&cc.egui_ctx, theme);
        cc.egui_ctx.set_zoom_factor(cfg.ui_scale.clamp(0.6, 2.5));
        // Zooming by keys is the app's own (saved, and rebindable): egui's would not be
        // saved, and its Ctrl+− would reach the terminals too.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let to_locs = |v: &Vec<String>| -> Vec<Location> {
            v.iter()
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
                .map(Location::Local)
                .collect()
        };
        // Start with a single pane; splits are created on demand. By default it
        // shows the user's home folder, and no terminal is opened at start.
        let home = dirs::home_dir().filter(|h| h.is_dir()).map(|h| h.to_string_lossy().to_string());
        let start = match (&home, cfg.start_in_home) {
            (Some(h), true) => vec![h.clone()],
            _ => cfg.left_paths.clone(),
        };
        // One tab per start folder; the first one is shown.
        let mut locs = to_locs(&start);
        if locs.is_empty() {
            locs.push(Location::Local(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))));
        }
        let first = Pane::new(locs.remove(0));
        let mut tabs = vec![Workspace::new(Vec::new(), Layout::Leaf(0))];
        tabs.extend(locs.into_iter().map(|l| Workspace::single(Pane::new(l))));
        let live = tabs[0].uid;
        let panes = vec![first];
        let session = if cfg.start_in_home { None } else { cfg.session.clone() };
        let mut app = Self {
            theme,
            strip: Strip { tabs, active: 0, groups: Vec::new() },
            live,
            panes,
            active: 0,
            layout: Layout::Leaf(0),
            prev_active: 0,
            known_shells: None,
            confirm_reset: false,
            last_active: 0,
            remotes: HashMap::new(),
            next_remote: 1,
            drives: crate::drives::list(),
            drive_watch: crate::drives::Watcher::spawn(&cc.egui_ctx),
            ejecting: Vec::new(),
            jobs: vec![],
            dialog: None,
            dialog_focus: false,
            quick: None,
            term_font_saved_at: None,
            viewer: None,
            backup: BackupUi::default(),
            show_help: false,
            show_about: false,
            update_check: None,
            update_found: None,
            toasts: vec![],
            connecting: None,
            languages: i18n::available(),
            title: String::new(),
            max_texture: 8192,
            copy_text: None,
            palette: Palette::default(),
            history: vec![],
            quit: Quit::No,
            drag_target: None,
            shift_down: false,
            pane_rects: Vec::new(),
            drive_rows: Vec::new(),
            dialog_dst: None,
            dialog_path: Default::default(),
            logo_large: None,
            icon_key: String::new(),
            icon_previews: HashMap::new(),
            icon_themes: None,
            egui_ctx: cc.egui_ctx.clone(),
            closed_tabs: Vec::new(),
            agents: None,
            agents_rx: None,
            pal_filter: None,
            pending_dialogs: std::collections::VecDeque::new(),
            cfg_dirty: None,
            type_search: None,
            settings_page: SettingsPage::Appearance,
            shortcut_capture: None,
            shortcut_note: None,
            shortcut_search: String::new(),
            confirm_reset_shortcuts: false,
            last_used_tab: None,
            default_app_types: vec![],
            default_app_new: String::new(),
            bookmark_paths: Default::default(),
            confirm_reset_apps: false,
            profile_sel: 0,
            font_search: String::new(),
            fonts_applied: crate::fonts::FontPrefs::of(&cfg),
            chrome,
            toolbar_right_w: 300.0,
            // Not in tests: they must not take over the running Matrix Finder's instance file.
            instance: if cfg!(test) { None } else { crate::instance::Server::start(cc.egui_ctx.clone()) },
            cfg,
            store,
            temp_copies: Default::default(),
            copy_prompts: Vec::new(),
        };
        if let Some(note) = app.store.note.take() {
            app.tell_settings_note(note);
        }
        let restored = session.is_some();
        if let Some(s) = session {
            app.restore_session(s);
        }
        if let Some(open) = open {
            app.open_from_launch(&open, restored);
        }
        let lc = lc!(app);
        for p in all_panes!(app) {
            if p.tab().is_files() && !p.tab().is_loading() && p.tab().entries.is_empty() {
                p.tab_mut().reload(&lc);
            }
        }
        if app.cfg.check_updates && !cfg!(test) && chrono::Utc::now().timestamp() - app.cfg.update_checked >= crate::update::AUTO_EVERY_SECS {
            app.check_for_updates(false);
        }
        app
    }

    /// Ask GitHub for the latest release in the background. `manual`: the user asked,
    /// so every outcome is told; the check at start only shows a newer version.
    fn check_for_updates(&mut self, manual: bool) {
        if let Some((_, was_manual)) = &mut self.update_check {
            *was_manual |= manual;
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::update::check());
        });
        self.update_check = Some((rx, manual));
        if manual {
            self.toast(t("Checking for updates…"), false);
        }
    }

    fn poll_update(&mut self, ctx: &egui::Context) {
        let Some((rx, manual)) = &self.update_check else { return };
        let manual = *manual;
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(250));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(t("The update check stopped unexpectedly.")),
        };
        self.update_check = None;
        let checking = t("Checking for updates…");
        self.toasts.retain(|x| x.text != checking);
        use crate::update::{version_text, Outcome};
        match result {
            Ok(Outcome::Newer(r)) => {
                self.cfg.update_checked = chrono::Utc::now().timestamp();
                if manual || r.tag != self.cfg.update_skip {
                    let notes = crate::preview::parse_markdown(&r.notes, self.theme.syntax);
                    self.update_found = Some((r, notes));
                }
            }
            Ok(Outcome::UpToDate(v)) => {
                self.cfg.update_checked = chrono::Utc::now().timestamp();
                if manual {
                    self.toast(format!("{} ({})", t("Matrix Finder is up to date"), version_text(v)), false);
                }
            }
            Err(e) if manual => self.toast(e, true),
            Err(_) => {}
        }
        self.save_cfg();
    }

    /// A newer version: what is in it, and where to get it. Installing is left to the user.
    fn update_window(&mut self, ctx: &egui::Context) {
        let Some((r, notes)) = &self.update_found else { return };
        let th = self.theme;
        let mut open = true;
        let mut close = false;
        let mut skip = false;
        let asset = crate::update::asset_for(r, std::env::consts::OS, std::env::consts::ARCH);
        egui::Window::new(t("Update available")).open(&mut open).collapsible(false).resizable(false).default_width(460.0).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
            ui.label(RichText::new(format!("Matrix Finder {}", crate::update::version_text(r.version))).size(18.0).strong().color(th.strong()));
            let mut line = format!("{} {}", t("You have"), env!("CARGO_PKG_VERSION"));
            if !r.date.is_empty() {
                line.push_str(&format!(" · {} {}", t("released"), r.date));
            }
            ui.label(RichText::new(line).color(th.weak));
            if !notes.is_empty() {
                ui.add_space(6.0);
                egui::Frame::NONE.fill(th.surface).corner_radius(6).inner_margin(8.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                        crate::preview::markdown_ui(ui, notes, th);
                    });
                });
            }
            ui.add_space(6.0);
            ui.label(RichText::new(t("Matrix Finder does not install updates itself: download the new version and install it as you did before.")).size(12.0).color(th.weak));
            if asset.is_none() && cfg!(target_os = "macos") {
                // Releases are built for Apple silicon only.
                ui.label(RichText::new(t("There is no download for this Mac (Intel): build the new version with the install script, as the release page shows.")).size(12.0).color(th.weak));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if let Some(a) = asset {
                    let label = format!("{} ({})", t("Download"), crate::util::human_size(a.size));
                    if ui.button(RichText::new(label).strong()).on_hover_text(&a.name).clicked() {
                        let _ = open::that(&a.url);
                        close = true;
                    }
                }
                if ui.button(t("Open release page")).clicked() {
                    let _ = open::that(&r.url);
                    close = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t("Later")).clicked() {
                        close = true;
                    }
                    if ui.button(t("Skip this version")).on_hover_text(t("The check at start will not offer this version again")).clicked() {
                        skip = true;
                    }
                });
            });
        });
        if skip {
            self.cfg.update_skip = r.tag.clone();
            self.save_cfg();
        }
        if !open || close || skip {
            self.update_found = None;
        }
    }

    /// The pane that "other panel" commands target, if there is more than one.
    fn other(&self) -> Option<usize> {
        if self.panes.len() < 2 {
            return None;
        }
        if self.prev_active != self.active && self.prev_active < self.panes.len() {
            return Some(self.prev_active);
        }
        self.layout.leaves().into_iter().find(|&i| i != self.active)
    }

    /// The other pane, splitting the active one to the right if there is none.
    fn other_or_split(&mut self) -> usize {
        match self.other() {
            Some(o) => o,
            None => {
                let a = self.active;
                let _ = self.split_pane(a, SplitDir::Right, false);
                let new = self.active;
                self.active = a;
                self.prev_active = new;
                self.last_active = a;
                new
            }
        }
    }

    /// Split window `side`: the new half shows a terminal (`terminal`) or files, in
    /// the same folder. Refused (with a message) beyond a 2 × 2 grid.
    fn split_pane(&mut self, side: usize, dir: SplitDir, terminal: bool) -> bool {
        if !self.layout.can_split(side, dir) {
            self.toast(t("At most 2 columns and 2 rows of panes"), true);
            return false;
        }
        let folder = self.tab_dir(side);
        let pane = if terminal {
            match self.terminal_pane(None, &folder) {
                Some(p) => p,
                None => return false,
            }
        } else {
            let tab = self.panes[side].tab();
            let loc = if tab.is_terminal() { Location::Local(folder) } else { tab.loc.clone() };
            self.files_pane(loc)
        };
        self.panes.push(pane);
        let new = self.panes.len() - 1;
        self.layout.split(side, new, dir);
        self.prev_active = side;
        self.active = new;
        self.focus_window();
        self.quick = None;
        true
    }

    /// Take window `side` out of the shown tab (which keeps at least one).
    fn remove_pane(&mut self, side: usize) -> Pane {
        self.layout.remove(side);
        let pane = self.panes.remove(side);
        let fix = |i: usize| if i > side { i - 1 } else if i == side { usize::MAX } else { i };
        let leaves = self.layout.leaves();
        let prev = fix(self.prev_active);
        // Another window closing keeps the focus where it is; the focused one closing
        // gives it to the window focused before, or else the first.
        self.active = match fix(self.active) {
            usize::MAX if prev != usize::MAX => prev,
            usize::MAX => leaves[0],
            i => i,
        };
        self.prev_active = match prev {
            usize::MAX => self.active,
            i => i,
        };
        self.last_active = self.active;
        self.quick = None;
        pane
    }

    /// Programs running in the terminals among `panes`, each named once (none when
    /// closing without asking is set in Settings → Terminal).
    fn running_programs<'a>(&self, panes: impl IntoIterator<Item = &'a Pane>) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        if self.cfg.term_confirm_close {
            for name in panes.into_iter().filter_map(|p| p.tab().term.as_ref()?.running_program()) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Close window `side`, first asking when its terminal is running a program.
    fn request_close_pane(&mut self, side: usize) {
        if self.panes.len() < 2 {
            return self.request_close_tabs(&[self.live]);
        }
        let programs = self.running_programs(&self.panes[side..=side]);
        if programs.is_empty() {
            self.close_pane(side);
        } else {
            self.show_dialog(Dialog::CloseTerminals { target: CloseTarget::Window(self.panes[side].tab().uid), programs }, false);
        }
    }

    /// Close tabs `uids`, first asking when their terminals are running programs.
    fn request_close_tabs(&mut self, uids: &[u64]) {
        let panes = uids.iter().flat_map(|&uid| match self.strip.tabs.iter().find(|w| w.uid == uid) {
            _ if uid == self.live => &self.panes[..],
            Some(w) => &w.panes[..],
            None => &[],
        });
        let programs = self.running_programs(panes);
        if programs.is_empty() {
            self.close_tabs(uids);
        } else {
            self.show_dialog(Dialog::CloseTerminals { target: CloseTarget::Tabs(uids.to_vec()), programs }, false);
        }
    }

    /// Close window `side`; the last window closes the tab.
    fn close_pane(&mut self, side: usize) {
        if self.panes.len() < 2 {
            self.close_tabs(&[self.live]);
            return;
        }
        let pane = self.remove_pane(side);
        self.remember_closed(std::slice::from_ref(&pane), &Layout::Leaf(0), None);
        self.focus_window();
    }

    fn swap_with_other(&mut self) {
        if let Some(o) = self.other() {
            self.panes.swap(self.active, o);
            self.quick = None;
        }
    }

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, at: Instant::now() });
    }

    fn save_cfg(&mut self) {
        let local = |p: &Pane| p.tab().loc.local_path().map(|p| p.to_string_lossy().to_string());
        // The focused window of every tab (in tab order), and the other window of the shown tab.
        self.cfg.left_paths = self
            .strip
            .tabs
            .iter()
            .filter_map(|w| if w.uid == self.live { self.panes.get(self.active) } else { w.panes.get(w.active) })
            .filter_map(local)
            .collect();
        self.cfg.session = Some(self.save_session());
        self.cfg.right_paths = self.other().and_then(|o| local(&self.panes[o])).into_iter().collect();
        match self.store.save(&self.cfg) {
            Ok(None) => {}
            // Another Matrix Finder saved meanwhile: what it added or changed is kept, here too.
            Ok(Some(merged)) => {
                self.cfg = merged;
                if self.quit != Quit::Go {
                    let ctx = self.egui_ctx.clone();
                    self.apply_cfg(&ctx);
                }
            }
            Err(e) => self.toast(format!("{}: {e}", t("Cannot save settings")), true),
        }
    }

    /// Tell what loading the settings ran into (a file that could not be read, …).
    fn tell_settings_note(&mut self, note: crate::config::LoadNote) {
        use crate::config::LoadNote;
        let (title, text) = match note {
            LoadNote::Repaired { reset, copy } => {
                let mut text = format!("{}\n\n{}", t("Some settings had values this version cannot read. Only these went back to their defaults; everything else, saved connections included, was kept:"), reset.join(", "));
                if let Some(c) = copy {
                    text.push_str(&format!("\n\n{}\n{}", t("The settings file as it was is kept at:"), c.display()));
                }
                (t("Some settings could not be read"), text)
            }
            LoadNote::Broken { error, moved, from_backup, read_only } => {
                let mut text = format!("{}\n{error}", self.store.path().display());
                if let Some(m) = moved {
                    text.push_str(&format!("\n\n{}\n{}", t("It was set aside, not deleted:"), m.display()));
                }
                text.push_str(&format!("\n\n{}", if from_backup { t("The previous version of the settings (the backup) was loaded instead.") } else { t("Matrix Finder starts with the default settings.") }));
                if read_only {
                    text.push_str(&format!("\n\n{}", t("Changes to the settings are not saved until that file can be read.")));
                }
                (t("The settings file could not be read"), text)
            }
            LoadNote::FromBackup(bak) => (t("Settings restored from the backup"), format!("{}\n{}", t("The settings file was missing, so the previous version was loaded:"), bak.display())),
        };
        self.show_dialog(Dialog::Message { title, text }, false);
    }

    /// Every tab with its windows and layout. Windows that cannot be saved (remote
    /// folders) are saved as the home folder so a split layout stays whole.
    fn save_session(&self) -> crate::config::SavedPane {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let window = |p: &Pane| -> (Option<PathBuf>, bool) {
            let t = p.tab();
            let path = match &t.term {
                Some(term) => Some(term.cwd.clone()),
                // Settings, Transfers and tool windows are not reopened as a folder.
                None if t.settings || t.transfers || t.is_tool() => None,
                None => t.loc.local_path().map(|p| p.to_path_buf()),
            };
            (path, t.is_terminal())
        };
        let mut tabs = Vec::new();
        let mut active = 0;
        for (i, w) in self.strip.tabs.iter().enumerate() {
            let (panes, layout, focus) = if w.uid == self.live { (&self.panes[..], &self.layout, self.active) } else { (&w.panes[..], &w.layout, w.active) };
            if panes.is_empty() {
                continue;
            }
            let wins: Vec<(Option<PathBuf>, bool)> = panes.iter().map(window).collect();
            let (path, terminal) = match &wins[focus.min(wins.len() - 1)] {
                (Some(p), term) => (p.clone(), *term),
                // A single remote window is not saved (as before).
                (None, _) if wins.len() == 1 => continue,
                (None, _) => (home.clone(), false),
            };
            if i == self.strip.active {
                active = tabs.len();
            }
            let windows = if wins.len() > 1 {
                wins.iter()
                    .map(|(p, term)| crate::config::SavedWindow { path: p.clone().unwrap_or_else(|| home.clone()).to_string_lossy().to_string(), terminal: *term && p.is_some() })
                    .collect()
            } else {
                Vec::new()
            };
            tabs.push(crate::config::SavedTab {
                path: path.to_string_lossy().to_string(),
                terminal,
                group: w.group,
                pinned: w.pinned,
                title: w.custom_title.clone(),
                layout: (wins.len() > 1).then(|| layout.clone()),
                windows,
                focus,
            });
        }
        crate::config::SavedPane { tabs, groups: self.strip.groups.clone(), active }
    }

    fn endpoint(&self, loc: &Location) -> Option<Endpoint> {
        Some(match loc {
            Location::Local(p) => Endpoint::Local(p.clone()),
            Location::Remote { id, path } => Endpoint::Remote(self.remotes.get(id)?.1.clone(), *id, path.clone()),
            Location::Archive { file, inner } => Endpoint::Archive(file.clone(), inner.clone()),
        })
    }

    fn refresh_all(&mut self) {
        // Shown windows reload now; hidden tabs only forget cached archive listings.
        let c = lc!(self);
        for p in &mut self.panes {
            p.tab_mut().invalidate_archive_cache();
            if p.tab().is_files() {
                p.tab_mut().reload(&c);
            }
        }
        for p in self.strip.tabs.iter_mut().flat_map(|w| w.panes.iter_mut()) {
            p.tab_mut().invalidate_archive_cache();
        }
        self.quick = None;
    }

    /// Start a background job. File operations (`After::Refresh`) go through the
    /// transfer queue; quick "open this file" downloads start immediately.
    /// `sources` are the local paths it reads; the destination is added to them.
    fn spawn_job<F>(&mut self, title: String, after: After, dest: Option<Location>, sources: Vec<PathBuf>, f: F)
    where
        F: FnOnce(&ops::Progress) -> Res<String> + Send + 'static,
    {
        let queued = matches!(after, After::Refresh);
        let job = Job::spawn(title, queued, f);
        let now = Instant::now();
        let mut paths = sources;
        paths.extend(dest.as_ref().and_then(local_file_of));
        self.jobs.push(RunningJob { job, after, dest, paths, speed: 0.0, sample: (now, 0), started_running: (!queued).then_some(now) });
        self.schedule_jobs();
    }

    /// Release queued jobs while fewer than `max_parallel` are running.
    fn schedule_jobs(&mut self) {
        let max = self.cfg.max_parallel.max(1);
        let mut running = self.jobs.iter().filter(|j| !j.job.progress.is_queued()).count();
        for j in &mut self.jobs {
            if running >= max {
                break;
            }
            if j.job.progress.is_queued() {
                j.job.progress.release();
                j.started_running = Some(Instant::now());
                running += 1;
            }
        }
    }

    fn remote_label(&self, id: u64) -> String {
        self.remotes.get(&id).map(|r| r.0.clone()).unwrap_or_default()
    }

    // ------------------------------------------------------------ navigation

    fn navigate_active(&mut self, loc: Location) {
        // Settings, Tasks, Disk usage, terminal and tool windows have no folder to go to:
        // use a file window of this tab, or a new tab.
        if !self.panes[self.active].tab().is_files() {
            match self.panes.iter().position(|p| p.tab().is_files()) {
                Some(side) => self.active = side,
                None => {
                    let (at, group) = self.next_slot();
                    self.new_files_tab(at, group, Some(loc));
                    return;
                }
            }
        }
        let a = self.active;
        self.panes[a].tab_mut().navigate(loc, &lc!(self));
    }

    fn parse_location_text(&mut self, text: &str) -> Option<Location> {
        let text = text.trim();
        if text.starts_with("ftp://") || text.starts_with("sftp://") {
            // Already connected with this label?
            for (id, (label, _)) in &self.remotes {
                if let Some(rest) = text.strip_prefix(label.as_str()) {
                    let path = if rest.is_empty() { "/".to_string() } else { rest.to_string() };
                    return Some(Location::Remote { id: *id, path });
                }
            }
            self.open_connect_url(text);
            return None;
        }
        if let Some((file, inner)) = text.split_once("::") {
            return Some(Location::Archive { file: PathBuf::from(file), inner: inner.trim_matches('/').to_string() });
        }
        let expanded = if let Some(rest) = text.strip_prefix('~') {
            dirs::home_dir().map(|h| h.join(rest.trim_start_matches(['/', '\\']))).unwrap_or_else(|| PathBuf::from(text))
        } else {
            PathBuf::from(text)
        };
        // Callers make relative paths whole (from the window's folder); one left over would
        // be read from wherever the app was started.
        if !expanded.is_absolute() {
            self.toast(format!("{}: {}", t("Folder not found"), expanded.display()), true);
            return None;
        }
        if expanded.is_file() && archive::is_archive(&expanded) {
            return Some(Location::Archive { file: expanded, inner: String::new() });
        }
        if expanded.is_dir() {
            Some(Location::Local(expanded))
        } else {
            self.toast(format!("{}: {}", t("Folder not found"), expanded.display()), true);
            None
        }
    }

    fn open_connect_url(&mut self, url: &str) {
        let (proto, rest) = if let Some(r) = url.strip_prefix("sftp://") {
            (Protocol::Sftp, r)
        } else if let Some(r) = url.strip_prefix("ftp://") {
            (Protocol::Ftp, r)
        } else {
            return;
        };
        let (auth_host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].to_string()),
            None => (rest, String::new()),
        };
        let (user, hostport) = match auth_host.rsplit_once('@') {
            Some((u, h)) => (u.to_string(), h),
            None => (String::new(), auth_host),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().unwrap_or(proto.default_port())),
            None => (hostport.to_string(), proto.default_port()),
        };
        // Reuse a saved profile (key file / password) when one matches.
        let mut profile = self
            .cfg
            .connections
            .iter()
            .find(|c| c.protocol == proto && c.host == host && c.port == port && c.user == user)
            .cloned()
            .unwrap_or(ConnectionProfile { name: format!("{user}@{host}"), protocol: proto, host, port, user, ..Default::default() });
        profile.initial_path = path;
        self.show_dialog(Dialog::Connect { profile, save: false, save_password: false }, true);
    }

    /// Open what a launch asked for (see `instance`): a folder, archive or URL, or a
    /// file (its folder, with the cursor on it). In a new tab, or in place of the
    /// start folder (`new_tab` false).
    fn open_from_launch(&mut self, text: &str, new_tab: bool) {
        let path = Path::new(text);
        let (target, select) = match (path.parent(), path.file_name()) {
            (Some(dir), Some(name)) if path.is_file() && !archive::is_archive(path) => {
                (dir.to_string_lossy().to_string(), Some(name.to_string_lossy().to_string()))
            }
            _ => (text.to_string(), None),
        };
        let Some(loc) = self.parse_location_text(&target) else { return };
        let mut pane = Pane::new(loc);
        if let Some(name) = &select {
            pane.tab_mut().select_name(name);
        }
        pane.tab_mut().reload(&lc!(self));
        if new_tab {
            let (at, group) = self.next_slot();
            self.insert_workspace(at, Workspace::single(pane), group);
        } else {
            let a = self.active;
            self.panes[a] = pane;
        }
    }

    /// Folders sent by later launches: each opens in a new tab, and the window comes forward.
    fn poll_instance(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.instance.as_ref().map(|s| &s.rx) else { return };
        let got: Vec<String> = rx.try_iter().collect();
        if got.is_empty() {
            return;
        }
        for open in got {
            self.open_from_launch(&open, true);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn open_bookmark(&mut self, b: &Bookmark) {
        let Some(loc) = self.parse_location_text(&bookmark_target(&b.path)) else { return };
        // Settings → Bookmarks: in a terminal window, `cd` the shell to a local folder.
        if self.cfg.bookmark_action == crate::config::BookmarkAction::FilesAndTerminal {
            if let (Location::Local(dir), Some(term)) = (&loc, self.panes[self.active].tab_mut().term.as_mut()) {
                if !crate::termview::is_typeable(&dir.to_string_lossy()) {
                    self.toast(t("This folder's name has control characters: it is not typed into the terminal"), true);
                } else if !term.cd(dir) {
                    self.toast(t("The terminal is busy: quit the running program first"), true);
                }
                return;
            }
        }
        self.navigate_active(loc);
    }

    // ------------------------------------------------------------ bookmarks

    /// Bring up the window `is_it` picks, wherever it is open. False when none is.
    fn focus_open_window(&mut self, is_it: fn(&Tab) -> bool) -> bool {
        if let Some(side) = self.panes.iter().position(|p| is_it(p.tab())) {
            self.active = side;
            return true;
        }
        if let Some(i) = self.strip.tabs.iter().position(|w| w.panes.iter().any(|p| is_it(p.tab()))) {
            let side = self.strip.tabs[i].panes.iter().position(|p| is_it(p.tab())).unwrap_or(0);
            self.strip.tabs[i].active = side;
            self.show_tab(i);
            return true;
        }
        false
    }

    /// Show the Settings tab at `page`: the one already open (in any tab), or a new one.
    fn open_settings(&mut self, page: SettingsPage) {
        self.settings_page = page;
        if !self.focus_open_window(|t| t.settings) {
            let (at, group) = self.next_slot();
            self.insert_workspace(at, Workspace::single(Pane::from_tab(Tab::settings())), group);
        }
    }

    /// Show the Transfers tab: the one already open (in any tab), or a new one.
    fn open_transfers(&mut self) {
        if !self.focus_open_window(|t| t.transfers) {
            let (at, group) = self.next_slot();
            self.insert_workspace(at, Workspace::single(Pane::from_tab(Tab::transfers())), group);
        }
    }

    /// The focused window is the Settings window.
    fn settings_shown(&self) -> bool {
        self.panes[self.active].tab().settings
    }

    fn bookmark_icon(&self, p: &egui::Painter, r: egui::Rect, b: &Bookmark) {
        if b.folder {
            icons::bookmark_folder(p, r, self.theme.weak);
        } else {
            icons::bookmark_icon(p, r, b.icon.resolve(&b.path, self.cfg.bookmark_system_icons), self.theme);
        }
    }

    /// Right-click menu of a bookmark (sidebar and bookmarks bar).
    fn bookmark_menu(&mut self, ui: &mut egui::Ui, b: &Bookmark, remove: &mut Option<String>) {
        if ui.button(t("Open in new tab")).clicked() {
            if let Some(loc) = self.parse_location_text(&bookmark_target(&b.path)) {
                let (at, group) = self.next_slot();
                self.new_files_tab(at, group, Some(loc));
            }
            ui.close_menu();
        }
        if ui.button(t("Open in other panel")).clicked() {
            if let Some(loc) = self.parse_location_text(&bookmark_target(&b.path)) {
                let o = self.other_or_split();
                self.panes[o].tab_mut().navigate(loc, &lc!(self));
            }
            ui.close_menu();
        }
        if ui.button(t("Remove bookmark")).clicked() {
            *remove = Some(b.path.clone());
            ui.close_menu();
        }
        if ui.button(t("Manage bookmarks…")).clicked() {
            self.open_settings(SettingsPage::Bookmarks);
            ui.close_menu();
        }
    }

    /// Right-click menu of a bookmark folder.
    fn bookmark_folder_menu(&mut self, ui: &mut egui::Ui, i: usize) {
        let Some(f) = self.cfg.bookmarks.get(i).cloned() else { return };
        if ui.button(t("Open all in new tabs")).clicked() {
            for b in &f.children {
                if let Some(loc) = self.parse_location_text(&bookmark_target(&b.path)) {
                    let at = self.strip.tabs.len();
                    self.new_files_tab(at, None, Some(loc));
                }
            }
            ui.close_menu();
        }
        if ui.button(t("Remove folder (keep its bookmarks)")).clicked() {
            remove_bookmark_folder(&mut self.cfg.bookmarks, i);
            self.save_cfg();
            ui.close_menu();
        }
        if ui.button(t("Manage bookmarks…")).clicked() {
            self.open_settings(SettingsPage::Bookmarks);
            ui.close_menu();
        }
    }

    /// One bookmark row in the sidebar.
    fn sidebar_bookmark(&mut self, ui: &mut egui::Ui, b: &Bookmark, active: &str, remove: &mut Option<String>) {
        let th = self.theme;
        let target = bookmark_target(&b.path);
        let resp = side_row(ui, th, target == active, &b.name, None, |p, r| self.bookmark_icon(p, r, b)).on_hover_text(&b.path);
        if std::path::Path::new(&target).is_dir() {
            self.sidebar_drop_target(ui, resp.rect, Location::Local(PathBuf::from(&target)));
        }
        if resp.clicked() {
            self.open_bookmark(b);
        }
        resp.context_menu(|ui| self.bookmark_menu(ui, b, remove));
    }

    /// The bookmarks section of the sidebar (vertical bar): folders open and close.
    fn sidebar_bookmarks(&mut self, ui: &mut egui::Ui, active: &str) {
        let th = self.theme;
        section_title(ui, th, &t("Bookmarks"));
        let mut remove = None;
        let mut toggle = None;
        for (i, b) in self.cfg.bookmarks.clone().iter().enumerate() {
            if !b.folder {
                self.sidebar_bookmark(ui, b, active, &mut remove);
                continue;
            }
            let resp = side_row(ui, th, false, &b.name, None, |p, r| icons::bookmark_folder(p, r, th.weak));
            let chev = egui::Rect::from_center_size(egui::pos2(resp.rect.right() - 14.0, resp.rect.center().y), egui::vec2(14.0, 14.0));
            icons::chevron(ui.painter(), chev, if b.collapsed { 1 } else { 2 }, th.weak);
            if resp.clicked() {
                toggle = Some(i);
            }
            let tip = format!("{} · {}", b.children.len(), if b.collapsed { t("Expand group") } else { t("Collapse group") });
            resp.on_hover_text(tip).context_menu(|ui| self.bookmark_folder_menu(ui, i));
            if !b.collapsed {
                for c in &b.children {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(14.0);
                        ui.vertical(|ui| self.sidebar_bookmark(ui, c, active, &mut remove));
                    });
                }
            }
        }
        if self.cfg.bookmarks.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new(format!("{} ({})", t("Bookmark current folder"), hint(Action::Bookmark))).small().color(th.weak));
            });
        }
        if let Some(i) = toggle {
            self.cfg.bookmarks[i].collapsed = !self.cfg.bookmarks[i].collapsed;
            self.save_cfg();
        }
        if let Some(path) = remove {
            Bookmark::remove_path(&mut self.cfg.bookmarks, &path);
            self.save_cfg();
        }
    }

    /// Width of a chip in the bookmarks bar.
    fn chip_width(&self, ui: &egui::Ui, b: &Bookmark) -> f32 {
        let text = ui.fonts(|f| f.layout_no_wrap(b.name.clone(), egui::FontId::proportional(13.0), egui::Color32::WHITE).size().x).min(160.0);
        text + if b.folder { 46.0 } else { 34.0 }
    }

    /// One bookmark or folder in the bookmarks bar.
    fn bar_chip(&mut self, ui: &mut egui::Ui, b: &Bookmark, i: usize, active: &str, remove: &mut Option<String>) {
        let th = self.theme;
        let w = self.chip_width(ui, b);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 26.0), egui::Sense::click());
        let popup = egui::Id::new(("bookmark-folder", i));
        let open = b.folder && ui.memory(|m| m.is_popup_open(popup));
        let p = ui.painter();
        if !b.folder && bookmark_target(&b.path) == active {
            p.rect_filled(rect, 7.0, th.selection);
        } else if resp.hovered() || open {
            p.rect_filled(rect, 7.0, th.hover());
        }
        self.bookmark_icon(p, egui::Rect::from_center_size(egui::pos2(rect.left() + 15.0, rect.center().y), egui::vec2(15.0, 15.0)), b);
        let text_w = w - if b.folder { 46.0 } else { 34.0 };
        let mut job = egui::text::LayoutJob::simple_singleline(b.name.clone(), egui::FontId::proportional(13.0), th.text);
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_w + 1.0);
        let g = ui.fonts(|f| f.layout_job(job));
        p.galley(egui::pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0), g, th.text);
        if b.folder {
            let c = egui::Rect::from_center_size(egui::pos2(rect.right() - 12.0, rect.center().y), egui::vec2(12.0, 12.0));
            icons::chevron(p, c, 2, th.weak);
            if resp.clicked() {
                ui.memory_mut(|m| m.toggle_popup(popup));
            }
            egui::popup::popup_below_widget(ui, popup, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                ui.set_min_width(200.0);
                for c in &b.children {
                    self.bar_menu_item(ui, c, remove);
                }
                if b.children.is_empty() {
                    ui.label(RichText::new(t("Empty folder")).color(th.weak));
                }
            });
            resp.context_menu(|ui| self.bookmark_folder_menu(ui, i));
        } else {
            let target = bookmark_target(&b.path);
            if std::path::Path::new(&target).is_dir() {
                self.sidebar_drop_target(ui, rect, Location::Local(PathBuf::from(&target)));
            }
            if resp.clicked() {
                self.open_bookmark(b);
            }
            let resp = if egui::DragAndDrop::has_any_payload(ui.ctx()) { resp } else { resp.on_hover_text(&b.path) };
            resp.context_menu(|ui| self.bookmark_menu(ui, b, remove));
        }
    }

    /// A bookmark inside a bookmarks-bar menu (folder or overflow): icon and name.
    fn bar_menu_item(&mut self, ui: &mut egui::Ui, b: &Bookmark, remove: &mut Option<String>) {
        let th = self.theme;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(200.0), 26.0), egui::Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 5.0, th.hover());
        }
        self.bookmark_icon(ui.painter(), egui::Rect::from_center_size(egui::pos2(rect.left() + 14.0, rect.center().y), egui::vec2(15.0, 15.0)), b);
        ui.painter().text(egui::pos2(rect.left() + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, &b.name, egui::FontId::proportional(13.5), th.text);
        if resp.clicked() {
            self.open_bookmark(b);
        }
        resp.on_hover_text(&b.path).context_menu(|ui| self.bookmark_menu(ui, b, remove));
    }

    /// The horizontal bookmarks bar above the tab bar. What does not fit goes into "»".
    fn bookmarks_bar(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        let active = self.panes[self.active].tab().loc.display(|id| self.remote_label(id));
        let list = self.cfg.bookmarks.clone();
        let mut remove = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let room = ui.available_width() - 30.0;
            let mut used = 0.0;
            let mut overflow = Vec::new();
            for (i, b) in list.iter().enumerate() {
                let w = self.chip_width(ui, b) + 2.0;
                if !overflow.is_empty() || used + w > room {
                    overflow.push(i);
                    continue;
                }
                used += w;
                self.bar_chip(ui, b, i, &active, &mut remove);
            }
            if !overflow.is_empty() {
                let more = icons::icon_button(ui, 26.0, true, th, |p, r, c| {
                    icons::chevron(p, r.translate(egui::vec2(-2.5, 0.0)), 1, c);
                    icons::chevron(p, r.translate(egui::vec2(2.5, 0.0)), 1, c);
                })
                .on_hover_text(t("More bookmarks"));
                let id = egui::Id::new("bookmarks-more");
                if more.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(id));
                }
                egui::popup::popup_below_widget(ui, id, &more, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                    ui.set_min_width(220.0);
                    for i in overflow {
                        let b = &list[i];
                        if b.folder {
                            ui.menu_button(format!("{}  ›", b.name), |ui| {
                                for c in &b.children {
                                    self.bar_menu_item(ui, c, &mut remove);
                                }
                            });
                        } else {
                            self.bar_menu_item(ui, b, &mut remove);
                        }
                    }
                });
            }
        });
        if let Some(path) = remove {
            Bookmark::remove_path(&mut self.cfg.bookmarks, &path);
            self.save_cfg();
        }
    }

    /// Settings → Bookmarks: where they are shown, icons, and the list itself.
    fn bookmark_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        use crate::config::{BookmarkAction, BookmarkBar, BookmarkIcon};
        let mut changed = false;
        settings_section(ui, th, &t("Bookmarks bar"), &t("Where bookmarks are shown and what they do."));
        let mut top = card_frame(th).begin(ui);
        {
        let ui = &mut top.content_ui;
        ui.set_width(ui.available_width());
        changed |= ui.radio_value(&mut self.cfg.bookmark_bar, BookmarkBar::Vertical, t("Vertical bar (in the sidebar)")).changed();
        changed |= ui.radio_value(&mut self.cfg.bookmark_bar, BookmarkBar::Horizontal, t("Horizontal bar (above the tabs)")).changed();
        ui.add_space(4.0);
        ui.label(RichText::new(t("Choosing a bookmark")).strong());
        changed |= ui
            .radio_value(&mut self.cfg.bookmark_action, BookmarkAction::FilesOnly, t("Go to file only"))
            .on_hover_text(t("In a terminal, the folder opens in a file window of the tab (or a new tab)"))
            .changed();
        changed |= ui
            .radio_value(&mut self.cfg.bookmark_action, BookmarkAction::FilesAndTerminal, t("File and Terminal"))
            .on_hover_text(t("In a terminal, run cd to the folder"))
            .changed();
        ui.add_space(4.0);
        changed |= toggle_row(
            ui,
            th,
            &mut self.cfg.bookmark_system_icons,
            &t("Icons for standard folders (Desktop, Documents, Downloads, …)"),
            &t("Off: every bookmark gets a plain folder icon (unless you picked an icon for it)"),
        );
        }
        top.end(ui);

        settings_section(ui, th, &t("Bookmarks"), &t("Rename, reorder and pick icons. Put bookmarks into folders to group them."));
        let folders: Vec<(usize, String)> = self.cfg.bookmarks.iter().enumerate().filter(|(_, b)| b.folder).map(|(i, b)| (i, b.name.clone())).collect();
        let mut list = card_frame(th).begin(ui);
        let ui_outer = ui;
        let ui = &mut list.content_ui;
        ui.set_width(ui.available_width());
        // The location takes the width the rest of a row leaves (a row inside a folder is indented).
        let path_w = (ui.available_width() - 413.0).max(120.0);
        let mut edit: Option<BmEdit> = None;
        let sys = self.cfg.bookmark_system_icons;
        let n = self.cfg.bookmarks.len();
        let mut paths = std::mem::take(&mut self.bookmark_paths);
        let (drives, show_hidden) = (&self.drives, self.cfg.show_hidden);
        let home = dirs::home_dir();
        let mut row = |ui: &mut egui::Ui, b: &mut Bookmark, at: (usize, Option<usize>), count: usize, edit: &mut Option<BmEdit>, changed: &mut bool| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if at.1.is_some() {
                    ui.add_space(22.0);
                }
                if b.folder {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                    icons::bookmark_folder(ui.painter(), r.shrink(5.0), th.weak);
                } else {
                    // Icon: click for the choices.
                    let shown = b.icon.resolve(&b.path, sys);
                    let resp = icons::icon_button(ui, 26.0, true, th, |p, r, _| icons::bookmark_icon(p, r.shrink(1.0), shown, th)).on_hover_text(t("Icon"));
                    let id = resp.id.with("icon-pick");
                    if resp.clicked() {
                        ui.memory_mut(|m| m.toggle_popup(id));
                    }
                    egui::popup::popup_below_widget(ui, id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                        ui.set_min_width(170.0);
                        for choice in BookmarkIcon::CHOICES {
                            let (r, resp) = ui.allocate_exact_size(egui::vec2(170.0, 24.0), egui::Sense::click());
                            crate::pane::menu_row_fill(ui, r, th, b.icon == choice, resp.hovered());
                            let draw = choice.resolve(&b.path, sys);
                            icons::bookmark_icon(ui.painter(), egui::Rect::from_center_size(egui::pos2(r.left() + 14.0, r.center().y), egui::vec2(15.0, 15.0)), draw, th);
                            ui.painter().text(egui::pos2(r.left() + 30.0, r.center().y), egui::Align2::LEFT_CENTER, t(choice.label()), egui::FontId::proportional(13.0), th.text);
                            if resp.clicked() {
                                b.icon = choice;
                                *changed = true;
                            }
                        }
                    });
                }
                *changed |= ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(if b.folder { 200.0 } else { 130.0 }).hint_text(t("Name"))).changed();
                if !b.folder {
                    let o = crate::pathfield::Opts { base: home.as_deref(), show_hidden, drives, width: path_w, hint: Some(t("Location")) };
                    *changed |= paths.show(ui, &mut b.path, egui::Id::new(("bm-path", at)), o, th).changed();
                    // Folder to keep it in.
                    let cur = at.1.and_then(|_| folders.iter().find(|(i, _)| *i == at.0)).map(|(_, n)| n.clone()).unwrap_or_else(|| "—".into());
                    egui::ComboBox::from_id_salt(("bm-folder", at.0, at.1)).width(110.0).selected_text(cur).show_ui(ui, |ui| {
                        if ui.selectable_label(at.1.is_none(), t("No folder")).clicked() && at.1.is_some() {
                            *edit = Some(BmEdit::MoveTo(at, None));
                        }
                        for (fi, name) in &folders {
                            let here = at.1.is_some() && at.0 == *fi;
                            if ui.selectable_label(here, if name.is_empty() { t("Unnamed folder") } else { name.clone() }).clicked() && !here {
                                *edit = Some(BmEdit::MoveTo(at, Some(*fi)));
                            }
                        }
                    })
                    .response
                    .on_hover_text(t("Folder"));
                }
                let k = at.1.unwrap_or(at.0);
                if icons::icon_button(ui, 24.0, k > 0, th, |p, r, c| icons::chevron(p, r, 0, c)).on_hover_text(t("Move up")).clicked() {
                    *edit = Some(BmEdit::Up(at));
                }
                if icons::icon_button(ui, 24.0, k + 1 < count, th, |p, r, c| icons::chevron(p, r, 2, c)).on_hover_text(t("Move down")).clicked() {
                    *edit = Some(BmEdit::Down(at));
                }
                let tip = if b.folder { t("Remove folder (keep its bookmarks)") } else { t("Remove bookmark") };
                if icons::icon_button(ui, 24.0, true, th, icons::mi_trash).on_hover_text(tip).clicked() {
                    *edit = Some(BmEdit::Delete(at));
                }
            });
        };
        for i in 0..n {
            let b = &mut self.cfg.bookmarks[i];
            row(ui, b, (i, None), n, &mut edit, &mut changed);
            if b.folder {
                let m = b.children.len();
                for j in 0..m {
                    let c = &mut self.cfg.bookmarks[i].children[j];
                    row(ui, c, (i, Some(j)), m, &mut edit, &mut changed);
                }
                if m == 0 {
                    ui.horizontal(|ui| {
                        ui.add_space(54.0);
                        ui.label(RichText::new(t("Empty folder: choose it in a bookmark's folder list")).size(11.5).color(th.weak));
                    });
                }
            }
        }
        self.bookmark_paths = paths;
        if let Some(e) = edit {
            apply_bookmark_edit(&mut self.cfg.bookmarks, e);
            changed = true;
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button(format!("+ {}", t("Add current folder"))).clicked() {
                let loc = self.panes[self.active].tab().loc.clone();
                let path = loc.display(|id| self.remote_label(id));
                if !Bookmark::leaves(&self.cfg.bookmarks).any(|b| bookmark_target(&b.path) == path) {
                    self.cfg.bookmarks.push(Bookmark::new(loc.short_name(), path));
                    changed = true;
                }
            }
            if ui.button(format!("+ {}", t("New folder"))).clicked() {
                self.cfg.bookmarks.push(Bookmark::new_folder(t("New folder")));
                changed = true;
            }
            if ui.button(t("Add standard folders")).on_hover_text(t("Desktop, Documents, Downloads, Music, Pictures and Movies, if they are missing")).clicked() {
                for b in crate::config::system_bookmarks() {
                    if !Bookmark::leaves(&self.cfg.bookmarks).any(|x| bookmark_target(&x.path) == bookmark_target(&b.path)) {
                        self.cfg.bookmarks.push(b);
                        changed = true;
                    }
                }
            }
            if ui.button(t("Export…")).clicked() {
                if let Some(p) = rfd::FileDialog::new().set_file_name("matrix_finder-bookmarks.json").save_file() {
                    let r = serde_json::to_string_pretty(&self.cfg.bookmarks).map_err(es).and_then(|s| std::fs::write(&p, s).map_err(es));
                    match r {
                        Ok(()) => self.toast(format!("{} {}", t("Exported to"), p.display()), false),
                        Err(e) => self.toast(e, true),
                    }
                }
            }
            if ui.button(t("Import…")).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                    let r: Res<Vec<Bookmark>> = std::fs::read_to_string(&p).map_err(es).and_then(|s| serde_json::from_str(&s).map_err(es));
                    match r {
                        Ok(list) => {
                            for mut b in list {
                                // Skip what is already bookmarked (also inside imported folders).
                                let have = |x: &Bookmark, list: &[Bookmark]| Bookmark::leaves(list).any(|y| y.path == x.path);
                                if b.folder {
                                    b.children.retain(|c| !have(c, &self.cfg.bookmarks));
                                    self.cfg.bookmarks.push(b);
                                } else if !have(&b, &self.cfg.bookmarks) {
                                    self.cfg.bookmarks.push(b);
                                }
                            }
                            changed = true;
                        }
                        Err(e) => self.toast(e, true),
                    }
                }
            }
        });
        list.end(ui_outer);
        changed
    }


    /// Enter / double-click on the cursor entry.
    fn open_cursor(&mut self) {
        let a = self.active;
        let tab = self.panes[a].tab();
        let Some(e) = tab.cursor_entry().cloned() else { return };
        let loc = tab.loc.clone();
        if e.is_parent {
            self.panes[a].tab_mut().go_up(&lc!(self));
            return;
        }
        if e.is_dir {
            self.navigate_active(loc.child(&e.name));
            return;
        }
        match &loc {
            Location::Local(dir) => {
                let path = dir.join(&e.name);
                // An app chosen for archives takes over from browsing inside them.
                // Only a regular file is browsed as an archive (a named pipe would block).
                if archive::is_archive(&path) && path.is_file() && crate::defaultapps::program_for(&self.cfg.default_apps, &e.name).is_none() {
                    if archive::lock_state(&path) == archive::Lock::Everything {
                        self.ask_archive_password(path.clone(), AfterPassword::Browse(path));
                        return;
                    }
                    self.navigate_active(Location::Archive { file: path, inner: String::new() });
                } else {
                    self.open_file(&path);
                }
            }
            _ => self.open_via_temp(&e, false),
        }
    }

    /// Open a file with the app chosen for its type, or else the system's default app.
    fn open_file(&mut self, path: &Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let opened = match crate::defaultapps::program_for(&self.cfg.default_apps, &name) {
            Some(program) => terminal::open_with(program, path).map_err(|e| format!("{} {program}: {e}", t("Could not open the file with"))),
            None => terminal::open_default(path),
        };
        if let Err(e) = opened {
            self.toast(e, true);
        }
    }

    /// The "Open with…" dialog, starting from the app chosen for this type.
    fn open_with_dialog(&mut self, path: PathBuf) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let program = crate::defaultapps::program_for(&self.cfg.default_apps, &name).unwrap_or_default().to_string();
        self.show_dialog(Dialog::OpenWith { path, program, always: false }, true);
    }

    fn ask_archive_password(&mut self, file: PathBuf, then: AfterPassword) {
        self.show_dialog(Dialog::ArchivePassword { file, password: String::new(), wrong: false, checking: None, then }, true);
    }

    /// The password was accepted: go on with what it was asked for.
    fn after_password(&mut self, then: AfterPassword) {
        self.quick = None;
        match then {
            AfterPassword::Browse(file) => self.navigate_active(Location::Archive { file, inner: String::new() }),
            AfterPassword::Preview => {}
            AfterPassword::Command(cmd) => self.run_cmd(cmd),
            AfterPassword::Confirm(dialog) => {
                if let Some(next) = self.confirm_dialog(*dialog) {
                    self.keep_dialog(next);
                }
            }
        }
    }

    /// Download / extract a non-local file to a temp folder and open it.
    fn open_via_temp(&mut self, e: &Entry, with_dialog: bool) {
        let a = self.active;
        let loc = self.panes[a].tab().loc.clone();
        if let Location::Archive { file, inner } = &loc {
            if archive::entry_is_locked(file, &join_slash(inner, &e.name)) {
                let cmd = if with_dialog { Cmd::OpenWith } else { Cmd::Open };
                self.ask_archive_password(file.clone(), AfterPassword::Command(cmd));
                return;
            }
        }
        let Some(src) = self.endpoint(&loc) else { return };
        // A new private folder each time: an earlier copy (maybe edited) is never written over.
        let tmp = match crate::util::private_temp_dir(crate::tempcopy::TEMP_PREFIX) {
            Ok(d) => d,
            Err(err) => {
                self.toast(format!("{} {}: {err}", t("Could not open"), e.name), true);
                return;
            }
        };
        let target = tmp.join(&e.name);
        let label = loc.child(&e.name).display(|id| self.remote_label(id));
        self.temp_copies.add(loc.clone(), e.name.clone(), label, tmp.clone(), target.clone());
        let items = vec![ops::Item { name: e.name.clone(), is_dir: false, size: e.size }];
        let dst = Endpoint::Local(tmp);
        let after = if with_dialog { After::OpenWithDialog(target) } else { After::OpenFile(target) };
        let sources = local_file_of(&loc).into_iter().collect();
        self.spawn_job(format!("{} {}", t("Opening"), e.name), after, None, sources, move |p| {
            ops::transfer(src, items, dst, false, Conflict::Skip, p)
        });
    }

    /// Every two seconds: a copy opened from a server or an archive that an app saved is
    /// asked about (upload the changes, or keep the copy).
    fn poll_temp_copies(&mut self, ctx: &egui::Context) {
        if self.temp_copies.list.is_empty() {
            return;
        }
        for id in self.temp_copies.poll(false) {
            if !self.copy_prompts.contains(&id) {
                self.copy_prompts.push(id);
            }
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    /// Upload the edited copy `id` over the file it came from on the server.
    fn upload_copy(&mut self, id: u64) {
        let Some(c) = self.temp_copies.list.iter().find(|c| c.id == id) else { return };
        let (src, name, dir, label) = (c.src.clone(), c.name.clone(), c.dir.clone(), c.label.clone());
        let Some(stamp) = crate::tempcopy::stamp(&c.path) else { return };
        let Some(dst) = self.endpoint(&src) else {
            self.toast(format!("{}: {label}", t("Not connected any more; the edited copy is kept")), true);
            return;
        };
        if let Some(c) = self.temp_copies.get_mut(id) {
            c.uploading = true;
            c.asked = Some(stamp);
        }
        let items = vec![ops::Item { name: name.clone(), is_dir: false, size: stamp.0 }];
        self.spawn_job(format!("{} {name}", t("Uploading changes to")), After::Uploaded(id, stamp), Some(src), vec![dir.clone()], move |p| {
            ops::transfer(Endpoint::Local(dir), items, dst, false, Conflict::Overwrite, p)
        });
    }

    /// The notes about edited copies, above the toasts.
    fn copy_prompts_ui(&mut self, ctx: &egui::Context) {
        self.copy_prompts.retain(|id| self.temp_copies.list.iter().any(|c| c.id == *id && !c.uploading));
        if self.copy_prompts.is_empty() {
            return;
        }
        let th = self.theme;
        let mut act: Option<(u64, u8)> = None;
        egui::Area::new(egui::Id::new("copy-prompts"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-18.0, 64.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for c in self.temp_copies.list.iter().filter(|c| self.copy_prompts.contains(&c.id)) {
                    egui::Frame::popup(ui.style()).fill(th.panel).corner_radius(10).inner_margin(egui::Margin::symmetric(14, 10)).stroke(egui::Stroke::new(1.0_f32, th.border)).show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.label(RichText::new(format!("{} “{}”", t("Changed in another app:"), c.name)).strong().color(th.text));
                        ui.add(egui::Label::new(RichText::new(&c.label).color(th.weak)).truncate());
                        if !c.is_remote() {
                            ui.label(RichText::new(t("It came from an archive, which cannot be changed: keep the copy somewhere.")).color(th.weak));
                        }
                        ui.horizontal(|ui| {
                            if c.is_remote() {
                                let b = egui::Button::new(RichText::new(t("Upload changes")).color(th.on_accent()).strong()).fill(th.accent);
                                if ui.add(b).clicked() {
                                    act = Some((c.id, 0));
                                }
                            } else if ui.button(t("Show copy")).clicked() {
                                act = Some((c.id, 1));
                            }
                            if ui.button(t("Keep local copy")).clicked() {
                                act = Some((c.id, 2));
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        let Some((id, what)) = act else { return };
        match what {
            0 => self.upload_copy(id),
            1 => {
                if let Some(c) = self.temp_copies.list.iter().find(|c| c.id == id) {
                    if let Err(e) = terminal::reveal(&c.path) {
                        self.toast(e, true);
                    }
                }
            }
            _ => {
                if let Some(c) = self.temp_copies.get_mut(id) {
                    c.asked = crate::tempcopy::stamp(&c.path);
                }
                let dir = crate::tempcopy::recovered_dir();
                self.toast(format!("{} {}", t("Kept. When Matrix Finder quits, edited copies are moved to"), dir.display()), false);
            }
        }
        if what != 1 {
            self.copy_prompts.retain(|x| *x != id);
        }
    }

    fn preview_for_cursor(&self, side: usize) -> Option<Preview> {
        let tab = self.panes[side].tab();
        let e = tab.cursor_entry()?.clone();
        let key = format!("{}|{}", tab.loc.display(|id| self.remote_label(id)), e.name);
        let syntax = self.theme.syntax;
        if e.is_parent {
            return Some(Preview::info(key, "..".into(), t("Parent folder")));
        }
        let limit = preview::read_limit(&e.name);
        match &tab.loc {
            Location::Local(dir) => {
                let path = dir.join(&e.name);
                if e.is_dir {
                    let name = e.name.clone();
                    return Some(Preview::text_async(key, e.name.clone(), move |cancel| folder_summary(&path, &name, cancel)));
                }
                // Only asks the file system (about the target, for a link). Only regular
                // files are read: a named pipe or a device could block forever, or give
                // away data meant for another program.
                let size = match std::fs::metadata(&path) {
                    Ok(m) if m.is_file() => m.len(),
                    Ok(_) => return Some(Preview::info(key, e.name.clone(), special_file_note(&path))),
                    Err(err) => return Some(Preview::info(key, e.name.clone(), err.to_string())),
                };
                if preview::too_large(&e.name, size, None) {
                    return Some(Preview::too_large(key, e.name.clone(), size));
                }
                // Comic books are archives shown by their cover.
                if archive::is_archive(&path) && preview::classify(&e.name) != preview::Kind::Package {
                    return Some(Preview::text_async(key, e.name.clone(), move |cancel| archive_summary(&path, cancel)));
                }
                if preview::classify(&e.name) == preview::Kind::Table {
                    return Some(Preview::table_local(key, e.name.clone(), path, syntax));
                }
                if Preview::reads_files_directly(&e.name) {
                    return Some(Preview::local_file(key, e.name.clone(), path, syntax, self.max_texture));
                }
                let loader: preview::Loader = Box::new(move || read_head(&path, limit));
                Some(Preview::new(key, e.name.clone(), loader, limit, syntax, self.max_texture).of_size(size))
            }
            Location::Archive { file, inner } => {
                if e.is_dir {
                    return Some(Preview::info(key, e.name.clone(), format!("{}: {}", t("Folder in archive"), human_size(e.size))));
                }
                if preview::too_large(&e.name, e.size, Some(limit as u64)) {
                    return Some(Preview::too_large(key, e.name.clone(), e.size));
                }
                let file = file.clone();
                let inner = join_slash(inner, &e.name);
                let archive_file = file.clone();
                let loader: preview::Loader = Box::new(move || archive::read_entry(&file, &inner, limit));
                Some(Preview::new(key, e.name.clone(), loader, limit, syntax, self.max_texture).in_archive(archive_file).of_size(e.size))
            }
            Location::Remote { id, path } => {
                if e.is_dir {
                    return Some(Preview::info(key, e.name.clone(), t("Remote folder")));
                }
                let fs = self.remotes.get(id)?.1.clone();
                let rp = join_slash(path, &e.name);
                let limit = limit.min(16 * 1024 * 1024);
                if preview::too_large(&e.name, e.size, Some(limit as u64)) {
                    return Some(Preview::too_large(key, e.name.clone(), e.size));
                }
                // The preview's cancel flag, once it exists: moving on stops the read. A
                // transfer holds the connection for a whole file, so this does not wait for it.
                let flag: Arc<std::sync::OnceLock<crate::media::Cancel>> = Arc::default();
                let f2 = flag.clone();
                let loader: preview::Loader = Box::new(move || {
                    // It is set right after the preview is made.
                    let cancel = (0..1000).find_map(|_| f2.get().cloned().or_else(|| { std::thread::sleep(Duration::from_millis(1)); None })).unwrap_or_default();
                    let gone = || cancel.load(std::sync::atomic::Ordering::Relaxed);
                    let mut g = remote::use_soon(&fs, Duration::from_secs(2), &gone)?;
                    g.read_head(&rp, limit, &cancel)
                });
                let p = Preview::new(key, e.name.clone(), loader, limit, syntax, self.max_texture).of_size(e.size);
                let _ = flag.set(p.cancel_flag());
                Some(p)
            }
        }
    }

    // ------------------------------------------------------------ commands

    fn run_cmd(&mut self, cmd: Cmd) {
        let a = self.active;
        let tab = self.panes[a].tab();
        // Settings, Tasks, Disk usage, terminal and tool windows have no file list: what acts
        // on one would work unseen in the folder behind them.
        if !cmd_works_in(cmd, tab) {
            return;
        }
        let loc = tab.loc.clone();
        let sel = tab.selection();
        let cursor = tab.cursor_entry().cloned();
        match cmd {
            Cmd::Open => self.open_cursor(),
            Cmd::OpenDefault => {
                let Some(e) = cursor else { return };
                // A folder (or "..") opens here, as Enter does; a file in its default app.
                if e.is_dir || e.is_parent {
                    self.open_cursor();
                    return;
                }
                match &loc {
                    Location::Local(d) => self.open_file(&d.join(&e.name)),
                    _ => self.open_via_temp(&e, false),
                }
            }
            Cmd::OpenWith => {
                let Some(e) = cursor.filter(|e| !e.is_parent && !e.is_dir) else { return };
                match &loc {
                    Location::Local(d) => self.open_with_dialog(d.join(&e.name)),
                    _ => self.open_via_temp(&e, true),
                }
            }
            Cmd::DefaultApps => self.open_settings(SettingsPage::DefaultApps),
            Cmd::View => {
                self.viewer = self.preview_for_cursor(a);
            }
            Cmd::QuickView => self.toggle_quick_view(),
            Cmd::Copy | Cmd::Move => {
                if sel.is_empty() {
                    return;
                }
                let dst_loc = match self.other() {
                    Some(o) => self.panes[o].tab().loc.clone(),
                    None => loc.clone(),
                };
                let dst = dst_loc.display(|id| self.remote_label(id));
                self.show_dialog(Dialog::Transfer { src: loc.clone(), mv: cmd == Cmd::Move, items: sel, dst, policy: Conflict::Rename }, true);
            }
            Cmd::BatchRename => self.open_batch_rename(),
            Cmd::Checksum => self.open_checksum(),
            Cmd::Split(dir, terminal) => {
                let _ = self.split_pane(a, dir, terminal);
            }
            Cmd::ClosePane => self.request_close_pane(a),
            Cmd::TerminalProfile(i) => self.open_terminal_profile(Some(i)),
            Cmd::Rename => {
                if self.panes[a].tab().marked.len() > 1 {
                    self.open_batch_rename();
                    return;
                }
                let Some(e) = cursor.filter(|e| !e.is_parent) else { return };
                if matches!(loc, Location::Archive { .. }) {
                    self.toast(t("Archives are read-only"), true);
                    return;
                }
                let view = self.panes[a].tab().uid;
                self.show_dialog(Dialog::Rename { old: e.name.clone(), new: e.name, loc, view }, true);
            }
            Cmd::Delete => {
                if sel.is_empty() {
                    return;
                }
                if matches!(loc, Location::Archive { .. }) {
                    self.toast(t("Archives are read-only"), true);
                    return;
                }
                let trash = self.cfg.use_trash && loc.is_local();
                let view = self.panes[a].tab().uid;
                // Turning confirmations off skips them only for the trash: what cannot be
                // brought back (Shift+Delete, servers) is always asked.
                if self.cfg.confirm_delete || !trash {
                    self.show_dialog(Dialog::Delete { items: sel, trash, loc, view }, false);
                } else {
                    self.do_delete(&loc, view, sel, trash);
                }
            }
            Cmd::NewFolder | Cmd::NewFile => {
                if matches!(loc, Location::Archive { .. }) {
                    self.toast(t("Archives are read-only"), true);
                    return;
                }
                let view = self.panes[a].tab().uid;
                self.show_dialog(Dialog::NewItem { name: String::new(), file: cmd == Cmd::NewFile, loc, view }, true);
            }
            Cmd::Pack => {
                let Location::Local(dir) = &loc else {
                    self.toast(t("Compression works on local files"), true);
                    return;
                };
                if sel.is_empty() {
                    return;
                }
                let name = if sel.len() == 1 {
                    let n = &sel[0].name;
                    if sel[0].is_dir || sel[0].ext.is_empty() { n.clone() } else { n[..n.len() - sel[0].ext.len() - 1].to_string() }
                } else {
                    dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "archive".into())
                };
                let dest_dir = match self.other().and_then(|o| self.panes[o].tab().loc.local_path().map(|p| p.to_path_buf())).as_deref() {
                    Some(p) => p.to_string_lossy().to_string(),
                    None => dir.to_string_lossy().to_string(),
                };
                let format = ArchiveFormat::Zip;
                self.show_dialog(Dialog::Pack { items: sel, src_dir: dir.clone(), name, dest_dir, format, level: 6 }, true);
            }
            Cmd::ExtractHere | Cmd::ExtractOther => self.start_unpack(cmd == Cmd::ExtractHere),
            Cmd::CopyPath => {
                let text = match (&loc, &cursor) {
                    (Location::Local(d), Some(e)) if !e.is_parent => d.join(&e.name).to_string_lossy().to_string(),
                    (l, Some(e)) if !e.is_parent => l.child(&e.name).display(|id| self.remote_label(id)),
                    (l, _) => l.display(|id| self.remote_label(id)),
                };
                self.toast(format!("{}: {text}", t("Copied")), false);
                self.copy_text = Some(text);
            }
            Cmd::Reveal => {
                if let (Location::Local(d), Some(e)) = (&loc, &cursor) {
                    let p = if e.is_parent { d.clone() } else { d.join(&e.name) };
                    if let Err(err) = terminal::reveal(&p) {
                        self.toast(err, true);
                    }
                }
            }
            Cmd::Terminal => self.open_terminal(),
            Cmd::Bookmark => {
                let path = loc.display(|id| self.remote_label(id));
                self.toggle_bookmark(path, loc.short_name());
            }
            Cmd::BookmarkItem => {
                if let (Location::Local(d), Some(e)) = (&loc, &cursor) {
                    if e.is_dir && !e.is_parent {
                        self.toggle_bookmark(d.join(&e.name).to_string_lossy().to_string(), e.name.clone());
                    }
                }
            }
            Cmd::Properties => self.show_properties(),
            Cmd::DiskUsage => {
                let p = match (&loc, &cursor) {
                    (Location::Local(d), Some(e)) if e.is_dir && !e.is_parent => d.join(&e.name),
                    (Location::Local(d), _) => d.clone(),
                    _ => {
                        self.toast(t("Disk usage works on local folders"), true);
                        return;
                    }
                };
                self.open_disk_usage(p);
            }
            Cmd::Refresh => {
                let tab = self.panes[a].tab_mut();
                if let Location::Local(dir) = &tab.loc {
                    crate::gitinfo::invalidate_children(dir);
                }
                tab.invalidate_archive_cache();
                tab.reload(&lc!(self));
                self.drive_watch.refresh();
            }
            Cmd::CalcSizes => self.calc_sizes(),
            Cmd::SelectAll => self.panes[a].tab_mut().mark_all(),
        }
    }

    fn toggle_bookmark(&mut self, path: String, name: String) {
        // A bookmark kept as `~/…` is the same folder.
        let stored = Bookmark::leaves(&self.cfg.bookmarks).find(|b| bookmark_target(&b.path) == path).map(|b| b.path.clone()).unwrap_or_else(|| path.clone());
        if Bookmark::remove_path(&mut self.cfg.bookmarks, &stored) {
            self.toast(format!("{}: {name}", t("Bookmark removed")), false);
        } else {
            self.cfg.bookmarks.push(Bookmark::new(name.clone(), path));
            self.toast(format!("{}: {name}", t("Bookmark added")), false);
        }
        self.save_cfg();
    }

    fn calc_sizes(&mut self) {
        let a = self.active;
        let tab = self.panes[a].tab_mut();
        let Some(dir) = tab.loc.local_path().map(|p| p.to_path_buf()) else { return };
        let names: Vec<String> = tab.entries.iter().filter(|e| e.is_dir && !e.is_parent).map(|e| e.name.clone()).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for n in names {
                let size: u64 = walkdir::WalkDir::new(dir.join(&n))
                    .into_iter()
                    .flatten()
                    .filter(|e| e.file_type().is_file())
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum();
                if tx.send((n, size)).is_err() {
                    break;
                }
            }
        });
        tab.dir_sizes = Some(rx);
    }

    fn open_terminal(&mut self) {
        let a = self.active;
        let dir = match &self.panes[a].tab().loc {
            Location::Local(p) => p.clone(),
            Location::Archive { file, .. } => file.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            Location::Remote { .. } => {
                self.toast(t("Terminal opens only for local folders"), true);
                return;
            }
        };
        let idx = self.cfg.default_terminal;
        self.launch_profile(idx, &dir);
    }

    /// Open a terminal in the active folder with a profile (`None` = default).
    fn open_terminal_profile(&mut self, idx: Option<usize>) {
        let i = idx.unwrap_or(self.cfg.default_terminal);
        let a = self.active;
        let dir = match &self.panes[a].tab().loc {
            Location::Local(p) => p.clone(),
            Location::Archive { file, .. } => file.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            Location::Remote { .. } => {
                self.toast(t("Terminal opens only for local folders"), true);
                return;
            }
        };
        self.launch_profile(i, &dir);
    }

    fn launch_profile(&mut self, idx: usize, dir: &Path) {
        let Some(p) = self.cfg.terminal_profiles.get(idx).cloned() else { return };
        if p.terminal == TerminalKind::BuiltIn {
            self.new_terminal_here(Some(idx), Some(dir.to_path_buf()));
            return;
        }
        if let Err(e) = terminal::open_profile(&p, dir) {
            self.toast(format!("{}: {e}", p.name), true);
        }
    }

    fn start_unpack(&mut self, here: bool) {
        let a = self.active;
        let tab = self.panes[a].tab();
        let other = match self.other() {
            Some(o) => self.panes[o].tab().loc.clone(),
            None => tab.loc.clone(),
        };
        match tab.loc.clone() {
            Location::Local(dir) => {
                let Some(e) = tab.cursor_entry().filter(|e| !e.is_dir) else { return };
                let path = dir.join(&e.name);
                if !archive::is_archive(&path) {
                    self.toast(t("Not a supported archive"), true);
                    return;
                }
                let base = if here { dir.clone() } else { other.local_path().map(|p| p.to_path_buf()).unwrap_or(dir.clone()) };
                let stem = e.name.trim_end_matches(&format!(".{}", ArchiveFormat::detect(&path).map(|f| f.extension()).unwrap_or(""))).to_string();
                let dest = base.join(stem);
                self.show_dialog(Dialog::Unpack { archive: path, dest: dest.to_string_lossy().to_string(), selection: vec![], strip: String::new(), policy: Conflict::Rename }, true);
            }
            Location::Archive { file, inner } => {
                // Inside an archive: extract the selection.
                let sel: Vec<String> = tab.selection().iter().map(|e| join_slash(&inner, &e.name)).collect();
                let dest = other.local_path().map(|p| p.to_path_buf()).or_else(|| file.parent().map(|p| p.to_path_buf())).unwrap_or_default();
                self.show_dialog(Dialog::Unpack { archive: file, dest: dest.to_string_lossy().to_string(), selection: sel, strip: inner, policy: Conflict::Rename }, true);
            }
            Location::Remote { .. } => self.toast(t("Download the archive first"), true),
        }
    }

    /// Show folder `loc` again wherever it is shown, with the cursor on `name` in window `view`.
    fn reload_and_select(&mut self, loc: &Location, view: u64, name: &str) {
        let c = lc!(self);
        for p in all_panes!(self) {
            let tab = p.tab_mut();
            if tab.loc == *loc && tab.is_files() {
                tab.reload(&c);
                if tab.uid == view {
                    tab.select_name(name);
                }
            }
        }
    }

    /// Delete `items` of folder `loc`, asked for in window `view`.
    fn do_delete(&mut self, loc: &Location, view: u64, items: Vec<Entry>, trash: bool) {
        let loc = loc.clone();
        let Some(src) = self.endpoint(&loc) else { return };
        let items: Vec<ops::Item> = items.iter().map(|e| ops::Item { name: e.name.clone(), is_dir: e.is_dir, size: e.size }).collect();
        if let Some(p) = all_panes!(self).find(|p| p.tab().uid == view && p.tab().loc == loc) {
            p.tab_mut().marked.clear();
        }
        let what = if items.len() == 1 { items[0].name.clone() } else { format!("{} {}", items.len(), t("item(s)")) };
        self.spawn_job(format!("{} {what}", t("Deleting")), After::Refresh, Some(loc), vec![], move |p| ops::delete(src, items, trash, p));
    }

    fn show_properties(&mut self) {
        let a = self.active;
        let tab = self.panes[a].tab();
        let Some(e) = tab.cursor_entry().cloned() else { return };
        let mut text = format!("{}: {}\n", t("Name"), e.name);
        text += &format!("{}: {}\n", t("Location"), tab.loc.display(|id| self.remote_label(id)));
        text += &format!("{}: {}\n", t("Type"), if e.is_dir { t("Folder") } else { format!("{} (.{})", t("File"), e.ext) });
        if !e.is_dir || e.size > 0 {
            text += &format!("{}: {} ({} bytes)\n", t("Size"), human_size(e.size), e.size);
        }
        text += &format!("{}: {}\n", t("Modified"), crate::util::format_time(e.modified));
        if let Location::Local(d) = &tab.loc {
            let p = d.join(&e.name);
            if let Ok(m) = std::fs::symlink_metadata(&p) {
                if let Ok(c) = m.created() {
                    text += &format!("{}: {}\n", t("Created"), crate::util::format_time(Some(c)));
                }
                if let Ok(ac) = m.accessed() {
                    text += &format!("{}: {}\n", t("Accessed"), crate::util::format_time(Some(ac)));
                }
                if m.file_type().is_symlink() {
                    if let Ok(target) = std::fs::read_link(&p) {
                        text += &format!("{}: {}\n", t("Link target"), target.display());
                    }
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::{MetadataExt, PermissionsExt};
                    let mode = m.permissions().mode();
                    text += &format!("{}: {:o} ({})\n", t("Permissions"), mode & 0o7777, unix_mode_string(mode));
                    text += &format!("UID/GID: {}/{}\n", m.uid(), m.gid());
                }
                #[cfg(windows)]
                {
                    text += &format!("{}: {}\n", t("Read-only"), m.permissions().readonly());
                }
            }
        }
        self.show_dialog(Dialog::Message { title: t("Properties"), text }, false);
    }

    // ------------------------------------------------------------ keyboard

    /// The next key press while Settings waits for a new shortcut: it goes nowhere else.
    fn capture_shortcut(&mut self, ctx: &egui::Context, action: Action, slot: Option<usize>) {
        let pressed = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } => Some((*key, *physical_key, *modifiers)),
                _ => None,
            })
        });
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_))));
        let Some((key, physical, m)) = pressed else { return };
        if key == Key::Escape && !m.any() {
            self.shortcut_capture = None;
            self.shortcut_note = None;
            return;
        }
        // Digit and punctuation keys report what they type: keep the key itself.
        let is_digit = |k: Key| k.name().len() == 1 && k.name().as_bytes()[0].is_ascii_digit();
        let is_letter = |k: Key| k.name().len() == 1 && k.name().as_bytes()[0].is_ascii_uppercase();
        let key = match physical {
            Some(p) if !is_letter(p) && (m.shift || m.alt || is_digit(p)) => p,
            _ => key,
        };
        let chord = Chord { ctrl: m.ctrl, alt: m.alt, shift: m.shift, cmd: m.mac_cmd, key };
        if let Some(why) = chord.refused_because() {
            self.shortcut_note = Some(why);
            return;
        }
        let current = shortcuts::active();
        let os = current.os();
        let mut note = None;
        if let Some(other) = current.used_by(chord, action) {
            let keys: Vec<Chord> = current.keys(other).iter().copied().filter(|c| *c != chord).collect();
            shortcuts::store(&mut self.cfg.shortcuts, os, other, &keys);
            note = Some(format!("{} {} ({})", t("Taken away from"), other.label(), chord.display(os)));
        }
        let mut keys = current.keys(action).to_vec();
        keys.retain(|c| *c != chord);
        match slot {
            Some(i) if i < keys.len() => keys[i] = chord,
            _ => keys.push(chord),
        }
        shortcuts::store(&mut self.cfg.shortcuts, os, action, &keys);
        if action.works_in_terminals() && !chord.leaves_terminals_alone() && note.is_none() {
            note = Some(t("In a terminal this key stays with the program there (Vim, Emacs…); it works in other windows."));
        }
        shortcuts::install(&self.cfg.shortcuts);
        self.shortcut_note = note;
        self.shortcut_capture = None;
        self.save_cfg();
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        // The address bar's folder tree takes its keys first (↑ ↓ ← → Enter Esc).
        if let Some(side) = self.panes.iter().position(|p| p.tab().tree.is_some()) {
            let res = self.panes[side].tab_mut().tree.as_mut().and_then(|t| t.keys(ctx));
            let handled = res.is_some();
            match res {
                Some(crate::foldertree::Popup::Picked(p)) => {
                    self.panes[side].tab_mut().tree = None;
                    self.active = side;
                    self.panes[side].tab_mut().navigate(Location::Local(p), &lc!(self));
                }
                Some(crate::foldertree::Popup::Closed) => self.panes[side].tab_mut().tree = None,
                _ => {}
            }
            if handled {
                return;
            }
        }
        if let Some((action, slot)) = self.shortcut_capture {
            self.capture_shortcut(ctx, action, slot);
            return;
        }
        let term_mode = self.panes[self.active].tab().is_terminal();
        let keys = shortcuts::active();
        // The palette has its keys in text fields too, and closes by them.
        let palette_key = ctx.input_mut(|i| {
            let before = i.events.len();
            i.events.retain(|e| match e {
                egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } => keys.action_for(*key, *physical_key, *modifiers, term_mode) != Some(Action::Palette),
                _ => true,
            });
            i.events.len() != before
        });
        if palette_key {
            self.palette.toggle();
            return;
        }
        if self.palette.open {
            return;
        }
        if term_mode {
            // The terminal keeps the keyboard; only text fields (e.g. a tab-group name)
            // can take it away. Anything else hands it back to the terminal.
            let a = self.active;
            let tid = self.panes[a].tab().term.as_ref().map(|t| t.id);
            if let Some(id) = ctx.memory(|m| m.focused()) {
                if egui::TextEdit::load_state(ctx, id).is_some() {
                    return;
                }
                if Some(id) != tid {
                    ctx.memory_mut(|m| m.surrender_focus(id));
                    if let Some(t) = &mut self.panes[a].tab_mut().term {
                        t.want_focus = true;
                    }
                }
            }
        } else if let Some(id) = ctx.memory(|m| m.focused()) {
            // Only text fields keep the keyboard; any other focused widget (e.g. a button
            // reached with Tab) gives it back to the file panels.
            if egui::TextEdit::load_state(ctx, id).is_some() {
                return;
            }
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
        if !term_mode {
            self.type_search_keys(ctx);
        }
        // Take the shortcuts out of the input, so that a terminal does not get them too.
        let mut actions = vec![];
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Key { key, physical_key, pressed, modifiers, .. } => match keys.action_for(*key, *physical_key, *modifiers, term_mode) {
                    Some(action) => {
                        if *pressed {
                            actions.push(action);
                        }
                        false
                    }
                    None => true,
                },
                _ => true,
            });
        });
        let shortcut_pressed = !actions.is_empty();
        for action in actions {
            self.run_shortcut(ctx, action);
            // It may have opened something, or switched between a terminal and files.
            if self.dialog.is_some() || self.palette.open || self.panes[self.active].tab().is_terminal() != term_mode {
                return;
            }
        }
        if !term_mode && !shortcut_pressed && self.type_to_search(ctx) {
            return;
        }
        if !term_mode {
            self.file_list_keys(ctx);
        }
    }

    /// Do what a shortcut stands for.
    fn run_shortcut(&mut self, ctx: &egui::Context, action: Action) {
        let a = self.active;
        // Disk usage and Settings windows have no file list.
        if action.needs_file_list() && !self.panes[a].tab().is_files() {
            return;
        }
        match action {
            Action::NextTab => self.strip.cycle(1),
            Action::PrevTab => self.strip.cycle(-1),
            Action::LastUsedTab => {
                if let Some(i) = self.last_used_tab.and_then(|uid| self.strip.index_of(uid)) {
                    self.strip.activate(i);
                }
            }
            Action::MoveTabLeft => self.strip.shift_active(-1),
            Action::MoveTabRight => self.strip.shift_active(1),
            Action::GoToTab(n) => self.strip.go_to(usize::from(n)),
            Action::GoToLastTab => self.strip.go_to(usize::MAX),
            Action::NewTab => self.new_tab(),
            Action::NewTerminalTab => self.new_terminal_here(None, None),
            Action::CloseTab => self.request_close_tabs(&[self.live]),
            Action::CloseWindow => self.request_close_pane(a),
            Action::ReopenTab => self.reopen_closed_tab(),
            Action::FocusWindow(n) => {
                if let Some(side) = self.layout.leaves().get(usize::from(n)).copied() {
                    self.active = side;
                }
            }
            Action::NextWindow => self.active = self.layout.next_after(self.active),
            Action::PrevWindow => self.active = self.layout.prev_before(self.active),
            Action::SplitRight | Action::SplitLeft | Action::SplitDown | Action::SplitUp => {
                let dir = match action {
                    Action::SplitRight => SplitDir::Right,
                    Action::SplitLeft => SplitDir::Left,
                    Action::SplitDown => SplitDir::Down,
                    _ => SplitDir::Up,
                };
                // The same kind of window: a terminal beside a terminal, files beside anything else.
                let terminal = self.panes[a].tab().is_terminal();
                let _ = self.split_pane(a, dir, terminal);
            }
            Action::SwapPanels => self.swap_with_other(),
            Action::View => self.run_cmd(Cmd::View),
            Action::OpenDefault => self.run_cmd(Cmd::OpenDefault),
            Action::NewFile => self.run_cmd(Cmd::NewFile),
            Action::NewFolder => self.run_cmd(Cmd::NewFolder),
            Action::Copy => self.run_cmd(Cmd::Copy),
            Action::Move => self.run_cmd(Cmd::Move),
            Action::Rename => self.run_cmd(Cmd::Rename),
            Action::BatchRename => self.open_batch_rename(),
            Action::Delete => self.run_cmd(Cmd::Delete),
            Action::DeletePermanently => {
                let saved = self.cfg.use_trash;
                self.cfg.use_trash = false;
                self.run_cmd(Cmd::Delete);
                self.cfg.use_trash = saved;
            }
            Action::Compress => self.run_cmd(Cmd::Pack),
            Action::ExtractOther => self.run_cmd(Cmd::ExtractOther),
            Action::Properties => self.show_properties(),
            Action::Checksum => self.run_cmd(Cmd::Checksum),
            Action::SelectAll => self.panes[a].tab_mut().mark_all(),
            Action::InvertSelection => self.panes[a].tab_mut().invert_marks(),
            Action::Back => self.panes[a].tab_mut().go_back(&lc!(self)),
            Action::Forward => self.panes[a].tab_mut().go_forward(&lc!(self)),
            Action::UpFolder => {
                if let Some(d) = self.panes[a].tab_mut().du.as_mut() {
                    d.go_up();
                } else if self.panes[a].tab().is_files() {
                    self.panes[a].tab_mut().go_up(&lc!(self));
                }
            }
            Action::Filter => {
                let q = self.panes[a].tab().search.as_ref().map(|s| s.query.clone()).unwrap_or_else(|| self.panes[a].tab().filter_text.clone());
                self.palette.open_with(&q);
            }
            Action::FocusPath => self.panes[a].tab_mut().focus_path = true,
            Action::Bookmark => self.run_cmd(Cmd::Bookmark),
            Action::Refresh => {
                if let Some(d) = self.panes[a].tab_mut().du.as_mut() {
                    d.rescan();
                } else {
                    self.run_cmd(Cmd::Refresh);
                }
            }
            Action::ToggleHidden => {
                self.cfg.show_hidden = !self.cfg.show_hidden;
                self.rebuild_views();
            }
            Action::Palette => self.palette.toggle(),
            Action::OpenTerminal => self.open_terminal(),
            Action::QuickView => self.toggle_quick_view(),
            Action::Sidebar => {
                self.cfg.show_sidebar = !self.cfg.show_sidebar;
                self.save_cfg();
            }
            Action::Transfers => self.open_transfers(),
            Action::Connect => self.open_connect_dialog(None),
            Action::Backup => self.backup.open = true,
            Action::Settings => self.open_settings(self.settings_page),
            Action::Help => self.show_help = true,
            Action::ZoomIn => self.set_ui_scale(ctx, self.cfg.ui_scale + 0.1),
            Action::ZoomOut => self.set_ui_scale(ctx, self.cfg.ui_scale - 0.1),
            Action::ZoomReset => self.set_ui_scale(ctx, 1.0),
        }
        // Whatever is now in front gets the keyboard.
        self.sync_tab();
        if matches!(
            action,
            Action::NextTab | Action::PrevTab | Action::LastUsedTab | Action::GoToTab(_) | Action::GoToLastTab | Action::FocusWindow(_) | Action::NextWindow | Action::PrevWindow
        ) {
            self.quick = None;
            self.focus_window();
        }
    }

    /// Zoom the whole interface (as Settings → UI scale does), and save it.
    fn set_ui_scale(&mut self, ctx: &egui::Context, scale: f32) {
        let scale = ((scale * 20.0).round() / 20.0).clamp(0.7, 2.0);
        if scale != self.cfg.ui_scale {
            self.cfg.ui_scale = scale;
            ctx.set_zoom_factor(scale);
            self.save_cfg();
        }
    }

    /// Typing in a file window opens a small search box in it with what was typed: it
    /// filters the folder as you type, Enter keeps the filter, Esc closes it and shows
    /// the whole folder again. Space marks files, so it does not start a search.
    fn type_to_search(&mut self, ctx: &egui::Context) -> bool {
        if !self.panes[self.active].tab().is_files() || self.type_search.is_some() {
            return false;
        }
        let typed: String = ctx.input(|i| {
            // Shortcut keys; Ctrl+Alt is AltGr, which types characters on Windows.
            let m = i.modifiers;
            if m.mac_cmd || ((m.ctrl || m.command) && !m.alt) {
                return String::new();
            }
            i.events.iter().filter_map(|e| if let egui::Event::Text(s) = e { Some(s.as_str()) } else { None }).collect()
        });
        if typed.is_empty() || typed.starts_with(char::is_whitespace) || typed.chars().any(char::is_control) {
            return false;
        }
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
        let tab = self.panes[self.active].tab();
        self.type_search = Some(TypeSearch { side: self.active, uid: tab.uid, loc: tab.loc.clone(), before: tab.filter_text.clone(), text: typed });
        self.apply_type_search();
        true
    }

    /// The search box's keys, taken before shortcuts: text and Backspace edit it, Enter
    /// keeps the filter, Esc puts the folder back. Arrows still move in the list.
    fn type_search_keys(&mut self, ctx: &egui::Context) {
        let Some(ts) = &self.type_search else { return };
        // Another window or tab came to the front: the search ends there.
        if ts.side != self.active || self.panes.get(ts.side).is_none_or(|p| p.tab().uid != ts.uid) {
            self.end_type_search(false);
            return;
        }
        // It went into another folder (a double click): the search was for the one before.
        if self.panes[ts.side].tab().loc != ts.loc {
            self.type_search = None;
            return;
        }
        let mut edits: Vec<TypeSearchKey> = vec![];
        ctx.input_mut(|i| {
            let m = i.modifiers;
            let shortcut = m.mac_cmd || ((m.ctrl || m.command) && !m.alt);
            i.events.retain(|e| match e {
                egui::Event::Text(t) if !shortcut => {
                    edits.push(TypeSearchKey::Text(t.chars().filter(|c| !c.is_control()).collect()));
                    false
                }
                egui::Event::Key { key, pressed, modifiers, .. } if !(modifiers.mac_cmd || modifiers.ctrl || modifiers.command) => match key {
                    Key::Backspace | Key::Enter | Key::Escape => {
                        if *pressed {
                            edits.push(match key {
                                Key::Backspace => TypeSearchKey::Back,
                                Key::Enter => TypeSearchKey::Keep,
                                _ => TypeSearchKey::Cancel,
                            });
                        }
                        false
                    }
                    _ => true,
                },
                _ => true,
            });
        });
        for e in edits {
            let Some(ts) = self.type_search.as_mut() else { break };
            match e {
                TypeSearchKey::Text(t) => ts.text.push_str(&t),
                TypeSearchKey::Back => {
                    ts.text.pop();
                    if ts.text.is_empty() {
                        self.end_type_search(false);
                        continue;
                    }
                }
                TypeSearchKey::Keep => {
                    self.end_type_search(true);
                    continue;
                }
                TypeSearchKey::Cancel => {
                    self.end_type_search(false);
                    continue;
                }
            }
            self.apply_type_search();
        }
    }

    /// Filter the window by what is typed.
    fn apply_type_search(&mut self) {
        let Some(ts) = &self.type_search else { return };
        let (side, text) = (ts.side, ts.text.trim().to_string());
        let lc = lc!(self);
        let tab = self.panes[side].tab_mut();
        if tab.filter_text != text {
            tab.set_filter(&text, &lc);
            // On the first match, so that Enter, Enter opens it.
            if let Some(first) = tab.view.iter().position(|&i| !tab.entries[i].is_parent) {
                tab.cursor = first;
                tab.ensure_visible = true;
            }
        }
    }

    /// Close the search box, keeping its filter or putting back the one from before.
    fn end_type_search(&mut self, keep: bool) {
        let Some(ts) = self.type_search.take() else { return };
        if keep {
            return;
        }
        let lc = lc!(self);
        if let Some(tab) = self.panes.get_mut(ts.side).map(|p| p.tab_mut()).filter(|t| t.uid == ts.uid) {
            if tab.filter_text != ts.before {
                tab.set_filter(&ts.before, &lc);
            }
        }
    }

    /// The search box, in the lower right corner of its window.
    fn type_search_ui(&mut self, ctx: &egui::Context) {
        let Some(ts) = &self.type_search else { return };
        let Some(&(_, rect)) = self.pane_rects.iter().find(|(s, _)| *s == ts.side) else { return };
        let th = self.theme;
        let tab = self.panes[ts.side].tab();
        let matches = tab.view.iter().filter(|&&i| !tab.entries[i].is_parent).count();
        let text = ts.text.clone();
        // As wide as the query needs (an SQL-like one can be long), within the window.
        let text_w = ctx.fonts(|f| f.layout_no_wrap(format!("{text}▏"), egui::FontId::proportional(13.5), th.text).size().x);
        let width = (text_w + 130.0).clamp(260.0, (rect.width() - 28.0).max(260.0)).min(rect.width() - 24.0).max(120.0);
        let pos = egui::pos2(rect.right() - width - 14.0, rect.bottom() - 28.0 - 44.0);
        egui::Area::new(egui::Id::new("type-search")).order(egui::Order::Foreground).fixed_pos(pos).interactable(false).show(ctx, |ui| {
            egui::Frame::new()
                .fill(th.panel)
                .stroke(egui::Stroke::new(1.0_f32, th.accent))
                .corner_radius(8.0)
                .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 10, spread: 0, color: egui::Color32::from_black_alpha(if th.dark { 90 } else { 40 }) })
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(width - 20.0);
                    let count = if matches == 1 { t("1 match") } else { format!("{} {}", crate::table::fmt_count(matches as u64), t("matches")) };
                    let count_w = ctx.fonts(|f| f.layout_no_wrap(count.clone(), egui::FontId::proportional(11.5), th.weak).size().x);
                    // The count only when the query fits beside it; the query is what matters.
                    let with_count = text_w + count_w + 40.0 <= width - 20.0;
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        icons::search(ui.painter(), r, th.weak);
                        // A caret that blinks after the text.
                        let blink = (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0;
                        let shown = if blink { format!("{text}▏") } else { format!("{text} ") };
                        let room = ui.available_width() - if with_count { count_w + 12.0 } else { 0.0 };
                        ui.allocate_ui(egui::vec2(room.max(20.0), 18.0), |ui| {
                            ui.add(egui::Label::new(RichText::new(shown).size(13.5).color(th.text)).truncate());
                        });
                        if with_count {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(&count).size(11.5).color(if matches == 0 { th.danger } else { th.weak }));
                            });
                        }
                    });
                });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn file_list_keys(&mut self, ctx: &egui::Context) {
        let keys: Vec<(Key, Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        // Tab must not move egui's keyboard focus.
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { key: Key::Tab, .. })));
        let has_list = self.panes[self.active].tab().is_files();
        for (key, m) in keys {
            let a = self.active;
            let shift = m.shift;
            let plain = !m.command && !m.alt && !m.ctrl;
            match key {
                Key::Tab => {
                    self.active = if shift { self.layout.prev_before(self.active) } else { self.layout.next_after(self.active) };
                    self.quick = None;
                    self.focus_window();
                }
                Key::Escape => {
                    if crate::pane::drag_payload(ctx).is_some() {
                        egui::DragAndDrop::clear_payload(ctx);
                    } else if self.viewer.is_some() {
                        self.viewer = None;
                    } else if self.panes[a].tab().marked.is_empty() && self.panes[a].tab().is_filtered() {
                        self.panes[a].tab_mut().clear_search(&lc!(self));
                    } else {
                        self.panes[a].tab_mut().marked.clear();
                    }
                }
                _ if !has_list => {}
                Key::ArrowUp if plain => self.panes[a].tab_mut().move_cursor(-1, shift),
                Key::ArrowDown if plain => self.panes[a].tab_mut().move_cursor(1, shift),
                Key::ArrowLeft if plain && !shift => {
                    self.panes[a].tab_mut().move_cursor(isize::MIN / 2, false);
                }
                Key::ArrowRight if plain && !shift => {
                    self.panes[a].tab_mut().move_cursor(isize::MAX / 2, false);
                }
                Key::PageUp if plain => {
                    let n = self.panes[a].tab().page_rows();
                    self.panes[a].tab_mut().move_cursor(-n, shift);
                }
                Key::PageDown if plain => {
                    let n = self.panes[a].tab().page_rows();
                    self.panes[a].tab_mut().move_cursor(n, shift);
                }
                Key::Home if plain => self.panes[a].tab_mut().move_cursor(isize::MIN / 2, shift),
                Key::End if plain => self.panes[a].tab_mut().move_cursor(isize::MAX / 2, shift),
                Key::Enter if plain => self.open_cursor(),
                Key::Space if plain => self.panes[a].tab_mut().toggle_mark_cursor(),
                Key::Insert if plain => {
                    self.panes[a].tab_mut().toggle_mark_cursor();
                    self.panes[a].tab_mut().move_cursor(1, false);
                }
                _ => {}
            }
        }
    }

    fn rebuild_views(&mut self) {
        let c = lc!(self);
        let quick = &self.cfg.quick_filters;
        for p in all_panes!(self) {
            let tab = p.tab_mut();
            // An edited category applies at once; a deleted one turns off.
            if let Some(names) = tab.quick.as_ref().map(|q| q.names.clone()) {
                tab.quick = crate::pane::QuickActive::from_names(&names, quick);
            }
            tab.rebuild_view(&c);
        }
    }

    /// Quick filter of the focused file window: tick or untick category `i`, or (None)
    /// show all files.
    fn set_quick_filter(&mut self, i: Option<usize>) {
        let a = self.active;
        if !self.panes[a].tab().is_files() {
            return;
        }
        let q = match i.and_then(|i| self.cfg.quick_filters.get(i)) {
            Some(f) => crate::pane::QuickActive::toggle(self.panes[a].tab().quick.as_ref(), &f.name, &self.cfg.quick_filters),
            None => None,
        };
        let c = lc!(self);
        self.panes[a].tab_mut().set_quick(q, &c);
    }

    /// Settings → Quick filters: the categories and their extensions.
    fn quick_filter_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        ui.label(RichText::new(t("The funnel button of a file window shows only the kinds of file you tick (one or several). Folders stay visible, and the filter stays on while you browse. Add your own lists here.")).size(12.0).color(th.weak));
        ui.add_space(6.0);
        let mut frame = card_frame(th).begin(ui);
        {
        let ui = &mut frame.content_ui;
        ui.set_width(ui.available_width());
        let n = self.cfg.quick_filters.len();
        let mut edit: Option<(usize, i32)> = None; // -1 up, 1 down, 0 delete
        // One width for every row (icon, name, count and buttons take ~310 px), so
        // the rows line up and never widen the page.
        let ext_w = (ui.available_width() - 310.0).max(160.0);
        for i in 0..n {
            let q = &mut self.cfg.quick_filters[i];
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                crate::pane::quick_filter_icon(ui.painter(), r.shrink(5.0), &q.ext_list(), th);
                changed |= ui.add(egui::TextEdit::singleline(&mut q.name).desired_width(130.0).hint_text(t("Name"))).changed();
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut q.exts).desired_width(ext_w).hint_text(t("Extensions, e.g. jpg png gif")))
                    .on_hover_text(t("Extensions separated by spaces or commas (dots are optional)"))
                    .changed();
                let (cr, cresp) = ui.allocate_exact_size(egui::vec2(26.0, 24.0), egui::Sense::hover());
                ui.painter().text(cr.center(), egui::Align2::CENTER_CENTER, q.ext_list().len().to_string(), egui::FontId::proportional(11.5), th.weak);
                cresp.on_hover_text(t("Extensions"));
                if icons::icon_button(ui, 24.0, i > 0, th, |p, r, c| icons::chevron(p, r, 0, c)).on_hover_text(t("Move up")).clicked() {
                    edit = Some((i, -1));
                }
                if icons::icon_button(ui, 24.0, i + 1 < n, th, |p, r, c| icons::chevron(p, r, 2, c)).on_hover_text(t("Move down")).clicked() {
                    edit = Some((i, 1));
                }
                if icons::icon_button(ui, 24.0, true, th, icons::mi_trash).on_hover_text(t("Remove")).clicked() {
                    edit = Some((i, 0));
                }
            });
        }
        match edit {
            Some((i, -1)) => self.cfg.quick_filters.swap(i, i - 1),
            Some((i, 1)) => self.cfg.quick_filters.swap(i, i + 1),
            Some((i, _)) => {
                self.cfg.quick_filters.remove(i);
            }
            None => {}
        }
        changed |= edit.is_some();
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button(format!("+ {}", t("New quick filter"))).clicked() {
                let name = crate::config::unused_quick_filter_name(&self.cfg.quick_filters, &t("New filter"));
                self.cfg.quick_filters.push(crate::config::QuickFilter::new(&name, ""));
                changed = true;
            }
            if ui.button(t("Restore the built-in filters")).on_hover_text(t("Adds the built-in categories that are missing and resets their extensions")).clicked() {
                for d in crate::config::default_quick_filters() {
                    match self.cfg.quick_filters.iter_mut().find(|q| q.name == d.name) {
                        Some(q) => q.exts = d.exts,
                        None => self.cfg.quick_filters.push(d),
                    }
                }
                changed = true;
            }
        });
        }
        frame.end(ui);
        changed
    }

    fn toggle_quick_view(&mut self) {
        self.cfg.quick_view = !self.cfg.quick_view;
        self.quick = None;
    }

    // ------------------------------------------------------------ tabs and windows

    /// The shown tab keeps its windows in `panes` / `layout` / `active`; every other
    /// tab keeps its own in `strip`. When the strip shows another tab (found by uid,
    /// so the strip's model functions need not know), swap them.
    fn sync_tab(&mut self) {
        let want = self.strip.tabs[self.strip.active].uid;
        if want == self.live {
            return;
        }
        self.last_used_tab = Some(self.live);
        let panes = std::mem::take(&mut self.panes);
        let layout = std::mem::replace(&mut self.layout, Layout::Leaf(0));
        if let Some(w) = self.strip.tabs.iter_mut().find(|w| w.uid == self.live) {
            w.panes = panes;
            w.layout = layout;
            w.active = self.active;
            w.prev_active = self.prev_active;
        }
        let w = &mut self.strip.tabs[self.strip.active];
        self.panes = std::mem::take(&mut w.panes);
        self.layout = w.layout.clone();
        let (active, prev) = (w.active, w.prev_active);
        if self.panes.is_empty() {
            let mut p = Pane::home();
            p.tab_mut().reload(&lc!(self));
            self.panes.push(p);
            self.layout = Layout::Leaf(0);
        }
        let n = self.panes.len();
        self.active = active.min(n - 1);
        self.prev_active = prev.min(n - 1);
        self.last_active = self.active;
        self.live = want;
        for p in &mut self.panes {
            if let Some(t) = &mut p.tab_mut().term {
                t.activity = false;
            }
        }
        self.focus_window();
        self.quick = None;
    }

    /// Give the focused window the keyboard (a terminal takes it explicitly).
    fn focus_window(&mut self) {
        let a = self.active;
        if let Some(t) = &mut self.panes[a].tab_mut().term {
            t.want_focus = true;
            t.activity = false;
        }
    }

    /// Show tab `i` of the strip.
    fn show_tab(&mut self, i: usize) {
        self.strip.activate(i);
        self.sync_tab();
    }

    /// How each tab looks in the tab bar.
    fn tab_infos(&self) -> Vec<TabInfo> {
        self.strip
            .tabs
            .iter()
            .map(|w| {
                let (panes, layout, active) = if w.uid == self.live { (&self.panes[..], &self.layout, self.active) } else { (&w.panes[..], &w.layout, w.active) };
                let focused = panes.get(active).or(panes.first()).map(|p| p.tab());
                let default_title = focused.map(|t| t.title()).unwrap_or_default();
                let title = w.custom_title.clone().filter(|c| !c.trim().is_empty()).unwrap_or_else(|| default_title.clone());
                let tooltip = panes
                    .iter()
                    .map(|p| {
                        let t = p.tab();
                        match &t.term {
                            Some(s) => format!("{}  ·  {}", t.title(), s.cwd.display()),
                            None if t.transfers || t.is_tool() => t.title(),
                            None => t.loc.display(|id| self.remote_label(id)),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let terms = panes.iter().filter_map(|p| p.tab().term.as_ref());
                let (bell, activity) = terms.fold((false, false), |(b, a), t| (b || t.bell.is_some(), a || t.activity));
                TabInfo {
                    title,
                    default_title,
                    tooltip,
                    term: focused.and_then(|t| t.term.as_ref()).map(|t| t.exited.is_some()),
                    disk_usage: focused.is_some_and(|t| t.is_disk_usage()),
                    settings: focused.is_some_and(|t| t.settings),
                    transfers: focused.is_some_and(|t| t.transfers),
                    tool: focused.and_then(|t| match (&t.checksum, &t.renamer) {
                        (Some(_), _) => Some(icons::mi_checksum as fn(&egui::Painter, egui::Rect, egui::Color32)),
                        (_, Some(_)) => Some(icons::mi_rename as fn(&egui::Painter, egui::Rect, egui::Color32)),
                        _ => None,
                    }),
                    bell,
                    activity,
                    cells: if panes.len() > 1 { crate::tabs::layout_cells(layout, active) } else { Vec::new() },
                }
            })
            .collect()
    }

    /// Position and group for a new tab next to the shown one.
    fn next_slot(&self) -> (usize, Option<u32>) {
        (self.strip.active + 1, self.strip.tabs[self.strip.active].group)
    }

    fn new_tab(&mut self) {
        let (at, group) = self.next_slot();
        self.new_files_tab(at, group, None);
    }

    /// Folder for a new tab or window opened next to window `side`.
    fn tab_dir(&self, side: usize) -> PathBuf {
        let tab = self.panes[side].tab();
        if let Some(t) = &tab.term {
            return t.cwd.clone();
        }
        match &tab.loc {
            Location::Local(p) => p.clone(),
            Location::Archive { file, .. } => file.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            Location::Remote { .. } => dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
        }
    }

    /// A file window showing `loc`, already loading.
    fn files_pane(&self, loc: Location) -> Pane {
        let mut p = Pane::new(loc);
        p.tab_mut().reload(&lc!(self));
        p
    }

    /// A terminal window in `dir` (a toast explains when it cannot start).
    fn terminal_pane(&mut self, profile: Option<usize>, dir: &Path) -> Option<Pane> {
        match self.spawn_terminal(dir, profile) {
            Ok(s) => {
                let mut tab = Tab::terminal(s);
                if let Some(p) = profile.and_then(|i| self.cfg.terminal_profiles.get(i)) {
                    if !p.name.is_empty() && profile != Some(self.cfg.default_terminal) {
                        tab.custom_title = Some(p.name.clone());
                    }
                }
                Some(Pane::from_tab(tab))
            }
            Err(e) => {
                self.toast(format!("{}: {e}", t("Cannot start terminal")), true);
                None
            }
        }
    }

    /// Add a tab holding the window `pane` at `at` and show it.
    fn insert_workspace(&mut self, at: usize, w: Workspace, group: Option<u32>) {
        self.strip.insert_tab(at, w, group);
        self.sync_tab();
    }

    fn new_files_tab(&mut self, at: usize, group: Option<u32>, loc: Option<Location>) {
        let loc = loc.unwrap_or_else(|| {
            let tab = self.panes[self.active].tab();
            if tab.is_terminal() { Location::Local(self.tab_dir(self.active)) } else { tab.loc.clone() }
        });
        let pane = self.files_pane(loc);
        self.insert_workspace(at, Workspace::single(pane), group);
    }

    fn spawn_terminal(&self, dir: &Path, profile: Option<usize>) -> Res<TermSession> {
        let prof = profile.or(Some(self.cfg.default_terminal)).and_then(|i| self.cfg.terminal_profiles.get(i));
        let argv = prof.and_then(terminal::shell_argv);
        let dir = if dir.is_dir() { dir.to_path_buf() } else { dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")) };
        let mut s = TermSession::spawn(&self.egui_ctx, &dir, argv, self.cfg.term_scrollback)?;
        s.run_at_start(prof.map(terminal::startup_lines).unwrap_or_default());
        s.theme = prof.filter(|p| !p.theme.is_empty()).map(|p| theme::term_by_name(&p.theme));
        s.profile = prof.map(|p| p.name.clone());
        Ok(s)
    }

    /// Open a built-in terminal tab at position `at`.
    fn new_terminal_tab(&mut self, at: usize, group: Option<u32>, profile: Option<usize>, dir: Option<PathBuf>) {
        let dir = dir.unwrap_or_else(|| self.tab_dir(self.active));
        if let Some(pane) = self.terminal_pane(profile, &dir) {
            self.insert_workspace(at, Workspace::single(pane), group);
        }
    }

    /// New terminal tab next to the shown tab.
    fn new_terminal_here(&mut self, profile: Option<usize>, dir: Option<PathBuf>) {
        let (at, group) = self.next_slot();
        self.new_terminal_tab(at, group, profile, dir);
    }

    /// Close the focused window, or the whole tab when it has only one.
    fn close_current(&mut self) {
        if self.panes.len() > 1 {
            self.close_pane(self.active);
        } else {
            self.close_tabs(&[self.live]);
        }
    }

    fn remember_closed(&mut self, panes: &[Pane], layout: &Layout, title: Option<String>) {
        let views = panes.iter().map(ClosedView::of).collect();
        self.closed_tabs.push(ClosedTab { views, layout: layout.clone(), title });
        if self.closed_tabs.len() > 25 {
            self.closed_tabs.remove(0);
        }
    }

    /// Close whole tabs. The last tab stays: it turns into a single file window.
    fn close_tabs(&mut self, uids: &[u64]) {
        for &uid in uids {
            let Some(i) = self.strip.index_of(uid) else { continue };
            if self.strip.tabs.len() == 1 {
                if self.panes.len() == 1 && self.panes[0].tab().is_files() {
                    continue;
                }
                let dir = self.tab_dir(self.active);
                let fresh = self.files_pane(Location::Local(dir));
                let old = std::mem::replace(&mut self.panes, vec![fresh]);
                let layout = std::mem::replace(&mut self.layout, Layout::Leaf(0));
                let title = self.strip.tabs[0].custom_title.take();
                self.remember_closed(&old, &layout, title);
                (self.active, self.prev_active, self.last_active) = (0, 0, 0);
                continue;
            }
            let mut w = self.strip.remove_tab(i);
            if uid == self.live {
                w.panes = std::mem::take(&mut self.panes);
                w.layout = self.layout.clone();
            }
            self.remember_closed(&w.panes, &w.layout, w.custom_title.clone());
            self.sync_tab();
        }
        self.quick = None;
    }

    /// A window like the closed one (a terminal starts anew, an analysis scans again).
    fn revive(&mut self, v: ClosedView) -> Pane {
        match v {
            ClosedView::Files(loc) => self.files_pane(loc),
            ClosedView::Terminal(dir) => match self.terminal_pane(None, &dir) {
                Some(p) => p,
                None => self.files_pane(Location::Local(dir)),
            },
            ClosedView::DiskUsage(dir, depth) => Pane::from_tab(Tab::disk_usage(dir, depth)),
            ClosedView::Settings => Pane::from_tab(Tab::settings()),
            ClosedView::Transfers => Pane::from_tab(Tab::transfers()),
        }
    }

    /// Analyse the disk usage of `dir` in a new tab next to the shown one.
    fn open_disk_usage(&mut self, dir: PathBuf) {
        if !dir.is_dir() {
            self.toast(t("Disk usage works on local folders"), true);
            return;
        }
        let (at, group) = self.next_slot();
        self.insert_workspace(at, Workspace::single(Pane::from_tab(Tab::disk_usage(dir, 1))), group);
    }

    fn reopen_closed_tab(&mut self) {
        let Some(c) = self.closed_tabs.pop() else { return };
        let mut panes = Vec::new();
        for v in c.views {
            let pane = self.revive(v);
            panes.push(pane);
        }
        let layout = if c.layout.leaves().len() == panes.len() { c.layout } else { Layout::Leaf(0) };
        panes.truncate(layout.leaves().len());
        let mut w = Workspace::new(panes, layout);
        w.custom_title = c.title;
        let at = self.strip.tabs.len();
        self.insert_workspace(at, w, None);
    }

    /// Take the window showing view `view` out of its tab (shown or not). `None`
    /// when it is not found or is the only window of its tab.
    fn take_window(&mut self, view: u64) -> Option<Pane> {
        if let Some(side) = self.panes.iter().position(|p| p.tab().uid == view) {
            if self.panes.len() < 2 {
                return None;
            }
            return Some(self.remove_pane(side));
        }
        for w in &mut self.strip.tabs {
            let Some(i) = w.panes.iter().position(|p| p.tab().uid == view) else { continue };
            if w.panes.len() < 2 || !w.layout.remove(i) {
                return None;
            }
            let p = w.panes.remove(i);
            let fix = |k: usize| if k > i { k - 1 } else if k == i { usize::MAX } else { k };
            let prev = fix(w.prev_active);
            w.active = match fix(w.active) {
                usize::MAX if prev != usize::MAX => prev,
                usize::MAX => 0,
                k => k,
            }
            .min(w.panes.len() - 1);
            w.prev_active = if prev == usize::MAX { w.active } else { prev.min(w.panes.len() - 1) };
            return Some(p);
        }
        None
    }

    /// A window dropped between tabs: it becomes a tab of its own there.
    fn detach_window(&mut self, view: u64, at: usize, group: Option<u32>) {
        if let Some(p) = self.take_window(view) {
            self.insert_workspace(at, Workspace::single(p), group);
        }
    }

    /// The first side of window `target` in `layout` where `sub` still fits.
    fn dock_dir(layout: &Layout, target: usize, sub: &Layout) -> Option<SplitDir> {
        [SplitDir::Right, SplitDir::Down, SplitDir::Left, SplitDir::Up].into_iter().find(|d| layout.can_dock(target, sub, *d))
    }

    /// A window dropped onto another tab: it moves into that tab (beside its
    /// focused window), which is then shown.
    fn window_into_tab(&mut self, view: u64, tab: u64) {
        let Some(i) = self.strip.index_of(tab).filter(|_| tab != self.live) else { return };
        let w = &self.strip.tabs[i];
        let Some(dir) = Self::dock_dir(&w.layout, w.active, &Layout::Leaf(usize::MAX)) else {
            self.toast(t("At most 2 columns and 2 rows of panes"), true);
            return;
        };
        let Some(pane) = self.take_window(view) else { return };
        let Some(i) = self.strip.index_of(tab) else { return };
        let w = &mut self.strip.tabs[i];
        w.panes.push(pane);
        let new = w.panes.len() - 1;
        let target = w.active;
        w.layout.split(target, new, dir);
        w.prev_active = target;
        w.active = new;
        self.show_tab(i);
    }

    /// Put the windows of tab `uid` beside window `target` of the shown tab
    /// (`dir` = None: wherever they fit). The other tab goes away.
    fn merge_tab(&mut self, uid: u64, target: usize, dir: Option<SplitDir>) {
        let Some(i) = self.strip.index_of(uid).filter(|_| uid != self.live) else { return };
        let mut sub = self.strip.tabs[i].layout.clone();
        sub.offset(self.panes.len());
        let dir = match dir {
            Some(d) => Some(d).filter(|d| self.layout.can_dock(target, &sub, *d)),
            None => Self::dock_dir(&self.layout, target, &sub),
        };
        let Some(dir) = dir else {
            self.toast(t("At most 2 columns and 2 rows of panes"), true);
            return;
        };
        let w = self.strip.remove_tab(i);
        let n = self.panes.len();
        self.panes.extend(w.panes);
        self.layout.dock(target, sub, dir);
        self.prev_active = target;
        self.active = n + w.active;
        // Removing a tab before the shown one shifts indices only; the shown uid stays.
        self.sync_tab();
        self.focus_window();
        self.quick = None;
    }

    /// Move window `src` beside window `target` of the shown tab.
    fn move_window(&mut self, src: usize, target: usize, dir: SplitDir) {
        let mut l = self.layout.clone();
        if src == target || !l.detach(src) || !l.dock(target, Layout::Leaf(src), dir) || !Layout::fits(&l) {
            self.toast(t("At most 2 columns and 2 rows of panes"), true);
            return;
        }
        self.layout = l;
        self.prev_active = target;
        self.active = src;
        self.focus_window();
        self.quick = None;
    }

    /// Swap two windows of the shown tab; the dragged one (`src`) keeps the focus.
    fn swap_windows(&mut self, src: usize, target: usize) {
        if src == target {
            return;
        }
        self.panes.swap(src, target);
        self.prev_active = src;
        self.active = target;
        self.focus_window();
        self.quick = None;
    }

    fn handle_tab_event(&mut self, e: TabEvent) {
        match e {
            TabEvent::NewFiles { at, group } => self.new_files_tab(at, group, None),
            TabEvent::NewTerminal { at, group, profile } => self.new_terminal_tab(at, group, profile, None),
            TabEvent::Close(uids) => self.request_close_tabs(&uids),
            TabEvent::Duplicate(uid) => self.duplicate_tab(uid),
            TabEvent::Focus => self.focus_window(),
            TabEvent::MergeIntoShown(uid) => self.merge_tab(uid, self.active, None),
            TabEvent::Detach { view, at, group } => self.detach_window(view, at, group),
            TabEvent::WindowInto { view, tab } => self.window_into_tab(view, tab),
            TabEvent::ReopenClosed => self.reopen_closed_tab(),
        }
    }

    /// Duplicate a tab with all its windows (terminals start anew in the same folder).
    fn duplicate_tab(&mut self, uid: u64) {
        let Some(i) = self.strip.index_of(uid) else { return };
        let (panes, layout, active) = if uid == self.live {
            (&self.panes, self.layout.clone(), self.active)
        } else {
            let w = &self.strip.tabs[i];
            (&w.panes, w.layout.clone(), w.active)
        };
        let views: Vec<ClosedView> = panes.iter().map(ClosedView::of).collect();
        let (group, title) = (self.strip.tabs[i].group, self.strip.tabs[i].custom_title.clone());
        let mut new = Vec::new();
        for v in views {
            new.push(self.revive(v));
        }
        let mut w = Workspace::new(new, layout);
        w.active = active;
        w.custom_title = title;
        self.insert_workspace(i + 1, w, group);
    }

    fn handle_term_response(&mut self, side: usize, r: crate::termview::TermResponse) {
        if r.focus && self.active != side {
            self.active = side;
            self.quick = None;
        }
        if r.font_delta != 0.0 {
            let (sizes, old) = (crate::fonts::TERM_SIZES, self.cfg.term_font_size);
            self.cfg.term_font_size = (old + r.font_delta).clamp(sizes.start().min(old), sizes.end().max(old));
            self.term_font_saved_at = Some(Instant::now());
        }
        if let Some(dir) = r.open_dir {
            let (at, group) = self.next_slot();
            self.new_files_tab(at, group, Some(Location::Local(dir)));
        }
        if let Some((dir, terminal)) = r.split {
            self.split_pane(side, dir, terminal);
        }
        if r.restart {
            let tab = self.panes[side].tab();
            if let Some(term) = &tab.term {
                let (dir, argv, th, startup) = (term.cwd.clone(), term.argv.clone(), term.theme, term.startup.clone());
                let dir = if dir.is_dir() { dir } else { dirs::home_dir().unwrap_or_default() };
                match TermSession::spawn(&self.egui_ctx, &dir, argv, self.cfg.term_scrollback) {
                    Ok(mut s) => {
                        s.theme = th;
                        s.run_at_start(startup);
                        self.panes[side].tab_mut().term = Some(Box::new(s));
                    }
                    Err(e) => self.toast(format!("{}: {e}", t("Cannot start terminal")), true),
                }
            }
        }
        if r.close {
            self.active = side;
            self.close_current();
        }
    }

    /// The agent the AI button in the terminals' info bars starts (`None`: no button),
    /// and every agent found. Starts looking for them the first time a terminal is shown.
    fn ai_button(&mut self) -> (Option<crate::agents::Found>, Vec<crate::agents::Found>) {
        if !self.cfg.term_agents {
            return (None, Vec::new());
        }
        if self.agents.is_none() && self.panes.iter().any(|p| p.tab().is_terminal()) {
            self.find_agents(false);
        }
        let found = self.agents.clone().unwrap_or_default();
        (crate::agents::pick(&found, &self.cfg.term_agent).cloned(), found)
    }

    /// Look for the installed AI coding agents (again, when `again`).
    fn find_agents(&mut self, again: bool) {
        if self.agents_rx.is_some() || (self.agents.is_some() && !again) {
            return;
        }
        // Tests (and README screenshots) do not depend on what this computer has.
        if cfg!(test) {
            self.agents.get_or_insert_with(Vec::new);
            return;
        }
        self.agents_rx = Some(crate::agents::detect_in_background(&self.egui_ctx));
    }

    fn poll_agents(&mut self) {
        if let Some(found) = self.agents_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.agents = Some(found);
            self.agents_rx = None;
        }
    }

    fn agent_act(&mut self, side: usize, act: crate::agents::AgentAct) {
        use crate::agents::AgentAct;
        match act {
            AgentAct::Run(id) => {
                let Some(run) = self.agents.as_ref().and_then(|v| v.iter().find(|f| f.id == id)).map(|f| f.run.clone()) else { return };
                self.active = side;
                if let Some(term) = self.panes[side].tab_mut().term.as_mut() {
                    term.want_focus = true;
                    // Nothing is sent unless the shell waits at its prompt (the button is
                    // greyed out otherwise).
                    term.run_program(&run);
                }
            }
            AgentAct::Choose(id) => {
                self.cfg.term_agent = id.to_string();
                self.save_cfg();
            }
            AgentAct::HideButton => {
                self.cfg.term_agents = false;
                self.save_cfg();
                self.toast(t("The AI button is hidden. Settings → Terminal shows it again."), false);
            }
            AgentAct::Settings => self.open_settings(SettingsPage::Terminal),
        }
    }

    /// Poll every terminal (also in hidden tabs): titles, folder, exit.
    fn poll_terminals(&mut self, ctx: &egui::Context) {
        // A pinch changes the text size many times a second: save once it settles.
        if let Some(at) = self.term_font_saved_at {
            if at.elapsed() >= Duration::from_millis(800) {
                self.term_font_saved_at = None;
                self.save_cfg();
            } else {
                ctx.request_repaint_after(Duration::from_millis(800) - at.elapsed());
            }
        }
        let mut done: Vec<u64> = Vec::new();
        let close = self.cfg.term_close_on_exit;
        for pane in all_panes!(self) {
            let tab = pane.tab_mut();
            let Some(t) = &mut tab.term else { continue };
            t.poll(ctx);
            if tab.loc.local_path() != Some(t.cwd.as_path()) {
                tab.loc = Location::Local(t.cwd.clone());
            }
            if t.exited == Some(0) && close {
                done.push(tab.uid);
            }
        }
        for view in done {
            if let Some(side) = self.panes.iter().position(|p| p.tab().uid == view) {
                // In the shown tab: close its window (or the tab, if it is the only one),
                // leaving the focus where it is.
                self.close_pane(side);
                self.closed_tabs.pop();
            } else if self.take_window(view).is_none() {
                // The only window of a hidden tab: close that tab.
                if let Some(w) = self.strip.tabs.iter().find(|w| w.panes.iter().any(|p| p.tab().uid == view)) {
                    let uid = w.uid;
                    self.close_tabs(&[uid]);
                    self.closed_tabs.pop();
                }
            }
        }
    }

    fn restore_session(&mut self, s: crate::config::SavedPane) {
        let mut ids = HashMap::new();
        let mut strip = Strip { tabs: Vec::new(), active: 0, groups: Vec::new() };
        for g in &s.groups {
            let id = crate::tabs::next_group_id();
            ids.insert(g.id, id);
            strip.groups.push(crate::tabs::TabGroup { id, ..g.clone() });
        }
        let mut active_uid = None;
        for (i, st) in s.tabs.iter().enumerate() {
            let windows: Vec<crate::config::SavedWindow> =
                if st.windows.is_empty() { vec![crate::config::SavedWindow { path: st.path.clone(), terminal: st.terminal }] } else { st.windows.clone() };
            let layout = st.layout.clone().filter(|l| {
                let mut leaves = l.leaves();
                leaves.sort();
                leaves == (0..windows.len()).collect::<Vec<_>>() && Layout::fits(l)
            });
            let split = windows.len() > 1 && layout.is_some();
            let mut panes = Vec::new();
            for win in windows.iter().take(if split { windows.len() } else { 1 }) {
                let mut dir = PathBuf::from(&win.path);
                if !dir.is_dir() {
                    if !split {
                        break;
                    }
                    // Keep the layout whole: a missing folder becomes the home folder.
                    dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
                }
                let term = if win.terminal && self.cfg.term_restore { self.spawn_terminal(&dir, None).ok() } else { None };
                match term {
                    Some(t) => panes.push(Pane::from_tab(Tab::terminal(t))),
                    None if win.terminal && !split => break,
                    None => panes.push(self.files_pane(Location::Local(dir))),
                }
            }
            if panes.is_empty() {
                continue;
            }
            let layout = if split { layout.unwrap_or(Layout::Leaf(0)) } else { Layout::Leaf(0) };
            let n = panes.len();
            let mut w = Workspace::new(panes, layout);
            w.active = st.focus.min(n - 1);
            w.group = st.group.and_then(|g| ids.get(&g).copied());
            w.pinned = st.pinned;
            w.custom_title = st.title.clone();
            if i == s.active || active_uid.is_none() {
                active_uid = Some(w.uid);
            }
            strip.tabs.push(w);
        }
        if strip.tabs.is_empty() {
            return;
        }
        strip.normalize_keep(active_uid.unwrap_or(0));
        let a = strip.active;
        strip.activate(a);
        self.strip = strip;
        // Nothing is shown yet: drop the start window and show the restored tab.
        self.live = 0;
        self.panes.clear();
        self.sync_tab();
    }

    fn open_connect_dialog(&mut self, profile: Option<ConnectionProfile>) {
        self.show_dialog(Dialog::Connect { profile: profile.unwrap_or_default(), save: true, save_password: false }, true);
    }

    fn start_connect(&mut self, profile: ConnectionProfile, save: bool, save_password: bool) {
        if save {
            let mut stored = profile.clone();
            if !save_password {
                stored.password.clear();
            }
            if stored.name.trim().is_empty() {
                stored.name = format!("{}@{}", stored.user, stored.host);
            }
            if let Some(existing) = self.cfg.connections.iter_mut().find(|c| c.name == stored.name) {
                *existing = stored;
            } else {
                self.cfg.connections.push(stored);
            }
            self.save_cfg();
        }
        self.spawn_connect(profile, save, None);
    }

    /// Connect in the background. `trusted_key` is a host key fingerprint the user accepted.
    fn spawn_connect(&mut self, profile: ConnectionProfile, save: bool, trusted_key: Option<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let p = profile.clone();
        std::thread::spawn(move || {
            let r = remote::connect(&p, trusted_key.as_deref()).map(|mut fs| {
                let home = if p.initial_path.is_empty() { fs.home() } else { p.initial_path.clone() };
                (fs, home)
            });
            let _ = tx.send(r);
        });
        self.connecting = Some((profile, save, rx));
    }

    fn poll_connect(&mut self) {
        // Not while a dialog is open: it would lose the folder it acts on, or be replaced.
        if self.dialog.is_some() {
            return;
        }
        let Some((profile, _, rx)) = &self.connecting else { return };
        let Ok(r) = rx.try_recv() else { return };
        let profile = profile.clone();
        self.connecting = None;
        match r {
            Ok((fs, home)) => {
                let id = self.next_remote;
                self.next_remote += 1;
                let label = format!("{}://{}@{}:{}", profile.protocol.scheme(), profile.user, profile.host, profile.port);
                self.remotes.insert(id, (label.clone(), Arc::new(Mutex::new(fs))));
                self.toast(format!("{} {label}", t("Connected to")), false);
                let home = if home.is_empty() { "/".to_string() } else { home };
                self.navigate_active(Location::Remote { id, path: home });
            }
            Err(remote::ConnectError::UnknownHostKey { fingerprint }) => {
                self.show_dialog(Dialog::TrustHostKey { profile, fingerprint }, false);
            }
            Err(remote::ConnectError::Other(e)) => {
                self.toast(format!("{}: {e}", t("Connection failed")), true);
                self.show_dialog(Dialog::Connect { profile, save: false, save_password: false }, false);
            }
        }
    }

    fn disconnect(&mut self, id: u64) {
        self.remotes.remove(&id);
        let home = Location::Local(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
        let c = lc!(self);
        for p in all_panes!(self) {
            if matches!(p.tab().loc, Location::Remote { id: rid, .. } if rid == id) {
                p.tab_mut().navigate(home.clone(), &c);
            }
        }
    }

    // ------------------------------------------------------------ jobs

    /// Ejects a drive in the background. Refused while a transfer uses it; previews of
    /// its files are closed first, and windows showing it go home once it is gone.
    fn eject_drive(&mut self, d: crate::drives::Drive) {
        let mount = d.mount.clone();
        if !d.ejectable || self.ejecting.iter().any(|e| e.0 == mount) {
            return;
        }
        let name = d.name.clone();
        if self.jobs.iter().any(|j| j.paths.iter().any(|p| crate::drives::on_drive(p, &mount))) {
            self.toast(format!("{} {name}: {}", t("Could not eject"), t("A task is using this drive. Wait for it to finish, or cancel it.")), true);
            return;
        }
        let hashing = all_panes!(self).any(|p| p.tab().checksum.as_ref().is_some_and(|c| c.reads_under(&mount)));
        if hashing {
            self.toast(format!("{} {name}: {}", t("Could not eject"), t("A Checksum tab is reading files on this drive. Wait for it to finish, or close the tab.")), true);
            return;
        }
        // Let go of what this app reads there.
        let shown = mount.to_string_lossy().to_string();
        if self.quick.as_ref().is_some_and(|q| q.key.contains(&shown)) {
            self.quick = None;
        }
        if self.viewer.as_ref().is_some_and(|q| q.key.contains(&shown)) {
            self.viewer = None;
        }
        for p in all_panes!(self) {
            let tab = p.tab_mut();
            if local_file_of(&tab.loc).is_some_and(|p| crate::drives::on_drive(&p, &mount)) {
                tab.dir_sizes = None;
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::drives::eject(&d));
        });
        self.ejecting.push((mount, name, rx));
    }

    /// Windows showing the drive at `mount` go to the home folder; with `only_gone`, only
    /// those whose folder no longer exists (the drive went away by itself).
    fn leave_drive(&mut self, mount: &Path, only_gone: bool) {
        let home = Location::Local(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
        let lc = lc!(self);
        for p in all_panes!(self) {
            let tab = p.tab_mut();
            if !tab.is_files() {
                continue;
            }
            let Some(path) = local_file_of(&tab.loc) else { continue };
            if crate::drives::on_drive(&path, mount) && (!only_gone || !path.exists()) {
                tab.navigate(home.clone(), &lc);
            }
        }
    }

    /// New drive lists from the watcher, and ejects that finished.
    fn poll_drives(&mut self, ctx: &egui::Context) {
        if let Some(new) = self.drive_watch.poll() {
            let gone = crate::drives::removed(&self.drives, &new);
            self.drives = new;
            for mount in gone {
                self.leave_drive(&mount, true);
            }
        }
        if self.ejecting.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        let mut done = Vec::new();
        self.ejecting.retain(|(mount, name, rx)| match rx.try_recv() {
            Ok(r) => {
                done.push((mount.clone(), name.clone(), r));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                done.push((mount.clone(), name.clone(), Err(t("The drive could not be ejected."))));
                false
            }
        });
        for (mount, name, result) in done {
            match result {
                Ok(()) => {
                    self.drives.retain(|d| d.mount != mount);
                    self.leave_drive(&mount, false);
                    self.toast(format!("{}: {name}", t("Ejected")), false);
                }
                Err(e) => self.toast(format!("{} {name}: {e}", t("Could not eject")), true),
            }
            self.drive_watch.refresh();
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        // Speed sampling (exponential moving average over ~0.5 s windows).
        let now = Instant::now();
        for j in &mut self.jobs {
            let dt = now.duration_since(j.sample.0).as_secs_f64();
            if dt >= 0.5 {
                let done = j.job.progress.done();
                let inst = done.saturating_sub(j.sample.1) as f64 / dt;
                j.speed = if j.speed == 0.0 { inst } else { j.speed * 0.6 + inst * 0.4 };
                if j.job.progress.is_paused() || j.job.progress.is_queued() {
                    j.speed = 0.0;
                }
                j.sample = (now, done);
            }
        }
        let mut finished = vec![];
        for (i, j) in self.jobs.iter().enumerate() {
            if let Some(r) = j.job.poll() {
                finished.push((i, r));
            }
        }
        let mut refresh = false;
        for (i, r) in finished.into_iter().rev() {
            let rj = self.jobs.remove(i);
            let secs = rj.started_running.map(|s| s.elapsed().as_secs_f32()).unwrap_or(0.0);
            let cancelled = rj.job.progress.cancelled();
            let entry = |ok: bool, msg: String| FinishedJob {
                title: rj.job.title.clone(),
                ok,
                cancelled: !ok && cancelled,
                msg,
                bytes: rj.job.progress.done(),
                items: rj.job.progress.counts_items(),
                secs,
                dest: rj.dest.clone(),
                at: chrono::Local::now().format("%H:%M:%S").to_string(),
            };
            match r {
                Ok(msg) => match rj.after {
                    After::OpenFile(p) => {
                        self.temp_copies.downloaded(&p);
                        self.open_file(&p)
                    }
                    After::OpenWithDialog(p) => {
                        self.temp_copies.downloaded(&p);
                        self.open_with_dialog(p)
                    }
                    After::Refresh | After::Uploaded(..) => {
                        if let After::Uploaded(id, stamp) = rj.after {
                            if let Some(c) = self.temp_copies.get_mut(id) {
                                c.uploading = false;
                                c.saved = Some(stamp);
                            }
                        }
                        refresh = true;
                        self.toast(format!("✔ {}: {msg}", rj.job.title), false);
                        self.history.insert(0, entry(true, msg));
                    }
                },
                Err(e) => {
                    refresh = true;
                    // A cancel the user asked for is no error: it shows only in the history.
                    if !cancelled {
                        self.toast(format!("✖ {}: {e}", rj.job.title), true);
                    }
                    match rj.after {
                        After::OpenFile(p) | After::OpenWithDialog(p) => self.temp_copies.forget(&p),
                        // Not uploaded: asked again.
                        After::Uploaded(id, _) => {
                            if let Some(c) = self.temp_copies.get_mut(id) {
                                c.uploading = false;
                                c.asked = None;
                            }
                            self.history.insert(0, entry(false, e));
                        }
                        After::Refresh => self.history.insert(0, entry(false, e)),
                    }
                }
            }
        }
        self.history.truncate(100);
        if refresh {
            self.refresh_all();
        }
        self.schedule_jobs();
        if !self.jobs.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Quitting while jobs are running, or programs in terminals, asks first; run once
    /// per frame, before input is used.
    fn quit_step(&mut self, ctx: &egui::Context) {
        let close = ctx.input(|i| i.viewport().close_requested());
        let asked = crate::chrome::take_native_quit_request();
        let jobs = !self.jobs.is_empty();
        let terms = self.cfg.term_confirm_close && all_panes!(self).any(|p| p.tab().term.as_ref().is_some_and(|t| t.foreground_pid().is_some()));
        if close || asked {
            // Looked at again now: one saved a moment ago counts.
            self.temp_copies.poll(true);
        }
        // Copies from servers or archives edited in other apps: where they go is told first.
        let copies = self.temp_copies.has_unsaved();
        let busy = jobs || terms || copies;
        if close && busy && self.quit != Quit::Go {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        match self.quit {
            Quit::No if busy && (close || asked) => {
                self.quit = Quit::Asking;
                // It may have been asked from the Dock, with the window out of sight.
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Quit::No if asked => self.quit = Quit::Go,
            // The last job ended while asking: nothing is left to ask about.
            Quit::Asking if !busy => self.quit = Quit::Go,
            Quit::Stopping(since) if !jobs || since.elapsed() > QUIT_WAIT => self.quit = Quit::Go,
            _ => {}
        }
        if self.quit == Quit::Go && !close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if matches!(self.quit, Quit::Asking | Quit::Stopping(_)) {
            if self.quit == Quit::Asking && ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.quit = Quit::No;
            }
            // The dialog is for the mouse, and keys must not reach what is behind it
            // (Enter would confirm a dialog there, F8 would delete).
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Ime(_))));
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        crate::chrome::set_quit_guard(busy && self.quit != Quit::Go);
    }

    /// The confirmation for quitting with jobs, or programs in terminals, still running.
    fn quit_dialog(&mut self, ctx: &egui::Context) {
        let th = self.theme;
        let n = self.jobs.len();
        let programs = self.running_programs(self.panes.iter().chain(self.strip.tabs.iter().flat_map(|w| w.panes.iter())));
        let stopping = matches!(self.quit, Quit::Stopping(_));
        let mut keep = false;
        let mut stop = false;
        let edited: Vec<String> = self.temp_copies.unsaved().map(|c| c.name.clone()).collect();
        egui::Modal::new(egui::Id::new("quit-confirm")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(if n > 0 {
                t("Quit while tasks are running?")
            } else if !programs.is_empty() {
                t("Quit while programs are running?")
            } else {
                t("Quit with edited copies?")
            });
            ui.add_space(6.0);
            if !edited.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(t("Edited in other apps and not uploaded:"));
                    ui.label(RichText::new(edited.join(", ")).strong());
                });
                ui.label(RichText::new(format!("{}\n{}", t("They are not deleted: quitting moves them to"), crate::tempcopy::recovered_dir().display())).color(th.weak));
                ui.add_space(4.0);
            }
            if n > 0 {
                ui.label(format!("{n} {}", t("task(s) still running or waiting.")));
                ui.label(RichText::new(t("Quitting stops them. What has not finished will not be completed; files already finished are kept.")).color(th.weak));
            }
            if !programs.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(t("Still running in terminals:"));
                    ui.label(RichText::new(programs.join(", ")).strong());
                });
                if n == 0 {
                    ui.label(RichText::new(t("Quitting stops them.")).color(th.weak));
                }
            }
            ui.add_space(14.0);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 32.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if stopping {
                    ui.label(t("Stopping tasks…"));
                    ui.spinner();
                    return;
                }
                let (go, back) = if n > 0 { (t("Stop and quit"), t("Keep running")) } else { (t("Quit"), t("Cancel")) };
                let b = egui::Button::new(RichText::new(go).strong().color(egui::Color32::WHITE)).fill(th.danger).min_size(egui::vec2(96.0, 30.0));
                stop = ui.add(b).clicked();
                keep = ui.add(egui::Button::new(back).min_size(egui::vec2(96.0, 30.0))).clicked();
            });
        });
        if keep {
            self.quit = Quit::No;
        }
        if stop && n == 0 {
            self.quit = Quit::Go;
        } else if stop {
            for j in &self.jobs {
                j.job.progress.cancel();
            }
            self.quit = Quit::Stopping(Instant::now());
        }
    }

    /// The Transfers button's indicator while there are jobs: how far along the running ones
    /// are together (if known), and whether they are all paused.
    fn transfers_gauge(&self) -> Option<(Option<f32>, bool)> {
        if self.jobs.is_empty() {
            return None;
        }
        let running: Vec<&ops::Progress> = self.jobs.iter().map(|j| &*j.job.progress).filter(|p| !p.is_queued()).collect();
        Some((ops::overall_fraction(&running), !running.is_empty() && running.iter().all(|p| p.is_paused())))
    }

    /// The Transfers window's content (in its tab).
    fn transfers_view(&mut self, ui: &mut egui::Ui, side: usize) {
        let th = self.theme;
        let mut navigate: Option<Location> = None;
        let mut remove_queued: Option<usize> = None;
        let mut start_now: Option<usize> = None;
        let mut clear = false;
        let mut max_changed = false;
        // A click anywhere in it focuses the window.
        if ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.any_pressed()) {
            self.active = side;
        }
        let running = self.jobs.iter().filter(|j| !j.job.progress.is_queued()).count();
        let queued = self.jobs.len() - running;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{running} {} · {queued} {}", t("running"), t("queued"))).color(th.weak));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                max_changed |= ui.add(egui::DragValue::new(&mut self.cfg.max_parallel).range(1..=8)).changed();
                ui.label(RichText::new(t("Run at the same time:")).size(12.0).color(th.weak));
            });
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.jobs.is_empty() && self.history.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(t("No tasks yet")).color(th.text));
                    ui.label(
                        RichText::new(t("Copy (F5), move (F6), compress or extract: progress shows up here while you keep browsing."))
                            .size(11.5)
                            .color(th.weak),
                    );
                });
            }
            for (i, j) in self.jobs.iter().enumerate() {
                let p = j.job.progress.clone();
                let is_q = p.is_queued();
                egui::Frame::NONE.fill(th.bg).corner_radius(10).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&j.job.title).font(egui::FontId::new(13.5, egui::FontFamily::Name("bold".into()))).color(th.strong()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::icon_button(ui, 24.0, true, th, |pp, r, c| close_x(pp, r, c)).on_hover_text(t("Cancel")).clicked() {
                                if is_q {
                                    remove_queued = Some(i);
                                } else {
                                    p.cancel();
                                }
                            }
                            if is_q {
                                if ui.small_button(t("Start now")).clicked() {
                                    start_now = Some(i);
                                }
                            } else if p.is_paused() {
                                if icons::icon_button(ui, 24.0, true, th, play_icon).on_hover_text(t("Resume")).clicked() {
                                    p.resume();
                                }
                            } else if icons::icon_button(ui, 24.0, true, th, pause_icon).on_hover_text(t("Pause")).clicked() {
                                p.pause();
                            }
                        });
                    });
                    // Progress bar
                    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 3.0, th.surface);
                    let bar_color = if p.is_paused() || is_q { th.weak } else { th.accent };
                    match p.fraction() {
                        Some(f) => {
                            ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(r.width() * f, r.height())), 3.0, bar_color);
                        }
                        None if !is_q => {
                            let t0 = ui.input(|i| i.time) as f32;
                            let x = (t0 * 0.6).fract() * (r.width() + 80.0) - 80.0;
                            let seg = egui::Rect::from_min_max(
                                egui::pos2(r.left() + x.max(0.0), r.top()),
                                egui::pos2((r.left() + x + 80.0).min(r.right()), r.bottom()),
                            );
                            ui.painter().rect_filled(seg, 3.0, bar_color);
                        }
                        None => {}
                    }
                    let line = if is_q {
                        t("Waiting in queue…")
                    } else if p.is_paused() {
                        match p.fraction() {
                            Some(f) => format!("{} · {:.0}% · {}", t("Paused"), f * 100.0, p.amount_text()),
                            None => format!("{} · {}", t("Paused"), p.amount_text()),
                        }
                    } else {
                        let mut parts = vec![];
                        if let Some(f) = p.fraction() {
                            parts.push(format!("{:.0}%", f * 100.0));
                        }
                        parts.push(p.amount_text());
                        // Items are no bytes: no speed or time left for them.
                        if j.speed > 1.0 && !p.counts_items() {
                            parts.push(format!("{}/s", human_size(j.speed as u64)));
                            if p.total() > p.done() {
                                let eta = (p.total() - p.done()) as f64 / j.speed;
                                parts.push(format!("{} {}", fmt_duration(eta as f32), t("left")));
                            }
                        }
                        parts.join(" · ")
                    };
                    ui.label(RichText::new(line).size(12.0).color(th.text));
                    let cur = p.current();
                    if !cur.is_empty() && !is_q {
                        ui.add(egui::Label::new(RichText::new(cur).size(11.0).color(th.weak)).truncate());
                    }
                });
                ui.add_space(6.0);
            }
            if !self.history.is_empty() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t("Finished")).size(11.5).color(th.weak));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(t("Clear")).clicked() {
                            clear = true;
                        }
                    });
                });
                for h in &self.history {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), egui::Sense::hover());
                        let color = if h.ok { th.accent } else if h.cancelled { th.weak } else { th.danger };
                        ui.painter().circle_filled(r.center(), 4.0, color);
                        ui.label(RichText::new(&h.title).color(th.text));
                        let amount = if h.items { format!("{} {}", h.bytes, t("items")) } else { human_size(h.bytes) };
                        let detail = if h.ok {
                            format!("{amount} · {} · {}", fmt_duration(h.secs), h.at)
                        } else if h.cancelled {
                            format!("{} · {amount} · {}", t("Cancelled"), h.at)
                        } else {
                            format!("{} · {}", h.msg, h.at)
                        };
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(d) = &h.dest {
                                if ui.small_button(t("Show")).on_hover_text(t("Open the destination")).clicked() {
                                    navigate = Some(d.clone());
                                }
                            }
                            ui.add(egui::Label::new(RichText::new(detail).size(11.5).color(if h.ok || h.cancelled { th.weak } else { th.danger })).truncate());
                        });
                    });
                }
            }
        });
        if let Some(i) = start_now {
            if let Some(j) = self.jobs.get_mut(i) {
                j.job.progress.release();
                j.started_running = Some(Instant::now());
            }
        }
        if let Some(i) = remove_queued {
            if let Some(j) = self.jobs.get(i) {
                j.job.progress.cancel();
            }
        }
        if clear {
            self.history.clear();
        }
        if max_changed {
            self.save_cfg();
            self.schedule_jobs();
        }
        if let Some(loc) = navigate {
            self.navigate_active(loc);
        }
    }

    // ------------------------------------------------------------ checksums

    /// Hash the selection (or the cursor item); a single checksum file is verified instead.
    fn open_checksum(&mut self) {
        let a = self.active;
        let tab = self.panes[a].tab();
        let Location::Local(dir) = &tab.loc else {
            self.toast(t("Checksums work on local files"), true);
            return;
        };
        let dir = dir.clone();
        let sel: Vec<Entry> = tab.selection().into_iter().filter(|e| !e.is_parent).collect();
        if sel.is_empty() {
            return;
        }
        let mut tool = crate::checksum::ChecksumUi::default();
        match sel.as_slice() {
            [one] if !one.is_dir && crate::checksum::is_checksum_file(&one.name) => {
                if let Err(e) = tool.start_verify(&self.egui_ctx, dir.join(&one.name)) {
                    self.toast(e, true);
                    return;
                }
            }
            _ => {
                let paths = sel.iter().map(|e| dir.join(&e.name)).collect();
                tool.start_files(&self.egui_ctx, dir.clone(), paths);
            }
        }
        self.open_tool(Tab::checksum_tool(tool, dir));
    }

    /// Show a tool (Checksum, Batch rename) in a new tab next to the shown one.
    fn open_tool(&mut self, tab: Tab) {
        let (at, group) = self.next_slot();
        self.insert_workspace(at, Workspace::single(Pane::from_tab(tab)), group);
    }

    /// A Checksum or Batch rename window: the app draws the tool into the space left for it.
    fn tool_view(&mut self, ui: &mut egui::Ui, side: usize) {
        let th = self.theme;
        // A click anywhere in it focuses the window.
        if ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.any_pressed()) {
            self.active = side;
        }
        let (mut saved, mut action) = (None, None);
        egui::Frame::NONE.inner_margin(egui::Margin { left: 14, right: 14, top: 6, bottom: 10 }).show(ui, |ui| {
            let tab = self.panes[side].tab_mut();
            if let Some(c) = tab.checksum.as_mut() {
                saved = c.ui(ui, th);
            } else if let Some(r) = tab.renamer.as_mut() {
                action = r.ui(ui, th);
            }
        });
        if let Some(dir) = saved {
            self.refresh_location(&Location::Local(dir));
        }
        match action {
            Some(RenameAction::Apply(loc, pairs)) => {
                // What went through is kept (and can be undone) even when some items failed.
                let (done, errors) = self.run_rename(&loc, &pairs);
                if errors.is_empty() {
                    self.toast(format!("{} {} {}", t("Renamed"), done.len(), t("item(s)")), false);
                } else {
                    self.toast(errors.join("\n"), true);
                }
                if !done.is_empty() {
                    if let Some(r) = self.panes[side].tab_mut().renamer.as_mut() {
                        r.applied(loc.clone(), done);
                    }
                }
                self.refresh_location(&loc);
            }
            Some(RenameAction::Undo(loc, back)) => {
                let (done, errors) = self.run_rename(&loc, &back);
                if errors.is_empty() {
                    self.toast(t("Rename undone"), false);
                } else {
                    self.toast(errors.join("\n"), true);
                }
                if !done.is_empty() {
                    if let Some(r) = self.panes[side].tab_mut().renamer.as_mut() {
                        r.undone(&done);
                    }
                    self.refresh_location(&loc);
                }
            }
            None => {}
        }
    }

    // ------------------------------------------------------------ batch rename

    fn open_batch_rename(&mut self) {
        let a = self.active;
        let tab = self.panes[a].tab();
        if matches!(tab.loc, Location::Archive { .. }) {
            self.toast(t("Archives are read-only"), true);
            return;
        }
        // Marked items, or everything currently visible (respects the quick filter).
        let entries: Vec<Entry> = if tab.marked_shown().is_empty() {
            tab.view.iter().map(|&i| tab.entries[i].clone()).filter(|e| !e.is_parent).collect()
        } else {
            tab.selection()
        };
        if entries.is_empty() {
            return;
        }
        let items = entries.iter().map(|e| ren::Item { name: e.name.clone(), is_dir: e.is_dir, modified: e.modified }).collect();
        // Every name taken in the folders involved, not just the ones listed: search
        // results and filters show only some, and the others must not be overwritten.
        let existing: std::collections::HashSet<String> = match &tab.loc {
            Location::Local(dir) => {
                let dirs: std::collections::HashSet<&str> = entries.iter().map(|e| ren::split_dir(&e.name).0).collect();
                let mut all = std::collections::HashSet::new();
                for sub in dirs {
                    if let Ok(rd) = std::fs::read_dir(dir.join(sub)) {
                        all.extend(rd.flatten().map(|d| format!("{sub}{}", d.file_name().to_string_lossy())));
                    }
                }
                all
            }
            _ => tab.entries.iter().filter(|e| !e.is_parent).map(|e| e.name.clone()).collect(),
        };
        let parent = tab.loc.short_name_raw();
        let loc = tab.loc.clone();
        let mut tool = BatchRename::default();
        tool.start(loc.clone(), parent, items, existing);
        self.open_tool(Tab::rename_tool(tool, loc));
    }

    /// Batch rename in `loc`: the pairs done and the errors.
    fn run_rename(&mut self, loc: &Location, pairs: &[(String, String)]) -> (Vec<(String, String)>, Vec<String>) {
        match loc {
            Location::Local(dir) => ren::apply(pairs, |f, to| crate::safefs::rename_item(&dir.join(f), &dir.join(to)).map_err(es), |n| crate::safefs::exists(&dir.join(n))),
            Location::Remote { id, path } => {
                let Some(fs) = self.remotes.get(id).map(|r| r.1.clone()) else { return (vec![], vec![t("Connection closed")]) };
                // Never waits for a transfer on this connection (the window would freeze).
                let mut g = match remote::try_use(&fs) {
                    Ok(g) => g,
                    Err(e) => return (vec![], vec![e]),
                };
                // What is in the folder now, less the items that move away first.
                let moving: std::collections::HashSet<&str> = pairs.iter().map(|(a, _)| a.as_str()).collect();
                let taken: std::collections::HashSet<String> = match g.list(path) {
                    Ok(list) => list.into_iter().map(|e| e.name).filter(|n| !moving.contains(n.as_str())).collect(),
                    Err(e) => return (vec![], vec![e]),
                };
                ren::apply(pairs, |f, to| g.rename(&join_slash(path, f), &join_slash(path, to)), |n| taken.contains(n))
            }
            Location::Archive { .. } => (vec![], vec![t("Archives are read-only")]),
        }
    }

    fn start_transfer(&mut self, src_loc: &Location, items: Vec<Entry>, dst_loc: &Location, mv: bool, policy: Conflict) {
        let (Some(src), Some(dst_ep)) = (self.endpoint(src_loc), self.endpoint(dst_loc)) else { return };
        let list: Vec<ops::Item> = items.iter().map(|e| ops::Item { name: e.name.clone(), is_dir: e.is_dir, size: e.size }).collect();
        if list.is_empty() {
            return;
        }
        for side in 0..self.panes.len() {
            if self.panes[side].tab().loc == *src_loc {
                self.panes[side].tab_mut().marked.clear();
            }
        }
        let what = if list.len() == 1 { list[0].name.clone() } else { format!("{} {}", list.len(), t("item(s)")) };
        let title = format!("{} {what} → {}", if mv { t("Moving") } else { t("Copying") }, dst_loc.short_name());
        let sources = local_file_of(src_loc).into_iter().collect();
        self.spawn_job(title, After::Refresh, Some(dst_loc.clone()), sources, move |p| ops::transfer(src, list, dst_ep, mv, policy, p));
    }

    /// Items dropped onto a folder (panel, folder row, breadcrumb or sidebar).
    fn handle_drop(&mut self, payload: &crate::pane::DragPayload, target: Location) {
        if !crate::pane::drop_allowed(payload, &target) {
            return;
        }
        let mv = self.shift_down;
        if matches!(payload.loc, Location::Archive { .. }) && mv {
            self.toast(t("Archives are read-only"), true);
            return;
        }
        if self.cfg.confirm_dnd {
            let dst = target.display(|id| self.remote_label(id));
            self.dialog_dst = Some(target);
            self.show_dialog(Dialog::Transfer { src: payload.loc.clone(), mv, items: payload.items.clone(), dst, policy: Conflict::Rename }, false);
        } else {
            self.start_transfer(&payload.loc, payload.items.clone(), &target, mv, Conflict::Rename);
        }
    }

    /// Make a sidebar row a drop target for the current drag.
    fn sidebar_drop_target(&mut self, ui: &egui::Ui, rect: egui::Rect, target: Location) {
        let Some(pl) = crate::pane::drag_payload(ui.ctx()) else { return };
        if !ui.rect_contains_pointer(rect) || !crate::pane::drop_allowed(&pl, &target) {
            return;
        }
        let th = self.theme;
        ui.painter().rect_filled(rect, 7.0, th.accent.gamma_multiply(0.16));
        ui.painter().rect_stroke(rect, 7.0, egui::Stroke::new(1.5_f32, th.accent), egui::StrokeKind::Inside);
        self.drag_target = Some(target.clone());
        if ui.input(|i| i.pointer.any_released()) {
            if let Some(pl) = egui::DragAndDrop::take_payload::<crate::pane::DragPayload>(ui.ctx()) {
                self.handle_drop(&pl, target);
            }
        }
    }

    /// Floating label that follows the pointer while dragging files.
    fn drag_ghost(&mut self, ctx: &egui::Context) {
        let Some(pl) = crate::pane::drag_payload(ctx) else { return };
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let th = self.theme;
        let what = if pl.items.len() == 1 { pl.items[0].name.clone() } else { format!("{} {}", pl.items.len(), t("item(s)")) };
        let (line, color) = match &self.drag_target {
            Some(tgt) => (
                format!("{} → {}", if self.shift_down { t("Move") } else { t("Copy") }, tgt.short_name()),
                th.accent,
            ),
            None => (t("Drop on a folder or the other panel"), th.weak),
        };
        egui::Area::new(egui::Id::new("drag-ghost"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + egui::vec2(16.0, 12.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(th.panel).corner_radius(9).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        if let Some(first) = pl.items.first() {
                            icons::entry_icon(ui.painter(), r, first, th);
                        }
                        if pl.items.len() > 1 {
                            let c = r.right_top() + egui::vec2(-1.0, 2.0);
                            ui.painter().circle_filled(c, 7.0, th.accent);
                            ui.painter().text(c, egui::Align2::CENTER_CENTER, pl.items.len().min(99).to_string(), egui::FontId::proportional(9.5), th.on_accent());
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(RichText::new(what).color(th.strong()));
                            ui.label(RichText::new(line).size(11.5).color(color));
                            if !self.shift_down && self.drag_target.is_some() {
                                ui.label(RichText::new(t("Hold Shift to move")).size(10.5).color(th.weak));
                            }
                        });
                    });
                });
            });
        ctx.set_cursor_icon(if self.drag_target.is_some() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::NoDrop });
    }

    fn refresh_location(&mut self, loc: &Location) {
        let c = lc!(self);
        for p in all_panes!(self) {
            if p.tab().loc == *loc && p.tab().is_files() {
                p.tab_mut().marked.clear();
                p.tab_mut().reload(&c);
            }
        }
    }

    // ------------------------------------------------------------ UI pieces

    fn main_menu(&mut self, ui: &mut egui::Ui) {
        {
            ui.menu_button(t("File"), |ui| {
                if ui.button(format!("{}\t{}", t("New tab"), hint(Action::NewTab))).clicked() {
                    self.new_tab();
                    ui.close_menu();
                }
                if ui.button(format!("{}\t{}", t("New terminal tab"), hint(Action::NewTerminalTab))).clicked() {
                    self.new_terminal_here(None, None);
                    ui.close_menu();
                }
                if ui.button(format!("{}\t{}", t("Close tab"), hint(Action::CloseTab))).clicked() {
                    self.request_close_tabs(&[self.live]);
                    ui.close_menu();
                }
                if ui.add_enabled(!self.closed_tabs.is_empty(), egui::Button::new(format!("{}\t{}", t("Reopen closed tab"), hint(Action::ReopenTab)))).clicked() {
                    self.reopen_closed_tab();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(format!("{}\tF7", t("New folder…"))).clicked() {
                    self.run_cmd(Cmd::NewFolder);
                    ui.close_menu();
                }
                if ui.button(format!("{}\t{}", t("New file…"), hint(Action::NewFile))).clicked() {
                    self.run_cmd(Cmd::NewFile);
                    ui.close_menu();
                }
                if ui.button(format!("{}\t{}", t("Open terminal here"), hint(Action::OpenTerminal))).clicked() {
                    self.open_terminal();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Export settings…")).clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new().set_file_name("matrix_finder-settings.json").save_file() {
                        match self.cfg.export_to(&p, false) {
                            Ok(()) => self.toast(format!("{} {}", t("Exported to"), p.display()), false),
                            Err(e) => self.toast(e, true),
                        }
                    }
                }
                if ui.button(t("Import settings…")).clicked() {
                    ui.close_menu();
                    self.import_settings();
                }
                ui.separator();
                if ui.button(t("Quit")).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(t("Mark"), |ui| {
                for (label, key, f) in [
                    (t("Select all"), hint(Action::SelectAll), 0),
                    (t("Deselect all"), "Esc", 1),
                    (t("Invert selection"), hint(Action::InvertSelection), 2),
                    (t("Calculate folder sizes"), "", 3),
                ] {
                    if ui.button(format!("{label}\t{key}")).clicked() {
                        let a = self.active;
                        match f {
                            0 => self.panes[a].tab_mut().mark_all(),
                            1 => self.panes[a].tab_mut().marked.clear(),
                            2 => self.panes[a].tab_mut().invert_marks(),
                            _ => self.calc_sizes(),
                        }
                        ui.close_menu();
                    }
                }
            });
            ui.menu_button(t("Commands"), |ui| {
                for (cmd, label, key) in [
                    (Cmd::View, t("View"), hint(Action::View)),
                    (Cmd::OpenDefault, t("Open with default app"), hint(Action::OpenDefault)),
                    (Cmd::OpenWith, t("Open with…"), ""),
                    (Cmd::Copy, t("Copy to other panel"), hint(Action::Copy)),
                    (Cmd::Move, t("Move to other panel"), hint(Action::Move)),
                    (Cmd::Rename, t("Rename"), hint(Action::Rename)),
                    (Cmd::BatchRename, t("Batch rename…"), hint(Action::BatchRename)),
            (Cmd::Checksum, t("Checksum: create / verify…"), hint(Action::Checksum)),
                    (Cmd::Checksum, t("Checksum…"), hint(Action::Checksum)),
                    (Cmd::Delete, t("Delete"), hint(Action::Delete)),
                    (Cmd::Pack, t("Compress…"), hint(Action::Compress)),
                    (Cmd::ExtractOther, t("Extract to other panel"), hint(Action::ExtractOther)),
                    (Cmd::Properties, t("Properties"), hint(Action::Properties)),
                ] {
                    if ui.button(format!("{label}\t{key}")).clicked() {
                        self.run_cmd(cmd);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button(format!("{}\t{}", t("Swap panels"), hint(Action::SwapPanels))).clicked() {
                    self.swap_with_other();
                    ui.close_menu();
                }
                if ui.button(t("Same folder in other panel")).clicked() {
                    let loc = self.panes[self.active].tab().loc.clone();
                    let o = self.other_or_split();
                    self.panes[o].tab_mut().navigate(loc, &lc!(self));
                    ui.close_menu();
                }
            });
            ui.menu_button(t("View"), |ui| {
                if ui.checkbox(&mut self.cfg.show_sidebar.clone(), format!("{}\t{}", t("Sidebar"), hint(Action::Sidebar))).clicked() {
                    self.cfg.show_sidebar = !self.cfg.show_sidebar;
                    self.save_cfg();
                    ui.close_menu();
                }
                if ui.checkbox(&mut self.cfg.quick_view.clone(), format!("{}\t{}", t("Quick view panel"), hint(Action::QuickView))).clicked() {
                    self.toggle_quick_view();
                    ui.close_menu();
                }
                if ui.checkbox(&mut self.cfg.show_hidden, format!("{}\t{}", t("Show hidden files"), hint(Action::ToggleHidden))).changed() {
                    self.rebuild_views();
                    self.save_cfg();
                }
                if ui.checkbox(&mut self.cfg.dirs_first, t("Folders first")).changed() {
                    self.rebuild_views();
                    self.save_cfg();
                }
                ui.separator();
                ui.menu_button(format!("🎨 {}", t("Theme")), |ui| {
                    for th in theme::THEMES {
                        if ui.selectable_label(self.cfg.theme == th.name, th.name).clicked() {
                            self.set_theme(ui.ctx(), th.name);
                        }
                    }
                });
                ui.menu_button(format!("🌐 {}", t("Language")), |ui| {
                    for (code, name) in self.languages.clone() {
                        if ui.selectable_label(self.cfg.language == code, &name).clicked() {
                            self.set_language(&code);
                        }
                    }
                });
                ui.separator();
                if ui.button(format!("{}\t{}", t("Refresh"), hint(Action::Refresh))).clicked() {
                    self.run_cmd(Cmd::Refresh);
                    ui.close_menu();
                }
            });
            ui.menu_button(t("Bookmarks"), |ui| {
                if ui.button(format!("{}\t{}", t("Bookmark current folder"), hint(Action::Bookmark))).clicked() {
                    self.run_cmd(Cmd::Bookmark);
                    ui.close_menu();
                }
                if ui.button(t("Manage bookmarks…")).clicked() {
                    self.open_settings(SettingsPage::Bookmarks);
                    ui.close_menu();
                }
                ui.separator();
                for b in self.cfg.bookmarks.clone() {
                    if b.folder {
                        ui.menu_button(b.name.clone(), |ui| {
                            for c in &b.children {
                                if ui.button(&c.name).on_hover_text(&c.path).clicked() {
                                    self.open_bookmark(c);
                                    ui.close_menu();
                                }
                            }
                        });
                    } else if ui.button(format!("★ {}", b.name)).on_hover_text(&b.path).clicked() {
                        self.open_bookmark(&b);
                        ui.close_menu();
                    }
                }
            });
            ui.menu_button(t("Network"), |ui| {
                if ui.button(format!("{}\t{}", t("Connect to server…"), hint(Action::Connect))).clicked() {
                    self.open_connect_dialog(None);
                    ui.close_menu();
                }
                if !self.cfg.connections.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(t("Saved connections")).small().color(self.theme.weak));
                    let mut remove = None;
                    for (i, c) in self.cfg.connections.clone().into_iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.button(format!("{} {}", c.protocol.label(), c.name)).on_hover_text(c.url()).clicked() {
                                if c.password.is_empty() && c.key_file.is_empty() {
                                    // Ask for the password (or use the SSH agent from the dialog).
                                    self.open_connect_dialog(Some(c.clone()));
                                } else {
                                    self.start_connect(c.clone(), false, false);
                                }
                                ui.close_menu();
                            }
                            if ui.small_button("✏").on_hover_text(t("Edit")).clicked() {
                                self.show_dialog(Dialog::Connect { save_password: !c.password.is_empty(), profile: c, save: true }, false);
                                ui.close_menu();
                            }
                            if ui.small_button("🗑").on_hover_text(t("Delete")).clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        self.cfg.connections.remove(i);
                        self.save_cfg();
                    }
                }
                if !self.remotes.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(t("Open connections")).small().color(self.theme.weak));
                    let list: Vec<(u64, String)> = self.remotes.iter().map(|(k, v)| (*k, v.0.clone())).collect();
                    for (id, label) in list {
                        ui.horizontal(|ui| {
                            if ui.button(&label).clicked() {
                                self.navigate_active(Location::Remote { id, path: "/".into() });
                                ui.close_menu();
                            }
                            if ui.small_button(t("Disconnect")).clicked() {
                                self.disconnect(id);
                                ui.close_menu();
                            }
                        });
                    }
                }
            });
            ui.menu_button(t("Tools"), |ui| {
                if ui.button(format!("📊 {}", t("Disk usage"))).clicked() {
                    self.run_cmd(Cmd::DiskUsage);
                    ui.close_menu();
                }
                if ui.button(format!("⇅ {}\t{}", t("Tasks"), hint(Action::Transfers))).clicked() {
                    self.open_transfers();
                    ui.close_menu();
                }
                if ui.button(format!("🛡 {}\t{}", t("Backup…"), hint(Action::Backup))).clicked() {
                    self.backup.open = true;
                    ui.close_menu();
                }
                if ui.button(format!("⚙ {}\t{}", t("Settings…"), hint(Action::Settings))).clicked() {
                    self.open_settings(self.settings_page);
                    ui.close_menu();
                }
            });
            ui.menu_button(t("Help"), |ui| {
                if ui.button(format!("{}\tF1", t("Keyboard shortcuts"))).clicked() {
                    self.show_help = true;
                    ui.close_menu();
                }
                if ui.button(t("Check for updates…")).clicked() {
                    self.check_for_updates(true);
                    ui.close_menu();
                }
                if ui.button(t("About")).clicked() {
                    self.show_about = true;
                    ui.close_menu();
                }
            });

        }
    }

    fn resolve_theme(&self, ctx: &egui::Context) -> &'static Theme {
        if self.cfg.theme == theme::SYSTEM {
            theme::native(ctx.system_theme() == Some(egui::Theme::Dark))
        } else {
            theme::by_name(&self.cfg.theme)
        }
    }

    /// Build the icon set for an icon setting value.
    fn make_icon_set(&self, choice: &str) -> icons::IconSet {
        use icons::{IconPack as P, IconSet};
        let open = |name: &str| crate::sysicons::IconTheme::open(name).map(Arc::new);
        let breeze = || if self.theme.dark { open("breeze-dark").or_else(|| open("breeze")) } else { open("breeze") };
        match choice {
            "matrix" => IconSet { pack: P::Matrix, system: None },
            "adwaita" => IconSet { pack: P::Adwaita, system: open("Adwaita") },
            "breeze" => IconSet { pack: P::Breeze, system: breeze() },
            "macos" => IconSet { pack: P::MacOS, system: None },
            "windows" => IconSet { pack: P::Windows, system: None },
            "system" => {
                if cfg!(target_os = "macos") {
                    IconSet { pack: P::MacOS, system: None }
                } else if cfg!(windows) {
                    IconSet { pack: P::Windows, system: None }
                } else {
                    let name = crate::sysicons::current_theme_name().unwrap_or_else(|| "Adwaita".into());
                    let pack = if name.to_lowercase().contains("breeze") { P::Breeze } else { P::Adwaita };
                    IconSet { pack, system: open(&name).or_else(|| open("Adwaita")) }
                }
            }
            other if other.starts_with("icons:") => IconSet { pack: P::Adwaita, system: open(&other[6..]) },
            _ => {
                let pack = self.theme.icons;
                let system = match pack {
                    P::Adwaita => open("Adwaita"),
                    P::Breeze => breeze(),
                    _ => None,
                };
                IconSet { pack, system }
            }
        }
    }

    fn set_theme(&mut self, ctx: &egui::Context, name: &str) {
        self.cfg.theme = name.to_string();
        self.theme = self.resolve_theme(ctx);
        theme::apply(ctx, self.theme);
        self.quick = None;
        if self.viewer.is_some() {
            self.viewer = self.preview_for_cursor(self.active);
        }
        self.save_cfg();
    }

    fn set_language(&mut self, code: &str) {
        self.cfg.language = code.to_string();
        i18n::set_language(code);
        self.save_cfg();
    }

    fn apply_cfg(&mut self, ctx: &egui::Context) {
        i18n::set_language(&self.cfg.language);
        self.theme = theme::by_name(&self.cfg.theme);
        theme::apply(ctx, self.theme);
        ctx.set_zoom_factor(self.cfg.ui_scale.clamp(0.6, 2.5));
        self.apply_fonts(ctx);
        shortcuts::install(&self.cfg.shortcuts);
        self.rebuild_views();
    }

    /// Logo + app name + chevron; opens the application menu.
    fn brand_menu(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        let bg = ui.painter().add(egui::Shape::Noop);
        let text = RichText::new("Matrix Finder      ").font(egui::FontId::new(14.5, egui::FontFamily::Name("bold".into()))).color(th.strong());
        // Text only: the logo lives in the About window, not in the toolbar.
        let button = egui::Button::new(text)
        .fill(egui::Color32::TRANSPARENT)
        .min_size(egui::vec2(0.0, 30.0));
        let menu = egui::menu::menu_custom_button(ui, button, |ui| {
            ui.set_min_width(230.0);
            if ui.button(t("About")).clicked() {
                self.show_about = true;
                ui.close_menu();
            }
            if ui.button(t("Check for updates…")).clicked() {
                self.check_for_updates(true);
                ui.close_menu();
            }
            ui.separator();
            self.main_menu(ui);
        });
        let r = menu.response.rect;
        let open = menu.inner.is_some();
        if menu.response.hovered() || open {
            ui.painter().set(bg, egui::Shape::rect_filled(r, th.radius as f32 + 2.0, th.surface));
        }
        // Chevron drawn in the reserved trailing space.
        let c = egui::pos2(r.right() - 14.0, r.center().y + 1.0);
        let col = if menu.response.hovered() || open { th.strong() } else { th.weak };
        let st = egui::Stroke::new(1.5_f32, col);
        ui.painter().line_segment([c + egui::vec2(-4.0, -2.0), c + egui::vec2(0.0, 2.0)], st);
        ui.painter().line_segment([c + egui::vec2(0.0, 2.0), c + egui::vec2(4.0, -2.0)], st);
        menu.response.on_hover_text(t("Menu"));
    }

    /// The sidebar button, just before the app menu (in the sidebar, or in the top
    /// bar while the sidebar is hidden).
    fn sidebar_toggle(&mut self, ui: &mut egui::Ui) {
        let label = if self.cfg.show_sidebar { t("Hide sidebar") } else { t("Show sidebar") };
        let resp = tool_icon(ui, self.theme, &format!("{label} ({})", hint(Action::Sidebar)), false, sidebar_icon);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
        if resp.clicked() {
            self.cfg.show_sidebar = !self.cfg.show_sidebar;
            self.save_cfg();
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        // Empty parts of the bar move the window.
        let bar = ui.max_rect().expand2(egui::vec2(10.0, 6.0));
        self.chrome.drag_area(ui, bar, "topbar");
        ui.horizontal(|ui| {
            ui.set_height(self.chrome.row_height());
            ui.spacing_mut().item_spacing.x = 4.0;
            // With the sidebar visible the app menu lives at its top (Finder/Nautilus style).
            if !self.cfg.show_sidebar {
                ui.add_space(self.chrome.left_inset());
                self.chrome.inline_buttons(ui, th, true);
                self.sidebar_toggle(ui);
                self.brand_menu(ui);
            }

            // Command search, centred on the whole window (not just the bar beside the
            // sidebar); in a narrow window it slides and shrinks to stay clear of the
            // buttons on both sides.
            let full = ui.max_rect();
            let window = ui.ctx().screen_rect();
            let start = ui.cursor().min.x + 8.0;
            let limit = full.right() - self.toolbar_right_w - 12.0;
            let want = (window.width() * 0.36).clamp(240.0, 520.0);
            let c = window.center().x;
            // Widest box that is still exactly centred.
            let centred = 2.0 * (c - start).min(limit - c);
            let (w, x) = if centred >= 260.0 {
                let w = want.min(centred);
                (w, c - w / 2.0)
            } else {
                let w = want.min((limit - start).max(120.0));
                (w, (c - w / 2.0).clamp(start, (limit - w).max(start)))
            };
            let gap = x - ui.cursor().min.x;
            if gap > 8.0 {
                ui.add_space(gap);
            }
            // Narrow windows: drop the shortcut chip, then the text (icon only).
            let w = if w < 150.0 { 32.0 } else { w };
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 28.0), egui::Sense::click());
            let fill = if resp.hovered() { th.surface.lerp_to_gamma(th.text, 0.05) } else { th.surface };
            ui.painter().rect_filled(rect, 8.0, fill);
            let icon_x = if w <= 32.0 { rect.center().x } else { rect.left() + 16.0 };
            icons::search(ui.painter(), egui::Rect::from_center_size(egui::pos2(icon_x, rect.center().y), egui::vec2(14.0, 14.0)), th.weak);
            if w > 32.0 {
                let shortcut = hint(Action::Palette);
                let text_right = if w >= 230.0 {
                    palette::keycap(ui.painter(), th, egui::pos2(rect.right() - 6.0, rect.center().y), shortcut).left() - 6.0
                } else {
                    rect.right() - 8.0
                };
                let mut job = egui::text::LayoutJob::simple_singleline(t("Search files, commands, settings…"), egui::FontId::proportional(13.5), th.weak);
                job.wrap = egui::text::TextWrapping::truncate_at_width((text_right - rect.left() - 30.0).max(10.0));
                let g = ui.fonts(|f| f.layout_job(job));
                ui.painter().galley(egui::pos2(rect.left() + 30.0, rect.center().y - g.size().y / 2.0), g, th.weak);
            }
            let resp = resp.on_hover_text(format!("{} ({})", t("Search"), hint(Action::Palette)));
            if resp.clicked() {
                self.palette.toggle();
            }

            let right_group = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(self.chrome.right_inset());
                ui.scope(|ui| self.chrome.inline_buttons(ui, th, false));
                ui.spacing_mut().item_spacing.x = 2.0;
                if tool_icon(ui, th, &t("Settings"), self.settings_shown(), icons::gear).clicked() {
                    self.open_settings(self.settings_page);
                }
                let n_jobs = self.jobs.len();
                let tr_shown = self.panes[self.active].tab().transfers;
                let gauge = self.transfers_gauge();
                let time = ui.input(|i| i.time) as f32;
                let tr = tool_icon(ui, th, &format!("{} ({})", t("Tasks"), hint(Action::Transfers)), tr_shown, |p, r, c| match gauge {
                    Some((fraction, paused)) => icons::progress_pie(p, r, if paused { c } else { th.accent }, fraction, time),
                    None => icons::transfers(p, r, c),
                });
                // The indicator stands for one job; the number tells when there are more.
                if n_jobs > 1 {
                    // In the corner, clear of the indicator, which is the same colour.
                    let c = egui::pos2(tr.rect.right() - 4.0, tr.rect.top() + 5.0);
                    ui.painter().circle_filled(c, 6.0, th.accent);
                    ui.painter().text(c, egui::Align2::CENTER_CENTER, n_jobs.min(99).to_string(), egui::FontId::proportional(8.5), th.on_accent());
                }
                if tr.clicked() {
                    self.open_transfers();
                }
                let du_shown = self.panes[self.active].tab().is_disk_usage();
                if self.cfg.show_disk_usage && tool_icon(ui, th, &t("Disk usage"), du_shown, icons::bars).clicked() && !du_shown {
                    self.run_cmd(Cmd::DiskUsage);
                }
                // Split button: main part opens the default profile, the chevron lists profiles.
                let popup_id = egui::Id::new("terminal-profiles");
                let (cr, chev) = ui.allocate_exact_size(egui::vec2(16.0, 28.0), egui::Sense::click());
                if chev.hovered() {
                    ui.painter().rect_filled(cr, 6.0, th.surface);
                }
                let cc = cr.center();
                let st = egui::Stroke::new(1.4_f32, if chev.hovered() { th.strong() } else { th.weak });
                ui.painter().line_segment([cc + egui::vec2(-3.5, -1.5), cc + egui::vec2(0.0, 2.0)], st);
                ui.painter().line_segment([cc + egui::vec2(0.0, 2.0), cc + egui::vec2(3.5, -1.5)], st);
                if chev.on_hover_text(t("Terminal profiles")).clicked() {
                    ui.memory_mut(|m| m.toggle_popup(popup_id));
                }
                let main = tool_icon(ui, th, &format!("{} ({})", t("Open terminal here"), hint(Action::OpenTerminal)), false, terminal_icon);
                if main.clicked() {
                    self.open_terminal_profile(None);
                }
                let anchor = main.union(chev_rect_response(ui, cr));
                egui::popup::popup_below_widget(ui, popup_id, &anchor, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                    ui.set_min_width(240.0);
                    for (i, p) in self.cfg.terminal_profiles.clone().iter().enumerate() {
                        let star = if i == self.cfg.default_terminal { "  ★" } else { "" };
                        let hint = format!("{} · {}", p.terminal.label(), if p.shell.is_empty() { t("default shell") } else { p.shell.clone() });
                        if ui.button(format!("{}{star}", p.name)).on_hover_text(hint).clicked() {
                            self.open_terminal_profile(Some(i));
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}  ({})", t("New terminal tab"), hint(Action::NewTerminalTab))).clicked() {
                        self.new_terminal_here(None, None);
                    }
                    if ui.button(t("Manage terminal profiles…")).clicked() {
                        self.open_settings(SettingsPage::Terminal);
                    }
                });
                ui.add_space(8.0);
                if tool_icon(ui, th, &format!("{} ({})", t("Quick view panel"), hint(Action::QuickView)), self.cfg.quick_view, icons::eye).clicked() {
                    self.toggle_quick_view();
                }
                if self.connecting.is_some() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(t("Connecting…")).small().color(th.weak));
                    ui.spinner();
                }
            });
            self.toolbar_right_w = right_group.response.rect.width();
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        let active_loc = self.panes[self.active].tab().loc.clone();
        let active_disp = active_loc.display(|id| self.remote_label(id));
        let row = egui::Rect::from_min_size(ui.max_rect().min - egui::vec2(8.0, 8.0), egui::vec2(ui.max_rect().width() + 16.0, self.chrome.row_height() + 10.0));
        self.chrome.drag_area(ui, row, "sidebar");
        if self.chrome.lights_own_row() {
            // The traffic lights sit alone in the first row; the app menu goes below.
            ui.allocate_space(egui::vec2(ui.available_width(), self.chrome.row_height()));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.set_height(30.0);
                self.sidebar_toggle(ui);
                self.brand_menu(ui);
            });
        } else {
            ui.horizontal(|ui| {
                ui.set_height(self.chrome.row_height());
                ui.add_space(self.chrome.left_inset());
                self.chrome.inline_buttons(ui, th, true);
                self.sidebar_toggle(ui);
                self.brand_menu(ui);
            });
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if self.cfg.bookmark_bar == crate::config::BookmarkBar::Vertical {
                self.sidebar_bookmarks(ui, &active_disp);
                ui.add_space(12.0);
            }
            // Drives: by the "Drives in the sidebar" setting, without the system's own mounts.
            let shown: Vec<crate::drives::Drive> = crate::drives::sidebar(&self.drives, self.cfg.sidebar_drives).into_iter().cloned().collect();
            if !shown.is_empty() {
                section_title(ui, th, &t("Drives"));
            }
            let mut eject = None;
            self.drive_rows.clear();
            for d in shown.iter().cloned() {
                let (mp, total, avail) = (d.mount.clone(), d.total, d.avail);
                let sel = active_loc.local_path().is_some_and(|p| p == mp);
                let frac = if total > 0 { 1.0 - avail as f32 / total as f32 } else { 0.0 };
                let name = d.name.clone();
                let bar = (total > 0 && self.cfg.show_disk_usage).then_some(frac);
                let button = if d.ejectable { 26.0 } else { 0.0 };
                let resp = side_row_with(ui, th, sel, &name, bar, button, |p, r| icons::drive_icon(p, r, th));
                self.drive_rows.push((mp.clone(), name.clone(), None));
                if d.ejectable {
                    // The eject button at the end of the row; a spinner while it ejects.
                    let r = egui::Rect::from_center_size(egui::pos2(resp.rect.right() - 16.0, resp.rect.top() + 15.0), egui::vec2(22.0, 22.0));
                    if let Some(row) = self.drive_rows.last_mut() {
                        row.2 = Some(r);
                    }
                    if self.ejecting.iter().any(|e| e.0 == mp) {
                        egui::Spinner::new().size(13.0).color(th.weak).paint_at(ui, r.shrink(4.0));
                    } else {
                        let b = ui.interact(r, ui.id().with(("eject", &mp)), egui::Sense::click());
                        if b.hovered() {
                            ui.painter().rect_filled(r, 5.0, if sel { th.bg } else { th.hover() });
                        }
                        icons::eject(ui.painter(), r.shrink(5.0), if b.hovered() { th.text } else { th.weak });
                        if b.on_hover_text(format!("{} {name}", t("Eject"))).clicked() {
                            eject = Some(d.clone());
                        }
                    }
                }
                self.sidebar_drop_target(ui, resp.rect, Location::Local(mp.clone()));
                let resp = resp
                    .on_hover_text(format!("{}\n{} {} / {}", mp.display(), human_size(avail), t("free"), human_size(total)));
                if resp.clicked() {
                    self.navigate_active(Location::Local(mp.clone()));
                }
                resp.context_menu(|ui| {
                    if ui.button(t("Disk usage")).clicked() {
                        self.open_disk_usage(mp.clone());
                        ui.close_menu();
                    }
                    if d.ejectable && ui.add_enabled(!self.ejecting.iter().any(|e| e.0 == mp), egui::Button::new(t("Eject"))).clicked() {
                        eject = Some(d.clone());
                        ui.close_menu();
                    }
                });
            }
            if let Some(d) = eject {
                self.eject_drive(d);
            }

            if !shown.is_empty() {
                ui.add_space(12.0);
            }
            section_title(ui, th, &t("Network"));
            let open: Vec<(u64, String)> = self.remotes.iter().map(|(k, v)| (*k, v.0.clone())).collect();
            for (id, label) in open {
                let sel = matches!(active_loc, Location::Remote { id: rid, .. } if rid == id);
                let short = label.split("://").nth(1).unwrap_or(&label).to_string();
                let resp = side_row(ui, th, sel, &short, None, |p, r| icons::globe(p, r, th.accent)).on_hover_text(&label);
                if resp.clicked() {
                    self.navigate_active(Location::Remote { id, path: "/".into() });
                }
                resp.context_menu(|ui| {
                    if ui.button(t("Disconnect")).clicked() {
                        self.disconnect(id);
                        ui.close_menu();
                    }
                });
            }
            for c in self.cfg.connections.clone() {
                let resp = side_row(ui, th, false, &c.name, None, |p, r| icons::globe(p, r, th.weak)).on_hover_text(c.url());
                if resp.clicked() {
                    self.connect_saved(c.clone());
                }
                resp.context_menu(|ui| {
                    if ui.button(t("Edit")).clicked() {
                        self.show_dialog(Dialog::Connect { save_password: !c.password.is_empty(), profile: c.clone(), save: true }, false);
                        ui.close_menu();
                    }
                    if ui.button(t("Delete")).clicked() {
                        self.cfg.connections.retain(|x| x.name != c.name);
                        self.save_cfg();
                        ui.close_menu();
                    }
                });
            }
            if side_row(ui, th, false, &t("Connect to server…"), None, |p, r| icons::plus(p, r, th.weak)).clicked() {
                self.open_connect_dialog(None);
            }
        });
    }

    fn connect_saved(&mut self, c: ConnectionProfile) {
        if c.password.is_empty() && c.key_file.is_empty() {
            self.open_connect_dialog(Some(c));
        } else {
            self.start_connect(c, false, false);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        let mut open_transfers = false;
        ui.horizontal(|ui| {
            ui.set_height(26.0);
            ui.spacing_mut().item_spacing.x = 8.0;
            if let Some(rj) = self.jobs.iter().find(|j| !j.job.progress.is_queued()).or(self.jobs.first()) {
                let p = rj.job.progress.clone();
                if ui.add(egui::Button::new(RichText::new(t("Tasks")).size(12.0)).small()).on_hover_text(hint(Action::Transfers)).clicked() {
                    open_transfers = true;
                }
                if p.is_paused() {
                    ui.label(RichText::new(t("Paused")).size(12.0).color(th.marked));
                } else if p.is_queued() {
                    ui.label(RichText::new(t("Waiting in queue…")).size(12.0).color(th.weak));
                } else {
                    ui.spinner();
                }
                // A long title is cut (the whole one shows on hover), so that the progress
                // bar and the Cancel button stay in view.
                let title_w = (ui.available_width() - 380.0).clamp(40.0, 360.0);
                ui.allocate_ui(egui::vec2(title_w, 16.0), |ui| ui.add(egui::Label::new(RichText::new(&rj.job.title).size(12.0).color(th.text)).truncate()));
                let (r, _) = ui.allocate_exact_size(egui::vec2(140.0, 5.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 2.5, th.surface);
                match p.fraction() {
                    Some(f) => {
                        let c = if p.is_paused() { th.weak } else { th.accent };
                        ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(r.width() * f, r.height())), 2.5, c);
                    }
                    None => {
                        let t0 = ui.input(|i| i.time) as f32;
                        let x = (t0 * 0.8).fract() * (r.width() + 40.0) - 40.0;
                        let seg = egui::Rect::from_min_max(egui::pos2(r.left() + x.max(0.0), r.top()), egui::pos2((r.left() + x + 40.0).min(r.right()), r.bottom()));
                        ui.painter().rect_filled(seg, 2.5, th.accent);
                    }
                }
                let rate = if rj.speed > 1.0 && !p.counts_items() { format!(" · {}/s", human_size(rj.speed as u64)) } else { String::new() };
                let done = if p.counts_items() { p.amount_text() } else { human_size(p.done()) };
                let txt = match p.fraction() {
                    Some(f) => format!("{:.0}% · {done}{rate}", f * 100.0),
                    None => format!("{done}{rate}"),
                };
                ui.label(RichText::new(txt).size(11.5).color(th.weak)).on_hover_text(p.current());
                if icons::icon_button(ui, 20.0, true, th, |pp, r, c| close_x(pp, r, c)).on_hover_text(t("Cancel")).clicked() {
                    p.cancel();
                }
                if self.jobs.len() > 1 {
                    let titles: Vec<String> = self.jobs.iter().skip(1).map(|j| j.job.title.clone()).collect();
                    ui.label(RichText::new(format!("+{} {}", self.jobs.len() - 1, t("more"))).size(11.5).color(th.weak))
                        .on_hover_text(titles.join("\n"));
                }
            } else if let Some(e) = self.panes[self.active].tab().cursor_entry().filter(|e| !e.is_parent) {
                let info = if e.is_dir {
                    format!("{}   ·   {}", e.name, crate::util::format_time(e.modified))
                } else {
                    format!("{}   ·   {}   ·   {}", e.name, human_size(e.size), crate::util::format_time(e.modified))
                };
                ui.add(egui::Label::new(RichText::new(info).size(11.5).color(th.weak)).truncate());
            }
            if self.type_search.is_some() {
                // While the search box is open: how to leave it.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if key_hint(ui, th, "Esc", &t("Exit search")).clicked() {
                        self.end_type_search(false);
                    }
                    if key_hint(ui, th, "Enter", &t("Keep the filter")).clicked() {
                        self.end_type_search(true);
                    }
                    key_hint(ui, th, "↑↓", &t("Move"));
                });
            } else if self.cfg.show_key_hints {
                // The keys of the focused window: none for file lists where there is none.
                let files = self.panes[self.active].tab().is_files();
                let terminal = self.panes[self.active].tab().is_terminal();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let hints: [(&str, String, Cmd); 7] = [
                        (hint(Action::QuickView), t("Quick view"), Cmd::QuickView),
                        (hint(Action::OpenDefault), t("Open"), Cmd::OpenDefault),
                        (hint(Action::Copy), t("Copy"), Cmd::Copy),
                        (hint(Action::Move), t("Move"), Cmd::Move),
                        (hint(Action::NewFolder), t("New folder"), Cmd::NewFolder),
                        (hint(Action::Delete), t("Delete"), Cmd::Delete),
                        (hint(Action::OpenTerminal), t("Terminal"), Cmd::Terminal),
                    ];
                    for (key, label, cmd) in hints.iter().rev().filter(|h| if h.2 == Cmd::Terminal { !terminal } else { files }) {
                        // Right to left: the ones that no longer fit are left out, not drawn over the job's progress.
                        if ui.available_width() < key_hint_width(ui, key, label) {
                            break;
                        }
                        if key_hint(ui, th, key, label).clicked() {
                            self.run_cmd(*cmd);
                        }
                    }
                });
            }
        });
        if open_transfers {
            self.open_transfers();
        }
    }

    fn palette_entries(&self) -> Vec<palette::Entry> {
        use palette::Entry as E;
        let mut v = Vec::new();
        let q = self.palette.query().trim().to_string();
        // The first two choices search the files: this folder (live filter) or every subfolder.
        let tab = self.panes[self.active].tab();
        let in_term = tab.is_terminal();
        // Beside a file window a terminal, beside a terminal a file window.
        let (other_right, other_below) = if in_term { (t("Files to the right"), t("Files below")) } else { (t("Terminal to the right"), t("Terminal below")) };
        if !q.is_empty() && !tab.is_terminal() && !looks_like_path(&q) {
            v.push(E::pinned(format!("{}: {q}", t("Filter this folder")), "Enter", PalItem::FilterHere(q.clone())));
            if tab.loc.is_local() {
                v.push(E::pinned(format!("{}: {q}", t("Search all subfolders")), "↓ Enter", PalItem::SearchAll(q.clone())));
            }
        }
        if looks_like_path(&q) {
            v.push(E::new(format!("{}: {q}", t("Go to")), "↵", PalItem::Go(q.clone())));
        }
        for (cmd, label, key) in [
            (Cmd::View, t("View"), hint(Action::View)),
            (Cmd::OpenDefault, t("Open with default app"), hint(Action::OpenDefault)),
            (Cmd::OpenWith, t("Open with…"), ""),
            (Cmd::Copy, t("Copy to other panel"), hint(Action::Copy)),
            (Cmd::Move, t("Move to other panel"), hint(Action::Move)),
            (Cmd::Rename, t("Rename"), hint(Action::Rename)),
            (Cmd::BatchRename, t("Batch rename…"), hint(Action::BatchRename)),
            (Cmd::Checksum, t("Checksum: create / verify…"), hint(Action::Checksum)),
            (Cmd::Delete, t("Delete"), hint(Action::Delete)),
            (Cmd::NewFolder, t("New folder…"), hint(Action::NewFolder)),
            (Cmd::NewFile, t("New file…"), hint(Action::NewFile)),
            (Cmd::Pack, t("Compress…"), hint(Action::Compress)),
            (Cmd::ExtractOther, t("Extract to other panel"), hint(Action::ExtractOther)),
            (Cmd::ExtractHere, t("Extract here"), ""),
            (Cmd::CopyPath, t("Copy path"), ""),
            (Cmd::Reveal, t("Show in system file manager"), ""),
            (Cmd::Terminal, t("Open terminal here"), hint(Action::OpenTerminal)),
            (Cmd::Bookmark, t("Bookmark current folder"), hint(Action::Bookmark)),
            (Cmd::Properties, t("Properties"), hint(Action::Properties)),
            (Cmd::DiskUsage, t("Disk usage"), ""),
            (Cmd::Refresh, t("Refresh"), hint(Action::Refresh)),
            (Cmd::CalcSizes, t("Calculate folder sizes"), ""),
            (Cmd::SelectAll, t("Select all"), hint(Action::SelectAll)),
            (Cmd::Split(SplitDir::Right, in_term), t("Split right"), hint(Action::SplitRight)),
            (Cmd::Split(SplitDir::Left, in_term), t("Split left"), hint(Action::SplitLeft)),
            (Cmd::Split(SplitDir::Down, in_term), t("Split down"), hint(Action::SplitDown)),
            (Cmd::Split(SplitDir::Up, in_term), t("Split up"), hint(Action::SplitUp)),
            (Cmd::Split(SplitDir::Right, !in_term), other_right, ""),
            (Cmd::Split(SplitDir::Down, !in_term), other_below, ""),
        ] {
            let dir = match cmd {
                Cmd::Split(d, _) => Some(d),
                _ => None,
            };
            // Only the splits that keep the 2 × 2 limit.
            if dir.is_none_or(|d| self.layout.can_split(self.active, d)) {
                v.push(E::new(label, key, PalItem::Cmd(cmd)));
            }
        }
        if self.panes.len() > 1 {
            v.push(E::new(t("Close window"), hint(Action::CloseWindow), PalItem::Cmd(Cmd::ClosePane)));
        }
        for (i, p) in self.cfg.terminal_profiles.iter().enumerate() {
            v.push(E::new(format!("{}: {}", t("Terminal"), p.name), if i == self.cfg.default_terminal { hint(Action::OpenTerminal) } else { "" }, PalItem::Cmd(Cmd::TerminalProfile(i))));
        }
        let windows = self.panes.len();
        for (act, label, key) in [
            (Act::NewTab, t("New tab"), hint(Action::NewTab)),
            (Act::NewTerminalTab, t("New terminal tab"), hint(Action::NewTerminalTab)),
            (Act::CloseTab, t("Close tab"), hint(Action::CloseTab)),
            (Act::ReopenClosedTab, t("Reopen closed tab"), hint(Action::ReopenTab)),
            (Act::GroupTab, t("Add tab to new group"), ""),
            (Act::UngroupTab, t("Remove from group"), ""),
            (Act::PinTab, t("Pin / unpin tab"), ""),
            (Act::DuplicateTab, t("Duplicate tab"), ""),
            (Act::WindowToNewTab, t("Move window to new tab"), ""),
            (Act::SwapPanels, t("Swap panels"), hint(Action::SwapPanels)),
            (Act::SameFolder, t("Same folder in other panel"), ""),
            (Act::QuickView, t("Quick view panel"), hint(Action::QuickView)),
            (Act::ToggleHidden, t("Show hidden files"), hint(Action::ToggleHidden)),
            (Act::ToggleSidebar, t("Sidebar"), hint(Action::Sidebar)),
            (Act::ToggleHints, t("Key hints"), ""),
            (Act::Connect, t("Connect to server…"), hint(Action::Connect)),
            (Act::Backup, t("Backup…"), hint(Action::Backup)),
            (Act::Transfers, t("Tasks"), hint(Action::Transfers)),
            (Act::Settings, t("Settings…"), hint(Action::Settings)),
            (Act::Shortcuts, t("Keyboard shortcuts"), hint(Action::Help)),
            (Act::Bookmarks, t("Manage bookmarks…"), ""),
            (Act::DefaultApps, t("Manage default apps…"), ""),
            (Act::InvertSelection, t("Invert selection"), hint(Action::InvertSelection)),
            (Act::DeselectAll, t("Deselect all"), "Esc"),
            (Act::ExportSettings, t("Export settings…"), ""),
            (Act::ImportSettings, t("Import settings…"), ""),
            (Act::About, t("About"), ""),
            (Act::CheckUpdates, t("Check for updates…"), ""),
            (Act::Quit, t("Quit"), ""),
        ] {
            if act == Act::WindowToNewTab && windows < 2 {
                continue;
            }
            v.push(E::new(label, key, PalItem::Act(act)));
        }
        // Every window of every tab, so the palette doubles as a tab search.
        for w in &self.strip.tabs {
            let panes = if w.uid == self.live { &self.panes } else { &w.panes };
            let group = w.group.and_then(|g| self.strip.group(g)).map(|g| if g.name.is_empty() { String::new() } else { format!(" · {}", g.name) }).unwrap_or_default();
            for p in panes {
                let tab = p.tab();
                let kind = if tab.is_terminal() { t("Terminal") } else { t("Tab") };
                v.push(E::new(format!("{kind}: {}{group}", tab.title()), "", PalItem::Tab(w.uid, tab.uid)));
            }
        }
        if self.panes[self.active].tab().is_files() {
            let on = self.panes[self.active].tab().quick.clone();
            for (i, q) in self.cfg.quick_filters.iter().enumerate() {
                let tick = if on.as_ref().is_some_and(|a| a.has(&q.name)) { " ✓" } else { "" };
                v.push(E::new(format!("{}: {}{tick}", t("Quick filter"), t(&q.name)), "", PalItem::QuickFilter(Some(i))));
            }
            if self.panes[self.active].tab().quick.is_some() {
                v.push(E::new(format!("{}: {}", t("Quick filter"), t("Show all files")), "", PalItem::QuickFilter(None)));
            }
        }
        for b in Bookmark::leaves(&self.cfg.bookmarks) {
            v.push(E::new(format!("{}: {}", t("Bookmark"), b.name), "", PalItem::Go(bookmark_target(&b.path))));
        }
        for d in self.drives.iter().filter(|d| !d.system) {
            v.push(E::new(format!("{}: {}", t("Drive"), d.name), "", PalItem::Go(d.mount.to_string_lossy().to_string())));
        }
        for c in &self.cfg.connections {
            v.push(E::new(format!("{}: {}", t("Connect"), c.name), "", PalItem::Connect(c.clone())));
        }
        for th in theme::THEMES {
            v.push(E::new(format!("{}: {}", t("Theme"), th.name), "", PalItem::Theme(th.name)));
        }
        for (code, name) in &self.languages {
            v.push(E::new(format!("{}: {name}", t("Language")), "", PalItem::Lang(code.clone())));
        }
        v
    }

    /// While the search box is open, the active file tab is filtered by what is typed.
    fn palette_live_filter(&mut self) {
        let a = self.active;
        if self.pal_filter.is_none() {
            let tab = self.panes[a].tab();
            if !tab.is_files() {
                return;
            }
            self.pal_filter = Some((a, tab.uid, tab.filter_text.clone(), tab.cursor_entry().map(|e| e.name.clone())));
        }
        let Some((side, uid, prev, _)) = self.pal_filter.clone() else { return };
        let q = self.palette.query().trim().to_string();
        let target = if looks_like_path(&q) { prev } else { q };
        let lc = lc!(self);
        if let Some(tab) = self.panes.get_mut(side).map(|p| p.tab_mut()).filter(|t| t.uid == uid) {
            if tab.filter_text != target {
                tab.set_filter(&target, &lc);
            }
        }
    }

    /// Put back the filter and the cursor the window had before the search box opened
    /// (a command chosen there acts on the file the cursor was on).
    fn restore_pal_filter(&mut self) {
        let Some((side, uid, prev, cursor)) = self.pal_filter.take() else { return };
        let lc = lc!(self);
        if let Some(tab) = self.panes.get_mut(side).map(|p| p.tab_mut()).filter(|t| t.uid == uid) {
            if tab.filter_text != prev {
                tab.set_filter(&prev, &lc);
            }
            if let Some(pos) = cursor.and_then(|n| tab.view.iter().position(|&i| tab.entries[i].name == n)) {
                tab.cursor = pos;
                tab.ensure_visible = true;
            }
        }
    }

    fn run_palette_item(&mut self, ctx: &egui::Context, item: PalItem) {
        let a = self.active;
        match item {
            PalItem::Cmd(c) => self.run_cmd(c),
            PalItem::Theme(n) => self.set_theme(ctx, n),
            PalItem::Lang(code) => self.set_language(&code),
            PalItem::Go(path) => {
                if let Some(loc) = self.parse_location_text(&path) {
                    self.navigate_active(loc);
                }
            }
            PalItem::Connect(c) => self.connect_saved(c),
            PalItem::QuickFilter(i) => self.set_quick_filter(i),
            PalItem::FilterHere(q) => {
                let lc = lc!(self);
                if self.panes[a].tab().filter_text != q {
                    self.panes[a].tab_mut().set_filter(&q, &lc);
                }
            }
            PalItem::SearchAll(q) => {
                let lc = lc!(self);
                if let Err(e) = self.panes[a].tab_mut().start_search(&q, &lc) {
                    self.toast(e, true);
                }
            }
            PalItem::Tab(tab, view) => {
                if let Some(i) = self.strip.index_of(tab) {
                    self.show_tab(i);
                    if let Some(side) = self.panes.iter().position(|p| p.tab().uid == view) {
                        self.active = side;
                        self.focus_window();
                    }
                    self.quick = None;
                }
            }
            PalItem::Act(act) => match act {
                Act::NewTab => self.new_tab(),
                Act::CloseTab => self.request_close_tabs(&[self.live]),
                Act::SwapPanels => {
                    self.swap_with_other();
                    self.quick = None;
                }
                Act::SameFolder => {
                    let loc = self.panes[a].tab().loc.clone();
                    let o = self.other_or_split();
                    self.panes[o].tab_mut().navigate(loc, &lc!(self));
                }
                Act::QuickView => self.toggle_quick_view(),
                Act::ToggleHidden => {
                    self.cfg.show_hidden = !self.cfg.show_hidden;
                    self.rebuild_views();
                    self.save_cfg();
                }
                Act::ToggleSidebar => {
                    self.cfg.show_sidebar = !self.cfg.show_sidebar;
                    self.save_cfg();
                }
                Act::ToggleHints => {
                    self.cfg.show_key_hints = !self.cfg.show_key_hints;
                    self.save_cfg();
                }
                Act::Connect => self.open_connect_dialog(None),
                Act::Backup => self.backup.open = true,
                Act::Transfers => self.open_transfers(),
                Act::Settings => self.open_settings(self.settings_page),
                Act::Shortcuts => self.show_help = true,
                Act::About => self.show_about = true,
                Act::CheckUpdates => self.check_for_updates(true),
                Act::Bookmarks => self.open_settings(SettingsPage::Bookmarks),
                Act::DefaultApps => self.open_settings(SettingsPage::DefaultApps),
                Act::InvertSelection => self.panes[a].tab_mut().invert_marks(),
                Act::DeselectAll => self.panes[a].tab_mut().marked.clear(),
                Act::ExportSettings => self.export_settings(),
                Act::ImportSettings => self.import_settings(),
                Act::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Act::NewTerminalTab => self.new_terminal_here(None, None),
                Act::ReopenClosedTab => self.reopen_closed_tab(),
                Act::GroupTab => {
                    let i = self.strip.active;
                    if !self.strip.tabs[i].pinned {
                        self.strip.new_group(i);
                    }
                }
                Act::UngroupTab => {
                    let i = self.strip.active;
                    self.strip.set_tab_group(i, None);
                }
                Act::PinTab => {
                    let i = self.strip.active;
                    self.strip.toggle_pin(i);
                }
                Act::DuplicateTab => self.duplicate_tab(self.live),
                Act::WindowToNewTab => {
                    let (at, group) = self.next_slot();
                    let view = self.panes[a].tab().uid;
                    self.detach_window(view, at, group);
                }
            },
        }
    }

    fn export_settings(&mut self) {
        if let Some(p) = rfd::FileDialog::new().set_file_name("matrix_finder-settings.json").save_file() {
            match self.cfg.export_to(&p, false) {
                Ok(()) => self.toast(format!("{} {}", t("Exported to"), p.display()), false),
                Err(e) => self.toast(e, true),
            }
        }
    }

    /// Pick a settings file; replacing the settings with it is asked first.
    fn import_settings(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
            self.ask_import_settings(p);
        }
    }

    fn ask_import_settings(&mut self, path: PathBuf) {
        match Settings::import_from(&path) {
            Ok(s) => self.show_dialog(Dialog::ImportSettings { path, settings: Box::new(s) }, false),
            Err(e) => self.toast(e, true),
        }
    }

    /// Replace the settings with imported ones, after keeping a copy of the current ones
    /// (passwords included). Without that copy nothing is replaced.
    fn import_confirmed(&mut self, settings: Settings) {
        // The open tabs too, as they are now.
        self.save_cfg();
        match self.store.backup_before_import(&self.cfg) {
            Ok(copy) => {
                self.cfg = settings;
                let ctx = self.egui_ctx.clone();
                self.apply_cfg(&ctx);
                self.save_cfg();
                self.toast(format!("{} {}", t("Settings imported. The previous settings are kept in"), copy.display()), false);
            }
            Err(e) => self.toast(format!("{}: {e}", t("Settings not imported: the current settings could not be backed up")), true),
        }
    }

    fn panes_area(&mut self, ui: &mut egui::Ui) {
        let mut full = ui.available_rect_before_wrap();
        // Horizontal bookmarks bar, above the tab bar.
        if self.cfg.bookmark_bar == crate::config::BookmarkBar::Horizontal && !self.cfg.bookmarks.is_empty() {
            let r = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 26.0));
            let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
            bui.set_clip_rect(r.intersect(ui.clip_rect()));
            self.bookmarks_bar(&mut bui);
            full.min.y = r.bottom() + 4.0;
        }
        // The one tab bar, above the windows of the shown tab.
        let bar = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), crate::tabs::TAB_H));
        let profiles: Vec<String> = self.cfg.terminal_profiles.iter().map(|p| p.name.clone()).collect();
        let infos = self.tab_infos();
        let dragged_window = egui::DragAndDrop::payload::<crate::tabs::WinDrag>(ui.ctx()).map(|d| (d.uid, self.panes.len()));
        let mut tab_events = Vec::new();
        {
            let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::top_down(egui::Align::Min)));
            let bctx = crate::tabs::BarCtx { theme: self.theme, terminal_profiles: &profiles, can_reopen: !self.closed_tabs.is_empty(), dragged_window };
            crate::tabs::tab_bar(&mut bar_ui, &mut self.strip, &infos, &bctx, &mut tab_events);
        }
        self.sync_tab();
        let rect = egui::Rect::from_min_max(egui::pos2(full.left(), bar.bottom() + 6.0), full.max);
        let mut rects = Vec::new();
        let mut handles = Vec::new();
        self.layout.arrange(rect, 10.0, &mut rects, &mut handles);
        // Dividers between panes.
        for (i, h) in handles.iter().enumerate() {
            let hr = ui.interact(h.rect, egui::Id::new(("splitter", i, h.path.clone())), egui::Sense::click_and_drag());
            if hr.hovered() || hr.dragged() {
                ui.ctx().set_cursor_icon(if h.horizontal { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
                let line = if h.horizontal {
                    egui::Rect::from_center_size(h.rect.center(), egui::vec2(3.0, h.rect.height() * 0.3))
                } else {
                    egui::Rect::from_center_size(h.rect.center(), egui::vec2(h.rect.width() * 0.3, 3.0))
                };
                ui.painter().rect_filled(line, 1.5, self.theme.accent.gamma_multiply(0.8));
            }
            if hr.dragged() {
                if let Some(pos) = hr.interact_pointer_pos() {
                    let v = if h.horizontal { (pos.x - h.area.left()) / h.area.width() } else { (pos.y - h.area.top()) / h.area.height() };
                    self.layout.set_ratio(&h.path, v);
                }
            }
            if hr.double_clicked() {
                self.layout.set_ratio(&h.path, 0.5);
            }
        }
        self.pane_rects = rects.clone();
        let (agent, agents) = self.ai_button();
        let mut events: Vec<(usize, PaneEvent)> = vec![];
        let mut settings_areas = Vec::new();
        let mut transfers_areas = Vec::new();
        let mut tool_areas = Vec::new();
        for &(side, r) in &rects {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
            child.set_clip_rect(r.intersect(ui.clip_rect()));
            let bookmarked = {
                let loc = self.panes[side].tab().loc.display(|id| self.remote_label(id));
                Bookmark::leaves(&self.cfg.bookmarks).any(|b| bookmark_target(&b.path) == loc)
            };
            let window_number = if self.panes.len() > 1 { self.layout.leaves().iter().position(|l| *l == side).map_or(0, |i| i + 1) } else { 0 };
            let ctx = PaneUiCtx {
                window_number,
                theme: self.theme,
                app_theme: self.theme,
                active: side == self.active && self.panes.len() > 1 || self.panes.len() == 1,
                drives: &self.drives,
                list: lc!(self),
                is_bookmarked: bookmarked,
                show_free: self.cfg.show_disk_usage,
                pane_count: self.panes.len(),
                can_split: [SplitDir::Right, SplitDir::Left, SplitDir::Down, SplitDir::Up].map(|d| self.layout.can_split(side, d)),
                terminal_profiles: &profiles,
                focused: side == self.active && self.dialog.is_none() && !self.palette.open,
                term_font_size: self.cfg.term_font_size,
                term_copy_on_select: self.cfg.term_copy_on_select,
                term_scroll: [self.cfg.term_scroll_trackpad, self.cfg.term_scroll_wheel],
                quick_filters: &self.cfg.quick_filters,
                agent: agent.as_ref(),
                agents: &agents,
                git_branches: self.cfg.git_branches,
                git_label: self.cfg.git_label,
                git_style: self.cfg.git_style,
                git_status: self.cfg.git_status,
            };
            for e in pane_ui(&mut child, &mut self.panes[side], side, &ctx) {
                match e {
                    PaneEvent::SettingsArea(r) => settings_areas.push((side, r)),
                    PaneEvent::TransfersArea(r) => transfers_areas.push((side, r)),
                    PaneEvent::ToolArea(r) => tool_areas.push((side, r)),
                    e => events.push((side, e)),
                }
            }
        }
        // Settings windows: the app draws their pages into the space left for them.
        for (side, r) in settings_areas {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
            child.set_clip_rect(r.intersect(ui.clip_rect()));
            self.settings_view(&mut child, side);
        }
        // Likewise Transfers windows.
        for (side, r) in transfers_areas {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
            child.set_clip_rect(r.intersect(ui.clip_rect()));
            self.transfers_view(&mut child, side);
        }
        // And Checksum / Batch rename windows.
        for (side, r) in tool_areas {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
            child.set_clip_rect(r.intersect(ui.clip_rect()));
            self.tool_view(&mut child, side);
        }
        self.window_drop_zones(ui, &rects);
        for (side, e) in events {
            self.handle_pane_event(side, e);
        }
        for e in tab_events {
            self.handle_tab_event(e);
        }
    }

    /// While a window (by its grip) or another tab is dragged over the windows:
    /// the middle of a window swaps the two, its edges dock beside it.
    fn window_drop_zones(&mut self, ui: &egui::Ui, rects: &[(usize, egui::Rect)]) {
        let ctx = ui.ctx().clone();
        let win = egui::DragAndDrop::payload::<crate::tabs::WinDrag>(&ctx);
        let tab = egui::DragAndDrop::payload::<crate::tabs::TabDrag>(&ctx).map(|d| d.uid).filter(|u| *u != self.live);
        let src = win.and_then(|w| self.panes.iter().position(|p| p.tab().uid == w.uid));
        let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        if src.is_none() && tab.is_none() {
            return;
        }
        let th = self.theme;
        // Ghost label for a dragged window.
        if let Some(s) = src {
            let title = self.panes[s].tab().title();
            egui::Area::new(egui::Id::new("window-drag-ghost")).order(egui::Order::Tooltip).fixed_pos(pointer + egui::vec2(12.0, 8.0)).interactable(false).show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(title);
                });
            });
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let Some(&(target, r)) = rects.iter().find(|(i, r)| r.contains(pointer) && Some(*i) != src) else { return };
        // Which zone: a quarter at each edge, the middle swaps (windows only).
        let u = (pointer - r.min) / r.size();
        let edge = [(u.x, SplitDir::Left), (1.0 - u.x, SplitDir::Right), (u.y, SplitDir::Up), (1.0 - u.y, SplitDir::Down)]
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap();
        let dir = if edge.0 < 0.25 || src.is_none() { Some(edge.1) } else { None };
        let ok = match (src, dir) {
            (Some(_), None) => true,
            (Some(s), Some(d)) => {
                let mut l = self.layout.clone();
                l.detach(s) && l.dock(target, Layout::Leaf(s), d) && Layout::fits(&l)
            }
            (None, Some(d)) => tab.and_then(|uid| self.strip.index_of(uid)).is_some_and(|i| {
                let mut sub = self.strip.tabs[i].layout.clone();
                sub.offset(self.panes.len());
                self.layout.can_dock(target, &sub, d)
            }),
            (None, None) => false,
        };
        let p = ui.painter();
        if !ok {
            p.rect_stroke(r.shrink(2.0), 8.0, egui::Stroke::new(2.0_f32, th.danger.gamma_multiply(0.6)), egui::StrokeKind::Inside);
            ctx.set_cursor_icon(egui::CursorIcon::NoDrop);
            return;
        }
        let zone = match dir {
            Some(SplitDir::Left) => egui::Rect::from_min_max(r.min, egui::pos2(r.center().x, r.bottom())),
            Some(SplitDir::Right) => egui::Rect::from_min_max(egui::pos2(r.center().x, r.top()), r.max),
            Some(SplitDir::Up) => egui::Rect::from_min_max(r.min, egui::pos2(r.right(), r.center().y)),
            Some(SplitDir::Down) => egui::Rect::from_min_max(egui::pos2(r.left(), r.center().y), r.max),
            None => r,
        }
        .shrink(4.0);
        p.rect(zone, 8.0, th.accent.gamma_multiply(0.16), egui::Stroke::new(2.0_f32, th.accent.gamma_multiply(0.8)), egui::StrokeKind::Inside);
        let label = if dir.is_none() { t("Swap") } else { t("Dock here") };
        p.text(zone.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(14.0), th.strong());
        if ctx.input(|i| i.pointer.any_released()) {
            egui::DragAndDrop::clear_payload(&ctx);
            match (src, dir, tab) {
                (Some(s), None, _) => self.swap_windows(s, target),
                (Some(s), Some(d), _) => self.move_window(s, target, d),
                (None, Some(d), Some(uid)) => self.merge_tab(uid, target, Some(d)),
                _ => {}
            }
        }
    }

    /// The single Quick View panel (right side), showing the active pane's item.
    fn quick_view_panel(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        let a = self.active;
        let want_key = {
            let tab = self.panes[a].tab();
            tab.cursor_entry().map(|e| format!("{}|{}", tab.loc.display(|id| self.remote_label(id)), e.name))
        };
        if want_key.is_some() && self.quick.as_ref().map(|q| Some(&q.key)) != Some(want_key.as_ref()) {
            self.quick = self.preview_for_cursor(a);
        } else if want_key.is_none() && self.panes[a].tab().is_files() {
            // An empty list (a filter that matches nothing) has nothing to preview.
            self.quick = None;
        }
        egui::Frame::NONE
            .fill(th.bg)
            .corner_radius((th.radius * 2).max(4))
            .stroke(egui::Stroke::new(1.0_f32, th.border))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                // Fixed size: previews never resize the panel.
                let size = ui.available_size();
                ui.set_min_size(size);
                ui.set_max_size(size);
                ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                match &mut self.quick {
                    Some(q) => q.ui(ui, th, t),
                    None => {
                        ui.centered_and_justified(|ui| {
                            ui.label(RichText::new(t("Nothing to preview")).color(th.weak));
                        });
                    }
                }
            });
        if let Some(file) = self.quick.as_mut().and_then(|q| q.take_password_request()) {
            self.ask_archive_password(file, AfterPassword::Preview);
        }
    }

    fn handle_pane_event(&mut self, side: usize, e: PaneEvent) {
        match e {
            PaneEvent::Focus => {
                if self.active != side {
                    self.active = side;
                    self.quick = None;
                }
            }
            PaneEvent::Open => {
                self.active = side;
                self.open_cursor();
            }
            PaneEvent::NavigateText(text) => {
                self.active = side;
                // A relative path is taken from the window's folder.
                let text = crate::pathfield::resolve_typed(&text, dirs::home_dir().as_deref(), self.panes[side].tab().loc.local_path());
                if let Some(loc) = self.parse_location_text(&text) {
                    self.navigate_active(loc);
                }
            }
            PaneEvent::Up => self.panes[side].tab_mut().go_up(&lc!(self)),
            PaneEvent::Back => self.panes[side].tab_mut().go_back(&lc!(self)),
            PaneEvent::Forward => self.panes[side].tab_mut().go_forward(&lc!(self)),
            PaneEvent::Navigate(loc) => {
                self.active = side;
                self.panes[side].tab_mut().navigate(loc, &lc!(self));
            }
            PaneEvent::Command(cmd) => {
                self.active = side;
                self.run_cmd(cmd);
            }
            PaneEvent::ClearFilter => {
                self.active = side;
                self.panes[side].tab_mut().clear_search(&lc!(self));
            }
            PaneEvent::DragOver(loc) => self.drag_target = Some(loc),
            PaneEvent::ClosePane => self.request_close_pane(side),
            PaneEvent::Drop(pl, target) => self.handle_drop(&pl, target),
            PaneEvent::Sort(k) => {
                self.active = side;
                self.panes[side].tab_mut().set_sort(k, &lc!(self));
            }
            PaneEvent::Term(r) => self.handle_term_response(side, r),
            PaneEvent::Agent(a) => self.agent_act(side, a),
            PaneEvent::SettingsArea(_) | PaneEvent::TransfersArea(_) | PaneEvent::ToolArea(_) => {}
            PaneEvent::QuickFilter(i) => {
                self.active = side;
                self.set_quick_filter(i);
            }
            PaneEvent::EditQuickFilters => self.open_settings(SettingsPage::QuickFilters),
            PaneEvent::Du(a) => match a {
                DuAction::OpenFolder(p) => {
                    let (at, group) = self.next_slot();
                    self.new_files_tab(at, group, Some(Location::Local(p)));
                }
                DuAction::Message(text, error) => self.toast(text, error),
            },
        }
    }

    // ------------------------------------------------------------ dialogs

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.dialog.is_none() {
            if let Some((next, focus)) = self.pending_dialogs.pop_front() {
                self.dialog = Some(next);
                self.dialog_focus = focus;
            }
        }
        let Some(mut dialog) = self.dialog.take() else { return };
        // The result of trying an archive's password.
        if let Dialog::ArchivePassword { file, password, wrong, checking, .. } = &mut dialog {
            match checking.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(true)) => {
                    archive::remember_password(file, password);
                    if let Dialog::ArchivePassword { then, .. } = dialog {
                        self.after_password(then);
                    }
                    return;
                }
                Some(Ok(false)) | Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    *checking = None;
                    *wrong = true;
                    password.clear();
                    self.dialog_focus = true;
                }
                Some(Err(std::sync::mpsc::TryRecvError::Empty)) => ctx.request_repaint_after(Duration::from_millis(50)),
                None => {}
            }
        }
        let th = self.theme;
        let mut close = false;
        let mut confirmed = false;
        // The folder field of Copy / Move, Compress and Extract: its suggestions and tree
        // take their keys before the dialog does.
        let dst_id = egui::Id::new("dialog-dst");
        if let Dialog::Transfer { dst, .. } | Dialog::Pack { dest_dir: dst, .. } | Dialog::Unpack { dest: dst, .. } = &mut dialog {
            self.dialog_path.keys(ctx, dst, dst_id);
        }
        let home = dirs::home_dir();
        let focus = std::mem::take(&mut self.dialog_focus);
        let modal = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            ui.set_min_width(440.0);
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            // Enter confirms from a text field, or when nothing has the keyboard. On a button
            // or a checkbox it is that widget's key: Cancel cancels, a checkbox only ticks.
            let enter_free = ui.ctx().memory(|m| m.focused()).is_none_or(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
            let focus_edit = |r: &egui::Response| {
                if focus {
                    r.request_focus();
                }
            };
            match &mut dialog {
                Dialog::Transfer { src, mv, items, dst, policy } => {
                    ui.heading(format!("{} {}", if *mv { t("Move") } else { t("Copy") }, what(items)));
                    if items.len() > 1 {
                        ui.label(RichText::new(preview_names(items)).color(th.weak));
                    }
                    ui.add_space(4.0);
                    ui.label(t("To:"));
                    let o = crate::pathfield::Opts { base: src.local_path(), show_hidden: self.cfg.show_hidden, drives: &self.drives, width: f32::INFINITY, hint: None };
                    let r = self.dialog_path.show(ui, dst, dst_id, o, th);
                    focus_edit(&r);
                    ui.horizontal(|ui| {
                        ui.label(t("If file exists:"));
                        ui.radio_value(policy, Conflict::Rename, t("Keep both"));
                        ui.radio_value(policy, Conflict::Overwrite, t("Overwrite"));
                        ui.radio_value(policy, Conflict::Skip, t("Skip"));
                    });
                }
                Dialog::NewItem { name, file, .. } => {
                    ui.heading(if *file { t("New file") } else { t("New folder") });
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY).hint_text(t("Name")));
                    focus_edit(&r);
                }
                Dialog::Rename { old, new, .. } => {
                    ui.heading(format!("{} “{}”", t("Rename"), old));
                    let r = ui.add(egui::TextEdit::singleline(new).desired_width(f32::INFINITY));
                    if focus {
                        r.request_focus();
                        // Select the name without the extension.
                        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                            let end = new.rfind('.').filter(|&i| i > 0).unwrap_or(new.len());
                            let end = new[..end].chars().count();
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(end))));
                            state.store(ui.ctx(), r.id);
                        }
                    }
                }
                Dialog::Delete { items, trash, loc, .. } => {
                    let verb = if *trash { t("Move to Trash") } else { t("Delete permanently") };
                    ui.heading(format!("{verb} {}?", what(items)));
                    if items.len() > 1 {
                        ui.label(RichText::new(preview_names(items)).color(th.weak));
                    }
                    ui.add_space(4.0);
                    if loc.is_local() {
                        ui.checkbox(trash, t("Move to trash (recycle bin)"));
                    }
                    if !*trash {
                        ui.colored_label(th.danger, t("Files will be permanently deleted!"));
                    }
                }
                Dialog::Pack { items, src_dir, name, dest_dir, format, level } => {
                    ui.set_width(520.0);
                    ui.heading(format!("{} {}", t("Compress"), what(items)));
                    if items.len() > 1 {
                        ui.add(egui::Label::new(RichText::new(preview_names(items)).color(th.weak)).truncate());
                    }
                    ui.add_space(8.0);
                    // The format: a row of choices, and what the one chosen is good for.
                    let one_file = items.len() == 1 && !items[0].is_dir;
                    let rar = archive::rar_cli().is_some();
                    ui.label(RichText::new(t("Format")).strong());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                        for f in ArchiveFormat::offered(one_file) {
                            let enabled = f != ArchiveFormat::Rar || rar;
                            let r = format_chip(ui, th, f, *format == f, enabled);
                            if r.clicked() && enabled {
                                *format = f;
                            }
                        }
                    });
                    ui.add_space(2.0);
                    ui.label(RichText::new(format.blurb()).size(12.0).color(th.weak));
                    ui.add_space(10.0);
                    let label_w = 96.0;
                    let row_label = |ui: &mut egui::Ui, text: String| {
                        ui.allocate_ui_with_layout(egui::vec2(label_w, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.set_min_width(label_w);
                            ui.label(text);
                        });
                    };
                    ui.horizontal(|ui| {
                        row_label(ui, t("Name"));
                        let ext = format!(".{}", format.extension());
                        let ext_w = ui.fonts(|f| f.layout_no_wrap(ext.clone(), egui::FontId::proportional(14.0), th.weak).size().x);
                        let w = ui.available_width() - ext_w - ui.spacing().item_spacing.x - 8.0;
                        let r = ui.add(egui::TextEdit::singleline(name).id(egui::Id::new("pack-name")).margin(egui::Margin::symmetric(4, 2)).desired_width(w));
                        focus_edit(&r);
                        // A format typed at the end of the name is chosen.
                        if r.changed() {
                            if let Some(f) = ArchiveFormat::typed_in(name).filter(|f| (one_file || !f.single_file()) && (*f != ArchiveFormat::Rar || rar)) {
                                *format = f;
                            }
                        }
                        let typed = ArchiveFormat::typed_in(name) == Some(*format);
                        ui.label(RichText::new(ext).color(if typed { th.surface } else { th.weak }));
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        row_label(ui, t("Save to"));
                        let o = crate::pathfield::Opts { base: Some(src_dir.as_path()), show_hidden: self.cfg.show_hidden, drives: &self.drives, width: f32::INFINITY, hint: None };
                        self.dialog_path.show(ui, dest_dir, dst_id, o, th);
                    });
                    if format.compresses() {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            row_label(ui, t("Compression"));
                            ui.label(RichText::new(t("Faster")).size(12.0).color(th.weak));
                            ui.spacing_mut().slider_width = 220.0;
                            ui.add(egui::Slider::new(level, 1..=9).show_value(false)).on_hover_text(format!("{} {level}", t("Level")));
                            ui.label(RichText::new(t("Smaller")).size(12.0).color(th.weak));
                        });
                    }
                    // Where it goes, and what happens next.
                    ui.add_space(10.0);
                    let dest = pack_dest(dest_dir, name, *format, home.as_deref(), src_dir);
                    let exists = dest.as_ref().is_some_and(|d| d.exists());
                    egui::Frame::NONE.fill(th.bg).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        // The file's name, and the folder it goes in (long paths are cut, not the name).
                        ui.horizontal(|ui| {
                            let (ir, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                            icons::mi_archive(ui.painter(), ir, th.weak);
                            let file = dest.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| t("Choose where to save it"));
                            ui.add(egui::Label::new(RichText::new(file).color(th.strong()).strong()).truncate());
                        });
                        if let Some(dir) = dest.as_ref().and_then(|d| d.parent()) {
                            let dir = dir.display().to_string();
                            ui.add(egui::Label::new(RichText::new(&dir).size(12.0).color(th.text)).truncate()).on_hover_text(dir);
                        }
                        let (note, color) = if exists {
                            (t("A file with this name is already there: choose another name."), th.danger)
                        } else if *format == ArchiveFormat::Rar && !rar {
                            (t("RAR creation needs the 'rar' command-line tool."), th.marked)
                        } else if format.single_file() && !one_file {
                            (t("This format compresses a single file. Use .tar.* for folders."), th.marked)
                        } else {
                            (format!("{} ({})", t("It runs in the background: its progress bar is in Tasks"), hint(Action::Transfers)), th.weak)
                        };
                        ui.label(RichText::new(note).size(12.0).color(color));
                    });
                }
                Dialog::Unpack { archive, dest, selection, policy, .. } => {
                    ui.heading(t("Extract"));
                    ui.label(archive.to_string_lossy());
                    if !selection.is_empty() {
                        ui.label(format!("{} {}", selection.len(), t("selected item(s)")));
                    }
                    ui.label(t("Extract to:"));
                    let o = crate::pathfield::Opts { base: archive.parent(), show_hidden: self.cfg.show_hidden, drives: &self.drives, width: f32::INFINITY, hint: None };
                    let r = self.dialog_path.show(ui, dest, dst_id, o, th);
                    focus_edit(&r);
                    ui.horizontal(|ui| {
                        ui.label(t("If file exists:"));
                        ui.radio_value(policy, Conflict::Rename, t("Keep both"));
                        ui.radio_value(policy, Conflict::Overwrite, t("Overwrite"));
                        ui.radio_value(policy, Conflict::Skip, t("Skip"));
                    });
                }
                Dialog::ArchivePassword { file, password, wrong, checking, .. } => {
                    ui.heading(format!("🔒 {}", t("Password required")));
                    ui.label(file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    let edit = egui::TextEdit::singleline(password).password(true).desired_width(f32::INFINITY).hint_text(RichText::new(t("Password")).color(th.weak));
                    let r = ui.add_enabled(checking.is_none(), edit);
                    focus_edit(&r);
                    if checking.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(t("Checking…")).color(th.weak));
                        });
                    } else if *wrong {
                        ui.label(RichText::new(t("The password is wrong.")).color(th.danger));
                    }
                    ui.label(RichText::new(t("Kept only until Matrix Finder closes, and never saved.")).size(12.0).color(th.weak));
                }
                Dialog::OpenWith { path, program, always } => {
                    ui.heading(t("Open with…"));
                    ui.label(path.to_string_lossy());
                    let file_type = path.file_name().and_then(|n| crate::defaultapps::type_of(&n.to_string_lossy()));
                    let hint = if cfg!(target_os = "macos") { "TextEdit.app / code / open -a Preview" } else if cfg!(windows) { "notepad.exe / code" } else { "gedit / code / gimp" };
                    let r = ui.add(egui::TextEdit::singleline(program).hint_text(hint).desired_width(f32::INFINITY));
                    focus_edit(&r);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(t("Quick picks:")).small());
                        for p in quick_apps() {
                            if ui.small_button(p).clicked() {
                                *program = p.to_string();
                            }
                        }
                        if ui.small_button(t("Browse…")).clicked() {
                            if let Some(f) = settings_ui::pick_application() {
                                *program = f;
                            }
                        }
                    });
                    if let Some(ext) = &file_type {
                        ui.add_space(4.0);
                        ui.checkbox(always, format!("{} .{ext}", t("Always use this app to open")));
                        let chosen = self.cfg.default_apps.contains_key(ext);
                        ui.horizontal_wrapped(|ui| {
                            let current = match (chosen, crate::defaultapps::system_app(ui.ctx(), ext)) {
                                (true, _) => format!("{}: {}", t("Default now"), self.cfg.default_apps[ext]),
                                (false, Some(Some(app))) => format!("{}: {app} ({})", t("Default now"), t("system default")),
                                (false, _) => format!("{}: {}", t("Default now"), t("system default")),
                            };
                            ui.label(RichText::new(current).size(12.0).color(th.weak));
                            if chosen && ui.small_button(t("Use system default")).clicked() {
                                self.cfg.default_apps.remove(ext);
                                program.clear();
                                *always = false;
                                self.save_cfg();
                            }
                        });
                    }
                }
                Dialog::Connect { profile, save, save_password } => {
                    ui.heading(t("Connect to server"));
                    egui::Grid::new("conn").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                        ui.label(t("Protocol"));
                        ui.horizontal(|ui| {
                            for p in [Protocol::Sftp, Protocol::Ftp] {
                                if ui.radio_value(&mut profile.protocol, p, p.label()).changed() {
                                    profile.port = p.default_port();
                                }
                            }
                        });
                        ui.end_row();
                        ui.label(t("Host"));
                        ui.horizontal(|ui| {
                            let r = ui.add(egui::TextEdit::singleline(&mut profile.host).desired_width(220.0).hint_text("example.com"));
                            focus_edit(&r);
                            ui.label(t("Port"));
                            ui.add(egui::DragValue::new(&mut profile.port).range(1..=65535));
                        });
                        ui.end_row();
                        ui.label(t("User"));
                        ui.add(egui::TextEdit::singleline(&mut profile.user).desired_width(220.0).hint_text(if profile.protocol == Protocol::Ftp { "anonymous" } else { "" }));
                        ui.end_row();
                        ui.label(t("Password"));
                        ui.add(egui::TextEdit::singleline(&mut profile.password).password(true).desired_width(220.0));
                        ui.end_row();
                        if profile.protocol == Protocol::Sftp {
                            ui.label(t("Private key"));
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut profile.key_file).desired_width(220.0).hint_text("~/.ssh/id_ed25519"));
                                if ui.small_button("…").clicked() {
                                    let mut d = rfd::FileDialog::new();
                                    if let Some(h) = dirs::home_dir() {
                                        d = d.set_directory(h.join(".ssh"));
                                    }
                                    if let Some(f) = d.pick_file() {
                                        profile.key_file = f.to_string_lossy().to_string();
                                    }
                                }
                            });
                            ui.end_row();
                        }
                        ui.label(t("Start folder"));
                        ui.add(egui::TextEdit::singleline(&mut profile.initial_path).desired_width(220.0).hint_text("/"));
                        ui.end_row();
                        ui.label(t("Profile name"));
                        ui.add(egui::TextEdit::singleline(&mut profile.name).desired_width(220.0));
                        ui.end_row();
                    });
                    ui.checkbox(save, t("Save this connection"));
                    if *save {
                        ui.checkbox(save_password, t("Also save the password (stored as plain text)"));
                    }
                    if profile.protocol == Protocol::Sftp && profile.password.is_empty() && profile.key_file.is_empty() {
                        ui.label(RichText::new(t("No password or key: the SSH agent will be used.")).small().color(th.weak));
                    }
                    if profile.protocol == Protocol::Ftp {
                        ui.label(RichText::new(t("Plain FTP is unencrypted. Prefer SFTP when possible.")).small().color(th.marked));
                    }
                }
                Dialog::TrustHostKey { profile, fingerprint } => {
                    ui.heading(t("Unknown server"));
                    ui.label(format!("{}:{}", profile.host, profile.port));
                    ui.add_space(4.0);
                    ui.label(t("This server's identity has not been seen before. Connect only if this fingerprint matches the one its administrator gave you:"));
                    ui.add(egui::Label::new(RichText::new(fingerprint.as_str()).monospace()).selectable(true));
                }
                Dialog::CloseTerminals { target, programs } => {
                    ui.heading(match target {
                        CloseTarget::Window(_) => t("Close this window?"),
                        CloseTarget::Tabs(uids) if uids.len() > 1 => t("Close these tabs?"),
                        CloseTarget::Tabs(_) => t("Close this tab?"),
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(t("Still running:"));
                        ui.label(RichText::new(programs.join(", ")).strong());
                    });
                    ui.label(RichText::new(t("Closing stops what is still running.")).color(th.weak));
                }
                Dialog::ImportSettings { path, .. } => {
                    ui.heading(t("Replace all settings?"));
                    ui.add(egui::Label::new(RichText::new(path.display().to_string()).color(th.weak)).truncate());
                    ui.add_space(4.0);
                    ui.label(t("Every setting is replaced by the imported ones, saved connections, bookmarks and backup profiles included. Exported files hold no passwords."));
                    ui.label(RichText::new(t("A copy of the current settings, passwords included, is kept next to the settings file first.")).color(th.weak));
                }
                Dialog::Message { title, text } => {
                    ui.heading(title.as_str());
                    egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(text.as_str()).monospace()).selectable(true));
                    });
                }
            }
            ui.add_space(14.0);
            // A fixed-height row, so the buttons never stretch the dialog.
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 32.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let is_msg = matches!(dialog, Dialog::Message { .. });
                if is_msg {
                    let b = egui::Button::new(RichText::new(t("Close")).color(th.on_accent()).strong()).fill(th.accent).min_size(egui::vec2(96.0, 30.0));
                    if ui.add(b).clicked() || enter {
                        close = true;
                    }
                    return;
                }
                let ok_label = match &dialog {
                    Dialog::Delete { trash: true, .. } => t("Move to Trash"),
                    Dialog::Delete { .. } => t("Delete"),
                    Dialog::Transfer { mv: true, .. } => t("Move"),
                    Dialog::Transfer { .. } => t("Copy"),
                    Dialog::NewItem { .. } => t("Create"),
                    Dialog::Rename { .. } => t("Rename"),
                    Dialog::Connect { .. } => t("Connect"),
                    Dialog::TrustHostKey { .. } => t("Trust and connect"),
                    Dialog::Pack { .. } => t("Compress"),
                    Dialog::Unpack { .. } => t("Extract"),
                    Dialog::OpenWith { .. } => t("Open"),
                    Dialog::ArchivePassword { .. } => t("Unlock"),
                    Dialog::CloseTerminals { .. } => t("Close"),
                    Dialog::ImportSettings { .. } => t("Replace settings"),
                    _ => t("OK"),
                };
                let danger = matches!(dialog, Dialog::Delete { trash: false, .. } | Dialog::CloseTerminals { .. } | Dialog::ImportSettings { .. });
                let (fill, fg) = if danger { (th.danger, egui::Color32::WHITE) } else { (th.accent, th.on_accent()) };
                let ok = ui.add(egui::Button::new(RichText::new(ok_label).color(fg).strong()).fill(fill).min_size(egui::vec2(96.0, 30.0)));
                // Trusting a server, deleting for good and replacing the settings take a
                // deliberate click (or Enter on that button, tabbed to), never a stray Enter.
                let deliberate = matches!(dialog, Dialog::TrustHostKey { .. } | Dialog::Delete { trash: false, .. } | Dialog::ImportSettings { .. });
                let enter_confirms = enter && enter_free && !deliberate;
                if ok.clicked() || enter_confirms {
                    confirmed = true;
                }
                if ui.add(egui::Button::new(t("Cancel")).min_size(egui::vec2(84.0, 30.0))).clicked() {
                    close = true;
                }
            });
        });
        // Esc closes every dialog; a click outside only those that hold nothing typed or chosen.
        let esc = modal.is_top_modal && !modal.any_popup_open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let simple = matches!(dialog, Dialog::Message { .. } | Dialog::CloseTerminals { .. } | Dialog::TrustHostKey { .. } | Dialog::Delete { .. } | Dialog::ImportSettings { .. });
        if esc || (simple && modal.backdrop_response.clicked()) {
            close = true;
        }
        // Cancel (or Esc) wins over an Enter pressed in the same frame.
        if close {
            confirmed = false;
        }
        if close || confirmed {
            self.dialog_path.reset();
        }
        if confirmed {
            if let Some(next) = self.confirm_dialog(dialog) {
                self.keep_dialog(next);
            }
        } else if !close {
            self.keep_dialog(dialog);
        }
    }

    /// Open dialog `d` (its field taking the keyboard if `focus`). One already open is
    /// not replaced: `d` waits until it closes.
    fn show_dialog(&mut self, d: Dialog, focus: bool) {
        if self.dialog.is_some() {
            self.pending_dialogs.push_back((d, focus));
        } else {
            self.dialog = Some(d);
            self.dialog_focus = focus;
        }
    }

    /// Keep showing dialog `d`; one opened meanwhile waits for it.
    fn keep_dialog(&mut self, d: Dialog) {
        if let Some(other) = self.dialog.replace(d) {
            self.pending_dialogs.push_front((other, true));
        }
    }

    /// Execute a confirmed dialog. Returns a dialog to keep showing (on validation errors).
    fn confirm_dialog(&mut self, d: Dialog) -> Option<Dialog> {
        // Reading from a protected archive: the password comes first, so that the
        // work does not fail halfway. It goes on from here afterwards.
        let reads_from = match &d {
            Dialog::Unpack { archive, .. } => Some(archive),
            Dialog::Transfer { src: Location::Archive { file, .. }, .. } => Some(file),
            _ => None,
        };
        if let Some(file) = reads_from.filter(|f| archive::lock_state(f) != archive::Lock::Open).cloned() {
            return Some(Dialog::ArchivePassword { file, password: String::new(), wrong: false, checking: None, then: AfterPassword::Confirm(Box::new(d)) });
        }
        match d {
            Dialog::ArchivePassword { file, password, wrong, checking, then } => {
                // Enter again while checking, or nothing typed: keep waiting.
                if checking.is_some() || password.is_empty() {
                    return Some(Dialog::ArchivePassword { file, password, wrong, checking, then });
                }
                let (tx, rx) = std::sync::mpsc::channel();
                let (path, typed) = (file.clone(), password.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(archive::check_password(&path, &typed));
                });
                return Some(Dialog::ArchivePassword { file, password, wrong: false, checking: Some(rx), then });
            }
            Dialog::Transfer { src, mv, items, dst, policy } => {
                // Resolve the destination text (it may have been edited).
                let mut known: Vec<Location> = self.panes.iter().map(|p| p.tab().loc.clone()).collect();
                if let Some(d) = self.dialog_dst.take() {
                    known.insert(0, d);
                }
                let dst_loc = match known.into_iter().find(|l| l.display(|id| self.remote_label(id)) == dst) {
                    Some(l) => l,
                    // A relative path is taken from the folder copied from.
                    None => match self.parse_location_text(&crate::pathfield::resolve_typed(&dst, dirs::home_dir().as_deref(), src.local_path())) {
                        Some(l) => l,
                        None => return Some(Dialog::Transfer { src, mv, items, dst, policy }),
                    },
                };
                self.start_transfer(&src, items, &dst_loc, mv, policy);
            }
            Dialog::NewItem { name, file, loc, view } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return None;
                }
                let res: Res<()> = match &loc {
                    Location::Local(d) => {
                        let p = d.join(&name);
                        if file {
                            std::fs::OpenOptions::new().write(true).create_new(true).open(&p).map(|_| ()).map_err(es)
                        } else if crate::safefs::exists(&p) {
                            Err(format!("{}: {name}", t("Already exists")))
                        } else {
                            std::fs::create_dir_all(&p).map_err(es)
                        }
                    }
                    Location::Remote { id, path } => match self.remotes.get(id) {
                        // A name that is taken is refused: uploading an empty file would empty it.
                        Some((_, fs)) => remote::try_use(fs).and_then(|mut g| {
                            let rp = join_slash(path, &name);
                            if g.exists(&rp)? {
                                Err(format!("{}: {name}", t("Already exists")))
                            } else if file {
                                g.create_file(&rp)
                            } else {
                                g.mkdir(&rp)
                            }
                        }),
                        None => Err(t("Connection closed")),
                    },
                    Location::Archive { .. } => Err(t("Archives are read-only")),
                };
                match res {
                    Ok(()) => self.reload_and_select(&loc, view, &name),
                    Err(e) => self.toast(e, true),
                }
            }
            Dialog::Rename { old, new, loc, view } => {
                let new = new.trim().to_string();
                if new.is_empty() || new == old {
                    return None;
                }
                // A server may replace a file silently: a name the folder shows is refused.
                if matches!(loc, Location::Remote { .. }) && all_panes!(self).any(|p| p.tab().loc == loc && p.tab().entries.iter().any(|e| e.name == new)) {
                    self.toast(format!("{}: {new}", t("Already exists")), true);
                    return None;
                }
                let res: Res<()> = match &loc {
                    Location::Local(d) => {
                        let target = d.join(&new);
                        // Only the same file may be there: the name changes case on a disk that ignores it.
                        if crate::safefs::exists(&target) && !same_entry(&d.join(&old), &target, &old, &new) {
                            Err(format!("{}: {new}", t("Already exists")))
                        } else {
                            // Never over a file that appeared since.
                            crate::safefs::rename_item(&d.join(&old), &target).map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { format!("{}: {new}", t("Already exists")) } else { es(e) })
                        }
                    }
                    Location::Remote { id, path } => match self.remotes.get(id) {
                        Some((_, fs)) => remote::try_use(fs).and_then(|mut g| g.rename(&join_slash(path, &old), &join_slash(path, &new))),
                        None => Err(t("Connection closed")),
                    },
                    Location::Archive { .. } => Err(t("Archives are read-only")),
                };
                match res {
                    Ok(()) => self.reload_and_select(&loc, view, &new),
                    Err(e) => self.toast(e, true),
                }
            }
            Dialog::Delete { items, trash, loc, view } => self.do_delete(&loc, view, items, trash),
            Dialog::CloseTerminals { target: CloseTarget::Window(view), .. } => {
                if let Some(side) = self.panes.iter().position(|p| p.tab().uid == view) {
                    self.close_pane(side);
                }
            }
            Dialog::CloseTerminals { target: CloseTarget::Tabs(uids), .. } => self.close_tabs(&uids),
            Dialog::Pack { items, src_dir, name, dest_dir, format, level } => {
                let Some(dest) = pack_dest(&dest_dir, &name, format, dirs::home_dir().as_deref(), &src_dir) else {
                    return Some(Dialog::Pack { items, src_dir, name, dest_dir, format, level });
                };
                if crate::safefs::exists(&dest) {
                    self.toast(format!("{}: {}", t("Already exists"), dest.display()), true);
                    return Some(Dialog::Pack { items, src_dir, name, dest_dir, format, level });
                }
                let sources: Vec<PathBuf> = items.iter().map(|e| src_dir.join(&e.name)).collect();
                let title = format!("{} {}", t("Compressing"), dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                let dest_dir_loc = Location::Local(dest.parent().map(Path::to_path_buf).unwrap_or_default());
                self.spawn_job(title, After::Refresh, Some(dest_dir_loc), vec![src_dir.clone()], move |p| {
                    let made = archive::create(&sources, None, &dest, format, level, &[], p)?;
                    Ok(made.to_string_lossy().to_string())
                });
            }
            Dialog::Unpack { archive, dest, selection, strip, policy } => {
                // `~` and relative paths, as the field suggests them (relative: from the archive's folder).
                let Some(dest) = crate::pathfield::typed_dir(&dest, dirs::home_dir().as_deref(), archive.parent()) else {
                    return Some(Dialog::Unpack { archive, dest, selection, strip, policy });
                };
                let title = format!("{} {}", t("Extracting"), archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                self.spawn_job(title, After::Refresh, Some(Location::Local(dest.clone())), vec![archive.clone()], move |p| {
                    archive::extract(&archive, &dest, &selection, &strip, policy, p)?;
                    Ok(dest.to_string_lossy().to_string())
                });
            }
            Dialog::OpenWith { path, program, always } => {
                let program = program.trim();
                if always {
                    if let Some(ext) = path.file_name().and_then(|n| crate::defaultapps::type_of(&n.to_string_lossy())) {
                        // "Always" with no app given means the system's default from now on.
                        if program.is_empty() {
                            self.cfg.default_apps.remove(&ext);
                        } else {
                            self.cfg.default_apps.insert(ext, program.to_string());
                        }
                        self.save_cfg();
                    }
                }
                let opened = if program.is_empty() { terminal::open_default(&path) } else { terminal::open_with(program, &path) };
                if let Err(e) = opened {
                    self.toast(e, true);
                }
            }
            Dialog::Connect { profile, save, save_password } => {
                if profile.host.trim().is_empty() {
                    return Some(Dialog::Connect { profile, save, save_password });
                }
                self.start_connect(profile, save, save_password);
            }
            Dialog::TrustHostKey { profile, fingerprint } => self.spawn_connect(profile, false, Some(fingerprint)),
            Dialog::Message { .. } => {}
            Dialog::ImportSettings { settings, .. } => self.import_confirmed(*settings),
        }
        None
    }

    /// Icon set picker with live previews. Returns true when the choice changed.
    fn icon_settings(&mut self, ui: &mut egui::Ui) -> bool {
        let th = self.theme;
        if self.icon_themes.is_none() {
            self.icon_themes = Some((crate::sysicons::installed_themes(), crate::sysicons::current_theme_name()));
        }
        let (installed, current) = self.icon_themes.clone().unwrap_or_default();
        let mut options: Vec<(String, String)> = vec![
            ("theme".into(), format!("{} ({})", t("Match theme"), self.theme.icons.label())),
        ];
        if !cfg!(any(target_os = "macos", windows)) {
            let cur = current.clone().map(|c| format!(" ({c})")).unwrap_or_default();
            options.push(("system".into(), format!("{}{cur}", t("System icon theme"))));
        }
        for (k, v) in [("matrix", "Matrix Finder"), ("adwaita", "Adwaita (GNOME)"), ("breeze", "Breeze (KDE Plasma)"), ("macos", "macOS"), ("windows", "Windows 11")] {
            options.push((k.into(), v.into()));
        }
        for n in installed {
            let l = n.to_lowercase();
            if !matches!(l.as_str(), "adwaita" | "breeze" | "breeze-dark" | "hicolor") && !l.contains("cursor") {
                options.push((format!("icons:{n}"), n.clone()));
            }
        }
        let samples: [(&str, bool, &str); 6] = [
            ("Documents", true, ""),
            ("main.rs", false, "rs"),
            ("photo.jpg", false, "jpg"),
            ("backup.zip", false, "zip"),
            ("report.pdf", false, "pdf"),
            ("sheet.xlsx", false, "xlsx"),
        ];
        let mut changed = false;
        egui::Grid::new("icon-packs").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            for (key, label) in &options {
                let selected = self.cfg.icon_pack == *key;
                if ui.radio(selected, label.as_str()).clicked() && !selected {
                    self.cfg.icon_pack = key.clone();
                    changed = true;
                }
                let cache_key = if key == "theme" { format!("theme|{}", th.name) } else { key.clone() };
                if !self.icon_previews.contains_key(&cache_key) {
                    let set = self.make_icon_set(key);
                    self.icon_previews.insert(cache_key.clone(), set);
                }
                let set = self.icon_previews[&cache_key].clone();
                let (rect, _) = ui.allocate_exact_size(egui::vec2(samples.len() as f32 * 26.0, 22.0), egui::Sense::hover());
                for (i, (name, is_dir, ext)) in samples.iter().enumerate() {
                    let r = egui::Rect::from_center_size(egui::pos2(rect.left() + 11.0 + i as f32 * 26.0, rect.center().y), egui::vec2(20.0, 20.0));
                    icons::file_icon_with(&set, ui.painter(), r, name, *is_dir, ext, false, th);
                }
                ui.end_row();
            }
        });
        changed
    }

    /// "Reset to defaults" confirmation (from the Settings window).
    fn reset_dialog(&mut self, ctx: &egui::Context) {
        let th = self.theme;
        let mut keep_open = true;
        let mut reset = false;
        egui::Window::new(t("Reset to defaults"))
            .id(egui::Id::new("settings-reset"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(t("All settings go back to their defaults: Breeze Light theme, English, UI scale, panels, terminal and safety options."));
                ui.label(RichText::new(t("Kept: bookmarks, saved connections, backup profiles, terminal profiles, default apps and open tabs.")).color(th.weak));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let b = egui::Button::new(RichText::new(t("Reset")).strong().color(egui::Color32::WHITE)).fill(th.danger).min_size(egui::vec2(96.0, 30.0));
                        if ui.add(b).clicked() {
                            reset = true;
                        }
                        if ui.add(egui::Button::new(t("Cancel")).min_size(egui::vec2(96.0, 30.0))).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            keep_open = false;
                        }
                    });
                });
            });
        if reset {
            self.cfg.reset_preferences();
            self.set_theme(ctx, &self.cfg.theme.clone());
            self.apply_cfg(ctx);
            self.save_cfg();
            self.toast(t("Settings reset to defaults"), false);
            keep_open = false;
        }
        self.confirm_reset = keep_open;
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        let keys = shortcuts::active();
        let os = keys.os();
        let mut go_to_settings = false;
        egui::Window::new(t("Keyboard shortcuts")).open(&mut open).collapsible(false).default_width(620.0).show(ctx, |ui| {
            if ui.link(t("Change them in Settings")).clicked() {
                go_to_settings = true;
            }
            egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                egui::Grid::new("keys").num_columns(2).striped(true).spacing([24.0, 4.0]).show(ui, |ui| {
                    let row = |ui: &mut egui::Ui, k: &str, d: &str| {
                        ui.label(RichText::new(k).monospace().color(self.theme.accent));
                        ui.label(d);
                        ui.end_row();
                    };
                    for group in shortcuts::GROUPS {
                        ui.label(RichText::new(group.label()).strong());
                        ui.end_row();
                        for action in Action::all().filter(|a| a.group() == group) {
                            let chords: Vec<String> = keys.keys(action).iter().map(|c| c.display(os)).collect();
                            if !chords.is_empty() {
                                row(ui, &chords.join(" / "), &action.label());
                            }
                        }
                    }
                    ui.label(RichText::new(t("Fixed keys")).strong());
                    ui.end_row();
                    let modifier = if os == shortcuts::Os::Mac { "⌘" } else { "Ctrl+" };
                    let copy_paste = if os == shortcuts::Os::Mac { "⌘C / ⌘V".to_string() } else { "Ctrl+Shift+C / V".to_string() };
                    for (k, d) in [
                        ("Tab".to_string(), t("Switch panel")),
                        ("↑ ↓ PgUp PgDn Home End".to_string(), t("Move cursor (Shift = select)")),
                        ("Enter".to_string(), t("Open folder / archive / file")),
                        ("Space / Insert".to_string(), t("Select item")),
                        (t("Typing"), t("Search this folder (Enter keeps the filter, Esc closes)")),
                        ("Esc".to_string(), t("Cancel dragging")),
                        (t("Drag & drop"), t("Copy to a folder or the other panel (Shift = move)")),
                        (format!("{copy_paste} ({})", t("terminal")), t("Copy / paste")),
                        (format!("{modifier}Click ({})", t("terminal")), t("Open link")),
                        (format!("Shift+PgUp / PgDn ({})", t("terminal")), t("Scroll back")),
                    ] {
                        row(ui, &k, &d);
                    }
                });
                ui.separator();
                ui.label(RichText::new(crate::pane::filter_help()).monospace());
                ui.separator();
                ui.label(t("Tip: drop files from your OS file manager onto the window to copy them into the active panel."));
            });
        });
        self.show_help = open && !go_to_settings;
        if go_to_settings {
            self.open_settings(SettingsPage::Shortcuts);
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        if self.logo_large.is_none() {
            self.logo_large = crate::logo::texture(ctx, false, 256);
        }
        egui::Window::new(t("About")).open(&mut open).collapsible(false).resizable(false).default_width(420.0).show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                if let Some(tex) = &self.logo_large {
                    ui.add(egui::Image::new((tex.id(), egui::vec2(112.0, 112.0))));
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Matrix Finder").font(egui::FontId::new(26.0, egui::FontFamily::Name("bold".into()))).color(self.theme.strong()));
                ui.label(RichText::new(t("Files, archives and servers, all in one place.")).color(self.theme.accent));
                ui.label(RichText::new(format!("{} {}", t("Version"), env!("CARGO_PKG_VERSION"))).size(11.5).color(self.theme.weak));
                let checking = self.update_check.is_some();
                if ui.add_enabled(!checking, egui::Button::new(if checking { t("Checking for updates…") } else { t("Check for updates…") })).clicked() {
                    self.check_for_updates(true);
                }
                ui.add_space(4.0);
                ui.label(t("A fast dual-panel file manager written in Rust."));
            });
            ui.separator();
            ui.label(format!("{}: zip, 7z, rar, tar, tar.gz, tar.xz, tar.zst, tar.bz2, gz, xz, zst, bz2", t("Archives")));
            ui.label(format!("{}: {}", t("Protocols"), if cfg!(feature = "sftp") { "Local, FTP, SFTP" } else { "Local, FTP" }));
            ui.label(format!("RAR CLI: {}", archive::rar_cli().map(|p| p.display().to_string()).unwrap_or_else(|| t("not found"))));
            ui.separator();
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    // Centre the credit line.
                    let w = ui.fonts(|f| f.layout_no_wrap(format!("{} quangtiencs.com", t("Made by")), egui::FontId::proportional(14.0), self.theme.text).size().x);
                    ui.add_space(((ui.available_width() - w) / 2.0 - 4.0).max(0.0));
                    ui.label(RichText::new(t("Made by")).color(self.theme.weak));
                    ui.hyperlink_to(RichText::new("quangtiencs.com").color(self.theme.accent), "https://quangtiencs.com");
                });
            });
        });
        self.show_about = open;
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(if t.error { 8 } else { 4 }));
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-18.0, -44.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for toast in self.toasts.iter().rev().take(5) {
                    let th = self.theme;
                    let color = if toast.error { th.danger } else { th.accent };
                    egui::Frame::popup(ui.style())
                        .fill(th.panel)
                        .corner_radius(10)
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .stroke(egui::Stroke::new(1.0_f32, th.border))
                        .show(ui, |ui| {
                            ui.set_max_width(420.0);
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                                ui.painter().circle_filled(r.center(), 4.0, color);
                                ui.add(egui::Label::new(RichText::new(&toast.text).color(th.text)).wrap());
                            });
                        });
                    ui.add_space(4.0);
                }
            });
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if dropped.is_empty() {
            return;
        }
        // Drop into the panel under the pointer (fall back to the active one).
        let pos = ctx.input(|i| i.pointer.hover_pos());
        let a = pos.and_then(|p| self.pane_rects.iter().find(|(_, r)| r.contains(p)).map(|(i, _)| *i)).unwrap_or(self.active);
        // On a terminal: type the paths at the prompt, as other terminals do.
        if let Some(term) = &self.panes[a].tab().term {
            if self.dialog.is_some() {
                return;
            }
            let text = dropped.iter().map(|p| quote_dropped(&p.to_string_lossy())).collect::<Vec<_>>().join(" ");
            if !crate::termview::is_typeable(&text) {
                self.toast(t("A dropped name has control characters: it is not typed into the terminal"), true);
                return;
            }
            term.paste(&format!("{text} "));
            self.active = a;
            self.focus_window();
            return;
        }
        // Settings, Tasks, Disk usage and tool windows show no folder to copy into.
        if !self.panes[a].tab().is_files() {
            self.toast(t("Drop files on a file window to copy them there"), true);
            return;
        }
        let loc = self.panes[a].tab().loc.clone();
        let Some(dst) = self.endpoint(&loc) else { return };
        // Group by parent folder (usually a single one).
        let mut groups: HashMap<PathBuf, Vec<ops::Item>> = HashMap::new();
        for p in dropped {
            let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else { continue };
            let meta = std::fs::metadata(&p).ok();
            groups.entry(parent.to_path_buf()).or_default().push(ops::Item {
                name: name.to_string_lossy().to_string(),
                is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                size: meta.map(|m| m.len()).unwrap_or(0),
            });
        }
        // Asked first when Settings say so, and always while a dialog is open (it waits
        // for that one).
        if self.cfg.confirm_dnd || self.dialog.is_some() {
            let dst_text = loc.display(|id| self.remote_label(id));
            for (parent, items) in groups {
                let items = items.into_iter().map(|i| Entry::new(i.name, i.is_dir, i.size, None)).collect();
                self.show_dialog(Dialog::Transfer { src: Location::Local(parent), mv: false, items, dst: dst_text.clone(), policy: Conflict::Rename }, false);
            }
            return;
        }
        for (parent, items) in groups {
            let d = dst.clone();
            let title = format!("{} {} {}", t("Copying"), items.len(), t("item(s)"));
            self.spawn_job(title, After::Refresh, Some(loc.clone()), vec![parent.clone()], move |p| ops::transfer(Endpoint::Local(parent), items, d, false, Conflict::Rename, p));
        }
    }
}

impl eframe::App for App {
    /// Paint the window background in the theme colour so live resizing never
    /// flashes a dark strip before the next frame.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(self.theme.panel).to_array()
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.chrome.setup_native(frame);
        self.max_texture = ctx.input(|i| i.max_texture_side).max(1024);
        // Follow the OS (theme "System") and keep the icon set in sync.
        crate::chrome::install_quit_guard(frame);
        let wanted = self.resolve_theme(ctx);
        if !std::ptr::eq(wanted, self.theme) {
            self.theme = wanted;
            theme::apply(ctx, wanted);
            self.quick = None;
        }
        let icon_key = format!("{}|{}", self.cfg.icon_pack, self.theme.name);
        if icon_key != self.icon_key {
            icons::set_icons(self.make_icon_set(&self.cfg.icon_pack.clone()));
            self.icon_key = icon_key;
        }
        self.shift_down = ctx.input(|i| i.modifiers.shift);
        crate::fonts::set_reading_serif(self.cfg.reading_font == crate::config::ReadingFont::Serif);
        self.drag_target = None;
        self.sync_tab();
        self.active = self.active.min(self.panes.len() - 1);
        if self.active != self.last_active {
            self.prev_active = self.last_active;
            self.last_active = self.active;
        }
        self.quit_step(ctx);
        self.handle_keys(ctx);
        self.poll_terminals(ctx);
        self.poll_agents();
        self.poll_instance(ctx);
        self.poll_jobs(ctx);
        self.poll_connect();
        self.handle_dropped_files(ctx);
        self.poll_drives(ctx);
        self.poll_update(ctx);
        self.poll_temp_copies(ctx);
        self.save_cfg_pending(ctx);
        let mut busy = self.connecting.is_some();
        for side in 0..self.panes.len() {
            let tab = self.panes[side].tab_mut();
            tab.poll(&lc!(self));
            busy |= self.panes[side].tab().is_loading() || self.panes[side].tab().is_scanning() || self.panes[side].tab().dir_sizes.is_some();
        }
        if busy {
            ctx.request_repaint_after(Duration::from_millis(60));
        }

        // Window title follows the active folder.
        let title = match &self.panes[self.active].tab().term {
            Some(t) => format!("Matrix Finder — {}", t.label()),
            None => format!("Matrix Finder — {}", self.panes[self.active].tab().loc.display(|id| self.remote_label(id))),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        let panel = self.theme.panel;
        self.chrome.resize_edges(ctx);
        // Sidebar first so it spans the full window height (Finder / Nautilus layout).
        if self.cfg.show_sidebar {
            egui::SidePanel::left("sidebar")
                .resizable(false)
                // In a narrow window the sidebar gives up space (never more than ~26 %).
                .exact_width(self.cfg.sidebar_width.min((ctx.screen_rect().width() * 0.26).max(160.0)).round())
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(panel).inner_margin(egui::Margin { left: 8, right: 8, top: self.chrome.top_margin(), bottom: 6 }))
                .show(ctx, |ui| {
                    // The gap on the right edge is the resize grip (no full-height line).
                    let inner = ui.max_rect();
                    let grip = egui::Rect::from_min_max(egui::pos2(inner.right() + 1.0, inner.top()), egui::pos2(inner.right() + 8.0, inner.bottom()));
                    let g = ui.interact(grip, egui::Id::new("sidebar-grip"), egui::Sense::click_and_drag());
                    // Only the cursor changes: a coloured bar next to the panel border looked like two lines.
                    if g.hovered() || g.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    if g.dragged() {
                        if let Some(p) = g.interact_pointer_pos() {
                            self.cfg.sidebar_width = (p.x - ctx.screen_rect().left()).clamp(160.0, 360.0);
                        }
                    }
                    if g.drag_stopped() {
                        self.save_cfg();
                    }
                    if g.double_clicked() {
                        self.cfg.sidebar_width = 210.0;
                        self.save_cfg();
                    }
                    self.sidebar(ui);
                });
        }
        egui::TopBottomPanel::top("topbar")
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(panel).inner_margin(egui::Margin { left: if self.cfg.show_sidebar { 4 } else { 10 }, right: 10, top: self.chrome.top_margin(), bottom: 4 }))
            .show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("status")
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(panel).inner_margin(egui::Margin { left: 6, right: 10, top: 2, bottom: 4 }))
            .show(ctx, |ui| self.status_bar(ui));
        // One Quick View for all panes, docked at the right under the top bar.
        // It is hidden while a terminal tab is active (the setting is kept).
        let shown = self.panes[self.active.min(self.panes.len() - 1)].tab();
        let show_quick = self.cfg.quick_view && shown.is_files();
        if show_quick {
            // Width is a share of the window (1/3 by default) and only changes
            // when the user drags the gap on its left edge.
            let screen = ctx.screen_rect();
            let w = (screen.width() * self.cfg.quick_view_frac).clamp(240.0, (screen.width() * 0.7).max(240.0)).round();
            egui::SidePanel::right("quickview")
                .resizable(false)
                .exact_width(w)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(panel).inner_margin(egui::Margin { left: 10, right: 10, top: 4, bottom: 2 }))
                .show(ctx, |ui| {
                    // The 10 px gap on the left edge is the resize grip.
                    let inner = ui.max_rect();
                    let grip = egui::Rect::from_min_max(egui::pos2(inner.left() - 10.0, inner.top()), egui::pos2(inner.left(), inner.bottom()));
                    let g = ui.interact(grip, egui::Id::new("quickview-grip"), egui::Sense::click_and_drag());
                    // Only the cursor changes: a coloured bar next to the panel border looked like two lines.
                    if g.hovered() || g.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    if g.dragged() {
                        if let Some(p) = g.interact_pointer_pos() {
                            self.cfg.quick_view_frac = ((screen.right() - p.x) / screen.width()).clamp(0.15, 0.7);
                        }
                    }
                    if g.drag_stopped() {
                        self.save_cfg();
                    }
                    if g.double_clicked() {
                        self.cfg.quick_view_frac = 1.0 / 3.0;
                        self.save_cfg();
                    }
                    self.quick_view_panel(ui);
                });
        }
        let left = if self.cfg.show_sidebar { 0 } else { 10 };
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(panel).inner_margin(egui::Margin { left, right: if show_quick { 0 } else { 10 }, top: 4, bottom: 2 }))
            .show(ctx, |ui| self.panes_area(ui));

        // Viewer window (Shift+F3)
        if self.viewer.is_some() {
            let mut open = true;
            let name = self.viewer.as_ref().map(|v| v.name.clone()).unwrap_or_default();
            egui::Window::new(name.clone())
                .id(egui::Id::new("viewer"))
                .open(&mut open)
                .collapsible(false)
                .default_size([820.0, 600.0])
                .show(ctx, |ui| {
                    if let Some(v) = &mut self.viewer {
                        v.ui(ui, self.theme, t);
                    }
                });
            if !open {
                self.viewer = None;
            }
        }
        if self.backup.open {
            let cur = self.panes[self.active].tab().loc.local_path().map(|p| p.to_path_buf());
            let action = self.backup.ui(ctx, &mut self.cfg.backups, cur.as_deref(), &self.drives, self.cfg.show_hidden, self.theme, &t);
            match action {
                Some(BackupAction::Run(profile)) => {
                    self.save_cfg();
                    let title = format!("{} {}", t("Backup"), profile.name);
                    let dest = Some(Location::Local(backup::destination_path(&profile)));
                    let sources = backup::source_paths(&profile);
                    self.spawn_job(title, After::Refresh, dest, sources, move |p| backup::run(&profile, p));
                }
                Some(BackupAction::Restore(file, dest)) => {
                    let title = format!("{} {}", t("Restoring"), file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                    self.spawn_job(title, After::Refresh, Some(Location::Local(dest.clone())), vec![file.clone()], move |p| {
                        // Files already there are kept: the restored ones get a free name next to them.
                        archive::extract(&file, &dest, &[], "", Conflict::Rename, p)?;
                        Ok(dest.to_string_lossy().to_string())
                    });
                }
                Some(BackupAction::Changed) => self.save_cfg(),
                Some(BackupAction::Export) => {
                    if let Some(p) = rfd::FileDialog::new().set_file_name("matrix_finder-backup-profiles.json").save_file() {
                        let r = serde_json::to_string_pretty(&self.cfg.backups).map_err(es).and_then(|s| std::fs::write(&p, s).map_err(es));
                        match r {
                            Ok(()) => self.toast(format!("{} {}", t("Exported to"), p.display()), false),
                            Err(e) => self.toast(e, true),
                        }
                    }
                }
                Some(BackupAction::Import) => {
                    if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                        let r: Res<Vec<crate::config::BackupProfile>> =
                            std::fs::read_to_string(&p).map_err(es).and_then(|s| serde_json::from_str(&s).map_err(es));
                        match r {
                            Ok(list) => {
                                let n = list.len();
                                self.cfg.backups.extend(list);
                                self.save_cfg();
                                self.toast(format!("{} {n}", t("Imported profiles:")), false);
                            }
                            Err(e) => self.toast(e, true),
                        }
                    }
                }
                None => {}
            }
        }
        if self.confirm_reset {
            self.reset_dialog(ctx);
        }
        if matches!(self.quit, Quit::Asking | Quit::Stopping(_)) {
            self.quit_dialog(ctx);
        }
        if self.show_help {
            self.help_window(ctx);
        }
        if self.show_about {
            self.about_window(ctx);
        }
        self.update_window(ctx);
        self.dialogs(ctx);
        self.type_search_ui(ctx);
        if self.palette.open {
            self.palette_live_filter();
            let entries = self.palette_entries();
            match self.palette.show(ctx, self.theme, entries) {
                Some(item) => {
                    // Keep the live filter only when it was the choice.
                    if !matches!(item, PalItem::FilterHere(_)) {
                        self.restore_pal_filter();
                    }
                    self.pal_filter = None;
                    self.run_palette_item(ctx, item);
                }
                None if !self.palette.open => {
                    self.restore_pal_filter();
                    self.pal_filter = None;
                }
                None => {}
            }
        }
        self.chrome.caption_buttons(ctx, self.theme);
        self.chrome.outline(ctx, self.theme);
        self.drag_ghost(ctx);
        self.copy_prompts_ui(ctx);
        self.toasts_ui(ctx);
        if let Some(text) = self.copy_text.take() {
            ctx.copy_text(text);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(s) = &self.instance {
            s.stop();
        }
        self.quit = Quit::Go;
        self.save_cfg();
        // Copies opened from servers and archives: edited ones are kept (moved where the
        // quit confirmation said), the others removed. One that cannot be moved stays in
        // its temporary folder, which is not cleaned up.
        for r in self.temp_copies.finish(&crate::tempcopy::recovered_dir()) {
            match r {
                Ok(p) => eprintln!("matrix_finder: kept an edited copy at {}", p.display()),
                Err(e) => eprintln!("matrix_finder: could not move an edited copy: {e}"),
            }
        }
    }
}

/// The search box opened by typing in a file window.
struct TypeSearch {
    side: usize,
    uid: u64,
    loc: Location,
    /// The window's filter before, put back by Esc.
    before: String,
    text: String,
}

enum TypeSearchKey {
    Text(String),
    Back,
    Keep,
    Cancel,
}

/// An edit made in Settings → Bookmarks, at (top-level index, index inside that folder).
#[derive(Clone, Copy, Debug)]
enum BmEdit {
    Up((usize, Option<usize>)),
    Down((usize, Option<usize>)),
    Delete((usize, Option<usize>)),
    /// Move a bookmark into a folder (top-level index) or out of its folder (`None`).
    MoveTo((usize, Option<usize>), Option<usize>),
}

fn apply_bookmark_edit(list: &mut Vec<Bookmark>, e: BmEdit) {
    match e {
        BmEdit::Up((i, None)) if i > 0 => list.swap(i, i - 1),
        BmEdit::Up((i, Some(j))) if j > 0 => list[i].children.swap(j, j - 1),
        BmEdit::Down((i, None)) if i + 1 < list.len() => list.swap(i, i + 1),
        BmEdit::Down((i, Some(j))) if j + 1 < list[i].children.len() => list[i].children.swap(j, j + 1),
        BmEdit::Delete((i, None)) if list[i].folder => remove_bookmark_folder(list, i),
        BmEdit::Delete((i, None)) => {
            list.remove(i);
        }
        BmEdit::Delete((i, Some(j))) => {
            list[i].children.remove(j);
        }
        BmEdit::MoveTo(at, target) => {
            let b = match at {
                (i, Some(j)) => list[i].children.remove(j),
                (i, None) if !list[i].folder => list.remove(i),
                _ => return,
            };
            match target {
                // Out of a folder: right after it.
                None => list.insert((at.0 + 1).min(list.len()), b),
                Some(f) => {
                    let f = if at.1.is_none() && at.0 < f { f - 1 } else { f };
                    list[f].children.push(b);
                }
            }
        }
        _ => {}
    }
}

/// Remove bookmark folder `i`; its bookmarks take its place.
fn remove_bookmark_folder(list: &mut Vec<Bookmark>, i: usize) {
    let f = list.remove(i);
    for (k, c) in f.children.into_iter().enumerate() {
        list.insert(i + k, c);
    }
}

fn looks_like_path(q: &str) -> bool {
    q.starts_with('/') || q.starts_with('~') || q.contains(":\\") || q.starts_with("sftp://") || q.starts_with("ftp://")
}

/// Whether `old` and `new` (named `old_name` and `new_name`) are the same directory entry:
/// on a disk that ignores case, `a.txt` is there as `A.TXT` too.
fn same_entry(old: &Path, new: &Path, old_name: &str, new_name: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::symlink_metadata(old), std::fs::symlink_metadata(new)) {
            // Two hard links of one file are two entries: only a change of case is the same one.
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino() && (a.is_dir() || a.nlink() <= 1 || old_name.to_lowercase() == new_name.to_lowercase()),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (std::fs::canonicalize(old), std::fs::canonicalize(new)) {
            (Ok(a), Ok(b)) => a == b,
            _ => old_name.to_lowercase() == new_name.to_lowercase(),
        }
    }
}

/// A dropped file's path as typed in a terminal: quoted when it has spaces or other
/// characters the shell would read.
fn quote_dropped(path: &str) -> String {
    if cfg!(windows) {
        if path.contains([' ', '&', '(', ')', '^', ';', ',', '\'', '%', '!']) { format!("\"{path}\"") } else { path.to_string() }
    } else {
        terminal::sh_quote(path)
    }
}

/// Whether `cmd` can act in window `tab`: most act on its file list (the cursor, the marks).
fn cmd_works_in(cmd: Cmd, tab: &Tab) -> bool {
    match cmd {
        Cmd::DefaultApps | Cmd::Terminal | Cmd::TerminalProfile(_) | Cmd::Split(..) | Cmd::ClosePane | Cmd::Refresh => true,
        // A terminal's folder and the one disk usage shows are real folders too.
        Cmd::DiskUsage | Cmd::Bookmark => tab.is_files() || tab.is_terminal() || tab.is_disk_usage(),
        _ => tab.is_files(),
    }
}

fn preview_names(items: &[Entry]) -> String {
    let names: Vec<&str> = items.iter().take(4).map(|e| e.name.as_str()).collect();
    let more = if items.len() > 4 { format!(" … (+{})", items.len() - 4) } else { String::new() };
    format!("{}{more}", names.join(", "))
}

/// Where Compress saves: `name` in format `fmt` inside the folder typed (`~` and
/// relative paths are taken from `home` and `base`). None when no name or folder is given.
fn pack_dest(dir: &str, name: &str, fmt: ArchiveFormat, home: Option<&Path>, base: &Path) -> Option<PathBuf> {
    if name.trim().is_empty() {
        return None;
    }
    let dir = crate::pathfield::typed_dir(dir, home, Some(base))?;
    Some(dir.join(archive::archive_file_name(name, fmt)))
}

/// A format to choose in the Compress dialog.
fn format_chip(ui: &mut egui::Ui, th: &Theme, f: ArchiveFormat, on: bool, enabled: bool) -> egui::Response {
    let text = format!(".{}", f.extension());
    let g = ui.fonts(|fo| fo.layout_no_wrap(text.clone(), egui::FontId::proportional(13.0), th.text));
    let size = egui::vec2(g.size().x + 20.0, 26.0);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (r, resp) = ui.allocate_exact_size(size, sense);
    let (fill, fg, stroke) = if on {
        (th.accent, th.on_accent(), th.accent)
    } else if resp.hovered() && enabled {
        (th.hover(), th.strong(), th.border)
    } else {
        (th.bg, if enabled { th.text } else { th.weak.gamma_multiply(0.6) }, th.border)
    };
    ui.painter().rect(r, 13.0, fill, egui::Stroke::new(1.0_f32, stroke), egui::StrokeKind::Inside);
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, &text, egui::FontId::proportional(13.0), fg);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, enabled, on, &text));
    if enabled { resp.on_hover_text(f.blurb()) } else { resp.on_disabled_hover_text(f.blurb()).on_hover_text(f.blurb()) }
}

/// “name” for one item, “3 items” for several.
fn what(items: &[Entry]) -> String {
    match items {
        [one] => format!("“{}”", one.name),
        _ => format!("{} {}", items.len(), t("items")),
    }
}

fn quick_apps() -> Vec<&'static str> {
    if cfg!(target_os = "macos") {
        vec!["TextEdit.app", "Preview.app", "code", "Safari.app"]
    } else if cfg!(windows) {
        vec!["notepad.exe", "mspaint.exe", "code"]
    } else {
        vec!["xdg-open", "code", "gedit", "kate", "gimp", "vlc"]
    }
}

/// What Quick View says about a named pipe, socket or device: it is not opened.
fn special_file_note(path: &Path) -> String {
    let kind = match crate::util::special_kind(path) {
        Some("Named pipe") => t("Named pipe"),
        Some("Socket") => t("Socket"),
        Some("Device") => t("Device"),
        _ => t("Special file"),
    };
    format!("{kind}\n\n{}", t("Not opened: only regular files are previewed, so nothing waits on it or takes data meant for another program."))
}

fn read_head(path: &Path, limit: usize) -> Res<Vec<u8>> {
    use std::io::Read;
    let f = crate::util::open_regular(path).map_err(es)?;
    let mut buf = Vec::new();
    f.take(limit as u64).read_to_end(&mut buf).map_err(es)?;
    Ok(buf)
}

/// How long a folder or archive summary may read before it shows what it has.
const SUMMARY_TIME: Duration = Duration::from_secs(4);
/// Most names a summary collects (and sorts) to show its first few.
const SUMMARY_NAMES: usize = 10_000;
/// Names shown in a summary.
const SUMMARY_SHOWN: usize = 40;

/// The last line of a summary list: how many names were left out.
fn summary_rest(left: usize, cut: bool) -> String {
    match (left, cut) {
        (_, true) => format!("\n{}", t("…and more")),
        (0, false) => String::new(),
        (n, false) => format!("\n… {n} {}", t("more not shown")),
    }
}

fn folder_summary(path: &Path, name: &str, cancel: &crate::media::Cancel) -> String {
    let started = Instant::now();
    let mut files = 0;
    let mut dirs = 0;
    let mut size = 0u64;
    let mut names: Vec<(bool, String)> = vec![];
    let mut cut = false;
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return String::new();
            }
            if started.elapsed() > SUMMARY_TIME {
                cut = true;
                break;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                dirs += 1;
            } else {
                files += 1;
                size += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
            // Past this many the counts go on, but only these names are sorted.
            if names.len() < SUMMARY_NAMES {
                names.push((is_dir, e.file_name().to_string_lossy().to_string()));
            }
        }
    }
    // Folders first, then by name.
    names.sort_by_cached_key(|(is_dir, n)| (!is_dir, n.to_lowercase()));
    let shown: Vec<String> = names.iter().take(SUMMARY_SHOWN).map(|(is_dir, n)| format!("{} {n}", if *is_dir { "📁" } else { "🗋" })).collect();
    let left = (dirs + files) - shown.len();
    let more = if cut { "+" } else { "" };
    format!(
        "📁 {name}\n\n{dirs}{more} {}, {files}{more} {}\n{}{more} {}\n\n{}{}",
        t("folders"),
        t("files"),
        human_size(size),
        t("in the files at this level (not counting subfolders)"),
        shown.join("\n"),
        summary_rest(left, cut)
    )
}

fn archive_summary(path: &Path, cancel: &crate::media::Cancel) -> String {
    let started = Instant::now();
    let stop = || cancel.load(std::sync::atomic::Ordering::Relaxed) || started.elapsed() > SUMMARY_TIME;
    match archive::list_until(path, 200_000, &stop) {
        _ if cancel.load(std::sync::atomic::Ordering::Relaxed) => String::new(),
        Err(e) if archive::is_password_error(&e) => format!("🔒 {}\n\n{}", t("Password protected"), t("Press Enter to enter the password.")),
        Ok((entries, cut)) => {
            let files = entries.iter().filter(|e| !e.is_dir).count();
            let total: u64 = entries.iter().map(|e| e.size).sum();
            let packed = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let ratio = if total > 0 && !cut { format!(" ({:.0}%)", packed as f64 * 100.0 / total as f64) } else { String::new() };
            let more = if cut { "+" } else { "" };
            let top = archive::children(&entries, "");
            let list: Vec<String> = top
                .iter()
                .take(60)
                .map(|e| format!("{} {}  {}", if e.is_dir { "📁" } else { "🗋" }, e.name, if e.is_dir { String::new() } else { human_size(e.size) }))
                .collect();
            format!(
                "📦 {}\n\n{files}{more} {} · {}{more} → {}{ratio}\n{}\n\n{}{}",
                path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                t("files"),
                human_size(total),
                human_size(packed),
                if archive::lock_state(path) == archive::Lock::Open { t("Press Enter to browse inside the archive.") } else { format!("🔒 {}. {}", t("Password protected"), t("Press Enter to browse inside the archive.")) },
                list.join("\n"),
                summary_rest(top.len() - list.len(), cut)
            )
        }
        Err(e) => format!("{}: {e}", t("Cannot read archive")),
    }
}

#[cfg(unix)]
fn unix_mode_string(mode: u32) -> String {
    let mut s = String::new();
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// A flat toolbar icon; `active` draws a toggled background.
fn tool_icon(ui: &mut egui::Ui, th: &Theme, tip: &str, active: bool, draw: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::click());
    if active {
        ui.painter().rect_filled(rect, 7.0, th.selection);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 7.0, th.surface);
    }
    let color = if active { th.accent } else if resp.hovered() { th.strong() } else { th.weak };
    draw(ui.painter(), rect.shrink2(egui::vec2(8.0, 7.0)), color);
    resp.on_hover_text(tip)
}

fn sidebar_icon(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.3_f32, c);
    p.rect_stroke(r, 2.5, s, egui::StrokeKind::Inside);
    let x = r.left() + r.width() * 0.38;
    p.line_segment([egui::pos2(x, r.top() + 1.0), egui::pos2(x, r.bottom() - 1.0)], s);
}

pub fn terminal_icon(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.4_f32, c);
    p.rect_stroke(r, 2.5, s, egui::StrokeKind::Inside);
    let y = r.center().y;
    p.line_segment([egui::pos2(r.left() + 3.0, y - 3.0), egui::pos2(r.left() + 6.0, y)], s);
    p.line_segment([egui::pos2(r.left() + 6.0, y), egui::pos2(r.left() + 3.0, y + 3.0)], s);
    p.line_segment([egui::pos2(r.left() + 8.0, y + 3.0), egui::pos2(r.right() - 3.0, y + 3.0)], s);
}

fn close_x(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.4_f32, c);
    let k = r.shrink(r.width() * 0.12);
    p.line_segment([k.left_top(), k.right_bottom()], s);
    p.line_segment([k.right_top(), k.left_bottom()], s);
}

fn section_title(ui: &mut egui::Ui, th: &Theme, text: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new(text).size(11.0).color(th.weak));
    });
    ui.add_space(2.0);
}

/// A sidebar entry: icon, label and an optional usage bar.
fn side_row(ui: &mut egui::Ui, th: &Theme, selected: bool, label: &str, bar: Option<f32>, icon: impl FnOnce(&egui::Painter, egui::Rect)) -> egui::Response {
    side_row_with(ui, th, selected, label, bar, 0.0, icon)
}

/// A sidebar row with `button` points kept free at its right end for a button.
fn side_row_with(ui: &mut egui::Ui, th: &Theme, selected: bool, label: &str, bar: Option<f32>, button: f32, icon: impl FnOnce(&egui::Painter, egui::Rect)) -> egui::Response {
    let h = if bar.is_some() { 38.0 } else { 30.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width() - 4.0, h), egui::Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 7.0, th.selection);
    } else if resp.hovered() {
        p.rect_filled(rect, 7.0, th.surface);
    }
    let cy = rect.top() + 15.0;
    icon(p, egui::Rect::from_center_size(egui::pos2(rect.left() + 18.0, cy), egui::vec2(16.0, 16.0)));
    let text_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 34.0, rect.top()), egui::pos2(rect.right() - 8.0 - button, rect.top() + 30.0));
    p.with_clip_rect(text_rect).text(
        egui::pos2(text_rect.left(), cy),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        if selected { th.strong() } else { th.text },
    );
    if let Some(f) = bar {
        let b = egui::Rect::from_min_size(egui::pos2(rect.left() + 34.0, rect.top() + 28.0), egui::vec2(rect.width() - 46.0, 4.0));
        p.rect_filled(b, 2.0, if selected { th.bg } else { th.surface });
        let fill = if f > 0.9 { th.danger } else { th.accent.gamma_multiply(0.85) };
        p.rect_filled(egui::Rect::from_min_size(b.min, egui::vec2(b.width() * f.clamp(0.0, 1.0), b.height())), 2.0, fill);
    }
    resp
}

/// Status-bar key hint: a keycap followed by a label, clickable.
fn key_hint_width(ui: &egui::Ui, key: &str, label: &str) -> f32 {
    let width = |text: &str| ui.fonts(|f| f.layout_no_wrap(text.to_string(), egui::FontId::proportional(11.5), egui::Color32::WHITE).size().x);
    width(key) + 12.0 + 6.0 + width(label) + 8.0
}

fn key_hint(ui: &mut egui::Ui, th: &Theme, key: &str, label: &str) -> egui::Response {
    let font = egui::FontId::proportional(11.5);
    let kw = ui.fonts(|f| f.layout_no_wrap(key.to_string(), font.clone(), th.weak).size().x);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(key_hint_width(ui, key, label), 22.0), egui::Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 6.0, th.surface);
    }
    let cap = egui::Rect::from_min_size(egui::pos2(rect.left() + 2.0, rect.center().y - 8.0), egui::vec2(kw + 10.0, 16.0));
    p.rect(cap, 4.0, th.surface, egui::Stroke::new(1.0_f32, th.border), egui::StrokeKind::Inside);
    p.text(cap.center(), egui::Align2::CENTER_CENTER, key, font.clone(), th.weak);
    p.text(egui::pos2(cap.right() + 6.0, rect.center().y), egui::Align2::LEFT_CENTER, label, font, if resp.hovered() { th.text } else { th.weak });
    resp
}

/// Where a bookmark goes: its path, with `~` for the home folder (and a relative path
/// from there), as the folder fields fill it in.
fn bookmark_target(path: &str) -> String {
    let home = dirs::home_dir();
    crate::pathfield::resolve_typed(path, home.as_deref(), home.as_deref())
}

/// The local file or folder behind a location: the folder, or the archive file.
fn local_file_of(loc: &Location) -> Option<PathBuf> {
    match loc {
        Location::Local(p) => Some(p.clone()),
        Location::Archive { file, .. } => Some(file.clone()),
        Location::Remote { .. } => None,
    }
}

fn pause_icon(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let w = r.width() * 0.22;
    p.rect_filled(egui::Rect::from_min_size(egui::pos2(r.center().x - w * 1.4, r.top() + 1.0), egui::vec2(w, r.height() - 2.0)), 1.0, c);
    p.rect_filled(egui::Rect::from_min_size(egui::pos2(r.center().x + w * 0.4, r.top() + 1.0), egui::vec2(w, r.height() - 2.0)), 1.0, c);
}

fn play_icon(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let pts = vec![egui::pos2(r.left() + r.width() * 0.25, r.top()), egui::pos2(r.right() - r.width() * 0.1, r.center().y), egui::pos2(r.left() + r.width() * 0.25, r.bottom())];
    p.add(egui::Shape::convex_polygon(pts, c, egui::Stroke::NONE));
}

fn fmt_duration(secs: f32) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Response for the chevron part of a split button (so the popup can anchor to both parts).
fn chev_rect_response(ui: &egui::Ui, r: egui::Rect) -> egui::Response {
    ui.interact(r, egui::Id::new("terminal-chevron-anchor"), egui::Sense::hover())
}

#[cfg(test)]
mod bookmark_tests {
    use super::*;

    fn names(list: &[Bookmark]) -> Vec<String> {
        list.iter().map(|b| if b.folder { format!("[{}: {}]", b.name, names(&b.children).join(" ")) } else { b.name.clone() }).collect()
    }

    #[test]
    fn edit_bookmarks_and_folders() {
        let mut l = vec![Bookmark::new("a", "/a"), Bookmark::new("b", "/b"), Bookmark::new_folder("F"), Bookmark::new("c", "/c")];
        apply_bookmark_edit(&mut l, BmEdit::MoveTo((0, None), Some(2))); // a into F (F shifts to 1)
        assert_eq!(names(&l), ["b", "[F: a]", "c"]);
        apply_bookmark_edit(&mut l, BmEdit::MoveTo((2, None), Some(1))); // c into F
        assert_eq!(names(&l), ["b", "[F: a c]"]);
        apply_bookmark_edit(&mut l, BmEdit::Up((1, Some(1))));
        assert_eq!(names(&l), ["b", "[F: c a]"]);
        apply_bookmark_edit(&mut l, BmEdit::MoveTo((1, Some(0)), None)); // c out, right after F
        assert_eq!(names(&l), ["b", "[F: a]", "c"]);
        apply_bookmark_edit(&mut l, BmEdit::Up((0, None)));
        assert_eq!(names(&l), ["b", "[F: a]", "c"], "the first one cannot go up");
        apply_bookmark_edit(&mut l, BmEdit::Down((0, None)));
        assert_eq!(names(&l), ["[F: a]", "b", "c"]);
        // Removing a folder keeps its bookmarks in its place.
        apply_bookmark_edit(&mut l, BmEdit::Delete((0, None)));
        assert_eq!(names(&l), ["a", "b", "c"]);
        apply_bookmark_edit(&mut l, BmEdit::Delete((1, None)));
        assert_eq!(names(&l), ["a", "c"]);
    }
}

#[path = "settings_ui.rs"]
mod settings_ui;
use settings_ui::{card_frame, settings_section, toggle_row};

#[cfg(test)]
#[path = "screenshots.rs"]
mod screenshots;
