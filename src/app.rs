//! The main application: layout, keyboard handling, dialogs and background jobs.
use crate::config::{Bookmark, ConnectionProfile, Protocol, Settings};
use crate::files::archive::{self, ArchiveFormat};
use crate::files::ops::{self, Endpoint, Job};
use crate::files::remote::{self, RemoteFs, SharedRemote};
use crate::files::safefs::Conflict;
use crate::files::vfs::{Entry, Location};
use crate::i18n::{self, t};
use crate::preview::{self, Preview};
use crate::style::icons;
use crate::style::theme::{self, Theme};
use crate::terminal::view::TermSession;
use crate::terminal::{self, TerminalKind};
use crate::tools::backup::{self, BackupAction, BackupUi};
use crate::tools::diskusage::DuAction;
use crate::tools::rename::{self as ren, BatchRename, RenameAction};
use crate::ui::layout::{Layout, SplitDir};
use crate::ui::palette::{self, Act, PalItem, Palette};
use crate::ui::pane::{pane_ui, Cmd, ListCtx, Pane, PaneEvent, PaneUiCtx, Tab};
use crate::ui::shortcuts::{self, hint, Action, Chord};
use crate::ui::tabs::{Strip, TabEvent, TabInfo, Workspace};
use crate::util::{es, human_size, join_slash, Res};
use egui::{Key, Modifiers, RichText};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Build a `ListCtx` from disjoint fields so that panes can be borrowed mutably.
macro_rules! lc {
    ($s:expr) => {
        ListCtx { remotes: &$s.remotes, show_hidden: $s.cfg.show_hidden, dirs_first: $s.cfg.dirs_first, default_view: $s.cfg.default_view }
    };
}

/// Every window of every tab: the shown tab's and the parked ones in the strip.
macro_rules! all_panes {
    ($s:expr) => {
        $s.panes.iter_mut().chain($s.strip.tabs.iter_mut().flat_map(|w| w.panes.iter_mut()))
    };
}

