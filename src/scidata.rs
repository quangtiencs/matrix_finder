//! More table previews: SQLite, JSON Lines, NumPy, Arrow/Feather and HDF5. Each
//! shows the first rows of each table or dataset through the same table view.
use crate::i18n::t;
use crate::table::{clip_cell, finish_sheet, Cell, Sheet, TableData, MAX_COLS, MAX_ROWS};
use crate::util::{es, Res};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- SQLite

const MAX_TABLES: usize = 100;
/// A query taking longer than this (a slow view, a count on a huge table) is given up on.
const SQL_TIME_LIMIT: Duration = Duration::from_secs(2);
/// Text kept from all tables of a database together.
const SQL_MAX_TEXT: usize = 256 * 1024 * 1024;

/// `file:` URI for a path, opened so nothing can be written or created next to it.
fn sqlite_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'/' | b'-' | b'_' | b'.' | b'~' => uri.push(b as char),
            b if b.is_ascii_alphanumeric() => uri.push(b as char),
            b => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    uri
}

pub fn sqlite_from_path(path: &Path) -> Res<TableData> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(sqlite_uri(path), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(es)?;
    // A database file is untrusted data: its views must not call anything risky.
    conn.pragma_update(None, "trusted_schema", "OFF").map_err(es)?;
    let objects: Vec<(String, String)> = {
        // An encrypted database looks like random bytes: nothing tells it from a damaged one.
        let unreadable = |e: rusqlite::Error| if e.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase) { t("This is not a SQLite database, or it is encrypted.") } else { es(e) };
        let mut q = conn.prepare("SELECT name, type FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY type, name").map_err(unreadable)?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(es)?;
        rows.collect::<Result<_, _>>().map_err(es)?
    };
    let total_objects = objects.len();
    let mut sheets = vec![];
    let mut text_left = SQL_MAX_TEXT;
    for (name, kind) in objects.into_iter().take(MAX_TABLES) {
        sheets.push(sqlite_sheet(&conn, &name, &kind, &mut text_left));
    }
    // Open on something worth seeing: empty tables go last.
    sheets.sort_by_key(|s| s.rows.is_empty());
    let mut info = vec![format!("{total_objects} {}", t("tables and views"))];
    if total_objects > MAX_TABLES {
        info.push(format!("{} {MAX_TABLES}", t("Showing the first")));
    }
    if sheets.is_empty() {
        sheets.push(finish_sheet(t("Empty database"), vec![], None, vec![], Some(0)));
    }
    Ok(TableData { format: "SQLite", info, sheets })
}

fn sqlite_sheet(conn: &rusqlite::Connection, name: &str, kind: &str, text_left: &mut usize) -> Sheet {
    let quoted = format!("\"{}\"", name.replace('"', "\"\""));
    let sheet_name = if kind == "view" { format!("{name} ({})", t("view")) } else { name.to_string() };
    let started = Instant::now();
    let _ = conn.progress_handler(10_000, Some(move || started.elapsed() > SQL_TIME_LIMIT));
    let mut read = || -> rusqlite::Result<Sheet> {
        let mut q = conn.prepare(&format!("SELECT * FROM {quoted} LIMIT {MAX_ROWS}"))?;
        let columns: Vec<String> = q.column_names().iter().take(MAX_COLS).map(|c| c.to_string()).collect();
        let types: Vec<String> = q.columns().iter().take(MAX_COLS).map(|c| c.decl_type().unwrap_or("").to_lowercase()).collect();
        let mut rows = vec![];
        let mut result = q.query([])?;
        while let Some(row) = result.next()? {
            let cells: Vec<Cell> = (0..columns.len()).map(|i| sqlite_cell(row.get_ref(i))).collect();
            let size: usize = cells.iter().flatten().map(|c| c.len() + 32).sum();
            if size > *text_left {
                break;
            }
            *text_left -= size;
            rows.push(cells);
        }
        drop(result);
        let total = if rows.len() < MAX_ROWS { Some(rows.len() as u64) } else { conn.query_row(&format!("SELECT count(*) FROM {quoted}"), [], |r| r.get::<_, i64>(0)).ok().map(|n| n as u64) };
        let known = types.iter().all(|t| !t.is_empty());
        Ok(finish_sheet(sheet_name.clone(), columns, known.then_some(types), rows, total))
    };
    let sheet = read().unwrap_or_else(|e| finish_sheet(sheet_name.clone(), vec![t("Error")], Some(vec!["text".into()]), vec![vec![Some(e.to_string())]], None));
    let _ = conn.progress_handler(0, None::<fn() -> bool>);
    sheet
}

fn sqlite_cell(v: rusqlite::Result<rusqlite::types::ValueRef>) -> Cell {
    use rusqlite::types::ValueRef as V;
    match v.ok()? {
        V::Null => None,
        V::Integer(i) => Some(i.to_string()),
        V::Real(f) => Some(f.to_string()),
        // Only the start is shown: no need to copy a value that may be a gigabyte.
        V::Text(s) => Some(clip_cell(String::from_utf8_lossy(&s[..s.len().min(4096)]).into_owned())),
        V::Blob(b) => Some(format!("<{} · {}>", t("binary"), crate::util::human_size(b.len() as u64))),
    }
}

// ---------------------------------------------------------------- JSON Lines

/// Longest line read, and how far a file is read looking for valid lines.
const MAX_JSON_LINE: u64 = 32 * 1024 * 1024;
const MAX_JSON_SCAN: u64 = 512 * 1024 * 1024;

