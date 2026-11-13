//! File previews: images (raster + SVG), Markdown, syntax-highlighted code,
//! plain text and a hex dump for binaries, and the views of formats read elsewhere.
use crate::theme::Theme;
use crate::util::{ext_of, human_size, Res};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, TextureHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, LazyLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

const MAX_TEXT: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 20_000;
pub const MAX_IMAGE: usize = 80 * 1024 * 1024;
/// Most memory one decoded image may take. Larger ones are refused before decoding.
const MAX_DECODED_IMAGE: u64 = 384 * 1024 * 1024;
/// Laying out text takes about 200 bytes per character, kept while it is on screen. One
/// block of Markdown is limited to this much text, and one document to `MAX_MARKDOWN`.
pub const MAX_BLOCK_TEXT: usize = 64 * 1024;
const MAX_MARKDOWN: usize = 1024 * 1024;
/// How many previews may load at the same time. The cursor can cross a folder of huge
/// files far faster than they can be read, and each may need hundreds of megabytes.
const MAX_LOADING: usize = 3;
/// Deeply nested files (SVG, PDF, JSON) make parsers recurse.
const LOADER_STACK: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Image,
    Svg,
    Markdown,
    Table,
    Pdf,
    Notebook,
    /// HEIC, HEIF and AVIF, decoded by a tool on the system.
    Heif,
    Video,
    Raw,
    Document,
    Font,
    Model,
    Certificate,
    Code,
    Text,
    Other,
}

pub fn classify(name: &str) -> Kind {
    let ext = ext_of(name);
    let lower = name.to_lowercase();
    if crate::table::is_table(name) {
        return Kind::Table;
    }
    if crate::textdocs::is_document(name) {
        return Kind::Document;
    }
    if crate::fontview::is_font(name) {
        return Kind::Font;
    }
    if crate::model3d::is_model(name) {
        return Kind::Model;
    }
    if crate::certview::is_cert(name) {
        return Kind::Certificate;
    }
    match ext.as_str() {
        "pdf" => return Kind::Pdf,
        "ipynb" => return Kind::Notebook,
        "heic" | "heif" | "hif" | "avif" => return Kind::Heif,
        "mp4" | "m4v" | "mov" | "mkv" | "webm" => return Kind::Video,
        "cr2" | "nef" | "arw" | "dng" => return Kind::Raw,
        _ => {}
    }
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" => Kind::Image,
        "svg" => Kind::Svg,
        "md" | "markdown" | "mdown" | "mkd" => Kind::Markdown,
        "rs" | "py" | "pyw" | "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "java" | "kt" | "kts" | "c" | "h" | "cpp"
        | "cc" | "cxx" | "hpp" | "hh" | "cs" | "go" | "rb" | "php" | "swift" | "scala" | "sh" | "bash" | "zsh"
        | "fish" | "ps1" | "bat" | "cmd" | "lua" | "pl" | "pm" | "r" | "sql" | "html" | "htm" | "xml" | "css"
        | "scss" | "sass" | "less" | "json" | "jsonc" | "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" | "hs"
        | "ml" | "mli" | "clj" | "erl" | "ex" | "exs" | "dart" | "vue" | "svelte" | "zig" | "nim" | "d" | "groovy"
        | "gradle" | "tex" | "m" | "mm" | "proto" | "dockerfile" | "makefile" | "cmake" | "diff" | "patch" | "vim"
        | "el" | "lisp" | "tf" | "nix" | "graphql" => Kind::Code,
        "txt" | "log" | "text" | "nfo" | "rst" | "adoc" | "properties" | "env" | "gitignore" | "lock" => Kind::Text,
        _ => match lower.as_str() {
            "makefile" | "dockerfile" | "cmakelists.txt" | "justfile" | "rakefile" | "gemfile" | "vagrantfile" => Kind::Code,
            "license" | "readme" | "authors" | "changelog" | "copying" => Kind::Text,
            _ => Kind::Other,
        },
    }
}

/// How many bytes are worth reading for a preview of this file.
pub fn read_limit(name: &str) -> usize {
    match classify(name) {
        Kind::Code | Kind::Text | Kind::Markdown | Kind::Other => MAX_TEXT,
        _ => MAX_IMAGE,
    }
}

/// Past this size a file is not previewed at all, whatever kind it is.
const LARGE_FILE: u64 = 1024 * 1024 * 1024;
/// Spreadsheets that cannot be read as a stream are loaded whole.
const LARGE_WORKBOOK: u64 = 256 * 1024 * 1024;

/// The size from which a file of this name gets no preview: `MAX_IMAGE` for what has to
/// be in memory as a whole, as more of it would not be read anyway, and else `LARGE_FILE`.
pub fn size_limit(name: &str) -> u64 {
    match classify(name) {
        Kind::Image | Kind::Svg | Kind::Pdf | Kind::Notebook | Kind::Heif | Kind::Font | Kind::Certificate => MAX_IMAGE as u64,
        Kind::Document if !crate::textdocs::reads_zip(name) => MAX_IMAGE as u64,
        Kind::Model => crate::model3d::MAX_FILE,
        Kind::Table if matches!(ext_of(name).as_str(), "xls" | "xlsb" | "ods") => LARGE_WORKBOOK,
        // Model weights of many gigabytes: only their header is read.
        Kind::Table if crate::table::reads_header_only(name) => u64::MAX,
        _ => LARGE_FILE,
    }
}

/// Whether a file of this size gets the "too large" message instead of a preview.
/// `readable` is how much can be fetched from where the file is, if not a local disk:
/// text can be shown from its start, while other formats need all of the file.
pub fn too_large(name: &str, size: u64, readable: Option<u64>) -> bool {
    let start_is_enough = matches!(classify(name), Kind::Code | Kind::Text | Kind::Markdown | Kind::Other);
    size > size_limit(name) || (!start_is_enough && readable.is_some_and(|r| size > r))
}

pub type Styled = Vec<(Color32, String, bool)>; // colour, text, italic

#[derive(Clone)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: bool,
}

pub enum MdBlock {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    Quote(Vec<Span>),
    Item { depth: usize, marker: String, spans: Vec<Span> },
    Code { lines: Vec<Styled>, bg: Color32 },
    Table { rows: Vec<Vec<Vec<Span>>>, has_header: bool },
    Rule,
}

enum Loaded {
    Image { rgba: Vec<u8>, w: usize, h: usize, orig: (u32, u32), premultiplied: bool, fmt: String, more_frames: Vec<AnimFrame>, first_delay: f32 },
    Code { lines: Vec<Styled>, bg: Color32, truncated: bool, lang: String },
    Markdown { blocks: Vec<MdBlock>, source: Vec<Styled>, bg: Color32 },
    Hex { bytes: Vec<u8>, total: usize },
    Table { data: crate::table::TableData, source: Option<(Vec<Styled>, Color32)> },
    Pdf(Box<crate::docview::PdfView>),
    Notebook(Box<crate::docview::NotebookView>),
    Model(Box<crate::model3d::ModelView>),
    Document(crate::textdocs::Document),
    Summary(Summary),
    /// Past its size limit: nothing was read.
    TooLarge(u64),
    /// Needs a password.
    Locked,
    Info(String),
    Error(String),
}

/// A later frame of an animation, and how long it stays (seconds).
struct AnimFrame {
    rgba: Vec<u8>,
    w: usize,
    h: usize,
    delay: f32,
}

/// A file described in rows, with an optional picture above them.
struct Summary {
    format: String,
    picture: Option<SummaryPicture>,
    rows: Vec<crate::certview::Row>,
    /// A picture is still on its way (its facts are shown meanwhile).
    picture_pending: bool,
}

struct SummaryPicture {
    rgba: Vec<u8>,
    w: usize,
    h: usize,
    /// White on transparent, to be drawn in the theme's text colour (font samples).
    tinted: bool,
}

pub struct Preview {
    pub key: String,
    pub name: String,
    rx: Option<Receiver<Loaded>>,
    loaded: Option<Loaded>,
    texture: Option<TextureHandle>,
    /// An animation's frames after the first, and how long each one stays.
    frames: Vec<(TextureHandle, f32)>,
    first_delay: f32,
    paused: bool,
    zoom: Option<f32>,
    wrap: bool,
    show_source: bool,
    sheet: usize,
    row_filter: String,
    /// Tells background work (and the tools it runs) that nobody is waiting any more.
    cancel: crate::media::Cancel,
    /// The archive this file is in, whose password this app can ask for.
    archive: Option<PathBuf>,
    /// `Enter password` was pressed.
    password_wanted: bool,
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub type Loader = Box<dyn FnOnce() -> Res<Vec<u8>> + Send>;

/// The error of a document protected by a password or DRM, which only its own app can open.
pub const LOCKED: &str = "\u{1}locked";

/// The small message for a protected file. `Enter password` is offered if this app can
/// open it, which it can for archives; returns whether it was pressed.
pub fn locked_ui(ui: &mut egui::Ui, theme: &Theme, t: &dyn Fn(&str) -> String, can_enter_password: bool) -> bool {
    let mut pressed = false;
    let room = ui.available_height();
    ui.vertical_centered(|ui| {
        ui.add_space((room * 0.5 - 40.0).max(0.0));
        ui.label(egui::RichText::new(format!("🔒 {}", t("Password protected"))).size(13.5).color(theme.text));
        if can_enter_password {
            ui.add_space(4.0);
            pressed = ui.button(t("Enter password…")).clicked();
        } else {
            ui.label(egui::RichText::new(t("Press Enter to open it in its own app.")).size(12.0).color(theme.weak));
        }
    });
    pressed
}

/// A turn to load a preview; the next one in line starts when it is dropped.
struct LoadingTurn;

static LOADING: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
static TURN_FREE: std::sync::Condvar = std::sync::Condvar::new();

impl LoadingTurn {
    /// Wait for a turn. `None` if the preview is gone by then, which is what happens
    /// to everything the cursor passed over: it never gets to read its file.
    fn wait(cancel: &crate::media::Cancel) -> Option<Self> {
        let mut running = LOADING.lock().ok()?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if *running < MAX_LOADING {
                *running += 1;
                return Some(LoadingTurn);
            }
            running = TURN_FREE.wait_timeout(running, std::time::Duration::from_millis(50)).ok()?.0;
        }
    }
}

