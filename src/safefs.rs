//! Safe writes: new data goes into a hidden temporary file next to its target, is synced
//! to disk, and only then renamed into place. The final rename never replaces anything
//! unless the user chose Overwrite, so an existing file survives a crash, a cancel, a full
//! disk, or another job that writes the same name at the same time.
use crate::ops::Conflict;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

/// Whether anything is at `p`, a dangling symbolic link included.
pub fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// A free name like `path`: `report.pdf`, else `report (1).pdf`, `report (2).pdf`…
/// (`logs (1).tar.gz` for a compressed tar). A dangling symbolic link counts as taken.
pub fn unique_path(path: &Path) -> PathBuf {
    if !exists(path) {
        return path.to_path_buf();
    }
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    if !ext.is_empty() && stem.len() > 4 && stem.to_lowercase().ends_with(".tar") {
        ext = format!("{}{ext}", &stem[stem.len() - 4..]);
        stem.truncate(stem.len() - 4);
    }
    (1..).map(|n| parent.join(format!("{stem} ({n}){ext}"))).find(|c| !exists(c)).unwrap_or_default()
}

/// A short random tag for temporary names (std's hasher keys are random per process).
fn random_tag() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(N.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    format!("{:012x}", h.finish() & 0xffff_ffff_ffff)
}

/// A hidden temporary name for writing `name`: `.name.mf-part-1a2b3c4d5e6f`. Long names are
/// cut, so the result still fits in a file name.
pub fn temp_name(name: &str, kind: &str) -> String {
    let mut end = name.len().min(100);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!(".{}.mf-{kind}-{}", &name[..end], random_tag())
}

/// An unused temporary path next to `dst`.
pub fn temp_path(dst: &Path, kind: &str) -> PathBuf {
    let name = dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    dst.with_file_name(temp_name(&name, kind))
}

/// Errors that only say a file system cannot sync, not that the data is lost.
fn sync_unsupported(e: &io::Error) -> bool {
    #[cfg(unix)]
    if let Some(code) = e.raw_os_error() {
        return code == libc::EINVAL || code == libc::ENOTSUP || code == libc::EOPNOTSUPP || code == libc::ENOSYS || code == libc::EROFS;
    }
    e.kind() == ErrorKind::Unsupported
}

/// Flush a written file to the disk (std uses F_FULLFSYNC on macOS).
pub fn sync_file(f: &File) -> io::Result<()> {
    match f.sync_all() {
        Err(e) if !sync_unsupported(&e) => Err(e),
        _ => Ok(()),
    }
}

/// Flush a file that something else wrote (a download, an unpacked entry). Links need nothing.
fn sync_path(p: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(p)?;
    if meta.is_dir() {
        sync_dir(p);
        return Ok(());
    }
    if !meta.is_file() {
        return Ok(());
    }
    // Read-only is enough to sync on unix (and works for files unpacked read-only).
    #[cfg(unix)]
    let f = File::open(p);
    #[cfg(not(unix))]
    let f = OpenOptions::new().write(true).open(p);
    match f {
        Ok(f) => sync_file(&f),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => Ok(()),
        Err(e) => Err(e),
    }
}

/// Make a rename in `dir` durable. Best effort: some file systems cannot sync folders.
pub fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(f) = File::open(dir) {
        let _ = f.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(unix)]
fn cstr(p: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::new(ErrorKind::InvalidInput, "path contains a null byte"))
}

/// The system's own no-replace rename, or `None` where this file system has none.
#[cfg(target_os = "macos")]
fn native_noreplace(from: &Path, to: &Path) -> Option<io::Result<()>> {
    let (f, t) = match (cstr(from), cstr(to)) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(e), _) | (_, Err(e)) => return Some(Err(e)),
    };
    // SAFETY: both are NUL-terminated paths.
    if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
        return Some(Ok(()));
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::ENOTSUP | libc::EOPNOTSUPP | libc::ENOSYS) => None,
        _ => Some(Err(e)),
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn native_noreplace(from: &Path, to: &Path) -> Option<io::Result<()>> {
    let (f, t) = match (cstr(from), cstr(to)) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(e), _) | (_, Err(e)) => return Some(Err(e)),
    };
    // SAFETY: both are NUL-terminated paths; renameat2 has no other pointers.
    let r = unsafe { libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), libc::RENAME_NOREPLACE) };
    if r == 0 {
        return Some(Ok(()));
    }
    let e = io::Error::last_os_error();
    // Old kernels and some file systems (network, FUSE) do not know the flag.
    match e.raw_os_error() {
        Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP) => None,
        _ => Some(Err(e)),
    }
}

