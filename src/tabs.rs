//! The window's single tab bar. Each tab holds one or more mini windows (file
//! views and terminals, see `layout`); tabs can be pinned, reordered by drag &
//! drop and kept in Chrome-style tab groups (named, coloured, collapsible; the
//! tabs of a group are always kept together). A mini window dragged onto the
//! bar becomes a tab of its own, or moves into the tab it is dropped on.
use crate::i18n::t;
use crate::icons;
use crate::layout::Layout;
use crate::pane::Pane;
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static UID: AtomicU64 = AtomicU64::new(1);
static GROUP_ID: AtomicU32 = AtomicU32::new(1);

pub fn next_uid() -> u64 {
    UID.fetch_add(1, Ordering::Relaxed)
}

pub fn next_group_id() -> u32 {
    GROUP_ID.fetch_add(1, Ordering::Relaxed)
}

/// Tab-group colours: the d3 / matplotlib "category10" palette.
pub const GROUP_COLORS: [(&str, u32); 10] = [
    ("Blue", 0x1f77b4),
    ("Orange", 0xff7f0e),
    ("Green", 0x2ca02c),
    ("Red", 0xd62728),
    ("Purple", 0x9467bd),
    ("Brown", 0x8c564b),
    ("Pink", 0xe377c2),
    ("Grey", 0x7f7f7f),
    ("Olive", 0xbcbd22),
    ("Cyan", 0x17becf),
];