impl Drop for LoadingTurn {
    fn drop(&mut self) {
        if let Ok(mut running) = LOADING.lock() {
            *running -= 1;
        }
        TURN_FREE.notify_one();
    }
}

/// Load in the background: on a roomy stack, and turning a panic into a message, so a
/// file that breaks a parser shows an error instead of loading forever.
fn in_background(cancel: Option<crate::media::Cancel>, job: impl FnOnce(&std::sync::mpsc::Sender<Loaded>) -> Loaded + Send + 'static) -> Receiver<Loaded> {
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new().name("preview".into()).stack_size(LOADER_STACK).spawn(move || {
        let _turn = match &cancel {
            Some(c) => match LoadingTurn::wait(c) {
                Some(turn) => Some(turn),
                None => return,
            },
            None => None,
        };
        let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&tx)));
        let loaded = loaded.unwrap_or_else(|_| Loaded::Error(crate::i18n::t("This file could not be previewed.")));
        let _ = tx.send(match loaded {
            Loaded::Error(e) if e == LOCKED || crate::archive::is_password_error(&e) => Loaded::Locked,
            other => other,
        });
    });
    rx
}

/// Upload an image as a texture. The graphics card has a maximum size, and going over
/// it is a fatal error, so anything larger is scaled down first.
pub fn upload_texture(ctx: &egui::Context, name: impl Into<String>, image: egui::ColorImage) -> TextureHandle {
    let max = ctx.input(|i| i.max_texture_side).max(1);
    let [w, h] = image.size;
    let image = if w.max(h) > max {
        let scale = max as f64 / w.max(h) as f64;
        let (nw, nh) = (((w as f64 * scale) as usize).max(1), ((h as f64 * scale) as usize).max(1));
        let mut pixels = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            for x in 0..nw {
                pixels.push(image.pixels[(y * h / nh) * w + x * w / nw]);
            }
        }
        egui::ColorImage { size: [nw, nh], pixels }
    } else {
        image
    };
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}

impl Preview {
    fn blank(key: String, name: String, wrap: bool) -> Self {
        Self {
            key,
            name,
            rx: None,
            loaded: None,
            texture: None,
            frames: vec![],
            first_delay: 0.0,
            paused: false,
            zoom: None,
            wrap,
            show_source: false,
            sheet: 0,
            row_filter: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            archive: None,
            password_wanted: false,
        }
    }

    /// This file is in an archive that a password can open.
    pub fn in_archive(mut self, archive: PathBuf) -> Self {
        self.archive = Some(archive);
        self
    }

    /// The archive whose password was asked for, once.
    pub fn take_password_request(&mut self) -> Option<PathBuf> {
        if std::mem::take(&mut self.password_wanted) { self.archive.clone() } else { None }
    }

    /// `limit` is how much of the file `loader` reads at most.
    pub fn new(key: String, name: String, loader: Loader, limit: usize, syntax_theme: &str, max_texture: usize) -> Self {
        let n = name.clone();
        let theme = syntax_theme.to_string();
        let mut p = Self::blank(key, name, false);
        let cancel = p.cancel.clone();
        p.rx = Some(in_background(Some(p.cancel.clone()), move |tx| match loader() {
            Ok(bytes) => {
                let cut_short = bytes.len() >= limit;
                match decode(&n, bytes, &theme, max_texture, &cancel, &|early| drop(tx.send(early))) {
                    // What it failed on is the missing rest of the file.
                    Loaded::Error(_) if cut_short => Loaded::Error(format!("{} ({} {})", crate::i18n::t("This file is too large to preview here."), crate::i18n::t("limit:"), human_size(limit as u64))),
                    loaded => loaded,
                }
            }
            Err(e) => Loaded::Error(e),
        }));
        p
    }

    /// Whether a local file of this name is better read in place than loaded into memory
    /// first: it may be huge, tools need its path, or files next to it matter.
    pub fn reads_files_directly(name: &str) -> bool {
        matches!(classify(name), Kind::Heif | Kind::Video | Kind::Raw | Kind::Model) || crate::textdocs::reads_zip(name)
    }

    /// A preview read directly from a local file (see `reads_files_directly`).
    pub fn local_file(key: String, name: String, path: PathBuf, syntax_theme: &str, max_texture: usize) -> Self {
        let n = name.clone();
        let mut p = Self::blank(key, name, false);
        let cancel = p.cancel.clone();
        let theme = syntax_theme.to_string();
        p.rx = Some(in_background(Some(p.cancel.clone()), move |tx| decode_file(&n, &path, &theme, max_texture, &cancel, &|early| drop(tx.send(early)))));
        p
    }

    /// Tabular preview read directly from a local file (avoids loading huge files).
    pub fn table_local(key: String, name: String, path: std::path::PathBuf, syntax_theme: &str) -> Self {
        let theme = syntax_theme.to_string();
        let n = name.clone();
        let mut p = Self::blank(key, name, true);
        p.rx = Some(in_background(Some(p.cancel.clone()), move |_| match crate::table::load_path(&path) {
            Ok(data) => {
                let source = crate::table::is_csv(&n).then(|| {
                    use std::io::Read;
                    let mut head = Vec::new();
                    if let Ok(f) = std::fs::File::open(&path) {
                        let _ = f.take(512 * 1024).read_to_end(&mut head);
                    }
                    let (lines, bg, _, _) = highlight(&String::from_utf8_lossy(&head), "csv", &theme);
                    (lines, bg)
                });
                Loaded::Table { data, source }
            }
            Err(e) => Loaded::Error(e),
        }));
        p
    }

    /// A text summary produced in the background (folders, archives).
    pub fn text_async(key: String, name: String, f: impl FnOnce() -> String + Send + 'static) -> Self {
        let mut p = Self::blank(key, name, true);
        // Summaries take little memory, and may wait long on a slow disk: no turn needed.
        p.rx = Some(in_background(None, move |_| Loaded::Info(f())));
        p
    }

    /// The message for a file past its size limit. The file is not touched.
    pub fn too_large(key: String, name: String, size: u64) -> Self {
        let mut p = Self::blank(key, name, true);
        p.loaded = Some(Loaded::TooLarge(size));
        p
    }

    pub fn info(key: String, name: String, text: String) -> Self {
        let mut p = Self::blank(key, name, true);
        p.loaded = Some(Loaded::Info(text));
        p
    }

    /// Background loading has finished.
    #[cfg(test)]
    pub fn is_ready(&self) -> bool {
        let pages_done = !matches!(&self.loaded, Some(Loaded::Pdf(p)) if !p.is_finished());
        self.loaded.is_some() && self.rx.is_none() && pages_done
    }