pub fn jsonl_from<R: Read>(reader: R) -> Res<TableData> {
    use serde_json::Value;
    let mut columns: Vec<String> = vec![];
    let mut records: Vec<Vec<(usize, Cell)>> = vec![];
    let (mut bad_lines, mut more) = (0u64, false);
    let cell = |v: &Value| -> Cell {
        match v {
            Value::Null => None,
            Value::String(s) => Some(clip_cell(s.clone())),
            other => Some(clip_cell(other.to_string())),
        }
    };
    let mut reader = std::io::BufReader::new(reader);
    let mut line = Vec::new();
    let mut scanned = 0u64;
    let mut line_too_long = false;
    loop {
        line.clear();
        // A line is only read up to the limit: a file need not have any line ends at all.
        let Ok(got) = (&mut reader).take(MAX_JSON_LINE + 1).read_until(b'\n', &mut line) else { break };
        if got == 0 {
            break;
        }
        scanned += got as u64;
        if got as u64 > MAX_JSON_LINE {
            line_too_long = true;
            more = true;
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if records.len() >= MAX_ROWS || scanned > MAX_JSON_SCAN {
            more = true;
            break;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            bad_lines += 1;
            continue;
        };
        let mut column_of = |name: &str| match columns.iter().position(|c| c == name) {
            Some(i) => Some(i),
            None if columns.len() < MAX_COLS => {
                columns.push(name.to_string());
                Some(columns.len() - 1)
            }
            None => None,
        };
        let record = match &value {
            Value::Object(map) => map.iter().filter_map(|(k, v)| Some((column_of(k)?, cell(v)))).collect(),
            other => column_of("value").map(|i| vec![(i, cell(other))]).unwrap_or_default(),
        };
        records.push(record);
    }
    if records.is_empty() && (bad_lines > 0 || line_too_long) {
        return Err(t("This file has no valid JSON lines."));
    }
    let rows: Vec<Vec<Cell>> = records
        .into_iter()
        .map(|record| {
            let mut row = vec![None; columns.len()];
            for (i, v) in record {
                row[i] = v;
            }
            row
        })
        .collect();
    let mut info = vec![];
    if bad_lines > 0 {
        info.push(format!("{bad_lines} {}", t("invalid lines skipped")));
    }
    if line_too_long {
        info.push(t("stopped at a very long line"));
    }
    let total = (!more).then_some(rows.len() as u64);
    Ok(TableData { format: "JSON Lines", info, sheets: vec![finish_sheet(String::new(), columns, None, rows, total)] })
}

// ---------------------------------------------------------------- NumPy

/// Largest array item read: larger ones would be read hundreds of thousands of times.
const NP_MAX_ITEM: u64 = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Debug)]
enum NpKind {
    Bool,
    Int,
    Uint,
    Float,
    Complex,
    Bytes,
    Unicode,
}

#[derive(Clone, PartialEq, Debug)]
struct NpField {
    name: String,
    kind: NpKind,
    size: usize,
    little: bool,
    text: String,
}

/// `<f8`, `|b1`, `>i4`, `<U5`, `|S2`. `None` for what cannot be shown (`O` is pickled Python data).
fn np_field(name: &str, descr: &str) -> Option<NpField> {
    let (order, rest) = match descr.as_bytes().first()? {
        b'<' | b'>' | b'|' | b'=' => descr.split_at(1),
        _ => ("=", descr),
    };
    let code = rest.chars().next()?;
    let size: usize = rest[1..].parse().ok()?;
    let (kind, bytes) = match code {
        'b' => (NpKind::Bool, size),
        'i' => (NpKind::Int, size),
        'u' => (NpKind::Uint, size),
        'f' => (NpKind::Float, size),
        'c' => (NpKind::Complex, size),
        'S' | 'a' => (NpKind::Bytes, size),
        'U' => (NpKind::Unicode, size.checked_mul(4)?),
        _ => return None,
    };
    Some(NpField { name: name.to_string(), kind, size: bytes, little: order != ">", text: descr.trim_start_matches(['<', '|', '=', '>']).to_string() })
}

fn np_value(f: &NpField, b: &[u8]) -> String {
    let ordered = |b: &[u8]| -> Vec<u8> {
        let mut v = b.to_vec();
        if !f.little {
            v.reverse();
        }
        v
    };
    let float = |b: &[u8]| -> f64 {
        let v = ordered(b);
        match v.len() {
            2 => half_to_f64(u16::from_le_bytes([v[0], v[1]])),
            4 => f64::from(f32::from_le_bytes([v[0], v[1], v[2], v[3]])),
            8 => f64::from_le_bytes(v[..8].try_into().unwrap_or_default()),
            _ => f64::NAN,
        }
    };
    match f.kind {
        NpKind::Bool => (b[0] != 0).to_string(),
        NpKind::Int | NpKind::Uint => {
            let v = ordered(b);
            let mut raw = [0u8; 8];
            let n = v.len().min(8);
            raw[..n].copy_from_slice(&v[..n]);
            if f.kind == NpKind::Int {
                let shift = (8 - n) * 8;
                ((i64::from_le_bytes(raw) << shift) >> shift).to_string()
            } else {
                u64::from_le_bytes(raw).to_string()
            }
        }
        NpKind::Float => format!("{}", float(b)),
        NpKind::Complex => {
            let (re, im) = b.split_at(b.len() / 2);
            let im = float(im);
            format!("{}{}{}i", float(re), if im < 0.0 { "-" } else { "+" }, im.abs())
        }
        NpKind::Bytes => String::from_utf8_lossy(b).trim_end_matches('\0').to_string(),
        NpKind::Unicode => {
            let order = |c: &[u8]| {
                let c = [c[0], c[1], c[2], c[3]];
                if f.little { u32::from_le_bytes(c) } else { u32::from_be_bytes(c) }
            };
            b.chunks_exact(4).map(order).take_while(|c| *c != 0).filter_map(char::from_u32).collect()
        }
    }
}

