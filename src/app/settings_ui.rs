//! The Settings tab: the page list, the pages and their building blocks.
use super::*;
use crate::style::fonts;
use egui::{pos2, vec2, Align2, FontFamily, FontId, Rect, Sense, Stroke};

/// Width of a settings row's label column.
const LABEL_W: f32 = 200.0;
const ROW_H: f32 = 36.0;

type PageIcon = fn(&egui::Painter, egui::Rect, egui::Color32);

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

/// A section title with an optional description, above its card.
pub(super) fn settings_section(ui: &mut egui::Ui, th: &Theme, title: &str, desc: &str) {
    ui.add_space(12.0);
    ui.label(RichText::new(title).font(bold(14.5)).color(th.strong()));
    if !desc.is_empty() {
        ui.label(RichText::new(desc).size(12.0).color(th.weak));
    }
    ui.add_space(2.0);
}

/// The frame of a settings card (`.begin()` it where a closure would be awkward).
pub(super) fn card_frame(th: &Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(th.bg.lerp_to_gamma(th.panel, 0.55))
        .stroke(Stroke::new(1.0_f32, th.border))
        .corner_radius(th.radius.saturating_add(5))
        .inner_margin(egui::Margin::symmetric(14, 8))
}

fn card<R>(ui: &mut egui::Ui, th: &Theme, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card_frame(th)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A line between two rows of a card.
fn row_line(ui: &mut egui::Ui, th: &Theme) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0_f32, th.border.gamma_multiply(0.6)));
}

/// A label with a small explanation under it, kept to `width`, in the middle of a row
/// of `height` (a vertical layout does not take part in the row's centring).
fn row_label(ui: &mut egui::Ui, th: &Theme, width: f32, height: f32, label: &str, hint: &str) {
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 1.0;
        let text_h = ui.text_style_height(&egui::TextStyle::Body) + if hint.is_empty() { 0.0 } else { 16.0 };
        ui.add_space(((height - text_h) / 2.0).max(0.0));
        ui.label(RichText::new(label).color(th.text));
        if !hint.is_empty() {
            ui.label(RichText::new(hint).size(11.5).color(th.weak));
        }
    });
}

fn keyboard_icon(p: &egui::Painter, r: Rect, c: egui::Color32) {
    let body = Rect::from_center_size(r.center(), vec2(r.width(), r.height() * 0.72));
    p.rect_stroke(body, 2.5, Stroke::new(1.3_f32, c), egui::StrokeKind::Inside);
    for row in 0..2 {
        for col in 0..4 {
            let at = pos2(body.left() + body.width() * (0.2 + col as f32 * 0.2), body.top() + body.height() * (0.34 + row as f32 * 0.3));
            p.circle_filled(at, 0.8, c);
        }
    }
}

/// A settings row: label on the left, its control beside it.
fn row<R>(ui: &mut egui::Ui, th: &Theme, label: &str, hint: &str, control: impl FnOnce(&mut egui::Ui) -> R) -> R {
    centered_row(ui, ROW_H, |ui| {
        row_label(ui, th, LABEL_W, ROW_H, label, hint);
        control(ui)
    })
}

/// A row of at least `height` whose items are centred vertically (`ui.horizontal`
/// centres them in one line of text, whatever height it is given later).
fn centered_row<R>(ui: &mut egui::Ui, height: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_height(height);
        add(ui)
    })
    .inner
}

/// An on/off switch. `label` names it for screen readers.
fn toggle(ui: &mut egui::Ui, th: &Theme, on: &mut bool, label: &str) -> egui::Response {
    let (r, mut resp) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, label));
    let k = ui.ctx().animate_bool_responsive(resp.id, *on);
    let off = th.weak.lerp_to_gamma(th.bg, 0.35);
    let mut track = off.lerp_to_gamma(th.accent, k);
    let mut knob = egui::Color32::WHITE;
    if !ui.is_enabled() {
        track = track.gamma_multiply(0.45);
        knob = knob.gamma_multiply(0.7);
    } else if resp.hovered() {
        track = track.lerp_to_gamma(th.text, 0.12);
    }
    ui.painter().rect_filled(r, 10.0, track);
    ui.painter().circle_filled(pos2(r.left() + 10.0 + k * (r.width() - 20.0), r.center().y), 7.5, knob);
    resp
}

/// A row that is one on/off setting: its label on the left, the switch at the right end.
pub(super) fn toggle_row(ui: &mut egui::Ui, th: &Theme, value: &mut bool, label: &str, hint: &str) -> bool {
    let mut changed = false;
    centered_row(ui, ROW_H, |ui| {
        let w = (ui.available_width() - 56.0).max(100.0);
        row_label(ui, th, w, ROW_H, label, hint);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = toggle(ui, th, value, label).changed();
        });
    });
    changed
}

/// A theme's picture: its panels and a selected file row.
fn theme_card(ui: &mut egui::Ui, th: &Theme, label: &str, width: f32, selected: bool, cur: &Theme) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 70.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, label));
    let p = ui.painter();
    p.rect_filled(rect, 9.0, th.panel);
    let content = egui::Rect::from_min_max(rect.min + egui::vec2(8.0, 8.0), egui::pos2(rect.right() - 8.0, rect.bottom() - 24.0));
    p.rect_filled(content, 5.0, th.bg);
    let row = egui::Rect::from_min_size(content.min + egui::vec2(4.0, 5.0), egui::vec2(content.width() - 8.0, 8.0));
    p.rect_filled(row, 3.0, th.selection);
    p.rect_filled(egui::Rect::from_min_size(row.min + egui::vec2(3.0, 2.0), egui::vec2(6.0, 4.0)), 1.0, th.dir);
    p.rect_filled(egui::Rect::from_min_size(row.min + egui::vec2(12.0, 2.5), egui::vec2(34.0, 3.0)), 1.5, th.text);
    let row2 = row.translate(egui::vec2(0.0, 13.0));
    p.rect_filled(egui::Rect::from_min_size(row2.min + egui::vec2(3.0, 2.0), egui::vec2(6.0, 4.0)), 1.0, th.archive);
    p.rect_filled(egui::Rect::from_min_size(row2.min + egui::vec2(12.0, 2.5), egui::vec2(26.0, 3.0)), 1.5, th.weak);
    p.circle_filled(egui::pos2(content.right() - 8.0, content.bottom() - 7.0), 4.0, th.accent);
    p.with_clip_rect(rect.shrink(4.0)).text(
        egui::pos2(rect.left() + 9.0, rect.bottom() - 12.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.5),
        th.text,
    );
    card_outline(p, rect, 9.0, selected, resp.hovered(), cur);
    resp.on_hover_text(label)
}

/// Card width for a gallery with as many columns as fit, filling `width` exactly.
fn gallery_card_width(width: f32, min: f32, gap: f32) -> f32 {
    let cols = ((width + gap) / (min + gap)).floor().max(1.0);
    ((width + gap) / cols - gap).floor()
}

fn card_outline(p: &egui::Painter, rect: Rect, radius: f32, selected: bool, hovered: bool, cur: &Theme) {
    let stroke = if selected {
        Stroke::new(2.0_f32, cur.accent)
    } else if hovered {
        Stroke::new(1.0_f32, cur.weak)
    } else {
        Stroke::new(1.0_f32, cur.border)
    };
    p.rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
}

/// A terminal colour scheme's picture: a prompt and its colours, on its background.
fn term_theme_card(ui: &mut egui::Ui, scheme: &Theme, label: &str, width: f32, selected: bool, cur: &Theme) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 76.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, label));
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 8.0, scheme.bg);
    let term = FontId::new(11.0, FontFamily::Name("term".into()));
    let ansi = |i: usize| scheme.ansi.map_or(scheme.accent, |a| egui::Color32::from_rgb((a[i] >> 16) as u8, (a[i] >> 8) as u8, a[i] as u8));
    let top = rect.min + vec2(9.0, 8.0);
    let prompt = p.text(top, Align2::LEFT_TOP, "$ ", term.clone(), ansi(2));
    p.text(pos2(prompt.right(), top.y), Align2::LEFT_TOP, "ls", term.clone(), scheme.text);
    p.text(top + vec2(0.0, 14.0), Align2::LEFT_TOP, "src", term, ansi(4));
    for i in 0..6 {
        let c = pos2(rect.left() + 13.0 + i as f32 * 12.0, rect.top() + 44.0);
        p.circle_filled(c, 4.0, ansi(i + 1));
    }
    p.text(pos2(rect.left() + 9.0, rect.bottom() - 13.0), Align2::LEFT_CENTER, label, FontId::proportional(11.5), scheme.text);
    card_outline(&p, rect.shrink(0.5), 8.0, selected, resp.hovered(), cur);
    resp.on_hover_text(label)
}