pub fn group_color(i: u8, _dark: bool) -> Color32 {
    let (_, c) = GROUP_COLORS[i as usize % GROUP_COLORS.len()];
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Background of a grouped tab: the group colour mixed into the bar.
fn group_tint(col: Color32, dark: bool, selected: bool, hovered: bool) -> Color32 {
    let a = match (selected, hovered) {
        (true, _) => if dark { 0.55 } else { 0.42 },
        (false, true) => if dark { 0.36 } else { 0.28 },
        (false, false) => if dark { 0.24 } else { 0.17 },
    };
    col.gamma_multiply(a)
}

/// Black or white, whichever reads better on `bg`.
fn text_on(bg: Color32) -> Color32 {
    let l = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if l > 150.0 { Color32::from_rgb(0x1b, 0x1c, 0x20) } else { Color32::WHITE }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TabGroup {
    pub id: u32,
    pub name: String,
    pub color: u8,
    pub collapsed: bool,
}

/// Requests from the tab bar that need the app (creating, closing or moving tabs).
#[derive(Debug, Clone)]
pub enum TabEvent {
    NewFiles { at: usize, group: Option<u32> },
    NewTerminal { at: usize, group: Option<u32>, profile: Option<usize> },
    Close(Vec<u64>),
    Duplicate(u64),
    /// The shown tab was clicked: give its focused window the keyboard.
    Focus,
    /// Show this tab's windows beside the shown tab's (merge the two tabs).
    MergeIntoShown(u64),
    /// A mini window dropped between tabs: it becomes a tab of its own.
    Detach { view: u64, at: usize, group: Option<u32> },
    /// A mini window dropped onto another tab: it moves into that tab.
    WindowInto { view: u64, tab: u64 },
    ReopenClosed,
}

/// What is dragged when a tab is dragged.
#[derive(Clone, Copy, Debug)]
pub struct TabDrag {
    pub uid: u64,
}

/// What is dragged when a mini window is dragged by its grip (the uid of its view).
#[derive(Clone, Copy, Debug)]
pub struct WinDrag {
    pub uid: u64,
}

/// How a tab looks, worked out by the app (the shown tab's windows live there).
pub struct TabInfo {
    pub title: String,
    pub default_title: String,
    pub tooltip: String,
    /// The focused window is a terminal (`true`: its shell has exited).
    pub term: Option<bool>,
    /// The focused window is a disk usage analysis.
    pub disk_usage: bool,
    /// The focused window is the Settings window.
    pub settings: bool,
    /// The focused window is the Transfers window.
    pub transfers: bool,
    /// A terminal of the tab rang the bell / printed something while hidden.
    pub bell: bool,
    pub activity: bool,
    /// Window rectangles in a unit square, the focused one flagged (for the icon).
    pub cells: Vec<(Rect, bool)>,
}

/// What the tab bar needs from the app.
pub struct BarCtx<'a> {
    pub theme: &'a crate::theme::Theme,
    pub terminal_profiles: &'a [String],
    pub can_reopen: bool,
    /// Uid of the view being dragged by its grip and its window count (for drop targets).
    pub dragged_window: Option<(u64, usize)>,
}

// ---------------------------------------------------------------- model

/// One tab of the tab bar: one or more mini windows (file views or terminals)
/// tiled by `layout`, at most 2 × 2.
pub struct Workspace {
    /// Stable identity (survives reordering).
    pub uid: u64,
    /// Tab group this tab belongs to.
    pub group: Option<u32>,
    pub pinned: bool,
    /// Name set by the user ("Rename tab").
    pub custom_title: Option<String>,
    /// The mini windows. Empty while this is the shown tab: the app holds the
    /// shown tab's windows itself (see `App::sync_tab`).
    pub panes: Vec<Pane>,
    pub layout: Layout,
    /// Focused window, and the one focused before it ("other panel").
    pub active: usize,
    pub prev_active: usize,
}

impl Workspace {
    pub fn new(panes: Vec<Pane>, layout: Layout) -> Self {
        Workspace { uid: next_uid(), group: None, pinned: false, custom_title: None, panes, layout, active: 0, prev_active: 0 }
    }

    pub fn single(pane: Pane) -> Self {
        Self::new(vec![pane], Layout::Leaf(0))
    }
}

/// The tab bar's tabs: order, pinning, groups and the shown tab.
pub struct Strip {
    pub tabs: Vec<Workspace>,
    pub active: usize,
    pub groups: Vec<TabGroup>,
}

impl Strip {
    pub fn index_of(&self, uid: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.uid == uid)
    }

    pub fn group(&self, id: u32) -> Option<&TabGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_mut(&mut self, id: u32) -> Option<&mut TabGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// First and last index of the group's tabs.
    pub fn group_range(&self, id: u32) -> Option<(usize, usize)> {
        let first = self.tabs.iter().position(|t| t.group == Some(id))?;
        let last = self.tabs.iter().rposition(|t| t.group == Some(id))?;
        Some((first, last))
    }

    /// Hidden inside a collapsed group?
    pub fn is_hidden(&self, i: usize) -> bool {
        self.tabs[i].group.and_then(|g| self.group(g)).is_some_and(|g| g.collapsed)
    }

    pub fn activate(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if let Some(g) = self.tabs[i].group {
            if let Some(g) = self.group_mut(g) {
                g.collapsed = false;
            }
        }
        self.active = i;
    }

    /// Insert `tab` at `at` (clamped) into `group`, and make it active.
    pub fn insert_tab(&mut self, at: usize, mut tab: Workspace, group: Option<u32>) {
        tab.group = group.filter(|g| self.group(*g).is_some());
        let uid = tab.uid;
        self.tabs.insert(at.min(self.tabs.len()), tab);
        self.normalize_keep(self.tabs[self.active.min(self.tabs.len() - 1)].uid);
        if let Some(i) = self.index_of(uid) {
            self.activate(i);
        }
    }

    /// Remove tab `i` and pick the next active tab (the right neighbour, as in Chrome).
    /// The caller makes sure the strip never ends up without tabs.
    pub fn remove_tab(&mut self, i: usize) -> Workspace {
        let was_active = i == self.active;
        let tab = self.tabs.remove(i);
        if self.tabs.is_empty() {
            self.active = 0;
            self.groups.clear();
            return tab;
        }
        if was_active {
            let mut n = i.min(self.tabs.len() - 1);
            // Prefer a visible tab next to it.
            if self.is_hidden(n) {
                if let Some(v) = (0..self.tabs.len()).filter(|&k| !self.is_hidden(k)).min_by_key(|&k| k.abs_diff(i)) {
                    n = v;
                }
            }
            self.active = n;
            self.activate(n);
        } else if i < self.active {
            self.active -= 1;
        }
        let uid = self.tabs[self.active].uid;
        self.normalize_keep(uid);
        tab
    }

    /// Move tab `from` so it lands before the tab now at `to` (`to` may be
    /// `tabs.len()`), and put it into `group`.
    pub fn move_tab(&mut self, from: usize, to: usize, group: Option<u32>) {
        if from >= self.tabs.len() {
            return;
        }
        let active = self.tabs[self.active].uid;
        let mut tab = self.tabs.remove(from);
        tab.group = group.filter(|g| self.group(*g).is_some());
        if tab.group.is_some() {
            tab.pinned = false;
        }
        let to = if to > from { to - 1 } else { to };
        self.tabs.insert(to.min(self.tabs.len()), tab);
        self.normalize_keep(active);
    }

    /// Group for a tab dropped at insertion index `to` (between `to - 1` and `to`).
    pub fn drop_group(&self, moving: u64, to: usize) -> Option<u32> {
        let neighbour = |i: Option<usize>| i.and_then(|i| self.tabs.get(i)).filter(|t| t.uid != moving).and_then(|t| t.group);
        let left = neighbour(to.checked_sub(1)).or_else(|| neighbour(to.checked_sub(2)).filter(|_| to >= 1 && self.tabs[to - 1].uid == moving));
        let right = neighbour(Some(to)).or_else(|| neighbour(Some(to + 1)).filter(|_| self.tabs.get(to).is_some_and(|t| t.uid == moving)));
        match (left, right) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => {
                let own = self.index_of(moving).and_then(|i| self.tabs[i].group);
                own.filter(|g| left == Some(*g) || right == Some(*g))
            }
        }
    }

    /// Put tab `i` into a new group and return its id.
    pub fn new_group(&mut self, i: usize) -> u32 {
        let used: HashSet<u8> = self.groups.iter().map(|g| g.color).collect();
        let color = (0..GROUP_COLORS.len() as u8).find(|c| !used.contains(c)).unwrap_or((self.groups.len() % GROUP_COLORS.len()) as u8);
        let id = next_group_id();
        self.groups.push(TabGroup { id, name: String::new(), color, collapsed: false });
        self.set_tab_group(i, Some(id));
        id
    }

    /// Move tab `i` into `group` (to the end of the group) or out of its group.
    pub fn set_tab_group(&mut self, i: usize, group: Option<u32>) {
        if i >= self.tabs.len() {
            return;
        }
        let old = self.tabs[i].group;
        if old == group {
            return;
        }
        let to = match (group, old) {
            (Some(g), _) => self.group_range(g).map(|(_, last)| last + 1).unwrap_or(i + 1),
            // Leaving a group: place it right after the group, as Chrome does.
            (None, Some(g)) => self.group_range(g).map(|(_, last)| last + 1).unwrap_or(i),
            (None, None) => i,
        };
        self.move_tab(i, to, group);
    }

    pub fn toggle_collapsed(&mut self, id: u32) {
        let Some(collapsed) = self.group(id).map(|g| g.collapsed) else { return };
        if !collapsed && self.tabs[self.active].group == Some(id) {
            // Move focus out of the group first; a group holding every tab stays open.
            let i = self.active;
            match (0..self.tabs.len()).filter(|&k| self.tabs[k].group != Some(id) && !self.is_hidden(k)).min_by_key(|&k| k.abs_diff(i)) {
                Some(k) => self.active = k,
                None => return,
            }
        }
        if let Some(g) = self.group_mut(id) {
            g.collapsed = !collapsed;
        }
    }

    pub fn ungroup(&mut self, id: u32) {
        for t in &mut self.tabs {
            if t.group == Some(id) {
                t.group = None;
            }
        }
        self.groups.retain(|g| g.id != id);
    }

    pub fn toggle_pin(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        let uid = self.tabs[self.active].uid;
        let pin = !self.tabs[i].pinned;
        self.tabs[i].pinned = pin;
        if pin {
            self.tabs[i].group = None;
            // Pinned tabs go to the end of the pinned area.
            let tab = self.tabs.remove(i);
            let at = self.tabs.iter().take_while(|t| t.pinned).count();
            self.tabs.insert(at, tab);
        }
        self.normalize_keep(uid);
    }

    /// Next/previous visible tab (Ctrl+Tab / Ctrl+Shift+Tab).
    pub fn cycle(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        let mut i = self.active as isize;
        for _ in 0..n {
            i = (i + delta).rem_euclid(n);
            if !self.is_hidden(i as usize) {
                self.activate(i as usize);
                return;
            }
        }
    }

    /// Activate the n-th visible tab (Ctrl/Alt+1..8); `usize::MAX` = the last one.
    pub fn go_to(&mut self, n: usize) {
        let visible: Vec<usize> = (0..self.tabs.len()).filter(|&i| !self.is_hidden(i)).collect();
        let pick = if n == usize::MAX { visible.last() } else { visible.get(n) };
        if let Some(&i) = pick {
            self.activate(i);
        }
    }

    /// Move the active tab left/right by one (Ctrl+Shift+PageUp/PageDown).
    pub fn shift_active(&mut self, delta: isize) {
        let i = self.active;
        let j = i as isize + delta;
        if j < 0 || j >= self.tabs.len() as isize {
            return;
        }
        let j = j as usize;
        if self.tabs[i].pinned != self.tabs[j].pinned {
            return;
        }
        let uid = self.tabs[i].uid;
        let group = self.tabs[i].group;
        if group.is_some() && self.tabs[j].group != group {
            // Stepping over the group edge leaves the group.
            self.tabs[i].group = None;
            self.normalize_keep(uid);
            return;
        }
        if group.is_none() && self.tabs[j].group.is_some() {
            // Jump over a whole group.
            let g = self.tabs[j].group.unwrap();
            let (first, last) = self.group_range(g).unwrap();
            let to = if delta > 0 { last + 1 } else { first };
            self.move_tab(i, to, None);
            return;
        }
        self.tabs.swap(i, j);
        self.normalize_keep(uid);
    }

    /// Restore the invariants: pinned tabs first and never grouped, the tabs of
    /// a group next to each other, no empty groups. `active` follows `active_uid`.
    pub fn normalize_keep(&mut self, active_uid: u64) {
        let ids: HashSet<u32> = self.groups.iter().map(|g| g.id).collect();
        for t in &mut self.tabs {
            if t.pinned || t.group.is_some_and(|g| !ids.contains(&g)) {
                t.group = None;
            }
        }
        let mut rest: Vec<Option<Workspace>> = std::mem::take(&mut self.tabs).into_iter().map(Some).collect();
        let mut out: Vec<Workspace> = Vec::with_capacity(rest.len());
        for slot in rest.iter_mut() {
            if slot.as_ref().is_some_and(|t| t.pinned) {
                out.push(slot.take().unwrap());
            }
        }
        let mut placed: HashSet<u32> = HashSet::new();
        for i in 0..rest.len() {
            let Some(g) = rest[i].as_ref().map(|t| t.group) else { continue };
            match g {
                None => out.push(rest[i].take().unwrap()),
                Some(g) if placed.insert(g) => {
                    for slot in rest.iter_mut().skip(i) {
                        if slot.as_ref().is_some_and(|t| t.group == Some(g)) {
                            out.push(slot.take().unwrap());
                        }
                    }
                }
                Some(_) => {}
            }
        }
        self.tabs = out;
        let used: HashSet<u32> = self.tabs.iter().filter_map(|t| t.group).collect();
        self.groups.retain(|g| used.contains(&g.id));
        // Keep groups in tab order (for menus).
        self.groups.sort_by_key(|g| self.tabs.iter().position(|t| t.group == Some(g.id)).unwrap_or(usize::MAX));
        self.active = self.index_of(active_uid).unwrap_or(0).min(self.tabs.len().saturating_sub(1));
    }
}

