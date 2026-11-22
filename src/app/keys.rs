//! The keyboard: shortcuts, the file list's keys and the type-to-search box.
use super::*;

impl App {
    /// Esc closes the app's window in front, while the keyboard is the windows': not
    /// while a text field, the palette, a folder tree or a drag has it, nor after a click
    /// in the files or a terminal (Esc is theirs then).
    fn esc_closes_window(&mut self, ctx: &egui::Context) -> bool {
        // A widget with the keyboard keeps Esc, unless it is a button of the window
        // reached with Tab (a text field clears or cancels with it).
        let focus_keeps_esc = ctx.memory(|m| m.focused()).is_some_and(|id| {
            let in_window = ctx.read_response(id).is_some_and(|r| self.windows.iter().any(|w| r.layer_id.id == window_id(w)));
            !in_window || egui::TextEdit::load_state(ctx, id).is_some()
        });
        if !self.keys_to_window
            || self.quit == Quit::Asking
            || !ctx.input(|i| i.key_pressed(Key::Escape) && !i.modifiers.any())
            || ctx.memory(|m| m.any_popup_open())
            || focus_keeps_esc
            || self.palette.open
            || crate::pane::drag_payload(ctx).is_some()
            || self.backup.paths.tree.is_some()
        {
            return false;
        }
        let Some(w) = self.windows.pop() else { return false };
        match w {
            "reset" => self.confirm_reset = false,
            "viewer" => self.viewer = None,
            "help" => self.show_help = false,
            "about" => self.show_about = false,
            "update" => self.update_found = None,
            _ => {
                self.backup.open = false;
                self.backup.paths.reset();
            }
        }
        ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        true
    }