/// A dropdown of the fonts that ship with the app and the installed ones, with a search
/// field. Returns the setting value of the font that was clicked.
fn font_picker(ui: &mut egui::Ui, th: &Theme, id_salt: &str, current: &str, builtin: &[(&str, &str)], mono_only: bool, search: &mut String) -> Option<String> {
    enum Item<'a> {
        Header(String),
        Font(&'a str, &'a str),
    }
    const W: f32 = 300.0;
    const ITEM_H: f32 = 28.0;
    let id = egui::Id::new(("font-picker", id_salt));
    let shown = fonts::label(current, builtin).to_string();
    let (r, btn) = ui.allocate_exact_size(vec2(W, 28.0), Sense::click());
    btn.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &shown));
    let visuals = ui.style().interact(&btn);
    ui.painter().rect_filled(r, th.radius, visuals.weak_bg_fill);
    ui.painter().with_clip_rect(r.shrink2(vec2(10.0, 0.0))).text(
        pos2(r.left() + 10.0, r.center().y),
        Align2::LEFT_CENTER,
        &shown,
        FontId::proportional(14.0),
        th.text,
    );
    icons::chevron(ui.painter(), Rect::from_center_size(pos2(r.right() - 16.0, r.center().y), vec2(10.0, 10.0)), 2, th.weak);
    if btn.clicked() {
        search.clear();
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    let mut picked = None;
    let installed = fonts::families(ui.ctx());
    egui::popup::popup_below_widget(ui, id, &btn, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_width(W);
        crate::ui::pane::menu_style(ui);
        let field = ui.add(
            egui::TextEdit::singleline(search).desired_width(W - 12.0).background_color(th.bg).hint_text(RichText::new(t("Search fonts…")).color(th.weak)),
        );
        field.request_focus();
        let needle = search.to_lowercase();
        let matches = |name: &str| name.to_lowercase().contains(&needle);
        let mut items = vec![Item::Header(t("Included with Matrix Finder"))];
        items.extend(builtin.iter().filter(|(_, n)| matches(n)).map(|(k, n)| Item::Font(k, n)));
        items.push(Item::Header(t("Installed on this computer")));
        for f in installed.unwrap_or_default().iter().filter(|f| (!mono_only || f.mono) && matches(&f.name)) {
            items.push(Item::Font(&f.name, &f.name));
        }
        egui::ScrollArea::vertical().id_salt(("font-list", id_salt)).max_height(280.0).show_rows(ui, ITEM_H, items.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for item in &items[range] {
                let (row, resp) = ui.allocate_exact_size(
                    vec2(W - 12.0, ITEM_H),
                    match item {
                        Item::Font(..) => Sense::click(),
                        Item::Header(_) => Sense::hover(),
                    },
                );
                match item {
                    Item::Header(text) => {
                        ui.painter().text(pos2(row.left() + 8.0, row.bottom() - 9.0), Align2::LEFT_CENTER, text, FontId::proportional(11.5), th.weak);
                    }
                    Item::Font(value, name) => {
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, *name));
                        crate::ui::pane::menu_row_fill(ui, row, th, *value == current, resp.hovered());
                        ui.painter().with_clip_rect(row).text(
                            pos2(row.left() + 8.0, row.center().y),
                            Align2::LEFT_CENTER,
                            name,
                            FontId::proportional(14.0),
                            th.text,
                        );
                        if resp.clicked() {
                            picked = Some(value.to_string());
                        }
                    }
                }
            }
        });
        if installed.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(t("Looking for installed fonts…")).size(12.0).color(th.weak));
            });
        }
    });
    if picked.is_some() {
        ui.memory_mut(|m| m.close_popup());
    }
    picked
}

/// Let the user pick an application; returns it ready to store as a program.
pub(super) fn pick_application() -> Option<String> {
    let mut dialog = rfd::FileDialog::new();
    if cfg!(target_os = "macos") {
        dialog = dialog.set_directory("/Applications").add_filter(t("Applications"), &["app"]);
    } else if cfg!(windows) {
        dialog = dialog.set_directory("C:\\Program Files").add_filter(t("Applications"), &["exe", "cmd", "bat"]);
    } else {
        dialog = dialog.set_directory("/usr/bin");
    }
    dialog.pick_file().map(|f| format!("\"{}\"", f.to_string_lossy()))
}

