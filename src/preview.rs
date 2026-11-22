//! File previews: images (raster + SVG), Markdown, syntax-highlighted code,
//! plain text and a hex dump for binaries, and the views of formats read elsewhere.
pub mod audio;
pub mod code;
pub mod certview;
pub mod datameta;
pub mod docview;
pub mod fontview;
pub mod geoview;
pub mod jsonview;
pub mod markdown;
pub mod mail;
pub mod media;
pub mod model3d;
pub mod pickle;
pub mod psd;
pub mod rawphoto;
pub mod rdata;
pub mod records;
pub mod scidata;
pub mod sheet;
pub mod table;
pub mod tensors;
pub mod textdocs;
pub mod widgets;
use crate::style::theme::Theme;
use crate::preview::code::{code_view, highlight, Styled};
use crate::preview::markdown::{markdown_ui, parse_markdown, MdBlock};
use crate::preview::widgets::{header_row, locked_ui, upload_texture, LOCKED};
use crate::util::{ext_of, human_size, Res};
use egui::{Color32, FontId, TextureHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

const MAX_TEXT: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 20_000;
pub const MAX_IMAGE: usize = 80 * 1024 * 1024;
/// Most memory one decoded image may take. Larger ones are refused before decoding.
const MAX_DECODED_IMAGE: u64 = 384 * 1024 * 1024;
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
    /// Tags, stream facts and the cover picture.
    Audio,
    /// Shown by the picture they carry: iWork documents, comic books.
    Package,
    Raw,
    Document,
    Font,
    Model,
    Certificate,
    /// GeoJSON, WKT and WKB: shapes drawn without a map.
    Geo,
    /// A tree to explore; text past `jsonview::MAX_JSON`.
    Json,
    Code,
    Text,
    Other,
}

pub fn classify(name: &str) -> Kind {
    let ext = ext_of(name);
    let lower = name.to_lowercase();
    if is_comic(name) || matches!(ext.as_str(), "pages" | "key" | "numbers") {
        return Kind::Package;
    }
    if crate::preview::table::is_table(name) {
        return Kind::Table;
    }
    if crate::preview::textdocs::is_document(name) {
        return Kind::Document;
    }
    if crate::preview::fontview::is_font(name) {
        return Kind::Font;
    }
    if crate::preview::model3d::is_model(name) {
        return Kind::Model;
    }
    if crate::preview::certview::is_cert(name) {
        return Kind::Certificate;
    }
    if crate::preview::geoview::is_geo(name) {
        return Kind::Geo;
    }
    if crate::preview::jsonview::is_json(name) {
        return Kind::Json;
    }
    if crate::preview::audio::is_audio(name) {
        return Kind::Audio;
    }
    match ext.as_str() {
        "pdf" => return Kind::Pdf,
        "ipynb" => return Kind::Notebook,
        "heic" | "heif" | "hif" | "avif" => return Kind::Heif,
        "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" | "flv" | "mpg" | "mpeg" | "m2ts" | "mts" | "3gp" | "ogv" => return Kind::Video,
        "cr2" | "nef" | "arw" | "dng" => return Kind::Raw,
        _ => {}
    }
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" | "tga" | "qoi" | "pbm" | "pgm" | "ppm" | "pnm" | "pam" | "dds" | "exr"
        | "hdr" | "psd" | "psb" => Kind::Image,
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
        // One byte more tells a file past the limit.
        Kind::Json => crate::preview::jsonview::MAX_JSON + 1,
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
        // Read in place: the layers before the merged picture are stepped over.
        Kind::Image if is_psd(name) => LARGE_FILE,
        Kind::Geo if crate::preview::geoview::reads_path(name) => LARGE_FILE,
        Kind::Image | Kind::Svg | Kind::Pdf | Kind::Notebook | Kind::Heif | Kind::Font | Kind::Certificate | Kind::Geo => MAX_IMAGE as u64,
        Kind::Document if !crate::preview::textdocs::reads_in_place(name) => MAX_IMAGE as u64,
        Kind::Model => crate::preview::model3d::MAX_FILE,
        Kind::Table if matches!(ext_of(name).as_str(), "xls" | "xlsb" | "ods") => LARGE_WORKBOOK,
        // Model weights of many gigabytes: only their header is read.
        Kind::Table if crate::preview::table::reads_header_only(name) => u64::MAX,
        _ => LARGE_FILE,
    }
}

/// Whether a file of this size gets the "too large" message instead of a preview.
/// `readable` is how much can be fetched from where the file is, if not a local disk:
/// text can be shown from its start, while other formats need all of the file.
pub fn too_large(name: &str, size: u64, readable: Option<u64>) -> bool {
    let start_is_enough = matches!(classify(name), Kind::Code | Kind::Text | Kind::Markdown | Kind::Other | Kind::Json);
    size > size_limit(name) || (!start_is_enough && readable.is_some_and(|r| size > r))
}

