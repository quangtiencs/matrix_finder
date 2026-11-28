//! Tabs and the windows in them: splitting, closing, reopening, detaching, merging and
//! dragging windows between tabs.
use super::*;

impl App {
    /// The pane that "other panel" commands target, if there is more than one.
    /// On X11 (and XWayland) winit never sets the window's cursor while it is the default
    /// arrow, so the window shows the screen's cursor instead: the desktop's spinning
    /// "starting" cursor while it waits for the app to say it has started, which eframe
    /// never does. Hiding the cursor for one frame and showing it again makes winit set
    /// the arrow on the window for good. Runs last, so no widget's cursor overrides it.
    pub(super) fn own_cursor(&mut self, ctx: &egui::Context) {
        if !cfg!(target_os = "linux") || self.cursor_owned == 2 || !ctx.input(|i| i.pointer.has_pointer()) {
            return;
        }
        if self.cursor_owned == 0 {
            ctx.set_cursor_icon(egui::CursorIcon::None);
            self.cursor_owned = 1;
            ctx.request_repaint();
        } else {
            self.cursor_owned = 2;
        }
    }

    pub(super) fn other(&self) -> Option<usize> {
        if self.panes.len() < 2 {
            return None;
        }
        if self.prev_active != self.active && self.prev_active < self.panes.len() {
            return Some(self.prev_active);
        }
        self.layout.leaves().into_iter().find(|&i| i != self.active)
    }