// ---------------------------------------------------------------- UI

pub const TAB_H: f32 = 28.0;
const PIN_W: f32 = 36.0;
const MAX_W: f32 = 210.0;
const MIN_W: f32 = 64.0;

enum Item {
    Tab(usize),
    Chip(u32),
}

/// Local edits applied after drawing (so menus can borrow the strip freely).
enum Local {
    Activate(usize),
    Pin(usize),
    NewGroup(usize),
    SetGroup(usize, Option<u32>),
    Collapse(u32),
    Ungroup(u32),
    Move { from: usize, to: usize, group: Option<u32> },
}

pub fn terminal_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.3_f32, c);
    p.rect_stroke(r, 2.5, s, egui::StrokeKind::Inside);
    let y = r.center().y;
    let k = r.width() / 13.0;
    p.line_segment([pos2(r.left() + 3.0 * k, y - 2.5 * k), pos2(r.left() + 5.5 * k, y)], s);
    p.line_segment([pos2(r.left() + 5.5 * k, y), pos2(r.left() + 3.0 * k, y + 2.5 * k)], s);
    p.line_segment([pos2(r.left() + 7.0 * k, y + 2.5 * k), pos2(r.right() - 3.0 * k, y + 2.5 * k)], s);
}

/// A miniature of a tab's split layout: one cell per window, the focused one filled.
pub fn layout_glyph(p: &egui::Painter, r: Rect, cells: &[(Rect, bool)], c: Color32, focus: Color32) {
    for (u, focused) in cells {
        let cr = Rect::from_min_max(
            pos2(r.left() + u.left() * r.width(), r.top() + u.top() * r.height()),
            pos2(r.left() + u.right() * r.width(), r.top() + u.bottom() * r.height()),
        );
        if *focused {
            p.rect_filled(cr, 1.5, focus);
        } else {
            p.rect_stroke(cr, 1.5, Stroke::new(1.1_f32, c), egui::StrokeKind::Inside);
        }
    }
}

