//! The app's fonts: the bundled ones plus the interface and terminal fonts picked in
//! Settings. Interface size works through a font scale, so all text follows it,
//! including the many widgets that ask for an explicit size.
use crate::config::Settings;
use egui::epaint::text::FontTweak;
use egui::{FontData, FontDefinitions, FontFamily};
use resvg::usvg::fontdb;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Text size the interface was designed for (`theme::apply`'s body text).
pub const DEFAULT_UI_SIZE: f32 = 14.0;
pub const UI_SIZES: std::ops::RangeInclusive<f32> = 11.0..=20.0;
pub const DEFAULT_TERM_SIZE: f32 = 14.0;
pub const TERM_SIZES: std::ops::RangeInclusive<f32> = 8.0..=28.0;

/// Interface fonts that ship with the app: (setting value, name). "" is the default.
pub const UI_BUILTIN: [(&str, &str); 3] = [("", "Source Sans 3"), ("builtin:serif", "Source Serif 4"), ("builtin:code", "Source Code Pro")];
/// Terminal fonts that ship with the app.
pub const TERM_BUILTIN: [(&str, &str); 3] = [("", "JetBrains Mono"), ("builtin:code", "Source Code Pro"), ("builtin:dejavu", "DejaVu Sans Mono")];

/// The name to show for a font setting.
pub fn label<'a>(value: &'a str, builtin: &'a [(&'a str, &'a str)]) -> &'a str {
    builtin.iter().find(|(k, _)| *k == value).map_or(value, |(_, n)| n)
}

/// The families that draw the interface (not the terminal).
const UI_FAMILIES: [&str; 4] = ["bold", "serif", "serif-bold", "serif-italic"];

fn ui_families() -> impl Iterator<Item = FontFamily> {
    [FontFamily::Proportional, FontFamily::Monospace].into_iter().chain(UI_FAMILIES.iter().map(|n| FontFamily::Name((*n).into())))
}

/// The font settings; the fonts only need rebuilding when these change.
#[derive(Clone, PartialEq, Debug)]
pub struct FontPrefs {
    pub ui_font: String,
    pub ui_size: f32,
    pub term_font: String,
}

impl FontPrefs {
    pub fn of(cfg: &Settings) -> Self {
        Self { ui_font: cfg.ui_font.clone(), ui_size: cfg.ui_font_size, term_font: cfg.term_font.clone() }
    }
}

/// The installed fonts. Scanning takes a moment, so `families` does it in the background.
pub fn db() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

pub struct Family {
    pub name: String,
    pub mono: bool,
}

/// Installed font families, sorted by name; `None` until the first scan has finished.
pub fn families(ctx: &egui::Context) -> Option<&'static [Family]> {
    static LIST: OnceLock<Vec<Family>> = OnceLock::new();
    static STARTED: OnceLock<()> = OnceLock::new();
    if let Some(list) = LIST.get() {
        return Some(list);
    }
    STARTED.get_or_init(|| {
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("font-list".into()).spawn(move || {
            let _ = LIST.set(list_families(db()));
            ctx.request_repaint();
        });
    });
    None
}

fn list_families(db: &fontdb::Database) -> Vec<Family> {
    let mut by_name: HashMap<&str, bool> = HashMap::new();
    for face in db.faces() {
        let Some((name, _)) = face.families.first() else { continue };
        // Names with a dot are private system fonts, and LastResort only draws placeholder boxes.
        if name.starts_with('.') || name.eq_ignore_ascii_case("LastResort") {
            continue;
        }
        *by_name.entry(name).or_default() |= face.monospaced;
    }
    let mut list: Vec<Family> = by_name.into_iter().map(|(name, mono)| Family { name: name.to_string(), mono }).collect();
    list.sort_by_cached_key(|f| f.name.to_lowercase());
    list
}