#[cfg(windows)]
fn native_noreplace(from: &Path, to: &Path) -> Option<io::Result<()>> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let wide = |p: &Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (f, t) = (wide(from), wide(to));
    // Without MOVEFILE_REPLACE_EXISTING it fails when `to` exists. SAFETY: NUL-terminated.
    if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) } != 0 {
        Some(Ok(()))
    } else {
        Some(Err(io::Error::last_os_error()))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
fn native_noreplace(_: &Path, _: &Path) -> Option<io::Result<()>> {
    None
}

/// Rename `from` to `to`, failing with `AlreadyExists` instead of replacing anything at `to`.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(r) = native_noreplace(from, to) {
        return r;
    }
    // A hard link fails when the name is taken; then the old name goes.
    if std::fs::symlink_metadata(from)?.is_file() {
        match std::fs::hard_link(from, to) {
            Ok(()) => {
                return std::fs::remove_file(from).inspect_err(|_| {
                    let _ = std::fs::remove_file(to);
                })
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => return Err(e),
            Err(_) => {}
        }
    }
    // No hard links here either (FAT, some network disks): check, then rename. A folder
    // cannot replace a folder with anything in it, so at most an empty one is lost.
    if exists(to) {
        return Err(ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

/// Rename an item to a new name, never over another file. Only the letter case changing
/// on a disk that ignores case is the same file, and is renamed as such.
pub fn rename_item(from: &Path, to: &Path) -> io::Result<()> {
    let case_only = from.file_name().zip(to.file_name()).is_some_and(|(a, b)| a != b && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase());
    if case_only && from.parent() == to.parent() && same_file(from, to) {
        return std::fs::rename(from, to);
    }
    rename_noreplace(from, to)
}

#[cfg(unix)]
type FileKey = (u64, u64);
#[cfg(not(unix))]
type FileKey = String;

/// What identifies the directory entry at `p` (not what a link points to).
fn file_key(p: &Path) -> Option<FileKey> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(p).ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let real = p.parent()?.canonicalize().ok()?.join(p.file_name()?);
        exists(p).then(|| real.to_string_lossy().to_lowercase())
    }
}

/// Whether `a` and `b` are one file (two spellings of a name on a disk that ignores case).
pub fn same_file(a: &Path, b: &Path) -> bool {
    matches!((file_key(a), file_key(b)), (Some(x), Some(y)) if x == y)
}

/// The files one job has written, so it never overwrites its own work: `README` and
/// `readme` from a disk that tells them apart land on one that does not.
#[derive(Default)]
pub struct Written(HashSet<FileKey>);

impl Written {
    pub fn insert(&mut self, p: &Path) {
        if let Some(k) = file_key(p) {
            self.0.insert(k);
        }
    }
    pub fn contains(&self, p: &Path) -> bool {
        file_key(p).is_some_and(|k| self.0.contains(&k))
    }
}

/// Move the finished `src` (a temporary file, or a file being moved on its disk) to `dst`
/// by the conflict policy: nothing is replaced unless the policy is Overwrite, and never a
/// file this job wrote itself (that gets a free name). A name taken meanwhile is decided
/// again here. Returns where it went, or `None` when skipped; `src` is then left as it is.
pub fn place(src: &Path, dst: &Path, policy: Conflict, mut written: Option<&mut Written>) -> io::Result<Option<PathBuf>> {
    let mut target = dst.to_path_buf();
    loop {
        match rename_noreplace(src, &target) {
            Ok(()) => break,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let ours = written.as_deref().is_some_and(|w| w.contains(&target));
        match policy {
            Conflict::Skip => return Ok(None),
            Conflict::Overwrite if !ours => {
                match std::fs::symlink_metadata(&target) {
                    // Gone again meanwhile: try the free name.
                    Err(e) if e.kind() == ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                    // A folder is never replaced by a file.
                    Ok(m) if m.is_dir() => return Err(io::Error::new(ErrorKind::AlreadyExists, format!("{}: {}", crate::i18n::t("Already exists"), target.display()))),
                    Ok(_) => {}
                }
                // The new file is complete and synced: replacing is one atomic step.
                std::fs::rename(src, &target)?;
                break;
            }
            _ => target = unique_path(dst),
        }
    }
    if let Some(dir) = target.parent() {
        sync_dir(dir);
    }
    if let Some(w) = written.as_deref_mut() {
        w.insert(&target);
    }
    Ok(Some(target))
}

/// A hidden temporary file (or just its name) next to a target, removed again unless
/// `commit` moved it into place.
pub struct TempFile {
    path: PathBuf,
    file: Option<File>,
    done: bool,
}

impl TempFile {
    /// A new, empty temporary file next to `dst`, open for writing.
    pub fn beside(dst: &Path) -> io::Result<Self> {
        loop {
            let path = temp_path(dst, "part");
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(f) => return Ok(Self { path, file: Some(f), done: false }),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// Only an unused temporary name next to `dst`, for something else to create (a link,
    /// an unpacked entry). Whatever appears there is removed unless committed.
    pub fn name_beside(dst: &Path) -> Self {
        Self { path: temp_path(dst, "part"), file: None, done: false }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The open file. Panics for a `name_beside` temporary that has none.
    pub fn file(&mut self) -> &mut File {
        self.file.as_mut().expect("temporary file is open")
    }

    /// Sync the finished file and move it to `dst` by `policy` (see `place`).
    pub fn commit(mut self, dst: &Path, policy: Conflict, written: Option<&mut Written>) -> io::Result<Option<PathBuf>> {
        match self.file.take() {
            Some(f) => sync_file(&f)?,
            None => sync_path(&self.path)?,
        }
        let placed = place(&self.path, dst, policy, written)?;
        // Skipped: dropping it removes the temporary file.
        self.done = placed.is_some();
        Ok(placed)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        drop(self.file.take());
        match std::fs::symlink_metadata(&self.path) {
            Ok(m) if m.is_dir() => {
                let _ = std::fs::remove_dir_all(&self.path);
            }
            Ok(_) => {
                let _ = std::fs::remove_file(&self.path);
            }
            Err(_) => {}
        }
    }
}

/// Write a whole file safely: `fill` writes the content into a temporary file, which is
/// synced and moved to `dst` by `policy`. On any error nothing at `dst` changes.
#[allow(dead_code)] // for code that writes a whole file at once
pub fn write_atomic(dst: &Path, policy: Conflict, fill: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<Option<PathBuf>> {
    let mut tmp = TempFile::beside(dst)?;
    fill(tmp.file())?;
    tmp.commit(dst, policy, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("matrix_finder-safefs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn rename_never_replaces() {
        let d = dir("noreplace");
        std::fs::write(d.join("a"), "A").unwrap();
        std::fs::write(d.join("b"), "B").unwrap();
        let e = rename_noreplace(&d.join("a"), &d.join("b")).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(d.join("b")).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(d.join("a")).unwrap(), "A");
        rename_noreplace(&d.join("a"), &d.join("c")).unwrap();
        assert_eq!(names(&d), ["b", "c"]);
        // Folders too.
        std::fs::create_dir(d.join("x")).unwrap();
        std::fs::create_dir(d.join("y")).unwrap();
        assert_eq!(rename_noreplace(&d.join("x"), &d.join("y")).unwrap_err().kind(), ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_taken() {
        let d = dir("dangling");
        std::os::unix::fs::symlink(d.join("nowhere"), d.join("link")).unwrap();
        assert!(exists(&d.join("link")));
        assert_eq!(unique_path(&d.join("link")), d.join("link (1)"));
        std::fs::write(d.join("f"), "F").unwrap();
        assert_eq!(rename_noreplace(&d.join("f"), &d.join("link")).unwrap_err().kind(), ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn place_follows_the_policy_when_the_name_is_taken() {
        let d = dir("place");
        let dst = d.join("r.txt");
        std::fs::write(&dst, "old").unwrap();
        // Skip: the temporary file goes, the old one stays.
        assert_eq!(write_atomic(&dst, Conflict::Skip, |f| f.write_all(b"new")).unwrap(), None);
        assert_eq!(names(&d), ["r.txt"]);
        // Keep both.
        assert_eq!(write_atomic(&dst, Conflict::Rename, |f| f.write_all(b"new")).unwrap(), Some(d.join("r (1).txt")));
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "old");
        // Overwrite, once the new file is complete.
        assert_eq!(write_atomic(&dst, Conflict::Overwrite, |f| f.write_all(b"new")).unwrap(), Some(dst.clone()));
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new");
        assert_eq!(names(&d), ["r (1).txt", "r.txt"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_write_keeps_the_old_file_and_no_temporary_file() {
        let d = dir("fail");
        let dst = d.join("r.txt");
        std::fs::write(&dst, "precious").unwrap();
        let r = write_atomic(&dst, Conflict::Overwrite, |f| {
            f.write_all(b"half")?;
            Err(io::Error::other("disk full"))
        });
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "precious");
        assert_eq!(names(&d), ["r.txt"]);
        // A temporary file dropped without commit is gone too.
        let mut t = TempFile::beside(&dst).unwrap();
        t.file().write_all(b"x").unwrap();
        assert!(t.path().file_name().unwrap().to_string_lossy().starts_with(".r.txt.mf-part-"));
        drop(t);
        assert_eq!(names(&d), ["r.txt"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_job_never_overwrites_what_it_wrote() {
        let d = dir("written");
        let mut w = Written::default();
        let dst = d.join("README");
        let mut t = TempFile::beside(&dst).unwrap();
        t.file().write_all(b"first").unwrap();
        assert_eq!(t.commit(&dst, Conflict::Overwrite, Some(&mut w)).unwrap(), Some(dst.clone()));
        // The same name again (or another spelling of it on a disk that ignores case).
        let mut t = TempFile::beside(&dst).unwrap();
        t.file().write_all(b"second").unwrap();
        assert_eq!(t.commit(&dst, Conflict::Overwrite, Some(&mut w)).unwrap(), Some(d.join("README (1)")));
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "first");
        // A file the job did not write is overwritten as asked.
        std::fs::write(d.join("other"), "old").unwrap();
        write_atomic(&d.join("other"), Conflict::Overwrite, |f| f.write_all(b"new")).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("other")).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parallel_writers_of_one_name_keep_both() {
        let d = dir("parallel");
        let dst = d.join("same.txt");
        let (mut a, mut b) = (TempFile::beside(&dst).unwrap(), TempFile::beside(&dst).unwrap());
        a.file().write_all(b"A").unwrap();
        b.file().write_all(b"B").unwrap();
        // Both decided "Overwrite" back when nothing was there; each must keep its own file.
        let pa = a.commit(&dst, Conflict::Rename, None).unwrap().unwrap();
        let pb = b.commit(&dst, Conflict::Rename, None).unwrap().unwrap();
        assert_ne!(pa, pb);
        let mut got = [std::fs::read_to_string(pa).unwrap(), std::fs::read_to_string(pb).unwrap()];
        got.sort();
        assert_eq!(got, ["A", "B"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn syncing_works_in_the_temp_folder() {
        let d = dir("sync");
        let f = File::create(d.join("f")).unwrap();
        sync_file(&f).unwrap();
        sync_path(&d.join("f")).unwrap();
        sync_dir(&d);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn temp_names_fit_and_differ() {
        let long = "é".repeat(120);
        let n = temp_name(&long, "part");
        assert!(n.len() < 255, "{}", n.len());
        assert_ne!(temp_name("a", "part"), temp_name("a", "part"));
    }

    #[test]
    fn only_a_change_of_case_renames_onto_the_same_file() {
        let d = dir("case");
        std::fs::write(d.join("a"), "A").unwrap();
        rename_item(&d.join("a"), &d.join("A")).unwrap();
        assert_eq!(names(&d), ["A"]);
        std::fs::write(d.join("b"), "B").unwrap();
        assert_eq!(rename_item(&d.join("b"), &d.join("A")).unwrap_err().kind(), ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir_all(&d);
    }
}