/// Window cells of `layout` in a unit square (for `layout_glyph`).
pub fn layout_cells(layout: &Layout, focused: usize) -> Vec<(Rect, bool)> {
    let (mut rects, mut handles) = (Vec::new(), Vec::new());
    layout.arrange(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), 0.14, &mut rects, &mut handles);
    rects.into_iter().map(|(i, r)| (r, i == focused)).collect()
}

fn truncate(ui: &egui::Ui, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(4.0));
    ui.fonts(|f| f.layout_job(job))
}

/// The tab bar. `info[i]` describes `strip.tabs[i]`.
pub fn tab_bar(ui: &mut egui::Ui, strip: &mut Strip, info: &[TabInfo], c: &BarCtx, ev: &mut Vec<TabEvent>) {
    let th = c.theme;
    let ctx = ui.ctx().clone();
    let mut local: Vec<Local> = Vec::new();
    let mut tab_rects: Vec<(usize, Rect)> = Vec::new();
    let mut chip_rects: Vec<(u32, Rect)> = Vec::new();
    let tab_drag = egui::DragAndDrop::payload::<TabDrag>(&ctx).map(|d| *d);
    let win_drag = egui::DragAndDrop::payload::<WinDrag>(&ctx).map(|d| *d);
    let font = FontId::proportional(13.0);

    // Visible items in order.
    let mut items = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for (i, tab) in strip.tabs.iter().enumerate() {
        if let Some(g) = tab.group {
            if seen.insert(g) {
                items.push(Item::Chip(g));
            }
            if strip.group(g).is_some_and(|g| g.collapsed) {
                continue;
            }
        }
        items.push(Item::Tab(i));
    }

    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let buttons_w = 56.0;
        let strip_w = (ui.available_width() - buttons_w).max(40.0);
        // Widths: shrink tabs to fit, like Chrome; scroll when even that is too wide.
        let chips_w: f32 = items
            .iter()
            .filter_map(|it| match it {
                Item::Chip(g) => strip.group(*g).map(|g| chip_width(ui, g, strip.tabs.iter().filter(|t| t.group == Some(g.id)).count())),
                _ => None,
            })
            .sum();
        let pinned = strip.tabs.iter().filter(|t| t.pinned).count() as f32;
        let normal = items.iter().filter(|it| matches!(it, Item::Tab(i) if !strip.tabs[*i].pinned)).count().max(1) as f32;
        let spacing = 2.0 * items.len() as f32;
        let tab_w = ((strip_w - chips_w - pinned * PIN_W - spacing) / normal).clamp(MIN_W, MAX_W);

        ui.allocate_ui_with_layout(vec2(strip_w, TAB_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_max_width(strip_w);
            egui::ScrollArea::horizontal()
                .id_salt("tab-strip")
                .max_width(strip_w)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for it in &items {
                            match *it {
                                Item::Chip(g) => {
                                    if let Some(r) = group_chip(ui, strip, g, c, ev, &mut local) {
                                        chip_rects.push((g, r));
                                    }
                                }
                                Item::Tab(i) => {
                                    let w = if strip.tabs[i].pinned { PIN_W } else { tab_w };
                                    let r = tab_item(ui, strip, info, i, w, c, ev, &mut local, &font);
                                    tab_rects.push((i, r));
                                }
                            }
                        }
                    });
                });
        });
        // "+" = new file tab, terminal button = new terminal tab (right-click: profiles).
        let (at, group) = (strip.active + 1, strip.tabs[strip.active].group);
        if icons::icon_button(ui, 24.0, true, th, icons::plus).on_hover_text(format!("{} ({})", t("New tab"), crate::shortcuts::hint(crate::shortcuts::Action::NewTab))).clicked() {
            ev.push(TabEvent::NewFiles { at, group });
        }
        let tr = icons::icon_button(ui, 24.0, true, th, |p, r, col| terminal_glyph(p, r.shrink(1.0), col)).on_hover_text(format!("{} ({})", t("New terminal tab"), crate::shortcuts::hint(crate::shortcuts::Action::NewTerminalTab)));
        if tr.clicked() {
            ev.push(TabEvent::NewTerminal { at, group, profile: None });
        }
        tr.context_menu(|ui| {
            ui.label(egui::RichText::new(t("New terminal tab with profile")).color(th.weak).size(11.5));
            for (i, name) in c.terminal_profiles.iter().enumerate() {
                if ui.button(name).clicked() {
                    ev.push(TabEvent::NewTerminal { at, group, profile: Some(i) });
                    ui.close_menu();
                }
            }
        });
    });
    // The whole width counts as the bar (also the empty part right of the tabs).
    let bar = Rect::from_min_max(row.response.rect.min, pos2(ui.max_rect().right(), row.response.rect.bottom()));

    // Scroll the active tab into view when it changes.
    let key = egui::Id::new("tab-strip-active");
    let uid = strip.tabs[strip.active].uid;
    if ctx.data(|d| d.get_temp::<u64>(key)) != Some(uid) {
        ctx.data_mut(|d| d.insert_temp(key, uid));
        if let Some((_, r)) = tab_rects.iter().find(|(i, _)| *i == strip.active) {
            ui.scroll_to_rect(*r, None);
        }
    }

    // ---- drag & drop onto the bar: tabs (reorder) and mini windows (new tab / move into a tab)
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let released = ctx.input(|i| i.pointer.any_released());
    let over_bar = pointer.filter(|p| bar.expand2(vec2(0.0, 6.0)).contains(*p));
    // A window that is the only one of its tab moves like its tab.
    let lone_window = c.dragged_window.is_some_and(|(_, n)| n <= 1);
    let moving = match (tab_drag, win_drag) {
        (Some(d), _) => Some(d.uid),
        (None, Some(_)) if lone_window => Some(strip.tabs[strip.active].uid),
        (None, Some(_)) => Some(u64::MAX),
        _ => None,
    };
    if let (Some(p), Some(moving)) = (over_bar, moving) {
        // A window held over the middle of another tab: move it into that tab.
        let into = win_drag.filter(|_| !lone_window).and_then(|w| {
            tab_rects
                .iter()
                .find(|(i, r)| *i != strip.active && r.contains(p) && (p.x - r.center().x).abs() < r.width() * 0.3)
                .map(|(i, r)| (w.uid, strip.tabs[*i].uid, *r))
        });
        if let Some((view, tab, r)) = into {
            ui.painter().rect_stroke(r.expand(1.0), 7.0, Stroke::new(2.0_f32, th.accent), egui::StrokeKind::Outside);
            if released {
                egui::DragAndDrop::clear_payload(&ctx);
                ev.push(TabEvent::WindowInto { view, tab });
            }
        } else {
            // Over a group chip: join that group.
            let target = if let Some((g, r)) = chip_rects.iter().find(|(_, r)| r.contains(p)) {
                ui.painter().rect_stroke(r.expand(1.5), 8.0, Stroke::new(1.5_f32, th.accent), egui::StrokeKind::Outside);
                let at = strip.group_range(*g).map(|(_, l)| l + 1).unwrap_or(strip.tabs.len());
                (at, Some(*g))
            } else {
                let at = tab_rects.iter().find(|(_, r)| p.x < r.center().x).map(|(i, _)| *i).unwrap_or_else(|| tab_rects.last().map(|(i, _)| i + 1).unwrap_or(strip.tabs.len()));
                let mut group = strip.drop_group(moving, at);
                // Between a group's chip and its first tab: join the group.
                if let Some(g) = strip.tabs.get(at).and_then(|t| t.group) {
                    if chip_rects.iter().any(|(cg, r)| *cg == g && p.x > r.right()) {
                        group = Some(g);
                    }
                }
                // Marker between tabs.
                let mut x = tab_rects
                    .iter()
                    .find(|(i, _)| *i == at)
                    .map(|(_, r)| r.left() - 1.0)
                    .or_else(|| tab_rects.last().map(|(_, r)| r.right() + 1.0))
                    .unwrap_or(bar.left());
                let y0 = tab_rects.first().map(|(_, r)| r.top()).unwrap_or(bar.top());
                // Outside the group: the marker goes before the group's chip.
                if let Some(g) = strip.tabs.get(at).and_then(|t| t.group).filter(|g| group != Some(*g)) {
                    if let Some((_, r)) = chip_rects.iter().find(|(cg, _)| *cg == g) {
                        x = r.left() - 1.0;
                    }
                }
                ui.painter().line_segment([pos2(x, y0), pos2(x, y0 + TAB_H)], Stroke::new(2.0_f32, th.accent));
                (at, group)
            };
            if released {
                let (at, group) = target;
                egui::DragAndDrop::clear_payload(&ctx);
                match win_drag {
                    Some(w) if !lone_window && tab_drag.is_none() => ev.push(TabEvent::Detach { view: w.uid, at, group }),
                    _ => {
                        if let Some(from) = strip.index_of(moving) {
                            local.push(Local::Move { from, to: at, group });
                        }
                    }
                }
            }
        }
    }
    // Ghost label following the pointer while a tab is dragged.
    if let (Some(d), Some(p)) = (tab_drag, pointer) {
        if let Some(i) = strip.index_of(d.uid) {
            let title = info[i].title.clone();
            egui::Area::new(egui::Id::new("tab-drag-ghost")).order(egui::Order::Tooltip).fixed_pos(p + vec2(12.0, 8.0)).interactable(false).show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(title);
                });
            });
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }

    for l in local {
        match l {
            Local::Activate(i) => {
                strip.activate(i);
                ev.push(TabEvent::Focus);
            }
            Local::Pin(i) => strip.toggle_pin(i),
            Local::NewGroup(i) => {
                strip.new_group(i);
            }
            Local::SetGroup(i, g) => strip.set_tab_group(i, g),
            Local::Collapse(g) => strip.toggle_collapsed(g),
            Local::Ungroup(g) => strip.ungroup(g),
            Local::Move { from, to, group } => strip.move_tab(from, to, group),
        }
    }
}

