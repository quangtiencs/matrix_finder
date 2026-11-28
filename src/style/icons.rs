//! Small vector icons painted with egui shapes, so they look crisp and
//! consistent on every platform (no dependency on emoji fonts).
use crate::files::vfs::Entry;
use crate::style::theme::Theme;
use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Image,
    Video,
    Audio,
    Archive,
    Code,
    /// Plain text, Markdown, logs.
    Text,
    /// Word processor documents and e-books.
    Doc,
    /// Spreadsheets and comma- or tab-separated tables.
    Sheet,
    /// Presentations.
    Slides,
    Pdf,
    Exec,
    /// Disk images.
    Disk,
    Font,
    Data,
    Other,
}

pub fn kind_of(ext: &str) -> FileKind {
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "tif" | "tiff" | "heic" | "heif" | "avif" | "psd" | "jxl" | "raw" | "cr2" | "cr3"
        | "nef" | "arw" | "dng" | "orf" | "rw2" | "raf" => FileKind::Image,
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv" | "m4v" | "mpg" | "mpeg" | "flv" | "3gp" => FileKind::Video,
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" | "aiff" | "aif" | "wma" | "mid" | "midi" => FileKind::Audio,
        "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "xz" | "txz" | "zst" | "tzst" | "bz2" | "tbz2" | "lz" | "lzma" | "jar" | "apk" | "cbz" | "cbr"
        | "whl" | "crate" | "gem" => FileKind::Archive,
        "rs" | "py" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "go" | "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" | "java" | "kt" | "kts" | "cs" | "rb"
        | "php" | "swift" | "lua" | "html" | "htm" | "css" | "scss" | "sass" | "less" | "vue" | "svelte" | "zig" | "dart" | "scala" | "sh" | "bash" | "zsh"
        | "fish" | "ps1" | "sql" | "hs" | "ex" | "exs" | "erl" | "clj" | "jl" | "r" | "m" | "mm" | "pl" | "nim" | "v" | "sv" | "vhdl" | "f90" | "asm" | "s"
        | "ipynb" | "makefile" | "cmake" | "gradle" | "nix" => FileKind::Code,
        "md" | "markdown" | "txt" | "rst" | "log" | "tex" | "org" | "adoc" | "nfo" | "srt" | "vtt" => FileKind::Text,
        "doc" | "docx" | "odt" | "rtf" | "pages" | "epub" | "mobi" | "djvu" => FileKind::Doc,
        "xls" | "xlsx" | "xlsm" | "xlsb" | "ods" | "numbers" | "csv" | "tsv" => FileKind::Sheet,
        // Not `.key`: a Keynote document is a folder on macOS; a `.key` file is a private key.
        "ppt" | "pptx" | "odp" => FileKind::Slides,
        "pdf" => FileKind::Pdf,
        "exe" | "msi" | "app" | "bat" | "cmd" | "appimage" | "deb" | "rpm" | "pkg" | "bin" | "run" | "com" | "flatpak" | "snap" => FileKind::Exec,
        "iso" | "dmg" | "img" | "vhd" | "vhdx" | "vmdk" | "qcow2" | "toast" => FileKind::Disk,
        "ttf" | "otf" | "woff" | "woff2" | "ttc" | "pfb" => FileKind::Font,
        "json" | "jsonl" | "ndjson" | "toml" | "yaml" | "yml" | "xml" | "parquet" | "pq" | "avro" | "orc" | "ini" | "cfg" | "conf" | "env" | "lock" | "db"
        | "sqlite" | "sqlite3" | "duckdb" | "geojson" | "geojsonl" | "wkt" | "wkb" | "shp" | "gpkg" | "kml" | "h5" | "hdf5" | "nc" | "npy" | "npz"
        | "safetensors" | "onnx" | "pt" | "pth" | "ckpt" | "gguf" | "pkl" | "feather" | "arrow" | "mat" | "dta" | "sav" | "rds" => FileKind::Data,
        _ => FileKind::Other,
    }
}

/// The short label a large icon shows for its file: the extension, up to 4 letters.
fn ext_label(ext: &str) -> Option<String> {
    let e = ext.trim_start_matches('.');
    (!e.is_empty() && e.chars().count() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric())).then(|| e.to_ascii_uppercase())
}

pub fn kind_color(k: FileKind, th: &Theme) -> Color32 {
    // Muted, harmonious hues (similar lightness) so the list stays calm.
    let c = match k {
        FileKind::Image => Color32::from_rgb(0x6f, 0xc2, 0x9b),
        FileKind::Video => Color32::from_rgb(0xe0, 0x84, 0x8e),
        FileKind::Audio => Color32::from_rgb(0xd0, 0x8c, 0xc8),
        FileKind::Archive => th.archive,
        FileKind::Code => Color32::from_rgb(0x8c, 0xaa, 0xee),
        FileKind::Text => Color32::from_rgb(0xa8, 0xb0, 0xbe),
        FileKind::Doc => Color32::from_rgb(0x6f, 0x9c, 0xf0),
        FileKind::Sheet => Color32::from_rgb(0x5c, 0xbf, 0x7e),
        FileKind::Slides => Color32::from_rgb(0xf0, 0x9a, 0x5e),
        FileKind::Pdf => Color32::from_rgb(0xe8, 0x6a, 0x62),
        FileKind::Exec => Color32::from_rgb(0xa3, 0xa9, 0xb8),
        FileKind::Disk => Color32::from_rgb(0x9a, 0xa6, 0xc4),
        FileKind::Font => Color32::from_rgb(0xb8, 0x9c, 0xe8),
        FileKind::Data => Color32::from_rgb(0x6c, 0xc0, 0xcc),
        FileKind::Other => th.weak,
    };
    if th.dark {
        c
    } else {
        c.lerp_to_gamma(Color32::BLACK, 0.22)
    }
}

/// The colour of a kind's symbol and label on white paper: as `kind_color` on a light
/// theme, whatever the theme (the paper is white in both).
fn ink(k: FileKind, th: &Theme) -> Color32 {
    let c = kind_color(k, th);
    let c = if th.dark { c.lerp_to_gamma(Color32::BLACK, 0.22) } else { c };
    if k == FileKind::Other {
        Color32::from_rgb(0x8a, 0x8f, 0x99)
    } else {
        c
    }
}

/// Folder: a tab and a body.
pub fn folder(p: &Painter, r: Rect, color: Color32) {
    let w = r.width();
    let tab = Rect::from_min_size(r.min + vec2(w * 0.06, w * 0.16), vec2(w * 0.42, w * 0.2));
    p.rect_filled(tab, w * 0.08, color.gamma_multiply(0.8));
    let body = Rect::from_min_max(r.min + vec2(w * 0.06, w * 0.26), r.max - vec2(w * 0.06, w * 0.12));
    p.rect_filled(body, w * 0.1, color.gamma_multiply(0.8));
    let front = Rect::from_min_max(body.min + vec2(0.0, w * 0.12), body.max);
    p.rect_filled(front, w * 0.1, color);
}

/// Document page with a folded corner (the Matrix Finder style: coloured paper). A large
/// one shows its kind's symbol and its extension.
fn page_matrix(p: &Painter, r: Rect, color: Color32, th: &Theme, kind: FileKind, ext: &str) {
    let w = r.width();
    if w < 28.0 {
        return page(p, r, color, th);
    }
    let x0 = r.left() + w * 0.16;
    let x1 = r.right() - w * 0.16;
    let y0 = r.top() + w * 0.04;
    let y1 = r.bottom() - w * 0.04;
    let fold = w * 0.24;
    let body = vec![pos2(x0, y0), pos2(x1 - fold, y0), pos2(x1, y0 + fold), pos2(x1, y1), pos2(x0, y1)];
    p.add(Shape::convex_polygon(body, color, Stroke::NONE));
    let corner = vec![pos2(x1 - fold, y0), pos2(x1 - fold, y0 + fold), pos2(x1, y0 + fold)];
    p.add(Shape::convex_polygon(corner, Color32::from_white_alpha(if th.dark { 110 } else { 150 }), Stroke::NONE));
    let on = Color32::from_white_alpha(235);
    let label = if kind == FileKind::Pdf { None } else { ext_label(ext) };
    let has_label = label.is_some();
    let g = Rect::from_center_size(pos2((x0 + x1) / 2.0, r.top() + w * if has_label { 0.47 } else { 0.56 }), vec2(w * 0.38, w * 0.34));
    kind_glyph(p, g, kind, on);
    if let Some(l) = label {
        let size = (w * 0.15).clamp(7.0, 13.0);
        p.text(pos2((x0 + x1) / 2.0, y1 - w * 0.13), egui::Align2::CENTER_CENTER, l, egui::FontId::new(size, egui::FontFamily::Name("bold".into())), on);
    }
}

