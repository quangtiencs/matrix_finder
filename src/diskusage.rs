//! Disk usage analyser, shown as a window in a tab: a background scan of a
//! folder (folders kept to a chosen depth, sizes always complete), drawn as a
//! squarified treemap or a sortable table, and exportable as CSV.
use crate::theme::Theme;
use crate::util::human_size;
use egui::{pos2, vec2, Color32, Rect, Sense, Stroke};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct Node {
    pub name: String,
    pub size: u64,
    pub files: u64,
    pub is_dir: bool,
    pub children: Vec<Node>,
    /// A folder below the scan depth: its size is complete, its contents were not kept.
    pub truncated: bool,
}

#[derive(Default)]
struct ScanProgress {
    files: AtomicU64,
    bytes: AtomicU64,
    current: Mutex<String>,
    cancel: AtomicBool,
}

/// Scan `path`. Everything below is counted, but only `depth` levels of
/// entries are kept (`depth` 0: none, just the totals).
fn scan_dir(path: &Path, name: String, dev: Option<u64>, depth: usize, p: &ScanProgress) -> Node {
    let mut node = Node { name, size: 0, files: 0, is_dir: true, children: Vec::new(), truncated: depth == 0 };
    if p.cancel.load(Ordering::Relaxed) {
        return node;
    }
    if let Ok(mut c) = p.current.try_lock() {
        *c = path.to_string_lossy().to_string();
    }
    let Ok(rd) = std::fs::read_dir(path) else { return node };
    for e in rd.flatten() {
        let Ok(meta) = e.path().symlink_metadata() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        if meta.is_dir() {
            if !same_device(&meta, dev) || is_virtual_fs(&e.path()) {
                continue;
            }
            let child = scan_dir(&e.path(), name, dev, depth.saturating_sub(1), p);
            node.size += child.size;
            node.files += child.files;
            if depth > 0 {
                node.children.push(child);
            }
        } else {
            let size = allocated_size(&meta);
            p.files.fetch_add(1, Ordering::Relaxed);
            p.bytes.fetch_add(size, Ordering::Relaxed);
            node.size += size;
            node.files += 1;
            if depth > 0 {
                node.children.push(Node { name, size, files: 1, is_dir: false, children: Vec::new(), truncated: false });
            }
        }
    }
    node.children.sort_by_key(|c| std::cmp::Reverse(c.size));
    node
}

#[cfg(unix)]
fn allocated_size(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    // Prefer the real on-disk size (sparse files), but never exceed apparent size by much.
    let blocks = m.blocks() * 512;
    if blocks == 0 { m.len() } else { blocks.min(m.len().max(4096)) }
}
#[cfg(not(unix))]
fn allocated_size(m: &std::fs::Metadata) -> u64 {
    m.len()
}

#[cfg(unix)]
fn device_of(p: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| m.dev())
}
#[cfg(not(unix))]
fn device_of(_p: &Path) -> Option<u64> {
    None
}

#[cfg(unix)]
fn same_device(m: &std::fs::Metadata, dev: Option<u64>) -> bool {
    use std::os::unix::fs::MetadataExt;
    dev.is_none_or(|d| m.dev() == d)
}
#[cfg(not(unix))]
fn same_device(_m: &std::fs::Metadata, _dev: Option<u64>) -> bool {
    true
}

fn is_virtual_fs(p: &Path) -> bool {
    cfg!(target_os = "linux") && matches!(p.to_str(), Some("/proc" | "/sys" | "/dev" | "/run"))
}

struct ScanJob {
    rx: Receiver<Node>,
    progress: Arc<ScanProgress>,
    path: PathBuf,
    depth: usize,
    started: Instant,
}

