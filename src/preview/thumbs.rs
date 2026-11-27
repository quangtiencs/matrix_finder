//! Thumbnails of pictures and videos for the icon view (Settings → Files & layout →
//! Show thumbnails in the icon view).
//!
//! They are made on worker threads with the readers Quick View uses (the same size
//! limits, the picture turned the way its metadata says), only from files on this
//! computer, and kept in memory: nothing is written next to a file or into a cache on
//! disk. A video's frame comes from FFmpeg or, on macOS, Quick Look, which write into a
//! temporary folder of their own. The most recently asked thumbnail is made first, and
//! one that scrolled out of sight before its turn is not made at all.
use super::{classify, decode_image, decode_oriented, Kind, MAX_IMAGE};
use crate::preview::media::Cancel;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// The longest side of a thumbnail, in pixels (sharp at twice the icon view's size).
pub const SIZE: u32 = 192;
/// Thumbnails kept at most (about 150 KB each); the least recently shown go first.
const MAX_KEPT: usize = 600;
/// A thumbnail not asked for in this long (scrolled away) is not made when its turn comes.
const STALE: Duration = Duration::from_millis(1500);
const WORKERS: usize = 2;

/// Files the icon view shows by a thumbnail: pictures, camera RAW, HEIC and videos.
pub fn has_thumbnail(name: &str) -> bool {
    match classify(name) {
        // Photoshop documents have their own reader in Quick View.
        Kind::Image => !matches!(crate::util::ext_of(name).as_str(), "psd" | "psb"),
        Kind::Raw | Kind::Heif | Kind::Video => true,
        _ => false,
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    modified: Option<SystemTime>,
    size: u64,
}

enum Slot {
    Queued,
    Done(egui::TextureHandle),
    /// Not a picture this can read (or the tool failed): the plain icon stays.
    Failed,
}

struct State {
    slots: HashMap<Key, (Slot, Instant)>,
    queue: Vec<Key>,
    stop: bool,
    /// How many slots there may be before the next clean-up: not one per new thumbnail
    /// while scrolling fast past thousands of pictures.
    trim_at: usize,
}

pub struct Thumbs {
    shared: Arc<(Mutex<State>, Condvar)>,
    cancel: Cancel,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Thumbs {
    pub fn spawn(ctx: egui::Context) -> Self {
        let shared = Arc::new((Mutex::new(State { slots: HashMap::new(), queue: Vec::new(), stop: false, trim_at: MAX_KEPT }), Condvar::new()));
        let cancel: Cancel = Arc::new(AtomicBool::new(false));
        for i in 0..WORKERS {
            let (shared, cancel, ctx) = (shared.clone(), cancel.clone(), ctx.clone());
            // As Quick View's loader: room for decoders that recurse deeply.
            let _ = std::thread::Builder::new().name(format!("thumbs-{i}")).stack_size(super::LOADER_STACK).spawn(move || worker(&shared, &cancel, &ctx));
        }
        Self { shared, cancel }
    }

    /// The thumbnail of the file at `path` (with the date and size the list shows, so
    /// that a changed file gets a new one), or `None` while it is made or when the file
    /// has none: then the plain icon is drawn.
    /// A file Quick View would find too large gets none either (and is not read).
    pub fn get(&self, path: &Path, modified: Option<SystemTime>, size: u64) -> Option<egui::TextureHandle> {
        if super::too_large(&path.file_name()?.to_string_lossy(), size, None) {
            return None;
        }
        let key = Key { path: path.to_path_buf(), modified, size };
        let (m, cv) = &*self.shared;
        let mut st = lock(m);
        let now = Instant::now();
        if let Some((slot, asked)) = st.slots.get_mut(&key) {
            *asked = now;
            return match slot {
                Slot::Done(tex) => Some(tex.clone()),
                Slot::Queued | Slot::Failed => None,
            };
        }
        if st.slots.len() >= st.trim_at {
            forget_oldest(&mut st);
            st.trim_at = (st.slots.len() + MAX_KEPT / 10).max(MAX_KEPT);
        }
        st.slots.insert(key.clone(), (Slot::Queued, now));
        st.queue.push(key);
        cv.notify_one();
        None
    }
}

impl Thumbs {
    /// Thumbnails asked for so far (made, being made or failed).
    #[cfg(test)]
    pub fn asked(&self) -> usize {
        lock(&self.shared.0).slots.len()
    }

    /// Thumbnails made.
    #[cfg(test)]
    pub fn made(&self) -> usize {
        lock(&self.shared.0).slots.values().filter(|(s, _)| matches!(s, Slot::Done(_))).count()
    }
}

/// Back under the limit: drop the waiting thumbnails scrolled out of sight (their turn
/// would skip them anyway), then the made ones shown longest ago, down to nine tenths
/// of `MAX_KEPT` (so that this does not run again at once).
fn forget_oldest(st: &mut State) {
    st.slots.retain(|_, (slot, asked)| !matches!(slot, Slot::Queued) || asked.elapsed() <= STALE);
    let excess = st.slots.len().saturating_sub(MAX_KEPT * 9 / 10);
    if excess == 0 {
        return;
    }
    let mut done: Vec<(Instant, Key)> =
        st.slots.iter().filter(|(_, (slot, _))| !matches!(slot, Slot::Queued)).map(|(k, (_, asked))| (*asked, k.clone())).collect();
    done.sort_by_key(|(asked, _)| *asked);
    for (_, k) in done.into_iter().take(excess) {
        st.slots.remove(&k);
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let (m, cv) = &*self.shared;
        lock(m).stop = true;
        cv.notify_all();
    }
}

fn worker(shared: &(Mutex<State>, Condvar), cancel: &Cancel, ctx: &egui::Context) {
    let (m, cv) = shared;
    loop {
        let key = {
            let mut st = lock(m);
            loop {
                if st.stop {
                    return;
                }
                match st.queue.pop() {
                    // Scrolled away before its turn: forgotten, asked again if shown again.
                    Some(k) if st.slots.get(&k).is_none_or(|(_, asked)| asked.elapsed() > STALE) => {
                        st.slots.remove(&k);
                    }
                    Some(k) => break k,
                    None => st = cv.wait(st).unwrap_or_else(|e| e.into_inner()),
                }
            }
        };
        let name = key.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // A decoder that panics on a broken file costs that file its thumbnail, not
        // the worker (which would leave every later one waiting forever).
        let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make(&key.path, &name, cancel))).ok().flatten();
        let slot = match made {
            Some(img) => {
                let size = [img.width() as usize, img.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                Slot::Done(ctx.load_texture(format!("thumb:{}", key.path.display()), color, egui::TextureOptions::LINEAR))
            }
            None => Slot::Failed,
        };
        let mut st = lock(m);
        if let Some((s, _)) = st.slots.get_mut(&key) {
            *s = slot;
        }
        // Many made in a burst (a fast scroll): back under the limit now, not at the
        // next one asked for.
        if st.slots.len() > MAX_KEPT {
            forget_oldest(&mut st);
        }
        drop(st);
        ctx.request_repaint();
    }
}

/// A thumbnail at most `SIZE` pixels on its longest side.
fn make(path: &Path, name: &str, cancel: &Cancel) -> Option<image::RgbaImage> {
    #[cfg(test)]
    if name.starts_with("panics-") {
        panic!("a decoder that panics (test)");
    }
    let img = match classify(name) {
        Kind::Image => {
            let f = crate::util::open_regular(path).ok()?;
            let mut bytes = Vec::new();
            f.take(MAX_IMAGE as u64 + 1).read_to_end(&mut bytes).ok()?;
            if bytes.len() > MAX_IMAGE {
                return None;
            }
            decode_image(&bytes, &crate::util::ext_of(name)).ok()?
        }
        Kind::Raw => {
            let mut r = std::io::BufReader::new(crate::util::open_regular(path).ok()?);
            let info = crate::preview::rawphoto::parse(&mut r)?;
            crate::preview::rawphoto::preview_image(&mut r, &info)?
        }
        Kind::Heif => decode_oriented(&crate::preview::media::decode_heif(path, cancel).ok()?).ok()?,
        Kind::Video => {
            let mut r = std::io::BufReader::new(crate::util::open_regular(path).ok()?);
            let duration = crate::preview::media::video_info(&mut r).duration;
            decode_oriented(&crate::preview::media::video_thumbnail(path, duration, cancel)?).ok()?
        }
        _ => return None,
    };
    Some(img.thumbnail(SIZE, SIZE).to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_files_have_one() {
        for name in ["a.jpg", "b.PNG", "c.webp", "d.mov", "e.mkv", "f.cr2", "g.heic"] {
            assert!(has_thumbnail(name), "{name}");
        }
        for name in ["a.txt", "b.psd", "c.pdf", "d.mp3", "e", "f.svg"] {
            assert!(!has_thumbnail(name), "{name}");
        }
    }

    /// A picture becomes a small texture, on a worker; a file that is no picture keeps its
    /// icon; and the file is left as it was.
    #[test]
    fn pictures_get_a_small_texture_and_files_stay_untouched() {
        let dir = std::env::temp_dir().join(format!("mf-thumbs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join("big.png");
        image::RgbaImage::from_pixel(800, 400, image::Rgba([200, 30, 30, 255])).save(&pic).unwrap();
        let fake = dir.join("fake.jpg");
        std::fs::write(&fake, b"not a picture").unwrap();
        let before: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| (e.file_name(), e.metadata().unwrap().modified().unwrap())).collect();
        let ctx = egui::Context::default();
        let thumbs = Thumbs::spawn(ctx);
        let meta = |p: &Path| std::fs::metadata(p).map(|m| (m.modified().ok(), m.len())).unwrap();
        let wait = |p: &Path| {
            let (modified, size) = meta(p);
            let t0 = Instant::now();
            loop {
                if let Some(t) = thumbs.get(p, modified, size) {
                    return Some(t);
                }
                let failed = matches!(lock(&thumbs.shared.0).slots.get(&Key { path: p.to_path_buf(), modified, size }), Some((Slot::Failed, _)));
                if failed || t0.elapsed() > Duration::from_secs(10) {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let tex = wait(&pic).expect("a thumbnail");
        assert_eq!(tex.size(), [SIZE as usize, SIZE as usize / 2], "longest side SIZE, the shape kept");
        assert!(wait(&fake).is_none(), "not a picture: no thumbnail");
        let after: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| (e.file_name(), e.metadata().unwrap().modified().unwrap())).collect();
        assert_eq!(before, after, "nothing written or touched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file Quick View finds too large is not read for a thumbnail either.
    #[test]
    fn too_large_files_are_not_read() {
        let thumbs = Thumbs::spawn(egui::Context::default());
        let at = Path::new("/nowhere");
        for (name, size) in [("huge.heic", MAX_IMAGE as u64 + 1), ("huge.png", MAX_IMAGE as u64 + 1), ("long.mp4", 2 << 30), ("huge.nef", 2 << 30)] {
            assert!(thumbs.get(&at.join(name), None, size).is_none());
        }
        assert_eq!(thumbs.asked(), 0, "none queued");
        assert!(thumbs.get(&at.join("small.png"), None, 1000).is_none());
        assert_eq!(thumbs.asked(), 1, "a small one is");
    }

    /// A decoder that panics costs that file its thumbnail; the workers live on, and
    /// the next picture still gets one.
    #[test]
    fn a_panicking_decoder_does_not_stop_thumbnails() {
        let dir = std::env::temp_dir().join(format!("mf-thumbs-panic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.png");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([1, 2, 3, 255])).save(&good).unwrap();
        let thumbs = Thumbs::spawn(egui::Context::default());
        let state = |p: &Path, size: u64| match lock(&thumbs.shared.0).slots.get(&Key { path: p.to_path_buf(), modified: None, size }) {
            Some((Slot::Failed, _)) => "failed",
            Some((Slot::Done(_), _)) => "done",
            Some((Slot::Queued, _)) => "queued",
            None => "none",
        };
        // More panics than workers: if they died, nothing would be made after them.
        let broken: Vec<PathBuf> = (0..WORKERS * 2).map(|i| dir.join(format!("panics-{i}.png"))).collect();
        let t0 = Instant::now();
        while broken.iter().any(|p| state(p, 10) != "failed") && t0.elapsed() < Duration::from_secs(10) {
            for p in &broken {
                thumbs.get(p, None, 10);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(broken.iter().all(|p| state(p, 10) == "failed"), "each one failed alone");
        let size = std::fs::metadata(&good).unwrap().len();
        let t0 = Instant::now();
        while thumbs.get(&good, None, size).is_none() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(state(&good, size), "done", "the workers still work");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Past the limit, the ones shown longest ago go, down under it; those just shown stay.
    #[test]
    fn the_cache_stays_under_its_limit() {
        let mut st = State { slots: HashMap::new(), queue: Vec::new(), stop: false, trim_at: MAX_KEPT };
        let t0 = Instant::now() - Duration::from_secs(1000);
        for i in 0..MAX_KEPT * 3 {
            st.slots.insert(Key { path: PathBuf::from(format!("/p{i}")), modified: None, size: 1 }, (Slot::Failed, t0 + Duration::from_millis(i as u64)));
        }
        forget_oldest(&mut st);
        assert!(st.slots.len() <= MAX_KEPT * 9 / 10, "{} kept", st.slots.len());
        let newest = Key { path: PathBuf::from(format!("/p{}", MAX_KEPT * 3 - 1)), modified: None, size: 1 };
        assert!(st.slots.contains_key(&newest), "the last shown stays");
        assert!(!st.slots.contains_key(&Key { path: PathBuf::from("/p0"), modified: None, size: 1 }));
    }

    /// Waiting thumbnails scrolled out of sight are forgotten when room is needed.
    #[test]
    fn stale_waiting_thumbnails_are_dropped() {
        let mut st = State { slots: HashMap::new(), queue: Vec::new(), stop: false, trim_at: MAX_KEPT };
        let old = Instant::now() - STALE * 2;
        for i in 0..10 {
            st.slots.insert(Key { path: PathBuf::from(format!("/q{i}")), modified: None, size: 1 }, (Slot::Queued, old));
        }
        st.slots.insert(Key { path: PathBuf::from("/fresh"), modified: None, size: 1 }, (Slot::Queued, Instant::now()));
        forget_oldest(&mut st);
        assert_eq!(st.slots.len(), 1, "only the one still in sight");
    }
}