impl App {
    /// The window's size at start: width × height, with the current size and the default
    /// a click away. Kept in range here and when the settings load.
    fn window_size_row(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        use crate::config::{WINDOW_DEFAULT, WINDOW_MAX, WINDOW_MIN};
        let mut changed = false;
        ui.add_space(4.0);
        ui.label(RichText::new(t("Window size at start")).color(th.text));
        ui.label(RichText::new(t("Width × height in pixels, at least 720 × 420. Takes effect the next time Matrix Finder starts.")).size(11.5).color(th.weak));
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let w = egui::DragValue::new(&mut self.cfg.window_width).range(WINDOW_MIN[0]..=WINDOW_MAX[0]).speed(4.0).max_decimals(0);
            changed |= ui.add(w).on_hover_text(t("Width")).changed();
            ui.label("×");
            let h = egui::DragValue::new(&mut self.cfg.window_height).range(WINDOW_MIN[1]..=WINDOW_MAX[1]).speed(4.0).max_decimals(0);
            changed |= ui.add(h).on_hover_text(t("Height")).changed();
            ui.add_space(8.0);
            // The window as it is now, in the same pixels as the start size (before the UI scale).
            let now = ui.ctx().screen_rect().size() * ui.ctx().zoom_factor();
            // Maximized, the window is the screen's size: no size to start a window with.
            let filled = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false));
            let b =
                ui.add_enabled(!filled, egui::Button::new(t("Use current size"))).on_disabled_hover_text(t("Not while the window is maximized or full screen"));
            if b.clicked() {
                self.cfg.window_width = now.x.round().clamp(WINDOW_MIN[0], WINDOW_MAX[0]);
                self.cfg.window_height = now.y.round().clamp(WINDOW_MIN[1], WINDOW_MAX[1]);
                changed = true;
            }
            if ui.button(format!("{} ({} × {})", t("Default"), WINDOW_DEFAULT[0], WINDOW_DEFAULT[1])).clicked() {
                (self.cfg.window_width, self.cfg.window_height) = (WINDOW_DEFAULT[0], WINDOW_DEFAULT[1]);
                changed = true;
            }
        });
        ui.add_space(4.0);
        changed
    }

    /// Settings → Default apps: one row per file type. An empty field means the type
    /// opens with the system's default app, which the field names as its hint.
    fn default_app_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        use crate::system::defaultapps as apps;
        const ICON_W: f32 = 20.0;
        const TYPE_W: f32 = 70.0;
        const BROWSE_W: f32 = 84.0;
        const RESET_W: f32 = 28.0;
        let mut changed = false;
        ui.label(RichText::new(t("Files open with the app your operating system has chosen for them. Choose another one here for any type; your system's own settings stay as they are.")).size(12.0).color(th.weak));
        ui.add_space(4.0);
        // The common types, then the others in use, each once.
        let mut types: Vec<String> = apps::COMMON.iter().map(|e| e.to_string()).collect();
        let mut more: Vec<String> = self.cfg.default_apps.keys().chain(&self.default_app_types).filter(|e| !types.contains(e)).cloned().collect();
        more.sort();
        more.dedup();
        types.extend(more);
        card(ui, th, |ui| {
            // One width for all rows, so they line up and never widen the page. Four gaps,
            // and a text field draws its margins (and frame) outside the width it is given.
            let field_w = (ui.available_width() - ICON_W - TYPE_W - BROWSE_W - RESET_W - 4.0 * ui.spacing().item_spacing.x - 14.0).max(120.0);
            for (i, ext) in types.iter().enumerate() {
                if i > 0 {
                    row_line(ui, th);
                }
                centered_row(ui, 34.0, |ui| {
                    let (icon, _) = ui.allocate_exact_size(vec2(ICON_W, 20.0), Sense::hover());
                    icons::file_icon(ui.painter(), icon, &format!("file.{ext}"), false, ext, th);
                    let (label, _) = ui.allocate_exact_size(vec2(TYPE_W, 20.0), Sense::hover());
                    ui.painter().with_clip_rect(label).text(
                        pos2(label.left(), label.center().y),
                        Align2::LEFT_CENTER,
                        format!(".{ext}"),
                        FontId::proportional(14.0),
                        th.text,
                    );
                    let hint = match apps::system_app(ui.ctx(), ext) {
                        Some(Some(app)) => format!("{app} ({})", t("system default")),
                        Some(None) => t("System default"),
                        None => "…".to_string(),
                    };
                    let mut program = self.cfg.default_apps.get(ext).cloned().unwrap_or_default();
                    let missing = apps::program_missing(&program);
                    let mut field = egui::TextEdit::singleline(&mut program)
                        .id_salt(("default-app", ext))
                        .background_color(th.bg)
                        .desired_width(field_w)
                        .hint_text(RichText::new(hint).color(th.weak));
                    if missing {
                        field = field.text_color(th.danger);
                    }
                    let response = ui.add(field);
                    let response = if missing { response.on_hover_text(t("This program was not found")) } else { response };
                    if response.changed() {
                        if program.trim().is_empty() {
                            self.cfg.default_apps.remove(ext);
                        } else {
                            self.cfg.default_apps.insert(ext.clone(), program);
                        }
                        changed = true;
                    }
                    if ui.add(egui::Button::new(t("Browse…")).min_size(vec2(BROWSE_W, 26.0))).clicked() {
                        if let Some(app) = pick_application() {
                            self.cfg.default_apps.insert(ext.clone(), app);
                            changed = true;
                        }
                    }
                    if self.cfg.default_apps.contains_key(ext) {
                        let reset = egui::Button::new("↺").min_size(vec2(RESET_W, 26.0));
                        if ui.add(reset).on_hover_text(t("Use system default")).clicked() {
                            self.cfg.default_apps.remove(ext);
                            changed = true;
                        }
                    }
                });
            }
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.default_app_new).desired_width(150.0).hint_text(RichText::new(t("File type, e.g. mkv")).color(th.weak)),
            );
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let new_type = apps::normalize_ext(&self.default_app_new);
            if (ui.add_enabled(!new_type.is_empty(), egui::Button::new(format!("+ {}", t("Add file type")))).clicked() || enter) && !new_type.is_empty() {
                self.default_app_types.push(new_type);
                self.default_app_new.clear();
            }
            let count = self.cfg.default_apps.len();
            if self.confirm_reset_apps && count > 0 {
                ui.label(format!("{} ({count})", t("Open all file types with the system's default apps again?")));
                if ui.button(RichText::new(t("Reset")).color(th.danger)).clicked() {
                    self.cfg.default_apps.clear();
                    self.confirm_reset_apps = false;
                    changed = true;
                }
                if ui.button(t("Cancel")).clicked() {
                    self.confirm_reset_apps = false;
                }
            } else if ui.add_enabled(count > 0, egui::Button::new(t("Reset all to system defaults"))).clicked() {
                self.confirm_reset_apps = true;
            }
        });
        changed
    }

    /// Settings → Keyboard shortcuts: every action with its keys, which can be changed one by one.
    fn shortcut_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        use crate::ui::shortcuts::{self as sc, Action};
        let mut changed = false;
        let keys = sc::active();
        let os = keys.os();
        let safe = if os == sc::Os::Mac { "⌘" } else { "Ctrl+Shift" };
        ui.label(
            RichText::new(format!(
                "{} ({safe}). {}",
                t("Defaults for this system. While a terminal has the keyboard, only combinations terminal programs never see are used"),
                t("All other keys reach Vim, Emacs and the like.")
            ))
            .size(12.0)
            .color(th.weak),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.shortcut_search).desired_width(220.0).hint_text(RichText::new(t("Search shortcuts…")).color(th.weak)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let count = self.cfg.shortcuts.len();
                if self.confirm_reset_shortcuts && count > 0 {
                    if ui.button(t("Cancel")).clicked() {
                        self.confirm_reset_shortcuts = false;
                    }
                    if ui.button(RichText::new(t("Reset")).color(th.danger)).clicked() {
                        self.cfg.shortcuts.clear();
                        self.confirm_reset_shortcuts = false;
                        changed = true;
                    }
                    ui.label(format!("{} {} ({count})?", t("Back to the defaults of"), os.name()));
                } else if ui.add_enabled(count > 0, egui::Button::new(format!("{} {}", t("Reset all to the defaults of"), os.name()))).clicked() {
                    self.confirm_reset_shortcuts = true;
                }
            });
        });
        if let Some(note) = &self.shortcut_note {
            ui.label(RichText::new(note).size(12.5).color(th.marked));
        }
        let needle = self.shortcut_search.trim().to_lowercase();
        let mut edit: Option<(Action, Vec<sc::Chord>)> = None;
        let mut capture = None;
        for group in sc::GROUPS {
            let matching: Vec<Action> = Action::all()
                .filter(|a| a.group() == group)
                .filter(|a| {
                    needle.is_empty()
                        || a.label().to_lowercase().contains(&needle)
                        || keys.keys(*a).iter().any(|c| c.display(os).to_lowercase().contains(&needle))
                })
                .collect();
            if matching.is_empty() {
                continue;
            }
            settings_section(ui, th, &group.label(), "");
            card(ui, th, |ui| {
                for (n, action) in matching.into_iter().enumerate() {
                    if n > 0 {
                        row_line(ui, th);
                    }
                    let waiting = self.shortcut_capture.map(|(a, slot)| (a == action).then_some(slot)).unwrap_or(None);
                    centered_row(ui, 30.0, |ui| {
                        let (label, _) = ui.allocate_exact_size(vec2(LABEL_W + 20.0, 20.0), Sense::hover());
                        ui.painter().with_clip_rect(label).text(
                            pos2(label.left(), label.center().y),
                            Align2::LEFT_CENTER,
                            action.label(),
                            FontId::proportional(14.0),
                            th.text,
                        );
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let current = keys.keys(action).to_vec();
                        for (i, chord) in current.iter().enumerate() {
                            if waiting == Some(Some(i)) {
                                ui.add(egui::Button::new(RichText::new(t("Press keys… (Esc cancels)")).size(12.5).color(th.on_accent())).fill(th.accent));
                                continue;
                            }
                            let only_outside = action.works_in_terminals() && !chord.leaves_terminals_alone();
                            let text = RichText::new(chord.display(os)).size(12.5).monospace().color(if only_outside { th.weak } else { th.text });
                            let chip = ui.add(egui::Button::new(text)).on_hover_text(if only_outside {
                                t("Not while a terminal has the keyboard: there this key goes to the program. Click to change.")
                            } else {
                                t("Click to change")
                            });
                            if chip.clicked() {
                                capture = Some((action, Some(i)));
                            }
                            if icons::icon_button(ui, 18.0, true, th, |p, r, c| icons::mi_trash(p, r, c)).on_hover_text(t("Remove")).clicked() {
                                let mut keys = current.clone();
                                keys.remove(i);
                                edit = Some((action, keys));
                            }
                        }
                        if waiting == Some(None) {
                            ui.add(egui::Button::new(RichText::new(t("Press keys… (Esc cancels)")).size(12.5).color(th.on_accent())).fill(th.accent));
                        } else if ui.add(egui::Button::new("+").small()).on_hover_text(t("Add another key")).clicked() {
                            capture = Some((action, None));
                        }
                        let defaults = sc::defaults(action, os);
                        if current != defaults
                            && ui
                                .add(egui::Button::new("↺").small())
                                .on_hover_text(format!(
                                    "{} ({})",
                                    t("Back to the default"),
                                    defaults.iter().map(|c| c.display(os)).collect::<Vec<_>>().join(" / ")
                                ))
                                .clicked()
                        {
                            edit = Some((action, defaults));
                        }
                    });
                }
            });
        }
        if let Some(c) = capture {
            self.shortcut_capture = Some(c);
            self.shortcut_note = None;
        }
        if let Some((action, new_keys)) = edit {
            sc::store(&mut self.cfg.shortcuts, os, action, &new_keys);
            self.shortcut_note = None;
            changed = true;
        }
        if changed {
            sc::install(&self.cfg.shortcuts);
        }
        changed
    }

    /// The Settings window's content (in its tab): page list on the left, the page on the right.
    pub(super) fn settings_view(&mut self, ui: &mut egui::Ui, side: usize) {
        let mut changed = false;
        let th = self.theme;
        let ctx = ui.ctx().clone();
        let pages: [(SettingsPage, String, PageIcon); 10] = [
            (SettingsPage::Appearance, t("Appearance"), icons::eye),
            (SettingsPage::Fonts, t("Fonts"), |p, r, c| {
                p.text(r.center(), Align2::CENTER_CENTER, "A", bold(r.height() + 1.0), c);
            }),
            (SettingsPage::Language, t("Language"), icons::globe),
            (SettingsPage::Files, t("Files & layout"), |p, r, c| icons::folder(p, r, c)),
            (SettingsPage::Bookmarks, t("Bookmarks"), |p, r, c| icons::star(p, r, c, false)),
            (SettingsPage::QuickFilters, t("Quick filters"), |p, r, c| icons::funnel(p, r, c, false)),
            (SettingsPage::DefaultApps, t("Default apps"), icons::mi_external),
            (SettingsPage::Shortcuts, t("Keyboard shortcuts"), keyboard_icon),
            (SettingsPage::Terminal, t("Terminal"), terminal_icon),
            (SettingsPage::Backup, t("Backup & sync"), icons::shield),
        ];
        // A click anywhere in it focuses the window.
        if ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.any_pressed()) {
            self.active = side;
        }
        // What the file lists depend on: they are rebuilt only when it changes.
        let list_before = (self.cfg.show_hidden, self.cfg.dirs_first, self.cfg.quick_filters.clone(), self.cfg.default_view);
        let h = ui.available_height();
        ui.horizontal_top(|ui| {
            // ---- navigation
            ui.vertical(|ui| {
                ui.set_width(190.0);
                ui.set_min_height(h);
                ui.add_space(6.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                for (page, label, icon) in &pages {
                    let sel = self.settings_page == *page;
                    let (r, resp) = ui.allocate_exact_size(egui::vec2(190.0, 32.0), egui::Sense::click());
                    if sel {
                        ui.painter().rect_filled(r, 7.0, th.selection);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, 7.0, th.hover());
                    }
                    let col = if sel { th.accent } else { th.weak };
                    icon(ui.painter(), egui::Rect::from_center_size(egui::pos2(r.left() + 18.0, r.center().y), egui::vec2(15.0, 15.0)), col);
                    ui.painter().text(
                        egui::pos2(r.left() + 36.0, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::proportional(14.0),
                        if sel { th.strong() } else { th.text },
                    );
                    if resp.clicked() {
                        self.settings_page = *page;
                        self.shortcut_capture = None;
                        self.shortcut_note = None;
                    }
                }
                ui.add_space(16.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new(format!("↺  {}", t("Reset to defaults…"))).color(th.danger))
                            .frame(false)
                            .min_size(egui::vec2(190.0, 28.0)),
                    )
                    .on_hover_text(t("Restore the default settings. Bookmarks, connections, backup, terminal profiles and default apps are kept."))
                    .clicked()
                {
                    self.confirm_reset = true;
                }
            });
            let (sep, _) = ui.allocate_exact_size(egui::vec2(1.0, h), egui::Sense::hover());
            ui.painter().rect_filled(sep, 0.0, th.border);
            ui.add_space(16.0);
            // ---- page (kept to a readable width in a wide window)
            ui.vertical(|ui| {
                let w = ui.available_width().min(860.0);
                ui.set_max_width(w);
                egui::ScrollArea::vertical().id_salt(("settings-page", self.settings_page as u8)).auto_shrink([false, false]).max_height(h).show(ui, |ui| {
                    ui.set_max_width(w - 12.0);
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    if let Some((_, title, _)) = pages.iter().find(|(p, ..)| *p == self.settings_page) {
                        ui.label(RichText::new(title).font(bold(21.0)).color(th.strong()));
                    }
                    changed |= self.settings_page_ui(ui, th);
                    ui.add_space(16.0);
                });
            });
        });
        // The UI scale applies once the slider is let go: zooming while it is dragged
        // moves the slider under the pointer, and the value jumps.
        let scale = self.cfg.ui_scale.clamp(0.6, 2.5);
        if (ctx.zoom_factor() - scale).abs() > 1e-4 && !ctx.input(|i| i.pointer.any_down()) {
            ctx.set_zoom_factor(scale);
        }
        if changed {
            self.apply_fonts(&ctx);
            self.sync_terminal_profiles();
            if (self.cfg.show_hidden, self.cfg.dirs_first, &self.cfg.quick_filters, self.cfg.default_view)
                != (list_before.0, list_before.1, &list_before.2, list_before.3)
            {
                self.rebuild_views();
            }
            self.save_cfg_soon(&ctx);
        }
    }

    /// Save the settings after a change: a click saves at once; a drag or typing waits
    /// until the pointer is let go or the typing pauses (see `save_cfg_pending`).
    pub(super) fn save_cfg_soon(&mut self, ctx: &egui::Context) {
        let keyed = typing(ctx) && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_) | egui::Event::Key { .. } | egui::Event::Paste(_))));
        if ctx.input(|i| i.pointer.any_down()) || keyed {
            self.cfg_dirty = Some(Instant::now());
            ctx.request_repaint_after(Duration::from_millis(500));
        } else {
            self.cfg_dirty = None;
            self.save_cfg();
        }
    }

    /// Save settings changed by a drag or typing, once that is over.
    pub(super) fn save_cfg_pending(&mut self, ctx: &egui::Context) {
        let Some(at) = self.cfg_dirty else { return };
        if ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        if typing(ctx) && at.elapsed() < Duration::from_millis(500) {
            ctx.request_repaint_after(Duration::from_millis(500) - at.elapsed());
            return;
        }
        self.cfg_dirty = None;
        self.save_cfg();
    }

    /// Rebuild the fonts if their settings changed. A font that turns out to be
    /// unusable is reported and goes back to the default.
    pub(super) fn apply_fonts(&mut self, ctx: &egui::Context) {
        let prefs = fonts::FontPrefs::of(&self.cfg);
        if prefs == self.fonts_applied {
            return;
        }
        let failed = fonts::apply(ctx, &prefs);
        for name in &failed {
            self.toast(format!("{}: {name}", t("This font cannot be used")), true);
            if self.cfg.ui_font == *name {
                self.cfg.ui_font.clear();
            }
            if self.cfg.term_font == *name {
                self.cfg.term_font.clear();
            }
        }
        self.fonts_applied = fonts::FontPrefs::of(&self.cfg);
    }

    /// Open terminals follow the colour theme of the profile they were started from.
    fn sync_terminal_profiles(&mut self) {
        let profiles = self.cfg.terminal_profiles.clone();
        for pane in all_panes!(self) {
            let Some(term) = pane.tab_mut().term.as_mut() else { continue };
            if let Some(p) = profiles.iter().find(|p| term.profile.as_deref() == Some(&p.name)) {
                term.theme = (!p.theme.is_empty()).then(|| theme::term_by_name(&p.theme));
            }
        }
    }

    fn settings_page_ui(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        match self.settings_page {
            SettingsPage::Appearance => {
                settings_section(ui, th, &t("Theme"), &t("Pick a theme. Changes apply instantly."));
                card(ui, th, |ui| {
                    ui.add_space(4.0);
                    let card_w = gallery_card_width(ui.available_width(), 118.0, 8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        let native = theme::native(ui.ctx().system_theme() == Some(egui::Theme::Dark));
                        let sys_label = format!("{} · {}", t("System"), native.name);
                        if theme_card(ui, native, &sys_label, card_w, self.cfg.theme == theme::SYSTEM, self.theme)
                            .on_hover_text(t("Follow the operating system: native look, light or dark automatically"))
                            .clicked()
                        {
                            self.set_theme(ui.ctx(), theme::SYSTEM);
                        }
                        for theme in theme::THEMES {
                            if theme_card(ui, theme, theme.name, card_w, self.cfg.theme == theme.name, self.theme).clicked() {
                                self.set_theme(ui.ctx(), theme.name);
                            }
                        }
                    });
                    ui.add_space(4.0);
                });
                settings_section(ui, th, &t("Size"), "");
                card(ui, th, |ui| {
                    row(ui, th, &t("UI scale"), &t("Zooms everything, not just the text."), |ui| {
                        ui.spacing_mut().slider_width = 200.0;
                        changed |= ui.add(egui::Slider::new(&mut self.cfg.ui_scale, 0.7..=2.0).step_by(0.05)).changed();
                    });
                });
                settings_section(ui, th, &t("Window"), "");
                card(ui, th, |ui| {
                    changed |= toggle_row(
                        ui,
                        th,
                        &mut self.cfg.integrated_titlebar,
                        &t("Integrated title bar with native window buttons"),
                        &t("Takes effect after restarting Matrix Finder"),
                    );
                    row_line(ui, th);
                    if cfg!(target_os = "linux") {
                        changed |= toggle_row(
                            ui,
                            th,
                            &mut self.cfg.round_corners,
                            &t("Round window corners"),
                            &t("GNOME and KDE on Wayland, with the integrated title bar. Takes effect after restarting Matrix Finder"),
                        );
                        row_line(ui, th);
                    }
                    changed |= self.window_size_row(ui, th);
                });
                settings_section(ui, th, &t("Icons"), &t("File and folder icons. On Linux, installed desktop icon themes are used directly."));
                card(ui, th, |ui| changed |= self.icon_settings(ui));
            }
            SettingsPage::Fonts => changed |= self.font_settings(ui, th),
            SettingsPage::Language => {
                settings_section(ui, th, &t("Language"), &t("Add languages by dropping JSON files into the language folder."));
                card(ui, th, |ui| {
                    row(ui, th, &t("Language"), "", |ui| {
                        let cur = self.languages.iter().find(|l| l.0 == self.cfg.language).map(|l| l.1.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("set-lang").width(220.0).selected_text(cur).show_ui(ui, |ui| {
                            for (code, name) in self.languages.clone() {
                                if ui.selectable_label(self.cfg.language == code, name).clicked() {
                                    self.set_language(&code);
                                }
                            }
                        });
                    });
                });
                settings_section(ui, th, &t("Translations"), "");
                card(ui, th, |ui| {
                    ui.add_space(2.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(t("Reload")).on_hover_text(t("Rescan the language folder")).clicked() {
                            self.languages = i18n::available();
                        }
                        if ui.button(t("Open language folder")).clicked() {
                            let d = crate::config::lang_dir();
                            let _ = std::fs::create_dir_all(&d);
                            let _ = terminal::open_default(&d);
                        }
                        if ui.button(t("Export translation template…")).clicked() {
                            let d = crate::config::lang_dir();
                            let _ = std::fs::create_dir_all(&d);
                            if let Some(p) = rfd::FileDialog::new().set_directory(&d).set_file_name("xx.json").save_file() {
                                match i18n::export_template(&p) {
                                    Ok(()) => self.toast(format!("{} {}", t("Exported to"), p.display()), false),
                                    Err(e) => self.toast(e, true),
                                }
                            }
                        }
                    });
                    ui.add_space(2.0);
                });
            }
            SettingsPage::Files => {
                settings_section(ui, th, &t("Panels"), "");
                card(ui, th, |ui| {
                    changed |=
                        toggle_row(ui, th, &mut self.cfg.start_in_home, &t("Open the home folder at start"), &t("Off: reopen the tabs of the last session"));
                    row_line(ui, th);
                    changed |= toggle_row(ui, th, &mut self.cfg.show_hidden, &t("Show hidden files"), "");
                    row_line(ui, th);
                    changed |= toggle_row(ui, th, &mut self.cfg.dirs_first, &t("Folders first"), "");
                    row_line(ui, th);
                    ui.add_space(4.0);
                    ui.label(RichText::new(t("Default view")).color(th.text));
                    ui.label(RichText::new(t("Each window can switch with the View options button in the toolbar.")).size(11.5).color(th.weak));
                    ui.add_space(2.0);
                    for (mode, label) in [(crate::config::ViewMode::List, t("List")), (crate::config::ViewMode::Icons, t("Icons"))] {
                        changed |= ui.radio_value(&mut self.cfg.default_view, mode, label).changed();
                    }
                    ui.add_space(4.0);
                    row_line(ui, th);
                    changed |= toggle_row(
                        ui,
                        th,
                        &mut self.cfg.thumbnails,
                        &t("Show thumbnails in the icon view"),
                        &t("Pictures, camera RAW and videos (a video's frame needs FFmpeg, or Quick Look on macOS). Made in the background and kept in memory only; on network drives each picture shown is read over the network."),
                    );
                    row_line(ui, th);
                    changed |= toggle_row(
                        ui,
                        th,
                        &mut self.cfg.auto_refresh,
                        &t("Refresh folders automatically"),
                        &t("Files added, removed or renamed in the open folders show up by themselves, also when another app made them."),
                    );
                    ui.add_enabled_ui(self.cfg.auto_refresh, |ui| {
                        row_line(ui, th);
                        row(ui, th, &t("Check every"), &t("Each check reads only the folder's date."), |ui| {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut self.cfg.auto_refresh_secs)
                                        .range(crate::config::AUTO_REFRESH_SECS)
                                        .suffix(format!(" {}", t("s"))),
                                )
                                .changed();
                        });
                        ui.add_space(2.0);
                        let refresh = crate::ui::shortcuts::hint(crate::ui::shortcuts::Action::Refresh);
                        for limit in [
                            format!(
                                "{} ({refresh}).",
                                t("A file that is still being written, such as a download or a log, keeps its old size and date in the list until you refresh")
                            ),
                            t("Folders on servers (SFTP, FTP) and inside archives are not checked."),
                            t("On network drives, changes made from other computers can show up late or only after a refresh."),
                            t("A folder that takes long to read (very large, or on a slow network drive) is not refreshed by itself."),
                        ] {
                            ui.label(RichText::new(format!("•  {limit}")).size(11.5).color(th.weak));
                        }
                        ui.add_space(4.0);
                    });
                    row_line(ui, th);
                    changed |= toggle_row(
                        ui,
                        th,
                        &mut self.cfg.show_disk_usage,
                        &t("Show disk space"),
                        &t("Usage bars in the sidebar, free space in the panels and the Disk usage button"),
                    );
                });
                settings_section(ui, th, &t("Git"), "");
                card(ui, th, |ui| {
                    changed |= toggle_row(
                        ui,
                        th,
                        &mut self.cfg.git_branches,
                        &t("Show the branch of git folders"),
                        &t("After the folder's name: folder (git) [branch]. Read from the folder, git does not run."),
                    );
                    row_line(ui, th);
                    ui.add_enabled_ui(self.cfg.git_branches, |ui| {
                        changed |= toggle_row(ui, th, &mut self.cfg.git_label, &t("Show the (git) label before the branch"), &t("On: folder (git) [branch] · Off: folder [branch]"));
                        row_line(ui, th);
                        ui.add_space(4.0);
                        ui.label(RichText::new(t("Style")).color(th.text));
                        ui.add_space(2.0);
                        for (style, label) in [(crate::config::GitStyle::Text, t("Grey text: (git) [branch]")), (crate::config::GitStyle::Filled, t("Filled label with white text: git · branch"))] {
                            changed |= ui.radio_value(&mut self.cfg.git_style, style, label).changed();
                        }
                        ui.add_space(4.0);
                        row_line(ui, th);
                        changed |= toggle_row(
                            ui,
                            th,
                            &mut self.cfg.git_status,
                            &t("Colour by commit state"),
                            &t("Green when everything is committed, orange with changes; off, it is grey. Runs git status in the background, with the repository's own programs (fsmonitor, filters) turned off."),
                        );
                    });
                });
                settings_section(ui, th, &t("Window"), "");
                card(ui, th, |ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(t("When Matrix Finder is started again")).color(th.text));
                    ui.label(
                        RichText::new(t(
                            "For example matrix_finder ~/src in a terminal, while it is already running. --new-tab and --new-window choose for one launch.",
                        ))
                        .size(11.5)
                        .color(th.weak),
                    );
                    ui.add_space(2.0);
                    changed |=
                        ui.radio_value(&mut self.cfg.instance_mode, crate::config::InstanceMode::NewTab, t("Open a new tab in the running window")).changed();
                    changed |= ui.radio_value(&mut self.cfg.instance_mode, crate::config::InstanceMode::NewWindow, t("Open a separate window")).changed();
                    ui.add_space(4.0);
                    row_line(ui, th);
                    changed |= toggle_row(ui, th, &mut self.cfg.show_sidebar, &t("Show sidebar"), "");
                    row_line(ui, th);
                    ui.add_space(4.0);
                    ui.label(RichText::new(t("Drives in the sidebar")).color(th.text));
                    ui.add_space(2.0);
                    for (mode, label) in [
                        (crate::config::SidebarDrives::WhenSeveral, t("Only when there is more than one drive")),
                        (crate::config::SidebarDrives::Always, t("Always")),
                        (crate::config::SidebarDrives::Never, t("Never")),
                    ] {
                        changed |= ui.radio_value(&mut self.cfg.sidebar_drives, mode, label).changed();
                    }
                    ui.add_space(4.0);
                    row_line(ui, th);
                    changed |= toggle_row(ui, th, &mut self.cfg.show_key_hints, &t("Show key hints in the status bar"), "");
                });
                settings_section(ui, th, &t("Safety"), "");
                card(ui, th, |ui| {
                    changed |= toggle_row(ui, th, &mut self.cfg.confirm_delete, &t("Confirm before deleting"), &t("Deleting permanently is always confirmed."));
                    row_line(ui, th);
                    changed |= toggle_row(ui, th, &mut self.cfg.use_trash, &t("Delete to trash (Shift+Del deletes permanently)"), "");
                    row_line(ui, th);
                    changed |= toggle_row(ui, th, &mut self.cfg.confirm_dnd, &t("Ask before drag & drop copies or moves"), "");
                });
                settings_section(ui, th, &t("Updates"), "");
                card(ui, th, |ui| {
                    changed |= toggle_row(
                        ui,
                        th,
                        &mut self.cfg.check_updates,
                        &t("Check for updates when Matrix Finder starts"),
                        &t("At most once a day. Only GitHub is asked for the latest release; nothing about you or your files is sent. Installing stays up to you."),
                    );
                    row_line(ui, th);
                    ui.horizontal(|ui| {
                        let checking = self.update_check.is_some();
                        if ui.add_enabled(!checking, egui::Button::new(t("Check now"))).clicked() {
                            self.check_for_updates(true);
                        }
                        let last = match self.cfg.update_checked {
                            0 => t("Never checked"),
                            secs => chrono::DateTime::from_timestamp(secs, 0)
                                .map(|d| format!("{} {}", t("Last checked"), d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M")))
                                .unwrap_or_default(),
                        };
                        ui.label(RichText::new(format!("{} {} · {last}", t("Version"), env!("CARGO_PKG_VERSION"))).size(11.5).color(th.weak));
                    });
                    if !self.cfg.update_skip.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} {}", t("Skipped version:"), self.cfg.update_skip)).size(11.5).color(th.weak));
                            if ui.small_button(t("Offer it again")).clicked() {
                                self.cfg.update_skip.clear();
                                changed = true;
                            }
                        });
                    }
                });
            }
            SettingsPage::Bookmarks => changed |= self.bookmark_settings(ui, th),
            SettingsPage::QuickFilters => changed |= self.quick_filter_settings(ui, th),
            SettingsPage::DefaultApps => changed |= self.default_app_settings(ui, th),
            SettingsPage::Shortcuts => changed |= self.shortcut_settings(ui, th),
            SettingsPage::Terminal => {
                settings_section(
                    ui,
                    th,
                    &t("Terminal profiles"),
                    &t("A profile is a terminal, a shell and an optional startup script. The terminal shortcut opens the default profile."),
                );
                changed |= self.terminal_profiles_editor(ui, th);
                settings_section(ui, th, &t("Built-in terminal"), &t("Opens in a tab. Ctrl+Shift+C / Ctrl+Shift+V copy and paste, Ctrl+Click opens links."));
                card(ui, th, |ui| {
                    row(ui, th, &t("Font"), "", |ui| {
                        ui.label(format!("{} · {:.1} px", fonts::label(&self.cfg.term_font, &fonts::TERM_BUILTIN), self.cfg.term_font_size));
                        if ui.button(t("Change…")).clicked() {
                            self.settings_page = SettingsPage::Fonts;
                        }
                    });
                    row_line(ui, th);
                    row(ui, th, &t("Scrollback lines"), "", |ui| {
                        changed |= ui.add(egui::DragValue::new(&mut self.cfg.term_scrollback).range(500..=200_000).speed(100)).changed();
                    });
                    row_line(ui, th);
                    for (v, label, hint, default) in [
                        (&mut self.cfg.term_scroll_trackpad, t("Trackpad scroll speed"), t("Lines per row the finger moves"), 1.0),
                        (&mut self.cfg.term_scroll_wheel, t("Mouse wheel scroll speed"), t("Lines per wheel notch"), 3.0),
                    ] {
                        row(ui, th, &label, &hint, |ui| {
                            if *v != default && ui.small_button(t("Reset")).clicked() {
                                *v = default;
                                changed = true;
                            }
                            changed |= ui
                                .add(egui::Slider::new(v, crate::config::SCROLL_SPEEDS).logarithmic(true).step_by(0.1).fixed_decimals(1).suffix("×"))
                                .changed();
                        });
                        row_line(ui, th);
                    }
                    changed |= toggle_row(ui, th, &mut self.cfg.term_copy_on_select, &t("Copy on select"), "");
                    row_line(ui, th);
                    changed |= toggle_row(ui, th, &mut self.cfg.term_close_on_exit, &t("Close the tab when the shell exits"), "");
                    row_line(ui, th);
                    changed |= toggle_row(
                        ui,
                        th,
                        &mut self.cfg.term_confirm_close,
                        &t("Ask before closing a running program"),
                        &t("When closing a window, a tab or the app while a program (not the shell) runs in its terminal"),
                    );
                    row_line(ui, th);
                    let in_home = self.cfg.start_in_home;
                    ui.add_enabled_ui(!in_home, |ui| {
                        let hint = if in_home { t("Matrix Finder starts in the home folder (Settings → Files & layout)") } else { String::new() };
                        changed |= toggle_row(ui, th, &mut self.cfg.term_restore, &t("Reopen terminal tabs at start"), &hint);
                    });
                });
                changed |= self.link_settings(ui, th);
                changed |= self.agent_settings(ui, th);
            }
            SettingsPage::Backup => {
                settings_section(ui, th, &t("Backup & sync"), &t("Passwords are never saved, so an export holds none."));
                card(ui, th, |ui| {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Export settings…")).clicked() {
                            self.export_settings();
                        }
                        if ui.button(t("Import settings…")).clicked() {
                            self.import_settings();
                        }
                    });
                    ui.label(RichText::new(format!("{}: {}", t("Settings file"), crate::config::config_file().display())).size(11.5).color(th.weak));
                });
            }
        }
        changed
    }

    /// Settings → Terminal → Links: the browser that opens web links, and the question
    /// before a link clicked in a terminal is opened.
    fn link_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        use crate::system::defaultapps as apps;
        let mut changed = false;
        let key = if cfg!(target_os = "macos") { "⌘+Click" } else { "Ctrl+Click" };
        settings_section(
            ui,
            th,
            &t("Links"),
            &t("{key} on a link in a terminal opens it, also while a program there (vim, tmux, Claude Code) uses the mouse.").replace("{key}", key),
        );
        let current = self.browser_label(ui.ctx());
        let system = match apps::system_app(ui.ctx(), "html") {
            Some(Some(app)) => format!("{app} ({})", t("system default")),
            _ => t("System default"),
        };
        let missing = apps::program_missing(&self.cfg.browser);
        card(ui, th, |ui| {
            row(ui, th, &t("Web browser"), &t("Opens web links from terminals and the update check"), |ui| {
                let text = if missing { RichText::new(&current).color(th.danger) } else { RichText::new(&current) };
                let combo = egui::ComboBox::from_id_salt("browser").selected_text(text).width(240.0).show_ui(ui, |ui| {
                    let chosen = self.cfg.browser.trim().to_string();
                    if ui.selectable_label(chosen.is_empty(), &system).clicked() {
                        self.cfg.browser.clear();
                        changed = true;
                    }
                    let found = apps::browsers();
                    for (name, program) in &found {
                        if ui.selectable_label(&chosen == program, name).clicked() {
                            self.cfg.browser = program.clone();
                            changed = true;
                        }
                    }
                    // A browser chosen with Other app… stays in the list.
                    if !chosen.is_empty() && !found.iter().any(|(_, p)| p == &chosen) {
                        let _ = ui.selectable_label(true, apps::program_name(&chosen)).on_hover_text(&chosen);
                    }
                    ui.separator();
                    if ui.selectable_label(false, t("Other app…")).clicked() {
                        if let Some(app) = pick_application() {
                            self.cfg.browser = app;
                            changed = true;
                        }
                    }
                });
                if missing {
                    combo.response.on_hover_text(t("This program was not found"));
                }
            });
            row_line(ui, th);
            changed |= toggle_row(
                ui,
                th,
                &mut self.cfg.term_confirm_links,
                &t("Ask before opening a link"),
                &t("Shows where the link goes first. The question can be turned off for one terminal until it closes."),
            );
        });
        changed
    }

    /// Settings → Fonts: the interface's and the terminal's font and size, with samples.
    fn font_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        settings_section(ui, th, &t("Interface"), &t("Menus, file lists and dialogs. Font changes apply instantly."));
        card(ui, th, |ui| {
            row(ui, th, &t("Font"), "", |ui| {
                if let Some(f) = font_picker(ui, th, "ui", &self.cfg.ui_font, &fonts::UI_BUILTIN, false, &mut self.font_search) {
                    self.cfg.ui_font = f;
                    changed = true;
                }
            });
            row_line(ui, th);
            row(ui, th, &t("Size"), "", |ui| {
                ui.spacing_mut().slider_width = 200.0;
                changed |= ui.add(egui::Slider::new(&mut self.cfg.ui_font_size, fonts::UI_SIZES).step_by(0.5).fixed_decimals(1).suffix(" px")).changed();
                if self.cfg.ui_font_size != fonts::DEFAULT_UI_SIZE && ui.small_button(t("Reset")).clicked() {
                    self.cfg.ui_font_size = fonts::DEFAULT_UI_SIZE;
                    changed = true;
                }
            });
            row_line(ui, th);
            ui.add_space(4.0);
            egui::Frame::new().fill(th.bg).stroke(Stroke::new(1.0_f32, th.border)).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 10)).show(
                ui,
                |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("The quick brown fox jumps over the lazy dog").font(bold(15.0)).color(th.strong()));
                    ui.label(RichText::new("Thư mục tiếng Việt · Ăă Đđ Ơơ Ưư · 0123456789").color(th.text));
                    ui.label(RichText::new("report-final.pdf · 2.4 MB · 2026-11-04 16:30").size(12.0).color(th.weak));
                },
            );
            ui.add_space(4.0);
        });
        settings_section(ui, th, &t("Reading"), &t("Markdown, the text of notebooks, EPUB and other documents in Quick View."));
        card(ui, th, |ui| {
            ui.add_space(4.0);
            for (font, label) in [(crate::config::ReadingFont::Sans, "Source Sans 3"), (crate::config::ReadingFont::Serif, "Source Serif 4")] {
                changed |= ui.radio_value(&mut self.cfg.reading_font, font, label).changed();
            }
            ui.add_space(4.0);
            crate::style::fonts::set_reading_serif(self.cfg.reading_font == crate::config::ReadingFont::Serif);
            let sample = crate::preview::markdown::parse_markdown(
                "**Quick View** reads *Markdown* like this: plain words, `code` and [links](https://example.com).\n\nThư mục tiếng Việt · Ăă Đđ Ơơ Ưư",
                th.syntax,
            );
            egui::Frame::new().fill(th.bg).stroke(Stroke::new(1.0_f32, th.border)).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 10)).show(
                ui,
                |ui| {
                    ui.set_width(ui.available_width());
                    crate::preview::markdown::markdown_ui(ui, &sample, th);
                },
            );
            ui.add_space(4.0);
        });
        settings_section(ui, th, &t("Terminal"), &t("The built-in terminal. The sample uses the colours of the default profile."));
        card(ui, th, |ui| {
            row(ui, th, &t("Font"), &t("Monospaced fonts only."), |ui| {
                if let Some(f) = font_picker(ui, th, "term", &self.cfg.term_font, &fonts::TERM_BUILTIN, true, &mut self.font_search) {
                    self.cfg.term_font = f;
                    changed = true;
                }
            });
            row_line(ui, th);
            row(ui, th, &t("Size"), "", |ui| {
                ui.spacing_mut().slider_width = 200.0;
                changed |= ui.add(egui::Slider::new(&mut self.cfg.term_font_size, fonts::TERM_SIZES).step_by(0.5).fixed_decimals(1).suffix(" px")).changed();
                if self.cfg.term_font_size != fonts::DEFAULT_TERM_SIZE && ui.small_button(t("Reset")).clicked() {
                    self.cfg.term_font_size = fonts::DEFAULT_TERM_SIZE;
                    changed = true;
                }
            });
            row_line(ui, th);
            ui.add_space(4.0);
            let scheme = self.cfg.terminal_profiles.get(self.cfg.default_terminal).map(|p| p.theme.clone()).unwrap_or_default();
            let scheme = if scheme.is_empty() { self.theme } else { theme::term_by_name(&scheme) };
            crate::terminal::view::preview(ui, scheme, self.cfg.term_font_size, ui.available_width());
            ui.add_space(4.0);
        });
        changed
    }

    /// Settings → Terminal → AI button: a button in the terminals' info bars that
    /// starts an AI coding agent, and which agent that is.
    fn agent_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        use crate::system::agents::{self, ALL};
        let mut changed = false;
        settings_section(
            ui,
            th,
            &t("AI button"),
            &t("A button in the terminal's info bar starts an AI coding agent at the prompt. It works while the terminal is idle, with no program running."),
        );
        if self.cfg.term_agents {
            self.find_agents(false);
        }
        card(ui, th, |ui| {
            changed |= toggle_row(ui, th, &mut self.cfg.term_agents, &t("Show the AI button"), &t("Off: Matrix Finder does not look for AI coding agents"));
            let searching = self.agents_rx.is_some();
            let found = self.agents.clone().unwrap_or_default();
            let current = agents::pick(&found, &self.cfg.term_agent).map(|f| f.id);
            ui.add_enabled_ui(self.cfg.term_agents, |ui| {
                row_line(ui, th);
                ui.add_space(4.0);
                ui.label(RichText::new(t("The button starts")).color(th.text));
                // Automatic, then one row per agent: its logo, name and what was found.
                let auto_hint = match current {
                    Some(id) if self.cfg.term_agent.is_empty() => agents::get(id).map(|a| a.name.to_string()).unwrap_or_default(),
                    _ => String::new(),
                };
                let mut rows: Vec<(&str, Option<&'static agents::Agent>)> = vec![("", None)];
                rows.extend(ALL.iter().map(|a| (a.id, Some(a))));
                for (id, a) in rows {
                    let f = a.and_then(|a| found.iter().find(|f| f.id == a.id));
                    let available = a.is_none() || f.is_some();
                    let selected = self.cfg.term_agent == id;
                    let (name, hint) = match (a, f) {
                        (None, _) => (t("Automatic: the first one installed"), auto_hint.clone()),
                        (Some(a), _) if searching => (a.name.to_string(), t("Looking…")),
                        (Some(a), None) => (a.name.to_string(), t("Not found")),
                        (Some(a), Some(f)) => (a.name.to_string(), f.run.clone()),
                    };
                    let row = ui.allocate_ui_with_layout(vec2(ui.available_width(), ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_height(ROW_H);
                        let (mr, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        let ring = if available { th.weak } else { th.weak.gamma_multiply(0.4) };
                        ui.painter().circle_stroke(mr.center(), 7.0, Stroke::new(1.3_f32, if selected { th.accent } else { ring }));
                        if selected {
                            ui.painter().circle_filled(mr.center(), 4.0, th.accent);
                        }
                        ui.add_space(4.0);
                        let (ir, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                        if let Some(a) = a {
                            agents::paint_logo(ui.painter(), ir.shrink(3.0), a, if f.is_some() { th.text } else { th.weak.gamma_multiply(0.6) });
                        }
                        ui.add_space(6.0);
                        row_label(ui, th, (ui.available_width() - 8.0).max(100.0), ROW_H, &name, &hint);
                    });
                    let resp = ui.interact(row.response.rect, ui.id().with(("ai-agent", id)), if available { Sense::click() } else { Sense::hover() });
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, available, selected, &name));
                    if available && resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if available && resp.clicked() && !selected {
                        self.cfg.term_agent = id.to_string();
                        changed = true;
                    }
                }
                row_line(ui, th);
                ui.add_space(4.0);
                if ui.add_enabled(!searching, egui::Button::new(t("Look again"))).on_hover_text(t("After installing or removing an agent")).clicked() {
                    self.find_agents(true);
                }
                ui.add_space(2.0);
            });
        });
        changed
    }

    /// Settings editor for terminal profiles: the list on the left, the selected one
    /// beside it (under it in a narrow window). Returns true when something changed.
    fn terminal_profiles_editor(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        self.profile_sel = self.profile_sel.min(self.cfg.terminal_profiles.len() - 1);
        card(ui, th, |ui| {
            ui.add_space(4.0);
            if ui.available_width() < 560.0 {
                changed |= self.profile_list(ui, th, ui.available_width());
                row_line(ui, th);
                changed |= self.profile_detail(ui, th);
            } else {
                let columns = ui.horizontal_top(|ui| {
                    changed |= self.profile_list(ui, th, 176.0);
                    let line_x = ui.cursor().left() + 6.0;
                    ui.add_space(14.0);
                    ui.vertical(|ui| changed |= self.profile_detail(ui, th));
                    line_x
                });
                let (line_x, r) = (columns.inner, columns.response.rect);
                ui.painter().vline(line_x, r.y_range(), Stroke::new(1.0_f32, th.border.gamma_multiply(0.6)));
            }
            ui.add_space(4.0);
        });
        changed
    }

    /// One row per profile, in its colours, and the button that adds a profile.
    fn profile_list(&mut self, ui: &mut egui::Ui, th: &Theme, width: f32) -> bool {
        let mut changed = false;
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 2.0;
            for (i, p) in self.cfg.terminal_profiles.iter().enumerate() {
                let scheme = if p.theme.is_empty() { self.theme } else { theme::term_by_name(&p.theme) };
                let (r, resp) = ui.allocate_exact_size(vec2(width, 50.0), Sense::click());
                let sel = self.profile_sel == i;
                if sel {
                    ui.painter().rect_filled(r, 7.0, th.selection);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 7.0, th.hover());
                }
                let swatch = Rect::from_center_size(pos2(r.left() + 25.0, r.center().y), vec2(36.0, 30.0));
                let built_in = p.terminal == TerminalKind::BuiltIn;
                ui.painter().rect(swatch, 5.0, if built_in { scheme.bg } else { th.surface }, Stroke::new(1.0_f32, th.border), egui::StrokeKind::Inside);
                ui.painter().text(
                    swatch.center(),
                    Align2::CENTER_CENTER,
                    ">_",
                    FontId::new(13.0, FontFamily::Name("term-bold".into())),
                    if built_in { scheme.accent } else { th.weak },
                );
                let text_x = swatch.right() + 9.0;
                let p2 = ui.painter().with_clip_rect(r.shrink(2.0));
                p2.text(pos2(text_x, r.center().y - 9.0), Align2::LEFT_CENTER, &p.name, bold(13.5), th.strong());
                let shell = if p.shell.is_empty() {
                    t("default shell")
                } else {
                    Path::new(&p.shell).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                };
                let sub = if self.cfg.default_terminal == i {
                    format!("★ {}", t("Default"))
                } else if built_in {
                    shell
                } else {
                    p.terminal.label().to_string()
                };
                let sub_col = if self.cfg.default_terminal == i { th.accent } else { th.weak };
                p2.text(pos2(text_x, r.center().y + 9.0), Align2::LEFT_CENTER, sub, FontId::proportional(11.5), sub_col);
                if resp.clicked() {
                    self.profile_sel = i;
                }
            }
            ui.add_space(6.0);
            if ui.add(egui::Button::new(format!("+ {}", t("Add profile"))).min_size(vec2(width, 28.0))).clicked() {
                let n = self.cfg.terminal_profiles.len();
                self.cfg.terminal_profiles.push(terminal::TerminalProfile { name: format!("{} {}", t("Profile"), n + 1), ..Default::default() });
                self.profile_sel = n;
                changed = true;
            }
        });
        changed
    }

    /// The selected profile: its name and actions, a live sample, its colour theme and its program.
    fn profile_detail(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        let i = self.profile_sel;
        let n = self.cfg.terminal_profiles.len();
        let shells = self.known_shells.get_or_insert_with(terminal::known_shells).clone();
        let is_default = self.cfg.default_terminal == i;
        let (mut remove, mut duplicate, mut open) = (false, false, false);
        let width = ui.available_width();
        // ---- name and actions
        let old_name = self.cfg.terminal_profiles[i].name.clone();
        ui.horizontal(|ui| {
            let p = &mut self.cfg.terminal_profiles[i];
            let name = egui::TextEdit::singleline(&mut p.name)
                .id_salt(("profile-name", i))
                .background_color(th.bg)
                .desired_width((width - 210.0).max(120.0))
                .font(bold(15.0));
            changed |= ui.add(name).changed();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if n > 1 && ui.button(RichText::new(t("Delete")).color(th.danger)).clicked() {
                    remove = true;
                }
                if ui.button(t("Duplicate")).clicked() {
                    duplicate = true;
                }
                let run = egui::Button::new(RichText::new(t("Open")).color(th.on_accent())).fill(th.accent);
                if ui.add(run).on_hover_text(t("Open this profile in the current folder")).clicked() {
                    open = true;
                }
            });
        });
        // A renamed profile keeps its open terminals.
        let new_name = self.cfg.terminal_profiles[i].name.clone();
        if new_name != old_name {
            for pane in all_panes!(self) {
                if let Some(term) = pane.tab_mut().term.as_mut() {
                    if term.profile.as_deref() == Some(old_name.as_str()) {
                        term.profile = Some(new_name.clone());
                    }
                }
            }
        }
        let mut make_default = is_default;
        // Turning the default off would leave no profile for its shortcut, so only "on" does anything.
        if toggle_row(ui, th, &mut make_default, &t("Default profile"), &t("Used by the terminal shortcut and button")) && make_default {
            self.cfg.default_terminal = i;
            changed = true;
        }
        row_line(ui, th);
        // ---- what it runs
        let p = &mut self.cfg.terminal_profiles[i];
        let field_w = (width - LABEL_W + 60.0).clamp(160.0, 320.0);
        let label_w = 110.0;
        let field_row = |ui: &mut egui::Ui, label: &str, control: &mut dyn FnMut(&mut egui::Ui)| {
            centered_row(ui, 32.0, |ui| {
                row_label(ui, th, label_w, 32.0, label, "");
                control(ui);
            });
        };
        field_row(ui, &t("Terminal"), &mut |ui| {
            egui::ComboBox::from_id_salt(("term-kind", i)).width(field_w).selected_text(p.terminal.label()).show_ui(ui, |ui| {
                for k in TerminalKind::available() {
                    changed |= ui.selectable_value(&mut p.terminal, k, k.label()).changed();
                }
            });
        });
        if p.terminal == TerminalKind::Custom {
            field_row(ui, &t("Command"), &mut |ui| {
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut p.custom)
                            .background_color(th.bg)
                            .hint_text(RichText::new("myterm --cwd {dir} -e {cmd}").color(th.weak))
                            .desired_width(field_w),
                    )
                    .on_hover_text(t("{dir} is replaced by the current folder, {cmd} by the shell command"))
                    .changed();
            });
        }
        field_row(ui, &t("Shell"), &mut |ui| {
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut p.shell)
                        .background_color(th.bg)
                        .hint_text(RichText::new(t("default shell")).color(th.weak))
                        .desired_width(field_w - 40.0),
                )
                .changed();
            egui::ComboBox::from_id_salt(("term-shell", i)).width(24.0).selected_text("").show_ui(ui, |ui| {
                if ui.selectable_label(p.shell.is_empty(), t("default shell")).clicked() {
                    p.shell.clear();
                    changed = true;
                }
                for sh in &shells {
                    if ui.selectable_label(&p.shell == sh, sh).clicked() {
                        p.shell = sh.clone();
                        changed = true;
                    }
                }
            });
        });
        // Only the built-in terminal can type the script once the shell has started.
        let built_in = p.terminal == TerminalKind::BuiltIn;
        field_row(ui, &t("Startup script"), &mut |ui| {
            let hint = if built_in {
                t("Commands to run when the terminal starts, e.g. source .venv/bin/activate")
            } else {
                t("Only the built-in terminal runs a startup script")
            };
            changed |= ui
                .add_enabled(
                    built_in,
                    egui::TextEdit::multiline(&mut p.startup)
                        .code_editor()
                        .background_color(th.bg)
                        .desired_rows(2)
                        .desired_width((width - label_w - 8.0).max(160.0))
                        .hint_text(RichText::new(hint).color(th.weak)),
                )
                .on_disabled_hover_text(t("Only the built-in terminal runs a startup script"))
                .changed();
        });
        // ---- colours (the other terminals have their own)
        let current = p.theme.clone();
        if built_in {
            row_line(ui, th);
            ui.add_space(4.0);
            let shown = if current.is_empty() { self.theme } else { theme::term_by_name(&current) };
            crate::terminal::view::preview(ui, shown, self.cfg.term_font_size, width);
            ui.add_space(6.0);
            ui.label(RichText::new(t("Colour theme")).font(bold(13.5)).color(th.strong()));
            let gap = 8.0;
            let card_w = gallery_card_width(width, 112.0, gap);
            let mut pick: Option<String> = None;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(gap, gap);
                if term_theme_card(ui, self.theme, &t("Same as the app"), card_w, current.is_empty(), th).clicked() {
                    pick = Some(String::new());
                }
                // A theme from older settings that is not one of the schemes stays selectable.
                let listed = theme::TERM_THEMES.iter().any(|s| s.name == current);
                let legacy = (!current.is_empty() && !listed).then(|| theme::term_by_name(&current));
                for s in legacy.into_iter().chain(theme::TERM_THEMES) {
                    if term_theme_card(ui, s, s.name, card_w, current == s.name, th).clicked() {
                        pick = Some(s.name.to_string());
                    }
                }
            });
            if let Some(name) = pick {
                self.cfg.terminal_profiles[i].theme = name;
                changed = true;
            }
        } else {
            ui.label(RichText::new(t("Colours and fonts are set in that terminal's own settings.")).size(12.0).color(th.weak));
        }
        if duplicate {
            let mut p = self.cfg.terminal_profiles[i].clone();
            p.name = format!("{} {}", p.name, t("copy"));
            self.cfg.terminal_profiles.insert(i + 1, p);
            if self.cfg.default_terminal > i {
                self.cfg.default_terminal += 1;
            }
            self.profile_sel = i + 1;
            changed = true;
        }
        if remove {
            self.cfg.terminal_profiles.remove(i);
            if self.cfg.default_terminal > i || self.cfg.default_terminal >= self.cfg.terminal_profiles.len() {
                self.cfg.default_terminal = self.cfg.default_terminal.saturating_sub(1);
            }
            self.profile_sel = i.min(self.cfg.terminal_profiles.len() - 1);
            changed = true;
        }
        if open {
            self.open_terminal_profile(Some(i));
        }
        changed
    }
}

