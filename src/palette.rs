//! Command palette (Ctrl+K): fuzzy search over commands, bookmarks, drives,
//! themes, languages and saved connections.
use crate::config::ConnectionProfile;
use crate::i18n::t;
use crate::icons;
use crate::pane::Cmd;
use crate::theme::Theme;
use egui::{pos2, vec2, Align2, FontId, Key, Modifiers, Rect, Sense};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    NewTab,
    CloseTab,
    SwapPanels,
    SameFolder,
    QuickView,
    ToggleHidden,
    ToggleSidebar,
    ToggleHints,
    Connect,
    Backup,
    Transfers,
    Settings,
    Shortcuts,
    About,
    Bookmarks,
    DefaultApps,
    InvertSelection,
    DeselectAll,
    ExportSettings,
    ImportSettings,
    Quit,
    NewTerminalTab,
    ReopenClosedTab,
    GroupTab,
    UngroupTab,
    PinTab,
    DuplicateTab,
    WindowToNewTab,
}

#[derive(Clone)]
pub enum PalItem {
    Cmd(Cmd),
    Act(Act),
    Theme(&'static str),
    Lang(String),
    Go(String),
    Connect(ConnectionProfile),
    /// Filter the active folder by the query.
    FilterHere(String),
    /// Search every folder below the active one.
    SearchAll(String),
    /// Switch to a window: (tab uid, window's view uid).
    Tab(u64, u64),
    /// Quick filter of the focused file window (a category, or off).
    QuickFilter(Option<usize>),
}

pub struct Entry {
    pub label: String,
    pub hint: String,
    pub item: PalItem,
    /// Always listed first, in order, whatever the query.
    pub pinned: bool,
}

impl Entry {
    pub fn new(label: impl Into<String>, hint: impl Into<String>, item: PalItem) -> Self {
        Self { label: label.into(), hint: hint.into(), item, pinned: false }
    }
    pub fn pinned(label: impl Into<String>, hint: impl Into<String>, item: PalItem) -> Self {
        Self { pinned: true, ..Self::new(label, hint, item) }
    }
}

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    query: String,
    selected: usize,
    focus: bool,
    /// Set on the frame the palette opens, so the click that opened it
    /// (e.g. on the toolbar search box) is not taken as a click outside.
    just_opened: bool,
}

/// Fuzzy score: substring matches rank first, then in-order subsequences.
pub fn score(label: &str, query: &str) -> Option<i32> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let l = label.to_lowercase();
    if let Some(pos) = l.find(&q) {
        let word_start = pos == 0 || !l[..pos].chars().last().is_some_and(|c| c.is_alphanumeric());
        return Some(1000 - pos as i32 + if word_start { 300 } else { 0 } - l.len() as i32 / 8);
    }
    let mut chars = l.chars();
    let mut gaps = 0;
    for qc in q.chars().filter(|c| !c.is_whitespace()) {
        let mut found = false;
        for c in chars.by_ref() {
            if c == qc {
                found = true;
                break;
            }
            gaps += 1;
        }
        if !found {
            return None;
        }
    }
    Some(500 - gaps)
}