fn half_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let (exp, frac) = (i32::from((h >> 10) & 0x1F), f64::from(h & 0x3FF));
    sign * match exp {
        0 => frac * 2f64.powi(-24),
        31 if frac == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        e => (1.0 + frac / 1024.0) * 2f64.powi(e - 15),
    }
}

/// The value after `'key':` in the header's Python dict, up to its end.
fn np_header_value<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("'{key}'");
    let rest = &header[header.find(&pat)? + pat.len()..];
    Some(rest.split_once(':')?.1.trim_start())
}

/// Bracket-matching: the `[...]` or `(...)` at the start of `s`.
fn np_group(s: &str) -> Option<&str> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn np_quoted(s: &str) -> Vec<&str> {
    s.split('\'').skip(1).step_by(2).collect()
}

fn np_fields(descr: &str) -> Option<Vec<NpField>> {
    if descr.starts_with('[') {
        let body = np_group(descr)?;
        // [('a', '<i4'), ('b', '<f8')]: nested or shaped fields are not shown.
        let mut fields = vec![];
        for part in body[1..body.len() - 1].split("),") {
            let names = np_quoted(part);
            if names.len() != 2 || part.matches('(').count() > 1 {
                return None;
            }
            fields.push(np_field(names[0], names[1])?);
        }
        Some(fields)
    } else {
        Some(vec![np_field("", np_quoted(descr).first()?)?])
    }
}

/// The header of a `.npy` file: what is needed to list an array without reading it.
pub struct NpyHeader {
    pub fortran: bool,
    pub shape: Vec<u64>,
    /// The rest of the header from the value of `'descr'` on.
    descr: String,
}

impl NpyHeader {
    /// The element type for people: `f8`, `U5`, the fields of a structured array, `O`.
    pub fn dtype_text(&self) -> String {
        match np_fields(&self.descr) {
            Some(f) if f.len() == 1 && f[0].name.is_empty() => f[0].text.clone(),
            Some(f) => format!("({})", f.iter().map(|f| format!("{}: {}", f.name, f.text)).collect::<Vec<_>>().join(", ")),
            None if self.descr.starts_with('[') => "structured".to_string(),
            None => np_quoted(&self.descr).first().map_or_else(|| "?".to_string(), |s| s.trim_start_matches(['<', '>', '|', '=']).to_string()),
        }
    }

    /// Whether the values can be shown (`O` arrays are pickled Python objects).
    pub fn readable(&self) -> bool {
        np_fields(&self.descr).is_some()
    }
}

pub fn npy_header<R: Read>(r: &mut R) -> Res<NpyHeader> {
    let mut head = [0u8; 10];
    r.read_exact(&mut head).map_err(|_| t("Not a valid NumPy file."))?;
    if &head[..6] != b"\x93NUMPY" {
        return Err(t("Not a valid NumPy file."));
    }
    let header_len = if head[6] == 1 {
        u64::from(u16::from_le_bytes([head[8], head[9]]))
    } else {
        let mut more = [0u8; 2];
        r.read_exact(&mut more).map_err(es)?;
        u64::from(u32::from_le_bytes([head[8], head[9], more[0], more[1]]))
    };
    if header_len > 1024 * 1024 {
        return Err(t("Not a valid NumPy file."));
    }
    let mut header = vec![0u8; header_len as usize];
    r.read_exact(&mut header).map_err(|_| t("This file is cut short."))?;
    let header = String::from_utf8_lossy(&header).into_owned();
    let bad = || t("Not a valid NumPy file.");
    let fortran = np_header_value(&header, "fortran_order").ok_or_else(bad)?.starts_with("True");
    let shape: Vec<u64> = np_group(np_header_value(&header, "shape").ok_or_else(bad)?).ok_or_else(bad)?.trim_matches(['(', ')']).split(',').filter_map(|n| n.trim().parse().ok()).collect();
    let descr = np_header_value(&header, "descr").ok_or_else(bad)?.to_string();
    Ok(NpyHeader { fortran, shape, descr })
}