    /// What was loaded, in a few words (for tests).
    #[cfg(test)]
    pub fn describe(&self) -> String {
        match &self.loaded {
            None => "loading".into(),
            Some(Loaded::Image { orig, fmt, .. }) => format!("image {fmt} {}x{} frames={}", orig.0, orig.1, self.frames.len() + 1),
            Some(Loaded::Code { lang, .. }) => format!("code {lang}"),
            Some(Loaded::Markdown { .. }) => "markdown".into(),
            Some(Loaded::Hex { .. }) => "HEX".into(),
            Some(Loaded::TooLarge(size)) => format!("too large {size}"),
            Some(Loaded::Locked) => "locked".into(),
            Some(Loaded::Table { data, .. }) => format!(
                "table {} {:?} {}",
                data.format,
                data.info,
                data.sheets.iter().map(|s| format!("{}[{}x{}]{:?}", s.name, s.rows.len(), s.columns.len(), s.total_rows)).collect::<Vec<_>>().join(" ")
            ),
            Some(Loaded::Pdf(p)) => format!("pdf {}", p.describe()),
            Some(Loaded::Notebook(_)) => "notebook".into(),
            Some(Loaded::Model(m)) => format!("model {}", m.describe()),
            Some(Loaded::Document(d)) => format!("doc {} {:?} {:?} sections={} first={:?}", d.format, d.title, d.author, d.sections.len(), d.sections.first().map(|s| s.blocks.len())),
            Some(Loaded::Summary(s)) => format!("summary {} pic={} {:?}", s.format, s.picture.as_ref().map_or("no".to_string(), |p| format!("{}x{}", p.w, p.h)), s.rows),
            Some(Loaded::Info(s)) => format!("info {s}"),
            Some(Loaded::Error(e)) => format!("ERROR {e}"),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        use std::sync::mpsc::TryRecvError;
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(mut l) => {
                if let Loaded::Image { rgba, w, h, premultiplied, more_frames, first_delay, .. } = &mut l {
                    let img = if *premultiplied {
                        egui::ColorImage::from_rgba_premultiplied([*w, *h], rgba)
                    } else {
                        egui::ColorImage::from_rgba_unmultiplied([*w, *h], rgba)
                    };
                    self.texture = Some(upload_texture(ctx, self.key.clone(), img));
                    // The texture holds the pixels now.
                    *rgba = Vec::new();
                    self.first_delay = *first_delay;
                    for (i, f) in std::mem::take(more_frames).into_iter().enumerate() {
                        let img = egui::ColorImage::from_rgba_unmultiplied([f.w, f.h], &f.rgba);
                        self.frames.push((upload_texture(ctx, format!("{}#{}", self.key, i + 1), img), f.delay));
                    }
                }
                if let Loaded::Summary(Summary { picture: Some(p), .. }) = &mut l {
                    let img = egui::ColorImage::from_rgba_premultiplied([p.w, p.h], &p.rgba);
                    self.texture = Some(upload_texture(ctx, self.key.clone(), img));
                    p.rgba = Vec::new();
                }
                self.loaded = Some(l);
                // More may follow (a video's thumbnail after its facts).
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                // Whatever happened to the loader, the preview must not load forever.
                if self.loaded.is_none() {
                    self.loaded = Some(Loaded::Error(crate::i18n::t("This file could not be previewed.")));
                }
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, theme: &Theme, t: impl Fn(&str) -> String) {
        self.poll(ui.ctx());
        // Documents with their own interactive views.
        match &mut self.loaded {
            Some(Loaded::Pdf(v)) => return v.ui(ui, &self.name, &self.key, theme, &t),
            Some(Loaded::Notebook(v)) => return v.ui(ui, &self.name, &self.key, theme, &t),
            Some(Loaded::Model(v)) => return v.ui(ui, &self.name, &self.key, theme),
            _ => {}
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&self.name).strong().color(theme.accent));
            match &self.loaded {
                Some(Loaded::Image { orig, fmt, .. }) => {
                    let frames = if self.frames.is_empty() { String::new() } else { format!(" · {} {}", self.frames.len() + 1, t("frames")) };
                    ui.label(egui::RichText::new(format!("{}×{} {}{frames}", orig.0, orig.1, fmt)).color(theme.weak));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("+").clicked() {
                            self.zoom = Some(self.zoom.unwrap_or(1.0) * 1.25);
                        }
                        if ui.button("−").clicked() {
                            self.zoom = Some(self.zoom.unwrap_or(1.0) / 1.25);
                        }
                        if ui.selectable_label(self.zoom == Some(1.0), "1:1").clicked() {
                            self.zoom = Some(1.0);
                        }
                        if ui.selectable_label(self.zoom.is_none(), t("Fit")).clicked() {
                            self.zoom = None;
                        }
                        if !self.frames.is_empty() && ui.button(if self.paused { t("Play") } else { t("Pause") }).clicked() {
                            self.paused = !self.paused;
                        }
                    });
                }
                Some(Loaded::Summary(s)) => {
                    ui.label(egui::RichText::new(&s.format).color(theme.weak));
                }
                Some(Loaded::Document(d)) => {
                    let mut parts = vec![d.format.to_string()];
                    parts.extend(d.title.clone());
                    parts.extend(d.author.clone());
                    ui.add(egui::Label::new(egui::RichText::new(parts.join(" · ")).color(theme.weak)).truncate());
                }
                Some(Loaded::Code { truncated, lang, lines, .. }) => {
                    ui.label(egui::RichText::new(format!("{lang} · {} {}", lines.len(), t("lines"))).color(theme.weak));
                    if *truncated {
                        ui.label(egui::RichText::new(t("(truncated)")).color(theme.marked));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.checkbox(&mut self.wrap, t("Wrap"));
                    });
                }
                Some(Loaded::Markdown { .. }) => {
                    ui.label(egui::RichText::new("Markdown").color(theme.weak));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.selectable_label(self.show_source, t("Source")).clicked() {
                            self.show_source = !self.show_source;
                        }
                    });
                }
                Some(Loaded::Table { data, source }) => {
                    let mut parts = vec![data.format.to_string()];
                    if let Some(sh) = data.sheets.get(self.sheet) {
                        parts.push(format!("{} {}", sh.columns.len(), t("columns")));
                        parts.push(match sh.total_rows {
                            Some(n) => format!("{} {}", crate::table::fmt_count(n), t("rows")),
                            None => format!("{}+ {}", crate::table::fmt_count(sh.rows.len() as u64), t("rows")),
                        });
                    }
                    parts.extend(data.info.iter().cloned());
                    let has_source = source.is_some();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if has_source && ui.selectable_label(self.show_source, t("Source")).clicked() {
                            self.show_source = !self.show_source;
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(egui::RichText::new(parts.join(" · ")).color(theme.weak)).truncate());
                        });
                    });
                }
                Some(Loaded::Hex { total, .. }) => {
                    ui.label(egui::RichText::new(format!("{} · {}", t("Binary"), human_size(*total as u64))).color(theme.weak));
                }
                _ => {}
            }
        });
        ui.separator();
        let Some(loaded) = &self.loaded else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t("Loading…"));
            });
            return;
        };
        match loaded {
            // Rendered by their own views above.
            Loaded::Pdf(_) | Loaded::Notebook(_) | Loaded::Model(_) => {}
            Loaded::Locked => {
                self.password_wanted |= locked_ui(ui, theme, &t, self.archive.is_some());
            }
            Loaded::TooLarge(size) => {
                ui.centered_and_justified(|ui| {
                    ui.label(egui::RichText::new(format!("{} · {}", t("Too large to preview"), human_size(*size))).size(12.5).color(theme.weak));
                });
            }
            Loaded::Summary(s) => summary_ui(ui, s, self.texture.as_ref(), theme),
            Loaded::Document(d) => {
                self.sheet = self.sheet.min(d.sections.len().saturating_sub(1));
                if d.sections.len() > 1 {
                    let current = d.sections[self.sheet].title.clone();
                    egui::ComboBox::from_id_salt(("doc-section", &self.key)).width(ui.available_width().min(420.0)).selected_text(current).show_ui(ui, |ui| {
                        for (i, s) in d.sections.iter().enumerate() {
                            ui.selectable_value(&mut self.sheet, i, &s.title);
                        }
                    });
                    ui.separator();
                }
                if let Some(section) = d.sections.get(self.sheet) {
                    egui::ScrollArea::vertical().id_salt(("doc", &self.key, self.sheet)).auto_shrink([false, false]).show(ui, |ui| {
                        markdown_ui(ui, &section.blocks, theme);
                    });
                }
            }
            Loaded::Info(s) => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.label(s);
                });
            }
            Loaded::Error(e) => {
                ui.colored_label(theme.danger, e);
            }
            Loaded::Image { orig, .. } => {
                // The frame this moment falls on; the first frame if it is not moving.
                let mut shown = self.texture.as_ref();
                if !self.frames.is_empty() {
                    let total = self.first_delay + self.frames.iter().map(|(_, d)| d).sum::<f32>();
                    if !self.paused && total > 0.0 {
                        let now = (ui.input(|i| i.time) % f64::from(total)) as f32;
                        let mut end = self.first_delay;
                        for (tex, delay) in &self.frames {
                            if now < end {
                                break;
                            }
                            shown = Some(tex);
                            end += delay;
                        }
                        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32((end - now).max(0.01)));
                    }
                }
                if let Some(tex) = shown {
                    let avail = ui.available_size();
                    let tex_size = tex.size_vec2();
                    // Display size relative to the original pixel dimensions.
                    let orig_v = egui::vec2(orig.0 as f32, orig.1 as f32);
                    let size = match self.zoom {
                        None => {
                            let s = (avail.x / orig_v.x).min(avail.y / orig_v.y).min(1.0).max(0.01);
                            orig_v * s
                        }
                        Some(z) => orig_v * z,
                    };
                    let _ = tex_size;
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        ui.centered_and_justified(|ui| {
                            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                            checkerboard(ui.painter(), rect, theme);
                            ui.painter().image(
                                tex.id(),
                                rect,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        });
                    });
                }
            }
            Loaded::Code { lines, bg, .. } => {
                code_view(ui, lines, *bg, self.wrap, theme);
            }
            Loaded::Markdown { blocks, source, bg } => {
                if self.show_source {
                    code_view(ui, source, *bg, true, theme);
                } else {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        markdown_ui(ui, blocks, theme);
                    });
                }
            }
            Loaded::Table { data, source } => {
                if let (true, Some((lines, bg))) = (self.show_source, source) {
                    code_view(ui, lines, *bg, false, theme);
                } else {
                    let key = self.key.clone();
                    table_ui(ui, data, &mut self.sheet, &mut self.row_filter, theme, &t, &key);
                }
            }
            Loaded::Hex { bytes, .. } => {
                let mut s = String::new();
                for (i, chunk) in bytes.chunks(16).enumerate() {
                    s.push_str(&format!("{:08x}  ", i * 16));
                    for j in 0..16 {
                        match chunk.get(j) {
                            Some(b) => s.push_str(&format!("{b:02x} ")),
                            None => s.push_str("   "),
                        }
                        if j == 7 {
                            s.push(' ');
                        }
                    }
                    s.push_str(" |");
                    for b in chunk {
                        s.push(if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' });
                    }
                    s.push_str("|\n");
                }
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.label(egui::RichText::new(s).monospace().size(12.0));
                });
            }
        }
    }
}

fn summary_ui(ui: &mut egui::Ui, s: &Summary, texture: Option<&TextureHandle>, theme: &Theme) {
    use crate::certview::Row;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if let (Some(p), Some(tex)) = (&s.picture, texture) {
            // Whole width for a sample sheet; a photo or video frame stays within the view.
            let width = ui.available_width().max(1.0);
            let scale = if p.tinted { (width / p.w as f32).min(1.0) } else { (width / p.w as f32).min(ui.available_height() * 0.62 / p.h as f32).min(1.0) };
            let size = egui::vec2(p.w as f32, p.h as f32) * scale;
            let tint = if p.tinted { theme.text } else { Color32::WHITE };
            ui.vertical_centered(|ui| {
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
            });
            ui.add_space(10.0);
        }
        if s.picture_pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new(crate::i18n::t("Getting a thumbnail…")).color(theme.weak));
            });
            ui.add_space(6.0);
        }
        let label_w = 150.0;
        let value_w = (ui.available_width() - label_w - 12.0).max(120.0);
        for row in &s.rows {
            match row {
                Row::Head(text) => {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(text).font(FontId::new(14.5, egui::FontFamily::Name("bold".into()))).color(theme.strong()));
                }
                Row::Note(text) => {
                    ui.add(egui::Label::new(egui::RichText::new(text).color(theme.marked)).wrap());
                }
                Row::Field(l, v) | Row::Good(l, v) | Row::Bad(l, v) => {
                    let color = match row {
                        Row::Good(..) => Color32::from_rgb(46, 160, 67),
                        Row::Bad(..) => theme.danger,
                        _ => theme.text,
                    };
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(egui::vec2(label_w, 18.0), egui::Layout::left_to_right(egui::Align::Min), |ui| {
                            ui.set_min_width(label_w);
                            ui.add(egui::Label::new(egui::RichText::new(l).color(theme.weak)).truncate());
                        });
                        ui.vertical(|ui| {
                            ui.set_width(value_w);
                            ui.add(egui::Label::new(egui::RichText::new(v).color(color)).wrap().selectable(true));
                        });
                    });
                }
            }
        }
    });
}

