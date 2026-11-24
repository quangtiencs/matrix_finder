//! E-mail messages (`.eml`) and mailboxes (`.mbox`): the headers that matter, the text
//! of the message (its plain part, or its HTML part as text) and the attachments by
//! name and size. Attachments are never decoded, and HTML is read as text: nothing in
//! a message is loaded from the network or run.
use crate::preview::markdown::{MdBlock, Span};
use crate::preview::textdocs::{Document, Section};
use crate::util::Res;
use base64::Engine;

/// Most parts read from one message, and how deeply multiparts may nest.
const MAX_PARTS: usize = 1000;
const MAX_DEPTH: usize = 16;
/// A text part larger than this is cut before it is decoded.
const MAX_TEXT_PART: usize = 8 * 1024 * 1024;
/// Most messages shown of a mailbox, and how much of it is read.
const MAX_MESSAGES: usize = 1000;
pub const MAX_MBOX_READ: u64 = 32 * 1024 * 1024;

fn span(text: impl Into<String>) -> Span {
    Span { text: text.into(), bold: false, italic: false, code: false, strike: false, link: false }
}

/// Split a message into its headers and body at the first blank line.
fn split_message(raw: &[u8]) -> (&[u8], &[u8]) {
    for (i, w) in raw.windows(2).enumerate() {
        if w == b"\n\n" {
            return (&raw[..i], &raw[i + 2..]);
        }
        if w == b"\n\r" && raw.get(i + 2) == Some(&b'\n') {
            return (&raw[..i], &raw[i + 3..]);
        }
    }
    (raw, &[])
}

