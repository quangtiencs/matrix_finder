//! Persistent settings (JSON in the platform config directory).
use crate::files::archive::ArchiveFormat;
use crate::terminal::{TerminalKind, TerminalProfile};
use crate::util::{es, Res};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A bookmark, or a bookmark folder holding bookmarks (one level deep).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Bookmark {
    pub name: String,
    /// Local path, or `ftp://…` / `sftp://…` style URL of a saved connection.
    /// Empty for a folder.
    pub path: String,
    pub icon: BookmarkIcon,
    /// A bookmark folder: holds `children` instead of pointing at a location.
    pub folder: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Bookmark>,
    /// A folder shown closed in the sidebar.
    pub collapsed: bool,
}

impl Bookmark {
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Bookmark { name: name.into(), path: path.into(), ..Default::default() }
    }

    pub fn new_folder(name: impl Into<String>) -> Self {
        Bookmark { name: name.into(), folder: true, ..Default::default() }
    }

    /// The bookmarks themselves: top-level ones and those inside folders, in order.
    pub fn leaves(list: &[Bookmark]) -> impl Iterator<Item = &Bookmark> {
        list.iter().flat_map(|b| if b.folder { b.children.iter().collect::<Vec<_>>() } else { vec![b] })
    }

    /// Remove the bookmark of `path` wherever it is; true if one was removed.
    pub fn remove_path(list: &mut Vec<Bookmark>, path: &str) -> bool {
        if let Some(i) = list.iter().position(|b| !b.folder && b.path == path) {
            list.remove(i);
            return true;
        }
        list.iter_mut().filter(|b| b.folder).any(|f| Self::remove_path(&mut f.children, path))
    }
}

/// How a bookmark is drawn. `Auto` picks the system folder's own icon (Desktop,
/// Downloads, …) unless plain folder icons are chosen in Settings.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum BookmarkIcon {
    #[default]
    Auto,
    Folder,
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Movies,
    Star,
    Code,
    Network,
}

impl BookmarkIcon {
    pub const CHOICES: [BookmarkIcon; 12] = [
        BookmarkIcon::Auto,
        BookmarkIcon::Folder,
        BookmarkIcon::Home,
        BookmarkIcon::Desktop,
        BookmarkIcon::Documents,
        BookmarkIcon::Downloads,
        BookmarkIcon::Music,
        BookmarkIcon::Pictures,
        BookmarkIcon::Movies,
        BookmarkIcon::Star,
        BookmarkIcon::Code,
        BookmarkIcon::Network,
    ];

    /// English label (translated by the caller).
    pub fn label(self) -> &'static str {
        match self {
            BookmarkIcon::Auto => "Automatic",
            BookmarkIcon::Folder => "Folder",
            BookmarkIcon::Home => "Home",
            BookmarkIcon::Desktop => "Desktop",
            BookmarkIcon::Documents => "Documents",
            BookmarkIcon::Downloads => "Downloads",
            BookmarkIcon::Music => "Music",
            BookmarkIcon::Pictures => "Pictures",
            BookmarkIcon::Movies => "Movies",
            BookmarkIcon::Star => "Star",
            BookmarkIcon::Code => "Code",
            BookmarkIcon::Network => "Network",
        }
    }

    /// The icon actually drawn for a bookmark of `path`.
    pub fn resolve(self, path: &str, system_icons: bool) -> BookmarkIcon {
        if self != BookmarkIcon::Auto {
            return self;
        }
        if !system_icons {
            return BookmarkIcon::Folder;
        }
        if path.contains("://") {
            return BookmarkIcon::Network;
        }
        system_folder(Path::new(path)).unwrap_or(BookmarkIcon::Folder)
    }
}

/// The user's standard folders (home, desktop, documents, …), in sidebar order.
pub fn system_folders() -> Vec<(BookmarkIcon, PathBuf)> {
    [
        (BookmarkIcon::Home, dirs::home_dir()),
        (BookmarkIcon::Desktop, dirs::desktop_dir()),
        (BookmarkIcon::Documents, dirs::document_dir()),
        (BookmarkIcon::Downloads, dirs::download_dir()),
        (BookmarkIcon::Music, dirs::audio_dir()),
        (BookmarkIcon::Pictures, dirs::picture_dir()),
        (BookmarkIcon::Movies, dirs::video_dir()),
    ]
    .into_iter()
    .filter_map(|(k, d)| Some((k, d?)))
    .collect()
}

/// Which standard folder `path` is, if any.
pub fn system_folder(path: &Path) -> Option<BookmarkIcon> {
    system_folders().into_iter().find(|(_, d)| d == path).map(|(k, _)| k)
}

/// Bookmarks for the standard folders that exist: Home, Desktop, Documents,
/// Downloads, Music, Pictures and Movies (named after the folder, so "Videos"
/// where the system calls it that).
pub fn system_bookmarks() -> Vec<Bookmark> {
    system_folders()
        .into_iter()
        .filter(|(_, d)| d.is_dir())
        .map(|(k, d)| {
            let name = if k == BookmarkIcon::Home {
                "Home".to_string()
            } else {
                d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| k.label().to_string())
            };
            Bookmark::new(name, d.to_string_lossy().to_string())
        })
        .collect()
}

/// A quick filter: a file category (Images, Music, …) and its extensions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct QuickFilter {
    pub name: String,
    /// Extensions without dots, separated by spaces or commas ("jpg png gif").
    pub exts: String,
}

impl QuickFilter {
    pub fn new(name: &str, exts: &str) -> Self {
        QuickFilter { name: name.into(), exts: exts.into() }
    }

    /// The extensions, lowercase, without dots or duplicates, in order.
    pub fn ext_list(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for e in self.exts.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
            let e = e.trim().trim_start_matches("*.").trim_start_matches('.').to_lowercase();
            if !e.is_empty() && !out.contains(&e) {
                out.push(e);
            }
        }
        out
    }
}

/// The built-in categories, with the common formats of each.
pub fn default_quick_filters() -> Vec<QuickFilter> {
    vec![
        QuickFilter::new("Images", "jpg jpeg png gif bmp webp tiff tif svg ico heic heif avif jxl psd xcf raw cr2 cr3 nef arw dng orf rw2"),
        QuickFilter::new("Music", "mp3 flac wav aac m4a ogg oga opus wma aiff aif alac ape mid midi"),
        QuickFilter::new("Videos", "mp4 mkv mov avi wmv flv webm m4v mpg mpeg 3gp ts mts m2ts vob ogv"),
        QuickFilter::new("Documents", "pdf doc docx odt rtf txt md markdown tex epub mobi pages djvu xps"),
        QuickFilter::new("Spreadsheets", "xls xlsx xlsm ods csv tsv numbers"),
        QuickFilter::new("Presentations", "ppt pptx odp key"),
        QuickFilter::new(
            "Development",
            "rs py js mjs cjs ts tsx jsx java kt kts scala go c h cpp cc cxx hpp hh cs swift m mm rb php pl lua r dart zig nim ex exs erl hs ml clj sh bash zsh fish ps1 bat sql html htm css scss sass less vue svelte json yaml yml toml ini xml gradle cmake mk proto",
        ),
        QuickFilter::new("Data", "parquet avro arrow feather orc jsonl ndjson ipynb sqlite sqlite3 db h5 hdf5 npy npz safetensors onnx rdata rda rds pkl pickle geojson wkt wkb"),
        QuickFilter::new("Archives", "zip rar 7z tar gz tgz bz2 tbz2 xz txz zst lz4 iso dmg cab deb rpm jar"),
    ]
}

