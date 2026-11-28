//! Bookmarks: the sidebar section, the bookmarks bar, their menus and Settings → Bookmarks.
use super::*;
use crate::ui::KeptContextMenu as _;

impl App {
    pub(super) fn open_bookmark(&mut self, b: &Bookmark) {
        let Some(loc) = self.parse_location_text(&bookmark_target(&b.path)) else { return };
        // Settings → Bookmarks: in a terminal window, `cd` the shell to a local folder.
        if self.cfg.bookmark_action == crate::config::BookmarkAction::FilesAndTerminal {
            if let (Location::Local(dir), Some(term)) = (&loc, self.panes[self.active].tab_mut().term.as_mut()) {
                if !crate::terminal::view::is_typeable(&dir.to_string_lossy()) {
                    self.toast(t("This folder's name has control characters: it is not typed into the terminal"), true);
                } else if !term.cd(dir) {
                    self.toast(t("The terminal is busy: quit the running program first"), true);
                }
                return;
            }
        }
        self.navigate_active(loc);
    }

    fn bookmark_icon(&self, p: &egui::Painter, r: egui::Rect, b: &Bookmark) {
        if b.folder {
            icons::bookmark_folder(p, r, self.theme.weak);
        } else {
            icons::bookmark_icon(p, r, b.icon.resolve(&b.path, self.cfg.bookmark_system_icons), self.theme);
        }
    }

    /// Right-click menu of a bookmark (sidebar and bookmarks bar).
    fn bookmark_menu(&mut self, ui: &mut egui::Ui, b: &Bookmark, remove: &mut Option<String>) {
        if ui.button(t("Open in new tab")).clicked() {
            if let Some(loc) = self.parse_location_text(&bookmark_target(&b.path)) {
                let (at, group) = self.next_slot();
                self.new_files_tab(at, group, Some(loc));
            }
            ui.close_kind(egui::UiKind::Menu);
        }
        if ui.button(t("Open in other panel")).clicked() {
            if let Some(loc) = self.parse_location_text(&bookmark_target(&b.path)) {
                let o = self.other_or_split();
                self.panes[o].tab_mut().navigate(loc, &lc!(self));
            }
            ui.close_kind(egui::UiKind::Menu);
        }
        if ui.button(t("Remove bookmark")).clicked() {
            *remove = Some(b.path.clone());
            ui.close_kind(egui::UiKind::Menu);
        }
        if ui.button(t("Manage bookmarks…")).clicked() {
            self.open_settings(SettingsPage::Bookmarks);
            ui.close_kind(egui::UiKind::Menu);
        }
    }

    /// Right-click menu of a bookmark folder.
    fn bookmark_folder_menu(&mut self, ui: &mut egui::Ui, i: usize) {
        let Some(f) = self.cfg.bookmarks.get(i).cloned() else { return };
        if ui.button(t("Open all in new tabs")).clicked() {
            for b in &f.children {
                if let Some(loc) = self.parse_location_text(&bookmark_target(&b.path)) {
                    let at = self.strip.tabs.len();
                    self.new_files_tab(at, None, Some(loc));
                }
            }
            ui.close_kind(egui::UiKind::Menu);
        }
        if ui.button(t("Remove group (keep its bookmarks)")).clicked() {
            remove_bookmark_folder(&mut self.cfg.bookmarks, i);
            self.save_cfg();
            ui.close_kind(egui::UiKind::Menu);
        }
        if ui.button(t("Manage bookmarks…")).clicked() {
            self.open_settings(SettingsPage::Bookmarks);
            ui.close_kind(egui::UiKind::Menu);
        }
    }

    /// One bookmark row in the sidebar.
    fn sidebar_bookmark(&mut self, ui: &mut egui::Ui, b: &Bookmark, active: &str, remove: &mut Option<String>) {
        let th = self.theme;
        let target = bookmark_target(&b.path);
        let resp = side_row(ui, th, target == active, &b.name, None, |p, r| self.bookmark_icon(p, r, b)).on_hover_text(&b.path);
        if std::path::Path::new(&target).is_dir() {
            self.sidebar_drop_target(ui, resp.rect, Location::Local(PathBuf::from(&target)));
        }
        if resp.clicked() {
            self.open_bookmark(b);
        }
        resp.kept_context_menu(|ui| self.bookmark_menu(ui, b, remove));
    }

