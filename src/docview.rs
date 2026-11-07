//! Document previews: PDF pages (rendered with hayro, pure Rust) and
//! Jupyter notebooks (.ipynb: markdown, highlighted code, text/image outputs).
use crate::preview::{code_view_styled, highlight_lines, markdown_ui, parse_markdown, MdBlock, Styled};
use crate::theme::Theme;
use egui::{vec2, Color32, RichText};
use std::sync::mpsc::{channel, Receiver};

pub const MAX_PDF_PAGES: usize = 200;
/// Page images are rendered at this width (px) and scaled for display.
const PDF_RENDER_WIDTH: f32 = 1200.0;
/// All pages of a preview together may take this much memory: long documents get
/// smaller page images. A single page has its own limit.
const PDF_TOTAL_BYTES: f32 = 384.0 * 1024.0 * 1024.0;
const PDF_PAGE_BYTES: f32 = 32.0 * 1024.0 * 1024.0;
const PDF_MAX_SIDE: f32 = 4096.0;
/// Images and text of one notebook that are kept for showing.
const NOTEBOOK_IMAGE_BYTES: usize = 256 * 1024 * 1024;
const NOTEBOOK_TEXT_BYTES: usize = 4 * 1024 * 1024;

// ---------------------------------------------------------------- PDF

enum PdfMsg {
    Total(usize),
    Page(egui::ColorImage),
    Error(String),
}

pub struct PdfView {
    rx: Option<Receiver<PdfMsg>>,
    total: Option<usize>,
    pages: Vec<egui::TextureHandle>,
    error: Option<String>,
    /// Display width relative to the available width (1.0 = fit width).
    zoom: f32,
}

/// Start rendering in the background; pages arrive one by one.
pub fn start_pdf(bytes: Vec<u8>, max_texture: usize) -> PdfView {
    let (tx, rx) = channel();
    // Page content can nest deeply.
    let _ = std::thread::Builder::new().name("pdf".into()).stack_size(32 * 1024 * 1024).spawn(move || {
        use hayro::hayro_interpret::InterpreterSettings;
        use hayro::hayro_syntax::Pdf;
        use hayro::{render, PixmapSettings, RenderCache, RenderSettings};
        let pdf = match Pdf::new(bytes) {
            Ok(p) => p,
            Err(e) => {
                // Files with an empty user password (print or copy limits only) do open here.
                let locked = format!("{e:?}").contains("Decryption");
                let _ = tx.send(PdfMsg::Error(if locked { crate::preview::LOCKED.to_string() } else { format!("Cannot open PDF: {e:?}") }));
                return;
            }
        };
        let pages = pdf.pages();
        let _ = tx.send(PdfMsg::Total(pages.len()));
        let cache = RenderCache::new();
        let settings = InterpreterSettings::default();
        let rs = RenderSettings::default();
        let shown = pages.len().clamp(1, MAX_PDF_PAGES) as f32;
        let page_bytes = (PDF_TOTAL_BYTES / shown).min(PDF_PAGE_BYTES);
        let max_side = PDF_MAX_SIDE.min(max_texture.max(64) as f32);
        for page in pages.iter().take(MAX_PDF_PAGES) {
            let (w, h) = page.render_dimensions();
            let scale = page_scale(w, h, page_bytes, max_side);
            let ps = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: hayro::vello_cpu::color::palette::css::WHITE };
            // A broken page must not take the whole preview down.
            let pix = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(page, &cache, &settings, &rs, &ps)));
            let Ok(pix) = pix else { continue };
            let size = [pix.width() as usize, pix.height() as usize];
            let img = egui::ColorImage::from_rgba_premultiplied(size, pix.data_as_u8_slice());
            if tx.send(PdfMsg::Page(img)).is_err() {
                return; // preview closed
            }
        }
    });
    PdfView { rx: Some(rx), total: None, pages: Vec::new(), error: None, zoom: 1.0 }
}

