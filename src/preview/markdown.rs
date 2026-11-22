//! Markdown: parsing it into blocks of styled spans, and drawing them.
use crate::preview::code::{highlight, Styled};
use crate::style::theme::Theme;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

/// Laying out text takes about 200 bytes per character, kept while it is on screen. One
/// block of Markdown is limited to this much text, and one document to `MAX_MARKDOWN`.
pub const MAX_BLOCK_TEXT: usize = 64 * 1024;

const MAX_MARKDOWN: usize = 1024 * 1024;

#[derive(Clone)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: bool,
}

pub enum MdBlock {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    Quote(Vec<Span>),
    Item { depth: usize, marker: String, spans: Vec<Span> },
    Code { lines: Vec<Styled>, bg: Color32 },
    Table { rows: Vec<Vec<Vec<Span>>>, has_header: bool },
    Rule,
}

/// Plain text as a code block, for documents that have preformatted text.
pub fn plain_code_block(code: &str, syntax_theme: &str) -> MdBlock {
    let code = &code[..floor_char(code, MAX_MARKDOWN / 4)];
    let (lines, bg, _, _) = highlight(code, "txt", syntax_theme);
    MdBlock::Code { lines, bg }
}

/// The largest length at most `max` that ends between two characters of `s`.
fn floor_char(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Keep at most `max` bytes of text across these spans.
fn clip_spans(spans: &mut Vec<Span>, max: usize) {
    let mut left = max;
    let mut cut = false;
    spans.retain_mut(|s| {
        if cut {
            return false;
        }
        if s.text.len() > left {
            s.text.truncate(floor_char(&s.text, left));
            s.text.push('…');
            cut = true;
        } else {
            left -= s.text.len();
        }
        true
    });
}

/// Limit how much text each block holds (see `MAX_BLOCK_TEXT`).
pub fn clip_block_text(blocks: &mut [MdBlock]) {
    for b in blocks {
        match b {
            MdBlock::Heading(_, spans) | MdBlock::Para(spans) | MdBlock::Quote(spans) | MdBlock::Item { spans, .. } => clip_spans(spans, MAX_BLOCK_TEXT),
            MdBlock::Table { rows, .. } => rows.iter_mut().flatten().for_each(|cell| clip_spans(cell, MAX_BLOCK_TEXT / 16)),
            MdBlock::Code { .. } | MdBlock::Rule => {}
        }
    }
}

pub fn parse_markdown(src: &str, syntax_theme: &str) -> Vec<MdBlock> {
    let end = floor_char(src, MAX_MARKDOWN);
    let mut blocks = parse_markdown_blocks(&src[..end], syntax_theme);
    clip_block_text(&mut blocks);
    if end < src.len() {
        blocks.push(MdBlock::Para(vec![Span { text: crate::i18n::t("(truncated)"), bold: false, italic: true, code: false, strike: false, link: false }]));
    }
    blocks
}

fn parse_markdown_blocks(src: &str, syntax_theme: &str) -> Vec<MdBlock> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut blocks = Vec::new();
    let mut buf: Vec<Span> = Vec::new();
    let (mut bold, mut italic, mut strike, mut link) = (0, 0, 0, 0);
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut marker: Option<String> = None;
    let mut quote = 0;
    let mut code: Option<(String, String)> = None;
    let mut heading: Option<u8> = None;
    let mut table: Option<(Vec<Vec<Vec<Span>>>, Vec<Vec<Span>>, bool)> = None;
    let mut in_head = false;
    let mut image_alt: Option<String> = None;

    macro_rules! push_span {
        ($text:expr, $code:expr) => {{
            let s = Span { text: $text, bold: bold > 0 || heading.is_some(), italic: italic > 0, code: $code, strike: strike > 0, link: link > 0 };
            buf.push(s);
        }};
    }

    let flush_item = |buf: &mut Vec<Span>, marker: &mut Option<String>, lists: &Vec<Option<u64>>, blocks: &mut Vec<MdBlock>| {
        if !buf.is_empty() || marker.is_some() {
            blocks.push(MdBlock::Item { depth: lists.len().saturating_sub(1), marker: marker.take().unwrap_or_default(), spans: std::mem::take(buf) });
        }
    };

    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => heading = Some(level as u8),
                Tag::BlockQuote { .. } => quote += 1,
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    if !lists.is_empty() {
                        flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                    }
                    lists.push(start);
                }
                Tag::Item => {
                    flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                    let m = match lists.last_mut() {
                        Some(Some(n)) => {
                            let s = format!("{n}.");
                            *n += 1;
                            s
                        }
                        _ => ["•", "–", "▪"][(lists.len().saturating_sub(1)) % 3].to_string(),
                    };
                    marker = Some(m);
                }
                Tag::Table(_) => table = Some((Vec::new(), Vec::new(), false)),
                Tag::TableHead => in_head = true,
                Tag::TableRow | Tag::TableCell => {}
                Tag::Emphasis => italic += 1,
                Tag::Strong => bold += 1,
                Tag::Strikethrough => strike += 1,
                Tag::Link { .. } => link += 1,
                Tag::Image { .. } => image_alt = Some(String::new()),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if !lists.is_empty() {
                        flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                        marker = None;
                    } else if quote > 0 {
                        blocks.push(MdBlock::Quote(std::mem::take(&mut buf)));
                    } else if !buf.is_empty() {
                        blocks.push(MdBlock::Para(std::mem::take(&mut buf)));
                    }
                }
                TagEnd::Heading { .. } => {
                    blocks.push(MdBlock::Heading(heading.take().unwrap_or(1), std::mem::take(&mut buf)));
                }
                TagEnd::BlockQuote { .. } => quote -= 1,
                TagEnd::CodeBlock => {
                    if let Some((lang, mut text)) = code.take() {
                        while text.ends_with('\n') {
                            text.pop();
                        }
                        let (lines, bg, _, _) = highlight(&text, if lang.is_empty() { "txt" } else { &lang }, syntax_theme);
                        blocks.push(MdBlock::Code { lines, bg });
                    }
                }
                TagEnd::List { .. } => {
                    flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                    lists.pop();
                }
                TagEnd::Item => {
                    if !buf.is_empty() {
                        flush_item(&mut buf, &mut marker, &lists, &mut blocks);
                    }
                    marker = None;
                }
                TagEnd::TableCell => {
                    if let Some((_, row, _)) = &mut table {
                        row.push(std::mem::take(&mut buf));
                    }
                }
                TagEnd::TableHead => {
                    in_head = false;
                    if let Some((rows, row, has)) = &mut table {
                        rows.push(std::mem::take(row));
                        *has = true;
                    }
                }
                TagEnd::TableRow => {
                    if let Some((rows, row, _)) = &mut table {
                        rows.push(std::mem::take(row));
                    }
                }
                TagEnd::Table => {
                    if let Some((rows, _, has_header)) = table.take() {
                        blocks.push(MdBlock::Table { rows, has_header });
                    }
                }
                TagEnd::Emphasis => italic -= 1,
                TagEnd::Strong => bold -= 1,
                TagEnd::Strikethrough => strike -= 1,
                TagEnd::Link => link -= 1,
                TagEnd::Image => {
                    if let Some(alt) = image_alt.take() {
                        push_span!(format!("[🖼 {}]", if alt.is_empty() { "image" } else { &alt }), false);
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, c)) = &mut code {
                    c.push_str(&t);
                } else if let Some(alt) = &mut image_alt {
                    alt.push_str(&t);
                } else {
                    push_span!(t.to_string(), false);
                }
            }
            Event::Code(t) => push_span!(t.to_string(), true),
            Event::SoftBreak => push_span!(" ".to_string(), false),
            Event::HardBreak => push_span!("\n".to_string(), false),
            Event::Rule => blocks.push(MdBlock::Rule),
            Event::TaskListMarker(done) => {
                marker = Some(if done { "☑" } else { "☐" }.to_string());
            }
            _ => {}
        }
    }
    let _ = in_head;
    if !buf.is_empty() {
        blocks.push(MdBlock::Para(buf));
    }
    blocks
}

