//! The icon sets that ship with the app as SVG files (assets/icons/, made by
//! scripts/icon-sets.py): Material Icon Theme, in colour, with icons for many file types,
//! file names (`Cargo.toml`, `Dockerfile`) and folder names (`src`, `docs`); and Fluent UI
//! System Icons, one-colour shapes tinted with the theme's colours.
//!
//! An icon is rendered once per size into a texture and kept.
use crate::style::icons::{kind_color, kind_of, FileKind};
use crate::style::theme::Theme;
use egui::{pos2, Color32, Painter, Rect};
use std::collections::HashMap;

mod fluent;
mod material;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Set {
    Material,
    Fluent,
}

/// Rendered icons: (set, icon, size in pixels) → texture (`None`: it did not render),
/// kept in the context's memory (a texture belongs to its context).
type Cache = HashMap<(Set, &'static str, u32), Option<egui::TextureHandle>>;

fn cache_id() -> egui::Id {
    egui::Id::new("bundled-icon-textures")
}

fn find(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table.binary_search_by(|(k, _)| (*k).cmp(key)).ok().map(|i| table[i].1)
}

fn svg(set: Set, name: &str) -> Option<(&'static str, &'static [u8])> {
    let icons = match set {
        Set::Material => material::ICONS,
        Set::Fluent => fluent::ICONS,
    };
    icons.binary_search_by(|(k, _)| (*k).cmp(name)).ok().map(|i| icons[i])
}

/// Material's icon for a file or folder: by its whole name, then by its extension (the
/// longest one first: `d.ts` before `ts`), then the plain file or folder.
pub fn material_icon(name: &str, is_dir: bool, light: bool) -> &'static str {
    let lower = name.to_lowercase();
    let icon = if is_dir {
        find(material::FOLDER_NAMES, &lower).unwrap_or(material::FOLDER)
    } else {
        find(material::FILE_NAMES, &lower)
            .or_else(|| lower.char_indices().filter(|(_, c)| *c == '.').find_map(|(i, _)| find(material::EXTENSIONS, &lower[i + 1..])))
            .unwrap_or(material::FILE)
    };
    if light {
        find(material::LIGHT, icon).unwrap_or(icon)
    } else {
        icon
    }
}

/// Fluent's icon for a file or folder, and the colour it is tinted.
pub fn fluent_icon(is_dir: bool, ext: &str, th: &Theme) -> (&'static str, Color32) {
    if is_dir {
        return ("folder", th.dir);
    }
    let kind = kind_of(ext);
    let icon = match ext {
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => "javascript",
        "css" | "scss" | "sass" | "less" => "css",
        "java" | "kt" | "kts" => "java",
        "yml" | "yaml" => "yml",
        "csv" | "tsv" => "csv",
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" => "console",
        "db" | "sqlite" | "sqlite3" | "duckdb" | "sql" => "database",
        "onnx" | "safetensors" | "pt" | "pth" | "ckpt" | "gguf" => "model",
        "pem" | "key" | "crt" | "cer" | "pub" | "asc" | "gpg" => "key",
        "lock" => "lock",
        "ini" | "cfg" | "conf" | "env" => "settings",
        "epub" | "mobi" => "book",
        "ipynb" => "notebook",
        _ => match kind {
            FileKind::Image => "image",
            FileKind::Video => "video",
            FileKind::Audio => "audio",
            FileKind::Archive => "archive",
            FileKind::Code => "code",
            FileKind::Text => "text",
            FileKind::Doc => "word",
            FileKind::Sheet => "table",
            FileKind::Slides => "slides",
            FileKind::Pdf => "pdf",
            FileKind::Exec => "app",
            FileKind::Disk => "disk",
            FileKind::Font => "font",
            FileKind::Data => "data",
            FileKind::Other => "document",
        },
    };
    (icon, if kind == FileKind::Other { th.weak } else { kind_color(kind, th) })
}

/// The sizes icons are rendered at (pixels): the next one up from the size shown, so a
/// change of zoom reuses them rather than rendering every icon again.
const SIZES: [u32; 12] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 192, 256, 384];

/// New icons rendered in one frame at most: a folder full of different types fills in
/// over a few frames instead of holding one up.
const PER_FRAME: usize = 16;

fn size_for(px: f32) -> u32 {
    SIZES.iter().copied().find(|s| *s as f32 >= px).unwrap_or(SIZES[SIZES.len() - 1])
}

/// Draw `set`'s icon for a file or folder into `r`. Returns false if it cannot be drawn
/// (the caller draws its own); an icon whose turn to be rendered has not come yet leaves
/// its place empty for a frame.
pub fn draw(set: Set, p: &Painter, r: Rect, name: &str, is_dir: bool, ext: &str, th: &Theme) -> bool {
    let (icon, tint) = match set {
        Set::Material => (material_icon(name, is_dir, !th.dark), Color32::WHITE),
        Set::Fluent => fluent_icon(is_dir, ext, th),
    };
    let px = size_for(r.width() * p.ctx().pixels_per_point());
    match texture(p.ctx(), set, icon, px) {
        Tex::Ready(tex) => {
            p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
            true
        }
        Tex::Later => {
            p.ctx().request_repaint();
            true
        }
        Tex::Failed => false,
    }
}

enum Tex {
    Ready(egui::TextureHandle),
    /// This frame has rendered its share of new icons.
    Later,
    Failed,
}

/// How many new icons this frame rendered: (frame, count).
#[derive(Clone, Copy, Default)]
struct Budget(u64, usize);