    /// The bookmarks section of the sidebar (vertical bar): folders open and close.
    pub(super) fn sidebar_bookmarks(&mut self, ui: &mut egui::Ui, active: &str) {
        let th = self.theme;
        section_title(ui, th, &t("Bookmarks"));
        let mut remove = None;
        let mut toggle = None;
        for (i, b) in self.cfg.bookmarks.clone().iter().enumerate() {
            if !b.folder {
                self.sidebar_bookmark(ui, b, active, &mut remove);
                continue;
            }
            let resp = side_row(ui, th, false, &b.name, None, |p, r| icons::bookmark_folder(p, r, th.weak));
            let chev = egui::Rect::from_center_size(egui::pos2(resp.rect.right() - 14.0, resp.rect.center().y), egui::vec2(14.0, 14.0));
            icons::chevron(ui.painter(), chev, if b.collapsed { 1 } else { 2 }, th.weak);
            if resp.clicked() {
                toggle = Some(i);
            }
            let tip = format!("{} · {}", b.children.len(), if b.collapsed { t("Expand group") } else { t("Collapse group") });
            resp.on_hover_text(tip).kept_context_menu(|ui| self.bookmark_folder_menu(ui, i));
            if !b.collapsed {
                for c in &b.children {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(14.0);
                        ui.vertical(|ui| self.sidebar_bookmark(ui, c, active, &mut remove));
                    });
                }
            }
        }
        if self.cfg.bookmarks.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new(format!("{} ({})", t("Bookmark current folder"), hint(Action::Bookmark))).small().color(th.weak));
            });
        }
        if let Some(i) = toggle {
            self.cfg.bookmarks[i].collapsed = !self.cfg.bookmarks[i].collapsed;
            self.save_cfg();
        }
        if let Some(path) = remove {
            Bookmark::remove_path(&mut self.cfg.bookmarks, &path);
            self.save_cfg();
        }
    }

    /// Width of a chip in the bookmarks bar.
    fn chip_width(&self, ui: &egui::Ui, b: &Bookmark) -> f32 {
        let text = ui.fonts_mut(|f| f.layout_no_wrap(b.name.clone(), egui::FontId::proportional(13.0), egui::Color32::WHITE).size().x).min(160.0);
        text + if b.folder { 46.0 } else { 34.0 }
    }

    /// One bookmark or folder in the bookmarks bar.
    fn bar_chip(&mut self, ui: &mut egui::Ui, b: &Bookmark, i: usize, active: &str, remove: &mut Option<String>) {
        let th = self.theme;
        let w = self.chip_width(ui, b);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 26.0), egui::Sense::click());
        let popup = egui::Id::unique(("bookmark-folder", i));
        let open = b.folder && egui::Popup::is_id_open(ui.ctx(), popup);
        let p = ui.painter();
        if !b.folder && bookmark_target(&b.path) == active {
            p.rect_filled(rect, 7.0, th.selection);
        } else if resp.hovered() || open {
            p.rect_filled(rect, 7.0, th.hover());
        }
        self.bookmark_icon(p, egui::Rect::from_center_size(egui::pos2(rect.left() + 15.0, rect.center().y), egui::vec2(15.0, 15.0)), b);
        let text_w = w - if b.folder { 46.0 } else { 34.0 };
        let mut job = egui::text::LayoutJob::simple_singleline(b.name.clone(), egui::FontId::proportional(13.0), th.text);
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_w + 1.0);
        let g = ui.fonts_mut(|f| f.layout_job(job));
        p.galley(egui::pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0), g, th.text);
        if b.folder {
            let c = egui::Rect::from_center_size(egui::pos2(rect.right() - 12.0, rect.center().y), egui::vec2(12.0, 12.0));
            icons::chevron(p, c, 2, th.weak);
            if resp.clicked() {
                egui::Popup::toggle_id(ui.ctx(), popup);
            }
            crate::ui::popup_below(popup, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                ui.set_min_width(200.0);
                for c in &b.children {
                    self.bar_menu_item(ui, c, remove);
                }
                if b.children.is_empty() {
                    ui.label(RichText::new(t("Empty folder")).color(th.weak));
                }
            });
            resp.kept_context_menu(|ui| self.bookmark_folder_menu(ui, i));
        } else {
            let target = bookmark_target(&b.path);
            if std::path::Path::new(&target).is_dir() {
                self.sidebar_drop_target(ui, rect, Location::Local(PathBuf::from(&target)));
            }
            if resp.clicked() {
                self.open_bookmark(b);
            }
            let resp = if egui::DragAndDrop::has_any_payload(ui.ctx()) { resp } else { resp.on_hover_text(&b.path) };
            resp.kept_context_menu(|ui| self.bookmark_menu(ui, b, remove));
        }
    }

    /// A bookmark inside a bookmarks-bar menu (folder or overflow): icon and name.
    fn bar_menu_item(&mut self, ui: &mut egui::Ui, b: &Bookmark, remove: &mut Option<String>) {
        let th = self.theme;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(200.0), 26.0), egui::Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 5.0, th.hover());
        }
        self.bookmark_icon(ui.painter(), egui::Rect::from_center_size(egui::pos2(rect.left() + 14.0, rect.center().y), egui::vec2(15.0, 15.0)), b);
        ui.painter().text(egui::pos2(rect.left() + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, &b.name, egui::FontId::proportional(13.5), th.text);
        if resp.clicked() {
            self.open_bookmark(b);
        }
        resp.on_hover_text(&b.path).kept_context_menu(|ui| self.bookmark_menu(ui, b, remove));
    }

    /// The horizontal bookmarks bar above the tab bar. What does not fit goes into "»".
    pub(super) fn bookmarks_bar(&mut self, ui: &mut egui::Ui) {
        let th = self.theme;
        let active = self.panes[self.active].tab().loc.display(|id| self.remote_label(id));
        let list = self.cfg.bookmarks.clone();
        let mut remove = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let room = ui.available_width() - 30.0;
            let mut used = 0.0;
            let mut overflow = Vec::new();
            for (i, b) in list.iter().enumerate() {
                let w = self.chip_width(ui, b) + 2.0;
                if !overflow.is_empty() || used + w > room {
                    overflow.push(i);
                    continue;
                }
                used += w;
                self.bar_chip(ui, b, i, &active, &mut remove);
            }
            if !overflow.is_empty() {
                let more = icons::icon_button(ui, 26.0, true, th, |p, r, c| {
                    icons::chevron(p, r.translate(egui::vec2(-2.5, 0.0)), 1, c);
                    icons::chevron(p, r.translate(egui::vec2(2.5, 0.0)), 1, c);
                })
                .on_hover_text(t("More bookmarks"));
                let id = egui::Id::unique("bookmarks-more");
                if more.clicked() {
                    egui::Popup::toggle_id(ui.ctx(), id);
                }
                crate::ui::popup_below(id, &more, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                    ui.set_min_width(220.0);
                    for i in overflow {
                        let b = &list[i];
                        if b.folder {
                            ui.menu_button(format!("{}  ›", b.name), |ui| {
                                for c in &b.children {
                                    self.bar_menu_item(ui, c, &mut remove);
                                }
                            });
                        } else {
                            self.bar_menu_item(ui, b, &mut remove);
                        }
                    }
                });
            }
        });
        if let Some(path) = remove {
            Bookmark::remove_path(&mut self.cfg.bookmarks, &path);
            self.save_cfg();
        }
    }

    /// Settings → Bookmarks: where they are shown, icons, and the list itself.
    /// The actions above the bookmark list: add a bookmark (the folder the user was in,
    /// one they choose, or the standard folders still missing), add a group, and import
    /// or export the list.
    fn bookmark_toolbar(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        let have = |list: &[Bookmark], path: &str| Bookmark::leaves(list).any(|b| bookmark_target(&b.path) == bookmark_target(path));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let add = egui::Button::new(RichText::new(format!("+ {}", t("Add bookmark"))).color(th.on_accent()).strong())
                .fill(th.accent)
                .min_size(egui::vec2(0.0, 28.0));
            egui::containers::menu::MenuButton::from_button(add)
                .config(crate::ui::kept_menu_config())
                .ui(ui, |ui| {
                    ui.set_min_width(280.0);
                    // The folder of the file window the user was in (Settings is a tab of its own).
                    if let Some(loc) = self.last_files_loc.clone() {
                        let path = loc.display(|id| self.remote_label(id));
                        let name = loc.short_name();
                        let label = t("The folder you were in: {name}").replace("{name}", &name);
                        let r = ui
                            .add_enabled(!have(&self.cfg.bookmarks, &path), egui::Button::new(label))
                            .on_hover_text(&path)
                            .on_disabled_hover_text(t("Already a bookmark"));
                        if r.clicked() {
                            changed |= self.add_bookmark(name, path);
                            ui.close_kind(egui::UiKind::Menu);
                        }
                    }
                    if ui.button(t("Choose a folder…")).clicked() {
                        ui.close_kind(egui::UiKind::Menu);
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string());
                            changed |= self.add_bookmark(name, p.display().to_string());
                        }
                    }
                    ui.separator();
                    // Home, Desktop, Documents…: the ones not bookmarked yet, named.
                    let missing: Vec<Bookmark> = crate::config::system_bookmarks().into_iter().filter(|b| !have(&self.cfg.bookmarks, &b.path)).collect();
                    let label = if missing.is_empty() {
                        t("Standard folders: all bookmarked")
                    } else {
                        t("Standard folders: {names}").replace("{names}", &missing.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", "))
                    };
                    if ui.add_enabled(!missing.is_empty(), egui::Button::new(label)).clicked() {
                        self.toast(t("Added {n} bookmarks").replace("{n}", &missing.len().to_string()), false);
                        self.cfg.bookmarks.extend(missing);
                        changed = true;
                        ui.close_kind(egui::UiKind::Menu);
                    }
                })
                .0
                .on_hover_text(t("Add a folder you use often"));
            if ui
                .add(egui::Button::new(format!("+ {}", t("New group"))).min_size(egui::vec2(0.0, 28.0)))
                .on_hover_text(t("A group keeps bookmarks together: pick it in a bookmark's Group list"))
                .clicked()
            {
                self.cfg.bookmarks.push(Bookmark::new_folder(t("New group")));
                self.bm_focus_last = true;
                changed = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t("Export…")).on_hover_text(t("Save the bookmarks to a file")).clicked() {
                    self.export_bookmarks();
                }
                if ui.button(t("Import…")).on_hover_text(t("Add the bookmarks of a file saved with Export")).clicked() {
                    changed |= self.import_bookmarks();
                }
            });
        });
        changed
    }

    /// Bookmark `path` as `name`, unless it already is; says which it was.
    fn add_bookmark(&mut self, name: String, path: String) -> bool {
        if Bookmark::leaves(&self.cfg.bookmarks).any(|b| bookmark_target(&b.path) == bookmark_target(&path)) {
            self.toast(t("Already a bookmark"), false);
            return false;
        }
        self.toast(t("Added to bookmarks: {name}").replace("{name}", &name), false);
        self.cfg.bookmarks.push(Bookmark::new(name, path));
        true
    }

    fn export_bookmarks(&mut self) {
        if let Some(p) = rfd::FileDialog::new().set_file_name("matrix_finder-bookmarks.json").save_file() {
            let r = serde_json::to_string_pretty(&self.cfg.bookmarks).map_err(es).and_then(|s| std::fs::write(&p, s).map_err(es));
            match r {
                Ok(()) => self.toast(format!("{} {}", t("Exported to"), p.display()), false),
                Err(e) => self.toast(e, true),
            }
        }
    }

    fn import_bookmarks(&mut self) -> bool {
        let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() else { return false };
        let r: Res<Vec<Bookmark>> = std::fs::read_to_string(&p).map_err(es).and_then(|s| serde_json::from_str(&s).map_err(es));
        match r {
            Ok(list) => {
                for mut b in list {
                    // Skip what is already bookmarked (also inside imported groups).
                    let have = |x: &Bookmark, list: &[Bookmark]| Bookmark::leaves(list).any(|y| y.path == x.path);
                    if b.folder {
                        b.children.retain(|c| !have(c, &self.cfg.bookmarks));
                        self.cfg.bookmarks.push(b);
                    } else if !have(&b, &self.cfg.bookmarks) {
                        self.cfg.bookmarks.push(b);
                    }
                }
                true
            }
            Err(e) => {
                self.toast(e, true);
                false
            }
        }
    }

    pub(super) fn bookmark_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        use crate::config::{BookmarkAction, BookmarkBar, BookmarkIcon};
        let mut changed = false;
        settings_section(ui, th, &t("Bookmarks bar"), &t("Where bookmarks are shown and what they do."));
        let mut top = card_frame(th).begin(ui);
        {
            let ui = &mut top.content_ui;
            ui.set_width(ui.available_width());
            changed |= ui.radio_value(&mut self.cfg.bookmark_bar, BookmarkBar::Vertical, t("Vertical bar (in the sidebar)")).changed();
            changed |= ui.radio_value(&mut self.cfg.bookmark_bar, BookmarkBar::Horizontal, t("Horizontal bar (above the tabs)")).changed();
            ui.add_space(4.0);
            ui.label(RichText::new(t("Choosing a bookmark")).strong());
            changed |= ui
                .radio_value(&mut self.cfg.bookmark_action, BookmarkAction::FilesOnly, t("Go to file only"))
                .on_hover_text(t("In a terminal, the folder opens in a file window of the tab (or a new tab)"))
                .changed();
            changed |= ui
                .radio_value(&mut self.cfg.bookmark_action, BookmarkAction::FilesAndTerminal, t("File and Terminal"))
                .on_hover_text(t("In a terminal, run cd to the folder"))
                .changed();
            ui.add_space(4.0);
            changed |= toggle_row(
                ui,
                th,
                &mut self.cfg.bookmark_system_icons,
                &t("Icons for standard folders (Desktop, Documents, Downloads, …)"),
                &t("Off: every bookmark gets a plain folder icon (unless you picked an icon for it)"),
            );
        }
        top.end(ui);

        settings_section(
            ui,
            th,
            &t("Bookmarks"),
            &t("Rename them and pick icons. Drag a row by its handle to reorder it, or to move a bookmark into or out of a group."),
        );
        let folders: Vec<(usize, String)> = self.cfg.bookmarks.iter().enumerate().filter(|(_, b)| b.folder).map(|(i, b)| (i, b.name.clone())).collect();
        let mut list = card_frame(th).begin(ui);
        let ui_outer = ui;
        let ui = &mut list.content_ui;
        ui.set_width(ui.available_width());
        changed |= self.bookmark_toolbar(ui, th);
        ui.add_space(6.0);
        let focus_last = std::mem::take(&mut self.bm_focus_last);
        // The location takes the width the rest of a row leaves (a row inside a folder is indented).
        let path_w = (ui.available_width() - 433.0).max(120.0);
        let mut edit: Option<BmEdit> = None;
        let sys = self.cfg.bookmark_system_icons;
        let n = self.cfg.bookmarks.len();
        let mut paths = std::mem::take(&mut self.bookmark_paths);
        let (drives, show_hidden) = (&self.drives, self.cfg.show_hidden);
        let home = dirs::home_dir();
        // Each row's place and its drag handle, to find where a dragged row is dropped.
        let mut rows: Vec<(BmRow, egui::Rect)> = Vec::new();
        let mut grips: Vec<((usize, Option<usize>), egui::Response)> = Vec::new();
        let mut row =
            |ui: &mut egui::Ui, b: &mut Bookmark, at: (usize, Option<usize>), count: usize, edit: &mut Option<BmEdit>, changed: &mut bool, focus: bool| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if at.1.is_some() {
                        ui.add_space(22.0);
                    }
                    let (gr, _) = ui.allocate_exact_size(egui::vec2(14.0, 26.0), egui::Sense::hover());
                    let grip = ui.interact(gr, egui::Id::unique(("bm-grip", at)), egui::Sense::drag());
                    let c = if grip.hovered() || grip.dragged() { th.text } else { th.weak };
                    super::settings_ui::grip_dots(ui.painter(), gr.center(), c);
                    let grip = if grip.dragged() { grip } else { grip.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text(t("Drag to reorder")) };
                    if b.folder {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                        icons::bookmark_folder(ui.painter(), r.shrink(5.0), th.weak);
                    } else {
                        // Icon: click for the choices.
                        let shown = b.icon.resolve(&b.path, sys);
                        let resp = icons::icon_button(ui, 26.0, true, th, |p, r, _| icons::bookmark_icon(p, r.shrink(1.0), shown, th)).on_hover_text(t("Icon"));
                        let id = resp.id.with("icon-pick");
                        if resp.clicked() {
                            egui::Popup::toggle_id(ui.ctx(), id);
                        }
                        crate::ui::popup_below(id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                            ui.set_min_width(170.0);
                            for choice in BookmarkIcon::CHOICES {
                                let (r, resp) = ui.allocate_exact_size(egui::vec2(170.0, 24.0), egui::Sense::click());
                                crate::ui::pane::menu_row_fill(ui, r, th, b.icon == choice, resp.hovered());
                                let draw = choice.resolve(&b.path, sys);
                                icons::bookmark_icon(
                                    ui.painter(),
                                    egui::Rect::from_center_size(egui::pos2(r.left() + 14.0, r.center().y), egui::vec2(15.0, 15.0)),
                                    draw,
                                    th,
                                );
                                ui.painter().text(
                                    egui::pos2(r.left() + 30.0, r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    t(choice.label()),
                                    egui::FontId::proportional(13.0),
                                    th.text,
                                );
                                if resp.clicked() {
                                    b.icon = choice;
                                    *changed = true;
                                }
                            }
                        });
                    }
                    let name = ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(if b.folder { 200.0 } else { 130.0 }).hint_text(t("Name")));
                    // A group just made: in view, its name ready to type.
                    if focus {
                        name.request_focus();
                        name.scroll_to_me(Some(egui::Align::Center));
                    }
                    *changed |= name.changed();
                    if !b.folder {
                        let o = crate::ui::pathfield::Opts { base: home.as_deref(), show_hidden, drives, width: path_w, hint: Some(t("Location")) };
                        *changed |= paths.show(ui, &mut b.path, egui::Id::unique(("bm-path", at)), o, th).changed();
                        // Folder to keep it in.
                        let cur = at.1.and_then(|_| folders.iter().find(|(i, _)| *i == at.0)).map(|(_, n)| n.clone()).unwrap_or_else(|| "—".into());
                        egui::ComboBox::from_id_salt(("bm-folder", at.0, at.1))
                            .width(110.0)
                            .selected_text(cur)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(at.1.is_none(), t("No group")).clicked() && at.1.is_some() {
                                    *edit = Some(BmEdit::MoveTo(at, None));
                                }
                                for (fi, name) in &folders {
                                    let here = at.1.is_some() && at.0 == *fi;
                                    if ui.selectable_label(here, if name.is_empty() { t("Unnamed group") } else { name.clone() }).clicked() && !here {
                                        *edit = Some(BmEdit::MoveTo(at, Some(*fi)));
                                    }
                                }
                            })
                            .response
                            .on_hover_text(t("Group"));
                    }
                    let k = at.1.unwrap_or(at.0);
                    if icons::icon_button(ui, 24.0, k > 0, th, |p, r, c| icons::chevron(p, r, 0, c)).on_hover_text(t("Move up")).clicked() {
                        *edit = Some(BmEdit::Up(at));
                    }
                    if icons::icon_button(ui, 24.0, k + 1 < count, th, |p, r, c| icons::chevron(p, r, 2, c)).on_hover_text(t("Move down")).clicked() {
                        *edit = Some(BmEdit::Down(at));
                    }
                    let tip = if b.folder { t("Remove group (keep its bookmarks)") } else { t("Remove bookmark") };
                    if icons::icon_button(ui, 24.0, true, th, icons::mi_trash).on_hover_text(tip).clicked() {
                        *edit = Some(BmEdit::Delete(at));
                    }
                    grip
                })
            };
        for i in 0..n {
            let b = &mut self.cfg.bookmarks[i];
            let folder = b.folder;
            let r = row(ui, b, (i, None), n, &mut edit, &mut changed, focus_last && i + 1 == n);
            rows.push((BmRow::Top(i, folder), r.response.rect));
            grips.push(((i, None), r.inner));
            if folder {
                let m = self.cfg.bookmarks[i].children.len();
                for j in 0..m {
                    let c = &mut self.cfg.bookmarks[i].children[j];
                    let r = row(ui, c, (i, Some(j)), m, &mut edit, &mut changed, false);
                    rows.push((BmRow::Child(i, j), r.response.rect));
                    grips.push(((i, Some(j)), r.inner));
                }
                if m == 0 {
                    let r = ui.horizontal(|ui| {
                        ui.add_space(74.0);
                        ui.label(RichText::new(t("Empty group: drag a bookmark here or pick it in a bookmark's Group list")).size(11.5).color(th.weak));
                    });
                    rows.push((BmRow::Empty(i), r.response.rect));
                }
            }
        }
        self.bookmark_paths = paths;
        if let Some((from, grip)) = grips.into_iter().find(|(_, g)| g.dragged() || g.drag_stopped()) {
            if let Some(e) = self.bookmark_drag(ui, th, &rows, from, &grip) {
                edit = Some(e);
            }
        }
        if let Some(e) = edit {
            apply_bookmark_edit(&mut self.cfg.bookmarks, e);
            changed = true;
        }
        if n == 0 {
            ui.label(RichText::new(t("No bookmarks yet: add the folder you were in, or choose one with Add bookmark.")).color(th.weak));
        }
        list.end(ui_outer);
        changed
    }

    /// A row of Settings → Bookmarks dragged by its handle: show where it goes (a line, the
    /// row's name at the pointer) and, when it is let go there, the move to make. Let go
    /// away from the list, it stays where it was.
    fn bookmark_drag(
        &self,
        ui: &mut egui::Ui,
        th: &Theme,
        rows: &[(BmRow, egui::Rect)],
        from: (usize, Option<usize>),
        grip: &egui::Response,
    ) -> Option<BmEdit> {
        let list = &self.cfg.bookmarks;
        let b = match from {
            (i, None) => list.get(i)?,
            (i, Some(j)) => list.get(i)?.children.get(j)?,
        };
        let group = from.1.is_none() && b.folder;
        let area = rows.iter().fold(egui::Rect::NOTHING, |a, (_, r)| a.union(*r));
        let pointer = ui.ctx().pointer_latest_pos()?;
        let gap = ui.spacing().item_spacing.y;
        let slot = Some(pointer)
            .filter(|p| area.expand2(egui::vec2(40.0, 30.0)).contains(*p))
            .and_then(|p| drop_slot(rows, group, p.y, gap))
            .filter(|(s, ..)| !bm_same_place(from, *s));
        let p = ui.painter();
        if let Some((_, r)) = rows.iter().find(|(k, _)| k.at() == Some(from)) {
            p.rect_stroke(r.expand(1.0), 6.0, egui::Stroke::new(1.0_f32, th.accent), egui::StrokeKind::Outside);
        }
        if let Some((_, y, indent)) = slot {
            let x0 = area.left() + if indent { 22.0 } else { 0.0 };
            p.circle_stroke(egui::pos2(x0 + 3.0, y), 3.0, egui::Stroke::new(2.0_f32, th.accent));
            p.line_segment([egui::pos2(x0 + 6.0, y), egui::pos2(area.right(), y)], egui::Stroke::new(2.0_f32, th.accent));
        }
        let name = if b.name.is_empty() {
            if b.folder {
                t("Unnamed group")
            } else {
                b.path.clone()
            }
        } else {
            b.name.clone()
        };
        super::settings_ui::drag_feedback(ui, grip, &name);
        if grip.drag_stopped() {
            return slot.map(|(s, ..)| BmEdit::Place(from, s));
        }
        None
    }

    pub(super) fn toggle_bookmark(&mut self, path: String, name: String) {
        // A bookmark kept as `~/…` is the same folder.
        let stored = Bookmark::leaves(&self.cfg.bookmarks).find(|b| bookmark_target(&b.path) == path).map(|b| b.path.clone()).unwrap_or_else(|| path.clone());
        if Bookmark::remove_path(&mut self.cfg.bookmarks, &stored) {
            self.toast(format!("{}: {name}", t("Bookmark removed")), false);
        } else {
            self.cfg.bookmarks.push(Bookmark::new(name.clone(), path));
            self.toast(format!("{}: {name}", t("Bookmark added")), false);
        }
        self.save_cfg();
    }
}

