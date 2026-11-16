//! Small helpers shared across the application.
use std::fmt::Display;
use std::time::SystemTime;

pub type Res<T> = Result<T, String>;

/// Convert any displayable error to a `String` (used with `map_err`).
pub fn es<E: Display>(e: E) -> String {
    e.to_string()
}

/// Human readable size, e.g. `12.3 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

pub fn format_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => {
            let dt: chrono::DateTime<chrono::Local> = t.into();
            dt.format("%Y-%m-%d %H:%M").to_string()
        }
        None => String::new(),
    }
}

/// Join two components of a remote / archive path that always uses `/`.
pub fn join_slash(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Parent of a `/`-separated path. Returns `None` at the root.
pub fn parent_slash(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
        None => Some(String::new()),
    }
}

/// Split a command line into arguments. Supports single and double quotes.
pub fn shell_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    in_word = true;
                }
                c if c.is_whitespace() => {
                    if in_word {
                        out.push(std::mem::take(&mut cur));
                        in_word = false;
                    }
                }
                c => {
                    cur.push(c);
                    in_word = true;
                }
            },
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

/// Lower-cased extension of a file name (without dot). Empty for none.
pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// Pick a free file name in `dir` by appending ` (n)` before the extension.
pub fn unique_path(path: &std::path::Path) -> std::path::PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1.. {
        let candidate = parent.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// `path` with the home folder written as `~` (the home folder itself is `~/`).
pub fn tilde_path(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    if let Some(h) = home.filter(|h| !h.as_os_str().is_empty()) {
        if path == h {
            return "~/".to_string();
        }
        if let Ok(rest) = path.strip_prefix(h) {
            return format!("~/{}", rest.to_string_lossy().replace('\\', "/"));
        }
    }
    path.to_string_lossy().to_string()
}

/// Shorter forms of a `tilde_path`, longest first: the whole path, then with
/// the middle folders replaced by "…" (`~/…/parent/leaf`, `~/…/leaf`).
pub fn short_path_candidates(full: &str) -> Vec<String> {
    let mut out = vec![full.to_string()];
    let (root, rest) = if let Some(r) = full.strip_prefix("~/") {
        ("~/", r)
    } else if let Some(r) = full.strip_prefix('/') {
        ("/", r)
    } else {
        ("", full)
    };
    let parts: Vec<&str> = rest.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    for keep in [2usize, 1] {
        if parts.len() > keep {
            out.push(format!("{root}…/{}", parts[parts.len() - keep..].join("/")));
        }
    }
    out
}

/// Write `text` to `path` (via a temporary file) so only the user can read it.
pub fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")))?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    std::io::Write::write_all(&mut o.open(&tmp)?, text.as_bytes())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_files_are_for_the_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let p = std::env::temp_dir().join(format!("matrix_finder-private-{}", std::process::id()));
        std::fs::write(&p, "old, readable by all").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&p, "secret").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "secret");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(10), "10 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024 * 1024 * 200), "200 MB");
    }

    #[test]
    fn slash_paths() {
        assert_eq!(join_slash("/a", "b"), "/a/b");
        assert_eq!(join_slash("", "b"), "b");
        assert_eq!(parent_slash("/a/b"), Some("/a".into()));
        assert_eq!(parent_slash("/a"), Some("/".into()));
        assert_eq!(parent_slash("/"), None);
        assert_eq!(parent_slash("a"), Some(String::new()));
        assert_eq!(parent_slash(""), None);
    }

    #[test]
    fn split() {
        assert_eq!(
            shell_split(r#"open -a "My App" {dir}"#),
            vec!["open", "-a", "My App", "{dir}"]
        );
    }

    #[test]
    fn ext() {
        assert_eq!(ext_of("a.RS"), "rs");
        assert_eq!(ext_of(".bashrc"), "");
        assert_eq!(ext_of("noext"), "");
    }

    #[test]
    fn tilde_and_short_paths() {
        use std::path::Path;
        let home = Path::new("/home/me");
        assert_eq!(tilde_path(home, Some(home)), "~/");
        assert_eq!(tilde_path(Path::new("/home/me/Projects/app"), Some(home)), "~/Projects/app");
        assert_eq!(tilde_path(Path::new("/home/meow"), Some(home)), "/home/meow", "only whole folder names");
        assert_eq!(tilde_path(Path::new("/etc"), Some(home)), "/etc");
        assert_eq!(short_path_candidates("~/a/b/c/d"), ["~/a/b/c/d", "~/…/c/d", "~/…/d"]);
        assert_eq!(short_path_candidates("/usr/local/bin"), ["/usr/local/bin", "/…/local/bin", "/…/bin"]);
        assert_eq!(short_path_candidates("~/"), ["~/"]);
    }
}
