//! Themes in Settings: the galleries of Appearance and Terminal, and the themes the user
//! makes (`config::CustomTheme`, `config::CustomTermTheme`): new from the theme shown,
//! edited with live colours, exported and imported as JSON files.
use super::settings_ui::{bold, card, gallery_card_width, row, row_line, term_theme_card, theme_card, toggle};
use super::*;
use crate::config::{unique_theme_names, CustomTermTheme, CustomTheme};
use egui::{pos2, vec2, Align2, FontId, Rect, Sense, Stroke};

/// The `type` of an exported theme file.
const APP_FILE: &str = "matrix-finder-theme";
const TERM_FILE: &str = "matrix-finder-terminal-theme";
/// A theme file is small; a larger one is not read.
const MAX_FILE: u64 = 256 * 1024;

/// The colours of an app theme, as the editor lists them: label, explanation, field.
type ColourField = (&'static str, &'static str, fn(&mut CustomTheme) -> &mut String);
const APP_COLOURS: [ColourField; 12] = [
    ("Background", "Files, terminal and previews", |c| &mut c.bg),
    ("Panels", "Sidebar, tab bar and windows", |c| &mut c.panel),
    ("Fields", "Buttons, fields and cards", |c| &mut c.surface),
    ("Text", "", |c| &mut c.text),
    ("Dim text", "Sizes, dates, hints", |c| &mut c.weak),
    ("Accent", "Selected controls and links", |c| &mut c.accent),
    ("Cursor row", "The file under the cursor", |c| &mut c.selection),
    ("Marked files", "", |c| &mut c.marked),
    ("Folders", "", |c| &mut c.dir),
    ("Archives", "", |c| &mut c.archive),
    ("Borders", "", |c| &mut c.border),
    ("Warnings", "Delete buttons and errors", |c| &mut c.danger),
];

/// The 16 terminal colours, in order.
const ANSI_NAMES: [&str; 16] = [
    "Black",
    "Red",
    "Green",
    "Yellow",
    "Blue",
    "Magenta",
    "Cyan",
    "White",
    "Bright black",
    "Bright red",
    "Bright green",
    "Bright yellow",
    "Bright blue",
    "Bright magenta",
    "Bright cyan",
    "Bright white",
];

/// Which list a theme the user made belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ThemeList {
    App,
    Terminal,
}

/// A colour button and its `#rrggbb` field; `value` changes only to a valid colour from
/// the button, and to whatever is typed (an invalid one draws in its fallback colour).
fn colour_edit(ui: &mut egui::Ui, th: &Theme, value: &mut String, fallback: egui::Color32) -> bool {
    let mut changed = false;
    let mut c = theme::parse_hex(value).unwrap_or(fallback);
    if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque).changed() {
        *value = theme::to_hex(c);
        changed = true;
    }
    let valid = theme::parse_hex(value).is_some();
    let field = egui::TextEdit::singleline(value).font(egui::TextStyle::Monospace).char_limit(7).text_color(if valid { th.text } else { th.danger });
    changed |= ui.add_sized([76.0, 22.0], field).changed();
    changed
}

/// A card that adds something to a gallery: a dashed outline with a plus and a label.
fn add_card(ui: &mut egui::Ui, th: &Theme, label: &str, width: f32, height: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 9.0, th.hover());
    }
    let stroke = Stroke::new(1.0_f32, if resp.hovered() { th.accent } else { th.weak.gamma_multiply(0.7) });
    let r = rect.shrink(1.0);
    // A dashed outline: four sides, each as dashes.
    for (a, b) in [(r.left_top(), r.right_top()), (r.right_top(), r.right_bottom()), (r.right_bottom(), r.left_bottom()), (r.left_bottom(), r.left_top())] {
        p.extend(egui::Shape::dashed_line(&[a, b], stroke, 5.0, 4.0));
    }
    let c = pos2(rect.center().x, rect.center().y - 9.0);
    icons::plus(p, Rect::from_center_size(c, vec2(16.0, 16.0)), if resp.hovered() { th.accent } else { th.weak });
    p.text(pos2(rect.center().x, rect.bottom() - 15.0), Align2::CENTER_CENTER, label, FontId::proportional(11.5), th.text);
    resp
}