fn table_ui(
    ui: &mut egui::Ui,
    data: &crate::table::TableData,
    sheet_idx: &mut usize,
    filter: &mut String,
    theme: &Theme,
    t: &dyn Fn(&str) -> String,
    key: &str,
) {
    use egui_extras::{Column, TableBuilder};
    *sheet_idx = (*sheet_idx).min(data.sheets.len().saturating_sub(1));
    ui.horizontal(|ui| {
        if data.sheets.len() > 1 {
            egui::ScrollArea::horizontal().id_salt(("sheets", key)).max_width(ui.available_width() - 200.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, sh) in data.sheets.iter().enumerate() {
                        if ui.selectable_label(*sheet_idx == i, &sh.name).clicked() {
                            *sheet_idx = i;
                        }
                    }
                });
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::TextEdit::singleline(filter)
                    .hint_text(egui::RichText::new(t("Filter rows…")).color(theme.weak))
                    .desired_width(180.0),
            );
        });
    });
    let Some(sheet) = data.sheets.get(*sheet_idx) else { return };
    if sheet.columns.is_empty() {
        ui.label(egui::RichText::new(t("No data")).color(theme.weak));
        return;
    }
    let needle = filter.trim().to_lowercase();
    let visible: Vec<usize> = if needle.is_empty() {
        (0..sheet.rows.len()).collect()
    } else {
        (0..sheet.rows.len())
            .filter(|&i| sheet.rows[i].iter().any(|c| c.as_deref().is_some_and(|v| v.to_lowercase().contains(&needle))))
            .collect()
    };
    let shown_info = if sheet.total_rows.is_some_and(|n| n as usize > sheet.rows.len()) || sheet.total_rows.is_none() && sheet.rows.len() >= crate::table::MAX_ROWS {
        format!("{} {} {}", t("Showing the first"), crate::table::fmt_count(sheet.rows.len() as u64), t("rows"))
    } else {
        String::new()
    };
    if !needle.is_empty() || !shown_info.is_empty() {
        let mut s = shown_info;
        if !needle.is_empty() {
            if !s.is_empty() {
                s.push_str(" · ");
            }
            s.push_str(&format!("{} {}", crate::table::fmt_count(visible.len() as u64), t("matching rows")));
        }
        ui.label(egui::RichText::new(s).size(11.5).color(theme.weak));
    }
    // Initial column widths from header and sample content.
    let font = FontId::proportional(12.5);
    let widths: Vec<f32> = (0..sheet.columns.len())
        .map(|c| {
            let longest = std::iter::once(sheet.columns[c].chars().count() + 2)
                .chain(sheet.rows.iter().take(40).filter_map(|r| r.get(c)?.as_ref().map(|v| v.chars().count())))
                .max()
                .unwrap_or(8);
            (longest as f32 * 7.2 + 18.0).clamp(56.0, 320.0)
        })
        .collect();
    let row_no_w = (sheet.rows.len().max(1).to_string().len() as f32 * 8.0 + 16.0).max(36.0);
    ui.push_id(("table", key, *sheet_idx), |ui| {
        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
            let mut tb = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .vscroll(true)
                .auto_shrink([false, false])
                .min_scrolled_height(0.0)
                .max_scroll_height(f32::INFINITY)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(row_no_w));
            for w in &widths {
                tb = tb.column(Column::initial(*w).at_least(40.0).clip(true));
            }
            tb.header(38.0, |mut header| {
                header.col(|ui| {
                    ui.label(egui::RichText::new("#").size(11.0).color(theme.weak));
                });
                for (c, name) in sheet.columns.iter().enumerate() {
                    header.col(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(egui::Label::new(egui::RichText::new(name).font(FontId::new(12.5, egui::FontFamily::Name("bold".into()))).color(theme.strong())).truncate());
                            let ty = sheet.types.get(c).map(String::as_str).unwrap_or("");
                            ui.add(egui::Label::new(egui::RichText::new(ty).size(10.5).color(theme.accent.gamma_multiply(0.85))).truncate());
                        });
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, visible.len(), |mut row| {
                    let r = visible[row.index()];
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new((r + 1).to_string()).size(11.0).color(theme.weak));
                        });
                    });
                    for c in 0..sheet.columns.len() {
                        row.col(|ui| {
                            let cell = sheet.rows[r].get(c).and_then(|v| v.as_deref());
                            let numeric = sheet.numeric.get(c).copied().unwrap_or(false);
                            let label = match cell {
                                Some(v) => egui::Label::new(egui::RichText::new(v).font(font.clone()).color(theme.text)),
                                None => egui::Label::new(egui::RichText::new("null").font(font.clone()).italics().color(theme.weak.gamma_multiply(0.7))),
                            }
                            .truncate();
                            if numeric {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.add(label);
                                });
                            } else {
                                ui.add(label);
                            }
                        });
                    }
                });
            });
        });
    });
}

fn checkerboard(p: &egui::Painter, rect: egui::Rect, theme: &Theme) {
    let a = if theme.dark { Color32::from_gray(60) } else { Color32::from_gray(220) };
    let b = if theme.dark { Color32::from_gray(80) } else { Color32::from_gray(245) };
    p.rect_filled(rect, 0.0, a);
    let s = 10.0;
    // Only the squares in view: an image at 1:1 can be many thousands of squares wide.
    let visible = rect.intersect(p.clip_rect());
    if !visible.is_positive() {
        return;
    }
    let clip = p.with_clip_rect(visible);
    let first_row = ((visible.top() - rect.top()) / s).floor() as i64;
    let first_col = ((visible.left() - rect.left()) / s).floor() as i64;
    let mut row = first_row;
    while rect.top() + row as f32 * s < visible.bottom() {
        // Light squares are on the even diagonals.
        let mut col = first_col - (first_col + row).rem_euclid(2);
        while rect.left() + col as f32 * s < visible.right() {
            let at = egui::pos2(rect.left() + col as f32 * s, rect.top() + row as f32 * s);
            clip.rect_filled(egui::Rect::from_min_size(at, egui::vec2(s, s)), 0.0, b);
            col += 2;
        }
        row += 1;
    }
}

fn code_view(ui: &mut egui::Ui, lines: &[Styled], bg: Color32, wrap: bool, theme: &Theme) {
    let font = FontId::monospace(13.0);
    let gutter = lines.len().to_string().len();
    let mut job = LayoutJob::default();
    for (i, line) in lines.iter().enumerate() {
        job.append(
            &format!("{:>w$}  ", i + 1, w = gutter),
            0.0,
            TextFormat { font_id: font.clone(), color: theme.weak, ..Default::default() },
        );
        for (c, s, italic) in line {
            job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *c, italics: *italic, ..Default::default() });
        }
        if !line.last().is_some_and(|l| l.1.ends_with('\n')) {
            job.append("\n", 0.0, TextFormat { font_id: font.clone(), ..Default::default() });
        }
    }
    job.wrap.max_width = if wrap { ui.available_width() - 16.0 } else { f32::INFINITY };
    egui::Frame::NONE.fill(bg).inner_margin(6.0).show(ui, |ui| {
        let sa = if wrap { egui::ScrollArea::vertical() } else { egui::ScrollArea::both() };
        sa.auto_shrink([false, false]).show(ui, |ui| {
            let label = egui::Label::new(job);
            ui.add(if wrap { label.wrap() } else { label.extend() });
        });
    });
}

/// Highlight `code` (language name or extension) into styled lines + background.
pub fn highlight_lines(code: &str, hint: &str, theme_name: &str) -> (Vec<Styled>, Color32, String, bool) {
    highlight(code, hint, theme_name)
}

/// A code block without line numbers (Markdown / notebook cells).
pub fn code_view_styled(ui: &mut egui::Ui, lines: &[Styled], bg: Color32, theme: &Theme, width: f32) {
    let _ = theme;
    let mut job = LayoutJob::default();
    let font = FontId::monospace(12.5);
    for (i, line) in lines.iter().enumerate() {
        for (c, s, it) in line {
            job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *c, italics: *it, ..Default::default() });
        }
        if i + 1 < lines.len() && !line.last().is_some_and(|l| l.1.ends_with('\n')) {
            job.append("\n", 0.0, TextFormat { font_id: font.clone(), ..Default::default() });
        }
    }
    egui::Frame::NONE.fill(bg).corner_radius(6).inner_margin(8.0).show(ui, |ui| {
        ui.set_min_width(width - 16.0);
        ui.set_max_width(width - 16.0);
        egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
            ui.add(egui::Label::new(job).extend());
        });
    });
}

fn find_syntax(hint: &str) -> &'static SyntaxReference {
    let ss = &*SYNTAXES;
    let h = hint.to_lowercase();
    let alias = match h.as_str() {
        "ts" | "tsx" | "typescript" | "mjs" | "cjs" | "jsx" | "vue" | "svelte" => "js",
        "kt" | "kts" | "kotlin" | "groovy" | "gradle" | "dart" => "java",
        "toml" | "ini" | "cfg" | "conf" | "properties" | "env" => "ini",
        "zsh" | "fish" | "bash" | "shell" => "sh",
        "jsonc" => "json",
        "yml" => "yaml",
        "htm" => "html",
        "scss" | "sass" | "less" => "css",
        "hpp" | "hh" | "cc" | "cxx" | "c++" => "cpp",
        "ps1" | "bat" | "cmd" => "bat",
        "dockerfile" | "justfile" => "sh",
        "rust" => "rs",
        "python" => "py",
        "javascript" => "js",
        "golang" => "go",
        "ruby" => "rb",
        "csharp" | "c#" => "cs",
        "swift" | "zig" | "nim" => "rs",
        "patch" => "diff",
        "tsv" => "csv",
        other => other,
    };
    ss.find_syntax_by_extension(alias)
        .or_else(|| ss.find_syntax_by_token(alias))
        .or_else(|| if alias == "ini" { ss.find_syntax_by_extension("yaml") } else { None })
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