enum Loaded {
    /// `details` are the photo's EXIF facts, if it has any.
    Image { rgba: Vec<u8>, w: usize, h: usize, orig: (u32, u32), premultiplied: bool, fmt: String, more_frames: Vec<AnimFrame>, first_delay: f32, details: Vec<crate::preview::certview::Row> },
    Code { lines: Vec<Styled>, bg: Color32, truncated: bool, lang: String },
    Markdown { blocks: Vec<MdBlock>, source: Vec<Styled>, bg: Color32 },
    /// `total` is how much was read; `more` when the file goes on past it.
    Hex { bytes: Vec<u8>, total: usize, more: bool },
    Table { data: crate::preview::sheet::TableData, source: Option<(Vec<Styled>, Color32)> },
    Pdf(Box<crate::preview::docview::PdfView>),
    Notebook(Box<crate::preview::docview::NotebookView>),
    Model(Box<crate::preview::model3d::ModelView>),
    Geo(Box<crate::preview::geoview::GeoView>),
    Json(Box<crate::preview::jsonview::JsonView>),
    Document(crate::preview::textdocs::Document),
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
    rows: Vec<crate::preview::certview::Row>,
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
    cancel: crate::preview::media::Cancel,
    /// The archive this file is in, whose password this app can ask for.
    archive: Option<PathBuf>,
    /// `Enter password` was pressed.
    password_wanted: bool,
    /// The file's real size, when the caller knows it (only the start may be read).
    size: Option<u64>,
    /// When loading started: the spinner shows only for slow files.
    born: std::time::Instant,
    /// The context to wake when the loader is done (set on the first frame).
    wake: Wake,
}

/// Wakes the UI when a background loader has sent its result.
type Wake = Arc<std::sync::OnceLock<egui::Context>>;

/// A preview still loading after this long shows a spinner; quicker ones do not flash it.
const SPINNER_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