/// The scale that draws a page at the usual width, unless that would make its image
/// larger than a texture may be or take more than its share of memory.
fn page_scale(w: f32, h: f32, max_bytes: f32, max_side: f32) -> f32 {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let wanted = (PDF_RENDER_WIDTH / w).min(4.0);
    wanted.min(max_side / w.max(h)).min((max_bytes / (4.0 * w * h)).sqrt())
}

impl PdfView {
    /// All pages have arrived (for tests).
    #[cfg(test)]
    pub fn is_finished(&self) -> bool {
        self.rx.is_none()
    }

    /// Pages drawn so far and the largest of them (for tests).
    #[cfg(test)]
    pub fn describe(&self) -> String {
        let largest = self.pages.iter().map(|p| p.size()).max_by_key(|s| s[0] * s[1]).unwrap_or_default();
        format!("{} pages drawn of {:?}, largest {}x{}, error {:?}", self.pages.len(), self.total, largest[0], largest[1], self.error)
    }

    fn poll(&mut self, ctx: &egui::Context, key: &str) {
        let Some(rx) = &self.rx else { return };
        loop {
            match rx.try_recv() {
                Ok(PdfMsg::Total(n)) => self.total = Some(n),
                Ok(PdfMsg::Page(img)) => {
                    let name = format!("{key}#p{}", self.pages.len());
                    self.pages.push(crate::preview::upload_texture(ctx, name, img));
                }
                Ok(PdfMsg::Error(e)) => self.error = Some(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(60));
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    break;
                }
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, name: &str, key: &str, th: &Theme, t: &dyn Fn(&str) -> String) {
        self.poll(ui.ctx(), key);
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).strong().color(th.accent));
            let info = match self.total {
                Some(n) if self.rx.is_some() => format!("PDF · {n} {} · {} {}/{}", t("pages"), t("rendering"), self.pages.len(), n.min(MAX_PDF_PAGES)),
                Some(n) => format!("PDF · {n} {}", t("pages")),
                None => "PDF".into(),
            };
            ui.label(RichText::new(info).color(th.weak));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("+").clicked() {
                    self.zoom = (self.zoom * 1.2).min(4.0);
                }
                if ui.button("−").clicked() {
                    self.zoom = (self.zoom / 1.2).max(0.25);
                }
                if ui.selectable_label((self.zoom - 1.0).abs() < 0.01, t("Fit width")).clicked() {
                    self.zoom = 1.0;
                }
            });
        });
        ui.separator();
        if self.error.as_deref() == Some(crate::preview::LOCKED) {
            crate::preview::locked_ui(ui, th, t, false);
            return;
        }
        if let Some(e) = &self.error {
            ui.colored_label(th.danger, e);
            return;
        }
        if self.pages.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t("Loading…"));
            });
            return;
        }
        let total = self.total.unwrap_or(self.pages.len());
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let avail = ui.available_width() - 16.0;
            ui.vertical_centered(|ui| {
                for (i, tex) in self.pages.iter().enumerate() {
                    let s = tex.size_vec2();
                    let w = (avail * self.zoom).max(80.0);
                    let size = vec2(w, w * s.y / s.x.max(1.0));
                    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    // Paper shadow + page
                    ui.painter().rect_filled(r.translate(vec2(0.0, 2.0)).expand(1.0), 3.0, Color32::from_black_alpha(60));
                    ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    ui.label(RichText::new(format!("{} / {total}", i + 1)).size(11.0).color(th.weak));
                    ui.add_space(8.0);
                }
                if total > MAX_PDF_PAGES && self.rx.is_none() {
                    ui.label(RichText::new(format!("{} {MAX_PDF_PAGES} {}", t("Showing the first"), t("pages"))).color(th.weak));
                }
            });
        });
    }
}

// ---------------------------------------------------------------- Jupyter

enum Output {
    Text { text: String, error: bool },
    Image(usize),
    Markdown(Vec<MdBlock>),
}

