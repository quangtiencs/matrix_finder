//! A file panel: tabs, listing, sorting, filtering, selection and rendering.
use crate::files::archive::{self, ArchEntry};
use crate::files::remote::SharedRemote;
use crate::files::vfs::{list_local, Entry, Location};
use crate::i18n::t;
use crate::style::icons;
use crate::style::theme::Theme;
use crate::ui::filter::{Filter, Find, FindMode};
use crate::ui::layout::SplitDir;
use crate::util::{format_time, human_size, natural_cmp, Res};
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Ext,
    Size,
    Modified,
}

enum Loaded {
    Entries(Vec<Entry>),
    Archive(PathBuf, Arc<Vec<ArchEntry>>),
}

/// A recursive search below the tab's folder; results replace the listing.
pub struct Search {
    /// What it looks for, as shown on the address bar's chip.
    pub query: String,
    filter: Filter,
    pub running: bool,
    pub truncated: bool,
    /// Stop was pressed: the results so far stay.
    pub stopped: bool,
    pub progress: Arc<crate::ui::search::Progress>,
    pub started: std::time::Instant,
    /// How long it took, once it ended.
    pub took: Option<std::time::Duration>,
    rx: Receiver<Vec<Entry>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// A re-run that was stopped: what it still sends is dropped, and the complete
    /// results of the run before stay.
    discard: bool,
    /// A re-run (after a reload): its results, swapped in when it ends, so the list
    /// does not empty and the cursor and marks stay.
    pending: Option<Vec<Entry>>,
}

impl Search {
    fn start(query: String, root: PathBuf, filter: Filter, show_hidden: bool, pending: Option<Vec<Entry>>) -> Self {
        let r = crate::ui::search::spawn_search(root, filter.clone(), show_hidden);
        Search {
            query,
            filter,
            running: true,
            truncated: false,
            stopped: false,
            progress: r.progress,
            started: r.started,
            took: None,
            rx: r.rx,
            cancel: r.cancel,
            discard: false,
            pending,
        }
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

use crate::ui::search::SEARCH_LIMIT;
use crate::ui::KeptContextMenu as _;

/// Listing a folder that takes this long stops it from reloading by itself.
const SLOW_LISTING: std::time::Duration = std::time::Duration::from_millis(500);

/// A filter that reads files (`source LIKE …`), run over the listing in the background.
/// The names that pass arrive in batches; until then they are not shown.
pub struct ContentScan {
    pub running: bool,
    /// Stop was pressed: only the files read so far are shown.
    pub stopped: bool,
    pub progress: Arc<crate::ui::search::Progress>,
    pub started: std::time::Instant,
    hits: HashSet<String>,
    rx: Receiver<Vec<String>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for ContentScan {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub struct Tab {
    pub loc: Location,
    pub entries: Vec<Entry>,
    pub view: Vec<usize>,
    pub cursor: usize,
    pub marked: HashSet<String>,
    /// Shift selection: the entry it started from (by name, so it survives re-sorting).
    anchor: Option<String>,
    /// The marks the current Shift range added (it can shrink again).
    range_marks: HashSet<String>,
    pub sort: SortKey,
    pub desc: bool,
    pub filter_text: String,
    pub filter: Result<Filter, String>,
    pub back: Vec<Location>,
    pub fwd: Vec<Location>,
    pub path_text: String,
    loading: Option<Receiver<Res<Loaded>>>,
    pub error: Option<String>,
    archive_cache: Option<(PathBuf, Arc<Vec<ArchEntry>>)>,
    scroll_offset: f32,
    view_height: f32,
    pub ensure_visible: bool,
    /// A right-click menu of the file list was open in the last frame: the Esc that
    /// closes it is not for the list (which would unmark the items).
    pub menu_open: bool,
    select_name: Option<String>,
    /// The Find panel's choices (kept while the window lives), and a request to open it
    /// (Ctrl+F) with the keyboard in its text field.
    pub find_draft: Find,
    pub open_find: bool,
    /// A stopped reading of files put aside while another filter is borrowed (the
    /// search box, the palette), with the filter text it belongs to.
    parked_scan: Option<(String, ContentScan)>,
    /// The view this window was switched to from the toolbar; `None` follows Settings.
    pub view_mode: Option<crate::config::ViewMode>,
    /// Icons per line while the icon view is drawn (`None`: the list): the arrow keys
    /// move by it.
    pub grid_cols: Option<usize>,
    /// The folder's modification time when it was last listed (local folders), which
    /// `files::dirwatch` compares to notice files added, removed or renamed.
    pub dir_stamp: Option<std::time::SystemTime>,
    /// The stamp was taken so soon after the folder changed that a disk with coarse
    /// times (FAT: 2 s, HFS+: 1 s) could still change without changing it.
    stamp_settled: bool,
    /// The folder changed on disk and reloads once nothing is in the way (a menu open).
    pub refresh_due: bool,
    /// How long the last listing of this local folder took, and when it last reloaded
    /// by itself: a slow folder reloads less often (`may_refresh_quietly`).
    list_cost: std::time::Duration,
    quiet_at: Option<std::time::Instant>,
    /// The entry the open right-click menu is about, by name (rows can move under it).
    menu_target: Option<String>,
    /// Files may have changed while this tab was hidden (a copy or compression finished):
    /// it reloads when it is shown again.
    pub stale: bool,
    /// "Calculate folder sizes" at work on this listing.
    pub dir_sizes: Option<crate::tools::diskusage::FolderSizes>,
    /// Recursive search results shown instead of the folder.
    pub search: Option<Search>,
    /// The filter reads files: which entries it let through so far.
    pub scan: Option<ContentScan>,
    pub focus_path: bool,
    pub editing_path: bool,
    /// Folder suggestions while the address is typed.
    pub complete: crate::ui::completion::Completer,
    /// One-off scroll request (auto-scroll while dragging).
    force_scroll: Option<f32>,
    /// Spring-loaded folder: (folder name, hover start time).
    spring: Option<(String, f64)>,
    /// Stable identity (survives moves between windows and tabs).
    pub uid: u64,
    /// Fixed name (e.g. the terminal profile it was started with).
    pub custom_title: Option<String>,
    /// Built-in terminal shown instead of the file list.
    pub term: Option<Box<crate::terminal::view::TermSession>>,
    /// Disk usage analysis shown instead of the file list.
    pub du: Option<Box<crate::tools::diskusage::DiskUsage>>,
    /// The Settings window (its pages are drawn by the app).
    pub settings: bool,
    /// The Transfers window (its content is drawn by the app).
    pub transfers: bool,
    /// The Checksum tool shown instead of the file list (drawn by the app).
    pub checksum: Option<Box<crate::tools::checksum::ChecksumUi>>,
    /// The Batch rename tool shown instead of the file list (drawn by the app).
    pub renamer: Option<Box<crate::tools::rename::BatchRename>>,
    /// The address bar's folder tree, while it is open.
    pub tree: Option<Box<crate::ui::foldertree::FolderTree>>,
    /// Quick filter by file category; unlike the text filter it stays on while
    /// browsing other folders, and folders stay visible.
    pub quick: Option<QuickActive>,
}

/// The quick filter a window has on: one or more categories, ticked in its menu.
#[derive(Clone, Debug)]
pub struct QuickActive {
    /// The categories ticked, in the order of the list in Settings.
    pub names: Vec<String>,
    /// Their extensions together.
    pub exts: HashSet<String>,
}

impl QuickActive {
    /// The categories called `names` that are in `all` (the list in Settings); None when
    /// there are none.
    pub fn from_names(names: &[String], all: &[crate::config::QuickFilter]) -> Option<Self> {
        // Each name once: the first list of that name.
        let mut on: Vec<&crate::config::QuickFilter> = Vec::new();
        for q in all.iter().filter(|q| names.contains(&q.name)) {
            if !on.iter().any(|o| o.name == q.name) {
                on.push(q);
            }
        }
        if on.is_empty() {
            return None;
        }
        Some(QuickActive { names: on.iter().map(|q| q.name.clone()).collect(), exts: on.iter().flat_map(|q| q.ext_list()).collect() })
    }

    /// `cur` with category `name` ticked or unticked; None when none is left.
    pub fn toggle(cur: Option<&QuickActive>, name: &str, all: &[crate::config::QuickFilter]) -> Option<Self> {
        let mut names = cur.map(|q| q.names.clone()).unwrap_or_default();
        match names.iter().position(|n| n == name) {
            Some(i) => {
                names.remove(i);
            }
            None => names.push(name.to_string()),
        }
        Self::from_names(&names, all)
    }

    pub fn has(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    /// "Images", "Images, Videos", or the first two and how many more.
    pub fn label(&self) -> String {
        let shown: Vec<String> = self.names.iter().take(2).map(|n| crate::config::quick_filter_name(n)).collect();
        match self.names.len() {
            0..=2 => shown.join(", "),
            n => format!("{} +{}", shown.join(", "), n - 2),
        }
    }

    /// The first category and how many more ("Images +2"): for a narrow chip.
    pub fn short_label(&self) -> String {
        match self.names.len() {
            0 | 1 => self.names.first().map(|n| crate::config::quick_filter_name(n)).unwrap_or_default(),
            n => format!("{} +{}", crate::config::quick_filter_name(&self.names[0]), n - 1),
        }
    }

    /// Folders always pass, so you can keep browsing.
    pub fn matches(&self, e: &Entry) -> bool {
        e.is_dir || self.exts.contains(&e.ext)
    }
}

#[derive(Clone)]
pub struct ListCtx<'a> {
    pub remotes: &'a HashMap<u64, (String, SharedRemote)>,
    pub show_hidden: bool,
    pub dirs_first: bool,
    /// Settings' view for windows that were not switched.
    pub default_view: crate::config::ViewMode,
}

impl Tab {
    pub fn new(loc: Location) -> Self {
        Self {
            loc,
            entries: vec![],
            view: vec![],
            cursor: 0,
            marked: HashSet::new(),
            anchor: None,
            range_marks: HashSet::new(),
            sort: SortKey::Name,
            desc: false,
            filter_text: String::new(),
            filter: Ok(Filter::All),
            back: vec![],
            fwd: vec![],
            path_text: String::new(),
            loading: None,
            error: None,
            archive_cache: None,
            scroll_offset: 0.0,
            view_height: 400.0,
            ensure_visible: true,
            menu_open: false,
            select_name: None,
            stale: false,
            dir_stamp: None,
            stamp_settled: true,
            refresh_due: false,
            list_cost: std::time::Duration::ZERO,
            quiet_at: None,
            menu_target: None,
            view_mode: None,
            grid_cols: None,
            find_draft: Find::default(),
            open_find: false,
            parked_scan: None,
            dir_sizes: None,
            search: None,
            scan: None,
            focus_path: false,
            editing_path: false,
            complete: Default::default(),
            force_scroll: None,
            spring: None,
            uid: crate::ui::tabs::next_uid(),
            custom_title: None,
            term: None,
            du: None,
            settings: false,
            transfers: false,
            checksum: None,
            renamer: None,
            tree: None,
            quick: None,
        }
    }

    /// The Settings window.
    pub fn settings() -> Self {
        let mut t = Self::new(Location::Local(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))));
        t.settings = true;
        t
    }

    /// The Transfers window.
    pub fn transfers() -> Self {
        let mut t = Self::new(Location::Local(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))));
        t.transfers = true;
        t
    }

    /// A plain file view (not a terminal, disk usage, settings, transfers or tool window).
    pub fn is_files(&self) -> bool {
        self.term.is_none() && self.du.is_none() && !self.settings && !self.transfers && !self.is_tool()
    }

    /// The Checksum tool, in a window of its own working in `dir`.
    pub fn checksum_tool(tool: crate::tools::checksum::ChecksumUi, dir: PathBuf) -> Self {
        let mut t = Self::new(Location::Local(dir));
        t.checksum = Some(Box::new(tool));
        t
    }

    /// The Batch rename tool, in a window of its own renaming in `loc`.
    pub fn rename_tool(tool: crate::tools::rename::BatchRename, loc: Location) -> Self {
        let mut t = Self::new(loc);
        t.renamer = Some(Box::new(tool));
        t
    }

    /// A Checksum or Batch rename window.
    pub fn is_tool(&self) -> bool {
        self.checksum.is_some() || self.renamer.is_some()
    }

    /// A disk usage window for `dir` (scans `depth` levels).
    pub fn disk_usage(dir: PathBuf, depth: usize) -> Self {
        let mut t = Self::new(Location::Local(dir.clone()));
        t.du = Some(Box::new(crate::tools::diskusage::DiskUsage::new(dir, depth)));
        t
    }

    pub fn is_disk_usage(&self) -> bool {
        self.du.is_some()
    }

    /// A terminal tab (its location follows the shell's folder for F5/F6 targets).
    pub fn terminal(session: crate::terminal::view::TermSession) -> Self {
        let mut t = Self::new(Location::Local(session.cwd.clone()));
        t.term = Some(Box::new(session));
        t
    }

    pub fn is_terminal(&self) -> bool {
        self.term.is_some()
    }

    pub fn is_loading(&self) -> bool {
        self.loading.is_some() || self.search.as_ref().is_some_and(|s| s.running)
    }

    /// The filter is still reading files.
    pub fn is_scanning(&self) -> bool {
        self.scan.as_ref().is_some_and(|s| s.running)
    }

    pub fn navigate(&mut self, loc: Location, ctx: &ListCtx) {
        if loc != self.loc {
            self.back.push(self.loc.clone());
            if self.back.len() > 100 {
                self.back.remove(0);
            }
            self.fwd.clear();
            let from = std::mem::replace(&mut self.loc, loc);
            self.entered_from(&from);
        }
        self.reload(ctx);
    }

    /// A folder was just entered from `from`: no marks, filter or search there, and
    /// when going up, the cursor on the folder we came from.
    fn entered_from(&mut self, from: &Location) {
        self.select_name = Some(from.short_name_raw()).filter(|n| !n.is_empty() && from.parent().as_ref() == Some(&self.loc));
        self.marked.clear();
        self.end_range();
        self.cursor = 0;
        self.scroll_offset = 0.0;
        self.filter_text.clear();
        self.filter = Ok(Filter::All);
        self.scan = None;
        self.parked_scan = None;
        self.search = None;
        self.loading = None;
        self.dir_sizes = None;
    }

    pub fn go_back(&mut self, ctx: &ListCtx) {
        if let Some(l) = self.back.pop() {
            let from = std::mem::replace(&mut self.loc, l);
            self.fwd.push(from.clone());
            self.entered_from(&from);
            self.reload(ctx);
        }
    }

    pub fn go_forward(&mut self, ctx: &ListCtx) {
        if let Some(l) = self.fwd.pop() {
            let from = std::mem::replace(&mut self.loc, l);
            self.back.push(from.clone());
            self.entered_from(&from);
            self.reload(ctx);
        }
    }

    pub fn go_up(&mut self, ctx: &ListCtx) {
        if let Some(p) = self.loc.parent() {
            self.navigate(p, ctx);
        }
    }

