//! Keyboard shortcuts: every action, its default keys on each operating system, and
//! the changes made in Settings (only those are saved, so defaults can improve).
//!
//! The rule that keeps Vim, Emacs and other terminal programs working: while a terminal
//! has the keyboard, the app only takes key combinations a terminal program never gets
//! to see (see `Chord::leaves_terminals_alone`). Every other combination goes to the
//! program, even if it is bound here, and still works in file windows.
use crate::i18n::t;
use egui::{Key, Modifiers};
use std::collections::{BTreeMap, HashSet};
use std::sync::{LazyLock, Mutex, RwLock};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Mac,
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Os::Mac => "macOS",
            Os::Windows => "Windows",
            Os::Linux => "Linux",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    // Tabs
    NextTab,
    PrevTab,
    LastUsedTab,
    MoveTabLeft,
    MoveTabRight,
    /// 0 to 7: the first eight tabs.
    GoToTab(u8),
    GoToLastTab,
    NewTab,
    NewTerminalTab,
    CloseTab,
    ReopenTab,
    // Windows in a split tab
    /// 0 to 3: windows by their number.
    FocusWindow(u8),
    NextWindow,
    PrevWindow,
    CloseWindow,
    SplitRight,
    SplitLeft,
    SplitDown,
    SplitUp,
    SwapPanels,
    // Files
    View,
    OpenDefault,
    NewFile,
    NewFolder,
    Copy,
    Move,
    Rename,
    BatchRename,
    Delete,
    DeletePermanently,
    Compress,
    ExtractOther,
    Properties,
    Checksum,
    SelectAll,
    InvertSelection,
    // Going around
    Back,
    Forward,
    UpFolder,
    Filter,
    FocusPath,
    Bookmark,
    Refresh,
    ToggleHidden,
    // The app
    Palette,
    OpenTerminal,
    QuickView,
    Sidebar,
    Transfers,
    Connect,
    Backup,
    Settings,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Tabs,
    Windows,
    Files,
    Navigation,
    App,
}

pub const GROUPS: [Group; 5] = [Group::Tabs, Group::Windows, Group::Files, Group::Navigation, Group::App];

impl Group {
    pub fn label(self) -> String {
        match self {
            Group::Tabs => t("Tabs"),
            Group::Windows => t("Split windows"),
            Group::Files => t("Files"),
            Group::Navigation => t("Going around"),
            Group::App => t("App"),
        }
    }
}

struct Def {
    action: Action,
    /// Its name in the settings file: never change one.
    id: &'static str,
    group: Group,
    /// It can be used while a terminal has the keyboard.
    in_terminal: bool,
    windows: &'static [&'static str],
    linux: &'static [&'static str],
    mac: &'static [&'static str],
}