/// An edit made in Settings → Bookmarks, at (top-level index, index inside that folder).
#[derive(Clone, Copy, Debug)]
enum BmEdit {
    Up((usize, Option<usize>)),
    Down((usize, Option<usize>)),
    Delete((usize, Option<usize>)),
    /// Move a bookmark into a folder (top-level index) or out of its folder (`None`).
    MoveTo((usize, Option<usize>), Option<usize>),
    /// Move a bookmark or group (dragged by its handle) to a place in the list.
    Place((usize, Option<usize>), BmSlot),
}

/// A place in the bookmark list: before bookmark `index` of the top list (`group: None`)
/// or of group `group`'s bookmarks; `index` may be the length (at the end).
#[derive(Clone, Copy, Debug, PartialEq)]
struct BmSlot {
    group: Option<usize>,
    index: usize,
}

/// A row of Settings → Bookmarks: a top-level bookmark or group (`true`: a group), a
/// bookmark in group `.0`, or the note in an empty group.
#[derive(Clone, Copy, Debug, PartialEq)]
enum BmRow {
    Top(usize, bool),
    Child(usize, usize),
    Empty(usize),
}

impl BmRow {
    fn at(self) -> Option<(usize, Option<usize>)> {
        match self {
            BmRow::Top(i, _) => Some((i, None)),
            BmRow::Child(i, j) => Some((i, Some(j))),
            BmRow::Empty(_) => None,
        }
    }
}

