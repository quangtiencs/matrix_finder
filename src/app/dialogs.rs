//! Dialogs: Copy / Move, New, Rename, Delete, Pack, Unpack, Connect, … and what they do
//! once confirmed; the quit and update notes, Help and About.
use super::*;

impl App {
    /// Copy / Move from `src` in a split tab: every other window (files, or a terminal's
    /// folder) to send to, in the tab's order, and each window's place in a unit square
    /// with whether it shows `src`. Nothing when the tab has one window.
    pub(super) fn dst_choices(&self, src: &Location) -> (Vec<DstChoice>, Vec<(usize, egui::Rect, bool)>) {
        if self.panes.len() < 2 {
            return (Vec::new(), Vec::new());
        }
        let mut places = Vec::new();
        self.layout.arrange(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1.0, 1.0)), 0.0, &mut places, &mut Vec::new());
        let mut choices = Vec::new();
        let mut thumb = Vec::new();
        for (k, &side) in self.layout.leaves().iter().enumerate() {
            let tab = self.panes[side].tab();
            // A terminal's folder as its shell has it now (not offered where that cannot
            // be read); file windows inside an archive cannot be written to.
            let (loc, terminal) = match &tab.term {
                Some(term) => (term.live_cwd().map(Location::Local), true),
                None if tab.is_files() && !matches!(tab.loc, Location::Archive { .. }) => (Some(tab.loc.clone()), false),
                None => (None, false),
            };
            // The source, also when reached another way (a terminal's shell reports
            // /private/tmp for /tmp on macOS, or a folder through a link).
            let source = match (&loc, src) {
                (Some(Location::Local(a)), Location::Local(b)) if terminal => {
                    a == b || std::fs::canonicalize(a).ok().is_some_and(|a| std::fs::canonicalize(b).ok() == Some(a))
                }
                (l, s) => l.as_ref() == Some(s),
            };
            if let Some(r) = places.iter().find(|(s, _)| *s == side).map(|(_, r)| *r) {
                thumb.push((side, r, source));
            }
            let Some(loc) = loc.filter(|_| !source) else { continue };
            let text = loc.display(|id| self.remote_label(id));
            let name = match &loc {
                Location::Local(p) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| text.clone()),
                _ => last_part(&text),
            };
            choices.push(DstChoice { side, number: k + 1, text, name, terminal });
        }
        (choices, thumb)
    }

    /// The folder tree under Copy / Move's address, following it (`typed`: the address
    /// changed in this frame); a click on a folder fills the address in.
    #[allow(clippy::too_many_arguments)]
    fn transfer_tree(
        &mut self,
        ui: &mut egui::Ui,
        th: &Theme,
        dst: &mut String,
        base: Option<&Path>,
        home: Option<&Path>,
        typed: bool,
        dst_id: egui::Id,
        max_h: f32,
    ) {
        let typed_dir = |dst: &str| crate::ui::pathfield::typed_dir(dst, home, base).filter(|p| p.is_dir());
        let tree = self.dialog_tree.get_or_insert_with(|| {
            let roots = crate::ui::foldertree::roots(home, &self.drives);
            match typed_dir(dst) {
                Some(d) => crate::ui::foldertree::FolderTree::new(roots, Some(&d), self.cfg.show_hidden),
                // Nothing chosen yet: open at the folder copied from, for bearings, with
                // nothing chosen in it.
                None => {
                    let mut t = crate::ui::foldertree::FolderTree::new(roots, base.filter(|b| b.is_dir()), self.cfg.show_hidden);
                    t.unselect();
                    t
                }
            }
        });
        tree.max_height = max_h;
        if typed {
            if let Some(p) = typed_dir(dst) {
                tree.reveal(&p);
                tree.select(&p);
            }
        }
        ui.add_space(6.0);
        // Its own background: the chosen folder stays visible inside a chosen (lit) row.
        let picked = egui::Frame::NONE
            .fill(ui.visuals().window_fill)
            .stroke(egui::Stroke::new(1.0_f32, th.border))
            .corner_radius(6)
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| tree.ui(ui, th))
            .inner;
        // Only a folder clicked: closing a branch also moves the choice (to the folder
        // closed), which is not a destination the user picked.
        if let Some(p) = picked.or(tree.clicked.take()) {
            *dst = p.display().to_string();
            self.dialog_path.reset();
            crate::ui::pathfield::cursor_to_end(ui.ctx(), dst_id, dst);
        }
    }

    /// The dimmed backdrop under a modal dialog. With rounded window corners it is drawn
    /// here, rounded too, below the dialog: egui's own square one would darken the
    /// see-through corners.
    fn modal_backdrop(&self, ctx: &egui::Context) -> egui::Color32 {
        let dim = egui::Color32::from_black_alpha(100);
        if self.window_radius <= 0.0 {
            return dim;
        }
        ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::unique("rounded-backdrop"))).rect_filled(
            ctx.viewport_rect(),
            self.window_radius,
            dim,
        );
        egui::Color32::TRANSPARENT
    }

    /// The app's windows (About, Keyboard shortcuts…) that are open, by name (see
    /// `window_id`).
    fn open_windows(&self) -> Vec<&'static str> {
        [
            ("reset", self.confirm_reset),
            ("viewer", self.viewer.is_some()),
            ("help", self.show_help),
            ("about", self.show_about),
            ("update", self.update_found.is_some()),
            ("backup", self.backup.open),
        ]
        .into_iter()
        .filter(|(_, open)| *open)
        .map(|(n, _)| n)
        .collect()
    }

    /// Keep `windows` in front-to-back order (the last opened or clicked last), and
    /// whether the keyboard is theirs: a window takes it when it opens or is clicked,
    /// the files and terminals take it back when they are clicked.
    pub(super) fn track_windows(&mut self, ctx: &egui::Context) {
        let open = self.open_windows();
        self.windows.retain(|w| open.contains(w));
        for w in &open {
            if !self.windows.contains(w) {
                self.windows.push(w);
                // A window the user opened takes the keyboard; the update notice, which
                // can open by itself while they type in a terminal, does not.
                if *w != "update" {
                    self.keys_to_window = true;
                }
            }
        }
        // Going to another window or tab by keyboard (or by a command) gives it the keys.
        let at = (self.active, self.live);
        if self.keys_at != at {
            self.keys_at = at;
            self.keys_to_window = false;
        }
        let id = window_id;
        // egui brings a clicked window to the front.
        let top = ctx.memory(|m| m.areas().top_layer_id(egui::Order::Middle)).map(|l| l.id);
        if let Some(k) = self.windows.iter().position(|w| Some(id(w)) == top) {
            let w = self.windows.remove(k);
            self.windows.push(w);
        }
        if let Some(p) = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None }) {
            self.keys_to_window = self.windows.iter().any(|w| ctx.memory(|m| m.area_rect(id(w))).is_some_and(|r| r.contains(p)));
        }
        if self.windows.is_empty() {
            self.keys_to_window = false;
        }
    }

    /// A start size saved on a bigger screen: brought within this one, so that the
    /// title bar (which moves the window) is never off the screen.
    pub(super) fn fit_to_screen(&self, ctx: &egui::Context) {
        let (inner, monitor, filled) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.monitor_size, v.maximized.unwrap_or(false) || v.fullscreen.unwrap_or(false))
        });
        let (Some(inner), Some(monitor)) = (inner, monitor) else { return };
        // Only a window bigger than the screen (a maximized one is the screen's size).
        if filled || (inner.width() <= monitor.x && inner.height() <= monitor.y) {
            return;
        }
        let fit = egui::vec2(inner.width().min(monitor.x * 0.95), inner.height().min(monitor.y * 0.9));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(fit));
    }

    /// A newer version: what is in it, and where to get it. Installing is left to the user.
    pub(super) fn update_window(&mut self, ctx: &egui::Context) {
        let Some((r, notes)) = &self.update_found else { return };
        let th = self.theme;
        let mut open = true;
        let mut close = false;
        let mut skip = false;
        let mut link: Option<String> = None;
        let asset = crate::system::update::asset_for(r, std::env::consts::OS, std::env::consts::ARCH);
        let title = t("Update available");
        let chrome = &self.chrome;
        chrome
            .dialog_window(ctx, &title, egui::Id::unique("update"))
            .open(&mut open)
            .resizable(false)
            .default_width(460.0)
            // A long line in the notes (a command) scrolls instead of widening the window.
            .min_width(460.0)
            .max_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                close |= chrome.dialog_title(ui, th, &title);
                ui.label(RichText::new(format!("Matrix Finder {}", crate::system::update::version_text(r.version))).size(18.0).strong().color(th.strong()));
                let mut line = format!("{} {}", t("You have"), env!("CARGO_PKG_VERSION"));
                if !r.date.is_empty() {
                    line.push_str(&format!(" · {} {}", t("released"), r.date));
                }
                ui.label(RichText::new(line).color(th.weak));
                if !notes.is_empty() {
                    ui.add_space(6.0);
                    egui::Frame::NONE.fill(th.surface).corner_radius(6).inner_margin(8.0).show(ui, |ui| {
                        egui::ScrollArea::both().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                            crate::preview::markdown::markdown_ui(ui, notes, th);
                        });
                    });
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(t("Matrix Finder does not install updates itself: download the new version and install it as you did before."))
                        .size(12.0)
                        .color(th.weak),
                );
                if asset.is_none() && cfg!(target_os = "macos") {
                    // Releases are built for Apple silicon only.
                    ui.label(
                        RichText::new(t(
                            "There is no download for this Mac (Intel): build the new version with the install script, as the release page shows.",
                        ))
                        .size(12.0)
                        .color(th.weak),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if let Some(a) = asset {
                        let label = format!("{} ({})", t("Download"), crate::util::human_size(a.size));
                        if ui.button(RichText::new(label).strong()).on_hover_text(&a.name).clicked() {
                            link = Some(a.url.clone());
                            close = true;
                        }
                    }
                    if ui.button(t("Open release page")).clicked() {
                        link = Some(r.url.clone());
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
        if let Some(url) = link {
            self.open_web(&url);
        }
    }

    /// The confirmation for quitting with jobs, or programs in terminals, still running.
    pub(super) fn quit_dialog(&mut self, ctx: &egui::Context) {
        let th = self.theme;
        let n = self.jobs.len();
        let programs = self.running_programs(self.panes.iter().chain(self.strip.tabs.iter().flat_map(|w| w.panes.iter())));
        let stopping = matches!(self.quit, Quit::Stopping(_));
        let mut keep = false;
        let mut stop = false;
        let edited: Vec<String> = self.temp_copies.unsaved().map(|c| c.name.clone()).collect();
        egui::Modal::new(egui::Id::unique("quit-confirm")).backdrop_color(self.modal_backdrop(ctx)).show(ctx, |ui| {
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
                ui.label(
                    RichText::new(format!("{}\n{}", t("They are not deleted: quitting moves them to"), crate::files::tempcopy::recovered_dir().display()))
                        .color(th.weak),
                );
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

    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
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
        let dst_id = egui::Id::unique("dialog-dst");
        if let Dialog::Transfer { dst, .. } | Dialog::Pack { dest_dir: dst, .. } | Dialog::Unpack { dest: dst, .. } = &mut dialog {
            self.dialog_path.keys(ctx, dst, dst_id);
        }
        let home = dirs::home_dir();
        // Copy / Move in a split tab: the other windows' folders, to pick one.
        // Read again twice a second, not in every frame: a terminal's folder is asked of
        // the system, and a folder on a slow network drive should not stall the window.
        let (choices, thumb) = match &dialog {
            Dialog::Transfer { src, .. } => {
                if self.dialog_choices.as_ref().is_none_or(|(at, ..)| at.elapsed() > Duration::from_millis(500)) {
                    let (c, t) = self.dst_choices(src);
                    self.dialog_choices = Some((Instant::now(), c, t));
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
                let (_, c, t) = self.dialog_choices.as_ref().unwrap();
                (c.clone(), t.clone())
            }
            _ => (Vec::new(), Vec::new()),
        };
        let focus = std::mem::take(&mut self.dialog_focus);
        let modal = egui::Modal::new(egui::Id::unique("dialog")).backdrop_color(self.modal_backdrop(ctx)).show(ctx, |ui| {
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
                    ui.set_width(520.0);
                    ui.add_space(6.0);
                    let base = src.local_path().map(|p| p.to_path_buf());
                    ui.label(RichText::new(t("To:")).strong());
                    if !choices.is_empty() {
                        // Scrolls in a window too low for every choice, so the buttons stay in it.
                        let max_h = (ctx.content_rect().height() - 230.0).max(140.0);
                        egui::ScrollArea::vertical().id_salt("transfer-choices").max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                            // A split tab: one list where exactly one place is chosen, a window
                            // or "Another folder"; the address shows only for the latter.
                            let typed = crate::ui::pathfield::resolve_typed(dst, home.as_deref(), base.as_deref());
                            // When no window is the destination (any more: a window may have moved
                            // on while the dialog is open), the address shows what will be used.
                            let custom = if choices.iter().any(|c| same_place(&typed, &c.text)) { *self.dialog_custom.get_or_insert(false) } else { true };
                            self.dialog_custom = Some(custom);
                            for c in &choices {
                                let chosen = !custom && same_place(&typed, &c.text);
                                if dst_choice_row(ui, th, c, chosen, &thumb).clicked() {
                                    *dst = c.text.clone();
                                    self.dialog_custom = Some(false);
                                    self.dialog_path.reset();
                                    self.dialog_tree = None;
                                    ui.memory_mut(|m| m.surrender_focus(dst_id));
                                }
                            }
                            // A window clicked just now hides the address in this frame already.
                            let custom = self.dialog_custom.unwrap_or(custom);
                            let frame = egui::Frame::NONE
                                .fill(if custom { th.selection } else { egui::Color32::TRANSPARENT })
                                .stroke(egui::Stroke::new(1.0_f32, if custom { th.accent } else { th.border }))
                                .corner_radius(7)
                                .inner_margin(egui::Margin { left: 0, right: 0, top: 0, bottom: if custom { 8 } else { 0 } });
                            let (head, field) = frame
                                .show(ui, |ui| {
                                    let head = other_folder_head(ui, th, custom);
                                    let field = custom.then(|| {
                                        ui.horizontal(|ui| {
                                            ui.add_space(CHOICE_TEXT_X - 8.0);
                                            ui.vertical(|ui| {
                                                ui.set_width(ui.available_width() - 8.0);
                                                let o = crate::ui::pathfield::Opts { base: base.as_deref(), show_hidden: self.cfg.show_hidden, drives: &self.drives, width: ui.available_width(), hint: Some(t("Type a path, or pick one from the folder tree")) };
                                                let r = self.dialog_path.show(ui, dst, dst_id, o, th);
                                                // The tree in the choice, under its address: what a small window has room for.
                                                let max_h = (ui.ctx().content_rect().height() - 470.0).clamp(96.0, 220.0);
                                                self.transfer_tree(ui, th, dst, base.as_deref(), home.as_deref(), r.changed(), dst_id, max_h);
                                                r
                                            })
                                            .inner
                                        })
                                        .inner
                                    });
                                    (head, field)
                                })
                                .inner;
                            if let Some(r) = &field {
                                focus_edit(r);
                            }
                            if head.clicked() && !custom {
                                self.dialog_custom = Some(true);
                                // The address is drawn from the next frame on: it takes the keyboard then.
                                self.dialog_focus = true;
                                crate::ui::pathfield::cursor_to_end(ui.ctx(), dst_id, dst);
                            }
                        });
                    } else {
                        let o = crate::ui::pathfield::Opts { base: base.as_deref(), show_hidden: self.cfg.show_hidden, drives: &self.drives, width: f32::INFINITY, hint: Some(t("Type a path, or pick one from the folder tree")) };
                        let r = self.dialog_path.show(ui, dst, dst_id, o, th);
                        focus_edit(&r);
                        // One window: the folder tree right under the address, following it.
                        let max_h = (ctx.content_rect().height() - 300.0).clamp(120.0, 264.0);
                        self.transfer_tree(ui, th, dst, base.as_deref(), home.as_deref(), r.changed(), dst_id, max_h);
                    }
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
                        let ext_w = ui.fonts_mut(|f| f.layout_no_wrap(ext.clone(), egui::FontId::proportional(14.0), th.weak).size().x);
                        let w = ui.available_width() - ext_w - ui.spacing().item_spacing.x - 8.0;
                        let r = ui.add(egui::TextEdit::singleline(name).id(egui::Id::unique("pack-name")).margin(egui::Margin::symmetric(4, 2)).desired_width(w));
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
                        let o = crate::ui::pathfield::Opts { base: Some(src_dir.as_path()), show_hidden: self.cfg.show_hidden, drives: &self.drives, width: f32::INFINITY, hint: None };
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
                    let o = crate::ui::pathfield::Opts { base: archive.parent(), show_hidden: self.cfg.show_hidden, drives: &self.drives, width: f32::INFINITY, hint: None };
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
                    let file_type = path.file_name().and_then(|n| crate::system::defaultapps::type_of(&n.to_string_lossy()));
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
                            let current = match (chosen, crate::system::defaultapps::system_app(ui.ctx(), ext)) {
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
                Dialog::Connect { profile, save } => {
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
                        ui.add(egui::TextEdit::singleline(&mut profile.password).password(true).desired_width(220.0).hint_text(t("In memory only")));
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new(format!("🔒 {}", t("Kept in memory (RAM) only and never written to disk. Forgotten when Matrix Finder quits."))).small().color(th.weak));
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
                        ui.label(RichText::new(t("Saved to disk: the server, user, key file and start folder. Never the password.")).small().color(th.weak));
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
                Dialog::OpenTerminals { dirs } => {
                    ui.heading(t("Open {n} terminals?").replace("{n}", &dirs.len().to_string()));
                    ui.add_space(4.0);
                    ui.label(t("A terminal opens in each of these folders:"));
                    const SHOWN: usize = 8;
                    for d in dirs.iter().take(SHOWN) {
                        let name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| d.display().to_string());
                        ui.add(egui::Label::new(RichText::new(format!("• {name}")).strong()).truncate());
                    }
                    if dirs.len() > SHOWN {
                        ui.label(RichText::new(t("and {n} more").replace("{n}", &(dirs.len() - SHOWN).to_string())).color(th.weak));
                    }
                }
                Dialog::OpenLink { url, stop_asking, .. } => {
                    ui.heading(t("Open this link?"));
                    ui.add_space(4.0);
                    // A program chooses what a link shows and where it goes: show the place first.
                    if let Some(place) = super::opening::link_place(url) {
                        ui.add(egui::Label::new(RichText::new(place).strong().size(15.0).color(th.strong())).truncate());
                    }
                    egui::ScrollArea::vertical().max_height(96.0).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(url.as_str()).monospace().size(12.0).color(th.weak)).wrap());
                    });
                    ui.add_space(4.0);
                    let (browser, profile) = self.link_browser(url);
                    let opens = super::opening::browser_name(ui.ctx(), &browser, &profile);
                    ui.label(RichText::new(format!("{} {}", t("Opens in"), opens)).size(12.0).color(th.weak));
                    ui.add_space(6.0);
                    ui.checkbox(stop_asking, t("Don't ask again in this terminal"));
                }
                Dialog::ImportSettings { path, .. } => {
                    ui.heading(t("Replace all settings?"));
                    ui.add(egui::Label::new(RichText::new(path.display().to_string()).color(th.weak)).truncate());
                    ui.add_space(4.0);
                    ui.label(t("Every setting is replaced by the imported ones, saved connections, bookmarks and backup profiles included. Exported files hold no passwords."));
                    ui.label(RichText::new(t("A copy of the current settings is kept next to the settings file first.")).color(th.weak));
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
                    // The place it goes to, named on the button.
                    Dialog::Transfer { mv, dst, .. } => match place_name(dst) {
                        Some(n) if *mv => t("Move to “{name}”").replace("{name}", &n),
                        Some(n) => t("Copy to “{name}”").replace("{name}", &n),
                        None if *mv => t("Move"),
                        None => t("Copy"),
                    },
                    Dialog::NewItem { .. } => t("Create"),
                    Dialog::Rename { .. } => t("Rename"),
                    Dialog::Connect { .. } => t("Connect"),
                    Dialog::TrustHostKey { .. } => t("Trust and connect"),
                    Dialog::Pack { .. } => t("Compress"),
                    Dialog::Unpack { .. } => t("Extract"),
                    Dialog::OpenWith { .. } => t("Open"),
                    Dialog::ArchivePassword { .. } => t("Unlock"),
                    Dialog::CloseTerminals { .. } => t("Close"),
                    Dialog::OpenTerminals { dirs } => t("Open {n} terminals").replace("{n}", &dirs.len().to_string()),
                    Dialog::ImportSettings { .. } => t("Replace settings"),
                    Dialog::OpenLink { .. } => t("Open link"),
                    _ => t("OK"),
                };
                let danger = matches!(dialog, Dialog::Delete { trash: false, .. } | Dialog::CloseTerminals { .. } | Dialog::ImportSettings { .. });
                let (fill, fg) = if danger { (th.danger, egui::Color32::WHITE) } else { (th.accent, th.on_accent()) };
                // Copy / Move needs a destination first (none is filled in for the user).
                let ready = !matches!(&dialog, Dialog::Transfer { dst, .. } if dst.trim().is_empty());
                let ok = ui.add_enabled(ready, egui::Button::new(RichText::new(ok_label).color(fg).strong()).fill(fill).min_size(egui::vec2(96.0, 30.0)));
                // Trusting a server, deleting for good and replacing the settings take a
                // deliberate click (or Enter on that button, tabbed to), never a stray Enter.
                let deliberate = matches!(dialog, Dialog::TrustHostKey { .. } | Dialog::Delete { trash: false, .. } | Dialog::ImportSettings { .. });
                let enter_confirms = enter && enter_free && !deliberate && ready;
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
        let simple = matches!(
            dialog,
            Dialog::Message { .. }
                | Dialog::CloseTerminals { .. }
                | Dialog::OpenTerminals { .. }
                | Dialog::OpenLink { .. }
                | Dialog::TrustHostKey { .. }
                | Dialog::Delete { .. }
                | Dialog::ImportSettings { .. }
        );
        if esc || (simple && modal.backdrop_response.clicked()) {
            close = true;
        }
        // Cancel (or Esc) wins over an Enter pressed in the same frame.
        if close {
            confirmed = false;
        }
        if close && matches!(dialog, Dialog::Transfer { .. } | Dialog::ArchivePassword { .. }) {
            // The drop target of a cancelled drag & drop is not for the next Copy.
            self.dialog_dst = None;
        }
        if close || confirmed {
            self.dialog_path.reset();
            self.dialog_tree = None;
            self.dialog_custom = None;
            self.dialog_choices = None;
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
    pub(super) fn show_dialog(&mut self, d: Dialog, focus: bool) {
        if self.dialog.is_some() {
            self.pending_dialogs.push_back((d, focus));
        } else {
            self.dialog = Some(d);
            self.dialog_focus = focus;
        }
    }

    /// Keep showing dialog `d`; one opened meanwhile waits for it.
    pub(super) fn keep_dialog(&mut self, d: Dialog) {
        if let Some(other) = self.dialog.replace(d) {
            self.pending_dialogs.push_front((other, true));
        }
    }

    /// Execute a confirmed dialog. Returns a dialog to keep showing (on validation errors).
    pub(super) fn confirm_dialog(&mut self, d: Dialog) -> Option<Dialog> {
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
                    None => match self.parse_location_text(&crate::ui::pathfield::resolve_typed(&dst, dirs::home_dir().as_deref(), src.local_path())) {
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
                        } else if crate::files::safefs::exists(&p) {
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
                        if crate::files::safefs::exists(&target) && !same_entry(&d.join(&old), &target, &old, &new) {
                            Err(format!("{}: {new}", t("Already exists")))
                        } else {
                            // Never over a file that appeared since.
                            crate::files::safefs::rename_item(&d.join(&old), &target).map_err(|e| {
                                if e.kind() == std::io::ErrorKind::AlreadyExists {
                                    format!("{}: {new}", t("Already exists"))
                                } else {
                                    es(e)
                                }
                            })
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
            Dialog::OpenLink { url, term, stop_asking } => {
                if stop_asking {
                    if let Some(s) = all_panes!(self).find_map(|p| p.tab_mut().term.as_mut().filter(|s| s.id == term)) {
                        s.ask_links = false;
                    }
                }
                self.open_web(&url);
            }
            Dialog::OpenTerminals { dirs } => {
                for d in &dirs {
                    self.launch_profile(self.cfg.default_terminal, d);
                }
            }
            Dialog::Pack { items, src_dir, name, dest_dir, format, level } => {
                let Some(dest) = pack_dest(&dest_dir, &name, format, dirs::home_dir().as_deref(), &src_dir) else {
                    return Some(Dialog::Pack { items, src_dir, name, dest_dir, format, level });
                };
                if crate::files::safefs::exists(&dest) {
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
                let Some(dest) = crate::ui::pathfield::typed_dir(&dest, dirs::home_dir().as_deref(), archive.parent()) else {
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
                    if let Some(ext) = path.file_name().and_then(|n| crate::system::defaultapps::type_of(&n.to_string_lossy())) {
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
            Dialog::Connect { profile, save } => {
                if profile.host.trim().is_empty() {
                    return Some(Dialog::Connect { profile, save });
                }
                self.start_connect(profile, save);
            }
            Dialog::TrustHostKey { profile, fingerprint } => self.spawn_connect(profile, false, Some(fingerprint)),
            Dialog::Message { .. } => {}
            Dialog::ImportSettings { settings, .. } => self.import_confirmed(*settings),
        }
        None
    }

    /// "Reset to defaults" confirmation (from the Settings window).
    pub(super) fn reset_dialog(&mut self, ctx: &egui::Context) {
        let th = self.theme;
        let mut keep_open = true;
        let mut reset = false;
        let title = t("Reset to defaults");
        let chrome = self.chrome.clone();
        chrome
            .dialog_window(ctx, &title, egui::Id::unique("settings-reset"))
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                if chrome.dialog_title(ui, th, &title) {
                    keep_open = false;
                }
                ui.label(t("All settings go back to their defaults: Breeze Light theme, English, UI scale, panels, terminal and safety options."));
                ui.label(
                    RichText::new(t("Kept: bookmarks, saved connections, backup profiles, terminal profiles, your themes, default apps, the browser and its website rules, and open tabs."))
                        .color(th.weak),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let b =
                            egui::Button::new(RichText::new(t("Reset")).strong().color(egui::Color32::WHITE)).fill(th.danger).min_size(egui::vec2(96.0, 30.0));
                        if ui.add(b).clicked() {
                            reset = true;
                        }
                        if ui.add(egui::Button::new(t("Cancel")).min_size(egui::vec2(96.0, 30.0))).clicked() {
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

    pub(super) fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        let keys = shortcuts::active();
        let os = keys.os();
        let mut go_to_settings = false;
        let title = t("Keyboard shortcuts");
        let chrome = self.chrome.clone();
        chrome.dialog_window(ctx, &title, egui::Id::unique("help")).open(&mut open).default_width(620.0).show(ctx, |ui| {
            if chrome.dialog_title(ui, self.theme, &title) {
                self.show_help = false;
            }
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
                ui.label(RichText::new(crate::ui::pane::filter_help()).monospace());
                ui.separator();
                ui.label(t("Tip: drop files from your OS file manager onto the window to copy them into the active panel."));
            });
        });
        self.show_help &= open && !go_to_settings;
        if go_to_settings {
            self.open_settings(SettingsPage::Shortcuts);
        }
    }

    pub(super) fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        if self.logo_large.is_none() {
            self.logo_large = crate::style::logo::texture(ctx, false, 256);
        }
        let title = t("About");
        let chrome = self.chrome.clone();
        let mut close = false;
        chrome.dialog_window(ctx, &title, egui::Id::unique("about")).open(&mut open).resizable(false).default_width(420.0).show(ctx, |ui| {
            close |= chrome.dialog_title(ui, self.theme, &title);
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
                    let w = ui.fonts_mut(|f| {
                        f.layout_no_wrap(format!("{} quangtiencs.com", t("Made by")), egui::FontId::proportional(14.0), self.theme.text).size().x
                    });
                    ui.add_space(((ui.available_width() - w) / 2.0 - 4.0).max(0.0));
                    ui.label(RichText::new(t("Made by")).color(self.theme.weak));
                    ui.hyperlink_to(RichText::new("quangtiencs.com").color(self.theme.accent), "https://quangtiencs.com");
                });
            });
        });
        self.show_about = open && !close;
    }
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

/// Copy / Move destination rows: their height, and where their text starts.
const CHOICE_H: f32 = 46.0;

const CHOICE_TEXT_X: f32 = 90.0;

/// A destination row's border and radio button: filled and lit when chosen.
fn choice_frame(p: &egui::Painter, rect: egui::Rect, chosen: bool, hovered: bool, th: &Theme) {
    if chosen {
        p.rect(rect, 7.0, th.selection, egui::Stroke::new(1.0_f32, th.accent), egui::StrokeKind::Inside);
    } else {
        p.rect(rect, 7.0, if hovered { th.hover() } else { egui::Color32::TRANSPARENT }, egui::Stroke::new(1.0_f32, th.border), egui::StrokeKind::Inside);
    }
    let c = egui::pos2(rect.left() + 16.0, rect.top() + CHOICE_H / 2.0);
    p.circle_stroke(c, 7.0, egui::Stroke::new(1.5_f32, if chosen { th.accent } else { th.weak }));
    if chosen {
        p.circle_filled(c, 4.0, th.accent);
    }
}

/// The "Another folder" choice of Copy / Move in a split tab (its frame is drawn around
/// it and the address under it when chosen). Returns the clickable row.
fn other_folder_head(ui: &mut egui::Ui, th: &Theme, chosen: bool) -> egui::Response {
    // Chosen, the address takes the place of the hint under the title.
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), if chosen { 32.0 } else { CHOICE_H }), egui::Sense::click());
    let p = ui.painter();
    if !chosen && resp.hovered() {
        p.rect_filled(rect, 7.0, th.hover());
    }
    let c = egui::pos2(rect.left() + 16.0, rect.center().y);
    p.circle_stroke(c, 7.0, egui::Stroke::new(1.5_f32, if chosen { th.accent } else { th.weak }));
    if chosen {
        p.circle_filled(c, 4.0, th.accent);
    }
    crate::style::icons::folder_icon(p, egui::Rect::from_center_size(egui::pos2(rect.left() + 55.0, rect.center().y), egui::vec2(24.0, 24.0)), th);
    let x = rect.left() + CHOICE_TEXT_X;
    let title = t("Another folder");
    let title_y = if chosen { rect.center().y - 9.0 } else { rect.top() + 6.0 };
    p.text(egui::pos2(x, title_y), egui::Align2::LEFT_TOP, &title, egui::FontId::new(13.5, egui::FontFamily::Name("bold".into())), th.strong());
    if !chosen {
        p.text(
            egui::pos2(x, rect.top() + 25.0),
            egui::Align2::LEFT_TOP,
            t("Type a path, or pick one from the folder tree"),
            egui::FontId::proportional(11.5),
            th.weak,
        );
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, chosen, &title));
    resp
}

