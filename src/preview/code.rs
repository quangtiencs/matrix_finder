//! Syntax-highlighted code: finding the syntax for a file and colouring its lines (syntect).
use crate::style::theme::Theme;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

pub type Styled = Vec<(Color32, String, bool)>; // colour, text, italic

pub fn code_view(ui: &mut egui::Ui, lines: &[Styled], bg: Color32, wrap: bool, theme: &Theme) {
    let font = FontId::monospace(13.0);
    let gutter = lines.len().to_string().len();
    let mut job = LayoutJob::default();
    for (i, line) in lines.iter().enumerate() {
        job.append(&format!("{:>w$}  ", i + 1, w = gutter), 0.0, TextFormat { font_id: font.clone(), color: theme.weak, ..Default::default() });
        for (c, s, italic) in line {
            job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *c, italics: *italic, ..Default::default() });
        }
        if !line.last().is_some_and(|l| l.1.ends_with('\n')) {
            job.append("\n", 0.0, TextFormat { font_id: font.clone(), ..Default::default() });
        }
    }
    job.wrap.max_width = if wrap { ui.available_width() - 16.0 } else { f32::INFINITY };
    egui::Frame::NONE.fill(bg).inner_margin(6.0).show(ui, |ui| {
        let sa = if wrap { egui::ScrollArea::vertical() } else { egui::ScrollArea::both() };
        sa.auto_shrink([false, false]).show(ui, |ui| {
            let label = egui::Label::new(job);
            ui.add(if wrap { label.wrap() } else { label.extend() });
        });
    });
}

/// Highlight `code` (language name or extension) into styled lines + background.
pub fn highlight_lines(code: &str, hint: &str, theme_name: &str) -> (Vec<Styled>, Color32, String, bool) {
    highlight(code, hint, theme_name)
}

/// A code block without line numbers (Markdown / notebook cells).
pub fn code_view_styled(ui: &mut egui::Ui, lines: &[Styled], bg: Color32, theme: &Theme, width: f32) {
    let _ = theme;
    let mut job = LayoutJob::default();
    let font = FontId::monospace(12.5);
    for (i, line) in lines.iter().enumerate() {
        for (c, s, it) in line {
            job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *c, italics: *it, ..Default::default() });
        }
        if i + 1 < lines.len() && !line.last().is_some_and(|l| l.1.ends_with('\n')) {
            job.append("\n", 0.0, TextFormat { font_id: font.clone(), ..Default::default() });
        }
    }
    egui::Frame::NONE.fill(bg).corner_radius(6).inner_margin(8.0).show(ui, |ui| {
        ui.set_min_width(width - 16.0);
        ui.set_max_width(width - 16.0);
        egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
            ui.add(egui::Label::new(job).extend());
        });
    });
}

fn find_syntax(hint: &str) -> &'static SyntaxReference {
    let ss = &*SYNTAXES;
    let h = hint.to_lowercase();
    let alias = match h.as_str() {
        "ts" | "tsx" | "typescript" | "mjs" | "cjs" | "jsx" | "vue" | "svelte" => "js",
        "kt" | "kts" | "kotlin" | "groovy" | "gradle" | "dart" => "java",
        "toml" | "ini" | "cfg" | "conf" | "properties" | "env" => "ini",
        "zsh" | "fish" | "bash" | "shell" => "sh",
        "jsonc" => "json",
        "yml" => "yaml",
        "htm" => "html",
        "scss" | "sass" | "less" => "css",
        "hpp" | "hh" | "cc" | "cxx" | "c++" => "cpp",
        "ps1" | "bat" | "cmd" => "bat",
        "dockerfile" | "justfile" => "sh",
        "rust" => "rs",
        "python" => "py",
        "javascript" => "js",
        "golang" => "go",
        "ruby" => "rb",
        "csharp" | "c#" => "cs",
        "swift" | "zig" | "nim" => "rs",
        "patch" => "diff",
        "tsv" => "csv",
        other => other,
    };
    ss.find_syntax_by_extension(alias)
        .or_else(|| ss.find_syntax_by_token(alias))
        .or_else(|| if alias == "ini" { ss.find_syntax_by_extension("yaml") } else { None })
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

fn to_color(c: syntect::highlighting::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

pub fn highlight(code: &str, hint: &str, theme_name: &str) -> (Vec<Styled>, Color32, String, bool) {
    let syntax = find_syntax(hint);
    let th = THEMES.themes.get(theme_name).or_else(|| THEMES.themes.values().next()).expect("syntect themes");
    let bg = th.settings.background.map(to_color).unwrap_or(Color32::from_gray(30));
    let fg = th.settings.foreground.map(to_color).unwrap_or(Color32::LIGHT_GRAY);
    let mut h = HighlightLines::new(syntax, th);
    let mut out = Vec::new();
    let mut truncated = false;
    for (i, line) in LinesWithEndings::from(code).enumerate() {
        if i >= super::MAX_LINES {
            truncated = true;
            break;
        }
        // Very long lines (minified files) are not highlighted.
        if line.len() > 4000 {
            out.push(vec![(fg, line.chars().take(4000).collect::<String>() + "…\n", false)]);
            continue;
        }
        match h.highlight_line(line, &SYNTAXES) {
            Ok(ranges) => out.push(
                ranges.into_iter().map(|(st, s)| (to_color(st.foreground), s.replace('\t', "    "), st.font_style.contains(FontStyle::ITALIC))).collect(),
            ),
            Err(_) => out.push(vec![(fg, line.to_string(), false)]),
        }
    }
    (out, bg, syntax.name.clone(), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_rust() {
        let (lines, _, lang, _) = highlight("fn main() {}\n", "rs", "base16-ocean.dark");
        assert_eq!(lang, "Rust");
        assert!(lines[0].len() > 1);
    }
}