/// Where a row dragged to height `y` goes, with the height of the line that shows it and
/// whether that line is indented (inside a group). Over the upper half of a row it goes
/// before that row, over the lower half after it; after a group's own row is the start of
/// the group. A group (`group`) only moves among the top-level rows, with its bookmarks.
/// `gap` is the space between rows, the line is drawn in its middle.
fn drop_slot(rows: &[(BmRow, egui::Rect)], group: bool, y: f32, gap: f32) -> Option<(BmSlot, f32, bool)> {
    let last = rows.last()?.1;
    let top = |index| BmSlot { group: None, index };
    let inside = |g, index| BmSlot { group: Some(g), index };
    let n = rows.iter().filter(|(r, _)| matches!(r, BmRow::Top(..))).count();
    let (before, after) = (|r: egui::Rect| r.top() - gap / 2.0, |r: egui::Rect| r.bottom() + gap / 2.0);
    if group {
        let starts: Vec<(usize, egui::Rect)> = rows.iter().filter_map(|(k, r)| if let BmRow::Top(i, _) = k { Some((*i, *r)) } else { None }).collect();
        for (k, (i, r)) in starts.iter().enumerate() {
            // A group's block reaches to the next top-level row.
            let end = starts.get(k + 1).map_or(last.bottom(), |(_, next)| next.top());
            if y < (r.top() + end) / 2.0 {
                return Some((top(*i), before(*r), false));
            }
        }
        return Some((top(n), after(last), false));
    }
    let Some(&(row, r)) = rows.iter().find(|(_, r)| y < r.bottom()) else {
        // Below the list: after its last row, or at the very end when well below it.
        return match rows.last()? {
            (BmRow::Top(i, false), r) => Some((top(i + 1), after(*r), false)),
            (_, r) if y > r.bottom() + 8.0 => Some((top(n), after(*r), false)),
            (BmRow::Child(i, j), r) => Some((inside(*i, j + 1), after(*r), true)),
            (BmRow::Empty(i), r) => Some((inside(*i, 0), r.center().y, true)),
            (BmRow::Top(i, true), r) => Some((inside(*i, 0), after(*r), true)),
        };
    };
    let upper = y < r.center().y;
    Some(match (row, upper) {
        (BmRow::Top(i, _), true) => (top(i), before(r), false),
        (BmRow::Top(i, false), false) => (top(i + 1), after(r), false),
        (BmRow::Top(i, true), false) => (inside(i, 0), after(r), true),
        (BmRow::Child(i, j), true) => (inside(i, j), before(r), true),
        (BmRow::Child(i, j), false) => (inside(i, j + 1), after(r), true),
        (BmRow::Empty(i), _) => (inside(i, 0), r.center().y, true),
    })
}