fn chip_width(ui: &egui::Ui, g: &TabGroup, count: usize) -> f32 {
    let label = chip_label(g, count);
    if label.is_empty() {
        return 16.0;
    }
    ui.fonts(|f| f.layout_no_wrap(label, FontId::proportional(12.0), Color32::WHITE).size().x) + 18.0
}

fn chip_label(g: &TabGroup, count: usize) -> String {
    match (g.name.trim().is_empty(), g.collapsed) {
        (true, false) => String::new(),
        (true, true) => format!("{count}"),
        (false, false) => g.name.trim().to_string(),
        (false, true) => format!("{} · {count}", g.name.trim()),
    }
}

fn group_chip(ui: &mut egui::Ui, strip: &mut Strip, gid: u32, c: &BarCtx, ev: &mut Vec<TabEvent>, local: &mut Vec<Local>) -> Option<Rect> {
    let th = c.theme;
    let count = strip.tabs.iter().filter(|t| t.group == Some(gid)).count();
    let g = strip.group(gid)?.clone();
    let color = group_color(g.color, th.dark);
    let label = chip_label(&g, count);
    let w = chip_width(ui, &g, count);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, TAB_H), Sense::click());
    let p = ui.painter();
    if label.is_empty() {
        p.circle_filled(rect.center(), 5.0, color);
        if resp.hovered() {
            p.circle_stroke(rect.center(), 7.0, Stroke::new(1.0_f32, color));
        }
    } else {
        let pill = Rect::from_center_size(rect.center(), vec2(w - 4.0, 20.0));
        let fill = if resp.hovered() { color.lerp_to_gamma(Color32::WHITE, 0.15) } else { color };
        p.rect_filled(pill, 6.0, fill);
        p.text(pill.center(), Align2::CENTER_CENTER, &label, FontId::proportional(12.0), text_on(fill));
    }
    if resp.clicked() {
        local.push(Local::Collapse(gid));
    }
    let focus_key = egui::Id::new(("group-name-focus", gid));
    if resp.secondary_clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(focus_key, true));
    }
    let tip = if g.collapsed { t("Expand group") } else { t("Collapse group") };
    let resp = resp.on_hover_text(format!("{} ({count})\n{tip}", if g.name.is_empty() { t("Unnamed group") } else { g.name.clone() }));
    resp.context_menu(|ui| {
        ui.set_min_width(220.0);
        if let Some(gm) = strip.group_mut(gid) {
            let r = ui.add(egui::TextEdit::singleline(&mut gm.name).hint_text(t("Name this group")).desired_width(200.0));
            // Focus the name field when the menu opens (the terminal keeps it otherwise).
            if ui.ctx().data_mut(|d| d.remove_temp::<bool>(focus_key)).unwrap_or(false) {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ui.close_menu();
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (k, (name, _)) in GROUP_COLORS.iter().enumerate() {
                    let (r, sresp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                    let col = group_color(k as u8, th.dark);
                    ui.painter().circle_filled(r.center(), 7.0, col);
                    if gm.color == k as u8 {
                        ui.painter().circle_stroke(r.center(), 9.5, Stroke::new(1.5_f32, th.text));
                    }
                    if sresp.on_hover_text(t(name)).clicked() {
                        gm.color = k as u8;
                    }
                }
            });
        }
        ui.separator();
        let end = strip.group_range(gid).map(|(_, l)| l + 1).unwrap_or(strip.tabs.len());
        if ui.button(t("New tab in group")).clicked() {
            ev.push(TabEvent::NewFiles { at: end, group: Some(gid) });
            ui.close_menu();
        }
        if ui.button(t("New terminal in group")).clicked() {
            ev.push(TabEvent::NewTerminal { at: end, group: Some(gid), profile: None });
            ui.close_menu();
        }
        if ui.button(if g.collapsed { t("Expand group") } else { t("Collapse group") }).clicked() {
            local.push(Local::Collapse(gid));
            ui.close_menu();
        }
        ui.separator();
        if ui.button(t("Ungroup")).clicked() {
            local.push(Local::Ungroup(gid));
            ui.close_menu();
        }
        if ui.button(t("Close group")).clicked() {
            let uids = strip.tabs.iter().filter(|t| t.group == Some(gid)).map(|t| t.uid).collect();
            ev.push(TabEvent::Close(uids));
            ui.close_menu();
        }
    });
    Some(rect)
}

