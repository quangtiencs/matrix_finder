//! Text files of records shown as tables: calendars (`.ics`), contacts (`.vcf`) and
//! subtitles (`.srt`, `.vtt`). Only the start of a file is read (`MAX_READ`), which
//! holds more records than the table shows.
use crate::i18n::t;
use crate::table::{clip_cell, finish_sheet, fmt_count, Cell, TableData, MAX_ROWS};
use crate::util::{ext_of, Res};

pub const MAX_READ: u64 = 32 * 1024 * 1024;
/// Records kept for the table (all of them are counted).
const MAX_KEPT: usize = 100_000;

pub fn is_record_text(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "ics" | "ical" | "ifb" | "vcf" | "vcard" | "srt" | "vtt")
}

/// Text in UTF-8, or else in the Windows Latin code page most old files use.
fn text_of(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        // A multi-byte character cut at the end of what was read.
        Err(e) if e.valid_up_to() + 4 >= bytes.len() => String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

fn sheet_of(title: String, columns: Vec<String>, rows: Vec<Vec<String>>, total: usize) -> crate::table::Sheet {
    let total = total as u64;
    let types = Some(vec!["text".to_string(); columns.len()]);
    let rows: Vec<Vec<Cell>> = rows.into_iter().take(MAX_ROWS).map(|r| r.into_iter().map(|c| (!c.is_empty()).then(|| clip_cell(c))).collect()).collect();
    finish_sheet(title, columns, types, rows, Some(total))
}

pub fn load(name: &str, bytes: &[u8]) -> Res<TableData> {
    let text = text_of(bytes);
    match ext_of(name).as_str() {
        "vcf" | "vcard" => Ok(vcard(&text)),
        "srt" | "vtt" => subtitles(&text),
        _ => calendar(&text),
    }
}

// ---------------------------------------------------------------- iCalendar and vCard

/// Content lines with folded lines joined: (name, parameters, value), one at a time.
fn content_lines(text: &str) -> impl Iterator<Item = (String, String, String)> + '_ {
    let mut lines = text.lines().peekable();
    std::iter::from_fn(move || loop {
        let first = lines.next()?;
        if first.trim().is_empty() {
            continue;
        }
        let mut line = first.to_string();
        while let Some(rest) = lines.peek().and_then(|l| l.strip_prefix([' ', '\t'])) {
            line.push_str(rest);
            lines.next();
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let (name, params) = key.split_once(';').unwrap_or((key, ""));
        // vCard groups: `item1.EMAIL`.
        let name = name.rsplit('.').next().unwrap_or(name).to_uppercase();
        return Some((name, params.to_string(), value.to_string()));
    })
}

/// `\n`, `\,` and `\;` as they are meant.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `20240131T090000Z` as `2024-01-31 09:00 UTC`, `20240131` as `2024-01-31`.
fn ical_time(v: &str, params: &str) -> String {
    let v = v.trim();
    let date = |d: &str| (d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit())).then(|| format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]));
    match v.split_once('T') {
        Some((d, time)) if time.len() >= 4 && time.is_ascii() => match date(d) {
            Some(d) => {
                let zone = if time.ends_with('Z') { " UTC".to_string() } else { params.split(';').find_map(|p| p.strip_prefix("TZID=")).map(|z| format!(" {}", z.trim_matches('"'))).unwrap_or_default() };
                format!("{d} {}:{}{zone}", &time[..2], &time[2..4])
            }
            None => v.to_string(),
        },
        _ => date(v).unwrap_or_else(|| v.to_string()),
    }
}