/// Document page with a folded corner.
pub fn page(p: &Painter, r: Rect, color: Color32, th: &Theme) {
    let w = r.width();
    let x0 = r.left() + w * 0.18;
    let x1 = r.right() - w * 0.18;
    let y0 = r.top() + w * 0.06;
    let y1 = r.bottom() - w * 0.06;
    let fold = w * 0.26;
    let pts = vec![pos2(x0, y0), pos2(x1 - fold, y0), pos2(x1, y0 + fold), pos2(x1, y1), pos2(x0, y1)];
    p.add(Shape::convex_polygon(pts, color.gamma_multiply(0.9), Stroke::NONE));
    let corner = vec![pos2(x1 - fold, y0), pos2(x1 - fold, y0 + fold), pos2(x1, y0 + fold)];
    let light = if th.dark { Color32::from_white_alpha(110) } else { Color32::from_white_alpha(150) };
    p.add(Shape::convex_polygon(corner, light, Stroke::NONE));
    // Two subtle "text" lines.
    let line = if th.dark { Color32::from_black_alpha(70) } else { Color32::from_white_alpha(140) };
    for (i, frac) in [0.55f32, 0.72].iter().enumerate() {
        let y = r.top() + w * frac;
        let xe = if i == 0 { x1 - w * 0.12 } else { x1 - w * 0.25 };
        p.line_segment([pos2(x0 + w * 0.12, y), pos2(xe, y)], Stroke::new((w * 0.07).max(1.0), line));
    }
}

pub fn parent(p: &Painter, r: Rect, color: Color32) {
    let c = r.center();
    let w = r.width();
    let s = Stroke::new((w * 0.11).max(1.2), color);
    let top = pos2(c.x - w * 0.12, r.top() + w * 0.22);
    p.line_segment([top, pos2(top.x, r.bottom() - w * 0.25)], s);
    p.line_segment([pos2(top.x, r.bottom() - w * 0.25), pos2(r.right() - w * 0.2, r.bottom() - w * 0.25)], s);
    p.line_segment([top, top + vec2(-w * 0.2, w * 0.2)], s);
    p.line_segment([top, top + vec2(w * 0.2, w * 0.2)], s);
}

/// Small arrow badge marking a symbolic link.
fn link_badge(p: &Painter, r: Rect, th: &Theme) {
    let w = r.width();
    let b = Rect::from_min_size(pos2(r.left(), r.bottom() - w * 0.45), vec2(w * 0.45, w * 0.45));
    p.circle_filled(b.center(), w * 0.24, th.bg);
    let s = Stroke::new((w * 0.08).max(1.0), th.text);
    p.line_segment([b.left_bottom() + vec2(w * 0.1, -w * 0.1), b.right_top() + vec2(-w * 0.1, w * 0.1)], s);
}

// ---------------------------------------------------------------- icon packs

/// Visual style of file and folder icons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconPack {
    Matrix,
    Adwaita,
    Breeze,
    MacOS,
    Windows,
    /// Material Icon Theme (bundled SVGs, `iconsets`).
    Material,
    /// Fluent UI System Icons (bundled SVGs, `iconsets`).
    Fluent,
}

impl IconPack {
    pub const ALL: [IconPack; 7] =
        [IconPack::Matrix, IconPack::Adwaita, IconPack::Breeze, IconPack::MacOS, IconPack::Windows, IconPack::Material, IconPack::Fluent];

    /// The name a saved theme uses for this style.
    pub fn key(self) -> &'static str {
        match self {
            IconPack::Matrix => "matrix",
            IconPack::Adwaita => "adwaita",
            IconPack::Breeze => "breeze",
            IconPack::MacOS => "macos",
            IconPack::Windows => "windows",
            IconPack::Material => "material",
            IconPack::Fluent => "fluent",
        }
    }

    pub fn from_key(key: &str) -> Option<IconPack> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            IconPack::Matrix => "Matrix Finder",
            IconPack::Adwaita => "Adwaita (GNOME)",
            IconPack::Breeze => "Breeze (KDE Plasma)",
            IconPack::MacOS => "macOS",
            IconPack::Windows => "Windows 11",
            IconPack::Material => "Material Icon Theme",
            IconPack::Fluent => "Fluent UI",
        }
    }
}

/// The active icon set: a drawn style plus, optionally, a real desktop icon theme.
#[derive(Clone)]
pub struct IconSet {
    pub pack: IconPack,
    pub system: Option<Arc<crate::style::sysicons::IconTheme>>,
}

static CURRENT: LazyLock<RwLock<IconSet>> = LazyLock::new(|| RwLock::new(IconSet { pack: IconPack::Matrix, system: None }));
type TexCache = HashMap<(PathBuf, u32), Option<egui::TextureHandle>>;
static TEXTURES: LazyLock<Mutex<TexCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static LOOKUPS: LazyLock<Mutex<HashMap<(String, Vec<String>), Option<PathBuf>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_icons(set: IconSet) {
    if let Ok(mut c) = CURRENT.write() {
        *c = set;
    }
}

pub fn current() -> IconSet {
    CURRENT.read().map(|c| c.clone()).unwrap_or(IconSet { pack: IconPack::Matrix, system: None })
}

fn lookup_cached(theme: &crate::style::sysicons::IconTheme, names: &[String]) -> Option<PathBuf> {
    let key = (theme.name.clone(), names.to_vec());
    if let Some(v) = LOOKUPS.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return v;
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let found = theme.lookup(&refs);
    if let Ok(mut m) = LOOKUPS.lock() {
        m.insert(key, found.clone());
    }
    found
}

/// Draw an icon file (SVG/PNG) from a desktop theme. Returns false if it cannot be loaded.
fn draw_theme_icon(p: &Painter, r: Rect, path: &Path, th: &Theme) -> bool {
    let px = (r.width() * p.ctx().pixels_per_point()).round().max(8.0) as u32;
    let key = (path.to_path_buf(), px);
    let symbolic = path.to_string_lossy().contains("-symbolic");
    let tex = {
        let mut cache = match TEXTURES.lock() {
            Ok(c) => c,
            Err(_) => return false,
        };
        cache
            .entry(key)
            .or_insert_with(|| {
                let mut rgba = load_icon_rgba(path, px)?;
                if symbolic {
                    // Symbolic icons are monochrome masks: keep alpha, tint at draw time.
                    for c in rgba.chunks_exact_mut(4) {
                        c[0] = 255;
                        c[1] = 255;
                        c[2] = 255;
                    }
                }
                let img = egui::ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &rgba);
                Some(p.ctx().load_texture(format!("icon:{}:{px}", path.display()), img, egui::TextureOptions::LINEAR))
            })
            .clone()
    };
    let Some(tex) = tex else { return false };
    let tint = if symbolic { th.text } else { Color32::WHITE };
    p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
    true
}

fn load_icon_rgba(path: &Path, px: u32) -> Option<Vec<u8>> {
    let is_svg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    if is_svg {
        let data = std::fs::read(path).ok()?;
        return crate::style::logo::render(&data, px);
    }
    let img = image::open(path).ok()?;
    let img = img.resize_exact(px, px, image::imageops::FilterType::Lanczos3);
    Some(img.to_rgba8().into_raw())
}