/// Depth choices; `usize::MAX` keeps everything.
pub const DEPTHS: [usize; 6] = [1, 2, 3, 4, 5, usize::MAX];
const MAX_TABLE_ROWS: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DuView {
    Treemap,
    Table,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DuSort {
    Name,
    Size,
    Files,
}

/// Requests from the view that need the app.
pub enum DuAction {
    /// Show this folder in a file tab.
    OpenFolder(PathBuf),
    /// A message for the user (text, is an error).
    Message(String, bool),
}

/// One disk usage view (the content of a window).
pub struct DiskUsage {
    job: Option<ScanJob>,
    /// Scanned folder, its tree, and the depth it was scanned to.
    root: Option<(PathBuf, Node, usize)>,
    /// Indices into children, from the root to the folder being shown.
    drill: Vec<usize>,
    pub depth: usize,
    pub view: DuView,
    sort: (DuSort, bool),
    /// Seconds the last scan took.
    took: f32,
    /// Folder being scanned or shown (also while no scan has finished).
    pub path: PathBuf,
}

impl Drop for DiskUsage {
    fn drop(&mut self) {
        if let Some(j) = &self.job {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl DiskUsage {
    /// A view of `path`, scanning `depth` levels (1 by default).
    pub fn new(path: PathBuf, depth: usize) -> Self {
        let mut d = Self { job: None, root: None, drill: vec![], depth: depth.max(1), view: DuView::Treemap, sort: (DuSort::Size, true), took: 0.0, path: path.clone() };
        d.start(path);
        d
    }

    pub fn is_scanning(&self) -> bool {
        self.job.is_some()
    }

    fn start(&mut self, path: PathBuf) {
        if let Some(j) = &self.job {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
        let progress = Arc::new(ScanProgress::default());
        let (tx, rx) = channel();
        let p = progress.clone();
        let pp = path.clone();
        let depth = self.depth;
        std::thread::spawn(move || {
            let dev = device_of(&pp);
            let node = scan_dir(&pp, pp.to_string_lossy().to_string(), dev, depth, &p);
            let _ = tx.send(node);
        });
        self.path = path.clone();
        self.job = Some(ScanJob { rx, progress, path, depth, started: Instant::now() });
    }

    /// Scan the shown folder again (also after changing the depth).
    pub fn rescan(&mut self) {
        let p = self.path.clone();
        self.start(p);
    }

    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        if let Ok(node) = job.rx.try_recv() {
            self.took = job.started.elapsed().as_secs_f32();
            self.root = Some((job.path.clone(), node, job.depth));
            self.drill.clear();
            self.job = None;
        }
    }

    /// The folder shown, its path, and how many levels below it were kept.
    fn current(&self) -> Option<(&Node, PathBuf, usize)> {
        let (base, root, depth) = self.root.as_ref()?;
        let mut node = root;
        let mut path = base.clone();
        for &i in &self.drill {
            node = node.children.get(i)?;
            path = path.join(&node.name);
        }
        Some((node, path, depth.saturating_sub(self.drill.len())))
    }

    /// Show a child folder: from the tree when it was kept, else by scanning it.
    fn open_child(&mut self, i: usize) {
        let Some((node, path, _)) = self.current() else { return };
        let Some(c) = node.children.get(i).filter(|c| c.is_dir) else { return };
        let (truncated, p) = (c.truncated, path.join(&c.name));
        if truncated {
            self.start(p);
        } else {
            self.drill.push(i);
            self.path = p;
        }
    }

    /// One folder up: within the tree, or by scanning the parent folder.
    pub fn go_up(&mut self) {
        if self.drill.pop().is_some() {
            if let Some((_, p, _)) = self.current() {
                self.path = p;
            }
        } else if let Some(parent) = self.path.parent().map(Path::to_path_buf) {
            self.start(parent);
        }
    }

    /// Change the depth; scan again only when more levels are needed than were kept.
    pub fn set_depth(&mut self, depth: usize) {
        self.depth = depth.max(1);
        // Levels the running scan will keep, or the shown tree has.
        let kept = match &self.job {
            Some(j) => j.depth,
            None => self.current().map(|(_, _, d)| d).unwrap_or(0),
        };
        if self.depth > kept {
            self.rescan();
        }
    }

    /// Rows of the table: (depth below the shown folder, node, path), children
    /// sorted, folders opened up to the chosen depth.
    fn rows<'a>(&self, node: &'a Node, path: &Path) -> Vec<(usize, &'a Node, PathBuf)> {
        let mut out = Vec::new();
        self.collect(node, path, 0, &mut out);
        out
    }

    fn collect<'a>(&self, node: &'a Node, path: &Path, level: usize, out: &mut Vec<(usize, &'a Node, PathBuf)>) {
        if level >= self.depth || out.len() >= MAX_TABLE_ROWS {
            return;
        }
        let mut kids: Vec<&Node> = node.children.iter().collect();
        let (key, desc) = self.sort;
        kids.sort_by(|a, b| {
            let o = match key {
                DuSort::Name => crate::pane::natural_cmp(&a.name.to_lowercase(), &b.name.to_lowercase()),
                DuSort::Size => a.size.cmp(&b.size),
                DuSort::Files => a.files.cmp(&b.files),
            };
            if desc { o.reverse() } else { o }
        });
        for k in kids {
            if out.len() >= MAX_TABLE_ROWS {
                return;
            }
            let p = path.join(&k.name);
            out.push((level, k, p.clone()));
            if k.is_dir {
                self.collect(k, &p, level + 1, out);
            }
        }
    }

    /// The table as CSV: path, name, type, size in bytes, readable size, files,
    /// share of the shown folder.
    pub fn csv(&self) -> Option<String> {
        let (node, path, _) = self.current()?;
        let mut w = csv::Writer::from_writer(Vec::new());
        w.write_record(["path", "name", "type", "level", "size_bytes", "size", "files", "percent"]).ok()?;
        let total = node.size.max(1) as f64;
        for (level, n, p) in self.rows(node, &path) {
            w.write_record([
                p.to_string_lossy().as_ref(),
                n.name.as_str(),
                if n.is_dir { "folder" } else { "file" },
                &(level + 1).to_string(),
                &n.size.to_string(),
                &human_size(n.size),
                &n.files.to_string(),
                &format!("{:.2}", n.size as f64 * 100.0 / total),
            ])
            .ok()?;
        }
        String::from_utf8(w.into_inner().ok()?).ok()
    }

    /// Summary for the info bar: "1.2 GB · 3,400 files" (or the scan progress).
    pub fn summary(&self, t: &dyn Fn(&str) -> String) -> String {
        if let Some(j) = &self.job {
            let p = &j.progress;
            return format!("{} {} {} · {}", t("Scanning…"), p.files.load(Ordering::Relaxed), t("files"), human_size(p.bytes.load(Ordering::Relaxed)));
        }
        match self.current() {
            Some((n, _, _)) => format!("{} · {} {} · {:.1} s", human_size(n.size), n.files, t("files"), self.took),
            None => String::new(),
        }
    }

    pub fn cancel(&mut self) {
        if let Some(j) = self.job.take() {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The view below the info bar: progress while scanning, else the treemap or the table.
    pub fn ui(&mut self, ui: &mut egui::Ui, theme: &Theme, t: &dyn Fn(&str) -> String) -> Option<DuAction> {
        self.poll();
        if let Some(job) = &self.job {
            let p = &job.progress;
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.label(format!("{} {} {} · {}", t("Scanning…"), p.files.load(Ordering::Relaxed), t("files"), human_size(p.bytes.load(Ordering::Relaxed))));
                if let Ok(c) = p.current.try_lock() {
                    ui.add(egui::Label::new(egui::RichText::new(c.as_str()).small().color(theme.weak)).truncate());
                }
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            return None;
        }
        let Some((node, path, _)) = self.current() else {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(egui::RichText::new(t("Nothing to show")).color(theme.weak)));
            return None;
        };
        let mut act = Act::None;
        let mut action = None;
        let rect = ui.available_rect_before_wrap().shrink2(vec2(6.0, 4.0));
        match self.view {
            DuView::Treemap => treemap(ui, rect, node, &path, theme, t, &mut act),
            DuView::Table => {
                let rows = self.rows(node, &path);
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
                table(&mut child, node, &rows, self.sort, theme, t, &mut act);
            }
        }
        ui.advance_cursor_after_rect(rect);
        match act {
            Act::None => {}
            Act::Open(i) => self.open_child(i),
            Act::OpenPath(p) => {
                // A folder deeper in the table: scan it as the shown folder.
                if p.is_dir() {
                    self.start(p);
                }
            }
            Act::Up => self.go_up(),
            Act::Sort(k) => {
                self.sort = if self.sort.0 == k { (k, !self.sort.1) } else { (k, k != DuSort::Name) };
            }
            Act::Folder(p) => action = Some(DuAction::OpenFolder(p)),
            Act::Reveal(p) => {
                if let Err(e) = crate::terminal::reveal(&p) {
                    action = Some(DuAction::Message(e, true));
                }
            }
        }
        action
    }
}

/// What a click in the treemap or the table asks for.
enum Act {
    None,
    /// Open child `i` of the shown folder.
    Open(usize),
    OpenPath(PathBuf),
    Up,
    Sort(DuSort),
    Folder(PathBuf),
    Reveal(PathBuf),
}

/// Right-click menu of an entry (treemap tile or table row).
fn entry_menu(ui: &mut egui::Ui, n: &Node, p: &Path, t: &dyn Fn(&str) -> String, act: &mut Act) {
    let folder = if n.is_dir { p.to_path_buf() } else { p.parent().map(Path::to_path_buf).unwrap_or_default() };
    if ui.button(t("Show in a file tab")).clicked() {
        *act = Act::Folder(folder);
        ui.close_menu();
    }
    if ui.button(t("Show in system file manager")).clicked() {
        *act = Act::Reveal(p.to_path_buf());
        ui.close_menu();
    }
    if ui.button(t("Copy path")).clicked() {
        ui.ctx().copy_text(p.to_string_lossy().to_string());
        ui.close_menu();
    }
}

fn treemap(ui: &mut egui::Ui, rect: Rect, node: &Node, path: &Path, theme: &Theme, t: &dyn Fn(&str) -> String, act: &mut Act) {
    let resp = ui.interact(rect, ui.id().with("du-treemap"), Sense::click());
    let painter = ui.painter_at(rect);
    let kids: Vec<(usize, &Node)> = node.children.iter().enumerate().filter(|(_, c)| c.size > 0).take(400).collect();
    if kids.is_empty() {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, t("Empty folder"), egui::FontId::proportional(14.0), theme.weak);
    }
    let rects = squarify(&kids.iter().map(|(_, k)| k.size as f64).collect::<Vec<_>>(), rect);
    let hover = resp.hover_pos();
    let mut hovered: Option<(usize, &Node)> = None;
    for ((i, k), r) in kids.iter().zip(&rects) {
        let base = color_for(*i, k, theme);
        painter.rect_filled(r.shrink(1.5), 5.0, base);
        // A second level when the scan kept it (depth 2 and more).
        if k.is_dir && r.width() > 40.0 && r.height() > 40.0 {
            let inner = Rect::from_min_max(r.min + vec2(3.0, 18.0), r.max - vec2(3.0, 3.0));
            let gk: Vec<&Node> = k.children.iter().filter(|c| c.size > 0).take(60).collect();
            let gr = squarify(&gk.iter().map(|g| g.size as f64).collect::<Vec<_>>(), inner);
            for (g, rr) in gk.iter().zip(&gr) {
                let shade = if theme.dark { Color32::from_black_alpha(if g.is_dir { 40 } else { 70 }) } else { Color32::from_white_alpha(if g.is_dir { 70 } else { 120 }) };
                painter.rect_filled(rr.shrink(1.0), 3.0, shade);
            }
        }
        if r.width() > 50.0 && r.height() > 16.0 {
            let txt = format!("{}   {}", k.name, human_size(k.size));
            let text_color = if theme.dark { Color32::from_gray(240) } else { Color32::from_gray(25) };
            painter.with_clip_rect(r.shrink(2.0)).text(r.min + vec2(7.0, 4.0), egui::Align2::LEFT_TOP, txt, egui::FontId::proportional(12.0), text_color);
        }
        if hover.is_some_and(|h| r.contains(h)) {
            hovered = Some((*i, k));
            painter.rect_stroke(r.shrink(1.5), 5.0, Stroke::new(2.0_f32, theme.strong()), egui::StrokeKind::Inside);
        }
    }
    if let Some((i, k)) = hovered {
        let pct = if node.size > 0 { k.size as f64 * 100.0 / node.size as f64 } else { 0.0 };
        let hint = if k.is_dir { format!("\n{}", t("Click to open · right-click for more")) } else { String::new() };
        let resp = resp.clone().on_hover_text(format!("{}\n{} ({:.1}%)\n{} {}{hint}", k.name, human_size(k.size), pct, k.files, t("files")));
        if resp.clicked() && k.is_dir {
            *act = Act::Open(i);
        }
        let p = path.join(&k.name);
        resp.context_menu(|ui| entry_menu(ui, k, &p, t, act));
    }
}

#[allow(clippy::too_many_arguments)]
fn table(ui: &mut egui::Ui, node: &Node, rows: &[(usize, &Node, PathBuf)], sort: (DuSort, bool), theme: &Theme, t: &dyn Fn(&str) -> String, act: &mut Act) {
    let w = ui.available_width();
    let (size_w, share_w, files_w) = (90.0, (w * 0.22).clamp(90.0, 200.0), 80.0);
    let name_w = (w - size_w - share_w - files_w - 24.0).max(120.0);
    // Header: click to sort.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (key, label, width, right) in [(DuSort::Name, t("Name"), name_w, false), (DuSort::Size, t("Size"), size_w, true)] {
            header_cell(ui, theme, &label, width, right, sort, key, act);
        }
        let (r, _) = ui.allocate_exact_size(vec2(share_w, 24.0), Sense::hover());
        ui.painter().text(pos2(r.left() + 12.0, r.center().y), egui::Align2::LEFT_CENTER, t("Share"), egui::FontId::proportional(12.0), theme.weak);
        header_cell(ui, theme, &t("Files"), files_w, true, sort, DuSort::Files, act);
    });
    let sep = ui.cursor().top();
    ui.painter().hline(ui.max_rect().x_range(), sep, Stroke::new(1.0_f32, theme.border));
    let total = node.size.max(1) as f32;
    egui::ScrollArea::vertical().id_salt("du-table").auto_shrink([false, false]).show_rows(ui, 28.0, rows.len(), |ui, range| {
        for (level, n, p) in &rows[range] {
            let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
            let painter = ui.painter();
            if resp.hovered() {
                painter.rect_filled(r, 5.0, theme.hover());
            }
            let indent = *level as f32 * 16.0;
            let icon_r = Rect::from_center_size(pos2(r.left() + 14.0 + indent, r.center().y), vec2(15.0, 15.0));
            let ext = if n.is_dir { String::new() } else { crate::util::ext_of(&n.name) };
            crate::icons::file_icon(painter, icon_r, &n.name, n.is_dir, &ext, theme);
            let name_clip = Rect::from_min_max(pos2(r.left(), r.top()), pos2(r.left() + name_w - 8.0, r.bottom()));
            painter.with_clip_rect(name_clip).text(pos2(icon_r.right() + 8.0, r.center().y), egui::Align2::LEFT_CENTER, &n.name, egui::FontId::proportional(13.0), theme.text);
            let x = r.left() + name_w;
            painter.text(pos2(x + size_w - 6.0, r.center().y), egui::Align2::RIGHT_CENTER, human_size(n.size), egui::FontId::proportional(12.5), theme.text);
            // Share of the shown folder, as a bar and a number.
            let frac = n.size as f32 / total;
            let track = Rect::from_min_size(pos2(x + size_w + 12.0, r.center().y - 3.0), vec2(share_w - 64.0, 6.0));
            painter.rect_filled(track, 3.0, theme.surface);
            painter.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * frac.clamp(0.0, 1.0), 6.0)), 3.0, theme.accent.gamma_multiply(0.85));
            painter.text(pos2(track.right() + 46.0, r.center().y), egui::Align2::RIGHT_CENTER, format!("{:.1}%", frac * 100.0), egui::FontId::proportional(11.5), theme.weak);
            let files = if n.is_dir { n.files.to_string() } else { String::new() };
            painter.text(pos2(x + size_w + share_w + files_w - 6.0, r.center().y), egui::Align2::RIGHT_CENTER, files, egui::FontId::proportional(12.5), theme.weak);
            if resp.double_clicked() && n.is_dir {
                *act = if *level == 0 { node.children.iter().position(|c| std::ptr::eq(c, *n)).map(Act::Open).unwrap_or(Act::None) } else { Act::OpenPath(p.clone()) };
            }
            let resp = resp.on_hover_text(p.to_string_lossy());
            resp.context_menu(|ui| entry_menu(ui, n, p, t, act));
        }
    });
    if rows.len() >= MAX_TABLE_ROWS {
        ui.label(egui::RichText::new(format!("{} {MAX_TABLE_ROWS} {}", t("Showing the first"), t("rows"))).small().color(theme.weak));
    }
    if ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Extra1)) {
        *act = Act::Up;
    }
}

