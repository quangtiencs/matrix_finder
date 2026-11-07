//! A file panel: tabs, listing, sorting, filtering, selection and rendering.
use crate::archive::{self, ArchEntry};
use crate::filter::Filter;
use crate::i18n::t;
use crate::icons;
use crate::layout::SplitDir;
use crate::remote::SharedRemote;
use crate::theme::Theme;
use crate::util::{format_time, human_size, Res};
use crate::vfs::{list_local, Entry, Location};
use egui::{pos2, vec2, Align2, FontId, Rect, Sense};
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
    pub query: String,
    pub running: bool,
    pub truncated: bool,
    rx: Receiver<Vec<Entry>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Upper bound on recursive search results.
const SEARCH_LIMIT: usize = 20_000;

pub struct Tab {
    pub loc: Location,
    pub entries: Vec<Entry>,
    pub view: Vec<usize>,
    pub cursor: usize,
    pub marked: HashSet<String>,
    anchor: Option<usize>,
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
    select_name: Option<String>,
    pub dir_sizes: Option<Receiver<(String, u64)>>,
    /// Recursive search results shown instead of the folder.
    pub search: Option<Search>,
    pub focus_path: bool,
    pub editing_path: bool,
    /// One-off scroll request (auto-scroll while dragging).
    force_scroll: Option<f32>,
    /// Spring-loaded folder: (folder name, hover start time).
    spring: Option<(String, f64)>,
    /// Stable identity (survives moves between windows and tabs).
    pub uid: u64,
    /// Fixed name (e.g. the terminal profile it was started with).
    pub custom_title: Option<String>,
    /// Built-in terminal shown instead of the file list.
    pub term: Option<Box<crate::termview::TermSession>>,
    /// Disk usage analysis shown instead of the file list.
    pub du: Option<Box<crate::diskusage::DiskUsage>>,
    /// The Settings window (its pages are drawn by the app).
    pub settings: bool,
    /// The Transfers window (its content is drawn by the app).
    pub transfers: bool,
    /// Quick filter by file category; unlike the text filter it stays on while
    /// browsing other folders, and folders stay visible.
    pub quick: Option<QuickActive>,
}

/// The quick filter a window has on.
#[derive(Clone, Debug)]
pub struct QuickActive {
    pub name: String,
    pub exts: HashSet<String>,
}

impl QuickActive {
    pub fn of(q: &crate::config::QuickFilter) -> Self {
        QuickActive { name: q.name.clone(), exts: q.ext_list().into_iter().collect() }
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
            select_name: None,
            dir_sizes: None,
            search: None,
            focus_path: false,
            editing_path: false,
            force_scroll: None,
            spring: None,
            uid: crate::tabs::next_uid(),
            custom_title: None,
            term: None,
            du: None,
            settings: false,
            transfers: false,
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

    /// A plain file view (not a terminal, disk usage, settings or transfers window).
    pub fn is_files(&self) -> bool {
        self.term.is_none() && self.du.is_none() && !self.settings && !self.transfers
    }

    /// A disk usage window for `dir` (scans `depth` levels).
    pub fn disk_usage(dir: PathBuf, depth: usize) -> Self {
        let mut t = Self::new(Location::Local(dir.clone()));
        t.du = Some(Box::new(crate::diskusage::DiskUsage::new(dir, depth)));
        t
    }

    pub fn is_disk_usage(&self) -> bool {
        self.du.is_some()
    }

    /// A terminal tab (its location follows the shell's folder for F5/F6 targets).
    pub fn terminal(session: crate::termview::TermSession) -> Self {
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

    pub fn navigate(&mut self, loc: Location, ctx: &ListCtx) {
        if loc != self.loc {
            self.back.push(self.loc.clone());
            if self.back.len() > 100 {
                self.back.remove(0);
            }
            self.fwd.clear();
            // When going up, keep the cursor on the folder we came from.
            self.select_name = if loc.parent().is_some() && self.loc.parent().as_ref() == Some(&loc) {
                Some(self.loc.short_name_raw())
            } else {
                None
            };
            self.loc = loc;
            self.marked.clear();
            self.cursor = 0;
            self.scroll_offset = 0.0;
            self.filter_text.clear();
            self.filter = Ok(Filter::All);
            self.search = None;
        }
        self.reload(ctx);
    }

    pub fn go_back(&mut self, ctx: &ListCtx) {
        if let Some(l) = self.back.pop() {
            self.fwd.push(std::mem::replace(&mut self.loc, l));
            self.marked.clear();
            self.cursor = 0;
            self.reload(ctx);
        }
    }

    pub fn go_forward(&mut self, ctx: &ListCtx) {
        if let Some(l) = self.fwd.pop() {
            self.back.push(std::mem::replace(&mut self.loc, l));
            self.marked.clear();
            self.cursor = 0;
            self.reload(ctx);
        }
    }

    pub fn go_up(&mut self, ctx: &ListCtx) {
        if let Some(p) = self.loc.parent() {
            let from = self.loc.short_name_raw();
            self.navigate(p, ctx);
            self.select_name = Some(from);
        }
    }

    /// Reload the listing, keeping the cursor on the same name if possible.
    pub fn reload(&mut self, ctx: &ListCtx) {
        if self.term.is_some() {
            return;
        }
        if let Some(q) = self.search.as_ref().map(|s| s.query.clone()) {
            let _ = self.start_search(&q, ctx);
            return;
        }
        if self.select_name.is_none() {
            self.select_name = self.cursor_entry().map(|e| e.name.clone());
        }
        self.error = None;
        self.path_text = self.loc.display(|id| remote_label(ctx, id));
        match self.loc.clone() {
            Location::Local(p) => match list_local(&p) {
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
                    let r = fs
                        .lock()
                        .map_err(|e| e.to_string())
                        .and_then(|mut g| g.list(&path))
                        .map(Loaded::Entries);
                    let _ = tx.send(r);
                });
                self.loading = Some(rx);
            }
        }
    }