/// A font file's bytes and the index of a face in it.
type Face = (&'static [u8], u32);

/// A face of an installed font that egui can use. Its bytes stay loaded: choosing a
/// font again is instant, and the number of fonts one person tries is small.
fn load_face(family: &str, weight: u16) -> Option<Face> {
    static LOADED: OnceLock<Mutex<HashMap<fontdb::ID, Face>>> = OnceLock::new();
    let db = db();
    let id = db.query(&fontdb::Query { families: &[fontdb::Family::Name(family)], weight: fontdb::Weight(weight), stretch: fontdb::Stretch::Normal, style: fontdb::Style::Normal })?;
    let mut loaded = LOADED.get_or_init(Default::default).lock().ok()?;
    if let Some(face) = loaded.get(&id) {
        return Some(*face);
    }
    let (data, index) = db.with_face_data(id, |data, index| (data.to_vec(), index))?;
    // egui panics on a font it cannot parse, and shows nothing with a font that has no outlines.
    let face = ttf_parser::Face::parse(&data, index).ok()?;
    if !crate::fontfallback::draws(&face, 'A') {
        return None;
    }
    let face: Face = (Box::leak(data.into_boxed_slice()), index);
    loaded.insert(id, face);
    Some(face)
}

fn font_data((data, index): Face) -> Arc<FontData> {
    let mut f = FontData::from_static(data);
    f.index = index;
    Arc::new(f)
}

/// Make font `value` available in `d`; returns its keys (regular, bold).
fn user_font(d: &mut FontDefinitions, slot: &str, value: &str, bold_weight: u16) -> Option<(String, String)> {
    match value {
        "builtin:serif" => return Some(("serif".into(), "serif-bold".into())),
        "builtin:code" => return Some(("mono".into(), "mono".into())),
        "builtin:dejavu" => return Some(("dejavu-mono".into(), "dejavu-mono".into())),
        _ => {}
    }
    let regular = load_face(value, 400)?;
    let (key, bold_key) = (format!("{slot}:{value}"), format!("{slot}-bold:{value}"));
    d.font_data.insert(key.clone(), font_data(regular));
    match load_face(value, bold_weight) {
        Some(bold) if bold != regular => {
            d.font_data.insert(bold_key.clone(), font_data(bold));
            Some((key, bold_key))
        }
        _ => Some((key.clone(), key)),
    }
}

fn put_first(d: &mut FontDefinitions, family: FontFamily, key: String) {
    d.families.entry(family).or_default().insert(0, key);
}

/// The scale of the interface font (read by the on-demand system fallback fonts too).
static UI_SCALE: AtomicU32 = AtomicU32::new(1.0f32.to_bits());

pub fn ui_scale() -> f32 {
    f32::from_bits(UI_SCALE.load(Ordering::Relaxed))
}

/// A copy of `data` that draws `scale` times as large.
pub fn scaled(data: &FontData, scale: f32) -> FontData {
    let mut f = data.clone();
    f.tweak = FontTweak { scale: f.tweak.scale * scale, ..f.tweak };
    f
}

/// Install the fonts for these settings (from the next frame on). Returns the
/// chosen fonts that could not be loaded; the bundled defaults stand in for them.
pub fn apply(ctx: &egui::Context, prefs: &FontPrefs) -> Vec<String> {
    let mut d = base().clone();
    let mut failed = vec![];
    if !prefs.ui_font.is_empty() {
        match user_font(&mut d, "ui", &prefs.ui_font, 600) {
            Some((regular, bold)) => {
                put_first(&mut d, FontFamily::Proportional, regular);
                put_first(&mut d, FontFamily::Name("bold".into()), bold);
            }
            None => failed.push(prefs.ui_font.clone()),
        }
    }
    if !prefs.term_font.is_empty() {
        match user_font(&mut d, "term", &prefs.term_font, 700) {
            Some((regular, bold)) => {
                put_first(&mut d, FontFamily::Name("term".into()), regular);
                put_first(&mut d, FontFamily::Name("term-bold".into()), bold);
            }
            None => failed.push(prefs.term_font.clone()),
        }
    }
    for f in crate::fontfallback::added() {
        d.font_data.insert(f.name.clone(), font_data((f.data, f.index)));
        for family in ui_families().chain(term_families()) {
            d.families.entry(family).or_default().push(f.name.clone());
        }
    }
    let scale = (prefs.ui_size / DEFAULT_UI_SIZE).clamp(0.5, 2.0);
    UI_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    if scale != 1.0 {
        scale_interface(&mut d, scale);
    }
    ctx.set_fonts(d);
    failed
}

fn term_families() -> [FontFamily; 2] {
    [FontFamily::Name("term".into()), FontFamily::Name("term-bold".into())]
}

/// Point the interface's families at scaled copies of their fonts. The terminal keeps
/// the originals, as it shares fallback fonts with the interface but has its own size.
fn scale_interface(d: &mut FontDefinitions, scale: f32) {
    let mut done = HashSet::new();
    for family in ui_families() {
        let Some(keys) = d.families.get(&family).cloned() else { continue };
        let scaled_keys = keys.iter().map(|k| format!("{k}@ui")).collect();
        for key in keys {
            if done.insert(key.clone()) {
                if let Some(data) = d.font_data.get(&key).cloned() {
                    d.font_data.insert(format!("{key}@ui"), Arc::new(scaled(&data, scale)));
                }
            }
        }
        d.families.insert(family, scaled_keys);
    }
}

/// The bundled fonts and system fallbacks, read once.
fn base() -> &'static FontDefinitions {
    static BASE: OnceLock<FontDefinitions> = OnceLock::new();
    BASE.get_or_init(build_base)
}

