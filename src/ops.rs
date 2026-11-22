//! Background jobs (copy / move / delete / pack / unpack / backup) with progress.
use crate::i18n::t;
use crate::remote::SharedRemote;
use crate::safefs::{self, Conflict, TempFile, Written};
use crate::util::{es, human_size, join_slash, Res};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Progress {
    total: AtomicU64,
    done: AtomicU64,
    current: Mutex<String>,
    cancel: AtomicBool,
    paused: AtomicBool,
    queued: AtomicBool,
    /// Progress counts items (deleted files and folders), not bytes.
    items: AtomicBool,
}

impl Progress {
    pub fn add(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
        self.wait_if_paused();
    }
    /// Count progress without pausing here: for work that holds a remote connection,
    /// which must not sleep with it locked (it pauses between files instead).
    pub fn add_unpaused(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }
    /// From now on, progress counts items instead of bytes.
    pub fn count_items(&self) {
        self.items.store(true, Ordering::Relaxed);
    }
    pub fn counts_items(&self) -> bool {
        self.items.load(Ordering::Relaxed)
    }
    /// How much is done: "12 MB / 40 MB", or "1 / 3 items" for a job that counts items.
    pub fn amount_text(&self) -> String {
        let (done, total) = (self.done(), self.total());
        if self.counts_items() {
            if total > 0 {
                t("{done} / {total} items").replace("{done}", &done.to_string()).replace("{total}", &total.to_string())
            } else {
                format!("{done} {}", t("items"))
            }
        } else if total > 0 {
            format!("{} / {}", human_size(done), human_size(total))
        } else {
            human_size(done)
        }
    }
    /// Blocks the worker while the job is paused (every operation reports
    /// progress regularly, so this pauses copies, archives and transfers alike).
    fn wait_if_paused(&self) {
        while self.paused.load(Ordering::Relaxed) && !self.cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
    pub fn is_queued(&self) -> bool {
        self.queued.load(Ordering::Relaxed)
    }
    /// Let a queued job start.
    pub fn release(&self) {
        self.queued.store(false, Ordering::Relaxed);
    }
    pub fn add_total(&self, n: u64) {
        self.total.fetch_add(n, Ordering::Relaxed);
    }
    pub fn set_total(&self, n: u64) {
        self.total.store(n, Ordering::Relaxed);
    }
    pub fn set_current(&self, s: &str) {
        if let Ok(mut c) = self.current.lock() {
            c.clear();
            c.push_str(s);
        }
    }
    pub fn current(&self) -> String {
        self.current.lock().map(|c| c.clone()).unwrap_or_default()
    }
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
    pub fn fraction(&self) -> Option<f32> {
        let t = self.total();
        (t > 0).then(|| (self.done() as f64 / t as f64).min(1.0) as f32)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }
    pub fn check(&self) -> Res<()> {
        self.wait_if_paused();
        if self.cancelled() {
            Err(t("Cancelled"))
        } else {
            Ok(())
        }
    }
}

/// How far along these jobs are together, by bytes. `None` until one of them knows its size.
/// Jobs that count items (deletes) cannot be weighed against bytes: they only count
/// while no job with a size in bytes runs, and then by their items.
pub fn overall_fraction(jobs: &[&Progress]) -> Option<f32> {
    let sum = |items: bool| {
        jobs.iter().filter(|p| p.counts_items() == items && p.total() > 0).fold((0u64, 0u64), |(d, n), p| (d + p.done().min(p.total()), n + p.total()))
    };
    let (done, total) = match sum(false) {
        (_, 0) => sum(true),
        bytes => bytes,
    };
    (total > 0).then(|| (done as f64 / total as f64) as f32)
}

pub struct Job {
    pub title: String,
    pub progress: Arc<Progress>,
    rx: Receiver<Res<String>>,
}

impl Job {
    /// Start a job. A `queued` job waits until `Progress::release` is called.
    pub fn spawn<F>(title: impl Into<String>, queued: bool, f: F) -> Job
    where
        F: FnOnce(&Progress) -> Res<String> + Send + 'static,
    {
        let progress = Arc::new(Progress::default());
        progress.queued.store(queued, Ordering::Relaxed);
        let (tx, rx) = channel();
        let p = progress.clone();
        std::thread::spawn(move || {
            while p.is_queued() && !p.cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if p.cancelled() {
                let _ = tx.send(Err(t("Cancelled")));
                return;
            }
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&p)))
                .unwrap_or_else(|_| Err("Internal error (panic)".into()));
            let _ = tx.send(r);
        });
        Job {
            title: title.into(),
            progress,
            rx,
        }
    }

    /// `Some(result)` once finished.
    pub fn poll(&self) -> Option<Res<String>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Job aborted".into())),
        }
    }
}