fn calendar(text: &str) -> Res<TableData> {
    let mut rows = vec![];
    let mut total = 0usize;
    let mut name = None;
    let mut current: Option<(String, [String; 5])> = None;
    let mut depth = 0usize;
    let mut found = false;
    for (key, params, value) in content_lines(text) {
        match key.as_str() {
            "BEGIN" => {
                found |= value.eq_ignore_ascii_case("VCALENDAR");
                depth += 1;
                if matches!(value.to_uppercase().as_str(), "VEVENT" | "VTODO" | "VJOURNAL") {
                    current = Some((value.to_uppercase(), Default::default()));
                }
            }
            "END" => {
                depth = depth.saturating_sub(1);
                if matches!(value.to_uppercase().as_str(), "VEVENT" | "VTODO" | "VJOURNAL") {
                    if let Some((kind, f)) = current.take() {
                        let kind = match kind.as_str() {
                            "VTODO" => t("task"),
                            "VJOURNAL" => t("note"),
                            _ => t("event"),
                        };
                        total += 1;
                        if rows.len() < MAX_KEPT {
                            rows.push(vec![f[0].clone(), f[1].clone(), f[2].clone(), f[3].clone(), kind, f[4].clone()]);
                        }
                    }
                }
            }
            "X-WR-CALNAME" if name.is_none() => name = Some(unescape(&value)),
            _ => {
                // Alarms inside an event have their own descriptions.
                let Some((_, f)) = current.as_mut().filter(|_| depth <= 3) else { continue };
                let slot = match key.as_str() {
                    "SUMMARY" => 0,
                    "DTSTART" => 1,
                    "DTEND" | "DUE" => 2,
                    "LOCATION" => 3,
                    "DESCRIPTION" => 4,
                    _ => continue,
                };
                if f[slot].is_empty() {
                    f[slot] = if matches!(slot, 1 | 2) { ical_time(&value, &params) } else { unescape(&value) };
                }
            }
        }
    }
    if !found {
        return Err(t("Not a calendar this preview can read."));
    }
    let mut info = vec![format!("{} {}", fmt_count(total as u64), t("items"))];
    info.extend(name);
    let columns = vec![t("Summary"), t("Start"), t("End"), t("Location"), t("Kind"), t("Description")];
    Ok(TableData { format: "iCalendar", info, sheets: vec![sheet_of(t("Events"), columns, rows, total)] })
}

fn quoted_printable_text(v: &str, params: &str) -> String {
    if !params.to_uppercase().contains("QUOTED-PRINTABLE") {
        return v.to_string();
    }
    let b = v.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' {
            if let Some(x) = v.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(x);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn vcard(text: &str) -> TableData {
    let mut rows = vec![];
    let mut total = 0usize;
    let mut card: Option<[Vec<String>; 5]> = None;
    for (key, params, value) in content_lines(text) {
        let value = unescape(&quoted_printable_text(&value, &params));
        match key.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VCARD") => card = Some(Default::default()),
            "END" if value.eq_ignore_ascii_case("VCARD") => {
                if let Some(c) = card.take() {
                    total += 1;
                    if rows.len() < MAX_KEPT {
                        rows.push(c.map(|v| v.join(", ")).to_vec());
                    }
                }
            }
            _ => {
                let Some(c) = card.as_mut() else { continue };
                let (slot, value) = match key.as_str() {
                    "FN" => (0, value),
                    // `Family;Given;…` when there is no formatted name.
                    "N" if c[0].is_empty() => (0, value.split(';').filter(|p| !p.is_empty()).rev().collect::<Vec<_>>().join(" ")),
                    "TEL" => (1, value.trim_start_matches("tel:").to_string()),
                    "EMAIL" => (2, value),
                    "ORG" => (3, value.replace(';', ", ").trim_matches([',', ' ']).to_string()),
                    "TITLE" => (4, value),
                    _ => continue,
                };
                if !value.trim().is_empty() && c[slot].len() < 8 && !(slot == 0 && !c[0].is_empty()) {
                    c[slot].push(value.trim().to_string());
                }
            }
        }
    }
    let columns = vec![t("Name"), t("Phone"), t("Email"), t("Organization"), t("Title")];
    TableData { format: "vCard", info: vec![format!("{} {}", fmt_count(total as u64), t("contacts"))], sheets: vec![sheet_of(t("Contacts"), columns, rows, total)] }
}

// ---------------------------------------------------------------- subtitles