    /// Search every folder below this one (local folders only).
    pub fn start_search(&mut self, query: &str, ctx: &ListCtx) -> Result<(), String> {
        let Some(root) = self.loc.local_path().map(|p| p.to_path_buf()) else {
            return Err(t("Searching subfolders works on local folders"));
        };
        let filter = Filter::parse(query)?;
        self.search = None;
        self.filter_text.clear();
        self.filter = Ok(Filter::All);
        self.marked.clear();
        self.cursor = 0;
        self.scroll_offset = 0.0;
        self.loading = None;
        self.error = None;
        let (tx, rx) = channel();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = cancel.clone();
        let show_hidden = ctx.show_hidden;
        std::thread::spawn(move || {
            let mut batch = Vec::new();
            let mut last = std::time::Instant::now();
            let mut found = 0usize;
            let walker = walkdir::WalkDir::new(&root).min_depth(1).follow_links(false).into_iter().filter_entry(|d| show_hidden || !d.file_name().to_string_lossy().starts_with('.'));
            for d in walker.flatten() {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let meta = d.metadata().ok();
                let is_dir = d.file_type().is_dir();
                let mut e = Entry::new(d.file_name().to_string_lossy().to_string(), is_dir, meta.as_ref().filter(|_| !is_dir).map(|m| m.len()).unwrap_or(0), meta.and_then(|m| m.modified().ok()));
                e.is_link = d.path_is_symlink();
                if !filter.matches(&e) {
                    continue;
                }
                // Results are named by their path below the search folder.
                e.name = d.path().strip_prefix(&root).unwrap_or(d.path()).to_string_lossy().to_string();
                batch.push(e);
                found += 1;
                if batch.len() >= 500 || last.elapsed().as_millis() > 120 {
                    if tx.send(std::mem::take(&mut batch)).is_err() {
                        return;
                    }
                    last = std::time::Instant::now();
                }
                if found >= SEARCH_LIMIT {
                    break;
                }
            }
            let _ = tx.send(batch);
        });
        self.search = Some(Search { query: query.to_string(), running: true, truncated: false, rx, cancel });
        self.entries.clear();
        if self.loc.parent().is_some() {
            self.entries.push(Entry::parent());
        }
        self.rebuild_view(ctx);
        Ok(())
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
        let keep = self.cursor_entry().map(|e| e.name.clone());
        self.rebuild_view(ctx);
        self.cursor = keep.and_then(|n| self.view.iter().position(|&i| self.entries[i].name == n)).unwrap_or(0);
        self.ensure_visible = true;
    }