impl App {
    /// Register the user's themes after they changed (an edit, an import, settings
    /// loaded); the theme shown follows at the next frame, as for a changed system theme.
    pub(super) fn sync_custom_themes(&mut self, ctx: &egui::Context) {
        theme::set_custom(&self.cfg.custom_themes, &self.cfg.custom_terminal_themes);
        self.sync_terminal_profiles();
        ctx.request_repaint();
    }

    /// The names a theme the user makes may not have: every built-in one and the user's
    /// other themes of either list (index `skip` of `list` is the one being renamed).
    fn theme_name_taken(&self, name: &str, list: ThemeList, skip: Option<usize>) -> bool {
        let n = name.trim();
        let mine = |l: ThemeList, i: usize, other: &str| !(l == list && Some(i) == skip) && other.eq_ignore_ascii_case(n);
        theme::is_builtin(n)
            || self.cfg.custom_themes.iter().enumerate().any(|(i, c)| mine(ThemeList::App, i, &c.name))
            || self.cfg.custom_terminal_themes.iter().enumerate().any(|(i, c)| mine(ThemeList::Terminal, i, &c.name))
    }

    /// A name of its own, from `base` ("Base", "Base 2", …).
    fn free_theme_name(&self, base: &str) -> String {
        let mut taken: Vec<String> = self.cfg.custom_themes.iter().map(|c| c.name.clone()).collect();
        taken.extend(self.cfg.custom_terminal_themes.iter().map(|c| c.name.clone()));
        let n = taken.len();
        taken.push(base.to_string());
        unique_theme_names(&taken, base).swap_remove(n)
    }