fn to_color(c: syntect::highlighting::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

fn highlight(code: &str, hint: &str, theme_name: &str) -> (Vec<Styled>, Color32, String, bool) {
    let syntax = find_syntax(hint);
    let th = THEMES
        .themes
        .get(theme_name)
        .or_else(|| THEMES.themes.values().next())
        .expect("syntect themes");
    let bg = th.settings.background.map(to_color).unwrap_or(Color32::from_gray(30));
    let fg = th.settings.foreground.map(to_color).unwrap_or(Color32::LIGHT_GRAY);
    let mut h = HighlightLines::new(syntax, th);
    let mut out = Vec::new();
    let mut truncated = false;
    for (i, line) in LinesWithEndings::from(code).enumerate() {
        if i >= MAX_LINES {
            truncated = true;
            break;
        }
        // Very long lines (minified files) are not highlighted.
        if line.len() > 4000 {
            out.push(vec![(fg, line.chars().take(4000).collect::<String>() + "…\n", false)]);
            continue;
        }
        match h.highlight_line(line, &SYNTAXES) {
            Ok(ranges) => out.push(
                ranges
                    .into_iter()
                    .map(|(st, s)| (to_color(st.foreground), s.replace('\t', "    "), st.font_style.contains(FontStyle::ITALIC)))
                    .collect(),
            ),
            Err(_) => out.push(vec![(fg, line.to_string(), false)]),
        }
    }
    (out, bg, syntax.name.clone(), truncated)
}

#[cfg(test)]
mod animation_tests {
    use super::*;

    fn gif(frames: u8) -> Vec<u8> {
        let mut bytes = vec![];
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut bytes);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
            for i in 0..frames {
                let img = image::RgbaImage::from_pixel(8, 6, image::Rgba([i * 80, 0, 0, 255]));
                let delay = image::Delay::from_numer_denom_ms(if i == 0 { 0 } else { 200 }, 1);
                enc.encode_frame(image::Frame::from_parts(img, 0, 0, delay)).unwrap();
            }
        }
        bytes
    }

    #[test]
    fn animated_gifs_keep_their_frames_and_delays() {
        let cancel = Arc::new(AtomicBool::new(false));
        let Some(Loaded::Image { orig, more_frames, first_delay, .. }) = decode_animation("gif", &gif(3), &cancel) else { panic!("an animation") };
        assert_eq!(orig, (8, 6));
        assert_eq!(more_frames.len(), 2);
        assert_eq!(first_delay, 0.1, "no delay is played as a tenth of a second");
        assert_eq!(more_frames[0].delay, 0.2);
    }

    #[test]
    fn still_images_and_broken_files_are_not_animations() {
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(decode_animation("gif", &gif(1), &cancel).is_none());
        assert!(decode_animation("gif", b"nope", &cancel).is_none());
        assert!(decode_animation("webp", b"nope", &cancel).is_none());
        assert!(decode_animation("png", &gif(3), &cancel).is_none());
        cancel.store(true, Ordering::Relaxed);
        assert!(decode_animation("gif", &gif(3), &cancel).is_none(), "stops when nobody is waiting");
    }

    #[test]
    fn new_formats_have_their_kinds() {
        for (name, kind) in [
            ("a.HEIC", Kind::Heif),
            ("a.avif", Kind::Heif),
            ("a.mp4", Kind::Video),
            ("a.webm", Kind::Video),
            ("a.CR2", Kind::Raw),
            ("a.arw", Kind::Raw),
            ("a.epub", Kind::Document),
            ("a.odt", Kind::Document),
            ("a.woff2", Kind::Font),
            ("a.glb", Kind::Model),
            ("a.pem", Kind::Certificate),
            ("a.sqlite", Kind::Table),
            ("a.ndjson", Kind::Table),
            ("a.npy", Kind::Table),
            ("a.feather", Kind::Table),
            ("a.h5", Kind::Table),
            ("a.npz", Kind::Table),
            ("model.safetensors", Kind::Table),
            ("a.onnx", Kind::Table),
            ("a.RData", Kind::Table),
            ("a.rda", Kind::Table),
            ("a.rds", Kind::Table),
            ("a.gif", Kind::Image),
        ] {
            assert_eq!(classify(name), kind, "{name}");
        }
        assert_eq!(read_limit("a.woff2"), MAX_IMAGE);
        assert_eq!(read_limit("a.txt"), MAX_TEXT);
    }

    #[test]
    fn size_limits_by_kind() {
        let (mb, gb) = (1024 * 1024, 1024 * 1024 * 1024);
        // What has to be in memory as a whole.
        for name in ["a.png", "a.pdf", "a.svg", "a.ipynb", "a.heic", "a.ttf", "a.pem", "a.rtf"] {
            assert!(!too_large(name, 80 * mb, None), "{name}");
            assert!(too_large(name, 80 * mb + 1, None), "{name}");
        }
        // What is read in place, or from its start.
        for name in ["a.mp4", "a.nef", "a.epub", "a.parquet", "a.sqlite", "a.h5", "a.xlsx", "a.txt", "a.bin", "a.zip", "noextension"] {
            assert!(!too_large(name, gb, None), "{name}");
            assert!(too_large(name, gb + 1, None), "{name}");
        }
        assert!(too_large("a.xls", 256 * mb + 1, None));
        assert!(!too_large("a.xls", 256 * mb, None));
        assert!(too_large("a.stl", crate::model3d::MAX_FILE + 1, None));
        // Only the header is read of these.
        for name in ["a.safetensors", "a.onnx", "a.npz"] {
            assert!(!too_large(name, 100 * gb, None), "{name}");
            assert!(too_large(name, 20 * mb, Some(16 * mb)), "{name}");
        }
        assert!(too_large("a.rds", gb + 1, None));
        // Fetched from elsewhere: only text can be shown from its start.
        assert!(too_large("a.parquet", 20 * mb, Some(16 * mb)));
        assert!(too_large("a.mp4", 20 * mb, Some(16 * mb)));
        assert!(!too_large("a.txt", 20 * mb, Some(16 * mb)));
        assert!(!too_large("a.png", 10 * mb, Some(16 * mb)));
    }

    #[test]
    fn a_file_claiming_a_huge_picture_is_refused_before_decoding() {
        fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            let mut h = crc32fast::Hasher::new();
            h.update(kind);
            h.update(data);
            c.extend(h.finalize().to_be_bytes());
            c
        }
        let mut ihdr = 60000u32.to_be_bytes().to_vec();
        ihdr.extend(60000u32.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        let png = [b"\x89PNG\r\n\x1a\n".to_vec(), chunk(b"IHDR", &ihdr), chunk(b"IDAT", &[0x78, 0x9C, 0x03, 0, 0, 0, 0, 1]), chunk(b"IEND", &[])].concat();
        assert!(decode_oriented(&png).unwrap_err().contains("too large"));
    }

    #[test]
    fn svg_nesting_is_limited_before_parsing() {
        let nested = |n: usize| format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{}{}</svg>", "<g>".repeat(n), "</g>".repeat(n));
        assert!(!svg_too_deep(nested(100).as_bytes()));
        assert!(svg_too_deep(nested(100_000).as_bytes()));
        assert!(matches!(render_svg(nested(100_000).as_bytes(), 1024), Loaded::Error(_)));
        // The deepest allowed still draws on a loader's stack, without overflowing it.
        let deepest = nested(MAX_SVG_DEPTH - 1).replace("<svg ", "<svg width=\"10\" height=\"10\" ");
        let rx = in_background(None, move |_| render_svg(deepest.as_bytes(), 1024));
        assert!(rx.recv_timeout(std::time::Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn huge_gif_canvases_are_not_animated() {
        let mut bytes = gif(2);
        bytes[6..10].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(decode_animation("gif", &bytes, &cancel).is_none());
        assert!(decode_oriented(&bytes).is_err());
    }

    #[test]
    fn long_text_is_clipped_for_layout() {
        let blocks = parse_markdown(&"word ".repeat(100_000), "InspiredGitHub");
        let MdBlock::Para(spans) = &blocks[0] else { panic!() };
        assert!(spans.iter().map(|s| s.text.len()).sum::<usize>() <= MAX_BLOCK_TEXT + 8);
        // Over the whole document's limit: cut, and it says so.
        let many = "para\n\n".repeat(400_000);
        let blocks = parse_markdown(&many, "InspiredGitHub");
        assert!(matches!(blocks.last(), Some(MdBlock::Para(s)) if s[0].text.contains("truncated")));
        let mut spans = vec![Span { text: "é".repeat(10), bold: false, italic: false, code: false, strike: false, link: false }];
        clip_spans(&mut spans, 5);
        assert_eq!(spans[0].text, "éé…", "cut between characters");
    }

    #[test]
    fn panics_become_messages_and_stale_previews_never_start() {
        let rx = in_background(None, |_| panic!("a parser broke"));
        let Ok(Loaded::Error(e)) = rx.recv_timeout(std::time::Duration::from_secs(5)) else { panic!("a message") };
        assert!(e.contains("could not be previewed"), "{e}");
        let cancelled = Arc::new(AtomicBool::new(true));
        let started = Arc::new(AtomicBool::new(false));
        let flag = started.clone();
        let rx = in_background(Some(cancelled), move |_| {
            flag.store(true, Ordering::Relaxed);
            Loaded::Info(String::new())
        });
        assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).is_err(), "no result");
        assert!(!started.load(Ordering::Relaxed), "and no work");
    }

    #[test]
    fn no_more_than_a_few_load_at_once() {
        let cancel = Arc::new(AtomicBool::new(false));
        let running = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let receivers: Vec<_> = (0..12)
            .map(|_| {
                let (running, peak) = (running.clone(), peak.clone());
                in_background(Some(cancel.clone()), move |_| {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    running.fetch_sub(1, Ordering::SeqCst);
                    Loaded::Info(String::new())
                })
            })
            .collect();
        for rx in receivers {
            assert!(rx.recv_timeout(std::time::Duration::from_secs(10)).is_ok());
        }
        assert!(peak.load(Ordering::SeqCst) <= MAX_LOADING);
    }
}

