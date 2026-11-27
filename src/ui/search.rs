//! Searches that run in the background for a file window: every folder below one
//! (a search), or the files of one folder read for their text (a filter that reads
//! files). Both report how far along they are, so that the window can show a progress
//! bar with the time left, and both stop as soon as they are told to (Stop, Esc, or
//! going to another folder drops them).
use crate::files::vfs::Entry;
use crate::ui::filter::Filter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Upper bound on the results of a search.
pub const SEARCH_LIMIT: usize = 20_000;
/// Results found are sent to the window at least this often.
const FLUSH_EVERY: Duration = Duration::from_millis(120);

/// How far a background search is: a fraction of the work, how many items it looked
/// at and how many passed. Written by the worker, read by the window each frame.
#[derive(Default)]
pub struct Progress {
    fraction: AtomicU32,
    checked: AtomicU64,
    found: AtomicU64,
}

impl Progress {
    /// From 0 to 1; it only grows.
    pub fn fraction(&self) -> f32 {
        f32::from_bits(self.fraction.load(Ordering::Relaxed))
    }

    pub fn checked(&self) -> u64 {
        self.checked.load(Ordering::Relaxed)
    }

    pub fn found(&self) -> u64 {
        self.found.load(Ordering::Relaxed)
    }

    fn reach(&self, f: f64) {
        let f = (f as f32).clamp(0.0, 1.0);
        if f > self.fraction() {
            self.fraction.store(f.to_bits(), Ordering::Relaxed);
        }
    }

    fn count(&self, passed: bool) {
        self.checked.fetch_add(1, Ordering::Relaxed);
        if passed {
            self.found.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The time still needed, guessed from the time so far and the fraction done; `None`
/// until there is enough to go by.
pub fn time_left(elapsed: Duration, fraction: f32) -> Option<Duration> {
    if elapsed < Duration::from_millis(800) || !(0.02..1.0).contains(&fraction) {
        return None;
    }
    Some(elapsed.mul_f32((1.0 - fraction) / fraction))
}

/// A search being run: its results arrive in batches.
pub struct Running {
    pub rx: Receiver<Vec<Entry>>,
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<Progress>,
    pub started: Instant,
}

/// Search every folder below `root` for what passes `filter`. Results are named by
/// their path below `root`. Symbolic links to folders are not followed.
pub fn spawn_search(root: PathBuf, filter: Filter, show_hidden: bool) -> Running {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Progress::default());
    let (stop, prog) = (cancel.clone(), progress.clone());
    // Deep trees go deep in the recursion: room for it.
    let _ = std::thread::Builder::new().name("search".into()).stack_size(16 << 20).spawn(move || {
        let mut w = Walk { root: &root, filter: &filter, show_hidden, stop: &stop, progress: &prog, tx: &tx, batch: Vec::new(), last: Instant::now() };
        w.dir(&root, 0.0, 1.0);
        let _ = tx.send(std::mem::take(&mut w.batch));
        if !stop.load(Ordering::Relaxed) {
            prog.reach(1.0);
        }
    });
    Running { rx, cancel, progress, started: Instant::now() }
}

struct Walk<'a> {
    root: &'a Path,
    filter: &'a Filter,
    show_hidden: bool,
    stop: &'a AtomicBool,
    progress: &'a Progress,
    tx: &'a Sender<Vec<Entry>>,
    batch: Vec<Entry>,
    last: Instant,
}

impl Walk<'_> {
    /// Send the results waiting once there are many or they have waited a little.
    /// False once nobody listens any more.
    fn flush(&mut self) -> bool {
        if !self.batch.is_empty() && (self.batch.len() >= 500 || self.last.elapsed() > FLUSH_EVERY) {
            if self.tx.send(std::mem::take(&mut self.batch)).is_err() {
                return false;
            }
            self.last = Instant::now();
        }
        true
    }

    /// Look at the folder `dir`, which stands for the part `lo..hi` of the whole search:
    /// each of its entries gets an equal share, so the fraction done only grows as the
    /// walk goes on, folder after folder. Returns false once the search is to end.
    fn dir(&mut self, dir: &Path, lo: f64, hi: f64) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else {
            self.progress.reach(hi);
            return true;
        };
        let items: Vec<std::fs::DirEntry> = rd.flatten().filter(|d| self.show_hidden || !d.file_name().to_string_lossy().starts_with('.')).collect();
        let n = items.len().max(1) as f64;
        for (i, d) in items.into_iter().enumerate() {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            let (a, b) = (lo + (hi - lo) * i as f64 / n, lo + (hi - lo) * (i + 1) as f64 / n);
            let path = d.path();
            // Not following links: the link itself, never what it points to.
            let ft = d.file_type().ok();
            let is_link = ft.is_some_and(|t| t.is_symlink());
            let is_dir = ft.is_some_and(|t| t.is_dir());
            let meta = d.metadata().ok();
            let mut e = Entry::new(
                d.file_name().to_string_lossy().to_string(),
                is_dir,
                meta.as_ref().filter(|_| !is_dir).map(|m| m.len()).unwrap_or(0),
                meta.and_then(|m| m.modified().ok()),
            );
            e.is_link = is_link;
            let passed = self.filter.matches_at(&e, Some(&path), self.stop);
            self.progress.count(passed);
            if passed {
                e.name = path.strip_prefix(self.root).unwrap_or(&path).to_string_lossy().to_string();
                self.batch.push(e);
                if self.progress.found() as usize >= SEARCH_LIMIT {
                    return false;
                }
            }
            // After every entry, not only a match: a result found before a long run of
            // others must not wait for the next one to be shown.
            if !self.flush() {
                return false;
            }
            if is_dir && !is_link && !self.dir(&path, a, b) {
                return false;
            }
            self.progress.reach(b);
        }
        self.progress.reach(hi);
        true
    }
}