pub fn entry_icon(p: &Painter, r: Rect, e: &Entry, th: &Theme) {
    if e.is_parent {
        parent(p, r, th.weak);
        return;
    }
    file_icon_with(&current(), p, r, &e.name, e.is_dir, &e.ext, e.is_link, th);
}

/// Icon for a file or folder name, using the active icon set.
pub fn file_icon(p: &Painter, r: Rect, name: &str, is_dir: bool, ext: &str, th: &Theme) {
    file_icon_with(&current(), p, r, name, is_dir, ext, false, th);
}

#[allow(clippy::too_many_arguments)]
pub fn file_icon_with(set: &IconSet, p: &Painter, r: Rect, name: &str, is_dir: bool, ext: &str, is_link: bool, th: &Theme) {
    let mut done = false;
    if let Some(sys) = &set.system {
        let names = crate::style::sysicons::names_for(name, is_dir, ext);
        if let Some(path) = lookup_cached(sys, &names) {
            done = draw_theme_icon(p, r, &path, th);
        }
    }
    if !done {
        let bundled = match set.pack {
            IconPack::Material => Some(crate::style::iconsets::Set::Material),
            IconPack::Fluent => Some(crate::style::iconsets::Set::Fluent),
            _ => None,
        };
        done = bundled.is_some_and(|b| crate::style::iconsets::draw(b, p, r, name, is_dir, ext, th));
    }
    if !done {
        if is_dir {
            folder_styled(p, r, set.pack, th);
        } else {
            let k = kind_of(ext);
            page_styled(p, r, kind_color(k, th), set.pack, th, k, ext);
        }
    }
    if is_link {
        link_badge(p, r, th);
    }
}

/// A plain folder in the active style (sidebar, tabs).
pub fn folder_icon(p: &Painter, r: Rect, th: &Theme) {
    file_icon(p, r, "", true, "", th);
}

pub fn drive_icon(p: &Painter, r: Rect, th: &Theme) {
    let set = current();
    if let Some(sys) = &set.system {
        if let Some(path) = lookup_cached(sys, &["drive-harddisk".into(), "drive-harddisk-system".into()]) {
            if draw_theme_icon(p, r, &path, th) {
                return;
            }
        }
    }
    drive(p, r, th.weak);
}

fn folder_styled(p: &Painter, r: Rect, pack: IconPack, th: &Theme) {
    let w = r.width();
    if pack == IconPack::Breeze {
        return folder_breeze(p, r);
    }
    let rgb = Color32::from_rgb;
    // Back (with its tab), front, a light line along the front's top edge.
    let (back, front, highlight) = match pack {
        IconPack::Matrix | IconPack::Material | IconPack::Fluent => {
            let d = th.dir;
            (d.lerp_to_gamma(Color32::BLACK, 0.22), d, Some(d.lerp_to_gamma(Color32::WHITE, 0.35)))
        }
        IconPack::Adwaita => (rgb(0x35, 0x84, 0xe4), rgb(0x62, 0xa0, 0xea), Some(rgb(0x99, 0xc1, 0xf1))),
        IconPack::Breeze => (rgb(0x14, 0x7e, 0xc9), rgb(0x3d, 0xae, 0xe9), Some(rgb(0x6f, 0xc5, 0xf2))),
        IconPack::MacOS => (rgb(0x4a, 0x9c, 0xe0), rgb(0x76, 0xbd, 0xf5), Some(rgb(0xa9, 0xd7, 0xfb))),
        IconPack::Windows => (rgb(0xe0, 0x9a, 0x12), rgb(0xff, 0xc8, 0x3d), Some(rgb(0xff, 0xe0, 0x8a))),
    };
    let round = match pack {
        IconPack::Breeze => 0.03,
        IconPack::Windows => 0.07,
        IconPack::MacOS => 0.1,
        _ => 0.09,
    };
    let rr = (w * round).max(1.0);
    let tab_w = if pack == IconPack::MacOS { 0.34 } else { 0.4 };
    let tab = Rect::from_min_size(r.min + vec2(w * 0.06, w * 0.13), vec2(w * tab_w, w * 0.2));
    p.rect_filled(tab, rr, back);
    let body = Rect::from_min_max(r.min + vec2(w * 0.06, w * 0.2), r.max - vec2(w * 0.06, w * 0.12));
    p.rect_filled(body, rr, back);
    if pack == IconPack::Windows && w >= 20.0 {
        // A sheet of paper sticking out of the folder.
        let sheet = Rect::from_min_max(body.min + vec2(w * 0.08, w * 0.06), pos2(body.right() - w * 0.08, body.bottom()));
        p.rect_filled(sheet, rr, Color32::from_rgb(0xfa, 0xfa, 0xfa));
    }
    let front_r = Rect::from_min_max(body.min + vec2(0.0, w * 0.13), body.max);
    p.rect_filled(front_r, rr, front);
    if let Some(h) = highlight {
        let line = Rect::from_min_size(front_r.min + vec2(rr, 0.0), vec2(front_r.width() - 2.0 * rr, (w * 0.05).max(1.0)));
        p.rect_filled(line, 0.0, h);
    }
}

/// Breeze's folder: flat blue, square-ish corners, no highlight.
fn folder_breeze(p: &Painter, r: Rect) {
    let w = r.width();
    let (back, front) = (Color32::from_rgb(0x1d, 0x99, 0xf3), Color32::from_rgb(0x3d, 0xae, 0xe9));
    let rr = w * 0.05;
    p.rect_filled(Rect::from_min_size(r.min + vec2(w * 0.06, w * 0.14), vec2(w * 0.4, w * 0.22)), rr, back);
    let body = Rect::from_min_max(r.min + vec2(w * 0.06, w * 0.24), r.max - vec2(w * 0.06, w * 0.12));
    p.rect_filled(body, rr, back);
    p.rect_filled(Rect::from_min_max(body.min + vec2(0.0, w * 0.13), body.max), rr, front);
}

/// Breeze's document: square paper, the fold in the kind's colour, the kind's symbol in
/// that colour in the middle.
fn page_breeze(p: &Painter, r: Rect, color: Color32, th: &Theme, kind: FileKind) {
    let w = r.width();
    let (x0, x1) = (r.left() + w * 0.13, r.right() - w * 0.13);
    let (y0, y1) = (r.top() + w * 0.02, r.bottom() - w * 0.02);
    let fold = w * 0.27;
    let (paper, edge) = if th.dark {
        (Color32::from_rgb(0xf2, 0xf2, 0xf2), Color32::from_rgb(0x8a, 0x8a, 0x8a))
    } else {
        (Color32::WHITE, Color32::from_rgb(0xb4, 0xb4, 0xba))
    };
    let pts = vec![pos2(x0, y0), pos2(x1 - fold, y0), pos2(x1, y0 + fold), pos2(x1, y1), pos2(x0, y1)];
    p.add(Shape::convex_polygon(pts, paper, Stroke::new(1.0_f32, edge)));
    let corner = vec![pos2(x1 - fold, y0), pos2(x1 - fold, y0 + fold), pos2(x1, y0 + fold)];
    p.add(Shape::convex_polygon(corner, color, Stroke::new(1.0_f32, edge)));
    let g = Rect::from_center_size(pos2((x0 + x1) / 2.0, r.top() + w * 0.62), vec2(w * 0.46, w * 0.42));
    kind_glyph(p, g, kind, color);
}