enum Dialog {
    Transfer {
        src: Location,
        mv: bool,
        items: Vec<Entry>,
        dst: String,
        policy: Conflict,
    },
    /// `loc`: the folder it works in, and `view` the window it was asked from (kept, so
    /// that the dialog acts there even if the window shows another folder meanwhile).
    NewItem {
        name: String,
        file: bool,
        loc: Location,
        view: u64,
    },
    Rename {
        old: String,
        new: String,
        loc: Location,
        view: u64,
    },
    Delete {
        items: Vec<Entry>,
        trash: bool,
        loc: Location,
        view: u64,
    },
    Pack {
        items: Vec<Entry>,
        src_dir: PathBuf,
        name: String,
        dest_dir: String,
        format: ArchiveFormat,
        level: u32,
    },
    Unpack {
        archive: PathBuf,
        dest: String,
        selection: Vec<String>,
        strip: String,
        policy: Conflict,
    },
    OpenWith {
        path: PathBuf,
        program: String,
        always: bool,
    },
    /// Asks for an archive's password, tries it in the background, then goes on with `then`.
    ArchivePassword {
        file: PathBuf,
        password: String,
        wrong: bool,
        checking: Option<Receiver<bool>>,
        then: AfterPassword,
    },
    Connect {
        profile: ConnectionProfile,
        save: bool,
    },
    /// An SFTP server whose host key is not known yet: trust it, or do not connect.
    TrustHostKey {
        profile: ConnectionProfile,
        fingerprint: String,
    },
    Message {
        title: String,
        text: String,
    },
    /// Replace every setting with those read from `path`.
    ImportSettings {
        path: PathBuf,
        settings: Box<Settings>,
    },
    /// Closing terminals that are running programs (named in `programs`).
    CloseTerminals {
        target: CloseTarget,
        programs: Vec<String>,
    },
    /// Opening a terminal in each of several folders.
    OpenTerminals {
        dirs: Vec<PathBuf>,
    },
    /// Opening a link clicked in terminal `term` (`TermSession::id`); `stop_asking`
    /// turns the question off for that terminal.
    OpenLink {
        url: String,
        term: egui::Id,
        stop_asking: bool,
    },
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
    Uploaded(u64, crate::files::tempcopy::Stamp),
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
    drives: Vec<crate::files::drives::Drive>,
    /// Keeps `drives` up to date off the UI thread.
    drive_watch: crate::files::drives::Watcher,
    /// Notices shown folders that changed on disk (Settings → Refresh folders automatically).
    dir_watch: crate::files::dirwatch::DirWatch,
    /// Thumbnails for the icon view, made on worker threads.
    thumbs: crate::preview::thumbs::Thumbs,
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
    update_check: Option<(std::sync::mpsc::Receiver<crate::util::Res<crate::system::update::Outcome>>, bool)>,
    /// A newer release found, with its notes ready to show.
    update_found: Option<(crate::system::update::Release, Vec<crate::preview::markdown::MdBlock>)>,
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
    /// The narrowest the sidebar can be without its content spilling past its edge
    /// (the app menu row): learned from the frames where it did.
    sidebar_min: f32,
    /// What `sidebar_min` was learned for (font size, UI scale, language): it is learned
    /// again when they change.
    sidebar_min_for: (f32, f32, String),
    /// The main window's corner radius in this frame (Linux; 0 = square).
    window_radius: f32,
    /// The folder of the file window the user was last in (Settings → Bookmarks adds it).
    last_files_loc: Option<Location>,
    /// Settings → Bookmarks: a group was just added; put its name in view, ready to type.
    bm_focus_last: bool,
    /// Settings: a new name being typed for one of the user's themes, until it is one of
    /// its own (list, index, text).
    theme_name_draft: Option<(themes::ThemeList, usize, String)>,
    /// Settings: one of the user's themes whose Delete asks to be confirmed.
    theme_delete: Option<(themes::ThemeList, usize)>,
    /// What Settings → Quick filters has open.
    qf: quickfilters::QfUi,
    /// Settings → Terminal → Links: a link typed to see where it would open.
    link_try: String,
    /// The app's windows that are open, back to front (see `track_windows`), and whether
    /// the keyboard is theirs rather than the files' and terminals'.
    windows: Vec<&'static str>,
    keys_to_window: bool,
    /// The active window and tab when the keyboard last went to the app's windows.
    keys_at: (usize, u64),
    /// Frames drawn so far (up to `u32::MAX`).
    frames: u32,
    /// Startup tokens for the window (see `activation`): this launch's, then those of
    /// later launches that handed their folder over. Used once the window is drawn.
    activation: Vec<String>,
    /// X11: the window's own cursor is set (see `own_cursor`); 0 not yet, 1 hidden for a
    /// frame, 2 done.
    cursor_owned: u8,
    /// Exact destination of a drag & drop awaiting confirmation.
    dialog_dst: Option<Location>,
    /// The suggestions and folder tree of a dialog's folder field (Copy / Move to,
    /// Compress's Save to, Extract to).
    dialog_path: crate::ui::pathfield::PathField,
    /// The folder tree shown in Copy / Move when the tab has one window.
    dialog_tree: Option<crate::ui::foldertree::FolderTree>,
    /// Copy / Move in a split tab: "Another folder" is chosen (rather than a window).
    /// None until the dialog's first frame decides it.
    dialog_custom: Option<bool>,
    /// Copy / Move's choices, and when they were read (see `dst_choices`).
    dialog_choices: Option<(Instant, Vec<DstChoice>, Vec<(usize, egui::Rect, bool)>)>,
    logo_large: Option<egui::TextureHandle>,
    icon_key: String,
    icon_previews: HashMap<String, icons::IconSet>,
    icon_themes: Option<(Vec<String>, Option<String>)>,
    egui_ctx: egui::Context,
    closed_tabs: Vec<ClosedTab>,
    /// AI coding agents found on this computer (`None`: not looked for yet), and
    /// the search under way.
    agents: Option<Vec<crate::system::agents::Found>>,
    agents_rx: Option<Receiver<Vec<crate::system::agents::Found>>>,
    /// Live filter while typing in the search box: (pane, tab uid, filter before, the
    /// cursor's entry before).
    pal_filter: Option<(usize, u64, FilterState, Option<String>)>,
    /// Typing in a file window: a small search box that filters it as you type.
    type_search: Option<TypeSearch>,
    settings_page: SettingsPage,
    /// File types added on the Default apps page that have no app yet.
    default_app_types: Vec<String>,
    /// Text in that page's "add a file type" field.
    default_app_new: String,
    /// Suggestions and the folder tree of the bookmarks' Location fields.
    bookmark_paths: crate::ui::pathfield::PathField,
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
    fonts_applied: crate::style::fonts::FontPrefs,
    chrome: crate::ui::chrome::Chrome,
    /// Width of the toolbar's right-hand group last frame (keeps the search box clear of it).
    toolbar_right_w: f32,
    /// Listens for later launches (single instance); `None` when another Matrix Finder does.
    instance: Option<crate::system::instance::Server>,
    /// The settings file: saves merge with what another Matrix Finder saved meanwhile.
    store: crate::config::Store,
    /// Files from servers and archives opened in other apps through a temporary copy.
    temp_copies: crate::files::tempcopy::Copies,
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
    pub fn new(cc: &eframe::CreationContext, chrome: crate::ui::chrome::Chrome, open: Option<String>) -> Self {
        let (mut cfg, mut store) = crate::config::Store::open();
        // Passwords that older versions saved when asked: kept in memory from now on, and
        // the settings saved without them.
        if crate::files::remote::take_saved_passwords(&mut cfg.connections) {
            if let Ok(Some(merged)) = store.save(&cfg) {
                cfg = merged;
            }
        }
        i18n::set_language(&cfg.language);
        // Previews' temporary folders left by a Matrix Finder that crashed.
        crate::util::clean_stale_temp_dirs(&[crate::util::TEMP_PREFIX]);
        theme::set_custom(&cfg.custom_themes, &cfg.custom_terminal_themes);
        let theme = theme::by_name(&cfg.theme);
        if cfg.theme != theme::SYSTEM {
            cfg.theme = theme.name.to_string();
        }
        shortcuts::install(&cfg.shortcuts);
        // A font that is no longer installed leaves the bundled default in its place.
        let _ = crate::style::fonts::apply(&cc.egui_ctx, &crate::style::fonts::FontPrefs::of(&cfg));
        theme::apply(&cc.egui_ctx, theme);
        cc.egui_ctx.set_zoom_factor(cfg.ui_scale.clamp(0.6, 2.5));
        // Zooming by keys is the app's own (saved, and rebindable): egui's would not be
        // saved, and its Ctrl+− would reach the terminals too.
        cc.egui_ctx.options_mut(|o| {
            o.zoom_with_keyboard = false;
            // A field gives up the keyboard when the mouse is pressed elsewhere (egui 0.32's
            // way): a click on a suggestion is taken on the press, and the field keeps it.
            o.input_options.surrender_focus_on = egui::SurrenderFocusOn::Presses;
            // No Ctrl+Q that closes the window at once: terminal programs use it, and
            // quitting asks first about running programs and jobs (the app's own Quit key).
            o.quit_shortcuts.clear();
        });
        let to_locs = |v: &Vec<String>| -> Vec<Location> { v.iter().map(PathBuf::from).filter(|p| p.is_dir()).map(Location::Local).collect() };
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
            drives: crate::files::drives::list(),
            drive_watch: crate::files::drives::Watcher::spawn(&cc.egui_ctx),
            dir_watch: crate::files::dirwatch::DirWatch::spawn(Some(cc.egui_ctx.clone())),
            thumbs: crate::preview::thumbs::Thumbs::spawn(cc.egui_ctx.clone()),
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
            sidebar_min: 0.0,
            sidebar_min_for: (0.0, 0.0, String::new()),
            window_radius: 0.0,
            last_files_loc: None,
            bm_focus_last: false,
            theme_name_draft: None,
            theme_delete: None,
            qf: Default::default(),
            link_try: String::new(),
            activation: crate::system::activation::startup_token().into_iter().collect(),
            windows: Vec::new(),
            keys_to_window: false,
            keys_at: (usize::MAX, 0),
            frames: 0,
            cursor_owned: 0,
            dialog_dst: None,
            dialog_path: Default::default(),
            dialog_tree: None,
            dialog_custom: None,
            dialog_choices: None,
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
            fonts_applied: crate::style::fonts::FontPrefs::of(&cfg),
            chrome,
            toolbar_right_w: 300.0,
            // Not in tests: they must not take over the running Matrix Finder's instance file.
            instance: if cfg!(test) { None } else { crate::system::instance::Server::start(cc.egui_ctx.clone()) },
            cfg,
            store,
            temp_copies: Default::default(),
            copy_prompts: Vec::new(),
        };
        if let Some(note) = app.store.note.take() {
            app.tell_settings_note(note);
        }
        // Not in tests: the test binary is never the install script's copy anyway.
        if !cfg!(test) {
            if let Some(copy) = crate::system::copies::script_over_package() {
                app.tell_script_over_package(&copy);
            }
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
        if app.cfg.check_updates && !cfg!(test) && chrono::Utc::now().timestamp() - app.cfg.update_checked >= crate::system::update::AUTO_EVERY_SECS {
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
            let _ = tx.send(crate::system::update::check());
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
        use crate::system::update::{version_text, Outcome};
        match result {
            Ok(Outcome::Newer(r)) => {
                self.cfg.update_checked = chrono::Utc::now().timestamp();
                if manual || r.tag != self.cfg.update_skip {
                    let notes = crate::preview::markdown::parse_markdown(&r.notes, self.theme.syntax);
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
                text.push_str(&format!(
                    "\n\n{}",
                    if from_backup {
                        t("The previous version of the settings (the backup) was loaded instead.")
                    } else {
                        t("Matrix Finder starts with the default settings.")
                    }
                ));
                if read_only {
                    text.push_str(&format!("\n\n{}", t("Changes to the settings are not saved until that file can be read.")));
                }
                (t("The settings file could not be read"), text)
            }
            LoadNote::FromBackup(bak) => (
                t("Settings restored from the backup"),
                format!("{}\n{}", t("The settings file was missing, so the previous version was loaded:"), bak.display()),
            ),
        };
        self.show_dialog(Dialog::Message { title, text }, false);
    }

    /// Linux: this is the install script's copy, and a .deb or .rpm package installed
    /// another one that never starts while this one is there (`system::copies`).
    fn tell_script_over_package(&mut self, copy: &crate::system::copies::ScriptOverPackage) {
        let package = match copy.package_version() {
            Some(v) => format!("{} ({v})", copy.package.display()),
            None => copy.package.display().to_string(),
        };
        let text = format!(
            "{}\n{}\n\n{}\n{package}\n\n{}\n\n{}\n\n{}",
            t("This window is the copy installed with the install script:"),
            copy.running.display(),
            t("A .deb or .rpm package installed another one:"),
            t("The script's copy comes first on PATH and in the applications menu, so the package's copy, and its updates, do not start. To use the package, remove the script's copy:"),
            copy.uninstall_command(),
            t("Settings are shared and kept."),
        );
        self.show_dialog(Dialog::Message { title: t("Two copies of Matrix Finder are installed"), text }, false);
    }

    /// Every tab with its windows and layout. Windows that cannot be saved (remote
    /// folders) are saved as the home folder so a split layout stays whole.
    fn save_session(&self) -> crate::config::SavedPane {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let window = |p: &Pane| -> (Option<PathBuf>, bool, Option<crate::config::ViewMode>) {
            let t = p.tab();
            let path = match &t.term {
                Some(term) => Some(term.cwd.clone()),
                // Settings, Transfers and tool windows are not reopened as a folder.
                None if t.settings || t.transfers || t.is_tool() => None,
                None => t.loc.local_path().map(|p| p.to_path_buf()),
            };
            (path, t.is_terminal(), t.view_mode)
        };
        let mut tabs = Vec::new();
        let mut active = 0;
        for (i, w) in self.strip.tabs.iter().enumerate() {
            let (panes, layout, focus) = if w.uid == self.live { (&self.panes[..], &self.layout, self.active) } else { (&w.panes[..], &w.layout, w.active) };
            if panes.is_empty() {
                continue;
            }
            let wins: Vec<(Option<PathBuf>, bool, Option<crate::config::ViewMode>)> = panes.iter().map(window).collect();
            let (path, terminal, view) = match &wins[focus.min(wins.len() - 1)] {
                (Some(p), term, view) => (p.clone(), *term, *view),
                // A single remote window is not saved (as before).
                (None, _, _) if wins.len() == 1 => continue,
                (None, _, _) => (home.clone(), false, None),
            };
            if i == self.strip.active {
                active = tabs.len();
            }
            let windows = if wins.len() > 1 {
                wins.iter()
                    .map(|(p, term, view)| crate::config::SavedWindow {
                        path: p.clone().unwrap_or_else(|| home.clone()).to_string_lossy().to_string(),
                        terminal: *term && p.is_some(),
                        view: *view,
                    })
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
                view: view.filter(|_| wins.len() == 1),
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
        // Shown windows reload now; hidden tabs forget cached archive listings and
        // reload when shown (`sync_tab`).
        let c = lc!(self);
        for p in &mut self.panes {
            p.tab_mut().invalidate_archive_cache();
            if p.tab().is_files() {
                p.tab_mut().reload(&c);
            }
        }
        for p in self.strip.tabs.iter_mut().flat_map(|w| w.panes.iter_mut()) {
            p.tab_mut().invalidate_archive_cache();
            if p.tab().is_files() {
                p.tab_mut().stale = true;
            }
        }
        self.quick = None;
    }

    fn remote_label(&self, id: u64) -> String {
        self.remotes.get(&id).map(|r| r.0.clone()).unwrap_or_default()
    }

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
                // The whole label: "sftp://db:2222/x" is not the connection "sftp://db:22".
                if let Some(rest) = text.strip_prefix(label.as_str()).filter(|r| r.is_empty() || r.starts_with('/')) {
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
        let got: Vec<(String, Option<String>)> = rx.try_iter().collect();
        if got.is_empty() {
            return;
        }
        for (open, token) in got {
            self.open_from_launch(&open, true);
            // The later launch's startup token raises this window (Wayland), in the
            // next frame.
            self.activation.extend(token);
        }
        if !self.activation.is_empty() {
            ctx.request_repaint();
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn rebuild_views(&mut self) {
        let c = lc!(self);
        let quick = &self.cfg.quick_filters;
        for p in all_panes!(self) {
            let tab = p.tab_mut();
            // An edited category applies at once; a deleted one turns off.
            if let Some(names) = tab.quick.as_ref().map(|q| q.names.clone()) {
                tab.quick = crate::ui::pane::QuickActive::from_names(&names, quick);
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
            Some(f) => crate::ui::pane::QuickActive::toggle(self.panes[a].tab().quick.as_ref(), &f.name, &self.cfg.quick_filters),
            None => None,
        };
        let c = lc!(self);
        self.panes[a].tab_mut().set_quick(q, &c);
    }

    fn restore_session(&mut self, s: crate::config::SavedPane) {
        let mut ids = HashMap::new();
        let mut strip = Strip { tabs: Vec::new(), active: 0, groups: Vec::new() };
        for g in &s.groups {
            let id = crate::ui::tabs::next_group_id();
            ids.insert(g.id, id);
            strip.groups.push(crate::ui::tabs::TabGroup { id, ..g.clone() });
        }
        let mut active_uid = None;
        for (i, st) in s.tabs.iter().enumerate() {
            let windows: Vec<crate::config::SavedWindow> = if st.windows.is_empty() {
                vec![crate::config::SavedWindow { path: st.path.clone(), terminal: st.terminal, view: st.view }]
            } else {
                st.windows.clone()
            };
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
                    None => {
                        let mut p = self.files_pane(Location::Local(dir));
                        p.tab_mut().view_mode = win.view;
                        p.tab_mut().rebuild_view(&lc!(self));
                        panes.push(p);
                    }
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

    /// Quitting while jobs are running, or programs in terminals, asks first; run once
    /// per frame, before input is used.
    fn quit_step(&mut self, ctx: &egui::Context) {
        let close = ctx.input(|i| i.viewport().close_requested());
        let asked = crate::ui::chrome::take_native_quit_request();
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
            ctx.input_mut(|i| {
                i.events.retain(|e| {
                    !matches!(
                        e,
                        egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Ime(_)
                    )
                })
            });
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        crate::ui::chrome::set_quit_guard(busy && self.quit != Quit::Go);
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
        let open = |name: &str| crate::style::sysicons::IconTheme::open(name).map(Arc::new);
        let breeze = || if self.theme.dark { open("breeze-dark").or_else(|| open("breeze")) } else { open("breeze") };
        match choice {
            "matrix" => IconSet { pack: P::Matrix, system: None },
            "adwaita" => IconSet { pack: P::Adwaita, system: open("Adwaita") },
            "breeze" => IconSet { pack: P::Breeze, system: breeze() },
            "macos" => IconSet { pack: P::MacOS, system: None },
            "windows" => IconSet { pack: P::Windows, system: None },
            "material" => IconSet { pack: P::Material, system: None },
            "fluent" => IconSet { pack: P::Fluent, system: None },
            "system" => {
                if cfg!(target_os = "macos") {
                    IconSet { pack: P::MacOS, system: None }
                } else if cfg!(windows) {
                    IconSet { pack: P::Windows, system: None }
                } else {
                    let name = crate::style::sysicons::current_theme_name().unwrap_or_else(|| "Adwaita".into());
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
        theme::set_custom(&self.cfg.custom_themes, &self.cfg.custom_terminal_themes);
        self.theme = theme::by_name(&self.cfg.theme);
        theme::apply(ctx, self.theme);
        ctx.set_zoom_factor(self.cfg.ui_scale.clamp(0.6, 2.5));
        self.apply_fonts(ctx);
        shortcuts::install(&self.cfg.shortcuts);
        self.rebuild_views();
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
        let bar = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), crate::ui::tabs::TAB_H));
        let profiles: Vec<String> = self.cfg.terminal_profiles.iter().map(|p| p.name.clone()).collect();
        let infos = self.tab_infos();
        let dragged_window = egui::DragAndDrop::payload::<crate::ui::tabs::WinDrag>(ui.ctx()).map(|d| (d.uid, self.panes.len()));
        let mut tab_events = Vec::new();
        {
            let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::top_down(egui::Align::Min)));
            let bctx = crate::ui::tabs::BarCtx { theme: self.theme, terminal_profiles: &profiles, can_reopen: !self.closed_tabs.is_empty(), dragged_window };
            crate::ui::tabs::tab_bar(&mut bar_ui, &mut self.strip, &infos, &bctx, &mut tab_events);
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
                focused: side == self.active && self.dialog.is_none() && !self.palette.open && !self.keys_to_window,
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
                thumbs: self.cfg.thumbnails.then_some(&self.thumbs),
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
                let text = crate::ui::pathfield::resolve_typed(&text, dirs::home_dir().as_deref(), self.panes[side].tab().loc.local_path());
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
            PaneEvent::Find(f) => {
                self.active = side;
                if let Err(e) = self.panes[side].tab_mut().set_find(f, &lc!(self)) {
                    self.toast(e, true);
                }
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
}

impl eframe::App for App {
    /// Paint the window background in the theme colour so live resizing never
    /// flashes a dark strip before the next frame.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if self.window_radius > 0.0 {
            return [0.0; 4];
        }
        egui::Rgba::from(self.theme.panel).to_array()
    }

    /// Work that goes on while the window is hidden or minimized too (eframe then runs no
    /// `ui`): terminals, jobs, connections, a launch handing over a folder, saving, and a
    /// quit asked from the Dock, which may need to show the window to ask first.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        crate::ui::chrome::install_quit_guard(frame);
        self.sync_tab();
        self.active = self.active.min(self.panes.len() - 1);
        self.quit_step(ctx);
        self.poll_terminals(ctx);
        self.poll_agents();
        self.poll_instance(ctx);
        self.poll_jobs(ctx);
        self.poll_connect();
        self.poll_drives(ctx);
        self.poll_update(ctx);
        self.poll_temp_copies(ctx);
        self.save_cfg_pending(ctx);
        self.poll_dir_watch(ctx);
        let mut busy = self.connecting.is_some();
        for side in 0..self.panes.len() {
            let tab = self.panes[side].tab_mut();
            tab.poll(&lc!(self));
            busy |= self.panes[side].tab().is_loading() || self.panes[side].tab().is_scanning() || self.panes[side].tab().dir_sizes.is_some();
        }
        if busy {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.chrome.setup_native(frame);
        // Startup tokens, once the window has been drawn: from the third frame on (a pass
        // may be redone within one frame, so frames are counted here, not passes). An
        // idle app draws no next frame by itself: ask for it while one waits.
        self.frames = self.frames.saturating_add(1);
        if self.frames == 2 {
            self.fit_to_screen(ctx);
        }
        if !self.activation.is_empty() {
            if self.frames > 2 {
                for token in std::mem::take(&mut self.activation) {
                    crate::system::activation::activate(frame, &token);
                }
            } else {
                ctx.request_repaint();
            }
        }
        self.max_texture = ctx.input(|i| i.max_texture_side).max(1024);
        // Follow the OS (theme "System") and keep the icon set in sync.
        let wanted = self.resolve_theme(ctx);
        if !std::ptr::eq(wanted, self.theme) {
            self.theme = wanted;
            theme::apply(ctx, wanted);
            self.quick = None;
        }
        let icon_key = format!("{}|{}|{}", self.cfg.icon_pack, self.theme.name, self.theme.icons.key());
        if icon_key != self.icon_key {
            icons::set_icons(self.make_icon_set(&self.cfg.icon_pack.clone()));
            self.icon_key = icon_key;
        }
        self.shift_down = ctx.input(|i| i.modifiers.shift);
        crate::style::fonts::set_reading_serif(self.cfg.reading_font == crate::config::ReadingFont::Serif);
        self.drag_target = None;
        self.sync_tab();
        self.active = self.active.min(self.panes.len() - 1);
        if self.active != self.last_active {
            self.prev_active = self.last_active;
            self.last_active = self.active;
        }

        self.track_windows(ctx);
        if let Some(tab) = self.panes.get(self.active).map(|p| p.tab()).filter(|t| t.is_files()) {
            if self.last_files_loc.as_ref() != Some(&tab.loc) {
                self.last_files_loc = Some(tab.loc.clone());
            }
        }
        self.handle_keys(ctx);
        self.handle_dropped_files(ctx);

        // Window title follows the active folder.
        let title = match &self.panes[self.active].tab().term {
            Some(t) => format!("Matrix Finder — {}", t.label()),
            None => format!("Matrix Finder — {}", self.panes[self.active].tab().loc.display(|id| self.remote_label(id))),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        // Linux: the window's background is one rounded shape, and the panels on it are
        // see-through, so its corners show the desktop (see `clear_color`).
        self.window_radius = self.chrome.window_radius(ctx);
        let panel = if self.window_radius > 0.0 {
            ctx.layer_painter(egui::LayerId::background()).rect_filled(ctx.viewport_rect(), self.window_radius, self.theme.panel);
            egui::Color32::TRANSPARENT
        } else {
            self.theme.panel
        };
        self.chrome.resize_edges(ctx);
        // Sidebar first so it spans the full window height (Finder / Nautilus layout).
        if self.cfg.show_sidebar {
            let (f, s, l) = &self.sidebar_min_for;
            if (*f, *s, l.as_str()) != (self.cfg.ui_font_size, self.cfg.ui_scale, self.cfg.language.as_str()) {
                self.sidebar_min_for = (self.cfg.ui_font_size, self.cfg.ui_scale, self.cfg.language.clone());
                self.sidebar_min = 0.0;
            }
            egui::Panel::left("sidebar")
                .resizable(false)
                // In a narrow window the sidebar gives up space (never more than ~26 %).
                // Never narrower than its content, which would spill past the panel's
                // edge and leave an unpainted band beside it.
                .exact_size(self.cfg.sidebar_width.min((ctx.content_rect().width() * 0.26).max(160.0)).max(self.sidebar_min).round())
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(panel).inner_margin(egui::Margin { left: 8, right: 8, top: self.chrome.top_margin(), bottom: 6 }))
                .show_inside(ui, |ui| {
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
                            self.cfg.sidebar_width = (p.x - ctx.content_rect().left()).clamp(160.0, 360.0);
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
                    // (`max_rect` grows with the content, so compare with the room it was given.)
                    let used = ui.min_rect();
                    if used.right() > inner.right() + 0.5 {
                        self.sidebar_min = (used.width() + 16.0).ceil();
                        ui.ctx().request_repaint();
                    }
                });
        }
        egui::Panel::top("topbar")
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(panel).inner_margin(egui::Margin {
                left: if self.cfg.show_sidebar { 4 } else { 10 },
                right: 10,
                top: self.chrome.top_margin(),
                bottom: 4,
            }))
            .show_inside(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(panel).inner_margin(egui::Margin { left: 6, right: 10, top: 2, bottom: 4 }))
            .show_inside(ui, |ui| self.status_bar(ui));
        // One Quick View for all panes, docked at the right under the top bar.
        // It is hidden while a terminal tab is active (the setting is kept).
        let shown = self.panes[self.active.min(self.panes.len() - 1)].tab();
        let show_quick = self.cfg.quick_view && shown.is_files();
        if show_quick {
            // Width is a share of the window (1/3 by default) and only changes
            // when the user drags the gap on its left edge.
            let screen = ctx.content_rect();
            let w = (screen.width() * self.cfg.quick_view_frac).clamp(240.0, (screen.width() * 0.7).max(240.0)).round();
            egui::Panel::right("quickview")
                .resizable(false)
                .exact_size(w)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(panel).inner_margin(egui::Margin { left: 10, right: 10, top: 4, bottom: 2 }))
                .show_inside(ui, |ui| {
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
            .show_inside(ui, |ui| self.panes_area(ui));

        // Viewer window (Shift+F3)
        if self.viewer.is_some() {
            let mut open = true;
            let name = self.viewer.as_ref().map(|v| v.name.clone()).unwrap_or_default();
            let chrome = self.chrome.clone();
            chrome.dialog_window(ctx, &name, egui::Id::new("viewer")).open(&mut open).default_size([820.0, 600.0]).show(ctx, |ui| {
                if chrome.dialog_title(ui, self.theme, &name) {
                    self.viewer = None;
                }
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
            let action = self.backup.ui(ctx, &mut self.cfg.backups, cur.as_deref(), &self.drives, self.cfg.show_hidden, self.theme, &self.chrome, &t);
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
        self.own_cursor(ctx);
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
        for r in self.temp_copies.finish(&crate::files::tempcopy::recovered_dir()) {
            match r {
                Ok(p) => eprintln!("matrix_finder: kept an edited copy at {}", p.display()),
                Err(e) => eprintln!("matrix_finder: could not move an edited copy: {e}"),
            }
        }
    }
}

/// A window's filter as it was (its text and what it does), to be put back.
type FilterState = (String, Result<crate::ui::filter::Filter, String>);

/// The search box opened by typing in a file window.
struct TypeSearch {
    side: usize,
    uid: u64,
    loc: Location,
    /// The window's filter before, put back by Esc.
    before: FilterState,
    text: String,
}

/// A window of the shown tab that Copy / Move can send to.
#[derive(Clone)]
struct DstChoice {
    /// The window's index in `panes`, and its number as shown (1 to 4).
    side: usize,
    number: usize,
    /// The folder as the "To" field takes it, and its last part.
    text: String,
    name: String,
    terminal: bool,
}

/// The egui id of one of the app's windows, by its name in `App::windows`.
fn window_id(name: &str) -> egui::Id {
    egui::Id::new(if name == "reset" { "settings-reset" } else { name })
}

/// The last part of a typed destination, for the dialog's button (cut past 18 characters,
/// so the button stays short). None
/// when nothing is typed.
fn place_name(dst: &str) -> Option<String> {
    let s = dst.trim();
    if s.is_empty() {
        return None;
    }
    let name = last_part(s);
    Some(if name.chars().count() > 18 { format!("{}…", name.chars().take(17).collect::<String>()) } else { name.to_string() })
}

/// The last part of an address, to name it: the folder, or for a server's top folder
/// the host (not the port: `sftp://me@db:22/` is "db").
fn last_part(s: &str) -> String {
    let s = s.trim();
    if let Some((_, rest)) = s.split_once("://") {
        let (auth, path) = rest.split_once('/').unwrap_or((rest, ""));
        if let Some(n) = path.split('/').rfind(|p| !p.is_empty()) {
            return n.to_string();
        }
        let host = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
        let host = match host.strip_prefix('[') {
            Some(v6) => v6.split(']').next().unwrap_or(v6),
            // "db:22" → "db"; "::1:22" (IPv6 written without brackets) → "::1".
            None => match host.rsplit_once(':') {
                Some((h, port)) if !h.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
                _ => host,
            },
        };
        return host.to_string();
    }
    s.trim_end_matches(['/', '\\']).rsplit(['/', '\\', ':']).find(|p| !p.is_empty()).unwrap_or(s).to_string()
}

/// “name” for one item, “3 items” for several.
fn what(items: &[Entry]) -> String {
    match items {
        [one] => format!("“{}”", one.name),
        _ => format!("{} {}", items.len(), t("items")),
    }
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
fn side_row_with(
    ui: &mut egui::Ui,
    th: &Theme,
    selected: bool,
    label: &str,
    bar: Option<f32>,
    button: f32,
    icon: impl FnOnce(&egui::Painter, egui::Rect),
) -> egui::Response {
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

/// Where a bookmark goes: its path, with `~` for the home folder (and a relative path
/// from there), as the folder fields fill it in.
fn bookmark_target(path: &str) -> String {
    let home = dirs::home_dir();
    crate::ui::pathfield::resolve_typed(path, home.as_deref(), home.as_deref())
}

/// The local file or folder behind a location: the folder, or the archive file.
fn local_file_of(loc: &Location) -> Option<PathBuf> {
    match loc {
        Location::Local(p) => Some(p.clone()),
        Location::Archive { file, .. } => Some(file.clone()),
        Location::Remote { .. } => None,
    }
}

mod bars;
mod bookmarks;
mod commands;
mod connect;
mod dialogs;
mod jobs;
mod keys;
mod opening;
mod quickfilters;
mod settings_ui;
mod terminals;
mod themes;
mod windows;
use settings_ui::{card_frame, settings_section, toggle_row};

#[cfg(test)]
mod gui_tests;
