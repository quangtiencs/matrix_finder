//! Persistent settings (JSON in the platform config directory).
use crate::archive::ArchiveFormat;
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
            let name = if k == BookmarkIcon::Home { "Home".to_string() } else { d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| k.label().to_string()) };
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
        QuickFilter::new("Data", "parquet avro arrow feather orc jsonl ndjson ipynb sqlite sqlite3 db h5 hdf5 npy npz safetensors onnx rdata rda rds pkl pickle"),
        QuickFilter::new("Archives", "zip rar 7z tar gz tgz bz2 tbz2 xz txz zst lz4 iso dmg cab deb rpm jar"),
    ]
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
            if self.initial_path.starts_with('/') || self.initial_path.is_empty() {
                self.initial_path.clone()
            } else {
                format!("/{}", self.initial_path)
            }
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
            excludes: vec![
                "**/.git/**".into(),
                "**/node_modules/**".into(),
                "**/target/**".into(),
                "*.tmp".into(),
            ],
            destination: dirs::document_dir()
                .or_else(dirs::home_dir)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            format: ArchiveFormat::Zip,
            level: 6,
            file_template: "{name}-{date}".into(),
            keep_last: 0,
            last_run: None,
        }
    }
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
    pub dirs_first: bool,
    pub show_sidebar: bool,
    pub show_key_hints: bool,
    pub max_parallel: usize,
    pub confirm_dnd: bool,
    /// "theme", "system", "matrix", "adwaita", "breeze", "macos", "windows" or "icons:<theme>".
    pub icon_pack: String,
    pub show_disk_usage: bool,
    /// Changed keyboard shortcuts: action id → its keys. The others use the defaults of the
    /// operating system (see `shortcuts`).
    pub shortcuts: std::collections::BTreeMap<String, Vec<String>>,
    /// The apps chosen to open file types: extension (lower case) → program.
    /// A type that is not listed opens with the operating system's default app.
    pub default_apps: std::collections::BTreeMap<String, String>,
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
    /// An AI button in a terminal's info bar that starts an AI coding agent.
    pub term_agents: bool,
    /// The agent it starts (`agents::Agent::id`); empty: the first one installed.
    pub term_agent: String,
    pub term_restore: bool,
    /// Always start in the user's home folder (one file tab, no terminal).
    pub start_in_home: bool,
    /// Draw the title bar inside the window, with native-looking buttons.
    pub integrated_titlebar: bool,
    /// Starting Matrix Finder again: a new tab of the running one, or a separate window.
    pub instance_mode: InstanceMode,
    /// Tabs and tab groups of the active pane, restored at start.
    pub session: Option<SavedPane>,
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
    pub layout: Option<crate::layout::Layout>,
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
    pub groups: Vec<crate::tabs::TabGroup>,
    pub active: usize,
}

/// See `Settings::bookmark_defaults`.
const BOOKMARK_DEFAULTS: u32 = 1;

/// See `Settings::quick_filter_defaults`.
const QUICK_FILTER_DEFAULTS: u32 = 1;

/// Formats added to a built-in quick filter, with the version that added them.
const QUICK_FILTER_ADDED: &[(u32, &str, &str)] = &[(1, "Data", "safetensors onnx rdata rda rds")];

/// Allowed terminal scroll speeds (lines per trackpad row or wheel notch).
pub const SCROLL_SPEEDS: std::ops::RangeInclusive<f32> = 0.1..=10.0;

impl Default for Settings {
    fn default() -> Self {
        let home = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".into());
        let mut bookmarks = system_bookmarks();
        if bookmarks.is_empty() {
            bookmarks.push(Bookmark::new("Home", home.clone()));
        }
        Self {
            language: "en".into(),
            theme: crate::theme::DEFAULT.into(),
            ui_scale: 1.0,
            ui_font: String::new(),
            ui_font_size: crate::fonts::DEFAULT_UI_SIZE,
            terminal: TerminalKind::Native,
            custom_terminal: String::new(),
            show_hidden: false,
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
            dirs_first: true,
            show_sidebar: true,
            show_key_hints: true,
            max_parallel: 2,
            confirm_dnd: true,
            icon_pack: "theme".into(),
            show_disk_usage: false,
            shortcuts: Default::default(),
            default_apps: Default::default(),
            terminal_profiles: vec![TerminalProfile::default()],
            default_terminal: 0,
            term_font: String::new(),
            term_font_size: crate::fonts::DEFAULT_TERM_SIZE,
            term_scrollback: 10_000,
            term_copy_on_select: false,
            term_scroll_trackpad: 1.0,
            term_scroll_wheel: 3.0,
            term_close_on_exit: true,
            term_confirm_close: true,
            term_agents: true,
            term_agent: String::new(),
            term_restore: false,
            start_in_home: true,
            integrated_titlebar: true,
            instance_mode: InstanceMode::NewTab,
            session: None,
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

impl Settings {
    pub fn load() -> Self {
        migrate_old_config();
        let mut s: Settings = std::fs::read_to_string(config_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        s.normalize();
        s
    }

    /// Fill in values that older settings files did not have.
    pub fn normalize(&mut self) {
        self.default_apps = std::mem::take(&mut self.default_apps)
            .into_iter()
            .map(|(ext, program)| (crate::defaultapps::normalize_ext(&ext), program))
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
            self.term_font_size = crate::fonts::DEFAULT_TERM_SIZE;
        }
        for (v, default) in [(&mut self.term_scroll_trackpad, 1.0), (&mut self.term_scroll_wheel, 3.0)] {
            if !SCROLL_SPEEDS.contains(v) {
                *v = default;
            }
        }
        if !crate::fonts::UI_SIZES.contains(&self.ui_font_size) {
            self.ui_font_size = crate::fonts::DEFAULT_UI_SIZE;
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
            ..Settings::default()
        };
        self.normalize();
    }

    pub fn save(&self) -> Res<()> {
        std::fs::create_dir_all(config_dir()).map_err(es)?;
        self.export_to(&config_file(), true)
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
        std::fs::write(path, json).map_err(es)
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
    fn quick_filter_migration() {
        // Older settings: the Data category without the new formats, plus a filter of your own.
        let old = r#"{"quick_filters":[{"name":"Data","exts":"parquet npy rds"},{"name":"Raw","exts":"cr2"}]}"#;
        let mut v: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(v.quick_filter_defaults, 0);
        v.normalize();
        assert_eq!(v.quick_filters[0].exts, "parquet npy rds safetensors onnx rdata rda");
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
        for e in ["safetensors", "onnx", "rdata", "rda", "rds"] {
            assert!(s.quick_filters.iter().any(|q| q.name == "Data" && q.ext_list().iter().any(|x| x == e)), "{e}");
        }
    }
}