/// One end of a transfer: a directory somewhere.
#[derive(Clone)]
pub enum Endpoint {
    Local(PathBuf),
    Remote(SharedRemote, u64, String),
    Archive(PathBuf, String),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

fn local_size(p: &Path) -> u64 {
    walkdir::WalkDir::new(p)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Shown when a copy does not match its source, and a move therefore keeps the source.
fn not_verified(name: &str) -> String {
    format!("{}: {name}", t("The copy could not be verified, so the original was kept"))
}

/// Copy one file through a hidden temporary file next to `dst`, which is synced and only
/// then moved into place by `policy`: a cancel, full disk, read error or crash keeps an
/// existing `dst`. With `verify` (a move), the source must not have changed size while it
/// was copied. Returns where the copy went, or `None` when it was skipped.
fn copy_file_progress(src: &Path, dst: &Path, policy: Conflict, verify: bool, written: &mut Written, p: &Progress) -> Res<Option<PathBuf>> {
    let mut tmp = TempFile::beside(dst).map_err(es)?;
    let copied = copy_file_inner(src, tmp.file(), p)?;
    let complete = tmp.file().metadata().map_err(es)?.len() == copied;
    if !complete || (verify && std::fs::metadata(src).map_err(es)?.len() != copied) {
        return Err(not_verified(&src.to_string_lossy()));
    }
    tmp.commit(dst, policy, Some(written)).map_err(es)
}

/// Copy `src` into `w`, with its permissions and modification time. Returns the bytes copied.
fn copy_file_inner(src: &Path, w: &mut std::fs::File, p: &Progress) -> Res<u64> {
    use std::io::{Read, Write};
    let mut r = std::fs::File::open(src).map_err(es)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut copied = 0u64;
    loop {
        p.check()?;
        let n = r.read(&mut buf).map_err(es)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).map_err(es)?;
        copied += n as u64;
        p.add(n as u64);
    }
    if let Ok(meta) = r.metadata() {
        let _ = w.set_permissions(meta.permissions());
        if let Ok(m) = meta.modified() {
            let _ = w.set_modified(m);
        }
    }
    Ok(copied)
}

/// `p` with its folder resolved through symlinks, but not `p` itself.
fn real_entry(p: &Path) -> Option<PathBuf> {
    Some(p.parent()?.canonicalize().ok()?.join(p.file_name()?))
}

/// True when `a` and `b` are the same directory entry, e.g. `/tmp/x` and `/private/tmp/x`.
fn same_entry(a: &Path, b: &Path) -> bool {
    a == b || matches!((real_entry(a), real_entry(b)), (Some(x), Some(y)) if x == y)
}

/// Sources a move deletes once its whole item is copied and verified: (path, is a folder),
/// each folder after what was in it.
type Gone<T> = Vec<(T, bool)>;

/// Delete the sources of a finished local move.
fn delete_local_sources(gone: Gone<PathBuf>) -> Res<()> {
    for (path, is_dir) in gone {
        if is_dir {
            std::fs::remove_dir(&path).map_err(es)?;
        } else {
            std::fs::remove_file(&path).map_err(es)?;
        }
    }
    Ok(())
}

/// Copy `src` to `dst`. For a move, `gone` collects the sources to delete once the whole
/// item is done (a file renamed on the same disk moves at once). Returns false when
/// something was skipped (by the conflict policy, or because it already is the
/// destination); its source, and the folders holding it, are kept.
fn copy_local_recursive(src: &Path, dst: &Path, policy: Conflict, mut gone: Option<&mut Gone<PathBuf>>, written: &mut Written, p: &Progress) -> Res<bool> {
    p.check()?;
    let meta = std::fs::symlink_metadata(src).map_err(es)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dst).map_err(es)?;
        let mut complete = true;
        for e in std::fs::read_dir(src).map_err(es)?.flatten() {
            complete &= copy_local_recursive(&e.path(), &dst.join(e.file_name()), policy, gone.as_deref_mut(), written, p)?;
        }
        if let Some(g) = gone.filter(|_| complete) {
            g.push((src.to_path_buf(), true));
        }
        return Ok(complete);
    }
    // Skipped, but done: the bar still reaches the end.
    if policy == Conflict::Skip && safefs::exists(dst) || safefs::exists(dst) && same_entry(src, dst) {
        p.add(meta.len());
        return Ok(false);
    }
    p.set_current(&src.to_string_lossy());
    #[cfg(unix)]
    if meta.file_type().is_symlink() {
        // Made under a temporary name, then moved over: an existing `dst` is only replaced by a finished link.
        let target = std::fs::read_link(src).map_err(es)?;
        let tmp = TempFile::name_beside(dst);
        std::os::unix::fs::symlink(target, tmp.path()).map_err(es)?;
        if tmp.commit(dst, policy, Some(written)).map_err(es)?.is_none() {
            return Ok(false);
        }
        if let Some(g) = gone {
            g.push((src.to_path_buf(), false));
        }
        return Ok(true);
    }
    if gone.is_some() {
        // On the same disk a rename moves it whole (never over another file, unless the policy says so).
        if let Ok(placed) = safefs::place(src, dst, policy, Some(written)) {
            p.add(meta.len());
            return Ok(placed.is_some());
        }
    }
    if copy_file_progress(src, dst, policy, gone.is_some(), written, p)?.is_none() {
        return Ok(false);
    }
    if let Some(g) = gone {
        g.push((src.to_path_buf(), false));
    }
    Ok(true)
}

/// `dir/name` when `name` is one plain file name. Names come from the server, and one
/// such as `../../.zshrc` or `/Users/me/.zshrc` must not lead out of `dir`.
fn local_child(dir: &Path, name: &str) -> Res<PathBuf> {
    let mut c = Path::new(name).components();
    match (c.next(), c.next()) {
        (Some(std::path::Component::Normal(n)), None) if n == std::ffi::OsStr::new(name) => Ok(dir.join(n)),
        _ => Err(format!("{}: {name:?}", t("Refusing an unsafe file name from the server"))),
    }
}

/// A name like `name` that `taken` does not have, the way `unique_path` makes one:
/// `report.pdf` becomes `report (1).pdf`, then `report (2).pdf`.
fn unique_name(name: &str, mut taken: impl FnMut(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.to_string());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..).map(|n| format!("{stem} ({n}){ext}")).find(|c| !taken(c)).unwrap_or_default()
}

/// Move the finished remote file or folder `from` to `dir/name` by the conflict policy,
/// deciding on a fresh look at the folder. Returns the path it got, or `None` when skipped
/// (`from` is then left for the caller). Overwrite first renames the old file to a hidden
/// name and deletes it only once the new one is in place, so one of them always exists.
/// An upload (`upload`) takes the permissions of the file it replaces. (FTP servers often
/// replace on rename: a file that appears between the check and the rename can still be
/// replaced there.)
fn remote_place(g: &mut dyn crate::remote::RemoteFs, from: &str, dir: &str, name: &str, policy: Conflict, upload: bool) -> Res<Option<String>> {
    for _ in 0..3 {
        let target = join_slash(dir, name);
        let free = if !g.exists(&target)? {
            target
        } else {
            match policy {
                Conflict::Skip => return Ok(None),
                Conflict::Rename => {
                    let mut probe = Ok(());
                    let free = unique_name(name, |n| match g.exists(&join_slash(dir, n)) {
                        Ok(b) => b,
                        Err(e) => {
                            probe = Err(e);
                            false
                        }
                    });
                    probe?;
                    join_slash(dir, &free)
                }
                Conflict::Overwrite => {
                    // A folder is never replaced by a file.
                    if g.list(dir)?.iter().any(|e| e.name == name && e.is_dir) {
                        return Ok(None);
                    }
                    let old = join_slash(dir, &safefs::temp_name(name, "old"));
                    if upload {
                        g.copy_mode(&target, from);
                    }
                    g.rename(&target, &old)?;
                    if let Err(e) = g.rename(from, &target) {
                        return Err(match g.rename(&old, &target) {
                            Ok(()) => e,
                            Err(_) => format!("{e} ({}: {old})", t("it is now named")),
                        });
                    }
                    // Should this fail, the old file stays under its hidden name.
                    let _ = g.remove_file(&old);
                    return Ok(Some(target));
                }
            }
        };
        match g.rename(from, &free) {
            Ok(()) => return Ok(Some(free)),
            // Taken meanwhile: decide again.
            Err(_) if g.exists(&free)? => continue,
            Err(e) => return Err(e),
        }
    }
    Err(format!("{}: {name}", t("Already exists")))
}