/// A filter that reads files (`source LIKE …`, Find's text in files), over the files
/// of one folder: the names that pass, in batches. Progress goes by bytes read.
pub fn spawn_scan(dir: PathBuf, filter: Filter, entries: Vec<Entry>) -> (Receiver<Vec<String>>, Arc<AtomicBool>, Arc<Progress>) {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Progress::default());
    let (stop, prog) = (cancel.clone(), progress.clone());
    std::thread::spawn(move || {
        // Each file counts for its size, and at least a little (an empty file is quick
        // but not free to open).
        let weight = |e: &Entry| e.size.max(4096) as f64;
        let total: f64 = entries.iter().map(weight).sum::<f64>().max(1.0);
        let mut done = 0.0;
        let mut batch = Vec::new();
        let mut last = Instant::now();
        for e in entries {
            // Stopped: what was found so far still goes out.
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let passed = filter.matches_at(&e, Some(&dir.join(&e.name)), &stop);
            prog.count(passed);
            done += weight(&e);
            prog.reach(done / total);
            if passed {
                batch.push(e.name);
            }
            if !batch.is_empty() && last.elapsed() > FLUSH_EVERY {
                if tx.send(std::mem::take(&mut batch)).is_err() {
                    return;
                }
                last = Instant::now();
            }
        }
        if !stop.load(Ordering::Relaxed) {
            prog.reach(1.0);
        }
        let _ = tx.send(batch);
    });
    (rx, cancel, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(r: &Running) -> Vec<String> {
        let t0 = Instant::now();
        let mut names = vec![];
        while t0.elapsed() < Duration::from_secs(10) {
            match r.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(b) => names.extend(b.into_iter().map(|e| e.name)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
        names.sort();
        names
    }

    #[test]
    fn finds_below_and_ends_at_one() {
        let dir = std::env::temp_dir().join(format!("mf-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::create_dir_all(dir.join("c")).unwrap();
        for f in ["a/x-report.txt", "a/b/Y_Report.md", "c/other.txt", "top report.txt"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let find = crate::ui::filter::Find { text: "report".into(), folders: false, ..Default::default() };
        let r = spawn_search(dir.clone(), Filter::Find(find), false);
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(collect(&r), [format!("a{sep}b{sep}Y_Report.md"), format!("a{sep}x-report.txt"), "top report.txt".into()]);
        assert_eq!(r.progress.fraction(), 1.0, "done");
        assert_eq!(r.progress.found(), 3);
        assert!(r.progress.checked() >= 7, "every entry looked at: {}", r.progress.checked());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The fraction only grows, and a stopped search sends nothing more.
    #[test]
    fn progress_grows_and_stop_stops() {
        let dir = std::env::temp_dir().join(format!("mf-search-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for i in 0..30 {
            std::fs::create_dir_all(dir.join(format!("d{i}"))).unwrap();
            for j in 0..30 {
                std::fs::write(dir.join(format!("d{i}/f{j}.txt")), "hello").unwrap();
            }
        }
        let find = crate::ui::filter::Find { text: "hello".into(), mode: crate::ui::filter::FindMode::Content, ..Default::default() };
        let r = spawn_search(dir.clone(), Filter::Find(find), false);
        let mut seen = 0.0f32;
        let t0 = Instant::now();
        while r.progress.fraction() < 0.2 && t0.elapsed() < Duration::from_secs(10) {
            let f = r.progress.fraction();
            assert!(f >= seen, "never goes back: {f} < {seen}");
            seen = f;
        }
        r.cancel.store(true, Ordering::Relaxed);
        let found = collect(&r).len();
        assert!(found < 900, "stopped before the end: {found}");
        assert!(r.progress.fraction() < 1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn time_left_from_the_pace_so_far() {
        assert_eq!(time_left(Duration::from_millis(100), 0.5), None, "too early to tell");
        assert_eq!(time_left(Duration::from_secs(10), 0.01), None, "too little done");
        assert_eq!(time_left(Duration::from_secs(10), 0.25), Some(Duration::from_secs(30)));
    }

    /// A result waiting is sent once it has waited a little, even with no new match.
    #[test]
    fn a_waiting_result_goes_out_in_time() {
        let (tx, rx) = channel();
        let (stop, progress, filter) = (AtomicBool::new(false), Progress::default(), Filter::All);
        let root = PathBuf::from("/");
        let mut w = Walk { root: &root, filter: &filter, show_hidden: false, stop: &stop, progress: &progress, tx: &tx, batch: vec![], last: Instant::now() };
        assert!(w.flush());
        assert!(rx.try_recv().is_err(), "nothing to send");
        w.batch.push(Entry::new("found.txt".into(), false, 1, None));
        assert!(w.flush());
        assert!(rx.try_recv().is_err(), "it has not waited yet");
        w.last = Instant::now() - FLUSH_EVERY * 2;
        assert!(w.flush());
        assert_eq!(rx.try_recv().unwrap().len(), 1, "sent without another match");
        drop(rx);
        w.batch.push(Entry::new("more.txt".into(), false, 1, None));
        w.last = Instant::now() - FLUSH_EVERY * 2;
        assert!(!w.flush(), "nobody listens: the walk ends");
    }
}