const fn def(action: Action, id: &'static str, group: Group, in_terminal: bool, windows: &'static [&'static str], linux: &'static [&'static str], mac: &'static [&'static str]) -> Def {
    Def { action, id, group, in_terminal, windows, linux, mac }
}

use Action as A;
use Group as G;

/// Every action, in the order Settings lists them. On Windows and Linux, what must work in
/// terminals has a `Ctrl+Shift` combination; on macOS, `Cmd` is enough. Left out on purpose:
/// `Alt+F4`, `Ctrl+Alt+…` (it is AltGr on many keyboards), `Cmd+H`, `Cmd+M`, `Cmd+Q`, `` Cmd+` ``.
const DEFS: &[Def] = &[
    def(A::NextTab, "next_tab", G::Tabs, true, &["Ctrl+Tab", "Ctrl+PageDown"], &["Ctrl+Tab", "Ctrl+PageDown"], &["Cmd+Shift+CloseBracket", "Ctrl+Tab"]),
    def(A::PrevTab, "previous_tab", G::Tabs, true, &["Ctrl+Shift+Tab", "Ctrl+PageUp"], &["Ctrl+Shift+Tab", "Ctrl+PageUp"], &["Cmd+Shift+OpenBracket", "Ctrl+Shift+Tab"]),
    def(A::LastUsedTab, "last_used_tab", G::Tabs, true, &["Ctrl+Shift+Backslash"], &["Ctrl+Shift+Backslash"], &["Cmd+Shift+Backslash"]),
    def(A::GoToTab(0), "tab_1", G::Tabs, true, &["Ctrl+Shift+1", "Alt+1"], &["Ctrl+Shift+1", "Alt+1"], &["Cmd+Alt+1"]),
    def(A::GoToTab(1), "tab_2", G::Tabs, true, &["Ctrl+Shift+2", "Alt+2"], &["Ctrl+Shift+2", "Alt+2"], &["Cmd+Alt+2"]),
    def(A::GoToTab(2), "tab_3", G::Tabs, true, &["Ctrl+Shift+3", "Alt+3"], &["Ctrl+Shift+3", "Alt+3"], &["Cmd+Alt+3"]),
    def(A::GoToTab(3), "tab_4", G::Tabs, true, &["Ctrl+Shift+4", "Alt+4"], &["Ctrl+Shift+4", "Alt+4"], &["Cmd+Alt+4"]),
    def(A::GoToTab(4), "tab_5", G::Tabs, true, &["Ctrl+Shift+5", "Alt+5"], &["Ctrl+Shift+5", "Alt+5"], &["Cmd+Alt+5"]),
    def(A::GoToTab(5), "tab_6", G::Tabs, true, &["Ctrl+Shift+6", "Alt+6"], &["Ctrl+Shift+6", "Alt+6"], &["Cmd+Alt+6"]),
    def(A::GoToTab(6), "tab_7", G::Tabs, true, &["Ctrl+Shift+7", "Alt+7"], &["Ctrl+Shift+7", "Alt+7"], &["Cmd+Alt+7"]),
    def(A::GoToTab(7), "tab_8", G::Tabs, true, &["Ctrl+Shift+8", "Alt+8"], &["Ctrl+Shift+8", "Alt+8"], &["Cmd+Alt+8"]),
    def(A::GoToLastTab, "last_tab", G::Tabs, true, &["Ctrl+Shift+9", "Alt+9"], &["Ctrl+Shift+9", "Alt+9"], &["Cmd+Alt+9"]),
    def(A::MoveTabLeft, "move_tab_left", G::Tabs, true, &["Ctrl+Shift+PageUp"], &["Ctrl+Shift+PageUp"], &["Cmd+Shift+PageUp"]),
    def(A::MoveTabRight, "move_tab_right", G::Tabs, true, &["Ctrl+Shift+PageDown"], &["Ctrl+Shift+PageDown"], &["Cmd+Shift+PageDown"]),
    def(A::NewTab, "new_tab", G::Tabs, true, &["Ctrl+T", "Ctrl+Shift+N"], &["Ctrl+T", "Ctrl+Shift+N"], &["Cmd+T"]),
    def(A::NewTerminalTab, "new_terminal_tab", G::Tabs, true, &["Ctrl+Backtick", "Ctrl+Shift+Backtick"], &["Ctrl+Backtick", "Ctrl+Shift+Backtick"], &["Cmd+Alt+T", "Ctrl+Backtick"]),
    def(A::CloseTab, "close_tab", G::Tabs, true, &["Ctrl+Shift+X", "Ctrl+F4"], &["Ctrl+Shift+X"], &["Cmd+Shift+W"]),
    def(A::ReopenTab, "reopen_tab", G::Tabs, true, &["Ctrl+Shift+T"], &["Ctrl+Shift+T"], &["Cmd+Shift+T"]),
    def(A::FocusWindow(0), "window_1", G::Windows, true, &["Ctrl+Shift+F1"], &["Ctrl+Shift+F1"], &["Cmd+1"]),
    def(A::FocusWindow(1), "window_2", G::Windows, true, &["Ctrl+Shift+F2"], &["Ctrl+Shift+F2"], &["Cmd+2"]),
    def(A::FocusWindow(2), "window_3", G::Windows, true, &["Ctrl+Shift+F3"], &["Ctrl+Shift+F3"], &["Cmd+3"]),
    def(A::FocusWindow(3), "window_4", G::Windows, true, &["Ctrl+Shift+F4"], &["Ctrl+Shift+F4"], &["Cmd+4"]),
    def(A::NextWindow, "next_window", G::Windows, true, &["Ctrl+Shift+CloseBracket"], &["Ctrl+Shift+CloseBracket"], &["Cmd+Alt+ArrowRight"]),
    def(A::PrevWindow, "previous_window", G::Windows, true, &["Ctrl+Shift+OpenBracket"], &["Ctrl+Shift+OpenBracket"], &["Cmd+Alt+ArrowLeft"]),
    def(A::CloseWindow, "close_window", G::Windows, true, &["Ctrl+W", "Ctrl+Shift+W"], &["Ctrl+W", "Ctrl+Shift+W"], &["Cmd+W"]),
    def(A::SplitRight, "split_right", G::Windows, true, &["Ctrl+Shift+O"], &["Ctrl+Shift+O"], &["Cmd+Backslash", "Ctrl+Cmd+ArrowRight"]),
    def(A::SplitDown, "split_down", G::Windows, true, &["Ctrl+Shift+E"], &["Ctrl+Shift+E"], &["Cmd+Alt+Backslash", "Ctrl+Cmd+ArrowDown"]),
    def(A::SplitLeft, "split_left", G::Windows, true, &["Ctrl+Shift+L"], &["Ctrl+Shift+L"], &["Ctrl+Cmd+ArrowLeft"]),
    def(A::SplitUp, "split_up", G::Windows, true, &["Ctrl+Shift+K"], &["Ctrl+Shift+K"], &["Ctrl+Cmd+ArrowUp"]),
    def(A::SwapPanels, "swap_panels", G::Windows, false, &["Ctrl+U"], &["Ctrl+U"], &["Cmd+U"]),
    def(A::View, "view", G::Files, false, &["F3"], &["F3"], &["F3"]),
    def(A::OpenDefault, "open_default", G::Files, false, &["F4"], &["F4"], &["F4"]),
    def(A::NewFile, "new_file", G::Files, false, &["Shift+F4"], &["Shift+F4"], &["Shift+F4"]),
    def(A::NewFolder, "new_folder", G::Files, false, &["F7"], &["F7"], &["F7", "Cmd+Shift+N"]),
    def(A::Copy, "copy", G::Files, false, &["F5"], &["F5"], &["F5"]),
    def(A::Move, "move", G::Files, false, &["F6"], &["F6"], &["F6"]),
    def(A::Rename, "rename", G::Files, false, &["F2", "Shift+F6"], &["F2", "Shift+F6"], &["F2", "Shift+F6"]),
    def(A::BatchRename, "batch_rename", G::Files, false, &["Ctrl+M"], &["Ctrl+M"], &["Cmd+Shift+R"]),
    def(A::Delete, "delete", G::Files, false, &["F8", "Delete"], &["F8", "Delete"], &["F8", "Delete", "Cmd+Backspace"]),
    def(A::DeletePermanently, "delete_permanently", G::Files, false, &["Shift+F8", "Shift+Delete"], &["Shift+F8", "Shift+Delete"], &["Shift+F8", "Shift+Delete", "Cmd+Alt+Backspace"]),
    def(A::Compress, "compress", G::Files, false, &["Alt+F5"], &["Alt+F5"], &["Alt+F5"]),
    def(A::ExtractOther, "extract", G::Files, false, &["Alt+F9"], &["Alt+F9"], &["Alt+F9"]),
    def(A::Properties, "properties", G::Files, false, &["Alt+Enter"], &["Alt+Enter"], &["Cmd+I", "Alt+Enter"]),
    def(A::Checksum, "checksum", G::Files, false, &["Ctrl+Shift+H"], &["Ctrl+Shift+H"], &["Cmd+Shift+H"]),
    def(A::SelectAll, "select_all", G::Files, false, &["Ctrl+A"], &["Ctrl+A"], &["Cmd+A"]),
    def(A::InvertSelection, "invert_selection", G::Files, false, &["Ctrl+I"], &["Ctrl+I"], &["Cmd+Shift+I"]),
    def(A::Back, "back", G::Navigation, false, &["Alt+ArrowLeft"], &["Alt+ArrowLeft"], &["Cmd+OpenBracket", "Alt+ArrowLeft"]),
    def(A::Forward, "forward", G::Navigation, false, &["Alt+ArrowRight"], &["Alt+ArrowRight"], &["Cmd+CloseBracket", "Alt+ArrowRight"]),
    def(A::UpFolder, "up_folder", G::Navigation, false, &["Backspace", "Alt+ArrowUp"], &["Backspace", "Alt+ArrowUp"], &["Backspace", "Cmd+ArrowUp"]),
    def(A::Filter, "filter", G::Navigation, false, &["Ctrl+F"], &["Ctrl+F"], &["Cmd+F"]),
    def(A::FocusPath, "focus_path", G::Navigation, false, &["Ctrl+L", "Alt+D"], &["Ctrl+L"], &["Cmd+L", "Cmd+Shift+G"]),
    def(A::Bookmark, "bookmark", G::Navigation, false, &["Ctrl+D"], &["Ctrl+D"], &["Cmd+D"]),
    def(A::Refresh, "refresh", G::Navigation, false, &["Ctrl+R"], &["Ctrl+R"], &["Cmd+R"]),
    def(A::ToggleHidden, "toggle_hidden", G::Navigation, false, &["Ctrl+H", "Ctrl+Shift+Period"], &["Ctrl+H", "Alt+Period"], &["Cmd+Shift+Period"]),
    def(A::Palette, "palette", G::App, true, &["Ctrl+K", "Ctrl+Shift+P"], &["Ctrl+K", "Ctrl+Shift+P"], &["Cmd+K", "Cmd+Shift+P"]),
    def(A::OpenTerminal, "open_terminal", G::App, false, &["F9"], &["F9"], &["F9"]),
    def(A::QuickView, "quick_view", G::App, false, &["Ctrl+Q"], &["Ctrl+Q"], &["Cmd+Y", "Ctrl+Q"]),
    def(A::Sidebar, "sidebar", G::App, false, &["Ctrl+Backslash"], &["Ctrl+Backslash"], &["Cmd+Alt+S"]),
    def(A::Transfers, "transfers", G::App, true, &["Ctrl+J", "Ctrl+Shift+J"], &["Ctrl+J", "Ctrl+Shift+J"], &["Cmd+J"]),
    def(A::Connect, "connect", G::App, false, &["Ctrl+N"], &["Ctrl+N"], &["Cmd+N"]),
    def(A::Backup, "backup", G::App, false, &["Ctrl+B"], &["Ctrl+B"], &["Cmd+B"]),
    def(A::Settings, "settings", G::App, true, &["Ctrl+Comma", "Ctrl+Shift+Comma"], &["Ctrl+Comma", "Ctrl+Shift+Comma"], &["Cmd+Comma"]),
    def(A::Help, "help", G::App, true, &["F1", "Ctrl+Shift+Slash"], &["F1", "Ctrl+Shift+Slash"], &["F1", "Cmd+Shift+Slash"]),
];

fn def_of(action: Action) -> &'static Def {
    DEFS.iter().find(|d| d.action == action).expect("every action has a definition")
}

impl Action {
    pub fn all() -> impl Iterator<Item = Action> {
        DEFS.iter().map(|d| d.action)
    }

    pub fn id(self) -> &'static str {
        def_of(self).id
    }

    pub fn group(self) -> Group {
        def_of(self).group
    }

    pub fn works_in_terminals(self) -> bool {
        def_of(self).in_terminal
    }

    /// It acts on a list of files, which disk usage and Settings windows do not have.
    pub fn needs_file_list(self) -> bool {
        matches!(
            self,
            A::View | A::OpenDefault | A::NewFile | A::NewFolder | A::Copy | A::Move | A::Rename | A::BatchRename | A::Delete | A::DeletePermanently | A::Compress | A::ExtractOther | A::Properties | A::Checksum | A::SelectAll | A::InvertSelection | A::Back | A::Forward | A::Filter | A::FocusPath | A::Bookmark | A::ToggleHidden
        )
    }

    pub fn label(self) -> String {
        match self {
            A::NextTab => t("Next tab"),
            A::PrevTab => t("Previous tab"),
            A::LastUsedTab => t("Last used tab"),
            A::MoveTabLeft => t("Move tab left"),
            A::MoveTabRight => t("Move tab right"),
            A::GoToTab(n) => format!("{} {}", t("Go to tab"), n + 1),
            A::GoToLastTab => t("Go to the last tab"),
            A::NewTab => t("New tab"),
            A::NewTerminalTab => t("New terminal tab"),
            A::CloseTab => t("Close tab"),
            A::ReopenTab => t("Reopen closed tab"),
            A::FocusWindow(n) => format!("{} {}", t("Go to window"), n + 1),
            A::NextWindow => t("Next window"),
            A::PrevWindow => t("Previous window"),
            A::CloseWindow => t("Close window"),
            A::SplitRight => t("Split right"),
            A::SplitLeft => t("Split left"),
            A::SplitDown => t("Split down"),
            A::SplitUp => t("Split up"),
            A::SwapPanels => t("Swap panels"),
            A::View => t("View"),
            A::OpenDefault => t("Open with default app"),
            A::NewFile => t("New file…"),
            A::NewFolder => t("New folder…"),
            A::Copy => t("Copy to other panel"),
            A::Move => t("Move to other panel"),
            A::Rename => t("Rename"),
            A::BatchRename => t("Batch rename…"),
            A::Delete => t("Delete"),
            A::DeletePermanently => t("Delete permanently"),
            A::Compress => t("Compress…"),
            A::ExtractOther => t("Extract to other panel"),
            A::Properties => t("Properties"),
            A::Checksum => t("Checksum…"),
            A::SelectAll => t("Select all"),
            A::InvertSelection => t("Invert selection"),
            A::Back => t("Back"),
            A::Forward => t("Forward"),
            A::UpFolder => t("Parent folder"),
            A::Filter => t("Focus filter"),
            A::FocusPath => t("Focus path"),
            A::Bookmark => t("Bookmark current folder"),
            A::Refresh => t("Refresh"),
            A::ToggleHidden => t("Show hidden files"),
            A::Palette => t("Search files, commands, settings…"),
            A::OpenTerminal => t("Open terminal here"),
            A::QuickView => t("Quick view panel"),
            A::Sidebar => t("Sidebar"),
            A::Transfers => t("Transfers"),
            A::Connect => t("Connect to server…"),
            A::Backup => t("Backup…"),
            A::Settings => t("Settings…"),
            A::Help => t("Keyboard shortcuts"),
        }
    }
}

/// One key combination.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Command key of macOS.
    pub cmd: bool,
    pub key: Key,
}

