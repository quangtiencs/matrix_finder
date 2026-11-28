//! Settings → Quick filters: the kinds of file the funnel menu of a file window offers.
//! One compact row per filter (its icon, name and extensions); a click opens its editor
//! below it, where the extensions are chips that can be removed one by one or added as
//! typed. Rows are reordered by dragging their handle; removing one asks first.
use super::settings_ui::{bold, drag_feedback, grip_dots};
use super::*;
use crate::config::{default_quick_filters, unused_quick_filter_name, QuickFilter};
use crate::ui::KeptContextMenu as _;
use egui::{pos2, vec2, Align2, FontId, Rect, Sense, Stroke};

/// What the Quick filters page has open: kept between frames.
#[derive(Default)]
pub(super) struct QfUi {
    /// The filter whose editor is open.
    pub(super) open: Option<usize>,
    /// The filter whose removal waits for a confirmation.
    delete: Option<usize>,
    /// Restoring the built-in filters waits for a confirmation.
    restore: bool,
    /// Extensions typed in the open editor and not added yet.
    add: String,
    /// Put the cursor in the open editor's name field (a filter just made).
    focus_name: bool,
}

enum QfEdit {
    /// Move filter `.0` to stand before the filter now at `.1` (`len`: the end).
    Move(usize, usize),
    Duplicate(usize),
    Remove(usize),
}

const ROW_H: f32 = 40.0;
/// Where the name starts and the extensions start, from the row's left.
const NAME_X: f32 = 52.0;
const NAME_W: f32 = 150.0;

/// The extensions as ".jpg .png …" in at most `w` points, with how many did not fit.
fn ext_summary(ui: &egui::Ui, exts: &[String], w: f32, font: &FontId) -> String {
    let width = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), egui::Color32::WHITE).size().x);
    let mut shown = String::new();
    for (i, e) in exts.iter().enumerate() {
        let next = if shown.is_empty() { format!(".{e}") } else { format!("{shown}  .{e}") };
        let left = exts.len() - i - 1;
        let more = if left > 0 { format!("  +{left}") } else { String::new() };
        if width(&format!("{next}{more}")) > w {
            let rest = exts.len() - i;
            return if shown.is_empty() { format!("+{rest}") } else { format!("{shown}  +{rest}") };
        }
        shown = next;
    }
    shown
}

/// `order` after moving the item at `from` before the one at `to` (`len`: the end).
fn moved(len: usize, from: usize, to: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    let x = order.remove(from);
    order.insert(if to > from { to - 1 } else { to }, x);
    order
}

impl App {
    /// Settings → Quick filters.
    pub(super) fn quick_filter_settings(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        ui.label(
            RichText::new(t("The funnel menu of a file window shows only the kinds of file you tick. Folders stay visible, and the filter stays on while you browse. Click a filter to edit it, or drag its handle to reorder."))
                .size(12.0)
                .color(th.weak),
        );
        ui.add_space(6.0);
        let mut frame = card_frame(th).begin(ui);
        {
            let ui = &mut frame.content_ui;
            ui.set_width(ui.available_width());
            changed |= self.quick_filter_buttons(ui, th);
            ui.add_space(4.0);
            changed |= self.quick_filter_list(ui, th);
        }
        frame.end(ui);
        changed
    }

