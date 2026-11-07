#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]
//! Matrix Finder: a cross-platform dual-panel file manager.

mod app;
mod archive;
mod checksum;
mod certview;
mod chrome;
mod backup;
mod config;
mod defaultapps;
mod diskusage;
mod docview;
mod filter;
mod fontfallback;
mod fonts;
mod fontview;
mod i18n;
mod layout;
mod logo;
mod media;
mod model3d;
mod icons;
mod instance;
mod ops;
mod palette;
mod pane;
mod preview;
mod rawphoto;
mod remote;
mod scidata;
mod rename;
mod shortcuts;
mod sysicons;
mod table;
mod terminal;
mod textdocs;
mod termview;
mod tabs;
mod theme;
mod util;
mod vfs;

fn main() -> eframe::Result<()> {
    let args = match instance::Args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err((text, code)) => {
            if code == 0 { println!("{text}") } else { eprintln!("{text}") }
            std::process::exit(code);
        }
    };
    let cfg = config::Settings::load();
    let cwd = std::env::current_dir().ok();
    let from_terminal = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let target = args.target(cwd.as_deref(), from_terminal);
    // Already running: open there in a new tab (Settings → Files & layout → Window).
    if args.mode.unwrap_or(cfg.instance_mode) == config::InstanceMode::NewTab {
        let home = || dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
        if instance::hand_off(&target.clone().unwrap_or_else(home)) {
            return Ok(());
        }
    }
    let chrome = chrome::Chrome::detect(cfg.integrated_titlebar);
    let mut viewport = chrome.viewport(egui::ViewportBuilder::default());
    if let Some(icon) = logo::icon_data() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_title("Matrix Finder")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 420.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Matrix Finder", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, chrome, target)))))
}

/// In tests: a heap that can be capped, so a preview that would run a machine out of
/// memory fails right away instead (see `quick_view_stress`), and that reports its peak.
#[cfg(test)]
mod memcap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    pub static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
    pub static PEAK: AtomicUsize = AtomicUsize::new(0);
    static USED: AtomicUsize = AtomicUsize::new(0);

    struct Capped;

    fn reserve(size: usize) -> bool {
        let used = USED.fetch_add(size, Relaxed) + size;
        if used > LIMIT.load(Relaxed) {
            USED.fetch_sub(size, Relaxed);
            return false;
        }
        PEAK.fetch_max(used, Relaxed);
        true
    }

    unsafe impl GlobalAlloc for Capped {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            if reserve(l.size()) { System.alloc(l) } else { std::ptr::null_mut() }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            if reserve(l.size()) { System.alloc_zeroed(l) } else { std::ptr::null_mut() }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            USED.fetch_sub(l.size(), Relaxed);
            System.dealloc(p, l)
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
            if new > l.size() {
                if !reserve(new - l.size()) {
                    return std::ptr::null_mut();
                }
            } else {
                USED.fetch_sub(l.size() - new, Relaxed);
            }
            System.realloc(p, l, new)
        }
    }

    #[global_allocator]
    static HEAP: Capped = Capped;
}
