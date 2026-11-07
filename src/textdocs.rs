//! Readable previews of RTF, EPUB and ODF text documents: text and its structure
//! (headings, lists, tables, emphasis), not page layout, shown by the Markdown view.
use crate::preview::{clip_block_text, plain_code_block, MdBlock, Span, MAX_BLOCK_TEXT};
use crate::util::{ext_of, Res};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::{Read, Seek};

/// Long documents are shown in parts, as the view lays out everything it shows.
const BLOCKS_PER_PART: usize = 600;
/// And laid out text takes about 200 bytes per character, kept while it is on screen.
const TEXT_PER_PART: usize = 512 * 1024;
/// Most text read from one file in an EPUB or ODF package.
const MAX_XML: u64 = 32 * 1024 * 1024;
const MAX_TOTAL: u64 = 96 * 1024 * 1024;

pub struct Section {
    pub title: String,
    pub blocks: Vec<MdBlock>,
}

pub struct Document {
    pub format: &'static str,
    pub title: Option<String>,
    pub author: Option<String>,
    pub sections: Vec<Section>,
}

pub fn is_document(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "rtf" | "epub" | "odt")
}

/// EPUB and ODF files are ZIP packages, which are read in place: their text is what
/// matters, however large the pictures next to it make the file.
pub fn reads_zip(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "epub" | "odt")
}

pub fn load(name: &str, bytes: Vec<u8>, syntax_theme: &str) -> Res<Document> {
    let doc = match ext_of(name).as_str() {
        "rtf" => Document { format: "RTF", title: None, author: None, sections: whole_document(parse_rtf(&bytes)) },
        "epub" => parse_epub(std::io::Cursor::new(bytes), syntax_theme)?,
        _ => parse_odt(std::io::Cursor::new(bytes))?,
    };
    Ok(finish(doc))
}

/// A document from a file on disk (see `reads_zip`).
pub fn load_path(name: &str, path: &std::path::Path, syntax_theme: &str) -> Res<Document> {
    let file = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let doc = if ext_of(name) == "epub" { parse_epub(file, syntax_theme)? } else { parse_odt(file)? };
    Ok(finish(doc))
}

fn finish(doc: Document) -> Document {
    Document { sections: into_parts(doc.sections), ..doc }
}

fn block_text_len(b: &MdBlock) -> usize {
    let spans = |s: &[Span]| s.iter().map(|x| x.text.len()).sum::<usize>();
    match b {
        MdBlock::Heading(_, s) | MdBlock::Para(s) | MdBlock::Quote(s) | MdBlock::Item { spans: s, .. } => spans(s),
        MdBlock::Table { rows, .. } => rows.iter().flatten().map(|c| spans(c)).sum(),
        MdBlock::Code { lines, .. } => lines.iter().flatten().map(|(_, t, _)| t.len()).sum(),
        MdBlock::Rule => 0,
    }
}

/// A very long paragraph becomes several, so that all of its text can still be read.
fn split_long_paragraphs(blocks: Vec<MdBlock>) -> Vec<MdBlock> {
    let mut out = Vec::with_capacity(blocks.len());
    for b in blocks {
        let spans = match b {
            MdBlock::Para(spans) if spans.iter().map(|s| s.text.len()).sum::<usize>() > MAX_BLOCK_TEXT => spans,
            other => {
                out.push(other);
                continue;
            }
        };
        let (mut current, mut used): (Vec<Span>, usize) = (vec![], 0);
        for span in spans {
            let mut rest = span.text.as_str();
            while !rest.is_empty() {
                let room = MAX_BLOCK_TEXT - used;
                let mut end = rest.len().min(room);
                while !rest.is_char_boundary(end) {
                    end -= 1;
                }
                // Between words, when there is one nearby.
                if end < rest.len() {
                    if let Some(space) = rest[..end].rfind(' ').filter(|i| *i > end / 2) {
                        end = space + 1;
                    }
                }
                current.push(Span { text: rest[..end].to_string(), ..span.clone() });
                used += end;
                rest = &rest[end..];
                if !rest.is_empty() || used >= MAX_BLOCK_TEXT {
                    out.push(MdBlock::Para(std::mem::take(&mut current)));
                    used = 0;
                }
            }
        }
        if !current.is_empty() {
            out.push(MdBlock::Para(current));
        }
    }
    clip_block_text(&mut out);
    out
}

fn whole_document(blocks: Vec<MdBlock>) -> Vec<Section> {
    vec![Section { title: String::new(), blocks }]
}

