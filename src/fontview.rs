//! Font previews: a sample sheet drawn from the file's own outlines, so looking at a
//! font never installs it or touches the app's fonts, and its names and properties.
use crate::i18n::t;
use crate::util::{ext_of, human_size, Res};
use resvg::tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};
use ttf_parser::{name_id, Face, GlyphId, OutlineBuilder};

pub fn is_font(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2")
}

pub struct FontPreview {
    pub format: String,
    /// White text on transparency, to be tinted with the theme's text colour.
    pub sheet: Option<(Vec<u8>, usize, usize)>,
    pub rows: Vec<(String, String)>,
}

const SHEET_W: u32 = 1100;

pub fn load(name: &str, bytes: &[u8]) -> Res<FontPreview> {
    let (sfnt, wrapper): (std::borrow::Cow<[u8]>, Option<&str>) = match bytes.get(..4) {
        Some(b"wOF2") => (wuff::decompress_woff2(bytes).map_err(|e| format!("WOFF2: {e:?}"))?.into(), Some("WOFF2")),
        Some(b"wOFF") => (wuff::decompress_woff1(bytes).map_err(|e| format!("WOFF: {e:?}"))?.into(), Some("WOFF")),
        _ => (bytes.into(), None),
    };
    let collection = ttf_parser::fonts_in_collection(&sfnt);
    let face = Face::parse(&sfnt, 0).map_err(|e| format!("{}: {e}", t("Not a usable font")))?;
    let outlines = if face.tables().cff.is_some() || face.tables().cff2.is_some() { "OpenType CFF" } else { "TrueType" };
    let format = match wrapper {
        Some(w) => format!("{w} · {outlines}"),
        None => outlines.to_string(),
    };
    let lookup = |id: u16| {
        let names = face.names();
        let mut fallback = None;
        for n in names.into_iter().filter(|n| n.name_id == id) {
            if let Some(s) = n.to_string() {
                if n.language() == ttf_parser::Language::English_UnitedStates {
                    return Some(s);
                }
                fallback.get_or_insert(s);
            }
        }
        fallback
    };
    let family = lookup(name_id::TYPOGRAPHIC_FAMILY).or_else(|| lookup(name_id::FAMILY)).unwrap_or_else(|| name.to_string());
    let mut rows = vec![(t("Family"), family.clone())];
    let mut add = |label: String, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            rows.push((label, v));
        }
    };
    add(t("Style"), lookup(name_id::TYPOGRAPHIC_SUBFAMILY).or_else(|| lookup(name_id::SUBFAMILY)));
    add(t("Version"), lookup(name_id::VERSION));
    add(t("Designer"), lookup(name_id::DESIGNER));
    add(t("Maker"), lookup(name_id::MANUFACTURER));
    add(t("Glyphs"), Some(crate::table::fmt_count(u64::from(face.number_of_glyphs()))));
    add(t("Weight"), Some(face.weight().to_number().to_string()));
    add(t("Monospaced"), Some(if face.is_monospaced() { t("Yes") } else { t("No") }));
    if face.is_variable() {
        add(t("Variable axes"), Some(face.variation_axes().into_iter().map(|a| a.tag.to_string()).collect::<Vec<_>>().join(", ")));
    }
    if let Some(n) = collection.filter(|n| *n > 1) {
        add(t("Fonts in this file"), Some(n.to_string()));
    }
    if wrapper.is_some() {
        add(t("Uncompressed size"), Some(human_size(sfnt.len() as u64)));
    }
    add(t("License"), lookup(name_id::LICENSE).map(|l| l.chars().take(300).collect()));
    Ok(FontPreview { format, sheet: sample_sheet(&face, &family), rows })
}

struct Outline(PathBuilder, f32, f32, f32);

impl Outline {
    fn x(&self, x: f32) -> f32 {
        self.1 + x * self.3
    }
    fn y(&self, y: f32) -> f32 {
        self.2 - y * self.3
    }
}

impl OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = (self.x(x), self.y(y));
        self.0.move_to(p.0, p.1);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = (self.x(x), self.y(y));
        self.0.line_to(p.0, p.1);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p = (self.x(x1), self.y(y1), self.x(x), self.y(y));
        self.0.quad_to(p.0, p.1, p.2, p.3);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p = (self.x(x1), self.y(y1), self.x(x2), self.y(y2), self.x(x), self.y(y));
        self.0.cubic_to(p.0, p.1, p.2, p.3, p.4, p.5);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