impl Drop for Preview {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub type Loader = Box<dyn FnOnce() -> Res<Vec<u8>> + Send>;

/// A turn to load a preview; the next one in line starts when it is dropped.
struct LoadingTurn;

static LOADING: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
static TURN_FREE: std::sync::Condvar = std::sync::Condvar::new();

impl LoadingTurn {
    /// Wait for a turn. `None` if the preview is gone by then, which is what happens
    /// to everything the cursor passed over: it never gets to read its file.
    fn wait(cancel: &crate::preview::media::Cancel) -> Option<Self> {
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

#[cfg(test)]
fn in_background(cancel: Option<crate::preview::media::Cancel>, job: impl FnOnce(&std::sync::mpsc::Sender<Loaded>) -> Loaded + Send + 'static) -> Receiver<Loaded> {
    in_background_waking(cancel, Wake::default(), job)
}

/// Load in the background: on a roomy stack, and turning a panic into a message, so a
/// file that breaks a parser shows an error instead of loading forever. The UI is woken
/// when the result is sent.
fn in_background_waking(cancel: Option<crate::preview::media::Cancel>, wake: Wake, job: impl FnOnce(&std::sync::mpsc::Sender<Loaded>) -> Loaded + Send + 'static) -> Receiver<Loaded> {
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
            Loaded::Error(e) if e == LOCKED || crate::files::archive::is_password_error(&e) => Loaded::Locked,
            other => other,
        });
        if let Some(ctx) = wake.get() {
            ctx.request_repaint();
        }
    });
    rx
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
            size: None,
            born: std::time::Instant::now(),
            wake: Wake::default(),
        }
    }

    /// Set once nobody waits for this preview any more: a loader still reading can stop.
    pub fn cancel_flag(&self) -> crate::preview::media::Cancel {
        self.cancel.clone()
    }

    /// Start loading in the background (with a loading turn when `turn`).
    fn load(&mut self, turn: bool, job: impl FnOnce(&std::sync::mpsc::Sender<Loaded>) -> Loaded + Send + 'static) {
        self.rx = Some(in_background_waking(turn.then(|| self.cancel.clone()), self.wake.clone(), job));
    }

    /// The file's real size, for a preview that reads only its start.
    pub fn of_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
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
        p.load(true, move |tx| match loader() {
            Ok(bytes) => {
                let cut_short = bytes.len() >= limit;
                match decode(&n, bytes, &theme, max_texture, &cancel, &|early| drop(tx.send(early))) {
                    // What it failed on is the missing rest of the file.
                    Loaded::Error(_) if cut_short => Loaded::Error(format!("{} ({} {})", crate::i18n::t("This file is too large to preview here."), crate::i18n::t("limit:"), human_size(limit as u64))),
                    Loaded::Hex { bytes, total, .. } => Loaded::Hex { bytes, total, more: cut_short },
                    loaded => loaded,
                }
            }
            Err(e) => Loaded::Error(e),
        });
        p
    }

    /// Whether a local file of this name is better read in place than loaded into memory
    /// first: it may be huge, tools need its path, or files next to it matter.
    pub fn reads_files_directly(name: &str) -> bool {
        matches!(classify(name), Kind::Heif | Kind::Video | Kind::Audio | Kind::Package | Kind::Raw | Kind::Model)
            || crate::preview::textdocs::reads_in_place(name)
            || is_psd(name)
            || crate::preview::geoview::reads_path(name)
    }

    /// A preview read directly from a local file (see `reads_files_directly`).
    pub fn local_file(key: String, name: String, path: PathBuf, syntax_theme: &str, max_texture: usize) -> Self {
        let n = name.clone();
        let mut p = Self::blank(key, name, false);
        let cancel = p.cancel.clone();
        let theme = syntax_theme.to_string();
        p.load(true, move |tx| decode_file(&n, &path, &theme, max_texture, &cancel, &|early| drop(tx.send(early))));
        p
    }

    /// Tabular preview read directly from a local file (avoids loading huge files).
    pub fn table_local(key: String, name: String, path: std::path::PathBuf, syntax_theme: &str) -> Self {
        let theme = syntax_theme.to_string();
        let n = name.clone();
        let mut p = Self::blank(key, name, true);
        p.load(true, move |_| match crate::preview::table::load_path(&path) {
            Ok(data) => {
                let source = crate::preview::table::source_language(&n).map(|lang| {
                    use std::io::Read;
                    let mut head = Vec::new();
                    if let Ok(f) = crate::util::open_regular(&path) {
                        let _ = f.take(512 * 1024).read_to_end(&mut head);
                    }
                    let (lines, bg, _, _) = highlight(&String::from_utf8_lossy(&head), lang, &theme);
                    (lines, bg)
                });
                Loaded::Table { data, source }
            }
            Err(e) => {
                use std::io::Read;
                let mut head = Vec::new();
                if let Ok(f) = crate::util::open_regular(&path) {
                    let _ = f.take(MAX_TEXT as u64).read_to_end(&mut head);
                }
                text_instead(e, &head, &n, &theme)
            }
        });
        p
    }

    /// A text summary produced in the background (folders, archives). `f` is given the
    /// flag that is set once the preview is gone, and should stop early then.
    pub fn text_async(key: String, name: String, f: impl FnOnce(&crate::preview::media::Cancel) -> String + Send + 'static) -> Self {
        let mut p = Self::blank(key, name, true);
        let cancel = p.cancel.clone();
        // Summaries take little memory, and may wait long on a slow disk: no turn needed.
        p.load(false, move |_| Loaded::Info(f(&cancel)));
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

    /// The JSON tree, if that is what was loaded (for tests).
    #[cfg(test)]
    pub fn json(&mut self) -> Option<&mut crate::preview::jsonview::JsonView> {
        match &mut self.loaded {
            Some(Loaded::Json(j)) => Some(j),
            _ => None,
        }
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
            Some(Loaded::Geo(g)) => format!("geo {}", g.describe()),
            Some(Loaded::Json(j)) => format!("json {}", j.describe()),
            Some(Loaded::Document(d)) => format!("doc {} {:?} {:?} sections={} first={:?}", d.format, d.title, d.author, d.sections.len(), d.sections.first().map(|s| s.blocks.len())),
            Some(Loaded::Summary(s)) => format!("summary {} pic={} {:?}", s.format, s.picture.as_ref().map_or("no".to_string(), |p| format!("{}x{}", p.w, p.h)), s.rows),
            Some(Loaded::Info(s)) => format!("info {s}"),
            Some(Loaded::Error(e)) => format!("ERROR {e}"),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        use std::sync::mpsc::TryRecvError;
        let Some(rx) = &self.rx else { return };
        // Set before looking, so a result sent after this look wakes the UI.
        self.wake.get_or_init(|| ctx.clone());
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
            Some(Loaded::Geo(v)) => return v.ui(ui, &self.name, &self.key, theme),
            Some(Loaded::Json(v)) => return v.ui(ui, &self.name, &self.key, theme),
            _ => {}
        }
        let weak = theme.weak;
        let facts: Vec<(String, Color32)> = match &self.loaded {
            Some(Loaded::Image { orig, fmt, .. }) => {
                let frames = if self.frames.is_empty() { String::new() } else { format!(" · {} {}", self.frames.len() + 1, t("frames")) };
                vec![(format!("{}×{} {}{frames}", orig.0, orig.1, fmt), weak)]
            }
            Some(Loaded::Summary(s)) => vec![(s.format.clone(), weak)],
            Some(Loaded::Document(d)) => {
                let mut parts = vec![d.format.to_string()];
                parts.extend(d.title.clone());
                parts.extend(d.author.clone());
                vec![(parts.join(" · "), weak)]
            }
            Some(Loaded::Code { truncated, lang, lines, .. }) => {
                let mut v = vec![(format!("{lang} · {} {}", lines.len(), t("lines")), weak)];
                if *truncated {
                    v.push((t("(truncated)"), theme.marked));
                }
                v
            }
            Some(Loaded::Markdown { .. }) => vec![("Markdown".to_string(), weak)],
            Some(Loaded::Table { data, .. }) => {
                let mut parts = vec![data.format.to_string()];
                if let Some(sh) = data.sheets.get(self.sheet) {
                    parts.push(format!("{} {}", sh.columns.len(), t("columns")));
                    parts.push(match sh.total_rows {
                        Some(n) => format!("{} {}", crate::util::fmt_count(n), t("rows")),
                        None => format!("{}+ {}", crate::util::fmt_count(sh.rows.len() as u64), t("rows")),
                    });
                }
                parts.extend(data.info.iter().cloned());
                vec![(parts.join(" · "), weak)]
            }
            Some(Loaded::Hex { total, more, .. }) => {
                let size = match self.size {
                    Some(s) => human_size(s),
                    // Only the start was read, and the size is not known: at least this much.
                    None if *more => format!("≥ {}", human_size(*total as u64)),
                    None => human_size(*total as u64),
                };
                vec![(format!("{} · {size}", t("Binary")), weak)]
            }
            _ => vec![],
        };
        header_row(ui, theme, &self.name, &facts, |ui| match &self.loaded {
            Some(Loaded::Image { .. }) => {
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
                if matches!(&self.loaded, Some(Loaded::Image { details, .. }) if !details.is_empty()) && ui.selectable_label(self.show_source, t("Details")).clicked() {
                    self.show_source = !self.show_source;
                }
            }
            Some(Loaded::Code { .. }) => {
                ui.checkbox(&mut self.wrap, t("Wrap"));
            }
            _ => {
                let has_source = matches!(&self.loaded, Some(Loaded::Markdown { .. }) | Some(Loaded::Table { source: Some(_), .. }));
                if has_source && ui.selectable_label(self.show_source, t("Source")).clicked() {
                    self.show_source = !self.show_source;
                }
            }
        });
        ui.separator();
        let Some(loaded) = &self.loaded else {
            // Most files load at once: the spinner waits a moment, so they don't flash it.
            let waited = self.born.elapsed();
            if waited < SPINNER_DELAY {
                ui.ctx().request_repaint_after(SPINNER_DELAY - waited);
                return;
            }
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t("Loading…"));
            });
            return;
        };
        // Scroll positions belong to this file: another file starts at the top.
        let key = self.key.clone();
        ui.push_id(("preview-body", key), |ui| match loaded {
            // Rendered by their own views above.
            Loaded::Pdf(_) | Loaded::Notebook(_) | Loaded::Model(_) | Loaded::Geo(_) | Loaded::Json(_) => {}
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
            Loaded::Image { details, .. } if self.show_source && !details.is_empty() => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| rows_ui(ui, details, theme));
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
        });
    }
}