/// `00:01:02,500` or `01:02.500` in seconds.
fn cue_time(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn cue_text(seconds: f64) -> String {
    let ms = (seconds * 1000.0).round() as u64;
    format!("{}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Text without its markup: `<i>`, `<c.yellow>`, `{\an8}`.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut skip: Option<char> = None;
    for c in s.chars() {
        match (skip, c) {
            (None, '<') => skip = Some('>'),
            (None, '{') => skip = Some('}'),
            (Some(end), c) if c == end => skip = None,
            (Some(_), _) => {}
            (None, c) => out.push(c),
        }
    }
    out
}

fn subtitles(text: &str) -> Res<TableData> {
    let text = text.replace("\r\n", "\n");
    let vtt = text.starts_with("WEBVTT");
    let mut rows = vec![];
    let mut total = 0usize;
    let mut last_end = 0.0f64;
    for block in text.split("\n\n") {
        let mut lines = block.lines().skip_while(|l| l.trim().is_empty());
        let Some(mut first) = lines.next() else { continue };
        let mut id = String::new();
        if !first.contains("-->") {
            id = first.trim().to_string();
            match lines.next() {
                Some(l) => first = l,
                None => continue,
            }
        }
        let Some((start, end)) = first.split_once("-->") else { continue };
        // A cue's settings follow its end time.
        let end = end.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (cue_time(start), cue_time(end)) else { continue };
        last_end = last_end.max(end);
        let words: Vec<String> = lines.map(strip_tags).filter(|l| !l.trim().is_empty()).collect();
        total += 1;
        if rows.len() < MAX_KEPT {
            rows.push(vec![if id.is_empty() { total.to_string() } else { id }, cue_text(start), cue_text(end), words.join("\n")]);
        }
    }
    if rows.is_empty() {
        return Err(t("No subtitles found in this file."));
    }
    let info = vec![format!("{} {}", fmt_count(total as u64), t("cues")), crate::media::format_duration(last_end)];
    let columns = vec!["#".to_string(), t("Start"), t("End"), t("Text")];
    Ok(TableData { format: if vtt { "WebVTT" } else { "SubRip" }, info, sheets: vec![sheet_of(t("Subtitles"), columns, rows, total)] })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(d: &TableData, r: usize, c: usize) -> Option<&str> {
        d.sheets[0].rows[r][c].as_deref()
    }

    #[test]
    fn calendars_list_events() {
        let ics = "BEGIN:VCALENDAR\r\nX-WR-CALNAME:Work\r\nBEGIN:VEVENT\r\nSUMMARY:Team\\, weekly\r\nDTSTART;TZID=Asia/Ho_Chi_Minh:20240131T090000\r\nDTEND:20240131T100000Z\r\nLOCATION:Room 1\r\nDESCRIPTION:Line one\\nline t\r\n wo\r\nBEGIN:VALARM\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VTODO\r\nSUMMARY:Pay\r\nDUE;VALUE=DATE:20240201\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let d = load("a.ics", ics.as_bytes()).unwrap();
        assert_eq!(d.info, ["2 items", "Work"]);
        assert_eq!(cell(&d, 0, 0), Some("Team, weekly"));
        assert_eq!(cell(&d, 0, 1), Some("2024-01-31 09:00 Asia/Ho_Chi_Minh"));
        assert_eq!(cell(&d, 0, 2), Some("2024-01-31 10:00 UTC"));
        assert_eq!(cell(&d, 0, 5), Some("Line one\nline two"));
        assert_eq!(cell(&d, 1, 2), Some("2024-02-01"));
        assert!(load("a.ics", b"just text").is_err());
        // A damaged time is shown as it is.
        let d = load("a.ics", "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20240131T\u{2020}00\nEND:VEVENT\nEND:VCALENDAR".as_bytes()).unwrap();
        assert_eq!(cell(&d, 0, 1), Some("20240131T\u{2020}00"));
    }

    #[test]
    fn contacts_list_people() {
        let vcf = "BEGIN:VCARD\nVERSION:3.0\nN:Le;Tien;;;\nTEL;TYPE=cell:+84 123\nTEL:+84 456\nitem1.EMAIL:a@b.c\nORG:Acme;Lab\nEND:VCARD\nBEGIN:VCARD\nVERSION:2.1\nFN;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8:Ti=E1=BA=BFn\nEND:VCARD\n";
        let d = load("a.vcf", vcf.as_bytes()).unwrap();
        assert_eq!(cell(&d, 0, 0), Some("Tien Le"));
        assert_eq!(cell(&d, 0, 1), Some("+84 123, +84 456"));
        assert_eq!(cell(&d, 0, 2), Some("a@b.c"));
        assert_eq!(cell(&d, 0, 3), Some("Acme, Lab"));
        assert_eq!(cell(&d, 1, 0), Some("Tiến"));
    }

    #[test]
    fn subtitles_list_cues() {
        let srt = "1\r\n00:00:01,000 --> 00:00:02,500\r\n<i>Hello</i>\r\nthere\r\n\r\n2\r\n00:01:00,000 --> 00:01:03,000\r\n{\\an8}Top\r\n";
        let d = load("a.srt", srt.as_bytes()).unwrap();
        assert_eq!(d.format, "SubRip");
        assert_eq!(cell(&d, 0, 1), Some("0:00:01.000"));
        assert_eq!(cell(&d, 0, 3), Some("Hello\nthere"));
        assert_eq!(cell(&d, 1, 3), Some("Top"));
        let vtt = "WEBVTT\n\nintro\n00:01.000 --> 00:02.000 align:start\nHi\n";
        let d = load("a.vtt", vtt.as_bytes()).unwrap();
        assert_eq!((cell(&d, 0, 0), cell(&d, 0, 2)), (Some("intro"), Some("0:00:02.000")));
        assert!(load("a.srt", b"nothing").is_err());
    }
}