/// Whether moving the row at `from` to `slot` leaves it where it is.
fn bm_same_place(from: (usize, Option<usize>), slot: BmSlot) -> bool {
    match from {
        (i, None) => slot.group.is_none() && (slot.index == i || slot.index == i + 1),
        (g, Some(j)) => slot.group == Some(g) && (slot.index == j || slot.index == j + 1),
    }
}

/// Move the bookmark or group at `from` to `slot` (a place counted before the move).
/// A group cannot go into a group; out-of-range places are ignored.
fn place_bookmark(list: &mut Vec<Bookmark>, from: (usize, Option<usize>), slot: BmSlot) {
    let valid = match from {
        (i, None) => i < list.len(),
        (i, Some(j)) => list.get(i).is_some_and(|b| b.folder && j < b.children.len()),
    };
    if !valid {
        return;
    }
    let target_ok = match slot.group {
        None => slot.index <= list.len(),
        Some(g) => list.get(g).is_some_and(|f| f.folder && slot.index <= f.children.len()) && !(from.1.is_none() && (list[from.0].folder || from.0 == g)),
    };
    if !target_ok || bm_same_place(from, slot) {
        return;
    }
    let b = match from {
        (i, Some(j)) => list[i].children.remove(j),
        (i, None) => list.remove(i),
    };
    match slot.group {
        None => {
            let k = if from.1.is_none() && slot.index > from.0 { slot.index - 1 } else { slot.index };
            list.insert(k.min(list.len()), b);
        }
        Some(g) => {
            let g = if from.1.is_none() && g > from.0 { g - 1 } else { g };
            let k = match from {
                (i, Some(j)) if i == g && slot.index > j => slot.index - 1,
                _ => slot.index,
            };
            let c = &mut list[g].children;
            c.insert(k.min(c.len()), b);
        }
    }
}