/// The two addresses name the same folder (a trailing slash aside).
fn same_place(a: &str, b: &str) -> bool {
    let trim = |s: &str| if s.len() > 1 { s.trim_end_matches(['/', '\\']).to_string() } else { s.to_string() };
    trim(a) == trim(b)
}

/// One window to send to: a small picture of the tab's windows with this one lit, its
/// number, and its folder. `thumb`: every window's place in a unit square, with whether
/// it is the source.
fn dst_choice_row(ui: &mut egui::Ui, th: &Theme, c: &DstChoice, chosen: bool, thumb: &[(usize, egui::Rect, bool)]) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), CHOICE_H), egui::Sense::click());
    let p = ui.painter();
    choice_frame(p, rect, chosen, resp.hovered(), th);
    // The tab's windows, this one filled with the accent colour, the source shaded.
    let pic = egui::Rect::from_min_size(egui::pos2(rect.left() + 32.0, rect.center().y - 15.0), egui::vec2(46.0, 30.0));
    for (side, u, source) in thumb {
        let r = egui::Rect::from_min_max(pic.min + u.min.to_vec2() * pic.size(), pic.min + u.max.to_vec2() * pic.size()).shrink(1.0);
        let (fill, stroke) = if *side == c.side {
            (th.accent, th.accent)
        } else if *source {
            (th.weak.gamma_multiply(0.35), th.border)
        } else {
            (th.surface, th.border)
        };
        p.rect(r, 2.0, fill, egui::Stroke::new(1.0_f32, stroke), egui::StrokeKind::Inside);
    }
    let x = rect.left() + CHOICE_TEXT_X;
    let w = rect.right() - x - 8.0;
    let title = format!(
        "{} · {}",
        t("Window {n}").replace("{n}", &c.number.to_string()),
        if c.terminal { format!("{} · {}", t("Terminal"), c.name) } else { c.name.clone() }
    );
    let line = |text: &str, size: f32, color: egui::Color32, strong: bool| {
        let font = if strong { egui::FontId::new(size, egui::FontFamily::Name("bold".into())) } else { egui::FontId::proportional(size) };
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(w.max(10.0));
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let g1 = line(&title, 13.5, th.strong(), true);
    let g2 = line(&c.text, 11.5, th.weak, false);
    p.galley(egui::pos2(x, rect.top() + 6.0), g1, th.text);
    p.galley(egui::pos2(x, rect.top() + 25.0), g2, th.weak);
    let label = format!("{title}: {}", c.text);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, chosen, &label));
    resp.on_hover_text(c.text.clone())
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
    let dir = crate::ui::pathfield::typed_dir(dir, home, Some(base))?;
    Some(dir.join(archive::archive_file_name(name, fmt)))
}

/// A format to choose in the Compress dialog.
fn format_chip(ui: &mut egui::Ui, th: &Theme, f: ArchiveFormat, on: bool, enabled: bool) -> egui::Response {
    let text = format!(".{}", f.extension());
    let g = ui.fonts_mut(|fo| fo.layout_no_wrap(text.clone(), egui::FontId::proportional(13.0), th.text));
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
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, on, &text));
    if enabled {
        resp.on_hover_text(f.blurb())
    } else {
        resp.on_disabled_hover_text(f.blurb()).on_hover_text(f.blurb())
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
