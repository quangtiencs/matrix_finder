//! Virtual file system abstractions: entries and locations.
use crate::util::{es, ext_of, Res};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub is_link: bool,
    pub hidden: bool,
    /// The synthetic `..` entry.
    pub is_parent: bool,
    pub ext: String,
}

impl Entry {
    pub fn new(name: String, is_dir: bool, size: u64, modified: Option<SystemTime>) -> Self {
        let ext = if is_dir { String::new() } else { ext_of(&name) };
        let hidden = name.starts_with('.');
        Self {
            name,
            is_dir,
            size,
            modified,
            is_link: false,
            hidden,
            is_parent: false,
            ext,
        }
    }

    pub fn parent() -> Self {
        let mut e = Self::new("..".into(), true, 0, None);
        e.is_parent = true;
        e.hidden = false;
        e
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Location {
    Local(PathBuf),
    /// A remote connection (id into the app's connection table) and a `/` path.
    Remote { id: u64, path: String },
    /// Browsing inside an archive file. `inner` is a `/` path without leading slash.
    Archive { file: PathBuf, inner: String },
}

impl Location {
    pub fn display(&self, remote_label: impl Fn(u64) -> String) -> String {
        match self {
            Location::Local(p) => p.to_string_lossy().to_string(),
            Location::Remote { id, path } => format!("{}{}", remote_label(*id), path),
            Location::Archive { file, inner } => {
                if inner.is_empty() {
                    format!("{}::/", file.to_string_lossy())
                } else {
                    format!("{}::/{}", file.to_string_lossy(), inner)
                }
            }
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Location::Local(_))
    }

    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Location::Local(p) => Some(p),
            _ => None,
        }
    }

    pub fn parent(&self) -> Option<Location> {
        match self {
            Location::Local(p) => p.parent().map(|p| Location::Local(p.to_path_buf())),
            Location::Remote { id, path } => crate::util::parent_slash(path).map(|p| Location::Remote {
                id: *id,
                path: if p.is_empty() { "/".into() } else { p },
            }),
            Location::Archive { file, inner } => {
                if inner.is_empty() {
                    file.parent().map(|p| Location::Local(p.to_path_buf()))
                } else {
                    let p = crate::util::parent_slash(inner).unwrap_or_default();
                    Some(Location::Archive {
                        file: file.clone(),
                        inner: p,
                    })
                }
            }
        }
    }

    pub fn child(&self, name: &str) -> Location {
        match self {
            Location::Local(p) => Location::Local(p.join(name)),
            Location::Remote { id, path } => Location::Remote {
                id: *id,
                path: crate::util::join_slash(path, name),
            },
            Location::Archive { file, inner } => Location::Archive {
                file: file.clone(),
                inner: crate::util::join_slash(inner, name),
            },
        }
    }

    /// Name of the last path component, used for tab titles.
    pub fn short_name(&self) -> String {
        match self {
            Location::Local(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.to_string_lossy().to_string()),
            Location::Remote { path, .. } => path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("/")
                .to_string(),
            Location::Archive { file, inner } => {
                let base = file
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                match inner.rsplit('/').next().filter(|s| !s.is_empty()) {
                    Some(n) => format!("{base}:{n}"),
                    None => base,
                }
            }
        }
    }
}

#[cfg(windows)]
fn is_hidden_local(path: &Path, meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    let _ = path;
    meta.file_attributes() & 0x2 != 0
}

#[cfg(not(windows))]
fn is_hidden_local(path: &Path, _meta: &std::fs::Metadata) -> bool {
    path.file_name()
        .map(|n| n.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
}

pub fn list_local(dir: &Path) -> Res<Vec<Entry>> {
    let rd = std::fs::read_dir(dir).map_err(es)?;
    let mut out = Vec::new();
    for item in rd.flatten() {
        let path = item.path();
        let name = item.file_name().to_string_lossy().to_string();
        let lmeta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_link = lmeta.file_type().is_symlink();
        // Follow symlinks for type/size, fall back to the link itself if dangling.
        let meta = if is_link {
            std::fs::metadata(&path).unwrap_or(lmeta)
        } else {
            lmeta
        };
        let mut e = Entry::new(name, meta.is_dir(), if meta.is_dir() { 0 } else { meta.len() }, meta.modified().ok());
        e.is_link = is_link;
        e.hidden = is_hidden_local(&path, &meta);
        out.push(e);
    }
    Ok(out)
}

/// Mount points / drives for the drive selector.
pub fn drives() -> Vec<(String, PathBuf, u64, u64)> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut out: Vec<(String, PathBuf, u64, u64)> = disks
        .list()
        .iter()
        .map(|d| {
            let mp = d.mount_point().to_path_buf();
            let name = d.name().to_string_lossy().to_string();
            let label = if cfg!(windows) || name.is_empty() {
                mp.to_string_lossy().to_string()
            } else {
                format!("{} ({})", mp.to_string_lossy(), name)
            };
            (label, mp, d.total_space(), d.available_space())
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup_by(|a, b| a.1 == b.1);
    if out.is_empty() {
        out.push(("/".into(), PathBuf::from("/"), 0, 0));
    }
    out
}

/// (total, available) bytes of the filesystem holding `path`.
pub fn free_space(path: &Path, cached: &[(String, PathBuf, u64, u64)]) -> Option<(u64, u64)> {
    cached
        .iter()
        .filter(|d| path.starts_with(&d.1))
        .max_by_key(|d| d.1.as_os_str().len())
        .map(|d| (d.2, d.3))
}