    /// New filter, and restoring the built-in ones (asks first: it resets their extensions).
    fn quick_filter_buttons(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            let new = egui::Button::new(RichText::new(format!("+ {}", t("New quick filter"))).color(th.on_accent())).fill(th.accent);
            if ui.add(new).clicked() {
                let name = unused_quick_filter_name(&self.cfg.quick_filters, &t("New filter"));
                self.cfg.quick_filters.push(QuickFilter::new(&name, ""));
                self.qf = QfUi { open: Some(self.cfg.quick_filters.len() - 1), focus_name: true, ..Default::default() };
                changed = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!self.qf.restore, egui::Button::new(t("Restore the built-in filters…"))).clicked() {
                    self.qf.restore = true;
                }
            });
        });
        if self.qf.restore {
            ui.add_space(4.0);
            ui.label(
                RichText::new(t("Adds the built-in filters that are missing and resets the extensions of the built-in ones. Your own filters are kept."))
                    .size(12.0)
                    .color(th.weak),
            );
            ui.horizontal(|ui| {
                if ui.button(RichText::new(t("Restore")).color(th.danger)).clicked() {
                    for d in default_quick_filters() {
                        match self.cfg.quick_filters.iter_mut().find(|q| q.name == d.name) {
                            Some(q) => q.exts = d.exts,
                            None => self.cfg.quick_filters.push(d),
                        }
                    }
                    self.qf.restore = false;
                    changed = true;
                }
                if ui.button(t("Cancel")).clicked() {
                    self.qf.restore = false;
                }
            });
        }
        changed
    }

    /// The filters: a row each, the open one with its editor below.
    fn quick_filter_list(&mut self, ui: &mut egui::Ui, th: &Theme) -> bool {
        let mut changed = false;
        let n = self.cfg.quick_filters.len();
        if n == 0 {
            ui.add_space(6.0);
            ui.label(RichText::new(t("No quick filters yet")).color(th.weak));
            return false;
        }
        let mut edit: Option<QfEdit> = None;
        // Each filter's place (row and editor) and its handle, to drop a dragged one.
        let mut blocks: Vec<Rect> = Vec::with_capacity(n);
        let mut grips: Vec<(usize, egui::Response)> = Vec::new();
        let w = ui.available_width();
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in 0..n {
            let top = ui.cursor().top();
            if i > 0 {
                let y = top;
                ui.painter().line_segment([pos2(ui.max_rect().left(), y), pos2(ui.max_rect().left() + w, y)], Stroke::new(1.0_f32, th.border));
            }
            let (grip, row_edit) = self.quick_filter_row(ui, th, i, w);
            grips.push((i, grip));
            if row_edit.is_some() {
                edit = row_edit;
            }
            if self.qf.open == Some(i) {
                changed |= self.quick_filter_editor(ui, th, i);
            }
            blocks.push(Rect::from_min_max(pos2(ui.max_rect().left(), top), pos2(ui.max_rect().left() + w, ui.cursor().top())));
        }
        // A dragged handle: where the filter would go, and the move when it is let go.
        if let Some((from, grip)) = grips.iter().find(|(_, g)| g.dragged() || g.drag_stopped()) {
            let from = *from;
            let to = ui.ctx().pointer_latest_pos().map(|p| blocks.iter().position(|b| p.y < b.center().y).unwrap_or(n));
            let to = to.filter(|t| *t != from && *t != from + 1);
            ui.painter().rect_stroke(blocks[from].shrink(1.0), 6.0, Stroke::new(1.0_f32, th.accent), egui::StrokeKind::Inside);
            if let Some(to) = to {
                let y = match to {
                    0 => blocks[0].top(),
                    k if k == n => blocks[n - 1].bottom(),
                    k => blocks[k].top(),
                };
                let x0 = blocks[0].left();
                ui.painter().circle_stroke(pos2(x0 + 3.0, y), 3.0, Stroke::new(2.0_f32, th.accent));
                ui.painter().line_segment([pos2(x0 + 6.0, y), pos2(blocks[0].right(), y)], Stroke::new(2.0_f32, th.accent));
            }
            let name = self.cfg.quick_filters[from].display_name();
            drag_feedback(ui, grip, &name);
            if grip.drag_stopped() {
                if let Some(to) = to {
                    edit = Some(QfEdit::Move(from, to));
                }
            }
        }
        match edit {
            Some(QfEdit::Move(from, to)) => {
                let order = moved(n, from, to);
                let old = std::mem::take(&mut self.cfg.quick_filters);
                let mut old: Vec<Option<QuickFilter>> = old.into_iter().map(Some).collect();
                self.cfg.quick_filters = order.iter().filter_map(|&k| old[k].take()).collect();
                // The open editor stays with its filter.
                self.qf.open = self.qf.open.and_then(|o| order.iter().position(|&k| k == o));
                self.qf.delete = None;
                changed = true;
            }
            Some(QfEdit::Duplicate(i)) => {
                let mut q = self.cfg.quick_filters[i].clone();
                q.name = unused_quick_filter_name(&self.cfg.quick_filters, &q.name);
                self.cfg.quick_filters.insert(i + 1, q);
                self.qf = QfUi { open: Some(i + 1), focus_name: true, ..Default::default() };
                changed = true;
            }
            Some(QfEdit::Remove(i)) => {
                self.cfg.quick_filters.remove(i);
                self.qf.open = match self.qf.open {
                    Some(o) if o == i => None,
                    Some(o) if o > i => Some(o - 1),
                    o => o,
                };
                self.qf.delete = None;
                changed = true;
            }
            None => {}
        }
        changed
    }

    /// One filter's row: handle, icon, name, its extensions, and open / remove. Returns the
    /// handle (for dragging) and what the row asks for.
    fn quick_filter_row(&mut self, ui: &mut egui::Ui, th: &Theme, i: usize, w: f32) -> (egui::Response, Option<QfEdit>) {
        let n = self.cfg.quick_filters.len();
        let q = &self.cfg.quick_filters[i];
        let exts = q.ext_list();
        let open = self.qf.open == Some(i);
        let asking = self.qf.delete == Some(i);
        let id = egui::Id::unique(("qf-row", i));
        let (row, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
        let label = q.display_name();
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::DisclosureTriangle, true, open, &label));
        let mut edit = None;

        // The handle, and the buttons on the right: drawn over the row, so they take its clicks.
        let gr = Rect::from_center_size(pos2(row.left() + 9.0, row.center().y), vec2(14.0, 26.0));
        let grip = ui.interact(gr, id.with("grip"), Sense::drag());
        // The buttons' place (their clicks are theirs), and where the text must end.
        let right = Rect::from_min_max(pos2(row.right() - if asking { 170.0 } else { 34.0 }, row.top()), row.right_bottom());
        let text_end = row.right() - if asking { 170.0 } else { 64.0 };
        // Background (under the buttons): hover and the open filter.
        let pointer = ui.ctx().pointer_hover_pos();
        let on_buttons = pointer.is_some_and(|p| right.contains(p));
        if open || (resp.hovered() && !on_buttons) || grip.dragged() {
            let fill = if open { th.selection.gamma_multiply(0.7) } else { th.hover() };
            ui.painter().rect_filled(row.shrink2(vec2(0.0, 2.0)), 6.0, fill);
        }
        if asking {
            let mut right_ui =
                ui.new_child(egui::UiBuilder::new().max_rect(right.shrink2(vec2(4.0, 7.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
            if right_ui.button(t("Cancel")).clicked() {
                self.qf.delete = None;
            }
            let remove = right_ui.button(RichText::new(t("Remove")).color(th.danger));
            if remove.clicked() {
                edit = Some(QfEdit::Remove(i));
            }
        } else {
            let tr = Rect::from_center_size(pos2(row.right() - 18.0, row.center().y), vec2(26.0, 26.0));
            let trash = ui.interact(tr, id.with("remove"), Sense::click());
            trash.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, t("Remove")));
            if trash.hovered() {
                ui.painter().rect_filled(tr, 6.0, th.hover());
            }
            icons::mi_trash(ui.painter(), tr.shrink(6.0), if trash.hovered() { th.danger } else { th.weak });
            if trash.on_hover_text(t("Remove")).clicked() {
                self.qf.delete = Some(i);
            }
        }

        let dots = if grip.hovered() || grip.dragged() { th.text } else { th.weak.gamma_multiply(0.8) };
        grip_dots(ui.painter(), gr.center(), dots);
        let grip = if grip.dragged() { grip } else { grip.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text(t("Drag to reorder")) };
        crate::ui::pane::quick_filter_icon(ui.painter(), Rect::from_center_size(pos2(row.left() + 34.0, row.center().y), vec2(20.0, 20.0)), &exts, th);
        let name_font = bold(13.5);
        let mut job = egui::text::LayoutJob::simple_singleline(label.clone(), name_font, if q.name.trim().is_empty() { th.weak } else { th.strong() });
        job.wrap = egui::text::TextWrapping::truncate_at_width(NAME_W - 8.0);
        let g = ui.fonts_mut(|f| f.layout_job(job));
        ui.painter().galley(pos2(row.left() + NAME_X, row.center().y - g.size().y / 2.0), g, th.strong());
        // The extensions, as many as fit; or why the filter shows nothing.
        let ex = row.left() + NAME_X + NAME_W;
        let room = (text_end - 8.0 - ex).max(0.0);
        if !asking && room > 40.0 {
            let font = FontId::monospace(11.5);
            let (text, col) = if exts.is_empty() { (t("No extensions yet"), th.danger) } else { (ext_summary(ui, &exts, room, &font), th.weak) };
            ui.painter().text(pos2(ex, row.center().y), Align2::LEFT_CENTER, text, font, col);
        }
        if asking {
            let msg = t("Remove “{name}”?").replace("{name}", &label);
            let mut job = egui::text::LayoutJob::simple_singleline(msg, FontId::proportional(12.5), th.danger);
            job.wrap = egui::text::TextWrapping::truncate_at_width((text_end - 8.0 - ex).max(20.0));
            let g = ui.fonts_mut(|f| f.layout_job(job));
            ui.painter().galley(pos2(ex, row.center().y - g.size().y / 2.0), g, th.danger);
        } else {
            // Open or closed: a chevron by the trash can.
            icons::chevron(ui.painter(), Rect::from_center_size(pos2(row.right() - 46.0, row.center().y), vec2(18.0, 18.0)), if open { 0 } else { 2 }, th.weak);
        }

        let tip = if exts.is_empty() { t("Click to edit") } else { format!("{} {}\n{}", exts.len(), t("extensions"), t("Click to edit")) };
        let resp = resp.on_hover_text(tip);
        if resp.clicked() && !on_buttons {
            self.qf.open = if open { None } else { Some(i) };
            self.qf.add.clear();
            self.qf.delete = None;
        }
        resp.kept_context_menu(|ui| {
            if ui.add_enabled(i > 0, egui::Button::new(t("Move up"))).clicked() {
                edit = Some(QfEdit::Move(i, i - 1));
                ui.close_kind(egui::UiKind::Menu);
            }
            if ui.add_enabled(i + 1 < n, egui::Button::new(t("Move down"))).clicked() {
                edit = Some(QfEdit::Move(i, i + 2));
                ui.close_kind(egui::UiKind::Menu);
            }
            if ui.button(t("Duplicate")).clicked() {
                edit = Some(QfEdit::Duplicate(i));
                ui.close_kind(egui::UiKind::Menu);
            }
            ui.separator();
            if ui.button(RichText::new(t("Remove…")).color(th.danger)).clicked() {
                self.qf.delete = Some(i);
                ui.close_kind(egui::UiKind::Menu);
            }
        });
        (grip, edit)
    }

    /// The editor of filter `i`: its name, and its extensions as chips with an add field.
    fn quick_filter_editor(&mut self, ui: &mut egui::Ui, th: &Theme, i: usize) -> bool {
        let mut changed = false;
        // Two filters that read the same in the menu (a built-in one is shown translated).
        let taken = {
            let name = self.cfg.quick_filters[i].display_name();
            self.cfg.quick_filters.iter().enumerate().any(|(k, q)| k != i && q.display_name().trim() == name.trim())
        };
        let old_name = self.cfg.quick_filters[i].name.clone();
        let builtin = default_quick_filters().into_iter().find(|d| d.name == self.cfg.quick_filters[i].name);
        let q = &mut self.cfg.quick_filters[i];
        let mut exts = q.ext_list();
        let inset = NAME_X - 6.0;
        ui.horizontal(|ui| {
            ui.add_space(inset);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t("Name")).color(th.weak));
                    // The name as the menu shows it: a built-in one in the language of the
                    // app; it is stored only once it is changed (then it is the user's own).
                    let mut shown = if q.name.trim().is_empty() { q.name.clone() } else { t(&q.name) };
                    let r = ui.add(egui::TextEdit::singleline(&mut shown).desired_width(220.0).hint_text(t("Name")));
                    if r.changed() {
                        q.name = shown.clone();
                    }
                    if std::mem::take(&mut self.qf.focus_name) {
                        r.request_focus();
                        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(shown.chars().count()));
                        if let Some(mut s) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                            s.cursor.set_char_range(Some(all));
                            s.store(ui.ctx(), r.id);
                        }
                    }
                    changed |= r.changed();
                    if q.name.trim().is_empty() {
                        ui.label(RichText::new(t("A filter needs a name")).size(12.0).color(th.danger));
                    } else if taken {
                        ui.label(RichText::new(t("Another quick filter has this name")).size(12.0).color(th.danger));
                    }
                });
                ui.label(RichText::new(format!("{} ({})", t("Extensions"), exts.len())).color(th.weak));
                // The extensions: a chip each, × removes it.
                let mut remove: Option<usize> = None;
                if !exts.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        for (k, e) in exts.iter().enumerate() {
                            if ext_chip(ui, th, e, egui::Id::unique(("qf-chip", i, k))) {
                                remove = Some(k);
                            }
                        }
                    });
                }
                if let Some(k) = remove {
                    exts.remove(k);
                    q.exts = exts.join(" ");
                    changed = true;
                }
                // Adding: one or several, as typed or pasted.
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.qf.add).desired_width(260.0).hint_text(t("Add extensions, e.g. heic or jpg png")));
                    let r = r.on_hover_text(t("Extensions separated by spaces or commas (dots are optional)"));
                    let enter = r.lost_focus() && ui.input(|inp| inp.key_pressed(Key::Enter));
                    let add = ui.add_enabled(!self.qf.add.trim().is_empty(), egui::Button::new(t("Add"))).clicked();
                    if (enter || add) && !self.qf.add.trim().is_empty() {
                        let new = QuickFilter::new("", &self.qf.add).ext_list();
                        let before = exts.len();
                        for e in new {
                            if !exts.contains(&e) {
                                exts.push(e);
                            }
                        }
                        if exts.len() != before {
                            q.exts = exts.join(" ");
                            changed = true;
                        }
                        self.qf.add.clear();
                        r.request_focus();
                    }
                    if let Some(d) = &builtin {
                        if d.ext_list() != exts {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(t("Reset to built-in")).on_hover_text(t("Back to the extensions this filter comes with")).clicked() {
                                    q.exts = d.exts.clone();
                                    changed = true;
                                }
                            });
                        }
                    }
                });
                ui.add_space(8.0);
            });
        });
        // Renamed: the windows that show this filter keep it on under its new name.
        let new_name = self.cfg.quick_filters[i].name.clone();
        if new_name != old_name {
            rename_in_windows(self, &old_name, &new_name);
        }
        changed
    }
}

