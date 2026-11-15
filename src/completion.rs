//! Suggestions for the address bar: the folders and archives that match what is typed.
//!
//! The folder being typed into is read on a thread, once per folder while the address
//! bar is open, so a slow disk or network mount never holds up typing.

use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

/// Most suggestions shown at once.
pub const MAX_SHOWN: usize = 10;
/// Most entries read from one folder.
const MAX_READ: usize = 50_000;

/// A folder or archive in the folder being typed into.
#[derive(Clone, Debug, PartialEq)]
pub struct Cand {
    pub name: String,
    pub archive: bool,
}

/// One row of the suggestion list.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub name: String,
    pub archive: bool,
    /// The address it stands for, without a separator at the end.
    pub text: String,
    /// What Tab puts in the address bar: folders get a separator, to go on typing inside.
    pub fill: String,
}

/// What is typed, split into the folder to list and the name being typed.
#[derive(Debug, PartialEq)]
pub struct Query {
    /// The folder whose folders and archives are suggested.
    pub dir: PathBuf,
    /// The text that goes before a suggested name, up to and including the last separator.
    pub head: String,
    /// The part of the name typed so far.
    pub prefix: String,
}

fn is_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Splits the address bar text. `~` stands for `home`, and a relative path is read
/// in `base` (the window's folder); its suggestions are full paths. Server addresses and
/// paths inside archives get no suggestions.
pub fn split(text: &str, home: Option<&Path>, base: Option<&Path>) -> Option<Query> {
    if text.trim().is_empty() || text.contains("://") || text.contains("::") {
        return None;
    }
    let (head, prefix) = match text.rfind(is_sep) {
        Some(i) => (&text[..=i], &text[i + 1..]),
        None => ("", text),
    };
    if head.is_empty() && prefix.starts_with('~') {
        // "~" itself, or "~name": nothing to list yet.
        return None;
    }
    let (dir, head) = if let Some(rest) = head.strip_prefix('~') {
        if !rest.starts_with(is_sep) {
            return None;
        }
        (home?.join(rest.trim_start_matches(is_sep)), head.to_string())
    } else if head.starts_with(is_sep) || Path::new(head).is_absolute() {
        (PathBuf::from(head), head.to_string())
    } else {
        let dir = base?.join(head);
        let mut h = dir.to_string_lossy().to_string();
        if !h.ends_with(is_sep) {
            h.push(MAIN_SEPARATOR);
        }
        (dir, h)
    };
    Some(Query { dir, head, prefix: prefix.to_string() })
}

/// The candidates that match `prefix`, case-insensitively: names that start with it
/// first, then names that contain it. Hidden names (starting with a dot) only when
/// they are shown in the file lists or the prefix starts with a dot.
pub fn matches(cands: &[Cand], prefix: &str, show_hidden: bool) -> Vec<usize> {
    let p = prefix.to_lowercase();
    let hidden_ok = show_hidden || p.starts_with('.');
    let (mut starts, mut contains) = (Vec::new(), Vec::new());
    for (i, c) in cands.iter().enumerate() {
        if c.name.starts_with('.') && !hidden_ok {
            continue;
        }
        let n = c.name.to_lowercase();
        if n.starts_with(&p) {
            starts.push(i);
            if starts.len() == MAX_SHOWN {
                break;
            }
        } else if contains.len() < MAX_SHOWN && n.contains(&p) {
            contains.push(i);
        }
    }
    starts.extend(contains);
    starts.truncate(MAX_SHOWN);
    starts
}

/// The separator used in `head`, for what is added after a folder name.
fn sep_of(head: &str) -> char {
    if cfg!(windows) && head.contains('\\') && !head.contains('/') {
        '\\'
    } else if head.contains('/') {
        '/'
    } else {
        MAIN_SEPARATOR
    }
}

/// The folders and archives in `dir`, in natural order.
fn read(dir: &Path) -> Vec<Cand> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out = Vec::new();
    for e in rd.flatten().take(MAX_READ) {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        let is_dir = ft.is_dir() || (ft.is_symlink() && e.path().is_dir());
        if is_dir {
            out.push(Cand { name, archive: false });
        } else if crate::archive::is_archive(Path::new(&name)) {
            out.push(Cand { name, archive: true });
        }
    }
    out.sort_by(|a, b| crate::pane::natural_cmp(&a.name, &b.name));
    out
}

/// The address bar's suggestion list.
#[derive(Default)]
pub struct Completer {
    /// The folder last read, with its folders and archives.
    listed: Option<(PathBuf, Arc<Vec<Cand>>)>,
    /// A folder being read.
    pending: Option<(PathBuf, Receiver<Vec<Cand>>)>,
    /// The suggestions for `for_text`.
    pub items: Vec<Suggestion>,
    pub sel: Option<usize>,
    for_text: Option<String>,
    /// Something was typed since the address bar was opened: only then are there suggestions.
    pub active: bool,
    /// Where each row was drawn in the last frame. A click is taken from these before the
    /// address bar sees it, because the bar gives up the keyboard as soon as it is pressed.
    pub rows: Vec<egui::Rect>,
    /// Folders only, no archives (a place to copy to).
    pub folders_only: bool,
}

