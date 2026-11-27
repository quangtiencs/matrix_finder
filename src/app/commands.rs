//! Commands (`Cmd`) shared by menus, shortcuts and the command palette, and the tool
//! windows they open (Checksum, Batch rename).
use super::*;

impl App {
    pub(super) fn run_cmd(&mut self, cmd: Cmd) {
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
                // The other window (as before), when it can take the items; never the
                // folder copied from, nor a window the user did not call the other one.
                // Else nothing: the user picks one (the tree opens at the folder copied from).
                let (choices, _) = self.dst_choices(&loc);
                let other = self.other();
                let dst = match choices.iter().find(|c| Some(c.side) == other) {
                    Some(c) => c.text.clone(),
                    None => String::new(),
                };
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
                    if sel[0].is_dir || sel[0].ext.is_empty() {
                        n.clone()
                    } else {
                        n[..n.len() - sel[0].ext.len() - 1].to_string()
                    }
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
            Cmd::TerminalInSelection => self.terminal_in_selection(),
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
                    crate::files::gitinfo::invalidate_children(dir);
                }
                tab.invalidate_archive_cache();
                tab.reload(&lc!(self));
                self.drive_watch.refresh();
            }
            Cmd::CalcSizes => self.calc_sizes(),
            Cmd::SelectAll => self.panes[a].tab_mut().mark_all(),
        }
    }

    /// Show or hide hidden files in every window, and keep the choice.
    pub(super) fn toggle_hidden(&mut self) {
        self.cfg.show_hidden = !self.cfg.show_hidden;
        self.rebuild_views();
        self.save_cfg();
    }

    /// Show the focused file window as a list or as icons (kept in the session).
    pub(super) fn set_view_mode(&mut self, mode: crate::config::ViewMode) {
        let c = lc!(self);
        let tab = self.panes[self.active].tab_mut();
        if tab.is_files() {
            tab.view_mode = Some(mode);
            // ".." comes and goes with the list.
            tab.rebuild_view(&c);
            tab.ensure_visible = true;
            self.save_cfg();
        }
    }

    pub(super) fn calc_sizes(&mut self) {
        let a = self.active;
        let tab = self.panes[a].tab_mut();
        let Some(dir) = tab.loc.local_path().map(|p| p.to_path_buf()) else { return };
        let names: Vec<String> = tab.entries.iter().filter(|e| e.is_dir && !e.is_parent).map(|e| e.name.clone()).collect();
        // A new count replaces (and stops) the one before.
        tab.dir_sizes = Some(crate::tools::diskusage::FolderSizes::start(dir, names));
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
                self.show_dialog(
                    Dialog::Unpack {
                        archive: path,
                        dest: dest.to_string_lossy().to_string(),
                        selection: vec![],
                        strip: String::new(),
                        policy: Conflict::Rename,
                    },
                    true,
                );
            }
            Location::Archive { file, inner } => {
                // Inside an archive: extract the selection.
                let sel: Vec<String> = tab.selection().iter().map(|e| join_slash(&inner, &e.name)).collect();
                let dest = other.local_path().map(|p| p.to_path_buf()).or_else(|| file.parent().map(|p| p.to_path_buf())).unwrap_or_default();
                self.show_dialog(
                    Dialog::Unpack { archive: file, dest: dest.to_string_lossy().to_string(), selection: sel, strip: inner, policy: Conflict::Rename },
                    true,
                );
            }
            Location::Remote { .. } => self.toast(t("Download the archive first"), true),
        }
    }

    /// Show folder `loc` again wherever it is shown, with the cursor on `name` in window `view`.
    pub(super) fn reload_and_select(&mut self, loc: &Location, view: u64, name: &str) {
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
    pub(super) fn do_delete(&mut self, loc: &Location, view: u64, items: Vec<Entry>, trash: bool) {
        let loc = loc.clone();
        let Some(src) = self.endpoint(&loc) else { return };
        let items: Vec<ops::Item> = items.iter().map(|e| ops::Item { name: e.name.clone(), is_dir: e.is_dir, size: e.size }).collect();
        if let Some(p) = all_panes!(self).find(|p| p.tab().uid == view && p.tab().loc == loc) {
            p.tab_mut().marked.clear();
        }
        let what = if items.len() == 1 { items[0].name.clone() } else { format!("{} {}", items.len(), t("item(s)")) };
        self.spawn_job(format!("{} {what}", t("Deleting")), After::Refresh, Some(loc), vec![], move |p| ops::delete(src, items, trash, p));
    }

    pub(super) fn show_properties(&mut self) {
        let a = self.active;
        let tab = self.panes[a].tab();
        let Some(e) = tab.cursor_entry().cloned() else { return };
        let mut text = format!("{}: {}\n", t("Name"), e.name);
        text += &format!("{}: {}\n", t("Location"), tab.loc.display(|id| self.remote_label(id)));
        text += &format!("{}: {}\n", t("Type"), if e.is_dir { t("Folder") } else { format!("{} (.{})", t("File"), e.ext) });
        if e.size_known {
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

    pub(super) fn palette_entries(&self) -> Vec<palette::Entry> {
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
            v.push(E::new(
                format!("{}: {}", t("Terminal"), p.name),
                if i == self.cfg.default_terminal { hint(Action::OpenTerminal) } else { "" },
                PalItem::Cmd(Cmd::TerminalProfile(i)),
            ));
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
            let group =
                w.group.and_then(|g| self.strip.group(g)).map(|g| if g.name.is_empty() { String::new() } else { format!(" · {}", g.name) }).unwrap_or_default();
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
    pub(super) fn palette_live_filter(&mut self) {
        let a = self.active;
        if self.pal_filter.is_none() {
            let tab = self.panes[a].tab();
            if !tab.is_files() {
                return;
            }
            self.pal_filter = Some((a, tab.uid, tab.filter_state(), tab.cursor_entry().map(|e| e.name.clone())));
        }
        let Some((side, uid, prev, _)) = self.pal_filter.clone() else { return };
        let q = self.palette.query().trim().to_string();
        let lc = lc!(self);
        if let Some(tab) = self.panes.get_mut(side).map(|p| p.tab_mut()).filter(|t| t.uid == uid) {
            if looks_like_path(&q) {
                tab.restore_filter(prev, &lc);
            } else if tab.filter_text != q {
                tab.set_filter(&q, &lc);
            }
        }
    }

    /// Put back the filter and the cursor the window had before the search box opened
    /// (a command chosen there acts on the file the cursor was on).
    pub(super) fn restore_pal_filter(&mut self) {
        let Some((side, uid, prev, cursor)) = self.pal_filter.take() else { return };
        let lc = lc!(self);
        if let Some(tab) = self.panes.get_mut(side).map(|p| p.tab_mut()).filter(|t| t.uid == uid) {
            tab.restore_filter(prev, &lc);
            if let Some(pos) = cursor.and_then(|n| tab.view.iter().position(|&i| tab.entries[i].name == n)) {
                tab.cursor = pos;
                tab.ensure_visible = true;
            }
        }
    }

    pub(super) fn run_palette_item(&mut self, ctx: &egui::Context, item: PalItem) {
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
                Act::ToggleHidden => self.toggle_hidden(),
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

    /// Hash the selection (or the cursor item); a single checksum file is verified instead.
    pub(super) fn open_checksum(&mut self) {
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
        let mut tool = crate::tools::checksum::ChecksumUi::default();
        match sel.as_slice() {
            [one] if !one.is_dir && crate::tools::checksum::is_checksum_file(&one.name) => {
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
    pub(super) fn tool_view(&mut self, ui: &mut egui::Ui, side: usize) {
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

    pub(super) fn open_batch_rename(&mut self) {
        let a = self.active;
        let tab = self.panes[a].tab();
        if matches!(tab.loc, Location::Archive { .. }) {
            self.toast(t("Archives are read-only"), true);
            return;
        }
        // Marked items, or everything currently visible (respects the quick filter).
        let entries: Vec<Entry> =
            if tab.marked_shown().is_empty() { tab.view.iter().map(|&i| tab.entries[i].clone()).filter(|e| !e.is_parent).collect() } else { tab.selection() };
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
            Location::Local(dir) => ren::apply(
                pairs,
                |f, to| crate::files::safefs::rename_item(&dir.join(f), &dir.join(to)).map_err(es),
                |n| crate::files::safefs::exists(&dir.join(n)),
            ),
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
}

fn looks_like_path(q: &str) -> bool {
    q.starts_with('/') || q.starts_with('~') || q.contains(":\\") || q.starts_with("sftp://") || q.starts_with("ftp://")
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