#[allow(clippy::too_many_arguments)]
fn tab_item(ui: &mut egui::Ui, strip: &mut Strip, info: &[TabInfo], i: usize, w: f32, c: &BarCtx, ev: &mut Vec<TabEvent>, local: &mut Vec<Local>, font: &FontId) -> Rect {
    let th = c.theme;
    let selected = i == strip.active;
    let (rect, _) = ui.allocate_exact_size(vec2(w, TAB_H), Sense::hover());
    let tab = &strip.tabs[i];
    let inf = &info[i];
    let id = egui::Id::new(("tab", tab.uid));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let p = ui.painter().clone();
    let being_dragged = egui::DragAndDrop::payload::<TabDrag>(ui.ctx()).is_some_and(|d| d.uid == tab.uid);
    match tab.group.and_then(|g| strip.group(g)) {
        // Grouped tabs are simply filled with the group colour.
        Some(g) => {
            p.rect_filled(rect, 6.0, group_tint(group_color(g.color, th.dark), th.dark, selected, resp.hovered()));
        }
        None if selected => {
            p.rect(rect, 6.0, th.bg, Stroke::new(1.0_f32, th.border), egui::StrokeKind::Inside);
        }
        None if resp.hovered() => {
            p.rect_filled(rect, 6.0, th.hover());
        }
        None => {}
    }
    if being_dragged {
        p.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, th.accent.gamma_multiply(0.6)), egui::StrokeKind::Inside);
    }
    let color = if selected { th.strong() } else if tab.group.is_some() { th.text } else { th.weak };
    let icon_c = if tab.pinned { rect.center().x } else { rect.left() + 13.0 };
    let icon_r = Rect::from_center_size(pos2(icon_c, rect.center().y), vec2(13.0, 13.0));
    if inf.cells.len() > 1 {
        // A split tab shows its layout.
        layout_glyph(&p, icon_r, &inf.cells, if selected { th.text } else { th.weak }, th.accent);
    } else if inf.settings {
        icons::gear(&p, icon_r, if selected { th.accent } else { th.weak });
    } else if inf.transfers {
        icons::transfers(&p, icon_r, if selected { th.accent } else { th.weak });
    } else if inf.disk_usage {
        icons::bars(&p, icon_r, if selected { th.accent } else { th.weak });
    } else {
        match inf.term {
            Some(exited) => {
                let ic = if exited { th.weak.gamma_multiply(0.6) } else if selected { th.accent } else { th.weak };
                terminal_glyph(&p, icon_r, ic);
            }
            None => icons::folder_icon(&p, icon_r, th),
        }
    }
    if inf.bell && !selected {
        p.circle_filled(icon_r.right_top() + vec2(1.0, 0.0), 3.2, th.danger);
    } else if inf.activity && !selected {
        p.circle_filled(icon_r.right_top() + vec2(1.0, 0.0), 3.0, th.accent);
    }
    let closable = strip.tabs.len() > 1 || inf.term.is_some() || inf.disk_usage || inf.settings || inf.transfers || inf.cells.len() > 1;
    let show_x = closable && !tab.pinned && (selected || resp.hovered()) && w > 70.0;
    if !tab.pinned {
        let text_w = w - 26.0 - if show_x { 20.0 } else { 6.0 };
        let galley = truncate(ui, &inf.title, font.clone(), color, text_w);
        p.galley(pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0), galley, color);
    }
    let mut closed = false;
    let uid = tab.uid;
    if show_x {
        let x_rect = Rect::from_center_size(pos2(rect.right() - 11.0, rect.center().y), vec2(16.0, 16.0));
        let over_x = ui.input(|i| i.pointer.hover_pos()).is_some_and(|h| x_rect.contains(h));
        if over_x {
            p.rect_filled(x_rect, 4.0, th.hover().lerp_to_gamma(th.text, 0.1));
        }
        let s = Stroke::new(1.2_f32, if over_x { th.text } else { th.weak });
        let r = x_rect.shrink(4.5);
        p.line_segment([r.left_top(), r.right_bottom()], s);
        p.line_segment([r.right_top(), r.left_bottom()], s);
        if resp.clicked() && over_x {
            ev.push(TabEvent::Close(vec![uid]));
            closed = true;
        }
    }
    if !closed && resp.clicked() {
        local.push(Local::Activate(i));
    }
    if resp.middle_clicked() {
        ev.push(TabEvent::Close(vec![uid]));
    }
    if resp.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), TabDrag { uid });
    }
    let resp = if egui::DragAndDrop::has_any_payload(ui.ctx()) { resp } else { resp.on_hover_text(inf.tooltip.clone()) };
    resp.context_menu(|ui| tab_menu(ui, strip, info, i, c, ev, local));
    rect
}