    /// The next key press while Settings waits for a new shortcut: it goes nowhere else.
    fn capture_shortcut(&mut self, ctx: &egui::Context, action: Action, slot: Option<usize>) {
        let pressed = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } => Some((*key, *physical_key, *modifiers)),
                _ => None,
            })
        });
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_))));
        let Some((key, physical, m)) = pressed else { return };
        if key == Key::Escape && !m.any() {
            self.shortcut_capture = None;
            self.shortcut_note = None;
            return;
        }
        // Digit and punctuation keys report what they type: keep the key itself.
        let is_digit = |k: Key| k.name().len() == 1 && k.name().as_bytes()[0].is_ascii_digit();
        let is_letter = |k: Key| k.name().len() == 1 && k.name().as_bytes()[0].is_ascii_uppercase();
        let key = match physical {
            Some(p) if !is_letter(p) && (m.shift || m.alt || is_digit(p)) => p,
            _ => key,
        };
        let chord = Chord { ctrl: m.ctrl, alt: m.alt, shift: m.shift, cmd: m.mac_cmd, key };
        if let Some(why) = chord.refused_because() {
            self.shortcut_note = Some(why);
            return;
        }
        let current = shortcuts::active();
        let os = current.os();
        let mut note = None;
        if let Some(other) = current.used_by(chord, action) {
            let keys: Vec<Chord> = current.keys(other).iter().copied().filter(|c| *c != chord).collect();
            shortcuts::store(&mut self.cfg.shortcuts, os, other, &keys);
            note = Some(format!("{} {} ({})", t("Taken away from"), other.label(), chord.display(os)));
        }
        let mut keys = current.keys(action).to_vec();
        keys.retain(|c| *c != chord);
        match slot {
            Some(i) if i < keys.len() => keys[i] = chord,
            _ => keys.push(chord),
        }
        shortcuts::store(&mut self.cfg.shortcuts, os, action, &keys);
        if action.works_in_terminals() && !chord.leaves_terminals_alone() && note.is_none() {
            note = Some(t("In a terminal this key stays with the program there (Vim, Emacs…); it works in other windows."));
        }
        shortcuts::install(&self.cfg.shortcuts);
        self.shortcut_note = note;
        self.shortcut_capture = None;
        self.save_cfg();
    }

    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        // The address bar's folder tree takes its keys first (↑ ↓ ← → Enter Esc).
        if let Some(side) = self.panes.iter().position(|p| p.tab().tree.is_some()) {
            let res = self.panes[side].tab_mut().tree.as_mut().and_then(|t| t.keys(ctx));
            let handled = res.is_some();
            match res {
                Some(crate::foldertree::Popup::Picked(p)) => {
                    self.panes[side].tab_mut().tree = None;
                    self.active = side;
                    self.panes[side].tab_mut().navigate(Location::Local(p), &lc!(self));
                }
                Some(crate::foldertree::Popup::Closed) => self.panes[side].tab_mut().tree = None,
                _ => {}
            }
            if handled {
                return;
            }
        }
        if let Some((action, slot)) = self.shortcut_capture {
            self.capture_shortcut(ctx, action, slot);
            return;
        }
        if self.esc_closes_window(ctx) {
            return;
        }
        let term_mode = self.panes[self.active].tab().is_terminal();
        let keys = shortcuts::active();
        // The palette has its keys in text fields too, and closes by them.
        let palette_key = ctx.input_mut(|i| {
            let before = i.events.len();
            i.events.retain(|e| match e {
                egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } => keys.action_for(*key, *physical_key, *modifiers, term_mode) != Some(Action::Palette),
                _ => true,
            });
            i.events.len() != before
        });
        if palette_key {
            self.palette.toggle();
            return;
        }
        if self.palette.open {
            return;
        }
        if term_mode {
            // The terminal keeps the keyboard; only text fields (e.g. a tab-group name)
            // can take it away. Anything else hands it back to the terminal.
            let a = self.active;
            let tid = self.panes[a].tab().term.as_ref().map(|t| t.id);
            if let Some(id) = ctx.memory(|m| m.focused()) {
                if egui::TextEdit::load_state(ctx, id).is_some() {
                    return;
                }
                if Some(id) != tid {
                    ctx.memory_mut(|m| m.surrender_focus(id));
                    if let Some(t) = &mut self.panes[a].tab_mut().term {
                        t.want_focus = true;
                    }
                }
            }
        } else if let Some(id) = ctx.memory(|m| m.focused()) {
            // Only text fields keep the keyboard; any other focused widget (e.g. a button
            // reached with Tab) gives it back to the file panels.
            if egui::TextEdit::load_state(ctx, id).is_some() {
                return;
            }
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
        if !term_mode {
            self.type_search_keys(ctx);
        }
        // Take the shortcuts out of the input, so that a terminal does not get them too.
        let mut actions = vec![];
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Key { key, physical_key, pressed, modifiers, .. } => match keys.action_for(*key, *physical_key, *modifiers, term_mode) {
                    Some(action) => {
                        if *pressed {
                            actions.push(action);
                        }
                        false
                    }
                    None => true,
                },
                _ => true,
            });
        });
        let shortcut_pressed = !actions.is_empty();
        for action in actions {
            self.run_shortcut(ctx, action);
            // It may have opened something, or switched between a terminal and files.
            if self.dialog.is_some() || self.palette.open || self.panes[self.active].tab().is_terminal() != term_mode {
                return;
            }
        }
        // A window in front has the keyboard: the files behind it (drawn inactive) take
        // no typing, Enter or Delete.
        if self.keys_to_window {
            return;
        }
        if !term_mode && !shortcut_pressed && self.type_to_search(ctx) {
            return;
        }
        if !term_mode {
            self.file_list_keys(ctx);
        }
    }

    /// Do what a shortcut stands for.
    pub(super) fn run_shortcut(&mut self, ctx: &egui::Context, action: Action) {
        let a = self.active;
        // Disk usage and Settings windows have no file list.
        if action.needs_file_list() && !self.panes[a].tab().is_files() {
            return;
        }
        match action {
            Action::NextTab => self.strip.cycle(1),
            Action::PrevTab => self.strip.cycle(-1),
            Action::LastUsedTab => {
                if let Some(i) = self.last_used_tab.and_then(|uid| self.strip.index_of(uid)) {
                    self.strip.activate(i);
                }
            }
            Action::MoveTabLeft => self.strip.shift_active(-1),
            Action::MoveTabRight => self.strip.shift_active(1),
            Action::GoToTab(n) => self.strip.go_to(usize::from(n)),
            Action::GoToLastTab => self.strip.go_to(usize::MAX),
            Action::NewTab => self.new_tab(),
            Action::NewTerminalTab => self.new_terminal_here(None, None),
            Action::CloseTab => self.request_close_tabs(&[self.live]),
            Action::CloseWindow => self.request_close_pane(a),
            Action::ReopenTab => self.reopen_closed_tab(),
            Action::FocusWindow(n) => {
                if let Some(side) = self.layout.leaves().get(usize::from(n)).copied() {
                    self.active = side;
                }
            }
            Action::NextWindow => self.active = self.layout.next_after(self.active),
            Action::PrevWindow => self.active = self.layout.prev_before(self.active),
            Action::SplitRight | Action::SplitLeft | Action::SplitDown | Action::SplitUp => {
                let dir = match action {
                    Action::SplitRight => SplitDir::Right,
                    Action::SplitLeft => SplitDir::Left,
                    Action::SplitDown => SplitDir::Down,
                    _ => SplitDir::Up,
                };
                // The same kind of window: a terminal beside a terminal, files beside anything else.
                let terminal = self.panes[a].tab().is_terminal();
                let _ = self.split_pane(a, dir, terminal);
            }
            Action::SwapPanels => self.swap_with_other(),
            Action::View => self.run_cmd(Cmd::View),
            Action::OpenDefault => self.run_cmd(Cmd::OpenDefault),
            Action::NewFile => self.run_cmd(Cmd::NewFile),
            Action::NewFolder => self.run_cmd(Cmd::NewFolder),
            Action::Copy => self.run_cmd(Cmd::Copy),
            Action::Move => self.run_cmd(Cmd::Move),
            Action::Rename => self.run_cmd(Cmd::Rename),
            Action::BatchRename => self.open_batch_rename(),
            Action::Delete => self.run_cmd(Cmd::Delete),
            Action::DeletePermanently => {
                let saved = self.cfg.use_trash;
                self.cfg.use_trash = false;
                self.run_cmd(Cmd::Delete);
                self.cfg.use_trash = saved;
            }
            Action::Compress => self.run_cmd(Cmd::Pack),
            Action::ExtractOther => self.run_cmd(Cmd::ExtractOther),
            Action::Properties => self.show_properties(),
            Action::Checksum => self.run_cmd(Cmd::Checksum),
            Action::SelectAll => self.panes[a].tab_mut().mark_all(),
            Action::InvertSelection => self.panes[a].tab_mut().invert_marks(),
            Action::Back => self.panes[a].tab_mut().go_back(&lc!(self)),
            Action::Forward => self.panes[a].tab_mut().go_forward(&lc!(self)),
            Action::UpFolder => {
                if let Some(d) = self.panes[a].tab_mut().du.as_mut() {
                    d.go_up();
                } else if self.panes[a].tab().is_files() {
                    self.panes[a].tab_mut().go_up(&lc!(self));
                }
            }
            Action::Filter => {
                let q = self.panes[a].tab().search.as_ref().map(|s| s.query.clone()).unwrap_or_else(|| self.panes[a].tab().filter_text.clone());
                self.palette.open_with(&q);
            }
            Action::FocusPath => self.panes[a].tab_mut().focus_path = true,
            Action::Bookmark => self.run_cmd(Cmd::Bookmark),
            Action::Refresh => {
                if let Some(d) = self.panes[a].tab_mut().du.as_mut() {
                    d.rescan();
                } else {
                    self.run_cmd(Cmd::Refresh);
                }
            }
            Action::ToggleHidden => {
                self.cfg.show_hidden = !self.cfg.show_hidden;
                self.rebuild_views();
            }
            Action::Palette => self.palette.toggle(),
            Action::OpenTerminal => self.open_terminal(),
            Action::QuickView => self.toggle_quick_view(),
            Action::Sidebar => {
                self.cfg.show_sidebar = !self.cfg.show_sidebar;
                self.save_cfg();
            }
            Action::Transfers => self.open_transfers(),
            Action::Connect => self.open_connect_dialog(None),
            Action::Backup => self.backup.open = true,
            Action::Settings => self.open_settings(self.settings_page),
            Action::Help => self.show_help = true,
            Action::ZoomIn => self.set_ui_scale(ctx, self.cfg.ui_scale + 0.1),
            Action::ZoomOut => self.set_ui_scale(ctx, self.cfg.ui_scale - 0.1),
            Action::ZoomReset => self.set_ui_scale(ctx, 1.0),
        }
        // Whatever is now in front gets the keyboard.
        self.sync_tab();
        if matches!(
            action,
            Action::NextTab | Action::PrevTab | Action::LastUsedTab | Action::GoToTab(_) | Action::GoToLastTab | Action::FocusWindow(_) | Action::NextWindow | Action::PrevWindow
        ) {
            self.quick = None;
            self.focus_window();
        }
    }

    /// Zoom the whole interface (as Settings → UI scale does), and save it.
    fn set_ui_scale(&mut self, ctx: &egui::Context, scale: f32) {
        let scale = ((scale * 20.0).round() / 20.0).clamp(0.7, 2.0);
        if scale != self.cfg.ui_scale {
            self.cfg.ui_scale = scale;
            ctx.set_zoom_factor(scale);
            self.save_cfg();
        }
    }

    /// Typing in a file window opens a small search box in it with what was typed: it
    /// filters the folder as you type, Enter keeps the filter, Esc closes it and shows
    /// the whole folder again. Space marks files, so it does not start a search.
    fn type_to_search(&mut self, ctx: &egui::Context) -> bool {
        if !self.panes[self.active].tab().is_files() || self.type_search.is_some() {
            return false;
        }
        let typed: String = ctx.input(|i| {
            // Shortcut keys; Ctrl+Alt is AltGr, which types characters on Windows.
            let m = i.modifiers;
            if m.mac_cmd || ((m.ctrl || m.command) && !m.alt) {
                return String::new();
            }
            i.events.iter().filter_map(|e| if let egui::Event::Text(s) = e { Some(s.as_str()) } else { None }).collect()
        });
        if typed.is_empty() || typed.starts_with(char::is_whitespace) || typed.chars().any(char::is_control) {
            return false;
        }
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
        let tab = self.panes[self.active].tab();
        self.type_search = Some(TypeSearch { side: self.active, uid: tab.uid, loc: tab.loc.clone(), before: tab.filter_text.clone(), text: typed });
        self.apply_type_search();
        true
    }

    /// The search box's keys, taken before shortcuts: text and Backspace edit it, Enter
    /// keeps the filter, Esc puts the folder back. Arrows still move in the list.
    fn type_search_keys(&mut self, ctx: &egui::Context) {
        let Some(ts) = &self.type_search else { return };
        // Another window or tab came to the front: the search ends there.
        if ts.side != self.active || self.panes.get(ts.side).is_none_or(|p| p.tab().uid != ts.uid) {
            self.end_type_search(false);
            return;
        }
        // It went into another folder (a double click): the search was for the one before.
        if self.panes[ts.side].tab().loc != ts.loc {
            self.type_search = None;
            return;
        }
        let mut edits: Vec<TypeSearchKey> = vec![];
        ctx.input_mut(|i| {
            let m = i.modifiers;
            let shortcut = m.mac_cmd || ((m.ctrl || m.command) && !m.alt);
            i.events.retain(|e| match e {
                egui::Event::Text(t) if !shortcut => {
                    edits.push(TypeSearchKey::Text(t.chars().filter(|c| !c.is_control()).collect()));
                    false
                }
                egui::Event::Key { key, pressed, modifiers, .. } if !(modifiers.mac_cmd || modifiers.ctrl || modifiers.command) => match key {
                    Key::Backspace | Key::Enter | Key::Escape => {
                        if *pressed {
                            edits.push(match key {
                                Key::Backspace => TypeSearchKey::Back,
                                Key::Enter => TypeSearchKey::Keep,
                                _ => TypeSearchKey::Cancel,
                            });
                        }
                        false
                    }
                    _ => true,
                },
                _ => true,
            });
        });
        for e in edits {
            let Some(ts) = self.type_search.as_mut() else { break };
            match e {
                TypeSearchKey::Text(t) => ts.text.push_str(&t),
                TypeSearchKey::Back => {
                    ts.text.pop();
                    if ts.text.is_empty() {
                        self.end_type_search(false);
                        continue;
                    }
                }
                TypeSearchKey::Keep => {
                    self.end_type_search(true);
                    continue;
                }
                TypeSearchKey::Cancel => {
                    self.end_type_search(false);
                    continue;
                }
            }
            self.apply_type_search();
        }
    }

    /// Filter the window by what is typed.
    fn apply_type_search(&mut self) {
        let Some(ts) = &self.type_search else { return };
        let (side, text) = (ts.side, ts.text.trim().to_string());
        let lc = lc!(self);
        let tab = self.panes[side].tab_mut();
        if tab.filter_text != text {
            tab.set_filter(&text, &lc);
            // On the first match, so that Enter, Enter opens it.
            if let Some(first) = tab.view.iter().position(|&i| !tab.entries[i].is_parent) {
                tab.cursor = first;
                tab.ensure_visible = true;
            }
        }
    }

    /// Close the search box, keeping its filter or putting back the one from before.
    pub(super) fn end_type_search(&mut self, keep: bool) {
        let Some(ts) = self.type_search.take() else { return };
        if keep {
            return;
        }
        let lc = lc!(self);
        if let Some(tab) = self.panes.get_mut(ts.side).map(|p| p.tab_mut()).filter(|t| t.uid == ts.uid) {
            if tab.filter_text != ts.before {
                tab.set_filter(&ts.before, &lc);
            }
        }
    }

    /// The search box, in the lower right corner of its window.
    pub(super) fn type_search_ui(&mut self, ctx: &egui::Context) {
        let Some(ts) = &self.type_search else { return };
        let Some(&(_, rect)) = self.pane_rects.iter().find(|(s, _)| *s == ts.side) else { return };
        let th = self.theme;
        let tab = self.panes[ts.side].tab();
        let matches = tab.view.iter().filter(|&&i| !tab.entries[i].is_parent).count();
        let text = ts.text.clone();
        // As wide as the query needs (an SQL-like one can be long), within the window.
        let text_w = ctx.fonts(|f| f.layout_no_wrap(format!("{text}▏"), egui::FontId::proportional(13.5), th.text).size().x);
        let width = (text_w + 130.0).clamp(260.0, (rect.width() - 28.0).max(260.0)).min(rect.width() - 24.0).max(120.0);
        let pos = egui::pos2(rect.right() - width - 14.0, rect.bottom() - 28.0 - 44.0);
        egui::Area::new(egui::Id::new("type-search")).order(egui::Order::Foreground).fixed_pos(pos).interactable(false).show(ctx, |ui| {
            egui::Frame::new()
                .fill(th.panel)
                .stroke(egui::Stroke::new(1.0_f32, th.accent))
                .corner_radius(8.0)
                .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 10, spread: 0, color: egui::Color32::from_black_alpha(if th.dark { 90 } else { 40 }) })
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(width - 20.0);
                    let count = if matches == 1 { t("1 match") } else { format!("{} {}", crate::util::fmt_count(matches as u64), t("matches")) };
                    let count_w = ctx.fonts(|f| f.layout_no_wrap(count.clone(), egui::FontId::proportional(11.5), th.weak).size().x);
                    // The count only when the query fits beside it; the query is what matters.
                    let with_count = text_w + count_w + 40.0 <= width - 20.0;
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        icons::search(ui.painter(), r, th.weak);
                        // A caret that blinks after the text.
                        let blink = (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0;
                        let shown = if blink { format!("{text}▏") } else { format!("{text} ") };
                        let room = ui.available_width() - if with_count { count_w + 12.0 } else { 0.0 };
                        ui.allocate_ui(egui::vec2(room.max(20.0), 18.0), |ui| {
                            ui.add(egui::Label::new(RichText::new(shown).size(13.5).color(th.text)).truncate());
                        });
                        if with_count {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(&count).size(11.5).color(if matches == 0 { th.danger } else { th.weak }));
                            });
                        }
                    });
                });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn file_list_keys(&mut self, ctx: &egui::Context) {
        let keys: Vec<(Key, Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        // Tab must not move egui's keyboard focus.
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { key: Key::Tab, .. })));
        let has_list = self.panes[self.active].tab().is_files();
        for (key, m) in keys {
            let a = self.active;
            let shift = m.shift;
            let plain = !m.command && !m.alt && !m.ctrl;
            match key {
                Key::Tab => {
                    self.active = if shift { self.layout.prev_before(self.active) } else { self.layout.next_after(self.active) };
                    self.quick = None;
                    self.focus_window();
                }
                Key::Escape => {
                    // Esc that closes a menu (right-click, …) is that menu's alone.
                    if ctx.memory(|m| m.any_popup_open()) || self.panes[a].tab().menu_open {
                    } else if crate::pane::drag_payload(ctx).is_some() {
                        egui::DragAndDrop::clear_payload(ctx);
                    } else if self.viewer.is_some() {
                        self.viewer = None;
                    } else if self.panes[a].tab().marked.is_empty() && self.panes[a].tab().is_filtered() {
                        self.panes[a].tab_mut().clear_search(&lc!(self));
                    } else {
                        self.panes[a].tab_mut().marked.clear();
                    }
                }
                _ if !has_list => {}
                Key::ArrowUp if plain => self.panes[a].tab_mut().move_cursor(-1, shift),
                Key::ArrowDown if plain => self.panes[a].tab_mut().move_cursor(1, shift),
                Key::ArrowLeft if plain && !shift => {
                    self.panes[a].tab_mut().move_cursor(isize::MIN / 2, false);
                }
                Key::ArrowRight if plain && !shift => {
                    self.panes[a].tab_mut().move_cursor(isize::MAX / 2, false);
                }
                Key::PageUp if plain => {
                    let n = self.panes[a].tab().page_rows();
                    self.panes[a].tab_mut().move_cursor(-n, shift);
                }
                Key::PageDown if plain => {
                    let n = self.panes[a].tab().page_rows();
                    self.panes[a].tab_mut().move_cursor(n, shift);
                }
                Key::Home if plain => self.panes[a].tab_mut().move_cursor(isize::MIN / 2, shift),
                Key::End if plain => self.panes[a].tab_mut().move_cursor(isize::MAX / 2, shift),
                Key::Enter if plain => self.open_cursor(),
                Key::Space if plain => self.panes[a].tab_mut().toggle_mark_cursor(),
                Key::Insert if plain => {
                    self.panes[a].tab_mut().toggle_mark_cursor();
                    self.panes[a].tab_mut().move_cursor(1, false);
                }
                _ => {}
            }
        }
    }
}

enum TypeSearchKey {
    Text(String),
    Back,
    Keep,
    Cancel,
}