    /// The other pane, splitting the active one to the right if there is none.
    pub(super) fn other_or_split(&mut self) -> usize {
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
    pub(super) fn split_pane(&mut self, side: usize, dir: SplitDir, terminal: bool) -> bool {
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
        let fix = |i: usize| {
            if i > side {
                i - 1
            } else if i == side {
                usize::MAX
            } else {
                i
            }
        };
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
    pub(super) fn running_programs<'a>(&self, panes: impl IntoIterator<Item = &'a Pane>) -> Vec<String> {
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
    pub(super) fn request_close_pane(&mut self, side: usize) {
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
    pub(super) fn request_close_tabs(&mut self, uids: &[u64]) {
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
    pub(super) fn close_pane(&mut self, side: usize) {
        if self.panes.len() < 2 {
            self.close_tabs(&[self.live]);
            return;
        }
        let pane = self.remove_pane(side);
        self.remember_closed(std::slice::from_ref(&pane), &Layout::Leaf(0), None);
        self.focus_window();
    }

    pub(super) fn swap_with_other(&mut self) {
        if let Some(o) = self.other() {
            self.panes.swap(self.active, o);
            self.quick = None;
        }
    }

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
    pub(super) fn open_settings(&mut self, page: SettingsPage) {
        self.settings_page = page;
        if !self.focus_open_window(|t| t.settings) {
            let (at, group) = self.next_slot();
            self.insert_workspace(at, Workspace::single(Pane::from_tab(Tab::settings())), group);
        }
    }

    /// Show the Transfers tab: the one already open (in any tab), or a new one.
    pub(super) fn open_transfers(&mut self) {
        if !self.focus_open_window(|t| t.transfers) {
            let (at, group) = self.next_slot();
            self.insert_workspace(at, Workspace::single(Pane::from_tab(Tab::transfers())), group);
        }
    }

    /// The focused window is the Settings window.
    pub(super) fn settings_shown(&self) -> bool {
        self.panes[self.active].tab().settings
    }

    /// The shown tab keeps its windows in `panes` / `layout` / `active`; every other
    /// tab keeps its own in `strip`. When the strip shows another tab (found by uid,
    /// so the strip's model functions need not know), swap them.
    pub(super) fn sync_tab(&mut self) {
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
        let c = lc!(self);
        for p in &mut self.panes {
            if let Some(t) = &mut p.tab_mut().term {
                t.activity = false;
            }
            if std::mem::take(&mut p.tab_mut().stale) {
                p.tab_mut().reload(&c);
            }
        }
        self.focus_window();
        self.quick = None;
    }

    /// Reload the shown folders that changed on disk (Settings → Files & layout →
    /// Refresh folders automatically), and tell the checker what is shown now.
    /// A folder waits while a menu or popup is open or something is being dragged: the
    /// rows under it must not move.
    pub(super) fn poll_dir_watch(&mut self, ctx: &egui::Context) {
        let changed = self.dir_watch.take_changed();
        let busy = egui::Popup::is_any_open(ctx) || crate::ui::pane::drag_payload(ctx).is_some();
        let c = lc!(self);
        for p in &mut self.panes {
            let tab = p.tab_mut();
            let Some((dir, _)) = tab.watched() else { continue };
            if changed.contains(&dir) {
                tab.refresh_due = true;
            }
            if tab.refresh_due && !busy && tab.may_refresh_quietly() {
                tab.refresh_quietly(&c);
            }
        }
        let every = self.cfg.auto_refresh.then(|| Duration::from_secs(self.cfg.auto_refresh_secs.into()));
        let dirs = self.panes.iter().filter_map(|p| p.tab().watched()).collect();
        self.dir_watch.watch(every, dirs);
    }

    /// Give the focused window the keyboard (a terminal takes it explicitly).
    pub(super) fn focus_window(&mut self) {
        let a = self.active;
        if let Some(t) = &mut self.panes[a].tab_mut().term {
            t.want_focus = true;
            t.activity = false;
        }
    }

    /// Show tab `i` of the strip.
    pub(super) fn show_tab(&mut self, i: usize) {
        self.strip.activate(i);
        self.sync_tab();
    }

    /// How each tab looks in the tab bar.
    pub(super) fn tab_infos(&self) -> Vec<TabInfo> {
        self.strip
            .tabs
            .iter()
            .map(|w| {
                let (panes, layout, active) =
                    if w.uid == self.live { (&self.panes[..], &self.layout, self.active) } else { (&w.panes[..], &w.layout, w.active) };
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
                    cells: if panes.len() > 1 { crate::ui::tabs::layout_cells(layout, active) } else { Vec::new() },
                }
            })
            .collect()
    }

    /// Position and group for a new tab next to the shown one.
    pub(super) fn next_slot(&self) -> (usize, Option<u32>) {
        (self.strip.active + 1, self.strip.tabs[self.strip.active].group)
    }

    pub(super) fn new_tab(&mut self) {
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
    pub(super) fn files_pane(&self, loc: Location) -> Pane {
        let mut p = Pane::new(loc);
        p.tab_mut().reload(&lc!(self));
        p
    }

    /// A terminal window in `dir` (a toast explains when it cannot start).
    pub(super) fn terminal_pane(&mut self, profile: Option<usize>, dir: &Path) -> Option<Pane> {
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
    pub(super) fn insert_workspace(&mut self, at: usize, w: Workspace, group: Option<u32>) {
        self.strip.insert_tab(at, w, group);
        self.sync_tab();
    }

    pub(super) fn new_files_tab(&mut self, at: usize, group: Option<u32>, loc: Option<Location>) {
        let loc = loc.unwrap_or_else(|| {
            let tab = self.panes[self.active].tab();
            if tab.is_terminal() {
                Location::Local(self.tab_dir(self.active))
            } else {
                tab.loc.clone()
            }
        });
        let pane = self.files_pane(loc);
        self.insert_workspace(at, Workspace::single(pane), group);
    }

    pub(super) fn spawn_terminal(&self, dir: &Path, profile: Option<usize>) -> Res<TermSession> {
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
    pub(super) fn new_terminal_tab(&mut self, at: usize, group: Option<u32>, profile: Option<usize>, dir: Option<PathBuf>) {
        let dir = dir.unwrap_or_else(|| self.tab_dir(self.active));
        if let Some(pane) = self.terminal_pane(profile, &dir) {
            self.insert_workspace(at, Workspace::single(pane), group);
        }
    }

    /// New terminal tab next to the shown tab.
    pub(super) fn new_terminal_here(&mut self, profile: Option<usize>, dir: Option<PathBuf>) {
        let (at, group) = self.next_slot();
        self.new_terminal_tab(at, group, profile, dir);
    }

    /// Close the focused window, or the whole tab when it has only one.
    pub(super) fn close_current(&mut self) {
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
    pub(super) fn close_tabs(&mut self, uids: &[u64]) {
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
    pub(super) fn open_disk_usage(&mut self, dir: PathBuf) {
        if !dir.is_dir() {
            self.toast(t("Disk usage works on local folders"), true);
            return;
        }
        let (at, group) = self.next_slot();
        self.insert_workspace(at, Workspace::single(Pane::from_tab(Tab::disk_usage(dir, 1))), group);
    }

    pub(super) fn reopen_closed_tab(&mut self) {
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
    pub(super) fn take_window(&mut self, view: u64) -> Option<Pane> {
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
            let fix = |k: usize| {
                if k > i {
                    k - 1
                } else if k == i {
                    usize::MAX
                } else {
                    k
                }
            };
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
    pub(super) fn detach_window(&mut self, view: u64, at: usize, group: Option<u32>) {
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

    pub(super) fn handle_tab_event(&mut self, e: TabEvent) {
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
    pub(super) fn duplicate_tab(&mut self, uid: u64) {
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

    /// While a window (by its grip) or another tab is dragged over the windows:
    /// the middle of a window swaps the two, its edges dock beside it.
    pub(super) fn window_drop_zones(&mut self, ui: &egui::Ui, rects: &[(usize, egui::Rect)]) {
        let ctx = ui.ctx().clone();
        let win = egui::DragAndDrop::payload::<crate::ui::tabs::WinDrag>(&ctx);
        let tab = egui::DragAndDrop::payload::<crate::ui::tabs::TabDrag>(&ctx).map(|d| d.uid).filter(|u| *u != self.live);
        let src = win.and_then(|w| self.panes.iter().position(|p| p.tab().uid == w.uid));
        let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        if src.is_none() && tab.is_none() {
            return;
        }
        let th = self.theme;
        // Ghost label for a dragged window.
        if let Some(s) = src {
            let title = self.panes[s].tab().title();
            egui::Area::new(egui::Id::new("window-drag-ghost"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pointer + egui::vec2(12.0, 8.0))
                .interactable(false)
                .show(&ctx, |ui| {
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
}
