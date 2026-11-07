//! Checksums: compute CRC32 / MD5 / SHA-1 / SHA-256 / SHA-512 / BLAKE3 for
//! files, compare with an expected value, write checksum files (GNU
//! `sha256sum` format, SFV for CRC32) and verify existing checksum files.
use crate::i18n::t;
use crate::theme::Theme;
use crate::util::human_size;
use egui::{Color32, RichText};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algo {
    Crc32,
    Md5,
    Sha1,
    Sha256,
    Sha512,
    Blake3,
}

impl Algo {
    pub const ALL: [Algo; 6] = [Algo::Crc32, Algo::Md5, Algo::Sha1, Algo::Sha256, Algo::Sha512, Algo::Blake3];

    pub fn label(self) -> &'static str {
        match self {
            Algo::Crc32 => "CRC32",
            Algo::Md5 => "MD5",
            Algo::Sha1 => "SHA-1",
            Algo::Sha256 => "SHA-256",
            Algo::Sha512 => "SHA-512",
            Algo::Blake3 => "BLAKE3",
        }
    }

    /// Tag used by the BSD format: `SHA256 (file) = …`.
    fn bsd_tag(self) -> &'static str {
        match self {
            Algo::Crc32 => "CRC32",
            Algo::Md5 => "MD5",
            Algo::Sha1 => "SHA1",
            Algo::Sha256 => "SHA256",
            Algo::Sha512 => "SHA512",
            Algo::Blake3 => "BLAKE3",
        }
    }

    /// Extension of the checksum file this algorithm writes.
    pub fn ext(self) -> &'static str {
        match self {
            Algo::Crc32 => "sfv",
            Algo::Md5 => "md5",
            Algo::Sha1 => "sha1",
            Algo::Sha256 => "sha256",
            Algo::Sha512 => "sha512",
            Algo::Blake3 => "b3",
        }
    }

    /// Length of the hex digest.
    pub fn hex_len(self) -> usize {
        match self {
            Algo::Crc32 => 8,
            Algo::Md5 => 32,
            Algo::Sha1 => 40,
            Algo::Sha256 | Algo::Blake3 => 64,
            Algo::Sha512 => 128,
        }
    }

    fn from_tag(tag: &str) -> Option<Algo> {
        let tag = tag.to_ascii_uppercase().replace('-', "");
        Algo::ALL.into_iter().find(|a| a.bsd_tag() == tag)
    }

    /// Algorithm implied by a checksum file name (`x.sha256`, `SHA256SUMS`, `x.md5sum`…).
    pub fn from_file_name(name: &str) -> Option<Algo> {
        let lower = name.to_ascii_lowercase();
        let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        let by_ext = match ext.trim_end_matches("sum") {
            "md5" => Some(Algo::Md5),
            "sha1" => Some(Algo::Sha1),
            "sha256" => Some(Algo::Sha256),
            "sha512" => Some(Algo::Sha512),
            "b3" | "blake3" => Some(Algo::Blake3),
            "sfv" | "crc32" | "crc" => Some(Algo::Crc32),
            _ => None,
        };
        by_ext.or(match lower.as_str() {
            "md5sums" => Some(Algo::Md5),
            "sha1sums" => Some(Algo::Sha1),
            "sha256sums" => Some(Algo::Sha256),
            "sha512sums" => Some(Algo::Sha512),
            "b3sums" | "blake3sums" => Some(Algo::Blake3),
            _ => None,
        })
    }

    /// Best guess for a hex digest of this length (SHA-256 for 64 digits,
    /// unless `prefer` is BLAKE3, which has the same length).
    pub fn guess(hex: &str, prefer: Algo) -> Option<Algo> {
        let n = hex.len();
        if prefer.hex_len() == n {
            return Some(prefer);
        }
        Algo::ALL.into_iter().find(|a| a.hex_len() == n)
    }
}

/// Is this file a checksum list that can be verified?
pub fn is_checksum_file(name: &str) -> bool {
    Algo::from_file_name(name).is_some() || name.eq_ignore_ascii_case("CHECKSUMS")
}