/// The paper of a document in the desktop styles: its outline with a folded corner (the
/// other three corners rounded by `round`), and the fold.
fn paper_shape(r: Rect, fold: f32, round: f32) -> (Vec<Pos2>, Vec<Pos2>) {
    let arc = |c: Pos2, from: f32, pts: &mut Vec<Pos2>| {
        for k in 0..=4 {
            let a = from + std::f32::consts::FRAC_PI_2 * k as f32 / 4.0;
            pts.push(c + vec2(a.cos(), a.sin()) * round);
        }
    };
    let q = std::f32::consts::FRAC_PI_2;
    let mut pts = Vec::with_capacity(18);
    // Clockwise from the top left (y grows downwards).
    if round > 0.5 {
        arc(pos2(r.left() + round, r.top() + round), 2.0 * q, &mut pts);
    } else {
        pts.push(r.left_top());
    }
    pts.push(pos2(r.right() - fold, r.top()));
    pts.push(pos2(r.right(), r.top() + fold));
    if round > 0.5 {
        arc(pos2(r.right() - round, r.bottom() - round), 0.0, &mut pts);
        arc(pos2(r.left() + round, r.bottom() - round), q, &mut pts);
    } else {
        pts.push(r.right_bottom());
        pts.push(r.left_bottom());
    }
    let corner = vec![pos2(r.right() - fold, r.top()), pos2(r.right() - fold, r.top() + fold), pos2(r.right(), r.top() + fold)];
    (pts, corner)
}

fn page_styled(p: &Painter, r: Rect, color: Color32, pack: IconPack, th: &Theme, kind: FileKind, ext: &str) {
    if matches!(pack, IconPack::Matrix | IconPack::Material | IconPack::Fluent) {
        return page_matrix(p, r, color, th, kind, ext);
    }
    if pack == IconPack::Breeze {
        return page_breeze(p, r, color, th, kind);
    }
    let w = r.width();
    let large = w >= 28.0;
    let ink = ink(kind, th);
    let paper_c = if th.dark { Color32::from_rgb(0xf4, 0xf4, 0xf6) } else { Color32::WHITE };
    let edge = if th.dark { Color32::from_rgb(0x9a, 0x9a, 0xa2) } else { Color32::from_rgb(0xb8, 0xb8, 0xc0) };
    let paper = Rect::from_min_max(pos2(r.left() + w * 0.15, r.top() + w * 0.03), pos2(r.right() - w * 0.15, r.bottom() - w * 0.03));
    let fold = w * 0.24;
    let line = Stroke::new(if large { 1.0_f32 } else { 0.8 }, edge);
    // The PDF symbol says it already.
    let label = if large && kind != FileKind::Pdf { ext_label(ext) } else { None };
    let bold = |size: f32| egui::FontId::new(size, egui::FontFamily::Name("bold".into()));
    let round = match pack {
        IconPack::Adwaita => w * 0.07,
        IconPack::MacOS => w * 0.04,
        IconPack::Windows => w * 0.03,
        _ => 0.0,
    };
    let (pts, corner) = paper_shape(paper, fold, round);
    match pack {
        IconPack::Adwaita => {
            // Rounded paper, a big symbol in the kind's colour, the extension under it.
            p.add(Shape::convex_polygon(pts, paper_c, line));
            p.add(Shape::convex_polygon(corner, Color32::from_rgb(0xde, 0xdd, 0xda), line));
            let g = Rect::from_center_size(pos2(paper.center().x, r.top() + w * if label.is_some() { 0.46 } else { 0.56 }), vec2(w * 0.4, w * 0.36));
            kind_glyph(p, g, kind, ink);
            if let Some(l) = label {
                p.text(pos2(paper.center().x, paper.bottom() - w * 0.13), egui::Align2::CENTER_CENTER, l, bold((w * 0.14).clamp(7.0, 12.0)), ink);
            }
        }
        IconPack::MacOS => {
            // White paper with a soft fold; the extension in grey capitals at the bottom.
            p.add(Shape::convex_polygon(pts, paper_c, line));
            p.add(Shape::convex_polygon(corner, Color32::from_rgb(0xe4, 0xe4, 0xe8), line));
            let g = Rect::from_center_size(pos2(paper.center().x, r.top() + w * if label.is_some() { 0.47 } else { 0.56 }), vec2(w * 0.42, w * 0.38));
            kind_glyph(p, g, kind, ink);
            if let Some(l) = label {
                p.text(
                    pos2(paper.center().x, paper.bottom() - w * 0.12),
                    egui::Align2::CENTER_CENTER,
                    l,
                    bold((w * 0.13).clamp(6.5, 11.0)),
                    Color32::from_rgb(0x8e, 0x8e, 0x93),
                );
            }
        }
        IconPack::Windows | IconPack::Matrix | IconPack::Breeze | IconPack::Material | IconPack::Fluent => {
            // White paper; a tile in the kind's colour at the lower left, its symbol white.
            p.add(Shape::convex_polygon(pts, paper_c, line));
            p.add(Shape::convex_polygon(corner, Color32::from_rgb(0xe6, 0xe6, 0xe6), line));
            let tile = Rect::from_min_size(pos2(r.left() + w * 0.06, r.bottom() - w * 0.5), vec2(w * 0.46, w * 0.46));
            p.rect_filled(tile, w * 0.07, ink);
            if large {
                kind_glyph(p, tile.shrink(w * 0.1), kind, Color32::WHITE);
                for k in 0..3 {
                    let y = paper.top() + w * (0.22 + 0.11 * k as f32);
                    p.line_segment(
                        [pos2(paper.left() + w * 0.12, y), pos2(paper.right() - w * if k == 0 { 0.3 } else { 0.1 }, y)],
                        Stroke::new(w * 0.035, edge),
                    );
                }
            }
        }
    }
}