fn looks_textual(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        // A multi-byte char may be cut at the boundary.
        Err(e) => e.valid_up_to() + 4 >= head.len(),
    }
}

/// Decode an image, turned the way its metadata says it should be shown.
fn decode_oriented(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    use image::ImageDecoder;
    let failed = |e: image::ImageError| format!("Cannot decode image: {e}");
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?.into_decoder().map_err(failed)?;
    // A few bytes of file can claim a picture of many gigabytes.
    if decoder.total_bytes() > MAX_DECODED_IMAGE {
        return Err(crate::i18n::t("This image is too large to preview."));
    }
    let orientation = decoder.orientation().map_err(failed)?;
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(failed)?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn image_loaded(img: image::DynamicImage, fmt: String, max_texture: usize) -> Loaded {
    let orig = (img.width(), img.height());
    let max = max_texture.clamp(512, 8192) as u32;
    let img = if img.width() > max || img.height() > max { img.thumbnail(max, max) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    Loaded::Image { rgba: rgba.into_raw(), w, h, orig, premultiplied: false, fmt, more_frames: vec![], first_delay: 0.0 }
}

/// How much memory an animation's frames may take (they all become textures).
const MAX_ANIMATION_BYTES: usize = 256 * 1024 * 1024;
/// The decoder keeps canvases of the full size while it works, whatever it is told to keep.
const MAX_ANIMATION_CANVAS: u64 = 256 * 1024 * 1024;
/// Animations larger than this are shown scaled down.
const MAX_ANIMATION_SIDE: u32 = 1280;

/// An animated GIF or WebP, or `None` if it is a still image or its frames cannot be read.
fn decode_animation(ext: &str, bytes: &[u8], cancel: &crate::media::Cancel) -> Option<Loaded> {
    use image::{AnimationDecoder, ImageDecoder};
    let cursor = || std::io::Cursor::new(bytes);
    let small_enough = |(w, h): (u32, u32)| u64::from(w) * u64::from(h) * 4 <= MAX_ANIMATION_CANVAS;
    let frames: image::Frames = match ext {
        "gif" => {
            let d = image::codecs::gif::GifDecoder::new(cursor()).ok()?;
            if !small_enough(d.dimensions()) {
                return None;
            }
            d.into_frames()
        }
        "webp" => {
            let d = image::codecs::webp::WebPDecoder::new(cursor()).ok()?;
            if !d.has_animation() || !small_enough(d.dimensions()) {
                return None;
            }
            d.into_frames()
        }
        _ => return None,
    };
    let mut out: Vec<AnimFrame> = vec![];
    let mut orig = (0, 0);
    let mut used = 0usize;
    for frame in frames {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(frame) = frame else { break };
        let (num, den) = frame.delay().numer_denom_ms();
        // Players treat very short delays (often 0) as about a tenth of a second.
        let ms = if den == 0 { 0.0 } else { f64::from(num) / f64::from(den) };
        let delay = if ms < 20.0 { 0.1 } else { (ms / 1000.0) as f32 };
        let buffer = frame.into_buffer();
        if out.is_empty() {
            orig = buffer.dimensions();
        }
        let buffer = if buffer.width().max(buffer.height()) > MAX_ANIMATION_SIDE {
            image::DynamicImage::ImageRgba8(buffer).thumbnail(MAX_ANIMATION_SIDE, MAX_ANIMATION_SIDE).to_rgba8()
        } else {
            buffer
        };
        used += buffer.as_raw().len();
        if used > MAX_ANIMATION_BYTES && !out.is_empty() {
            break;
        }
        let (w, h) = (buffer.width() as usize, buffer.height() as usize);
        out.push(AnimFrame { rgba: buffer.into_raw(), w, h, delay });
    }
    if out.len() < 2 {
        return None;
    }
    let first = out.remove(0);
    Some(Loaded::Image { rgba: first.rgba, w: first.w, h: first.h, orig, premultiplied: false, fmt: ext.to_uppercase(), more_frames: out, first_delay: first.delay })
}

/// A picture for a summary, sized for the view.
fn summary_picture(img: image::DynamicImage, max_texture: usize) -> SummaryPicture {
    let max = max_texture.clamp(512, 4096) as u32;
    let img = if img.width() > max || img.height() > max { img.thumbnail(max, max) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    // This image's alpha is meant as it is; opaque photos are unaffected by the premultiplied upload.
    SummaryPicture { rgba: rgba.into_raw(), w, h, tinted: false }
}

fn field_row(label: String, value: Option<String>, rows: &mut Vec<crate::certview::Row>) {
    if let Some(v) = value {
        rows.push(crate::certview::Row::Field(label, v));
    }
}

fn video_summary(path: &Path, max_texture: usize, cancel: &crate::media::Cancel, early: &dyn Fn(Loaded)) -> Loaded {
    use crate::i18n::t;
    use crate::media::{format_duration, video_info, video_thumbnail};
    let info = match std::fs::File::open(path) {
        Ok(f) => video_info(&mut std::io::BufReader::new(f)),
        Err(e) => return Loaded::Error(e.to_string()),
    };
    let mut rows = vec![];
    field_row(t("Duration"), info.duration.map(format_duration), &mut rows);
    field_row(t("Resolution"), info.size.map(|(w, h)| format!("{w} × {h}")), &mut rows);
    field_row(t("Frame rate"), info.fps.map(|f| format!("{:.2} fps", f).replace(".00 fps", " fps")), &mut rows);
    field_row(t("Video"), info.video_codec.clone(), &mut rows);
    field_row(t("Audio"), info.audio_codec.clone(), &mut rows);
    if rows.is_empty() {
        rows.push(crate::certview::Row::Note(t("The details of this video could not be read.")));
    }
    // The facts are ready now; a thumbnail can take a while.
    let format = ext_of(&path.to_string_lossy()).to_uppercase();
    early(Loaded::Summary(Summary { format: format.clone(), picture: None, rows: rows.clone(), picture_pending: true }));
    let picture = video_thumbnail(path, info.duration, cancel).and_then(|png| decode_oriented(&png).ok()).map(|img| summary_picture(img, max_texture));
    if picture.is_none() {
        rows.push(crate::certview::Row::Note(if cfg!(target_os = "macos") { t("No thumbnail: macOS cannot read this video. Install FFmpeg to see one.") } else { t("No thumbnail: install FFmpeg to see one.") }));
    }
    Loaded::Summary(Summary { format, picture, rows, picture_pending: false })
}

fn raw_summary<R: std::io::Read + std::io::Seek>(r: &mut R, fmt: String, max_texture: usize) -> Loaded {
    use crate::i18n::t;
    let Some(info) = crate::rawphoto::parse(r) else {
        return Loaded::Error(t("Not a camera RAW file this preview can read."));
    };
    let mut rows = vec![];
    let camera = [info.make.clone(), info.model.clone()].into_iter().flatten().collect::<Vec<_>>();
    // Models often repeat the maker's name.
    let camera = match camera.as_slice() {
        [make, model] if model.starts_with(make.split(' ').next().unwrap_or(make)) => Some(model.clone()),
        [] => None,
        names => Some(names.join(" ")),
    };
    field_row(t("Camera"), camera, &mut rows);
    field_row(t("Lens"), info.lens.clone(), &mut rows);
    field_row(t("Taken"), info.date.clone(), &mut rows);
    field_row(t("Exposure"), info.exposure.clone(), &mut rows);
    field_row(t("Aperture"), info.aperture.clone(), &mut rows);
    field_row("ISO".to_string(), info.iso.map(|i| i.to_string()), &mut rows);
    field_row(t("Focal length"), info.focal.clone(), &mut rows);
    field_row(t("Sensor"), info.sensor.map(|(w, h)| format!("{w} × {h}")), &mut rows);
    let picture = crate::rawphoto::preview_image(r, &info).map(|img| summary_picture(img, max_texture));
    if picture.is_none() {
        rows.push(crate::certview::Row::Note(t("This file has no preview image the app can show.")));
    }
    Loaded::Summary(Summary { format: format!("RAW · {fmt}"), picture, rows, picture_pending: false })
}

fn font_summary(name: &str, bytes: &[u8]) -> Loaded {
    match crate::fontview::load(name, bytes) {
        Ok(f) => Loaded::Summary(Summary {
            format: f.format,
            picture: f.sheet.map(|(rgba, w, h)| SummaryPicture { rgba, w, h, tinted: true }),
            rows: f.rows.into_iter().map(|(l, v)| crate::certview::Row::Field(l, v)).collect(),
            picture_pending: false,
        }),
        Err(e) => Loaded::Error(e),
    }
}

fn certificate_summary(bytes: &[u8]) -> Loaded {
    match crate::certview::load(bytes) {
        Ok((format, rows)) => Loaded::Summary(Summary { format, picture: None, rows, picture_pending: false }),
        Err(e) => Loaded::Error(e),
    }
}

/// A HEIC, HEIF or AVIF image, from the system's tools.
fn heif_loaded(path: &Path, fmt: String, max_texture: usize, cancel: &crate::media::Cancel) -> Loaded {
    match crate::media::decode_heif(path, cancel).and_then(|png| decode_oriented(&png)) {
        Ok(img) => image_loaded(img, fmt, max_texture),
        Err(e) => Loaded::Error(e),
    }
}

/// Previews read from a file in place (see `Preview::reads_files_directly`).
fn decode_file(name: &str, path: &Path, syntax_theme: &str, max_texture: usize, cancel: &crate::media::Cancel, early: &dyn Fn(Loaded)) -> Loaded {
    let fmt = ext_of(name).to_uppercase();
    match classify(name) {
        Kind::Document => match crate::textdocs::load_path(name, path, syntax_theme) {
            Ok(d) => Loaded::Document(d),
            Err(e) => Loaded::Error(e),
        },
        Kind::Heif => heif_loaded(path, fmt, max_texture, cancel),
        Kind::Video => video_summary(path, max_texture, cancel, early),
        Kind::Raw => match std::fs::File::open(path) {
            Ok(f) => raw_summary(&mut std::io::BufReader::new(f), fmt, max_texture),
            Err(e) => Loaded::Error(e.to_string()),
        },
        _ => {
            // Parsing needs all of it in memory.
            if std::fs::metadata(path).is_ok_and(|m| m.len() > crate::model3d::MAX_FILE) {
                return Loaded::Error(crate::i18n::t("This model is too large to preview."));
            }
            match std::fs::read(path) {
                Ok(bytes) => match crate::model3d::load(name, &bytes, path.parent()) {
                    Ok(mesh) => Loaded::Model(Box::new(crate::model3d::ModelView::new(mesh))),
                    Err(e) => Loaded::Error(e),
                },
                Err(e) => Loaded::Error(e.to_string()),
            }
        }
    }
}

/// Run `f` on a temporary copy of a file that has no path of its own (in an archive or on a server).
fn with_temp_file(name: &str, bytes: &[u8], f: impl FnOnce(&Path) -> Loaded) -> Loaded {
    let dir = match crate::media::TempDir::new() {
        Ok(d) => d,
        Err(e) => return Loaded::Error(e),
    };
    let ext = ext_of(name);
    let path = dir.path().join(format!("preview.{ext}"));
    if let Err(e) = std::fs::write(&path, bytes) {
        return Loaded::Error(e.to_string());
    }
    f(&path)
}

fn decode(name: &str, bytes: Vec<u8>, syntax_theme: &str, max_texture: usize, cancel: &crate::media::Cancel, early: &dyn Fn(Loaded)) -> Loaded {
    let kind = classify(name);
    let ext = ext_of(name);
    if cancel.load(Ordering::Relaxed) {
        // Nobody is waiting for this any more: leave the decoding undone.
        return Loaded::Error(String::new());
    }
    match kind {
        Kind::Image => {
            if let Some(animation) = decode_animation(&ext, &bytes, cancel) {
                return animation;
            }
            match decode_oriented(&bytes) {
                Ok(img) => image_loaded(img, ext.to_uppercase(), max_texture),
                Err(e) => Loaded::Error(e),
            }
        }
        Kind::Heif => with_temp_file(name, &bytes, |p| heif_loaded(p, ext.to_uppercase(), max_texture, cancel)),
        Kind::Video => with_temp_file(name, &bytes, |p| video_summary(p, max_texture, cancel, early)),
        Kind::Raw => raw_summary(&mut std::io::Cursor::new(bytes), ext.to_uppercase(), max_texture),
        Kind::Document => match crate::textdocs::load(name, bytes, syntax_theme) {
            Ok(d) => Loaded::Document(d),
            Err(e) => Loaded::Error(e),
        },
        Kind::Font => font_summary(name, &bytes),
        Kind::Certificate => certificate_summary(&bytes),
        Kind::Model => match crate::model3d::load(name, &bytes, None) {
            Ok(mesh) => Loaded::Model(Box::new(crate::model3d::ModelView::new(mesh))),
            Err(e) => Loaded::Error(e),
        },
        Kind::Svg => render_svg(&bytes, max_texture),
        Kind::Markdown => {
            let text = String::from_utf8_lossy(&bytes);
            let (source, bg, _, _) = highlight(&text, "md", syntax_theme);
            Loaded::Markdown { blocks: parse_markdown(&text, syntax_theme), source, bg }
        }
        Kind::Pdf => {
            if bytes.len() >= MAX_IMAGE {
                // The list of its objects is at the end of the file.
                return Loaded::Error(format!("{} ({} {})", crate::i18n::t("This file is too large to preview here."), crate::i18n::t("limit:"), human_size(MAX_IMAGE as u64)));
            }
            Loaded::Pdf(Box::new(crate::docview::start_pdf(bytes, max_texture)))
        }
        Kind::Notebook => match crate::docview::parse_notebook(&bytes, syntax_theme) {
            Ok(v) => Loaded::Notebook(Box::new(v)),
            Err(e) => Loaded::Error(e),
        },
        Kind::Table => match crate::table::load_bytes(name, bytes.clone()) {
            Ok(data) => {
                let source = crate::table::is_csv(name).then(|| {
                    let head = &bytes[..bytes.len().min(512 * 1024)];
                    let (lines, bg, _, _) = highlight(&String::from_utf8_lossy(head), "csv", syntax_theme);
                    (lines, bg)
                });
                Loaded::Table { data, source }
            }
            Err(e) => Loaded::Error(e),
        },
        Kind::Code | Kind::Text => text_preview(&bytes, if kind == Kind::Code { &ext } else { "txt" }, name, syntax_theme),
        Kind::Other => {
            if looks_textual(&bytes) {
                text_preview(&bytes, &ext, name, syntax_theme)
            } else {
                let total = bytes.len();
                let mut b = bytes;
                b.truncate(16 * 1024);
                Loaded::Hex { bytes: b, total }
            }
        }
    }
}

fn text_preview(bytes: &[u8], hint: &str, name: &str, theme: &str) -> Loaded {
    let truncated_bytes = bytes.len() >= MAX_TEXT;
    let text = String::from_utf8_lossy(bytes);
    let hint = if hint.is_empty() { name.to_lowercase() } else { hint.to_string() };
    let (lines, bg, lang, truncated) = highlight(&text, &hint, theme);
    Loaded::Code { lines, bg, truncated: truncated || truncated_bytes, lang }
}

/// Deeper than this an SVG is refused: the parsers recurse through its nesting, and a
/// stack overflow cannot be caught, it ends the whole app.
const MAX_SVG_DEPTH: usize = 512;

fn svg_too_deep(bytes: &[u8]) -> bool {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let config = reader.config_mut();
    config.check_end_names = false;
    config.allow_unmatched_ends = true;
    let mut buf = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => {
                depth += 1;
                if depth > MAX_SVG_DEPTH {
                    return true;
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
        buf.clear();
    }
}

fn render_svg(bytes: &[u8], max_texture: usize) -> Loaded {
    use resvg::{tiny_skia, usvg};
    if svg_too_deep(bytes) {
        return Loaded::Error(crate::i18n::t("This SVG is nested too deeply to preview."));
    }
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = match usvg::Tree::from_data(bytes, &opt) {
        Ok(t) => t,
        Err(e) => return Loaded::Error(format!("Invalid SVG: {e}")),
    };
    let size = tree.size();
    let (ow, oh) = (size.width().max(1.0), size.height().max(1.0));
    // Render at a comfortable resolution (at least 1024px on the long side).
    let target = 1024f32.max(ow.max(oh)).min(max_texture.min(4096) as f32);
    let scale = target / ow.max(oh);
    let (w, h) = ((ow * scale).ceil() as u32, (oh * scale).ceil() as u32);
    let Some(mut pixmap) = tiny_skia::Pixmap::new(w.max(1), h.max(1)) else {
        return Loaded::Error("SVG too large".into());
    };
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Loaded::Image {
        rgba: pixmap.data().to_vec(),
        w: w as usize,
        h: h as usize,
        orig: (ow.round() as u32, oh.round() as u32),
        premultiplied: true,
        fmt: "SVG".into(),
        more_frames: vec![],
        first_delay: 0.0,
    }
}

/// Plain text as a code block, for documents that have preformatted text.
pub fn plain_code_block(code: &str, syntax_theme: &str) -> MdBlock {
    let code = &code[..floor_char(code, MAX_MARKDOWN / 4)];
    let (lines, bg, _, _) = highlight(code, "txt", syntax_theme);
    MdBlock::Code { lines, bg }
}

/// The largest length at most `max` that ends between two characters of `s`.
fn floor_char(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Keep at most `max` bytes of text across these spans.
fn clip_spans(spans: &mut Vec<Span>, max: usize) {
    let mut left = max;
    let mut cut = false;
    spans.retain_mut(|s| {
        if cut {
            return false;
        }
        if s.text.len() > left {
            s.text.truncate(floor_char(&s.text, left));
            s.text.push('…');
            cut = true;
        } else {
            left -= s.text.len();
        }
        true
    });
}

/// Limit how much text each block holds (see `MAX_BLOCK_TEXT`).
pub fn clip_block_text(blocks: &mut [MdBlock]) {
    for b in blocks {
        match b {
            MdBlock::Heading(_, spans) | MdBlock::Para(spans) | MdBlock::Quote(spans) | MdBlock::Item { spans, .. } => clip_spans(spans, MAX_BLOCK_TEXT),
            MdBlock::Table { rows, .. } => rows.iter_mut().flatten().for_each(|cell| clip_spans(cell, MAX_BLOCK_TEXT / 16)),
            MdBlock::Code { .. } | MdBlock::Rule => {}
        }
    }
}

// ---------------------------------------------------------------- markdown

pub fn parse_markdown(src: &str, syntax_theme: &str) -> Vec<MdBlock> {
    let end = floor_char(src, MAX_MARKDOWN);
    let mut blocks = parse_markdown_blocks(&src[..end], syntax_theme);
    clip_block_text(&mut blocks);
    if end < src.len() {
        blocks.push(MdBlock::Para(vec![Span { text: crate::i18n::t("(truncated)"), bold: false, italic: true, code: false, strike: false, link: false }]));
    }
    blocks
}

fn parse_markdown_blocks(src: &str, syntax_theme: &str) -> Vec<MdBlock> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut blocks = Vec::new();
    let mut buf: Vec<Span> = Vec::new();
    let (mut bold, mut italic, mut strike, mut link) = (0, 0, 0, 0);
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut marker: Option<String> = None;
    let mut quote = 0;
    let mut code: Option<(String, String)> = None;
    let mut heading: Option<u8> = None;
    let mut table: Option<(Vec<Vec<Vec<Span>>>, Vec<Vec<Span>>, bool)> = None;
    let mut in_head = false;
    let mut image_alt: Option<String> = None;

    macro_rules! push_span {
        ($text:expr, $code:expr) => {{
            let s = Span {
                text: $text,
                bold: bold > 0 || heading.is_some(),
                italic: italic > 0,
                code: $code,
                strike: strike > 0,
                link: link > 0,
            };
            buf.push(s);
        }};
    }

    let flush_item = |buf: &mut Vec<Span>, marker: &mut Option<String>, lists: &Vec<Option<u64>>, blocks: &mut Vec<MdBlock>| {
        if !buf.is_empty() || marker.is_some() {
            blocks.push(MdBlock::Item {
                depth: lists.len().saturating_sub(1),
                marker: marker.take().unwrap_or_default(),
                spans: std::mem::take(buf),
            });
        }
    };

    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => heading = Some(level as u8),
                Tag::BlockQuote { .. } => quote += 1,
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    if !lists.is_empty() {
                        flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                    }
                    lists.push(start);
                }
                Tag::Item => {
                    flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                    let m = match lists.last_mut() {
                        Some(Some(n)) => {
                            let s = format!("{n}.");
                            *n += 1;
                            s
                        }
                        _ => ["•", "–", "▪"][(lists.len().saturating_sub(1)) % 3].to_string(),
                    };
                    marker = Some(m);
                }
                Tag::Table(_) => table = Some((Vec::new(), Vec::new(), false)),
                Tag::TableHead => in_head = true,
                Tag::TableRow | Tag::TableCell => {}
                Tag::Emphasis => italic += 1,
                Tag::Strong => bold += 1,
                Tag::Strikethrough => strike += 1,
                Tag::Link { .. } => link += 1,
                Tag::Image { .. } => image_alt = Some(String::new()),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if !lists.is_empty() {
                        flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                        marker = None;
                    } else if quote > 0 {
                        blocks.push(MdBlock::Quote(std::mem::take(&mut buf)));
                    } else if !buf.is_empty() {
                        blocks.push(MdBlock::Para(std::mem::take(&mut buf)));
                    }
                }
                TagEnd::Heading { .. } => {
                    blocks.push(MdBlock::Heading(heading.take().unwrap_or(1), std::mem::take(&mut buf)));
                }
                TagEnd::BlockQuote { .. } => quote -= 1,
                TagEnd::CodeBlock => {
                    if let Some((lang, mut text)) = code.take() {
                        while text.ends_with('\n') {
                            text.pop();
                        }
                        let (lines, bg, _, _) = highlight(&text, if lang.is_empty() { "txt" } else { &lang }, syntax_theme);
                        blocks.push(MdBlock::Code { lines, bg });
                    }
                }
                TagEnd::List { .. } => {
                    flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                    lists.pop();
                }
                TagEnd::Item => {
                    if !buf.is_empty() {
                        flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                    }
                    marker = None;
                }
                TagEnd::TableCell => {
                    if let Some((_, row, _)) = &mut table {
                        row.push(std::mem::take(&mut buf));
                    }
                }
                TagEnd::TableHead => {
                    in_head = false;
                    if let Some((rows, row, has)) = &mut table {
                        rows.push(std::mem::take(row));
                        *has = true;
                    }
                }
                TagEnd::TableRow => {
                    if let Some((rows, row, _)) = &mut table {
                        rows.push(std::mem::take(row));
                    }
                }
                TagEnd::Table => {
                    if let Some((rows, _, has_header)) = table.take() {
                        blocks.push(MdBlock::Table { rows, has_header });
                    }
                }
                TagEnd::Emphasis => italic -= 1,
                TagEnd::Strong => bold -= 1,
                TagEnd::Strikethrough => strike -= 1,
                TagEnd::Link => link -= 1,
                TagEnd::Image => {
                    if let Some(alt) = image_alt.take() {
                        push_span!(format!("[🖼 {}]", if alt.is_empty() { "image" } else { &alt }), false);
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, c)) = &mut code {
                    c.push_str(&t);
                } else if let Some(alt) = &mut image_alt {
                    alt.push_str(&t);
                } else {
                    push_span!(t.to_string(), false);
                }
            }
            Event::Code(t) => push_span!(t.to_string(), true),
            Event::SoftBreak => push_span!(" ".to_string(), false),
            Event::HardBreak => push_span!("\n".to_string(), false),
            Event::Rule => blocks.push(MdBlock::Rule),
            Event::TaskListMarker(done) => {
                marker = Some(if done { "☑" } else { "☐" }.to_string());
            }
            _ => {}
        }
    }
    let _ = in_head;
    if !buf.is_empty() {
        blocks.push(MdBlock::Para(buf));
    }
    blocks
}