impl Completer {
    /// Closes the list; folders are read again the next time.
    pub fn reset(&mut self) {
        *self = Completer { folders_only: self.folders_only, ..Completer::default() };
    }

    /// Brings the suggestions up to date with `text`.
    pub fn update(&mut self, text: &str, home: Option<&Path>, base: Option<&Path>, show_hidden: bool, ctx: &egui::Context) {
        if let Some((dir, rx)) = &self.pending {
            let got = match rx.try_recv() {
                Ok(v) => Some(v),
                Err(TryRecvError::Disconnected) => Some(vec![]),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(v) = got {
                self.listed = Some((dir.clone(), Arc::new(v)));
                self.pending = None;
                self.for_text = None;
            }
        }
        if !self.active {
            return;
        }
        let Some(q) = split(text, home, base) else {
            self.items.clear();
            self.sel = None;
            return;
        };
        let cands = match &self.listed {
            Some((d, c)) if *d == q.dir => c.clone(),
            _ => {
                if !matches!(&self.pending, Some((d, _)) if *d == q.dir) {
                    let (tx, rx) = channel();
                    let (dir, ctx) = (q.dir.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send(read(&dir));
                        ctx.request_repaint();
                    });
                    self.pending = Some((q.dir, rx));
                }
                self.items.clear();
                self.sel = None;
                return;
            }
        };
        if self.for_text.as_deref() == Some(text) {
            return;
        }
        self.for_text = Some(text.to_string());
        let sep = sep_of(&q.head);
        let cands = if self.folders_only { Arc::new(cands.iter().filter(|c| !c.archive).cloned().collect()) } else { cands };
        self.items = matches(&cands, &q.prefix, show_hidden)
            .into_iter()
            .map(|i| {
                let c = &cands[i];
                let text = format!("{}{}", q.head, c.name);
                let fill = if c.archive { text.clone() } else { format!("{text}{sep}") };
                Suggestion { name: c.name.clone(), archive: c.archive, text, fill }
            })
            .collect();
        self.sel = None;
    }

    /// Moves the highlight down (`1`) or up (`-1`), round the ends.
    pub fn step(&mut self, by: isize) {
        let n = self.items.len() as isize;
        if n == 0 {
            return;
        }
        self.sel = Some(match self.sel {
            None if by > 0 => 0,
            None => n as usize - 1,
            Some(s) => (s as isize + by).rem_euclid(n) as usize,
        });
    }

