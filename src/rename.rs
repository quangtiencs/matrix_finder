//! Smart batch rename: name masks with tokens, find & replace (plain or
//! regex), case conversion, Vietnamese diacritics removal, whitespace clean-up
//! and counters. Renames are applied in two phases so swaps (a↔b) are safe.
use crate::util::Res;
use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaseMode {
    Keep,
    Lower,
    Upper,
    Title,
    Sentence,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpaceMode {
    Keep,
    Underscore,
    Dash,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    /// Mask for the name without extension, e.g. `[N]`, `IMG_[D]_[C]`.
    pub name_mask: String,
    /// Mask for the extension, e.g. `[E]` (keep) or `jpg`.
    pub ext_mask: String,
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub case: CaseMode,
    pub strip_diacritics: bool,
    pub spaces: SpaceMode,
    pub collapse_spaces: bool,
    /// Keep only letters and digits, joined by single dashes (after other rules).
    pub slug: bool,
    pub counter_start: i64,
    pub counter_step: i64,
    pub counter_digits: usize,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            name_mask: "[N]".into(),
            ext_mask: "[E]".into(),
            find: String::new(),
            replace: String::new(),
            regex: false,
            case_sensitive: false,
            case: CaseMode::Keep,
            strip_diacritics: false,
            spaces: SpaceMode::Keep,
            collapse_spaces: true,
            slug: false,
            counter_start: 1,
            counter_step: 1,
            counter_digits: 3,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub is_dir: bool,
    pub modified: Option<SystemTime>,
}

/// Split into (stem, extension) — directories and dot-files have no extension.
pub fn split_name(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

/// Remove Vietnamese (and other Latin) diacritics: "Thư mục Đà Nẵng" → "Thu muc Da Nang".
pub fn strip_diacritics(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .map(|c| match c {
            'đ' => 'd',
            'Đ' => 'D',
            c => c,
        })
        .collect()
}

fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

fn sentence_case(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut chars = lower.chars();
    let mut out = String::new();
    for c in chars.by_ref() {
        if c.is_alphabetic() {
            out.extend(c.to_uppercase());
            break;
        }
        out.push(c);
    }
    out.extend(chars);
    out
}

/// Expand the tokens of a mask for one item.
fn expand(mask: &str, stem: &str, ext: &str, counter: &str, item: &Item, parent: &str) -> String {
    let mut out = String::new();
    let mut rest = mask;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &after[..close];
        match expand_token(token, stem, ext, counter, item, parent) {
            Some(v) => out.push_str(&v),
            None => {
                out.push('[');
                out.push_str(token);
                out.push(']');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn expand_token(token: &str, stem: &str, ext: &str, counter: &str, item: &Item, parent: &str) -> Option<String> {
    let dt = item.modified.map(chrono::DateTime::<chrono::Local>::from);
    let date = |fmt: &str| dt.map(|d| d.format(fmt).to_string()).unwrap_or_default();
    Some(match token {
        "N" => stem.to_string(),
        "E" => ext.to_string(),
        "C" => counter.to_string(),
        "P" => parent.to_string(),
        "D" => date("%Y-%m-%d"),
        "T" => date("%H%M%S"),
        "Y" => date("%Y"),
        "M" => date("%m"),
        "d" => date("%d"),
        t if t.starts_with('N') => {
            // [N2-5] chars 2..=5, [N3-] from 3 to end, [N4] only char 4 (1-based).
            let spec = &t[1..];
            let chars: Vec<char> = stem.chars().collect();
            let (a, b) = match spec.split_once('-') {
                Some((a, "")) => (a.parse::<usize>().ok()?, chars.len()),
                Some((a, b)) => (a.parse::<usize>().ok()?, b.parse::<usize>().ok()?),
                None => {
                    let a = spec.parse::<usize>().ok()?;
                    (a, a)
                }
            };
            let a = a.max(1);
            if a > chars.len() {
                String::new()
            } else {
                chars[a - 1..b.min(chars.len()).max(a - 1)].iter().collect()
            }
        }
        _ => return None,
    })
}

/// Compute the new names. `Err` for an invalid regex (same for every item).
pub fn compute(items: &[Item], rules: &Rules, parent: &str) -> Result<Vec<String>, String> {
    let re = if rules.regex && !rules.find.is_empty() {
        Some(
            regex::RegexBuilder::new(&rules.find)
                .case_insensitive(!rules.case_sensitive)
                .build()
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let (stem, ext) = split_name(&item.name, item.is_dir);
        let n = rules.counter_start + rules.counter_step * i as i64;
        let counter = format!("{:0width$}", n, width = rules.counter_digits);
        let mut new_stem = expand(&rules.name_mask, stem, ext, &counter, item, parent);
        let mut new_ext = if item.is_dir { String::new() } else { expand(&rules.ext_mask, stem, ext, &counter, item, parent) };

        // Find & replace on the stem (the extension is controlled by its mask).
        if !rules.find.is_empty() {
            new_stem = match &re {
                Some(re) => re.replace_all(&new_stem, rules.replace.as_str()).to_string(),
                None if rules.case_sensitive => new_stem.replace(&rules.find, &rules.replace),
                None => replace_ci(&new_stem, &rules.find, &rules.replace),
            };
        }
        if rules.strip_diacritics {
            new_stem = strip_diacritics(&new_stem);
            new_ext = strip_diacritics(&new_ext);
        }
        new_stem = match rules.case {
            CaseMode::Keep => new_stem,
            CaseMode::Lower => new_stem.to_lowercase(),
            CaseMode::Upper => new_stem.to_uppercase(),
            CaseMode::Title => title_case(&new_stem),
            CaseMode::Sentence => sentence_case(&new_stem),
        };
        if matches!(rules.case, CaseMode::Lower | CaseMode::Upper) {
            new_ext = if rules.case == CaseMode::Lower { new_ext.to_lowercase() } else { new_ext.to_uppercase() };
        }
        if rules.collapse_spaces {
            new_stem = new_stem.split_whitespace().collect::<Vec<_>>().join(" ");
        }
        new_stem = match rules.spaces {
            SpaceMode::Keep => new_stem,
            SpaceMode::Underscore => new_stem.replace(' ', "_"),
            SpaceMode::Dash => new_stem.replace(' ', "-"),
            SpaceMode::Remove => new_stem.replace(' ', ""),
        };
        // Separators produced by the space rule collapse into one ("a - b" → "a-b").
        new_stem = match rules.spaces {
            SpaceMode::Dash => collapse_runs(&new_stem, '-'),
            SpaceMode::Underscore => collapse_runs(&new_stem, '_'),
            _ => new_stem,
        };
        if rules.slug {
            let sep = if rules.spaces == SpaceMode::Underscore { '_' } else { '-' };
            let mapped: String = new_stem.chars().map(|c| if c.is_alphanumeric() { c } else { sep }).collect();
            new_stem = collapse_runs(&mapped, sep).trim_matches(sep).to_string();
        }
        let new_ext = new_ext.trim().trim_start_matches('.').to_string();
        out.push(if new_ext.is_empty() { new_stem } else { format!("{new_stem}.{new_ext}") });
    }
    Ok(out)
}

fn collapse_runs(s: &str, sep: char) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_sep = false;
    for c in s.chars() {
        if c == sep {
            if !prev_sep {
                out.push(c);
            }
            prev_sep = true;
        } else {
            out.push(c);
            prev_sep = false;
        }
    }
    out
}

fn replace_ci(s: &str, find: &str, rep: &str) -> String {
    let lower = s.to_lowercase();
    let f = find.to_lowercase();
    if f.is_empty() || lower.len() != s.len() {
        // Lower-casing changed byte lengths: fall back to a regex-free char scan.
        return regex::RegexBuilder::new(&regex::escape(find))
            .case_insensitive(true)
            .build()
            .map(|re| re.replace_all(s, regex::NoExpand(rep)).to_string())
            .unwrap_or_else(|_| s.to_string());
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&f) {
        out.push_str(&s[i..i + pos]);
        out.push_str(rep);
        i += pos + f.len();
    }
    out.push_str(&s[i..]);
    out
}

/// Characters that are invalid in file names on at least one major OS.
fn invalid_name(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("empty name");
    }
    if name == "." || name == ".." {
        return Some("reserved name");
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0']) {
        return Some("invalid character");
    }
    None
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Unchanged,
    Ok,
    Error(String),
}

/// Validate a plan. `existing` holds every name currently in the folder.
pub fn validate(old: &[String], new: &[String], existing: &HashSet<String>, case_insensitive_fs: bool) -> Vec<Status> {
    let key = |s: &str| if case_insensitive_fs { s.to_lowercase() } else { s.to_string() };
    let moving: HashSet<String> = old.iter().map(|s| key(s)).collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for n in new {
        *counts.entry(key(n)).or_default() += 1;
    }
    old.iter()
        .zip(new)
        .map(|(o, n)| {
            if o == n {
                return Status::Unchanged;
            }
            if let Some(why) = invalid_name(n) {
                return Status::Error(why.into());
            }
            if counts.get(&key(n)).copied().unwrap_or(0) > 1 {
                return Status::Error("duplicate name".into());
            }
            // Taken by a file that is not part of this rename.
            let k = key(n);
            if existing.iter().any(|e| key(e) == k) && !moving.contains(&k) {
                return Status::Error("already exists".into());
            }
            Status::Ok
        })
        .collect()
}

/// Apply renames with `rename(from, to)`, going through temporary names so
/// that swaps and chains work. Returns the pairs actually applied (for undo).
pub fn apply(pairs: &[(String, String)], mut rename: impl FnMut(&str, &str) -> Res<()>) -> Res<Vec<(String, String)>> {
    let pairs: Vec<&(String, String)> = pairs.iter().filter(|(a, b)| a != b).collect();
    let stamp = std::process::id();
    let mut staged: Vec<(String, String, String)> = Vec::new(); // (orig, temp, target)
    for (i, (from, to)) in pairs.iter().enumerate() {
        let tmp = format!(".matrix_finder-rename-{stamp}-{i}.tmp");
        if let Err(e) = rename(from, &tmp) {
            // Roll back the ones already staged.
            for (orig, t, _) in staged.iter().rev() {
                let _ = rename(t, orig);
            }
            return Err(format!("{from}: {e}"));
        }
        staged.push((from.clone(), tmp, to.clone()));
    }
    let mut done = Vec::new();
    let mut errors = Vec::new();
    for (orig, tmp, target) in &staged {
        match rename(tmp, target) {
            Ok(()) => done.push((orig.clone(), target.clone())),
            Err(e) => {
                let _ = rename(tmp, orig);
                errors.push(format!("{orig} → {target}: {e}"));
            }
        }
    }
    if errors.is_empty() {
        Ok(done)
    } else {
        Err(errors.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(names: &[&str]) -> Vec<Item> {
        names.iter().map(|n| Item { name: n.to_string(), is_dir: false, modified: None }).collect()
    }

    #[test]
    fn masks_and_counter() {
        let r = Rules { name_mask: "Photo_[C]_[N]".into(), counter_digits: 2, ..Default::default() };
        let out = compute(&items(&["a.JPG", "b.jpg"]), &r, "trip").unwrap();
        assert_eq!(out, vec!["Photo_01_a.JPG", "Photo_02_b.jpg"]);
        let r = Rules { name_mask: "[P]-[N2-4]".into(), ext_mask: "txt".into(), ..Default::default() };
        assert_eq!(compute(&items(&["abcdef.md"]), &r, "docs").unwrap(), vec!["docs-bcd.txt"]);
        let r = Rules { name_mask: "[N3-]".into(), ..Default::default() };
        assert_eq!(compute(&items(&["xxreport.pdf"]), &r, "").unwrap(), vec!["report.pdf"]);
        let r = Rules { name_mask: "[Q] [N]".into(), ..Default::default() };
        assert_eq!(compute(&items(&["a.txt"]), &r, "").unwrap(), vec!["[Q] a.txt"]);
    }

    #[test]
    fn replace_case_and_vietnamese() {
        let r = Rules {
            find: "  bản  NHÁP ".into(),
            replace: " ".into(),
            strip_diacritics: true,
            case: CaseMode::Lower,
            spaces: SpaceMode::Dash,
            ..Default::default()
        };
        let out = compute(&items(&["Báo cáo  bản  nháp  Đà Nẵng.DOCX"]), &r, "").unwrap();
        assert_eq!(out, vec!["bao-cao-da-nang.docx"]);
        let r = Rules { case: CaseMode::Title, ..Default::default() };
        assert_eq!(compute(&items(&["hello wORLD-foo.txt"]), &r, "").unwrap(), vec!["Hello World-Foo.txt"]);
        assert_eq!(strip_diacritics("Thư mục tiếng Việt"), "Thu muc tieng Viet");
        let slug = Rules { strip_diacritics: true, case: CaseMode::Lower, spaces: SpaceMode::Dash, slug: true, ..Default::default() };
        let out = compute(&items(&["Báo cáo tháng 2 - (bản nháp)!.DOCX", "ảnh  hội an.jpg"]), &slug, "").unwrap();
        assert_eq!(out, vec!["bao-cao-thang-2-ban-nhap.docx", "anh-hoi-an.jpg"]);
        let dash = Rules { spaces: SpaceMode::Dash, ..Default::default() };
        assert_eq!(compute(&items(&["a - b.txt"]), &dash, "").unwrap(), vec!["a-b.txt"]);
    }

    #[test]
    fn regex_groups() {
        let r = Rules { find: r"(\d{4})-(\d{2})-(\d{2})".into(), replace: "$3.$2.$1".into(), regex: true, ..Default::default() };
        assert_eq!(compute(&items(&["log 2024-06-14.txt"]), &r, "").unwrap(), vec!["log 14.06.2024.txt"]);
        let bad = Rules { find: "(".into(), regex: true, ..Default::default() };
        assert!(compute(&items(&["a"]), &bad, "").is_err());
    }

    #[test]
    fn validation() {
        let old: Vec<String> = vec!["a.txt".into(), "b.txt".into(), "c.txt".into()];
        let new: Vec<String> = vec!["b.txt".into(), "a.txt".into(), "x.txt".into()];
        let existing: HashSet<String> = ["a.txt", "b.txt", "c.txt", "x.txt"].iter().map(|s| s.to_string()).collect();
        let st = validate(&old, &new, &existing, false);
        assert_eq!(st[0], Status::Ok); // swap is fine
        assert_eq!(st[1], Status::Ok);
        assert_eq!(st[2], Status::Error("already exists".into()));
        let st = validate(&old[..2], &["z".into(), "z".into()], &existing, false);
        assert!(matches!(st[0], Status::Error(_)));
        let st = validate(&old[..1], &["bad/name".into()], &existing, false);
        assert!(matches!(st[0], Status::Error(_)));
    }

    #[test]
    fn apply_swap_on_disk() {
        let dir = std::env::temp_dir().join(format!("matrix_finder-ren-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "A").unwrap();
        std::fs::write(dir.join("b"), "B").unwrap();
        let pairs = vec![("a".to_string(), "b".to_string()), ("b".to_string(), "a".to_string())];
        let d = dir.clone();
        let done = apply(&pairs, |f, t| std::fs::rename(d.join(f), d.join(t)).map_err(|e| e.to_string())).unwrap();
        assert_eq!(done.len(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("a")).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(dir.join("b")).unwrap(), "A");
        // Undo
        let undo: Vec<(String, String)> = done.iter().map(|(a, b)| (b.clone(), a.clone())).collect();
        apply(&undo, |f, t| std::fs::rename(d.join(f), d.join(t)).map_err(|e| e.to_string())).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a")).unwrap(), "A");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---------------------------------------------------------------- UI

use crate::i18n::t;
use crate::theme::Theme;
use crate::vfs::Location;

pub enum RenameAction {
    Apply(Location, Vec<(String, String)>),
    Undo(Location, Vec<(String, String)>),
}

pub struct BatchRename {
    pub open: bool,
    loc: Option<Location>,
    parent: String,
    items: Vec<Item>,
    existing: HashSet<String>,
    rules: Rules,
    /// Last applied batch, for undo: (location, [(old, new)]).
    pub last: Option<(Location, Vec<(String, String)>)>,
    focus: bool,
}

impl Default for BatchRename {
    fn default() -> Self {
        Self { open: false, loc: None, parent: String::new(), items: vec![], existing: HashSet::new(), rules: Rules::default(), last: None, focus: false }
    }
}

fn case_insensitive_fs() -> bool {
    cfg!(windows) || cfg!(target_os = "macos")
}

impl BatchRename {
    pub fn start(&mut self, loc: Location, parent: String, items: Vec<Item>, existing: HashSet<String>) {
        self.loc = Some(loc);
        self.parent = parent;
        self.items = items;
        self.existing = existing;
        self.open = true;
        self.focus = true;
    }

    /// After applying, the renamed items keep their new names in the list.
    pub fn applied(&mut self, loc: Location, done: Vec<(String, String)>) {
        let map: HashMap<&str, &str> = done.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        for it in &mut self.items {
            if let Some(n) = map.get(it.name.as_str()) {
                it.name = n.to_string();
            }
        }
        for (a, b) in &done {
            self.existing.remove(a);
            self.existing.insert(b.clone());
        }
        self.last = Some((loc, done));
        self.rules = Rules::default();
    }

    pub fn ui(&mut self, ctx: &egui::Context, th: &Theme) -> Option<RenameAction> {
        let mut action = None;
        let mut open = self.open;
        let names: Vec<String> = self.items.iter().map(|i| i.name.clone()).collect();
        let computed = compute(&self.items, &self.rules, &self.parent);
        let (new_names, regex_err) = match computed {
            Ok(v) => (v, None),
            Err(e) => (names.clone(), Some(e)),
        };
        let status = validate(&names, &new_names, &self.existing, case_insensitive_fs());
        let changes = status.iter().filter(|s| **s == Status::Ok).count();
        let errors = status.iter().filter(|s| matches!(s, Status::Error(_))).count();

        egui::Window::new(t("Batch rename"))
            .open(&mut open)
            .collapsible(false)
            .default_size([920.0, 600.0])
            // Limits the content only: leave room for the title bar and the frame too.
            .max_height(ctx.screen_rect().height() - 100.0)
            .min_width(640.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("{} {} · {}", self.items.len(), t("item(s)"), self.parent))
                        .size(11.5)
                        .color(th.weak),
                );
                // Presets
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(t("Presets")).size(11.5).color(th.weak));
                    let r = &mut self.rules;
                    if ui.button(t("Number sequentially")).on_hover_text("[N]_[C]").clicked() {
                        r.name_mask = "[N]_[C]".into();
                    }
                    if ui.button(t("Date prefix")).on_hover_text("[D] [N]").clicked() {
                        r.name_mask = "[D] [N]".into();
                    }
                    if ui.button(t("Photos by date")).on_hover_text("IMG_[Y][M][d]_[C]").clicked() {
                        r.name_mask = "IMG_[Y][M][d]_[C]".into();
                        r.counter_digits = 4;
                    }
                    if ui.button(t("Remove diacritics")).on_hover_text("Thư mục → Thu muc").clicked() {
                        r.strip_diacritics = true;
                    }
                    if ui.button(t("Web-safe (slug)")).on_hover_text("Báo Cáo 2024 → bao-cao-2024").clicked() {
                        r.strip_diacritics = true;
                        r.case = CaseMode::Lower;
                        r.spaces = SpaceMode::Dash;
                        r.collapse_spaces = true;
                        r.slug = true;
                    }
                    if ui.button(t("Title Case")).clicked() {
                        r.case = CaseMode::Title;
                    }
                    if ui.button(t("Reset")).clicked() {
                        *r = Rules::default();
                    }
                });
                ui.add_space(4.0);
                ui.columns(2, |cols| {
                    let r = &mut self.rules;
                    let ui = &mut cols[0];
                    egui::Grid::new("ren-left").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label(t("Name mask"));
                        let resp = ui.add(egui::TextEdit::singleline(&mut r.name_mask).desired_width(220.0).font(egui::TextStyle::Monospace));
                        if self.focus {
                            resp.request_focus();
                            self.focus = false;
                        }
                        ui.end_row();
                        ui.label("");
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (tok, tip) in [
                                ("[N]", t("original name")),
                                ("[N1-3]", t("characters 1 to 3")),
                                ("[C]", t("counter")),
                                ("[D]", t("modified date")),
                                ("[T]", t("modified time")),
                                ("[P]", t("parent folder")),
                                ("[E]", t("extension")),
                            ] {
                                if ui.small_button(egui::RichText::new(tok).monospace().size(11.5)).on_hover_text(tip).clicked() {
                                    r.name_mask.push_str(tok);
                                }
                            }
                        });
                        ui.end_row();
                        ui.label(t("Extension"));
                        ui.add(egui::TextEdit::singleline(&mut r.ext_mask).desired_width(120.0).font(egui::TextStyle::Monospace));
                        ui.end_row();
                        ui.label(t("Counter"));
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(t("start")).size(11.5).color(th.weak));
                            ui.add(egui::DragValue::new(&mut r.counter_start));
                            ui.label(egui::RichText::new(t("step")).size(11.5).color(th.weak));
                            ui.add(egui::DragValue::new(&mut r.counter_step).range(1..=1000));
                            ui.label(egui::RichText::new(t("digits")).size(11.5).color(th.weak));
                            ui.add(egui::DragValue::new(&mut r.counter_digits).range(1..=10));
                        });
                        ui.end_row();
                    });
                    let ui = &mut cols[1];
                    egui::Grid::new("ren-right").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label(t("Find"));
                        ui.add(egui::TextEdit::singleline(&mut r.find).desired_width(220.0));
                        ui.end_row();
                        ui.label(t("Replace with"));
                        ui.add(egui::TextEdit::singleline(&mut r.replace).desired_width(220.0));
                        ui.end_row();
                        ui.label("");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut r.regex, t("Regular expression")).on_hover_text("(\\d+)-(\\d+) → $2-$1");
                            ui.checkbox(&mut r.case_sensitive, t("Match case"));
                        });
                        ui.end_row();
                        ui.label(t("Letter case"));
                        egui::ComboBox::from_id_salt("ren-case").width(160.0).selected_text(case_label(r.case)).show_ui(ui, |ui| {
                            for c in [CaseMode::Keep, CaseMode::Lower, CaseMode::Upper, CaseMode::Title, CaseMode::Sentence] {
                                ui.selectable_value(&mut r.case, c, case_label(c));
                            }
                        });
                        ui.end_row();
                        ui.label(t("Spaces"));
                        egui::ComboBox::from_id_salt("ren-space").width(160.0).selected_text(space_label(r.spaces)).show_ui(ui, |ui| {
                            for c in [SpaceMode::Keep, SpaceMode::Underscore, SpaceMode::Dash, SpaceMode::Remove] {
                                ui.selectable_value(&mut r.spaces, c, space_label(c));
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut r.strip_diacritics, t("Remove diacritics"));
                            ui.checkbox(&mut r.collapse_spaces, t("Collapse spaces"));
                        });
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut r.slug, t("Letters and digits only"))
                            .on_hover_text("Báo cáo (bản nháp) → bao-cao-ban-nhap");
                        ui.end_row();
                    });
                });
                if let Some(e) = &regex_err {
                    ui.colored_label(th.danger, format!("⚠ {e}"));
                }
                ui.add_space(6.0);

                // Preview
                let footer_h = 44.0;
                let table_h = (ui.available_height() - footer_h).clamp(140.0, 340.0);
                egui::Frame::NONE.fill(th.bg).corner_radius(8).inner_margin(6.0).show(ui, |ui| {
                    ui.set_height(table_h);
                    use egui_extras::{Column, TableBuilder};
                    let w = ui.available_width();
                    TableBuilder::new(ui)
                        .striped(true)
                        .auto_shrink([false, false])
                        .max_scroll_height(table_h)
                        .column(Column::exact((w - 150.0) * 0.5))
                        .column(Column::exact(24.0))
                        .column(Column::exact((w - 150.0) * 0.5))
                        .column(Column::remainder())
                        .header(24.0, |mut h| {
                            h.col(|ui| {
                                ui.label(egui::RichText::new(t("Current name")).size(11.5).color(th.weak));
                            });
                            h.col(|_| {});
                            h.col(|ui| {
                                ui.label(egui::RichText::new(t("New name")).size(11.5).color(th.weak));
                            });
                            h.col(|_| {});
                        })
                        .body(|body| {
                            body.rows(24.0, names.len(), |mut row| {
                                let i = row.index();
                                let st = &status[i];
                                row.col(|ui| {
                                    ui.add(egui::Label::new(egui::RichText::new(&names[i]).color(th.text)).truncate());
                                });
                                row.col(|ui| {
                                    ui.label(egui::RichText::new("→").color(th.weak));
                                });
                                row.col(|ui| {
                                    let color = match st {
                                        Status::Unchanged => th.weak,
                                        Status::Ok => th.accent,
                                        Status::Error(_) => th.danger,
                                    };
                                    ui.add(egui::Label::new(egui::RichText::new(&new_names[i]).color(color)).truncate());
                                });
                                row.col(|ui| {
                                    if let Status::Error(why) = st {
                                        ui.label(egui::RichText::new(format!("⚠ {}", t(why))).size(11.0).color(th.danger));
                                    }
                                });
                            });
                        });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let summary = if errors > 0 {
                        format!("{changes} {} · {errors} {}", t("to rename"), t("conflicts"))
                    } else {
                        format!("{changes} {}", t("to rename"))
                    };
                    ui.label(egui::RichText::new(summary).color(if errors > 0 { th.danger } else { th.weak }));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can = changes > 0 && errors == 0 && regex_err.is_none();
                        let b = egui::Button::new(egui::RichText::new(t("Rename")).strong().color(th.on_accent()))
                            .fill(if can { th.accent } else { th.surface })
                            .min_size(egui::vec2(110.0, 30.0));
                        if ui.add_enabled(can, b).clicked() {
                            if let Some(loc) = &self.loc {
                                let pairs: Vec<(String, String)> = names
                                    .iter()
                                    .zip(&new_names)
                                    .zip(&status)
                                    .filter(|(_, s)| **s == Status::Ok)
                                    .map(|((a, b), _)| (a.clone(), b.clone()))
                                    .collect();
                                action = Some(RenameAction::Apply(loc.clone(), pairs));
                            }
                        }
                        if let Some((loc, done)) = &self.last {
                            if ui.add(egui::Button::new(t("Undo last rename")).min_size(egui::vec2(0.0, 30.0))).clicked() {
                                let back: Vec<(String, String)> = done.iter().map(|(a, b)| (b.clone(), a.clone())).collect();
                                action = Some(RenameAction::Undo(loc.clone(), back));
                            }
                        }
                    });
                });
            });
        self.open = open;
        action
    }

    /// After an undo, restore the list to the original names.
    pub fn undone(&mut self, back: &[(String, String)]) {
        let map: HashMap<&str, &str> = back.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        for it in &mut self.items {
            if let Some(n) = map.get(it.name.as_str()) {
                it.name = n.to_string();
            }
        }
        for (a, b) in back {
            self.existing.remove(a);
            self.existing.insert(b.clone());
        }
        self.last = None;
    }
}

fn case_label(c: CaseMode) -> String {
    match c {
        CaseMode::Keep => t("Unchanged"),
        CaseMode::Lower => t("lowercase"),
        CaseMode::Upper => t("UPPERCASE"),
        CaseMode::Title => t("Title Case"),
        CaseMode::Sentence => t("Sentence case"),
    }
}

fn space_label(c: SpaceMode) -> String {
    match c {
        SpaceMode::Keep => t("Unchanged"),
        SpaceMode::Underscore => t("Spaces → _"),
        SpaceMode::Dash => t("Spaces → -"),
        SpaceMode::Remove => t("Remove spaces"),
    }
}