fn spans_job(spans: &[Span], size: f32, theme: &Theme, base: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for s in spans {
        let mut fmt = TextFormat {
            // Reading view: Source Serif for prose, Source Code Pro for code.
            font_id: if s.code {
                FontId::monospace(size * 0.88)
            } else if s.bold {
                FontId::new(size, egui::FontFamily::Name("serif-bold".into()))
            } else if s.italic {
                FontId::new(size, egui::FontFamily::Name("serif-italic".into()))
            } else {
                FontId::new(size, egui::FontFamily::Name("serif".into()))
            },
            color: if s.link {
                theme.accent
            } else {
                base
            },
            italics: s.italic && s.code,
            ..Default::default()
        };
        if s.code {
            fmt.background = theme.surface;
            fmt.color = theme.marked;
        }
        if s.link {
            fmt.underline = egui::Stroke::new(1.0_f32, theme.accent);
        }
        if s.strike {
            fmt.strikethrough = egui::Stroke::new(1.0_f32, base);
        }
        job.append(&s.text, 0.0, fmt);
    }
    job.wrap.max_width = width;
    job
}

pub fn markdown_ui(ui: &mut egui::Ui, blocks: &[MdBlock], theme: &Theme) {
    let width = ui.available_width() - 8.0;
    for b in blocks {
        match b {
            MdBlock::Heading(level, spans) => {
                let size = match level {
                    1 => 26.0,
                    2 => 21.0,
                    3 => 18.0,
                    4 => 16.0,
                    _ => 15.0,
                };
                ui.add_space(6.0);
                ui.label(spans_job(spans, size, theme, theme.accent, width));
                if *level <= 2 {
                    let r = ui.available_rect_before_wrap();
                    ui.painter().hline(r.x_range(), r.top(), egui::Stroke::new(1.0_f32, theme.border));
                    ui.add_space(4.0);
                }
            }
            MdBlock::Para(spans) => {
                ui.label(spans_job(spans, 14.0, theme, theme.text, width));
                ui.add_space(6.0);
            }
            MdBlock::Quote(spans) => {
                egui::Frame::NONE
                    .stroke(egui::Stroke::new(0.0_f32, theme.border))
                    .inner_margin(egui::Margin { left: 10, right: 4, top: 2, bottom: 2 })
                    .show(ui, |ui| {
                        let r = ui.label(spans_job(spans, 14.0, theme, theme.weak, width - 20.0)).rect;
                        ui.painter().vline(r.left() - 7.0, r.y_range(), egui::Stroke::new(3.0_f32, theme.accent));
                    });
                ui.add_space(6.0);
            }
            MdBlock::Item { depth, marker, spans } => {
                ui.horizontal_top(|ui| {
                    ui.add_space(12.0 + *depth as f32 * 18.0);
                    ui.label(egui::RichText::new(marker).color(theme.accent));
                    let w = ui.available_width();
                    ui.label(spans_job(spans, 14.0, theme, theme.text, w));
                });
            }
            MdBlock::Code { lines, bg } => {
                let mut job = LayoutJob::default();
                let font = FontId::monospace(12.5);
                for line in lines {
                    for (c, s, it) in line {
                        job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *c, italics: *it, ..Default::default() });
                    }
                }
                egui::Frame::NONE.fill(*bg).inner_margin(8.0).show(ui, |ui| {
                    ui.set_min_width(width - 16.0);
                    egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
                        ui.add(egui::Label::new(job).extend());
                    });
                });
                ui.add_space(6.0);
            }
            MdBlock::Table { rows, has_header } => {
                egui::Grid::new(ui.next_auto_id()).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                    for (i, row) in rows.iter().enumerate() {
                        for cell in row {
                            let mut c = cell.clone();
                            if i == 0 && *has_header {
                                for s in &mut c {
                                    s.bold = true;
                                }
                            }
                            ui.label(spans_job(&c, 13.5, theme, theme.text, 400.0));
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
            }
            MdBlock::Rule => {
                ui.separator();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_ext() {
        assert_eq!(classify("a.PNG"), Kind::Image);
        assert_eq!(classify("README.md"), Kind::Markdown);
        assert_eq!(classify("main.rs"), Kind::Code);
        assert_eq!(classify("Makefile"), Kind::Code);
        assert_eq!(classify("x.bin"), Kind::Other);
    }

    #[test]
    fn markdown_blocks() {
        let md = "# Title\n\nSome **bold** and `code`.\n\n- a\n- b\n  1. nested\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n> quote\n\n---\n- [x] done\n";
        let blocks = parse_markdown(md, "base16-ocean.dark");
        assert!(matches!(blocks[0], MdBlock::Heading(1, _)));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Code { .. })));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Table { .. })));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Quote(_))));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Rule)));
        let items: Vec<_> = blocks
            .iter()
            .filter_map(|b| if let MdBlock::Item { depth, marker, .. } = b { Some((*depth, marker.clone())) } else { None })
            .collect();
        assert_eq!(items[0], (0, "•".into()));
        assert_eq!(items[2], (1, "1.".into()));
        assert!(items.iter().any(|i| i.1.contains('☑')));
    }

    #[test]
    fn highlight_rust() {
        let (lines, _, lang, _) = highlight("fn main() {}\n", "rs", "base16-ocean.dark");
        assert_eq!(lang, "Rust");
        assert!(lines[0].len() > 1);
    }

    #[test]
    fn text_sniff() {
        assert!(looks_textual(b"hello\nworld"));
        assert!(!looks_textual(&[0, 1, 2, 3]));
    }
}