    /// Settings → Appearance: the theme gallery (system, light, dark, the user's own), and
    /// the editor of the user's theme that is shown.
    pub(super) fn theme_gallery(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        settings_section(ui, th, &t("Theme"), &t("Pick a theme. Changes apply instantly."));
        let mut pick: Option<String> = None;
        let mut new_theme = false;
        let mut import = false;
        card(ui, th, |ui| {
            ui.add_space(4.0);
            let card_w = gallery_card_width(ui.available_width(), 118.0, 8.0);
            let heading = |ui: &mut egui::Ui, text: String| {
                ui.add_space(2.0);
                ui.label(RichText::new(text).font(bold(12.5)).color(th.weak));
            };
            let cur = self.cfg.theme.clone();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                let native = theme::native(ui.ctx().system_theme() == Some(egui::Theme::Dark));
                let sys_label = format!("{} · {}", t("System"), native.name);
                if theme_card(ui, native, &sys_label, card_w, cur == theme::SYSTEM, th)
                    .on_hover_text(t("Follow the operating system: native look, light or dark automatically"))
                    .clicked()
                {
                    pick = Some(theme::SYSTEM.into());
                }
            });
            for (label, dark) in [(t("Light"), false), (t("Dark"), true)] {
                heading(ui, label);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    for theme in theme::THEMES.iter().filter(|x| x.dark == dark) {
                        if theme_card(ui, theme, theme.name, card_w, cur == theme.name, th).clicked() {
                            pick = Some(theme.name.into());
                        }
                    }
                });
            }
            heading(ui, t("Your themes"));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for theme in theme::custom() {
                    if theme_card(ui, theme, theme.name, card_w, cur == theme.name, th).clicked() {
                        pick = Some(theme.name.into());
                    }
                }
                new_theme = add_card(ui, th, &t("New theme"), card_w, 70.0).on_hover_text(t("A copy of the theme shown, to change as you like")).clicked();
            });
            ui.horizontal(|ui| {
                import = ui.button(t("Import theme…")).on_hover_text(t("A theme file exported from Matrix Finder (.json)")).clicked();
            });
            ui.add_space(4.0);
        });
        if new_theme {
            let name = self.free_theme_name(&format!("{} ({})", self.theme.name, t("custom")));
            self.cfg.custom_themes.push(theme::to_custom(self.theme, &name));
            self.sync_custom_themes(ui.ctx());
            pick = Some(name);
            changed = true;
        }
        if import {
            changed |= self.import_theme(ui.ctx(), ThemeList::App, None);
        }
        if let Some(name) = pick {
            self.theme_name_draft = None;
            self.theme_delete = None;
            self.set_theme(ui.ctx(), &name);
        }
        if let Some(i) = self.cfg.custom_themes.iter().position(|c| c.name == self.cfg.theme) {
            changed |= self.theme_editor(ui, th, i);
        }
        changed
    }

    /// The name field of a theme the user made: a new name applies once it is one of its
    /// own (the theme is found by name, by the settings and by terminal profiles).
    fn theme_name_row(&mut self, ui: &mut egui::Ui, th: &Theme, list: ThemeList, i: usize) -> bool {
        let current = match list {
            ThemeList::App => self.cfg.custom_themes[i].name.clone(),
            ThemeList::Terminal => self.cfg.custom_terminal_themes[i].name.clone(),
        };
        let mut draft = match &self.theme_name_draft {
            Some((l, k, d)) if *l == list && *k == i => d.clone(),
            _ => current.clone(),
        };
        let mut renamed = None;
        let mut field = |ui: &mut egui::Ui, width: f32| {
            let resp = ui.add(egui::TextEdit::singleline(&mut draft).desired_width(width));
            let problem = if draft.trim().is_empty() {
                Some(t("A theme needs a name"))
            } else if draft.trim() != current && self.theme_name_taken(&draft, list, Some(i)) {
                Some(t("Another theme has this name"))
            } else {
                None
            };
            if resp.changed() && problem.is_none() && draft.trim() != current {
                renamed = Some(draft.trim().to_string());
            }
            problem
        };
        // The app's editor has a row per setting; a terminal profile's column is narrow.
        let problem = match list {
            ThemeList::App => row(ui, th, &t("Name"), "", |ui| field(ui, 240.0)),
            ThemeList::Terminal => {
                ui.horizontal(|ui| {
                    ui.label(t("Name"));
                    let w = (ui.available_width() - 8.0).clamp(80.0, 240.0);
                    field(ui, w)
                })
                .inner
            }
        };
        if let Some(p) = &problem {
            ui.label(RichText::new(p).size(12.0).color(th.danger));
        }
        self.theme_name_draft = (draft != current).then(|| (list, i, draft));
        let Some(name) = renamed else { return false };
        match list {
            ThemeList::App => {
                self.cfg.custom_themes[i].name = name.clone();
                if self.cfg.theme == current {
                    self.cfg.theme = name;
                }
            }
            ThemeList::Terminal => {
                self.cfg.custom_terminal_themes[i].name = name.clone();
                for p in self.cfg.terminal_profiles.iter_mut().filter(|p| p.theme == current) {
                    p.theme = name.clone();
                }
            }
        }
        self.theme_name_draft = None;
        true
    }

    /// Duplicate, Export and Delete for a theme the user made. Delete asks first.
    fn theme_actions(&mut self, ui: &mut egui::Ui, th: &Theme, list: ThemeList, i: usize) -> bool {
        let mut changed = false;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(t("Duplicate")).clicked() {
                changed = true;
                // A name being typed belongs to this theme, not to its copy shown next.
                self.theme_name_draft = None;
                self.theme_delete = None;
                match list {
                    ThemeList::App => {
                        let mut c = self.cfg.custom_themes[i].clone();
                        c.name = self.free_theme_name(&c.name);
                        self.cfg.custom_themes.insert(i + 1, c.clone());
                        self.cfg.theme = c.name;
                    }
                    ThemeList::Terminal => {
                        let mut c = self.cfg.custom_terminal_themes[i].clone();
                        c.name = self.free_theme_name(&c.name);
                        self.cfg.custom_terminal_themes.insert(i + 1, c.clone());
                        if let Some(p) = self.cfg.terminal_profiles.get_mut(self.profile_sel) {
                            p.theme = c.name;
                        }
                    }
                }
            }
            if ui.button(t("Export…")).on_hover_text(t("Save this theme as a file, to share it or to keep it")).clicked() {
                self.export_theme(list, i);
            }
            let asking = self.theme_delete == Some((list, i));
            if !asking && ui.button(RichText::new(t("Delete")).color(th.danger)).clicked() {
                self.theme_delete = Some((list, i));
            }
            if asking {
                let name = match list {
                    ThemeList::App => &self.cfg.custom_themes[i].name,
                    ThemeList::Terminal => &self.cfg.custom_terminal_themes[i].name,
                };
                ui.label(RichText::new(t("Delete “{name}”?").replace("{name}", name)).color(th.danger));
                if ui.button(RichText::new(t("Delete")).color(th.danger)).clicked() {
                    self.delete_theme(list, i);
                    changed = true;
                }
                if ui.button(t("Cancel")).clicked() {
                    self.theme_delete = None;
                }
            }
        });
        changed
    }

    fn delete_theme(&mut self, list: ThemeList, i: usize) {
        self.theme_delete = None;
        self.theme_name_draft = None;
        match list {
            ThemeList::App => {
                let c = self.cfg.custom_themes.remove(i);
                if self.cfg.theme == c.name {
                    // Back to a built-in theme of the same kind.
                    self.cfg.theme = (if c.dark { "Breeze Dark" } else { theme::DEFAULT }).into();
                }
            }
            ThemeList::Terminal => {
                let c = self.cfg.custom_terminal_themes.remove(i);
                for p in self.cfg.terminal_profiles.iter_mut().filter(|p| p.theme == c.name) {
                    p.theme = String::new();
                }
            }
        }
    }

    /// The editor of the user's app theme `i` (the one shown): name, light or dark, the
    /// colours, corners, icons, code colours and terminal colours.
    fn theme_editor(&mut self, ui: &mut egui::Ui, th: &Theme, i: usize) -> bool {
        let mut changed = false;
        let title = t("Edit “{name}”").replace("{name}", &self.cfg.custom_themes[i].name);
        settings_section(ui, th, &title, &t("Changes apply as you make them."));
        card(ui, th, |ui| {
            changed |= self.theme_name_row(ui, th, ThemeList::App, i);
            row_line(ui, th);
            let c = &mut self.cfg.custom_themes[i];
            row(ui, th, &t("Dark theme"), &t("Light or dark controls, scroll bars and menus"), |ui| {
                changed |= toggle(ui, th, &mut c.dark, &t("Dark theme")).changed();
            });
            row_line(ui, th);
            let shown = theme::from_custom(c);
            let fallback = |k: usize| -> egui::Color32 {
                [
                    shown.bg,
                    shown.panel,
                    shown.surface,
                    shown.text,
                    shown.weak,
                    shown.accent,
                    shown.selection,
                    shown.marked,
                    shown.dir,
                    shown.archive,
                    shown.border,
                    shown.danger,
                ][k]
            };
            ui.add_space(4.0);
            egui::Grid::new("theme-colours").num_columns(4).spacing([12.0, 6.0]).show(ui, |ui| {
                for (k, (label, hint, field)) in APP_COLOURS.iter().enumerate() {
                    ui.horizontal(|ui| {
                        changed |= colour_edit(ui, th, field(c), fallback(k));
                    });
                    let l = ui.label(t(label));
                    if !hint.is_empty() {
                        l.on_hover_text(t(hint));
                    }
                    if k % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
            ui.add_space(4.0);
            row_line(ui, th);
            row(ui, th, &t("Corners"), &t("Rounding of buttons, fields and windows"), |ui| {
                changed |= ui.add(egui::Slider::new(&mut c.radius, 0..=12)).changed();
            });
            row_line(ui, th);
            row(ui, th, &t("Icons"), &t("When Icons is set to Match theme"), |ui| {
                let shown = icons::IconPack::from_key(&c.icons).unwrap_or(icons::IconPack::Matrix).label();
                egui::ComboBox::from_id_salt("theme-icons").selected_text(shown).show_ui(ui, |ui| {
                    for p in icons::IconPack::ALL {
                        if ui.selectable_label(c.icons == p.key(), p.label()).clicked() {
                            c.icons = p.key().into();
                            changed = true;
                        }
                    }
                });
            });
            row_line(ui, th);
            row(ui, th, &t("Code colours"), &t("Previews of source code"), |ui| {
                egui::ComboBox::from_id_salt("theme-syntax").selected_text(c.syntax.clone()).width(200.0).show_ui(ui, |ui| {
                    for s in theme::SYNTAX_THEMES {
                        if ui.selectable_label(c.syntax == *s, *s).clicked() {
                            c.syntax = s.to_string();
                            changed = true;
                        }
                    }
                });
            });
            row_line(ui, th);
            row(ui, th, &t("Terminal colours"), &t("For terminals set to Same as the app"), |ui| {
                let mut own = !c.ansi.is_empty();
                if toggle(ui, th, &mut own, &t("Terminal colours")).changed() {
                    c.ansi = if own { theme::to_custom_term(&shown, "").ansi } else { Vec::new() };
                    changed = true;
                }
                ui.label(RichText::new(if own { t("Its own") } else { t("A general light or dark set") }).size(12.0).color(th.weak));
            });
            if c.ansi.len() == 16 {
                changed |= ansi_grid(ui, th, "theme-ansi", &mut c.ansi);
            }
            row_line(ui, th);
            changed |= self.theme_actions(ui, th, ThemeList::App, i);
        });
        if changed {
            self.sync_custom_themes(ui.ctx());
        }
        changed
    }

    /// Settings → Terminal, a built-in terminal profile's colours: the schemes, the user's
    /// own, a new one or an import, and the editor of the user's scheme it uses.
    pub(super) fn term_theme_gallery(&mut self, ui: &mut egui::Ui, th: &Theme, i: usize, width: f32) -> bool {
        let mut changed = false;
        let current = self.cfg.terminal_profiles[i].theme.clone();
        let gap = 8.0;
        let card_w = gallery_card_width(width, 112.0, gap);
        let mut pick: Option<String> = None;
        let mut new_scheme = false;
        let mut import = false;
        let mine = theme::custom_term();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            if term_theme_card(ui, self.theme, &t("Same as the app"), card_w, current.is_empty(), th).clicked() {
                pick = Some(String::new());
            }
            // A theme from older settings that is not one of the schemes stays selectable.
            let listed = theme::TERM_THEMES.iter().chain(mine.iter().copied()).any(|s| s.name == current);
            let legacy = (!current.is_empty() && !listed).then(|| theme::term_by_name(&current));
            for s in legacy.into_iter().chain(theme::TERM_THEMES) {
                if term_theme_card(ui, s, s.name, card_w, current == s.name, th).clicked() {
                    pick = Some(s.name.to_string());
                }
            }
        });
        ui.label(RichText::new(t("Your colour themes")).font(bold(12.5)).color(th.weak));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            for s in &mine {
                if term_theme_card(ui, s, s.name, card_w, current == s.name, th).clicked() {
                    pick = Some(s.name.to_string());
                }
            }
            new_scheme =
                add_card(ui, th, &t("New colour theme"), card_w, 76.0).on_hover_text(t("A copy of the colours shown, to change as you like")).clicked();
        });
        ui.horizontal(|ui| {
            import = ui.button(t("Import colour theme…")).on_hover_text(t("A colour theme file exported from Matrix Finder (.json)")).clicked();
        });
        if new_scheme {
            let shown = if current.is_empty() { self.theme } else { theme::term_by_name(&current) };
            let name = self.free_theme_name(&format!("{} ({})", shown.name, t("custom")));
            self.cfg.custom_terminal_themes.push(theme::to_custom_term(shown, &name));
            pick = Some(name);
        }
        if import {
            changed |= self.import_theme(ui.ctx(), ThemeList::Terminal, Some(i));
        }
        if let Some(name) = pick {
            self.theme_name_draft = None;
            self.theme_delete = None;
            self.cfg.terminal_profiles[i].theme = name;
            self.sync_custom_themes(ui.ctx());
            changed = true;
        }
        let current = self.cfg.terminal_profiles[i].theme.clone();
        if let Some(k) = self.cfg.custom_terminal_themes.iter().position(|c| c.name == current) {
            ui.add_space(6.0);
            let title = t("Edit “{name}”").replace("{name}", &current);
            ui.label(RichText::new(title).font(bold(13.5)).color(th.strong()));
            let mut edited = self.theme_name_row(ui, th, ThemeList::Terminal, k);
            let c = &mut self.cfg.custom_terminal_themes[k];
            let shown = theme::from_custom_term(c);
            for (label, value, fallback) in
                [(t("Background"), &mut c.background, shown.bg), (t("Text"), &mut c.foreground, shown.text), (t("Cursor"), &mut c.cursor, shown.accent)]
            {
                ui.horizontal(|ui| {
                    edited |= colour_edit(ui, th, value, fallback);
                    ui.label(label);
                });
            }
            if c.ansi.len() != 16 {
                c.ansi = theme::to_custom_term(&shown, "").ansi;
            }
            edited |= ansi_grid(ui, th, "term-ansi", &mut c.ansi);
            edited |= self.theme_actions(ui, th, ThemeList::Terminal, k);
            if edited {
                self.sync_custom_themes(ui.ctx());
                changed = true;
            }
        }
        changed
    }

    /// Save the user's theme `i` of `list` as a JSON file the user picks.
    fn export_theme(&mut self, list: ThemeList, i: usize) {
        let (name, value) = match list {
            ThemeList::App => {
                let c = &self.cfg.custom_themes[i];
                (c.name.clone(), theme_file(APP_FILE, c))
            }
            ThemeList::Terminal => {
                let c = &self.cfg.custom_terminal_themes[i];
                (c.name.clone(), theme_file(TERM_FILE, c))
            }
        };
        let file = format!("{}.json", file_stem(&name));
        let Some(p) = rfd::FileDialog::new().set_file_name(file).add_filter("JSON", &["json"]).save_file() else { return };
        let r = value.and_then(|v| serde_json::to_string_pretty(&v).map_err(es)).and_then(|s| crate::util::write_atomic(&p, s.as_bytes(), false).map_err(es));
        match r {
            Ok(()) => self.toast(format!("{} {}", t("Exported to"), p.display()), false),
            Err(e) => self.toast(e, true),
        }
    }

    /// Read a theme file the user picks into `list` and use it: as the app's theme, or for
    /// terminal profile `profile`. Returns whether a theme was added.
    fn import_theme(&mut self, ctx: &egui::Context, list: ThemeList, profile: Option<usize>) -> bool {
        let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() else { return false };
        match read_theme_file(&p, list) {
            Ok(Imported::App(mut c)) => {
                c.name = self.free_theme_name(&c.name);
                let name = c.name.clone();
                self.cfg.custom_themes.push(c);
                self.sync_custom_themes(ctx);
                self.set_theme(ctx, &name);
                self.toast(format!("{}: {name}", t("Theme imported")), false);
                true
            }
            Ok(Imported::Terminal(mut c)) => {
                c.name = self.free_theme_name(&c.name);
                let name = c.name.clone();
                self.cfg.custom_terminal_themes.push(c);
                if let Some(p) = profile.and_then(|k| self.cfg.terminal_profiles.get_mut(k)) {
                    p.theme = name.clone();
                }
                self.sync_custom_themes(ctx);
                self.toast(format!("{}: {name}", t("Colour theme imported")), false);
                true
            }
            Err(e) => {
                self.toast(e, true);
                false
            }
        }
    }
}