/// Lower-case hex digits only, or `None` if `s` is not a hex string.
pub fn clean_hex(s: &str) -> Option<String> {
    let s = s.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    (!s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
}

enum Hasher {
    Crc32(crc32fast::Hasher),
    Md5(md5::Md5),
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
    Blake3(Box<blake3::Hasher>),
}

impl Hasher {
    fn new(a: Algo) -> Self {
        use sha2::Digest;
        match a {
            Algo::Crc32 => Hasher::Crc32(crc32fast::Hasher::new()),
            Algo::Md5 => Hasher::Md5(md5::Md5::new()),
            Algo::Sha1 => Hasher::Sha1(sha1::Sha1::new()),
            Algo::Sha256 => Hasher::Sha256(sha2::Sha256::new()),
            Algo::Sha512 => Hasher::Sha512(sha2::Sha512::new()),
            Algo::Blake3 => Hasher::Blake3(Box::new(blake3::Hasher::new())),
        }
    }
    fn update(&mut self, b: &[u8]) {
        use sha2::Digest;
        match self {
            Hasher::Crc32(h) => h.update(b),
            Hasher::Md5(h) => h.update(b),
            Hasher::Sha1(h) => h.update(b),
            Hasher::Sha256(h) => h.update(b),
            Hasher::Sha512(h) => h.update(b),
            Hasher::Blake3(h) => {
                h.update(b);
            }
        }
    }
    fn finish(self) -> String {
        use sha2::Digest;
        fn hex(b: &[u8]) -> String {
            b.iter().map(|x| format!("{x:02x}")).collect()
        }
        match self {
            Hasher::Crc32(h) => format!("{:08x}", h.finalize()),
            Hasher::Md5(h) => hex(&h.finalize()),
            Hasher::Sha1(h) => hex(&h.finalize()),
            Hasher::Sha256(h) => hex(&h.finalize()),
            Hasher::Sha512(h) => hex(&h.finalize()),
            Hasher::Blake3(h) => h.finalize().to_hex().to_string(),
        }
    }
}

/// Hash everything `r` yields. `tick(n)` is called after each block with the
/// number of bytes read; returning `false` cancels.
pub fn hash_reader(mut r: impl Read, algo: Algo, mut tick: impl FnMut(u64) -> bool) -> Result<String, String> {
    let mut h = Hasher::new(algo);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        h.update(&buf[..n]);
        if !tick(n as u64) {
            return Err(t("Cancelled"));
        }
    }
    Ok(h.finish())
}

#[cfg(test)]
fn hash_bytes(b: &[u8], algo: Algo) -> String {
    hash_reader(b, algo, |_| true).unwrap_or_default()
}

/// One line of a checksum list.
#[derive(Clone, Debug, PartialEq)]
pub struct ListEntry {
    pub name: String,
    pub hash: String,
    pub algo: Algo,
}

/// Parse a checksum list: GNU (`hash  name`, `hash *name`), BSD
/// (`SHA256 (name) = hash`) or SFV (`name crc32`). `default` is the algorithm
/// implied by the file name, if any; otherwise it is guessed from the length.
pub fn parse_list(text: &str, default: Option<Algo>) -> Vec<ListEntry> {
    let mut out = Vec::new();
    let sfv = default == Some(Algo::Crc32);
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
            continue;
        }
        // BSD style.
        if let Some((head, hash)) = trimmed.rsplit_once(" = ").or_else(|| trimmed.rsplit_once(")= ")) {
            if let (Some(open), Some(hash)) = (head.find(" ("), clean_hex(hash)) {
                if let Some(algo) = Algo::from_tag(head[..open].trim()) {
                    let name = head[open + 2..].trim_end_matches(')').to_string();
                    out.push(ListEntry { name, hash, algo });
                    continue;
                }
            }
        }
        // SFV: name, space, 8 hex digits.
        let as_sfv = || -> Option<ListEntry> {
            let (name, crc) = trimmed.rsplit_once(char::is_whitespace)?;
            let hash = clean_hex(crc).filter(|h| h.len() == 8)?;
            Some(ListEntry { name: name.trim().to_string(), hash, algo: Algo::Crc32 })
        };
        if sfv {
            if let Some(e) = as_sfv() {
                out.push(e);
                continue;
            }
        }
        // GNU style; a leading backslash means the name is escaped.
        let (escaped, body) = match line.strip_prefix('\\') {
            Some(b) => (true, b),
            None => (false, line),
        };
        if let Some((hash, rest)) = body.split_once(' ') {
            if let Some(hash) = clean_hex(hash) {
                let name = rest.strip_prefix(['*', ' ']).unwrap_or(rest);
                let name = if escaped { name.replace("\\n", "\n").replace("\\\\", "\\") } else { name.to_string() };
                if let Some(algo) = default.filter(|a| a.hex_len() == hash.len()).or_else(|| Algo::guess(&hash, Algo::Sha256)) {
                    if !name.is_empty() {
                        out.push(ListEntry { name, hash, algo });
                        continue;
                    }
                }
            }
        }
        if !sfv {
            if let Some(e) = as_sfv() {
                out.push(e);
            }
        }
    }
    out
}