/// Download a remote file or folder. For a move, `gone` collects the remote sources to
/// delete once the whole item is downloaded and verified. Returns false when something was
/// skipped, and then keeps the remote folders holding it. A pause takes effect between
/// files: the connection stays locked during a file, and nothing may sleep while holding it.
#[allow(clippy::too_many_arguments)]
fn download_recursive(fs: &SharedRemote, rpath: &str, lpath: &Path, is_dir: bool, size: u64, policy: Conflict, mut gone: Option<&mut Gone<String>>, written: &mut Written, p: &Progress) -> Res<bool> {
    p.check()?;
    if is_dir {
        std::fs::create_dir_all(lpath).map_err(es)?;
        // A link to a folder: download what it points to, but delete only the link.
        let link = gone.is_some() && fs.lock().map_err(es)?.is_symlink(rpath)?;
        let list = fs.lock().map_err(es)?.list(rpath)?;
        let mut complete = true;
        for e in list {
            let child = local_child(lpath, &e.name)?;
            let inner = if link { None } else { gone.as_deref_mut() };
            complete &= download_recursive(fs, &join_slash(rpath, &e.name), &child, e.is_dir, e.size, policy, inner, written, p)?;
        }
        if let Some(g) = gone.filter(|_| complete) {
            // A link goes as a file.
            g.push((rpath.to_string(), !link));
        }
        Ok(complete)
    } else {
        if policy == Conflict::Skip && safefs::exists(lpath) {
            // Skipped, but done: the bar still reaches the end.
            p.add(size);
            return Ok(false);
        }
        p.set_current(rpath);
        // Into a temporary file first, so a failed download keeps an existing file.
        let tmp = TempFile::name_beside(lpath);
        let mut g = fs.lock().map_err(es)?;
        g.download(rpath, tmp.path(), &|n| p.add_unpaused(n), p.cancel_flag())?;
        // Before a move deletes the remote file, the copy must have its size.
        let mut verified = true;
        if gone.is_some() {
            let got = std::fs::metadata(tmp.path()).map_err(es)?.len();
            match g.file_size(rpath)? {
                Some(n) if n != got => return Err(not_verified(rpath)),
                Some(_) => {}
                None => verified = false,
            }
        }
        drop(g);
        if policy == Conflict::Overwrite {
            // An overwritten file keeps its permissions, as it would if written in place.
            if let Ok(meta) = std::fs::metadata(lpath) {
                let _ = std::fs::set_permissions(tmp.path(), meta.permissions());
            }
        }
        if tmp.commit(lpath, policy, Some(written)).map_err(es)?.is_none() || !verified {
            return Ok(false);
        }
        if let Some(g) = gone {
            g.push((rpath.to_string(), false));
        }
        Ok(true)
    }
}

/// Upload a local file or folder. For a move, `gone` collects the local sources to delete
/// once the whole item is uploaded and verified. Returns false when something was skipped,
/// and then keeps the local folders holding it. Like downloads, it pauses between files.
#[allow(clippy::too_many_arguments)]
fn upload_recursive(fs: &SharedRemote, lpath: &Path, rdir: &str, name: &str, existing: Option<&HashSet<String>>, policy: Conflict, mut gone: Option<&mut Gone<PathBuf>>, p: &Progress) -> Res<bool> {
    p.check()?;
    let rpath = join_slash(rdir, name);
    let exists = existing.is_some_and(|s| s.contains(name));
    if lpath.is_dir() {
        // A link to a folder: upload what it points to, but delete only the link.
        let link = std::fs::symlink_metadata(lpath).map_err(es)?.file_type().is_symlink();
        if !exists {
            fs.lock().map_err(es)?.mkdir(&rpath)?;
        }
        let children: HashSet<String> = if exists {
            fs.lock().map_err(es)?.list(&rpath)?.into_iter().map(|e| e.name).collect()
        } else {
            HashSet::new()
        };
        let mut complete = true;
        for e in std::fs::read_dir(lpath).map_err(es)?.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let inner = if link { None } else { gone.as_deref_mut() };
            complete &= upload_recursive(fs, &e.path(), &rpath, &n, Some(&children), policy, inner, p)?;
        }
        if let Some(g) = gone.filter(|_| complete) {
            g.push((lpath.to_path_buf(), !link));
        }
        Ok(complete)
    } else {
        let len = std::fs::metadata(lpath).map(|m| m.len()).unwrap_or(0);
        if exists && policy == Conflict::Skip {
            // Skipped, but done: the bar still reaches the end.
            p.add(len);
            return Ok(false);
        }
        p.set_current(&lpath.to_string_lossy());
        // Under a hidden temporary name, checked, then renamed into place: an existing
        // file is never truncated, and a cancelled upload leaves nothing under the real name.
        let tmp = join_slash(rdir, &safefs::temp_name(name, "part"));
        let mut g = fs.lock().map_err(es)?;
        let uploaded = g.upload(lpath, &tmp, &|n| p.add_unpaused(n), p.cancel_flag()).and_then(|_| match g.file_size(&tmp)? {
            Some(n) if n != len => Err(not_verified(&lpath.to_string_lossy())),
            known => Ok(known.is_some()),
        });
        let placed = uploaded.and_then(|verified| Ok((verified, remote_place(&mut **g, &tmp, rdir, name, policy, true)?)));
        let (verified, placed) = match placed {
            Ok((v, Some(_))) => (v, true),
            r => {
                let _ = g.remove_file(&tmp);
                r?;
                (false, false)
            }
        };
        drop(g);
        if !placed {
            return Ok(false);
        }
        match gone {
            Some(g) if verified => g.push((lpath.to_path_buf(), false)),
            // The server did not say the size: the local file stays.
            Some(_) => return Ok(false),
            None => {}
        }
        Ok(true)
    }
}

/// Size of a remote file or folder; stops (with a partial sum) once the job is cancelled.
fn remote_size(fs: &SharedRemote, path: &str, is_dir: bool, size: u64, p: &Progress) -> u64 {
    if !is_dir {
        return size;
    }
    if p.cancelled() {
        return 0;
    }
    let list = match fs.lock() {
        Ok(mut g) => g.list(path).unwrap_or_default(),
        Err(_) => return 0,
    };
    list.iter()
        .map(|e| remote_size(fs, &join_slash(path, &e.name), e.is_dir, e.size, p))
        .sum()
}

