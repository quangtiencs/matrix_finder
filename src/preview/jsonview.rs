//! JSON previews: the document as a tree to open and close, whose rows give their path
//! (`j['data'][0]['name']`, or as JavaScript, jq or JSONPath) and value to copy.
use crate::i18n::t;
use crate::preview::code::Styled;
use crate::style::theme::Theme;
use crate::ui::KeptContextMenu as _;
use crate::util::ext_of;
use egui::{Color32, FontId, Sense};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU8, Ordering};

/// Larger files are shown as text: their tree would take many times their size in memory.
pub const MAX_JSON: usize = 32 * 1024 * 1024;
/// A folder of the tree shows this many children at first, then this many more per click.
const PAGE: usize = 1000;
/// "Expand all" stops once this many rows are showing.
const MAX_EXPANDED_ROWS: usize = 20_000;
/// The first view opens levels while it stays about this short.
const FIRST_ROWS: usize = 80;
const ROW_H: f32 = 20.0;

pub fn is_json(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "json" | "jsonld" | "har" | "webmanifest")
}

/// A JSON value. Objects keep their keys in the order of the file; integers stay exact.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Node>),
    Obj(Vec<(String, Node)>),
}

impl<'de> serde::Deserialize<'de> for Node {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Node;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Node, E> {
                Ok(Node::Null)
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Node, E> {
                Ok(Node::Bool(v))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Node, E> {
                Ok(Node::Num(v.to_string()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Node, E> {
                Ok(Node::Num(v.to_string()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Node, E> {
                Ok(Node::Num(serde_json::Number::from_f64(v).map_or_else(|| v.to_string(), |n| n.to_string())))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Node, E> {
                Ok(Node::Str(v.to_string()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Node, E> {
                Ok(Node::Str(v))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
                // The length it claims is not trusted for the allocation.
                let mut v = Vec::new();
                while let Some(n) = seq.next_element()? {
                    v.push(n);
                }
                Ok(Node::Arr(v))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
                let mut v = Vec::new();
                while let Some((k, n)) = map.next_entry::<String, Node>()? {
                    v.push((k, n));
                }
                Ok(Node::Obj(v))
            }
        }
        d.deserialize_any(V)
    }
}

impl Node {
    fn len(&self) -> usize {
        match self {
            Node::Arr(v) => v.len(),
            Node::Obj(v) => v.len(),
            _ => 0,
        }
    }

    fn is_container(&self) -> bool {
        matches!(self, Node::Arr(_) | Node::Obj(_))
    }

    fn child(&self, i: usize) -> Option<&Node> {
        match self {
            Node::Arr(v) => v.get(i),
            Node::Obj(v) => v.get(i).map(|(_, n)| n),
            _ => None,
        }
    }

    /// How many values there are in all, and how deep they go.
    fn stats(&self) -> (usize, usize) {
        match self {
            Node::Arr(v) => v.iter().fold((1, 1), |(n, d), c| {
                let (cn, cd) = c.stats();
                (n + cn, d.max(cd + 1))
            }),
            Node::Obj(v) => v.iter().fold((1, 1), |(n, d), (_, c)| {
                let (cn, cd) = c.stats();
                (n + cn, d.max(cd + 1))
            }),
            _ => (1, 0),
        }
    }
}

/// Read a JSON document (one value, possibly with white space around it).
pub fn parse(bytes: &[u8]) -> Result<Node, String> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}

/// The value as JSON text: indented for objects and arrays, plain for a string.
pub fn value_text(n: &Node) -> String {
    match n {
        Node::Str(s) => s.clone(),
        _ => {
            let mut out = String::new();
            write_json(n, 0, &mut out);
            out
        }
    }
}

fn write_json(n: &Node, indent: usize, out: &mut String) {
    let pad = |out: &mut String, level: usize| out.extend(std::iter::repeat_n("  ", level));
    match n {
        Node::Null => out.push_str("null"),
        Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Node::Num(s) => out.push_str(s),
        Node::Str(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Node::Arr(v) if v.is_empty() => out.push_str("[]"),
        Node::Obj(v) if v.is_empty() => out.push_str("{}"),
        Node::Arr(v) => {
            out.push_str("[\n");
            for (i, c) in v.iter().enumerate() {
                pad(out, indent + 1);
                write_json(c, indent + 1, out);
                out.push_str(if i + 1 < v.len() { ",\n" } else { "\n" });
            }
            pad(out, indent);
            out.push(']');
        }
        Node::Obj(v) => {
            out.push_str("{\n");
            for (i, (k, c)) in v.iter().enumerate() {
                pad(out, indent + 1);
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push_str(": ");
                write_json(c, indent + 1, out);
                out.push_str(if i + 1 < v.len() { ",\n" } else { "\n" });
            }
            pad(out, indent);
            out.push('}');
        }
    }
}

// ---------------------------------------------------------------- paths

/// One step down the tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Seg<'a> {
    Key(&'a str),
    Index(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathStyle {
    Python,
    JavaScript,
    Jq,
    JsonPath,
}

impl PathStyle {
    const ALL: [PathStyle; 4] = [PathStyle::Python, PathStyle::JavaScript, PathStyle::Jq, PathStyle::JsonPath];

    fn label(self) -> &'static str {
        match self {
            PathStyle::Python => "Python",
            PathStyle::JavaScript => "JS",
            PathStyle::Jq => "jq",
            PathStyle::JsonPath => "JSONPath",
        }
    }
}

/// The style last chosen, for every JSON preview.
static STYLE: AtomicU8 = AtomicU8::new(0);

fn style() -> PathStyle {
    PathStyle::ALL[STYLE.load(Ordering::Relaxed) as usize % PathStyle::ALL.len()]
}

fn set_style(s: PathStyle) {
    STYLE.store(PathStyle::ALL.iter().position(|x| *x == s).unwrap_or(0) as u8, Ordering::Relaxed);
}

fn is_ident(k: &str, extra: &[char]) -> bool {
    let mut c = k.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_' || extra.contains(&f))
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_' || extra.contains(&x))
}

fn single_quoted(k: &str) -> String {
    format!("'{}'", k.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The path to a value, written for `style`: `j['data'][0]['name']` (Python),
/// `j.data[0].name` (JavaScript), `.data[0].name` (jq), `$.data[0].name` (JSONPath).
pub fn format_path(segs: &[Seg], style: PathStyle) -> String {
    let mut out = String::from(match style {
        PathStyle::Python | PathStyle::JavaScript => "j",
        PathStyle::Jq => "",
        PathStyle::JsonPath => "$",
    });
    for (i, s) in segs.iter().enumerate() {
        match (style, s) {
            (_, Seg::Index(n)) if style == PathStyle::Jq && i == 0 => out.push_str(&format!(".[{n}]")),
            (_, Seg::Index(n)) => out.push_str(&format!("[{n}]")),
            (PathStyle::Python, Seg::Key(k)) => out.push_str(&format!("[{}]", single_quoted(k))),
            (PathStyle::JavaScript, Seg::Key(k)) if is_ident(k, &['$']) => out.push_str(&format!(".{k}")),
            (PathStyle::JavaScript, Seg::Key(k)) => out.push_str(&format!("[{}]", serde_json::to_string(k).unwrap_or_default())),
            (PathStyle::Jq, Seg::Key(k)) if is_ident(k, &[]) => out.push_str(&format!(".{k}")),
            (PathStyle::Jq, Seg::Key(k)) => out.push_str(&format!(".[{}]", serde_json::to_string(k).unwrap_or_default())),
            (PathStyle::JsonPath, Seg::Key(k)) if is_ident(k, &[]) => out.push_str(&format!(".{k}")),
            (PathStyle::JsonPath, Seg::Key(k)) => out.push_str(&format!("[{}]", single_quoted(k))),
        }
    }
    if out.is_empty() {
        out.push('.');
    }
    out
}

// ---------------------------------------------------------------- the tree

fn node_at<'a>(root: &'a Node, at: &[u32]) -> Option<&'a Node> {
    at.iter().try_fold(root, |n, i| n.child(*i as usize))
}

/// Where a row is: child indexes from the root.
type At = Vec<u32>;

#[derive(Debug, PartialEq)]
enum RowKind {
    Value,
    /// "… n more" under a folder that shows only part of its children.
    More(usize),
}

#[derive(Debug)]
struct Row {
    depth: usize,
    at: At,
    kind: RowKind,
}

pub struct JsonView {
    root: Node,
    values: usize,
    depth: usize,
    source: Option<(Vec<Styled>, Color32)>,
    show_source: bool,
    expanded: HashSet<At>,
    /// How many children a folder shows, when not `PAGE`.
    shown: HashMap<At, usize>,
    rows: Vec<Row>,
    selected: At,
}

impl JsonView {
    pub fn new(root: Node, source: Option<(Vec<Styled>, Color32)>) -> Self {
        let (values, depth) = root.stats();
        let mut v =
            Self { root, values, depth, source, show_source: false, expanded: HashSet::new(), shown: HashMap::new(), rows: Vec::new(), selected: Vec::new() };
        v.expanded.insert(Vec::new());
        v.rebuild();
        v.open_first_levels();
        v
    }

    fn node(&self, at: &[u32]) -> Option<&Node> {
        node_at(&self.root, at)
    }

    fn segs(&self, at: &[u32]) -> Vec<Seg<'_>> {
        let mut n = &self.root;
        let mut out = Vec::new();
        for &i in at {
            let i = i as usize;
            match n {
                Node::Arr(v) => {
                    out.push(Seg::Index(i));
                    n = &v[i];
                }
                Node::Obj(v) => {
                    out.push(Seg::Key(&v[i].0));
                    n = &v[i].1;
                }
                _ => break,
            }
        }
        out
    }

    pub fn path(&self, at: &[u32], style: PathStyle) -> String {
        format_path(&self.segs(at), style)
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        self.push_rows(&self.root, Vec::new(), 0, &mut rows);
        self.rows = rows;
    }

    fn push_rows(&self, n: &Node, at: At, depth: usize, rows: &mut Vec<Row>) {
        let open = n.is_container() && self.expanded.contains(&at);
        rows.push(Row { depth, at: at.clone(), kind: RowKind::Value });
        if !open {
            return;
        }
        let limit = self.shown.get(&at).copied().unwrap_or(PAGE);
        for i in 0..n.len().min(limit) {
            let mut child = at.clone();
            child.push(i as u32);
            self.push_rows(n.child(i).unwrap(), child, depth + 1, rows);
        }
        if n.len() > limit {
            rows.push(Row { depth: depth + 1, at, kind: RowKind::More(n.len() - limit) });
        }
    }

    /// Open level after level while the view stays short.
    fn open_first_levels(&mut self) {
        for depth in 0..3 {
            let next: Vec<At> =
                self.rows.iter().filter(|r| r.kind == RowKind::Value && r.depth == depth + 1 && !self.expanded.contains(&r.at)).map(|r| r.at.clone()).collect();
            let added: usize = next.iter().filter_map(|a| self.node(a)).map(|n| n.len().min(PAGE)).sum();
            if added == 0 || self.rows.len() + added > FIRST_ROWS {
                break;
            }
            let open: Vec<At> = next.into_iter().filter(|a| self.node(a).is_some_and(Node::is_container)).collect();
            self.expanded.extend(open);
            self.rebuild();
        }
    }

    fn toggle(&mut self, at: &At) {
        if !self.expanded.remove(at) {
            self.expanded.insert(at.clone());
        }
        self.rebuild();
    }

    /// Open everything below `at`, level by level, until the view is long enough.
    fn expand_all(&mut self, at: &At) {
        let mut level = vec![at.clone()];
        let mut rows = self.rows.len();
        while !level.is_empty() && rows < MAX_EXPANDED_ROWS {
            let mut next = Vec::new();
            for a in level {
                let Some(n) = node_at(&self.root, &a) else { continue };
                if !n.is_container() {
                    continue;
                }
                if self.expanded.insert(a.clone()) {
                    rows += n.len().min(PAGE);
                }
                for i in 0..n.len().min(PAGE) {
                    let mut c = a.clone();
                    c.push(i as u32);
                    next.push(c);
                }
                if rows >= MAX_EXPANDED_ROWS {
                    break;
                }
            }
            level = next;
        }
        self.rebuild();
    }

    fn collapse_all(&mut self) {
        self.expanded.clear();
        self.expanded.insert(Vec::new());
        self.rebuild();
    }

    /// Select a row, as a click would.
    #[cfg(test)]
    pub fn select(&mut self, at: &[u32]) {
        self.selected = at.to_vec();
    }

    #[cfg(test)]
    pub fn describe(&self) -> String {
        format!("values={} depth={} rows={}", self.values, self.depth, self.rows.len())
    }

    fn facts(&self) -> String {
        let what = match &self.root {
            Node::Obj(v) => format!("{} {}", crate::util::fmt_count(v.len() as u64), if v.len() == 1 { t("key") } else { t("keys") }),
            Node::Arr(v) => format!("{} {}", crate::util::fmt_count(v.len() as u64), if v.len() == 1 { t("item") } else { t("items") }),
            _ => t("a single value"),
        };
        format!("JSON · {what} · {} {} · {} {}", crate::util::fmt_count(self.values as u64), t("values"), t("depth"), self.depth)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, name: &str, key: &str, th: &Theme) {
        let facts = self.facts();
        let mut action = None;
        crate::preview::widgets::header_row(ui, th, name, &[(facts, th.weak)], |ui| {
            if self.source.is_some() && ui.selectable_label(self.show_source, t("Source")).clicked() {
                self.show_source = !self.show_source;
            }
            if !self.show_source {
                if ui.button(t("Collapse all")).clicked() {
                    action = Some(false);
                }
                if ui.button(t("Expand all")).clicked() {
                    action = Some(true);
                }
            }
        });
        match action {
            Some(true) => self.expand_all(&Vec::new()),
            Some(false) => self.collapse_all(),
            None => {}
        }
        ui.separator();
        if let (true, Some((lines, bg))) = (self.show_source, &self.source) {
            // Its scroll position belongs to this file.
            ui.push_id(("source", key), |ui| crate::preview::code::code_view(ui, lines, *bg, false, th));
            return;
        }
        self.path_bar(ui, key, th);
        ui.add_space(2.0);
        self.tree(ui, key, th);
    }

    /// The selected row's path, in the chosen style, with a button to copy it.
    fn path_bar(&mut self, ui: &mut egui::Ui, key: &str, th: &Theme) {
        let _ = key;
        let st = style();
        let path = self.path(&self.selected, st);
        let chips = |ui: &mut egui::Ui| {
            let mut chosen = st;
            for s in PathStyle::ALL {
                ui.selectable_value(&mut chosen, s, s.label()).on_hover_text(format_path(&[Seg::Key("data"), Seg::Index(0), Seg::Key("name")], s));
            }
            if chosen != st {
                set_style(chosen);
            }
        };
        let path_label = |ui: &mut egui::Ui| {
            egui::Frame::NONE.fill(th.surface).corner_radius(4).inner_margin(egui::vec2(6.0, 2.0)).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&path).monospace().size(12.5).color(th.text)).truncate().selectable(true));
            });
        };
        // Narrow: the style chips on a line of their own, the path and its button below.
        let narrow = ui.available_width() < 460.0;
        if narrow {
            ui.horizontal(chips);
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t("Copy path")).on_hover_text(t("Copy the path of the selected row")).clicked() {
                    ui.ctx().copy_text(path.clone());
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if !narrow {
                        chips(ui);
                    }
                    path_label(ui);
                });
            });
        });
    }