fn build_base() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    // Bundled Adobe Source fonts (SIL OFL 1.1), full Latin coverage incl. Vietnamese:
    // Source Sans 3 for the UI, Source Serif 4 for reading (Markdown), Source Code Pro for code.
    let add = |fonts: &mut FontDefinitions, key: &str, data: &'static [u8]| {
        fonts.font_data.insert(key.into(), Arc::new(FontData::from_static(data)));
    };
    add(&mut fonts, "sans", include_bytes!("../assets/fonts/SourceSans3-Regular.otf"));
    add(&mut fonts, "sans-bold", include_bytes!("../assets/fonts/SourceSans3-Semibold.otf"));
    add(&mut fonts, "serif", include_bytes!("../assets/fonts/SourceSerif4-Regular.otf"));
    add(&mut fonts, "serif-bold", include_bytes!("../assets/fonts/SourceSerif4-Semibold.otf"));
    add(&mut fonts, "serif-italic", include_bytes!("../assets/fonts/SourceSerif4-It.otf"));
    add(&mut fonts, "mono", include_bytes!("../assets/fonts/SourceCodePro-Regular.otf"));
    let defaults_prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let defaults_mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let with = |first: &str, rest: &Vec<String>| [vec![first.to_string()], rest.clone()].concat();
    fonts.families.insert(FontFamily::Proportional, with("sans", &defaults_prop));
    fonts.families.insert(FontFamily::Monospace, with("mono", &defaults_mono));
    for (fam, key) in [("bold", "sans-bold"), ("serif", "serif"), ("serif-bold", "serif-bold"), ("serif-italic", "serif-italic")] {
        fonts.families.insert(FontFamily::Name(fam.into()), with(key, &defaults_prop));
    }
    // Terminal: JetBrains Mono (OFL), then the Nerd Font symbols (MIT) for the icons and
    // Powerline glyphs that oh-my-zsh / powerlevel10k prompts use.
    add(&mut fonts, "term", include_bytes!("../assets/fonts/JetBrainsMonoNL-Regular.ttf"));
    add(&mut fonts, "term-bold", include_bytes!("../assets/fonts/JetBrainsMonoNL-Bold.ttf"));
    add(&mut fonts, "nerd-symbols", include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf"));
    // Wider symbol coverage so prompts never show empty boxes: DejaVu Sans Mono (dingbats,
    // technical symbols) and Noto Sans Symbols 2 (triangle arrows U+2B00…, braille spinners,
    // geometric shapes and arrows in U+1F780…U+1F8FF).
    add(&mut fonts, "dejavu-mono", include_bytes!("../assets/fonts/DejaVuSansMono.ttf"));
    add(&mut fonts, "noto-symbols2", include_bytes!("../assets/fonts/NotoSansSymbols2-Regular.ttf"));
    let mut term_fallback = vec!["nerd-symbols".to_string(), "dejavu-mono".to_string(), "noto-symbols2".to_string()];
    // Last resort: the system's own symbol font, when it is there.
    for path in [
        "/System/Library/Fonts/Apple Symbols.ttf",
        "C:\\Windows\\Fonts\\seguisym.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            add(&mut fonts, "sys-symbols", Box::leak(bytes.into_boxed_slice()));
            term_fallback.push("sys-symbols".to_string());
            break;
        }
    }
    for (fam, key) in [("term", "term"), ("term-bold", "term-bold")] {
        let list = [vec![key.to_string()], term_fallback.clone(), with("mono", &defaults_mono)].concat();
        fonts.families.insert(FontFamily::Name(fam.into()), list);
    }
    // The interface shows key symbols (⌃ ⌥ ⇧ ⌘ ⌫ ⇥) that its own fonts lack.
    for family in ui_families() {
        let list = fonts.families.entry(family).or_default();
        list.push("noto-symbols2".to_string());
        list.push("dejavu-mono".to_string());
    }
    // Optional system fallback fonts for CJK and other scripts in file names.
    let candidates = [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\YuGothM.ttc",
        "C:\\Windows\\Fonts\\malgun.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];
    for c in candidates {
        if let Ok(bytes) = std::fs::read(c) {
            add(&mut fonts, "fallback", Box::leak(bytes.into_boxed_slice()));
            for fam in ui_families().chain(term_families()) {
                fonts.families.entry(fam).or_default().push("fallback".into());
            }
            break;
        }
    }
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(d: &FontDefinitions, family: FontFamily) -> Vec<String> {
        d.families[&family].clone()
    }

    #[test]
    fn scaling_leaves_the_terminal_alone() {
        let mut d = base().clone();
        let term_before = keys(&d, FontFamily::Name("term".into()));
        scale_interface(&mut d, 1.5);
        assert_eq!(keys(&d, FontFamily::Name("term".into())), term_before);
        assert_eq!(keys(&d, FontFamily::Proportional)[0], "sans@ui");
        assert_eq!(d.font_data["sans@ui"].tweak.scale, 1.5);
        assert_eq!(d.font_data["sans"].tweak.scale, 1.0);
    }

    #[test]
    fn builtin_and_unknown_fonts() {
        let mut d = base().clone();
        assert_eq!(user_font(&mut d, "ui", "builtin:serif", 600), Some(("serif".into(), "serif-bold".into())));
        assert_eq!(user_font(&mut d, "term", "No Such Font 123", 700), None);
        assert_eq!(label("", &TERM_BUILTIN), "JetBrains Mono");
        assert_eq!(label("Menlo", &TERM_BUILTIN), "Menlo");
    }
}