fn is_digit(key: Key) -> bool {
    matches!(key, Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9)
}

fn is_letter(key: Key) -> bool {
    key.name().len() == 1 && key.name().as_bytes()[0].is_ascii_uppercase()
}

fn is_function_key(key: Key) -> bool {
    let name = key.name();
    name.len() >= 2 && name.starts_with('F') && name[1..].bytes().all(|b| b.is_ascii_digit())
}

impl Chord {
    /// `Ctrl+Shift+T`, as saved in the settings file.
    pub fn parse(text: &str) -> Option<Self> {
        let mut chord = Chord { ctrl: false, alt: false, shift: false, cmd: false, key: Key::A };
        let mut key = None;
        for part in text.split('+') {
            match part {
                "Ctrl" => chord.ctrl = true,
                "Alt" => chord.alt = true,
                "Shift" => chord.shift = true,
                "Cmd" => chord.cmd = true,
                name => key = Some(Key::from_name(name)?),
            }
        }
        chord.key = key?;
        Some(chord)
    }

    pub fn to_saved(self) -> String {
        let mut parts = vec![];
        for (on, name) in [(self.ctrl, "Ctrl"), (self.cmd, "Cmd"), (self.alt, "Alt"), (self.shift, "Shift")] {
            if on {
                parts.push(name);
            }
        }
        parts.push(self.key.name());
        parts.join("+")
    }