fn texture(ctx: &egui::Context, set: Set, icon: &str, px: u32) -> Tex {
    let Some((name, data)) = svg(set, icon) else { return Tex::Failed };
    let key = (set, name, px);
    if let Some(t) = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(cache_id()).get(&key).cloned()) {
        return t.map_or(Tex::Failed, Tex::Ready);
    }
    let frame = ctx.cumulative_pass_nr();
    let allowed = ctx.data_mut(|d| {
        let b = d.get_temp_mut_or_default::<Budget>(egui::Id::new("bundled-icon-budget"));
        if b.0 != frame {
            *b = Budget(frame, 0);
        }
        b.1 += 1;
        b.1 <= PER_FRAME
    });
    if !allowed {
        return Tex::Later;
    }
    // Rendered outside the context's lock (making a texture takes it too).
    let tex = crate::style::logo::render(data, px).map(|mut rgba| {
        if set == Set::Fluent {
            // One-colour shapes: keep the shape (alpha), tint it when drawn.
            for c in rgba.chunks_exact_mut(4) {
                c[0] = 255;
                c[1] = 255;
                c[2] = 255;
            }
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &rgba);
        ctx.load_texture(format!("iconset:{set:?}:{name}:{px}"), img, egui::TextureOptions::LINEAR)
    });
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(cache_id()).insert(key, tex.clone()));
    tex.map_or(Tex::Failed, Tex::Ready)
}

/// The icons drawn so far in `ctx` (tests).
#[cfg(test)]
pub fn drawn(ctx: &egui::Context, set: Set) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(cache_id()).keys().filter(|k| k.0 == set).map(|k| k.1).collect());
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_and_point_at_icons() {
        for (what, t) in
            [("extensions", material::EXTENSIONS), ("file names", material::FILE_NAMES), ("folders", material::FOLDER_NAMES), ("light", material::LIGHT)]
        {
            assert!(t.windows(2).all(|w| w[0].0 < w[1].0), "{what} sorted");
            for (k, icon) in t {
                assert!(svg(Set::Material, icon).is_some(), "{what}: {k} → {icon}");
            }
        }
        assert!(material::ICONS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(fluent::ICONS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn files_and_folders_get_their_icons() {
        assert_eq!(material_icon("main.rs", false, false), "rust");
        assert_eq!(material_icon("Package.json", false, false), "nodejs", "by its whole name");
        assert_eq!(material_icon("Cargo.toml", false, true), "toml_light", "a light variant");
        assert_eq!(material_icon("types.d.ts", false, false), find(material::EXTENSIONS, "d.ts").unwrap(), "the longest extension");
        assert_eq!(material_icon("App.JS", false, false), find(material::EXTENSIONS, "js").unwrap());
        assert_eq!(material_icon("no-extension", false, false), material::FILE);
        assert_eq!(material_icon("src", true, false), find(material::FOLDER_NAMES, "src").unwrap());
        assert_eq!(material_icon("My Folder", true, false), material::FOLDER);
        // A `.key` file is a private key, not a Keynote document (those are folders).
        assert_ne!(kind_of("key"), FileKind::Slides);
        assert_eq!(fluent_icon(false, "key", crate::style::theme::by_name("Breeze Light")).0, "key");
        // Every Fluent icon the code asks for is there.
        let th = crate::style::theme::by_name("Breeze Light");
        for ext in [
            "rs", "js", "css", "java", "yml", "csv", "sh", "db", "onnx", "pem", "lock", "ini", "epub", "ipynb", "png", "mp4", "mp3", "zip", "txt", "docx",
            "xlsx", "pptx", "pdf", "exe", "iso", "ttf", "json", "xyz",
        ] {
            let (icon, _) = fluent_icon(false, ext, th);
            assert!(svg(Set::Fluent, icon).is_some(), "{ext} → {icon}");
        }
        assert!(svg(Set::Fluent, fluent_icon(true, "", th).0).is_some());
    }

    #[test]
    fn many_new_icons_fill_in_over_a_few_frames() {
        let ctx = egui::Context::default();
        let th = crate::style::theme::by_name("Breeze Light");
        let names: Vec<String> = material::EXTENSIONS.iter().filter(|(e, _)| !e.contains('.')).take(400).map(|(e, _)| format!("x.{e}")).collect();
        let want: std::collections::HashSet<&str> = names.iter().map(|n| material_icon(n, false, true)).collect();
        let r = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(58.0, 58.0));
        let mut done = 0;
        for _ in 0..100 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                let p = ctx.layer_painter(egui::LayerId::background());
                for n in &names {
                    assert!(draw(Set::Material, &p, r, n, false, "", th), "drawn or waiting its turn");
                }
            });
            let now = drawn(&ctx, Set::Material).len();
            assert!(now - done <= PER_FRAME, "at most {PER_FRAME} new icons a frame, not {}", now - done);
            done = now;
            if done == want.len() {
                break;
            }
        }
        assert_eq!(done, want.len(), "all of them, over the frames");
        // Sizes are rounded up to a few: a slightly other zoom reuses them.
        assert_eq!((size_for(57.0), size_for(58.0), size_for(64.0), size_for(65.0), size_for(9999.0)), (64, 64, 64, 96, 384));
    }

    #[test]
    fn every_bundled_icon_renders() {
        for (name, data) in material::ICONS.iter().chain(fluent::ICONS) {
            let px = crate::style::logo::render(data, 24).unwrap_or_else(|| panic!("{name} does not render"));
            assert!(px.chunks_exact(4).any(|c| c[3] > 0), "{name} is empty");
        }
    }
}
