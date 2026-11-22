//! Files from a server or from inside an archive open in other apps through a local
//! copy. Edits saved to such a copy must not be lost: the copy is watched, the user is
//! offered to upload the changes (servers) or to keep the copy (archives cannot be
//! changed), a copy is never written over, and an edited one is kept, not deleted, when
//! Matrix Finder quits.
use crate::files::vfs::Location;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Size and modification time of a copy: tells when an app saved it.
pub type Stamp = (u64, Option<SystemTime>);

/// How often the copies are looked at.
const POLL: Duration = Duration::from_secs(2);

/// The name of the private temporary folders of copies.
pub const TEMP_PREFIX: &str = "open";

pub fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::symlink_metadata(path).ok()?;
    Some((m.len(), m.modified().ok()))
}

pub struct Copy {
    pub id: u64,
    /// The folder it came from (on a server, or in an archive) and its name there.
    pub src: Location,
    pub name: String,
    /// Where it came from, for messages.
    pub label: String,
    /// The copy, alone in its own private temporary folder.
    pub dir: PathBuf,
    pub path: PathBuf,
    /// As downloaded or last uploaded; `None` while it is still being downloaded.
    pub saved: Option<Stamp>,
    /// The changes the user already answered about ("Keep local copy").
    pub asked: Option<Stamp>,
    /// Its changes are being uploaded.
    pub uploading: bool,
}

impl Copy {
    pub fn is_remote(&self) -> bool {
        matches!(self.src, Location::Remote { .. })
    }

    /// Edited since it was downloaded or last uploaded (and still there).
    pub fn changed(&self) -> bool {
        self.saved.is_some_and(|s| stamp(&self.path).is_some_and(|now| now != s))
    }
}

#[derive(Default)]
pub struct Copies {
    pub list: Vec<Copy>,
    next: u64,
    polled: Option<Instant>,
    /// Some copy is edited and not uploaded (as last looked at).
    unsaved: bool,
}