    /// How to show it on this operating system.
    pub fn display(self, os: Os) -> String {
        let key = match self.key {
            Key::ArrowLeft => "←",
            Key::ArrowRight => "→",
            Key::ArrowUp => "↑",
            Key::ArrowDown => "↓",
            Key::PageUp => "PgUp",
            Key::PageDown => "PgDn",
            Key::Backspace if os == Os::Mac => "⌫",
            Key::Delete if os != Os::Mac => "Del",
            Key::Delete => "⌦",
            Key::Enter if os == Os::Mac => "↩",
            Key::Tab if os == Os::Mac => "⇥",
            k => k.symbol_or_name(),
        };
        if os == Os::Mac {
            let mut s = String::new();
            for (on, sign) in [(self.ctrl, "⌃"), (self.alt, "⌥"), (self.shift, "⇧"), (self.cmd, "⌘")] {
                if on {
                    s.push_str(sign);
                }
            }
            s + key
        } else {
            let mut parts = vec![];
            for (on, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift"), (self.cmd, "Win")] {
                if on {
                    parts.push(name);
                }
            }
            parts.push(key);
            parts.join("+")
        }
    }

    /// Whether a terminal program never sees this combination. `Cmd` is not sent to
    /// terminals at all. `Ctrl+Shift+letter` is the same byte as `Ctrl+letter` to a
    /// program, so none can bind it apart from the key without Shift, which is left for it.
    /// `Ctrl+Tab` has no code in a terminal. Everything else belongs to the program:
    /// `Ctrl+W` (Vim's windows), `Alt+1` (Emacs's numeric argument), `Ctrl+PageUp` (Vim's tabs)…
    pub fn leaves_terminals_alone(self) -> bool {
        self.cmd || (self.ctrl && self.shift) || (self.ctrl && !self.alt && self.key == Key::Tab)
    }