pub fn npy_from<R: Read + Seek>(mut r: R) -> Res<TableData> {
    let NpyHeader { fortran, shape, descr } = npy_header(&mut r)?;
    let data_at = r.stream_position().map_err(es)?;
    let bad = || t("Not a valid NumPy file.");
    let shape_text = format!("[{}]", shape.iter().map(u64::to_string).collect::<Vec<_>>().join(" × "));
    let fields = np_fields(&descr).ok_or_else(|| t("This array holds Python objects, which are not read for safety."))?;
    let item = fields.iter().try_fold(0u64, |sum, f| sum.checked_add(f.size as u64)).filter(|i| *i <= NP_MAX_ITEM).ok_or_else(|| t("The items of this array are too large to preview."))?;
    // The numbers come from the file: they may be anything, and offsets must not overflow.
    shape.iter().try_fold(item, |bytes, n| bytes.checked_mul(*n)).ok_or_else(bad)?;
    // Rows are the first axis; the rest becomes columns. A scalar is one row.
    let n_rows = shape.first().copied().unwrap_or(1);
    let per_row: u64 = if shape.len() <= 1 { 1 } else { shape.iter().skip(1).product() };
    let shown_cols = per_row.min(MAX_COLS as u64) as usize;
    let structured = fields.len() > 1 || !fields[0].name.is_empty();
    let mut columns: Vec<String> = vec![];
    let mut types: Vec<String> = vec![];
    if structured {
        for f in fields.iter().take(MAX_COLS) {
            columns.push(f.name.clone());
            types.push(f.text.clone());
        }
    } else {
        for c in 0..shown_cols {
            columns.push(if shape.len() <= 1 { "value".to_string() } else { c.to_string() });
            types.push(fields[0].text.clone());
        }
    }
    let shown_rows = n_rows.min(MAX_ROWS as u64) as usize;
    let mut rows = Vec::with_capacity(shown_rows);
    let cell_at = |r: &mut R, index: u64| -> Res<Vec<u8>> {
        r.seek(SeekFrom::Start(data_at + index * item)).map_err(es)?;
        let mut b = vec![0u8; item as usize];
        r.read_exact(&mut b).map_err(|_| t("This file is cut short."))?;
        Ok(b)
    };
    for row in 0..shown_rows as u64 {
        let mut cells: Vec<Cell> = vec![];
        if structured {
            let raw = cell_at(&mut r, row)?;
            let mut at = 0;
            for f in fields.iter().take(MAX_COLS) {
                cells.push(Some(clip_cell(np_value(f, &raw[at..at + f.size]))));
                at += f.size;
            }
        } else {
            for col in 0..shown_cols as u64 {
                // C order: the row's items are together. Fortran order: columns are.
                let index = if fortran { row + col * n_rows } else { row * per_row + col };
                let raw = cell_at(&mut r, index)?;
                cells.push(Some(clip_cell(np_value(&fields[0], &raw))));
            }
        }
        rows.push(cells);
    }
    let mut info = vec![shape_text];
    if fortran {
        info.push("Fortran order".to_string());
    }
    if per_row > shown_cols as u64 {
        info.push(format!("{} {shown_cols} {}", t("Showing the first"), t("columns")));
    }
    Ok(TableData { format: "NumPy", info, sheets: vec![finish_sheet(String::new(), columns, Some(types), rows, Some(n_rows))] })
}

// ---------------------------------------------------------------- Arrow / Feather

/// One record batch is read whole, however few of its rows are shown.
const ARROW_MAX_BATCH: i64 = 512 * 1024 * 1024;
const ARROW_MAX_METADATA: u64 = 64 * 1024 * 1024;

fn arrow_batch_too_large() -> String {
    t("The data blocks of this file are too large to preview.")
}

/// Check the batch sizes in an Arrow file's footer before anything reads them.
fn check_arrow_file<R: Read + Seek>(r: &mut R) -> Res<()> {
    let mut tail = [0u8; 10];
    let end = r.seek(SeekFrom::End(0)).map_err(es)?;
    if end < 10 {
        return Ok(());
    }
    r.seek(SeekFrom::Start(end - 10)).map_err(es)?;
    r.read_exact(&mut tail).map_err(es)?;
    let footer_len = u64::from(u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]));
    if &tail[4..] == b"ARROW1" && footer_len <= ARROW_MAX_METADATA && footer_len + 10 <= end {
        let mut footer = vec![0u8; footer_len as usize];
        r.seek(SeekFrom::Start(end - 10 - footer_len)).map_err(es)?;
        r.read_exact(&mut footer).map_err(es)?;
        if let Some(blocks) = arrow_ipc::root_as_footer(&footer).ok().and_then(|f| f.recordBatches()) {
            if blocks.iter().any(|b| b.bodyLength() > ARROW_MAX_BATCH) {
                return Err(arrow_batch_too_large());
            }
        }
    }
    r.seek(SeekFrom::Start(0)).map_err(es)?;
    Ok(())
}

/// The same for a stream, whose messages say how long their data is.
fn check_arrow_stream<R: Read + Seek>(r: &mut R) -> Res<()> {
    let mut at = 0u64;
    for _ in 0..1024 {
        let mut word = [0u8; 4];
        if r.seek(SeekFrom::Start(at)).is_err() || r.read_exact(&mut word).is_err() {
            break;
        }
        at += 4;
        if word == [0xFF; 4] {
            if r.read_exact(&mut word).is_err() {
                break;
            }
            at += 4;
        }
        let len = u64::from(u32::from_le_bytes(word));
        if len == 0 || len > ARROW_MAX_METADATA {
            break;
        }
        let mut metadata = vec![0u8; len as usize];
        if r.read_exact(&mut metadata).is_err() {
            break;
        }
        let Ok(message) = arrow_ipc::root_as_message(&metadata) else { break };
        if message.bodyLength() > ARROW_MAX_BATCH {
            return Err(arrow_batch_too_large());
        }
        at += len + message.bodyLength().max(0) as u64;
    }
    r.seek(SeekFrom::Start(0)).map_err(es)?;
    Ok(())
}