    /// What Tab puts in the address bar for `text`: the highlighted or only suggestion,
    /// else the longest start the names that begin with what is typed share. Without
    /// either, Tab highlights the first suggestion and returns `None`.
    pub fn tab(&mut self, text: &str) -> Option<String> {
        if let Some(s) = self.sel.and_then(|s| self.items.get(s)) {
            return Some(s.fill.clone());
        }
        if self.items.len() == 1 {
            return Some(self.items[0].fill.clone());
        }
        let typed = text.rfind(is_sep).map_or(text, |i| &text[i + 1..]).to_lowercase();
        let names: Vec<&str> = self.items.iter().map(|s| s.name.as_str()).filter(|n| n.to_lowercase().starts_with(&typed)).collect();
        if let Some(first) = names.first() {
            let mut len = first.len();
            for n in &names[1..] {
                len = first.char_indices().zip(n.chars()).take_while(|((_, a), b)| a.to_lowercase().eq(b.to_lowercase())).map(|((i, a), _)| i + a.len_utf8()).last().unwrap_or(0).min(len);
            }
            if first[..len].chars().count() > typed.chars().count() {
                let head = &self.items[0].text[..self.items[0].text.len() - first.len()];
                return Some(format!("{head}{}", &first[..len]));
            }
        }
        self.step(1);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands(names: &[&str]) -> Vec<Cand> {
        names.iter().map(|n| Cand { name: n.to_string(), archive: n.ends_with(".zip") }).collect()
    }

    #[test]
    fn split_text() {
        let home = Path::new("/home/me");
        let base = Path::new("/work");
        let q = split("/usr/lo", Some(home), Some(base)).unwrap();
        assert_eq!((q.dir, q.head.as_str(), q.prefix.as_str()), (PathBuf::from("/usr/"), "/usr/", "lo"));
        let q = split("~/Doc", Some(home), Some(base)).unwrap();
        assert_eq!((q.dir, q.head.as_str(), q.prefix.as_str()), (PathBuf::from("/home/me"), "~/", "Doc"));
        let q = split("~/src/", Some(home), None).unwrap();
        assert_eq!((q.dir, q.prefix.as_str()), (PathBuf::from("/home/me/src"), ""));
        // Relative: read in the window's folder, suggested as full paths.
        let q = split("pro", Some(home), Some(base)).unwrap();
        assert_eq!((q.dir, q.head.as_str()), (PathBuf::from("/work"), format!("/work{MAIN_SEPARATOR}").as_str()));
        let q = split("a/b", None, Some(Path::new("/"))).unwrap();
        assert_eq!((q.dir, q.head.as_str(), q.prefix.as_str()), (PathBuf::from("/a/"), "/a/", "b"));
        assert!(split("pro", Some(home), None).is_none(), "no folder to read relative paths in");
        for none in ["", "  ", "~", "~me", "~me/x", "sftp://host/x", "/x/a.zip::in", "ftp://h"] {
            assert!(split(none, Some(home), Some(base)).is_none(), "{none:?}");
        }
    }

    #[test]
    fn matching() {
        let c = cands(&[".git", "Documents", "Downloads", "docs.zip", "my docs", "src"]);
        let names = |v: Vec<usize>| v.into_iter().map(|i| c[i].name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(matches(&c, "do", false)), ["Documents", "Downloads", "docs.zip", "my docs"]);
        // Names that only contain it come after the ones that start with it.
        assert_eq!(names(matches(&c, "docs", false)), ["docs.zip", "my docs"]);
        assert_eq!(names(matches(&c, "", false)), ["Documents", "Downloads", "docs.zip", "my docs", "src"]);
        assert_eq!(names(matches(&c, "", true))[0], ".git");
        assert_eq!(names(matches(&c, ".g", false)), [".git"]);
        assert!(matches(&c, "zzz", false).is_empty());
        let many: Vec<Cand> = (0..50).map(|i| Cand { name: format!("d{i}"), archive: false }).collect();
        assert_eq!(matches(&many, "d", false).len(), MAX_SHOWN);
    }

    fn completer(text: &str, names: &[&str]) -> Completer {
        let q = split(text, None, None).unwrap();
        let mut c = Completer { active: true, listed: Some((q.dir, Arc::new(cands(names)))), ..Default::default() };
        c.update(text, None, None, false, &egui::Context::default());
        c
    }

    #[test]
    fn suggestions_and_tab() {
        let mut c = completer("/data/pro", &["projects", "Prototypes", "archive.zip", "old"]);
        assert_eq!(c.items.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["/data/projects", "/data/Prototypes"]);
        assert_eq!(c.items[0].fill, "/data/projects/");
        // Tab: the shared start, case-insensitively, then the highlighted one.
        assert_eq!(c.tab("/data/pro"), None, "no shared start longer than what is typed");
        assert_eq!(c.sel, Some(0));
        assert_eq!(c.tab("/data/pro").as_deref(), Some("/data/projects/"));
        c.step(1);
        assert_eq!(c.sel, Some(1));
        c.step(1);
        assert_eq!(c.sel, Some(0), "round the end");
        c.step(-1);
        assert_eq!(c.sel, Some(1));

        let mut c = completer("/data/re", &["reports-2025", "reports-2026", "other"]);
        assert_eq!(c.tab("/data/re").as_deref(), Some("/data/reports-202"));
        // The only one, and archives get no separator.
        let mut c = completer("/data/arc", &["archive.zip", "old"]);
        assert_eq!(c.tab("/data/arc").as_deref(), Some("/data/archive.zip"));
        let mut c = completer("/data/o", &["old"]);
        assert_eq!(c.tab("/data/o").as_deref(), Some("/data/old/"));

        // Nothing typed yet: no list.
        let q = split("/data/o", None, None).unwrap();
        let mut c = Completer { listed: Some((q.dir, Arc::new(cands(&["old"])))), ..Default::default() };
        c.update("/data/o", None, None, false, &egui::Context::default());
        assert!(c.items.is_empty());
    }

    #[test]
    fn reads_folders_and_archives_in_background() {
        let dir = std::env::temp_dir().join(format!("mf-complete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["alpha", "alps", "beta"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("al.zip"), b"").unwrap();
        std::fs::write(dir.join("almanac.txt"), b"").unwrap();
        let text = format!("{}/al", dir.display());
        let ctx = egui::Context::default();
        let mut c = Completer { active: true, ..Default::default() };
        let start = std::time::Instant::now();
        while c.items.is_empty() && start.elapsed().as_secs() < 5 {
            c.update(&text, None, None, false, &ctx);
            std::thread::yield_now();
        }
        let names: Vec<(&str, bool)> = c.items.iter().map(|s| (s.name.as_str(), s.archive)).collect();
        assert_eq!(names, [("al.zip", true), ("alpha", false), ("alps", false)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A place to copy to: folders only, and the setting outlives a reset.
    #[test]
    fn folders_only() {
        let q = split("/data/a", None, None).unwrap();
        let mut c = Completer { active: true, folders_only: true, listed: Some((q.dir.clone(), Arc::new(cands(&["a.zip", "apps", "b"])))), ..Default::default() };
        c.update("/data/a", None, None, false, &egui::Context::default());
        assert_eq!(c.items.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["apps"]);
        c.reset();
        assert!(c.folders_only && c.items.is_empty() && !c.active);
    }
}