#[allow(clippy::too_many_arguments)]
fn header_cell(ui: &mut egui::Ui, theme: &Theme, label: &str, width: f32, right: bool, sort: (DuSort, bool), key: DuSort, act: &mut Act) {
    let (r, resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    let on = sort.0 == key;
    let col = if on || resp.hovered() { theme.accent } else { theme.weak };
    let text = if on { format!("{label} {}", if sort.1 { "▼" } else { "▲" }) } else { label.to_string() };
    let (pos, align) = if right { (pos2(r.right() - 6.0, r.center().y), egui::Align2::RIGHT_CENTER) } else { (pos2(r.left() + 30.0, r.center().y), egui::Align2::LEFT_CENTER) };
    ui.painter().text(pos, align, text, egui::FontId::proportional(12.0), col);
    if resp.clicked() {
        *act = Act::Sort(key);
    }
}

fn color_for(i: usize, n: &Node, theme: &Theme) -> Color32 {
    let hue = (i as f32 * 0.618_034).fract();
    let (s, v) = if theme.dark { (0.45, 0.20) } else { (0.30, 0.86) };
    let hsv = egui::ecolor::Hsva::new(hue, if n.is_dir { s } else { s * 0.5 }, v, 1.0);
    hsv.into()
}

/// Squarified treemap layout (Bruls, Huizing, van Wijk). `values` must be sorted descending.
pub fn squarify(values: &[f64], rect: Rect) -> Vec<Rect> {
    let n = values.len();
    let mut out = vec![Rect::NOTHING; n];
    let total: f64 = values.iter().sum();
    if n == 0 || total <= 0.0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return out;
    }
    let scale = (rect.width() as f64 * rect.height() as f64) / total;
    let areas: Vec<f64> = values.iter().map(|v| v * scale).collect();
    let mut r = rect;
    let mut i = 0;
    while i < n {
        let short = (r.width().min(r.height()) as f64).max(1e-6);
        let mut j = i + 1;
        let mut best = worst(&areas[i..j], short);
        while j < n {
            let w = worst(&areas[i..j + 1], short);
            if w > best {
                break;
            }
            best = w;
            j += 1;
        }
        let row_sum: f64 = areas[i..j].iter().sum();
        if r.width() >= r.height() {
            let w = (row_sum / r.height() as f64) as f32;
            let mut y = r.top();
            for k in i..j {
                let h = (areas[k] / w as f64) as f32;
                out[k] = Rect::from_min_size(pos2(r.left(), y), vec2(w, h));
                y += h;
            }
            r.min.x += w;
        } else {
            let h = (row_sum / r.width() as f64) as f32;
            let mut x = r.left();
            for k in i..j {
                let w = (areas[k] / h as f64) as f32;
                out[k] = Rect::from_min_size(pos2(x, r.top()), vec2(w, h));
                x += w;
            }
            r.min.y += h;
        }
        i = j;
    }
    out
}

