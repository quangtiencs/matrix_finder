//! Freedesktop icon-theme lookup (Linux/BSD), so file lists can use the
//! desktop's real icons (Adwaita, Breeze, Papirus, …).
//! Spec: https://specifications.freedesktop.org/icon-theme-spec/
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct IconTheme {
    pub name: String,
    /// Directories of this theme and the themes it inherits, best first.
    dirs: Vec<PathBuf>,
}

fn base_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".icons"));
    }
    if let Some(d) = dirs::data_dir() {
        v.push(d.join("icons"));
    }
    let xdg = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in xdg.split(':').filter(|d| !d.is_empty()) {
        v.push(Path::new(d).join("icons"));
    }
    v.push(PathBuf::from("/usr/share/pixmaps"));
    v
}

/// Find a theme directory (`…/icons/<name>/index.theme`).
fn theme_roots(name: &str) -> Vec<PathBuf> {
    base_dirs().into_iter().map(|b| b.join(name)).filter(|d| d.join("index.theme").is_file()).collect()
}

struct Parsed {
    /// (relative dir, preference score — lower is better)
    dirs: Vec<(String, i32)>,
    inherits: Vec<String>,
}

fn parse_index(text: &str) -> Parsed {
    let mut section = String::new();
    let mut listed: Vec<String> = Vec::new();
    let mut inherits = Vec::new();
    let mut info: std::collections::HashMap<String, (i32, String, String)> = Default::default(); // size, type, context
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if section == "Icon Theme" {
            match k {
                "Directories" | "ScaledDirectories" => listed.extend(v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty())),
                "Inherits" => inherits = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                _ => {}
            }
        } else {
            let e = info.entry(section.clone()).or_insert((0, String::new(), String::new()));
            match k {
                "Size" => e.0 = v.parse().unwrap_or(0),
                "Type" => e.1 = v.to_string(),
                "Context" => e.2 = v.to_string(),
                _ => {}
            }
        }
    }
    let wanted = ["places", "mimetypes", "devices", "actions"];
    let mut dirs: Vec<(String, i32)> = listed
        .into_iter()
        .filter_map(|d| {
            let (size, ty, ctx) = info.get(&d).cloned().unwrap_or((0, String::new(), String::new()));
            let ctx = ctx.to_lowercase();
            if !ctx.is_empty() && !wanted.iter().any(|w| ctx.contains(w)) {
                return None;
            }
            let lower = d.to_lowercase();
            // Prefer full-colour scalable icons, then sizes near 48 px; symbolic last.
            let mut score = if ty.eq_ignore_ascii_case("Scalable") { 0 } else { (size - 48).abs() + if size < 32 { 60 } else { 0 } };
            if lower.contains("symbolic") {
                score += 1000;
            }
            if lower.contains('@') {
                score += 5; // HiDPI duplicates: same art, keep them as a fallback
            }
            Some((d, score))
        })
        .collect();
    dirs.sort_by_key(|d| d.1);
    Parsed { dirs, inherits }
}

impl IconTheme {
    /// Open a theme with its inheritance chain (always ending in `hicolor`).
    pub fn open(name: &str) -> Option<IconTheme> {
        if theme_roots(name).is_empty() {
            return None;
        }
        let mut queue = vec![name.to_string()];
        let mut seen = HashSet::new();
        let mut dirs = Vec::new();
        while let Some(n) = queue.first().cloned() {
            queue.remove(0);
            if !seen.insert(n.clone()) {
                continue;
            }
            for root in theme_roots(&n) {
                let Ok(text) = std::fs::read_to_string(root.join("index.theme")) else { continue };
                let parsed = parse_index(&text);
                dirs.extend(parsed.dirs.iter().map(|(d, _)| root.join(d)).filter(|p| p.is_dir()));
                queue.extend(parsed.inherits);
            }
            if queue.is_empty() && !seen.contains("hicolor") {
                queue.push("hicolor".into());
            }
        }
        Some(IconTheme { name: name.to_string(), dirs })
    }

