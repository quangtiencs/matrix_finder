//! Background jobs (copy / move / delete / pack / unpack / backup) with progress.
use crate::remote::SharedRemote;
use crate::util::{es, join_slash, unique_path, Res};
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
}

impl Progress {
    pub fn add(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
        self.wait_if_paused();
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
            Err("Cancelled".into())
        } else {
            Ok(())
        }
    }
}

/// How far along these jobs are together, by bytes. `None` until one of them knows its size.
pub fn overall_fraction(jobs: &[&Progress]) -> Option<f32> {
    let (done, total) = jobs.iter().fold((0u64, 0u64), |(d, t), p| if p.total() > 0 { (d + p.done().min(p.total()), t + p.total()) } else { (d, t) });
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
                let _ = tx.send(Err("Cancelled".into()));
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    Overwrite,
    Skip,
    Rename,
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

/// Copy one file through a temporary file next to `dst`, so an existing `dst` is only
/// replaced once the new copy is complete: a cancel, full disk or read error keeps it.
fn copy_file_progress(src: &Path, dst: &Path, p: &Progress) -> Res<()> {
    let tmp = temp_sibling(dst);
    let r = copy_file_inner(src, &tmp, p).and_then(|_| std::fs::rename(&tmp, dst).map_err(es));
    if r.is_err() {
        // Never leave a half-written file behind.
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// A hidden, unused name in `dst`'s folder for writing `dst` before it is renamed into place.
fn temp_sibling(dst: &Path) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    // Not derived from `dst`'s name, which may already be as long as names can get.
    dst.with_file_name(format!(".matrix_finder-{}-{n}.part", std::process::id()))
}

fn copy_file_inner(src: &Path, dst: &Path, p: &Progress) -> Res<()> {
    use std::io::{Read, Write};
    let mut r = std::fs::File::open(src).map_err(es)?;
    let mut w = std::fs::File::create(dst).map_err(es)?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        p.check()?;
        let n = r.read(&mut buf).map_err(es)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).map_err(es)?;
        p.add(n as u64);
    }
    if let Ok(meta) = std::fs::metadata(src) {
        let _ = std::fs::set_permissions(dst, meta.permissions());
        if let Ok(m) = meta.modified() {
            let _ = w.set_modified(m);
        }
    }
    Ok(())
}

/// `p` with its folder resolved through symlinks, but not `p` itself.
fn real_entry(p: &Path) -> Option<PathBuf> {
    Some(p.parent()?.canonicalize().ok()?.join(p.file_name()?))
}

/// True when `a` and `b` are the same directory entry, e.g. `/tmp/x` and `/private/tmp/x`.
fn same_entry(a: &Path, b: &Path) -> bool {
    a == b || matches!((real_entry(a), real_entry(b)), (Some(x), Some(y)) if x == y)
}

/// Copy `src` to `dst`, or with `mv` move it: each file's source is deleted once that file
/// has been copied. Returns false when something was skipped (by the conflict policy, or
/// because it already is the destination); its source, and the folders holding it, are kept.
fn copy_local_recursive(src: &Path, dst: &Path, policy: Conflict, mv: bool, p: &Progress) -> Res<bool> {
    p.check()?;
    let meta = std::fs::symlink_metadata(src).map_err(es)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dst).map_err(es)?;
        let mut complete = true;
        for e in std::fs::read_dir(src).map_err(es)?.flatten() {
            complete &= copy_local_recursive(&e.path(), &dst.join(e.file_name()), policy, mv, p)?;
        }
        if mv && complete {
            std::fs::remove_dir(src).map_err(es)?;
        }
        Ok(complete)
    } else {
        let dst = match resolve_local(dst, policy) {
            Some(d) => d,
            None => {
                p.add(meta.len());
                return Ok(false);
            }
        };
        if std::fs::symlink_metadata(&dst).is_ok() && same_entry(src, &dst) {
            p.add(meta.len());
            return Ok(false);
        }
        p.set_current(&src.to_string_lossy());
        #[cfg(unix)]
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(src).map_err(es)?;
            let _ = std::fs::remove_file(&dst);
            std::os::unix::fs::symlink(target, &dst).map_err(es)?;
            if mv {
                std::fs::remove_file(src).map_err(es)?;
            }
            return Ok(true);
        }
        if mv && std::fs::rename(src, &dst).is_ok() {
            p.add(meta.len());
            return Ok(true);
        }
        copy_file_progress(src, &dst, p)?;
        if mv {
            std::fs::remove_file(src).map_err(es)?;
        }
        Ok(true)
    }
}

/// Apply the conflict policy to a local target. `None` = skip.
fn resolve_local(dst: &Path, policy: Conflict) -> Option<PathBuf> {
    if !dst.exists() {
        return Some(dst.to_path_buf());
    }
    match policy {
        Conflict::Overwrite => Some(dst.to_path_buf()),
        Conflict::Skip => None,
        Conflict::Rename => Some(unique_path(dst)),
    }
}