fn worst(row: &[f64], short: f64) -> f64 {
    let s: f64 = row.iter().sum();
    let max = row.iter().cloned().fold(f64::MIN, f64::max);
    let min = row.iter().cloned().fold(f64::MAX, f64::min);
    let s2 = s * s;
    let w2 = short * short;
    (w2 * max / s2).max(s2 / (w2 * min))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn treemap_covers_area() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0));
        let vals = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let rs = squarify(&vals, rect);
        let area: f32 = rs.iter().map(|r| r.area()).sum();
        assert!((area - rect.area()).abs() < 1.0);
        for r in &rs {
            assert!(rect.expand(0.01).contains_rect(*r));
        }
    }

    fn wait(d: &mut DiskUsage) {
        let t0 = Instant::now();
        while d.is_scanning() && t0.elapsed().as_secs() < 10 {
            d.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn depth_keeps_levels_but_counts_everything() {
        let dir = std::env::temp_dir().join(format!("mf-du-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b/c")).unwrap();
        std::fs::write(dir.join("top.txt"), vec![1u8; 10]).unwrap();
        std::fs::write(dir.join("a/b/c/deep.bin"), vec![1u8; 5000]).unwrap();
        // Asking for more depth while the first scan runs scans again, deeper.
        let mut d = DiskUsage::new(dir.clone(), 1);
        d.set_depth(2);
        wait(&mut d);
        assert_eq!(d.current().unwrap().2, 2);
        let mut d = DiskUsage::new(dir.clone(), 1);
        wait(&mut d);
        let (node, _, kept) = d.current().unwrap();
        assert_eq!(kept, 1);
        assert_eq!(node.files, 2, "files below the depth still count");
        let a = node.children.iter().find(|c| c.name == "a").unwrap();
        assert!(a.truncated && a.children.is_empty() && a.files == 1);
        assert_eq!(d.rows(node, &dir).len(), 2, "depth 1: just the folder's own entries");
        // A truncated folder is scanned when opened; more depth rescans.
        let i = node.children.iter().position(|c| c.name == "a").unwrap();
        d.open_child(i);
        wait(&mut d);
        assert_eq!(d.path, dir.join("a"));
        d.go_up();
        wait(&mut d);
        assert_eq!(d.path, dir);
        d.set_depth(3);
        wait(&mut d);
        let (node, _, _) = d.current().unwrap();
        let names: Vec<String> = d.rows(node, &dir).iter().map(|(l, n, _)| format!("{l}:{}", n.name)).collect();
        assert!(names.contains(&"2:c".to_string()) && !names.iter().any(|n| n.ends_with("deep.bin")), "{names:?}");
        let csv = d.csv().unwrap();
        assert!(csv.starts_with("path,name,type,level,size_bytes"));
        assert!(csv.lines().any(|l| l.contains(",top.txt,file,1,")), "{csv}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
