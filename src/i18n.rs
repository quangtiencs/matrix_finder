//! Tiny i18n layer. Keys are the English strings themselves, so English needs
//! no table. Other languages are a key → translation map: Vietnamese is built
//! in, and any number of extra languages can be dropped into the config
//! folder as JSON files (`lang/<code>.json`).
use crate::util::{es, Res};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, RwLock};

static CURRENT: LazyLock<RwLock<HashMap<String, String>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

#[derive(Serialize, Deserialize)]
pub struct LangFile {
    pub code: String,
    pub name: String,
    pub strings: HashMap<String, String>,
}

/// Translate a UI string.
pub fn t(key: &str) -> String {
    CURRENT
        .read()
        .ok()
        .and_then(|m| m.get(key).cloned())
        .unwrap_or_else(|| key.to_string())
}

/// (code, display name) of every available language.
pub fn available() -> Vec<(String, String)> {
    let mut v = vec![("en".to_string(), "English".to_string()), ("vi".to_string(), "Tiếng Việt".to_string())];
    if let Ok(rd) = std::fs::read_dir(crate::config::lang_dir()) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                if let Ok(lf) = load_file(&e.path()) {
                    if !v.iter().any(|(c, _)| *c == lf.code) {
                        v.push((lf.code, lf.name));
                    }
                }
            }
        }
    }
    v
}

fn load_file(p: &Path) -> Res<LangFile> {
    let s = std::fs::read_to_string(p).map_err(es)?;
    serde_json::from_str(&s).map_err(es)
}

pub fn set_language(code: &str) {
    let map: HashMap<String, String> = match code {
        "en" => HashMap::new(),
        "vi" => VI.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        other => {
            let path = crate::config::lang_dir().join(format!("{other}.json"));
            let from_file = load_file(&path).ok().or_else(|| {
                // Fall back to scanning files whose `code` matches.
                std::fs::read_dir(crate::config::lang_dir())
                    .ok()?
                    .flatten()
                    .filter_map(|e| load_file(&e.path()).ok())
                    .find(|l| l.code == other)
            });
            from_file.map(|l| l.strings).unwrap_or_default()
        }
    };
    if let Ok(mut m) = CURRENT.write() {
        *m = map;
    }
}

/// Write a template containing every UI string (with the Vietnamese
/// translation as a hint) so users can add a new language.
pub fn export_template(path: &Path) -> Res<()> {
    let strings: HashMap<String, String> = KEYS.iter().map(|k| (k.to_string(), k.to_string())).collect();
    let lf = LangFile { code: "xx".into(), name: "My language".into(), strings };
    let json = serde_json::to_string_pretty(&lf).map_err(es)?;
    std::fs::write(path, json).map_err(es)
}

include!("i18n_data.rs");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vietnamese_covers_all_keys() {
        let vi: std::collections::HashSet<&str> = VI.iter().map(|(k, _)| *k).collect();
        let missing: Vec<&&str> = KEYS.iter().filter(|k| !vi.contains(**k)).collect();
        assert!(missing.is_empty(), "missing vi translations: {missing:?}");
    }

    /// Every translated string literal in the sources must be listed in KEYS.
    #[test]
    fn keys_cover_sources() {
        let sources = [
            include_str!("app.rs"),
            include_str!("pane.rs"),
            include_str!("preview.rs"),
            include_str!("backup.rs"),
            include_str!("diskusage.rs"),
            include_str!("palette.rs"),
            include_str!("table.rs"),
            include_str!("rename.rs"),
            include_str!("docview.rs"),
            include_str!("tabs.rs"),
            include_str!("termview.rs"),
            include_str!("chrome.rs"),
            include_str!("checksum.rs"),
            include_str!("settings_ui.rs"),
            include_str!("media.rs"),
            include_str!("textdocs.rs"),
            include_str!("fontview.rs"),
            include_str!("certview.rs"),
            include_str!("model3d.rs"),
            include_str!("scidata.rs"),
            include_str!("tensors.rs"),
            include_str!("rdata.rs"),
            include_str!("archive.rs"),
            include_str!("shortcuts.rs"),
        ];
        let keys: std::collections::HashSet<&str> = KEYS.iter().copied().collect();
        let mut missing = vec![];
        for src in sources {
            let b = src.as_bytes();
            let mut i = 0;
            while let Some(pos) = src[i..].find("t(\"") {
                let start = i + pos;
                i = start + 3;
                if start > 0 && (b[start - 1].is_ascii_alphanumeric() || b[start - 1] == b'_') {
                    continue;
                }
                let mut j = i;
                while j < b.len() && !(b[j] == b'"' && b[j - 1] != b'\\') {
                    j += 1;
                }
                let key = &src[i..j];
                if !keys.contains(key) {
                    missing.push(key.to_string());
                }
            }
        }
        assert!(missing.is_empty(), "add these to i18n_data.rs: {missing:?}");
    }
}