/// Small type symbol drawn on a document (image, note, play, </>, zipper…).
fn kind_glyph(p: &Painter, g: Rect, kind: FileKind, c: Color32) {
    let w = g.width();
    let h = g.height();
    let s = Stroke::new((w * 0.13).max(1.0), c);
    let thin = Stroke::new((w * 0.08).max(1.0), c);
    let bold = |size: f32| egui::FontId::new(size, egui::FontFamily::Name("bold".into()));
    match kind {
        FileKind::Image => {
            let base = g.bottom();
            p.add(Shape::convex_polygon(
                vec![pos2(g.left(), base), pos2(g.left() + w * 0.38, g.top() + h * 0.3), pos2(g.left() + w * 0.7, base)],
                c,
                Stroke::NONE,
            ));
            p.add(Shape::convex_polygon(
                vec![pos2(g.left() + w * 0.45, base), pos2(g.left() + w * 0.72, g.top() + h * 0.5), pos2(g.right(), base)],
                c.gamma_multiply(0.75),
                Stroke::NONE,
            ));
            p.circle_filled(pos2(g.right() - w * 0.18, g.top() + h * 0.14), w * 0.12, c);
        }
        FileKind::Video => {
            // A play mark in a rounded frame.
            p.rect_stroke(g.shrink(w * 0.02), w * 0.12, thin, egui::StrokeKind::Inside);
            let cy = g.center().y;
            let pts = vec![pos2(g.left() + w * 0.38, cy - h * 0.24), pos2(g.left() + w * 0.72, cy), pos2(g.left() + w * 0.38, cy + h * 0.24)];
            p.add(Shape::convex_polygon(pts, c, Stroke::NONE));
        }
        FileKind::Audio => {
            let head = pos2(g.left() + w * 0.3, g.bottom() - h * 0.16);
            p.circle_filled(head, w * 0.17, c);
            p.line_segment([pos2(head.x + w * 0.15, head.y), pos2(head.x + w * 0.15, g.top())], s);
            p.line_segment([pos2(head.x + w * 0.15, g.top()), pos2(g.right() - w * 0.05, g.top() + h * 0.2)], s);
        }
        FileKind::Archive => {
            // A zipper: teeth left and right of a line, and its pull at the bottom.
            let x = g.center().x;
            let tooth = vec2(w * 0.24, h * 0.13);
            for i in 0..4 {
                let y = g.top() + i as f32 * h * 0.14;
                let min = if i % 2 == 0 { pos2(x - tooth.x, y) } else { pos2(x, y) };
                p.rect_filled(Rect::from_min_size(min, tooth), w * 0.02, c);
            }
            let pull = Rect::from_min_max(pos2(x - w * 0.2, g.top() + h * 0.58), pos2(x + w * 0.2, g.bottom()));
            p.rect_filled(pull, w * 0.07, c);
            let hole = if c == Color32::WHITE { Color32::from_black_alpha(90) } else { Color32::from_white_alpha(200) };
            p.rect_filled(Rect::from_center_size(pull.center() + vec2(0.0, h * 0.03), vec2(w * 0.14, h * 0.16)), w * 0.02, hole);
        }
        FileKind::Code => {
            let cy = g.center().y;
            let k = h * 0.32;
            p.add(Shape::line(vec![pos2(g.left() + w * 0.28, cy - k), pos2(g.left(), cy), pos2(g.left() + w * 0.28, cy + k)], s));
            p.add(Shape::line(vec![pos2(g.right() - w * 0.28, cy - k), pos2(g.right(), cy), pos2(g.right() - w * 0.28, cy + k)], s));
            p.line_segment([pos2(g.center().x + w * 0.08, cy - k), pos2(g.center().x - w * 0.08, cy + k)], s);
        }
        FileKind::Sheet => {
            // A table: a frame, a header row and columns.
            let t = g.shrink(w * 0.04);
            p.rect_filled(Rect::from_min_size(t.min, vec2(t.width(), t.height() * 0.28)), w * 0.06, c);
            p.rect_stroke(t, w * 0.06, thin, egui::StrokeKind::Inside);
            for k in 1..3 {
                let y = t.top() + t.height() * (0.28 + 0.24 * k as f32);
                p.line_segment([pos2(t.left(), y), pos2(t.right(), y)], thin);
            }
            p.line_segment([pos2(t.left() + t.width() * 0.4, t.top()), pos2(t.left() + t.width() * 0.4, t.bottom())], thin);
        }
        FileKind::Slides => {
            // A slide with a little bar chart on it.
            let t = Rect::from_min_size(g.min + vec2(0.0, h * 0.08), vec2(w, h * 0.7));
            p.rect_stroke(t, w * 0.06, thin, egui::StrokeKind::Inside);
            for (k, frac) in [0.45f32, 0.75, 0.6].iter().enumerate() {
                let bw = t.width() * 0.16;
                let x = t.left() + t.width() * (0.2 + 0.24 * k as f32);
                let top = t.bottom() - t.height() * 0.14 - (t.height() * 0.62) * frac;
                p.rect_filled(Rect::from_min_max(pos2(x, top), pos2(x + bw, t.bottom() - t.height() * 0.14)), 0.0, c);
            }
            p.line_segment([pos2(g.center().x, t.bottom()), pos2(g.center().x, g.bottom())], thin);
        }
        FileKind::Pdf => {
            // "PDF" in heavy letters: white on a band of `c`, or in `c` itself when that is
            // white (on a coloured tile).
            let band = Rect::from_center_size(g.center(), vec2(w * 1.05, h * 0.52));
            if c == Color32::WHITE {
                p.text(band.center(), egui::Align2::CENTER_CENTER, "PDF", bold(band.height() * 0.8), c);
            } else {
                p.rect_filled(band, w * 0.08, c);
                p.text(band.center(), egui::Align2::CENTER_CENTER, "PDF", bold(band.height() * 0.72), Color32::WHITE);
            }
        }
        FileKind::Exec => {
            let cy = g.center().y;
            p.add(Shape::line(vec![pos2(g.left(), cy - h * 0.3), pos2(g.left() + w * 0.35, cy), pos2(g.left(), cy + h * 0.3)], s));
            p.line_segment([pos2(g.left() + w * 0.5, cy + h * 0.3), pos2(g.right(), cy + h * 0.3)], s);
        }
        FileKind::Disk => {
            let c0 = g.center();
            let rad = w.min(h) * 0.5;
            p.circle_stroke(c0, rad - thin.width / 2.0, thin);
            p.circle_stroke(c0, rad * 0.62, Stroke::new(thin.width * 0.6, c.gamma_multiply(0.6)));
            p.circle_filled(c0, rad * 0.2, c);
        }
        FileKind::Font => {
            p.text(g.center(), egui::Align2::CENTER_CENTER, "Aa", egui::FontId::new(h * 0.95, egui::FontFamily::Name("bold".into())), c);
        }
        FileKind::Data => {
            p.text(g.center(), egui::Align2::CENTER_CENTER, "{ }", bold(h * 0.95), c);
        }
        FileKind::Text | FileKind::Doc | FileKind::Other => {
            // Lines of text; a document has a heading.
            let n = 4;
            for i in 0..n {
                let y = g.top() + h * (0.08 + 0.28 * i as f32);
                let x_end = if i == n - 1 { g.left() + w * 0.55 } else { g.right() };
                let width = if kind == FileKind::Doc && i == 0 { (w * 0.17).max(1.0) } else { (w * 0.1).max(1.0) };
                p.line_segment([pos2(g.left(), y), pos2(x_end, y)], Stroke::new(width, c));
            }
        }
    }
}

pub fn drive(p: &Painter, r: Rect, color: Color32) {
    let w = r.width();
    let body = Rect::from_min_max(r.min + vec2(w * 0.08, w * 0.3), r.max - vec2(w * 0.08, w * 0.3));
    p.rect_filled(body, w * 0.12, color);
    p.circle_filled(pos2(body.right() - w * 0.18, body.center().y), w * 0.07, Color32::from_black_alpha(120));
}

pub fn globe(p: &Painter, r: Rect, color: Color32) {
    let c = r.center();
    let rad = r.width() * 0.38;
    let s = Stroke::new((r.width() * 0.09).max(1.0), color);
    p.circle_stroke(c, rad, s);
    p.line_segment([pos2(c.x - rad, c.y), pos2(c.x + rad, c.y)], s);
    p.add(Shape::ellipse_stroke(c, vec2(rad * 0.45, rad), s));
}

pub fn star(p: &Painter, r: Rect, color: Color32, filled: bool) {
    let c = r.center();
    let outer = r.width() * 0.44;
    let inner = outer * 0.45;
    let pts: Vec<Pos2> = (0..10)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
            let rad = if i % 2 == 0 { outer } else { inner };
            pos2(c.x + rad * a.cos(), c.y + rad * a.sin())
        })
        .collect();
    if filled {
        // Star is concave: fill as a fan of triangles from the centre.
        for i in 0..10 {
            let tri = vec![c, pts[i], pts[(i + 1) % 10]];
            p.add(Shape::convex_polygon(tri, color, Stroke::NONE));
        }
    } else {
        p.add(Shape::closed_line(pts, Stroke::new(1.2_f32, color)));
    }
}

pub fn search(p: &Painter, r: Rect, color: Color32) {
    let w = r.width();
    let c = r.center() - vec2(w * 0.08, w * 0.08);
    let s = Stroke::new((w * 0.1).max(1.2), color);
    p.circle_stroke(c, w * 0.26, s);
    p.line_segment([c + vec2(w * 0.19, w * 0.19), c + vec2(w * 0.4, w * 0.4)], s);
}

/// Chevron used by navigation buttons: dir = -1 left, 1 right, 0 up.
/// `dir`: -1 left, 1 right, 0 up, 2 down.
pub fn chevron(p: &Painter, r: Rect, dir: i8, color: Color32) {
    let c = r.center();
    let d = r.width() * 0.2;
    let s = Stroke::new(1.6_f32, color);
    let pts = match dir {
        -1 => vec![pos2(c.x + d * 0.6, c.y - d * 1.2), pos2(c.x - d * 0.6, c.y), pos2(c.x + d * 0.6, c.y + d * 1.2)],
        1 => vec![pos2(c.x - d * 0.6, c.y - d * 1.2), pos2(c.x + d * 0.6, c.y), pos2(c.x - d * 0.6, c.y + d * 1.2)],
        2 => vec![pos2(c.x - d * 1.2, c.y - d * 0.6), pos2(c.x, c.y + d * 0.6), pos2(c.x + d * 1.2, c.y - d * 0.6)],
        _ => vec![pos2(c.x - d * 1.2, c.y + d * 0.6), pos2(c.x, c.y - d * 0.6), pos2(c.x + d * 1.2, c.y + d * 0.6)],
    };
    p.add(Shape::line(pts, s));
}