/// The 16 terminal colours as two rows of eight swatches (a colour button each, its name
/// on hover).
fn ansi_grid(ui: &mut egui::Ui, th: &Theme, id: &str, ansi: &mut [String]) -> bool {
    let mut changed = false;
    egui::Grid::new(id).num_columns(9).spacing([6.0, 6.0]).show(ui, |ui| {
        for (row_k, label) in [(0, t("Normal")), (8, t("Bright"))] {
            ui.label(RichText::new(label).size(12.0).color(th.weak));
            for k in row_k..row_k + 8 {
                let mut c = theme::parse_hex(&ansi[k]).unwrap_or(th.text);
                let resp = egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque);
                if resp.changed() {
                    ansi[k] = theme::to_hex(c);
                    changed = true;
                }
                resp.on_hover_text(format!("{} · {}", t(ANSI_NAMES[k]), ansi[k]));
            }
            ui.end_row();
        }
    });
    changed
}

/// A theme's name as a file name: what a file name cannot hold becomes `-`.
fn file_stem(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '-' } else { c }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() {
        "theme".into()
    } else {
        s
    }
}

/// A theme as the JSON object of an exported file: its fields and the file's `type`.
fn theme_file<T: serde::Serialize>(kind: &str, theme: &T) -> Res<serde_json::Value> {
    let mut v = serde_json::to_value(theme).map_err(es)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("type".into(), serde_json::Value::String(kind.into()));
    }
    Ok(v)
}