impl Copies {
    /// A copy being downloaded to `path` (in its own folder `dir`); returns its id.
    pub fn add(&mut self, src: Location, name: String, label: String, dir: PathBuf, path: PathBuf) -> u64 {
        self.next += 1;
        self.list.push(Copy { id: self.next, src, name, label, dir, path, saved: None, asked: None, uploading: false });
        self.next
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Copy> {
        self.list.iter_mut().find(|c| c.id == id)
    }

    /// The download finished: changes are counted from now.
    pub fn downloaded(&mut self, path: &Path) {
        if let Some(c) = self.list.iter_mut().find(|c| c.path == path) {
            c.saved = stamp(path);
        }
    }

    /// The download failed: nothing to watch (its empty folder goes).
    pub fn forget(&mut self, path: &Path) {
        if let Some(i) = self.list.iter().position(|c| c.path == path && c.saved.is_none()) {
            let c = self.list.remove(i);
            let _ = std::fs::remove_dir_all(&c.dir);
        }
    }

    /// At most every two seconds (or now, with `now`): the copies edited since the user
    /// was last asked about them, which are to be asked about now.
    pub fn poll(&mut self, now: bool) -> Vec<u64> {
        if !now && self.polled.is_some_and(|t| t.elapsed() < POLL) {
            return vec![];
        }
        self.polled = Some(Instant::now());
        self.unsaved = self.list.iter().any(Copy::changed);
        self.list
            .iter()
            .filter(|c| !c.uploading && c.changed())
            .filter(|c| c.asked.is_none() || c.asked != stamp(&c.path))
            .map(|c| c.id)
            .collect()
    }

    /// Some copy is edited and its changes are not uploaded (as of the last `poll`).
    pub fn has_unsaved(&self) -> bool {
        self.unsaved
    }

    /// The copies edited and not uploaded.
    pub fn unsaved(&self) -> impl Iterator<Item = &Copy> {
        self.list.iter().filter(|c| c.changed())
    }

    /// Quitting: edited copies are moved into `keep_dir`, the other copies are removed.
    /// Returns where each edited copy went, or why it could not be moved (then it stays
    /// where it is).
    pub fn finish(&mut self, keep_dir: &Path) -> Vec<Result<PathBuf, String>> {
        let mut out = Vec::new();
        for c in self.list.drain(..) {
            // Whatever else an app saved in its folder is left there too.
            let others = || std::fs::read_dir(&c.dir).map(|rd| rd.flatten().any(|e| e.path() != c.path)).unwrap_or(false);
            if c.changed() {
                match keep(&c.path, keep_dir) {
                    Ok(p) => {
                        if !others() {
                            let _ = std::fs::remove_dir_all(&c.dir);
                        }
                        out.push(Ok(p));
                    }
                    Err(e) => out.push(Err(format!("{}: {e}", c.path.display()))),
                }
            } else if !others() {
                let _ = std::fs::remove_dir_all(&c.dir);
            }
        }
        out
    }
}

/// Where edited copies are kept when Matrix Finder quits.
pub fn recovered_dir() -> PathBuf {
    dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join("Matrix Finder recovered")
}

/// Move file `from` into `dir` under a name not taken there (never over another file).
/// On another disk it is copied and flushed before the original is removed.
pub fn keep(from: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = from.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    for _ in 0..100 {
        let to = crate::files::safefs::unique_path(&dir.join(name));
        // A link fails rather than replace a file that appeared meanwhile.
        match std::fs::hard_link(from, &to) {
            Ok(()) => {
                std::fs::remove_file(from)?;
                crate::util::sync_dir(dir);
                return Ok(to);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {}
        }
        let mut out = match std::fs::File::create_new(&to) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let copied = std::fs::File::open(from).and_then(|mut f| std::io::copy(&mut f, &mut out)).and_then(|_| crate::util::sync_file(&out));
        if let Err(e) = copied {
            drop(out);
            let _ = std::fs::remove_file(&to);
            return Err(e);
        }
        std::fs::remove_file(from)?;
        crate::util::sync_dir(dir);
        return Ok(to);
    }
    Err(std::io::Error::other("no free name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edited_copies_are_kept() {
        let base = crate::util::private_temp_dir("test").unwrap();
        let keep_dir = base.join("kept");
        let mut copies = Copies::default();
        let mut add = |name: &str| {
            let dir = base.join(format!("d-{name}"));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, "server version").unwrap();
            let id = copies.add(Location::Remote { id: 1, path: "/x".into() }, name.into(), name.into(), dir, path.clone());
            copies.downloaded(&path);
            (id, path)
        };
        let (edited, edited_path) = add("a.txt");
        let (_, same_path) = add("b.txt");
        assert!(copies.poll(true).is_empty() && !copies.has_unsaved());
        std::fs::write(&edited_path, "edited, longer than before").unwrap();
        assert_eq!(copies.poll(true), vec![edited]);
        assert!(copies.has_unsaved());
        // "Keep local copy": not asked again until it changes again.
        let c = copies.get_mut(edited).unwrap();
        c.asked = stamp(&c.path);
        assert!(copies.poll(true).is_empty());
        assert_eq!(copies.unsaved().count(), 1);
        // A file of that name is already kept: neither is written over.
        std::fs::create_dir_all(&keep_dir).unwrap();
        std::fs::write(keep_dir.join("a.txt"), "older").unwrap();
        let done = copies.finish(&keep_dir);
        assert_eq!(done.len(), 1);
        let kept = done[0].as_ref().unwrap();
        assert_ne!(kept, &keep_dir.join("a.txt"));
        assert_eq!(std::fs::read_to_string(kept).unwrap(), "edited, longer than before");
        assert_eq!(std::fs::read_to_string(keep_dir.join("a.txt")).unwrap(), "older");
        assert!(!edited_path.exists() && !same_path.exists() && !same_path.parent().unwrap().exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