fn apply_bookmark_edit(list: &mut Vec<Bookmark>, e: BmEdit) {
    match e {
        BmEdit::Up((i, None)) if i > 0 => list.swap(i, i - 1),
        BmEdit::Up((i, Some(j))) if j > 0 => list[i].children.swap(j, j - 1),
        BmEdit::Down((i, None)) if i + 1 < list.len() => list.swap(i, i + 1),
        BmEdit::Down((i, Some(j))) if j + 1 < list[i].children.len() => list[i].children.swap(j, j + 1),
        BmEdit::Delete((i, None)) if list[i].folder => remove_bookmark_folder(list, i),
        BmEdit::Delete((i, None)) => {
            list.remove(i);
        }
        BmEdit::Delete((i, Some(j))) => {
            list[i].children.remove(j);
        }
        BmEdit::MoveTo(at, target) => {
            let b = match at {
                (i, Some(j)) => list[i].children.remove(j),
                (i, None) if !list[i].folder => list.remove(i),
                _ => return,
            };
            match target {
                // Out of a folder: right after it.
                None => list.insert((at.0 + 1).min(list.len()), b),
                Some(f) => {
                    let f = if at.1.is_none() && at.0 < f { f - 1 } else { f };
                    list[f].children.push(b);
                }
            }
        }
        BmEdit::Place(from, slot) => place_bookmark(list, from, slot),
        _ => {}
    }
}