/// `base`, or `base 2`, `base 3`… when a quick filter already has that name: the menu
/// tells its categories apart by name.
pub fn unused_quick_filter_name(all: &[QuickFilter], base: &str) -> String {
    let taken = |n: &str| all.iter().any(|q| q.name == n);
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap_or_default()
}

/// Where bookmarks are shown.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum BookmarkBar {
    /// In the sidebar, one per row.
    #[default]
    Vertical,
    /// A bar above the tabs, like a browser's bookmarks bar.
    Horizontal,
}

/// How a git folder's branch looks after its name.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum GitStyle {
    /// `(git) [branch]` in grey (or the state's colour).
    #[default]
    Text,
    /// `git · branch` in white on a grey (or the state's colour) label.
    Filled,
}

/// When the sidebar lists the drives.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum SidebarDrives {
    Always,
    /// Only with more than one drive (the system's own mounts aside).
    #[default]
    WhenSeveral,
    Never,
}

/// What starting Matrix Finder again does (e.g. `matrix_finder ~/src` in a terminal)
/// while it is already running.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum InstanceMode {
    /// Open the folder in a new tab of the running Matrix Finder.
    #[default]
    NewTab,
    /// Start a separate window (its own process).
    NewWindow,
}

/// What choosing a bookmark does while a terminal window is focused.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum BookmarkAction {
    /// Always show the folder in a file window.
    FilesOnly,
    /// In a terminal window, `cd` the shell to the folder instead.
    #[default]
    FilesAndTerminal,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Protocol {
    Ftp,
    Sftp,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Ftp => "FTP",
            Protocol::Sftp => "SFTP",
        }
    }
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Ftp => 21,
            Protocol::Sftp => 22,
        }
    }
    pub fn scheme(self) -> &'static str {
        match self {
            Protocol::Ftp => "ftp",
            Protocol::Sftp => "sftp",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConnectionProfile {
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Only stored when the user explicitly asks for it.
    #[serde(default)]
    pub password: String,
    /// SFTP private key file (optional).
    #[serde(default)]
    pub key_file: String,
    #[serde(default)]
    pub initial_path: String,
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            protocol: Protocol::Sftp,
            host: String::new(),
            port: 22,
            user: String::new(),
            password: String::new(),
            key_file: String::new(),
            initial_path: String::new(),
        }
    }
}