pub fn arrow_from<R: Read + Seek>(mut r: R) -> Res<TableData> {
    use arrow_array::{Array, RecordBatch};
    use arrow_cast::display::{ArrayFormatter, FormatOptions};
    let mut magic = [0u8; 6];
    let got = r.read(&mut magic).map_err(es)?;
    r.seek(SeekFrom::Start(0)).map_err(es)?;
    if got >= 4 && &magic[..4] == b"FEA1" {
        return Err(t("This is the old Feather V1 format, which the preview does not read."));
    }
    let is_file = got == 6 && &magic == b"ARROW1";
    let mut next_batch: Box<dyn FnMut() -> Option<Result<RecordBatch, String>>>;
    let schema;
    let format;
    if is_file {
        check_arrow_file(&mut r)?;
        let mut reader = arrow_ipc::reader::FileReader::try_new(r, None).map_err(es)?;
        schema = reader.schema();
        format = "Arrow";
        next_batch = Box::new(move || reader.next().map(|b| b.map_err(es)));
    } else {
        check_arrow_stream(&mut r)?;
        let mut reader = arrow_ipc::reader::StreamReader::try_new(r, None).map_err(|_| t("Not a valid Arrow file."))?;
        schema = reader.schema();
        format = "Arrow stream";
        next_batch = Box::new(move || reader.next().map(|b| b.map_err(es)));
    }
    let fields = schema.fields();
    let columns: Vec<String> = fields.iter().take(MAX_COLS).map(|f| f.name().clone()).collect();
    let types: Vec<String> = fields.iter().take(MAX_COLS).map(|f| f.data_type().to_string().to_lowercase()).collect();
    let options = FormatOptions::default();
    let mut rows: Vec<Vec<Cell>> = vec![];
    let mut complete = true;
    while rows.len() < MAX_ROWS {
        let Some(batch) = next_batch() else { break };
        let batch = batch?;
        let formatters: Vec<_> = batch.columns().iter().take(MAX_COLS).map(|c| ArrayFormatter::try_new(c.as_ref(), &options)).collect::<Result<_, _>>().map_err(es)?;
        let take = batch.num_rows().min(MAX_ROWS - rows.len());
        for i in 0..take {
            rows.push(batch.columns().iter().take(MAX_COLS).zip(&formatters).map(|(c, f)| (!c.is_null(i)).then(|| clip_cell(f.value(i).to_string()))).collect());
        }
        if take < batch.num_rows() {
            complete = false;
        }
    }
    complete = complete && next_batch().is_none();
    let total = complete.then_some(rows.len() as u64);
    Ok(TableData { format, info: vec![], sheets: vec![finish_sheet(String::new(), columns, Some(types), rows, total)] })
}

// ---------------------------------------------------------------- HDF5

/// Largest dataset read to show its rows, and the most read for one file.
const H5_MAX_DATASET: u64 = 64 * 1024 * 1024;
const H5_MAX_TOTAL: u64 = 256 * 1024 * 1024;
const H5_MAX_DATASETS: usize = 40;

/// The library reads all of a file into memory, except in a mode that only knows the
/// newest kind of file. So this is the largest file of any kind that can be previewed.
const H5_MAX_BUFFERED: u64 = 768 * 1024 * 1024;

fn h5_collect(group: &hdf5_pure::Group, prefix: &str, depth: usize, visits: &mut usize, out: &mut Vec<String>) {
    // Groups can link back to each other.
    *visits += 1;
    if depth > 32 || out.len() >= 2000 || *visits > 20_000 {
        return;
    }
    for name in group.datasets().unwrap_or_default() {
        out.push(format!("{prefix}{name}"));
    }
    for name in group.groups().unwrap_or_default() {
        if let Ok(child) = group.group(&name) {
            h5_collect(&child, &format!("{prefix}{name}/"), depth + 1, visits, out);
        }
    }
}

