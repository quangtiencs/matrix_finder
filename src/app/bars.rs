//! The frame around the windows: the application menu, the top bar, the sidebar, the
//! status bar and the toasts.
use super::*;
use crate::ui::KeptContextMenu as _;

impl App {
    fn main_menu(&mut self, ui: &mut egui::Ui) {
        {
            ui.menu_button(t("File"), |ui| {
                if ui.button(format!("{}\t{}", t("New tab"), hint(Action::NewTab))).clicked() {
                    self.new_tab();
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(format!("{}\t{}", t("New terminal tab"), hint(Action::NewTerminalTab))).clicked() {
                    self.new_terminal_here(None, None);
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(format!("{}\t{}", t("Close tab"), hint(Action::CloseTab))).clicked() {
                    self.request_close_tabs(&[self.live]);
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.add_enabled(!self.closed_tabs.is_empty(), egui::Button::new(format!("{}\t{}", t("Reopen closed tab"), hint(Action::ReopenTab)))).clicked()
                {
                    self.reopen_closed_tab();
                    ui.close_kind(egui::UiKind::Menu);
                }
                ui.separator();
                if ui.button(format!("{}\tF7", t("New folder…"))).clicked() {
                    self.run_cmd(Cmd::NewFolder);
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(format!("{}\t{}", t("New file…"), hint(Action::NewFile))).clicked() {
                    self.run_cmd(Cmd::NewFile);
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(format!("{}\t{}", t("Open terminal here"), hint(Action::OpenTerminal))).clicked() {
                    self.open_terminal();
                    ui.close_kind(egui::UiKind::Menu);
                }
                ui.separator();
                if ui.button(t("Export settings…")).clicked() {
                    ui.close_kind(egui::UiKind::Menu);
                    if let Some(p) = rfd::FileDialog::new().set_file_name("matrix_finder-settings.json").save_file() {
                        match self.cfg.export_to(&p, false) {
                            Ok(()) => self.toast(format!("{} {}", t("Exported to"), p.display()), false),
                            Err(e) => self.toast(e, true),
                        }
                    }
                }
                if ui.button(t("Import settings…")).clicked() {
                    ui.close_kind(egui::UiKind::Menu);
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
                        ui.close_kind(egui::UiKind::Menu);
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
                        ui.close_kind(egui::UiKind::Menu);
                    }
                }
                ui.separator();
                if ui.button(format!("{}\t{}", t("Swap panels"), hint(Action::SwapPanels))).clicked() {
                    self.swap_with_other();
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(t("Same folder in other panel")).clicked() {
                    let loc = self.panes[self.active].tab().loc.clone();
                    let o = self.other_or_split();
                    self.panes[o].tab_mut().navigate(loc, &lc!(self));
                    ui.close_kind(egui::UiKind::Menu);
                }
            });
            ui.menu_button(t("View"), |ui| {
                if ui.checkbox(&mut self.cfg.show_sidebar.clone(), format!("{}\t{}", t("Sidebar"), hint(Action::Sidebar))).clicked() {
                    self.cfg.show_sidebar = !self.cfg.show_sidebar;
                    self.save_cfg();
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.checkbox(&mut self.cfg.quick_view.clone(), format!("{}\t{}", t("Quick view panel"), hint(Action::QuickView))).clicked() {
                    self.toggle_quick_view();
                    ui.close_kind(egui::UiKind::Menu);
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
                    for th in theme::THEMES.iter().chain(theme::custom()) {
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
                    ui.close_kind(egui::UiKind::Menu);
                }
            });
            ui.menu_button(t("Bookmarks"), |ui| {
                if ui.button(format!("{}\t{}", t("Bookmark current folder"), hint(Action::Bookmark))).clicked() {
                    self.run_cmd(Cmd::Bookmark);
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(t("Manage bookmarks…")).clicked() {
                    self.open_settings(SettingsPage::Bookmarks);
                    ui.close_kind(egui::UiKind::Menu);
                }
                ui.separator();
                for b in self.cfg.bookmarks.clone() {
                    if b.folder {
                        ui.menu_button(b.name.clone(), |ui| {
                            for c in &b.children {
                                if ui.button(&c.name).on_hover_text(&c.path).clicked() {
                                    self.open_bookmark(c);
                                    ui.close_kind(egui::UiKind::Menu);
                                }
                            }
                        });
                    } else if ui.button(format!("★ {}", b.name)).on_hover_text(&b.path).clicked() {
                        self.open_bookmark(&b);
                        ui.close_kind(egui::UiKind::Menu);
                    }
                }
            });
            ui.menu_button(t("Network"), |ui| {
                if ui.button(format!("{}\t{}", t("Connect to server…"), hint(Action::Connect))).clicked() {
                    self.open_connect_dialog(None);
                    ui.close_kind(egui::UiKind::Menu);
                }
                if !self.cfg.connections.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(t("Saved connections")).small().color(self.theme.weak));
                    let mut remove = None;
                    for (i, c) in self.cfg.connections.clone().into_iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.button(format!("{} {}", c.protocol.label(), c.name)).on_hover_text(connection_hover(&c)).clicked() {
                                self.connect_saved(c.clone());
                                ui.close_kind(egui::UiKind::Menu);
                            }
                            if ui.small_button("✏").on_hover_text(t("Edit")).clicked() {
                                self.show_dialog(Dialog::Connect { profile: remote::with_known_password(c), save: true }, false);
                                ui.close_kind(egui::UiKind::Menu);
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
                                ui.close_kind(egui::UiKind::Menu);
                            }
                            if ui.small_button(t("Disconnect")).clicked() {
                                self.disconnect(id);
                                ui.close_kind(egui::UiKind::Menu);
                            }
                        });
                    }
                }
            });
            ui.menu_button(t("Tools"), |ui| {
                if ui.button(format!("📊 {}", t("Disk usage"))).clicked() {
                    self.run_cmd(Cmd::DiskUsage);
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(format!("⇅ {}\t{}", t("Tasks"), hint(Action::Transfers))).clicked() {
                    self.open_transfers();
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(format!("🛡 {}\t{}", t("Backup…"), hint(Action::Backup))).clicked() {
                    self.backup.open = true;
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(format!("⚙ {}\t{}", t("Settings…"), hint(Action::Settings))).clicked() {
                    self.open_settings(self.settings_page);
                    ui.close_kind(egui::UiKind::Menu);
                }
            });
            ui.menu_button(t("Help"), |ui| {
                if ui.button(format!("{}\tF1", t("Keyboard shortcuts"))).clicked() {
                    self.show_help = true;
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(t("Check for updates…")).clicked() {
                    self.check_for_updates(true);
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button(t("About")).clicked() {
                    self.show_about = true;
                    ui.close_kind(egui::UiKind::Menu);
                }
            });
        }
    }

    /// Logo + app name + chevron; opens the application menu.
    fn brand_menu(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        let bg = ui.painter().add(egui::Shape::Noop);
        let text = RichText::new("Matrix Finder      ").font(egui::FontId::new(14.5, egui::FontFamily::Name("bold".into()))).color(th.strong());
        // Text only: the logo lives in the About window, not in the toolbar.
        let button = egui::Button::new(text).fill(egui::Color32::TRANSPARENT).min_size(egui::vec2(0.0, 30.0));
        let menu = egui::containers::menu::MenuButton::from_button(button).config(crate::ui::kept_menu_config()).ui(ui, |ui| {
            ui.set_min_width(230.0);
            if ui.button(t("About")).clicked() {
                self.show_about = true;
                ui.close_kind(egui::UiKind::Menu);
            }
            if ui.button(t("Check for updates…")).clicked() {
                self.check_for_updates(true);
                ui.close_kind(egui::UiKind::Menu);
            }
            ui.separator();
            self.main_menu(ui);
        });
        let r = menu.0.rect;
        let open = menu.1.is_some();
        if menu.0.hovered() || open {
            ui.painter().set(bg, egui::Shape::rect_filled(r, th.radius as f32 + 2.0, th.surface));
        }
        // Chevron drawn in the reserved trailing space.
        let c = egui::pos2(r.right() - 14.0, r.center().y + 1.0);
        let col = if menu.0.hovered() || open { th.strong() } else { th.weak };
        let st = egui::Stroke::new(1.5_f32, col);
        ui.painter().line_segment([c + egui::vec2(-4.0, -2.0), c + egui::vec2(0.0, 2.0)], st);
        ui.painter().line_segment([c + egui::vec2(0.0, 2.0), c + egui::vec2(4.0, -2.0)], st);
        menu.0.on_hover_text(t("Menu"));
    }

    /// The sidebar button, just before the app menu (in the sidebar, or in the top
    /// bar while the sidebar is hidden).
    fn sidebar_toggle(&mut self, ui: &mut egui::Ui) {
        let label = if self.cfg.show_sidebar { t("Hide sidebar") } else { t("Show sidebar") };
        let resp = tool_icon(ui, self.theme, &format!("{label} ({})", hint(Action::Sidebar)), false, sidebar_icon);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        if resp.clicked() {
            self.cfg.show_sidebar = !self.cfg.show_sidebar;
            self.save_cfg();
        }
    }

    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
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
            let window = ui.ctx().content_rect();
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
                let g = ui.fonts_mut(|f| f.layout_job(job));
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
                self.view_options_button(ui, th);
                self.split_view_button(ui, th);
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
                let popup_id = egui::Id::unique("terminal-profiles");
                let (cr, chev) = ui.allocate_exact_size(egui::vec2(16.0, 28.0), egui::Sense::click());
                if chev.hovered() {
                    ui.painter().rect_filled(cr, 6.0, th.surface);
                }
                let cc = cr.center();
                let st = egui::Stroke::new(1.4_f32, if chev.hovered() { th.strong() } else { th.weak });
                ui.painter().line_segment([cc + egui::vec2(-3.5, -1.5), cc + egui::vec2(0.0, 2.0)], st);
                ui.painter().line_segment([cc + egui::vec2(0.0, 2.0), cc + egui::vec2(3.5, -1.5)], st);
                if chev.on_hover_text(t("Terminal profiles")).clicked() {
                    egui::Popup::toggle_id(ui.ctx(), popup_id);
                }
                let main = tool_icon(ui, th, &format!("{} ({})", t("Open terminal here"), hint(Action::OpenTerminal)), false, terminal_icon);
                if main.clicked() {
                    self.open_terminal_profile(None);
                }
                let anchor = main.union(chev_rect_response(ui, cr));
                crate::ui::popup_below(popup_id, &anchor, egui::PopupCloseBehavior::CloseOnClick, |ui| {
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

    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        let active_loc = self.panes[self.active].tab().loc.clone();
        let active_disp = active_loc.display(|id| self.remote_label(id));
        let row =
            egui::Rect::from_min_size(ui.max_rect().min - egui::vec2(8.0, 8.0), egui::vec2(ui.max_rect().width() + 16.0, self.chrome.row_height() + 10.0));
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
            let shown: Vec<crate::files::drives::Drive> = crate::files::drives::sidebar(&self.drives, self.cfg.sidebar_drives).into_iter().cloned().collect();
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
                        let b = ui.interact(r, ui.scope_id().with(("eject", &mp)), egui::Sense::click());
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
                let resp = resp.on_hover_text(format!("{}\n{} {} / {}", mp.display(), human_size(avail), t("free"), human_size(total)));
                if resp.clicked() {
                    self.navigate_active(Location::Local(mp.clone()));
                }
                resp.kept_context_menu(|ui| {
                    if ui.button(t("Disk usage")).clicked() {
                        self.open_disk_usage(mp.clone());
                        ui.close_kind(egui::UiKind::Menu);
                    }
                    if d.ejectable && ui.add_enabled(!self.ejecting.iter().any(|e| e.0 == mp), egui::Button::new(t("Eject"))).clicked() {
                        eject = Some(d.clone());
                        ui.close_kind(egui::UiKind::Menu);
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
                resp.kept_context_menu(|ui| {
                    if ui.button(t("Disconnect")).clicked() {
                        self.disconnect(id);
                        ui.close_kind(egui::UiKind::Menu);
                    }
                });
            }
            for c in self.cfg.connections.clone() {
                let resp = side_row(ui, th, false, &c.name, None, |p, r| icons::globe(p, r, th.weak)).on_hover_text(connection_hover(&c));
                if resp.clicked() {
                    self.connect_saved(c.clone());
                }
                resp.kept_context_menu(|ui| {
                    if ui.button(t("Edit")).clicked() {
                        self.show_dialog(Dialog::Connect { profile: remote::with_known_password(c.clone()), save: true }, false);
                        ui.close_kind(egui::UiKind::Menu);
                    }
                    if ui.button(t("Delete")).clicked() {
                        self.cfg.connections.retain(|x| x.name != c.name);
                        self.save_cfg();
                        ui.close_kind(egui::UiKind::Menu);
                    }
                });
            }
            if side_row(ui, th, false, &t("Connect to server…"), None, |p, r| icons::plus(p, r, th.weak)).clicked() {
                self.open_connect_dialog(None);
            }
        });
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
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
                        let seg =
                            egui::Rect::from_min_max(egui::pos2(r.left() + x.max(0.0), r.top()), egui::pos2((r.left() + x + 40.0).min(r.right()), r.bottom()));
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
                    ui.label(RichText::new(format!("+{} {}", self.jobs.len() - 1, t("more"))).size(11.5).color(th.weak)).on_hover_text(titles.join("\n"));
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

    pub(super) fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(if t.error { 8 } else { 4 }));
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::Area::new(egui::Id::unique("toasts")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-18.0, -44.0)).order(egui::Order::Foreground).show(
            ctx,
            |ui| {
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
            },
        );
    }
}

impl App {
    /// The toolbar's view options for the focused file window: hidden files, and list
    /// or icons. Dimmed (and closed) while another kind of window has the focus.
    fn view_options_button(&mut self, ui: &mut egui::Ui, th: &Theme) {
        use crate::config::ViewMode;
        let id = egui::Id::unique("view-options");
        let tab = self.panes[self.active].tab();
        let files = tab.is_files();
        let mode = tab.view_mode.unwrap_or(self.cfg.default_view);
        if !files && egui::Popup::is_id_open(ui.ctx(), id) {
            egui::Popup::close_id(ui.ctx(), id);
        }
        let open = egui::Popup::is_id_open(ui.ctx(), id);
        let resp = ui
            .add_enabled_ui(files, |ui| tool_icon(ui, th, &t("View options"), open, |p, r, c| view_icon(p, r, c, mode == ViewMode::Icons)))
            .inner
            .on_disabled_hover_text(t("View options are for file windows"));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, files, t("View options")));
        if resp.clicked() {
            egui::Popup::toggle_id(ui.ctx(), id);
        }
        crate::ui::popup_below(id, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_min_width(230.0);
            let mut hidden = self.cfg.show_hidden;
            if ui.checkbox(&mut hidden, format!("{}  ({})", t("Show hidden files"), hint(Action::ToggleHidden))).changed() {
                self.toggle_hidden();
            }
            ui.separator();
            let mut chosen = mode;
            ui.radio_value(&mut chosen, ViewMode::List, t("List"));
            ui.radio_value(&mut chosen, ViewMode::Icons, t("Icons"));
            if chosen != mode {
                self.set_view_mode(chosen);
            }
            // Make this window's view the default, without a trip to Settings.
            ui.separator();
            if mode == self.cfg.default_view {
                ui.label(RichText::new(format!("✓ {}", t("This is the default view"))).size(12.0).color(th.weak))
                    .on_hover_text(t("Windows that you have not switched show it. Settings → Files & layout"));
            } else if ui
                .button(t("Set as default view"))
                .on_hover_text(t("Every window that you have not switched shows this view, and so do new ones"))
                .clicked()
            {
                self.set_default_view(mode);
            }
        });
    }
}

impl App {
    /// The toolbar's split view: split the focused window (files or a terminal on any
    /// side, within 2 × 2), or close it.
    fn split_view_button(&mut self, ui: &mut egui::Ui, th: &Theme) {
        let id = egui::Id::unique("split-view");
        let open = egui::Popup::is_id_open(ui.ctx(), id);
        let resp = tool_icon(ui, th, &t("Split view"), open, icons::mi_split);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, t("Split view")));
        if resp.clicked() {
            egui::Popup::toggle_id(ui.ctx(), id);
        }
        crate::ui::popup_below(id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            crate::ui::pane::menu_style(ui);
            ui.set_min_width(240.0);
            let a = self.active;
            let can = [SplitDir::Right, SplitDir::Left, SplitDir::Down, SplitDir::Up].map(|d| self.layout.can_split(a, d));
            let entries = crate::ui::pane::split_entries(self.panes[a].tab().is_terminal(), can, self.panes.len());
            let mut cmd = None;
            crate::ui::pane::split_menu(ui, th, &entries, &mut cmd);
            if let Some(c) = cmd {
                self.run_cmd(c);
                ui.close();
            }
        });
    }
}

/// A flat toolbar icon; `active` draws a toggled background.
fn tool_icon(ui: &mut egui::Ui, th: &Theme, tip: &str, active: bool, draw: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::click());
    if active {
        ui.painter().rect_filled(rect, 7.0, th.selection);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 7.0, th.surface);
    }
    let color = if active {
        th.accent
    } else if resp.hovered() {
        th.strong()
    } else {
        th.weak
    };
    draw(ui.painter(), rect.shrink2(egui::vec2(8.0, 7.0)), color);
    resp.on_hover_text(tip)
}