    fn tree(&mut self, ui: &mut egui::Ui, key: &str, th: &Theme) {
        let colors = ValueColors::new(th);
        let font = FontId::monospace(12.5);
        let mut action: Option<Action> = None;
        egui::ScrollArea::both().id_salt(("json-tree", key)).auto_shrink([false, false]).show_rows(ui, ROW_H, self.rows.len(), |ui, range| {
            let width = ui.available_width().max(200.0);
            for i in range {
                let row = &self.rows[i];
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, ROW_H), Sense::click());
                let indent = rect.left() + 6.0 + row.depth as f32 * 16.0;
                let p = ui.painter_at(rect);
                if let RowKind::More(n) = row.kind {
                    let text = format!("… {} {}", crate::util::fmt_count(n as u64), t("hidden · click to show more"));
                    p.text(
                        egui::pos2(indent + 16.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        text,
                        font.clone(),
                        if resp.hovered() { th.accent } else { th.weak },
                    );
                    if resp.clicked() {
                        action = Some(Action::More(row.at.clone()));
                    }
                    continue;
                }
                let Some(node) = self.node(&row.at) else { continue };
                let selected = row.at == self.selected;
                if selected {
                    p.rect_filled(rect, 3.0, th.selection);
                } else if resp.hovered() {
                    p.rect_filled(rect, 3.0, th.surface);
                }
                // Chevron.
                let open = self.expanded.contains(&row.at);
                let chevron = egui::Rect::from_center_size(egui::pos2(indent + 6.0, rect.center().y), egui::vec2(12.0, 12.0));
                if node.is_container() {
                    let c = chevron.center();
                    let pts = if open {
                        vec![c + egui::vec2(-4.0, -2.0), c + egui::vec2(4.0, -2.0), c + egui::vec2(0.0, 3.0)]
                    } else {
                        vec![c + egui::vec2(-2.0, -4.0), c + egui::vec2(3.0, 0.0), c + egui::vec2(-2.0, 4.0)]
                    };
                    p.add(egui::Shape::convex_polygon(pts, th.weak, egui::Stroke::NONE));
                }
                let mut job = egui::text::LayoutJob::default();
                let fmt = |c: Color32| egui::TextFormat { font_id: font.clone(), color: c, ..Default::default() };
                match self.segs(&row.at).last() {
                    Some(Seg::Key(k)) => {
                        job.append(k, 0.0, fmt(colors.key));
                        job.append(": ", 0.0, fmt(th.weak));
                    }
                    Some(Seg::Index(n)) => {
                        job.append(&n.to_string(), 0.0, fmt(th.weak));
                        job.append(": ", 0.0, fmt(th.weak));
                    }
                    None => {}
                }
                let (text, color) = summary(node, &colors, open);
                job.append(&text, 0.0, fmt(color));
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                p.galley(egui::pos2(indent + 16.0, rect.center().y - galley.size().y / 2.0), galley, th.text);
                let at = row.at.clone();
                let on_chevron = resp.interact_pointer_pos().is_some_and(|q| q.x < chevron.right() + 2.0);
                if resp.double_clicked() || (resp.clicked() && on_chevron) {
                    action = Some(Action::Toggle(at.clone()));
                } else if resp.clicked() {
                    action = Some(Action::Select(at.clone()));
                }
                if let Node::Str(s) = node {
                    if s.chars().count() > 80 {
                        let s: String = s.chars().take(2000).collect();
                        resp.clone().on_hover_text(s);
                    }
                }
                resp.kept_context_menu(|ui| {
                    let st = style();
                    if ui.button(format!("{} ({})", t("Copy path"), st.label())).clicked() {
                        ui.ctx().copy_text(self.path(&at, st));
                        ui.close_kind(egui::UiKind::Menu);
                    }
                    if ui.button(t("Copy value")).clicked() {
                        ui.ctx().copy_text(value_text(node));
                        ui.close_kind(egui::UiKind::Menu);
                    }
                    if let Some(Seg::Key(k)) = self.segs(&at).last() {
                        if ui.button(t("Copy key")).clicked() {
                            ui.ctx().copy_text(k.to_string());
                            ui.close_kind(egui::UiKind::Menu);
                        }
                    }
                    if node.is_container() {
                        ui.separator();
                        if ui.button(t("Expand all")).clicked() {
                            action = Some(Action::ExpandAll(at.clone()));
                            ui.close_kind(egui::UiKind::Menu);
                        }
                    }
                });
                if resp.secondary_clicked() {
                    action = Some(Action::Select(at));
                }
            }
        });
        match action {
            Some(Action::Toggle(at)) => {
                self.selected = at.clone();
                self.toggle(&at);
            }
            Some(Action::Select(at)) => self.selected = at,
            Some(Action::ExpandAll(at)) => self.expand_all(&at),
            Some(Action::More(at)) => {
                let n = self.shown.get(&at).copied().unwrap_or(PAGE) + PAGE;
                self.shown.insert(at, n);
                self.rebuild();
            }
            None => {}
        }
    }
}