    /// First icon found for any of `names` (in order of preference).
    pub fn lookup(&self, names: &[&str]) -> Option<PathBuf> {
        for n in names {
            for d in &self.dirs {
                for ext in ["svg", "png"] {
                    let p = d.join(format!("{n}.{ext}"));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
        None
    }
}

/// The icon theme configured in the running desktop, if it can be detected.
pub fn current_theme_name() -> Option<String> {
    if cfg!(any(target_os = "macos", windows)) {
        return None;
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_uppercase();
    if desktop.contains("KDE") {
        let cfg = dirs::config_dir()?.join("kdeglobals");
        if let Ok(text) = std::fs::read_to_string(cfg) {
            let mut in_icons = false;
            for line in text.lines() {
                let l = line.trim();
                if l.starts_with('[') {
                    in_icons = l == "[Icons]";
                } else if in_icons {
                    if let Some(v) = l.strip_prefix("Theme=") {
                        return Some(v.trim().to_string());
                    }
                }
            }
        }
        return Some("breeze".into());
    }
    let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "icon-theme"]).output().ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string();
    (!v.is_empty()).then_some(v)
}

/// Icon themes that are useful to offer (installed ones only).
pub fn installed_themes() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for b in base_dirs() {
        let Ok(rd) = std::fs::read_dir(&b) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let idx = e.path().join("index.theme");
            if n == "hicolor" || n == "default" || v.contains(&n) || !idx.is_file() {
                continue;
            }
            // Only offer complete file-icon themes: not hidden, and with their own folder icon
            // (skips cursor themes and small special-purpose sets such as login icons).
            let Ok(text) = std::fs::read_to_string(&idx) else { continue };
            if text.lines().any(|l| l.trim().eq_ignore_ascii_case("Hidden=true")) {
                continue;
            }
            let parsed = parse_index(&text);
            let has_folder = parsed.dirs.iter().any(|(d, _)| {
                let dir = e.path().join(d);
                dir.join("folder.svg").is_file() || dir.join("folder.png").is_file()
            });
            if has_folder {
                v.push(n);
            }
        }
    }
    v.sort_by_key(|s| s.to_lowercase());
    v
}