    /// Poll background loads. Returns true if something changed.
    pub fn poll(&mut self, ctx: &ListCtx) -> bool {
        let mut changed = false;
        if let Some(s) = &mut self.search {
            let mut got = false;
            loop {
                match s.rx.try_recv() {
                    Ok(batch) => {
                        self.entries.extend(batch);
                        got = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        s.running = false;
                        s.truncated = self.entries.len() >= SEARCH_LIMIT;
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
                Err(_) => self.loading = None,
            }
        }
        if let Some(rx) = &self.dir_sizes {
            loop {
                match rx.try_recv() {
                    Ok((name, size)) => {
                        if let Some(e) = self.entries.iter_mut().find(|e| e.name == name) {
                            e.size = size;
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

    pub fn invalidate_archive_cache(&mut self) {
        self.archive_cache = None;
    }

    fn set_entries(&mut self, mut list: Vec<Entry>, ctx: &ListCtx) {
        if self.loc.parent().is_some() {
            list.insert(0, Entry::parent());
        }
        self.entries = list;
        let existing: HashSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        self.marked.retain(|m| existing.contains(m.as_str()));
        self.rebuild_view(ctx);
        if let Some(name) = self.select_name.take() {
            if let Some(pos) = self.view.iter().position(|&i| self.entries[i].name == name) {
                self.cursor = pos;
            }
        }
        self.cursor = self.cursor.min(self.view.len().saturating_sub(1));
        self.ensure_visible = true;
    }

    pub fn set_filter(&mut self, text: &str, ctx: &ListCtx) {
        self.filter_text = text.to_string();
        self.filter = Filter::parse(text);
        let keep = self.cursor_entry().map(|e| e.name.clone());
        self.rebuild_view(ctx);
        self.cursor = keep
            .and_then(|n| self.view.iter().position(|&i| self.entries[i].name == n))
            .unwrap_or(0);
        self.ensure_visible = true;
    }

    pub fn rebuild_view(&mut self, ctx: &ListCtx) {
        let filter = self.filter.as_ref().ok();
        let mut view: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                if e.is_parent {
                    return true;
                }
                if !ctx.show_hidden && e.hidden {
                    return false;
                }
                if self.quick.as_ref().is_some_and(|q| !q.matches(e)) {
                    return false;
                }
                filter.is_none_or(|f| f.matches(e))
            })
            .collect();
        let (sort, desc, dirs_first) = (self.sort, self.desc, ctx.dirs_first);
        let entries = &self.entries;
        view.sort_by(|&a, &b| {
            let (x, y) = (&entries[a], &entries[b]);
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
        });
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
        let keep = self.cursor_entry().map(|e| e.name.clone());
        self.rebuild_view(ctx);
        if let Some(n) = keep {
            if let Some(p) = self.view.iter().position(|&i| self.entries[i].name == n) {
                self.cursor = p;
            }
        }
        self.ensure_visible = true;
    }

    pub fn cursor_entry(&self) -> Option<&Entry> {
        self.view.get(self.cursor).map(|&i| &self.entries[i])
    }

    /// Marked entries, or the cursor entry if nothing is marked. Never includes `..`.
    pub fn selection(&self) -> Vec<Entry> {
        if !self.marked.is_empty() {
            return self
                .view
                .iter()
                .map(|&i| &self.entries[i])
                .filter(|e| self.marked.contains(&e.name))
                .cloned()
                .collect();
        }
        self.cursor_entry().filter(|e| !e.is_parent).cloned().into_iter().collect()
    }

    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
        if self.view.is_empty() {
            return;
        }
        let old = self.cursor;
        let n = self.view.len() as isize;
        let new = (self.cursor as isize + delta).clamp(0, n - 1) as usize;
        if extend {
            let anchor = *self.anchor.get_or_insert(old);
            self.select_range(anchor, new);
        } else {
            self.anchor = None;
        }
        self.cursor = new;
        self.ensure_visible = true;
    }

    pub fn page_rows(&self) -> isize {
        ((self.view_height / ROW_H) as isize - 1).max(1)
    }

    fn select_range(&mut self, a: usize, b: usize) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        for i in lo..=hi {
            let e = &self.entries[self.view[i]];
            if !e.is_parent {
                self.marked.insert(e.name.clone());
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
        for &i in &self.view {
            let e = &self.entries[i];
            if !e.is_parent {
                self.marked.insert(e.name.clone());
            }
        }
    }

    pub fn invert_marks(&mut self) {
        let names: Vec<String> = self
            .view
            .iter()
            .map(|&i| &self.entries[i])
            .filter(|e| !e.is_parent)
            .map(|e| e.name.clone())
            .collect();
        for n in names {
            if !self.marked.remove(&n) {
                self.marked.insert(n);
            }
        }
    }

    pub fn select_name(&mut self, name: &str) {
        self.select_name = Some(name.to_string());
        if let Some(p) = self.view.iter().position(|&i| self.entries[i].name == name) {
            self.cursor = p;
            self.ensure_visible = true;
        }
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
            return t("Transfers");
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

/// Compare strings treating digit runs as numbers (file2 < file10).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                    nb.push(c);
                    bi.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    Open,
    OpenDefault,
    OpenWith,
    DefaultApps,
    View,
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
    Term(crate::termview::TermResponse),
    /// From a disk usage window.
    Du(crate::diskusage::DuAction),
    /// Quick filter: category `i` of the settings, or off.
    QuickFilter(Option<usize>),
    /// Open Settings → Quick filters.
    EditQuickFilters,
    /// A Settings window: the area its pages go in (the app draws them).
    SettingsArea(Rect),
    /// A Transfers window: the area its content goes in (the app draws it).
    TransfersArea(Rect),
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
    pub drives: &'a [(String, PathBuf, u64, u64)],
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
    /// The quick-filter categories.
    pub quick_filters: &'a [crate::config::QuickFilter],
}

pub const SIDE_PAD: f32 = 10.0;

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
    let fill = term_theme.map(crate::termview::background).unwrap_or(th.bg);
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
            let opts = crate::termview::TermOpts {
                theme: tth,
                font_size: c.term_font_size,
                copy_on_select: c.term_copy_on_select,
                active: c.focused,
                can_split: c.can_split,
                in_split: c.pane_count > 1,
                radius: radius as f32 - 1.0,
            };
            let r = term.ui(ui, &opts);
            if r.focus || r.close || r.restart || r.open_dir.is_some() || r.font_delta != 0.0 || r.split.is_some() {
                ev.push(PaneEvent::Term(r));
            }
        } else if pane.tab().settings {
            let area = tool_window_header(ui, pane.tab().uid, card, c, &mut ev, icons::gear, &t("Settings"));
            ev.push(PaneEvent::SettingsArea(area));
        } else if pane.tab().transfers {
            let area = tool_window_header(ui, pane.tab().uid, card, c, &mut ev, icons::transfers, &t("Transfers"));
            ev.push(PaneEvent::TransfersArea(area));
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
fn tool_window_header(ui: &mut egui::Ui, uid: u64, card: Rect, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>, icon: fn(&egui::Painter, Rect, egui::Color32), title: &str) -> Rect {
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
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(top, vec2(w, ui.available_height()))).layout(egui::Layout::top_down(egui::Align::Min)));
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
        if !compact && icons::icon_button(ui, 26.0, !tab.back.is_empty(), th, |p, r, col| icons::chevron(p, r, -1, col))
            .on_hover_text(t("Back (Alt+←)"))
            .clicked()
        {
            ev.push(PaneEvent::Back);
        }
        if !compact && icons::icon_button(ui, 26.0, !tab.fwd.is_empty(), th, |p, r, col| icons::chevron(p, r, 1, col))
            .on_hover_text(t("Forward (Alt+→)"))
            .clicked()
        {
            ev.push(PaneEvent::Forward);
        }
        if icons::icon_button(ui, 26.0, tab.loc.parent().is_some(), th, |p, r, col| icons::chevron(p, r, 0, col))
            .on_hover_text(t("Parent folder (Backspace)"))
            .clicked()
        {
            ev.push(PaneEvent::Up);
        }
        ui.add_space(4.0);

        let extra = if c.pane_count > 1 { 56.0 } else { 0.0 };
        // Filter / search mode: a chip at the end of the address bar; click it to turn it off.
        let chip = match &tab.search {
            Some(s) => Some((
                format!("{}: {} · {}{}", t("Subfolders"), s.query, tab.entries.iter().filter(|e| !e.is_parent).count(), if s.running { "…" } else if s.truncated { "+" } else { "" }),
                true,
            )),
            None => {
                let quick = tab.quick.as_ref().map(|q| t(&q.name));
                match (quick, tab.filter_text.is_empty()) {
                    (Some(q), true) => Some((q, false)),
                    (Some(q), false) => Some((format!("{q} · {}", tab.filter_text), false)),
                    (None, false) => Some((format!("{}: {}", t("Filter"), tab.filter_text), false)),
                    (None, true) => None,
                }
            }
        };
        let room = ui.available_width() - 54.0 - extra - SIDE_PAD - 8.0;
        let chip_w = chip
            .as_ref()
            .map(|(text, _)| (ui.fonts(|f| f.layout_no_wrap(text.clone(), FontId::proportional(12.5), th.text).size().x) + 46.0).min(300.0).min((room - 80.0).max(60.0)))
            .unwrap_or(0.0);
        let crumb_w = (room - chip_w).max(30.0);
        ui.allocate_ui(vec2(crumb_w, 28.0), |ui| breadcrumb(ui, tab, side, c, ev));
        if let Some((text, recursive)) = chip {
            let (rect, resp) = ui.allocate_exact_size(vec2(chip_w, 24.0), Sense::click());
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
                _ => t("Click to turn off the filter (Esc)"),
            };
            if resp.on_hover_text(tip).clicked() {
                ev.push(PaneEvent::ClearFilter);
            }
        }
        quick_filter_button(ui, tab, c, ev);
        let star_color = if c.is_bookmarked { th.marked } else { th.weak };
        let bookmarked = c.is_bookmarked;
        if icons::icon_button(ui, 26.0, true, th, |p, r, col| icons::star(p, r, if bookmarked { star_color } else { col }, bookmarked))
            .on_hover_text(format!("{} ({})", t("Bookmark this folder"), crate::shortcuts::hint(crate::shortcuts::Action::Bookmark)))
            .clicked()
        {
            ev.push(PaneEvent::Command(Cmd::Bookmark));
        }
        if c.pane_count > 1 {
            window_buttons(ui, false, c, ev);
        }
    });
}

/// The quick filter button and its menu of file categories.
fn quick_filter_button(ui: &mut egui::Ui, tab: &Tab, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    let th = c.theme;
    let on = tab.quick.is_some();
    let accent = th.accent;
    let r = icons::icon_button(ui, 26.0, true, th, |p, r, col| icons::funnel(p, r, if on { accent } else { col }, on));
    let tip = match &tab.quick {
        Some(q) => format!("{}: {}", t("Quick filter"), t(&q.name)),
        None => t("Quick filter: show one kind of file"),
    };
    let r = r.on_hover_text(tip);
    let id = egui::Id::new(("quick-filter", tab.uid));
    if r.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    egui::popup::popup_below_widget(ui, id, &r, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(220.0);
        menu_style(ui);
        for (i, q) in c.quick_filters.iter().enumerate() {
            let exts = q.ext_list();
            let (row, resp) = ui.allocate_exact_size(vec2(220.0, 28.0), Sense::click());
            let sel = tab.quick.as_ref().is_some_and(|a| a.name == q.name);
            menu_row_fill(ui, row, th, sel, resp.hovered());
            quick_filter_icon(ui.painter(), Rect::from_center_size(pos2(row.left() + 15.0, row.center().y), vec2(16.0, 16.0)), &exts, th);
            ui.painter().text(pos2(row.left() + 32.0, row.center().y), Align2::LEFT_CENTER, t(&q.name), FontId::proportional(13.5), th.text);
            ui.painter().text(pos2(row.right() - 8.0, row.center().y), Align2::RIGHT_CENTER, exts.len().to_string(), FontId::proportional(11.5), th.weak);
            let shown: Vec<String> = exts.iter().take(14).map(|e| format!(".{e}")).collect();
            let more = if exts.len() > 14 { " …" } else { "" };
            if resp.on_hover_text(format!("{}{more}", shown.join(" "))).clicked() {
                ev.push(PaneEvent::QuickFilter(if sel { None } else { Some(i) }));
            }
        }
        if c.quick_filters.is_empty() {
            ui.label(egui::RichText::new(t("No quick filters yet")).color(th.weak));
        }
        ui.separator();
        if on && ui.button(t("Show all files")).clicked() {
            ev.push(PaneEvent::QuickFilter(None));
        }
        if ui.button(t("Edit quick filters…")).clicked() {
            ev.push(PaneEvent::EditQuickFilters);
        }
    });
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
    let dragging = egui::DragAndDrop::payload::<crate::tabs::WinDrag>(ui.ctx()).is_some_and(|d| d.uid == uid);
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
        egui::DragAndDrop::set_payload(ui.ctx(), crate::tabs::WinDrag { uid });
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
        let action = crate::shortcuts::Action::FocusWindow((c.window_number - 1).min(3) as u8);
        resp.on_hover_text(format!("{} {} ({})", t("Window"), c.window_number, crate::shortcuts::hint(action)));
        ui.add_space(4.0);
    }
}