/// Text in some character set, as UTF-8.
fn decode_charset(bytes: &[u8], charset: &str) -> String {
    match encoding_rs::Encoding::for_label(charset.trim().as_bytes()) {
        Some(enc) => enc.decode(bytes).0.into_owned(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn quoted_printable(b: &[u8], header: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'=' => {
                let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                if let Some(v) = hex {
                    out.push(v);
                    i += 3;
                } else if b.get(i + 1) == Some(&b'\n') {
                    i += 2;
                } else if b.get(i + 1..i + 3) == Some(b"\r\n") {
                    i += 3;
                } else {
                    out.push(b'=');
                    i += 1;
                }
            }
            b'_' if header => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Base64 as mail has it: in lines, with or without padding, sometimes with stray bytes.
fn base64_lenient(b: &[u8]) -> Vec<u8> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    const LENIENT: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::RequireNone).with_decode_allow_trailing_bits(true),
    );
    let mut clean: Vec<u8> = b.iter().copied().filter(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/')).collect();
    // One character past a whole group holds no byte.
    if clean.len() % 4 == 1 {
        clean.pop();
    }
    LENIENT.decode(&clean).unwrap_or_default()
}

/// `=?utf-8?B?…?=` words in a header, decoded.
fn decode_words(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        let parsed = (|| {
            let body = &rest[start + 2..];
            let (charset, body) = body.split_once('?')?;
            let (enc, body) = body.split_once('?')?;
            let end = body.find("?=")?;
            let data = &body[..end];
            let bytes = match enc {
                "B" | "b" => base64_lenient(data.as_bytes()),
                "Q" | "q" => quoted_printable(data.as_bytes(), true),
                _ => return None,
            };
            let charset = charset.split('*').next().unwrap_or(charset);
            Some((decode_charset(&bytes, charset), start + 2 + (rest[start + 2..].len() - body.len()) + end + 2))
        })();
        match parsed {
            Some((text, consumed)) => {
                // White space between two encoded words is dropped.
                if !(last_was_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&text);
                rest = &rest[consumed..];
                last_was_word = true;
            }
            None => {
                out.push_str(&rest[..start + 2]);
                rest = &rest[start + 2..];
                last_was_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Headers with folded lines joined, names in lower case.
fn parse_headers(head: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(head);
    let mut out: Vec<(String, String)> = vec![];
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            if out.len() >= 2000 {
                break;
            }
            out.push((name.trim().to_lowercase(), value.trim().to_string()));
        }
    }
    out
}

fn header<'a>(hs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    hs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// A header value's main part and its parameters: `text/plain; charset=utf-8`.
fn with_params(value: &str) -> (String, Vec<(String, String)>) {
    let mut parts = value.split(';');
    let main = parts.next().unwrap_or("").trim().to_lowercase();
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            let k = k.trim().to_lowercase();
            let mut v = v.trim().trim_matches('"').to_string();
            // RFC 2231: `filename*=utf-8''na%C3%AFve.txt`.
            if k.ends_with('*') {
                if let Some((charset, encoded)) = v.split_once("''") {
                    let bytes = percent_bytes(encoded);
                    v = decode_charset(&bytes, charset);
                }
            }
            Some((k.trim_end_matches('*').to_string(), v))
        })
        .collect();
    (main, params)
}

fn percent_bytes(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[derive(Default)]
struct Parts {
    plain: Option<String>,
    html: Option<String>,
    attachments: Vec<(String, u64)>,
    count: usize,
}

fn walk(raw: &[u8], depth: usize, out: &mut Parts) {
    out.count += 1;
    if depth > MAX_DEPTH || out.count > MAX_PARTS {
        return;
    }
    let (head, body) = split_message(raw);
    let hs = parse_headers(head);
    let (mime, params) = with_params(header(&hs, "content-type").unwrap_or("text/plain"));
    let encoding = header(&hs, "content-transfer-encoding").unwrap_or("").trim().to_lowercase();
    let (disposition, dparams) = with_params(header(&hs, "content-disposition").unwrap_or(""));
    if mime.starts_with("multipart/") {
        let Some(boundary) = param(&params, "boundary").filter(|b| !b.is_empty()) else { return };
        let marker = format!("--{boundary}");
        let mut parts = vec![];
        let mut start: Option<usize> = None;
        let mut at = 0;
        for line in body.split_inclusive(|c| *c == b'\n') {
            let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
            let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
            if trimmed.starts_with(marker.as_bytes()) {
                if let Some(s) = start {
                    parts.push(&body[s..at]);
                }
                if trimmed[marker.len()..].starts_with(b"--") {
                    start = None;
                    break;
                }
                start = Some(at + line.len());
            }
            at += line.len();
        }
        if let Some(s) = start {
            parts.push(&body[s..]);
        }
        for p in parts {
            walk(p, depth + 1, out);
        }
        return;
    }
    let name = param(&dparams, "filename").or_else(|| param(&params, "name")).map(decode_words);
    let is_text = mime == "text/plain" || mime == "text/html";
    if disposition == "attachment" || name.is_some() || !is_text {
        let size = if encoding == "base64" { body.iter().filter(|c| !c.is_ascii_whitespace()).count() as u64 * 3 / 4 } else { body.len() as u64 };
        let name = name.unwrap_or_else(|| if mime == "message/rfc822" { "message.eml".into() } else { mime.clone() });
        out.attachments.push((name, size));
        return;
    }
    let body = &body[..body.len().min(MAX_TEXT_PART)];
    let bytes = match encoding.as_str() {
        "base64" => base64_lenient(body),
        "quoted-printable" => quoted_printable(body, false),
        _ => body.to_vec(),
    };
    let text = decode_charset(&bytes, param(&params, "charset").unwrap_or("utf-8"));
    let slot = if mime == "text/html" { &mut out.html } else { &mut out.plain };
    if slot.is_none() {
        *slot = Some(text);
    }
}

/// Plain text as paragraphs, with quoted lines (`> …`) as quotes.
fn plain_blocks(text: &str) -> Vec<MdBlock> {
    let mut out = vec![];
    for para in text.replace("\r\n", "\n").split("\n\n") {
        let para = para.trim_end();
        if para.trim().is_empty() {
            continue;
        }
        if para.lines().all(|l| l.starts_with('>')) {
            let quoted: Vec<&str> = para.lines().map(|l| l.trim_start_matches('>').trim_start()).collect();
            out.push(MdBlock::Quote(vec![span(quoted.join("\n"))]));
        } else {
            out.push(MdBlock::Para(vec![span(para.trim_start_matches('\n'))]));
        }
    }
    out
}

struct Message {
    subject: Option<String>,
    from: Option<String>,
    blocks: Vec<MdBlock>,
}

fn message(raw: &[u8], syntax_theme: &str) -> Message {
    use crate::i18n::t;
    let (head, _) = split_message(raw);
    let hs = parse_headers(head);
    let get = |n: &str| header(&hs, n).map(decode_words).filter(|v| !v.is_empty());
    let mut rows = vec![];
    for (key, label) in [("from", t("From")), ("to", t("To")), ("cc", t("Cc")), ("date", t("Date")), ("subject", t("Subject"))] {
        if let Some(v) = get(key) {
            rows.push(vec![vec![Span { bold: true, ..span(label) }], vec![span(v)]]);
        }
    }
    let mut parts = Parts::default();
    walk(raw, 0, &mut parts);
    let mut blocks = vec![];
    if !rows.is_empty() {
        blocks.push(MdBlock::Table { rows, has_header: false });
    }
    match (parts.plain, parts.html) {
        (Some(plain), _) if !plain.trim().is_empty() => blocks.extend(plain_blocks(&plain)),
        (_, Some(html)) => blocks.extend(crate::preview::textdocs::html_blocks(&html, syntax_theme)),
        _ => {}
    }
    if !parts.attachments.is_empty() {
        blocks.push(MdBlock::Heading(3, vec![span(format!("{} ({})", t("Attachments"), parts.attachments.len()))]));
        for (name, size) in parts.attachments.into_iter().take(500) {
            blocks.push(MdBlock::Item { depth: 0, marker: "•".into(), spans: vec![span(format!("{name} · {}", crate::util::human_size(size)))] });
        }
    }
    Message { subject: get("subject"), from: get("from"), blocks }
}

pub fn load_eml(bytes: &[u8], syntax_theme: &str) -> Res<Document> {
    let m = message(bytes, syntax_theme);
    Ok(Document { format: "Email", title: m.subject, author: m.from, sections: vec![Section { title: String::new(), blocks: m.blocks }] })
}

/// A mailbox: messages one after another, each starting with a `From ` line.
pub fn load_mbox(bytes: &[u8], cut_short: bool, syntax_theme: &str) -> Res<Document> {
    use crate::i18n::t;
    let mut starts = vec![];
    let mut at = 0;
    for line in bytes.split_inclusive(|c| *c == b'\n') {
        let after_blank = at == 0 || bytes[..at].ends_with(b"\n\n") || bytes[..at].ends_with(b"\n\r\n");
        if line.starts_with(b"From ") && after_blank {
            starts.push(at);
        }
        at += line.len();
    }
    if starts.is_empty() {
        return Err(t("Not a mailbox this preview can read."));
    }
    let total = starts.len();
    starts.push(bytes.len());
    let mut sections = vec![];
    for w in starts.windows(2).take(MAX_MESSAGES) {
        let raw = &bytes[w[0]..w[1]];
        // The `From ` line is the mailbox's, not the message's.
        let raw = raw.iter().position(|c| *c == b'\n').map_or(raw, |i| &raw[i + 1..]);
        let m = message(raw, syntax_theme);
        let title = format!("{}. {}", sections.len() + 1, m.subject.unwrap_or_else(|| t("(no subject)")));
        sections.push(Section { title, blocks: m.blocks });
    }
    let more = total > MAX_MESSAGES || cut_short;
    let title = Some(format!("{}{} {}", crate::util::fmt_count(total.min(MAX_MESSAGES) as u64), if more { "+" } else { "" }, t("messages")));
    Ok(Document { format: "Mailbox", title, author: None, sections })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIL: &str = "From: =?UTF-8?B?VGnhur9u?= <a@example.com>\r\nTo: b@example.com\r\nSubject: =?utf-8?Q?Ch=C3=A0o?=\r\n =?utf-8?Q?_b=E1=BA=A1n?=\r\nContent-Type: multipart/mixed; boundary=\"XX\"\r\n\r\npreamble\r\n--XX\r\nContent-Type: multipart/alternative; boundary=YY\r\n\r\n--YY\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nHello=20there,\r\nline two=\r\n joined\r\n\r\n> quoted\r\n--YY\r\nContent-Type: text/html\r\n\r\n<p>html</p>\r\n--YY--\r\n--XX\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nAAAAAAAA\r\n--XX--\r\n";

    #[test]
    fn reads_headers_text_and_attachments() {
        let d = load_eml(MAIL.as_bytes(), "base16-ocean.dark").unwrap();
        assert_eq!(d.title.as_deref(), Some("Chào bạn"));
        assert_eq!(d.author.as_deref(), Some("Tiến <a@example.com>"));
        let blocks = &d.sections[0].blocks;
        assert!(matches!(&blocks[0], MdBlock::Table { rows, .. } if rows.len() == 3));
        assert!(matches!(&blocks[1], MdBlock::Para(s) if s[0].text == "Hello there,\nline two joined"));
        assert!(matches!(&blocks[2], MdBlock::Quote(s) if s[0].text == "quoted"));
        assert!(matches!(blocks.last(), Some(MdBlock::Item { spans, .. }) if spans[0].text.starts_with("report.pdf · 6 B")));
    }

    #[test]
    fn mailboxes_split_into_messages() {
        let mbox = "From a@b Mon Jan 1 00:00:00 2024\nSubject: one\n\nbody\nFrom here is not a separator\n\nFrom c@d Tue Jan 2 00:00:00 2024\nSubject: two\n\nbody two\n";
        let d = load_mbox(mbox.as_bytes(), false, "base16-ocean.dark").unwrap();
        assert_eq!(d.sections.len(), 2);
        assert_eq!(d.sections[1].title, "2. two");
        assert!(load_mbox(b"no mailbox here", false, "x").is_err());
    }

    #[test]
    fn damaged_messages_do_not_fail() {
        for raw in ["", "Content-Type: multipart/mixed\n\n--", "Subject: =?utf-8?B?!!!?=\n\n", "Content-Type: multipart/mixed; boundary=a\n\n--a\n--a\n--a--"] {
            let _ = load_eml(raw.as_bytes(), "x").unwrap();
        }
        // Nesting past the limit stops quietly.
        let mut deep = String::new();
        for i in 0..100 {
            deep.push_str(&format!("Content-Type: multipart/mixed; boundary=b{i}\n\n--b{i}\n"));
        }
        let _ = load_eml(deep.as_bytes(), "x").unwrap();
    }
}
