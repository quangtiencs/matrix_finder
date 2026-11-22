#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]
//! Matrix Finder: a cross-platform dual-panel file manager.

mod app;
mod config;
mod files;
mod i18n;
mod preview;
mod style;
mod system;
mod terminal;
mod tools;
mod ui;
mod util;

use style::logo;
use system::{activation, instance};
use ui::chrome;

fn main() -> eframe::Result<()> {
    let args = match instance::Args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err((text, code)) => {
            if code == 0 { println!("{text}") } else { eprintln!("{text}") }
            std::process::exit(code);
        }
    };
    // The desktop's startup token, for the window (and not for programs started from here).
    activation::take_token();
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
    let mut chrome = chrome::Chrome::detect(cfg.integrated_titlebar);
    chrome.rounded &= cfg.round_corners;
    let mut viewport = chrome.viewport(egui::ViewportBuilder::default());
    if let Some(icon) = logo::icon_data() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_title("Matrix Finder")
            // The name of the .desktop file scripts/install.sh installs, so the dock shows the
            // app's icon and name: the app id on Wayland, and (winit keeps one name for both)
            // the window class on X11, which the file's StartupWMClass matches.
            .with_app_id("org.matrixfinder.MatrixFinder")
            // Settings → Files & layout → Window (kept in range when the settings load).
            .with_inner_size([cfg.window_width, cfg.window_height])
            .with_min_inner_size(config::WINDOW_MIN)
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