/// Split sections too long to show at once, keeping them in order.
fn into_parts(sections: Vec<Section>) -> Vec<Section> {
    let mut out = vec![];
    for s in sections {
        let blocks = split_long_paragraphs(s.blocks);
        let mut parts: Vec<Vec<MdBlock>> = vec![];
        let (mut part, mut text) = (vec![], 0);
        for b in blocks {
            let len = block_text_len(&b);
            if !part.is_empty() && (part.len() >= BLOCKS_PER_PART || text + len > TEXT_PER_PART) {
                parts.push(std::mem::take(&mut part));
                text = 0;
            }
            text += len;
            part.push(b);
        }
        parts.push(part);
        let count = parts.len();
        for (i, blocks) in parts.into_iter().enumerate() {
            let of = format!("{}/{count}", i + 1);
            let title = match (count, s.title.is_empty()) {
                (1, _) => s.title.clone(),
                (_, true) => of,
                _ => format!("{} ({of})", s.title),
            };
            out.push(Section { title, blocks });
        }
    }
    out
}

// ---------------------------------------------------------------- building blocks

#[derive(Clone, Copy, Default, PartialEq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
    link: bool,
}

/// Collects the text of one paragraph-like thing at a time.
#[derive(Default)]
struct Builder {
    out: Vec<MdBlock>,
    spans: Vec<Span>,
    style: Style,
    /// Collapse runs of white space, as HTML does.
    collapse: bool,
}

impl Builder {
    fn text(&mut self, text: &str) {
        let mut cleaned = String::with_capacity(text.len());
        if self.collapse {
            let mut after_space = self.spans.last().is_none_or(|s| s.text.ends_with([' ', '\n']));
            for c in text.chars() {
                if c.is_whitespace() && c != '\u{A0}' {
                    if !after_space {
                        cleaned.push(' ');
                    }
                    after_space = true;
                } else {
                    cleaned.push(c);
                    after_space = false;
                }
            }
        } else {
            cleaned.push_str(text);
        }
        if cleaned.is_empty() {
            return;
        }
        let st = self.style;
        match self.spans.last_mut() {
            Some(l) if l.bold == st.bold && l.italic == st.italic && l.code == st.code && l.strike == st.strike && l.link == st.link => l.text.push_str(&cleaned),
            _ => self.spans.push(Span { text: cleaned, bold: st.bold, italic: st.italic, code: st.code, strike: st.strike, link: st.link }),
        }
    }

    fn line_break(&mut self) {
        if let Some(l) = self.spans.last_mut() {
            while l.text.ends_with(' ') {
                l.text.pop();
            }
            l.text.push('\n');
        }
    }

    /// The text collected so far, or nothing if it is blank.
    fn take(&mut self) -> Option<Vec<Span>> {
        let mut spans = std::mem::take(&mut self.spans);
        if let Some(l) = spans.last_mut() {
            l.text.truncate(l.text.trim_end().len());
        }
        spans.retain(|s| !s.text.is_empty());
        (!spans.is_empty()).then_some(spans)
    }

    fn para(&mut self) {
        if let Some(spans) = self.take() {
            self.out.push(MdBlock::Para(spans));
        }
    }
}

fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

fn new_reader(xml: &str) -> Reader<&[u8]> {
    let mut r = Reader::from_str(xml);
    // Real-world books are not always well-formed.
    let c = r.config_mut();
    c.check_end_names = false;
    c.allow_unmatched_ends = true;
    r
}

/// The local part of an element name, in lower case: `dc:Title` gives `title`.
fn local(name: &[u8]) -> String {
    let n = String::from_utf8_lossy(name);
    n.rsplit(':').next().unwrap_or_default().to_lowercase()
}

fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.as_ref() == key.as_bytes() || local(a.key.as_ref()) == key).map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

/// `&amp;`, `&#160;` and the named characters common in EPUBs.
fn reference(name: &str) -> String {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => num.parse().ok(),
        };
        return code.and_then(char::from_u32).map(String::from).unwrap_or_default();
    }
    match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{A0}",
        "mdash" => "—",
        "ndash" => "–",
        "hellip" => "…",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "bull" => "•",
        "middot" => "·",
        "laquo" => "«",
        "raquo" => "»",
        "shy" => "",
        _ => "",
    }
    .to_string()
}