fn spans_job(spans: &[Span], size: f32, theme: &Theme, base: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for s in spans {
        // Reading view: Source Sans (or Source Serif, see Settings → Fonts) for prose,
        // Source Code Pro for code.
        let (family, slanted) = crate::style::fonts::reading_font(s.bold, s.italic);
        let mut fmt = TextFormat {
            font_id: if s.code { FontId::monospace(size * 0.88) } else { FontId::new(size, family) },
            color: if s.link { theme.accent } else { base },
            italics: s.italic && (s.code || slanted),
            ..Default::default()
        };
        if s.code {
            fmt.background = theme.surface;
            fmt.color = theme.marked;
        }
        if s.link {
            fmt.underline = egui::Stroke::new(1.0_f32, theme.accent);
        }
        if s.strike {
            fmt.strikethrough = egui::Stroke::new(1.0_f32, base);
        }
        job.append(&s.text, 0.0, fmt);
    }
    job.wrap.max_width = width;
    job
}

pub fn markdown_ui(ui: &mut egui::Ui, blocks: &[MdBlock], theme: &Theme) {
    let width = ui.available_width() - 8.0;
    for b in blocks {
        match b {
            MdBlock::Heading(level, spans) => {
                let size = match level {
                    1 => 26.0,
                    2 => 21.0,
                    3 => 18.0,
                    4 => 16.0,
                    _ => 15.0,
                };
                ui.add_space(6.0);
                ui.label(spans_job(spans, size, theme, theme.accent, width));
                if *level <= 2 {
                    let r = ui.available_rect_before_wrap();
                    ui.painter().hline(r.x_range(), r.top(), egui::Stroke::new(1.0_f32, theme.border));
                    ui.add_space(4.0);
                }
            }
            MdBlock::Para(spans) => {
                ui.label(spans_job(spans, 14.0, theme, theme.text, width));
                ui.add_space(6.0);
            }
            MdBlock::Quote(spans) => {
                egui::Frame::NONE.stroke(egui::Stroke::new(0.0_f32, theme.border)).inner_margin(egui::Margin { left: 10, right: 4, top: 2, bottom: 2 }).show(
                    ui,
                    |ui| {
                        let r = ui.label(spans_job(spans, 14.0, theme, theme.weak, width - 20.0)).rect;
                        ui.painter().vline(r.left() - 7.0, r.y_range(), egui::Stroke::new(3.0_f32, theme.accent));
                    },
                );
                ui.add_space(6.0);
            }
            MdBlock::Item { depth, marker, spans } => {
                ui.horizontal_top(|ui| {
                    ui.add_space(12.0 + *depth as f32 * 18.0);
                    ui.label(egui::RichText::new(marker).color(theme.accent));
                    let w = ui.available_width();
                    ui.label(spans_job(spans, 14.0, theme, theme.text, w));
                });
            }
            MdBlock::Code { lines, bg } => {
                let mut job = LayoutJob::default();
                let font = FontId::monospace(12.5);
                for line in lines {
                    for (c, s, it) in line {
                        job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *c, italics: *it, ..Default::default() });
                    }
                }
                egui::Frame::NONE.fill(*bg).inner_margin(8.0).show(ui, |ui| {
                    ui.set_min_width(width - 16.0);
                    egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
                        ui.add(egui::Label::new(job).extend());
                    });
                });
                ui.add_space(6.0);
            }
            MdBlock::Table { rows, has_header } => {
                egui::Grid::new(ui.next_auto_id()).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                    for (i, row) in rows.iter().enumerate() {
                        for cell in row {
                            let mut c = cell.clone();
                            if i == 0 && *has_header {
                                for s in &mut c {
                                    s.bold = true;
                                }
                            }
                            ui.label(spans_job(&c, 13.5, theme, theme.text, 400.0));
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
            }
            MdBlock::Rule => {
                ui.separator();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_clipped_for_layout() {
        let blocks = parse_markdown(&"word ".repeat(100_000), "InspiredGitHub");
        let MdBlock::Para(spans) = &blocks[0] else { panic!() };
        assert!(spans.iter().map(|s| s.text.len()).sum::<usize>() <= MAX_BLOCK_TEXT + 8);
        // Over the whole document's limit: cut, and it says so.
        let many = "para\n\n".repeat(400_000);
        let blocks = parse_markdown(&many, "InspiredGitHub");
        assert!(matches!(blocks.last(), Some(MdBlock::Para(s)) if s[0].text.contains("truncated")));
        let mut spans = vec![Span { text: "é".repeat(10), bold: false, italic: false, code: false, strike: false, link: false }];
        clip_spans(&mut spans, 5);
        assert_eq!(spans[0].text, "éé…", "cut between characters");
    }

    #[test]
    fn markdown_blocks() {
        let md = "# Title\n\nSome **bold** and `code`.\n\n- a\n- b\n  1. nested\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n> quote\n\n---\n- [x] done\n";
        let blocks = parse_markdown(md, "base16-ocean.dark");
        assert!(matches!(blocks[0], MdBlock::Heading(1, _)));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Code { .. })));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Table { .. })));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Quote(_))));
        assert!(blocks.iter().any(|b| matches!(b, MdBlock::Rule)));
        let items: Vec<_> =
            blocks.iter().filter_map(|b| if let MdBlock::Item { depth, marker, .. } = b { Some((*depth, marker.clone())) } else { None }).collect();
        assert_eq!(items[0], (0, "•".into()));
        assert_eq!(items[2], (1, "1.".into()));
        assert!(items.iter().any(|i| i.1.contains('☑')));
    }
}