fn summary_ui(ui: &mut egui::Ui, s: &Summary, texture: Option<&TextureHandle>, theme: &Theme) {
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
        rows_ui(ui, &s.rows, theme);
    });
}

/// Facts as label and value rows, with headings and notes between them.
fn rows_ui(ui: &mut egui::Ui, rows: &[crate::preview::certview::Row], theme: &Theme) {
    use crate::preview::certview::Row;
    let label_w = 150.0;
    let value_w = (ui.available_width() - label_w - 12.0).max(120.0);
    for row in rows {
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
}

fn table_ui(
    ui: &mut egui::Ui,
    data: &crate::preview::sheet::TableData,
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
    let shown_info = if sheet.total_rows.is_some_and(|n| n as usize > sheet.rows.len()) || sheet.total_rows.is_none() && sheet.rows.len() >= crate::preview::sheet::MAX_ROWS {
        format!("{} {} {}", t("Showing the first"), crate::util::fmt_count(sheet.rows.len() as u64), t("rows"))
    } else {
        String::new()
    };
    if !needle.is_empty() || !shown_info.is_empty() {
        let mut s = shown_info;
        if !needle.is_empty() {
            if !s.is_empty() {
                s.push_str(" · ");
            }
            s.push_str(&format!("{} {}", crate::util::fmt_count(visible.len() as u64), t("matching rows")));
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
            ("a.geojson", Kind::Geo),
            ("a.json", Kind::Json),
            ("a.jsonc", Kind::Code),
            ("a.WKT", Kind::Geo),
            ("a.wkb", Kind::Geo),
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
            ("a.tga", Kind::Image),
            ("a.psd", Kind::Image),
            ("a.exr", Kind::Image),
            ("a.mp3", Kind::Audio),
            ("a.FLAC", Kind::Audio),
            ("a.m4a", Kind::Audio),
            ("a.avi", Kind::Video),
            ("a.docx", Kind::Document),
            ("a.pptx", Kind::Document),
            ("a.odp", Kind::Document),
            ("a.eml", Kind::Document),
            ("a.mbox", Kind::Document),
            ("a.pages", Kind::Package),
            ("a.key", Kind::Package),
            ("a.cbz", Kind::Package),
            ("a.cbr", Kind::Package),
            ("model.gguf", Kind::Table),
            ("a.pt", Kind::Table),
            ("pytorch_model-00001-of-00002.bin", Kind::Table),
            ("a.bin", Kind::Other),
            ("a.mat", Kind::Table),
            ("a.nc", Kind::Table),
            ("a.orc", Kind::Table),
            ("a.dta", Kind::Table),
            ("a.sav", Kind::Table),
            ("a.sas7bdat", Kind::Table),
            ("a.duckdb", Kind::Table),
            ("a.ics", Kind::Table),
            ("a.vcf", Kind::Table),
            ("a.srt", Kind::Table),
            ("a.gpx", Kind::Geo),
            ("a.kmz", Kind::Geo),
            ("a.shp", Kind::Geo),
            ("a.gpkg", Kind::Geo),
            ("a.ts", Kind::Code),
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
        for name in ["a.png", "a.pdf", "a.svg", "a.ipynb", "a.heic", "a.ttf", "a.pem", "a.rtf", "a.geojson", "a.wkb"] {
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
        assert!(too_large("a.stl", crate::preview::model3d::MAX_FILE + 1, None));
        // Read in place: a Photoshop file's layers are stepped over, a GeoPackage is queried.
        for name in ["a.psd", "a.gpkg", "a.mbox", "a.docx", "a.mp3", "a.cbz"] {
            assert!(!too_large(name, gb, None), "{name}");
            assert!(too_large(name, gb + 1, None), "{name}");
        }
        assert!(too_large("a.eml", 80 * mb + 1, None));
        // Only the header is read of these.
        for name in ["a.safetensors", "a.onnx", "a.npy", "a.npz", "a.gguf", "a.pt", "a.mat", "a.nc", "a.orc", "a.dta", "a.sav", "a.sas7bdat", "a.duckdb"] {
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
    decode_image(bytes, "")
}

/// Decode an image of a format known from its first bytes or, for formats without a
/// signature (TGA), from the extension `ext`.
fn decode_image(bytes: &[u8], ext: &str) -> Result<image::DynamicImage, String> {
    use image::ImageDecoder;
    let failed = |e: image::ImageError| format!("Cannot decode image: {e}");
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    if reader.format().is_none() {
        if let Some(f) = image::ImageFormat::from_extension(ext) {
            reader.set_format(f);
        }
    }
    let mut decoder = reader.into_decoder().map_err(failed)?;
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
    Loaded::Image { rgba: rgba.into_raw(), w, h, orig, premultiplied: false, fmt, more_frames: vec![], first_delay: 0.0, details: vec![] }
}

/// How much memory an animation's frames may take (they all become textures).
const MAX_ANIMATION_BYTES: usize = 256 * 1024 * 1024;
/// The decoder keeps canvases of the full size while it works, whatever it is told to keep.
const MAX_ANIMATION_CANVAS: u64 = 256 * 1024 * 1024;
/// Animations larger than this are shown scaled down.
const MAX_ANIMATION_SIDE: u32 = 1280;

/// An animated GIF or WebP, or `None` if it is a still image or its frames cannot be read.
fn decode_animation(ext: &str, bytes: &[u8], cancel: &crate::preview::media::Cancel) -> Option<Loaded> {
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
    Some(Loaded::Image { rgba: first.rgba, w: first.w, h: first.h, orig, premultiplied: false, fmt: ext.to_uppercase(), more_frames: out, first_delay: first.delay, details: vec![] })
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

fn field_row(label: String, value: Option<String>, rows: &mut Vec<crate::preview::certview::Row>) {
    if let Some(v) = value {
        rows.push(crate::preview::certview::Row::Field(label, v));
    }
}

fn video_summary(path: &Path, max_texture: usize, cancel: &crate::preview::media::Cancel, early: &dyn Fn(Loaded)) -> Loaded {
    use crate::i18n::t;
    use crate::preview::media::{video_info, video_thumbnail};
    use crate::util::format_duration;
    let info = match crate::util::open_regular(path) {
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
        rows.push(crate::preview::certview::Row::Note(t("The details of this video could not be read.")));
    }
    // The facts are ready now; a thumbnail can take a while.
    let format = ext_of(&path.to_string_lossy()).to_uppercase();
    early(Loaded::Summary(Summary { format: format.clone(), picture: None, rows: rows.clone(), picture_pending: true }));
    let picture = video_thumbnail(path, info.duration, cancel).and_then(|png| decode_oriented(&png).ok()).map(|img| summary_picture(img, max_texture));
    if picture.is_none() {
        rows.push(crate::preview::certview::Row::Note(if cfg!(target_os = "macos") { t("No thumbnail: macOS cannot read this video. Install FFmpeg to see one.") } else { t("No thumbnail: install FFmpeg to see one.") }));
    }
    Loaded::Summary(Summary { format, picture, rows, picture_pending: false })
}

/// The facts of a photo's EXIF data, as rows.
fn exif_fields(info: &crate::preview::rawphoto::RawInfo) -> Vec<crate::preview::certview::Row> {
    use crate::i18n::t;
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
    field_row(t("Location"), info.gps.map(crate::preview::rawphoto::format_gps), &mut rows);
    rows
}

/// The EXIF facts of a JPEG, TIFF, PNG or WebP image, if it has any.
fn image_details(bytes: &[u8], ext: &str) -> Vec<crate::preview::certview::Row> {
    use image::ImageDecoder;
    let info = if matches!(ext, "tif" | "tiff") {
        crate::preview::rawphoto::parse(&mut std::io::Cursor::new(bytes))
    } else if matches!(ext, "jpg" | "jpeg" | "png" | "webp") {
        let exif = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_decoder().ok())
            .and_then(|mut d| d.exif_metadata().ok().flatten());
        // The EXIF block is a small TIFF file of its own.
        exif.filter(|e| e.len() <= 4 * 1024 * 1024).and_then(|e| {
            let tiff = e.strip_prefix(b"Exif\0\0".as_slice()).unwrap_or(&e);
            crate::preview::rawphoto::parse(&mut std::io::Cursor::new(tiff))
        })
    } else {
        None
    };
    info.map(|i| exif_fields(&i)).unwrap_or_default()
}

fn is_psd(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "psd" | "psb")
}

/// A Photoshop document's merged picture.
fn psd_loaded<R: std::io::Read + std::io::Seek>(r: R, name: &str, max_texture: usize) -> Loaded {
    match crate::preview::psd::decode(r, MAX_DECODED_IMAGE) {
        Ok(img) => image_loaded(img, ext_of(name).to_uppercase(), max_texture),
        Err(e) => Loaded::Error(e),
    }
}

fn raw_summary<R: std::io::Read + std::io::Seek>(r: &mut R, fmt: String, max_texture: usize) -> Loaded {
    use crate::i18n::t;
    let Some(info) = crate::preview::rawphoto::parse(r) else {
        return Loaded::Error(t("Not a camera RAW file this preview can read."));
    };
    let mut rows = exif_fields(&info);
    field_row(t("Sensor"), info.sensor.map(|(w, h)| format!("{w} × {h}")), &mut rows);
    let picture = crate::preview::rawphoto::preview_image(r, &info).map(|img| summary_picture(img, max_texture));
    if picture.is_none() {
        rows.push(crate::preview::certview::Row::Note(t("This file has no preview image the app can show.")));
    }
    Loaded::Summary(Summary { format: format!("RAW · {fmt}"), picture, rows, picture_pending: false })
}

fn is_comic(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "cbz" | "cbr" | "cb7")
}

/// An iWork document or a comic book, from a file on disk.
fn package_summary(name: &str, path: &Path, max_texture: usize) -> Loaded {
    if is_comic(name) { comic_summary(name, path, max_texture) } else { iwork_summary(name, path, max_texture) }
}

/// Pages, Keynote and Numbers files keep a picture of their first page or slide.
fn iwork_summary(name: &str, path: &Path, max_texture: usize) -> Loaded {
    use crate::i18n::t;
    use std::io::Read;
    let format = match ext_of(name).as_str() {
        "pages" => "Pages",
        "key" => "Keynote",
        _ => "Numbers",
    };
    let no_picture = || Loaded::Error(t("This document has no preview picture."));
    let Ok(file) = crate::util::open_regular(path) else { return no_picture() };
    let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        // A `.key` that is not a Keynote file is a key: named, never shown.
        if format == "Keynote" {
            let mut head = Vec::new();
            if crate::util::open_regular(path).and_then(|f| f.take(1024 * 1024).read_to_end(&mut head)).is_ok() {
                return certificate_summary(&head);
            }
        }
        return no_picture();
    };
    // Newer files keep a full-size picture; older ones a Quick Look thumbnail.
    for candidate in ["preview.jpg", "QuickLook/Thumbnail.jpg", "preview-web.jpg", "preview-micro.jpg"] {
        let Ok(f) = zip.by_name(candidate) else { continue };
        let mut bytes = Vec::new();
        if f.size() > MAX_IMAGE as u64 || f.take(MAX_IMAGE as u64).read_to_end(&mut bytes).is_err() {
            continue;
        }
        if let Ok(img) = decode_oriented(&bytes) {
            let rows = vec![crate::preview::certview::Row::Note(t("This is the picture saved with the document; its text is not read."))];
            return Loaded::Summary(Summary { format: format.into(), picture: Some(summary_picture(img, max_texture)), rows, picture_pending: false });
        }
    }
    no_picture()
}

/// A comic book: how many pages, and the first one as its cover.
fn comic_summary(name: &str, path: &Path, max_texture: usize) -> Loaded {
    use crate::preview::certview::Row;
    use crate::i18n::t;
    let entries = match crate::files::archive::list(path) {
        Ok(e) => e,
        Err(e) => return Loaded::Error(e),
    };
    let is_page = |p: &str| {
        let file = p.rsplit('/').next().unwrap_or(p);
        !file.starts_with('.') && !p.starts_with("__MACOSX/") && matches!(ext_of(file).as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp")
    };
    let mut pages: Vec<&crate::files::archive::ArchEntry> = entries.iter().filter(|e| !e.is_dir && is_page(&e.path)).collect();
    pages.sort_by(|a, b| crate::util::natural_cmp(&a.path, &b.path));
    let picture = pages
        .first()
        .filter(|e| e.size <= MAX_IMAGE as u64)
        .and_then(|e| crate::files::archive::read_entry(path, &e.path, MAX_IMAGE).ok())
        .and_then(|b| decode_oriented(&b).ok())
        .map(|img| summary_picture(img, max_texture));
    let mut rows = vec![Row::Field(t("Pages"), crate::util::fmt_count(pages.len() as u64)), Row::Field(t("Size"), human_size(pages.iter().map(|e| e.size).sum()))];
    if picture.is_none() && !pages.is_empty() {
        rows.push(Row::Note(if crate::files::archive::lock_state(path) == crate::files::archive::Lock::Open { t("The cover could not be read.") } else { t("Password protected") }));
    }
    rows.push(Row::Note(t("Press Enter to browse inside the archive.")));
    Loaded::Summary(Summary { format: format!("{} · {}", t("Comic book"), ext_of(name).to_uppercase()), picture, rows, picture_pending: false })
}

fn audio_summary<R: std::io::Read + std::io::Seek>(r: R, name: &str, max_texture: usize) -> Loaded {
    match crate::preview::audio::read(r, &ext_of(name)) {
        Ok(a) => {
            let picture = a.cover.and_then(|c| decode_oriented(&c).ok()).map(|img| summary_picture(img, max_texture));
            Loaded::Summary(Summary { format: a.format, picture, rows: a.rows, picture_pending: false })
        }
        Err(e) => Loaded::Error(e),
    }
}

fn font_summary(name: &str, bytes: &[u8]) -> Loaded {
    match crate::preview::fontview::load(name, bytes) {
        Ok(f) => Loaded::Summary(Summary {
            format: f.format,
            picture: f.sheet.map(|(rgba, w, h)| SummaryPicture { rgba, w, h, tinted: true }),
            rows: f.rows.into_iter().map(|(l, v)| crate::preview::certview::Row::Field(l, v)).collect(),
            picture_pending: false,
        }),
        Err(e) => Loaded::Error(e),
    }
}

fn certificate_summary(bytes: &[u8]) -> Loaded {
    match crate::preview::certview::load(bytes) {
        Ok((format, rows)) => Loaded::Summary(Summary { format, picture: None, rows, picture_pending: false }),
        Err(e) => Loaded::Error(e),
    }
}

/// A HEIC, HEIF or AVIF image, from the system's tools.
fn heif_loaded(path: &Path, fmt: String, max_texture: usize, cancel: &crate::preview::media::Cancel) -> Loaded {
    match crate::preview::media::decode_heif(path, cancel).and_then(|png| decode_oriented(&png)) {
        Ok(img) => image_loaded(img, fmt, max_texture),
        Err(e) => Loaded::Error(e),
    }
}

/// Previews read from a file in place (see `Preview::reads_files_directly`).
fn decode_file(name: &str, path: &Path, syntax_theme: &str, max_texture: usize, cancel: &crate::preview::media::Cancel, early: &dyn Fn(Loaded)) -> Loaded {
    let fmt = ext_of(name).to_uppercase();
    match classify(name) {
        Kind::Image => match crate::util::open_regular(path) {
            Ok(f) => psd_loaded(std::io::BufReader::new(f), name, max_texture),
            Err(e) => Loaded::Error(e.to_string()),
        },
        Kind::Document => match crate::preview::textdocs::load_path(name, path, syntax_theme) {
            Ok(d) => Loaded::Document(d),
            Err(e) => Loaded::Error(e),
        },
        Kind::Heif => heif_loaded(path, fmt, max_texture, cancel),
        Kind::Video => video_summary(path, max_texture, cancel, early),
        Kind::Audio => match crate::util::open_regular(path) {
            Ok(f) => audio_summary(f, name, max_texture),
            Err(e) => Loaded::Error(e.to_string()),
        },
        Kind::Package => package_summary(name, path, max_texture),
        Kind::Geo => match crate::preview::geoview::geopackage(path) {
            Ok(data) => Loaded::Geo(Box::new(crate::preview::geoview::GeoView::new(data, None))),
            Err(e) => Loaded::Error(e),
        },
        Kind::Raw => match crate::util::open_regular(path) {
            Ok(f) => raw_summary(&mut std::io::BufReader::new(f), fmt, max_texture),
            Err(e) => Loaded::Error(e.to_string()),
        },
        _ => {
            // Parsing needs all of it in memory.
            if std::fs::metadata(path).is_ok_and(|m| m.len() > crate::preview::model3d::MAX_FILE) {
                return Loaded::Error(crate::i18n::t("This model is too large to preview."));
            }
            match crate::util::read_regular(path) {
                Ok(bytes) => match crate::preview::model3d::load(name, &bytes, path.parent()) {
                    Ok(mesh) => Loaded::Model(Box::new(crate::preview::model3d::ModelView::new(mesh))),
                    Err(e) => Loaded::Error(e),
                },
                Err(e) => Loaded::Error(e.to_string()),
            }
        }
    }
}

/// Run `f` on a temporary copy of a file that has no path of its own (in an archive or on a server).
fn with_temp_file(name: &str, bytes: &[u8], f: impl FnOnce(&Path) -> Loaded) -> Loaded {
    let dir = match crate::util::TempDir::new() {
        Ok(d) => d,
        Err(e) => return Loaded::Error(e),
    };
    let ext = ext_of(name);
    let path = dir.path().join(format!("preview.{ext}"));
    if let Err(e) = std::fs::File::create_new(&path).and_then(|mut f| std::io::Write::write_all(&mut f, bytes)) {
        return Loaded::Error(e.to_string());
    }
    f(&path)
}

fn decode(name: &str, bytes: Vec<u8>, syntax_theme: &str, max_texture: usize, cancel: &crate::preview::media::Cancel, early: &dyn Fn(Loaded)) -> Loaded {
    let kind = classify(name);
    let ext = ext_of(name);
    if cancel.load(Ordering::Relaxed) {
        // Nobody is waiting for this any more: leave the decoding undone.
        return Loaded::Error(String::new());
    }
    match kind {
        Kind::Image if is_psd(name) => psd_loaded(std::io::Cursor::new(bytes), name, max_texture),
        Kind::Image => {
            if let Some(animation) = decode_animation(&ext, &bytes, cancel) {
                return animation;
            }
            match decode_image(&bytes, &ext) {
                Ok(img) => match image_loaded(img, ext.to_uppercase(), max_texture) {
                    Loaded::Image { rgba, w, h, orig, premultiplied, fmt, more_frames, first_delay, .. } => {
                        Loaded::Image { rgba, w, h, orig, premultiplied, fmt, more_frames, first_delay, details: image_details(&bytes, &ext) }
                    }
                    other => other,
                },
                Err(e) => text_instead(e, &bytes, name, syntax_theme),
            }
        }
        Kind::Heif => with_temp_file(name, &bytes, |p| heif_loaded(p, ext.to_uppercase(), max_texture, cancel)),
        Kind::Video => with_temp_file(name, &bytes, |p| video_summary(p, max_texture, cancel, early)),
        Kind::Raw => raw_summary(&mut std::io::Cursor::new(bytes), ext.to_uppercase(), max_texture),
        Kind::Audio => audio_summary(std::io::Cursor::new(bytes), name, max_texture),
        Kind::Package => with_temp_file(name, &bytes, |p| package_summary(name, p, max_texture)),
        Kind::Document => match crate::preview::textdocs::load(name, bytes, syntax_theme) {
            Ok(d) => Loaded::Document(d),
            Err(e) => Loaded::Error(e),
        },
        Kind::Font => font_summary(name, &bytes),
        Kind::Certificate => certificate_summary(&bytes),
        Kind::Model => match crate::preview::model3d::load(name, &bytes, None) {
            Ok(mesh) => Loaded::Model(Box::new(crate::preview::model3d::ModelView::new(mesh))),
            Err(e) => Loaded::Error(e),
        },
        Kind::Geo => match crate::preview::geoview::load(name, &bytes) {
            Ok(data) => {
                // Text formats can show their source too.
                let source = looks_textual(&bytes).then(|| {
                    let head = &bytes[..bytes.len().min(512 * 1024)];
                    let (lines, bg, _, _) = highlight(&String::from_utf8_lossy(head), crate::preview::geoview::source_language(name), syntax_theme);
                    (lines, bg)
                });
                Loaded::Geo(Box::new(crate::preview::geoview::GeoView::new(data, source)))
            }
            Err(e) => Loaded::Error(e),
        },
        Kind::Json => {
            // Too large for a tree, or not quite JSON (comments, several values): as text.
            let parsed = (bytes.len() <= crate::preview::jsonview::MAX_JSON).then(|| crate::preview::jsonview::parse(&bytes).ok()).flatten();
            match parsed {
                Some(root) => {
                    let head = &bytes[..bytes.len().min(512 * 1024)];
                    let (lines, bg, _, _) = highlight(&String::from_utf8_lossy(head), "json", syntax_theme);
                    Loaded::Json(Box::new(crate::preview::jsonview::JsonView::new(root, Some((lines, bg)))))
                }
                None => text_preview(&bytes[..bytes.len().min(MAX_TEXT)], "json", name, syntax_theme),
            }
        }
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
            Loaded::Pdf(Box::new(crate::preview::docview::start_pdf(bytes, max_texture)))
        }
        Kind::Notebook => match crate::preview::docview::parse_notebook(&bytes, syntax_theme) {
            Ok(v) => Loaded::Notebook(Box::new(v)),
            Err(e) => Loaded::Error(e),
        },
        Kind::Table => match crate::preview::table::load_bytes(name, bytes.clone()) {
            Ok(data) => {
                let source = crate::preview::table::source_language(name).map(|lang| {
                    let head = &bytes[..bytes.len().min(512 * 1024)];
                    let (lines, bg, _, _) = highlight(&String::from_utf8_lossy(head), lang, syntax_theme);
                    (lines, bg)
                });
                Loaded::Table { data, source }
            }
            Err(e) => text_instead(e, &bytes, name, syntax_theme),
        },
        Kind::Code | Kind::Text => text_preview(&bytes, if kind == Kind::Code { &ext } else { "txt" }, name, syntax_theme),
        Kind::Other => {
            if looks_textual(&bytes) {
                text_preview(&bytes, &ext, name, syntax_theme)
            } else {
                let total = bytes.len();
                let mut b = bytes;
                b.truncate(16 * 1024);
                Loaded::Hex { bytes: b, total, more: false }
            }
        }
    }
}

/// A file that its format's reader refused, shown as text if it is text: extensions
/// are shared (`.nc` is also G-code, `.hdr` also an ENVI header).
fn text_instead(error: String, bytes: &[u8], name: &str, theme: &str) -> Loaded {
    if !bytes.is_empty() && looks_textual(bytes) {
        text_preview(&bytes[..bytes.len().min(MAX_TEXT)], "", name, theme)
    } else {
        Loaded::Error(error)
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
        details: vec![],
    }
}

// ---------------------------------------------------------------- markdown

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
    fn text_sniff() {
        assert!(looks_textual(b"hello\nworld"));
        assert!(!looks_textual(&[0, 1, 2, 3]));
    }
}
