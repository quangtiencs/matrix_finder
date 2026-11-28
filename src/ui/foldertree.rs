//! A tree of folders to pick one from quickly: the "Folder tree" button of the address
//! bar and of every field that takes a folder (Copy / Move, Compress, Extract, …).
//!
//! It starts at the home folder and the drives, with the folders down to the current
//! one open. A folder is read on a thread the first time it is opened, so a slow disk
//! or network mount never holds up the window. A click (or ↑ ↓ ← →) chooses a folder,
//! OK, Enter or a double click takes it, Cancel or Esc closes the tree.

use crate::i18n::t;
use crate::style::theme::Theme;
use egui::{pos2, vec2, Align2, FontId, Rect, Sense};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

/// Most folders read from one folder.
const MAX_READ: usize = 50_000;
const ROW_H: f32 = 24.0;
const INDENT: f32 = 14.0;
const WIDTH: f32 = 300.0;
const MAX_H: f32 = 264.0;
/// The popup's frame, separator and OK / Cancel row around the list.
const POPUP_EXTRA: f32 = 64.0;

/// A top-level entry: the home folder or a drive.
#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    pub name: String,
    pub path: PathBuf,
    pub drive: bool,
}

/// What is known of the folders inside a folder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kids {
    /// Not read yet (it is closed).
    Unknown,
    Loading,
    None,
    Some,
}

/// One line of the tree as shown.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub open: bool,
    pub kids: Kids,
    pub drive: bool,
}

/// The folders inside `dir` (links to folders too), in natural order. Hidden ones
/// (starting with a dot) only with `show_hidden`. An unreadable folder has none.
pub fn list_dirs(dir: &Path, show_hidden: bool) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out = Vec::new();
    for e in rd.flatten().take(MAX_READ) {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        if ft.is_dir() || (ft.is_symlink() && e.path().is_dir()) {
            out.push(name);
        }
    }
    out.sort_by(|a, b| crate::util::natural_cmp(a, b));
    out
}

/// The home folder first, then each drive (not the system's own mounts), each once.
pub fn roots(home: Option<&Path>, drives: &[crate::files::drives::Drive]) -> Vec<Root> {
    let mut out: Vec<Root> = Vec::new();
    if let Some(h) = home {
        out.push(Root { name: t("Home"), path: h.to_path_buf(), drive: false });
    }
    for d in drives.iter().filter(|d| !d.system) {
        if !out.iter().any(|r| r.path == d.mount) {
            let name = if d.name.is_empty() { d.mount.display().to_string() } else { d.name.clone() };
            out.push(Root { name, path: d.mount.clone(), drive: true });
        }
    }
    out
}

/// The root `path` is in: the deepest one (the home folder rather than the drive it is on).
fn root_of<'a>(roots: &'a [Root], path: &Path) -> Option<&'a Root> {
    roots.iter().filter(|r| path.starts_with(&r.path)).max_by_key(|r| r.path.components().count())
}

/// What happened in a tree popup this frame.
#[derive(Debug, PartialEq)]
pub enum Popup {
    Open,
    Picked(PathBuf),
    Closed,
}

pub struct FolderTree {
    roots: Vec<Root>,
    open: HashSet<PathBuf>,
    kids: HashMap<PathBuf, Vec<String>>,
    loading: HashSet<PathBuf>,
    tx: Sender<(PathBuf, Vec<String>)>,
    rx: Receiver<(PathBuf, Vec<String>)>,
    show_hidden: bool,
    current: Option<PathBuf>,
    /// The folder chosen (OK takes it).
    selected: Option<PathBuf>,
    /// The row it was chosen on: a folder can be shown twice (the home folder, and inside
    /// the drive holding it), and the keys go on from the row chosen.
    sel_at: Option<usize>,
    /// Scroll the chosen folder into view (the first frame, and after a key).
    follow: bool,
    /// Where the list was scrolled to in the last frame.
    offset: f32,
    /// Every folder down to the chosen one is read, and it was scrolled into view: from
    /// then on the list only scrolls to follow the keys.
    settled: bool,
    /// Where each folder's row and chevron were drawn in the last frame.
    pub rows: Vec<(PathBuf, Rect)>,
    pub toggles: Vec<(PathBuf, Rect)>,
    /// Where the popup's OK and Cancel buttons were drawn in the last frame.
    pub buttons: Option<(Rect, Rect)>,
    /// The most the list is tall (less in a dialog that must fit a small window).
    pub max_height: f32,
    /// A folder clicked in the last frame (taken by whoever shows the tree).
    pub clicked: Option<PathBuf>,
}