/// `dir/name` when `name` is one plain file name. Names come from the server, and one
/// such as `../../.zshrc` or `/Users/me/.zshrc` must not lead out of `dir`.
fn local_child(dir: &Path, name: &str) -> Res<PathBuf> {
    let mut c = Path::new(name).components();
    match (c.next(), c.next()) {
        (Some(std::path::Component::Normal(n)), None) if n == std::ffi::OsStr::new(name) => Ok(dir.join(n)),
        _ => Err(format!("Refusing an unsafe file name from the server: {name:?}")),
    }
}

/// Download a remote file or folder; with `mv`, delete each remote file once it is downloaded.
/// Returns false when something was skipped, and then keeps the remote folders holding it.
fn download_recursive(fs: &SharedRemote, rpath: &str, lpath: &Path, is_dir: bool, policy: Conflict, mv: bool, p: &Progress) -> Res<bool> {
    p.check()?;
    if is_dir {
        std::fs::create_dir_all(lpath).map_err(es)?;
        // A link to a folder: download what it points to, but delete only the link.
        let link = mv && fs.lock().map_err(es)?.is_symlink(rpath)?;
        let list = fs.lock().map_err(es)?.list(rpath)?;
        let mut complete = true;
        for e in list {
            let child = local_child(lpath, &e.name)?;
            complete &= download_recursive(fs, &join_slash(rpath, &e.name), &child, e.is_dir, policy, mv && !link, p)?;
        }
        if mv && complete {
            let mut g = fs.lock().map_err(es)?;
            if link {
                g.remove_file(rpath)?;
            } else {
                g.remove_dir(rpath)?;
            }
        }
        Ok(complete)
    } else {
        let Some(target) = resolve_local(lpath, policy) else { return Ok(false) };
        p.set_current(rpath);
        // Into a temporary file first, so a failed download keeps an existing `target`.
        let tmp = temp_sibling(&target);
        let mut g = fs.lock().map_err(es)?;
        let r = g
            .download(rpath, &tmp, &|n| p.add(n), p.cancel_flag())
            .and_then(|_| {
                // An overwritten file keeps its permissions, as it would if written in place.
                if let Ok(meta) = std::fs::metadata(&target) {
                    let _ = std::fs::set_permissions(&tmp, meta.permissions());
                }
                std::fs::rename(&tmp, &target).map_err(es)
            });
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r?;
        if mv {
            g.remove_file(rpath)?;
        }
        Ok(true)
    }
}

/// Upload a local file or folder; with `mv`, delete each local file once it is uploaded.
/// Returns false when something was skipped, and then keeps the local folders holding it.
fn upload_recursive(fs: &SharedRemote, lpath: &Path, rpath: &str, existing: Option<&HashSet<String>>, policy: Conflict, mv: bool, p: &Progress) -> Res<bool> {
    p.check()?;
    let name = lpath.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let exists = existing.is_some_and(|s| s.contains(&name));
    if lpath.is_dir() {
        // A link to a folder: upload what it points to, but delete only the link.
        let link = mv && std::fs::symlink_metadata(lpath).map_err(es)?.file_type().is_symlink();
        if !exists {
            fs.lock().map_err(es)?.mkdir(rpath)?;
        }
        let children: HashSet<String> = if exists {
            fs.lock().map_err(es)?.list(rpath)?.into_iter().map(|e| e.name).collect()
        } else {
            HashSet::new()
        };
        let mut complete = true;
        for e in std::fs::read_dir(lpath).map_err(es)?.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            complete &= upload_recursive(fs, &e.path(), &join_slash(rpath, &n), Some(&children), policy, mv && !link, p)?;
        }
        if mv && complete {
            if link {
                std::fs::remove_file(lpath).map_err(es)?;
            } else {
                std::fs::remove_dir(lpath).map_err(es)?;
            }
        }
        Ok(complete)
    } else {
        let mut target = rpath.to_string();
        if exists {
            match policy {
                Conflict::Skip => return Ok(false),
                Conflict::Overwrite => {}
                Conflict::Rename => target = format!("{rpath}.copy"),
            }
        }
        p.set_current(&lpath.to_string_lossy());
        let mut g = fs.lock().map_err(es)?;
        g.upload(lpath, &target, &|n| p.add(n), p.cancel_flag())?;
        drop(g);
        if mv {
            std::fs::remove_file(lpath).map_err(es)?;
        }
        Ok(true)
    }
}

fn remote_size(fs: &SharedRemote, path: &str, is_dir: bool, size: u64) -> u64 {
    if !is_dir {
        return size;
    }
    let list = match fs.lock() {
        Ok(mut g) => g.list(path).unwrap_or_default(),
        Err(_) => return 0,
    };
    list.iter()
        .map(|e| remote_size(fs, &join_slash(path, &e.name), e.is_dir, e.size))
        .sum()
}

