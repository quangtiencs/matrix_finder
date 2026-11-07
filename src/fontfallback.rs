//! On-demand system font fallback, the way native terminals do it: when text
//! contains a character none of the loaded fonts can draw (e.g. the `ᐅ` prompt
//! arrow of oh-my-zsh's `muse` theme, a Canadian Syllabics letter only system
//! fonts such as Euphemia UCAS, Gadugi or DejaVu Sans have), a background
//! thread searches the installed fonts for one that covers it and adds that
//! font as the lowest-priority fallback of every font family.
use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use resvg::usvg::fontdb;
use std::collections::HashSet;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};

struct Resolver {
    /// Characters already sent to the worker (found or not), so each is looked up once.
    asked: HashSet<char>,
    tx: Sender<Vec<char>>,
}

static RESOLVER: OnceLock<Mutex<Resolver>> = OnceLock::new();

/// Ask for fallback fonts covering `missing` (characters `has_glyph` said no to).
/// Returns immediately; the fonts take effect from a later frame, which is repainted.
pub fn request(ctx: &egui::Context, missing: impl IntoIterator<Item = char>) {
    let r = RESOLVER.get_or_init(|| {
        let (tx, rx) = channel::<Vec<char>>();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("font-fallback".into())
            .spawn(move || {
                let mut w = Worker::new();
                while let Ok(mut chars) = rx.recv() {
                    // Take whatever else queued up meanwhile in the same scan.
                    while let Ok(more) = rx.try_recv() {
                        chars.extend(more);
                    }
                    if w.resolve(&ctx, chars) {
                        ctx.request_repaint();
                    }
                }
            })
            .ok();
        Mutex::new(Resolver { asked: HashSet::new(), tx })
    });
    let Ok(mut r) = r.lock() else { return };
    let new: Vec<char> = missing.into_iter().filter(|c| !c.is_control() && r.asked.insert(*c)).collect();
    if !new.is_empty() {
        let _ = r.tx.send(new);
    }
}

/// A font added so far; `fonts::apply` keeps it when it rebuilds the font list.
#[derive(Clone)]
pub struct Added {
    pub name: String,
    pub data: &'static [u8],
    pub index: u32,
}

static ADDED: Mutex<Vec<Added>> = Mutex::new(Vec::new());

pub fn added() -> Vec<Added> {
    ADDED.lock().map(|a| a.clone()).unwrap_or_default()
}

struct Worker {
    db: &'static fontdb::Database,
    /// Candidate faces, most suitable first.
    order: Vec<fontdb::ID>,
    added: HashSet<fontdb::ID>,
}

impl Worker {
    fn new() -> Self {
        let db = crate::fonts::db();
        let mut faces: Vec<&fontdb::FaceInfo> = db
            .faces()
            .filter(|f| {
                // Apple's LastResort font "covers" everything with placeholder boxes.
                !f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case("LastResort")) && !f.post_script_name.starts_with("LastResort")
            })
            .collect();
        // Upright regular faces first, monospaced ones before proportional; then the
        // rest, so a bold or italic face is still better than a box.
        let rank = |f: &fontdb::FaceInfo| {
            let regular = f.style == fontdb::Style::Normal && f.weight == fontdb::Weight::NORMAL;
            (!regular, !f.monospaced)
        };
        faces.sort_by_key(|f| (rank(f), f.families.first().map(|(n, _)| n.clone()).unwrap_or_default()));
        let order = faces.iter().map(|f| f.id).collect();
        Worker { db, order, added: HashSet::new() }
    }

    /// Add fonts until every char in `chars` is covered or no installed font has it.
    /// Returns whether any font was added.
    fn resolve(&mut self, ctx: &egui::Context, chars: Vec<char>) -> bool {
        let mut pending: Vec<char> = chars;
        let mut any = false;
        for &id in &self.order {
            if pending.is_empty() {
                break;
            }
            if self.added.contains(&id) {
                continue;
            }
            let covered: Vec<char> = self
                .db
                .with_face_data(id, |data, index| {
                    let Ok(face) = ttf_parser::Face::parse(data, index) else { return Vec::new() };
                    pending.iter().copied().filter(|&c| draws(&face, c)).collect()
                })
                .unwrap_or_default();
            if covered.is_empty() {
                continue;
            }
            let Some((bytes, index)) = self.db.with_face_data(id, |data, index| (data.to_vec(), index)) else { continue };
            let name = self.db.face(id).map(|f| f.post_script_name.clone()).unwrap_or_default();
            add_font(ctx, format!("system-fallback:{name}:{index}"), bytes, index);
            self.added.insert(id);
            any = true;
            pending.retain(|c| !covered.contains(c));
        }
        any
    }
}

/// Whether `face` has a real outline for `c` (not just a cmap entry pointing at an
/// empty or bitmap-only glyph, as in color emoji fonts that egui cannot draw).
pub fn draws(face: &ttf_parser::Face, c: char) -> bool {
    struct Sink;
    impl ttf_parser::OutlineBuilder for Sink {
        fn move_to(&mut self, _: f32, _: f32) {}
        fn line_to(&mut self, _: f32, _: f32) {}
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
        fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
        fn close(&mut self) {}
    }
    face.glyph_index(c).is_some_and(|g| face.outline_glyph(g, &mut Sink).is_some())
}

fn add_font(ctx: &egui::Context, name: String, bytes: Vec<u8>, index: u32) {
    let added = Added { name: name.clone(), data: Box::leak(bytes.into_boxed_slice()), index };
    if let Ok(mut all) = ADDED.lock() {
        all.push(added.clone());
    }
    let mut data = egui::FontData::from_static(added.data);
    data.index = index;
    let insert = |name: String, data: egui::FontData, families: &[egui::FontFamily]| FontInsert {
        name,
        data,
        families: families.iter().map(|f| InsertFontFamily { family: f.clone(), priority: FontPriority::Lowest }).collect(),
    };
    // The terminal has its own size; the interface's fonts are scaled.
    let term = [egui::FontFamily::Name("term".into()), egui::FontFamily::Name("term-bold".into())];
    let ui = [
        egui::FontFamily::Proportional,
        egui::FontFamily::Monospace,
        egui::FontFamily::Name("bold".into()),
        egui::FontFamily::Name("serif".into()),
        egui::FontFamily::Name("serif-bold".into()),
        egui::FontFamily::Name("serif-italic".into()),
    ];
    let scale = crate::fonts::ui_scale();
    ctx.add_font(insert(format!("{name}@ui"), crate::fonts::scaled(&data, scale), &ui));
    ctx.add_font(insert(name, data, &term));
}

#[cfg(test)]
mod tests {
    #[test]
    fn draws_needs_an_outline() {
        let data = include_bytes!("../assets/fonts/JetBrainsMonoNL-Regular.ttf");
        let face = ttf_parser::Face::parse(data, 0).unwrap();
        assert!(super::draws(&face, '\u{279C}')); // ➜ is in JetBrains Mono
        assert!(!super::draws(&face, '\u{1405}')); // ᐅ (oh-my-zsh muse prompt) is not
    }
}