/// Copy or move `items` from `src` to `dst`.
pub fn transfer(src: Endpoint, items: Vec<Item>, dst: Endpoint, mv: bool, policy: Conflict, p: &Progress) -> Res<String> {
    let count = items.len();
    // False once anything was skipped (moves then keep those sources).
    let mut complete = true;
    // What this job wrote: it never overwrites its own files.
    let mut written = Written::default();
    match (&src, &dst) {
        (Endpoint::Local(sdir), Endpoint::Local(ddir)) => {
            p.set_current(&t("Calculating…"));
            if !mv {
                p.set_total(items.iter().map(|i| if i.is_dir { local_size(&sdir.join(&i.name)) } else { i.size }).sum());
            }
            let real_ddir = ddir.canonicalize().unwrap_or_else(|_| ddir.clone());
            for it in &items {
                p.check()?;
                let s = sdir.join(&it.name);
                let d = ddir.join(&it.name);
                if same_entry(&s, &d) {
                    if mv {
                        continue;
                    }
                    // Copy onto itself: always create a renamed copy.
                    copy_local_recursive(&s, &safefs::unique_path(&d), Conflict::Rename, None, &mut written, p)?;
                    continue;
                }
                if it.is_dir && (ddir.starts_with(&s) || real_entry(&s).is_some_and(|rs| real_ddir.starts_with(rs))) {
                    return Err(t("Cannot copy “{name}” into itself").replace("{name}", &it.name));
                }
                if mv {
                    // On the same disk, into a free name: one rename moves it all.
                    if safefs::rename_noreplace(&s, &d).is_ok() {
                        written.insert(&d);
                        safefs::sync_dir(ddir);
                        continue;
                    }
                    // Another disk, or a merge: copy everything, then delete the sources.
                    p.add_total(if it.is_dir { local_size(&s) } else { it.size });
                    let mut gone = Vec::new();
                    complete &= copy_local_recursive(&s, &d, policy, Some(&mut gone), &mut written, p)?;
                    delete_local_sources(gone)?;
                } else {
                    complete &= copy_local_recursive(&s, &d, policy, None, &mut written, p)?;
                }
            }
        }
        (Endpoint::Remote(fs, _, rdir), Endpoint::Local(ddir)) => {
            p.set_current(&t("Calculating…"));
            p.set_total(items.iter().map(|i| remote_size(fs, &join_slash(rdir, &i.name), i.is_dir, i.size, p)).sum());
            for it in &items {
                let rp = join_slash(rdir, &it.name);
                let mut gone = Vec::new();
                complete &= download_recursive(fs, &rp, &local_child(ddir, &it.name)?, it.is_dir, it.size, policy, mv.then_some(&mut gone), &mut written, p)?;
                if !gone.is_empty() {
                    let mut g = fs.lock().map_err(es)?;
                    for (path, is_dir) in gone {
                        if is_dir {
                            g.remove_dir(&path)?;
                        } else {
                            g.remove_file(&path)?;
                        }
                    }
                }
            }
        }
        (Endpoint::Local(sdir), Endpoint::Remote(fs, _, rdir)) => {
            p.set_current(&t("Calculating…"));
            p.set_total(items.iter().map(|i| if i.is_dir { local_size(&sdir.join(&i.name)) } else { i.size }).sum());
            let existing: HashSet<String> = fs.lock().map_err(es)?.list(rdir)?.into_iter().map(|e| e.name).collect();
            for it in &items {
                let lp = sdir.join(&it.name);
                let mut gone = Vec::new();
                complete &= upload_recursive(fs, &lp, rdir, &it.name, Some(&existing), policy, mv.then_some(&mut gone), p)?;
                delete_local_sources(gone)?;
            }
        }
        (Endpoint::Remote(fs, a, sdir), Endpoint::Remote(_, b, ddir)) if a == b && mv => {
            let mut g = fs.lock().map_err(es)?;
            for it in &items {
                p.check()?;
                let from = join_slash(sdir, &it.name);
                if sdir.trim_end_matches('/') == ddir.trim_end_matches('/') {
                    continue;
                }
                if it.is_dir && (ddir == &from || ddir.starts_with(&format!("{}/", from.trim_end_matches('/')))) {
                    return Err(t("Cannot copy “{name}” into itself").replace("{name}", &it.name));
                }
                complete &= remote_place(&mut **g, &from, ddir, &it.name, policy, false)?.is_some();
            }
        }
        (Endpoint::Archive(file, inner), Endpoint::Local(ddir)) => {
            if mv {
                return Err(t("Cannot move out of an archive (archives are read-only). Use copy."));
            }
            let sel: Vec<String> = items.iter().map(|i| join_slash(inner, &i.name)).collect();
            crate::archive::extract(file, ddir, &sel, inner, policy, p)?;
        }
        (_, Endpoint::Archive(..)) => {
            return Err(t("Archives are read-only here. Use Compress to create a new archive."));
        }
        _ => return Err(t("This transfer combination is not supported (copy via a local folder).")),
    }
    let msg = match (mv, complete) {
        (false, true) => t("Copied {n} item(s)"),
        (true, true) => t("Moved {n} item(s)"),
        (false, false) => t("Copied {n} item(s); some files were skipped"),
        (true, false) => t("Moved {n} item(s); skipped files were left in place"),
    };
    Ok(msg.replace("{n}", &count.to_string()))
}

