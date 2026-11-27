//! Notices when the folders shown in the panels change on disk, so that they reload
//! by themselves (Settings → Files & layout → Refresh folders automatically).
//!
//! A thread looks at each shown folder's modification time every few seconds: adding,
//! removing or renaming a file in a folder changes it. Writing into a file does not, so
//! a file that is still growing (a download, a log) keeps the size and date the list
//! showed until the folder reloads for another reason. Only the folder's metadata is
//! read; nothing is opened or kept open, so an eject is never held up, and a folder on
//! a slow or stuck network drive only holds up this thread.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// How often the thread wakes to see whether a check is due (or it should stop).
const TICK: Duration = Duration::from_millis(200);

/// How long after a change a folder's time may still not show the next one: disks with
/// coarse times (FAT: 2 s, HFS+: 1 s, some network shares) give both the same time.
const SETTLE: Duration = Duration::from_secs(3);

/// Whether a folder time `t` is too recent to rely on: a listing stamped with it is
/// checked again even if the time stays the same.
pub fn is_recent(t: SystemTime) -> bool {
    matches!(SystemTime::now().duration_since(t), Ok(d) if d < SETTLE)
}

/// The folder's modification time, the stamp a listing is compared with.
pub fn modified(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

#[derive(Default)]
struct Shared {
    /// How often to check; `None`: not at all.
    every: Option<Duration>,
    /// The folders shown, each with the stamp it had when it was listed.
    dirs: Vec<(PathBuf, Option<SystemTime>)>,
    /// Folders whose stamp is no longer the listed one.
    changed: HashSet<PathBuf>,
}

pub struct DirWatch {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
}

fn lock(m: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl DirWatch {
    /// Start the checking thread; it wakes `ctx` when a folder changed.
    pub fn spawn(ctx: Option<egui::Context>) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, halt) = (shared.clone(), stop.clone());
        let _ = std::thread::Builder::new().name("dirwatch".into()).spawn(move || {
            let mut last = Instant::now();
            while !halt.load(Ordering::Relaxed) {
                std::thread::sleep(TICK);
                let (every, dirs) = {
                    let g = lock(&s);
                    (g.every, g.dirs.clone())
                };
                if every.is_none_or(|e| last.elapsed() < e) {
                    continue;
                }
                last = Instant::now();
                // Outside the lock: a stat on a stuck network drive must not hold up the UI.
                let changed: Vec<PathBuf> = dirs.into_iter().filter(|(d, stamp)| modified(d) != *stamp).map(|(d, _)| d).collect();
                if !changed.is_empty() {
                    lock(&s).changed.extend(changed);
                    if let Some(c) = &ctx {
                        c.request_repaint();
                    }
                }
            }
        });
        Self { shared, stop }
    }

    /// What to check from now on: the folders shown with their listed stamps, every
    /// `every` (`None` stops checking).
    pub fn watch(&self, every: Option<Duration>, dirs: Vec<(PathBuf, Option<SystemTime>)>) {
        let mut g = lock(&self.shared);
        g.every = every;
        g.dirs = dirs;
    }

    /// The folders found changed since the last call.
    pub fn take_changed(&self) -> HashSet<PathBuf> {
        std::mem::take(&mut lock(&self.shared).changed)
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_change(w: &DirWatch, secs: f32) -> HashSet<PathBuf> {
        let t0 = Instant::now();
        loop {
            let c = w.take_changed();
            if !c.is_empty() || t0.elapsed().as_secs_f32() > secs {
                return c;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn notices_files_added_and_removed_but_not_a_quiet_folder() {
        let dir = std::env::temp_dir().join(format!("mf-dirwatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.txt"), "x").unwrap();
        let w = DirWatch::spawn(None);
        let every = Some(Duration::from_millis(50));
        w.watch(every, vec![(dir.clone(), modified(&dir))]);
        assert!(wait_change(&w, 0.6).is_empty(), "nothing changed");
        // Writing into a file is not seen: the folder's time stays.
        std::fs::write(dir.join("old.txt"), "longer").unwrap();
        assert!(wait_change(&w, 0.6).is_empty(), "a file's content is not a change of the folder");
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("new.txt"), "x").unwrap();
        assert_eq!(wait_change(&w, 5.0), HashSet::from([dir.clone()]), "a new file");
        // Listed again: quiet until the next change.
        w.watch(every, vec![(dir.clone(), modified(&dir))]);
        let _ = w.take_changed();
        assert!(wait_change(&w, 0.4).is_empty());
        std::fs::remove_file(dir.join("new.txt")).unwrap();
        assert_eq!(wait_change(&w, 5.0), HashSet::from([dir.clone()]), "a removed file");
        // Turned off: nothing is checked.
        w.watch(None, vec![(dir.clone(), None)]);
        let _ = w.take_changed();
        assert!(wait_change(&w, 0.4).is_empty(), "off");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