/// Windows with filter `old` on (ticked in their menu) now have it as `new`.
fn rename_in_windows(app: &mut App, old: &str, new: &str) {
    for p in all_panes!(app) {
        if let Some(q) = p.tab_mut().quick.as_mut() {
            for n in q.names.iter_mut().filter(|n| *n == old) {
                *n = new.to_string();
            }
        }
    }
}

/// An extension as a chip: ".jpg ×". True when its × is clicked.
fn ext_chip(ui: &mut egui::Ui, th: &Theme, ext: &str, id: egui::Id) -> bool {
    let font = FontId::monospace(11.5);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(format!(".{ext}"), font, th.text));
    let size = vec2(g.size().x + 32.0, 22.0);
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    let xr = Rect::from_center_size(pos2(r.right() - 11.0, r.center().y), vec2(16.0, 16.0));
    let x = ui.interact(xr, id, Sense::click());
    x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} .{ext}", t("Remove"))));
    let fill = if x.hovered() { th.danger.gamma_multiply(0.15) } else { th.surface };
    ui.painter().rect(r, 11.0, fill, Stroke::new(1.0_f32, th.border), egui::StrokeKind::Inside);
    ui.painter().galley(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, th.text);
    let c = if x.hovered() { th.danger } else { th.weak };
    let s = xr.shrink(4.5);
    ui.painter().line_segment([s.left_top(), s.right_bottom()], Stroke::new(1.3_f32, c));
    ui.painter().line_segment([s.right_top(), s.left_bottom()], Stroke::new(1.3_f32, c));
    x.on_hover_text(format!("{} .{ext}", t("Remove"))).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_filter_keeps_the_others_in_order() {
        assert_eq!(moved(4, 0, 2), [1, 0, 2, 3], "down: before the one at 2");
        assert_eq!(moved(4, 3, 0), [3, 0, 1, 2], "up to the top");
        assert_eq!(moved(4, 1, 4), [0, 2, 3, 1], "to the end");
        assert_eq!(moved(3, 1, 1), [0, 1, 2], "its own place");
    }
}