/// Text of a checksum file (GNU format; SFV for CRC32). Names use `/`.
pub fn format_list(algo: Algo, items: &[(String, String)]) -> String {
    let mut s = String::new();
    if algo == Algo::Crc32 {
        s.push_str("; Generated by Matrix Finder\n");
    }
    for (name, hash) in items {
        if algo == Algo::Crc32 {
            s.push_str(&format!("{name} {}\n", hash.to_ascii_uppercase()));
        } else if name.contains(['\\', '\n']) {
            s.push_str(&format!("\\{hash}  {}\n", name.replace('\\', "\\\\").replace('\n', "\\n")));
        } else {
            s.push_str(&format!("{hash}  {name}\n"));
        }
    }
    s
}

// ------------------------------------------------------------------ UI

#[derive(Clone, PartialEq)]
enum State {
    Pending,
    Done(String),
    Error(String),
    Missing,
}

struct Row {
    /// Name relative to the base folder, with `/` separators.
    name: String,
    path: PathBuf,
    size: u64,
    algo: Algo,
    expected: Option<String>,
    state: State,
}

impl Row {
    fn verdict(&self) -> Option<bool> {
        match (&self.state, &self.expected) {
            (State::Done(h), Some(e)) => Some(h == e),
            (State::Missing | State::Error(_), Some(_)) => Some(false),
            _ => None,
        }
    }
}

struct Worker {
    rx: Receiver<(usize, Result<String, String>)>,
    cancel: Arc<AtomicBool>,
    bytes: Arc<AtomicU64>,
    current: Arc<Mutex<String>>,
    total: u64,
    started: Instant,
}

#[derive(Default)]
pub struct ChecksumUi {
    pub open: bool,
    base: PathBuf,
    /// Checksum file being verified (verify mode).
    list_file: Option<PathBuf>,
    rows: Vec<Row>,
    algo: Option<Algo>,
    compare: String,
    worker: Option<Worker>,
    elapsed: f32,
    note: Option<(String, bool)>,
}

/// Files under `paths` (folders are walked), as (relative name, path, size).
fn collect(base: &Path, paths: &[PathBuf]) -> Vec<(String, PathBuf, u64)> {
    let mut out = Vec::new();
    let rel = |p: &Path| -> String {
        let r = p.strip_prefix(base).unwrap_or(p);
        r.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
    };
    for p in paths {
        if p.is_dir() {
            let mut files: Vec<_> = walkdir::WalkDir::new(p)
                .follow_links(false)
                .into_iter()
                .flatten()
                .filter(|e| e.file_type().is_file())
                .map(|e| {
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    (rel(e.path()), e.into_path(), size)
                })
                .collect();
            files.sort_by(|a, b| a.0.cmp(&b.0));
            out.extend(files);
        } else if p.is_file() {
            let size = p.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((rel(p), p.clone(), size));
        }
    }
    out
}

fn ok_color(th: &Theme) -> Color32 {
    if th.dark {
        Color32::from_rgb(0x5c, 0xc8, 0x7a)
    } else {
        Color32::from_rgb(0x1e, 0x8e, 0x3e)
    }
}

impl ChecksumUi {
    /// Hash `paths` (files or folders inside `base`).
    pub fn start_files(&mut self, ctx: &egui::Context, base: PathBuf, paths: Vec<PathBuf>) {
        self.stop();
        let algo = self.algo.unwrap_or(Algo::Sha256);
        self.rows = collect(&base, &paths)
            .into_iter()
            .map(|(name, path, size)| Row { name, path, size, algo, expected: None, state: State::Pending })
            .collect();
        self.base = base;
        self.list_file = None;
        self.algo = Some(algo);
        self.note = None;
        // A hash copied to the clipboard is probably what the user wants to compare with.
        self.compare = crate::termview::clipboard_get(false)
            .and_then(|c| clean_hex(&c))
            .filter(|h| Algo::ALL.iter().any(|a| a.hex_len() == h.len()))
            .unwrap_or_default();
        if let Some(a) = clean_hex(&self.compare).and_then(|h| Algo::guess(&h, algo)) {
            self.set_algo(a);
        }
        self.open = true;
        self.run(ctx);
    }