/// Copy or move `items` from `src` to `dst`.
pub fn transfer(src: Endpoint, items: Vec<Item>, dst: Endpoint, mv: bool, policy: Conflict, p: &Progress) -> Res<String> {
    let count = items.len();
    // False once anything was skipped (moves then keep those sources).
    let mut complete = true;
    match (&src, &dst) {
        (Endpoint::Local(sdir), Endpoint::Local(ddir)) => {
            p.set_current("Calculating…");
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
                    copy_local_recursive(&s, &unique_path(&d), Conflict::Rename, false, p)?;
                    continue;
                }
                if it.is_dir && (ddir.starts_with(&s) || real_entry(&s).is_some_and(|rs| real_ddir.starts_with(rs))) {
                    return Err(format!("Cannot copy '{}' into itself", it.name));
                }
                if mv {
                    let target = if d.exists() && !it.is_dir {
                        match resolve_local(&d, policy) {
                            Some(t) => t,
                            None => {
                                complete = false;
                                continue;
                            }
                        }
                    } else {
                        d.clone()
                    };
                    if !target.exists() && std::fs::rename(&s, &target).is_ok() {
                        continue;
                    }
                    // Cross-device or merge: copy, deleting each source file once copied.
                    p.add_total(if it.is_dir { local_size(&s) } else { it.size });
                    complete &= copy_local_recursive(&s, &target, policy, true, p)?;
                } else {
                    complete &= copy_local_recursive(&s, &d, policy, false, p)?;
                }
            }
        }
        (Endpoint::Remote(fs, _, rdir), Endpoint::Local(ddir)) => {
            p.set_current("Calculating…");
            p.set_total(items.iter().map(|i| remote_size(fs, &join_slash(rdir, &i.name), i.is_dir, i.size)).sum());
            for it in &items {
                let rp = join_slash(rdir, &it.name);
                complete &= download_recursive(fs, &rp, &local_child(ddir, &it.name)?, it.is_dir, policy, mv, p)?;
            }
        }
        (Endpoint::Local(sdir), Endpoint::Remote(fs, _, rdir)) => {
            p.set_current("Calculating…");
            p.set_total(items.iter().map(|i| if i.is_dir { local_size(&sdir.join(&i.name)) } else { i.size }).sum());
            let existing: HashSet<String> = fs.lock().map_err(es)?.list(rdir)?.into_iter().map(|e| e.name).collect();
            for it in &items {
                let lp = sdir.join(&it.name);
                complete &= upload_recursive(fs, &lp, &join_slash(rdir, &it.name), Some(&existing), policy, mv, p)?;
            }
        }
        (Endpoint::Remote(fs, a, sdir), Endpoint::Remote(_, b, ddir)) if a == b && mv => {
            let mut g = fs.lock().map_err(es)?;
            for it in &items {
                g.rename(&join_slash(sdir, &it.name), &join_slash(ddir, &it.name))?;
            }
        }
        (Endpoint::Archive(file, inner), Endpoint::Local(ddir)) => {
            if mv {
                return Err("Cannot move out of an archive (archives are read-only). Use copy.".into());
            }
            let sel: Vec<String> = items.iter().map(|i| join_slash(inner, &i.name)).collect();
            crate::archive::extract(file, ddir, &sel, inner, policy, p)?;
        }
        (_, Endpoint::Archive(..)) => {
            return Err("Archives are read-only here. Use Pack (Alt+F5) to create a new archive.".into());
        }
        _ => return Err("This transfer combination is not supported (copy via a local folder).".into()),
    }
    let verb = if mv { "Moved" } else { "Copied" };
    Ok(if complete {
        format!("{verb} {count} item(s)")
    } else if mv {
        format!("{verb} {count} item(s); skipped files were left in place")
    } else {
        format!("{verb} {count} item(s); some files were skipped")
    })
}

pub fn delete(src: Endpoint, items: Vec<Item>, to_trash: bool, p: &Progress) -> Res<String> {
    let count = items.len();
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
                p.add(1);
            }
        }
        Endpoint::Archive(..) => return Err("Archives are read-only".into()),
    }
    Ok(format!("Deleted {count} item(s)"))
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
        assert_eq!(job.poll(), Some(Err("Cancelled".to_string())));
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
    }

    impl crate::remote::RemoteFs for DiskRemote {
        fn home(&mut self) -> String {
            "/".into()
        }
        fn list(&mut self, path: &str) -> Res<Vec<crate::vfs::Entry>> {
            let mut out = vec![];
            for e in std::fs::read_dir(path).map_err(es)?.flatten() {
                let is_dir = e.path().is_dir();
                let mut en = crate::vfs::Entry::new(e.file_name().to_string_lossy().to_string(), is_dir, 0, None);
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
            std::fs::copy(local, remote).map(|_| ()).map_err(es)
        }
        fn read_head(&mut self, _: &str, _: usize) -> Res<Vec<u8>> {
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
            std::fs::rename(from, to).map_err(es)
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
        assert!(copy_file_progress(&b.join("missing"), &dst, &Progress::default()).is_err());
        assert_eq!(read(&dst), "precious");
        // Cancelled.
        let p = Progress::default();
        p.cancel();
        assert!(copy_file_progress(&b.join("src.txt"), &dst, &p).is_err());
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