/// A type's name for people: compound fields by name, without the library's inner details.
fn h5_type_name(t: &hdf5_pure::DType) -> String {
    use hdf5_pure::DType as D;
    match t {
        D::Compound(fields) => format!("compound ({})", fields.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")),
        D::Enum(_) => "enum".to_string(),
        D::Array(inner, dims) => format!("{}[{}]", h5_type_name(inner), dims.iter().map(u32::to_string).collect::<Vec<_>>().join("×")),
        D::VariableLengthString | D::String => "string".to_string(),
        D::ObjectReference => "reference".to_string(),
        D::Other(_) => "other".to_string(),
        other => other.to_string(),
    }
}

fn h5_type_size(t: &hdf5_pure::DType) -> Option<u64> {
    use hdf5_pure::DType as D;
    Some(match t {
        D::F32 | D::I32 | D::U32 => 4,
        D::F64 | D::I64 | D::U64 => 8,
        D::I8 | D::U8 => 1,
        D::I16 | D::U16 => 2,
        _ => return None,
    })
}

/// The dataset's values in order, as text. `Ok(None)` for types that are not shown.
fn h5_values(d: &hdf5_pure::Dataset, dtype: &hdf5_pure::DType) -> Result<Option<Vec<String>>, hdf5_pure::Error> {
    use hdf5_pure::DType as D;
    fn text<T: ToString>(v: Vec<T>) -> Vec<String> {
        v.iter().map(T::to_string).collect()
    }
    Ok(Some(match dtype {
        D::F32 => text(d.read_f32()?),
        D::F64 => text(d.read_f64()?),
        D::I8 => text(d.read_i8()?),
        D::I16 => text(d.read_i16()?),
        D::I32 => text(d.read_i32()?),
        D::I64 => text(d.read_i64()?),
        D::U8 => text(d.read_u8()?),
        D::U16 => text(d.read_u16()?),
        D::U32 => text(d.read_u32()?),
        D::U64 => text(d.read_u64()?),
        D::String | D::VariableLengthString => d.read_string()?,
        _ => return Ok(None),
    }))
}

pub fn hdf5_table(file: &hdf5_pure::File) -> Res<TableData> {
    let mut paths = vec![];
    h5_collect(&file.root(), "", 0, &mut 0, &mut paths);
    let mut contents: Vec<Vec<Cell>> = vec![];
    let mut sheets = vec![];
    let mut budget = H5_MAX_TOTAL;
    for path in &paths {
        let Ok(d) = file.dataset(path) else {
            contents.push(vec![Some(path.clone()), None, None, Some(t("Cannot be read"))]);
            continue;
        };
        let shape = d.shape().unwrap_or_default();
        let dtype = d.dtype().ok();
        let type_text = dtype.as_ref().map_or_else(String::new, h5_type_name);
        let shape_text = if shape.is_empty() { t("scalar") } else { shape.iter().map(u64::to_string).collect::<Vec<_>>().join(" × ") };
        let count: u64 = shape.iter().product::<u64>().max(u64::from(shape.is_empty()));
        // Strings have no fixed size here; a generous guess keeps huge ones from being read.
        let bytes = dtype.as_ref().map_or(0, |t| count.saturating_mul(h5_type_size(t).unwrap_or(16)));
        let shown = sheets.len() < H5_MAX_DATASETS && bytes <= H5_MAX_DATASET && bytes <= budget;
        let read = if shown { dtype.as_ref().map(|t| h5_values(&d, t)) } else { None };
        let note = match &read {
            Some(Ok(Some(_))) => format!("{} {}", t("Tab:"), path),
            Some(Ok(None)) => t("This type is not shown"),
            Some(Err(e)) => format!("{}: {e}", t("Cannot be read")),
            None => t("Too large to preview"),
        };
        let values = read.and_then(Result::ok).flatten();
        contents.push(vec![Some(path.clone()), Some(shape_text), Some(type_text.clone()), Some(note)]);
        let Some(values) = values else { continue };
        budget -= bytes;
        // Rows are the first axis; the remaining axes become columns.
        let n_rows = shape.first().copied().unwrap_or(1).max(1) as usize;
        let per_row = (values.len() / n_rows).max(1);
        let shown_cols = per_row.min(MAX_COLS);
        let columns: Vec<String> = if per_row == 1 { vec!["value".to_string()] } else { (0..shown_cols).map(|c| c.to_string()).collect() };
        let rows: Vec<Vec<Cell>> = values.chunks(per_row).take(MAX_ROWS).map(|row| row.iter().take(shown_cols).map(|v| Some(clip_cell(v.clone()))).collect()).collect();
        sheets.push(finish_sheet(path.clone(), columns, Some(vec![type_text; shown_cols]), rows, Some(n_rows as u64)));
    }
    if paths.is_empty() {
        return Ok(TableData { format: "HDF5", info: vec![t("No datasets")], sheets: vec![finish_sheet(t("Contents"), vec![], None, vec![], Some(0))] });
    }
    let names = vec![t("Dataset"), t("Shape"), t("Type"), t("Note")];
    let overview = finish_sheet(t("Contents"), names, Some(vec!["text".into(); 4]), contents, Some(paths.len() as u64));
    sheets.insert(0, overview);
    Ok(TableData { format: "HDF5", info: vec![format!("{} {}", paths.len(), t("datasets"))], sheets })
}

pub fn hdf5_from_path(path: &Path) -> Res<TableData> {
    if std::fs::metadata(path).map_err(es)?.len() <= H5_MAX_BUFFERED {
        let file = hdf5_pure::File::open(path).map_err(|e| format!("HDF5: {e}"))?;
        return hdf5_table(&file);
    }
    let too_large = || t("This HDF5 file is too large to preview: only files in the newest format can be read in place.");
    let file = hdf5_pure::File::open_streaming(path).map_err(|_| too_large())?;
    let table = hdf5_table(&file)?;
    // Older files open, and then turn out to have no readable groups.
    if table.sheets.len() == 1 && table.sheets[0].rows.is_empty() {
        return Err(too_large());
    }
    Ok(table)
}

pub fn hdf5_from_bytes(bytes: Vec<u8>) -> Res<TableData> {
    let file = hdf5_pure::File::from_bytes(bytes).map_err(|e| format!("HDF5: {e}"))?;
    hdf5_table(&file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn cell(s: &Sheet, r: usize, c: usize) -> Option<&str> {
        s.rows[r][c].as_deref()
    }

    #[test]
    fn jsonl_columns_from_all_lines() {
        let text = "{\"id\":1,\"name\":\"a\"}\n\n{\"id\":2,\"nested\":{\"x\":[1,2]},\"none\":null}\nnot json\n7\n";
        let d = jsonl_from(Cursor::new(text)).unwrap();
        let s = &d.sheets[0];
        assert_eq!(s.columns, ["id", "name", "nested", "none", "value"]);
        assert_eq!(s.rows.len(), 3);
        assert_eq!(cell(s, 0, 1), Some("a"));
        assert_eq!(cell(s, 1, 1), None, "missing in this line");
        assert_eq!(cell(s, 1, 2), Some("{\"x\":[1,2]}"));
        assert_eq!(cell(s, 2, 4), Some("7"));
        assert_eq!(s.total_rows, Some(3));
        assert!(d.info[0].starts_with("1 "), "{:?}", d.info);
        assert!(jsonl_from(Cursor::new("nope\nnada\n")).is_err());
    }

    #[test]
    fn jsonl_stops_at_the_row_limit() {
        let text: String = (0..MAX_ROWS + 5).map(|i| format!("{{\"i\":{i}}}\n")).collect();
        let s = jsonl_from(Cursor::new(text)).unwrap().sheets.remove(0);
        assert_eq!(s.rows.len(), MAX_ROWS);
        assert_eq!(s.total_rows, None);
    }

    fn npy(descr: &str, fortran: bool, shape: &str, data: &[u8]) -> Vec<u8> {
        let mut header = format!("{{'descr': {descr}, 'fortran_order': {}, 'shape': {shape}, }}", if fortran { "True" } else { "False" });
        while (10 + header.len() + 1) % 64 != 0 {
            header.push(' ');
        }
        header.push('\n');
        let mut f = b"\x93NUMPY\x01\x00".to_vec();
        f.extend((header.len() as u16).to_le_bytes());
        f.extend(header.as_bytes());
        f.extend_from_slice(data);
        f
    }

    #[test]
    fn npy_shapes_and_orders() {
        let floats: Vec<u8> = [1.5f64, 2.5].iter().flat_map(|f| f.to_le_bytes()).collect();
        let s = npy_from(Cursor::new(npy("'<f8'", false, "(2,)", &floats))).unwrap().sheets.remove(0);
        assert_eq!(s.columns, ["value"]);
        assert_eq!(cell(&s, 1, 0), Some("2.5"));
        let big: Vec<u8> = (0..6i32).flat_map(|i| i.to_be_bytes()).collect();
        let s = npy_from(Cursor::new(npy("'>i4'", false, "(2, 3)", &big))).unwrap().sheets.remove(0);
        assert_eq!(s.columns.len(), 3);
        assert_eq!(cell(&s, 1, 2), Some("5"));
        // Fortran order: 0 2 4 / 1 3 5.
        let le: Vec<u8> = (0..6i64).flat_map(|i| i.to_le_bytes()).collect();
        let s = npy_from(Cursor::new(npy("'<i8'", true, "(2, 3)", &le))).unwrap().sheets.remove(0);
        assert_eq!(cell(&s, 0, 1), Some("2"));
        assert_eq!(cell(&s, 1, 2), Some("5"));
        // Three axes: the last two become columns.
        let s = npy_from(Cursor::new(npy("'|u1'", false, "(2, 2, 2)", &[0, 1, 2, 3, 4, 5, 6, 7]))).unwrap().sheets.remove(0);
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.columns.len(), 4);
        assert_eq!(cell(&s, 1, 3), Some("7"));
    }

    #[test]
    fn npy_kinds() {
        let s = npy_from(Cursor::new(npy("'|b1'", false, "(2,)", &[1, 0]))).unwrap().sheets.remove(0);
        assert_eq!(cell(&s, 0, 0), Some("true"));
        let s = npy_from(Cursor::new(npy("'<i2'", false, "(1,)", &(-5i16).to_le_bytes()))).unwrap().sheets.remove(0);
        assert_eq!(cell(&s, 0, 0), Some("-5"), "the sign survives");
        let s = npy_from(Cursor::new(npy("'|S3'", false, "(1,)", b"ab\0"))).unwrap().sheets.remove(0);
        assert_eq!(cell(&s, 0, 0), Some("ab"));
        let text: Vec<u8> = ['h', 'é', '\0'].iter().flat_map(|c| (*c as u32).to_le_bytes()).collect();
        let s = npy_from(Cursor::new(npy("'<U3'", false, "(1,)", &text))).unwrap().sheets.remove(0);
        assert_eq!(cell(&s, 0, 0), Some("hé"));
        let c: Vec<u8> = [1.0f64, -2.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let s = npy_from(Cursor::new(npy("'<c16'", false, "(1,)", &c))).unwrap().sheets.remove(0);
        assert_eq!(cell(&s, 0, 0), Some("1-2i"));
        assert_eq!(half_to_f64(0x3C00), 1.0);
        let mut rec = 7i32.to_le_bytes().to_vec();
        rec.extend(2.5f64.to_le_bytes());
        let s = npy_from(Cursor::new(npy("[('a', '<i4'), ('b', '<f8')]", false, "(1,)", &rec))).unwrap().sheets.remove(0);
        assert_eq!(s.columns, ["a", "b"]);
        assert_eq!(cell(&s, 0, 1), Some("2.5"));
        // A scalar array.
        let s = npy_from(Cursor::new(npy("'<f8'", false, "()", &1.0f64.to_le_bytes()))).unwrap().sheets.remove(0);
        assert_eq!(s.rows.len(), 1);
    }

    #[test]
    fn jsonl_without_line_ends_stops_at_the_limit() {
        struct Endless;
        impl Read for Endless {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                b.fill(b'x');
                Ok(b.len())
            }
        }
        assert!(jsonl_from(Endless).is_err());
        // Good lines before it still show.
        let good = Cursor::new(b"{\"a\":1}\n".to_vec());
        let s = jsonl_from(good.chain(Endless)).unwrap();
        assert_eq!(s.sheets[0].rows.len(), 1);
        assert_eq!(s.sheets[0].total_rows, None);
        assert!(s.info[0].contains("long line"), "{:?}", s.info);
    }

    #[test]
    fn npy_with_absurd_numbers_is_refused() {
        assert!(npy_from(Cursor::new(npy("'<U2000000000'", false, "(3,)", &[]))).err().unwrap().contains("too large"));
        assert!(npy_from(Cursor::new(npy("'<U99999999999999999999'", false, "(3,)", &[]))).is_err());
        let overflow = npy("'<f8'", false, "(99999999999999, 99999999999999, 99999999999999)", &[]);
        assert!(npy_from(Cursor::new(overflow)).is_err());
        // Zero rows or columns are fine.
        assert_eq!(npy_from(Cursor::new(npy("'<f8'", false, "(0,)", &[]))).unwrap().sheets[0].rows.len(), 0);
        assert_eq!(npy_from(Cursor::new(npy("'<f8'", false, "(2, 0)", &[]))).unwrap().sheets[0].columns.len(), 0);
    }

    #[test]
    fn arrow_batches_too_large_to_read_are_refused() {
        use arrow_array::{ArrayRef, Int32Array, RecordBatch};
        use std::sync::Arc;
        let batch = RecordBatch::try_from_iter([("id", Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5, 6, 7])) as ArrayRef)]).unwrap();
        let mut stream = vec![];
        {
            let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut stream, &batch.schema()).unwrap();
            w.write(&batch).unwrap();
            w.finish().unwrap();
        }
        // Walk to the message with data, and overwrite the length it states.
        let mut at = 0;
        loop {
            let len = u32::from_le_bytes(stream[at + 4..at + 8].try_into().unwrap()) as usize;
            let metadata = stream[at + 8..at + 8 + len].to_vec();
            let body = arrow_ipc::root_as_message(&metadata).unwrap().bodyLength();
            if body > 0 {
                let found = metadata.windows(8).position(|w| w == body.to_le_bytes()).expect("the length is in the message");
                stream[at + 8 + found..at + 16 + found].copy_from_slice(&(ARROW_MAX_BATCH + 1).to_le_bytes());
                break;
            }
            at += 8 + len;
        }
        assert!(arrow_from(Cursor::new(stream)).err().unwrap().contains("too large"));
    }

    #[test]
    fn npy_never_reads_python_objects_and_reports_bad_files() {
        let e = npy_from(Cursor::new(npy("'|O'", false, "(1,)", &[0x80; 16]))).err().unwrap();
        assert!(e.contains("Python"), "{e}");
        assert!(npy_from(Cursor::new(b"nope".to_vec())).is_err());
        assert!(npy_from(Cursor::new(npy("'<f8'", false, "(9,)", &[0; 8]))).is_err(), "cut short");
    }

    #[test]
    fn sqlite_tables_views_and_blobs() {
        let dir = crate::media::TempDir::new().unwrap();
        let path = dir.path().join("a b#c?.sqlite");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE t(id INTEGER, name TEXT, pic BLOB); CREATE VIEW v AS SELECT name FROM t; CREATE TABLE \"odd \"\"name\"(x);").unwrap();
        for i in 0..MAX_ROWS + 10 {
            c.execute("INSERT INTO t VALUES(?1, ?2, x'0102')", rusqlite::params![i as i64, format!("n{i}")]).unwrap();
        }
        drop(c);
        let before = std::fs::read_dir(dir.path()).unwrap().count();
        let d = sqlite_from_path(&path).unwrap();
        let names: Vec<&str> = d.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["t", "v (view)", "odd \"name"], "empty tables last");
        let t = &d.sheets[0];
        assert_eq!(t.columns, ["id", "name", "pic"]);
        assert_eq!(t.rows.len(), MAX_ROWS);
        assert_eq!(t.total_rows, Some(MAX_ROWS as u64 + 10));
        assert_eq!(cell(t, 3, 1), Some("n3"));
        assert!(cell(t, 0, 2).unwrap().starts_with("<binary"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), before, "nothing created next to the file");
        let junk = dir.path().join("junk.sqlite");
        std::fs::write(&junk, b"this is not a database at all, not even close").unwrap();
        assert!(sqlite_from_path(&junk).is_err());
    }

    #[test]
    fn hdf5_contents_and_datasets() {
        let mut b = hdf5_pure::FileBuilder::new();
        b.create_dataset("flat").with_f64_data(&[1.5, 2.5, 3.5]).with_shape(&[3]);
        b.create_dataset("grid").with_i32_data(&[1, 2, 3, 4, 5, 6]).with_shape(&[2, 3]);
        let d = hdf5_from_bytes(b.finish().unwrap()).unwrap();
        assert_eq!(d.format, "HDF5");
        let contents = &d.sheets[0];
        assert_eq!(contents.name, "Contents");
        assert_eq!(contents.rows.len(), 2);
        let flat = d.sheets.iter().find(|s| s.name == "flat").unwrap();
        assert_eq!(cell(flat, 2, 0), Some("3.5"));
        let grid = d.sheets.iter().find(|s| s.name == "grid").unwrap();
        assert_eq!(grid.columns.len(), 3);
        assert_eq!(grid.rows.len(), 2);
        assert_eq!(cell(grid, 1, 2), Some("6"));
        assert!(hdf5_from_bytes(b"not hdf5".to_vec()).is_err());
    }

    #[test]
    fn arrow_file_and_stream() {
        use arrow_array::{ArrayRef, Int32Array, RecordBatch, StringArray};
        use std::sync::Arc;
        let batch = RecordBatch::try_from_iter([
            ("id", Arc::new(Int32Array::from(vec![1, 2, 3])) as ArrayRef),
            ("name", Arc::new(StringArray::from(vec![Some("a"), None, Some("c")])) as ArrayRef),
        ])
        .unwrap();
        let mut file = vec![];
        {
            let mut w = arrow_ipc::writer::FileWriter::try_new(&mut file, &batch.schema()).unwrap();
            w.write(&batch).unwrap();
            w.finish().unwrap();
        }
        let d = arrow_from(Cursor::new(file)).unwrap();
        assert_eq!(d.format, "Arrow");
        let s = &d.sheets[0];
        assert_eq!(s.columns, ["id", "name"]);
        assert_eq!(cell(s, 2, 1), Some("c"));
        assert_eq!(cell(s, 1, 1), None);
        assert_eq!(s.total_rows, Some(3));
        let mut stream = vec![];
        {
            let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut stream, &batch.schema()).unwrap();
            w.write(&batch).unwrap();
            w.finish().unwrap();
        }
        assert_eq!(arrow_from(Cursor::new(stream)).unwrap().format, "Arrow stream");
        assert!(arrow_from(Cursor::new(b"FEA1\0\0\0\0".to_vec())).err().unwrap().contains("V1"));
        assert!(arrow_from(Cursor::new(b"garbage".to_vec())).is_err());
    }
}