/// Lines of (size, text) broken at spaces to fit the sheet. Text the font lacks is left out.
fn layout_lines(face: &Face, family: &str) -> Vec<(f32, Vec<GlyphId>)> {
    let mut samples: Vec<(f32, String)> = vec![
        (46.0, family.to_string()),
        (28.0, "The quick brown fox jumps over the lazy dog".to_string()),
        (24.0, "ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_string()),
        (24.0, "abcdefghijklmnopqrstuvwxyz".to_string()),
        (24.0, "0123456789 !?&@#$%*()[]{}".to_string()),
    ];
    if "ệữơ".chars().all(|c| face.glyph_index(c).is_some()) {
        samples.push((24.0, "Tiếng Việt: Đường phố ngữ âm".to_string()));
    }
    for size in [18.0, 14.0, 11.0] {
        samples.push((size, "Sphinx of black quartz, judge my vow. 1234567890".to_string()));
    }
    let upem = f32::from(face.units_per_em().max(1));
    let width = (SHEET_W - 48) as f32;
    let mut lines = vec![];
    for (size, text) in samples {
        let advance = |c: char| face.glyph_index(c).map_or(0.0, |g| f32::from(face.glyph_hor_advance(g).unwrap_or(0)) * size / upem);
        let (mut line, mut x) = (vec![], 0.0);
        for word in text.split_inclusive(' ') {
            let w: f32 = word.chars().map(advance).sum();
            if x + w > width && !line.is_empty() {
                lines.push((size, std::mem::take(&mut line)));
                x = 0.0;
            }
            line.extend(word.chars().filter_map(|c| face.glyph_index(c)));
            x += w;
        }
        if !line.is_empty() {
            lines.push((size, line));
        }
    }
    lines
}

fn sample_sheet(face: &Face, family: &str) -> Option<(Vec<u8>, usize, usize)> {
    let upem = f32::from(face.units_per_em().max(1));
    let has_latin = face.glyph_index('a').is_some();
    let lines = if has_latin { layout_lines(face, family) } else { vec![] };
    let line_h = |size: f32| (size * 1.5).ceil();
    // A symbol font: its first glyphs in a grid instead.
    const CELL: f32 = 54.0;
    let grid: Vec<GlyphId> = if has_latin { vec![] } else { (1..face.number_of_glyphs().min(2000)).map(GlyphId).filter(|g| face.outline_glyph(*g, &mut Outline(PathBuilder::new(), 0.0, 0.0, 0.0)).is_some()).take(160).collect() };
    let per_row = ((SHEET_W - 48) as f32 / CELL) as usize;
    let height = if has_latin { lines.iter().map(|(s, _)| line_h(*s)).sum::<f32>() + 40.0 } else { grid.len().div_ceil(per_row) as f32 * CELL + 40.0 };
    if lines.is_empty() && grid.is_empty() {
        return None;
    }
    let mut pixmap = Pixmap::new(SHEET_W, height.ceil() as u32)?;
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    let mut draw_any = false;
    let mut draw = |g: GlyphId, x: f32, baseline: f32, size: f32, pixmap: &mut Pixmap| {
        let scale = size / upem;
        let mut o = Outline(PathBuilder::new(), x, baseline, scale);
        if face.outline_glyph(g, &mut o).is_some() {
            if let Some(path) = o.0.finish() {
                pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                draw_any = true;
            }
        }
        f32::from(face.glyph_hor_advance(g).unwrap_or(0)) * scale
    };
    let mut y = 20.0;
    for (size, glyphs) in &lines {
        let baseline = y + size * 1.15;
        let mut x = 24.0;
        for g in glyphs {
            x += draw(*g, x, baseline, *size, &mut pixmap);
        }
        y += line_h(*size);
    }
    for (i, g) in grid.iter().enumerate() {
        let (cx, cy) = ((i % per_row) as f32, (i / per_row) as f32);
        draw(*g, 24.0 + cx * CELL + 8.0, 20.0 + cy * CELL + 38.0, 32.0, &mut pixmap);
    }
    if !draw_any {
        return None;
    }
    let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
    Some((pixmap.take(), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTF: &[u8] = include_bytes!("../assets/fonts/JetBrainsMonoNL-Regular.ttf");
    const OTF: &[u8] = include_bytes!("../assets/fonts/SourceSans3-Regular.otf");
    const SYMBOLS: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

    fn field<'a>(p: &'a FontPreview, label: &str) -> Option<&'a str> {
        p.rows.iter().find(|(l, _)| l == label).map(|(_, v)| v.as_str())
    }

    #[test]
    fn names_and_a_drawn_sheet() {
        let p = load("a.ttf", TTF).unwrap();
        assert_eq!(p.format, "TrueType");
        assert_eq!(field(&p, "Family"), Some("JetBrains Mono NL"));
        assert_eq!(field(&p, "Monospaced"), Some("Yes"));
        let (rgba, w, h) = p.sheet.unwrap();
        assert_eq!(rgba.len(), w * h * 4);
        assert!(rgba.chunks_exact(4).filter(|px| px[3] > 128).count() > 5000, "text was drawn");
        let p = load("b.otf", OTF).unwrap();
        assert_eq!(p.format, "OpenType CFF");
        assert_eq!(field(&p, "Monospaced"), Some("No"));
        assert!(p.sheet.is_some());
    }

    #[test]
    fn symbol_fonts_show_a_grid() {
        let p = load("s.ttf", SYMBOLS).unwrap();
        let (rgba, ..) = p.sheet.unwrap();
        assert!(rgba.chunks_exact(4).any(|px| px[3] > 128));
    }

    #[test]
    fn not_a_font() {
        assert!(load("x.ttf", b"nonsense").is_err());
        assert!(load("x.woff2", b"wOF2 nonsense").is_err());
        assert!(load("x.ttf", b"").is_err());
    }
}