enum Action {
    Toggle(At),
    Select(At),
    ExpandAll(At),
    More(At),
}

struct ValueColors {
    key: Color32,
    string: Color32,
    number: Color32,
    literal: Color32,
    weak: Color32,
}

impl ValueColors {
    fn new(th: &Theme) -> Self {
        let pick = |light: (u8, u8, u8), dark: (u8, u8, u8)| {
            let (r, g, b) = if th.dark { dark } else { light };
            Color32::from_rgb(r, g, b)
        };
        Self {
            key: th.accent,
            string: pick((0x3a, 0x8a, 0x3e), (0x98, 0xc3, 0x79)),
            number: pick((0xb0, 0x5a, 0x00), (0xd1, 0x9a, 0x66)),
            literal: pick((0x9b, 0x3f, 0xb5), (0xc6, 0x78, 0xdd)),
            weak: th.weak,
        }
    }
}

/// What a row shows of its value: scalars in full (long strings cut), folders by size.
fn summary(n: &Node, c: &ValueColors, open: bool) -> (String, Color32) {
    let count = |len: usize, one: &str, many: &str| format!("{} {}", crate::util::fmt_count(len as u64), if len == 1 { t(one) } else { t(many) });
    match n {
        Node::Null => ("null".into(), c.literal),
        Node::Bool(b) => (b.to_string(), c.literal),
        Node::Num(s) => (s.clone(), c.number),
        Node::Str(s) => {
            let mut short: String = s.chars().take(300).collect();
            if short.len() < s.len() {
                short.push('…');
            }
            (serde_json::to_string(&short).unwrap_or_default(), c.string)
        }
        Node::Arr(v) if open => (format!("[ ] {}", count(v.len(), "item", "items")), c.weak),
        Node::Obj(v) if open => (format!("{{ }} {}", count(v.len(), "key", "keys")), c.weak),
        Node::Arr(v) => (format!("[…] {}", count(v.len(), "item", "items")), c.weak),
        Node::Obj(v) => (format!("{{…}} {}", count(v.len(), "key", "keys")), c.weak),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"zeta": 1, "data": [{"name": "Huế", "id": 12345678901234567890, "tags": ["a", "b"]}, {"name": null, "ok": true, "x": 1.5e300}], "it's": {"with space": {}}, "alpha": []}"#;

    fn view() -> JsonView {
        JsonView::new(parse(DOC.as_bytes()).unwrap(), None)
    }

    #[test]
    fn keys_keep_their_order_and_big_integers_stay_exact() {
        let n = parse(DOC.as_bytes()).unwrap();
        let Node::Obj(v) = &n else { panic!() };
        let keys: Vec<&str> = v.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["zeta", "data", "it's", "alpha"]);
        let v = view();
        assert_eq!(v.node(&[1, 0, 1]), Some(&Node::Num("12345678901234567890".into())), "big integers stay exact");
        assert_eq!(v.node(&[1, 1, 2]), Some(&Node::Num("1.5e+300".into())));
        assert_eq!((v.values, v.depth), (16, 4));
        assert!(parse(b"\xef\xbb\xbf[1]").is_ok(), "a byte order mark is skipped");
        assert!(parse(b"{\"a\": 1,}").is_err());
        assert!(parse(b"[1] [2]").is_err());
        let deep = format!("{}1{}", "[".repeat(300), "]".repeat(300));
        assert!(parse(deep.as_bytes()).is_err(), "too deep: refused, not a crash");
    }

    #[test]
    fn paths_in_every_style() {
        let v = view();
        let at = [1, 0, 0];
        assert_eq!(v.path(&at, PathStyle::Python), "j['data'][0]['name']");
        assert_eq!(v.path(&at, PathStyle::JavaScript), "j.data[0].name");
        assert_eq!(v.path(&at, PathStyle::Jq), ".data[0].name");
        assert_eq!(v.path(&at, PathStyle::JsonPath), "$.data[0].name");
        let odd = [2, 0];
        assert_eq!(v.path(&odd, PathStyle::Python), r"j['it\'s']['with space']");
        assert_eq!(v.path(&odd, PathStyle::JavaScript), r#"j["it's"]["with space"]"#);
        assert_eq!(v.path(&odd, PathStyle::Jq), r#".["it's"].["with space"]"#);
        assert_eq!(v.path(&odd, PathStyle::JsonPath), r"$['it\'s']['with space']");
        assert_eq!(v.path(&[], PathStyle::Python), "j");
        assert_eq!(v.path(&[], PathStyle::Jq), ".");
        assert_eq!(v.path(&[], PathStyle::JsonPath), "$");
        let arr = JsonView::new(parse(b"[[1, {\"$id\": 2}]]").unwrap(), None);
        assert_eq!(arr.path(&[0, 1, 0], PathStyle::Jq), ".[0][1].[\"$id\"]");
        assert_eq!(arr.path(&[0, 1, 0], PathStyle::JavaScript), "j[0][1].$id");
    }

    #[test]
    fn values_to_copy() {
        let v = view();
        assert_eq!(value_text(v.node(&[1, 0, 0]).unwrap()), "Huế", "strings without quotes");
        assert_eq!(value_text(v.node(&[1, 0, 2]).unwrap()), "[\n  \"a\",\n  \"b\"\n]");
        assert_eq!(value_text(v.node(&[2]).unwrap()), "{\n  \"with space\": {}\n}");
        assert_eq!(parse(value_text(&v.root).as_bytes()).unwrap(), v.root, "copied JSON reads back the same");
    }

    #[test]
    fn opening_and_closing() {
        let mut v = view();
        // Small: opened down to the leaves at first.
        assert_eq!(v.rows.len(), 1 + 4 + 2 + 3 + 3 + 1 + 2, "{:?}", v.rows);
        v.collapse_all();
        assert_eq!(v.rows.len(), 5);
        v.toggle(&vec![1]);
        assert_eq!(v.rows.len(), 7);
        v.toggle(&vec![1]);
        assert_eq!(v.rows.len(), 5);
        v.expand_all(&vec![1]);
        assert_eq!(v.rows.len(), 5 + 2 + 3 + 2 + 3);
        // Big arrays show a page at a time.
        let big = format!("[{}]", (0..2500).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
        let mut v = JsonView::new(parse(big.as_bytes()).unwrap(), None);
        assert_eq!(v.rows.len(), 1 + PAGE + 1);
        assert_eq!(v.rows.last().unwrap().kind, RowKind::More(1500));
        v.shown.insert(Vec::new(), 2 * PAGE);
        v.rebuild();
        assert_eq!(v.rows.last().unwrap().kind, RowKind::More(500));
        assert!(is_json("a.JSON") && is_json("x.har") && !is_json("a.jsonl") && !is_json("a.geojson"));
    }
}