    /// Reload the listing, keeping the cursor on the same name if possible.
    pub fn reload(&mut self, ctx: &ListCtx) {
        if self.term.is_some() {
            return;
        }
        if self.search.is_some() {
            self.refresh_search(ctx);
            return;
        }
        if self.select_name.is_none() {
            self.select_name = self.cursor_entry().map(|e| e.name.clone());
        }
        self.error = None;
        // Not while the address is being typed: that would wipe it.
        if !self.editing_path {
            self.path_text = self.loc.display(|id| remote_label(ctx, id));
        }
        match self.loc.clone() {
            Location::Local(p) => match {
                // Before listing: a change while it lists is noticed next time.
                self.dir_stamp = crate::files::dirwatch::modified(&p);
                self.stamp_settled = self.dir_stamp.is_none_or(|m| !crate::files::dirwatch::is_recent(m));
                self.refresh_due = false;
                let started = std::time::Instant::now();
                let list = list_local(&p);
                self.list_cost = started.elapsed();
                list
            } {
                Ok(list) => self.set_entries(list, ctx),
                Err(e) => {
                    self.error = Some(e);
                    self.set_entries(vec![], ctx);
                }
            },
            Location::Archive { file, inner } => {
                if let Some((f, all)) = &self.archive_cache {
                    if *f == file {
                        let list = archive::children(all, &inner);
                        self.set_entries(list, ctx);
                        return;
                    }
                }
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let r = archive::list(&file).map(|all| Loaded::Archive(file, Arc::new(all)));
                    let _ = tx.send(r);
                });
                self.loading = Some(rx);
            }
            Location::Remote { id, path } => {
                let Some((_, fs)) = ctx.remotes.get(&id) else {
                    self.error = Some(t("Connection closed"));
                    self.set_entries(vec![], ctx);
                    return;
                };
                let fs = fs.clone();
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let r = fs.lock().map_err(|e| e.to_string()).and_then(|mut g| g.list(&path)).map(Loaded::Entries);
                    let _ = tx.send(r);
                });
                self.loading = Some(rx);
            }
        }
    }

    /// Search every folder below this one (local folders only).
    pub fn start_search(&mut self, query: &str, ctx: &ListCtx) -> Result<(), String> {
        let filter = Filter::parse(query)?;
        self.start_search_with(query.to_string(), filter, ctx)
    }

    /// Search every folder below this one for what passes `filter`, shown as `label`.
    pub fn start_search_with(&mut self, label: String, filter: Filter, ctx: &ListCtx) -> Result<(), String> {
        let Some(root) = self.loc.local_path().map(|p| p.to_path_buf()) else {
            return Err(t("Searching subfolders works on local folders"));
        };
        self.search = None;
        self.filter_text.clear();
        self.filter = Ok(Filter::All);
        self.scan = None;
        self.marked.clear();
        self.end_range();
        self.cursor = 0;
        self.scroll_offset = 0.0;
        self.loading = None;
        self.error = None;
        self.search = Some(Search::start(label, root, filter, ctx.show_hidden, None));
        self.view.clear();
        self.entries.clear();
        if self.loc.parent().is_some() {
            self.entries.push(Entry::parent());
        }
        self.rebuild_view(ctx);
        Ok(())
    }

    /// Run the search again (a reload, e.g. after a file operation). The results shown
    /// stay until the new ones are complete; then the cursor and the marks are kept by name.
    /// A search that was stopped is not run again: its results stay as they are.
    fn refresh_search(&mut self, ctx: &ListCtx) {
        if self.search.as_ref().is_some_and(|s| s.stopped || s.discard) {
            return;
        }
        let Some((query, filter)) = self.search.as_ref().map(|s| (s.query.clone(), s.filter.clone())) else { return };
        let Some(root) = self.loc.local_path().map(|p| p.to_path_buf()) else { return };
        // The search it replaces stops when dropped.
        self.search = Some(Search::start(query, root, filter, ctx.show_hidden, Some(Vec::new())));
    }

    /// Stop the search or the reading of files under way; what was found so far stays.
    pub fn stop_search(&mut self) {
        if let Some(s) = self.search.as_mut().filter(|s| s.running) {
            s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            if s.pending.take().is_some() {
                // A re-run: the complete results of the run before stay, not its part.
                s.discard = true;
            } else {
                s.stopped = true;
            }
        }
        if let Some(s) = self.scan.as_mut().filter(|s| s.running) {
            s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            s.stopped = true;
        }
    }

    /// Run what the Find panel asks for: a filter of this folder, or a search of every
    /// folder below. An empty text shows the folder again.
    pub fn set_find(&mut self, find: Find, ctx: &ListCtx) -> Result<(), String> {
        self.find_draft = find.clone();
        if find.text.trim().is_empty() {
            if self.search.is_some() || matches!(self.filter, Ok(Filter::Find(_))) {
                self.clear_filter_and_search(ctx);
            }
            return Ok(());
        }
        if find.mode == FindMode::Content && self.loc.local_path().is_none() {
            return Err(t("Searching file contents works on local folders"));
        }
        if find.subfolders {
            return self.start_search_with(find.summary(), Filter::Find(find), ctx);
        }
        if self.search.take().is_some() {
            self.reload(ctx);
        }
        self.park_scan();
        self.filter_text = find.summary();
        self.filter = Ok(Filter::Find(find));
        self.end_range();
        self.restart_scan(ctx);
        if !self.rebuild_view(ctx) {
            self.cursor = 0;
        }
        self.ensure_visible = true;
        Ok(())
    }

    /// The filter as it is, to be put back later with `restore_filter` (the search box
    /// and the palette filter the window while they are open).
    pub fn filter_state(&self) -> (String, Result<Filter, String>) {
        (self.filter_text.clone(), self.filter.clone())
    }

    pub fn restore_filter(&mut self, state: (String, Result<Filter, String>), ctx: &ListCtx) {
        if self.filter_text == state.0 {
            return;
        }
        self.park_scan();
        let parked = self.parked_scan.take().filter(|(text, _)| *text == state.0);
        self.filter_text = state.0;
        self.filter = state.1;
        self.end_range();
        match parked {
            Some((_, s)) => {
                self.scan = Some(s);
                self.keep_or_restart_scan(ctx);
            }
            None => self.restart_scan(ctx),
        }
        if !self.rebuild_view(ctx) {
            self.cursor = 0;
        }
        self.ensure_visible = true;
    }

    fn clear_filter_and_search(&mut self, ctx: &ListCtx) {
        if self.search.take().is_some() {
            self.reload(ctx);
        }
        if !self.filter_text.is_empty() {
            self.set_filter("", ctx);
        }
    }

    /// Leave search / filter mode.
    pub fn clear_search(&mut self, ctx: &ListCtx) {
        if self.quick.is_some() {
            self.set_quick(None, ctx);
        }
        if self.search.take().is_some() {
            self.reload(ctx);
        }
        if !self.filter_text.is_empty() {
            self.set_filter("", ctx);
        }
    }

    pub fn is_filtered(&self) -> bool {
        self.search.is_some() || !self.filter_text.is_empty() || self.quick.is_some()
    }

    /// Turn the quick filter on (a category) or off (`None`).
    pub fn set_quick(&mut self, q: Option<QuickActive>, ctx: &ListCtx) {
        self.quick = q;
        self.end_range();
        if !self.rebuild_view(ctx) {
            self.cursor = 0;
        }
        self.ensure_visible = true;
    }

    /// Poll background loads. Returns true if something changed.
    pub fn poll(&mut self, ctx: &ListCtx) -> bool {
        let mut changed = false;
        if let Some(s) = &mut self.search {
            let (mut got, mut done) = (false, false);
            loop {
                match s.rx.try_recv() {
                    Ok(_) if s.discard => {}
                    Ok(batch) => match &mut s.pending {
                        Some(p) => p.extend(batch),
                        None => {
                            self.entries.extend(batch);
                            got = true;
                        }
                    },
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        s.running = false;
                        s.took = Some(s.started.elapsed());
                        got = true;
                        done = true;
                        break;
                    }
                }
            }
            if done {
                let found = match s.pending.take() {
                    // A re-run: its results replace the old ones at once.
                    Some(list) => {
                        let n = list.len();
                        let keep = self.cursor_entry().map(|e| e.name.clone());
                        self.view.clear();
                        self.entries.clear();
                        if self.loc.parent().is_some() {
                            self.entries.push(Entry::parent());
                        }
                        self.entries.extend(list);
                        let existing: HashSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
                        self.marked.retain(|m| existing.contains(m.as_str()));
                        self.range_marks.retain(|m| existing.contains(m.as_str()));
                        if self.select_name.is_none() {
                            self.select_name = keep;
                        }
                        n
                    }
                    None => self.entries.iter().filter(|e| !e.is_parent).count(),
                };
                if let Some(s) = &mut self.search {
                    s.truncated = found >= SEARCH_LIMIT;
                }
            }
            if got {
                self.rebuild_view(ctx);
                // A name to show (a reload, a new item) once it is among the results.
                if let Some(p) = self.select_name.as_deref().and_then(|n| self.pos_of(n)) {
                    self.cursor = p;
                    self.select_name = None;
                    self.ensure_visible = true;
                }
                if done {
                    self.select_name = None;
                }
                changed = true;
            }
        }
        if let Some(s) = &mut self.scan {
            let mut got = false;
            loop {
                match s.rx.try_recv() {
                    Ok(names) => {
                        s.hits.extend(names);
                        got = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        s.running = false;
                        got = true;
                        break;
                    }
                }
            }
            if got {
                self.rebuild_view(ctx);
                changed = true;
            }
        }
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(Ok(Loaded::Entries(list))) => {
                    self.loading = None;
                    self.set_entries(list, ctx);
                    changed = true;
                }
                Ok(Ok(Loaded::Archive(file, all))) => {
                    self.loading = None;
                    let inner = match &self.loc {
                        Location::Archive { inner, .. } => inner.clone(),
                        _ => String::new(),
                    };
                    let list = archive::children(&all, &inner);
                    self.archive_cache = Some((file, all));
                    self.set_entries(list, ctx);
                    changed = true;
                }
                Ok(Err(e)) => {
                    self.loading = None;
                    self.error = Some(e);
                    self.set_entries(vec![], ctx);
                    changed = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => {
                    self.loading = None;
                    self.select_name = None;
                }
            }
        }
        // Sizes belong to the folder they were counted in, never to one that has the same names.
        if self.dir_sizes.as_ref().is_some_and(|j| self.loc.local_path() != Some(j.dir.as_path())) {
            self.dir_sizes = None;
        }
        if let Some(job) = &self.dir_sizes {
            loop {
                match job.try_recv() {
                    Ok((name, size)) => {
                        if let Some(e) = self.entries.iter_mut().find(|e| e.is_dir && !e.is_parent && e.name == name) {
                            e.size = size;
                            e.size_known = true;
                        }
                        changed = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        self.dir_sizes = None;
                        self.rebuild_view(ctx);
                        break;
                    }
                }
            }
        }
        changed
    }

    /// The local folder shown and its stamp, for `files::dirwatch`: not while it loads or
    /// shows search results (a reload would search again).
    /// Nor while a filter reads the files (`source LIKE …`): each reload would read them
    /// all again and empty the list meanwhile.
    pub fn watched(&self) -> Option<(PathBuf, Option<std::time::SystemTime>)> {
        match &self.loc {
            Location::Local(p) if self.is_files() && self.search.is_none() && self.loading.is_none() && self.scan.is_none() => {
                // A stamp taken too soon is looked at again: one that no folder has.
                Some((p.clone(), if self.stamp_settled { self.dir_stamp } else { Some(std::time::SystemTime::UNIX_EPOCH) }))
            }
            _ => None,
        }
    }

    /// Whether an automatic reload may happen now: not under an open right-click menu,
    /// and for a folder slow to list, not more than a twentieth of the time (so the
    /// window stays usable in a huge folder that keeps changing).
    /// A folder that takes long to list (huge, or on a slow network drive) does not
    /// reload by itself at all: that would freeze the window each time.
    pub fn may_refresh_quietly(&self) -> bool {
        !self.menu_open && self.list_cost < SLOW_LISTING && self.quiet_at.is_none_or(|t| t.elapsed() >= self.list_cost * 20)
    }

    /// Reload because the folder changed on disk, without getting in the way: the list
    /// does not scroll to the cursor, and the sizes counted for folders that are still
    /// there stay (what is inside them did not change the folder's stamp).
    /// A Shift range being made goes on from the same anchor.
    pub fn refresh_quietly(&mut self, ctx: &ListCtx) {
        // A folder removed and made again under the same name has another date: its old
        // size is not the new one's.
        let sizes: HashMap<String, (u64, Option<std::time::SystemTime>)> =
            self.entries.iter().filter(|e| e.is_dir && !e.is_parent && e.size_known).map(|e| (e.name.clone(), (e.size, e.modified))).collect();
        let ensure = self.ensure_visible;
        let (anchor, range_marks) = (self.anchor.take(), std::mem::take(&mut self.range_marks));
        self.reload(ctx);
        self.quiet_at = Some(std::time::Instant::now());
        self.ensure_visible = ensure;
        if anchor.as_deref().is_some_and(|a| self.pos_of(a).is_some()) {
            self.anchor = anchor;
            self.range_marks = range_marks.into_iter().filter(|m| self.marked.contains(m)).collect();
        }
        if !sizes.is_empty() {
            for e in self.entries.iter_mut().filter(|e| e.is_dir && !e.is_parent && !e.size_known) {
                if let Some(&(s, modified)) = sizes.get(&e.name) {
                    if modified == e.modified {
                        e.size = s;
                        e.size_known = true;
                    }
                }
            }
            self.rebuild_view(ctx);
        }
    }

    pub fn invalidate_archive_cache(&mut self) {
        self.archive_cache = None;
    }

    /// A filter reading files that was stopped keeps what it found (for the names
    /// still there) instead of reading every file again.
    fn keep_or_restart_scan(&mut self, ctx: &ListCtx) {
        match self.scan.take() {
            Some(mut s) if s.stopped && !s.running => {
                let names: HashSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
                s.hits.retain(|n| names.contains(n.as_str()));
                self.scan = Some(s);
            }
            _ => self.restart_scan(ctx),
        }
    }

    /// The filter is about to change: a stopped reading of files is put aside, to be
    /// taken back if that filter is put back (`restore_filter`).
    fn park_scan(&mut self) {
        if self.scan.as_ref().is_some_and(|s| s.stopped && !s.running) {
            self.parked_scan = self.scan.take().map(|s| (self.filter_text.clone(), s));
        }
    }

    fn set_entries(&mut self, mut list: Vec<Entry>, ctx: &ListCtx) {
        if self.loc.parent().is_some() {
            list.insert(0, Entry::parent());
        }
        // The old view points into the old list.
        self.view.clear();
        self.entries = list;
        let existing: HashSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        self.marked.retain(|m| existing.contains(m.as_str()));
        self.end_range();
        self.keep_or_restart_scan(ctx);
        self.rebuild_view(ctx);
        if let Some(name) = self.select_name.take() {
            if let Some(pos) = self.pos_of(&name) {
                self.cursor = pos;
            }
        }
        self.cursor = self.cursor.min(self.view.len().saturating_sub(1));
        self.ensure_visible = true;
    }

    pub fn set_filter(&mut self, text: &str, ctx: &ListCtx) {
        self.park_scan();
        self.filter_text = text.to_string();
        self.filter = Filter::parse(text);
        self.end_range();
        self.restart_scan(ctx);
        if !self.rebuild_view(ctx) {
            self.cursor = 0;
        }
        self.ensure_visible = true;
    }

    /// Start reading the files of the listing again, when the filter looks into them
    /// (`source LIKE …`): on the UI thread that could take minutes.
    fn restart_scan(&mut self, ctx: &ListCtx) {
        self.scan = None;
        let Ok(filter) = &self.filter else { return };
        if !filter.reads_content() {
            return;
        }
        let Some(dir) = self.loc.local_path().map(|p| p.to_path_buf()) else {
            self.filter = Err(t("Searching file contents works on local folders"));
            return;
        };
        let filter = filter.clone();
        let show_hidden = ctx.show_hidden;
        // Search results are named by their path below the folder, which joins the same way.
        let entries: Vec<Entry> = self.entries.iter().filter(|e| !e.is_parent && !e.is_dir && (show_hidden || !e.hidden)).cloned().collect();
        let (rx, cancel, progress) = crate::ui::search::spawn_scan(dir, filter, entries);
        self.scan = Some(ContentScan { running: true, stopped: false, progress, started: std::time::Instant::now(), hits: HashSet::new(), rx, cancel });
    }

    /// Filter and sort the listing again, keeping the cursor on the same entry. Returns
    /// whether that entry is still shown.
    pub fn rebuild_view(&mut self, ctx: &ListCtx) -> bool {
        let keep = self.cursor_entry().map(|e| e.name.clone());
        self.build_view(ctx);
        match keep.and_then(|n| self.pos_of(&n)) {
            Some(p) => {
                self.cursor = p;
                true
            }
            None => false,
        }
    }

    /// The folder is shown as a grid of icons (this window's choice, else Settings').
    pub fn shows_icons(&self, ctx: &ListCtx) -> bool {
        self.view_mode.unwrap_or(ctx.default_view) == crate::config::ViewMode::Icons
    }

    fn build_view(&mut self, ctx: &ListCtx) {
        // The entries themselves are kept in the order shown, so that the view taken from
        // them comes out sorted: sorting a big folder from the disk's order on every
        // keystroke of a filter is what made typing slow. Sorting what is already in
        // order is quick (the sort finds the runs).
        let (sort, desc, dirs_first) = (self.sort, self.desc, ctx.dirs_first);
        self.entries.sort_by(|x, y| entry_order(x, y, sort, desc, dirs_first));
        let filter = self.filter.as_ref().ok();
        let hits = self.scan.as_ref().map(|s| &s.hits);
        let icons = self.shows_icons(ctx);
        let view: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                // ".." is a row of the list; the icon view goes up with Backspace or the path.
                if e.is_parent {
                    return !icons;
                }
                if !ctx.show_hidden && e.hidden {
                    return false;
                }
                if self.quick.as_ref().is_some_and(|q| !q.matches(e)) {
                    return false;
                }
                match hits {
                    Some(h) => h.contains(&e.name),
                    None => filter.is_none_or(|f| f.matches(e)),
                }
            })
            .collect();
        self.view = view;
        self.cursor = self.cursor.min(self.view.len().saturating_sub(1));
    }

    pub fn set_sort(&mut self, key: SortKey, ctx: &ListCtx) {
        if self.sort == key {
            self.desc = !self.desc;
        } else {
            self.sort = key;
            self.desc = matches!(key, SortKey::Size | SortKey::Modified);
        }
        // Rows move: a Shift range starts again.
        self.end_range();
        self.rebuild_view(ctx);
        self.ensure_visible = true;
    }

    pub fn cursor_entry(&self) -> Option<&Entry> {
        self.view.get(self.cursor).and_then(|&i| self.entries.get(i))
    }

    /// The row of the entry called `name`, if it is shown.
    fn pos_of(&self, name: &str) -> Option<usize> {
        self.view.iter().position(|&i| self.entries.get(i).is_some_and(|e| e.name == name))
    }

    /// The marked entries that are shown (the filter may hide some): what the footer
    /// counts and what commands act on.
    pub fn marked_shown(&self) -> Vec<&Entry> {
        if self.marked.is_empty() {
            return vec![];
        }
        self.view.iter().filter_map(|&i| self.entries.get(i)).filter(|e| !e.is_parent && self.marked.contains(&e.name)).collect()
    }

    /// Marked entries that are shown, or the cursor entry if none is. Never includes `..`.
    pub fn selection(&self) -> Vec<Entry> {
        let marked = self.marked_shown();
        if !marked.is_empty() {
            return marked.into_iter().cloned().collect();
        }
        self.cursor_entry().filter(|e| !e.is_parent).cloned().into_iter().collect()
    }

    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
        if self.view.is_empty() {
            return;
        }
        let n = self.view.len() as isize;
        let new = (self.cursor as isize + delta).clamp(0, n - 1) as usize;
        if extend {
            self.extend_to(new);
        } else {
            self.end_range();
            self.cursor = new;
        }
        self.ensure_visible = true;
    }

    /// Up or down `lines` lines of the icon view. Past the first line nothing happens;
    /// down to a shorter last line goes to its last icon, as in other file managers.
    pub fn move_grid(&mut self, lines: isize, extend: bool) {
        let (Some(cols), n) = (self.grid_cols, self.view.len() as isize) else { return self.move_cursor(lines, extend) };
        let (cols, cur) = (cols as isize, self.cursor as isize);
        let target = cur + lines * cols;
        let to = if target < 0 {
            return;
        } else if target >= n {
            // Already on the last line: stay.
            if cur / cols == (n - 1) / cols {
                return;
            }
            n - 1
        } else {
            target
        };
        self.move_cursor(to - cur, extend);
    }

    /// The cursor moves this far for Page Up / Page Down: a screenful less one line.
    pub fn page_rows(&self) -> isize {
        match self.grid_cols {
            Some(cols) => ((self.view_height / CELL_H) as isize - 1).max(1) * cols as isize,
            None => ((self.view_height / ROW_H) as isize - 1).max(1),
        }
    }

    /// The marks made so far stay as they are; the next Shift range starts afresh.
    fn end_range(&mut self) {
        self.anchor = None;
        self.range_marks.clear();
    }

    /// Shift+click or Shift+arrow: mark the rows from the anchor (where the range began)
    /// to `row` and put the cursor there. The range replaces the one marked before, so it
    /// can also shrink.
    fn extend_to(&mut self, row: usize) {
        if self.view.is_empty() {
            return;
        }
        let row = row.min(self.view.len() - 1);
        let a = match self.anchor.as_deref().and_then(|n| self.pos_of(n)) {
            Some(a) => a,
            None => {
                let a = self.cursor.min(self.view.len() - 1);
                self.range_marks.clear();
                self.anchor = self.entries.get(self.view[a]).map(|e| e.name.clone());
                a
            }
        };
        self.select_range(a, row);
        self.cursor = row;
    }

    fn select_range(&mut self, a: usize, b: usize) {
        for n in std::mem::take(&mut self.range_marks) {
            self.marked.remove(&n);
        }
        let Some(last) = self.view.len().checked_sub(1) else { return };
        let (lo, hi) = (a.min(b).min(last), a.max(b).min(last));
        for i in lo..=hi {
            let Some(e) = self.entries.get(self.view[i]) else { continue };
            if !e.is_parent && self.marked.insert(e.name.clone()) {
                self.range_marks.insert(e.name.clone());
            }
        }
    }

    pub fn toggle_mark_cursor(&mut self) {
        if let Some(e) = self.cursor_entry() {
            if e.is_parent {
                return;
            }
            let n = e.name.clone();
            if !self.marked.remove(&n) {
                self.marked.insert(n);
            }
        }
    }

    pub fn mark_all(&mut self) {
        self.end_range();
        for &i in &self.view {
            let e = &self.entries[i];
            if !e.is_parent {
                self.marked.insert(e.name.clone());
            }
        }
    }

    pub fn invert_marks(&mut self) {
        self.end_range();
        let names: Vec<String> = self.view.iter().map(|&i| &self.entries[i]).filter(|e| !e.is_parent).map(|e| e.name.clone()).collect();
        for n in names {
            if !self.marked.remove(&n) {
                self.marked.insert(n);
            }
        }
    }

    /// Put the cursor on `name` now if it is shown. It is kept for later only while a
    /// listing is on its way (or none was loaded yet): once found, a later reload must
    /// not jump back to it.
    pub fn select_name(&mut self, name: &str) {
        let pos = self.pos_of(name);
        if let Some(p) = pos {
            self.cursor = p;
            self.ensure_visible = true;
            self.end_range();
        }
        let later = self.loading.is_some() || self.search.as_ref().is_some_and(|s| s.running) || (pos.is_none() && self.entries.is_empty());
        self.select_name = later.then(|| name.to_string());
    }

    pub fn title(&self) -> String {
        if let Some(c) = self.custom_title.as_ref().filter(|c| !c.trim().is_empty()) {
            return c.clone();
        }
        self.default_title()
    }

    pub fn default_title(&self) -> String {
        if let Some(t) = &self.term {
            return t.label();
        }
        if self.settings {
            return t("Settings");
        }
        if self.transfers {
            return t("Tasks");
        }
        if let Some(c) = &self.checksum {
            return c.title();
        }
        if self.renamer.is_some() {
            return t("Batch rename");
        }
        if let Some(d) = &self.du {
            let name = d.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| d.path.to_string_lossy().to_string());
            return format!("{}: {name}", t("Disk usage"));
        }
        let s = self.loc.short_name();
        if s.chars().count() > 22 {
            format!("{}…", s.chars().take(21).collect::<String>())
        } else {
            s
        }
    }
}

/// Walk the folders below `root` in the background; the entries that pass `filter`
/// arrive in batches, named by their path below `root`.
impl Location {
    /// Raw last component (no archive decoration) for cursor restoration.
    pub fn short_name_raw(&self) -> String {
        match self {
            Location::Local(p) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            Location::Remote { path, .. } => path.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string(),
            Location::Archive { file, inner } => {
                if inner.is_empty() {
                    file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                } else {
                    inner.rsplit('/').next().unwrap_or("").to_string()
                }
            }
        }
    }
}

pub fn remote_label(ctx: &ListCtx, id: u64) -> String {
    ctx.remotes.get(&id).map(|r| r.0.clone()).unwrap_or_else(|| "remote:".into())
}

/// A mini window of a tab: a file view or a terminal.
pub struct Pane {
    tab: Tab,
}

impl Pane {
    pub fn new(loc: Location) -> Self {
        Self { tab: Tab::new(loc) }
    }
    pub fn home() -> Self {
        Self::new(Location::Local(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))))
    }
    pub fn from_tab(tab: Tab) -> Self {
        Self { tab }
    }
    pub fn tab(&self) -> &Tab {
        &self.tab
    }
    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tab
    }
}

// ---------------------------------------------------------------- rendering

pub const ROW_H: f32 = 28.0;
/// A cell of the icon view: its least width and its height.
pub const CELL_W: f32 = 104.0;
pub const CELL_H: f32 = 104.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    Open,
    OpenDefault,
    OpenWith,
    DefaultApps,
    View,
    /// Show or hide the Quick View panel.
    QuickView,
    Copy,
    Move,
    Rename,
    BatchRename,
    Checksum,
    Delete,
    Pack,
    ExtractHere,
    ExtractOther,
    CopyPath,
    Reveal,
    Terminal,
    /// Open a terminal in each selected folder (several are confirmed first).
    TerminalInSelection,
    Bookmark,
    BookmarkItem,
    Properties,
    DiskUsage,
    NewFolder,
    NewFile,
    Refresh,
    CalcSizes,
    SelectAll,
    /// Split the window; `true` opens a terminal in the new half, `false` files.
    Split(SplitDir, bool),
    ClosePane,
    TerminalProfile(usize),
}

/// What is being dragged between panels (set on drag start, taken on drop).
#[derive(Clone)]
pub struct DragPayload {
    pub side: usize,
    pub loc: Location,
    pub items: Vec<Entry>,
}

pub fn drag_payload(ctx: &egui::Context) -> Option<Arc<DragPayload>> {
    egui::DragAndDrop::payload::<DragPayload>(ctx)
}

fn is_inside(target: &Location, ancestor: &Location) -> bool {
    let mut cur = Some(target.clone());
    for _ in 0..128 {
        match cur {
            Some(l) if l == *ancestor => return true,
            Some(l) => cur = l.parent(),
            None => return false,
        }
    }
    false
}

/// Can the payload be dropped into `target`?
pub fn drop_allowed(p: &DragPayload, target: &Location) -> bool {
    if *target == p.loc || matches!(target, Location::Archive { .. }) {
        return false;
    }
    // A folder cannot go into itself or one of its sub-folders.
    !p.items.iter().any(|it| it.is_dir && is_inside(target, &p.loc.child(&it.name)))
}

pub enum PaneEvent {
    /// Pointer is over a valid drop target while dragging.
    DragOver(Location),
    Drop(Arc<DragPayload>, Location),
    ClosePane,
    Focus,
    Open,
    NavigateText(String),
    Navigate(Location),
    Up,
    Back,
    Forward,
    Command(Cmd),
    /// The filter chip in the address bar was clicked: show the folder again.
    ClearFilter,
    Sort(SortKey),
    Term(crate::terminal::view::TermResponse),
    /// From a disk usage window.
    Du(crate::tools::diskusage::DuAction),
    /// Quick filter: category `i` of the settings, or off.
    QuickFilter(Option<usize>),
    /// The Find panel's search (an empty text: off).
    Find(crate::ui::filter::Find),
    /// Open Settings → Quick filters.
    EditQuickFilters,
    /// A Settings window: the area its pages go in (the app draws them).
    SettingsArea(Rect),
    /// A Transfers window: the area its content goes in (the app draws it).
    TransfersArea(Rect),
    /// The space for a Checksum or Batch rename tool (the app draws it).
    ToolArea(Rect),
    /// An AI coding agent button in a terminal's info bar.
    Agent(crate::system::agents::AgentAct),
}