/// A frameless square icon button. Returns the response.
pub fn icon_button(ui: &mut egui::Ui, size: f32, enabled: bool, th: &Theme, draw: impl FnOnce(&Painter, Rect, Color32)) -> egui::Response {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), sense);
    let color = if !enabled {
        th.weak.gamma_multiply(0.45)
    } else if resp.hovered() {
        th.strong()
    } else {
        th.weak
    };
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, th.hover());
    }
    draw(ui.painter(), rect.shrink(size * 0.2), color);
    resp
}

/// Eject: a triangle over a bar.
pub fn eject(p: &Painter, r: Rect, col: Color32) {
    let (l, w) = (r.left(), r.width());
    let tri_bottom = r.top() + r.height() * 0.6;
    p.add(egui::Shape::convex_polygon(
        vec![pos2(r.center().x, r.top() + r.height() * 0.08), pos2(l + w * 0.95, tri_bottom), pos2(l + w * 0.05, tri_bottom)],
        col,
        Stroke::NONE,
    ));
    p.rect_filled(Rect::from_min_max(pos2(l + w * 0.05, r.top() + r.height() * 0.76), pos2(l + w * 0.95, r.top() + r.height() * 0.92)), 1.0, col);
}

pub fn plus(p: &Painter, r: Rect, col: Color32) {
    let s = Stroke::new(1.5_f32, col);
    let c = r.center();
    let d = r.width() * 0.35;
    p.line_segment([pos2(c.x - d, c.y), pos2(c.x + d, c.y)], s);
    p.line_segment([pos2(c.x, c.y - d), pos2(c.x, c.y + d)], s);
}

// ---------------------------------------------------------------- line icons
// Consistent 1.5 px outline icons for the toolbar (drawn inside `r`).

fn line(w: f32) -> Stroke {
    Stroke::new((w * 0.11).clamp(1.3, 1.8), Color32::TRANSPARENT)
}

fn st(r: Rect, c: Color32) -> Stroke {
    let mut s = line(r.width());
    s.color = c;
    s
}

pub fn gear(p: &Painter, r: Rect, c: Color32) {
    let s = st(r, c);
    let ctr = r.center();
    let ro = r.width() * 0.48;
    let ri = r.width() * 0.34;
    // Cog outline: 8 teeth.
    let mut pts = Vec::new();
    let n = 8;
    for i in 0..n {
        let a0 = std::f32::consts::TAU * i as f32 / n as f32;
        let step = std::f32::consts::TAU / n as f32;
        for (frac, rad) in [(0.0, ri), (0.18, ro), (0.5, ro), (0.68, ri)] {
            let a = a0 + step * frac;
            pts.push(pos2(ctr.x + rad * a.cos(), ctr.y + rad * a.sin()));
        }
    }
    p.add(Shape::closed_line(pts, s));
    p.circle_stroke(ctr, r.width() * 0.14, s);
}

pub fn shield(p: &Painter, r: Rect, c: Color32) {
    let s = st(r, c);
    let (x0, x1, y0, y1) = (r.left() + r.width() * 0.12, r.right() - r.width() * 0.12, r.top() + 1.0, r.bottom() - 1.0);
    let cx = r.center().x;
    let pts = vec![
        pos2(cx, y0),
        pos2(x1, y0 + r.height() * 0.18),
        pos2(x1, r.center().y),
        pos2(cx + r.width() * 0.18, y1 - r.height() * 0.12),
        pos2(cx, y1),
        pos2(cx - r.width() * 0.18, y1 - r.height() * 0.12),
        pos2(x0, r.center().y),
        pos2(x0, y0 + r.height() * 0.18),
    ];
    p.add(Shape::closed_line(pts, s));
    // check mark
    p.line_segment([pos2(cx - r.width() * 0.17, r.center().y), pos2(cx - r.width() * 0.03, r.center().y + r.height() * 0.14)], s);
    p.line_segment([pos2(cx - r.width() * 0.03, r.center().y + r.height() * 0.14), pos2(cx + r.width() * 0.2, r.center().y - r.height() * 0.12)], s);
}

/// A disc that fills clockwise from the top (`None`: progress unknown, a wedge goes around).
pub fn progress_pie(p: &Painter, r: Rect, c: Color32, fraction: Option<f32>, time: f32) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let center = r.center();
    let radius = r.width().min(r.height()) * 0.5 + 0.5;
    p.circle_filled(center, radius, c.gamma_multiply(0.25));
    let (start, sweep) = match fraction {
        Some(f) => (-FRAC_PI_2, f.clamp(0.0, 1.0) * TAU),
        None => ((time * 0.8).fract() * TAU, TAU * 0.25),
    };
    if sweep >= TAU - 0.01 {
        p.circle_filled(center, radius, c);
    } else if sweep > 0.0 {
        // An arc as wide as the radius, halfway out, fills the wedge with smooth edges.
        // Fine steps: a line's end is square to its first step, which should nearly be a tangent.
        let steps = ((sweep / 0.05).ceil() as usize).max(2);
        let points = (0..=steps)
            .map(|i| {
                let a = start + sweep * i as f32 / steps as f32;
                center + egui::vec2(a.cos(), a.sin()) * radius * 0.5
            })
            .collect();
        p.add(egui::epaint::PathShape::line(points, egui::Stroke::new(radius, c)));
    }
}

/// Two opposite arrows: the transfer manager.
pub fn transfers(p: &Painter, r: Rect, c: Color32) {
    let s = egui::Stroke::new(1.4_f32, c);
    let (x1, x2) = (r.left() + r.width() * 0.3, r.right() - r.width() * 0.3);
    p.line_segment([egui::pos2(x1, r.bottom()), egui::pos2(x1, r.top())], s);
    p.line_segment([egui::pos2(x1, r.top()), egui::pos2(x1 - 3.5, r.top() + 3.5)], s);
    p.line_segment([egui::pos2(x1, r.top()), egui::pos2(x1 + 3.5, r.top() + 3.5)], s);
    p.line_segment([egui::pos2(x2, r.top()), egui::pos2(x2, r.bottom())], s);
    p.line_segment([egui::pos2(x2, r.bottom()), egui::pos2(x2 - 3.5, r.bottom() - 3.5)], s);
    p.line_segment([egui::pos2(x2, r.bottom()), egui::pos2(x2 + 3.5, r.bottom() - 3.5)], s);
}

pub fn bars(p: &Painter, r: Rect, c: Color32) {
    let s = st(r, c);
    let w = r.width();
    let base = r.bottom() - 1.0;
    for (i, h) in [0.45f32, 0.8, 0.6].iter().enumerate() {
        let x = r.left() + w * (0.2 + 0.3 * i as f32);
        p.line_segment([pos2(x, base), pos2(x, base - (r.height() - 2.0) * h)], Stroke::new(s.width + 0.8, c));
    }
}

pub fn eye(p: &Painter, r: Rect, c: Color32) {
    let s = st(r, c);
    let ctr = r.center();
    let w = r.width() * 0.5;
    let h = r.height() * 0.32;
    let top: Vec<Pos2> = (0..=16)
        .map(|i| {
            let t = i as f32 / 16.0;
            let x = ctr.x - w + 2.0 * w * t;
            pos2(x, ctr.y - h * (1.0 - (2.0 * t - 1.0).powi(2)))
        })
        .collect();
    let bottom: Vec<Pos2> = top.iter().map(|q| pos2(q.x, 2.0 * ctr.y - q.y)).collect();
    p.add(Shape::line(top, s));
    p.add(Shape::line(bottom, s));
    p.circle_stroke(ctr, r.width() * 0.15, s);
}

// ---------------------------------------------------------------- menu icons
// Small outline icons for context menus (drawn inside a ~14 px square).