/// The view of a file window: three rows, or four tiles for the icon view.
fn view_icon(p: &egui::Painter, r: egui::Rect, c: egui::Color32, icons: bool) {
    let s = egui::Stroke::new(1.3_f32, c);
    if icons {
        let g = r.width() * 0.16;
        let w = (r.width() - g) / 2.0;
        for (i, j) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            let tile = egui::Rect::from_min_size(r.min + egui::vec2(i * (w + g), j * (w + g)), egui::vec2(w, w));
            p.rect_stroke(tile, 1.5, s, egui::StrokeKind::Inside);
        }
    } else {
        for k in 0..3 {
            let y = r.top() + 1.5 + k as f32 * (r.height() - 3.0) / 2.0;
            p.circle_filled(egui::pos2(r.left() + 1.5, y), 1.3, c);
            p.line_segment([egui::pos2(r.left() + 5.0, y), egui::pos2(r.right(), y)], s);
        }
    }
}

fn sidebar_icon(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.3_f32, c);
    p.rect_stroke(r, 2.5, s, egui::StrokeKind::Inside);
    let x = r.left() + r.width() * 0.38;
    p.line_segment([egui::pos2(x, r.top() + 1.0), egui::pos2(x, r.bottom() - 1.0)], s);
}

/// Status-bar key hint: a keycap followed by a label, clickable.
fn key_hint_width(ui: &egui::Ui, key: &str, label: &str) -> f32 {
    let width = |text: &str| ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), egui::FontId::proportional(11.5), egui::Color32::WHITE).size().x);
    width(key) + 12.0 + 6.0 + width(label) + 8.0
}

fn key_hint(ui: &mut egui::Ui, th: &Theme, key: &str, label: &str) -> egui::Response {
    let font = egui::FontId::proportional(11.5);
    let kw = ui.fonts_mut(|f| f.layout_no_wrap(key.to_string(), font.clone(), th.weak).size().x);
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

/// Response for the chevron part of a split button (so the popup can anchor to both parts).
fn chev_rect_response(ui: &egui::Ui, r: egui::Rect) -> egui::Response {
    ui.interact(r, egui::Id::unique("terminal-chevron-anchor"), egui::Sense::hover())
}

/// A saved connection's address, and whether its password is held in memory this session.
fn connection_hover(c: &ConnectionProfile) -> String {
    if c.password.is_empty() && !remote::with_known_password(c.clone()).password.is_empty() {
        format!("{}\n🔒 {}", c.url(), t("Password in memory until Matrix Finder quits"))
    } else {
        c.url()
    }
}