#[derive(Clone)]
pub struct PaneUiCtx<'a> {
    /// This window's number in a split tab (1 to 4), 0 if it is alone.
    pub window_number: usize,
    /// Colours of the window (a terminal's own theme in its info bar).
    pub theme: &'a Theme,
    /// The app's theme: menus and popups are drawn with it, whatever the window's colours.
    pub app_theme: &'a Theme,
    pub active: bool,
    pub drives: &'a [crate::files::drives::Drive],
    pub list: ListCtx<'a>,
    pub is_bookmarked: bool,
    pub show_free: bool,
    /// Windows in this tab (more than one: windows get a grip and a close button).
    pub pane_count: usize,
    /// Allowed splits of this pane: right, left, down, up (2 × 2 grid at most).
    pub can_split: [bool; 4],
    pub terminal_profiles: &'a [String],
    /// This pane has the keyboard (exactly the active pane, no dialog open).
    pub focused: bool,
    pub term_font_size: f32,
    pub term_copy_on_select: bool,
    /// Terminal scroll speed: trackpad, mouse wheel.
    pub term_scroll: [f32; 2],
    /// The quick-filter categories.
    pub quick_filters: &'a [crate::config::QuickFilter],
    /// The agent the AI button in terminal info bars starts (`None`: no button), and
    /// every agent found (to switch from its menu).
    pub agent: Option<&'a crate::system::agents::Found>,
    pub agents: &'a [crate::system::agents::Found],
    /// Git folders: their branch after the name, with `(git)` before it, and coloured
    /// by whether everything is committed (which runs git).
    pub git_branches: bool,
    pub git_label: bool,
    pub git_style: crate::config::GitStyle,
    pub git_status: bool,
    /// Thumbnails of pictures and videos in the icon view (`None`: turned off).
    pub thumbs: Option<&'a crate::preview::thumbs::Thumbs>,
}

pub const SIDE_PAD: f32 = 10.0;

/// Longest branch name shown after a folder's name; longer ones end in "…".
const MAX_BRANCH_CHARS: usize = 24;

/// The colour of a branch: by its state when `colored` (green when everything is
/// committed, orange with changes), else grey. Text is drawn in it; a filled label has
/// it as background, a little deeper, under white text.
fn branch_color(state: crate::files::gitinfo::State, colored: bool, filled: bool, th: &Theme) -> Color32 {
    use crate::files::gitinfo::State;
    let state = if colored { state } else { State::Unknown };
    match (state, filled, th.dark) {
        (State::Clean, false, false) => Color32::from_rgb(0x1a, 0x7f, 0x37),
        (State::Clean, false, true) => Color32::from_rgb(0x3f, 0xb9, 0x50),
        (State::Clean, true, _) => Color32::from_rgb(0x1f, 0x88, 0x3d),
        (State::Dirty, false, false) => Color32::from_rgb(0xbc, 0x4c, 0x00),
        (State::Dirty, false, true) => Color32::from_rgb(0xf0, 0x88, 0x3e),
        (State::Dirty, true, _) => Color32::from_rgb(0xc2, 0x5a, 0x0a),
        (State::Unknown, false, _) => th.weak,
        (State::Unknown, true, false) => Color32::from_rgb(0x8c, 0x95, 0x9f),
        (State::Unknown, true, true) => Color32::from_rgb(0x5c, 0x64, 0x6e),
    }
}

/// A git folder's branch after its name ending at `name_right`: `(git) [branch]` as text,
/// or `git · branch` on a filled label. The branch is cut with "…" to fit the name
/// column; nothing when there is no room for even a little of it. Its rect.
/// The order of a listing: ".." first, folders before files if asked, then by the
/// sort key (ties by name).
fn entry_order(x: &Entry, y: &Entry, sort: SortKey, desc: bool, dirs_first: bool) -> Ordering {
    if x.is_parent != y.is_parent {
        return if x.is_parent { Ordering::Less } else { Ordering::Greater };
    }
    if dirs_first && x.is_dir != y.is_dir {
        return if x.is_dir { Ordering::Less } else { Ordering::Greater };
    }
    let o = match sort {
        SortKey::Name => natural_cmp(&x.name, &y.name),
        SortKey::Ext => x.ext.cmp(&y.ext).then_with(|| natural_cmp(&x.name, &y.name)),
        SortKey::Size => x.size.cmp(&y.size).then_with(|| natural_cmp(&x.name, &y.name)),
        SortKey::Modified => x.modified.cmp(&y.modified).then_with(|| natural_cmp(&x.name, &y.name)),
    };
    if desc {
        o.reverse()
    } else {
        o
    }
}

fn branch_label(p: &egui::Painter, branch: &str, name_rect: Rect, name_right: f32, label: bool, filled: bool, color: Color32) -> Option<Rect> {
    let font = FontId::proportional(if filled { 12.0 } else { 13.0 });
    let pad = if filled { vec2(5.0, 1.5) } else { vec2(0.0, 0.0) };
    let left = name_right + 6.0;
    let room = name_rect.right() - left - 2.0 * pad.x;
    let total = branch.chars().count();
    let mut max = total.min(MAX_BRANCH_CHARS);
    loop {
        let short = crate::files::gitinfo::shorten(branch, max);
        let text = match (filled, label) {
            (false, true) => format!("(git) [{short}]"),
            (false, false) => format!("[{short}]"),
            (true, true) => format!("git · {short}"),
            (true, false) => short,
        };
        let ink = if filled { Color32::WHITE } else { color };
        let galley = p.layout_no_wrap(text, font.clone(), ink);
        if galley.size().x <= room {
            let pos = pos2(left + pad.x, name_rect.center().y - galley.size().y / 2.0);
            let r = Rect::from_min_size(pos, galley.size()).expand2(pad);
            if filled {
                p.rect_filled(r, 4.0, color);
            }
            p.galley(pos, galley, ink);
            return Some(r);
        }
        // At least a few characters and the "…", or nothing.
        if max <= 3 {
            return None;
        }
        max = (max - 2).max(3);
    }
}

fn branch_tooltip(repo: &crate::files::gitinfo::Repo, checked: bool) -> String {
    use crate::files::gitinfo::State;
    let mut lines = vec![if repo.detached { format!("{} {}", t("Detached at"), repo.branch) } else { format!("{} {}", t("Branch:"), repo.branch) }];
    if !checked {
        return lines.join("\n");
    }
    lines.push(match repo.state {
        State::Clean => t("Everything is committed"),
        State::Dirty => {
            let mut parts = vec![];
            if repo.changed > 0 {
                parts.push(format!("{} {}", repo.changed, t("changed")));
            }
            if repo.untracked > 0 {
                parts.push(format!("{} {}", repo.untracked, t("not tracked")));
            }
            format!("{}: {}", t("Not committed"), parts.join(", "))
        }
        State::Unknown => t("Changes not checked"),
    });
    if repo.ahead + repo.behind > 0 {
        lines.push(format!("↑ {} {} · ↓ {} {}", repo.ahead, t("to push"), repo.behind, t("to pull")));
    }
    lines.join("\n")
}

pub fn pane_ui(ui: &mut egui::Ui, pane: &mut Pane, side: usize, c: &PaneUiCtx) -> Vec<PaneEvent> {
    let mut ev = Vec::new();
    let th = c.theme;
    // Only highlight the focused pane when there is more than one.
    let border = if c.active && c.pane_count > 1 { th.accent.gamma_multiply(0.6) } else { th.border };
    // The card is painted at exactly the rectangle the layout gave this pane:
    // content is clipped inside it and the border is drawn last, so neither
    // the list nor a neighbouring panel can hide an edge.
    let card = ui.max_rect();
    let radius = (th.radius * 2).max(4);
    // A terminal window is filled with the terminal's own background, rounded
    // corners included (its colour theme may differ from the app's).
    let term_theme = pane.tab().term.as_ref().map(|t| t.theme.unwrap_or(th));
    let fill = term_theme.map(crate::terminal::view::background).unwrap_or(th.bg);
    ui.painter().rect_filled(card, radius, fill);
    let inner = Rect::from_min_max(pos2(card.left() + 1.0, card.top() + 6.0), pos2(card.right() - 1.0, card.bottom() - 1.0));
    let mut content = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    content.set_clip_rect(card.shrink(1.0).intersect(ui.clip_rect()));
    {
        let ui = &mut content;
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        if let Some(tth) = term_theme {
            // The info bar, drawn in the terminal's colours.
            let mut tc = c.clone();
            tc.theme = tth;
            fixed_width(ui, |ui| term_header(ui, pane.tab(), &tc, &mut ev));
            let y = ui.cursor().top();
            ui.painter().hline(card.x_range().shrink(1.0), y, egui::Stroke::new(1.0_f32, tth.border.gamma_multiply(0.7)));
            ui.add_space(1.0);
            let term = pane.tab_mut().term.as_mut().unwrap();
            let opts = crate::terminal::view::TermOpts {
                theme: tth,
                font_size: c.term_font_size,
                copy_on_select: c.term_copy_on_select,
                scroll: c.term_scroll,
                active: c.focused,
                can_split: c.can_split,
                in_split: c.pane_count > 1,
                radius: radius as f32 - 1.0,
            };
            let r = term.ui(ui, &opts);
            if r.focus || r.close || r.restart || r.open_dir.is_some() || r.open_url.is_some() || r.font_delta != 0.0 || r.split.is_some() {
                ev.push(PaneEvent::Term(r));
            }
        } else if pane.tab().settings {
            let area = tool_window_header(ui, pane.tab().uid, card, c, &mut ev, icons::gear, &t("Settings"));
            ev.push(PaneEvent::SettingsArea(area));
        } else if pane.tab().transfers {
            let area = tool_window_header(ui, pane.tab().uid, card, c, &mut ev, icons::transfers, &t("Tasks"));
            ev.push(PaneEvent::TransfersArea(area));
        } else if pane.tab().is_tool() {
            let icon: fn(&egui::Painter, Rect, egui::Color32) = if pane.tab().checksum.is_some() { icons::mi_checksum } else { icons::mi_rename };
            let area = tool_window_header(ui, pane.tab().uid, card, c, &mut ev, icon, &pane.tab().title());
            ev.push(PaneEvent::ToolArea(area));
        } else if pane.tab().du.is_some() {
            let tab = pane.tab_mut();
            fixed_width(ui, |ui| du_header(ui, tab, c, &mut ev));
            let y = ui.cursor().top();
            ui.painter().hline(card.x_range().shrink(1.0), y, egui::Stroke::new(1.0_f32, th.border.gamma_multiply(0.7)));
            ui.add_space(2.0);
            let du = tab.du.as_mut().unwrap();
            let resp = ui.scope(|ui| du.ui(ui, th, &t));
            if let Some(a) = resp.inner {
                ev.push(PaneEvent::Du(a));
            }
            // The window's folder follows what the analysis shows.
            if tab.loc.local_path() != Some(du.path.as_path()) {
                tab.loc = Location::Local(du.path.clone());
            }
            if resp.response.clicked() || ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.any_pressed()) {
                ev.push(PaneEvent::Focus);
            }
        } else {
            let tab = pane.tab_mut();
            fixed_width(ui, |ui| header_row(ui, tab, side, c, &mut ev));
            ui.add_space(2.0);
            list_and_footer(ui, tab, side, c, &mut ev);
        }
    }
    ui.painter().rect_stroke(card, radius, egui::Stroke::new(1.0_f32, border), egui::StrokeKind::Inside);
    ev
}

/// Lay out a row in a child that cannot widen the pane: in a narrow pane an
/// overflowing header is clipped instead of pushing the file list wider.
/// The title row of a window whose content the app draws (Settings, Transfers).
/// Returns the area left for that content.
fn tool_window_header(
    ui: &mut egui::Ui,
    uid: u64,
    card: Rect,
    c: &PaneUiCtx,
    ev: &mut Vec<PaneEvent>,
    icon: fn(&egui::Painter, Rect, egui::Color32),
    title: &str,
) -> Rect {
    let th = c.theme;
    fixed_width(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_height(28.0);
            ui.add_space(SIDE_PAD - 4.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            if c.pane_count > 1 {
                window_grip(ui, uid, c, ev);
            }
            let (ir, _) = ui.allocate_exact_size(vec2(22.0, 26.0), Sense::hover());
            icon(ui.painter(), Rect::from_center_size(ir.center(), vec2(15.0, 15.0)), if c.active { th.accent } else { th.weak });
            ui.label(egui::RichText::new(title).font(FontId::new(13.5, egui::FontFamily::Name("bold".into()))).color(th.strong()));
            let room = (ui.available_width() - if c.pane_count > 1 { 56.0 } else { 28.0 } - SIDE_PAD).max(0.0);
            ui.add_space(room);
            window_buttons(ui, false, c, ev);
        });
    });
    let y = ui.cursor().top();
    ui.painter().hline(card.x_range().shrink(1.0), y, egui::Stroke::new(1.0_f32, th.border.gamma_multiply(0.7)));
    ui.available_rect_before_wrap().shrink2(vec2(10.0, 6.0))
}

fn fixed_width(ui: &mut egui::Ui, f: impl FnOnce(&mut egui::Ui)) {
    let top = ui.cursor().min;
    let w = ui.available_width();
    let mut child = ui
        .new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(top, vec2(w, ui.available_height()))).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(child.max_rect().intersect(ui.clip_rect()));
    f(&mut child);
    let h = child.min_rect().height();
    ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(w, h)));
}

fn header_row(ui: &mut egui::Ui, tab: &mut Tab, side: usize, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    let th = c.theme;
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        ui.add_space(SIDE_PAD - 4.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        if c.pane_count > 1 {
            window_grip(ui, tab.uid, c, ev);
        }
        // Narrow pane: keep only "parent folder" so the bookmark/close buttons fit.
        let compact = ui.available_width() < 380.0;
        if !compact
            && icons::icon_button(ui, 26.0, !tab.back.is_empty(), th, |p, r, col| icons::chevron(p, r, -1, col)).on_hover_text(t("Back (Alt+←)")).clicked()
        {
            ev.push(PaneEvent::Back);
        }
        if !compact
            && icons::icon_button(ui, 26.0, !tab.fwd.is_empty(), th, |p, r, col| icons::chevron(p, r, 1, col)).on_hover_text(t("Forward (Alt+→)")).clicked()
        {
            ev.push(PaneEvent::Forward);
        }
        if icons::icon_button(ui, 26.0, tab.loc.parent().is_some(), th, |p, r, col| icons::chevron(p, r, 0, col))
            .on_hover_text(t("Parent folder (Backspace)"))
            .clicked()
        {
            ev.push(PaneEvent::Up);
        }
        folder_tree_button(ui, tab, c, ev);
        ui.add_space(4.0);

        let extra = if c.pane_count > 1 { 56.0 } else { 0.0 };
        // Filter / search mode: a chip at the end of the address bar; click it to turn it off.
        let mut chip = match &tab.search {
            Some(s) => Some((
                format!(
                    "{}: {} · {}{}",
                    t("Subfolders"),
                    s.query,
                    tab.entries.iter().filter(|e| !e.is_parent).count(),
                    if s.running {
                        "…"
                    } else if s.truncated {
                        "+"
                    } else {
                        ""
                    }
                ),
                true,
            )),
            None => {
                let quick = tab.quick.as_ref().map(|q| q.label());
                match (quick, tab.filter_text.is_empty()) {
                    (Some(q), true) => Some((q, false)),
                    (Some(q), false) => Some((format!("{q} · {}", tab.filter_text), false)),
                    (None, false) => Some((format!("{}: {}", t("Filter"), tab.filter_text), false)),
                    (None, true) => None,
                }
            }
        };
        let room = ui.available_width() - 54.0 - extra - SIDE_PAD - 8.0;
        let cap = 300.0_f32.min((room - 80.0).max(60.0));
        let chip_width = |text: &str| ui.fonts(|f| f.layout_no_wrap(text.to_string(), FontId::proportional(12.5), th.text).size().x) + 46.0;
        // Categories that do not fit: the first one and how many more, or only how many
        // (the tooltip names them), rather than "Spr…".
        if let (Some((text, false)), Some(q)) = (chip.as_mut(), tab.quick.as_ref()) {
            if tab.filter_text.is_empty() && chip_width(text) > cap {
                *text = q.short_label();
                if q.names.len() > 1 && chip_width(text) > cap {
                    *text = q.names.len().to_string();
                }
            }
        }
        let chip_w = chip.as_ref().map(|(text, _)| chip_width(text).min(cap)).unwrap_or(0.0);
        let crumb_w = (room - chip_w).max(30.0);
        ui.allocate_ui(vec2(crumb_w, 28.0), |ui| breadcrumb(ui, tab, side, c, ev));
        if let Some((text, recursive)) = chip {
            let (rect, resp) = ui.allocate_exact_size(vec2(chip_w, 24.0), Sense::click());
            let label = text.clone();
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
            let bad = !recursive && tab.filter.is_err();
            let base = if bad { th.danger } else { th.accent };
            let fill = base.gamma_multiply(if resp.hovered() { 0.32 } else { 0.2 });
            ui.painter().rect(rect, 12.0, fill, egui::Stroke::new(1.0_f32, base.gamma_multiply(0.55)), egui::StrokeKind::Inside);
            let ir = Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(13.0, 13.0));
            if tab.quick.is_some() && !recursive {
                icons::funnel(ui.painter(), ir, base, true);
            } else {
                icons::search(ui.painter(), ir, base);
            }
            let tw = rect.width() - 42.0;
            let mut job = egui::text::LayoutJob::simple_singleline(text, FontId::proportional(12.5), th.strong());
            job.wrap = egui::text::TextWrapping::truncate_at_width(tw.max(8.0));
            let g = ui.fonts(|f| f.layout_job(job));
            ui.painter().galley(pos2(rect.left() + 26.0, rect.center().y - g.size().y / 2.0), g, th.strong());
            let x = Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(14.0, 14.0)).shrink(3.5);
            let s = egui::Stroke::new(1.4_f32, if resp.hovered() { th.strong() } else { th.weak });
            ui.painter().line_segment([x.left_top(), x.right_bottom()], s);
            ui.painter().line_segment([x.right_top(), x.left_bottom()], s);
            let tip = match &tab.filter {
                Err(e) if bad => format!("{e}\n\n{}", filter_help()),
                _ => match tab.quick.as_ref().filter(|_| !recursive) {
                    Some(q) => format!(
                        "{}: {}\n{}",
                        t("Quick filter"),
                        q.names.iter().map(|n| t(n)).collect::<Vec<_>>().join(", "),
                        t("Click to turn off the filter (Esc)")
                    ),
                    None => t("Click to turn off the filter (Esc)"),
                },
            };
            if resp.on_hover_text(tip).clicked() {
                ev.push(PaneEvent::ClearFilter);
            }
        }
        quick_filter_button(ui, tab, c, ev);
        let star_color = if c.is_bookmarked { th.marked } else { th.weak };
        let bookmarked = c.is_bookmarked;
        if icons::icon_button(ui, 26.0, true, th, |p, r, col| icons::star(p, r, if bookmarked { star_color } else { col }, bookmarked))
            .on_hover_text(format!("{} ({})", t("Bookmark this folder"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Bookmark)))
            .clicked()
        {
            ev.push(PaneEvent::Command(Cmd::Bookmark));
        }
        if c.pane_count > 1 {
            window_buttons(ui, false, c, ev);
        }
    });
}

/// The folder tree button of the address bar and its popup: OK (or a double click) on a
/// folder goes there.
fn folder_tree_button(ui: &mut egui::Ui, tab: &mut Tab, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    use crate::ui::foldertree::{self, FolderTree, Popup};
    let th = c.theme;
    let shown = tab.tree.is_some();
    let accent = th.accent;
    let r = icons::icon_button(ui, 26.0, true, th, |p, r, col| foldertree::icon(p, r, if shown { accent } else { col })).on_hover_text(t("Folder tree"));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t("Folder tree")));
    if r.clicked() {
        tab.tree = match tab.tree {
            Some(_) => None,
            None => {
                let roots = foldertree::roots(dirs::home_dir().as_deref(), c.drives);
                Some(Box::new(FolderTree::new(roots, tab.loc.local_path(), c.list.show_hidden)))
            }
        };
    }
    let Some(tree) = tab.tree.as_mut() else { return };
    match foldertree::popup(ui, egui::Id::new(("folder-tree", tab.uid)), r.rect, tree, c.app_theme) {
        Popup::Picked(p) => {
            tab.tree = None;
            ev.push(PaneEvent::Navigate(Location::Local(p)));
        }
        Popup::Closed => tab.tree = None,
        Popup::Open => {}
    }
}

/// The Find and filter button (a funnel) and its panel. On top, Find: by name (it
/// contains the text, or fits a pattern) or by text inside files, in this folder or in
/// every folder below; cheap ones apply as you type, the others with Enter or Find.
/// Below, the quick filter's file categories: each one is ticked or unticked by a
/// click, and the panel stays open so that several can be chosen.
fn quick_filter_button(ui: &mut egui::Ui, tab: &mut Tab, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    const FIND_W: f32 = 340.0;
    let th = c.theme;
    let finding = matches!(tab.filter, Ok(Filter::Find(_))) || tab.search.as_ref().is_some_and(|s| matches!(s.filter, Filter::Find(_)));
    let on = tab.quick.is_some() || finding;
    let accent = th.accent;
    let r = icons::icon_button(ui, 26.0, true, th, |p, r, col| icons::funnel(p, r, if on { accent } else { col }, on));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t("Find and filter")));
    let tip = match &tab.quick {
        Some(q) => format!("{}: {}", t("Quick filter"), q.label()),
        None => format!("{} ({})", t("Find and filter"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Filter)),
    };
    let r = r.on_hover_text(tip);
    let id = egui::Id::new(("quick-filter", tab.uid));
    let focus = id.with("focus");
    if std::mem::take(&mut tab.open_find) {
        egui::Popup::open_id(ui.ctx(), id);
        ui.data_mut(|d| d.insert_temp(focus, true));
    } else if r.clicked() {
        egui::Popup::toggle_id(ui.ctx(), id);
        ui.data_mut(|d| d.insert_temp(focus, true));
    }
    crate::ui::popup_below(id, &r, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        menu_style(ui);
        ui.set_width(FIND_W);
        // One column: Find, then the kinds of files; it scrolls in a short window.
        let room = (ui.ctx().screen_rect().bottom() - r.rect.bottom() - 40.0).max(160.0);
        egui::ScrollArea::vertical().max_height(room).min_scrolled_height(room).auto_shrink([false, true]).show(ui, |ui| {
            find_column(ui, tab, c, ev, focus, finding, FIND_W);
            ui.separator();
            kinds_column(ui, tab, c, ev, FIND_W);
        });
    });
}