/// Point at fractions (0..1) of `r`.
fn at(r: Rect, x: f32, y: f32) -> egui::Pos2 {
    pos2(r.left() + r.width() * x, r.top() + r.height() * y)
}

fn poly(p: &Painter, r: Rect, s: Stroke, pts: &[(f32, f32)], closed: bool) {
    let v: Vec<egui::Pos2> = pts.iter().map(|&(x, y)| at(r, x, y)).collect();
    p.add(if closed { Shape::closed_line(v, s) } else { Shape::line(v, s) });
}

fn ms(c: Color32) -> Stroke {
    Stroke::new(1.3_f32, c)
}

pub fn mi_open(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    poly(p, r, s, &[(0.08, 0.85), (0.08, 0.15), (0.4, 0.15), (0.5, 0.28), (0.85, 0.28), (0.85, 0.45)], false);
    poly(p, r, s, &[(0.08, 0.85), (0.25, 0.5), (0.98, 0.5), (0.8, 0.85)], true);
}

pub fn mi_copy(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    p.rect_stroke(Rect::from_min_max(at(r, 0.32, 0.3), at(r, 0.92, 0.95)), 2.0, s, egui::StrokeKind::Inside);
    poly(p, r, s, &[(0.18, 0.72), (0.08, 0.72), (0.08, 0.05), (0.68, 0.05), (0.68, 0.18)], false);
}

pub fn mi_move(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    poly(p, r, s, &[(0.08, 0.5), (0.9, 0.5)], false);
    poly(p, r, s, &[(0.58, 0.2), (0.9, 0.5), (0.58, 0.8)], false);
}

pub fn mi_rename(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    poly(p, r, s, &[(0.2, 0.62), (0.68, 0.12), (0.9, 0.34), (0.42, 0.84), (0.14, 0.9)], true);
    poly(p, r, s, &[(0.58, 0.22), (0.8, 0.44)], false);
}

pub fn mi_trash(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    poly(p, r, s, &[(0.08, 0.22), (0.92, 0.22)], false);
    poly(p, r, s, &[(0.36, 0.22), (0.4, 0.06), (0.6, 0.06), (0.64, 0.22)], false);
    poly(p, r, s, &[(0.18, 0.22), (0.25, 0.94), (0.75, 0.94), (0.82, 0.22)], false);
    poly(p, r, s, &[(0.42, 0.4), (0.42, 0.78)], false);
    poly(p, r, s, &[(0.58, 0.4), (0.58, 0.78)], false);
}

pub fn mi_archive(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    p.rect_stroke(Rect::from_min_max(at(r, 0.06, 0.1), at(r, 0.94, 0.32)), 1.5, s, egui::StrokeKind::Inside);
    poly(p, r, s, &[(0.14, 0.32), (0.14, 0.92), (0.86, 0.92), (0.86, 0.32)], false);
    poly(p, r, s, &[(0.38, 0.5), (0.62, 0.5)], false);
}

pub fn mi_link(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    // Clipboard with a line of text: "copy path".
    p.rect_stroke(Rect::from_min_max(at(r, 0.14, 0.14), at(r, 0.86, 0.96)), 2.0, s, egui::StrokeKind::Inside);
    p.rect_stroke(Rect::from_min_max(at(r, 0.34, 0.04), at(r, 0.66, 0.24)), 1.5, s, egui::StrokeKind::Inside);
    poly(p, r, s, &[(0.3, 0.52), (0.7, 0.52)], false);
    poly(p, r, s, &[(0.3, 0.72), (0.6, 0.72)], false);
}

pub fn mi_checksum(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    // Hash sign with a check mark: "checksum".
    poly(p, r, s, &[(0.34, 0.08), (0.24, 0.72)], false);
    poly(p, r, s, &[(0.62, 0.08), (0.52, 0.72)], false);
    poly(p, r, s, &[(0.1, 0.28), (0.76, 0.28)], false);
    poly(p, r, s, &[(0.06, 0.54), (0.7, 0.54)], false);
    poly(p, r, s, &[(0.56, 0.8), (0.7, 0.94), (0.96, 0.62)], false);
}

pub fn mi_info(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    p.circle_stroke(r.center(), r.width() * 0.45, s);
    poly(p, r, s, &[(0.5, 0.45), (0.5, 0.74)], false);
    p.circle_filled(at(r, 0.5, 0.3), 1.0, c);
}

pub fn mi_more(p: &Painter, r: Rect, c: Color32) {
    for x in [0.18, 0.5, 0.82] {
        p.circle_filled(at(r, x, 0.5), 1.4, c);
    }
}

pub fn mi_external(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    poly(p, r, s, &[(0.45, 0.12), (0.1, 0.12), (0.1, 0.9), (0.88, 0.9), (0.88, 0.55)], false);
    poly(p, r, s, &[(0.42, 0.58), (0.9, 0.1)], false);
    poly(p, r, s, &[(0.6, 0.1), (0.9, 0.1), (0.9, 0.4)], false);
}

pub fn mi_new_folder(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    poly(p, r, s, &[(0.06, 0.18), (0.38, 0.18), (0.48, 0.3), (0.94, 0.3), (0.94, 0.88), (0.06, 0.88)], true);
    poly(p, r, s, &[(0.5, 0.45), (0.5, 0.75)], false);
    poly(p, r, s, &[(0.35, 0.6), (0.65, 0.6)], false);
}

pub fn mi_new_file(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    poly(p, r, s, &[(0.16, 0.04), (0.62, 0.04), (0.86, 0.28), (0.86, 0.96), (0.16, 0.96)], true);
    poly(p, r, s, &[(0.51, 0.45), (0.51, 0.77)], false);
    poly(p, r, s, &[(0.35, 0.61), (0.67, 0.61)], false);
}

pub fn mi_refresh(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    let ctr = r.center();
    let rad = r.width() * 0.4;
    let pts: Vec<egui::Pos2> = (0..=20)
        .map(|i| {
            let a = 0.6 + (i as f32 / 20.0) * 4.9;
            pos2(ctr.x + rad * a.cos(), ctr.y + rad * a.sin())
        })
        .collect();
    let end = *pts.last().unwrap();
    p.add(Shape::line(pts, s));
    p.line_segment([end, end + egui::vec2(3.5, 0.5)], s);
    p.line_segment([end, end + egui::vec2(0.5, 3.5)], s);
}

pub fn mi_select(p: &Painter, r: Rect, c: Color32) {
    let s = ms(c);
    p.rect_stroke(r.shrink(r.width() * 0.08), 2.5, s, egui::StrokeKind::Inside);
    poly(p, r, s, &[(0.28, 0.52), (0.44, 0.68), (0.74, 0.34)], false);
}

pub fn mi_view(p: &Painter, r: Rect, c: Color32) {
    eye(p, r, c);
}

pub fn mi_star(p: &Painter, r: Rect, c: Color32) {
    star(p, r, c, false);
}

/// Split a window: a window in two, the new half (on the right) filled.
pub fn mi_split(p: &Painter, r: Rect, c: Color32) {
    split_half(p, r, c, Side::Right, false);
}

/// Which half of a window a new one takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Right,
    Left,
    Bottom,
    Top,
}