fn zip_text<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str, budget: &mut u64) -> Res<String> {
    let f = zip.by_name(name).map_err(|e| format!("{name}: {e}"))?;
    let mut bytes = Vec::new();
    let limit = MAX_XML.min(*budget);
    f.take(limit).read_to_end(&mut bytes).map_err(|e| format!("{name}: {e}"))?;
    *budget -= bytes.len() as u64;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ---------------------------------------------------------------- XHTML (EPUB)

struct ListLevel {
    ordered: bool,
    next: usize,
}

fn xhtml_blocks(xml: &str, syntax_theme: &str) -> Vec<MdBlock> {
    let mut r = new_reader(xml);
    let mut b = Builder { collapse: true, ..Builder::default() };
    let mut skip = 0usize;
    let mut quote = 0usize;
    let mut lists: Vec<ListLevel> = vec![];
    let mut item: Option<(usize, String)> = None;
    let mut pre: Option<String> = None;
    let mut table: Option<(Vec<Vec<Vec<Span>>>, bool)> = None;
    let mut row: Vec<Vec<Span>> = vec![];
    let flush = |b: &mut Builder, quote: usize, item: &mut Option<(usize, String)>| {
        let Some(spans) = b.take() else { return };
        b.out.push(match item.take() {
            Some((depth, marker)) => MdBlock::Item { depth, marker, spans },
            None if quote > 0 => MdBlock::Quote(spans),
            None => MdBlock::Para(spans),
        });
    };
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                let tag = local(e.name().as_ref());
                if skip > 0 || matches!(tag.as_str(), "head" | "script" | "style" | "svg" | "nav") {
                    skip += 1;
                    continue;
                }
                match tag.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" | "section" | "article" | "aside" | "header" | "footer" | "figure" | "figcaption" | "dl" | "dt" | "dd" if table.is_none() => flush(&mut b, quote, &mut item),
                    "blockquote" => {
                        flush(&mut b, quote, &mut item);
                        quote += 1;
                    }
                    "ul" | "ol" => {
                        flush(&mut b, quote, &mut item);
                        lists.push(ListLevel { ordered: tag == "ol", next: 1 });
                    }
                    "li" => {
                        flush(&mut b, quote, &mut item);
                        let depth = lists.len().saturating_sub(1);
                        let marker = match lists.last_mut() {
                            Some(l) if l.ordered => {
                                l.next += 1;
                                format!("{}.", l.next - 1)
                            }
                            _ => "•".to_string(),
                        };
                        item = Some((depth, marker));
                    }
                    "pre" => {
                        flush(&mut b, quote, &mut item);
                        pre = Some(String::new());
                    }
                    "table" => {
                        flush(&mut b, quote, &mut item);
                        table = Some((vec![], false));
                    }
                    "tr" => row = vec![],
                    "th" | "td" => {
                        if tag == "th" {
                            if let Some(t) = table.as_mut().filter(|t| t.0.is_empty()) {
                                t.1 = true;
                            }
                        }
                        b.spans.clear();
                    }
                    "b" | "strong" => b.style.bold = true,
                    "i" | "em" | "cite" => b.style.italic = true,
                    "code" | "kbd" | "tt" | "samp" => b.style.code = true,
                    "s" | "del" | "strike" => b.style.strike = true,
                    "a" => b.style.link = attr(&e, "href").is_some(),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let tag = local(e.name().as_ref());
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                match tag.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        if let Some(spans) = b.take() {
                            b.out.push(MdBlock::Heading(tag.as_bytes()[1] - b'0', spans));
                        }
                    }
                    "p" | "div" | "section" | "article" | "aside" | "header" | "footer" | "figure" | "figcaption" | "dl" | "dt" | "dd" | "li" if table.is_none() => flush(&mut b, quote, &mut item),
                    "blockquote" => {
                        flush(&mut b, quote, &mut item);
                        quote = quote.saturating_sub(1);
                    }
                    "ul" | "ol" => {
                        flush(&mut b, quote, &mut item);
                        lists.pop();
                    }
                    "pre" => {
                        if let Some(code) = pre.take() {
                            b.out.push(plain_code_block(code.trim_end_matches('\n'), syntax_theme));
                        }
                    }
                    "th" | "td" => row.push(b.take().unwrap_or_default()),
                    "tr" => {
                        if let Some(t) = table.as_mut() {
                            t.0.push(std::mem::take(&mut row));
                        }
                    }
                    "table" => {
                        if let Some((rows, has_header)) = table.take() {
                            if !rows.is_empty() {
                                b.out.push(MdBlock::Table { rows, has_header });
                            }
                        }
                    }
                    "b" | "strong" => b.style.bold = false,
                    "i" | "em" | "cite" => b.style.italic = false,
                    "code" | "kbd" | "tt" | "samp" => b.style.code = false,
                    "s" | "del" | "strike" => b.style.strike = false,
                    "a" => b.style.link = false,
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) if skip == 0 => match local(e.name().as_ref()).as_str() {
                "br" => match pre.as_mut() {
                    Some(code) => code.push('\n'),
                    None => b.line_break(),
                },
                "hr" => {
                    flush(&mut b, quote, &mut item);
                    b.out.push(MdBlock::Rule);
                }
                _ => {}
            },
            Ok(Event::Text(e)) if skip == 0 => {
                let text = String::from_utf8_lossy(e.as_ref()).into_owned();
                match pre.as_mut() {
                    Some(code) => code.push_str(&text),
                    None => b.text(&text),
                }
            }
            Ok(Event::CData(e)) if skip == 0 => b.text(&String::from_utf8_lossy(e.as_ref())),
            Ok(Event::GeneralRef(e)) if skip == 0 => {
                let text = reference(&String::from_utf8_lossy(e.as_ref()));
                match pre.as_mut() {
                    Some(code) => code.push_str(&text),
                    None => b.text(&text),
                }
            }
            Ok(Event::Eof) => break,
            // What was read so far is still worth showing.
            Err(_) => break,
            _ => {}
        }
    }
    flush(&mut b, quote, &mut item);
    b.out
}