impl ConnectionProfile {
    pub fn url(&self) -> String {
        format!(
            "{}://{}@{}:{}{}",
            self.protocol.scheme(),
            self.user,
            self.host,
            self.port,
            if self.initial_path.starts_with('/') || self.initial_path.is_empty() { self.initial_path.clone() } else { format!("/{}", self.initial_path) }
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BackupProfile {
    pub name: String,
    pub sources: Vec<String>,
    /// Glob patterns, e.g. `**/node_modules/**`, `*.tmp`.
    pub excludes: Vec<String>,
    pub destination: String,
    pub format: ArchiveFormat,
    /// Compression level 1..=9 (mapped per format).
    pub level: u32,
    /// `{name}` and `{date}` placeholders.
    pub file_template: String,
    /// Keep only the newest N backups of this profile (0 = keep all).
    pub keep_last: usize,
    #[serde(default)]
    pub last_run: Option<String>,
}

impl Default for BackupProfile {
    fn default() -> Self {
        Self {
            name: "My backup".into(),
            sources: vec![],
            excludes: vec!["**/.git/**".into(), "**/node_modules/**".into(), "**/target/**".into(), "*.tmp".into()],
            destination: dirs::document_dir().or_else(dirs::home_dir).map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            format: ArchiveFormat::Zip,
            level: 6,
            file_template: "{name}-{date}".into(),
            keep_last: 0,
            last_run: None,
        }
    }
}

/// The font that Markdown and documents are read in.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReadingFont {
    /// Source Sans 3, like the interface.
    #[default]
    Sans,
    /// Source Serif 4.
    Serif,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: String,
    pub theme: String,
    pub ui_scale: f32,
    /// Interface font: a family name, `builtin:…` or empty for the default (see `fonts`).
    pub ui_font: String,
    /// Interface text size in points.
    pub ui_font_size: f32,
    pub terminal: TerminalKind,
    pub custom_terminal: String,
    pub show_hidden: bool,
    /// Git folders show their branch after the name in the file list: `name (git) [branch]`.
    pub git_branches: bool,
    /// With `(git)` before the branch.
    pub git_label: bool,
    pub git_style: GitStyle,
    /// The branch is coloured by whether everything is committed (runs `git status`);
    /// off, it is in the colour of the name and git never runs.
    pub git_status: bool,
    pub confirm_delete: bool,
    pub use_trash: bool,
    pub split_horizontal: bool,
    pub left_paths: Vec<String>,
    pub right_paths: Vec<String>,
    pub bookmarks: Vec<Bookmark>,
    pub bookmark_bar: BookmarkBar,
    /// What a bookmark does when a terminal window is focused.
    pub bookmark_action: BookmarkAction,
    /// Standard folders (Desktop, Downloads, …) get their own icons; off: plain folders.
    pub bookmark_system_icons: bool,
    /// Version of the default bookmarks this file has seen (adds new standard
    /// folders once to settings saved by older versions).
    #[serde(default)]
    pub bookmark_defaults: u32,
    /// The quick-filter categories offered in every file window.
    pub quick_filters: Vec<QuickFilter>,
    /// Version of the default quick filters this file has seen (adds new
    /// formats once to the built-in categories of older settings).
    #[serde(default)]
    pub quick_filter_defaults: u32,
    pub connections: Vec<ConnectionProfile>,
    pub backups: Vec<BackupProfile>,
    pub quick_view: bool,
    /// Quick View width as a share of the window width.
    pub quick_view_frac: f32,
    pub sidebar_width: f32,
    /// The window's size when Matrix Finder starts, in logical pixels (see `WINDOW_MIN`).
    pub window_width: f32,
    pub window_height: f32,
    pub dirs_first: bool,
    pub show_sidebar: bool,
    pub show_key_hints: bool,
    pub max_parallel: usize,
    pub confirm_dnd: bool,
    /// "theme", "system", "matrix", "adwaita", "breeze", "macos", "windows" or "icons:<theme>".
    pub icon_pack: String,
    pub show_disk_usage: bool,
    /// When the sidebar lists the drives.
    pub sidebar_drives: SidebarDrives,
    /// Ask GitHub for a newer version at start (at most once a day). Off by default:
    /// the app goes online only when asked to.
    pub check_updates: bool,
    /// The font of Markdown, notebooks' text and documents.
    pub reading_font: ReadingFont,
    /// When updates were last looked for (seconds since 1970), for the daily check.
    pub update_checked: i64,
    /// A version the user chose to skip: the check at start does not offer it again.
    pub update_skip: String,
    /// Changed keyboard shortcuts: action id → its keys. The others use the defaults of the
    /// operating system (see `shortcuts`).
    pub shortcuts: std::collections::BTreeMap<String, Vec<String>>,
    /// The apps chosen to open file types: extension (lower case) → program.
    /// A type that is not listed opens with the operating system's default app.
    pub default_apps: std::collections::BTreeMap<String, String>,
    /// The web browser that opens web links (from terminals, the update check): a program
    /// as in `default_apps`. Empty: the operating system's default browser.
    pub browser: String,
    pub terminal_profiles: Vec<TerminalProfile>,
    /// Index into `terminal_profiles` used by the terminal shortcut and button.
    pub default_terminal: usize,
    /// Built-in terminal.
    pub term_font: String,
    pub term_font_size: f32,
    pub term_scrollback: usize,
    pub term_copy_on_select: bool,
    /// Lines scrolled per row of trackpad travel and per mouse wheel notch.
    pub term_scroll_trackpad: f32,
    pub term_scroll_wheel: f32,
    pub term_close_on_exit: bool,
    /// Ask before closing a terminal (window, tab, the app) that is running a program.
    pub term_confirm_close: bool,
    /// Ask before opening a link clicked in a terminal (each terminal can stop asking).
    pub term_confirm_links: bool,
    /// An AI button in a terminal's info bar that starts an AI coding agent.
    pub term_agents: bool,
    /// The agent it starts (`agents::Agent::id`); empty: the first one installed.
    pub term_agent: String,
    pub term_restore: bool,
    /// Always start in the user's home folder (one file tab, no terminal).
    pub start_in_home: bool,
    /// Draw the title bar inside the window, with native-looking buttons.
    pub integrated_titlebar: bool,
    /// Linux (Wayland, GNOME or KDE): round the window's corners like the desktop's own.
    pub round_corners: bool,
    /// Starting Matrix Finder again: a new tab of the running one, or a separate window.
    pub instance_mode: InstanceMode,
    /// Tabs and tab groups of the active pane, restored at start.
    pub session: Option<SavedPane>,
    /// Settings this version does not know (written by a newer one): kept as they are,
    /// so that going back to an older version and forward again loses nothing.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedTab {
    /// The focused window (all of it for a tab with one window).
    pub path: String,
    pub terminal: bool,
    pub group: Option<u32>,
    pub pinned: bool,
    pub title: Option<String>,
    /// A split tab: every window, how they are tiled, and the focused one.
    pub windows: Vec<SavedWindow>,
    pub layout: Option<crate::ui::layout::Layout>,
    pub focus: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedWindow {
    pub path: String,
    pub terminal: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedPane {
    pub tabs: Vec<SavedTab>,
    pub groups: Vec<crate::ui::tabs::TabGroup>,
    pub active: usize,
}

/// See `Settings::bookmark_defaults`.
const BOOKMARK_DEFAULTS: u32 = 1;

/// See `Settings::quick_filter_defaults`.
const QUICK_FILTER_DEFAULTS: u32 = 2;

/// Formats added to a built-in quick filter, with the version that added them.
const QUICK_FILTER_ADDED: &[(u32, &str, &str)] = &[(1, "Data", "safetensors onnx rdata rda rds"), (2, "Data", "geojson wkt wkb")];

/// Allowed terminal scroll speeds (lines per trackpad row or wheel notch).
pub const SCROLL_SPEEDS: std::ops::RangeInclusive<f32> = 0.1..=10.0;

/// The window's size at start (logical pixels): the default, the least that still
/// shows two panels and the sidebar (also the window's own minimum), and the most.
pub const WINDOW_DEFAULT: [f32; 2] = [1000.0, 600.0];
pub const WINDOW_MIN: [f32; 2] = [720.0, 420.0];
pub const WINDOW_MAX: [f32; 2] = [7680.0, 4320.0];

impl Default for Settings {
    fn default() -> Self {
        let home = dirs::home_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
        let mut bookmarks = system_bookmarks();
        if bookmarks.is_empty() {
            bookmarks.push(Bookmark::new("Home", home.clone()));
        }
        Self {
            language: "en".into(),
            theme: crate::style::theme::DEFAULT.into(),
            ui_scale: 1.0,
            ui_font: String::new(),
            ui_font_size: crate::style::fonts::DEFAULT_UI_SIZE,
            terminal: TerminalKind::Native,
            custom_terminal: String::new(),
            show_hidden: false,
            git_branches: true,
            git_label: true,
            git_style: GitStyle::Text,
            git_status: false,
            confirm_delete: true,
            use_trash: true,
            split_horizontal: false,
            left_paths: vec![home.clone()],
            right_paths: vec![home],
            bookmarks,
            bookmark_bar: BookmarkBar::Vertical,
            bookmark_action: BookmarkAction::FilesAndTerminal,
            bookmark_system_icons: true,
            bookmark_defaults: BOOKMARK_DEFAULTS,
            quick_filters: default_quick_filters(),
            quick_filter_defaults: QUICK_FILTER_DEFAULTS,
            connections: vec![],
            backups: vec![],
            quick_view: false,
            quick_view_frac: 1.0 / 3.0,
            sidebar_width: 210.0,
            window_width: WINDOW_DEFAULT[0],
            window_height: WINDOW_DEFAULT[1],
            dirs_first: true,
            show_sidebar: true,
            show_key_hints: true,
            max_parallel: 2,
            confirm_dnd: true,
            icon_pack: "theme".into(),
            show_disk_usage: false,
            sidebar_drives: SidebarDrives::default(),
            check_updates: false,
            reading_font: ReadingFont::default(),
            update_checked: 0,
            update_skip: String::new(),
            shortcuts: Default::default(),
            default_apps: Default::default(),
            browser: String::new(),
            terminal_profiles: vec![TerminalProfile::default()],
            default_terminal: 0,
            term_font: String::new(),
            term_font_size: crate::style::fonts::DEFAULT_TERM_SIZE,
            term_scrollback: 10_000,
            term_copy_on_select: false,
            term_scroll_trackpad: 1.0,
            term_scroll_wheel: 3.0,
            term_close_on_exit: true,
            term_confirm_close: true,
            term_confirm_links: true,
            term_agents: true,
            term_agent: String::new(),
            term_restore: false,
            start_in_home: true,
            integrated_titlebar: true,
            round_corners: true,
            instance_mode: InstanceMode::NewTab,
            session: None,
            unknown: Default::default(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("matrix_finder")
}

/// Carry settings and languages over from the app's former names
/// (Matrix Files, Keymaker, Ferrum Commander), newest first.
fn migrate_old_config() {
    let Some(base) = dirs::config_dir() else { return };
    let new = config_dir();
    if new.join("settings.json").exists() {
        return;
    }
    let Some(old) = ["matrix-files", "keymaker", "ferrum-commander"].iter().map(|n| base.join(n)).find(|d| d.join("settings.json").is_file()) else {
        return;
    };
    let _ = std::fs::create_dir_all(new.join("lang"));
    let _ = std::fs::copy(old.join("settings.json"), new.join("settings.json"));
    if let Ok(rd) = std::fs::read_dir(old.join("lang")) {
        for e in rd.flatten() {
            let _ = std::fs::copy(e.path(), new.join("lang").join(e.file_name()));
        }
    }
}

pub fn config_file() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn lang_dir() -> PathBuf {
    config_dir().join("lang")
}

/// `settings.json.bak`: the version a save replaced.
pub fn backup_file(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// `settings.json.<what>-<date and time>`, a name not taken yet.
fn stamped_file(path: &Path, what: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{what}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    crate::files::safefs::unique_path(&path.with_file_name(name))
}

/// Read settings, keeping every value that can be read: a bad or unknown value (a hand
/// edit, or a newer version's choice this one does not know) resets only that setting,
/// or only that entry of a list, not the whole file. Returns the settings and the names
/// of what was reset; `Err` when the text is not a settings file at all.
pub fn parse_lenient(text: &str) -> Result<(Settings, Vec<String>), String> {
    use serde_json::{Map, Value};
    let v: Value = serde_json::from_str(text).map_err(es)?;
    let Value::Object(file) = v else { return Err("not a settings file".into()) };
    if let Ok(mut s) = serde_json::from_value::<Settings>(Value::Object(file.clone())) {
        s.normalize();
        return Ok((s, vec![]));
    }
    // Every setting has a default, so each one can be tried on its own.
    let fits = |k: &str, v: Value| serde_json::from_value::<Settings>(Value::Object(Map::from_iter([(k.to_string(), v)]))).is_ok();
    let mut good = Map::new();
    let mut reset = Vec::new();
    for (k, v) in file {
        if fits(&k, v.clone()) {
            good.insert(k, v);
            continue;
        }
        match v {
            Value::Array(items) => {
                let n = items.len();
                let kept: Vec<Value> = items.into_iter().filter(|i| fits(&k, Value::Array(vec![i.clone()]))).collect();
                reset.push(format!("{k} ({} / {n})", n - kept.len()));
                good.insert(k, Value::Array(kept));
            }
            Value::Object(entries) => {
                let kept: Map<String, Value> =
                    entries.into_iter().filter(|(ek, ev)| fits(&k, Value::Object(Map::from_iter([(ek.clone(), ev.clone())])))).collect();
                reset.push(k.clone());
                good.insert(k, Value::Object(kept));
            }
            _ => reset.push(k),
        }
    }
    let mut s: Settings = serde_json::from_value(Value::Object(good)).map_err(es)?;
    s.normalize();
    Ok((s, reset))
}

/// What loading the settings ran into, told to the user at start.
#[derive(Clone, Debug, PartialEq)]
pub enum LoadNote {
    /// Values this version cannot read were reset (`reset` names them); the file as it
    /// was is kept at `copy`.
    Repaired { reset: Vec<String>, copy: Option<PathBuf> },
    /// The file could not be read at all. It was set aside at `moved` (never deleted);
    /// `from_backup`: the previous version was loaded instead of the defaults; `read_only`:
    /// it could not be set aside, so the settings are not saved over it.
    Broken { error: String, moved: Option<PathBuf>, from_backup: bool, read_only: bool },
    /// The settings file was missing; the previous version (`.bak`) was loaded.
    FromBackup(PathBuf),
}

/// The settings file of this process. It remembers what it last read or wrote there, so
/// that a save notices when another Matrix Finder (`--new-window` runs one) saved
/// meanwhile, and keeps what that one added or changed instead of writing over it.
pub struct Store {
    path: PathBuf,
    /// SHA-256 of the file as this process last read or wrote it.
    seen: Option<[u8; 32]>,
    /// The settings in it then (as `Settings` writes them): what both sides of a merge
    /// started from.
    base: serde_json::Value,
    /// The file on disk loaded cleanly: a save keeps it as `.bak`.
    disk_good: bool,
    /// Saving is off: the file could not be read nor set aside.
    read_only: Option<String>,
    pub note: Option<LoadNote>,
}

impl Store {
    /// Load the settings (see `open_at`).
    pub fn open() -> (Settings, Store) {
        migrate_old_config();
        Self::open_at(&config_file())
    }

    /// Load the settings at `path`. Nothing is lost on the way: a value that cannot be read
    /// is reset alone (the file is copied aside first); a file that cannot be read at all
    /// is renamed `….broken-<date>` and the previous version (`.bak`) is loaded if there is
    /// one, the defaults otherwise.
    pub fn open_at(path: &Path) -> (Settings, Store) {
        let mut store = Store { path: path.to_path_buf(), seen: None, base: serde_json::Value::Null, disk_good: false, read_only: None, note: None };
        let broken = match std::fs::read(path) {
            Ok(bytes) => match parse_lenient(&String::from_utf8_lossy(&bytes)) {
                Ok((s, reset)) => {
                    store.seen = Some(crate::util::sha256(&bytes));
                    store.base = serde_json::to_value(&s).unwrap_or_default();
                    store.disk_good = reset.is_empty();
                    if !reset.is_empty() {
                        let copy = stamped_file(path, "broken");
                        let copy = crate::util::write_atomic(&copy, &bytes, true).is_ok().then_some(copy);
                        store.note = Some(LoadNote::Repaired { reset, copy });
                    }
                    return (s, store);
                }
                Err(e) => {
                    let moved = stamped_file(path, "broken");
                    let moved = std::fs::rename(path, &moved).is_ok().then_some(moved);
                    if moved.is_none() {
                        store.read_only = Some(e.clone());
                    }
                    Some((e, moved))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                store.read_only = Some(e.to_string());
                Some((e.to_string(), None))
            }
        };
        let bak = backup_file(path);
        let from_backup = if store.read_only.is_none() { std::fs::read_to_string(&bak).ok().and_then(|t| parse_lenient(&t).ok()) } else { None };
        store.note = match &broken {
            Some((error, moved)) => {
                Some(LoadNote::Broken { error: error.clone(), moved: moved.clone(), from_backup: from_backup.is_some(), read_only: store.read_only.is_some() })
            }
            None if from_backup.is_some() => Some(LoadNote::FromBackup(bak)),
            None => None,
        };
        let mut s = from_backup.map(|(s, _)| s).unwrap_or_default();
        s.normalize();
        // Nothing on disk to merge with: the first save writes these.
        store.base = serde_json::to_value(&s).unwrap_or_default();
        (s, store)
    }

    /// Where the settings are kept.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Save `s`, atomically and flushed to the disk. When another Matrix Finder saved
    /// since this one last read or wrote the file, both are merged: what only that one
    /// changed (connections, bookmarks, backup profiles, filters, preferences…) is kept,
    /// and what both changed takes this one's value. Returns the merged settings when they
    /// differ from `s`, to be used from now on. The version replaced, if it was good,
    /// stays as `.bak`.
    pub fn save(&mut self, s: &Settings) -> Res<Option<Settings>> {
        if let Some(e) = &self.read_only {
            return Err(format!("{} ({e})", self.path.display()));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(es)?;
            // Saved passwords are in it: only the user may look inside.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        let mut mine = serde_json::to_value(s).map_err(es)?;
        let mut merged = None;
        let disk = std::fs::read(&self.path).ok();
        let mut disk_good = self.disk_good;
        if let Some(d) = disk.as_deref().filter(|d| Some(crate::util::sha256(d)) != self.seen) {
            match parse_lenient(&String::from_utf8_lossy(d)) {
                Ok((theirs, reset)) => {
                    disk_good = reset.is_empty();
                    let theirs = serde_json::to_value(&theirs).map_err(es)?;
                    let m = merge_value("", Some(&self.base), &mine, &theirs);
                    if m != mine {
                        let mut m: Settings = serde_json::from_value(m).map_err(es)?;
                        m.normalize();
                        mine = serde_json::to_value(&m).map_err(es)?;
                        merged = Some(m);
                    }
                }
                Err(_) => {
                    // Written by something else and unreadable: set aside, not overwritten.
                    disk_good = false;
                    crate::util::write_atomic(&stamped_file(&self.path, "broken"), d, true).map_err(es)?;
                }
            }
        }
        let json = serde_json::to_string_pretty(&mine).map_err(es)?;
        if disk.as_deref() != Some(json.as_bytes()) {
            if let (true, Some(d)) = (disk_good, &disk) {
                let _ = crate::util::write_atomic(&backup_file(&self.path), d, true);
            }
            crate::util::write_private(&self.path, &json).map_err(es)?;
        }
        self.seen = Some(crate::util::sha256(json.as_bytes()));
        self.base = mine;
        self.disk_good = true;
        Ok(merged)
    }

    /// Keep the settings `s` (with passwords) as `settings.json.before-import-<date>`
    /// before an import replaces them; returns where.
    pub fn backup_before_import(&self, s: &Settings) -> Res<PathBuf> {
        let p = stamped_file(&self.path, "before-import");
        s.export_to(&p, true)?;
        Ok(p)
    }
}

/// Lists whose items are told apart by a name (or a bookmark's path) in a merge.
fn list_id(key: &str, item: &serde_json::Value) -> Option<String> {
    let o = item.as_object()?;
    let text = |k: &str| o.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match key {
        "connections" | "backups" | "quick_filters" | "terminal_profiles" => text("name"),
        "bookmarks" | "children" if o.get("folder") == Some(&serde_json::Value::Bool(true)) => text("name").map(|n| format!("folder:{n}")),
        "bookmarks" | "children" => text("path").map(|p| format!("path:{p}")),
        _ => None,
    }
}

/// Settings that are maps, where a key removed on one side is removed in a merge.
const MAP_SETTINGS: [&str; 2] = ["shortcuts", "default_apps"];

/// Three-way merge of one setting: `base` is what this process last saw on disk, `mine`
/// its value now and `theirs` the value another process saved meanwhile.
fn merge_value(key: &str, base: Option<&serde_json::Value>, mine: &serde_json::Value, theirs: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    if mine == theirs || base == Some(theirs) {
        return mine.clone();
    }
    if base == Some(mine) {
        return theirs.clone();
    }
    // Both changed it.
    match (mine, theirs) {
        (Value::Object(m), Value::Object(t)) => Value::Object(merge_object(key, base.and_then(Value::as_object), m, t)),
        (Value::Array(m), Value::Array(t)) if m.iter().chain(t).all(|i| list_id(key, i).is_some()) => {
            Value::Array(merge_list(key, base.and_then(Value::as_array), m, t))
        }
        _ => mine.clone(),
    }
}

fn merge_object(
    key: &str,
    base: Option<&serde_json::Map<String, serde_json::Value>>,
    mine: &serde_json::Map<String, serde_json::Value>,
    theirs: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut out = mine.clone();
    for (k, t) in theirs {
        let b = base.and_then(|b| b.get(k));
        match mine.get(k) {
            Some(m) => {
                out.insert(k.clone(), merge_value(k, b, m, t));
            }
            // Added there, or removed here but changed there: kept.
            None if b != Some(t) => {
                out.insert(k.clone(), t.clone());
            }
            None => {}
        }
    }
    // A map entry removed there and not changed here is removed. (A setting missing
    // there is only one that version does not know.)
    if MAP_SETTINGS.contains(&key) {
        for (k, m) in mine {
            if !theirs.contains_key(k) && base.and_then(|b| b.get(k)) == Some(m) {
                out.remove(k);
            }
        }
    }
    out
}

/// Merge a list of named items (see `list_id`): items added on either side are kept,
/// one removed on one side and unchanged on the other is removed, and an item changed on
/// both is merged field by field.
fn merge_list(key: &str, base: Option<&Vec<serde_json::Value>>, mine: &[serde_json::Value], theirs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let none = Vec::new();
    let base = base.map_or(&none[..], |b| &b[..]);
    let id = |v: &serde_json::Value| list_id(key, v);
    let unique = |l: &[serde_json::Value]| {
        let mut seen = std::collections::HashSet::new();
        l.iter().all(|i| seen.insert(id(i)))
    };
    if !(unique(mine) && unique(theirs) && unique(base)) {
        // Two items of one name: keep everything of this side, and add theirs that are new.
        let mut out = mine.to_vec();
        out.extend(theirs.iter().filter(|t| !mine.contains(t) && !base.contains(t)).cloned());
        return out;
    }
    let find = |l: &'_ [serde_json::Value], k: &Option<String>| l.iter().find(|i| id(i) == *k).cloned();
    let mut out = Vec::new();
    for m in mine {
        let k = id(m);
        let b = find(base, &k);
        match find(theirs, &k) {
            Some(t) => out.push(merge_value(key, b.as_ref(), m, &t)),
            // Removed there and unchanged here: removed.
            None if b.as_ref() == Some(m) => {}
            None => out.push(m.clone()),
        }
    }
    for t in theirs {
        let k = id(t);
        if find(mine, &k).is_none() && find(base, &k).as_ref() != Some(t) {
            // Added there, or removed here but changed there: kept.
            out.push(t.clone());
        }
    }
    out
}

impl Settings {
    /// The settings as saved, without changing anything on disk (see `Store::open`).
    pub fn load() -> Self {
        migrate_old_config();
        let path = config_file();
        let read = |p: &Path| std::fs::read_to_string(p).ok().and_then(|t| parse_lenient(&t).ok()).map(|(s, _)| s);
        let mut s = read(&path).or_else(|| read(&backup_file(&path))).unwrap_or_default();
        s.normalize();
        s
    }

    /// Fill in values that older settings files did not have.
    pub fn normalize(&mut self) {
        self.default_apps = std::mem::take(&mut self.default_apps)
            .into_iter()
            .map(|(ext, program)| (crate::system::defaultapps::normalize_ext(&ext), program))
            .filter(|(ext, program)| !ext.is_empty() && !program.trim().is_empty())
            .collect();
        if self.terminal_profiles.is_empty() {
            // Older versions had a single terminal choice.
            self.terminal_profiles.push(TerminalProfile {
                name: "Default".into(),
                terminal: self.terminal,
                custom: self.custom_terminal.clone(),
                ..Default::default()
            });
        }
        if !self.terminal_profiles.iter().any(|p| p.terminal == TerminalKind::BuiltIn) {
            // Offer the built-in terminal to configs from before it existed.
            self.terminal_profiles.push(TerminalProfile { name: "Built-in".into(), terminal: TerminalKind::BuiltIn, ..Default::default() });
        }
        self.default_terminal = self.default_terminal.min(self.terminal_profiles.len() - 1);
        if !(6.0..=40.0).contains(&self.term_font_size) {
            self.term_font_size = crate::style::fonts::DEFAULT_TERM_SIZE;
        }
        for (v, default) in [(&mut self.term_scroll_trackpad, 1.0), (&mut self.term_scroll_wheel, 3.0)] {
            if !SCROLL_SPEEDS.contains(v) {
                *v = default;
            }
        }
        if !crate::style::fonts::UI_SIZES.contains(&self.ui_font_size) {
            self.ui_font_size = crate::style::fonts::DEFAULT_UI_SIZE;
        }
        // The window size at start: within reach of the screen and big enough to use,
        // whatever was typed or read from a damaged file.
        for (v, k) in [(&mut self.window_width, 0), (&mut self.window_height, 1)] {
            *v = if v.is_finite() { v.round().clamp(WINDOW_MIN[k], WINDOW_MAX[k]) } else { WINDOW_DEFAULT[k] };
        }
        if !(160.0..=360.0).contains(&self.sidebar_width) {
            self.sidebar_width = 210.0;
        }
        if !(0.15..=0.7).contains(&self.quick_view_frac) {
            self.quick_view_frac = 1.0 / 3.0;
        }
        if self.bookmark_defaults < BOOKMARK_DEFAULTS {
            // Settings from before Music and Movies were defaults: add the standard
            // folders that are not bookmarked yet, once.
            for b in system_bookmarks() {
                if !Bookmark::leaves(&self.bookmarks).any(|x| x.path == b.path) {
                    self.bookmarks.push(b);
                }
            }
            self.bookmark_defaults = BOOKMARK_DEFAULTS;
        }
        if self.quick_filter_defaults < QUICK_FILTER_DEFAULTS {
            // Settings saved before these formats were in the built-in categories:
            // add them once, to the category of that name if it is still there.
            for &(version, name, exts) in QUICK_FILTER_ADDED {
                if version <= self.quick_filter_defaults {
                    continue;
                }
                if let Some(q) = self.quick_filters.iter_mut().find(|q| q.name == name) {
                    let have = q.ext_list();
                    for e in exts.split_whitespace().filter(|e| !have.iter().any(|h| h == e)) {
                        if !q.exts.trim().is_empty() {
                            q.exts.push(' ');
                        }
                        q.exts.push_str(e);
                    }
                }
            }
            self.quick_filter_defaults = QUICK_FILTER_DEFAULTS;
        }
    }

    /// Back to the default preferences. Your data is kept: bookmarks, saved
    /// connections, backup profiles, terminal profiles and the open tabs.
    pub fn reset_preferences(&mut self) {
        let keep = std::mem::take(self);
        *self = Settings {
            bookmarks: keep.bookmarks,
            bookmark_defaults: keep.bookmark_defaults,
            quick_filters: keep.quick_filters,
            quick_filter_defaults: keep.quick_filter_defaults,
            default_apps: keep.default_apps,
            connections: keep.connections,
            backups: keep.backups,
            terminal_profiles: keep.terminal_profiles,
            default_terminal: keep.default_terminal,
            left_paths: keep.left_paths,
            right_paths: keep.right_paths,
            session: keep.session,
            update_checked: keep.update_checked,
            update_skip: keep.update_skip,
            unknown: keep.unknown,
            ..Settings::default()
        };
        self.normalize();
    }

    /// Write settings to `path`. Passwords are stripped unless `with_secrets`.
    pub fn export_to(&self, path: &Path, with_secrets: bool) -> Res<()> {
        let mut copy = self.clone();
        if !with_secrets {
            for c in &mut copy.connections {
                c.password.clear();
            }
        }
        let json = serde_json::to_string_pretty(&copy).map_err(es)?;
        // With passwords, readable by the user alone.
        if with_secrets {
            crate::util::write_private(path, &json).map_err(es)
        } else {
            crate::util::write_atomic(path, json.as_bytes(), false).map_err(es)
        }
    }

    pub fn import_from(path: &Path) -> Res<Self> {
        let s = std::fs::read_to_string(path).map_err(es)?;
        let mut v: Settings = serde_json::from_str(&s).map_err(es)?;
        v.normalize();
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("matrix_finder-cfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn conn(name: &str) -> ConnectionProfile {
        ConnectionProfile { name: name.into(), host: format!("{name}.example.com"), password: "pw".into(), ..Default::default() }
    }

    fn names(s: &Settings) -> Vec<&str> {
        s.connections.iter().map(|c| c.name.as_str()).collect()
    }

    fn files_in(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    }

    /// A start size out of range is brought into it; one of the wrong kind is the default.
    #[test]
    fn window_size_stays_usable() {
        let (mut s, _) = parse_lenient(r#"{"window_width": 10, "window_height": 99999}"#).unwrap();
        s.normalize();
        assert_eq!((s.window_width, s.window_height), (WINDOW_MIN[0], WINDOW_MAX[1]));
        let (mut s, reset) = parse_lenient(r#"{"window_width": "wide", "window_height": -5}"#).unwrap();
        s.normalize();
        assert_eq!((s.window_width, s.window_height), (WINDOW_DEFAULT[0], WINDOW_MIN[1]));
        assert!(reset.iter().any(|r| r == "window_width"));
        let mut s = Settings { window_width: f32::NAN, window_height: 1000.4, ..Settings::default() };
        s.normalize();
        assert_eq!((s.window_width, s.window_height), (WINDOW_DEFAULT[0], 1000.0));
    }

    /// One unreadable value resets that value alone: saved connections survive a newer
    /// version's setting, a hand edit gone wrong or an unknown kind of connection.
    #[test]
    fn bad_values_reset_alone() {
        let text = r#"{"theme":"Dracula","terminal":"FutureTerminal","ui_scale":"big",
            "connections":[{"name":"a","protocol":"Sftp","host":"h","port":22,"user":"u","password":"pw"},
                           {"name":"b","protocol":"Scp","host":"h","port":22,"user":"u"}],
            "shortcuts":{"copy":["F5"],"move":"F6"},
            "from_the_future":{"x":1}}"#;
        let (s, reset) = parse_lenient(text).unwrap();
        assert_eq!(s.theme, "Dracula");
        assert_eq!(s.terminal, Settings::default().terminal);
        assert_eq!(s.ui_scale, 1.0);
        assert_eq!(names(&s), ["a"]);
        assert_eq!(s.connections[0].password, "pw");
        assert_eq!(s.shortcuts.get("copy"), Some(&vec!["F5".to_string()]));
        assert!(!s.shortcuts.contains_key("move"));
        for k in ["terminal", "ui_scale", "connections (1 / 2)", "shortcuts"] {
            assert!(reset.iter().any(|r| r == k), "{k} in {reset:?}");
        }
        // A setting of a newer version is kept as it is.
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["from_the_future"]["x"], 1);
        assert!(parse_lenient("not json").is_err());
        assert!(parse_lenient("[1]").is_err());
        assert!(parse_lenient(r#"{"theme":"#).is_err());
    }

    #[test]
    fn broken_file_is_set_aside_and_the_backup_loaded() {
        let d = test_dir("broken");
        let p = d.join("settings.json");
        // First start: defaults, nothing to tell.
        let (s, store) = Store::open_at(&p);
        assert!(store.note.is_none());
        assert_eq!(s.connections, vec![]);
        // Saved twice: the first version stays as the backup.
        let (mut s, mut store) = Store::open_at(&p);
        s.connections.push(conn("one"));
        store.save(&s).unwrap();
        s.connections.push(conn("two"));
        store.save(&s).unwrap();
        assert_eq!(names(&Store::open_at(&backup_file(&p)).0), ["one"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(backup_file(&p)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Half written by something: renamed aside, never deleted; the backup is loaded.
        std::fs::write(&p, r#"{"connections":[{"name":"#).unwrap();
        let (s, mut store) = Store::open_at(&p);
        assert_eq!(names(&s), ["one"]);
        let Some(LoadNote::Broken { moved: Some(moved), from_backup: true, read_only: false, .. }) = &store.note else { panic!("{:?}", store.note) };
        assert_eq!(std::fs::read_to_string(moved).unwrap(), r#"{"connections":[{"name":"#);
        assert!(!p.exists());
        // Saving now writes the loaded settings (and keeps the backup).
        store.save(&s).unwrap();
        assert_eq!(names(&Store::open_at(&p).0), ["one"]);
        // Missing file: the backup.
        std::fs::remove_file(&p).unwrap();
        let (s, store) = Store::open_at(&p);
        assert_eq!(names(&s), ["one"]);
        assert!(matches!(store.note, Some(LoadNote::FromBackup(_))));
        // A bad value: the rest is kept, and the file as it was is copied aside.
        std::fs::write(&p, r#"{"terminal":"FutureTerminal","connections":[{"name":"c","protocol":"Ftp","host":"h","port":21,"user":"u"}]}"#).unwrap();
        let (s, store) = Store::open_at(&p);
        assert_eq!(names(&s), ["c"]);
        let Some(LoadNote::Repaired { reset, copy: Some(copy) }) = &store.note else { panic!("{:?}", store.note) };
        assert_eq!(reset, &["terminal"]);
        assert!(std::fs::read_to_string(copy).unwrap().contains("FutureTerminal"));
        assert!(files_in(&d).iter().all(|f| !f.contains(".tmp-")), "no temporary files: {:?}", files_in(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Two windows (processes) save the same file: neither loses what the other added.
    #[test]
    fn saves_of_two_processes_are_merged() {
        let d = test_dir("merge");
        let p = d.join("settings.json");
        let (mut s, mut store) = Store::open_at(&p);
        s.connections = vec![conn("old"), conn("gone")];
        s.bookmarks = vec![Bookmark::new("Work", "/work")];
        store.save(&s).unwrap();

        let (mut a, mut sa) = Store::open_at(&p);
        let (mut b, mut sb) = Store::open_at(&p);
        // A adds a connection and a bookmark, removes one, edits another and changes the theme.
        a.connections.push(conn("from-a"));
        a.connections.retain(|c| c.name != "gone");
        a.connections[0].password = "new".into();
        a.bookmarks.push(Bookmark::new("A", "/a"));
        a.theme = "Dracula".into();
        assert!(sa.save(&a).unwrap().is_none(), "nothing to merge yet");
        // B, still with the old file in mind, adds its own and changes another setting.
        b.connections.push(conn("from-b"));
        b.show_hidden = true;
        b.default_apps.insert("txt".into(), "vim".into());
        let merged = sb.save(&b).unwrap().expect("merged with A's save");
        assert_eq!(names(&merged), ["old", "from-b", "from-a"]);
        assert_eq!(merged.connections[0].password, "new", "A's edit kept");
        assert_eq!(merged.theme, "Dracula");
        assert!(merged.show_hidden);
        assert_eq!(merged.bookmarks.len(), 2);
        let on_disk = Store::open_at(&p).0;
        assert_eq!(names(&on_disk), names(&merged));
        assert_eq!(on_disk.default_apps.get("txt").map(String::as_str), Some("vim"));

        // A saves again without knowing B's changes: they stay, and its own removal holds.
        a.connections.retain(|c| c.name != "old");
        let merged = sa.save(&a).unwrap().expect("merged with B's save");
        assert_eq!(names(&merged), ["from-a", "from-b"]);
        assert!(merged.show_hidden);
        assert_eq!(merged.theme, "Dracula");

        // Both changed the same setting: this process's value wins.
        let (mut c1, mut s1) = Store::open_at(&p);
        let (mut c2, mut s2) = Store::open_at(&p);
        c1.theme = "One".into();
        s1.save(&c1).unwrap();
        c2.theme = "Two".into();
        c2.connections.retain(|c| c.name != "from-b");
        let m = s2.save(&c2).unwrap();
        assert!(m.is_none(), "nothing of the other one's to keep");
        let on_disk = Store::open_at(&p).0;
        assert_eq!(on_disk.theme, "Two");
        assert_eq!(names(&on_disk), ["from-a"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn merge_rules() {
        use serde_json::json;
        let base = json!({"connections": [{"name": "x", "host": "h"}], "shortcuts": {"a": ["F1"], "b": ["F2"]}});
        // Removed here, changed there: kept, so nothing changed by the user disappears.
        let mine = json!({"connections": [], "shortcuts": {"a": ["F1"], "b": ["F2"]}});
        let theirs = json!({"connections": [{"name": "x", "host": "new"}], "shortcuts": {"a": ["F1"]}});
        let m = merge_value("", Some(&base), &mine, &theirs);
        assert_eq!(m["connections"], json!([{"name": "x", "host": "new"}]));
        // A shortcut removed there is removed.
        assert_eq!(m["shortcuts"], json!({"a": ["F1"]}));
        // Same names twice: everything is kept.
        let mine = json!({"connections": [{"name": "x", "host": "1"}, {"name": "x", "host": "2"}]});
        let theirs = json!({"connections": [{"name": "y", "host": "3"}]});
        let m = merge_value("", Some(&base), &mine, &theirs);
        assert_eq!(m["connections"].as_array().unwrap().len(), 3);
        // A setting the other version does not know is kept.
        let m = merge_value("", Some(&json!({"a": 1, "new": 2})), &json!({"a": 2, "new": 2}), &json!({"a": 1}));
        assert_eq!(m, json!({"a": 2, "new": 2}));
    }

    #[test]
    fn reset_keeps_user_data() {
        let mut s = Settings::default();
        s.theme = "Dracula".into();
        s.ui_scale = 1.6;
        s.show_hidden = true;
        s.quick_view = true;
        s.start_in_home = false;
        s.language = "vi".into();
        s.bookmarks.push(Bookmark::new("Work", "/work"));
        s.connections.push(ConnectionProfile { name: "srv".into(), host: "example.com".into(), ..Default::default() });
        s.terminal_profiles.push(TerminalProfile { name: "Fish".into(), shell: "fish".into(), ..Default::default() });
        s.default_terminal = 1;
        s.reset_preferences();
        let d = Settings::default();
        assert_eq!(s.theme, d.theme);
        assert_eq!(s.theme, "Breeze Light");
        assert_eq!(s.ui_scale, d.ui_scale);
        assert_eq!(s.language, d.language);
        assert!(!s.show_hidden && !s.quick_view && s.start_in_home);
        assert!(s.bookmarks.iter().any(|b| b.name == "Work"));
        assert_eq!(s.connections.len(), 1);
        assert_eq!(s.terminal_profiles.len(), 2);
        assert_eq!(s.default_terminal, 1);
    }

    #[test]
    fn bookmark_folders_and_migration() {
        // Old settings: name + path only, no bookmark fields at all.
        let old = r#"{"bookmarks":[{"name":"Work","path":"/work"}]}"#;
        let mut v: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(v.bookmark_defaults, 0);
        v.normalize();
        assert_eq!(v.bookmarks[0], Bookmark::new("Work", "/work"));
        assert_eq!(v.bookmark_defaults, BOOKMARK_DEFAULTS);
        assert_eq!(v.bookmark_bar, BookmarkBar::Vertical);
        assert!(v.bookmark_system_icons);
        for b in system_bookmarks() {
            assert!(Bookmark::leaves(&v.bookmarks).any(|x| x.path == b.path), "standard folder added: {}", b.name);
        }
        // Once only: a removed standard folder stays removed.
        v.bookmarks.truncate(1);
        v.normalize();
        assert_eq!(v.bookmarks.len(), 1);

        // Folders: leaves and removal by path look inside them.
        let mut list = vec![Bookmark::new("A", "/a"), Bookmark::new_folder("Work")];
        list[1].children.push(Bookmark::new("B", "/b"));
        let names: Vec<&str> = Bookmark::leaves(&list).map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["A", "B"]);
        assert!(Bookmark::remove_path(&mut list, "/b"));
        assert!(list[1].children.is_empty() && list.len() == 2);
        let back: Vec<Bookmark> = serde_json::from_str(&serde_json::to_string(&list).unwrap()).unwrap();
        assert_eq!(back, list);

        // Icons: automatic follows the standard folder unless plain icons are wanted.
        if let Some(d) = dirs::download_dir() {
            let p = d.to_string_lossy();
            assert_eq!(BookmarkIcon::Auto.resolve(&p, true), BookmarkIcon::Downloads);
            assert_eq!(BookmarkIcon::Auto.resolve(&p, false), BookmarkIcon::Folder);
            assert_eq!(BookmarkIcon::Star.resolve(&p, false), BookmarkIcon::Star);
        }
        assert_eq!(BookmarkIcon::Auto.resolve("sftp://host/x", true), BookmarkIcon::Network);
    }

    #[test]
    fn quick_filters() {
        let q = QuickFilter::new("Mixed", ".JPG, png;*.gif  jpg");
        assert_eq!(q.ext_list(), ["jpg", "png", "gif"]);
        // Older settings get the built-in categories.
        let v: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(v.quick_filters, default_quick_filters());
        let names: Vec<&str> = v.quick_filters.iter().map(|q| q.name.as_str()).collect();
        for n in ["Images", "Music", "Development", "Documents"] {
            assert!(names.contains(&n));
        }
        for q in &v.quick_filters {
            assert!(q.ext_list().len() >= 4, "{} has its common formats", q.name);
        }
        // Your filters are kept by "Reset to defaults".
        let mut s = Settings::default();
        s.quick_filters = vec![QuickFilter::new("Raw", "cr2 nef")];
        s.reset_preferences();
        assert_eq!(s.quick_filters.len(), 1);
    }

    #[test]
    fn new_quick_filters_get_their_own_names() {
        let mut all = vec![QuickFilter::new("Images", "jpg")];
        assert_eq!(unused_quick_filter_name(&all, "New filter"), "New filter");
        all.push(QuickFilter::new("New filter", ""));
        assert_eq!(unused_quick_filter_name(&all, "New filter"), "New filter 2");
        all.push(QuickFilter::new("New filter 2", ""));
        assert_eq!(unused_quick_filter_name(&all, "New filter"), "New filter 3");
    }

    #[test]
    fn sidebar_drives_setting() {
        // Older settings: drives only when there are several.
        let v: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(v.sidebar_drives, SidebarDrives::WhenSeveral);
        assert_eq!(Settings::default().sidebar_drives, SidebarDrives::WhenSeveral);
        let s = Settings { sidebar_drives: SidebarDrives::Never, ..Default::default() };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.sidebar_drives, SidebarDrives::Never);
    }

    #[test]
    fn quick_filter_migration() {
        // Older settings: the Data category without the new formats, plus a filter of your own.
        let old = r#"{"quick_filters":[{"name":"Data","exts":"parquet npy rds"},{"name":"Raw","exts":"cr2"}]}"#;
        let mut v: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(v.quick_filter_defaults, 0);
        v.normalize();
        assert_eq!(v.quick_filters[0].exts, "parquet npy rds safetensors onnx rdata rda geojson wkt wkb");
        assert_eq!(v.quick_filters[1].exts, "cr2");
        assert_eq!(v.quick_filter_defaults, QUICK_FILTER_DEFAULTS);
        // Once only: an extension you remove stays removed.
        v.quick_filters[0].exts = "parquet".into();
        v.normalize();
        assert_eq!(v.quick_filters[0].exts, "parquet");
        // New settings already have them.
        let mut s = Settings::default();
        s.normalize();
        assert_eq!(s.quick_filters, default_quick_filters());
        for e in ["safetensors", "onnx", "rdata", "rda", "rds", "geojson", "wkt", "wkb"] {
            assert!(s.quick_filters.iter().any(|q| q.name == "Data" && q.ext_list().iter().any(|x| x == e)), "{e}");
        }
    }
}