/// The Find column of the panel: what to look for, how, and where.
fn find_column(ui: &mut egui::Ui, tab: &mut Tab, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>, focus: egui::Id, finding: bool, w: f32) {
    let th = c.theme;
    let local = tab.loc.local_path().is_some();
    let mut d = tab.find_draft.clone();
    let weak = |s: String| egui::RichText::new(s).size(11.5).color(th.weak);
    ui.label(weak(t("Find")));
    // What to look for.
    let hint = match d.mode {
        FindMode::Contains => t("Part of a name, e.g. report"),
        FindMode::Like => t("A pattern, e.g. IMG_*.jpg"),
        FindMode::Content => t("Text inside files, e.g. TODO"),
    };
    let field = ui.add(egui::TextEdit::singleline(&mut d.text).hint_text(hint).desired_width(w - 8.0));
    if ui.data_mut(|m| m.remove_temp::<bool>(focus)).unwrap_or(false) {
        field.request_focus();
    }
    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut d.mode, FindMode::Contains, t("Name contains"));
        ui.selectable_value(&mut d.mode, FindMode::Like, t("Name like"));
        ui.add_enabled_ui(local, |ui| {
            ui.selectable_value(&mut d.mode, FindMode::Content, t("Text in files")).on_disabled_hover_text(t("Works on folders of this computer"));
        });
    });
    let explain = match d.mode {
        FindMode::Contains => t("Names with the text anywhere in them."),
        FindMode::Like => t("The whole name fits the pattern: * or % stands for any text, ? for one character."),
        FindMode::Content => t("Lines inside files, like grep. Binary files are skipped; reading takes longer."),
    };
    ui.label(weak(explain));
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.subfolders, false, t("This folder"));
        ui.add_enabled_ui(local, |ui| {
            ui.radio_value(&mut d.subfolders, true, t("Subfolders too")).on_disabled_hover_text(t("Works on folders of this computer"));
        });
    });
    find_options(ui, &mut d, th, w);
    if !local {
        d.subfolders = false;
        if d.mode == FindMode::Content {
            d.mode = FindMode::Contains;
        }
    }
    // Heavy ones (reading files, every folder below) wait for Enter or Find.
    let heavy = d.subfolders || d.mode == FindMode::Content;
    ui.add_space(4.0);
    let mut find = enter;
    let mut clear = false;
    ui.horizontal(|ui| {
        if heavy {
            ui.label(weak(t("Press Enter or Find to start.")));
        } else {
            ui.label(weak(t("Applies as you type.")));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Real buttons, though the panel's other items are frameless menu rows.
            let primary = egui::Button::new(egui::RichText::new(t("Find")).color(th.on_accent())).fill(th.accent).frame(true).min_size(vec2(64.0, 24.0));
            find |= ui.add_enabled(!d.text.trim().is_empty(), primary).on_hover_text("Enter").clicked();
            let plain = egui::Button::new(t("Clear")).stroke(egui::Stroke::new(1.0_f32, th.border)).frame(true).min_size(vec2(56.0, 24.0));
            clear = ui.add_enabled(finding || !d.text.is_empty(), plain).clicked();
        });
    });
    if clear {
        d.text.clear();
        find = true;
    }
    let changed = d != tab.find_draft;
    tab.find_draft = d.clone();
    // Switched to one that waits for Find: the filter applied while typing is for
    // other choices, so the folder shows whole until then.
    if changed && heavy && matches!(&tab.filter, Ok(Filter::Find(f)) if !f.subfolders && f.mode != FindMode::Content) {
        tab.set_filter("", &c.list);
    }
    if find || (changed && !heavy) {
        ev.push(PaneEvent::Find(d));
        // A search shows its progress in the window: out of the way.
        if find && heavy && !clear {
            ui.close();
        }
    }
}

/// The Find panel's options, folded under one row that says which ones are changed:
/// files and folders shown, and how names compare (case, `-` `_` and spaces).
fn find_options(ui: &mut egui::Ui, d: &mut Find, th: &Theme, w: f32) {
    let id = egui::Id::new("find-options-open");
    let mut open = ui.data(|m| m.get_temp::<bool>(id)).unwrap_or(false);
    let (row, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, t("Options")));
    if resp.hovered() {
        ui.painter().rect_filled(row, 5.0, th.hover());
    }
    icons::chevron(ui.painter(), Rect::from_center_size(pos2(row.left() + 9.0, row.center().y), vec2(14.0, 14.0)), if open { 2 } else { 1 }, th.weak);
    let label = ui.painter().text(pos2(row.left() + 20.0, row.center().y), Align2::LEFT_CENTER, t("Options"), FontId::proportional(13.0), th.text);
    // Closed: what is not as usual, so a changed option is never forgotten.
    let mut changed: Vec<String> = Vec::new();
    // Text in files looks in files only, whatever is ticked: nothing to say then.
    match (d.files, d.folders) {
        _ if d.mode == FindMode::Content => {}
        (true, false) => changed.push(t("Files only")),
        (false, true) => changed.push(t("Folders only")),
        (false, false) => changed.push(t("Nothing shown")),
        _ => {}
    }
    if !d.ignore_case {
        changed.push(t("Match case"));
    }
    if !d.ignore_separators {
        changed.push(t("Exact - _ and spaces"));
    }
    if !changed.is_empty() {
        let mut job = egui::text::LayoutJob::simple_singleline(changed.join(" · "), FontId::proportional(11.5), th.accent);
        job.wrap = egui::text::TextWrapping::truncate_at_width((row.right() - label.right() - 16.0).max(10.0));
        let g = ui.fonts(|f| f.layout_job(job));
        ui.painter().galley(pos2(label.right() + 10.0, row.center().y - g.size().y / 2.0), g, th.accent);
    }
    if resp.on_hover_text(t("Files and folders shown, and how names compare")).clicked() {
        open = !open;
        ui.data_mut(|m| m.insert_temp(id, open));
    }
    if open {
        ui.indent("find-options", |ui| {
            ui.horizontal(|ui| {
                ui.label(t("Show"));
                ui.add_enabled_ui(d.mode != FindMode::Content, |ui| {
                    ui.checkbox(&mut d.files, t("Files")).on_disabled_hover_text(t("Text is looked for in files only"));
                    ui.checkbox(&mut d.folders, t("Folders")).on_disabled_hover_text(t("Text is looked for in files only"));
                });
            });
            ui.checkbox(&mut d.ignore_case, t("Ignore case")).on_hover_text(t("report also finds Report and REPORT"));
            ui.checkbox(&mut d.ignore_separators, t("Treat - _ and spaces as the same")).on_hover_text(t("my-file also finds my_file and my file"));
        });
    }
}

/// The quick filter column: kinds of files to show, ticked or unticked by a click (the
/// panel stays open, so that several can be chosen).
fn kinds_column(ui: &mut egui::Ui, tab: &mut Tab, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>, w: f32) {
    let th = c.theme;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(t("Show only")).size(11.5).color(th.weak));
        // Several ticked: one click unticks them all (the Find text stays).
        if tab.quick.as_ref().is_some_and(|q| q.names.len() > 1) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = egui::Button::new(egui::RichText::new(t("Untick all")).size(11.5).color(th.accent)).frame(false);
                if ui.add(b).on_hover_text(t("Show every kind of file again; the Find text stays")).clicked() {
                    ev.push(PaneEvent::QuickFilter(None));
                }
            });
        }
    });
    // Files of the listing by extension, to count each category.
    let mut by_ext: HashMap<&str, usize> = HashMap::new();
    for e in tab.entries.iter().filter(|e| !e.is_dir && (c.list.show_hidden || !e.hidden)) {
        *by_ext.entry(e.ext.as_str()).or_default() += 1;
    }
    // Two columns, so the list stays short under the Find options.
    let gap = 4.0;
    let col_w = ((w - gap) / 2.0).floor();
    let top = ui.cursor().top();
    let left = ui.cursor().left();
    let n = c.quick_filters.len();
    for (i, q) in c.quick_filters.iter().enumerate() {
        let exts = q.ext_list();
        let count: usize = exts.iter().map(|x| by_ext.get(x.as_str()).copied().unwrap_or(0)).sum();
        let at = pos2(left + (i % 2) as f32 * (col_w + gap), top + (i / 2) as f32 * 28.0);
        let row = Rect::from_min_size(at, vec2(col_w, 28.0));
        let resp = ui.interact(row, egui::Id::new(("quick-kind", i)), Sense::click());
        let sel = tab.quick.as_ref().is_some_and(|a| a.has(&q.name));
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, sel, t(&q.name)));
        menu_row_fill(ui, row, th, sel, resp.hovered());
        // The tick box.
        let bx = Rect::from_center_size(pos2(row.left() + 13.0, row.center().y), vec2(15.0, 15.0));
        if sel {
            ui.painter().rect_filled(bx, 3.5, th.accent);
            let s = egui::Stroke::new(1.8_f32, th.on_accent());
            ui.painter().line_segment([pos2(bx.left() + 3.5, bx.center().y + 0.2), pos2(bx.left() + 6.3, bx.bottom() - 4.0)], s);
            ui.painter().line_segment([pos2(bx.left() + 6.3, bx.bottom() - 4.0), pos2(bx.right() - 3.2, bx.top() + 4.0)], s);
        } else {
            ui.painter().rect_stroke(bx, 3.5, egui::Stroke::new(1.2_f32, th.weak), egui::StrokeKind::Inside);
        }
        quick_filter_icon(ui.painter(), Rect::from_center_size(pos2(row.left() + 36.0, row.center().y), vec2(16.0, 16.0)), &exts, th);
        // How many files here are of this kind (none: no number).
        let count_w = if count > 0 {
            let r = ui.painter().text(pos2(row.right() - 8.0, row.center().y), Align2::RIGHT_CENTER, count.to_string(), FontId::proportional(11.5), th.weak);
            r.width() + 6.0
        } else {
            0.0
        };
        let mut job = egui::text::LayoutJob::simple_singleline(q.display_name(), FontId::proportional(13.5), th.text);
        job.wrap = egui::text::TextWrapping::truncate_at_width((row.width() - 52.0 - 8.0 - count_w).max(10.0));
        let g = ui.fonts(|f| f.layout_job(job));
        ui.painter().galley(pos2(row.left() + 52.0, row.center().y - g.size().y / 2.0), g, th.text);
        let shown: Vec<String> = exts.iter().take(14).map(|e| format!(".{e}")).collect();
        let more = if exts.len() > 14 { " …" } else { "" };
        let tip = format!("{}{more}\n{count} {}", shown.join(" "), t("files here"));
        if resp.on_hover_text(tip).clicked() {
            ev.push(PaneEvent::QuickFilter(Some(i)));
        }
    }
    ui.allocate_space(vec2(w, n.div_ceil(2) as f32 * 28.0));
    if c.quick_filters.is_empty() {
        ui.label(egui::RichText::new(t("No quick filters yet")).color(th.weak));
    }
    ui.separator();
    // Unticking turns a kind off; the chip's × in the title row turns them all off.
    if ui.button(t("Edit quick filters…")).clicked() {
        ev.push(PaneEvent::EditQuickFilters);
        ui.close();
    }
}

/// Background of a row in a custom menu: hover shows on every row, the chosen
/// one included (a deeper shade of the selection colour there).
/// Plain rows use the hover colour of the other menu items (egui's), which
/// stands out from the popup background in every theme.
pub fn menu_row_fill(ui: &egui::Ui, r: Rect, th: &Theme, selected: bool, hovered: bool) {
    let fill = match (selected, hovered) {
        (true, true) => th.selection.lerp_to_gamma(th.accent, 0.22),
        (true, false) => th.selection,
        (false, true) => ui.visuals().widgets.hovered.weak_bg_fill,
        (false, false) => return,
    };
    ui.painter().rect_filled(r, 5.0, fill);
}

/// Make a popup look like the other menus: frameless items (egui's own menu
/// style, which plain popups do not get).
pub fn menu_style(ui: &mut egui::Ui) {
    let st = ui.style_mut();
    st.spacing.button_padding = vec2(2.0, 0.0);
    for w in [&mut st.visuals.widgets.active, &mut st.visuals.widgets.hovered, &mut st.visuals.widgets.inactive] {
        w.bg_stroke = egui::Stroke::NONE;
    }
    st.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
}

/// Icon of a quick-filter category: the file icon of its first extension.
pub fn quick_filter_icon(p: &egui::Painter, r: Rect, exts: &[String], th: &Theme) {
    match exts.first() {
        Some(e) => icons::file_icon(p, r, &format!("file.{e}"), false, e, th),
        None => icons::funnel(p, r, th.weak, false),
    }
}

/// The six-dot handle a mini window is dragged by: onto another window (swap,
/// or dock beside it) or onto the tab bar (a tab of its own, or into that tab).
fn window_grip(ui: &mut egui::Ui, uid: u64, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    let th = c.theme;
    let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 26.0), Sense::click_and_drag());
    let dragging = egui::DragAndDrop::payload::<crate::ui::tabs::WinDrag>(ui.ctx()).is_some_and(|d| d.uid == uid);
    let col = if resp.hovered() || dragging { th.strong() } else { th.weak.gamma_multiply(0.8) };
    if resp.hovered() || dragging {
        ui.painter().rect_filled(rect.shrink2(vec2(0.0, 3.0)), 4.0, th.hover());
    }
    for k in 0..3 {
        for j in 0..2 {
            let p = pos2(rect.center().x - 2.5 + 5.0 * j as f32, rect.center().y - 5.0 + 5.0 * k as f32);
            ui.painter().circle_filled(p, 1.3, col);
        }
    }
    if resp.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), crate::ui::tabs::WinDrag { uid });
    }
    if resp.hovered() && !egui::DragAndDrop::has_any_payload(ui.ctx()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if resp.clicked() {
        ev.push(PaneEvent::Focus);
    }
    resp.on_hover_text(t("Drag to move: onto another window to swap or dock it, onto the tab bar to make it a tab"));
    // The number its shortcut goes by.
    if c.window_number > 0 {
        let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 26.0), Sense::click());
        let badge = Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
        let (fill, text) = if c.focused { (th.accent, th.on_accent()) } else { (th.surface, th.weak) };
        ui.painter().rect_filled(badge, 4.0, fill);
        ui.painter().text(badge.center(), Align2::CENTER_CENTER, c.window_number.to_string(), FontId::new(11.0, egui::FontFamily::Name("bold".into())), text);
        if resp.clicked() {
            ev.push(PaneEvent::Focus);
        }
        let action = crate::ui::shortcuts::Action::FocusWindow((c.window_number - 1).min(3) as u8);
        resp.on_hover_text(format!("{} {} ({})", t("Window"), c.window_number, crate::ui::shortcuts::hint(action)));
        ui.add_space(4.0);
    }
}

/// Split menu and close button at the right end of a window's title row.
fn window_buttons(ui: &mut egui::Ui, terminal: bool, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    let th = c.theme;
    let sr = icons::icon_button(ui, 26.0, true, th, icons::mi_split).on_hover_text(t("Split window"));
    let id = sr.id.with("split-menu");
    if sr.clicked() {
        egui::Popup::toggle_id(ui.ctx(), id);
    }
    crate::ui::popup_below(id, &sr, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(200.0);
        menu_style(ui);
        let mut out = None;
        // The popup has the app's colours, so its items must too.
        let mut mc = c.clone();
        mc.theme = c.app_theme;
        split_items(ui, terminal, &mc, &mut out);
        if let Some(cmd) = out {
            ev.push(PaneEvent::Command(cmd));
        }
    });
    if c.pane_count > 1
        && icons::icon_button(ui, 26.0, true, th, |p, r, col| {
            let s = egui::Stroke::new(1.4_f32, col);
            let k = r.shrink(r.width() * 0.18);
            p.line_segment([k.left_top(), k.right_bottom()], s);
            p.line_segment([k.right_top(), k.left_bottom()], s);
        })
        .on_hover_text(format!("{} ({})", t("Close window"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::CloseWindow)))
        .clicked()
    {
        ev.push(PaneEvent::ClosePane);
    }
}

/// The info bar on top of every terminal window: what runs and where (the folder
/// written as `~/…`), and buttons for the folder, the screen, the font size and
/// the layout.
fn term_header(ui: &mut egui::Ui, tab: &Tab, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    let th = c.theme;
    let Some(term) = &tab.term else { return };
    let mut out = crate::terminal::view::TermResponse::default();
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        ui.add_space(SIDE_PAD - 4.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        if c.pane_count > 1 {
            window_grip(ui, tab.uid, c, ev);
        }
        let exited = term.exited;
        let (ir, _) = ui.allocate_exact_size(vec2(18.0, 26.0), Sense::hover());
        let icon_col = if exited.is_some() {
            th.danger
        } else if c.active {
            th.accent
        } else {
            th.weak
        };
        crate::ui::tabs::terminal_glyph(ui.painter(), Rect::from_center_size(ir.center(), vec2(14.0, 14.0)), icon_col);
        ui.add_space(4.0);

        // Labels get what the buttons leave.
        let agent = c.agent.and_then(|f| crate::system::agents::get(f.id));
        let buttons = 6.0 + if exited.is_some() { 1.0 } else { 0.0 } + if c.pane_count > 1 { 1.0 } else { 0.0 } + if agent.is_some() { 1.3 } else { 0.0 };
        let room = (ui.available_width() - buttons * 28.0 - SIDE_PAD).max(40.0);
        let (lr, lresp) = ui.allocate_exact_size(vec2(room, 26.0), Sense::click());
        let name = term.activity_label();
        let bold = FontId::new(13.0, egui::FontFamily::Name("bold".into()));
        let name_g = ui.fonts(|f| f.layout_no_wrap(name, bold.clone(), th.strong()));
        let name_w = name_g.size().x.min(room * 0.45);
        let mut job = egui::text::LayoutJob::simple_singleline(term.activity_label(), bold, if c.active { th.strong() } else { th.text });
        job.wrap = egui::text::TextWrapping::truncate_at_width(name_w + 1.0);
        let name_g = ui.fonts(|f| f.layout_job(job));
        let p = ui.painter();
        p.galley(pos2(lr.left(), lr.center().y - name_g.size().y / 2.0), name_g, th.text);
        let mut x = lr.left() + name_w + 10.0;
        if let Some(code) = exited {
            let g = ui.fonts(|f| f.layout_no_wrap(format!("{} ({code})", t("exited")), FontId::proportional(12.5), th.danger));
            p.galley(pos2(x, lr.center().y - g.size().y / 2.0), g.clone(), th.danger);
            x += g.size().x + 10.0;
        }
        // The folder: ~/ for home, middle folders shortened to fit.
        let full = crate::util::tilde_path(&term.cwd, dirs::home_dir().as_deref());
        let font = FontId::proportional(12.5);
        let path_room = (lr.right() - x).max(0.0);
        let cands = crate::util::short_path_candidates(&full);
        let pick = cands
            .iter()
            .find(|cand| ui.fonts(|f| f.layout_no_wrap((*cand).clone(), font.clone(), th.weak).size().x) <= path_room)
            .unwrap_or_else(|| cands.last().unwrap())
            .clone();
        let mut job = egui::text::LayoutJob::simple_singleline(pick, font, th.weak);
        job.wrap = egui::text::TextWrapping::truncate_at_width(path_room.max(4.0));
        let g = ui.fonts(|f| f.layout_job(job));
        p.galley(pos2(x, lr.center().y - g.size().y / 2.0), g, th.weak);
        if lresp.clicked() {
            ev.push(PaneEvent::Focus);
        }
        let cwd = term.cwd.clone();
        lresp.on_hover_text(cwd.display().to_string()).kept_context_menu(|ui| {
            if ui.button(t("Copy path")).clicked() {
                ui.ctx().copy_text(cwd.display().to_string());
                ui.close_kind(egui::UiKind::Menu);
            }
            if ui.button(t("Open folder in a file tab")).clicked() {
                out.open_dir = Some(cwd.clone());
                ui.close_kind(egui::UiKind::Menu);
            }
        });

        // Buttons: the AI button first, a little apart from the rest. It works only
        // while the shell waits at its prompt.
        if let Some(a) = agent {
            use crate::system::agents::AgentAct;
            let idle = term.is_idle();
            let r = icons::icon_button(ui, 26.0, idle, th, |p, r, col| crate::system::agents::paint_logo(p, r.shrink(0.5), a, col));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, idle, a.name));
            let r = r.on_hover_ui(|ui| {
                ui.label(egui::RichText::new(a.name).strong());
                ui.label(if idle { t("Start it in this terminal") } else { t("Available when the terminal is idle (no program running)") });
            });
            if r.clicked() && idle {
                ev.push(PaneEvent::Agent(AgentAct::Run(a.id)));
            }
            r.kept_context_menu(|ui| {
                if c.agents.len() > 1 {
                    for f in c.agents {
                        let Some(other) = crate::system::agents::get(f.id) else { continue };
                        if ui.radio(other.id == a.id, other.name).clicked() {
                            ev.push(PaneEvent::Agent(AgentAct::Choose(other.id)));
                            ui.close_kind(egui::UiKind::Menu);
                        }
                    }
                    ui.separator();
                }
                if ui.button(t("Hide the AI button")).clicked() {
                    ev.push(PaneEvent::Agent(AgentAct::HideButton));
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(t("AI button settings…")).clicked() {
                    ev.push(PaneEvent::Agent(AgentAct::Settings));
                    ui.close_kind(egui::UiKind::Menu);
                }
            });
            ui.add_space(8.0);
        }
        if icons::icon_button(ui, 26.0, true, th, icons::mi_open).on_hover_text(t("Show this folder in a file window")).clicked() {
            match (c.can_split[0], c.can_split[2]) {
                (true, _) => ev.push(PaneEvent::Command(Cmd::Split(SplitDir::Right, false))),
                (false, true) => ev.push(PaneEvent::Command(Cmd::Split(SplitDir::Down, false))),
                _ => out.open_dir = Some(term.cwd.clone()),
            }
        }
        let copied_key = egui::Id::new(("term-path-copied", tab.uid));
        let now = ui.input(|i| i.time);
        let copied = ui.ctx().data(|d| d.get_temp::<f64>(copied_key)).is_some_and(|at| now - at < 1.2);
        let ok_col = th.accent;
        let r = icons::icon_button(ui, 26.0, true, th, |p, r, col| if copied { check_glyph(p, r, ok_col) } else { icons::mi_copy(p, r, col) })
            .on_hover_text(if copied { t("Copied") } else { t("Copy path") });
        if r.clicked() {
            ui.ctx().copy_text(term.cwd.display().to_string());
            ui.ctx().data_mut(|d| d.insert_temp(copied_key, now));
        }
        if copied {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
        }
        if icons::icon_button(ui, 26.0, true, th, clear_glyph).on_hover_text(t("Clear the screen and scrollback")).clicked() {
            term.clear();
        }
        if exited.is_some() && icons::icon_button(ui, 26.0, true, th, icons::mi_refresh).on_hover_text(t("Restart (Enter)")).clicked() {
            out.restart = true;
        }
        if icons::icon_button(ui, 26.0, true, th, |p, r, col| letter_glyph(p, r, col, 10.0)).on_hover_text(t("Smaller text")).clicked() {
            out.font_delta -= 1.0;
        }
        if icons::icon_button(ui, 26.0, true, th, |p, r, col| letter_glyph(p, r, col, 15.0)).on_hover_text(t("Larger text")).clicked() {
            out.font_delta += 1.0;
        }
        window_buttons(ui, true, c, ev);
    });
    if out.restart || out.open_dir.is_some() || out.font_delta != 0.0 {
        ev.push(PaneEvent::Term(out));
    }
}