/// Split menu and close button at the right end of a window's title row.
fn window_buttons(ui: &mut egui::Ui, terminal: bool, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    let th = c.theme;
    let sr = icons::icon_button(ui, 26.0, true, th, icons::mi_split).on_hover_text(t("Split window"));
    let id = sr.id.with("split-menu");
    if sr.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    egui::popup::popup_below_widget(ui, id, &sr, egui::PopupCloseBehavior::CloseOnClick, |ui| {
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
        .on_hover_text(format!("{} ({})", t("Close window"), crate::shortcuts::hint(crate::shortcuts::Action::CloseWindow)))
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
    let mut out = crate::termview::TermResponse::default();
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        ui.add_space(SIDE_PAD - 4.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        if c.pane_count > 1 {
            window_grip(ui, tab.uid, c, ev);
        }
        let exited = term.exited;
        let (ir, _) = ui.allocate_exact_size(vec2(18.0, 26.0), Sense::hover());
        let icon_col = if exited.is_some() { th.danger } else if c.active { th.accent } else { th.weak };
        crate::tabs::terminal_glyph(ui.painter(), Rect::from_center_size(ir.center(), vec2(14.0, 14.0)), icon_col);
        ui.add_space(4.0);

        // Labels get what the buttons leave.
        let buttons = 6.0 + if exited.is_some() { 1.0 } else { 0.0 } + if c.pane_count > 1 { 1.0 } else { 0.0 };
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
        lresp.on_hover_text(cwd.display().to_string()).context_menu(|ui| {
            if ui.button(t("Copy path")).clicked() {
                ui.ctx().copy_text(cwd.display().to_string());
                ui.close_menu();
            }
            if ui.button(t("Open folder in a file tab")).clicked() {
                out.open_dir = Some(cwd.clone());
                ui.close_menu();
            }
        });

        // Buttons.
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
    use crate::diskusage::{DuAction, DuView, DEPTHS};
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
        if icons::icon_button(ui, 26.0, du.path.parent().is_some(), th, |p, r, col| icons::chevron(p, r, 0, col)).on_hover_text(t("Parent folder (Backspace)")).clicked() {
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
        let pick = cands.iter().find(|s| ui.fonts(|f| f.layout_no_wrap((*s).clone(), bold.clone(), th.strong()).size().x) <= path_room).unwrap_or_else(|| cands.last().unwrap()).clone();
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
        egui::ComboBox::from_id_salt(("du-depth", uid)).width(70.0).selected_text(label(depth)).show_ui(ui, |ui| {
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
        let edit = egui::TextEdit::singleline(&mut tab.path_text)
            .id(path_id)
            .desired_width(width)
            .font(egui::TextStyle::Monospace)
            .margin(egui::Margin::symmetric(8, 5));
        let r = ui.add(edit);
        if tab.focus_path {
            r.request_focus();
            tab.focus_path = false;
        }
        if r.lost_focus() {
            tab.editing_path = false;
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ev.push(PaneEvent::NavigateText(tab.path_text.clone()));
            } else {
                tab.path_text = tab.loc.display(|id| remote_label(&c.list, id));
            }
        }
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
    let widths: Vec<f32> = segs
        .iter()
        .map(|(l, _)| ui.fonts(|f| f.layout_no_wrap(l.clone(), font.clone(), th.text).size().x) + 12.0)
        .collect();
    let sep_w = 12.0;
    let mut first = segs.len().saturating_sub(1);
    let mut total = widths.last().copied().unwrap_or(0.0);
    while first > 0 && total + widths[first - 1] + sep_w + 30.0 < width {
        first -= 1;
        total += widths[first] + sep_w;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if first > 0 {
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
        let last = segs.len().saturating_sub(1);
        for i in first..segs.len() {
            let (label, loc) = &segs[i];
            let (r, resp) = ui.allocate_exact_size(vec2(widths[i].min(width), 26.0), Sense::click());
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
                if c.active { th.strong() } else { th.text }
            } else if resp.hovered() {
                th.text
            } else {
                th.weak
            };
            let f = if i == last { FontId::new(13.5, egui::FontFamily::Name("bold".into())) } else { font.clone() };
            ui.painter().with_clip_rect(r).text(r.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, label, f, color);
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

fn sep(ui: &mut egui::Ui, th: &Theme, w: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    ui.painter().text(r.center(), Align2::CENTER_CENTER, "›", FontId::proportional(14.0), th.weak.gamma_multiply(0.7));
}

fn list_and_footer(ui: &mut egui::Ui, tab: &mut Tab, side: usize, c: &PaneUiCtx, ev: &mut Vec<PaneEvent>) {
    let th = c.theme;
    // Column header
    let avail_w = ui.available_width();
    let (w_ext, w_size, w_date) = (64.0, 86.0, 132.0);
    let show_date = avail_w > 460.0;
    let show_ext = avail_w > 360.0;
    let icon_x = SIDE_PAD + 6.0;
    let name_x = icon_x + 26.0;
    let w_name = avail_w - name_x - w_size - SIDE_PAD - if show_ext { w_ext } else { 0.0 } - if show_date { w_date } else { 0.0 };
    let (hrect, _) = ui.allocate_exact_size(vec2(avail_w, 22.0), Sense::hover());
    let mut cols: Vec<(SortKey, String, f32, f32, Align2)> = vec![(SortKey::Name, t("Name"), name_x, w_name, Align2::LEFT_CENTER)];
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
    for (key, label, x, w, align) in &cols {
        let r = Rect::from_min_size(pos2(hrect.left() + x, hrect.top()), vec2(*w, hrect.height()));
        let resp = ui.interact(r, egui::Id::new(("hdr", side, *key as u8)), Sense::click());
        let sorted = tab.sort == *key;
        let arrow = if sorted { if tab.desc { "  ⏷" } else { "  ⏶" } } else { "" };
        let pos = if *align == Align2::LEFT_CENTER { r.left_center() } else { r.right_center() };
        ui.painter().text(
            pos,
            *align,
            format!("{label}{arrow}"),
            FontId::proportional(11.5),
            if resp.hovered() { th.text } else if sorted { th.accent } else { th.weak },
        );
        if resp.clicked() {
            ev.push(PaneEvent::Sort(*key));
        }
    }
    ui.painter().hline(
        (hrect.left() + SIDE_PAD)..=(hrect.right() - SIDE_PAD),
        hrect.bottom(),
        egui::Stroke::new(1.0_f32, th.border),
    );

    // Rows
    let footer_h = 28.0;
    let list_h = (ui.available_height() - footer_h).max(ROW_H * 2.0);
    tab.view_height = list_h;
    if tab.ensure_visible {
        let y = tab.cursor as f32 * ROW_H;
        if y < tab.scroll_offset {
            tab.scroll_offset = y;
        } else if y + ROW_H > tab.scroll_offset + list_h {
            tab.scroll_offset = y + ROW_H - list_h;
        }
    }
    let mut sa = egui::ScrollArea::vertical()
        .id_salt(("list", tab.uid, format!("{:?}", tab.loc)))
        .auto_shrink([false, false])
        .max_height(list_h);
    if tab.ensure_visible {
        sa = sa.vertical_scroll_offset(tab.scroll_offset);
        tab.ensure_visible = false;
    } else if let Some(off) = tab.force_scroll.take() {
        sa = sa.vertical_scroll_offset(off);
    }
    let sa = sa.drag_to_scroll(false);
    let payload = drag_payload(ui.ctx());
    let mut row_target: Option<(Location, String)> = None;
    let mut drag_started: Option<(usize, Vec<Entry>)> = None;
    let mut clicked: Option<(usize, egui::Modifiers)> = None;
    let mut double: Option<usize> = None;
    let mut context: Option<(usize, Cmd)> = None;
    let mut bg_context: Option<Cmd> = None;
    let n = tab.view.len();
    let only_parent = n <= 1 && tab.entries.iter().filter(|e| !e.is_parent).count() == tab.entries.len().saturating_sub(1);
    let out = ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        sa.show_rows(ui, ROW_H, n, |ui, range| {
            for row in range {
                let e = &tab.entries[tab.view[row]];
                let (rect, resp) = ui.allocate_exact_size(vec2(avail_w, ROW_H), Sense::click_and_drag());
                if resp.drag_started() && !e.is_parent {
                    let items = if tab.marked.contains(&e.name) { tab.selection() } else { vec![e.clone()] };
                    drag_started = Some((row, items));
                }
                let p = ui.painter();
                let is_cur = row == tab.cursor;
                let is_marked = tab.marked.contains(&e.name);
                let pill = rect.shrink2(vec2(6.0, 1.0));
                if is_marked {
                    p.rect_filled(pill, 6.0, th.marked.gamma_multiply(if th.dark { 0.10 } else { 0.08 }));
                }
                if is_cur {
                    if c.active {
                        p.rect_filled(pill, 6.0, th.selection);
                    } else {
                        p.rect_stroke(pill, 6.0, egui::Stroke::new(1.0_f32, th.selection.lerp_to_gamma(th.text, 0.15)), egui::StrokeKind::Inside);
                    }
                } else if resp.hovered() {
                    p.rect_filled(pill, 6.0, th.hover());
                }
                if is_marked {
                    let bar = Rect::from_min_size(pos2(pill.left() + 2.0, pill.center().y - 6.0), vec2(3.0, 12.0));
                    p.rect_filled(bar, 1.5, th.marked);
                }
                icons::entry_icon(p, Rect::from_center_size(pos2(rect.left() + icon_x + 8.0, rect.center().y), vec2(17.0, 17.0)), e, th);
                let is_arch = !e.is_dir && archive::is_archive(std::path::Path::new(&e.name));
                let is_sum = !e.is_dir && crate::checksum::is_checksum_file(&e.name);
                let name_color = if is_marked {
                    th.marked
                } else if e.is_parent {
                    th.weak
                } else if e.hidden {
                    th.weak
                } else if is_cur && c.active {
                    th.strong()
                } else {
                    th.text
                };
                let base_name = match e.name.rfind('.') {
                    Some(i) if show_ext && !e.is_dir && !e.ext.is_empty() && i > 0 => &e.name[..i],
                    _ => e.name.as_str(),
                };
                let name_rect = Rect::from_min_size(pos2(rect.left() + name_x, rect.top()), vec2(w_name - 8.0, ROW_H));
                p.with_clip_rect(name_rect).text(name_rect.left_center(), Align2::LEFT_CENTER, base_name, FontId::proportional(14.0), name_color);
                let meta = FontId::proportional(13.0);
                let meta_color = if is_marked { th.marked.gamma_multiply(0.85) } else { th.weak };
                let mut x = rect.left() + name_x + w_name;
                if show_ext {
                    let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w_ext - 6.0, ROW_H));
                    p.with_clip_rect(r).text(r.left_center(), Align2::LEFT_CENTER, &e.ext, meta.clone(), meta_color);
                    x += w_ext;
                }
                let size_txt = if e.is_parent {
                    String::new()
                } else if e.is_dir {
                    if e.size > 0 { human_size(e.size) } else { "—".into() }
                } else {
                    human_size(e.size)
                };
                p.text(pos2(x + w_size, rect.center().y), Align2::RIGHT_CENTER, size_txt, meta.clone(), meta_color);
                x += w_size;
                if show_date {
                    p.text(pos2(x + w_date, rect.center().y), Align2::RIGHT_CENTER, format_time(e.modified), meta, th.weak);
                }
                if let Some(pl) = &payload {
                    let dragged_here = pl.side == side && pl.loc == tab.loc && pl.items.iter().any(|i| i.name == e.name);
                    if dragged_here {
                        p.rect_filled(rect, 0.0, th.bg.gamma_multiply(0.55));
                    } else if e.is_dir && ui.rect_contains_pointer(rect) {
                        let target = if e.is_parent { tab.loc.parent() } else { Some(tab.loc.child(&e.name)) };
                        if let Some(target) = target.filter(|t| drop_allowed(pl, t)) {
                            p.rect_filled(pill, 6.0, th.accent.gamma_multiply(0.16));
                            p.rect_stroke(pill, 6.0, egui::Stroke::new(1.5_f32, th.accent), egui::StrokeKind::Inside);
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
                if resp.secondary_clicked() && !tab.marked.contains(&e.name) {
                    clicked = Some((row, egui::Modifiers::NONE));
                }
                let (entry_is_dir, entry_is_parent) = (e.is_dir, e.is_parent);
                resp.context_menu(|ui| {
                    if let Some(cmd) = context_menu(ui, entry_is_dir, entry_is_parent, is_arch, is_sum, c) {
                        context = Some((row, cmd));
                        ui.close_menu();
                    }
                });
            }
            // Empty area below the rows.
            // Empty space below the rows, only when the rows do not fill the
            // view (measured in content coordinates so it never grows the
            // scrollable height).
            let rest = vec2(0.0, (list_h - n as f32 * ROW_H - 1.0).max(0.0));
            if rest.y > 4.0 {
                let (r, bg) = ui.allocate_exact_size(vec2(avail_w, rest.y.max(ROW_H)), Sense::click());
                if only_parent && !tab.is_loading() && tab.error.is_none() {
                    // Empty state: an icon, what is going on and what one can do.
                    let searching = tab.search.as_ref().is_some_and(|s| s.running);
                    let (msg, hint) = if searching {
                        (t("Searching…"), String::new())
                    } else if tab.is_filtered() {
                        (t("No matches"), t("Click the filter chip or press Esc to show everything"))
                    } else {
                        (t("This folder is empty"), t("Drop files here, or press F7 for a new folder"))
                    };
                    let top = r.top() + (r.height() * 0.18).clamp(28.0, 120.0);
                    let icon = Rect::from_center_size(pos2(r.center().x, top), vec2(40.0, 40.0));
                    let ic = th.weak.gamma_multiply(0.45);
                    if tab.is_filtered() || searching {
                        icons::search(ui.painter(), icon, ic);
                    } else {
                        icons::mi_open(ui.painter(), icon, ic);
                    }
                    ui.painter().text(pos2(r.center().x, top + 38.0), Align2::CENTER_CENTER, msg, FontId::proportional(15.0), th.weak);
                    if !hint.is_empty() {
                        ui.painter().text(pos2(r.center().x, top + 60.0), Align2::CENTER_CENTER, hint, FontId::proportional(12.5), th.weak.gamma_multiply(0.75));
                    }
                }
                if bg.clicked() {
                    ev.push(PaneEvent::Focus);
                }
                bg.context_menu(|ui| {
                    if let Some(cmd) = background_menu(ui, c) {
                        bg_context = Some(cmd);
                        ui.close_menu();
                    }
                });
            }
        })
    });
    if !tab.ensure_visible {
        tab.scroll_offset = out.inner.state.offset.y;
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
            tab.toggle_mark_cursor();
        } else if m.shift {
            let anchor = tab.anchor.unwrap_or(tab.cursor);
            tab.anchor = Some(anchor);
            tab.select_range(anchor, row);
            tab.cursor = row;
        } else {
            tab.cursor = row;
            tab.anchor = None;
        }
    }
    if let Some(row) = double {
        tab.cursor = row;
        ev.push(PaneEvent::Open);
    }
    if let Some((row, cmd)) = context {
        tab.cursor = row;
        ev.push(PaneEvent::Focus);
        ev.push(PaneEvent::Command(cmd));
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
    if tab.is_loading() {
        p.text(left, Align2::LEFT_CENTER, t("Loading…"), small.clone(), th.weak);
        ui.ctx().request_repaint();
    } else if let Some(err) = &tab.error {
        p.with_clip_rect(frect).text(left, Align2::LEFT_CENTER, format!("⚠ {err}"), small.clone(), th.danger);
    } else if let Err(e) = &tab.filter {
        p.with_clip_rect(frect).text(left, Align2::LEFT_CENTER, format!("⚠ {e}"), small.clone(), th.danger);
    } else {
        let (files, dirs) = tab
            .view
            .iter()
            .map(|&i| &tab.entries[i])
            .filter(|e| !e.is_parent)
            .fold((0, 0), |(f, d), e| if e.is_dir { (f, d + 1) } else { (f + 1, d) });
        let mut s = format!("{dirs} {} · {files} {}", t("folders"), t("files"));
        if !tab.marked.is_empty() {
            let size: u64 = tab.entries.iter().filter(|e| tab.marked.contains(&e.name)).map(|e| e.size).sum();
            s = format!("{} {} · {}   ·   {s}", tab.marked.len(), t("selected"), human_size(size));
        }
        if !tab.filter.as_ref().map(|f| f.is_all()).unwrap_or(true) {
            s.push_str(&format!("   ·   {} {}", t("filtered from"), tab.entries.len().saturating_sub(1)));
        }
        p.with_clip_rect(frect).text(left, Align2::LEFT_CENTER, s, small.clone(), if tab.marked.is_empty() { th.weak } else { th.marked });
    }
    if let Some(path) = tab.loc.local_path().filter(|_| c.show_free) {
        if let Some((total, avail)) = crate::vfs::free_space(path, c.drives) {
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
        ui.close_menu();
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

/// Terminal and pane-layout submenus shared by every context menu.
fn layout_items(ui: &mut egui::Ui, c: &PaneUiCtx, out: &mut Option<Cmd>) {
    let th = c.theme;
    msub(ui, th, crate::tabs::terminal_glyph, &t("Terminal"), |ui| {
        if mitem(ui, th, crate::tabs::terminal_glyph, &t("Open terminal here"), crate::shortcuts::hint(crate::shortcuts::Action::OpenTerminal), false) {
            *out = Some(Cmd::Terminal);
        }
        if c.terminal_profiles.len() > 1 {
            ui.separator();
            for (i, name) in c.terminal_profiles.iter().enumerate() {
                if mitem(ui, th, crate::tabs::terminal_glyph, name, "", false) {
                    *out = Some(Cmd::TerminalProfile(i));
                }
            }
        }
    });
    msub(ui, th, icons::mi_split, &t("Layout"), |ui| split_items(ui, false, c, out));
}

/// Split this window (same kind, or the other kind beside it) and close it.
pub fn split_items(ui: &mut egui::Ui, terminal: bool, c: &PaneUiCtx, out: &mut Option<Cmd>) {
    let th = c.theme;
    let dirs = [SplitDir::Right, SplitDir::Left, SplitDir::Down, SplitDir::Up];
    use crate::shortcuts::{hint, Action};
    let splits = [(t("Split right"), Action::SplitRight), (t("Split left"), Action::SplitLeft), (t("Split down"), Action::SplitDown), (t("Split up"), Action::SplitUp)];
    for (k, (label, action)) in splits.into_iter().enumerate() {
        if c.can_split[k] && mitem(ui, th, icons::mi_split, &label, hint(action), false) {
            *out = Some(Cmd::Split(dirs[k], terminal));
        }
    }
    if c.can_split[0] || c.can_split[2] {
        ui.separator();
        let (right, below) = if terminal { (t("Files to the right"), t("Files below")) } else { (t("Terminal to the right"), t("Terminal below")) };
        let glyph: MenuIcon = if terminal { icons::mi_split } else { crate::tabs::terminal_glyph };
        if c.can_split[0] && mitem(ui, th, glyph, &right, "", false) {
            *out = Some(Cmd::Split(SplitDir::Right, !terminal));
        }
        if c.can_split[2] && mitem(ui, th, glyph, &below, "", false) {
            *out = Some(Cmd::Split(SplitDir::Down, !terminal));
        }
    }
    if !c.can_split.iter().any(|&b| b) {
        ui.label(egui::RichText::new(t("At most 2 columns and 2 rows of panes")).size(11.5).color(th.weak));
    }
    if c.pane_count > 1 {
        ui.separator();
        if mitem(ui, th, close_glyph, &t("Close window"), hint(Action::CloseWindow), false) {
            *out = Some(Cmd::ClosePane);
        }
    }
}

fn close_glyph(p: &egui::Painter, r: Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.3_f32, c);
    let k = r.shrink(r.width() * 0.2);
    p.line_segment([k.left_top(), k.right_bottom()], s);
    p.line_segment([k.right_top(), k.left_bottom()], s);
}

fn context_menu(ui: &mut egui::Ui, is_dir: bool, is_parent: bool, is_arch: bool, is_sum: bool, c: &PaneUiCtx) -> Option<Cmd> {
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
        layout_items(ui, c, &mut out);
        return out;
    }
    pick(mitem(ui, th, icons::mi_open, &t("Open"), "Enter", false), Cmd::Open, &mut out);
    if !is_dir {
        msub(ui, th, icons::mi_external, &t("Open with"), |ui| {
            pick(mitem(ui, th, icons::mi_external, &t("Default app"), crate::shortcuts::hint(crate::shortcuts::Action::OpenDefault), false), Cmd::OpenDefault, &mut out);
            pick(mitem(ui, th, icons::mi_more, &t("Choose program…"), "", false), Cmd::OpenWith, &mut out);
            pick(mitem(ui, th, icons::mi_view, &t("View"), crate::shortcuts::hint(crate::shortcuts::Action::View), false), Cmd::View, &mut out);
            ui.separator();
            pick(mitem(ui, th, icons::mi_more, &t("Manage default apps…"), "", false), Cmd::DefaultApps, &mut out);
        });
    }
    new_submenu(ui, th, &mut out);
    ui.separator();
    pick(mitem(ui, th, icons::mi_copy, &t("Copy to other panel"), crate::shortcuts::hint(crate::shortcuts::Action::Copy), false), Cmd::Copy, &mut out);
    pick(mitem(ui, th, icons::mi_move, &t("Move to other panel"), crate::shortcuts::hint(crate::shortcuts::Action::Move), false), Cmd::Move, &mut out);
    pick(mitem(ui, th, icons::mi_rename, &t("Rename"), crate::shortcuts::hint(crate::shortcuts::Action::Rename), false), Cmd::Rename, &mut out);
    pick(mitem(ui, th, icons::mi_trash, &t("Delete"), crate::shortcuts::hint(crate::shortcuts::Action::Delete), true), Cmd::Delete, &mut out);
    ui.separator();
    msub(ui, th, icons::mi_archive, &t("Archive"), |ui| {
        pick(mitem(ui, th, icons::mi_archive, &t("Compress…"), crate::shortcuts::hint(crate::shortcuts::Action::Compress), false), Cmd::Pack, &mut out);
        if is_arch {
            pick(mitem(ui, th, icons::mi_open, &t("Extract here"), "", false), Cmd::ExtractHere, &mut out);
            pick(mitem(ui, th, icons::mi_move, &t("Extract to other panel"), crate::shortcuts::hint(crate::shortcuts::Action::ExtractOther), false), Cmd::ExtractOther, &mut out);
        }
    });
    pick(mitem(ui, th, icons::mi_link, &t("Copy path"), "", false), Cmd::CopyPath, &mut out);
    msub(ui, th, icons::mi_more, &t("More"), |ui| {
        pick(mitem(ui, th, icons::mi_rename, &t("Batch rename…"), crate::shortcuts::hint(crate::shortcuts::Action::BatchRename), false), Cmd::BatchRename, &mut out);
        let label = if is_sum { t("Verify checksums") } else { t("Checksum…") };
        pick(mitem(ui, th, icons::mi_checksum, &label, crate::shortcuts::hint(crate::shortcuts::Action::Checksum), false), Cmd::Checksum, &mut out);
        pick(mitem(ui, th, icons::mi_external, &t("Show in system file manager"), "", false), Cmd::Reveal, &mut out);
        if is_dir {
            pick(mitem(ui, th, icons::mi_star, &t("Add to bookmarks"), "", false), Cmd::BookmarkItem, &mut out);
        }
    });
    if is_dir && !is_parent {
        pick(mitem(ui, th, icons::mi_disk, &t("Analyze disk usage"), "", false), Cmd::DiskUsage, &mut out);
    }
    pick(mitem(ui, th, icons::mi_info, &t("Properties"), crate::shortcuts::hint(crate::shortcuts::Action::Properties), false), Cmd::Properties, &mut out);
    ui.separator();
    layout_items(ui, c, &mut out);
    out
}

/// "New ▸ Folder / Blank file" submenu.
fn new_submenu(ui: &mut egui::Ui, th: &Theme, out: &mut Option<Cmd>) {
    msub(ui, th, icons::mi_new_file, &t("New"), |ui| {
        if mitem(ui, th, icons::mi_new_folder, &t("Folder…"), crate::shortcuts::hint(crate::shortcuts::Action::NewFolder), false) {
            *out = Some(Cmd::NewFolder);
        }
        if mitem(ui, th, icons::mi_new_file, &t("Blank file…"), crate::shortcuts::hint(crate::shortcuts::Action::NewFile), false) {
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
        (icons::mi_refresh as MenuIcon, Cmd::Refresh, t("Refresh"), crate::shortcuts::hint(crate::shortcuts::Action::Refresh)),
        (icons::mi_select, Cmd::SelectAll, t("Select all"), crate::shortcuts::hint(crate::shortcuts::Action::SelectAll)),
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
    layout_items(ui, c, &mut out);
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
    ];
    let mut s = format!("{}\n", t("Quick filter syntax"));
    for (example, desc) in rows {
        s.push_str(&format!("  {example:<31} {desc}\n"));
    }
    s.push_str(&format!("{}: name, ext, size, type, modified, age, hidden", t("Fields")));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural() {
        let mut v = vec!["file10", "file2", "File1", "a"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["a", "File1", "file2", "file10"]);
    }
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
        let ctx = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true };
        let mut tab = Tab::new(Location::Local(dir.clone()));
        tab.reload(&ctx);
        wait(&mut tab, &ctx);
        let images = crate::config::default_quick_filters().into_iter().find(|q| q.name == "Images").unwrap();
        tab.set_quick(Some(QuickActive::of(&images)), &ctx);
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
}
