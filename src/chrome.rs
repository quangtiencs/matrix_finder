//! Window chrome: an integrated title bar that looks native on each platform.
//!
//! * macOS: the content extends under a transparent title bar; the real traffic
//!   lights stay where macOS puts them and the UI leaves room for them.
//! * Windows: Windows 11 caption buttons (46 px wide, red close) in the top-right corner.
//! * Linux: GNOME/Adwaita round buttons or KDE/Breeze glyph buttons, in the order
//!   and on the side configured by the desktop (`button-layout`).
//!
//! The empty parts of the top bar move the window (double-click maximises), and an
//! undecorated window can be resized from its edges.
use crate::theme::Theme;
use egui::{pos2, vec2, Color32, CursorIcon, Rect, ResizeDirection, Sense, Stroke, ViewportCommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Mac,
    Windows,
    Gnome,
    Kde,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Debug)]
pub struct Chrome {
    pub style: Style,
    /// The app draws the title bar (false = the system title bar).
    pub integrated: bool,
    pub left: Vec<Button>,
    pub right: Vec<Button>,
    /// macOS: the 52 pt unified title bar is in place (see `setup_native`).
    mac_tall: bool,
    setup_done: bool,
}

/// Width of one Windows caption button.
const WIN_BTN_W: f32 = 46.0;
const WIN_BTN_H: f32 = 32.0;
/// Room for the macOS traffic lights (compact 28 pt and unified 52 pt title bars).
const MAC_INSET: f32 = 74.0;
const MAC_INSET_TALL: f32 = 84.0;

impl Chrome {
    pub fn detect(integrated: bool) -> Self {
        // MATRIX_FINDER_CHROME=mac|windows|gnome|kde previews another platform's buttons.
        let forced = match std::env::var("MATRIX_FINDER_CHROME").unwrap_or_default().as_str() {
            "mac" | "mac-tall" => Some(Style::Mac),
            "windows" => Some(Style::Windows),
            "gnome" => Some(Style::Gnome),
            "kde" => Some(Style::Kde),
            _ => None,
        };
        let style = if let Some(f) = forced {
            f
        } else if cfg!(target_os = "macos") {
            Style::Mac
        } else if cfg!(windows) {
            Style::Windows
        } else {
            let desk = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
            if desk.contains("kde") || desk.contains("plasma") || desk.contains("lxqt") { Style::Kde } else { Style::Gnome }
        };
        let (left, right) = match style {
            Style::Gnome => gnome_layout().unwrap_or_else(|| (vec![], vec![Button::Minimize, Button::Maximize, Button::Close])),
            _ => (vec![], vec![Button::Minimize, Button::Maximize, Button::Close]),
        };
        // "mac-tall" previews the 52 pt title bar layout on another OS.
        let mac_tall = std::env::var("MATRIX_FINDER_CHROME").is_ok_and(|v| v == "mac-tall");
        Self { style, integrated, left, right, mac_tall, setup_done: false }
    }