    /// Verify the checksum list `file`; names in it are relative to its folder.
    pub fn start_verify(&mut self, ctx: &egui::Context, file: PathBuf) -> Result<(), String> {
        let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
        let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let entries = parse_list(&text, Algo::from_file_name(&name));
        if entries.is_empty() {
            return Err(t("No checksums found in this file"));
        }
        self.stop();
        let base = file.parent().map(Path::to_path_buf).unwrap_or_default();
        self.rows = entries
            .into_iter()
            .map(|e| {
                let path = base.join(e.name.trim_start_matches("./"));
                let meta = path.metadata().ok().filter(|m| m.is_file());
                Row {
                    size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                    state: if meta.is_some() { State::Pending } else { State::Missing },
                    name: e.name,
                    path,
                    algo: e.algo,
                    expected: Some(e.hash),
                }
            })
            .collect();
        self.base = base;
        self.list_file = Some(file);
        self.algo = None;
        self.compare.clear();
        self.note = None;
        self.open = true;
        self.run(ctx);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(w) = self.worker.take() {
            w.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn set_algo(&mut self, a: Algo) {
        if self.algo != Some(a) {
            self.algo = Some(a);
            for r in &mut self.rows {
                r.algo = a;
                r.state = State::Pending;
            }
        }
    }

    /// Hash every pending row in a background thread.
    fn run(&mut self, ctx: &egui::Context) {
        self.stop();
        let jobs: Vec<(usize, PathBuf, Algo)> =
            self.rows.iter().enumerate().filter(|(_, r)| r.state == State::Pending).map(|(i, r)| (i, r.path.clone(), r.algo)).collect();
        if jobs.is_empty() {
            return;
        }
        let total = jobs.iter().map(|(i, ..)| self.rows[*i].size).sum();
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(AtomicU64::new(0));
        let current = Arc::new(Mutex::new(String::new()));
        let names: Vec<String> = jobs.iter().map(|(i, ..)| self.rows[*i].name.clone()).collect();
        let (c2, b2, cur2, ctx2) = (cancel.clone(), bytes.clone(), current.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut last = Instant::now();
            for ((i, path, algo), name) in jobs.into_iter().zip(names) {
                if c2.load(Ordering::Relaxed) {
                    return;
                }
                if let Ok(mut c) = cur2.lock() {
                    *c = name;
                }
                let res = std::fs::File::open(&path).map_err(|e| e.to_string()).and_then(|f| {
                    hash_reader(std::io::BufReader::with_capacity(1 << 20, f), algo, |n| {
                        b2.fetch_add(n, Ordering::Relaxed);
                        if last.elapsed().as_millis() > 100 {
                            last = Instant::now();
                            ctx2.request_repaint();
                        }
                        !c2.load(Ordering::Relaxed)
                    })
                });
                if c2.load(Ordering::Relaxed) || tx.send((i, res)).is_err() {
                    return;
                }
                ctx2.request_repaint();
            }
        });
        self.worker = Some(Worker { rx, cancel, bytes, current, total, started: Instant::now() });
    }

    fn poll(&mut self) {
        let Some(w) = &self.worker else { return };
        let mut finished = false;
        loop {
            match w.rx.try_recv() {
                Ok((i, res)) => {
                    if let Some(r) = self.rows.get_mut(i) {
                        r.state = match res {
                            Ok(h) => State::Done(h),
                            Err(e) => State::Error(e),
                        };
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.elapsed = w.started.elapsed().as_secs_f32();
            self.worker = None;
        }
    }

    fn done_list(&self) -> Vec<(String, String)> {
        self.rows
            .iter()
            .filter_map(|r| match &r.state {
                State::Done(h) => Some((r.name.clone(), h.clone())),
                _ => None,
            })
            .collect()
    }

    /// Default name of the checksum file to write.
    fn default_list_name(&self, algo: Algo) -> String {
        let stem = match self.rows.as_slice() {
            [one] => one.name.rsplit('/').next().unwrap_or(&one.name).to_string(),
            _ => self.base.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "checksums".into()),
        };
        format!("{stem}.{}", algo.ext())
    }

    /// Returns the folder that changed (a checksum file was saved there).
    pub fn ui(&mut self, ctx: &egui::Context, th: &Theme) -> Option<PathBuf> {
        if !self.open {
            return None;
        }
        self.poll();
        let mut changed = None;
        let mut open = self.open;
        let mut rerun = false;
        let verify = self.list_file.is_some();
        let title = if verify { t("Verify checksums") } else { t("Checksum") };
        egui::Window::new(title)
            .id(egui::Id::new("checksum-window"))
            .open(&mut open)
            .collapsible(false)
            .default_size([860.0, 480.0])
            .min_width(560.0)
            .max_height(ctx.screen_rect().height() - 24.0)
            .show(ctx, |ui| {
                let total: u64 = self.rows.iter().map(|r| r.size).sum();
                let sub = match &self.list_file {
                    Some(f) => format!(
                        "{} · {} {} · {}",
                        f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        self.rows.len(),
                        t("file(s)"),
                        self.base.display()
                    ),
                    None => format!("{} {} · {} · {}", self.rows.len(), t("file(s)"), human_size(total), self.base.display()),
                };
                ui.add(egui::Label::new(RichText::new(sub).size(11.5).color(th.weak)).truncate());
                ui.add_space(4.0);

                // Algorithm and "compare with".
                let expected = clean_hex(&self.compare);
                if !verify {
                    ui.horizontal(|ui| {
                        ui.label(t("Algorithm"));
                        let cur = self.algo.unwrap_or(Algo::Sha256);
                        for a in Algo::ALL {
                            if ui.selectable_label(cur == a, a.label()).clicked() && cur != a {
                                self.set_algo(a);
                                rerun = true;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(t("Compare with"));
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.compare)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(ui.available_width() - 90.0)
                                .hint_text(RichText::new(t("Paste the expected checksum")).color(th.weak)),
                        );
                        if r.changed() {
                            // Switch to the algorithm that matches the pasted value.
                            if let Some(a) = clean_hex(&self.compare).and_then(|h| Algo::guess(&h, self.algo.unwrap_or(Algo::Sha256))) {
                                if Some(a) != self.algo {
                                    self.set_algo(a);
                                    rerun = true;
                                }
                            }
                        }
                        if ui.button(t("Paste")).clicked() {
                            if let Some(c) = crate::termview::clipboard_get(false) {
                                self.compare = c.trim().to_string();
                                if let Some(a) = clean_hex(&self.compare).and_then(|h| Algo::guess(&h, self.algo.unwrap_or(Algo::Sha256))) {
                                    if Some(a) != self.algo {
                                        self.set_algo(a);
                                        rerun = true;
                                    }
                                }
                            }
                        }
                    });
                    if !self.compare.trim().is_empty() {
                        let (text, color) = match &expected {
                            None => (t("Not a valid checksum (hex digits only)"), th.danger),
                            Some(e) => match self.rows.iter().find(|r| r.state == State::Done(e.clone())) {
                                Some(r) => (format!("✔ {} {}", t("Matches"), r.name), ok_color(th)),
                                None if self.worker.is_some() => (t("Calculating…"), th.weak),
                                None => (format!("✘ {}", t("No file matches this checksum")), th.danger),
                            },
                        };
                        ui.label(RichText::new(text).strong().color(color));
                    }
                }
                ui.add_space(4.0);

                // Progress.
                if let Some(w) = &self.worker {
                    let done = w.bytes.load(Ordering::Relaxed);
                    let frac = if w.total > 0 { done as f32 / w.total as f32 } else { 0.0 };
                    let secs = w.started.elapsed().as_secs_f32().max(0.001);
                    let cur = w.current.lock().map(|c| c.clone()).unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::ProgressBar::new(frac.min(1.0))
                                .desired_width(ui.available_width() - 90.0)
                                .text(format!("{} / {} · {}/s · {cur}", human_size(done), human_size(w.total), human_size((done as f32 / secs) as u64))),
                        );
                        if ui.button(t("Cancel")).clicked() {
                            w.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                }

                // Results table.
                let footer_h = 58.0;
                let table_h = (ui.available_height() - footer_h).clamp(120.0, 560.0);
                let match_hash = if verify { None } else { expected.clone() };
                let mut copy = None;
                egui::Frame::NONE.fill(th.bg).corner_radius(8).inner_margin(6.0).show(ui, |ui| {
                    ui.set_height(table_h);
                    use egui_extras::{Column, TableBuilder};
                    let w = ui.available_width();
                    let mono = egui::FontId::monospace(12.0);
                    TableBuilder::new(ui)
                        .striped(true)
                        .auto_shrink([false, false])
                        .max_scroll_height(table_h)
                        .column(Column::exact(22.0))
                        .column(Column::exact(((w - 120.0) * 0.38).max(140.0)).clip(true))
                        .column(Column::exact(78.0))
                        .column(Column::remainder().clip(true))
                        .header(22.0, |mut h| {
                            h.col(|_| {});
                            h.col(|ui| {
                                ui.label(RichText::new(t("Name")).size(11.5).color(th.weak));
                            });
                            h.col(|ui| {
                                ui.label(RichText::new(t("Size")).size(11.5).color(th.weak));
                            });
                            h.col(|ui| {
                                let a = if verify { String::new() } else { format!(" ({})", self.algo.unwrap_or(Algo::Sha256).label()) };
                                ui.label(RichText::new(format!("{}{a}", t("Checksum"))).size(11.5).color(th.weak));
                            });
                        })
                        .body(|body| {
                            body.rows(22.0, self.rows.len(), |mut row| {
                                let r = &self.rows[row.index()];
                                let good = r.verdict().or_else(|| match (&match_hash, &r.state) {
                                    (Some(e), State::Done(h)) if e == h => Some(true),
                                    _ => None,
                                });
                                row.col(|ui| {
                                    let (sym, c) = match (&r.state, good) {
                                        (_, Some(true)) => ("✔", ok_color(th)),
                                        (_, Some(false)) => ("✘", th.danger),
                                        (State::Pending, _) => ("…", th.weak),
                                        (State::Error(_) | State::Missing, _) => ("⚠", th.danger),
                                        _ => ("", th.weak),
                                    };
                                    ui.label(RichText::new(sym).color(c));
                                });
                                row.col(|ui| {
                                    ui.add(egui::Label::new(RichText::new(&r.name).color(th.text)).truncate()).on_hover_text(r.path.display().to_string());
                                });
                                row.col(|ui| {
                                    ui.label(RichText::new(if r.state == State::Missing { String::new() } else { human_size(r.size) }).size(12.0).color(th.weak));
                                });
                                row.col(|ui| match &r.state {
                                    State::Done(h) => {
                                        let color = match good {
                                            Some(true) => ok_color(th),
                                            Some(false) => th.danger,
                                            None => th.text,
                                        };
                                        let tip = match (&r.expected, good) {
                                            (Some(e), Some(false)) => format!("{}: {e}\n{}", t("Expected"), t("Click to copy")),
                                            _ => t("Click to copy"),
                                        };
                                        let resp = ui.add(egui::Label::new(RichText::new(h).font(mono.clone()).color(color)).truncate().sense(egui::Sense::click()));
                                        if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                            copy = Some(h.clone());
                                        }
                                    }
                                    State::Pending => {
                                        ui.label(RichText::new(t("Calculating…")).color(th.weak));
                                    }
                                    State::Missing => {
                                        ui.label(RichText::new(t("File not found")).color(th.danger));
                                    }
                                    State::Error(e) => {
                                        ui.add(egui::Label::new(RichText::new(e).color(th.danger)).truncate());
                                    }
                                });
                            });
                        });
                });
                if let Some(h) = copy {
                    ui.ctx().copy_text(h);
                    self.note = Some((t("Checksum copied"), false));
                }

                // Footer.
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if verify {
                        let ok = self.rows.iter().filter(|r| r.verdict() == Some(true)).count();
                        let bad = self.rows.iter().filter(|r| matches!(r.state, State::Done(_) | State::Error(_)) && r.verdict() == Some(false)).count();
                        let missing = self.rows.iter().filter(|r| r.state == State::Missing).count();
                        let text = format!("{ok} {} · {bad} {} · {missing} {}", t("OK"), t("failed"), t("missing"));
                        let color = if bad + missing > 0 { th.danger } else if self.worker.is_none() { ok_color(th) } else { th.weak };
                        ui.label(RichText::new(text).strong().color(color));
                    } else if let Some((n, err)) = &self.note {
                        ui.label(RichText::new(n).color(if *err { th.danger } else { th.weak }));
                    } else if self.worker.is_none() && !self.rows.is_empty() {
                        ui.label(RichText::new(format!("{} {:.1} s", t("Done in"), self.elapsed)).color(th.weak));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let list = self.done_list();
                        let idle = self.worker.is_none();
                        if !verify {
                            let algo = self.algo.unwrap_or(Algo::Sha256);
                            let b = egui::Button::new(RichText::new(t("Save checksum file…")).strong().color(th.on_accent()))
                                .fill(if idle && !list.is_empty() { th.accent } else { th.surface })
                                .min_size(egui::vec2(0.0, 30.0));
                            if ui.add_enabled(idle && !list.is_empty(), b).clicked() {
                                let dlg = rfd::FileDialog::new().set_directory(&self.base).set_file_name(self.default_list_name(algo));
                                if let Some(p) = dlg.save_file() {
                                    // Names are relative to the folder the list is saved in.
                                    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
                                    let items: Vec<(String, String)> = self
                                        .rows
                                        .iter()
                                        .filter_map(|r| match &r.state {
                                            State::Done(h) => Some((relative_name(&dir, &r.path), h.clone())),
                                            _ => None,
                                        })
                                        .collect();
                                    match std::fs::write(&p, format_list(algo, &items)) {
                                        Ok(()) => {
                                            self.note = Some((format!("{} {}", t("Saved"), p.display()), false));
                                            changed = Some(dir);
                                        }
                                        Err(e) => self.note = Some((e.to_string(), true)),
                                    }
                                }
                            }
                        }
                        if ui.add_enabled(!list.is_empty(), egui::Button::new(t("Copy all")).min_size(egui::vec2(0.0, 30.0))).clicked() {
                            ui.ctx().copy_text(format_list(self.algo.unwrap_or(Algo::Sha256), &list));
                            self.note = Some((t("Checksums copied"), false));
                        }
                        if idle && ui.add(egui::Button::new(t("Recalculate")).min_size(egui::vec2(0.0, 30.0))).clicked() {
                            for r in &mut self.rows {
                                if r.state != State::Missing {
                                    r.state = State::Pending;
                                }
                            }
                            rerun = true;
                        }
                    });
                });
            });
        if rerun {
            self.run(ctx);
        }
        if !open {
            self.stop();
            self.rows.clear();
        }
        self.open = open;
        changed
    }
}