/// The info bar of a disk usage window: folder (up, `~/…`), totals, scan depth,
/// treemap or table, rescan, CSV export, and the window buttons.
fn du_header(ui: &mut egui::Ui, tab: &mut Tab, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    use crate::tools::diskusage::{DuAction, DuView, DEPTHS};
    let th = c.theme;
    let uid = tab.uid;
    let Some(du) = tab.du.as_mut() else { return };
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        ui.add_space(SIDE_PAD - 4.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        if c.pane_count > 1 {
            window_grip(ui, uid, c, ev);
        }
        if icons::icon_button(ui, 26.0, du.path.parent().is_some(), th, |p, r, col| icons::chevron(p, r, 0, col))
            .on_hover_text(t("Parent folder (Backspace)"))
            .clicked()
        {
            du.go_up();
        }
        let (ir, _) = ui.allocate_exact_size(vec2(20.0, 26.0), Sense::hover());
        icons::bars(ui.painter(), Rect::from_center_size(ir.center(), vec2(14.0, 14.0)), if c.active { th.accent } else { th.weak });
        ui.add_space(4.0);
        // Labels get what the controls leave.
        let controls = 26.0 * 3.0 + 70.0 + 150.0 + if c.pane_count > 1 { 56.0 } else { 28.0 };
        let room = (ui.available_width() - controls - SIDE_PAD).max(60.0);
        let (lr, lresp) = ui.allocate_exact_size(vec2(room, 26.0), Sense::click());
        let full = crate::util::tilde_path(&du.path, dirs::home_dir().as_deref());
        let summary = du.summary(&t);
        let bold = FontId::new(13.0, egui::FontFamily::Name("bold".into()));
        let small = FontId::proportional(12.5);
        let sum_w = ui.fonts(|f| f.layout_no_wrap(summary.clone(), small.clone(), th.weak).size().x).min(room * 0.5);
        let path_room = (room - sum_w - 12.0).max(20.0);
        let cands = crate::util::short_path_candidates(&full);
        let pick = cands
            .iter()
            .find(|s| ui.fonts(|f| f.layout_no_wrap((*s).clone(), bold.clone(), th.strong()).size().x) <= path_room)
            .unwrap_or_else(|| cands.last().unwrap())
            .clone();
        let mut job = egui::text::LayoutJob::simple_singleline(pick, bold, th.strong());
        job.wrap = egui::text::TextWrapping::truncate_at_width(path_room);
        let g = ui.fonts(|f| f.layout_job(job));
        let gw = g.size().x;
        ui.painter().galley(pos2(lr.left(), lr.center().y - g.size().y / 2.0), g, th.strong());
        let mut job = egui::text::LayoutJob::simple_singleline(summary, small, th.weak);
        job.wrap = egui::text::TextWrapping::truncate_at_width(sum_w + 1.0);
        let g = ui.fonts(|f| f.layout_job(job));
        ui.painter().galley(pos2(lr.left() + gw + 12.0, lr.center().y - g.size().y / 2.0), g, th.weak);
        if lresp.clicked() {
            ev.push(PaneEvent::Focus);
        }
        lresp.on_hover_text(du.path.display().to_string());

        // Depth of the scan.
        let label = |d: usize| if d == usize::MAX { t("All levels") } else { format!("{} {d}", t("Depth")) };
        let mut depth = du.depth;
        egui::ComboBox::from_id_salt(("du-depth", uid))
            .width(70.0)
            .selected_text(label(depth))
            .show_ui(ui, |ui| {
                for d in DEPTHS {
                    ui.selectable_value(&mut depth, d, label(d));
                }
            })
            .response
            .on_hover_text(t("How many folder levels the analysis shows (sizes always include everything below)"));
        if depth != du.depth {
            du.set_depth(depth);
        }
        // Treemap or table.
        ui.add_space(4.0);
        for (v, name) in [(DuView::Treemap, t("Treemap")), (DuView::Table, t("Table"))] {
            if ui.selectable_label(du.view == v, name).clicked() {
                du.view = v;
            }
        }
        ui.add_space(4.0);
        let scanning = du.is_scanning();
        let r = icons::icon_button(ui, 26.0, true, th, |p, r, col| if scanning { close_glyph(p, r, col) } else { icons::mi_refresh(p, r, col) });
        if r.on_hover_text(if scanning { t("Cancel") } else { t("Scan again") }).clicked() {
            if scanning {
                du.cancel();
            } else {
                du.rescan();
            }
        }
        if icons::icon_button(ui, 26.0, !scanning, th, icons::mi_new_file).on_hover_text(t("Save as CSV…")).clicked() {
            let name = format!("disk-usage-{}.csv", du.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "root".into()));
            if let Some(csv) = du.csv() {
                if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name(name).save_file() {
                    let msg = match std::fs::write(&p, csv) {
                        Ok(()) => DuAction::Message(format!("{} {}", t("Exported to"), p.display()), false),
                        Err(e) => DuAction::Message(e.to_string(), true),
                    };
                    ev.push(PaneEvent::Du(msg));
                }
            }
        }
        if icons::icon_button(ui, 26.0, true, th, icons::mi_open).on_hover_text(t("Show in a file tab")).clicked() {
            ev.push(PaneEvent::Du(DuAction::OpenFolder(du.path.clone())));
        }
        window_buttons(ui, false, c, ev);
    });
}

fn check_glyph(p: &egui::Painter, r: Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.6_f32, c);
    let a = |x: f32, y: f32| pos2(r.left() + r.width() * x, r.top() + r.height() * y);
    p.line_segment([a(0.2, 0.52), a(0.42, 0.74)], s);
    p.line_segment([a(0.42, 0.74), a(0.82, 0.3)], s);
}

/// A screen with a sweep across it: "clear".
fn clear_glyph(p: &egui::Painter, r: Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.3_f32, c);
    let a = |x: f32, y: f32| pos2(r.left() + r.width() * x, r.top() + r.height() * y);
    p.rect_stroke(Rect::from_min_max(a(0.08, 0.16), a(0.92, 0.84)), 2.0, s, egui::StrokeKind::Inside);
    p.line_segment([a(0.22, 0.38), a(0.5, 0.38)], s);
    p.line_segment([a(0.22, 0.56), a(0.4, 0.56)], s);
    p.line_segment([a(0.56, 0.7), a(0.8, 0.3)], s);
}

/// An "A" of the given size (font size buttons).
fn letter_glyph(p: &egui::Painter, r: Rect, c: egui::Color32, size: f32) {
    p.text(r.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, "A", FontId::new(size, egui::FontFamily::Name("bold".into())), c);
}

fn crumb_label(l: &Location, c: &PaneUiCtx) -> String {
    match l {
        Location::Local(p) => match p.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => p.to_string_lossy().to_string(),
        },
        Location::Remote { id, path } => {
            let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            if last.is_empty() {
                let label = remote_label(&c.list, *id);
                let host = label.split("://").nth(1).unwrap_or(&label);
                host.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_else(|| host.to_string())
            } else {
                last.to_string()
            }
        }
        Location::Archive { file, inner } => {
            if inner.is_empty() {
                file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            } else {
                inner.rsplit('/').next().unwrap_or("").to_string()
            }
        }
    }
}

fn breadcrumb(ui: &mut egui::Ui, tab: &mut Tab, side: usize, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    let th = c.theme;
    let width = ui.available_width();
    let path_id = egui::Id::new(("path", side));
    if tab.focus_path {
        tab.editing_path = true;
    }
    if tab.editing_path {
        address_edit(ui, tab, path_id, width, c, ev);
        return;
    }
    // Segments from the root to the current location.
    let mut segs: Vec<(String, Location)> = Vec::new();
    let mut cur = Some(tab.loc.clone());
    while let Some(l) = cur {
        if segs.len() > 64 {
            break;
        }
        let next = l.parent();
        segs.push((crumb_label(&l, c), l));
        cur = next;
    }
    segs.reverse();
    let font = FontId::proportional(14.0);
    // The current folder is drawn bold: measured with that font.
    let bold = FontId::new(13.5, egui::FontFamily::Name("bold".into()));
    let last = segs.len().saturating_sub(1);
    let widths: Vec<f32> = segs
        .iter()
        .enumerate()
        .map(|(i, (l, _))| ui.fonts(|f| f.layout_no_wrap(l.clone(), if i == last { bold.clone() } else { font.clone() }, th.text).size().x) + 12.0)
        .collect();
    let sep_w = 12.0;
    // The "…" button for the folders left out, and its separator.
    let more_w = 24.0 + sep_w;
    let mut first = last;
    let mut total = widths.last().copied().unwrap_or(0.0);
    while first > 0 {
        let more = if first > 1 { more_w } else { 0.0 };
        if total + widths[first - 1] + sep_w + more > width {
            break;
        }
        first -= 1;
        total += widths[first] + sep_w;
    }
    // In a narrow window the current folder comes first: the "…" only when there is
    // still room for a fair part of its name.
    let show_more = first > 0 && width - more_w >= total.min(90.0);
    // What is left for the current folder's name; cut with "…" when it does not fit.
    let last_w = (width - (total - widths.get(last).copied().unwrap_or(0.0)) - if show_more { more_w } else { 0.0 }).max(24.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if show_more {
            let (r, resp) = ui.allocate_exact_size(vec2(24.0, 26.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, 6.0, th.hover());
            }
            ui.painter().text(r.center(), Align2::CENTER_CENTER, "…", font.clone(), th.weak);
            let hidden: Vec<String> = segs[..first].iter().map(|s| s.0.clone()).collect();
            if resp.on_hover_text(hidden.join(" › ")).clicked() {
                ev.push(PaneEvent::Navigate(segs[first - 1].1.clone()));
            }
            sep(ui, th, sep_w);
        }
        for i in first..segs.len() {
            let (label, loc) = &segs[i];
            let w = if i == last { widths[i].min(last_w) } else { widths[i] };
            let (r, mut resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, 6.0, th.hover());
            }
            if let Some(pl) = drag_payload(ui.ctx()) {
                if ui.rect_contains_pointer(r) && drop_allowed(&pl, loc) {
                    ui.painter().rect_filled(r, 6.0, th.accent.gamma_multiply(0.18));
                    ui.painter().rect_stroke(r, 6.0, egui::Stroke::new(1.5_f32, th.accent), egui::StrokeKind::Inside);
                    ev.push(PaneEvent::DragOver(loc.clone()));
                    if ui.input(|i| i.pointer.any_released()) {
                        if let Some(pl) = egui::DragAndDrop::take_payload::<DragPayload>(ui.ctx()) {
                            ev.push(PaneEvent::Drop(pl, loc.clone()));
                        }
                    }
                }
            }
            let color = if i == last {
                if c.active {
                    th.strong()
                } else {
                    th.text
                }
            } else if resp.hovered() {
                th.text
            } else {
                th.weak
            };
            let f = if i == last { bold.clone() } else { font.clone() };
            let (galley, cut) = fit_text(ui, label, f, color, r.width() - 12.0);
            ui.painter().with_clip_rect(r).galley(pos2(r.left() + 6.0, r.center().y - galley.size().y / 2.0), galley, color);
            if cut {
                resp = resp.on_hover_text(label);
            }
            if resp.clicked() {
                if i == last {
                    tab.editing_path = true;
                    tab.focus_path = true;
                } else {
                    ev.push(PaneEvent::Navigate(loc.clone()));
                }
                ev.push(PaneEvent::Focus);
            }
            if i != last {
                sep(ui, th, sep_w);
            }
        }
        // Clicking the empty space switches to text editing.
        let rest = ui.available_width();
        if rest > 4.0 {
            let (_, resp) = ui.allocate_exact_size(vec2(rest, 26.0), Sense::click());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
            if resp.clicked() {
                tab.editing_path = true;
                tab.focus_path = true;
                ev.push(PaneEvent::Focus);
            }
        }
    });
}

/// The address bar as a text field, with the folders that match what is typed below it.
fn address_edit(ui: &mut egui::Ui, tab: &mut Tab, path_id: egui::Id, width: f32, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    use egui::{Key, Modifiers};
    let comp = &mut tab.complete;
    // A click on a suggestion opens it. The rows are the ones drawn in the last frame:
    // the text field gives up the keyboard as soon as the mouse is pressed elsewhere.
    let press = ui.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None });
    if let Some(k) = press.and_then(|p| comp.rows.iter().position(|r| r.contains(p))) {
        if let Some(s) = comp.items.get(k) {
            ev.push(PaneEvent::NavigateText(s.text.clone()));
            tab.path_text = s.text.clone();
            ui.memory_mut(|m| m.surrender_focus(path_id));
            comp.reset();
            tab.editing_path = false;
            return;
        }
    }
    // Keys for the list, taken before the text field sees them.
    let mut filled = false;
    if ui.memory(|m| m.has_focus(path_id)) && !comp.items.is_empty() {
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            comp.step(1);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
            comp.step(-1);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
            if let Some(text) = comp.tab(&tab.path_text) {
                tab.path_text = text;
                filled = true;
            }
        }
        if let Some(s) = comp.sel.and_then(|s| comp.items.get(s)) {
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                tab.path_text = s.text.clone();
            }
        }
    }
    if filled {
        // Go on typing at the end of what Tab put in.
        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), path_id) {
            let end = egui::text::CCursor::new(tab.path_text.chars().count());
            st.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            st.store(ui.ctx(), path_id);
        }
    }
    // lock_focus: Tab completes instead of moving the keyboard to another widget.
    let edit = egui::TextEdit::singleline(&mut tab.path_text)
        .id(path_id)
        .desired_width(width)
        .font(egui::TextStyle::Monospace)
        .margin(egui::Margin::symmetric(8, 5))
        .lock_focus(true);
    let r = ui.add(edit);
    if tab.focus_path {
        r.request_focus();
        tab.focus_path = false;
    }
    if r.lost_focus() {
        tab.editing_path = false;
        tab.complete.reset();
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            ev.push(PaneEvent::NavigateText(tab.path_text.clone()));
        } else {
            tab.path_text = tab.loc.display(|id| remote_label(&c.list, id));
        }
        return;
    }
    let comp = &mut tab.complete;
    comp.active |= r.changed() || filled;
    let base = match &tab.loc {
        Location::Local(p) => Some(p.as_path()),
        _ => None,
    };
    comp.update(&tab.path_text, dirs::home_dir().as_deref(), base, c.list.show_hidden, ui.ctx());
    comp.rows.clear();
    if comp.items.is_empty() || !r.has_focus() {
        return;
    }
    suggestion_popup(ui, path_id.with("suggestions"), r.rect, comp, c.app_theme);
}

/// The list of folders (and archives) suggested for a path field, under `anchor`
/// and above `parent`'s layer. Each row's place is kept in `comp.rows`.
pub fn suggestion_popup(parent: &egui::Ui, id: egui::Id, anchor: Rect, comp: &mut crate::ui::completion::Completer, th: &Theme) {
    let w = anchor.width().max(260.0);
    if parent.layer_id().order == egui::Order::Foreground {
        // Inside a dialog: keep the list above it.
        parent.ctx().set_sublayer(parent.layer_id(), egui::LayerId::new(egui::Order::Foreground, id));
    }
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(anchor.left_bottom() + vec2(0.0, 4.0)).show(parent.ctx(), |ui| {
        egui::Frame::popup(ui.style()).fill(th.panel).corner_radius(8).inner_margin(egui::Margin::same(4)).stroke(egui::Stroke::new(1.0_f32, th.border)).show(
            ui,
            |ui| {
                ui.set_width(w - 8.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, s) in comp.items.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                    let sel = comp.sel == Some(i);
                    if sel {
                        ui.painter().rect_filled(rect, 6.0, th.selection);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 6.0, th.hover());
                    }
                    let p = ui.painter().with_clip_rect(rect);
                    let ext = std::path::Path::new(&s.name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    icons::file_icon(&p, Rect::from_center_size(pos2(rect.left() + 15.0, rect.center().y), vec2(16.0, 16.0)), &s.name, !s.archive, &ext, th);
                    p.text(
                        pos2(rect.left() + 32.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        &s.name,
                        FontId::proportional(14.0),
                        if sel { th.strong() } else { th.text },
                    );
                    comp.rows.push(rect);
                }
            },
        );
    });
}

/// `text` on one line, cut with "…" to fit `width`; and whether it had to be cut.
fn fit_text(ui: &egui::Ui, text: &str, font: FontId, color: Color32, width: f32) -> (Arc<egui::Galley>, bool) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(4.0));
    let galley = ui.fonts(|f| f.layout_job(job));
    let cut = galley.elided;
    (galley, cut)
}

fn sep(ui: &mut egui::Ui, th: &Theme, w: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    ui.painter().text(r.center(), Align2::CENTER_CENTER, "›", FontId::proportional(14.0), th.weak.gamma_multiply(0.7));
}