    /// Whether a key press is this combination.
    fn matches(self, key: Key, physical: Option<Key>, m: Modifiers) -> bool {
        if m.ctrl != self.ctrl || m.alt != self.alt || m.shift != self.shift || m.mac_cmd != self.cmd {
            return false;
        }
        // Digit and punctuation keys report what they type: `!` for Shift+1, `[` for Option+5
        // on a German Mac, `-` for 6 on a French one. Their place on the keyboard is what counts
        // then. Not for letters: they move around between layouts, and Ctrl+Z must be the key
        // that says Z.
        key == self.key || (!is_letter(self.key) && (self.shift || self.alt || is_digit(self.key)) && physical == Some(self.key))
    }

    /// Why this cannot be a shortcut, if so.
    pub fn refused_because(self) -> Option<String> {
        let plain = !self.ctrl && !self.alt && !self.cmd;
        if matches!(self.key, Key::Escape) {
            return Some(t("Esc always cancels."));
        }
        if plain && !self.shift && matches!(self.key, Key::Tab | Key::Enter | Key::Space | Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::PageUp | Key::PageDown | Key::Home | Key::End) {
            return Some(t("This key moves around the file list."));
        }
        if plain && !is_function_key(self.key) && !matches!(self.key, Key::Delete | Key::Insert | Key::Backspace) {
            return Some(t("Add Ctrl, Alt or Cmd: this key alone types text."));
        }
        None
    }
}

/// The keys of all actions.
#[derive(Clone, Debug)]
pub struct Shortcuts {
    os: Os,
    bindings: Vec<(Action, Vec<Chord>)>,
}

fn parse_all(texts: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<Chord> {
    texts.into_iter().filter_map(|t| Chord::parse(t.as_ref())).collect()
}

/// The default keys of an action on an operating system.
pub fn defaults(action: Action, os: Os) -> Vec<Chord> {
    let d = def_of(action);
    parse_all(match os {
        Os::Mac => d.mac,
        Os::Windows => d.windows,
        Os::Linux => d.linux,
    }.iter())
}

impl Shortcuts {
    /// `changes` holds, per action id, the keys that replace its defaults.
    pub fn new(os: Os, changes: &BTreeMap<String, Vec<String>>) -> Self {
        let bindings = Action::all()
            .map(|a| {
                let keys = match changes.get(a.id()) {
                    Some(saved) => parse_all(saved),
                    None => defaults(a, os),
                };
                (a, keys)
            })
            .collect();
        Self { os, bindings }
    }

    pub fn keys(&self, action: Action) -> &[Chord] {
        self.bindings.iter().find(|(a, _)| *a == action).map_or(&[], |(_, k)| k)
    }

    /// The action of a key press. In a terminal, only actions that work there, and only
    /// by combinations that terminal programs never see.
    pub fn action_for(&self, key: Key, physical: Option<Key>, m: Modifiers, in_terminal: bool) -> Option<Action> {
        self.bindings.iter().find_map(|(action, keys)| {
            if in_terminal && !action.works_in_terminals() {
                return None;
            }
            keys.iter().any(|c| (!in_terminal || c.leaves_terminals_alone()) && c.matches(key, physical, m)).then_some(*action)
        })
    }

    pub fn os(&self) -> Os {
        self.os
    }

    /// Who else has this combination, apart from `action`.
    pub fn used_by(&self, chord: Chord, action: Action) -> Option<Action> {
        self.bindings.iter().find(|(a, keys)| *a != action && keys.contains(&chord)).map(|(a, _)| *a)
    }

    pub fn display_first(&self, action: Action, terminal: bool) -> String {
        let keys = self.keys(action);
        let chord = if terminal { keys.iter().find(|c| c.leaves_terminals_alone()) } else { keys.first() };
        chord.map_or_else(String::new, |c| c.display(self.os))
    }
}

/// The new settings entry after an action's keys changed: no entry if back at the defaults.
pub fn store(changes: &mut BTreeMap<String, Vec<String>>, os: Os, action: Action, keys: &[Chord]) {
    if keys == defaults(action, os) {
        changes.remove(action.id());
    } else {
        changes.insert(action.id().to_string(), keys.iter().map(|c| c.to_saved()).collect());
    }
}

// ---------------------------------------------------------------- the shortcuts in use

static ACTIVE: LazyLock<RwLock<Shortcuts>> = LazyLock::new(|| RwLock::new(Shortcuts::new(Os::current(), &BTreeMap::new())));

/// Put these keys in use everywhere: key handling, menus, tooltips and the palette.
pub fn install(changes: &BTreeMap<String, Vec<String>>) {
    if let Ok(mut active) = ACTIVE.write() {
        *active = Shortcuts::new(Os::current(), changes);
    }
}

pub fn active() -> Shortcuts {
    ACTIVE.read().map(|a| a.clone()).unwrap_or_else(|_| Shortcuts::new(Os::current(), &BTreeMap::new()))
}

/// Text that lasts: menus want a plain `&str`, and only a few different ones ever exist.
fn lasting(text: String) -> &'static str {
    static TEXTS: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(Default::default);
    let Ok(mut texts) = TEXTS.lock() else { return "" };
    match texts.get(text.as_str()) {
        Some(known) => known,
        None => {
            let known: &'static str = Box::leak(text.into_boxed_str());
            texts.insert(known);
            known
        }
    }
}