impl FolderTree {
    /// A tree open down to `current` (or at the first root when there is none).
    pub fn new(roots: Vec<Root>, current: Option<&Path>, show_hidden: bool) -> Self {
        let (tx, rx) = channel();
        let mut tree = Self {
            roots,
            open: HashSet::new(),
            kids: HashMap::new(),
            loading: HashSet::new(),
            tx,
            rx,
            show_hidden,
            current: None,
            selected: None,
            sel_at: None,
            follow: true,
            offset: 0.0,
            settled: false,
            rows: Vec::new(),
            toggles: Vec::new(),
            buttons: None,
            max_height: MAX_H,
            clicked: None,
        };
        let current = current.filter(|c| root_of(&tree.roots, c).is_some());
        match current {
            Some(c) => {
                tree.current = Some(c.to_path_buf());
                tree.reveal(c);
            }
            None => {
                if let Some(r) = tree.roots.first() {
                    tree.open.insert(r.path.clone());
                }
            }
        }
        tree.selected = tree.current.clone().or_else(|| tree.roots.first().map(|r| r.path.clone()));
        tree
    }

    /// Choose nothing (until a folder is clicked or reached with the keys).
    pub fn unselect(&mut self) {
        self.selected = None;
        self.sel_at = None;
    }

    /// The folder chosen, which OK takes.
    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    pub fn select(&mut self, path: &Path) {
        self.selected = Some(path.to_path_buf());
        self.sel_at = None;
        self.follow = true;
        // Chosen by hand: no more scrolling to where it opened.
        self.settled = true;
    }

    /// Chooses row `i` of `rows`.
    fn select_row(&mut self, rows: &[Row], i: usize) {
        self.select(&rows[i].path);
        self.sel_at = Some(i);
    }

    /// The row of the folder chosen: of the rows showing it, the one nearest the row it
    /// was chosen on.
    fn sel_index(&self, rows: &[Row]) -> Option<usize> {
        let sel = self.selected.as_ref()?;
        let near = self.sel_at.unwrap_or(0);
        rows.iter().enumerate().filter(|(_, r)| &r.path == sel).map(|(i, _)| i).min_by_key(|i| i.abs_diff(near))
    }