    /// Window options for the platform (call before the window is created).
    pub fn viewport(&self, vp: egui::ViewportBuilder) -> egui::ViewportBuilder {
        if !self.integrated {
            return vp;
        }
        match self.style {
            Style::Mac => vp.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false),
            _ => vp.with_decorations(false),
        }
    }

    /// Top margin of the first row (macOS: centre it on the traffic lights).
    pub fn top_margin(&self) -> i8 {
        match self.style {
            Style::Mac if self.integrated && self.mac_tall => 10,
            Style::Mac if self.integrated => 1,
            _ => 6,
        }
    }

    /// Height of the first row, centred on the macOS traffic lights.
    pub fn row_height(&self) -> f32 {
        match self.style {
            Style::Mac if self.integrated && !self.mac_tall => 28.0,
            _ => 32.0,
        }
    }

    /// One-time native setup after the window exists (call from `update`).
    ///
    /// macOS: an empty unified `NSToolbar` (what Electron's `hiddenInset` does) makes
    /// the transparent title bar 52 pt tall, like Finder's, with the traffic lights
    /// centred in it and inset from the corner, so they are not squeezed against the UI.
    pub fn setup_native(&mut self, frame: &eframe::Frame) {
        if std::mem::replace(&mut self.setup_done, true) || !self.integrated || self.style != Style::Mac {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            self.mac_tall = mac::unified_toolbar(frame);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = frame;
    }

    /// macOS: the sidebar's first row belongs to the traffic lights (as in Finder).
    pub fn lights_own_row(&self) -> bool {
        self.integrated && self.style == Style::Mac
    }

    /// Space to keep free at the left end of the first row.
    pub fn left_inset(&self) -> f32 {
        match self.style {
            _ if !self.integrated => 0.0,
            Style::Mac if self.mac_tall => MAC_INSET_TALL,
            Style::Mac => MAC_INSET,
            _ => 0.0,
        }
    }

    /// Space to keep free at the right end of the top bar.
    pub fn right_inset(&self) -> f32 {
        match self.style {
            Style::Windows if self.integrated => WIN_BTN_W * self.right.len() as f32 - 4.0,
            _ => 0.0,
        }
    }

    /// Draw window buttons that live inside the bar (GNOME, KDE). `left` picks the side.
    pub fn inline_buttons(&self, ui: &mut egui::Ui, th: &Theme, left: bool) {
        if !self.integrated || !matches!(self.style, Style::Gnome | Style::Kde) {
            return;
        }
        let list = if left { &self.left } else { &self.right };
        if list.is_empty() {
            return;
        }
        let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        ui.spacing_mut().item_spacing.x = if self.style == Style::Gnome { 8.0 } else { 4.0 };
        // In a right-to-left layout the first button added is the rightmost one.
        let rtl = ui.layout().prefer_right_to_left();
        let order: Vec<Button> = if rtl { list.iter().rev().copied().collect() } else { list.clone() };
        for b in order {
            let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
            let p = ui.painter();
            let c = rect.center();
            let hovered = resp.hovered();
            let pressed = resp.is_pointer_button_down_on();
            let mut glyph = th.text;
            match self.style {
                Style::Gnome => {
                    // Adwaita: a soft round button with a small symbolic icon.
                    let a = if pressed { 0.26 } else if hovered { 0.18 } else { 0.10 };
                    p.circle_filled(c, 12.0, th.text.gamma_multiply(a));
                }
                _ => {
                    // Breeze: glyph only; close turns into a red disc on hover.
                    if b == Button::Close && hovered {
                        p.circle_filled(c, 10.0, Color32::from_rgb(0xda, 0x44, 0x53));
                        glyph = Color32::WHITE;
                    } else if hovered {
                        p.circle_filled(c, 10.0, th.text.gamma_multiply(0.14));
                    }
                }
            }
            let r = Rect::from_center_size(c, vec2(9.0, 9.0));
            let s = Stroke::new(if self.style == Style::Gnome { 1.6_f32 } else { 1.3 }, glyph);
            match (self.style, b) {
                (Style::Kde, Button::Minimize) => chevron(p, r, true, s),
                (Style::Kde, Button::Maximize) if maximized => diamond(p, r, s),
                (Style::Kde, Button::Maximize) => chevron(p, r, false, s),
                (_, Button::Minimize) => {
                    p.line_segment([pos2(r.left(), r.bottom() - 1.0), pos2(r.right(), r.bottom() - 1.0)], s);
                }
                (_, Button::Maximize) => max_glyph(p, r, maximized, s),
                (_, Button::Close) => close_glyph(p, r, s),
            }
            let tip = button_tip(b, maximized);
            if resp.on_hover_text(tip).clicked() {
                press(ui.ctx(), b, maximized);
            }
        }
        ui.add_space(8.0);
    }

    /// Windows 11 caption buttons, flush with the top-right corner of the window.
    pub fn caption_buttons(&self, ctx: &egui::Context, th: &Theme) {
        if !self.integrated || self.style != Style::Windows {
            return;
        }
        let screen = ctx.screen_rect();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let n = self.right.len() as f32;
        egui::Area::new(egui::Id::new("caption-buttons"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.right() - WIN_BTN_W * n, screen.top()))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.horizontal(|ui| {
                    for &b in &self.right {
                        let (rect, resp) = ui.allocate_exact_size(vec2(WIN_BTN_W, WIN_BTN_H), Sense::click());
                        let p = ui.painter();
                        let hovered = resp.hovered();
                        let pressed = resp.is_pointer_button_down_on();
                        let mut glyph = th.text;
                        if b == Button::Close && (hovered || pressed) {
                            let red = if pressed { Color32::from_rgb(0x94, 0x1f, 0x17) } else { Color32::from_rgb(0xc4, 0x2b, 0x1c) };
                            p.rect_filled(rect, 0.0, red);
                            glyph = Color32::WHITE;
                        } else if hovered || pressed {
                            p.rect_filled(rect, 0.0, th.text.gamma_multiply(if pressed { 0.12 } else { 0.07 }));
                        }
                        let r = Rect::from_center_size(rect.center(), vec2(10.0, 10.0));
                        let s = Stroke::new(1.0_f32, glyph);
                        match b {
                            Button::Minimize => {
                                p.line_segment([pos2(r.left(), r.center().y), pos2(r.right(), r.center().y)], s);
                            }
                            Button::Maximize => max_glyph(p, r, maximized, s),
                            Button::Close => close_glyph(p, r, s),
                        }
                        if resp.on_hover_text(button_tip(b, maximized)).clicked() {
                            press(ui.ctx(), b, maximized);
                        }
                    }
                });
            });
    }

    /// Make `rect` move the window (and double-click maximise). Register it before
    /// the widgets that share the area, so they stay clickable.
    pub fn drag_area(&self, ui: &egui::Ui, rect: Rect, id: &str) {
        if !self.integrated {
            return;
        }
        let resp = ui.interact(rect, egui::Id::new(("window-drag", id)), Sense::click_and_drag());
        if resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            let m = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!m));
        }
    }

    /// Resize an undecorated window from its edges (Windows, Linux).
    pub fn resize_edges(&self, ctx: &egui::Context) {
        if !self.integrated || self.style == Style::Mac {
            return;
        }
        let (pos, pressed, maximized) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.primary_pressed(), i.viewport().maximized.unwrap_or(false)));
        let Some(p) = pos else { return };
        if maximized {
            return;
        }
        let r = ctx.screen_rect();
        let m = 5.0;
        let corner = 14.0;
        let (l, rt, t, b) = (p.x - r.left() < m, r.right() - p.x < m, p.y - r.top() < m, r.bottom() - p.y < m);
        let (lc, rc, tc, bc) = (p.x - r.left() < corner, r.right() - p.x < corner, p.y - r.top() < corner, r.bottom() - p.y < corner);
        let dir = match () {
            _ if (t && lc) || (l && tc) => Some((ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest)),
            _ if (t && rc) || (rt && tc) => Some((ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast)),
            _ if (b && lc) || (l && bc) => Some((ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest)),
            _ if (b && rc) || (rt && bc) => Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast)),
            _ if t => Some((ResizeDirection::North, CursorIcon::ResizeNorth)),
            _ if b => Some((ResizeDirection::South, CursorIcon::ResizeSouth)),
            _ if l => Some((ResizeDirection::West, CursorIcon::ResizeWest)),
            _ if rt => Some((ResizeDirection::East, CursorIcon::ResizeEast)),
            _ => None,
        };
        if let Some((d, icon)) = dir {
            ctx.set_cursor_icon(icon);
            if pressed {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
            }
        }
    }

    /// A thin outline so an undecorated window stays visible on a same-coloured desktop.
    pub fn outline(&self, ctx: &egui::Context, th: &Theme) {
        if !self.integrated || self.style == Style::Mac || ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-outline")));
        p.rect_stroke(ctx.screen_rect(), 0.0, Stroke::new(1.0_f32, th.border), egui::StrokeKind::Inside);
    }
}