/// `path` relative to `dir` with `/` separators (`../` where needed).
fn relative_name(dir: &Path, path: &Path) -> String {
    let d: Vec<_> = dir.components().collect();
    let p: Vec<_> = path.components().collect();
    let common = d.iter().zip(&p).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return path.to_string_lossy().to_string();
    }
    let mut parts: Vec<String> = vec!["..".into(); d.len() - common];
    parts.extend(p[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digests() {
        let s = b"hello world";
        assert_eq!(hash_bytes(s, Algo::Crc32), "0d4a1185");
        assert_eq!(hash_bytes(s, Algo::Md5), "5eb63bbbe01eeed093cb22bb8f5acdc3");
        assert_eq!(hash_bytes(s, Algo::Sha1), "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed");
        assert_eq!(hash_bytes(s, Algo::Sha256), "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
        assert_eq!(
            hash_bytes(s, Algo::Sha512),
            "309ecc489c12d6eb4cc40f50c902f2b4d0ed77ee511a7c7a9bcd3ca86d4cd86f989dd35bc5ff499670da34255b45b0cfd830e81f605dcf7dc5542e93ae9cd76f"
        );
        assert_eq!(hash_bytes(s, Algo::Blake3), "d74981efa70a0c880b8d8c1985d075dbcbf679b99a5f9914e5aaf96b831a9e24");
        for a in Algo::ALL {
            assert_eq!(hash_bytes(s, a).len(), a.hex_len(), "{a:?}");
        }
    }

    #[test]
    fn parse_formats() {
        let gnu = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9  a.txt\n\
                   B94D27B9934D3E08A52E52D7DA7DABFAC484EFE37A5380EE9088F7ACE2EFCDE9 *dir/b c.bin\n";
        let v = parse_list(gnu, Some(Algo::Sha256));
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "a.txt");
        assert_eq!(v[1].name, "dir/b c.bin");
        assert_eq!(v[1].hash, v[0].hash);
        assert!(v.iter().all(|e| e.algo == Algo::Sha256));

        let bsd = "MD5 (x y.iso) = 5eb63bbbe01eeed093cb22bb8f5acdc3\nSHA1 (z) = 2aae6c35c94fcfb415dbe95f408b9ce91ee846ed\n";
        let v = parse_list(bsd, None);
        assert_eq!(v[0], ListEntry { name: "x y.iso".into(), hash: "5eb63bbbe01eeed093cb22bb8f5acdc3".into(), algo: Algo::Md5 });
        assert_eq!(v[1].algo, Algo::Sha1);

        let sfv = "; comment\nfile one.txt 0D4A1185\n";
        let v = parse_list(sfv, Some(Algo::Crc32));
        assert_eq!(v, vec![ListEntry { name: "file one.txt".into(), hash: "0d4a1185".into(), algo: Algo::Crc32 }]);

        // Length decides when the file name gives no hint.
        let v = parse_list("5eb63bbbe01eeed093cb22bb8f5acdc3  f\n", None);
        assert_eq!(v[0].algo, Algo::Md5);
    }

    #[test]
    fn roundtrip() {
        let items = vec![("a.txt".to_string(), "0d4a1185".to_string()), ("sub/b c".to_string(), "deadbeef".to_string())];
        for algo in [Algo::Crc32, Algo::Sha256] {
            let items: Vec<(String, String)> = items.iter().map(|(n, h)| (n.clone(), if algo == Algo::Crc32 { h.clone() } else { h.repeat(8) })).collect();
            let text = format_list(algo, &items);
            let back: Vec<(String, String)> = parse_list(&text, Some(algo)).into_iter().map(|e| (e.name, e.hash)).collect();
            assert_eq!(back, items);
        }
        let odd = vec![("we\\ird\nname".to_string(), "0".repeat(64))];
        let back = parse_list(&format_list(Algo::Sha256, &odd), Some(Algo::Sha256));
        assert_eq!(back[0].name, odd[0].0);
    }

    #[test]
    fn names_and_guesses() {
        assert_eq!(Algo::from_file_name("ubuntu.iso.sha256"), Some(Algo::Sha256));
        assert_eq!(Algo::from_file_name("SHA256SUMS"), Some(Algo::Sha256));
        assert_eq!(Algo::from_file_name("x.md5sum"), Some(Algo::Md5));
        assert_eq!(Algo::from_file_name("disk.sfv"), Some(Algo::Crc32));
        assert_eq!(Algo::from_file_name("notes.txt"), None);
        assert_eq!(Algo::guess(&"a".repeat(64), Algo::Md5), Some(Algo::Sha256));
        assert_eq!(Algo::guess(&"a".repeat(64), Algo::Blake3), Some(Algo::Blake3));
        assert_eq!(clean_hex(" 0xABcd "), Some("abcd".into()));
        assert_eq!(clean_hex("xyz"), None);
        assert_eq!(relative_name(Path::new("/a/b"), Path::new("/a/b/c/d.txt")), "c/d.txt");
        assert_eq!(relative_name(Path::new("/a/b"), Path::new("/a/x.txt")), "../x.txt");
    }

    #[test]
    fn verify_a_list_on_disk() {
        let dir = std::env::temp_dir().join(format!("mf-cksum-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello world").unwrap();
        std::fs::write(dir.join("sub/b.txt"), b"other").unwrap();
        let rows = collect(&dir, &[dir.join("a.txt"), dir.join("sub")]);
        let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(names, ["a.txt", "sub/b.txt"]);
        let f = std::fs::File::open(dir.join("a.txt")).unwrap();
        assert_eq!(hash_reader(f, Algo::Sha256, |_| true).unwrap(), hash_bytes(b"hello world", Algo::Sha256));
        let f = std::fs::File::open(dir.join("a.txt")).unwrap();
        assert!(hash_reader(f, Algo::Sha256, |_| false).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
