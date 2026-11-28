//! Pieces of the preview's interface that the viewers share: the header row, the note
//! for a locked file, textures.
use crate::style::theme::Theme;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, TextureHandle};

/// The error of a document protected by a password or DRM, which only its own app can open.
pub const LOCKED: &str = "\u{1}locked";

/// The small message for a protected file. `Enter password` is offered if this app can
/// open it, which it can for archives; returns whether it was pressed.
pub fn locked_ui(ui: &mut egui::Ui, theme: &Theme, t: &dyn Fn(&str) -> String, can_enter_password: bool) -> bool {
    let mut pressed = false;
    let room = ui.available_height();
    ui.vertical_centered(|ui| {
        ui.add_space((room * 0.5 - 40.0).max(0.0));
        ui.label(egui::RichText::new(format!("🔒 {}", t("Password protected"))).size(13.5).color(theme.text));
        if can_enter_password {
            ui.add_space(4.0);
            pressed = ui.button(t("Enter password…")).clicked();
        } else {
            ui.label(egui::RichText::new(t("Press Enter to open it in its own app.")).size(12.0).color(theme.weak));
        }
    });
    pressed
}

/// Upload an image as a texture. The graphics card has a maximum size, and going over
/// it is a fatal error, so anything larger is scaled down first.
pub fn upload_texture(ctx: &egui::Context, name: impl Into<String>, image: egui::ColorImage) -> TextureHandle {
    let max = ctx.input(|i| i.max_texture_side).max(1);
    let [w, h] = image.size;
    let image = if w.max(h) > max {
        let scale = max as f64 / w.max(h) as f64;
        let (nw, nh) = (((w as f64 * scale) as usize).max(1), ((h as f64 * scale) as usize).max(1));
        let mut pixels = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            for x in 0..nw {
                pixels.push(image.pixels[(y * h / nh) * w + x * w / nw]);
            }
        }
        egui::ColorImage::new([nw, nh], pixels)
    } else {
        image
    };
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}

/// Below this much room beside its buttons, the name of a preview gets a line of its own.
const NAME_ROOM: f32 = 220.0;

/// The title row of a preview: the file name and its facts on the left, cut with "…"
/// when they do not fit (the whole text shows on hover), and the buttons that `buttons`
/// adds, right to left, on the right. The buttons always keep their room; when they
/// leave too little for the name, it goes on a line of its own above them.
pub fn header_row(ui: &mut egui::Ui, theme: &Theme, name: &str, facts: &[(String, Color32)], buttons: impl FnOnce(&mut egui::Ui)) {
    // How wide the buttons were last time: they are drawn before the name.
    let id = ui.id().with("preview-header-buttons");
    let buttons_w: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
    let own_line = ui.available_width() - buttons_w < NAME_ROOM;
    let name_label = || egui::Label::new(egui::RichText::new(name).strong().color(theme.accent)).truncate();
    if own_line {
        ui.add(name_label());
    }
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let before = ui.available_width();
            buttons(ui);
            let used = before - ui.available_width();
            if (used - buttons_w).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(id, used));
                ui.ctx().request_repaint();
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let font = egui::TextStyle::Body.resolve(ui.style());
                let gap = ui.spacing().item_spacing.x;
                let room = ui.available_width();
                let text: Vec<&str> = facts.iter().map(|(s, _)| s.as_str()).collect();
                let facts_w = if facts.is_empty() { 0.0 } else { ui.fonts_mut(|f| f.layout_no_wrap(text.join(" "), font.clone(), theme.weak).size().x) + gap };
                if !own_line {
                    // The name gets what the facts leave, and at least half the row.
                    let name_room = (room - facts_w).max(room * 0.5).max(24.0);
                    ui.allocate_ui_with_layout(egui::vec2(name_room, ui.available_height()), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(name_label())
                    });
                }
                if !facts.is_empty() {
                    let mut job = LayoutJob::default();
                    for (i, (s, c)) in facts.iter().enumerate() {
                        let format = TextFormat { font_id: font.clone(), color: *c, ..Default::default() };
                        if i > 0 {
                            job.append(" ", 0.0, format.clone());
                        }
                        job.append(s, 0.0, format);
                    }
                    ui.add(egui::Label::new(job).truncate());
                }
            });
        });
    });
}