enum Cell {
    Markdown(Vec<MdBlock>),
    Code { count: Option<i64>, lines: Vec<Styled>, bg: Color32, outputs: Vec<Output> },
    Raw(String),
}

pub struct NotebookView {
    language: String,
    kernel: String,
    cells: Vec<Cell>,
    /// Decoded output images (rgba, w, h) until uploaded as textures.
    images: Vec<Option<(Vec<u8>, usize, usize)>>,
    textures: Vec<Option<egui::TextureHandle>>,
}

const MAX_CELLS: usize = 1000;
const MAX_OUTPUT_CHARS: usize = 20_000;

/// nbformat "multiline strings" are either a string or a list of strings.
fn joined(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<String>(),
        _ => String::new(),
    }
}

fn strip_ansi(s: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
    RE.replace_all(s, "").to_string()
}

fn clip_text(mut s: String) -> String {
    if s.len() > MAX_OUTPUT_CHARS {
        let mut end = MAX_OUTPUT_CHARS;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push_str("\n…");
    }
    s
}

fn decode_image(b64: &str) -> Option<(Vec<u8>, usize, usize)> {
    use base64::Engine;
    let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(clean).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let img = if img.width() > 2048 || img.height() > 2048 { img.thumbnail(2048, 2048) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    Some((rgba.into_raw(), w, h))
}

pub fn parse_notebook(bytes: &[u8], syntax_theme: &str) -> Result<NotebookView, String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("Invalid notebook JSON: {e}"))?;
    let meta = &v["metadata"];
    let language = meta["language_info"]["name"]
        .as_str()
        .or_else(|| meta["kernelspec"]["language"].as_str())
        .unwrap_or("python")
        .to_string();
    let kernel = meta["kernelspec"]["display_name"].as_str().unwrap_or(&language).to_string();
    let mut images = Vec::new();
    let mut cells = Vec::new();
    let raw_cells = v["cells"].as_array().ok_or("Not a Jupyter notebook (no \"cells\")")?;
    let mut image_bytes = 0;
    let mut text_bytes = 0;
    for c in raw_cells.iter().take(MAX_CELLS) {
        // The rest of a huge notebook is left out; laying text out is what takes the memory.
        if text_bytes > NOTEBOOK_TEXT_BYTES {
            cells.push(Cell::Raw(crate::i18n::t("(truncated)")));
            break;
        }
        let src = joined(&c["source"]);
        text_bytes += src.len();
        match c["cell_type"].as_str().unwrap_or("") {
            "markdown" => cells.push(Cell::Markdown(parse_markdown(&src, syntax_theme))),
            "code" => {
                let (lines, bg, _, _) = highlight_lines(src.trim_end_matches('\n'), &language, syntax_theme);
                let mut outputs = Vec::new();
                for o in c["outputs"].as_array().into_iter().flatten() {
                    match o["output_type"].as_str().unwrap_or("") {
                        "stream" => outputs.push(Output::Text { text: clip_text(strip_ansi(&joined(&o["text"]))), error: o["name"] == "stderr" }),
                        "error" => {
                            let tb: Vec<String> = o["traceback"].as_array().into_iter().flatten().filter_map(|l| l.as_str()).map(strip_ansi).collect();
                            let text = if tb.is_empty() {
                                format!("{}: {}", o["ename"].as_str().unwrap_or("Error"), o["evalue"].as_str().unwrap_or(""))
                            } else {
                                tb.join("\n")
                            };
                            outputs.push(Output::Text { text: clip_text(text), error: true });
                        }
                        "execute_result" | "display_data" => {
                            let data = &o["data"];
                            let room_for_image = image_bytes < NOTEBOOK_IMAGE_BYTES;
                            if let Some(img) = ["image/png", "image/jpeg", "image/gif"].iter().filter(|_| room_for_image).find_map(|k| data[*k].as_str()).and_then(decode_image) {
                                image_bytes += img.0.len();
                                images.push(Some(img));
                                outputs.push(Output::Image(images.len() - 1));
                            } else if !room_for_image && ["image/png", "image/jpeg", "image/gif"].iter().any(|k| data.get(*k).is_some()) {
                                outputs.push(Output::Text { text: crate::i18n::t("(truncated)"), error: false });
                            } else if data.get("text/markdown").is_some() {
                                outputs.push(Output::Markdown(parse_markdown(&joined(&data["text/markdown"]), syntax_theme)));
                            } else if data.get("text/plain").is_some() {
                                outputs.push(Output::Text { text: clip_text(joined(&data["text/plain"])), error: false });
                            } else if data.get("text/html").is_some() {
                                outputs.push(Output::Text { text: "[HTML output]".into(), error: false });
                            }
                        }
                        _ => {}
                    }
                }
                cells.push(Cell::Code { count: c["execution_count"].as_i64(), lines, bg, outputs });
            }
            _ => cells.push(Cell::Raw(clip_text(src))),
        }
    }
    let n = images.len();
    Ok(NotebookView { language, kernel, cells, images, textures: (0..n).map(|_| None).collect() })
}