fn list_and_footer(ui: &mut egui::Ui, tab: &mut Tab, side: usize, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    let th = c.theme;
    let grid = tab.shows_icons(&c.list);
    // Column header (the icon view has no columns: just the sort keys, side by side)
    let avail_w = ui.available_width();
    let (w_ext, w_size, w_date) = (64.0, 86.0, 132.0);
    let show_date = avail_w > 460.0;
    let show_ext = avail_w > 360.0;
    let icon_x = SIDE_PAD + 6.0;
    let name_x = icon_x + 26.0;
    let w_name = avail_w - name_x - w_size - SIDE_PAD - if show_ext { w_ext } else { 0.0 } - if show_date { w_date } else { 0.0 };
    let (hrect, _) = ui.allocate_exact_size(vec2(avail_w, 22.0), Sense::hover());
    let cols: Vec<(SortKey, String, f32, f32, Align2)> = if grid {
        [(SortKey::Name, t("Name")), (SortKey::Ext, t("Ext")), (SortKey::Size, t("Size")), (SortKey::Modified, t("Modified"))]
            .into_iter()
            .enumerate()
            .map(|(i, (key, label))| (key, label, icon_x + i as f32 * 92.0, 88.0, Align2::LEFT_CENTER))
            .collect()
    } else {
        let mut cols = vec![(SortKey::Name, t("Name"), name_x, w_name, Align2::LEFT_CENTER)];
        let mut x = name_x + w_name;
        if show_ext {
            cols.push((SortKey::Ext, t("Ext"), x, w_ext, Align2::LEFT_CENTER));
            x += w_ext;
        }
        cols.push((SortKey::Size, t("Size"), x, w_size, Align2::RIGHT_CENTER));
        x += w_size;
        if show_date {
            cols.push((SortKey::Modified, t("Modified"), x, w_date, Align2::RIGHT_CENTER));
        }
        cols
    };
    for (key, label, x, w, align) in &cols {
        let r = Rect::from_min_size(pos2(hrect.left() + x, hrect.top()), vec2(*w, hrect.height()));
        let resp = ui.interact(r, egui::Id::new(("hdr", side, *key as u8)), Sense::click());
        let sorted = tab.sort == *key;
        let arrow = if sorted {
            if tab.desc {
                "  ⏷"
            } else {
                "  ⏶"
            }
        } else {
            ""
        };
        let pos = if *align == Align2::LEFT_CENTER { r.left_center() } else { r.right_center() };
        ui.painter().text(
            pos,
            *align,
            format!("{label}{arrow}"),
            FontId::proportional(11.5),
            if resp.hovered() {
                th.text
            } else if sorted {
                th.accent
            } else {
                th.weak
            },
        );
        if resp.clicked() {
            ev.push(PaneEvent::Sort(*key));
        }
    }
    ui.painter().hline((hrect.left() + SIDE_PAD)..=(hrect.right() - SIDE_PAD), hrect.bottom(), egui::Stroke::new(1.0_f32, th.border));

    // Rows: one entry per line, or as many cells per line as fit.
    let per_line = if grid { ((avail_w - 2.0 * SIDE_PAD) / CELL_W).floor().max(1.0) as usize } else { 1 };
    let cell_w = if grid { (avail_w - 2.0 * SIDE_PAD) / per_line as f32 } else { avail_w };
    let line_h = if grid { CELL_H } else { ROW_H };
    tab.grid_cols = grid.then_some(per_line);
    let footer_h = 28.0;
    let list_h = (ui.available_height() - footer_h).max(line_h * 2.0);
    tab.view_height = list_h;
    if tab.ensure_visible {
        let y = (tab.cursor / per_line) as f32 * line_h;
        if y < tab.scroll_offset {
            tab.scroll_offset = y;
        } else if y + line_h > tab.scroll_offset + list_h {
            tab.scroll_offset = y + line_h - list_h;
        }
    }
    let mut sa = egui::ScrollArea::vertical().id_salt(("list", tab.uid, format!("{:?}", tab.loc))).auto_shrink([false, false]).max_height(list_h);
    if tab.ensure_visible {
        sa = sa.vertical_scroll_offset(tab.scroll_offset);
        tab.ensure_visible = false;
    } else if let Some(off) = tab.force_scroll.take() {
        sa = sa.vertical_scroll_offset(off);
    }
    let sa = sa.scroll_source(egui::scroll_area::ScrollSource { drag: false, ..egui::scroll_area::ScrollSource::ALL });
    let payload = drag_payload(ui.ctx());
    let mut row_target: Option<(Location, String)> = None;
    let mut drag_started: Option<(usize, Vec<Entry>)> = None;
    let mut clicked: Option<(usize, egui::Modifiers)> = None;
    let mut unmark = false;
    let mut double: Option<usize> = None;
    let mut context: Option<(usize, Cmd)> = None;
    let mut bg_context: Option<Cmd> = None;
    let mut menu_open = false;
    let mut bg_menu_open = false;
    let mut menu_for: Option<String> = None;
    let n = tab.view.len();
    let lines = n.div_ceil(per_line);
    // Nothing to show but ".." (which the icon view leaves out).
    let only_parent = !tab.view.iter().any(|&i| !tab.entries[i].is_parent);
    let out = ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let tab_ro: &Tab = tab;
        // One entry, in a row of the list or a cell of the icon view: its look, and what
        // the mouse does to it (the same in both views).
        let mut item = |ui: &mut egui::Ui, row: usize, rect: Rect, resp: egui::Response| {
            let tab = tab_ro;
            let e = &tab.entries[tab.view[row]];
            if resp.drag_started() && !e.is_parent {
                let items = if tab.marked.contains(&e.name) { tab.selection() } else { vec![e.clone()] };
                drag_started = Some((row, items));
            }
            let p = ui.painter();
            let is_cur = row == tab.cursor;
            let is_marked = tab.marked.contains(&e.name);
            let (pill, radius) = if grid { (rect.shrink2(vec2(4.0, 3.0)), 8.0) } else { (rect.shrink2(vec2(6.0, 1.0)), 6.0) };
            if is_marked {
                p.rect_filled(pill, radius, th.marked.gamma_multiply(if th.dark { 0.10 } else { 0.08 }));
            }
            if is_cur {
                if c.active {
                    p.rect_filled(pill, radius, th.selection);
                } else {
                    p.rect_stroke(pill, radius, egui::Stroke::new(1.0_f32, th.selection.lerp_to_gamma(th.text, 0.15)), egui::StrokeKind::Inside);
                }
            } else if resp.hovered() {
                p.rect_filled(pill, radius, th.hover());
            }
            if is_marked {
                let bar = if grid {
                    Rect::from_min_size(pos2(pill.left() + 3.0, pill.top() + 8.0), vec2(3.0, 14.0))
                } else {
                    Rect::from_min_size(pos2(pill.left() + 2.0, pill.center().y - 6.0), vec2(3.0, 12.0))
                };
                p.rect_filled(bar, 1.5, th.marked);
            }
            let is_arch = !e.is_dir && archive::is_archive(std::path::Path::new(&e.name));
            let is_sum = !e.is_dir && crate::tools::checksum::is_checksum_file(&e.name);
            let name_color = if is_marked {
                th.marked
            } else if e.is_parent || e.hidden {
                th.weak
            } else if is_cur && c.active {
                th.strong()
            } else {
                th.text
            };
            // On the lit cursor row every colour is made to read against it.
            let on_row = |col: egui::Color32| if is_cur && c.active { th.on_cursor(col) } else { col };
            let name_color = on_row(name_color);
            let size_txt = if e.is_parent {
                String::new()
            } else if e.is_dir && !e.size_known {
                "—".into()
            } else {
                human_size(e.size)
            };
            if grid {
                // A large icon (or the picture's thumbnail) with the whole name under it,
                // on two lines at most.
                let icon_rect = Rect::from_center_size(pos2(rect.center().x, rect.top() + 36.0), vec2(58.0, 58.0));
                let thumb = match (&tab.loc, c.thumbs) {
                    (Location::Local(dir), Some(thumbs)) if !e.is_dir && crate::preview::thumbs::has_thumbnail(&e.name) => {
                        thumbs.get(&dir.join(&e.name), e.modified, e.size)
                    }
                    _ => None,
                };
                match thumb {
                    Some(tex) => {
                        let area = Rect::from_center_size(icon_rect.center() - vec2(0.0, 1.0), vec2(cell_w - 20.0, 58.0));
                        let [w, h] = tex.size().map(|v| v.max(1) as f32);
                        let scale = (area.width() / w).min(area.height() / h);
                        let r = Rect::from_center_size(area.center(), vec2(w * scale, h * scale));
                        p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        p.rect_stroke(r, 0.0, egui::Stroke::new(1.0_f32, th.border), egui::StrokeKind::Outside);
                        if crate::preview::classify(&e.name) == crate::preview::Kind::Video {
                            // A video: a play mark in the corner.
                            let c0 = pos2(r.right() - 9.0, r.bottom() - 9.0);
                            p.circle_filled(c0, 7.0, Color32::from_black_alpha(150));
                            let tri = vec![c0 + vec2(-2.0, -3.5), c0 + vec2(-2.0, 3.5), c0 + vec2(3.5, 0.0)];
                            p.add(egui::Shape::convex_polygon(tri, Color32::WHITE, egui::Stroke::NONE));
                        }
                    }
                    None => icons::entry_icon(p, icon_rect, e, th),
                }
                let mut job = egui::text::LayoutJob::single_section(
                    e.name.clone(),
                    egui::TextFormat { font_id: FontId::proportional(12.5), color: name_color, ..Default::default() },
                );
                job.wrap = egui::text::TextWrapping { max_width: cell_w - 14.0, max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
                job.halign = egui::Align::Center;
                let galley = ui.fonts(|f| f.layout_job(job));
                p.galley(pos2(rect.center().x, rect.top() + 68.0), galley, name_color);
                if !e.is_parent && ui.rect_contains_pointer(rect) && payload.is_none() {
                    resp.clone().on_hover_text(format!("{}\n{} · {}", e.name, size_txt, format_time(e.modified)));
                }
            } else {
                let icon_rect = Rect::from_center_size(pos2(rect.left() + icon_x + 8.0, rect.center().y), vec2(17.0, 17.0));
                icons::entry_icon(p, icon_rect, e, th);
                // A git folder: its branch after the name.
                let git = match &tab.loc {
                    Location::Local(dir) if c.git_branches && e.is_dir && !e.is_parent => {
                        crate::files::gitinfo::lookup(ui.ctx(), &dir.join(&e.name), c.git_status)
                    }
                    _ => None,
                };
                let base_name = match e.name.rfind('.') {
                    Some(i) if show_ext && !e.is_dir && !e.ext.is_empty() && i > 0 => &e.name[..i],
                    _ => e.name.as_str(),
                };
                let name_rect = Rect::from_min_size(pos2(rect.left() + name_x, rect.top()), vec2(w_name - 8.0, ROW_H));
                // Too long for the column: cut with "…", and the whole name on hover.
                let (galley, cut) = fit_text(ui, base_name, FontId::proportional(14.0), name_color, name_rect.width());
                let name_drawn = Rect::from_min_size(pos2(name_rect.left(), name_rect.center().y - galley.size().y / 2.0), galley.size());
                p.with_clip_rect(name_rect).galley(name_drawn.min, galley, name_color);
                if cut && ui.rect_contains_pointer(name_rect) {
                    resp.clone().on_hover_text(&e.name);
                }
                if let Some(repo) = &git {
                    let filled = c.git_style == crate::config::GitStyle::Filled;
                    let color = branch_color(repo.state, c.git_status, filled, th);
                    if let Some(branch_rect) = branch_label(p, &repo.branch, name_rect, name_drawn.right(), c.git_label, filled, color) {
                        if ui.rect_contains_pointer(branch_rect) {
                            resp.clone().on_hover_text(branch_tooltip(repo, c.git_status));
                        }
                    }
                }
                let meta = FontId::proportional(13.0);
                let meta_color = on_row(if is_marked { th.marked.gamma_multiply(0.85) } else { th.weak });
                let mut x = rect.left() + name_x + w_name;
                if show_ext {
                    let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w_ext - 6.0, ROW_H));
                    p.with_clip_rect(r).text(r.left_center(), Align2::LEFT_CENTER, &e.ext, meta.clone(), meta_color);
                    x += w_ext;
                }
                p.text(pos2(x + w_size, rect.center().y), Align2::RIGHT_CENTER, size_txt, meta.clone(), meta_color);
                x += w_size;
                if show_date {
                    p.text(pos2(x + w_date, rect.center().y), Align2::RIGHT_CENTER, format_time(e.modified), meta, on_row(th.weak));
                }
            }
            if let Some(pl) = &payload {
                let dragged_here = pl.side == side && pl.loc == tab.loc && pl.items.iter().any(|i| i.name == e.name);
                if dragged_here {
                    p.rect_filled(rect, 0.0, th.bg.gamma_multiply(0.55));
                } else if e.is_dir && ui.rect_contains_pointer(rect) {
                    let target = if e.is_parent { tab.loc.parent() } else { Some(tab.loc.child(&e.name)) };
                    if let Some(target) = target.filter(|t| drop_allowed(pl, t)) {
                        p.rect_filled(pill, radius, th.accent.gamma_multiply(0.16));
                        p.rect_stroke(pill, radius, egui::Stroke::new(1.5_f32, th.accent), egui::StrokeKind::Inside);
                        row_target = Some((target, e.name.clone()));
                    }
                }
            }
            if resp.clicked() {
                clicked = Some((row, ui.input(|i| i.modifiers)));
            }
            if resp.double_clicked() {
                double = Some(row);
            }
            if resp.secondary_clicked() {
                menu_for = Some(e.name.clone());
            }
            // A right-click outside the marked rows is about that row alone (as in
            // Finder): the marks go, so the menu and its command name the same items.
            if resp.secondary_clicked() && !tab.marked.contains(&e.name) {
                clicked = Some((row, egui::Modifiers::NONE));
                // ".." has no commands that act on items: the marks stay.
                unmark = !e.is_parent;
            }
            let (entry_is_dir, entry_is_parent) = (e.is_dir, e.is_parent);
            resp.kept_context_menu(|ui| {
                menu_open = true;
                // The local folders the menu acts on: the marked ones when this row is
                // one of them, else this row (a right-click there unmarks the others).
                let dirs: Vec<String> = if !matches!(tab.loc, Location::Local(_)) || entry_is_parent {
                    vec![]
                } else if tab.marked.contains(&e.name) {
                    tab.selection().into_iter().filter(|x| x.is_dir).map(|x| x.name).collect()
                } else if entry_is_dir {
                    vec![e.name.clone()]
                } else {
                    vec![]
                };
                if let Some(cmd) = context_menu(ui, entry_is_dir, entry_is_parent, is_arch, is_sum, &dirs, c) {
                    context = Some((row, cmd));
                    ui.close_kind(egui::UiKind::Menu);
                }
            });
        };
        sa.show_rows(ui, line_h, lines, |ui, range| {
            for line in range {
                if grid {
                    let (line_rect, _) = ui.allocate_exact_size(vec2(avail_w, CELL_H), Sense::hover());
                    for k in 0..per_line {
                        let row = line * per_line + k;
                        if row >= n {
                            break;
                        }
                        let rect = Rect::from_min_size(pos2(line_rect.left() + SIDE_PAD + k as f32 * cell_w, line_rect.top()), vec2(cell_w, CELL_H));
                        let resp = ui.interact(rect, egui::Id::new(("cell", tab_ro.uid, row)), Sense::click_and_drag());
                        item(ui, row, rect, resp);
                    }
                    // The empty cells after the last icon are background, as below it.
                    let used = (n - line * per_line).min(per_line);
                    if used < per_line {
                        let x0 = line_rect.left() + SIDE_PAD + used as f32 * cell_w;
                        let rest = Rect::from_min_max(pos2(x0, line_rect.top()), pos2(line_rect.right(), line_rect.bottom()));
                        let bg = ui.interact(rest, egui::Id::new(("cells-rest", tab_ro.uid)), Sense::click());
                        if bg.clicked() {
                            ev.push(PaneEvent::Focus);
                        }
                        bg.kept_context_menu(|ui| {
                            bg_menu_open = true;
                            if let Some(cmd) = background_menu(ui, c) {
                                bg_context = Some(cmd);
                                ui.close_kind(egui::UiKind::Menu);
                            }
                        });
                    }
                } else {
                    let (rect, resp) = ui.allocate_exact_size(vec2(avail_w, ROW_H), Sense::click_and_drag());
                    item(ui, line, rect, resp);
                }
            }
            // Empty space below the rows, only when the rows do not fill the
            // view (measured in content coordinates so it never grows the
            // scrollable height).
            let rest = vec2(0.0, (list_h - lines as f32 * line_h - 1.0).max(0.0));
            if rest.y > 4.0 {
                let (r, bg) = ui.allocate_exact_size(vec2(avail_w, rest.y.max(ROW_H)), Sense::click());
                // A search under way says so (its progress is in the footer).
                if only_parent && tab_ro.loading.is_none() && tab_ro.error.is_none() {
                    // Empty state: an icon, what is going on and what one can do.
                    let searching = tab_ro.search.as_ref().is_some_and(|s| s.running) || tab_ro.is_scanning();
                    let (msg, hint) = if searching {
                        (t("Searching…"), String::new())
                    } else if tab_ro.is_filtered() {
                        (t("No matches"), t("Click the filter chip or press Esc to show everything"))
                    } else {
                        (t("This folder is empty"), t("Drop files here, or press F7 for a new folder"))
                    };
                    let top = r.top() + (r.height() * 0.18).clamp(28.0, 120.0);
                    let icon = Rect::from_center_size(pos2(r.center().x, top), vec2(40.0, 40.0));
                    let ic = th.weak.gamma_multiply(0.45);
                    if tab_ro.is_filtered() || searching {
                        icons::search(ui.painter(), icon, ic);
                    } else {
                        icons::mi_open(ui.painter(), icon, ic);
                    }
                    ui.painter().text(pos2(r.center().x, top + 38.0), Align2::CENTER_CENTER, msg, FontId::proportional(15.0), th.weak);
                    if !hint.is_empty() {
                        ui.painter().text(
                            pos2(r.center().x, top + 60.0),
                            Align2::CENTER_CENTER,
                            hint,
                            FontId::proportional(12.5),
                            th.weak.gamma_multiply(0.75),
                        );
                    }
                }
                if bg.clicked() {
                    ev.push(PaneEvent::Focus);
                }
                bg.kept_context_menu(|ui| {
                    bg_menu_open = true;
                    if let Some(cmd) = background_menu(ui, c) {
                        bg_context = Some(cmd);
                        ui.close_kind(egui::UiKind::Menu);
                    }
                });
            }
        })
    });
    if !tab.ensure_visible {
        tab.scroll_offset = out.inner.state.offset.y;
    }
    tab.menu_open = menu_open || bg_menu_open;
    if menu_for.is_some() {
        tab.menu_target = menu_for;
    }
    if let Some((row, items)) = drag_started {
        tab.cursor = row;
        egui::DragAndDrop::set_payload(ui.ctx(), DragPayload { side, loc: tab.loc.clone(), items });
    }
    // Drop handling: a folder row, otherwise the folder shown by this panel.
    let list_rect = out.inner.inner_rect;
    if let Some(pl) = &payload {
        if ui.rect_contains_pointer(list_rect) {
            let now = ui.input(|i| i.time);
            let target = match &row_target {
                Some((t, name)) => {
                    // Spring-loaded folders: hovering ~1 s opens the folder.
                    match &tab.spring {
                        Some((n, t0)) if n == name => {
                            if now - t0 > 1.0 {
                                tab.spring = None;
                                ev.push(PaneEvent::Navigate(t.clone()));
                            }
                        }
                        _ => tab.spring = Some((name.clone(), now)),
                    }
                    ui.ctx().request_repaint();
                    Some(t.clone())
                }
                None => {
                    tab.spring = None;
                    let t = tab.loc.clone();
                    drop_allowed(pl, &t).then(|| {
                        let p = ui.painter();
                        p.rect_filled(list_rect.shrink(3.0), 8.0, th.accent.gamma_multiply(0.05));
                        p.rect_stroke(list_rect.shrink(3.0), 8.0, egui::Stroke::new(1.5_f32, th.accent.gamma_multiply(0.8)), egui::StrokeKind::Inside);
                        t
                    })
                }
            };
            if let Some(t) = &target {
                ev.push(PaneEvent::DragOver(t.clone()));
            }
            // Auto-scroll near the edges.
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                let edge = 28.0;
                if pos.y < list_rect.top() + edge {
                    tab.force_scroll = Some((tab.scroll_offset - 12.0).max(0.0));
                    ui.ctx().request_repaint();
                } else if pos.y > list_rect.bottom() - edge {
                    tab.force_scroll = Some(tab.scroll_offset + 12.0);
                    ui.ctx().request_repaint();
                }
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let Some(t) = target {
                    if let Some(pl) = egui::DragAndDrop::take_payload::<DragPayload>(ui.ctx()) {
                        ev.push(PaneEvent::Drop(pl, t));
                    }
                }
            }
        } else {
            tab.spring = None;
        }
    } else {
        tab.spring = None;
    }
    if out.response.hovered() && ui.input(|i| i.pointer.any_pressed()) {
        ev.push(PaneEvent::Focus);
    }
    if let Some((row, m)) = clicked {
        ev.push(PaneEvent::Focus);
        if m.command {
            tab.cursor = row;
            tab.end_range();
            tab.toggle_mark_cursor();
        } else if m.shift {
            tab.extend_to(row);
        } else {
            tab.cursor = row;
            tab.end_range();
            if unmark {
                tab.marked.clear();
            }
        }
    }
    if let Some(row) = double {
        tab.cursor = row;
        ev.push(PaneEvent::Open);
    }
    if let Some((row, cmd)) = context {
        // The entry the menu was opened on, found by name: if the folder was reloaded
        // while the menu was open, the row may now hold another file. Gone: nothing.
        let at = match tab.menu_target.take() {
            Some(name) => tab.pos_of(&name),
            None => Some(row),
        };
        if let Some(at) = at {
            tab.cursor = at;
            ev.push(PaneEvent::Focus);
            ev.push(PaneEvent::Command(cmd));
        }
    }
    if let Some(cmd) = bg_context {
        ev.push(PaneEvent::Focus);
        ev.push(PaneEvent::Command(cmd));
    }

    // Footer
    let (frect, _) = ui.allocate_exact_size(vec2(avail_w, footer_h), Sense::hover());
    let p = ui.painter();
    p.hline((frect.left() + SIDE_PAD)..=(frect.right() - SIDE_PAD), frect.top(), egui::Stroke::new(1.0_f32, th.border));
    let small = FontId::proportional(11.5);
    let left = pos2(frect.left() + SIDE_PAD + 4.0, frect.center().y);
    // A search or a filter reading files: how far it is, the time left, and Stop.
    let job = match (&tab.search, &tab.scan) {
        (Some(s), _) if s.running || s.stopped => Some((s.running, s.progress.clone(), s.started, t("Searching subfolders…"))),
        (_, Some(s)) if s.running || s.stopped => Some((s.running, s.progress.clone(), s.started, t("Reading files…"))),
        _ => None,
    };
    if let Some((running, progress, started, what)) = job {
        let fraction = progress.fraction();
        let found = crate::util::fmt_count(progress.found());
        let right = frect.right() - SIDE_PAD - 4.0;
        let mut x = left.x;
        if running {
            let g = p.layout_no_wrap(what, small.clone(), th.text);
            p.galley(pos2(x, frect.center().y - g.size().y / 2.0), g.clone(), th.text);
            x += g.size().x + 10.0;
            // The bar, then what it means in words.
            let bar = Rect::from_min_size(pos2(x, frect.center().y - 3.0), vec2(110.0, 6.0));
            p.rect_filled(bar, 3.0, th.surface);
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * fraction, bar.height())), 3.0, th.accent);
            x = bar.right() + 10.0;
            let left_txt = match crate::ui::search::time_left(started.elapsed(), fraction) {
                Some(d) if d.as_secs() >= 60 => format!("{} {} {}", t("about"), d.as_secs().div_ceil(60), t("min left")),
                Some(d) => format!("{} {} {}", t("about"), d.as_secs().max(1), t("s left")),
                None => t("estimating…"),
            };
            let checked = crate::util::fmt_count(progress.checked());
            let info = format!("{:.0}% · {left_txt} · {found} {} · {checked} {}", fraction * 100.0, t("found"), t("checked"));
            // Stop, at the right end.
            let stop = Rect::from_min_size(pos2(right - 64.0, frect.center().y - 10.0), vec2(64.0, 20.0));
            let resp = ui.interact(stop, egui::Id::new(("stop-search", tab.uid)), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t("Stop")));
            p.rect(
                stop,
                5.0,
                if resp.hovered() { th.danger.gamma_multiply(0.18) } else { th.surface },
                egui::Stroke::new(1.0_f32, th.border),
                egui::StrokeKind::Inside,
            );
            let sq = Rect::from_center_size(pos2(stop.left() + 13.0, stop.center().y), vec2(8.0, 8.0));
            p.rect_filled(sq, 1.5, th.danger);
            p.text(pos2(stop.left() + 23.0, stop.center().y), Align2::LEFT_CENTER, t("Stop"), small.clone(), th.text);
            let clip = Rect::from_min_max(pos2(x, frect.top()), pos2(stop.left() - 8.0, frect.bottom()));
            p.with_clip_rect(clip).text(pos2(x, frect.center().y), Align2::LEFT_CENTER, info, small.clone(), th.weak);
            if resp.on_hover_text(format!("{} (Esc)", t("Stop searching; what was found so far stays"))).clicked() {
                tab.stop_search();
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            let txt = format!("{} {:.0}% · {found} {}   ·   {}", t("Stopped at"), fraction * 100.0, t("found"), t("Esc shows the folder again"));
            p.with_clip_rect(frect).text(left, Align2::LEFT_CENTER, txt, small.clone(), th.marked);
        }
        return;
    }
    if tab.is_loading() {
        p.text(left, Align2::LEFT_CENTER, t("Loading…"), small.clone(), th.weak);
        ui.ctx().request_repaint();
    } else if let Some(err) = &tab.error {
        p.with_clip_rect(frect).text(left, Align2::LEFT_CENTER, format!("⚠ {err}"), small.clone(), th.danger);
    } else if let Err(e) = &tab.filter {
        p.with_clip_rect(frect).text(left, Align2::LEFT_CENTER, format!("⚠ {e}"), small.clone(), th.danger);
    } else if tab.is_scanning() {
        let found = tab.view.iter().filter(|&&i| !tab.entries[i].is_parent).count();
        p.with_clip_rect(frect).text(left, Align2::LEFT_CENTER, format!("{} · {found} {}", t("Searching file contents…"), t("found")), small.clone(), th.weak);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    } else {
        let (files, dirs) =
            tab.view.iter().map(|&i| &tab.entries[i]).filter(|e| !e.is_parent).fold((0, 0), |(f, d), e| if e.is_dir { (f, d + 1) } else { (f + 1, d) });
        let mut s = format!("{dirs} {} · {files} {}", t("folders"), t("files"));
        // Only the marks the filter shows: the ones commands act on.
        let marked = tab.marked_shown();
        if !marked.is_empty() {
            let size: u64 = marked.iter().map(|e| e.size).sum();
            s = format!("{} {} · {}   ·   {s}", marked.len(), t("selected"), human_size(size));
        }
        if !tab.filter.as_ref().map(|f| f.is_all()).unwrap_or(true) {
            s.push_str(&format!("   ·   {} {}", t("filtered from"), tab.entries.len().saturating_sub(1)));
        }
        p.with_clip_rect(frect).text(left, Align2::LEFT_CENTER, s, small.clone(), if marked.is_empty() { th.weak } else { th.marked });
    }
    if let Some(path) = tab.loc.local_path().filter(|_| c.show_free) {
        if let Some((total, avail)) = crate::files::vfs::free_space(path, c.drives) {
            let right = frect.right() - SIDE_PAD - 4.0;
            let bar = Rect::from_min_size(pos2(right - 48.0, frect.center().y - 2.0), vec2(48.0, 4.0));
            let used = if total > 0 { 1.0 - avail as f32 / total as f32 } else { 0.0 };
            p.rect_filled(bar, 2.0, th.surface);
            let fill = if used > 0.9 { th.danger } else { th.accent.gamma_multiply(0.8) };
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * used, bar.height())), 2.0, fill);
            p.text(pos2(bar.left() - 8.0, frect.center().y), Align2::RIGHT_CENTER, format!("{} {}", human_size(avail), t("free")), small, th.weak);
        }
    }
}