// ---------------------------------------------------------------- EPUB

/// Whether an EPUB's `encryption.xml` protects its text. Only the two font obfuscation
/// schemes are harmless: they are common in ordinary, readable books.
fn epub_is_protected(encryption_xml: &str) -> bool {
    let mut r = new_reader(encryption_xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if local(e.name().as_ref()) == "encryptionmethod" => {
                let algorithm = attr(&e, "Algorithm").unwrap_or_default();
                if !matches!(algorithm.as_str(), "http://www.idpf.org/2008/embedding" | "http://ns.adobe.com/pdf/enc#RC") {
                    return true;
                }
            }
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}

/// The text of the first element with this local name.
fn first_text(xml: &str, wanted: &str) -> Option<String> {
    let mut r = new_reader(xml);
    let mut inside = false;
    let mut out = String::new();
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) if local(e.name().as_ref()) == wanted => inside = true,
            Ok(Event::Text(e)) if inside => out.push_str(&String::from_utf8_lossy(e.as_ref())),
            Ok(Event::GeneralRef(e)) if inside => out.push_str(&reference(&String::from_utf8_lossy(e.as_ref()))),
            Ok(Event::End(e)) if inside && local(e.name().as_ref()) == wanted => break,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%').then(|| s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        if let Some(v) = hex {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `href` relative to the folder of `base`, without `.` and `..`.
fn join_zip_path(base: &str, href: &str) -> String {
    let mut parts: Vec<&str> = base.rsplit_once('/').map_or(vec![], |(d, _)| d.split('/').collect());
    for p in href.split('/') {
        match p {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn parse_epub<R: Read + Seek>(reader: R, syntax_theme: &str) -> Res<Document> {
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| format!("EPUB: {e}"))?;
    let mut budget = MAX_TOTAL;
    if let Ok(encryption) = zip_text(&mut zip, "META-INF/encryption.xml", &mut budget) {
        if epub_is_protected(&encryption) {
            return Err(crate::preview::LOCKED.to_string());
        }
    }
    let container = zip_text(&mut zip, "META-INF/container.xml", &mut budget)?;
    let mut opf_path = None;
    let mut r = new_reader(&container);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if local(e.name().as_ref()) == "rootfile" => {
                opf_path = attr(&e, "full-path");
                break;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let opf_path = opf_path.ok_or("EPUB: no book in its container")?;
    let opf = zip_text(&mut zip, &opf_path, &mut budget)?;
    let mut manifest: HashMap<String, String> = HashMap::new();
    let mut spine: Vec<String> = vec![];
    let mut r = new_reader(&opf);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => match local(e.name().as_ref()).as_str() {
                "item" => {
                    if let (Some(id), Some(href)) = (attr(&e, "id"), attr(&e, "href")) {
                        manifest.insert(id, href);
                    }
                }
                "itemref" => spine.extend(attr(&e, "idref")),
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let mut sections = vec![];
    for id in &spine {
        let Some(href) = manifest.get(id) else { continue };
        if budget == 0 {
            break;
        }
        let path = join_zip_path(&opf_path, &percent_decode(href));
        let Ok(xml) = zip_text(&mut zip, &path, &mut budget) else { continue };
        let blocks = xhtml_blocks(&xml, syntax_theme);
        if blocks.is_empty() {
            continue;
        }
        let heading = blocks.iter().find_map(|b| match b {
            MdBlock::Heading(_, spans) => Some(spans_text(spans)),
            _ => None,
        });
        let title = heading.unwrap_or_else(|| format!("{} {}", crate::i18n::t("Section"), sections.len() + 1));
        sections.push(Section { title, blocks });
    }
    if sections.is_empty() {
        return Err("EPUB: no readable text found".into());
    }
    Ok(Document { format: "EPUB", title: first_text(&opf, "title"), author: first_text(&opf, "creator"), sections })
}

// ---------------------------------------------------------------- ODF text

/// The automatic styles of an ODF document that make text bold or italic.
fn odf_text_styles(xml: &str) -> HashMap<String, (bool, bool)> {
    let mut r = new_reader(xml);
    let mut out = HashMap::new();
    let mut current: Option<String> = None;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == b"style:style" => current = attr(&e, "style:name"),
            Ok(Event::End(e)) if e.name().as_ref() == b"style:style" => current = None,
            Ok(Event::Start(e) | Event::Empty(e)) if e.name().as_ref() == b"style:text-properties" => {
                if let Some(name) = &current {
                    let bold = attr(&e, "fo:font-weight").is_some_and(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
                    let italic = attr(&e, "fo:font-style").as_deref() == Some("italic");
                    out.insert(name.clone(), (bold, italic));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn parse_odt<R: Read + Seek>(reader: R) -> Res<Document> {
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| format!("ODT: {e}"))?;
    let mut budget = MAX_TOTAL;
    // An encrypted document says so in its manifest; its content.xml is then not XML at all.
    if zip_text(&mut zip, "META-INF/manifest.xml", &mut budget).is_ok_and(|m| m.contains("encryption-data")) {
        return Err(crate::preview::LOCKED.to_string());
    }
    let content = zip_text(&mut zip, "content.xml", &mut budget)?;
    let meta = zip_text(&mut zip, "meta.xml", &mut budget).unwrap_or_default();
    let styles = odf_text_styles(&content);
    let mut r = new_reader(&content);
    let mut b = Builder::default();
    let (mut in_body, mut list_depth) = (false, 0usize);
    let mut item_first = false;
    let mut style_stack: Vec<Style> = vec![];
    let mut table: Option<Vec<Vec<Vec<Span>>>> = None;
    let mut row: Vec<Vec<Span>> = vec![];
    let mut cell: Option<Vec<Span>> = None;
    let mut skip = 0usize;
    let style_of = |e: &quick_xml::events::BytesStart, base: Style| {
        let (bold, italic) = attr(e, "text:style-name").and_then(|n| styles.get(&n).copied()).unwrap_or_default();
        Style { bold: base.bold || bold, italic: base.italic || italic, ..base }
    };
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let name = name.as_ref();
                if name == b"office:text" {
                    in_body = true;
                    continue;
                }
                if !in_body {
                    continue;
                }
                if skip > 0 || matches!(name, b"draw:frame" | b"text:note" | b"office:annotation" | b"text:tracked-changes") {
                    skip += 1;
                    continue;
                }
                match name {
                    b"text:list" => list_depth += 1,
                    b"text:list-item" => item_first = true,
                    b"text:p" | b"text:h" => {
                        if cell.is_none() {
                            b.spans.clear();
                        } else if !b.spans.is_empty() {
                            b.line_break();
                        }
                        b.style = style_of(&e, Style::default());
                    }
                    b"text:span" => {
                        style_stack.push(b.style);
                        b.style = style_of(&e, b.style);
                    }
                    b"text:a" => {
                        style_stack.push(b.style);
                        b.style.link = true;
                    }
                    b"table:table" => table = Some(vec![]),
                    b"table:table-row" => row = vec![],
                    b"table:table-cell" => {
                        cell = Some(vec![]);
                        b.spans.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                let name = name.as_ref();
                if name == b"office:text" {
                    break;
                }
                if !in_body {
                    continue;
                }
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                match name {
                    b"text:list" => list_depth = list_depth.saturating_sub(1),
                    b"text:h" => {
                        if let Some(spans) = b.take() {
                            b.out.push(MdBlock::Heading(1, spans));
                        }
                        b.style = Style::default();
                    }
                    b"text:p" if cell.is_none() => {
                        if let Some(spans) = b.take() {
                            let marker = if std::mem::take(&mut item_first) { "•" } else { " " };
                            b.out.push(if list_depth > 0 {
                                MdBlock::Item { depth: list_depth - 1, marker: marker.into(), spans }
                            } else {
                                MdBlock::Para(spans)
                            });
                        }
                        b.style = Style::default();
                    }
                    b"text:span" | b"text:a" => b.style = style_stack.pop().unwrap_or_default(),
                    b"table:table-cell" => {
                        cell = None;
                        row.push(b.take().unwrap_or_default());
                    }
                    b"table:table-row" => {
                        if let Some(t) = table.as_mut() {
                            t.push(std::mem::take(&mut row));
                        }
                    }
                    b"table:table" => {
                        if let Some(rows) = table.take().filter(|r| !r.is_empty()) {
                            b.out.push(MdBlock::Table { rows, has_header: false });
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) if in_body && skip == 0 => match e.name().as_ref() {
                b"text:s" => {
                    let n = attr(&e, "text:c").and_then(|c| c.parse::<usize>().ok()).unwrap_or(1).min(256);
                    b.text(&" ".repeat(n));
                }
                b"text:tab" => b.text("\t"),
                b"text:line-break" => b.line_break(),
                _ => {}
            },
            Ok(Event::Text(e)) if in_body && skip == 0 => b.text(&String::from_utf8_lossy(e.as_ref())),
            Ok(Event::GeneralRef(e)) if in_body && skip == 0 => b.text(&reference(&String::from_utf8_lossy(e.as_ref()))),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let author = first_text(&meta, "creator").or_else(|| first_text(&meta, "initial-creator"));
    Ok(Document { format: "ODF", title: first_text(&meta, "title"), author, sections: whole_document(b.out) })
}

// ---------------------------------------------------------------- RTF

#[derive(Clone, Copy)]
struct RtfGroup {
    bold: bool,
    italic: bool,
    skip: bool,
    /// How many characters follow a `\u` escape as its fallback.
    unicode_skip: u32,
}

/// Destinations that hold no text of the document.
fn rtf_ignored(word: &str) -> bool {
    matches!(
        word,
        "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "object" | "header" | "headerl" | "headerr" | "headerf" | "footer" | "footerl" | "footerr" | "footerf" | "themedata"
            | "colorschememapping" | "latentstyles" | "datastore" | "generator" | "listtable" | "listoverridetable" | "rsidtbl" | "mmathPr" | "xmlnstbl" | "pgptbl" | "wgrffmtfilter" | "filetbl"
    )
}

fn parse_rtf(bytes: &[u8]) -> Vec<MdBlock> {
    let mut b = Builder::default();
    let mut stack: Vec<RtfGroup> = vec![];
    let mut g = RtfGroup { bold: false, italic: false, skip: false, unicode_skip: 1 };
    let mut encoding = encoding_rs::WINDOWS_1252;
    let mut pending_bytes: Vec<u8> = vec![];
    let mut skip_chars = 0u32;
    let mut i = 0;
    // Text arrives byte by byte: multi-byte code pages are decoded together.
    fn flush(b: &mut Builder, pending: &mut Vec<u8>, encoding: &'static encoding_rs::Encoding, g: &RtfGroup) {
        if !pending.is_empty() {
            let (text, _, _) = encoding.decode(pending);
            b.style = Style { bold: g.bold, italic: g.italic, ..Style::default() };
            b.text(&text);
            pending.clear();
        }
    }
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'{' => {
                flush(&mut b, &mut pending_bytes, encoding, &g);
                stack.push(g);
                i += 1;
            }
            b'}' => {
                flush(&mut b, &mut pending_bytes, encoding, &g);
                g = stack.pop().unwrap_or(g);
                i += 1;
            }
            b'\\' => {
                let Some(&next) = bytes.get(i + 1) else { break };
                if next.is_ascii_alphabetic() {
                    let start = i + 1;
                    let mut j = start;
                    while bytes.get(j).is_some_and(u8::is_ascii_alphabetic) {
                        j += 1;
                    }
                    let word = std::str::from_utf8(&bytes[start..j]).unwrap_or_default();
                    let num_start = j;
                    if bytes.get(j) == Some(&b'-') {
                        j += 1;
                    }
                    while bytes.get(j).is_some_and(u8::is_ascii_digit) {
                        j += 1;
                    }
                    let param: Option<i32> = std::str::from_utf8(&bytes[num_start..j]).ok().and_then(|n| n.parse().ok());
                    if bytes.get(j) == Some(&b' ') {
                        j += 1;
                    }
                    i = j;
                    if rtf_ignored(word) {
                        g.skip = true;
                        continue;
                    }
                    if word == "ansicpg" {
                        if let Some(enc) = param.and_then(|p| encoding_rs::Encoding::for_label(format!("windows-{p}").as_bytes())) {
                            encoding = enc;
                        }
                        continue;
                    }
                    if word == "mac" {
                        encoding = encoding_rs::MACINTOSH;
                        continue;
                    }
                    if word == "uc" {
                        g.unicode_skip = param.unwrap_or(1).max(0) as u32;
                        continue;
                    }
                    if g.skip {
                        continue;
                    }
                    flush(&mut b, &mut pending_bytes, encoding, &g);
                    match word {
                        "par" | "sect" | "page" | "row" => b.para(),
                        "line" => b.line_break(),
                        "tab" => b.text("\t"),
                        "cell" => b.text("  |  "),
                        "pard" | "plain" => {
                            if word == "plain" {
                                g.bold = false;
                                g.italic = false;
                            }
                        }
                        "b" => g.bold = param != Some(0),
                        "i" => g.italic = param != Some(0),
                        "bullet" => b.text("•"),
                        "emdash" => b.text("—"),
                        "endash" => b.text("–"),
                        "lquote" => b.text("‘"),
                        "rquote" => b.text("’"),
                        "ldblquote" => b.text("“"),
                        "rdblquote" => b.text("”"),
                        "u" => {
                            if let Some(p) = param {
                                let code = if p < 0 { (p + 65536) as u32 } else { p as u32 };
                                b.style = Style { bold: g.bold, italic: g.italic, ..Style::default() };
                                b.text(&char::from_u32(code).map(String::from).unwrap_or_default());
                                skip_chars = g.unicode_skip;
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                i += 2;
                match next {
                    // `\*` marks an ignorable destination this reader does not know.
                    b'*' => g.skip = true,
                    b'\'' => {
                        let value = bytes.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                        i += 2;
                        if skip_chars > 0 {
                            skip_chars -= 1;
                        } else if let (Some(v), false) = (value, g.skip) {
                            pending_bytes.push(v);
                        }
                    }
                    b'~' if !g.skip => pending_bytes.push(0xA0),
                    b'_' if !g.skip => pending_bytes.push(b'-'),
                    b'\\' | b'{' | b'}' if !g.skip => pending_bytes.push(next),
                    b'\n' | b'\r' if !g.skip => {
                        flush(&mut b, &mut pending_bytes, encoding, &g);
                        b.para();
                    }
                    _ => {}
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                if skip_chars > 0 {
                    skip_chars -= 1;
                } else if !g.skip {
                    pending_bytes.push(c);
                }
                i += 1;
            }
        }
    }
    flush(&mut b, &mut pending_bytes, encoding, &g);
    b.para();
    b.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn plain(blocks: &[MdBlock]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| match b {
                MdBlock::Heading(l, s) => format!("h{l}:{}", spans_text(s)),
                MdBlock::Para(s) => format!("p:{}", spans_text(s)),
                MdBlock::Quote(s) => format!("q:{}", spans_text(s)),
                MdBlock::Item { depth, marker, spans } => format!("{depth}{marker}{}", spans_text(spans)),
                MdBlock::Code { .. } => "code".into(),
                MdBlock::Table { rows, .. } => format!("table:{}", rows.iter().map(|r| r.iter().map(|c| spans_text(c)).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join(";")),
                MdBlock::Rule => "rule".into(),
            })
            .collect()
    }

    #[test]
    fn rtf_text_styles_and_escapes() {
        let rtf = br"{\rtf1\ansi\ansicpg1252\uc1{\fonttbl{\f0 Helvetica;}}{\colortbl;\red0;}{\*\generator Foo;}
\f0 Plain \b bold\b0  caf\'e9\par Ti\u7871?ng \{x\}\line next\par}";
        let blocks = parse_rtf(rtf);
        assert_eq!(plain(&blocks), ["p:Plain bold café", "p:Tiếng {x}\nnext"]);
        let MdBlock::Para(spans) = &blocks[0] else { panic!() };
        assert!(spans.iter().any(|s| s.bold && s.text == "bold"), "bold run");
        assert!(spans.iter().any(|s| !s.bold && s.text.contains("Plain")));
    }

    #[test]
    fn rtf_ignores_pictures_and_survives_garbage() {
        assert_eq!(plain(&parse_rtf(br"{\rtf1 a{\pict\pngblip 89504e47}b\par}")), ["p:ab"]);
        for junk in [&b""[..], b"\\", b"{{{{", b"}}}}", br"\'", br"\u", br"{\rtf1\uc99\u1 x}"] {
            let _ = parse_rtf(junk);
        }
    }

    #[test]
    fn xhtml_structure() {
        let xml = r#"<html><head><title>No</title><style>p{}</style></head><body><h2>Title &amp; more</h2>
<p>Hello <b>bold</b> <i>it</i>&#160;x&nbsp;y<br/>next</p><ul><li>one<ul><li>inner</li></ul></li><li>two</li></ul><ol><li>first</li></ol>
<blockquote><p>said</p></blockquote><pre>a  b
c</pre><table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table><hr/><script>no()</script></body></html>"#;
        let blocks = xhtml_blocks(xml, "InspiredGitHub");
        assert_eq!(
            plain(&blocks),
            ["h2:Title & more", "p:Hello bold it\u{A0}x\u{A0}y\nnext", "0•one", "1•inner", "0•two", "01.first", "q:said", "code", "table:A,B;1,2", "rule"]
        );
    }

    fn epub() -> Vec<u8> {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        for (name, text) in [
            ("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>"#),
            (
                "OPS/book.opf",
                r#"<package><metadata><dc:title>My Book</dc:title><dc:creator>Someone</dc:creator></metadata><manifest><item id="a" href="a%20b.xhtml"/><item id="b" href="x/../b.xhtml"/><item id="gone" href="gone.xhtml"/></manifest><spine><itemref idref="b"/><itemref idref="gone"/><itemref idref="a"/></spine></package>"#,
            ),
            ("OPS/a b.xhtml", "<html><body><p>No heading here</p></body></html>"),
            ("OPS/b.xhtml", "<html><body><h1>Second first</h1><p>Text</p></body></html>"),
        ] {
            z.start_file(name, o).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    #[test]
    fn epub_in_spine_order() {
        let d = load("x.epub", epub(), "InspiredGitHub").unwrap();
        assert_eq!(d.title.as_deref(), Some("My Book"));
        assert_eq!(d.author.as_deref(), Some("Someone"));
        let titles: Vec<&str> = d.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles[0], "Second first");
        assert_eq!(titles.len(), 2);
        assert_eq!(plain(&d.sections[1].blocks), ["p:No heading here"]);
    }

    #[test]
    fn odt_structure() {
        let content = r#"<office:document-content><office:automatic-styles><style:style style:name="T1"><style:text-properties fo:font-weight="bold"/></style:style></office:automatic-styles><office:body><office:text><text:h text:outline-level="1">Head</text:h><text:p>Plain <text:span text:style-name="T1">bold</text:span><text:s text:c="2"/>x<text:line-break/>y &amp; z</text:p><text:list><text:list-item><text:p>item</text:p></text:list-item></text:list><table:table><table:table-row><table:table-cell><text:p>c1</text:p></table:table-cell><table:table-cell><text:p>c2</text:p></table:table-cell></table:table-row></table:table></office:text></office:body></office:document-content>"#;
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("content.xml", o).unwrap();
        z.write_all(content.as_bytes()).unwrap();
        let d = load("x.odt", z.finish().unwrap().into_inner(), "InspiredGitHub").unwrap();
        let blocks = &d.sections[0].blocks;
        assert_eq!(plain(blocks), ["h1:Head", "p:Plain bold  x\ny & z", "0•item", "table:c1,c2"]);
        let MdBlock::Para(spans) = &blocks[1] else { panic!() };
        assert!(spans.iter().any(|s| s.bold && s.text == "bold"));
    }

    #[test]
    fn long_documents_come_in_parts() {
        let blocks: Vec<MdBlock> = (0..1300).map(|_| MdBlock::Rule).collect();
        let parts = into_parts(whole_document(blocks));
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2].title, "3/3");
        assert_eq!(parts.iter().map(|p| p.blocks.len()).sum::<usize>(), 1300);
    }

    #[test]
    fn one_enormous_paragraph_is_split_and_nothing_is_lost() {
        let text = "word ".repeat(400_000);
        let span = Span { text: text.clone(), bold: true, italic: false, code: false, strike: false, link: false };
        let parts = into_parts(whole_document(vec![MdBlock::Para(vec![span])]));
        assert!(parts.len() > 1, "more than one part");
        let mut joined = String::new();
        for part in &parts {
            assert!(part.blocks.iter().map(block_text_len).sum::<usize>() <= TEXT_PER_PART);
            for b in &part.blocks {
                assert!(block_text_len(b) <= MAX_BLOCK_TEXT);
                let MdBlock::Para(s) = b else { panic!() };
                assert!(s[0].bold, "styles stay");
                joined.push_str(&spans_text(s));
            }
        }
        assert_eq!(joined, text);
    }

    fn package(files: &[(&str, &str)]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, text) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    #[test]
    fn protected_documents_are_recognised() {
        let manifest = r#"<manifest:manifest><manifest:file-entry manifest:full-path="content.xml"><manifest:encryption-data/></manifest:file-entry></manifest:manifest>"#;
        let e = load("a.odt", package(&[("META-INF/manifest.xml", manifest), ("content.xml", "scrambled")]), "InspiredGitHub").err().unwrap();
        assert_eq!(e, crate::preview::LOCKED);
        let method = |algorithm: &str| format!(r#"<encryption><EncryptedData><EncryptionMethod Algorithm="{algorithm}"/></EncryptedData></encryption>"#);
        let drm = method("http://www.w3.org/2001/04/xmlenc#aes128-cbc");
        let e = load("a.epub", package(&[("META-INF/encryption.xml", &drm)]), "InspiredGitHub").err().unwrap();
        assert_eq!(e, crate::preview::LOCKED);
        // Obfuscated fonts are not protection: the book still reads.
        assert!(!epub_is_protected(&method("http://www.idpf.org/2008/embedding")));
        assert!(!epub_is_protected(&method("http://ns.adobe.com/pdf/enc#RC")));
    }

    #[test]
    fn broken_packages_report_errors() {
        assert!(load("x.epub", b"not a zip".to_vec(), "InspiredGitHub").is_err());
        assert!(load("x.odt", b"not a zip".to_vec(), "InspiredGitHub").is_err());
    }

    #[test]
    fn zip_paths() {
        assert_eq!(join_zip_path("OPS/book.opf", "../x/./y.html"), "x/y.html");
        assert_eq!(join_zip_path("book.opf", "a.html"), "a.html");
        assert_eq!(percent_decode("a%20b%zz"), "a b%zz");
    }
}