/// Remove bookmark folder `i`; its bookmarks take its place.
fn remove_bookmark_folder(list: &mut Vec<Bookmark>, i: usize) {
    let f = list.remove(i);
    for (k, c) in f.children.into_iter().enumerate() {
        list.insert(i + k, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[Bookmark]) -> Vec<String> {
        list.iter().map(|b| if b.folder { format!("[{}: {}]", b.name, names(&b.children).join(" ")) } else { b.name.clone() }).collect()
    }

    #[test]
    fn edit_bookmarks_and_folders() {
        let mut l = vec![Bookmark::new("a", "/a"), Bookmark::new("b", "/b"), Bookmark::new_folder("F"), Bookmark::new("c", "/c")];
        apply_bookmark_edit(&mut l, BmEdit::MoveTo((0, None), Some(2))); // a into F (F shifts to 1)
        assert_eq!(names(&l), ["b", "[F: a]", "c"]);
        apply_bookmark_edit(&mut l, BmEdit::MoveTo((2, None), Some(1))); // c into F
        assert_eq!(names(&l), ["b", "[F: a c]"]);
        apply_bookmark_edit(&mut l, BmEdit::Up((1, Some(1))));
        assert_eq!(names(&l), ["b", "[F: c a]"]);
        apply_bookmark_edit(&mut l, BmEdit::MoveTo((1, Some(0)), None)); // c out, right after F
        assert_eq!(names(&l), ["b", "[F: a]", "c"]);
        apply_bookmark_edit(&mut l, BmEdit::Up((0, None)));
        assert_eq!(names(&l), ["b", "[F: a]", "c"], "the first one cannot go up");
        apply_bookmark_edit(&mut l, BmEdit::Down((0, None)));
        assert_eq!(names(&l), ["[F: a]", "b", "c"]);
        // Removing a folder keeps its bookmarks in its place.
        apply_bookmark_edit(&mut l, BmEdit::Delete((0, None)));
        assert_eq!(names(&l), ["a", "b", "c"]);
        apply_bookmark_edit(&mut l, BmEdit::Delete((1, None)));
        assert_eq!(names(&l), ["a", "c"]);
    }

    fn slot(group: Option<usize>, index: usize) -> BmSlot {
        BmSlot { group, index }
    }

    #[test]
    fn dragged_bookmarks_go_where_they_are_dropped() {
        let start = || vec![Bookmark::new("a", "/a"), Bookmark::new("b", "/b"), Bookmark::new_folder("F"), Bookmark::new("c", "/c")];
        let moved = |from, to| {
            let mut l = start();
            l[2].children = vec![Bookmark::new("x", "/x"), Bookmark::new("y", "/y")];
            apply_bookmark_edit(&mut l, BmEdit::Place(from, to));
            names(&l)
        };
        // Within the top list, both ways.
        assert_eq!(moved((0, None), slot(None, 2)), ["b", "a", "[F: x y]", "c"]);
        assert_eq!(moved((3, None), slot(None, 0)), ["c", "a", "b", "[F: x y]"]);
        assert_eq!(moved((0, None), slot(None, 4)), ["b", "[F: x y]", "c", "a"], "to the end");
        // A group moves with its bookmarks.
        assert_eq!(moved((2, None), slot(None, 0)), ["[F: x y]", "a", "b", "c"]);
        // Into a group (it is after the bookmark: its index shifts), out of it, within it.
        assert_eq!(moved((0, None), slot(Some(2), 1)), ["b", "[F: x a y]", "c"]);
        assert_eq!(moved((3, None), slot(Some(2), 0)), ["a", "b", "[F: c x y]"]);
        assert_eq!(moved((2, Some(1)), slot(None, 0)), ["y", "a", "b", "[F: x]", "c"]);
        assert_eq!(moved((2, Some(0)), slot(None, 3)), ["a", "b", "[F: y]", "x", "c"], "right after the group");
        assert_eq!(moved((2, Some(0)), slot(Some(2), 2)), ["a", "b", "[F: y x]", "c"]);
        // Nothing happens: the same place, a group into a group, places that do not exist.
        let same = names(&{
            let mut l = start();
            l[2].children = vec![Bookmark::new("x", "/x"), Bookmark::new("y", "/y")];
            l
        });
        for (from, to) in [
            ((1, None), slot(None, 1)),
            ((1, None), slot(None, 2)),
            ((2, Some(0)), slot(Some(2), 1)),
            ((2, None), slot(Some(2), 0)),
            ((0, None), slot(Some(1), 0)),
            ((9, None), slot(None, 0)),
            ((2, Some(5)), slot(None, 0)),
            ((0, None), slot(None, 9)),
            ((0, None), slot(Some(2), 9)),
        ] {
            assert_eq!(moved(from, to), same, "{from:?} → {to:?}");
        }
    }

    #[test]
    fn the_drop_place_follows_the_pointer() {
        // Rows 20 high with 4 between: a, F, x (in F), the note of the empty group E, b.
        let kinds = [BmRow::Top(0, false), BmRow::Top(1, true), BmRow::Child(1, 0), BmRow::Top(2, true), BmRow::Empty(2), BmRow::Top(3, false)];
        let rows: Vec<(BmRow, egui::Rect)> =
            kinds.iter().enumerate().map(|(k, r)| (*r, egui::Rect::from_min_size(egui::pos2(0.0, k as f32 * 24.0), egui::vec2(300.0, 20.0)))).collect();
        let at = |group, y| drop_slot(&rows, group, y, 4.0).map(|(s, line, indent)| (s.group, s.index, line, indent));
        assert_eq!(at(false, -30.0), Some((None, 0, -2.0, false)), "above the list: first");
        assert_eq!(at(false, 5.0), Some((None, 0, -2.0, false)), "upper half of a: before it");
        assert_eq!(at(false, 15.0), Some((None, 1, 22.0, false)), "lower half of a: after it");
        assert_eq!(at(false, 40.0), Some((Some(1), 0, 46.0, true)), "lower half of a group's row: its start");
        assert_eq!(at(false, 64.0), Some((Some(1), 1, 70.0, true)), "lower half of its last bookmark: its end");
        assert_eq!(at(false, 75.0), Some((None, 2, 70.0, false)), "upper half of the next row: after the group");
        assert_eq!(at(false, 100.0), Some((Some(2), 0, 106.0, true)), "an empty group's note: into it");
        assert_eq!(at(false, 125.0), Some((None, 3, 118.0, false)), "before b");
        assert_eq!(at(false, 135.0), Some((None, 4, 142.0, false)), "after b, the last");
        assert_eq!(at(false, 300.0), Some((None, 4, 142.0, false)), "below the list: the end");
        // A group only goes between top-level rows: F's block is rows 24..72.
        assert_eq!(at(true, 40.0), Some((None, 1, 22.0, false)));
        assert_eq!(at(true, 60.0), Some((None, 2, 70.0, false)), "lower half of F with x: after it");
        assert_eq!(at(true, 300.0), Some((None, 4, 142.0, false)));
        assert_eq!(drop_slot(&[], false, 0.0, 4.0), None);
    }
}