/// Shared tail of every context menu: terminals and pane layout.
type MenuIcon = fn(&egui::Painter, Rect, egui::Color32);

/// Label with room on the left for an icon.
fn menu_text(ui: &egui::Ui, label: &str, color: egui::Color32) -> egui::WidgetText {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut job = egui::text::LayoutJob::simple_singleline(label.to_string(), font, color);
    if let Some(sec) = job.sections.first_mut() {
        sec.leading_space = 24.0;
    }
    job.into()
}

fn paint_menu_icon(ui: &egui::Ui, rect: Rect, icon: MenuIcon, color: egui::Color32) {
    let x = rect.left() + ui.spacing().button_padding.x + 8.0;
    icon(ui.painter(), Rect::from_center_size(pos2(x, rect.center().y), vec2(14.0, 14.0)), color);
}

/// A menu entry with an icon and a shortcut. `danger` paints it red.
fn mitem(ui: &mut egui::Ui, th: &Theme, icon: MenuIcon, label: &str, key: &str, danger: bool) -> bool {
    let col = if danger { th.danger } else { th.text };
    let r = ui.add(egui::Button::new(menu_text(ui, label, col)).shortcut_text(egui::RichText::new(key).color(th.weak)));
    paint_menu_icon(ui, r.rect, icon, if danger { th.danger } else { th.weak });
    if r.clicked() {
        ui.close_kind(egui::UiKind::Menu);
        return true;
    }
    false
}

/// A submenu with an icon.
fn msub(ui: &mut egui::Ui, th: &Theme, icon: MenuIcon, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    let r = ui.menu_button(menu_text(ui, label, th.text), |ui| {
        ui.set_min_width(200.0);
        add(ui)
    });
    paint_menu_icon(ui, r.response.rect, icon, th.weak);
}

/// Terminal and pane-layout submenus shared by every context menu. `dirs`: the selected
/// folders the Terminal submenu also offers (none on the empty part of a folder).
fn layout_items(ui: &mut egui::Ui, c: &PaneUiCtx, dirs: &[String], out: &mut Option<Cmd>) {
    let th = c.theme;
    msub(ui, th, crate::ui::tabs::terminal_glyph, &t("Terminal"), |ui| {
        if mitem(
            ui,
            th,
            crate::ui::tabs::terminal_glyph,
            &t("Open terminal here"),
            crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::OpenTerminal),
            false,
        ) {
            *out = Some(Cmd::Terminal);
        }
        let label = match dirs {
            [] => None,
            [one] => Some(t("Open terminal in “{name}”").replace("{name}", &short_name(one, 28))),
            many => Some(t("Open terminals in {n} folders…").replace("{n}", &many.len().to_string())),
        };
        if let Some(label) = label {
            if mitem(ui, th, crate::ui::tabs::terminal_glyph, &label, "", false) {
                *out = Some(Cmd::TerminalInSelection);
            }
        }
        if c.terminal_profiles.len() > 1 {
            ui.separator();
            for (i, name) in c.terminal_profiles.iter().enumerate() {
                if mitem(ui, th, crate::ui::tabs::terminal_glyph, name, "", false) {
                    *out = Some(Cmd::TerminalProfile(i));
                }
            }
        }
    });
    msub(ui, th, icons::mi_split, &t("Layout"), |ui| split_items(ui, false, c, out));
}

/// One entry of a split menu: what it draws, says and does.
pub struct SplitEntry {
    pub icon: MenuIcon,
    pub label: String,
    pub key: String,
    pub cmd: Cmd,
    /// A line goes above it.
    pub group: bool,
}

/// The split menu of a window (`terminal`: a terminal window), as entries: a window of
/// the same kind on each side, the other kind to the right or below, and closing it.
/// `can_split` is right, left, down, up; `panes` the windows of the tab.
pub fn split_entries(terminal: bool, can_split: [bool; 4], panes: usize) -> Vec<SplitEntry> {
    use crate::ui::shortcuts::{hint, Action};
    use icons::Side;
    let e = |icon: fn(&egui::Painter, Rect, egui::Color32), label: String, key: String, cmd: Cmd, group: bool| SplitEntry { icon, label, key, cmd, group };
    let mut v = Vec::new();
    let same: [(fn(&egui::Painter, Rect, egui::Color32), String, Action, SplitDir); 4] = [
        (|p, r, c| icons::split_half(p, r, c, Side::Right, false), t("Split right"), Action::SplitRight, SplitDir::Right),
        (|p, r, c| icons::split_half(p, r, c, Side::Left, false), t("Split left"), Action::SplitLeft, SplitDir::Left),
        (|p, r, c| icons::split_half(p, r, c, Side::Bottom, false), t("Split down"), Action::SplitDown, SplitDir::Down),
        (|p, r, c| icons::split_half(p, r, c, Side::Top, false), t("Split up"), Action::SplitUp, SplitDir::Up),
    ];
    for (k, (icon, label, action, dir)) in same.into_iter().enumerate() {
        if can_split[k] {
            v.push(e(icon, label, hint(action).to_string(), Cmd::Split(dir, terminal), false));
        }
    }
    // The other kind: a terminal beside files, or files beside a terminal.
    let (right, below) = if terminal { (t("Files to the right"), t("Files below")) } else { (t("Terminal to the right"), t("Terminal below")) };
    let (ri, bi): (fn(&egui::Painter, Rect, egui::Color32), fn(&egui::Painter, Rect, egui::Color32)) = if terminal {
        (|p, r, c| icons::split_half(p, r, c, Side::Right, false), |p, r, c| icons::split_half(p, r, c, Side::Bottom, false))
    } else {
        (|p, r, c| icons::split_half(p, r, c, Side::Right, true), |p, r, c| icons::split_half(p, r, c, Side::Bottom, true))
    };
    let mut group = !v.is_empty();
    if can_split[0] {
        v.push(e(ri, right, String::new(), Cmd::Split(SplitDir::Right, !terminal), group));
        group = false;
    }
    if can_split[2] {
        v.push(e(bi, below, String::new(), Cmd::Split(SplitDir::Down, !terminal), group));
    }
    if panes > 1 {
        v.push(e(close_glyph, t("Close window"), hint(Action::CloseWindow).to_string(), Cmd::ClosePane, !v.is_empty()));
    }
    v
}

/// Split this window (same kind, or the other kind beside it) and close it.
pub fn split_items(ui: &mut egui::Ui, terminal: bool, c: &PaneUiCtx, out: &mut Option<Cmd>) {
    split_menu(ui, c.theme, &split_entries(terminal, c.can_split, c.pane_count), out);
}

/// The entries of a split menu, with "at most 2 × 2" when no split is left.
pub fn split_menu(ui: &mut egui::Ui, th: &Theme, entries: &[SplitEntry], out: &mut Option<Cmd>) {
    if !entries.iter().any(|e| matches!(e.cmd, Cmd::Split(..))) {
        ui.label(egui::RichText::new(t("At most 2 columns and 2 rows of panes")).size(11.5).color(th.weak));
    }
    for e in entries {
        if e.group {
            ui.separator();
        }
        if mitem(ui, th, e.icon, &e.label, &e.key, false) {
            *out = Some(e.cmd);
        }
    }
}

/// `name` cut to `max` characters, with "…" when it is longer.
fn short_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    format!("{}…", name.chars().take(max - 1).collect::<String>())
}

fn close_glyph(p: &egui::Painter, r: Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.3_f32, c);
    let k = r.shrink(r.width() * 0.2);
    p.line_segment([k.left_top(), k.right_bottom()], s);
    p.line_segment([k.right_top(), k.left_bottom()], s);
}

fn context_menu(ui: &mut egui::Ui, is_dir: bool, is_parent: bool, is_arch: bool, is_sum: bool, dirs: &[String], c: &PaneUiCtx) -> Option<Cmd> {
    let th = c.theme;
    ui.set_min_width(250.0);
    let mut out = None;
    let pick = |hit: bool, cmd: Cmd, out: &mut Option<Cmd>| {
        if hit {
            *out = Some(cmd);
        }
    };
    if is_parent {
        pick(mitem(ui, th, icons::mi_open, &t("Open"), "Enter", false), Cmd::Open, &mut out);
        new_submenu(ui, th, &mut out);
        ui.separator();
        layout_items(ui, c, &[], &mut out);
        return out;
    }
    pick(mitem(ui, th, icons::mi_open, &t("Open"), "Enter", false), Cmd::Open, &mut out);
    if !is_dir {
        msub(ui, th, icons::mi_external, &t("Open with"), |ui| {
            pick(
                mitem(ui, th, icons::mi_external, &t("Default app"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::OpenDefault), false),
                Cmd::OpenDefault,
                &mut out,
            );
            pick(mitem(ui, th, icons::mi_more, &t("Choose program…"), "", false), Cmd::OpenWith, &mut out);
            pick(mitem(ui, th, icons::mi_view, &t("View"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::View), false), Cmd::View, &mut out);
            ui.separator();
            pick(mitem(ui, th, icons::mi_more, &t("Manage default apps…"), "", false), Cmd::DefaultApps, &mut out);
        });
    }
    new_submenu(ui, th, &mut out);
    ui.separator();
    pick(mitem(ui, th, icons::mi_copy, &t("Copy to other panel"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Copy), false), Cmd::Copy, &mut out);
    pick(mitem(ui, th, icons::mi_move, &t("Move to other panel"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Move), false), Cmd::Move, &mut out);
    pick(mitem(ui, th, icons::mi_rename, &t("Rename"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Rename), false), Cmd::Rename, &mut out);
    pick(mitem(ui, th, icons::mi_trash, &t("Delete"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Delete), true), Cmd::Delete, &mut out);
    ui.separator();
    msub(ui, th, icons::mi_archive, &t("Archive"), |ui| {
        pick(mitem(ui, th, icons::mi_archive, &t("Compress…"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Compress), false), Cmd::Pack, &mut out);
        if is_arch {
            pick(mitem(ui, th, icons::mi_open, &t("Extract here"), "", false), Cmd::ExtractHere, &mut out);
            pick(
                mitem(ui, th, icons::mi_move, &t("Extract to other panel"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::ExtractOther), false),
                Cmd::ExtractOther,
                &mut out,
            );
        }
    });
    pick(mitem(ui, th, icons::mi_link, &t("Copy path"), "", false), Cmd::CopyPath, &mut out);
    msub(ui, th, icons::mi_more, &t("More"), |ui| {
        pick(
            mitem(ui, th, icons::mi_rename, &t("Batch rename…"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::BatchRename), false),
            Cmd::BatchRename,
            &mut out,
        );
        let label = if is_sum { t("Verify checksums") } else { t("Checksum…") };
        pick(mitem(ui, th, icons::mi_checksum, &label, crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Checksum), false), Cmd::Checksum, &mut out);
        pick(mitem(ui, th, icons::mi_external, &t("Show in system file manager"), "", false), Cmd::Reveal, &mut out);
        if is_dir {
            pick(mitem(ui, th, icons::mi_star, &t("Add to bookmarks"), "", false), Cmd::BookmarkItem, &mut out);
        }
    });
    if is_dir && !is_parent {
        pick(mitem(ui, th, icons::mi_disk, &t("Analyze disk usage"), "", false), Cmd::DiskUsage, &mut out);
    }
    pick(
        mitem(ui, th, icons::mi_info, &t("Properties"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Properties), false),
        Cmd::Properties,
        &mut out,
    );
    ui.separator();
    layout_items(ui, c, dirs, &mut out);
    out
}

/// "New ▸ Folder / Blank file" submenu.
fn new_submenu(ui: &mut egui::Ui, th: &Theme, out: &mut Option<Cmd>) {
    msub(ui, th, icons::mi_new_file, &t("New"), |ui| {
        if mitem(ui, th, icons::mi_new_folder, &t("Folder…"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::NewFolder), false) {
            *out = Some(Cmd::NewFolder);
        }
        if mitem(ui, th, icons::mi_new_file, &t("Blank file…"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::NewFile), false) {
            *out = Some(Cmd::NewFile);
        }
    });
}

/// Right-click on the empty part of a folder.
fn background_menu(ui: &mut egui::Ui, c: &PaneUiCtx) -> Option<Cmd> {
    let th = c.theme;
    ui.set_min_width(250.0);
    let mut out = None;
    new_submenu(ui, th, &mut out);
    ui.separator();
    for (icon, cmd, label, key) in [
        (icons::mi_refresh as MenuIcon, Cmd::Refresh, t("Refresh"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Refresh)),
        (icons::mi_select, Cmd::SelectAll, t("Select all"), crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::SelectAll)),
    ] {
        if mitem(ui, th, icon, &label, key, false) {
            out = Some(cmd);
        }
    }
    msub(ui, th, icons::mi_disk, &t("Folder sizes"), |ui| {
        if mitem(ui, th, icons::mi_disk, &t("Calculate folder sizes"), "", false) {
            out = Some(Cmd::CalcSizes);
        }
        if mitem(ui, th, icons::mi_disk, &t("Disk usage of this folder"), "", false) {
            out = Some(Cmd::DiskUsage);
        }
    });
    ui.separator();
    layout_items(ui, c, &[], &mut out);
    out
}

pub fn filter_help() -> String {
    let rows = [
        ("report", t("name contains 'report'")),
        ("*.rs   img_??.png", t("glob patterns")),
        (".jpg .png", t("by extension")),
        ("name LIKE '%2024%'", t("% = any characters, _ = one character")),
        ("ext IN ('rs', 'toml')", t("extension in a list")),
        ("size > 10MB AND type = 'file'", t("size and type")),
        ("age < 7d", t("modified within 7 days (s, m, h, d, w, mo, y)")),
        ("modified >= '2024-01-01'", t("modified since a date")),
        ("NOT name LIKE '.%'", t("negation; combine with AND, OR, ( )")),
        (r"name REGEXP '^img_\d+'", t("regular expression (~ minds case, !~ for no match)")),
        ("source LIKE '%TODO%'", t("a line of the file matches, like grep (also REGEXP)")),
    ];
    let mut s = format!("{}\n", t("Quick filter syntax"));
    for (example, desc) in rows {
        s.push_str(&format!("  {example:<31} {desc}\n"));
    }
    s.push_str(&format!("{}: name, ext, size, type, modified, age, hidden, source", t("Fields")));
    s
}

#[cfg(test)]
mod dnd_tests {
    use super::*;
    #[test]
    fn drop_rules() {
        let src = Location::Local(PathBuf::from("/a"));
        let p = DragPayload { side: 0, loc: src.clone(), items: vec![Entry::new("dir".into(), true, 0, None), Entry::new("f.txt".into(), false, 1, None)] };
        assert!(!drop_allowed(&p, &src), "same folder");
        assert!(!drop_allowed(&p, &Location::Local(PathBuf::from("/a/dir"))), "into itself");
        assert!(!drop_allowed(&p, &Location::Local(PathBuf::from("/a/dir/sub/deep"))), "into a descendant");
        assert!(drop_allowed(&p, &Location::Local(PathBuf::from("/a/other"))));
        assert!(drop_allowed(&p, &Location::Local(PathBuf::from("/"))));
        assert!(!drop_allowed(&p, &Location::Archive { file: PathBuf::from("/x.zip"), inner: String::new() }));
    }
}

#[cfg(test)]
mod quick_filter_tests {
    use super::*;