/// The keys to show next to an action in a menu or tooltip.
pub fn hint(action: Action) -> &'static str {
    lasting(active().display_first(action, false))
}

/// The same, for a menu that shows while a terminal has the keyboard.
pub fn terminal_hint(action: Action) -> &'static str {
    lasting(active().display_first(action, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OSES: [Os; 3] = [Os::Mac, Os::Windows, Os::Linux];

    fn defaults_only(os: Os) -> Shortcuts {
        Shortcuts::new(os, &BTreeMap::new())
    }

    fn mods(ctrl: bool, alt: bool, shift: bool, cmd: bool) -> Modifiers {
        Modifiers { ctrl, alt, shift, mac_cmd: cmd, command: if cfg!(target_os = "macos") { cmd } else { ctrl } }
    }

    fn all_keys() -> Vec<Key> {
        Key::ALL.to_vec()
    }

    #[test]
    fn every_default_is_a_real_combination() {
        for d in DEFS {
            for (os, list) in [(Os::Windows, d.windows), (Os::Linux, d.linux), (Os::Mac, d.mac)] {
                for text in list {
                    let chord = Chord::parse(text).unwrap_or_else(|| panic!("{os:?} {}: {text}", d.id));
                    assert_eq!(Chord::parse(&chord.to_saved()), Some(chord), "{text} reads back as saved");
                }
                assert!(!list.is_empty(), "{} has keys on {os:?}", d.id);
            }
        }
        let mut ids: Vec<&str> = DEFS.iter().map(|d| d.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), DEFS.len(), "unique ids");
    }

    #[test]
    fn no_default_is_used_twice() {
        for os in OSES {
            let s = defaults_only(os);
            for a in Action::all() {
                for c in defaults(a, os) {
                    assert_eq!(s.used_by(c, a), None, "{os:?}: {} is on {a:?} too", c.to_saved());
                }
            }
        }
    }

    #[test]
    fn each_system_has_its_own_set() {
        let keys = |os| Action::all().map(|a| defaults(a, os)).collect::<Vec<_>>();
        assert_ne!(keys(Os::Mac), keys(Os::Windows));
        assert_ne!(keys(Os::Mac), keys(Os::Linux));
        assert_ne!(keys(Os::Windows), keys(Os::Linux));
        // macOS uses Command: no action that has a Control default lacks a Cmd or function key.
        for d in DEFS {
            let chord = Chord::parse(d.mac[0]).unwrap();
            assert!(chord.cmd || !chord.ctrl, "{}: {}", d.id, d.mac[0]);
        }
    }

    #[test]
    fn reserved_system_keys_are_left_alone() {
        let reserved: [(Os, &[&str]); 3] = [
            (Os::Mac, &["Cmd+H", "Cmd+M", "Cmd+Q", "Cmd+Tab", "Cmd+Backtick", "Cmd+Space", "Cmd+Shift+3", "Cmd+Shift+4", "Cmd+Shift+5", "Cmd+Alt+Escape", "Cmd+Shift+Q"]),
            (Os::Windows, &["Alt+F4", "Alt+Tab", "Ctrl+Alt+Delete", "Alt+Space"]),
            (Os::Linux, &["Alt+F4", "Alt+Tab", "Ctrl+Alt+T", "Alt+F2", "Ctrl+Alt+ArrowLeft", "Ctrl+Alt+ArrowRight", "Alt+Space"]),
        ];
        for (os, list) in reserved {
            let s = defaults_only(os);
            for text in list {
                let c = Chord::parse(text).unwrap();
                assert_eq!(s.used_by(c, A::Help).filter(|a| *a != A::Help), None, "{os:?}: {text}");
            }
        }
    }

    /// The reason this module exists: everything Vim and Emacs bind reaches them.
    #[test]
    fn terminal_programs_keep_their_keys() {
        for os in OSES {
            let s = defaults_only(os);
            for key in all_keys() {
                for m in [
                    mods(true, false, false, false),  // Vim's Ctrl+W, Ctrl+T, Ctrl+V, Ctrl+], Ctrl+6…, Emacs's C-x, C-g, C-h…
                    mods(false, true, false, false),  // Emacs's M-x, M-1 (Alt on most keyboards)
                    mods(false, true, true, false),   // Emacs's M-!, M-<, M->
                    mods(true, true, false, false),   // Emacs's C-M-…, and AltGr
                    mods(false, false, true, false),  // Shifted keys and text
                    mods(false, false, false, false), // Plain keys: Esc, F-keys…
                ] {
                    if let Some(a) = s.action_for(key, None, m, true) {
                        assert!(key == Key::Tab && m.ctrl && !m.alt, "{os:?}: {} goes to {a:?}", key.name());
                    }
                }
            }
        }
    }

    #[test]
    fn terminal_tabs_and_windows_can_still_be_reached() {
        for os in OSES {
            let s = defaults_only(os);
            for d in DEFS.iter().filter(|d| d.in_terminal) {
                assert!(defaults(d.action, os).iter().any(|c| c.leaves_terminals_alone()), "{os:?}: {} has no key that works in a terminal", d.id);
            }
            for a in [A::NextTab, A::PrevTab, A::GoToTab(0), A::GoToTab(7), A::GoToLastTab, A::LastUsedTab, A::FocusWindow(0), A::FocusWindow(3), A::NextWindow, A::PrevWindow] {
                let c = *defaults(a, os).iter().find(|c| c.leaves_terminals_alone()).unwrap();
                assert_eq!(s.action_for(c.key, None, mods(c.ctrl, c.alt, c.shift, c.cmd), true), Some(a), "{os:?}");
            }
        }
    }

    #[test]
    fn file_only_keys_stay_out_of_terminals() {
        let s = defaults_only(Os::Linux);
        // Ctrl+W closes a window among files, and is Vim's in a terminal.
        assert_eq!(s.action_for(Key::W, None, mods(true, false, false, false), false), Some(A::CloseWindow));
        assert_eq!(s.action_for(Key::W, None, mods(true, false, false, false), true), None);
        assert_eq!(s.action_for(Key::W, None, mods(true, false, true, false), true), Some(A::CloseWindow));
        // An action that is not for terminals, even by a safe combination.
        let mut changes = BTreeMap::new();
        changes.insert("copy".to_string(), vec!["Ctrl+Shift+Y".to_string()]);
        let s = Shortcuts::new(Os::Linux, &changes);
        assert_eq!(s.action_for(Key::Y, None, mods(true, false, true, false), false), Some(A::Copy));
        assert_eq!(s.action_for(Key::Y, None, mods(true, false, true, false), true), None);
    }

    #[test]
    fn shifted_digits_match_by_their_place_and_letters_by_their_name() {
        let s = defaults_only(Os::Linux);
        // Ctrl+Shift+1 usually arrives as `!`, on the 1 key.
        let m = mods(true, false, true, false);
        assert_eq!(s.action_for(Key::Exclamationmark, Some(Key::Num1), m, true), Some(A::GoToTab(0)));
        assert_eq!(s.action_for(Key::Num3, Some(Key::Num3), m, true), Some(A::GoToTab(2)));
        // On a French keyboard the key in Q's place says A: it is A.
        let ctrl = mods(true, false, false, false);
        assert_eq!(s.action_for(Key::A, Some(Key::Q), ctrl, false), Some(A::SelectAll));
        assert_eq!(s.action_for(Key::Y, Some(Key::T), mods(true, false, true, false), false), None);
    }

    #[test]
    fn on_macos_the_numbers_go_to_windows_and_with_option_to_tabs() {
        let s = defaults_only(Os::Mac);
        let cmd = mods(false, false, false, true);
        let cmd_option = mods(false, true, false, true);
        assert_eq!(s.action_for(Key::Num1, Some(Key::Num1), cmd, false), Some(A::FocusWindow(0)));
        assert_eq!(s.action_for(Key::Num4, Some(Key::Num4), cmd, true), Some(A::FocusWindow(3)), "in a terminal too");
        assert_eq!(s.action_for(Key::Num5, Some(Key::Num5), cmd, false), None, "four windows at most");
        assert_eq!(s.action_for(Key::Num1, Some(Key::Num1), cmd_option, false), Some(A::GoToTab(0)));
        assert_eq!(s.action_for(Key::Num8, Some(Key::Num8), cmd_option, true), Some(A::GoToTab(7)), "in a terminal too");
        assert_eq!(s.action_for(Key::Num9, Some(Key::Num9), cmd_option, false), Some(A::GoToLastTab));
    }

    #[test]
    fn splitting_and_closing_work_everywhere_on_every_system() {
        for os in OSES {
            let s = defaults_only(os);
            for a in [A::SplitRight, A::SplitLeft, A::SplitDown, A::SplitUp, A::CloseWindow, A::CloseTab] {
                let c = *defaults(a, os).iter().find(|c| c.leaves_terminals_alone()).expect("a key that works in a terminal");
                assert_eq!(s.action_for(c.key, None, mods(c.ctrl, c.alt, c.shift, c.cmd), true), Some(a), "{os:?}");
            }
        }
    }

    #[test]
    fn digits_are_found_whatever_the_layout_makes_them_type() {
        let s = defaults_only(Os::Mac);
        // Option+5 types `[` on a German Mac.
        assert_eq!(s.action_for(Key::OpenBracket, Some(Key::Num5), mods(false, true, false, true), false), Some(A::GoToTab(4)));
        // The 6 key types `-` on a French Mac.
        let mut changes = BTreeMap::new();
        changes.insert("window_1".to_string(), vec!["Cmd+6".to_string()]);
        let s = Shortcuts::new(Os::Mac, &changes);
        assert_eq!(s.action_for(Key::Minus, Some(Key::Num6), mods(false, false, false, true), false), Some(A::FocusWindow(0)));
    }

    #[test]
    fn changes_replace_defaults_and_are_only_saved_when_they_differ() {
        let mut changes = BTreeMap::new();
        let new = [Chord::parse("Ctrl+Shift+G").unwrap()];
        store(&mut changes, Os::Linux, A::Filter, &new);
        assert_eq!(changes["filter"], ["Ctrl+Shift+G"]);
        let s = Shortcuts::new(Os::Linux, &changes);
        assert_eq!(s.keys(A::Filter), new);
        assert_eq!(s.action_for(Key::F, None, mods(true, false, false, false), false), None, "the old key is free");
        store(&mut changes, Os::Linux, A::Filter, &defaults(A::Filter, Os::Linux));
        assert!(changes.is_empty(), "back at the defaults");
        // No keys at all is a choice too, and bad text in the file is skipped.
        store(&mut changes, Os::Linux, A::Filter, &[]);
        assert_eq!(Shortcuts::new(Os::Linux, &changes).keys(A::Filter), []);
        changes.insert("view".into(), vec!["Nonsense+Key".into(), "F12".into()]);
        assert_eq!(Shortcuts::new(Os::Linux, &changes).keys(A::View).len(), 1);
    }

    #[test]
    fn unusable_combinations_are_explained() {
        for text in ["A", "Shift+A", "1", "Tab", "Enter", "ArrowUp", "Space", "Escape", "Ctrl+Escape"] {
            assert!(Chord::parse(text).unwrap().refused_because().is_some(), "{text}");
        }
        for text in ["F5", "Shift+F5", "Delete", "Ctrl+A", "Alt+A", "Cmd+A", "Ctrl+Tab", "Alt+Enter", "Backspace"] {
            assert_eq!(Chord::parse(text).unwrap().refused_because(), None, "{text}");
        }
    }

    /// An empty box instead of a key's symbol would make the shortcut unreadable.
    #[test]
    fn every_shortcut_can_be_drawn() {
        let ctx = egui::Context::default();
        crate::fonts::apply(&ctx, &crate::fonts::FontPrefs { ui_font: String::new(), ui_size: crate::fonts::DEFAULT_UI_SIZE, term_font: String::new() });
        let _ = ctx.run(Default::default(), |_| {});
        let bold = egui::FontFamily::Name("bold".into());
        let mut texts: Vec<String> = vec![];
        for os in OSES {
            for a in Action::all() {
                texts.extend(defaults(a, os).iter().map(|c| c.display(os)));
            }
            // And any key someone may choose.
            for key in all_keys() {
                let all = Chord { ctrl: true, alt: true, shift: true, cmd: false, key };
                texts.push(all.display(os));
                texts.push(Chord { cmd: true, ..all }.display(os));
            }
        }
        ctx.fonts(|f| {
            for text in &texts {
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace, bold.clone()] {
                    assert!(f.has_glyphs(&egui::FontId::new(14.0, family.clone()), text), "{text} in {family:?}");
                }
            }
        });
    }

    #[test]
    fn shown_the_way_each_system_writes_them() {
        let c = Chord::parse("Ctrl+Shift+T").unwrap();
        assert_eq!(c.display(Os::Linux), "Ctrl+Shift+T");
        assert_eq!(Chord::parse("Cmd+Alt+1").unwrap().display(Os::Mac), "⌥⌘1");
        assert_eq!(Chord::parse("Cmd+Shift+T").unwrap().display(Os::Mac), "⇧⌘T");
        assert_eq!(Chord::parse("Alt+ArrowLeft").unwrap().display(Os::Windows), "Alt+←");
        assert_eq!(Chord::parse("Ctrl+PageUp").unwrap().display(Os::Windows), "Ctrl+PgUp");
    }
}