/// Freedesktop icon names to try for a file, most specific first.
pub fn names_for(name: &str, is_dir: bool, ext: &str) -> Vec<String> {
    if is_dir {
        let special = match name.to_lowercase().as_str() {
            "desktop" => Some("user-desktop"),
            "documents" => Some("folder-documents"),
            "downloads" => Some("folder-download"),
            "music" => Some("folder-music"),
            "pictures" | "photos" => Some("folder-pictures"),
            "videos" | "movies" => Some("folder-videos"),
            "public" => Some("folder-publicshare"),
            "templates" => Some("folder-templates"),
            ".git" => Some("folder-git"),
            _ => None,
        };
        let mut v: Vec<String> = special.into_iter().map(String::from).collect();
        v.extend(["folder".into(), "inode-directory".into()]);
        return v;
    }
    let mime: &[&str] = match ext {
        "png" => &["image-png"],
        "jpg" | "jpeg" => &["image-jpeg"],
        "gif" => &["image-gif"],
        "svg" => &["image-svg+xml"],
        "webp" => &["image-webp"],
        "pdf" => &["application-pdf"],
        "zip" => &["application-zip"],
        "7z" => &["application-x-7z-compressed"],
        "rar" => &["application-vnd.rar", "application-x-rar"],
        "gz" | "tgz" => &["application-x-compressed-tar", "application-gzip"],
        "xz" | "txz" => &["application-x-xz-compressed-tar", "application-x-xz"],
        "zst" | "tzst" => &["application-zstd", "application-x-zstd-compressed-tar"],
        "bz2" => &["application-x-bzip-compressed-tar", "application-x-bzip"],
        "tar" => &["application-x-tar"],
        "rs" => &["text-rust", "text-x-rust"],
        "py" => &["text-x-python3", "text-x-python"],
        "js" | "mjs" => &["application-javascript", "text-javascript"],
        "ts" | "tsx" => &["application-typescript", "text-typescript"],
        "c" => &["text-x-csrc"],
        "h" => &["text-x-chdr"],
        "cpp" | "cc" | "cxx" => &["text-x-c++src"],
        "java" => &["text-x-java"],
        "go" => &["text-x-go"],
        "sh" | "bash" | "zsh" => &["application-x-shellscript", "text-x-script"],
        "html" | "htm" => &["text-html"],
        "css" => &["text-css"],
        "json" => &["application-json"],
        "xml" => &["application-xml", "text-xml"],
        "yaml" | "yml" => &["application-x-yaml", "text-x-yaml"],
        "toml" => &["application-toml", "text-x-toml"],
        "md" | "markdown" => &["text-markdown", "text-x-markdown"],
        "txt" | "log" => &["text-plain", "text-x-generic"],
        "csv" => &["text-csv", "x-office-spreadsheet"],
        "xlsx" | "xls" | "ods" | "xlsm" => &["x-office-spreadsheet"],
        "docx" | "doc" | "odt" | "rtf" => &["x-office-document"],
        "pptx" | "ppt" | "odp" => &["x-office-presentation"],
        "mp3" => &["audio-mpeg", "audio-x-mpeg"],
        "flac" => &["audio-flac", "audio-x-flac"],
        "wav" => &["audio-x-wav"],
        "mp4" | "m4v" => &["video-mp4"],
        "mkv" => &["video-x-matroska"],
        "webm" => &["video-webm"],
        "exe" | "msi" => &["application-x-ms-dos-executable", "application-x-executable"],
        "deb" => &["application-vnd.debian.binary-package", "application-x-deb"],
        "rpm" => &["application-x-rpm"],
        "iso" => &["application-x-cd-image"],
        "ttf" | "otf" => &["font-x-generic"],
        "parquet" | "avro" | "sqlite" | "db" => &["application-x-sqlite3", "x-office-spreadsheet"],
        _ => &[],
    };
    let generic: &[&str] = match crate::icons::kind_of(ext) {
        crate::icons::FileKind::Image => &["image-x-generic"],
        crate::icons::FileKind::Video => &["video-x-generic"],
        crate::icons::FileKind::Audio => &["audio-x-generic"],
        crate::icons::FileKind::Archive => &["package-x-generic", "application-x-archive"],
        crate::icons::FileKind::Code => &["text-x-script", "text-x-generic"],
        crate::icons::FileKind::Doc => &["x-office-document", "text-x-generic"],
        crate::icons::FileKind::Pdf => &["application-pdf"],
        crate::icons::FileKind::Exec => &["application-x-executable"],
        crate::icons::FileKind::Data => &["x-office-spreadsheet", "text-x-generic"],
        crate::icons::FileKind::Other => &["text-x-generic"],
    };
    mime.iter().chain(generic).chain(["application-x-generic", "unknown"].iter()).map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_index_prefers_scalable_and_skips_symbolic() {
        let idx = "[Icon Theme]\nName=T\nInherits=hicolor\nDirectories=16x16/places,scalable/mimetypes,48x48/places,symbolic/places,apps/48\n\
                   [16x16/places]\nSize=16\nContext=Places\n[scalable/mimetypes]\nSize=64\nType=Scalable\nContext=MimeTypes\n\
                   [48x48/places]\nSize=48\nContext=Places\n[symbolic/places]\nSize=16\nType=Scalable\nContext=Places\n[apps/48]\nSize=48\nContext=Applications\n";
        let p = parse_index(idx);
        let order: Vec<&str> = p.dirs.iter().map(|d| d.0.as_str()).collect();
        assert_eq!(order, vec!["scalable/mimetypes", "48x48/places", "16x16/places", "symbolic/places"]);
        assert_eq!(p.inherits, vec!["hicolor"]);
    }

    #[test]
    fn names() {
        let n = names_for("main.rs", false, "rs");
        assert_eq!(n[0], "text-rust");
        assert!(n.contains(&"text-x-generic".to_string()));
        assert_eq!(names_for("Downloads", true, "")[0], "folder-download");
    }

    /// Uses whatever themes are installed on the build machine (no-op otherwise).
    #[test]
    fn lookup_installed() {
        for t in ["Adwaita", "breeze", "Papirus"] {
            if let Some(theme) = IconTheme::open(t) {
                assert!(theme.lookup(&["folder"]).is_some(), "{t} has a folder icon");
            }
        }
    }
}