/// Have quitting asked about while `busy`, for the ways out that do not pass through the
/// window's close request (on macOS: Cmd+Q and the Dock's Quit). Elsewhere they all do.
pub fn set_quit_guard(busy: bool) {
    #[cfg(target_os = "macos")]
    mac::QUIT_GUARD.store(busy, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(target_os = "macos"))]
    let _ = busy;
}

/// Whether the system asked to quit since last time, and the guard turned it down.
pub fn take_native_quit_request() -> bool {
    #[cfg(target_os = "macos")]
    return mac::QUIT_ASKED.swap(false, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(target_os = "macos"))]
    false
}

/// Put the quit guard in place, once there is a real window (not in headless tests).
pub fn install_quit_guard(frame: &eframe::Frame) {
    #[cfg(target_os = "macos")]
    {
        use raw_window_handle::HasWindowHandle;
        static ONCE: std::sync::Once = std::sync::Once::new();
        if frame.window_handle().is_ok() {
            ONCE.call_once(mac::install_quit_guard);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = frame;
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::{class, msg_send, sel};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::atomic::{AtomicBool, Ordering};

    pub static QUIT_GUARD: AtomicBool = AtomicBool::new(false);
    pub static QUIT_ASKED: AtomicBool = AtomicBool::new(false);

    /// NSApplicationTerminateReply.
    const TERMINATE_CANCEL: usize = 0;
    const TERMINATE_NOW: usize = 1;

    extern "C-unwind" fn should_terminate(_this: *mut AnyObject, _cmd: Sel, _sender: *mut AnyObject) -> usize {
        if QUIT_GUARD.load(Ordering::Relaxed) {
            QUIT_ASKED.store(true, Ordering::Relaxed);
            TERMINATE_CANCEL
        } else {
            TERMINATE_NOW
        }
    }

    /// `terminate:` ends the app with no close request for eframe to see, unless the app
    /// delegate's `applicationShouldTerminate:` says otherwise. winit's delegate has none.
    pub fn install_quit_guard() {
        // SAFETY: plain AppKit messages on the main thread (eframe runs `update` there). The
        // method's signature and its type encoding match `applicationShouldTerminate:`, and
        // the function it points to lives as long as the program.
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let delegate: *mut AnyObject = msg_send![app, delegate];
            if delegate.is_null() {
                return;
            }
            let cls: *const AnyClass = msg_send![delegate, class];
            let imp = std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize, Imp>(should_terminate);
            objc2::ffi::class_addMethod(cls as *mut AnyClass, sel!(applicationShouldTerminate:), imp, c"Q@:@".as_ptr());
        }
    }

    /// NSWindowToolbarStyleUnified (macOS 11+).
    const TOOLBAR_STYLE_UNIFIED: isize = 3;
    /// NSWindowTitleHidden.
    const TITLE_HIDDEN: isize = 1;

    pub fn unified_toolbar(frame: &eframe::Frame) -> bool {
        let Ok(handle) = frame.window_handle() else { return false };
        let RawWindowHandle::AppKit(h) = handle.as_raw() else { return false };
        let view = h.ns_view.as_ptr() as *mut AnyObject;
        // SAFETY: plain AppKit messages on the main thread (eframe runs `update` there);
        // every pointer is checked before use.
        unsafe {
            let window: *mut AnyObject = msg_send![view, window];
            if window.is_null() {
                return false;
            }
            let toolbar: *mut AnyObject = msg_send![class!(NSToolbar), new];
            if toolbar.is_null() {
                return false;
            }
            let _: () = msg_send![toolbar, setShowsBaselineSeparator: false];
            let _: () = msg_send![window, setToolbar: toolbar];
            let _: () = msg_send![window, setTitleVisibility: TITLE_HIDDEN];
            let _: () = msg_send![window, setTitlebarAppearsTransparent: true];
            let responds: bool = msg_send![window, respondsToSelector: objc2::sel!(setToolbarStyle:)];
            if responds {
                let _: () = msg_send![window, setToolbarStyle: TOOLBAR_STYLE_UNIFIED];
            }
            responds
        }
    }
}

fn button_tip(b: Button, maximized: bool) -> String {
    use crate::i18n::t;
    match b {
        Button::Minimize => t("Minimize"),
        Button::Maximize if maximized => t("Restore"),
        Button::Maximize => t("Maximize"),
        Button::Close => t("Close"),
    }
}

fn press(ctx: &egui::Context, b: Button, maximized: bool) {
    ctx.send_viewport_cmd(match b {
        Button::Minimize => ViewportCommand::Minimized(true),
        Button::Maximize => ViewportCommand::Maximized(!maximized),
        Button::Close => ViewportCommand::Close,
    });
}

fn max_glyph(p: &egui::Painter, r: Rect, maximized: bool, s: Stroke) {
    if maximized {
        // Restore: two overlapping squares.
        let k = r.width() * 0.22;
        p.rect_stroke(Rect::from_min_max(pos2(r.left(), r.top() + k), pos2(r.right() - k, r.bottom())), 1.0, s, egui::StrokeKind::Inside);
        p.line_segment([pos2(r.left() + k, r.top() + k), pos2(r.left() + k, r.top())], s);
        p.line_segment([pos2(r.left() + k, r.top()), pos2(r.right(), r.top())], s);
        p.line_segment([pos2(r.right(), r.top()), pos2(r.right(), r.bottom() - k)], s);
        p.line_segment([pos2(r.right(), r.bottom() - k), pos2(r.right() - k, r.bottom() - k)], s);
    } else {
        p.rect_stroke(r, 1.0, s, egui::StrokeKind::Inside);
    }
}

fn close_glyph(p: &egui::Painter, r: Rect, s: Stroke) {
    p.line_segment([r.left_top(), r.right_bottom()], s);
    p.line_segment([r.right_top(), r.left_bottom()], s);
}

fn chevron(p: &egui::Painter, r: Rect, down: bool, s: Stroke) {
    let (y0, y1) = if down { (r.top() + r.height() * 0.3, r.bottom() - r.height() * 0.2) } else { (r.bottom() - r.height() * 0.3, r.top() + r.height() * 0.2) };
    p.line_segment([pos2(r.left(), y0), pos2(r.center().x, y1)], s);
    p.line_segment([pos2(r.center().x, y1), pos2(r.right(), y0)], s);
}

fn diamond(p: &egui::Painter, r: Rect, s: Stroke) {
    let c = r.center();
    let h = r.width() / 2.0;
    let pts = vec![pos2(c.x, c.y - h), pos2(c.x + h, c.y), pos2(c.x, c.y + h), pos2(c.x - h, c.y)];
    p.add(egui::Shape::closed_line(pts, s));
}

/// GNOME's `button-layout`, e.g. "appmenu:minimize,maximize,close" (left:right).
fn gnome_layout() -> Option<(Vec<Button>, Vec<Button>)> {
    let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.wm.preferences", "button-layout"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_layout(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_layout(s: &str) -> Option<(Vec<Button>, Vec<Button>)> {
    let s = s.trim().trim_matches('\'').trim_matches('"');
    let (l, r) = s.split_once(':')?;
    let side = |part: &str| -> Vec<Button> {
        part.split(',')
            .filter_map(|n| match n.trim() {
                "minimize" => Some(Button::Minimize),
                "maximize" => Some(Button::Maximize),
                "close" => Some(Button::Close),
                _ => None,
            })
            .collect()
    };
    let (left, right) = (side(l), side(r));
    (!left.is_empty() || !right.is_empty()).then_some((left, right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        assert_eq!(parse_layout("'appmenu:close'\n"), Some((vec![], vec![Button::Close])));
        assert_eq!(parse_layout("'close,minimize,maximize:'"), Some((vec![Button::Close, Button::Minimize, Button::Maximize], vec![])));
        assert_eq!(parse_layout("'appmenu:minimize,maximize,close'"), Some((vec![], vec![Button::Minimize, Button::Maximize, Button::Close])));
        assert_eq!(parse_layout("'appmenu:'"), None);
    }
}