fn tab_menu(ui: &mut egui::Ui, strip: &mut Strip, info: &[TabInfo], i: usize, c: &BarCtx, ev: &mut Vec<TabEvent>, local: &mut Vec<Local>) {
    ui.set_min_width(230.0);
    let uid = strip.tabs[i].uid;
    let group = strip.tabs[i].group;
    let is_term = info[i].term.is_some();
    // In a terminal, the keys that work there.
    let key_of = |action| if is_term { crate::shortcuts::terminal_hint(action) } else { crate::shortcuts::hint(action) };
    let new_key = key_of(crate::shortcuts::Action::NewTab);
    if ui.add(egui::Button::new(t("New tab to the right")).shortcut_text(new_key)).clicked() {
        ev.push(TabEvent::NewFiles { at: i + 1, group });
        ui.close_menu();
    }
    if ui.add(egui::Button::new(t("New terminal to the right")).shortcut_text(key_of(crate::shortcuts::Action::NewTerminalTab))).clicked() {
        ev.push(TabEvent::NewTerminal { at: i + 1, group, profile: None });
        ui.close_menu();
    }
    ui.separator();
    // Rename (live).
    ui.horizontal(|ui| {
        ui.label(t("Name"));
        let tab = &mut strip.tabs[i];
        let mut name = tab.custom_title.clone().unwrap_or_default();
        let hint = info[i].default_title.clone();
        if ui.add(egui::TextEdit::singleline(&mut name).hint_text(hint).desired_width(150.0)).changed() {
            tab.custom_title = (!name.trim().is_empty()).then_some(name);
        }
    });
    if !strip.tabs[i].pinned {
        if ui.button(t("Add tab to new group")).clicked() {
            local.push(Local::NewGroup(i));
            ui.close_menu();
        }
        let others: Vec<(u32, String, u8)> = strip
            .groups
            .iter()
            .filter(|g| Some(g.id) != group)
            .map(|g| (g.id, if g.name.is_empty() { t("Unnamed group") } else { g.name.clone() }, g.color))
            .collect();
        if !others.is_empty() {
            ui.menu_button(t("Add tab to group"), |ui| {
                for (gid, name, color) in others {
                    let resp = ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 5.0, group_color(color, c.theme.dark));
                        ui.button(name)
                    });
                    if resp.inner.clicked() {
                        local.push(Local::SetGroup(i, Some(gid)));
                        ui.close_menu();
                    }
                }
            });
        }
        if group.is_some() && ui.button(t("Remove from group")).clicked() {
            local.push(Local::SetGroup(i, None));
            ui.close_menu();
        }
    }
    if ui.button(if strip.tabs[i].pinned { t("Unpin") } else { t("Pin") }).clicked() {
        local.push(Local::Pin(i));
        ui.close_menu();
    }
    if ui.button(t("Duplicate")).clicked() {
        ev.push(TabEvent::Duplicate(uid));
        ui.close_menu();
    }
    if i != strip.active && ui.button(t("Show beside the current tab")).clicked() {
        ev.push(TabEvent::MergeIntoShown(uid));
        ui.close_menu();
    }
    ui.separator();
    let close_key = key_of(crate::shortcuts::Action::CloseTab);
    if ui.add(egui::Button::new(t("Close tab")).shortcut_text(close_key)).clicked() {
        ev.push(TabEvent::Close(vec![uid]));
        ui.close_menu();
    }
    if strip.tabs.len() > 1 && ui.button(t("Close other tabs")).clicked() {
        let uids = strip.tabs.iter().filter(|t| t.uid != uid && !t.pinned).map(|t| t.uid).collect();
        ev.push(TabEvent::Close(uids));
        ui.close_menu();
    }
    if i + 1 < strip.tabs.len() && ui.button(t("Close tabs to the right")).clicked() {
        let uids = strip.tabs[i + 1..].iter().map(|t| t.uid).collect();
        ev.push(TabEvent::Close(uids));
        ui.close_menu();
    }
    if c.can_reopen && ui.add(egui::Button::new(t("Reopen closed tab")).shortcut_text(key_of(crate::shortcuts::Action::ReopenTab))).clicked() {
        ev.push(TabEvent::ReopenClosed);
        ui.close_menu();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(n: usize) -> Strip {
        let tabs = (0..n)
            .map(|i| {
                let mut w = Workspace::new(Vec::new(), Layout::Leaf(0));
                w.custom_title = Some(format!("t{i}"));
                w
            })
            .collect();
        Strip { tabs, active: 0, groups: Vec::new() }
    }

    fn names(p: &Strip) -> Vec<String> {
        p.tabs.iter().map(|t| t.custom_title.clone().unwrap_or_default()).collect()
    }

    #[test]
    fn groups_stay_contiguous() {
        let mut p = strip(5); // t0 t1 t2 t3 t4
        let g = p.new_group(1);
        p.set_tab_group(3, Some(g)); // t3 joins and moves next to t1
        assert_eq!(names(&p), ["t0", "t1", "t3", "t2", "t4"]);
        assert_eq!(p.group_range(g), Some((1, 2)));
        // Leaving the group puts the tab right after it.
        p.set_tab_group(1, None);
        assert_eq!(names(&p), ["t0", "t3", "t1", "t2", "t4"]);
        // Dropping a tab between two tabs of the same group joins it.
        let t4 = p.tabs[4].uid;
        p.set_tab_group(2, Some(g)); // t3 t1 grouped
        assert_eq!(p.drop_group(t4, 2), Some(g));
        p.move_tab(4, 2, p.drop_group(t4, 2));
        assert_eq!(names(&p), ["t0", "t3", "t4", "t1", "t2"]);
        assert_eq!(p.group_range(g), Some((1, 3)));
        // Ungroup removes the group.
        p.ungroup(g);
        assert!(p.groups.is_empty() && p.tabs.iter().all(|t| t.group.is_none()));
    }

    #[test]
    fn pin_collapse_and_close() {
        let mut p = strip(4);
        p.activate(2);
        p.toggle_pin(3);
        assert_eq!(names(&p), ["t3", "t0", "t1", "t2"]);
        assert_eq!(p.active, 3, "active tab follows its uid");
        let g = p.new_group(3); // t2 (active) in a group
        p.toggle_collapsed(g);
        assert!(p.group(g).unwrap().collapsed);
        assert!(p.tabs[p.active].group != Some(g), "focus leaves a collapsed group");
        p.cycle(1);
        assert!(!p.is_hidden(p.active));
        // Activating a hidden tab expands its group.
        let i = p.index_of(p.tabs.iter().find(|t| t.group == Some(g)).unwrap().uid).unwrap();
        p.activate(i);
        assert!(!p.group(g).unwrap().collapsed);
        // Closing the active tab activates its right neighbour (or the left one at the end).
        p.activate(1);
        let removed = p.remove_tab(1);
        assert_eq!(removed.custom_title.as_deref(), Some("t0"));
        assert_eq!(names(&p)[p.active], "t1");
        let last = p.tabs.len() - 1;
        p.activate(last);
        p.remove_tab(last);
        assert_eq!(p.active, p.tabs.len() - 1);
        assert!(p.groups.is_empty(), "empty groups disappear");
    }

    #[test]
    fn shift_over_group() {
        let mut p = strip(4);
        let g = p.new_group(1);
        p.set_tab_group(2, Some(g)); // t0 [t1 t2] t3
        p.activate(0);
        p.shift_active(1); // t0 jumps over the group
        assert_eq!(names(&p), ["t1", "t2", "t0", "t3"]);
        assert_eq!(p.active, 2);
        p.activate(1);
        p.shift_active(1); // t2 leaves the group to the right
        assert_eq!(p.tabs[1].group, None);
    }

    #[test]
    fn layout_cells_mark_focus() {
        let mut l = Layout::Leaf(0);
        l.split(0, 1, crate::layout::SplitDir::Right);
        let cells = layout_cells(&l, 1);
        assert_eq!(cells.len(), 2);
        assert!(!cells[0].1 && cells[1].1);
        assert!(cells[0].0.right() < cells[1].0.left());
    }
}