impl NotebookView {
    pub fn ui(&mut self, ui: &mut egui::Ui, name: &str, key: &str, th: &Theme, t: &dyn Fn(&str) -> String) {
        // Upload decoded images once.
        for (i, img) in self.images.iter_mut().enumerate() {
            if let Some((rgba, w, h)) = img.take() {
                let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                self.textures[i] = Some(crate::preview::upload_texture(ui.ctx(), format!("{key}#img{i}"), ci));
            }
        }
        let code_cells = self.cells.iter().filter(|c| matches!(c, Cell::Code { .. })).count();
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).strong().color(th.accent));
            ui.label(RichText::new(format!("Jupyter · {} · {} {} · {code_cells} {}", self.kernel, self.cells.len(), t("cells"), t("code"))).color(th.weak));
        });
        ui.separator();
        let gutter = 58.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for cell in &self.cells {
                ui.horizontal_top(|ui| {
                    let label = match cell {
                        Cell::Code { count, .. } => format!("[{}]", count.map(|c| c.to_string()).unwrap_or_else(|| " ".into())),
                        _ => String::new(),
                    };
                    ui.allocate_ui(vec2(gutter, 18.0), |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            ui.label(RichText::new(label).monospace().size(11.5).color(th.accent.gamma_multiply(0.8)));
                        });
                    });
                    ui.vertical(|ui| {
                        let w = ui.available_width();
                        match cell {
                            Cell::Markdown(blocks) => markdown_ui(ui, blocks, th),
                            Cell::Raw(text) => {
                                ui.label(RichText::new(text).monospace().color(th.weak));
                            }
                            Cell::Code { lines, bg, outputs, .. } => {
                                code_view_styled(ui, lines, *bg, th, w);
                                for o in outputs {
                                    ui.add_space(4.0);
                                    match o {
                                        Output::Text { text, error } => {
                                            let fill = if *error { th.danger.gamma_multiply(0.12) } else { Color32::TRANSPARENT };
                                            egui::Frame::NONE.fill(fill).corner_radius(4).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                                                ui.set_max_width(w - 12.0);
                                                let color = if *error { th.danger } else { th.text };
                                                ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0).color(color)).wrap());
                                            });
                                        }
                                        Output::Image(i) => {
                                            if let Some(Some(tex)) = self.textures.get(*i) {
                                                let s = tex.size_vec2();
                                                let scale = (w / s.x).min(1.0);
                                                ui.add(egui::Image::new((tex.id(), s * scale)));
                                            }
                                        }
                                        Output::Markdown(blocks) => markdown_ui(ui, blocks, th),
                                    }
                                }
                            }
                        }
                    });
                });
                ui.add_space(10.0);
            }
            if self.cells.len() >= MAX_CELLS {
                ui.label(RichText::new(t("(truncated)")).color(th.weak));
            }
        });
        let _ = &self.language;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notebook_parses_cells_and_outputs() {
        // 1x1 red PNG
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";
        let nb = format!(
            r##"{{"nbformat":4,"nbformat_minor":5,
            "metadata":{{"kernelspec":{{"display_name":"Python 3","language":"python","name":"python3"}},"language_info":{{"name":"python"}}}},
            "cells":[
              {{"cell_type":"markdown","metadata":{{}},"source":["# Title\n","Some *text*"]}},
              {{"cell_type":"code","execution_count":1,"metadata":{{}},"source":"print('hi')\n1+1",
                "outputs":[{{"output_type":"stream","name":"stdout","text":["hi\n"]}},
                           {{"output_type":"execute_result","execution_count":1,"data":{{"text/plain":"2"}},"metadata":{{}}}},
                           {{"output_type":"display_data","data":{{"image/png":"{png}","text/plain":"<Figure>"}},"metadata":{{}}}},
                           {{"output_type":"error","ename":"ValueError","evalue":"bad","traceback":["\u001b[0;31mValueError\u001b[0m: bad"]}}]}}
            ]}}"##
        );
        let v = parse_notebook(nb.as_bytes(), "base16-ocean.dark").unwrap();
        assert_eq!(v.kernel, "Python 3");
        assert_eq!(v.cells.len(), 2);
        assert!(matches!(v.cells[0], Cell::Markdown(_)));
        let Cell::Code { count, outputs, .. } = &v.cells[1] else { panic!() };
        assert_eq!(*count, Some(1));
        assert_eq!(outputs.len(), 4);
        assert!(matches!(&outputs[0], Output::Text { text, error: false } if text == "hi\n"));
        assert!(matches!(outputs[2], Output::Image(0)));
        assert!(matches!(&outputs[3], Output::Text { text, error: true } if text == "ValueError: bad"));
        assert!(v.images[0].is_some());
        assert!(parse_notebook(b"{}", "base16-ocean.dark").is_err());
    }

    #[test]
    fn pdf_renders_pages() {
        // Minimal one-page PDF with a filled rectangle.
        let content = "0 0 1 rg 60 60 300 300 re f";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 600] /Contents 4 0 R /Resources << >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = vec![];
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
        }
        let xref = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1));
        for off in offsets {
            pdf.push_str(&format!("{off:010} 00000 n \n"));
        }
        pdf.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1));
        let view = start_pdf(pdf.into_bytes(), 4096);
        let rx = view.rx.unwrap();
        let mut total = None;
        let mut page = None;
        for _ in 0..100 {
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(PdfMsg::Total(n)) => total = Some(n),
                Ok(PdfMsg::Page(img)) => {
                    page = Some(img);
                    break;
                }
                Ok(PdfMsg::Error(e)) => panic!("{e}"),
                Err(_) => break,
            }
        }
        assert_eq!(total, Some(1));
        let img = page.expect("page rendered");
        assert_eq!(img.size[0], PDF_RENDER_WIDTH as usize);
        // The blue rectangle is drawn somewhere on the white page.
        assert!(img.pixels.iter().any(|p| p.b() > 200 && p.r() < 60));
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn page_images_stay_within_their_limits() {
        let budget = 2.0 * 1024.0 * 1024.0;
        let side = 4096.0;
        // An ordinary page in a short document keeps the usual width.
        assert_eq!(page_scale(612.0, 792.0, PDF_PAGE_BYTES, side), PDF_RENDER_WIDTH / 612.0);
        for (w, h) in [(612.0, 792.0), (612.0, 200_000.0), (14_400.0, 14_400.0), (200_000.0, 10.0), (0.0, 0.0)] {
            let s = page_scale(w, h, budget, side);
            let (pw, ph) = (w as f32 * s, h as f32 * s);
            assert!(pw <= side && ph <= side, "{w}x{h} gives {pw}x{ph}");
            assert!(pw * ph * 4.0 <= budget * 1.01, "{w}x{h} gives {pw}x{ph}");
            assert!(s > 0.0);
        }
    }
}