    fn wait(tab: &mut Tab, ctx: &ListCtx) {
        let t0 = std::time::Instant::now();
        while tab.is_loading() && t0.elapsed().as_secs() < 5 {
            tab.poll(ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn shown(tab: &Tab) -> Vec<String> {
        tab.view.iter().map(|&i| &tab.entries[i]).filter(|e| !e.is_parent).map(|e| e.name.clone()).collect()
    }

    #[test]
    fn category_keeps_folders_and_stays_on() {
        let dir = std::env::temp_dir().join(format!("mf-qf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["a.jpg", "b.PNG", "c.txt", "sub/d.gif", "sub/e.rs"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let remotes = HashMap::new();
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        wait(&mut tab, &ctx);
        tab.set_quick(QuickActive::from_names(&["Images".to_string()], &crate::config::default_quick_filters()), &ctx);
        assert_eq!(shown(&tab), ["sub", "a.jpg", "b.PNG"], "folders stay, upper-case extensions match");
        assert!(tab.is_filtered());
        // Unlike the text filter, the category survives opening a folder.
        tab.navigate(Location::Local(dir.join("sub")), &ctx);
        wait(&mut tab, &ctx);
        assert_eq!(shown(&tab), ["d.gif"]);
        tab.clear_search(&ctx);
        assert!(tab.quick.is_none());
        assert_eq!(shown(&tab), ["d.gif", "e.rs"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_categories_at_once() {
        use crate::config::QuickFilter;
        let all = vec![QuickFilter::new("Images", "jpg png"), QuickFilter::new("Music", "mp3"), QuickFilter::new("Videos", "mp4 mkv")];
        let names = |q: &Option<QuickActive>| q.as_ref().map(|q| q.names.clone()).unwrap_or_default();
        // Ticking adds a category; the order is the menu's, whatever the clicks.
        let q = QuickActive::toggle(None, "Videos", &all);
        let q = QuickActive::toggle(q.as_ref(), "Images", &all);
        assert_eq!(names(&q), ["Images", "Videos"]);
        let on = q.clone().unwrap();
        assert!(on.has("Images") && !on.has("Music"));
        let e = |name: &str| Entry::new(name.into(), false, 0, None);
        assert!(on.matches(&e("a.png")) && on.matches(&e("b.MKV")) && !on.matches(&e("c.mp3")));
        assert!(on.matches(&Entry::new("folder.mp3".into(), true, 0, None)), "folders stay");
        assert_eq!(on.label(), format!("{}, {}", t("Images"), t("Videos")));
        let three = QuickActive::toggle(q.as_ref(), "Music", &all).unwrap();
        assert_eq!(three.label(), format!("{}, {} +1", t("Images"), t("Music")));
        assert_eq!(three.short_label(), format!("{} +2", t("Images")));
        // Unticking the last one turns the filter off.
        let q = QuickActive::toggle(q.as_ref(), "Images", &all);
        assert_eq!(names(&q), ["Videos"]);
        assert!(QuickActive::toggle(q.as_ref(), "Videos", &all).is_none());
        // Edited in Settings: a renamed or deleted category drops out, an edited one applies.
        let mut edited = all.clone();
        edited[2].exts = "webm".into();
        edited.remove(0);
        let q = QuickActive::from_names(&three.names, &edited).unwrap();
        assert_eq!(q.names, ["Music", "Videos"]);
        assert!(q.matches(&e("x.webm")) && !q.matches(&e("x.mp4")));
        assert!(QuickActive::from_names(&["Gone".to_string()], &edited).is_none());
        // Two lists with the same name: the first one, once.
        let twins = vec![QuickFilter::new("Mine", "a"), QuickFilter::new("Mine", "b")];
        let q = QuickActive::toggle(None, "Mine", &twins).unwrap();
        assert_eq!(q.names, ["Mine"]);
        assert!(q.matches(&e("x.a")) && !q.matches(&e("x.b")));
    }
}

#[cfg(test)]
mod list_state_tests {
    use super::*;

    fn wait(tab: &mut Tab, ctx: &ListCtx) {
        let t0 = std::time::Instant::now();
        while tab.is_loading() && t0.elapsed().as_secs() < 5 {
            tab.poll(ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn cur(tab: &Tab) -> String {
        tab.cursor_entry().map(|e| e.name.clone()).unwrap_or_default()
    }

    fn marks(tab: &Tab) -> Vec<String> {
        let mut v: Vec<String> = tab.marked.iter().cloned().collect();
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    /// A folder with `big` (60 files), `small` (3 files) and a few files of its own.
    fn tree(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mf-ls-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("big")).unwrap();
        std::fs::create_dir_all(dir.join("small")).unwrap();
        for i in 0..60 {
            std::fs::write(dir.join(format!("big/f{i}.txt")), b"x").unwrap();
        }
        for f in ["small/a.txt", "small/b.txt", "small/c.txt", "notebook.ipynb", "one.txt", "two.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        dir
    }

    #[test]
    fn shift_range_survives_new_folders_and_shrinks() {
        let dir = tree("shift");
        let remotes = HashMap::new();
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let mut tab = Tab::new(Location::Local(dir.join("big")));
        tab.reload(&ctx);
        tab.cursor = 50;
        tab.extend_to(52);
        assert_eq!(tab.marked.len(), 3);
        // A smaller folder: the old anchor row no longer exists. This used to panic.
        tab.navigate(Location::Local(dir.join("small")), &ctx);
        tab.move_cursor(1, true);
        tab.move_cursor(1, true);
        assert_eq!(marks(&tab), ["a.txt", "b.txt"]);
        // Down ×3 then Up ×2 (from the top): the range follows the cursor back.
        tab.cursor = 0;
        tab.marked.clear();
        tab.end_range();
        for _ in 0..3 {
            tab.move_cursor(1, true);
        }
        assert_eq!(marks(&tab), ["a.txt", "b.txt", "c.txt"]);
        tab.move_cursor(-2, true);
        assert_eq!(marks(&tab), ["a.txt"]);
        // Marks made before the range are kept when it shrinks.
        tab.end_range();
        tab.marked.clear();
        tab.marked.insert("c.txt".into());
        tab.cursor = 1;
        tab.move_cursor(2, true);
        tab.move_cursor(-2, true);
        assert_eq!(marks(&tab), ["a.txt", "c.txt"]);
        // An anchor that is gone is never used as a row.
        tab.anchor = Some("gone".into());
        tab.extend_to(99);
        assert_eq!(cur(&tab), "c.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cursor_lands_on_the_folder_left_and_stays_put() {
        let dir = tree("up");
        let remotes = HashMap::new();
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let mut tab = Tab::new(Location::Local(dir.join("small")));
        tab.reload(&ctx);
        tab.go_up(&ctx);
        assert_eq!(cur(&tab), "small");
        // Moving on, then a reload (e.g. after a copy): no jump back to "small".
        tab.select_name("one.txt");
        tab.reload(&ctx);
        assert_eq!(cur(&tab), "one.txt");
        tab.move_cursor(1, false);
        let here = cur(&tab);
        tab.reload(&ctx);
        assert_eq!(cur(&tab), here);
        // Back to the folder: the cursor goes to it too, and the filter is gone.
        tab.navigate(Location::Local(dir.join("big")), &ctx);
        tab.set_filter("f1", &ctx);
        tab.go_back(&ctx);
        assert_eq!(cur(&tab), "big");
        tab.go_forward(&ctx);
        assert!(tab.filter_text.is_empty() && tab.view.len() == 61);
        // Up to the root of the file system: on the folder left as well.
        let mut tab = Tab::new(Location::Local(PathBuf::from("/tmp")));
        tab.reload(&ctx);
        tab.go_up(&ctx);
        if tab.error.is_none() && tab.loc == Location::Local(PathBuf::from("/")) {
            assert_eq!(cur(&tab), "tmp");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn re_sorting_keeps_the_cursor_entry() {
        let dir = tree("sort");
        std::fs::write(dir.join("big/.aaa"), b"x").unwrap();
        let remotes = HashMap::new();
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let mut tab = Tab::new(Location::Local(dir.join("big")));
        tab.reload(&ctx);
        tab.select_name("f7.txt");
        // Hidden files shown: rows move, the cursor stays on the file.
        let shown = ListCtx { show_hidden: true, ..ctx.clone() };
        tab.rebuild_view(&shown);
        assert_eq!(cur(&tab), "f7.txt");
        tab.set_sort(SortKey::Name, &ctx);
        assert_eq!(cur(&tab), "f7.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_keeps_search_results_cursor_and_marks() {
        let dir = tree("search");
        let remotes = HashMap::new();
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        tab.start_search("*.txt", &ctx).unwrap();
        wait(&mut tab, &ctx);
        let b = ["small", "b.txt"].iter().collect::<PathBuf>().to_string_lossy().to_string();
        tab.select_name(&b);
        tab.toggle_mark_cursor();
        let n = tab.view.len();
        tab.reload(&ctx);
        // While it runs again, the results stay.
        assert_eq!(tab.view.len(), n);
        wait(&mut tab, &ctx);
        assert_eq!(cur(&tab), b);
        assert!(tab.marked.contains(&b));
        assert_eq!(tab.view.len(), n);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn address_being_typed_survives_a_reload() {
        let dir = tree("addr");
        let remotes = HashMap::new();
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        tab.editing_path = true;
        tab.path_text = "/usr/lo".into();
        tab.reload(&ctx);
        assert_eq!(tab.path_text, "/usr/lo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn marks_hidden_by_the_filter_are_not_acted_on() {
        let dir = tree("marks");
        let remotes = HashMap::new();
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        tab.marked.insert("one.txt".into());
        tab.marked.insert("two.txt".into());
        assert_eq!(tab.marked_shown().len(), 2);
        tab.set_filter("two", &ctx);
        assert_eq!(tab.marked_shown().len(), 1);
        assert_eq!(tab.selection().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["two.txt"]);
        // Typing "not" on the way to "notebook" filters as text.
        tab.set_filter("not", &ctx);
        assert!(tab.filter.is_ok());
        assert_eq!(tab.marked_shown().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn wait_sizes(tab: &mut Tab, ctx: &ListCtx) {
        let t0 = std::time::Instant::now();
        while tab.dir_sizes.is_some() && t0.elapsed().as_secs() < 10 {
            tab.poll(ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Folder sizes land in the folder they were counted in only, and leaving it stops the count.
    #[test]
    fn folder_sizes_stay_with_their_folder() {
        let dir = std::env::temp_dir().join(format!("mf-pane-sizes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["one/same/deep", "one/empty", "two/same"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("one/same/deep/f.bin"), vec![1u8; 3000]).unwrap();
        let remotes = HashMap::new();
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let mut tab = Tab::new(Location::Local(dir.join("one")));
        tab.reload(&ctx);
        let names = |t: &Tab| t.entries.iter().filter(|e| e.is_dir && !e.is_parent).map(|e| e.name.clone()).collect::<Vec<_>>();
        tab.dir_sizes = Some(crate::tools::diskusage::FolderSizes::start(dir.join("one"), names(&tab)));
        // Another folder with the same names before any result was read: none is used there.
        tab.navigate(Location::Local(dir.join("two")), &ctx);
        assert!(tab.dir_sizes.is_none(), "leaving the folder stops the count");
        let same = tab.entries.iter().find(|e| e.name == "same").unwrap();
        assert!(!same.size_known && same.size == 0);
        // A count whose folder is not the shown one is dropped too.
        tab.dir_sizes = Some(crate::tools::diskusage::FolderSizes::start(dir.join("one"), vec!["same".into()]));
        wait_sizes(&mut tab, &ctx);
        assert!(!tab.entries.iter().find(|e| e.name == "same").unwrap().size_known);
        // In its own folder: the total of everything below, and 0 B (known) for an empty one.
        tab.navigate(Location::Local(dir.join("one")), &ctx);
        tab.dir_sizes = Some(crate::tools::diskusage::FolderSizes::start(dir.join("one"), names(&tab)));
        wait_sizes(&mut tab, &ctx);
        let get = |n: &str| tab.entries.iter().find(|e| e.name == n).cloned().unwrap();
        assert_eq!((get("same").size, get("same").size_known), (3000, true));
        assert_eq!((get("empty").size, get("empty").size_known), (0, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reload because the folder changed keeps the cursor, the view and the counted
    /// folder sizes, and lists the new file.
    #[test]
    fn a_quiet_refresh_keeps_cursor_view_and_sizes() {
        let dir = std::env::temp_dir().join(format!("mf-pane-quiet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for n in ["b.txt", "c.txt"] {
            std::fs::write(dir.join(n), "x").unwrap();
        }
        let remotes = HashMap::new();
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        assert_eq!(tab.watched().map(|w| w.0), Some(dir.clone()), "watched");
        tab.cursor = tab.pos_of("c.txt").unwrap();
        let sub = tab.entries.iter_mut().find(|e| e.name == "sub").unwrap();
        (sub.size, sub.size_known) = (1234, true);
        tab.ensure_visible = false;
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        tab.refresh_quietly(&ctx);
        assert!(tab.entries.iter().any(|e| e.name == "a.txt"), "the new file");
        assert_eq!(cur(&tab), "c.txt", "the cursor stays on its file");
        assert!(!tab.ensure_visible, "the list does not jump to the cursor");
        let sub = tab.entries.iter().find(|e| e.name == "sub").unwrap();
        assert_eq!((sub.size, sub.size_known), (1234, true), "the counted size stays");
        // A folder removed and made again under the same name is not the one counted.
        std::fs::remove_dir(dir.join("sub")).unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::open(dir.join("sub")).unwrap().set_modified(earlier).unwrap();
        tab.refresh_quietly(&ctx);
        assert!(!tab.entries.iter().find(|e| e.name == "sub").unwrap().size_known, "a new folder of the same name");
        let sub = tab.entries.iter_mut().find(|e| e.name == "sub").unwrap();
        (sub.size, sub.size_known) = (1234, true);
        // A plain reload (after a copy, say) counts again.
        tab.reload(&ctx);
        assert!(!tab.entries.iter().find(|e| e.name == "sub").unwrap().size_known);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ".." is a row of the list only; an empty folder shows nothing in the icon view.
    #[test]
    fn the_icon_view_has_no_parent_entry() {
        let dir = std::env::temp_dir().join(format!("mf-pane-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let remotes = HashMap::new();
        let list = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
        let icons = ListCtx { default_view: crate::config::ViewMode::Icons, ..list.clone() };
        let names = |t: &Tab| t.view.iter().map(|&i| t.entries[i].name.clone()).collect::<Vec<_>>();
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&list);
        assert_eq!(names(&tab), ["..", "empty", "a.txt"]);
        // Settings' default, then this window's own choice over it.
        tab.rebuild_view(&icons);
        assert_eq!(names(&tab), ["empty", "a.txt"]);
        tab.view_mode = Some(crate::config::ViewMode::List);
        tab.rebuild_view(&icons);
        assert_eq!(names(&tab), ["..", "empty", "a.txt"]);
        tab.view_mode = Some(crate::config::ViewMode::Icons);
        tab.navigate(Location::Local(dir.join("empty")), &list);
        assert!(tab.view.is_empty(), "an empty folder: no icon at all");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ctx_of(remotes: &HashMap<u64, (String, SharedRemote)>, view: crate::config::ViewMode) -> ListCtx<'_> {
        ListCtx { remotes, show_hidden: false, dirs_first: true, default_view: view }
    }

    fn folder_with(tag: &str, names: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mf-pane-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in names {
            std::fs::write(dir.join(n), "x").unwrap();
        }
        dir
    }

    /// A Shift range made before a folder reloads by itself goes on from its anchor.
    #[test]
    fn a_quiet_refresh_keeps_the_shift_range() {
        let dir = folder_with("range", &["a", "b", "c", "d", "e"]);
        let remotes = HashMap::new();
        let ctx = ctx_of(&remotes, crate::config::ViewMode::List);
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        tab.cursor = tab.pos_of("a").unwrap();
        tab.extend_to(tab.pos_of("d").unwrap());
        assert_eq!(marks(&tab), ["a", "b", "c", "d"]);
        std::fs::write(dir.join("0"), "x").unwrap();
        tab.refresh_quietly(&ctx);
        // Shrinking the range back to b: from the anchor a, so c and d are let go.
        tab.extend_to(tab.pos_of("b").unwrap());
        assert_eq!(marks(&tab), ["a", "b"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Not watched while a filter reads the files; a slow folder or an open menu waits.
    #[test]
    fn when_a_folder_may_reload_by_itself() {
        let dir = folder_with("may", &["a.txt"]);
        let remotes = HashMap::new();
        let ctx = ctx_of(&remotes, crate::config::ViewMode::List);
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        assert!(tab.watched().is_some());
        tab.set_filter("source LIKE '%x%'", &ctx);
        assert!(tab.watched().is_none(), "a filter reading the files");
        tab.set_filter("", &ctx);
        assert!(tab.watched().is_some());
        assert!(tab.may_refresh_quietly());
        tab.menu_open = true;
        assert!(!tab.may_refresh_quietly(), "under an open menu");
        tab.menu_open = false;
        // Listing took 100 ms: not again for 2 s.
        tab.list_cost = std::time::Duration::from_millis(100);
        tab.quiet_at = Some(std::time::Instant::now());
        assert!(!tab.may_refresh_quietly(), "a slow folder");
        tab.quiet_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(3));
        assert!(tab.may_refresh_quietly());
        // So slow that each reload would freeze the window: not by itself at all.
        tab.list_cost = std::time::Duration::from_millis(600);
        tab.quiet_at = None;
        assert!(!tab.may_refresh_quietly(), "too slow to list");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder that changed just before it was listed is looked at again (its time may
    /// not show the next change on a disk with coarse times); an older one is not.
    #[test]
    fn a_fresh_folder_time_is_checked_again() {
        let dir = folder_with("fresh", &["a.txt"]);
        let remotes = HashMap::new();
        let ctx = ctx_of(&remotes, crate::config::ViewMode::List);
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        assert_eq!(tab.watched().unwrap().1, Some(std::time::SystemTime::UNIX_EPOCH), "just changed: checked again");
        let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::open(&dir).unwrap().set_modified(earlier).unwrap();
        tab.reload(&ctx);
        assert_eq!(tab.watched().unwrap().1, crate::files::dirwatch::modified(&dir), "settled: its own time");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Up and down in the icon view go a line; past the first line nothing happens, and
    /// down to a shorter last line goes to its last icon.
    #[test]
    fn arrows_in_the_icon_view() {
        let names: Vec<String> = (0..10).map(|i| format!("f{i}")).collect();
        let dir = folder_with("grid", &names.iter().map(String::as_str).collect::<Vec<_>>());
        let remotes = HashMap::new();
        let ctx = ctx_of(&remotes, crate::config::ViewMode::Icons);
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        assert_eq!(tab.view.len(), 10, "no \"..\" among the icons");
        tab.grid_cols = Some(4);
        for (from, lines, to) in [(2, -1, 2), (5, 1, 9), (7, 1, 9), (9, 1, 9), (8, -1, 4), (1, 1, 5)] {
            tab.cursor = from;
            tab.move_grid(lines, false);
            assert_eq!(tab.cursor, to, "from {from} by {lines}");
        }
        tab.cursor = 2;
        tab.move_grid(-1, true);
        assert!(tab.marked.is_empty(), "Shift+Up on the first line marks nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn wait_search(tab: &mut Tab, ctx: &ListCtx) {
        let t0 = std::time::Instant::now();
        while tab.search.as_ref().is_some_and(|s| s.running) && t0.elapsed().as_secs() < 10 {
            tab.poll(ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        tab.poll(ctx);
    }

    fn shown(t: &Tab) -> Vec<String> {
        let mut v: Vec<String> = t.view.iter().map(|&i| t.entries[i].name.clone()).filter(|n| n != "..").collect();
        v.sort();
        v
    }

    /// The Find panel: a filter of this folder, a search below it, and back.
    #[test]
    fn find_filters_here_or_searches_below() {
        let dir = folder_with("find", &["My_Report.txt", "my-report.md", "notes.txt"]);
        std::fs::create_dir_all(dir.join("deep")).unwrap();
        std::fs::write(dir.join("deep/Q3 report.pdf"), "x").unwrap();
        let remotes = HashMap::new();
        let ctx = ctx_of(&remotes, crate::config::ViewMode::List);
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        let mut f = Find { text: "MY report".into(), ..Default::default() };
        tab.set_find(f.clone(), &ctx).unwrap();
        assert_eq!(shown(&tab), ["My_Report.txt", "my-report.md"]);
        assert!(tab.is_filtered() && tab.filter_text.contains("MY report"));
        // The search box borrows the filter and puts the Find filter back as it was.
        let state = tab.filter_state();
        tab.set_filter("notes", &ctx);
        assert_eq!(shown(&tab), ["notes.txt"]);
        tab.restore_filter(state, &ctx);
        assert_eq!(shown(&tab), ["My_Report.txt", "my-report.md"]);
        // Every folder below.
        f.text = "report".into();
        f.subfolders = true;
        tab.set_find(f.clone(), &ctx).unwrap();
        wait_search(&mut tab, &ctx);
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(shown(&tab), ["My_Report.txt", format!("deep{sep}Q3 report.pdf").as_str(), "my-report.md"]);
        let s = tab.search.as_ref().unwrap();
        assert!(!s.running && !s.stopped && s.took.is_some());
        // An empty text: the whole folder again.
        f.text.clear();
        tab.set_find(f, &ctx).unwrap();
        assert!(tab.search.is_none() && !tab.is_filtered());
        assert_eq!(shown(&tab).len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Stop keeps what was found; going into another folder ends the search at once.
    #[test]
    fn a_search_stops_and_ends_when_leaving() {
        let dir = folder_with("find-stop", &[]);
        for i in 0..40 {
            std::fs::create_dir_all(dir.join(format!("d{i}"))).unwrap();
            for j in 0..40 {
                std::fs::write(dir.join(format!("d{i}/f{j}.txt")), "x").unwrap();
            }
        }
        let remotes = HashMap::new();
        let ctx = ctx_of(&remotes, crate::config::ViewMode::List);
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        let f = Find { text: "f".into(), subfolders: true, ..Default::default() };
        tab.set_find(f.clone(), &ctx).unwrap();
        tab.stop_search();
        let s = tab.search.as_ref().unwrap();
        assert!(s.stopped && s.cancel.load(std::sync::atomic::Ordering::Relaxed));
        wait_search(&mut tab, &ctx);
        let s = tab.search.as_ref().expect("the results stay");
        assert!(!s.running && s.stopped);
        assert!(s.progress.found() < 40 * 41, "stopped early");
        // Again, then into a folder: that search is dropped and told to stop.
        tab.set_find(f, &ctx).unwrap();
        let cancel = tab.search.as_ref().unwrap().cancel.clone();
        tab.navigate(Location::Local(dir.join("d0")), &ctx);
        assert!(tab.search.is_none());
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed), "the worker stops");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn wait_scan(tab: &mut Tab, ctx: &ListCtx) {
        let t0 = std::time::Instant::now();
        while tab.scan.as_ref().is_some_and(|s| s.running) && t0.elapsed().as_secs() < 10 {
            tab.poll(ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        tab.poll(ctx);
    }

    /// Stopping a search's re-run (after a reload) keeps the complete results of the
    /// run before, not the part the re-run found.
    #[test]
    fn stopping_a_rerun_keeps_the_full_results() {
        let names: Vec<String> = (0..300).map(|i| format!("f{i}.txt")).collect();
        let dir = folder_with("rerun", &names.iter().map(String::as_str).collect::<Vec<_>>());
        let remotes = HashMap::new();
        let ctx = ctx_of(&remotes, crate::config::ViewMode::List);
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        tab.set_find(Find { text: "f".into(), subfolders: true, ..Default::default() }, &ctx).unwrap();
        wait_search(&mut tab, &ctx);
        assert_eq!(shown(&tab).len(), 300);
        tab.refresh_search(&ctx);
        tab.stop_search();
        wait_search(&mut tab, &ctx);
        assert_eq!(shown(&tab).len(), 300, "the full list stays");
        let s = tab.search.as_ref().unwrap();
        assert!(!s.running && !s.stopped, "complete, from the run before");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What was stopped stays stopped: a reload (after a job, a tab shown again) does
    /// not start the search or the reading of files over.
    #[test]
    fn a_stopped_search_or_scan_is_not_run_again() {
        let names: Vec<String> = (0..400).map(|i| format!("f{i}.txt")).collect();
        let dir = folder_with("stopped", &names.iter().map(String::as_str).collect::<Vec<_>>());
        let remotes = HashMap::new();
        let ctx = ctx_of(&remotes, crate::config::ViewMode::List);
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        // A search below.
        tab.set_find(Find { text: "f".into(), subfolders: true, ..Default::default() }, &ctx).unwrap();
        tab.stop_search();
        wait_search(&mut tab, &ctx);
        let found = shown(&tab).len();
        tab.reload(&ctx);
        tab.poll(&ctx);
        let s = tab.search.as_ref().unwrap();
        assert!(s.stopped && !s.running, "not started again");
        assert_eq!(shown(&tab).len(), found);
        tab.clear_search(&ctx);
        // A filter reading the files of this folder.
        let text = Find { text: "x".into(), mode: FindMode::Content, ..Default::default() };
        tab.set_find(text, &ctx).unwrap();
        tab.stop_search();
        wait_scan(&mut tab, &ctx);
        let hits = tab.scan.as_ref().unwrap().hits.len();
        let checked = tab.scan.as_ref().unwrap().progress.checked();
        assert!(tab.scan.as_ref().unwrap().stopped);
        tab.reload(&ctx);
        let s = tab.scan.as_ref().expect("kept");
        assert!(s.stopped && !s.running && s.hits.len() == hits, "a reload keeps it");
        // The search box borrows the filter and puts it back: still the stopped one.
        let state = tab.filter_state();
        tab.set_filter("f1", &ctx);
        assert!(tab.scan.is_none());
        tab.restore_filter(state, &ctx);
        let s = tab.scan.as_ref().expect("taken back");
        assert!(s.stopped && !s.running && s.progress.checked() == checked, "not read again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The entries are kept in the order shown, whatever the sort, so that a filter's
    /// view comes out sorted without sorting a big folder again.
    #[test]
    fn entries_stay_in_display_order() {
        let dir = folder_with("order", &["b10.txt", "B2.md", "a1.txt", "c.rs"]);
        std::fs::create_dir_all(dir.join("zdir")).unwrap();
        let remotes = HashMap::new();
        let ctx = ctx_of(&remotes, crate::config::ViewMode::List);
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        let order = |t: &Tab| t.entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        let shown = |t: &Tab| t.view.iter().map(|&i| t.entries[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(order(&tab), ["..", "zdir", "a1.txt", "B2.md", "b10.txt", "c.rs"]);
        tab.set_filter("b", &ctx);
        assert_eq!(shown(&tab), ["..", "B2.md", "b10.txt"]);
        tab.set_sort(SortKey::Ext, &ctx);
        assert_eq!(order(&tab), ["..", "zdir", "B2.md", "c.rs", "a1.txt", "b10.txt"]);
        assert_eq!(shown(&tab), ["..", "B2.md", "b10.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