    /// The row chosen, among the rows shown.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected_row(&self) -> Option<usize> {
        self.sel_index(&self.visible())
    }

    /// ↑ ↓: the row `by` rows above or below the chosen one (stopping at the ends).
    pub fn step(&mut self, by: isize) {
        let rows = self.visible();
        if rows.is_empty() {
            return;
        }
        let i = match self.sel_index(&rows) {
            Some(i) => (i as isize + by).clamp(0, rows.len() as isize - 1) as usize,
            None => 0,
        };
        self.select_row(&rows, i);
    }

    /// →: opens the chosen folder, or goes to its first subfolder when it is open.
    pub fn right(&mut self) {
        let rows = self.visible();
        let Some(i) = self.sel_index(&rows) else { return };
        let row = &rows[i];
        if !row.open {
            if row.kids != Kids::None {
                self.open.insert(row.path.clone());
                self.sel_at = Some(i);
            }
        } else if rows.get(i + 1).is_some_and(|n| n.depth > row.depth) {
            self.select_row(&rows, i + 1);
        }
    }

    /// ←: closes the chosen folder, or goes to the folder holding it when it is closed.
    pub fn left(&mut self) {
        let rows = self.visible();
        let Some(i) = self.sel_index(&rows) else { return };
        let row = &rows[i];
        if row.open && row.kids != Kids::None {
            self.open.remove(&row.path);
            self.sel_at = Some(i);
        } else if let Some(up) = (0..i).rev().find(|&j| rows[j].depth < row.depth) {
            self.select_row(&rows, up);
        }
    }

    /// The keys of an open tree: ↑ ↓ ← → choose, Enter takes the folder chosen, Esc
    /// closes it. They are taken before anything else sees them. None: no key of the tree.
    pub fn keys(&mut self, ctx: &egui::Context) -> Option<Popup> {
        use egui::{Key, Modifiers};
        let take = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        if take(Key::Escape) {
            return Some(Popup::Closed);
        }
        if take(Key::Enter) {
            return Some(match self.selected.clone() {
                Some(p) => Popup::Picked(p),
                None => Popup::Open,
            });
        }
        let mut any = false;
        for (k, f) in [(Key::ArrowUp, 0), (Key::ArrowDown, 1), (Key::ArrowLeft, 2), (Key::ArrowRight, 3)] {
            while take(k) {
                any = true;
                match f {
                    0 => self.step(-1),
                    1 => self.step(1),
                    2 => self.left(),
                    _ => self.right(),
                }
            }
        }
        if any {
            ctx.request_repaint();
        }
        any.then_some(Popup::Open)
    }

    /// The folder the tree was opened at.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn current(&self) -> Option<&Path> {
        self.current.as_deref()
    }

    /// Opens the folders from its root down to `path`, and `path` itself.
    pub fn reveal(&mut self, path: &Path) {
        let Some(root) = root_of(&self.roots, path).map(|r| r.path.clone()) else { return };
        let mut p = path;
        loop {
            self.open.insert(p.to_path_buf());
            if p == root {
                break;
            }
            match p.parent() {
                Some(up) => p = up,
                None => break,
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn toggle(&mut self, path: &Path) {
        self.toggle_row(path, None);
    }

    /// Opens or closes the folder at `path`, shown on row `row` (when known).
    fn toggle_row(&mut self, path: &Path, row: Option<usize>) {
        if !self.open.remove(path) {
            self.open.insert(path.to_path_buf());
        } else if self.selected.as_deref().is_some_and(|s| s != path && s.starts_with(path)) {
            // The folder chosen is no longer shown: choose the one closed.
            let near = row.or(self.sel_at);
            self.select(path);
            self.sel_at = near;
        }
    }

    /// The folders read for `dir` (from a thread, or a test).
    pub fn set_kids(&mut self, dir: PathBuf, names: Vec<String>) {
        self.loading.remove(&dir);
        self.kids.insert(dir, names);
    }

    /// The lines to show: each root, and inside each open folder the folders it holds.
    pub fn visible(&self) -> Vec<Row> {
        let mut out = Vec::new();
        for r in &self.roots {
            self.push_row(&mut out, r.path.clone(), r.name.clone(), 0, r.drive);
        }
        out
    }

    fn push_row(&self, out: &mut Vec<Row>, path: PathBuf, name: String, depth: usize, drive: bool) {
        let open = self.open.contains(&path);
        let kids = match self.kids.get(&path) {
            Some(k) if k.is_empty() => Kids::None,
            Some(_) => Kids::Some,
            None if self.loading.contains(&path) => Kids::Loading,
            None if open => Kids::Loading,
            None => Kids::Unknown,
        };
        out.push(Row { path: path.clone(), name, depth, open, kids, drive });
        if open {
            for k in self.kids.get(&path).into_iter().flatten() {
                self.push_row(out, path.join(k), k.clone(), depth + 1, false);
            }
        }
    }

    /// Takes the folders read in the background and starts reading the open folders
    /// shown that are not read yet.
    pub fn load(&mut self, ctx: Option<&egui::Context>) {
        while let Ok((dir, names)) = self.rx.try_recv() {
            self.set_kids(dir, names);
        }
        let want: Vec<PathBuf> =
            self.visible().into_iter().filter(|r| r.open && r.kids == Kids::Loading && !self.loading.contains(&r.path)).map(|r| r.path).collect();
        for dir in want {
            self.loading.insert(dir.clone());
            let (tx, hidden, ctx) = (self.tx.clone(), self.show_hidden, ctx.cloned());
            std::thread::spawn(move || {
                let names = list_dirs(&dir, hidden);
                let _ = tx.send((dir, names));
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
        }
    }

    /// Draws the tree. A click chooses a folder; returns the folder double-clicked.
    pub fn ui(&mut self, ui: &mut egui::Ui, th: &Theme) -> Option<PathBuf> {
        self.load(Some(ui.ctx()));
        let rows = self.visible();
        let mut picked = None;
        let mut toggle = None;
        let mut chosen = None;
        self.rows.clear();
        self.toggles.clear();
        // As tall as its rows (up to MAX_H): the area would keep the height of the first
        // frame, when the folders are still being read.
        let max_h = self.max_height;
        let h = (rows.len() as f32 * ROW_H).min(max_h);
        let mut area = egui::ScrollArea::vertical().max_height(h).min_scrolled_height(h).auto_shrink([false, true]);
        // While the folders above the chosen one are read, it moves down: follow it.
        let at = self.sel_index(&rows);
        self.sel_at = at.or(self.sel_at);
        // With nothing chosen, the folder it was opened at is the one kept in view.
        let shown_at = at.or_else(|| self.current.as_ref().and_then(|c| rows.iter().position(|r| &r.path == c)));
        let reading = match shown_at {
            Some(i) => rows[..i].iter().any(|r| r.kids == Kids::Loading && r.open),
            None => true,
        };
        if std::mem::take(&mut self.follow) || !self.settled {
            // Keep the chosen folder in view (a third down when it opens).
            if let Some(i) = shown_at {
                let (top, bottom) = (i as f32 * ROW_H, (i + 1) as f32 * ROW_H);
                let offset = if !self.settled {
                    (top - max_h / 3.0).max(0.0)
                } else if top < self.offset {
                    top
                } else if bottom > self.offset + h {
                    bottom - h
                } else {
                    self.offset
                };
                area = area.vertical_scroll_offset(offset);
            }
        }
        // No gap between rows (show_rows counts it in the height of each row).
        let spacing = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
        let out = area.show_rows(ui, ROW_H, rows.len(), |ui, range| {
            let first = range.start;
            for (k, row) in rows[range].iter().enumerate() {
                let i = first + k;
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let x = rect.left() + 2.0 + row.depth as f32 * INDENT;
                let chev = Rect::from_center_size(pos2(x + 8.0, rect.center().y), vec2(14.0, 14.0));
                let has_chev = !matches!(row.kids, Kids::None);
                let chev_resp = has_chev.then(|| ui.interact(chev.expand(3.0), ui.id().with(("tree-chevron", &row.path)), Sense::click()));
                let is_sel = at == Some(i);
                let is_current = self.current.as_ref() == Some(&row.path);
                if is_sel {
                    ui.painter().rect_filled(rect, 5.0, th.selection);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 5.0, th.hover());
                }
                let p = ui.painter().with_clip_rect(rect);
                if has_chev {
                    let hot = chev_resp.as_ref().is_some_and(|r| r.hovered());
                    let col = if hot { th.strong() } else { th.weak };
                    if row.kids == Kids::Loading {
                        p.text(chev.center(), Align2::CENTER_CENTER, "…", FontId::proportional(12.0), th.weak);
                    } else {
                        crate::style::icons::chevron(&p, chev, if row.open { 2 } else { 1 }, col);
                    }
                }
                let ir = Rect::from_center_size(pos2(x + 24.0, rect.center().y), vec2(15.0, 15.0));
                if row.drive {
                    crate::style::icons::drive_icon(&p, ir, th);
                } else {
                    crate::style::icons::folder_icon(&p, ir, th);
                }
                let text_x = x + 37.0;
                // The folder the tree was opened at is in bold.
                let font = if is_current { FontId::new(13.0, egui::FontFamily::Name("bold".into())) } else { FontId::proportional(13.0) };
                let mut job = egui::text::LayoutJob::simple_singleline(row.name.clone(), font, if is_sel || is_current { th.strong() } else { th.text });
                job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - text_x - 6.0).max(10.0));
                let g = ui.fonts_mut(|f| f.layout_job(job));
                p.galley(pos2(text_x, rect.center().y - g.size().y / 2.0), g, th.text);
                let resp = resp.on_hover_text(row.path.display().to_string());
                self.rows.push((row.path.clone(), rect));
                if has_chev {
                    self.toggles.push((row.path.clone(), chev));
                }
                if chev_resp.is_some_and(|r| r.clicked()) {
                    toggle = Some((row.path.clone(), i));
                } else if resp.double_clicked() {
                    picked = Some(row.path.clone());
                } else if resp.clicked() {
                    chosen = Some((row.path.clone(), i));
                }
            }
        });
        ui.spacing_mut().item_spacing.y = spacing;
        self.offset = out.state.offset.y;
        self.settled |= !reading;
        if let Some((p, i)) = toggle {
            self.toggle_row(&p, Some(i));
            ui.ctx().request_repaint();
        }
        if let Some((p, i)) = chosen {
            self.clicked = Some(p.clone());
            self.selected = Some(p);
            self.sel_at = Some(i);
            self.settled = true;
        }
        picked
    }
}

/// Shows `tree` in a popup under `anchor` (the button that opened it), above `parent`'s
/// layer, with the folder chosen and OK / Cancel under the tree. A press outside the
/// popup and the button closes it. (Its keys are taken by [`FolderTree::keys`].)
pub fn popup(parent: &egui::Ui, id: egui::Id, anchor: Rect, tree: &mut FolderTree, th: &Theme) -> Popup {
    let ctx = parent.ctx().clone();
    let layer = egui::LayerId::new(egui::Order::Foreground, id);
    if parent.layer_id().order == egui::Order::Foreground {
        // Inside a dialog: keep the popup above it.
        ctx.set_sublayer(parent.layer_id(), layer);
    }
    let screen = ctx.content_rect();
    let x = anchor.left().min(screen.right() - WIDTH - 8.0).max(screen.left() + 4.0);
    // Under the button, or above it when the tallest it can be does not fit below (a tall
    // dialog in a small window). Above, its top stays put while folders open and close.
    let h = tree.max_height + POPUP_EXTRA;
    let below = anchor.bottom() + 4.0;
    let y = if below + h > screen.bottom() - 8.0 && anchor.top() - 4.0 - h >= screen.top() + 4.0 { anchor.top() - 4.0 - h } else { below };
    let shown = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos2(x, y)).show(&ctx, |ui| {
        egui::Frame::popup(ui.style())
            .fill(th.panel)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(6))
            .stroke(egui::Stroke::new(1.0_f32, th.border))
            .show(ui, |ui| {
                ui.set_width(WIDTH - 12.0);
                let mut res = tree.ui(ui, th).map(Popup::Picked);
                ui.add_space(4.0);
                let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(line, 0.0, th.border);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let name = tree.selected().map(|p| p.display().to_string()).unwrap_or_default();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ok = ui.add_enabled(
                            tree.selected().is_some(),
                            egui::Button::new(egui::RichText::new(t("OK")).color(th.on_accent()).strong()).fill(th.accent).min_size(vec2(64.0, 26.0)),
                        );
                        let cancel = ui.add(egui::Button::new(t("Cancel")).min_size(vec2(64.0, 26.0)));
                        tree.buttons = Some((ok.rect, cancel.rect));
                        ui.add(egui::Label::new(egui::RichText::new(name.clone()).size(11.5).color(th.weak)).truncate()).on_hover_text(name);
                        if ok.clicked() {
                            res = tree.selected().map(|p| Popup::Picked(p.to_path_buf()));
                        }
                        if cancel.clicked() {
                            res = Some(Popup::Closed);
                        }
                    });
                });
                res
            })
            .inner
    });
    if let Some(p) = shown.inner {
        return p;
    }
    let press = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None });
    match press {
        Some(p) if !shown.response.rect.contains(p) && !anchor.contains(p) => Popup::Closed,
        _ => Popup::Open,
    }
}