/// A text field has the keyboard.
fn typing(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused()).is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
}

impl App {
    /// Icon set picker with live previews. Returns true when the choice changed.
    pub(super) fn icon_settings(&mut self, ui: &mut egui::Ui) -> bool {
        let th = self.theme;
        if self.icon_themes.is_none() {
            self.icon_themes = Some((crate::style::sysicons::installed_themes(), crate::style::sysicons::current_theme_name()));
        }
        let (installed, current) = self.icon_themes.clone().unwrap_or_default();
        let mut options: Vec<(String, String)> = vec![("theme".into(), format!("{} ({})", t("Match theme"), self.theme.icons.label()))];
        if !cfg!(any(target_os = "macos", windows)) {
            let cur = current.clone().map(|c| format!(" ({c})")).unwrap_or_default();
            options.push(("system".into(), format!("{}{cur}", t("System icon theme"))));
        }
        for (k, v) in
            [("matrix", "Matrix Finder"), ("adwaita", "Adwaita (GNOME)"), ("breeze", "Breeze (KDE Plasma)"), ("macos", "macOS"), ("windows", "Windows 11")]
        {
            options.push((k.into(), v.into()));
        }
        for n in installed {
            let l = n.to_lowercase();
            if !matches!(l.as_str(), "adwaita" | "breeze" | "breeze-dark" | "hicolor") && !l.contains("cursor") {
                options.push((format!("icons:{n}"), n.clone()));
            }
        }
        let samples: [(&str, bool, &str); 6] = [
            ("Documents", true, ""),
            ("main.rs", false, "rs"),
            ("photo.jpg", false, "jpg"),
            ("backup.zip", false, "zip"),
            ("report.pdf", false, "pdf"),
            ("sheet.xlsx", false, "xlsx"),
        ];
        let mut changed = false;
        egui::Grid::new("icon-packs").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            for (key, label) in &options {
                let selected = self.cfg.icon_pack == *key;
                if ui.radio(selected, label.as_str()).clicked() && !selected {
                    self.cfg.icon_pack = key.clone();
                    changed = true;
                }
                let cache_key = if key == "theme" { format!("theme|{}", th.name) } else { key.clone() };
                if !self.icon_previews.contains_key(&cache_key) {
                    let set = self.make_icon_set(key);
                    self.icon_previews.insert(cache_key.clone(), set);
                }
                let set = self.icon_previews[&cache_key].clone();
                let (rect, _) = ui.allocate_exact_size(egui::vec2(samples.len() as f32 * 26.0, 22.0), egui::Sense::hover());
                for (i, (name, is_dir, ext)) in samples.iter().enumerate() {
                    let r = egui::Rect::from_center_size(egui::pos2(rect.left() + 11.0 + i as f32 * 26.0, rect.center().y), egui::vec2(20.0, 20.0));
                    icons::file_icon_with(&set, ui.painter(), r, name, *is_dir, ext, false, th);
                }
                ui.end_row();
            }
        });
        changed
    }

    /// Settings → Quick filters: the categories and their extensions.
    pub(super) fn quick_filter_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        ui.label(RichText::new(t("The funnel button of a file window shows only the kinds of file you tick (one or several). Folders stay visible, and the filter stays on while you browse. Add your own lists here.")).size(12.0).color(th.weak));
        ui.add_space(6.0);
        let mut frame = card_frame(th).begin(ui);
        {
            let ui = &mut frame.content_ui;
            ui.set_width(ui.available_width());
            let n = self.cfg.quick_filters.len();
            let mut edit: Option<(usize, i32)> = None; // -1 up, 1 down, 0 delete
                                                       // One width for every row (icon, name, count and buttons take ~310 px), so
                                                       // the rows line up and never widen the page.
            let ext_w = (ui.available_width() - 310.0).max(160.0);
            for i in 0..n {
                let q = &mut self.cfg.quick_filters[i];
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                    crate::ui::pane::quick_filter_icon(ui.painter(), r.shrink(5.0), &q.ext_list(), th);
                    changed |= ui.add(egui::TextEdit::singleline(&mut q.name).desired_width(130.0).hint_text(t("Name"))).changed();
                    changed |= ui
                        .add(egui::TextEdit::singleline(&mut q.exts).desired_width(ext_w).hint_text(t("Extensions, e.g. jpg png gif")))
                        .on_hover_text(t("Extensions separated by spaces or commas (dots are optional)"))
                        .changed();
                    let (cr, cresp) = ui.allocate_exact_size(egui::vec2(26.0, 24.0), egui::Sense::hover());
                    ui.painter().text(cr.center(), egui::Align2::CENTER_CENTER, q.ext_list().len().to_string(), egui::FontId::proportional(11.5), th.weak);
                    cresp.on_hover_text(t("Extensions"));
                    if icons::icon_button(ui, 24.0, i > 0, th, |p, r, c| icons::chevron(p, r, 0, c)).on_hover_text(t("Move up")).clicked() {
                        edit = Some((i, -1));
                    }
                    if icons::icon_button(ui, 24.0, i + 1 < n, th, |p, r, c| icons::chevron(p, r, 2, c)).on_hover_text(t("Move down")).clicked() {
                        edit = Some((i, 1));
                    }
                    if icons::icon_button(ui, 24.0, true, th, icons::mi_trash).on_hover_text(t("Remove")).clicked() {
                        edit = Some((i, 0));
                    }
                });
            }
            match edit {
                Some((i, -1)) => self.cfg.quick_filters.swap(i, i - 1),
                Some((i, 1)) => self.cfg.quick_filters.swap(i, i + 1),
                Some((i, _)) => {
                    self.cfg.quick_filters.remove(i);
                }
                None => {}
            }
            changed |= edit.is_some();
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(format!("+ {}", t("New quick filter"))).clicked() {
                    let name = crate::config::unused_quick_filter_name(&self.cfg.quick_filters, &t("New filter"));
                    self.cfg.quick_filters.push(crate::config::QuickFilter::new(&name, ""));
                    changed = true;
                }
                if ui
                    .button(t("Restore the built-in filters"))
                    .on_hover_text(t("Adds the built-in categories that are missing and resets their extensions"))
                    .clicked()
                {
                    for d in crate::config::default_quick_filters() {
                        match self.cfg.quick_filters.iter_mut().find(|q| q.name == d.name) {
                            Some(q) => q.exts = d.exts,
                            None => self.cfg.quick_filters.push(d),
                        }
                    }
                    changed = true;
                }
            });
        }
        frame.end(ui);
        changed
    }

    pub(super) fn export_settings(&mut self) {
        if let Some(p) = rfd::FileDialog::new().set_file_name("matrix_finder-settings.json").save_file() {
            match self.cfg.export_to(&p, false) {
                Ok(()) => self.toast(format!("{} {}", t("Exported to"), p.display()), false),
                Err(e) => self.toast(e, true),
            }
        }
    }

    /// Pick a settings file; replacing the settings with it is asked first.
    pub(super) fn import_settings(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
            self.ask_import_settings(p);
        }
    }

    fn ask_import_settings(&mut self, path: PathBuf) {
        match Settings::import_from(&path) {
            Ok(s) => self.show_dialog(Dialog::ImportSettings { path, settings: Box::new(s) }, false),
            Err(e) => self.toast(e, true),
        }
    }

    /// Replace the settings with imported ones, after keeping a copy of the current ones.
    /// Without that copy nothing is replaced.
    pub(super) fn import_confirmed(&mut self, settings: Settings) {
        // The open tabs too, as they are now.
        self.save_cfg();
        match self.store.backup_before_import(&self.cfg) {
            Ok(copy) => {
                self.cfg = settings;
                let ctx = self.egui_ctx.clone();
                self.apply_cfg(&ctx);
                self.save_cfg();
                self.toast(format!("{} {}", t("Settings imported. The previous settings are kept in"), copy.display()), false);
            }
            Err(e) => self.toast(format!("{}: {e}", t("Settings not imported: the current settings could not be backed up")), true),
        }
    }
}