/// A window divided in two, the half where the new window goes filled; `terminal`
/// draws a prompt (`>`) there instead of the fill. Drawn on whole pixels, the fill and
/// the divider inside the outline, so every corner has the same round.
pub fn split_half(p: &Painter, r: Rect, c: Color32, side: Side, terminal: bool) {
    const W: f32 = 1.3;
    const RADIUS: f32 = 3.0;
    let s = Stroke::new(W, c);
    // The outline on whole pixels, a square-ish window a little wider than tall.
    use egui::emath::GuiRounding;
    let ppp = p.pixels_per_point();
    let f = Rect::from_center_size(r.center(), egui::vec2(r.width(), (r.height() * 0.86).max(8.0))).round_to_pixels(ppp);
    let inner = f.shrink(W);
    let (cx, cy) = (inner.center().x.round_to_pixel_center(ppp), inner.center().y.round_to_pixel_center(ppp));
    let half = match side {
        Side::Right => Rect::from_min_max(pos2(cx, inner.top()), inner.max),
        Side::Left => Rect::from_min_max(inner.min, pos2(cx, inner.bottom())),
        Side::Bottom => Rect::from_min_max(pos2(inner.left(), cy), inner.max),
        Side::Top => Rect::from_min_max(inner.min, pos2(inner.right(), cy)),
    };
    if terminal {
        // A prompt (">") as large as the new half allows.
        let k = half.width().min(half.height()) * 0.62;
        let c0 = half.center();
        let t = Stroke::new(1.3_f32, c);
        p.line_segment([pos2(c0.x - k * 0.3, c0.y - k * 0.5), pos2(c0.x + k * 0.25, c0.y)], t);
        p.line_segment([pos2(c0.x + k * 0.25, c0.y), pos2(c0.x - k * 0.3, c0.y + k * 0.5)], t);
    } else {
        // Rounded where it meets the outline's corners (about its radius less the stroke).
        let o = 2;
        let corners = match side {
            Side::Right => egui::CornerRadius { ne: o, se: o, ..Default::default() },
            Side::Left => egui::CornerRadius { nw: o, sw: o, ..Default::default() },
            Side::Bottom => egui::CornerRadius { sw: o, se: o, ..Default::default() },
            Side::Top => egui::CornerRadius { nw: o, ne: o, ..Default::default() },
        };
        p.rect_filled(half, corners, c.gamma_multiply(0.8));
    }
    // The divider between the halves, inside the outline.
    match side {
        Side::Right | Side::Left => p.line_segment([pos2(cx, inner.top()), pos2(cx, inner.bottom())], s),
        Side::Bottom | Side::Top => p.line_segment([pos2(inner.left(), cy), pos2(inner.right(), cy)], s),
    };
    p.rect_stroke(f, RADIUS, s, egui::StrokeKind::Inside);
}

pub fn mi_disk(p: &Painter, r: Rect, c: Color32) {
    bars(p, r, c);
}

// ---------------------------------------------------------------- bookmarks

/// A bookmark's icon. Standard folders get a line glyph in the accent colour (as
/// in Finder or Files), or the desktop icon theme's own folder icon when one is in
/// use; plain bookmarks get the active style's folder.
pub fn bookmark_icon(p: &Painter, r: Rect, icon: crate::config::BookmarkIcon, th: &Theme) {
    use crate::config::BookmarkIcon as B;
    let names: &[&str] = match icon {
        B::Home => &["user-home", "folder-home"],
        B::Desktop => &["user-desktop", "folder-desktop"],
        B::Documents => &["folder-documents", "folder-document"],
        B::Downloads => &["folder-download", "folder-downloads"],
        B::Music => &["folder-music", "folder-sound"],
        B::Pictures => &["folder-pictures", "folder-images"],
        B::Movies => &["folder-videos", "folder-video"],
        B::Network => &["folder-remote", "network-server"],
        _ => &[],
    };
    if !names.is_empty() {
        if let Some(sys) = &current().system {
            let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
            if let Some(path) = lookup_cached(sys, &names) {
                if draw_theme_icon(p, r, &path, th) {
                    return;
                }
            }
        }
    }
    let c = th.accent;
    let s = Stroke::new((r.width() * 0.085).max(1.2), c);
    match icon {
        B::Auto | B::Folder => folder_icon(p, r, th),
        B::Home => {
            poly(p, r, s, &[(0.08, 0.5), (0.5, 0.12), (0.92, 0.5)], false);
            poly(p, r, s, &[(0.22, 0.4), (0.22, 0.9), (0.78, 0.9), (0.78, 0.4)], false);
            poly(p, r, s, &[(0.42, 0.9), (0.42, 0.64), (0.58, 0.64), (0.58, 0.9)], false);
        }
        B::Desktop => {
            p.rect_stroke(Rect::from_min_max(at(r, 0.06, 0.14), at(r, 0.94, 0.7)), r.width() * 0.08, s, egui::StrokeKind::Inside);
            poly(p, r, s, &[(0.5, 0.7), (0.5, 0.88)], false);
            poly(p, r, s, &[(0.28, 0.9), (0.72, 0.9)], false);
        }
        B::Documents => {
            poly(p, r, s, &[(0.18, 0.06), (0.6, 0.06), (0.84, 0.3), (0.84, 0.94), (0.18, 0.94)], true);
            poly(p, r, s, &[(0.6, 0.06), (0.6, 0.3), (0.84, 0.3)], false);
            for y in [0.5, 0.64, 0.78] {
                poly(p, r, Stroke::new(s.width * 0.8, c), &[(0.32, y), (0.7, y)], false);
            }
        }
        B::Downloads => {
            poly(p, r, s, &[(0.5, 0.08), (0.5, 0.64)], false);
            poly(p, r, s, &[(0.26, 0.42), (0.5, 0.66), (0.74, 0.42)], false);
            poly(p, r, s, &[(0.1, 0.64), (0.1, 0.9), (0.9, 0.9), (0.9, 0.64)], false);
        }
        B::Music => {
            poly(p, r, s, &[(0.36, 0.76), (0.36, 0.22), (0.84, 0.1), (0.84, 0.66)], false);
            p.circle_filled(at(r, 0.25, 0.78), r.width() * 0.13, c);
            p.circle_filled(at(r, 0.73, 0.68), r.width() * 0.13, c);
        }
        B::Pictures => {
            p.rect_stroke(Rect::from_min_max(at(r, 0.06, 0.14), at(r, 0.94, 0.86)), r.width() * 0.08, s, egui::StrokeKind::Inside);
            poly(p, r, s, &[(0.14, 0.78), (0.4, 0.5), (0.6, 0.7), (0.72, 0.58), (0.86, 0.74)], false);
            p.circle_filled(at(r, 0.68, 0.34), r.width() * 0.08, c);
        }
        B::Movies => {
            p.rect_stroke(Rect::from_min_max(at(r, 0.06, 0.16), at(r, 0.94, 0.84)), r.width() * 0.08, s, egui::StrokeKind::Inside);
            p.add(Shape::convex_polygon(vec![at(r, 0.4, 0.34), at(r, 0.68, 0.5), at(r, 0.4, 0.66)], c, Stroke::NONE));
        }
        B::Star => star(p, r, c, false),
        B::Code => {
            poly(p, r, s, &[(0.32, 0.26), (0.08, 0.5), (0.32, 0.74)], false);
            poly(p, r, s, &[(0.68, 0.26), (0.92, 0.5), (0.68, 0.74)], false);
            poly(p, r, s, &[(0.58, 0.18), (0.42, 0.82)], false);
        }
        B::Network => globe(p, r, c),
    }
}

/// A bookmark folder (a group of bookmarks): an outlined folder.
pub fn bookmark_folder(p: &Painter, r: Rect, c: Color32) {
    let s = Stroke::new((r.width() * 0.085).max(1.2), c);
    poly(p, r, s, &[(0.06, 0.86), (0.06, 0.16), (0.38, 0.16), (0.48, 0.28), (0.94, 0.28), (0.94, 0.86)], true);
    poly(p, r, s, &[(0.06, 0.42), (0.94, 0.42)], false);
}

/// A funnel: the quick filter button.
pub fn funnel(p: &Painter, r: Rect, c: Color32, filled: bool) {
    let pts = [(0.08, 0.14), (0.92, 0.14), (0.6, 0.52), (0.6, 0.86), (0.4, 0.94), (0.4, 0.52)];
    if filled {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| at(r, x, y)).collect();
        // Convex in two parts: the cone and the stem.
        p.add(Shape::convex_polygon(vec![v[0], v[1], v[2], v[5]], c, Stroke::NONE));
        p.add(Shape::convex_polygon(vec![v[5], v[2], v[3], v[4]], c, Stroke::NONE));
    } else {
        poly(p, r, ms(c), &pts, true);
    }
}