impl Palette {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.query.clear();
            self.selected = 0;
            self.focus = true;
            self.just_opened = true;
        }
    }

    /// Open with `q` already typed (Ctrl+F: edit the current filter).
    pub fn open_with(&mut self, q: &str) {
        self.open = true;
        self.query = q.to_string();
        self.selected = 0;
        self.focus = true;
        self.just_opened = true;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn show(&mut self, ctx: &egui::Context, th: &Theme, entries: Vec<Entry>) -> Option<PalItem> {
        if !self.open {
            return None;
        }
        // Navigation keys are taken before the text field sees them.
        let (up, down, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if esc {
            self.open = false;
            return None;
        }
        let (pinned, rest): (Vec<Entry>, Vec<Entry>) = entries.into_iter().partition(|e| e.pinned);
        let mut ranked: Vec<(i32, Entry)> = rest.into_iter().filter_map(|e| score(&e.label, &self.query).map(|s| (s, e))).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0));
        let n_pinned = pinned.len();
        let results: Vec<Entry> = pinned.into_iter().chain(ranked.into_iter().map(|(_, e)| e).take(60)).collect();
        if down {
            self.selected = (self.selected + 1).min(results.len().saturating_sub(1));
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        self.selected = self.selected.min(results.len().saturating_sub(1));

        let mut chosen: Option<PalItem> = None;
        let screen = ctx.screen_rect();
        let width = (screen.width() - 48.0).min(600.0);
        let area = egui::Area::new(egui::Id::new("palette"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, 64.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(th.panel)
                    .corner_radius(14)
                    .inner_margin(egui::Margin::same(10))
                    .stroke(egui::Stroke::new(1.0_f32, th.border))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(22.0, 30.0), Sense::hover());
                            icons::search(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), th.weak);
                            let edit = egui::TextEdit::singleline(&mut self.query)
                                .id(egui::Id::new("palette-input"))
                                .frame(false)
                                .font(FontId::proportional(16.0))
                                .hint_text(egui::RichText::new(t("Search files, commands, folders, settings…")).color(th.weak))
                                .desired_width(f32::INFINITY);
                            let resp = ui.add(edit);
                            if self.focus {
                                resp.request_focus();
                                self.focus = false;
                            }
                            if resp.changed() {
                                self.selected = 0;
                            }
                        });
                        ui.add_space(4.0);
                        let r = ui.available_rect_before_wrap();
                        ui.painter().hline(r.x_range(), r.top(), egui::Stroke::new(1.0_f32, th.border));
                        ui.add_space(6.0);
                        if results.is_empty() {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(t("No matches")).color(th.weak));
                            ui.add_space(8.0);
                        }
                        egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for (i, e) in results.iter().enumerate() {
                                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                                let sel = i == self.selected;
                                if sel {
                                    ui.painter().rect_filled(rect, 7.0, th.selection);
                                    if down || up {
                                        ui.scroll_to_rect(rect, None);
                                    }
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(rect, 7.0, th.hover());
                                }
                                let p = ui.painter();
                                // Category prefix ("Theme: …") in weak colour.
                                let (cat, rest) = match e.label.split_once(": ") {
                                    Some((c, r)) => (format!("{c}: "), r.to_string()),
                                    None => (String::new(), e.label.clone()),
                                };
                                let font = FontId::proportional(14.0);
                                let mut x = rect.left() + 12.0;
                                if e.pinned {
                                    icons::search(p, Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 14.0)), th.accent);
                                    x += 22.0;
                                }
                                if !cat.is_empty() {
                                    let g = p.layout_no_wrap(cat, font.clone(), th.weak);
                                    let w = g.size().x;
                                    p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, th.weak);
                                    x += w;
                                }
                                p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, rest, font, if sel { th.strong() } else { th.text });
                                if !e.hint.is_empty() {
                                    keycap(p, th, pos2(rect.right() - 10.0, rect.center().y), &e.hint);
                                }
                                if resp.clicked() {
                                    chosen = Some(e.item.clone());
                                }
                                if i + 1 == n_pinned && results.len() > n_pinned {
                                    // Separate the file search from the commands.
                                    ui.add_space(3.0);
                                    let r = ui.available_rect_before_wrap();
                                    ui.painter().hline(r.x_range().shrink(8.0), r.top(), egui::Stroke::new(1.0_f32, th.border));
                                    ui.add_space(4.0);
                                }
                            }
                        });
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(t("↑↓ to navigate · Enter to run · Esc to close")).small().color(th.weak));
                    });
            });
        if enter {
            chosen = results.get(self.selected).map(|e| e.item.clone());
        }
        // Clicking outside closes the palette.
        let clicked_outside = !std::mem::take(&mut self.just_opened)
            && ctx.input(|i| i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
        if chosen.is_some() || clicked_outside {
            self.open = false;
        }
        chosen
    }
}

/// Draw a keyboard-shortcut chip right-aligned at `right_center`. Returns its rect.
pub fn keycap(p: &egui::Painter, th: &Theme, right_center: egui::Pos2, text: &str) -> Rect {
    let g = p.layout_no_wrap(text.to_string(), FontId::proportional(11.0), th.weak);
    let size = g.size() + vec2(12.0, 6.0);
    let rect = Rect::from_min_size(pos2(right_center.x - size.x, right_center.y - size.y / 2.0), size);
    p.rect(rect, 5.0, th.surface, egui::Stroke::new(1.0_f32, th.border), egui::StrokeKind::Inside);
    p.galley(rect.min + vec2(6.0, 3.0), g, th.weak);
    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy() {
        assert!(score("Copy to other panel", "copy").unwrap() > score("Swap panels", "copy").unwrap_or(-1));
        assert!(score("Theme: Nord", "nord").is_some());
        assert!(score("Disk usage", "dsk").is_some());
        assert!(score("Disk usage", "xyz").is_none());
        assert!(score("New folder", "new f").is_some());
    }
}