/// The "Folder tree" button: two folders joined by a branch.
pub fn icon(p: &egui::Painter, r: Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.3_f32, c);
    let u = r.width() / 14.0;
    let at = |x: f32, y: f32| pos2(r.left() + x * u, r.top() + y * u);
    let box_at = |x: f32, y: f32| Rect::from_min_size(at(x, y), vec2(6.0 * u, 4.0 * u));
    p.rect_stroke(box_at(0.5, 0.5), 1.0, s, egui::StrokeKind::Inside);
    p.rect_stroke(box_at(7.0, 5.0), 1.0, s, egui::StrokeKind::Inside);
    p.rect_stroke(box_at(7.0, 9.5), 1.0, s, egui::StrokeKind::Inside);
    p.line_segment([at(3.0, 4.5), at(3.0, 11.5)], s);
    p.line_segment([at(3.0, 7.0), at(7.0, 7.0)], s);
    p.line_segment([at(3.0, 11.5), at(7.0, 11.5)], s);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(roots: &[(&str, &str)], current: Option<&str>) -> FolderTree {
        let roots = roots.iter().map(|(n, p)| Root { name: n.to_string(), path: PathBuf::from(p), drive: *p == "/" }).collect();
        FolderTree::new(roots, current.map(Path::new), false)
    }

    fn shown(t: &FolderTree) -> Vec<(String, usize, Kids)> {
        t.visible().into_iter().map(|r| (r.path.display().to_string(), r.depth, r.kids)).collect()
    }

    #[test]
    fn opens_down_to_the_current_folder() {
        let mut t = tree(&[("Home", "/home/me"), ("Disk", "/")], Some("/home/me/src/app"));
        assert_eq!(t.current(), Some(Path::new("/home/me/src/app")));
        // The home folder is the deepest root holding it: the drive stays closed.
        assert_eq!(shown(&t), [("/home/me".to_string(), 0, Kids::Loading), ("/".to_string(), 0, Kids::Unknown)]);
        t.set_kids("/home/me".into(), vec!["docs".into(), "src".into()]);
        t.set_kids("/home/me/src".into(), vec!["app".into(), "lib".into()]);
        assert_eq!(
            shown(&t),
            [
                ("/home/me".to_string(), 0, Kids::Some),
                ("/home/me/docs".to_string(), 1, Kids::Unknown),
                ("/home/me/src".to_string(), 1, Kids::Some),
                ("/home/me/src/app".to_string(), 2, Kids::Loading),
                ("/home/me/src/lib".to_string(), 2, Kids::Unknown),
                ("/".to_string(), 0, Kids::Unknown),
            ]
        );
        t.set_kids("/home/me/src/app".into(), vec![]);
        assert_eq!(t.visible()[3].kids, Kids::None, "no subfolders: no chevron");
    }

    #[test]
    fn outside_home_opens_the_drive() {
        let t = tree(&[("Home", "/home/me"), ("Disk", "/")], Some("/var/log"));
        let open: Vec<_> = t.visible().into_iter().filter(|r| r.open).map(|r| r.path).collect();
        assert_eq!(open, [PathBuf::from("/")]);
        assert!(t.open.contains(Path::new("/var")) && t.open.contains(Path::new("/var/log")));
    }

    #[test]
    fn no_current_folder_opens_the_first_root() {
        let t = tree(&[("Home", "/home/me"), ("Disk", "/")], None);
        assert_eq!(t.current(), None);
        assert!(t.visible()[0].open);
        // A folder under no root (a remote one) is like none.
        let t = tree(&[("Home", "/home/me")], Some("/elsewhere"));
        assert_eq!(t.current(), None);
        assert!(t.visible()[0].open);
    }

    /// Closing a branch above the chosen folder chooses the folder closed, but that is
    /// not a click: a dialog that follows the tree keeps its destination.
    #[test]
    fn closing_a_branch_is_not_a_click() {
        let mut t = tree(&[("Home", "/home/me")], Some("/home/me/a"));
        t.set_kids("/home/me".into(), vec!["a".into()]);
        t.toggle(Path::new("/home/me"));
        assert_eq!(t.selected(), Some(Path::new("/home/me")));
        assert_eq!(t.clicked, None);
    }

    #[test]
    fn toggling_closes_and_opens() {
        let mut t = tree(&[("Home", "/home/me")], Some("/home/me"));
        t.set_kids("/home/me".into(), vec!["a".into(), "b".into()]);
        assert_eq!(t.visible().len(), 3);
        t.toggle(Path::new("/home/me"));
        assert_eq!(t.visible().len(), 1, "closed");
        t.toggle(Path::new("/home/me"));
        assert_eq!(t.visible().len(), 3, "open again, without reading it again");
        t.toggle(Path::new("/home/me/b"));
        assert_eq!(t.visible()[2].kids, Kids::Loading);
    }

    #[test]
    fn the_keyboard_moves_the_choice() {
        let mut t = tree(&[("Home", "/home/me"), ("Disk", "/")], Some("/home/me/src"));
        t.set_kids("/home/me".into(), vec!["docs".into(), "src".into()]);
        t.set_kids("/home/me/src".into(), vec!["app".into()]);
        let sel = |t: &FolderTree| t.selected().map(|p| p.display().to_string());
        assert_eq!(sel(&t), Some("/home/me/src".into()), "the current folder is chosen");
        // ↑ ↓ go through the rows shown, and stop at the ends.
        t.step(-1);
        assert_eq!(sel(&t), Some("/home/me/docs".into()));
        t.step(-5);
        assert_eq!(sel(&t), Some("/home/me".into()));
        t.step(1);
        t.step(1);
        t.step(1);
        assert_eq!(sel(&t), Some("/home/me/src/app".into()));
        t.step(9);
        assert_eq!(sel(&t), Some("/".into()));
        // ← on an open folder closes it; on a closed one goes to its parent.
        t.select(Path::new("/home/me/src/app"));
        t.left();
        assert_eq!(sel(&t), Some("/home/me/src".into()));
        t.left();
        assert_eq!(sel(&t), Some("/home/me/src".into()));
        assert!(!t.visible()[2].open, "closed");
        t.left();
        assert_eq!(sel(&t), Some("/home/me".into()));
        // → opens a closed folder, and goes into an open one.
        t.select(Path::new("/home/me/src"));
        t.right();
        assert!(t.visible()[2].open, "opened");
        assert_eq!(sel(&t), Some("/home/me/src".into()));
        t.right();
        assert_eq!(sel(&t), Some("/home/me/src/app".into()));
        // A folder with no subfolders stays chosen.
        t.set_kids("/home/me/src/app".into(), vec![]);
        t.right();
        assert_eq!(sel(&t), Some("/home/me/src/app".into()));
        // Closing a folder holding the chosen row moves the choice to it.
        t.toggle(Path::new("/home/me"));
        assert_eq!(sel(&t), Some("/home/me".into()));
        // With no current folder, the first root is chosen.
        let t = tree(&[("Home", "/home/me")], None);
        assert_eq!(t.selected(), Some(Path::new("/home/me")));
    }

    /// The home folder is also inside the drive: the keys go on from the row chosen, not
    /// from the same folder higher up.
    #[test]
    fn a_folder_shown_twice() {
        let mut t = tree(&[("Home", "/home/me"), ("Disk", "/")], Some("/home/me"));
        t.set_kids("/home/me".into(), vec!["a".into(), "b".into()]);
        t.set_kids("/".into(), vec!["home".into()]);
        t.set_kids("/home".into(), vec!["me".into()]);
        t.toggle(Path::new("/"));
        t.toggle(Path::new("/home"));
        let paths: Vec<String> = t.visible().into_iter().map(|r| r.path.display().to_string()).collect();
        assert_eq!(paths, ["/home/me", "/home/me/a", "/home/me/b", "/", "/home", "/home/me", "/home/me/a", "/home/me/b"]);
        assert_eq!(t.selected_row(), Some(0), "the home folder at the top");
        t.select(Path::new("/home"));
        t.step(1);
        assert_eq!(t.selected_row(), Some(5), "the one inside the drive");
        t.step(1);
        t.step(1);
        assert_eq!(t.selected_row(), Some(7));
        t.step(1);
        assert_eq!(t.selected_row(), Some(7), "the last row stays chosen");
        t.left();
        assert_eq!(t.selected_row(), Some(5), "← goes to the folder holding it, down there");
        t.step(-1);
        assert_eq!(t.selected_row(), Some(4));
    }

    #[test]
    fn roots_are_home_then_drives() {
        let drive = |name: &str, mount: &str, system: bool| crate::files::drives::Drive {
            label: mount.into(),
            name: name.into(),
            system,
            mount: PathBuf::from(mount),
            total: 0,
            avail: 0,
            fs: String::new(),
            device: String::new(),
            removable: false,
            ejectable: false,
        };
        let drives =
            [drive("Macintosh HD", "/", false), drive("Data", "/System/Volumes/Data", true), drive("USB", "/Volumes/USB", false), drive("", "/mnt/x", false)];
        let r = roots(Some(Path::new("/Users/me")), &drives);
        let got: Vec<(&str, &str, bool)> = r.iter().map(|r| (r.name.as_str(), r.path.to_str().unwrap(), r.drive)).collect();
        let home = t("Home");
        assert_eq!(got, [(home.as_str(), "/Users/me", false), ("Macintosh HD", "/", true), ("USB", "/Volumes/USB", true), ("/mnt/x", "/mnt/x", true)]);
        assert_eq!(roots(None, &drives[..1]).len(), 1);
    }

    #[test]
    fn lists_folders_only_in_natural_order() {
        let d = std::env::temp_dir().join(format!("mf-foldertree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for n in ["b10", "b2", "A", ".hidden"] {
            std::fs::create_dir_all(d.join(n)).unwrap();
        }
        std::fs::write(d.join("file.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.join("A"), d.join("link")).unwrap();
        let mut want = vec!["A", "b2", "b10"];
        if cfg!(unix) {
            want.push("link");
        }
        assert_eq!(list_dirs(&d, false), want);
        assert!(list_dirs(&d, true).contains(&".hidden".to_string()));
        assert!(list_dirs(&d.join("missing"), false).is_empty());

        // Read on a thread when opened.
        let root = Root { name: "d".into(), path: d.clone(), drive: false };
        let mut t = FolderTree::new(vec![root], Some(&d), false);
        for _ in 0..200 {
            t.load(None);
            if t.visible().len() > 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let names: Vec<String> = t.visible().into_iter().skip(1).map(|r| r.name).collect();
        assert_eq!(names, want);
        let _ = std::fs::remove_dir_all(&d);
    }
}