/// A theme read from a file.
#[derive(Debug, PartialEq)]
enum Imported {
    App(CustomTheme),
    Terminal(CustomTermTheme),
}

/// Read a theme file: an app theme (for `ThemeList::App`) or a terminal colour theme.
fn read_theme_file(path: &Path, list: ThemeList) -> Res<Imported> {
    let len = std::fs::metadata(path).map_err(es)?.len();
    if len > MAX_FILE {
        return Err(t("This is not a theme file (it is too large)"));
    }
    let text = std::fs::read_to_string(path).map_err(es)?;
    parse_theme_file(&text, list)
}

fn parse_theme_file(text: &str, list: ThemeList) -> Res<Imported> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("{}: {e}", t("This is not a theme file")))?;
    let kind = v.get("type").and_then(|k| k.as_str()).unwrap_or_default();
    let wrong = match (list, kind) {
        (ThemeList::App, TERM_FILE) => Some(t("This is a terminal colour theme: import it in Settings → Terminal")),
        (ThemeList::Terminal, APP_FILE) => Some(t("This is an app theme: import it in Settings → Appearance")),
        (_, APP_FILE | TERM_FILE) => None,
        _ => Some(t("This is not a theme file")),
    };
    if let Some(e) = wrong {
        return Err(e);
    }
    let fallback = |n: &str| if n.trim().is_empty() { t("Imported theme") } else { n.trim().chars().take(60).collect() };
    match list {
        ThemeList::App => {
            let mut c: CustomTheme = serde_json::from_value(v).map_err(|e| format!("{}: {e}", t("This is not a theme file")))?;
            c.name = fallback(&c.name);
            Ok(Imported::App(c))
        }
        ThemeList::Terminal => {
            let mut c: CustomTermTheme = serde_json::from_value(v).map_err(|e| format!("{}: {e}", t("This is not a theme file")))?;
            c.name = fallback(&c.name);
            Ok(Imported::Terminal(c))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_files_round_trip_and_go_to_their_page() {
        let app = theme::to_custom(theme::by_name("Nord"), "My Nord");
        let text = serde_json::to_string(&theme_file(APP_FILE, &app).unwrap()).unwrap();
        assert_eq!(parse_theme_file(&text, ThemeList::App), Ok(Imported::App(app.clone())));
        assert!(parse_theme_file(&text, ThemeList::Terminal).unwrap_err().contains("Appearance"), "an app theme belongs to Appearance");
        let term = theme::to_custom_term(theme::term_by_name("Dracula"), "Night");
        let text = serde_json::to_string(&theme_file(TERM_FILE, &term).unwrap()).unwrap();
        assert_eq!(parse_theme_file(&text, ThemeList::Terminal), Ok(Imported::Terminal(term)));
        assert!(parse_theme_file(&text, ThemeList::App).unwrap_err().contains("Terminal"));
        // Not a theme: other JSON, other text, a file without a name.
        assert!(parse_theme_file(r#"{"name": "x"}"#, ThemeList::App).is_err(), "no type");
        assert!(parse_theme_file("[1, 2]", ThemeList::App).is_err());
        assert!(parse_theme_file("not json", ThemeList::App).is_err());
        let unnamed = format!(r##"{{"type": "{APP_FILE}", "bg": "#000000", "dark": true}}"##);
        let Ok(Imported::App(c)) = parse_theme_file(&unnamed, ThemeList::App) else { panic!("a theme with missing fields still reads") };
        assert_eq!(c.name, t("Imported theme"));
        assert!(theme::from_custom(&c).dark);
    }
}