pub fn delete(src: Endpoint, items: Vec<Item>, to_trash: bool, p: &Progress) -> Res<String> {
    let count = items.len();
    p.count_items();
    match src {
        Endpoint::Local(dir) => {
            let paths: Vec<PathBuf> = items.iter().map(|i| dir.join(&i.name)).collect();
            p.set_total(paths.len() as u64);
            if to_trash {
                trash::delete_all(&paths).map_err(es)?;
                p.add(paths.len() as u64);
            } else {
                for (path, it) in paths.iter().zip(&items) {
                    p.check()?;
                    p.set_current(&path.to_string_lossy());
                    if it.is_dir {
                        std::fs::remove_dir_all(path).map_err(es)?;
                    } else {
                        std::fs::remove_file(path).map_err(es)?;
                    }
                    p.add(1);
                }
            }
        }
        Endpoint::Remote(fs, _, dir) => {
            p.set_total(items.len() as u64);
            for it in &items {
                p.check()?;
                let path = join_slash(&dir, &it.name);
                p.set_current(&path);
                let mut g = fs.lock().map_err(es)?;
                crate::remote::remove_recursive(&mut **g, &path, it.is_dir)?;
                // Not paused while the connection is locked.
                drop(g);
                p.add(1);
            }
        }
        Endpoint::Archive(..) => return Err(t("Archives are read-only")),
    }
    Ok(t("Deleted {n} item(s)").replace("{n}", &count.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_copy_leaves_no_partial_file() {
        let base = std::env::temp_dir().join(format!("matrix_finder-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("dst")).unwrap();
        std::fs::write(base.join("big.bin"), vec![7u8; 4 * 1024 * 1024]).unwrap();
        let p = Progress::default();
        p.cancel();
        let r = transfer(
            Endpoint::Local(base.clone()),
            vec![Item { name: "big.bin".into(), is_dir: false, size: 4 * 1024 * 1024 }],
            Endpoint::Local(base.join("dst")),
            false,
            Conflict::Overwrite,
            &p,
        );
        assert!(r.is_err());
        assert!(!base.join("dst/big.bin").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn overall_fraction_is_weighted_by_size() {
        assert_eq!(overall_fraction(&[]), None);
        let (small, big, unsized_job) = (Progress::default(), Progress::default(), Progress::default());
        assert_eq!(overall_fraction(&[&unsized_job]), None, "no size known yet");
        small.set_total(100);
        small.add(100);
        big.set_total(300);
        unsized_job.add(50);
        assert_eq!(overall_fraction(&[&small, &big, &unsized_job]), Some(0.25));
        // A job that reports more than it expected does not push it past the end.
        big.add(900);
        assert_eq!(overall_fraction(&[&small, &big]), Some(1.0));
        // Items are not weighed against bytes, but count when nothing has a size in bytes.
        let deleting = Progress::default();
        deleting.count_items();
        deleting.set_total(4);
        deleting.add(1);
        assert_eq!(overall_fraction(&[&small, &deleting]), Some(1.0));
        assert_eq!(overall_fraction(&[&deleting, &unsized_job]), Some(0.25));
    }

    #[test]
    fn deletes_count_items_not_bytes() {
        let b = tmp("delete-items");
        write(&b.join("a.txt"), "a");
        write(&b.join("d/b.txt"), "b");
        let p = Progress::default();
        let msg = delete(local(&b), vec![file("a.txt"), dir("d")], false, &p).unwrap();
        assert_eq!(msg, "Deleted 2 item(s)");
        assert!(p.counts_items());
        assert_eq!(p.amount_text(), "2 / 2 items");
        let bytes = Progress::default();
        bytes.set_total(2048);
        assert!(!bytes.amount_text().contains("items"));
    }

    #[test]
    fn unique_names_keep_the_extension() {
        let taken = ["report.pdf", "report (1).pdf", ".bashrc", "notes"];
        let free = |n: &str| unique_name(n, |c| taken.contains(&c));
        assert_eq!(free("other.pdf"), "other.pdf");
        assert_eq!(free("report.pdf"), "report (2).pdf");
        assert_eq!(free(".bashrc"), ".bashrc (1)");
        assert_eq!(free("notes"), "notes (1)");
    }

    #[test]
    fn upload_keep_both_picks_a_free_name() {
        let b = tmp("up-rename");
        let (r, l) = (b.join("remote"), b.join("local"));
        write(&l.join("a.txt"), "new");
        write(&r.join("a.txt"), "old");
        write(&r.join("a (1).txt"), "older");
        transfer(local(&l), vec![file("a.txt")], remote(DiskRemote::default(), &r), false, Conflict::Rename, &Progress::default()).unwrap();
        assert_eq!(read(&r.join("a.txt")), "old");
        assert_eq!(read(&r.join("a (1).txt")), "older");
        assert_eq!(read(&r.join("a (2).txt")), "new");
    }

    #[test]
    fn skipped_remote_files_still_fill_the_bar() {
        let b = tmp("skip-progress");
        let (r, l) = (b.join("remote"), b.join("local"));
        write(&l.join("a.txt"), "local");
        write(&r.join("a.txt"), "remote");
        let p = Progress::default();
        transfer(local(&l), vec![Item { name: "a.txt".into(), is_dir: false, size: 5 }], remote(DiskRemote::default(), &r), false, Conflict::Skip, &p).unwrap();
        assert_eq!((p.done(), p.total()), (5, 5));
        let p = Progress::default();
        transfer(remote(DiskRemote::default(), &r), vec![Item { name: "a.txt".into(), is_dir: false, size: 6 }], local(&l), false, Conflict::Skip, &p).unwrap();
        assert_eq!((p.done(), p.total()), (6, 6));
        assert_eq!(read(&l.join("a.txt")), "local");
    }

    /// A paused remote transfer does not sleep while it holds the connection.
    #[test]
    fn remote_progress_does_not_pause_mid_file() {
        let p = Progress::default();
        p.pause();
        let t0 = std::time::Instant::now();
        p.add_unpaused(10);
        assert_eq!(p.done(), 10);
        assert!(t0.elapsed() < std::time::Duration::from_millis(50));
    }

    #[test]
    fn the_ui_never_waits_for_a_busy_connection() {
        let b = tmp("busy");
        let Endpoint::Remote(fs, ..) = remote(DiskRemote::default(), &b) else { unreachable!() };
        let held = fs.lock().unwrap();
        assert_eq!(crate::remote::try_use(&fs).err(), Some(crate::remote::busy_text()));
        let t0 = std::time::Instant::now();
        assert!(crate::remote::use_soon(&fs, std::time::Duration::from_secs(5), &|| true).is_err(), "gives up once cancelled");
        assert!(t0.elapsed() < std::time::Duration::from_secs(1));
        drop(held);
        assert!(crate::remote::try_use(&fs).is_ok());
    }

    #[test]
    fn queue_pause_resume() {
        let job = Job::spawn("t", true, |p| {
            for _ in 0..5 {
                p.add(1);
            }
            Ok("done".into())
        });
        std::thread::sleep(std::time::Duration::from_millis(250));
        assert!(job.poll().is_none(), "queued job must not run");
        assert_eq!(job.progress.done(), 0);
        job.progress.pause();
        job.progress.release();
        std::thread::sleep(std::time::Duration::from_millis(250));
        // First `add` blocks while paused.
        assert_eq!(job.progress.done(), 1);
        assert!(job.poll().is_none());
        job.progress.resume();
        let mut r = None;
        for _ in 0..50 {
            r = job.poll();
            if r.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(r, Some(Ok("done".to_string())));
        assert_eq!(job.progress.done(), 5);
        // Cancelling a queued job finishes it without running.
        let job = Job::spawn("q", true, |_| Ok("ran".into()));
        job.progress.cancel();
        std::thread::sleep(std::time::Duration::from_millis(250));
        assert_eq!(job.poll(), Some(Err(t("Cancelled"))));
    }

    #[test]
    fn local_copy_move() {
        let base = std::env::temp_dir().join(format!("matrix_finder-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("a");
        let b = base.join("b");
        std::fs::create_dir_all(a.join("d")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("f.txt"), "x").unwrap();
        std::fs::write(a.join("d/g.txt"), "y").unwrap();
        let items = vec![
            Item { name: "f.txt".into(), is_dir: false, size: 1 },
            Item { name: "d".into(), is_dir: true, size: 0 },
        ];
        let p = Progress::default();
        transfer(Endpoint::Local(a.clone()), items.clone(), Endpoint::Local(b.clone()), false, Conflict::Skip, &p).unwrap();
        assert!(b.join("d/g.txt").exists());
        assert_eq!(p.done(), 2);
        // Copy again with rename policy -> f (1).txt
        transfer(Endpoint::Local(a.clone()), items[..1].to_vec(), Endpoint::Local(b.clone()), false, Conflict::Rename, &p).unwrap();
        assert!(b.join("f (1).txt").exists());
        // Move
        let c = base.join("c");
        std::fs::create_dir_all(&c).unwrap();
        transfer(Endpoint::Local(a.clone()), items, Endpoint::Local(c.clone()), true, Conflict::Overwrite, &p).unwrap();
        assert!(c.join("d/g.txt").exists());
        assert!(!a.join("d").exists());
        // Into itself
        let err = transfer(
            Endpoint::Local(base.clone()),
            vec![Item { name: "c".into(), is_dir: true, size: 0 }],
            Endpoint::Local(c.join("d")),
            false,
            Conflict::Skip,
            &p,
        );
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A temporary folder, removed again at the end of the test.
    struct TestDir(PathBuf);

    impl std::ops::Deref for TestDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for TestDir {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(name: &str) -> TestDir {
        let d = std::env::temp_dir().join(format!("matrix_finder-ops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TestDir(d)
    }

    fn file(name: &str) -> Item {
        Item { name: name.into(), is_dir: false, size: 1 }
    }

    fn dir(name: &str) -> Item {
        Item { name: name.into(), is_dir: true, size: 0 }
    }

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    fn local(p: &Path) -> Endpoint {
        Endpoint::Local(p.to_path_buf())
    }

    /// A "server" on the local disk that lists links the way SFTP does: a link to a folder
    /// reports `is_dir` (from its target) and `is_link`.
    #[derive(Default)]
    struct DiskRemote {
        /// Extra names to list in every folder, as a malicious server would.
        inject: Option<String>,
        fail_downloads: bool,
        /// Fail uploads after writing part of the file.
        fail_uploads: bool,
        rename_replaces: bool,
    }

    impl crate::remote::RemoteFs for DiskRemote {
        fn home(&mut self) -> String {
            "/".into()
        }
        fn list(&mut self, path: &str) -> Res<Vec<crate::vfs::Entry>> {
            let mut out = vec![];
            for e in std::fs::read_dir(path).map_err(es)?.flatten() {
                let is_dir = e.path().is_dir();
                let size = if is_dir { 0 } else { e.metadata().map_err(es)?.len() };
                let mut en = crate::vfs::Entry::new(e.file_name().to_string_lossy().to_string(), is_dir, size, None);
                en.is_link = e.path().symlink_metadata().map_err(es)?.file_type().is_symlink();
                out.push(en);
            }
            if let Some(n) = &self.inject {
                out.push(crate::vfs::Entry::new(n.clone(), false, 0, None));
            }
            Ok(out)
        }
        fn download(&mut self, remote: &str, local: &Path, _: crate::remote::ProgressFn, _: &AtomicBool) -> Res<()> {
            if self.fail_downloads {
                std::fs::write(local, "partial").map_err(es)?;
                return Err("connection lost".into());
            }
            std::fs::copy(remote, local).map(|_| ()).map_err(es)
        }
        fn upload(&mut self, local: &Path, remote: &str, _: crate::remote::ProgressFn, _: &AtomicBool) -> Res<()> {
            if self.fail_uploads {
                std::fs::write(remote, "partial").map_err(es)?;
                return Err("connection lost".into());
            }
            std::fs::copy(local, remote).map(|_| ()).map_err(es)
        }
        fn read_head(&mut self, _: &str, _: usize, _: &AtomicBool) -> Res<Vec<u8>> {
            Ok(vec![])
        }
        fn mkdir(&mut self, path: &str) -> Res<()> {
            std::fs::create_dir(path).map_err(es)
        }
        fn remove_file(&mut self, path: &str) -> Res<()> {
            std::fs::remove_file(path).map_err(es)
        }
        fn remove_dir(&mut self, path: &str) -> Res<()> {
            std::fs::remove_dir(path).map_err(es)
        }
        fn rename(&mut self, from: &str, to: &str) -> Res<()> {
            // Like an FTP server: an existing file is replaced.
            if self.rename_replaces {
                return std::fs::rename(from, to).map_err(es);
            }
            safefs::rename_noreplace(Path::new(from), Path::new(to)).map_err(es)
        }
        fn create_file(&mut self, path: &str) -> Res<()> {
            std::fs::OpenOptions::new().write(true).create_new(true).open(path).map(|_| ()).map_err(es)
        }
        fn is_symlink(&mut self, path: &str) -> Res<bool> {
            Ok(std::fs::symlink_metadata(path).map_err(es)?.file_type().is_symlink())
        }
    }

    fn remote(fs: DiskRemote, p: &Path) -> Endpoint {
        let boxed: Box<dyn crate::remote::RemoteFs> = Box::new(fs);
        Endpoint::Remote(Arc::new(Mutex::new(boxed)), 1, p.to_string_lossy().to_string())
    }

    #[test]
    fn folder_move_with_skip_keeps_what_it_skipped() {
        let b = tmp("merge-skip");
        write(&b.join("a/d/report.txt"), "source");
        write(&b.join("a/d/sub/other.txt"), "other");
        write(&b.join("a/d/keep/deep.txt"), "deep src");
        write(&b.join("b/d/report.txt"), "dest");
        write(&b.join("b/d/keep/deep.txt"), "deep dst");
        let msg = transfer(local(&b.join("a")), vec![dir("d")], local(&b.join("b")), true, Conflict::Skip, &Progress::default()).unwrap();
        assert!(msg.contains("skipped"), "{msg}");
        assert_eq!(read(&b.join("a/d/report.txt")), "source");
        assert_eq!(read(&b.join("a/d/keep/deep.txt")), "deep src");
        assert_eq!(read(&b.join("b/d/report.txt")), "dest");
        assert_eq!(read(&b.join("b/d/sub/other.txt")), "other");
        assert!(!b.join("a/d/sub").exists(), "what did move is gone from the source");
    }

    #[test]
    fn folder_merge_without_conflicts_removes_the_source() {
        let b = tmp("merge");
        write(&b.join("a/d/x.txt"), "x");
        write(&b.join("a/d/y.txt"), "new y");
        write(&b.join("b/d/y.txt"), "old y");
        let msg = transfer(local(&b.join("a")), vec![dir("d")], local(&b.join("b")), true, Conflict::Overwrite, &Progress::default()).unwrap();
        assert!(!msg.contains("skipped"), "{msg}");
        assert!(!b.join("a/d").exists());
        assert_eq!(read(&b.join("b/d/y.txt")), "new y");
        assert_eq!(read(&b.join("b/d/x.txt")), "x");
    }

    #[test]
    fn a_failed_overwrite_keeps_the_existing_file() {
        let b = tmp("failed-overwrite");
        let dst = b.join("dst.txt");
        write(&dst, "precious");
        write(&b.join("src.txt"), "new");
        // The source cannot be opened.
        assert!(copy_file_progress(&b.join("missing"), &dst, Conflict::Overwrite, false, &mut Written::default(), &Progress::default()).is_err());
        assert_eq!(read(&dst), "precious");
        // Cancelled.
        let p = Progress::default();
        p.cancel();
        assert!(copy_file_progress(&b.join("src.txt"), &dst, Conflict::Overwrite, false, &mut Written::default(), &p).is_err());
        assert_eq!(read(&dst), "precious");
        // And no temporary files are left over.
        assert_eq!(std::fs::read_dir(&b).unwrap().count(), 2);
    }

    #[test]
    fn a_file_name_of_the_longest_length_can_be_copied() {
        let b = tmp("long-name");
        let name = "n".repeat(250);
        write(&b.join("a").join(&name), "x");
        std::fs::create_dir_all(b.join("b")).unwrap();
        transfer(local(&b.join("a")), vec![file(&name)], local(&b.join("b")), false, Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(read(&b.join("b").join(&name)), "x");
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_reached_through_a_symlink_is_recognised_as_itself() {
        let b = tmp("alias");
        write(&b.join("real/f.txt"), "data");
        std::os::unix::fs::symlink(b.join("real"), b.join("link")).unwrap();
        let (real, link) = (b.join("real"), b.join("link"));
        // Moving onto itself leaves the file alone.
        transfer(local(&real), vec![file("f.txt")], local(&link), true, Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(read(&real.join("f.txt")), "data");
        // Copying onto itself makes a renamed copy, and does not truncate the source.
        transfer(local(&real), vec![file("f.txt")], local(&link), false, Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(read(&real.join("f.txt")), "data");
        assert_eq!(read(&real.join("f (1).txt")), "data");
        // A folder cannot be copied into itself through the link either.
        write(&b.join("real/sub/g.txt"), "g");
        let r = transfer(local(&b), vec![dir("real")], local(&link.join("sub")), false, Conflict::Overwrite, &Progress::default());
        assert!(r.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_merge_into_a_link_back_to_the_source_keeps_the_files() {
        let b = tmp("alias-merge");
        write(&b.join("a/d/f.txt"), "only copy");
        std::fs::create_dir_all(b.join("b")).unwrap();
        std::os::unix::fs::symlink(b.join("a/d"), b.join("b/d")).unwrap();
        transfer(local(&b.join("a")), vec![dir("d")], local(&b.join("b")), true, Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(read(&b.join("a/d/f.txt")), "only copy");
    }

    /// Names in a folder, temporary files included.
    fn listing(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    }

    /// `README` and `readme` from a disk that tells them apart, moved with Overwrite to one
    /// that may not: the second never replaces the first, whose source is already gone.
    #[test]
    fn a_move_never_overwrites_a_file_it_wrote_itself() {
        let b = tmp("case-move");
        write(&b.join("one/README"), "one");
        write(&b.join("two/README"), "two");
        std::fs::create_dir_all(b.join("dst")).unwrap();
        let mut written = Written::default();
        let mut gone = Vec::new();
        let p = Progress::default();
        assert!(copy_local_recursive(&b.join("one/README"), &b.join("dst/README"), Conflict::Overwrite, Some(&mut gone), &mut written, &p).unwrap());
        assert!(copy_local_recursive(&b.join("two/README"), &b.join("dst/readme"), Conflict::Overwrite, Some(&mut gone), &mut written, &p).unwrap());
        delete_local_sources(gone).unwrap();
        let mut got: Vec<String> = listing(&b.join("dst")).iter().map(|n| read(&b.join("dst").join(n))).collect();
        got.sort();
        assert_eq!(got, ["one", "two"]);
    }

    #[test]
    fn a_move_onto_a_taken_name_follows_the_policy() {
        let b = tmp("move-policy");
        write(&b.join("a/f.txt"), "new");
        write(&b.join("b/f.txt"), "old");
        transfer(local(&b.join("a")), vec![file("f.txt")], local(&b.join("b")), true, Conflict::Skip, &Progress::default()).unwrap();
        assert_eq!((read(&b.join("a/f.txt")), read(&b.join("b/f.txt"))), ("new".into(), "old".into()));
        transfer(local(&b.join("a")), vec![file("f.txt")], local(&b.join("b")), true, Conflict::Rename, &Progress::default()).unwrap();
        assert_eq!(listing(&b.join("b")), ["f (1).txt", "f.txt"]);
        assert_eq!(read(&b.join("b/f (1).txt")), "new");
        assert!(listing(&b.join("a")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_replaced_whole_and_dangling_ones_count_as_taken() {
        let b = tmp("links");
        std::fs::create_dir_all(b.join("a")).unwrap();
        std::os::unix::fs::symlink("target.txt", b.join("a/l")).unwrap();
        write(&b.join("b/l"), "old file");
        std::os::unix::fs::symlink(b.join("nowhere"), b.join("b/dangling")).unwrap();
        write(&b.join("a/dangling"), "data");
        transfer(local(&b.join("a")), vec![file("l")], local(&b.join("b")), false, Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(std::fs::read_link(b.join("b/l")).unwrap(), Path::new("target.txt"));
        // Skip: a dangling link is something there too.
        transfer(local(&b.join("a")), vec![file("dangling")], local(&b.join("b")), false, Conflict::Skip, &Progress::default()).unwrap();
        assert!(std::fs::symlink_metadata(b.join("b/dangling")).unwrap().file_type().is_symlink());
        transfer(local(&b.join("a")), vec![file("dangling")], local(&b.join("b")), false, Conflict::Rename, &Progress::default()).unwrap();
        assert_eq!(read(&b.join("b/dangling (1)")), "data");
        assert_eq!(listing(&b.join("b")), ["dangling", "dangling (1)", "l"]);
    }

    #[test]
    fn a_failed_upload_keeps_the_remote_file_and_leaves_no_part() {
        let b = tmp("failed-upload");
        let (r, l) = (b.join("remote"), b.join("local"));
        write(&l.join("a.txt"), "new");
        write(&l.join("b.txt"), "new b");
        write(&r.join("a.txt"), "precious");
        let fs = || DiskRemote { fail_uploads: true, ..Default::default() };
        assert!(transfer(local(&l), vec![file("a.txt")], remote(fs(), &r), true, Conflict::Overwrite, &Progress::default()).is_err());
        assert!(transfer(local(&l), vec![file("b.txt")], remote(fs(), &r), true, Conflict::Overwrite, &Progress::default()).is_err());
        assert_eq!(listing(&r), ["a.txt"], "no partial file under any name");
        assert_eq!(read(&r.join("a.txt")), "precious");
        assert_eq!(listing(&l), ["a.txt", "b.txt"], "the sources of the moves stay");
    }

    #[test]
    fn an_overwriting_upload_replaces_only_when_complete() {
        for replaces in [false, true] {
            let b = tmp(&format!("up-overwrite-{replaces}"));
            let (r, l) = (b.join("remote"), b.join("local"));
            write(&l.join("a.txt"), "new");
            write(&r.join("a.txt"), "old");
            let fs = DiskRemote { rename_replaces: replaces, ..Default::default() };
            transfer(local(&l), vec![file("a.txt")], remote(fs, &r), true, Conflict::Overwrite, &Progress::default()).unwrap();
            assert_eq!(listing(&r), ["a.txt"], "no temporary or old file left");
            assert_eq!(read(&r.join("a.txt")), "new");
            assert!(listing(&l).is_empty());
        }
    }

    #[test]
    fn a_move_on_one_server_follows_the_policy() {
        let b = tmp("server-move");
        let r = b.join("r");
        let ep = |d: &str| remote(DiskRemote::default(), &r.join(d));
        write(&r.join("src/a.txt"), "new");
        write(&r.join("dst/a.txt"), "old");
        transfer(ep("src"), vec![file("a.txt")], ep("dst"), true, Conflict::Skip, &Progress::default()).unwrap();
        assert_eq!((read(&r.join("src/a.txt")), read(&r.join("dst/a.txt"))), ("new".into(), "old".into()));
        transfer(ep("src"), vec![file("a.txt")], ep("dst"), true, Conflict::Rename, &Progress::default()).unwrap();
        assert_eq!(listing(&r.join("dst")), ["a (1).txt", "a.txt"]);
        write(&r.join("src/a.txt"), "newer");
        transfer(ep("src"), vec![file("a.txt")], ep("dst"), true, Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(listing(&r.join("dst")), ["a (1).txt", "a.txt"]);
        assert_eq!(read(&r.join("dst/a.txt")), "newer");
        assert!(listing(&r.join("src")).is_empty());
    }

    #[test]
    fn remote_to_local_move_with_skip_keeps_the_remote_file() {
        let b = tmp("down-skip");
        let (r, l) = (b.join("remote"), b.join("local"));
        write(&r.join("a.txt"), "remote a");
        write(&r.join("d/keep.txt"), "remote keep");
        write(&r.join("d/go.txt"), "remote go");
        write(&l.join("a.txt"), "local a");
        write(&l.join("d/keep.txt"), "local keep");
        transfer(remote(DiskRemote::default(), &r), vec![file("a.txt"), dir("d")], local(&l), true, Conflict::Skip, &Progress::default()).unwrap();
        assert_eq!(read(&r.join("a.txt")), "remote a");
        assert_eq!(read(&r.join("d/keep.txt")), "remote keep");
        assert_eq!(read(&l.join("a.txt")), "local a");
        assert_eq!(read(&l.join("d/go.txt")), "remote go");
        assert!(!r.join("d/go.txt").exists());
    }

    #[test]
    fn local_to_remote_move_with_skip_keeps_the_local_file() {
        let b = tmp("up-skip");
        let (r, l) = (b.join("remote"), b.join("local"));
        write(&l.join("a.txt"), "local a");
        write(&l.join("d/keep.txt"), "local keep");
        write(&l.join("d/go.txt"), "local go");
        write(&r.join("a.txt"), "remote a");
        write(&r.join("d/keep.txt"), "remote keep");
        transfer(local(&l), vec![file("a.txt"), dir("d")], remote(DiskRemote::default(), &r), true, Conflict::Skip, &Progress::default()).unwrap();
        assert_eq!(read(&l.join("a.txt")), "local a");
        assert_eq!(read(&l.join("d/keep.txt")), "local keep");
        assert_eq!(read(&r.join("a.txt")), "remote a");
        assert_eq!(read(&r.join("d/go.txt")), "local go");
        assert!(!l.join("d/go.txt").exists());
    }

    #[test]
    fn moves_to_and_from_a_remote_remove_the_source_when_complete() {
        let b = tmp("remote-move");
        let (r, l) = (b.join("remote"), b.join("local"));
        write(&r.join("d/sub/x.txt"), "x");
        std::fs::create_dir_all(&l).unwrap();
        transfer(remote(DiskRemote::default(), &r), vec![dir("d")], local(&l), true, Conflict::Overwrite, &Progress::default()).unwrap();
        assert!(!r.join("d").exists());
        assert_eq!(read(&l.join("d/sub/x.txt")), "x");
        transfer(local(&l), vec![dir("d")], remote(DiskRemote::default(), &r), true, Conflict::Overwrite, &Progress::default()).unwrap();
        assert!(!l.join("d").exists());
        assert_eq!(read(&r.join("d/sub/x.txt")), "x");
    }

    #[test]
    fn a_failed_download_keeps_the_existing_file() {
        let b = tmp("failed-download");
        let (r, l) = (b.join("remote"), b.join("local"));
        write(&r.join("a.txt"), "new");
        write(&l.join("a.txt"), "precious");
        let fs = DiskRemote { fail_downloads: true, ..Default::default() };
        assert!(transfer(remote(fs, &r), vec![file("a.txt")], local(&l), true, Conflict::Overwrite, &Progress::default()).is_err());
        assert_eq!(read(&l.join("a.txt")), "precious");
        assert_eq!(read(&r.join("a.txt")), "new");
        assert_eq!(std::fs::read_dir(&l).unwrap().count(), 1, "no temporary file left over");
    }

    #[cfg(unix)]
    #[test]
    fn an_overwriting_download_keeps_the_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let b = tmp("download-perms");
        let (r, l) = (b.join("remote"), b.join("local"));
        write(&r.join("run.sh"), "new");
        write(&l.join("run.sh"), "old");
        std::fs::set_permissions(l.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        transfer(remote(DiskRemote::default(), &r), vec![file("run.sh")], local(&l), false, Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(read(&l.join("run.sh")), "new");
        assert_eq!(std::fs::metadata(l.join("run.sh")).unwrap().permissions().mode() & 0o777, 0o755);
    }

    #[test]
    fn server_names_cannot_leave_the_download_folder() {
        let b = tmp("traversal");
        for name in ["../../evil.txt", "/tmp/evil.txt", "a/b.txt", ".."] {
            assert!(local_child(&b, name).is_err(), "{name}");
        }
        assert_eq!(local_child(&b, "ok.txt").unwrap(), b.join("ok.txt"));
        assert_eq!(local_child(&b, "..hidden").unwrap(), b.join("..hidden"));
        // The same through a real download.
        let (r, l) = (b.join("remote"), b.join("out/deep"));
        write(&r.join("d/a.txt"), "a");
        std::fs::create_dir_all(&l).unwrap();
        let fs = DiskRemote { inject: Some("../../evil.txt".into()), ..Default::default() };
        let res = transfer(remote(fs, &r), vec![dir("d")], local(&l), false, Conflict::Overwrite, &Progress::default());
        assert!(res.unwrap_err().contains("unsafe file name"));
        assert!(!b.join("out/evil.txt").exists() && !b.join("evil.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_remote_link_to_a_folder_keeps_the_folder() {
        let b = tmp("rm-link");
        let r = b.join("remote");
        write(&b.join("releases/42/app.bin"), "app");
        std::fs::create_dir_all(r.join("nested")).unwrap();
        std::os::unix::fs::symlink(b.join("releases/42"), r.join("current")).unwrap();
        std::os::unix::fs::symlink(b.join("releases/42"), r.join("nested/inner")).unwrap();
        // The selected item itself is a link...
        delete(remote(DiskRemote::default(), &r), vec![dir("current")], false, &Progress::default()).unwrap();
        assert!(std::fs::symlink_metadata(r.join("current")).is_err());
        assert_eq!(read(&b.join("releases/42/app.bin")), "app");
        // ...and one further down.
        delete(remote(DiskRemote::default(), &r), vec![dir("nested")], false, &Progress::default()).unwrap();
        assert!(!r.join("nested").exists());
        assert_eq!(read(&b.join("releases/42/app.bin")), "app");
    }

    #[cfg(unix)]
    #[test]
    fn moving_a_link_to_a_folder_keeps_the_folder_it_points_to() {
        let b = tmp("mv-link");
        let (r, l) = (b.join("remote"), b.join("local"));
        write(&b.join("target/app.bin"), "app");
        std::fs::create_dir_all(&r).unwrap();
        std::fs::create_dir_all(&l).unwrap();
        std::os::unix::fs::symlink(b.join("target"), r.join("current")).unwrap();
        // Download: gets the contents, and removes only the remote link.
        transfer(remote(DiskRemote::default(), &r), vec![dir("current")], local(&l), true, Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(read(&l.join("current/app.bin")), "app");
        assert!(std::fs::symlink_metadata(r.join("current")).is_err());
        assert_eq!(read(&b.join("target/app.bin")), "app");
        // Upload: likewise for a local link.
        std::fs::remove_dir_all(l.join("current")).unwrap();
        std::os::unix::fs::symlink(b.join("target"), l.join("current")).unwrap();
        transfer(local(&l), vec![dir("current")], remote(DiskRemote::default(), &r), true, Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(read(&r.join("current/app.bin")), "app");
        assert!(std::fs::symlink_metadata(l.join("current")).is_err());
        assert_eq!(read(&b.join("target/app.bin")), "app");
    }
}
