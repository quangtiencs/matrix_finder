//! What scientific and statistical data files hold, without reading their data:
//! MATLAB (`.mat`), NetCDF (`.nc`), ORC, Stata (`.dta`), SPSS (`.sav`, `.zsav`), SAS
//! (`.sas7bdat`) and DuckDB. Each lists its variables or columns (name, shape or type,
//! label) from the file's header, so a file of any size opens at once. A header that
//! is damaged, or larger than any real one, is refused with a message instead.
use crate::i18n::t;
use crate::table::{clip_cell, finish_sheet, fmt_count, Cell, Sheet, TableData, MAX_ROWS};
use crate::util::{es, human_size, Res};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Most variables or columns listed from one file (more are counted, not listed).
const MAX_VARIABLES: usize = 100_000;
/// Most bytes of header read from one file.
const MAX_HEADER: u64 = 64 * 1024 * 1024;
/// A file whose description takes longer than this to read is shown in part.
const MAX_TIME: std::time::Duration = std::time::Duration::from_secs(3);

fn too_complex() -> String {
    t("This file is too complex to preview.")
}

fn cut_short() -> String {
    t("This file is cut short.")
}

/// A sheet of text columns.
fn text_sheet(title: String, columns: Vec<String>, rows: Vec<Vec<String>>) -> Sheet {
    let total = rows.len() as u64;
    let types = Some(vec!["text".to_string(); columns.len()]);
    let rows: Vec<Vec<Cell>> = rows.into_iter().take(MAX_ROWS).map(|r| r.into_iter().map(|c| (!c.is_empty()).then(|| clip_cell(c))).collect()).collect();
    finish_sheet(title, columns, types, rows, Some(total))
}

fn key_values(title: String, pairs: Vec<(String, String)>) -> Sheet {
    text_sheet(title, vec![t("Key"), t("Value")], pairs.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| vec![k, v]).collect())
}

/// Text from a fixed-size field: up to its first NUL, without padding.
fn field_text(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

fn shape_text(dims: &[u64]) -> String {
    if dims.is_empty() { t("scalar") } else { dims.iter().map(u64::to_string).collect::<Vec<_>>().join(" × ") }
}

/// Reads numbers of either byte order, refusing to go past the end of the file.
struct Bin<R> {
    r: R,
    little: bool,
    len: u64,
}

impl<R: Read + Seek> Bin<R> {
    fn new(mut r: R) -> Res<Self> {
        let len = r.seek(SeekFrom::End(0)).map_err(es)?;
        r.seek(SeekFrom::Start(0)).map_err(es)?;
        Ok(Self { r, little: true, len })
    }

    fn pos(&mut self) -> Res<u64> {
        self.r.stream_position().map_err(es)
    }

    fn seek(&mut self, at: u64) -> Res<()> {
        if at > self.len {
            return Err(cut_short());
        }
        self.r.seek(SeekFrom::Start(at)).map_err(es)?;
        Ok(())
    }

    fn bytes(&mut self, n: u64) -> Res<Vec<u8>> {
        if n > MAX_HEADER || self.pos()?.saturating_add(n) > self.len {
            return Err(if n > MAX_HEADER { too_complex() } else { cut_short() });
        }
        let mut b = vec![0u8; n as usize];
        self.r.read_exact(&mut b).map_err(|_| cut_short())?;
        Ok(b)
    }

    fn array<const N: usize>(&mut self) -> Res<[u8; N]> {
        let mut b = [0u8; N];
        self.r.read_exact(&mut b).map_err(|_| cut_short())?;
        if !self.little {
            b.reverse();
        }
        Ok(b)
    }

    fn u8(&mut self) -> Res<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Res<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Res<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Res<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Res<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn f64(&mut self) -> Res<f64> {
        Ok(f64::from_le_bytes(self.array()?))
    }

    fn skip(&mut self, n: u64) -> Res<()> {
        let at = self.pos()?.checked_add(n).ok_or_else(cut_short)?;
        self.seek(at)
    }
}

// ---------------------------------------------------------------- HDF5-based (MATLAB 7.3, NetCDF-4)

/// The datasets of an HDF5 file with their shapes and types, nothing of their data.
fn hdf5_listing(file: &hdf5_pure::File, format: &'static str) -> TableData {
    let (paths, rows) = crate::scidata::hdf5_datasets(file);
    let info = vec![format!("{} {}", fmt_count(paths as u64), t("variables")), "HDF5".to_string()];
    TableData { format, info, sheets: vec![text_sheet(t("Variables"), vec![t("Name"), t("Shape"), t("Type")], rows)] }
}

fn hdf5_path(path: &Path, format: &'static str) -> Res<TableData> {
    Ok(hdf5_listing(&crate::scidata::hdf5_open(path)?, format))
}

fn hdf5_bytes(bytes: Vec<u8>, format: &'static str) -> Res<TableData> {
    let file = crate::scidata::hdf5_from_memory(bytes)?;
    Ok(hdf5_listing(&file, format))
}

const HDF5_MAGIC: &[u8] = b"\x89HDF\r\n\x1a\n";

// ---------------------------------------------------------------- MATLAB

fn mat_class(class: u32) -> &'static str {
    match class {
        1 => "cell",
        2 => "struct",
        3 => "object",
        4 => "char",
        5 => "sparse",
        6 => "double",
        7 => "single",
        8 => "int8",
        9 => "uint8",
        10 => "int16",
        11 => "uint16",
        12 => "int32",
        13 => "uint32",
        14 => "int64",
        15 => "uint64",
        16 => "function",
        17 => "object (opaque)",
        _ => "?",
    }
}

/// One element inside a matrix: its type, its data and where the next one starts.
fn mat_element(b: &[u8], at: usize, little: bool) -> Option<(u32, &[u8], usize)> {
    let word = |i: usize| b.get(i..i + 4).map(|w| if little { u32::from_le_bytes(w.try_into().unwrap()) } else { u32::from_be_bytes(w.try_into().unwrap()) });
    let first = word(at)?;
    if first >> 16 != 0 {
        // Small data element: type, size and up to 4 bytes of data in 8 bytes.
        let size = (first >> 16) as usize;
        return Some((first & 0xFFFF, b.get(at + 4..at + 4 + size.min(4))?, at + 8));
    }
    let size = word(at + 4)? as usize;
    let data = b.get(at + 8..at.checked_add(8)?.checked_add(size)?)?;
    Some((first, data, at + 8 + size.div_ceil(8) * 8))
}

/// Name, shape, class and notes of a matrix element's body.
fn mat_matrix(b: &[u8], little: bool) -> Option<(String, Option<Vec<u64>>, String, String)> {
    let int = |w: &[u8]| if little { i32::from_le_bytes(w.try_into().unwrap()) } else { i32::from_be_bytes(w.try_into().unwrap()) };
    let (_, flags, next) = mat_element(b, 0, little)?;
    let flags = u32::try_from(int(flags.get(..4)?)).ok()?;
    let class_code = flags & 0xFF;
    let mut class = mat_class(class_code).to_string();
    if flags & 0x200 != 0 {
        class = "logical".into();
    }
    if flags & 0x800 != 0 {
        class = format!("complex {class}");
    }
    if flags & 0x400 != 0 {
        class = format!("global {class}");
    }
    if class_code == 17 {
        let (_, name, _) = mat_element(b, next, little)?;
        return Some((field_text(name), None, class, String::new()));
    }
    let (_, dims, next) = mat_element(b, next, little)?;
    let dims: Vec<u64> = dims.chunks_exact(4).map(|w| int(w).max(0) as u64).collect();
    let (_, name, mut next) = mat_element(b, next, little)?;
    let name = field_text(name);
    let mut note = String::new();
    if class_code == 2 || class_code == 3 {
        if class_code == 3 {
            let (_, class_name, after) = mat_element(b, next, little)?;
            note = field_text(class_name);
            next = after;
        }
        // Field names: their padded length, then all of them back to back.
        if let Some((_, len, after)) = mat_element(b, next, little) {
            let len = len.get(..4).map(int).unwrap_or(0).max(0) as usize;
            if let (true, Some((_, names, _))) = (len > 0, mat_element(b, after, little)) {
                let fields: Vec<String> = names.chunks(len).map(field_text).filter(|f| !f.is_empty()).take(30).collect();
                let more = if names.len() / len > 30 { ", …" } else { "" };
                let fields = format!("{}: {}{more}", t("fields"), fields.join(", "));
                note = if note.is_empty() { fields } else { format!("{note} · {fields}") };
            }
        }
    }
    Some((name, Some(dims), class, note))
}

/// A MATLAB file: version 7.3 is HDF5; versions 5 to 7 are a list of elements, each a
/// variable, possibly packed with zlib. Only the start of each one is read.
fn mat_v5<R: Read + Seek>(r: R) -> Res<TableData> {
    let not_mat = || t("Not a MATLAB file this preview can read.");
    let mut f = Bin::new(r)?;
    let head = f.bytes(128).map_err(|_| not_mat())?;
    if !head.starts_with(b"MATLAB 5.0 MAT-file") && !head.starts_with(b"MATLAB 7") {
        return Err(not_mat());
    }
    let little = match &head[126..128] {
        b"IM" => true,
        b"MI" => false,
        _ => return Err(not_mat()),
    };
    f.little = little;
    let description = field_text(&head[..116]);
    let mut rows = vec![];
    let (mut count, mut packed) = (0usize, false);
    let mut at = 128u64;
    let (started, mut elements, mut truncated) = (std::time::Instant::now(), 0usize, false);
    while at + 8 <= f.len {
        elements += 1;
        if elements > MAX_VARIABLES * 2 || started.elapsed() > MAX_TIME {
            truncated = true;
            break;
        }
        f.seek(at)?;
        let kind = f.u32()?;
        let size = u64::from(f.u32()?);
        if kind >> 16 != 0 {
            at += 8;
            continue;
        }
        // Enough of a variable to know its name, shape and fields.
        const HEAD: u64 = 16 * 1024;
        let body = match kind {
            14 => f.bytes(size.min(HEAD).min(f.len - at - 8))?,
            15 => {
                packed = true;
                let mut out = Vec::new();
                let decoder = flate2::read::ZlibDecoder::new((&mut f.r).take(size));
                let _ = decoder.take(HEAD).read_to_end(&mut out);
                // The packed data is itself a matrix element, of which only the start is here.
                let tag = out.get(..4).map(|w| if little { u32::from_le_bytes(w.try_into().unwrap()) } else { u32::from_be_bytes(w.try_into().unwrap()) });
                if tag == Some(14) { out.get(8..).unwrap_or_default().to_vec() } else { vec![] }
            }
            _ => vec![],
        };
        if kind == 14 || kind == 15 {
            if let Some((name, dims, class, note)) = mat_matrix(&body, little) {
                // MATLAB keeps a hidden variable for function handles' workspaces.
                if !name.is_empty() && !name.starts_with("__") {
                    count += 1;
                    if rows.len() < MAX_VARIABLES {
                        rows.push(vec![name, dims.as_deref().map(shape_text).unwrap_or_else(|| "?".into()), class, human_size(size), note]);
                    }
                }
            }
        }
        at = at.saturating_add(8).saturating_add(if kind == 15 { size } else { size.div_ceil(8) * 8 });
    }
    let mut info = vec![format!("{} {}", fmt_count(count as u64), t("variables")), if packed { "v7".into() } else { "v5".into() }];
    if truncated {
        info.push(t("(truncated)"));
    }
    let sheets = vec![
        text_sheet(t("Variables"), vec![t("Name"), t("Shape"), t("Class"), t("Size"), t("Note")], rows),
        key_values(t("Metadata"), vec![(t("Header"), description)]),
    ];
    Ok(TableData { format: "MATLAB", info, sheets })
}

fn is_mat_73(head: &[u8]) -> bool {
    head.starts_with(b"MATLAB 7.3 MAT-file") || head.starts_with(HDF5_MAGIC)
}

pub fn mat_path(path: &Path) -> Res<TableData> {
    let mut head = [0u8; 20];
    let n = crate::util::open_regular(path).map_err(es)?.read(&mut head).map_err(es)?;
    if is_mat_73(&head[..n]) {
        return hdf5_path(path, "MATLAB 7.3");
    }
    mat_v5(std::io::BufReader::new(crate::util::open_regular(path).map_err(es)?))
}

pub fn mat_bytes(bytes: Vec<u8>) -> Res<TableData> {
    if is_mat_73(&bytes) {
        return hdf5_bytes(bytes, "MATLAB 7.3");
    }
    mat_v5(std::io::Cursor::new(bytes))
}

// ---------------------------------------------------------------- NetCDF

/// The size of a NetCDF type's items, and its name.
fn nc_type(code: u32) -> Option<(u64, &'static str)> {
    Some(match code {
        1 => (1, "byte"),
        2 => (1, "char"),
        3 => (2, "short"),
        4 => (4, "int"),
        5 => (4, "float"),
        6 => (8, "double"),
        7 => (1, "ubyte"),
        8 => (2, "ushort"),
        9 => (4, "uint"),
        10 => (8, "int64"),
        11 => (8, "uint64"),
        _ => return None,
    })
}

struct Nc<R> {
    f: Bin<R>,
    /// CDF-5 counts in 64 bits.
    big: bool,
}

impl<R: Read + Seek> Nc<R> {
    fn count(&mut self) -> Res<u64> {
        let n = if self.big { self.f.u64()? } else { u64::from(self.f.u32()?) };
        if n > self.f.len {
            return Err(too_complex());
        }
        Ok(n)
    }

    fn name(&mut self) -> Res<String> {
        let n = self.count()?;
        let b = self.f.bytes(n.div_ceil(4) * 4)?;
        Ok(String::from_utf8_lossy(&b[..n as usize]).into_owned())
    }

    /// A list's tag and length (`ABSENT` is two zeros).
    fn list(&mut self, tag: u32) -> Res<u64> {
        if self.f.pos()? > MAX_HEADER {
            return Err(too_complex());
        }
        let found = self.f.u32()?;
        let n = self.count()?;
        if found == 0 && n == 0 {
            return Ok(0);
        }
        if found != tag || n > MAX_VARIABLES as u64 {
            return Err(too_complex());
        }
        Ok(n)
    }

    /// Attributes as (name, value text): text as it is, numbers for the first few.
    fn attributes(&mut self) -> Res<Vec<(String, String)>> {
        let n = self.list(0x0C)?;
        let mut out = vec![];
        for _ in 0..n {
            let name = self.name()?;
            let kind = self.f.u32()?;
            let (size, _) = nc_type(kind).ok_or_else(too_complex)?;
            let count = self.count()?;
            let bytes = count.checked_mul(size).ok_or_else(too_complex)?;
            let padded = bytes.div_ceil(4) * 4;
            let value = if kind == 2 && bytes <= 64 * 1024 {
                field_text(&self.f.bytes(padded)?)
            } else if kind != 2 && count <= 16 {
                let raw = self.f.bytes(padded)?;
                let items: Vec<String> = raw[..bytes as usize]
                    .chunks_exact(size as usize)
                    .map(|c| {
                        let be = |n: usize| {
                            let mut a = [0u8; 8];
                            a[8 - n..].copy_from_slice(c);
                            u64::from_be_bytes(a)
                        };
                        match kind {
                            1 => (c[0] as i8).to_string(),
                            3 => (be(2) as u16 as i16).to_string(),
                            4 => (be(4) as u32 as i32).to_string(),
                            5 => f32::from_bits(be(4) as u32).to_string(),
                            6 => f64::from_bits(be(8)).to_string(),
                            10 => (be(8) as i64).to_string(),
                            _ => be(size as usize).to_string(),
                        }
                    })
                    .collect();
                items.join(", ")
            } else {
                self.f.skip(padded)?;
                format!("({} {})", fmt_count(count), t("items"))
            };
            out.push((name, value));
        }
        Ok(out)
    }
}

/// A NetCDF file: the classic formats are read from their header; NetCDF-4 is HDF5.
fn netcdf_classic<R: Read + Seek>(r: R) -> Res<TableData> {
    let not_nc = || t("Not a NetCDF file this preview can read.");
    let mut f = Bin::new(r)?;
    f.little = false;
    let magic = f.bytes(4).map_err(|_| not_nc())?;
    if &magic[..3] != b"CDF" || !matches!(magic[3], 1 | 2 | 5) {
        return Err(not_nc());
    }
    let version = magic[3];
    let mut nc = Nc { f, big: version == 5 };
    let records = if nc.big { nc.f.u64()? } else { u64::from(nc.f.u32()?) };
    let records = (records != 0xFFFF_FFFF && records != u64::MAX).then_some(records);
    let n_dims = nc.list(0x0A)?;
    let mut dims = vec![];
    for _ in 0..n_dims {
        let name = nc.name()?;
        let len = nc.count()?;
        dims.push((name, len));
    }
    let globals = nc.attributes()?;
    let n_vars = nc.list(0x0B)?;
    let mut rows = vec![];
    for _ in 0..n_vars {
        let name = nc.name()?;
        let n = nc.count()?;
        if n > 1024 {
            return Err(too_complex());
        }
        let mut shape = vec![];
        let mut dim_names = vec![];
        for _ in 0..n {
            let id = nc.count()? as usize;
            let (dim, len) = dims.get(id).cloned().unwrap_or_else(|| ("?".into(), 0));
            // The record dimension's length is the number of records.
            let len = if len == 0 { records.unwrap_or(0) } else { len };
            shape.push(len);
            dim_names.push(dim);
        }
        let attrs = nc.attributes()?;
        let kind = nc.f.u32()?;
        let _vsize = nc.count()?;
        if version == 1 { nc.f.u32()?; } else { nc.f.u64()?; }
        let attr = |key: &str| attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_default();
        let units = attr("units");
        let label = [attr("long_name"), if units.is_empty() { units } else { format!("[{units}]") }].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        let type_name = nc_type(kind).map_or("?", |(_, n)| n);
        rows.push(vec![name, shape_text(&shape), dim_names.join(", "), type_name.to_string(), label]);
    }
    let format = match version {
        1 => "NetCDF classic",
        2 => "NetCDF 64-bit offset",
        _ => "NetCDF CDF-5",
    };
    let mut info = vec![format!("{} {}", fmt_count(rows.len() as u64), t("variables")), format!("{} {}", fmt_count(dims.len() as u64), t("dimensions"))];
    if let Some(r) = records {
        info.push(format!("{} {}", fmt_count(r), t("records")));
    }
    let dim_rows = dims.iter().map(|(n, l)| vec![n.clone(), if *l == 0 { format!("{} ({})", records.unwrap_or(0), t("unlimited")) } else { l.to_string() }]).collect();
    let sheets = vec![
        text_sheet(t("Variables"), vec![t("Name"), t("Shape"), t("Dimensions"), t("Type"), t("Label")], rows),
        text_sheet(t("Dimensions"), vec![t("Name"), t("Length")], dim_rows),
        key_values(t("Attributes"), globals),
    ];
    Ok(TableData { format, info, sheets })
}

pub fn netcdf_path(path: &Path) -> Res<TableData> {
    let mut head = [0u8; 8];
    let n = crate::util::open_regular(path).map_err(es)?.read(&mut head).map_err(es)?;
    if head[..n].starts_with(HDF5_MAGIC) {
        return hdf5_path(path, "NetCDF-4");
    }
    netcdf_classic(std::io::BufReader::new(crate::util::open_regular(path).map_err(es)?))
}

pub fn netcdf_bytes(bytes: Vec<u8>) -> Res<TableData> {
    if bytes.starts_with(HDF5_MAGIC) {
        return hdf5_bytes(bytes, "NetCDF-4");
    }
    netcdf_classic(std::io::Cursor::new(bytes))
}

// ---------------------------------------------------------------- ORC

/// Fields of a protobuf message: (number, wire type, value or bytes).
fn pb_fields(mut b: &[u8]) -> impl Iterator<Item = (u64, u8, u64, &[u8])> {
    fn varint(b: &mut &[u8]) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = b.split_first()?;
            *b = rest;
            v |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    std::iter::from_fn(move || {
        let key = varint(&mut b)?;
        let (field, wire) = (key >> 3, (key & 7) as u8);
        match wire {
            0 => Some((field, wire, varint(&mut b)?, &[][..])),
            1 | 5 => {
                let n = if wire == 1 { 8 } else { 4 };
                let (v, rest) = (b.get(..n)?, b.get(n..)?);
                b = rest;
                Some((field, wire, 0, v))
            }
            2 => {
                let n = usize::try_from(varint(&mut b)?).ok()?;
                let (v, rest) = (b.get(..n)?, b.get(n..)?);
                b = rest;
                Some((field, wire, n as u64, v))
            }
            _ => None,
        }
    })
}

/// Unpacked varints of a packed repeated field.
fn pb_packed(b: &[u8]) -> Vec<u64> {
    let mut out = vec![];
    let mut v = 0u64;
    let mut shift = 0;
    for byte in b {
        v |= u64::from(byte & 0x7F) << shift.min(63);
        shift += 7;
        if byte & 0x80 == 0 {
            out.push(v);
            v = 0;
            shift = 0;
        }
    }
    out
}

/// Undo ORC's block compression: blocks of a 3-byte header and data, stored or packed.
fn orc_unpack(b: &[u8], codec: u64) -> Res<Vec<u8>> {
    if codec == 0 {
        return Ok(b.to_vec());
    }
    let mut out = vec![];
    let mut i = 0;
    while i + 3 <= b.len() {
        let header = u32::from(b[i]) | u32::from(b[i + 1]) << 8 | u32::from(b[i + 2]) << 16;
        let (len, original) = ((header >> 1) as usize, header & 1 == 1);
        let chunk = b.get(i + 3..i + 3 + len).ok_or_else(cut_short)?;
        i += 3 + len;
        let room = (MAX_HEADER as usize).saturating_sub(out.len());
        if original {
            out.extend_from_slice(chunk);
            continue;
        }
        let unpacked = match codec {
            1 => {
                let mut v = vec![];
                flate2::read::DeflateDecoder::new(chunk).take(room as u64 + 1).read_to_end(&mut v).map_err(es)?;
                v
            }
            2 => {
                let n = snap::raw::decompress_len(chunk).map_err(es)?;
                if n > room {
                    return Err(too_complex());
                }
                snap::raw::Decoder::new().decompress_vec(chunk).map_err(es)?
            }
            4 => lz4_flex::block::decompress(chunk, room.min(16 * 1024 * 1024)).map_err(es)?,
            5 => {
                let mut v = vec![];
                zstd::stream::read::Decoder::new(chunk).map_err(es)?.take(room as u64 + 1).read_to_end(&mut v).map_err(es)?;
                v
            }
            // LZO and anything newer.
            _ => return Err(too_complex()),
        };
        out.extend(unpacked);
        if out.len() as u64 > MAX_HEADER {
            return Err(too_complex());
        }
    }
    Ok(out)
}

struct OrcType {
    kind: u64,
    subtypes: Vec<u64>,
    names: Vec<String>,
    max_length: Option<u64>,
    precision: Option<u64>,
    scale: Option<u64>,
}

/// A type's name, with its parts. `budget` limits the work: types may refer to each
/// other in a damaged file, which would make the names grow without end.
fn orc_type_name(types: &[OrcType], i: usize, depth: usize, budget: &mut usize) -> String {
    *budget = budget.saturating_sub(1);
    let Some(ty) = types.get(i) else { return "?".into() };
    if *budget == 0 {
        return "…".into();
    }
    let mut sub = |k: usize| ty.subtypes.get(k).map_or_else(|| "?".to_string(), |&s| if depth > 8 { "…".into() } else { orc_type_name(types, s as usize, depth + 1, budget) });
    match ty.kind {
        0 => "boolean".into(),
        1 => "tinyint".into(),
        2 => "smallint".into(),
        3 => "int".into(),
        4 => "bigint".into(),
        5 => "float".into(),
        6 => "double".into(),
        7 => "string".into(),
        8 => "binary".into(),
        9 => "timestamp".into(),
        10 => format!("array<{}>", sub(0)),
        11 => {
            let (k, v) = (sub(0), sub(1));
            format!("map<{k}, {v}>")
        }
        12 => {
            let fields: Vec<String> = ty.names.iter().enumerate().take(20).map(|(k, n)| format!("{n}: {}", sub(k))).collect();
            format!("struct<{}{}>", fields.join(", "), if ty.names.len() > 20 { ", …" } else { "" })
        }
        13 => format!("uniontype<{}>", (0..ty.subtypes.len().min(20)).map(&mut sub).collect::<Vec<_>>().join(", ")),
        14 => format!("decimal({}, {})", ty.precision.unwrap_or(38), ty.scale.unwrap_or(0)),
        15 => "date".into(),
        16 => format!("varchar({})", ty.max_length.unwrap_or(0)),
        17 => format!("char({})", ty.max_length.unwrap_or(0)),
        18 => "timestamp with local time zone".into(),
        k => format!("type {k}"),
    }
}

/// An ORC file's schema and row count, from the footer at its end.
pub fn orc_from<R: Read + Seek>(r: R) -> Res<TableData> {
    let not_orc = || t("Not an ORC file this preview can read.");
    let mut f = Bin::new(r)?;
    if f.len < 4 || f.bytes(3).map_err(|_| not_orc())? != b"ORC" {
        return Err(not_orc());
    }
    f.seek(f.len - 1)?;
    let ps_len = u64::from(f.u8()?);
    f.seek(f.len.checked_sub(1 + ps_len).ok_or_else(not_orc)?)?;
    let ps = f.bytes(ps_len)?;
    let (mut footer_len, mut codec, mut metadata_len) = (0u64, 0u64, 0u64);
    let mut version = vec![];
    for (field, _, v, bytes) in pb_fields(&ps) {
        match field {
            1 => footer_len = v,
            2 => codec = v,
            4 => version = if bytes.is_empty() { vec![v] } else { pb_packed(bytes) },
            5 => metadata_len = v,
            _ => {}
        }
    }
    let footer_at = f.len.checked_sub(1 + ps_len + footer_len).ok_or_else(not_orc)?;
    let _ = metadata_len;
    f.seek(footer_at)?;
    let footer = orc_unpack(&f.bytes(footer_len)?, codec)?;
    let (mut rows_total, mut stripes, mut types, mut user) = (None, 0u64, vec![], vec![]);
    for (field, _, v, bytes) in pb_fields(&footer) {
        match field {
            3 => stripes += 1,
            4 => {
                let mut ty = OrcType { kind: 0, subtypes: vec![], names: vec![], max_length: None, precision: None, scale: None };
                for (field, wire, v, bytes) in pb_fields(bytes) {
                    match (field, wire) {
                        (1, _) => ty.kind = v,
                        (2, 2) => ty.subtypes.extend(pb_packed(bytes)),
                        (2, _) => ty.subtypes.push(v),
                        (3, _) => ty.names.push(String::from_utf8_lossy(bytes).into_owned()),
                        (4, _) => ty.max_length = Some(v),
                        (5, _) => ty.precision = Some(v),
                        (6, _) => ty.scale = Some(v),
                        _ => {}
                    }
                }
                if types.len() < 1_000_000 {
                    types.push(ty);
                }
            }
            5 => {
                let (mut k, mut val) = (String::new(), String::new());
                for (field, _, _, bytes) in pb_fields(bytes) {
                    match field {
                        1 => k = String::from_utf8_lossy(bytes).into_owned(),
                        2 => val = if bytes.len() <= 1024 { String::from_utf8_lossy(bytes).into_owned() } else { human_size(bytes.len() as u64) },
                        _ => {}
                    }
                }
                user.push((k, val));
            }
            6 => rows_total = Some(v),
            _ => {}
        }
    }
    let root = types.first().ok_or_else(not_orc)?;
    let mut budget = 200_000usize;
    let rows: Vec<Vec<String>> = if root.kind == 12 {
        root.names
            .iter()
            .take(MAX_VARIABLES)
            .enumerate()
            .map(|(k, n)| {
                // Each column has a share of the work, so one deep column cannot hide the rest.
                let mut own = 1000usize.min(budget);
                let name = root.subtypes.get(k).map_or_else(|| "?".to_string(), |&s| orc_type_name(&types, s as usize, 0, &mut own));
                budget = budget.saturating_sub(1000 - own.min(1000));
                vec![n.clone(), name]
            })
            .collect()
    } else {
        vec![vec![String::new(), orc_type_name(&types, 0, 0, &mut budget)]]
    };
    let compression = match codec {
        0 => "none",
        1 => "zlib",
        2 => "snappy",
        3 => "lzo",
        4 => "lz4",
        5 => "zstd",
        _ => "?",
    };
    let columns = if root.kind == 12 { root.names.len() } else { 1 };
    let mut info = vec![format!("{} {}", fmt_count(columns as u64), t("columns"))];
    if let Some(n) = rows_total {
        info.push(format!("{} {}", fmt_count(n), t("rows")));
    }
    info.push(format!("{} {}", fmt_count(stripes), t("stripes")));
    let mut about = vec![(t("Compression"), compression.to_string())];
    if !version.is_empty() {
        about.push((t("Version"), version.iter().map(u64::to_string).collect::<Vec<_>>().join(".")));
    }
    about.extend(user);
    let sheets = vec![text_sheet(t("Columns"), vec![t("Name"), t("Type")], rows), key_values(t("Metadata"), about)];
    Ok(TableData { format: "ORC", info, sheets })
}

// ---------------------------------------------------------------- Stata

fn stata_type_old(code: u8) -> String {
    match code {
        1..=244 => format!("str{code}"),
        251 => "byte".into(),
        252 => "int".into(),
        253 => "long".into(),
        254 => "float".into(),
        255 => "double".into(),
        other => format!("type {other}"),
    }
}

fn stata_type_new(code: u16) -> String {
    match code {
        1..=2045 => format!("str{code}"),
        32768 => "strL".into(),
        65526 => "double".into(),
        65527 => "float".into(),
        65528 => "long".into(),
        65529 => "int".into(),
        65530 => "byte".into(),
        other => format!("type {other}"),
    }
}

/// A Stata dataset's variables: name, type, format and label.
pub fn stata_from<R: Read + Seek>(r: R) -> Res<TableData> {
    let not_dta = || t("Not a Stata file this preview can read.");
    let mut f = Bin::new(r)?;
    let start = f.bytes(11.min(f.len)).map_err(|_| not_dta())?;
    f.seek(0)?;
    let (release, k, n, label, stamp, types, names, formats, labels);
    if start == b"<stata_dta>" {
        let expect = |f: &mut Bin<R>, tag: &[u8]| -> Res<()> { if f.bytes(tag.len() as u64)? == tag { Ok(()) } else { Err(too_complex()) } };
        expect(&mut f, b"<stata_dta><header><release>")?;
        release = String::from_utf8_lossy(&f.bytes(3)?).parse::<u32>().map_err(|_| not_dta())?;
        if !(117..=119).contains(&release) {
            return Err(format!("{} (Stata {release})", t("This version of the format is not supported.")));
        }
        expect(&mut f, b"</release><byteorder>")?;
        f.little = f.bytes(3)? == b"LSF";
        expect(&mut f, b"</byteorder><K>")?;
        k = if release == 119 { u64::from(f.u32()?) } else { u64::from(f.u16()?) };
        expect(&mut f, b"</K><N>")?;
        n = if release == 117 { u64::from(f.u32()?) } else { f.u64()? };
        expect(&mut f, b"</N><label>")?;
        let len = if release == 117 { u64::from(f.u8()?) } else { u64::from(f.u16()?) };
        label = String::from_utf8_lossy(&f.bytes(len)?).into_owned();
        expect(&mut f, b"</label><timestamp>")?;
        let len = u64::from(f.u8()?);
        stamp = String::from_utf8_lossy(&f.bytes(len)?).into_owned();
        expect(&mut f, b"</timestamp></header><map>")?;
        let map = (0..14).map(|_| f.u64()).collect::<Res<Vec<u64>>>()?;
        if k as usize > MAX_VARIABLES * 10 {
            return Err(too_complex());
        }
        let (name_len, format_len, label_len) = if release == 117 { (33, 49, 81) } else { (129, 57, 321) };
        // Each block's place is in the map: types, names, sort order, formats, value labels, labels.
        let block = |f: &mut Bin<R>, at: u64, tag: &[u8], each: u64| -> Res<Vec<u8>> {
            f.seek(at)?;
            expect(f, tag)?;
            f.bytes(k.checked_mul(each).ok_or_else(too_complex)?)
        };
        let raw = block(&mut f, map[2], b"<variable_types>", 2)?;
        types = raw.chunks_exact(2).map(|c| stata_type_new(if f.little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })).collect::<Vec<_>>();
        names = block(&mut f, map[3], b"<varnames>", name_len)?.chunks(name_len as usize).map(field_text).collect::<Vec<_>>();
        formats = block(&mut f, map[5], b"<formats>", format_len)?.chunks(format_len as usize).map(field_text).collect::<Vec<_>>();
        labels = block(&mut f, map[7], b"<variable_labels>", label_len)?.chunks(label_len as usize).map(field_text).collect::<Vec<_>>();
    } else {
        release = u32::from(start.first().copied().unwrap_or(0));
        if !(113..=115).contains(&release) {
            return Err(not_dta());
        }
        f.seek(1)?;
        f.little = f.u8()? == 2;
        f.skip(2)?;
        k = u64::from(f.u16()?);
        n = u64::from(f.u32()?);
        label = field_text(&f.bytes(81)?);
        stamp = field_text(&f.bytes(18)?);
        types = f.bytes(k)?.into_iter().map(stata_type_old).collect();
        names = f.bytes(k * 33)?.chunks(33).map(field_text).collect();
        f.skip(2 * (k + 1))?;
        let format_len = if release == 113 { 12 } else { 49 };
        formats = f.bytes(k * format_len)?.chunks(format_len as usize).map(field_text).collect();
        f.skip(k * 33)?;
        labels = f.bytes(k * 81)?.chunks(81).map(field_text).collect();
    }
    let rows = (0..k as usize)
        .take(MAX_VARIABLES)
        .map(|i| vec![names.get(i).cloned().unwrap_or_default(), types.get(i).cloned().unwrap_or_default(), formats.get(i).cloned().unwrap_or_default(), labels.get(i).cloned().unwrap_or_default()])
        .collect();
    let info = vec![format!("{} {}", fmt_count(k), t("variables")), format!("{} {}", fmt_count(n), t("observations"))];
    let about = vec![(t("Version"), release.to_string()), (t("Label"), label), (t("Saved"), stamp.trim().to_string())];
    let sheets = vec![text_sheet(t("Variables"), vec![t("Name"), t("Type"), t("Format"), t("Label")], rows), key_values(t("Metadata"), about)];
    Ok(TableData { format: "Stata", info, sheets })
}

// ---------------------------------------------------------------- SPSS

/// `F8.2`, `A20`, `DATE11`: an SPSS print format.
fn spss_format(code: u32) -> String {
    let (kind, width, decimals) = ((code >> 16) & 0xFF, (code >> 8) & 0xFF, code & 0xFF);
    let name = match kind {
        1 => "A",
        2 => "AHEX",
        3 => "COMMA",
        4 => "DOLLAR",
        5 => "F",
        16 => "PCT",
        17 => "E",
        20 => "DATE",
        21 => "TIME",
        22 => "DATETIME",
        23 => "ADATE",
        24 => "JDATE",
        25 => "DTIME",
        26 => "WKDAY",
        27 => "MONTH",
        28 => "MOYR",
        29 => "QYR",
        30 => "WKYR",
        38 => "EDATE",
        39 => "SDATE",
        _ => return String::new(),
    };
    if decimals > 0 { format!("{name}{width}.{decimals}") } else { format!("{name}{width}") }
}

/// An SPSS data file's dictionary: its variables, read up to where the data starts.
pub fn spss_from<R: Read + Seek>(r: R) -> Res<TableData> {
    let not_sav = || t("Not an SPSS file this preview can read.");
    let mut f = Bin::new(r)?;
    let magic = f.bytes(4).map_err(|_| not_sav())?;
    if magic != b"$FL2" && magic != b"$FL3" {
        return Err(not_sav());
    }
    let product = field_text(&f.bytes(60)?);
    // The layout code is 2 or 3 in the file's byte order.
    let layout = f.bytes(4)?;
    f.little = matches!(i32::from_le_bytes(layout[..4].try_into().unwrap()), 2 | 3);
    let _case_size = f.i32()?;
    let compression = f.i32()?;
    let _weight = f.i32()?;
    let cases = f.i32()?;
    let _bias = f.f64()?;
    let date = field_text(&f.bytes(9)?);
    let time = field_text(&f.bytes(8)?);
    let label = field_text(&f.bytes(64)?);
    f.skip(3)?;
    let mut vars: Vec<(String, String, String, String)> = vec![];
    let mut long_names: std::collections::HashMap<String, String> = Default::default();
    let mut count = 0u64;
    loop {
        if f.pos()? > MAX_HEADER {
            return Err(too_complex());
        }
        match f.i32()? {
            2 => {
                let kind = f.i32()?;
                let has_label = f.i32()?;
                let missing = f.i32()?;
                let print = f.u32()?;
                let _write = f.u32()?;
                let name = field_text(&f.bytes(8)?);
                let label = if has_label == 1 {
                    let len = u64::from(f.u32()?);
                    let text = f.bytes(len.div_ceil(4) * 4)?;
                    String::from_utf8_lossy(&text[..len as usize]).into_owned()
                } else {
                    String::new()
                };
                f.skip(u64::from(missing.unsigned_abs()) * 8)?;
                // Long strings take more 8-byte slots, each with a record of type -1.
                if kind >= 0 {
                    count += 1;
                    if vars.len() < MAX_VARIABLES {
                        let ty = if kind == 0 { "numeric".to_string() } else { format!("string({kind})") };
                        vars.push((name, ty, spss_format(print), label));
                    }
                }
            }
            3 => {
                let n = f.u32()?;
                for _ in 0..n {
                    // Read, not skipped: small steps stay in the read buffer.
                    let label = f.bytes(9)?;
                    let len = u64::from(label[8]);
                    f.bytes((len + 1).div_ceil(8) * 8 - 1)?;
                    if f.pos()? > MAX_HEADER {
                        return Err(too_complex());
                    }
                }
                if f.i32()? != 4 {
                    return Err(too_complex());
                }
                let n = u64::from(f.u32()?);
                f.skip(n * 4)?;
            }
            6 => {
                let n = u64::from(f.u32()?);
                f.skip(n * 80)?;
            }
            7 => {
                let subtype = f.i32()?;
                let size = u64::from(f.u32()?);
                let n = u64::from(f.u32()?);
                let bytes = size.checked_mul(n).ok_or_else(too_complex)?;
                if subtype == 13 {
                    let text = f.bytes(bytes)?;
                    for pair in String::from_utf8_lossy(&text).split('\t').take(MAX_VARIABLES) {
                        if let Some((short, long)) = pair.split_once('=') {
                            long_names.insert(short.to_uppercase(), long.to_string());
                        }
                    }
                } else {
                    f.skip(bytes)?;
                }
            }
            999 => break,
            _ => return Err(too_complex()),
        }
    }
    let rows = vars
        .into_iter()
        .map(|(name, ty, format, label)| {
            let name = long_names.get(&name.to_uppercase()).cloned().unwrap_or(name);
            vec![name, ty, format, label]
        })
        .collect();
    let mut info = vec![format!("{} {}", fmt_count(count), t("variables"))];
    if cases >= 0 {
        info.push(format!("{} {}", fmt_count(cases as u64), t("cases")));
    }
    let packing = match compression {
        0 => t("none"),
        1 => "bytecode".into(),
        2 => "zlib".into(),
        _ => "?".into(),
    };
    let about = vec![(t("Label"), label), (t("Created"), format!("{date} {time}").trim().to_string()), (t("Product"), product), (t("Compression"), packing)];
    let sheets = vec![text_sheet(t("Variables"), vec![t("Name"), t("Type"), t("Format"), t("Label")], rows), key_values(t("Metadata"), about)];
    Ok(TableData { format: "SPSS", info, sheets })
}

// ---------------------------------------------------------------- SAS

const SAS_MAGIC: [u8; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xc2, 0xea, 0x81, 0x60, 0xb3, 0x14, 0x11, 0xcf, 0xbd, 0x92, 0x08, 0x00, 0x09, 0xc7, 0x31, 0x8c, 0x18, 0x1f, 0x10, 0x11,
];
/// Pages of metadata read at most, and bytes; the columns are described on the first pages.
const SAS_MAX_PAGES: u64 = 10_000;
const SAS_MAX_READ: u64 = 256 * 1024 * 1024;

/// A SAS date-time (seconds from 1960) as text.
fn sas_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds.abs() > 1e11 {
        return String::new();
    }
    let epoch = chrono::NaiveDate::from_ymd_opt(1960, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0));
    epoch.and_then(|e| e.checked_add_signed(chrono::Duration::seconds(seconds as i64))).map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default()
}

/// A SAS dataset's columns and row count, from its metadata pages.
pub fn sas_from<R: Read + Seek>(r: R) -> Res<TableData> {
    let not_sas = || t("Not a SAS file this preview can read.");
    let mut f = Bin::new(r)?;
    let head = f.bytes(288.min(f.len)).map_err(|_| not_sas())?;
    if head.len() < 288 || head[..32] != SAS_MAGIC {
        return Err(not_sas());
    }
    let u64_file = head[32] == 0x33;
    let align = if head[35] == 0x33 { 4 } else { 0 };
    let little = head[37] == 0x01;
    let int_len: usize = if u64_file { 8 } else { 4 };
    let total_align = align + if u64_file { 4 } else { 0 };
    let rd = |b: &[u8], at: usize, n: usize| -> Option<u64> {
        let s = b.get(at..at + n)?;
        let mut a = [0u8; 8];
        if little {
            a[..n].copy_from_slice(s);
            Some(u64::from_le_bytes(a))
        } else {
            a[8 - n..].copy_from_slice(s);
            Some(u64::from_be_bytes(a))
        }
    };
    let f64_at = |b: &[u8], at: usize| rd(b, at, 8).map(f64::from_bits);
    let name = field_text(&head[92..156]);
    let file_type = field_text(&head[156..164]);
    let created = f64_at(&head, 164 + align).map(sas_time).unwrap_or_default();
    let modified = f64_at(&head, 172 + align).map(sas_time).unwrap_or_default();
    let header_len = rd(&head, 196 + align, 4).ok_or_else(not_sas)?;
    let page_len = rd(&head, 200 + align, 4).ok_or_else(not_sas)?;
    let page_count = rd(&head, 204 + align, int_len).ok_or_else(not_sas)?;
    if !(1024..=16 * 1024 * 1024).contains(&page_len) || !(288..=1024 * 1024).contains(&header_len) {
        return Err(not_sas());
    }
    f.seek(0)?;
    let full_head = f.bytes(header_len)?;
    let release = full_head.get(216 + total_align..224 + total_align).map(field_text).unwrap_or_default();
    let host = full_head.get(224 + total_align..240 + total_align).map(field_text).unwrap_or_default();
    let (page_bit, ptr_len) = if u64_file { (32usize, 24usize) } else { (16, 12) };
    let (mut rows_total, mut col_count) = (None, None);
    let mut texts: Vec<Vec<u8>> = vec![];
    let mut names: Vec<(usize, usize, usize)> = vec![];
    let mut attrs: Vec<(u64, u8)> = vec![];
    let mut labels: Vec<(usize, usize, usize, usize, usize, usize)> = vec![];
    let mut texts_size = 0usize;
    for page in 0..page_count.min(SAS_MAX_PAGES).min(SAS_MAX_READ / page_len) {
        f.seek(header_len + page * page_len)?;
        let p = f.bytes(page_len)?;
        let kind = rd(&p, page_bit, 2).unwrap_or(0);
        // Data pages hold no column descriptions; past the metadata, stop.
        let meta = matches!(kind, 0 | 0x4000) || kind & 0x0F00 == 0x0200;
        if !meta {
            if col_count.is_some() && !names.is_empty() {
                break;
            }
            continue;
        }
        let subheaders = rd(&p, page_bit + 4, 2).unwrap_or(0) as usize;
        for k in 0..subheaders {
            let at = page_bit + 8 + k * ptr_len;
            let (Some(off), Some(len)) = (rd(&p, at, int_len), rd(&p, at + int_len, int_len)) else { break };
            let compression = p.get(at + 2 * int_len).copied().unwrap_or(0);
            let (off, len) = (off as usize, len as usize);
            if len == 0 || compression == 1 || off.checked_add(len).is_none_or(|e| e > p.len()) {
                continue;
            }
            let s = &p[off..off + len];
            let sig = if u64_file {
                let v = rd(s, 0, 8).unwrap_or(0);
                let (hi, lo) = (v >> 32, v & 0xFFFF_FFFF);
                if hi == 0 || hi == 0xFFFF_FFFF { lo } else { hi }
            } else {
                rd(s, 0, 4).unwrap_or(0)
            };
            match sig {
                // Row size: the row count.
                0xF7F7_F7F7 => rows_total = rd(s, 6 * int_len, int_len),
                // Column size: the column count.
                0xF6F6_F6F6 => col_count = rd(s, int_len, int_len),
                // Column text: the names and labels are slices of these.
                0xFFFF_FFFD => {
                    let size = rd(s, int_len, 2).unwrap_or(0) as usize;
                    let text = s.get(int_len..int_len + size).unwrap_or(&s[int_len.min(s.len())..]);
                    texts_size += text.len();
                    texts.push(text.to_vec());
                }
                // Column name: (text, offset, length) for each column.
                0xFFFF_FFFF => {
                    let n = len.saturating_sub(2 * int_len + 12) / 8;
                    for c in 0..n {
                        let at = int_len + 8 + c * 8;
                        if let (Some(t), Some(o), Some(l)) = (rd(s, at, 2), rd(s, at + 2, 2), rd(s, at + 4, 2)) {
                            names.push((t as usize, o as usize, l as usize));
                        }
                    }
                }
                // Column attributes: width and type (1 number, 2 text) for each column.
                0xFFFF_FFFC => {
                    let n = len.saturating_sub(2 * int_len + 12) / (int_len + 8);
                    for c in 0..n {
                        let base = int_len + 8 + c * (int_len + 8);
                        let width = rd(s, base + int_len, 4).unwrap_or(0);
                        let ty = s.get(base + int_len + 6).copied().unwrap_or(0);
                        attrs.push((width, ty));
                    }
                }
                // Format and label: where the column's format and label are in the texts.
                0xFFFF_FBFE => {
                    let base = 3 * int_len;
                    let at = |k: usize| rd(s, base + k, 2).unwrap_or(0) as usize;
                    labels.push((at(22), at(24), at(26), at(28), at(30), at(32)));
                }
                _ => {}
            }
            if names.len().max(attrs.len()).max(labels.len()) > MAX_VARIABLES * 10 || texts_size > MAX_HEADER as usize {
                return Err(too_complex());
            }
        }
    }
    if names.is_empty() && col_count.unwrap_or(0) > 0 {
        return Err(too_complex());
    }
    let text = |(i, o, l): (usize, usize, usize)| texts.get(i).and_then(|t| t.get(o..o + l)).map(|b| String::from_utf8_lossy(b).trim().to_string()).unwrap_or_default();
    let rows: Vec<Vec<String>> = names
        .iter()
        .enumerate()
        .take(MAX_VARIABLES)
        .map(|(c, &n)| {
            let (width, ty) = attrs.get(c).copied().unwrap_or((0, 0));
            let ty = match ty {
                1 => "number".to_string(),
                2 => format!("text({width})"),
                _ => "?".into(),
            };
            let (format, label) = labels.get(c).map_or_else(Default::default, |&(fi, fo, fl, li, lo, ll)| (text((fi, fo, fl)), text((li, lo, ll))));
            vec![text(n), ty, format, label]
        })
        .collect();
    let mut info = vec![format!("{} {}", fmt_count(col_count.unwrap_or(rows.len() as u64)), t("columns"))];
    if let Some(n) = rows_total {
        info.push(format!("{} {}", fmt_count(n), t("rows")));
    }
    let about = vec![
        (t("Name"), name),
        (t("Kind"), file_type),
        (t("Created"), created),
        (t("Modified"), modified),
        (t("Version"), release),
        (t("Host"), host),
        (t("Encoding"), if little { "little-endian".into() } else { "big-endian".into() }),
    ];
    let sheets = vec![text_sheet(t("Columns"), vec![t("Name"), t("Type"), t("Format"), t("Label")], rows), key_values(t("Metadata"), about)];
    Ok(TableData { format: "SAS", info, sheets })
}

// ---------------------------------------------------------------- DuckDB

/// What the DuckDB tool says about a database: its tables and views, their columns and
/// estimated row counts. The database is opened read-only, and nothing of its data is read.
fn duckdb_query(path: &Path, sql: &str, cancel: &crate::media::Cancel) -> Option<Vec<serde_json::Map<String, serde_json::Value>>> {
    let tool = crate::media::find_tool("duckdb")?;
    let mut cmd = std::process::Command::new(tool);
    cmd.args(["-json", "-init"]).arg(if cfg!(windows) { "NUL" } else { "/dev/null" }).args(["-c", &duckdb_script(path, sql)?]);
    let out = crate::media::tool_output(cmd, cancel)?;
    if out.iter().all(u8::is_ascii_whitespace) {
        return Some(vec![]);
    }
    serde_json::from_slice::<Vec<serde_json::Map<String, serde_json::Value>>>(&out).ok()
}

/// The DuckDB statements that run `sql` on the database at `path` without changing it:
/// DuckDB starts on an empty in-memory database, is told never to fetch or load an
/// extension, and only then attaches the file read-only (so its log is not replayed into
/// it), after which it may read no other file and its settings are locked. The database
/// is `db`. `None` for a path that cannot be written in SQL.
fn duckdb_script(path: &Path, sql: &str) -> Option<String> {
    let path = path.to_str().filter(|p| !p.contains('\0'))?;
    let quoted = path.replace('\'', "''");
    Some(format!(
        "SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; \
         ATTACH '{quoted}' AS db (READ_ONLY); USE db; \
         SET enable_external_access = false; SET lock_configuration = true; {sql}"
    ))
}

fn json_text(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The facts in the header of a DuckDB file.
fn duckdb_header(path: &Path) -> Res<Vec<(String, String)>> {
    let not_duck = || t("Not a DuckDB file this preview can read.");
    let mut f = Bin::new(std::io::BufReader::new(crate::util::open_regular(path).map_err(es)?))?;
    let head = f.bytes(64).map_err(|_| not_duck())?;
    if &head[8..12] != b"DUCK" {
        return Err(not_duck());
    }
    let storage = u64::from_le_bytes(head[12..20].try_into().unwrap_or_default());
    let mut facts = vec![(t("Storage version"), storage.to_string())];
    // The newest of the two database headers (at 4 KB and 8 KB) says how large it is.
    let mut best: Option<(u64, u64, u64)> = None;
    for at in [4096u64, 8192] {
        if f.seek(at + 8).is_err() {
            continue;
        }
        let (Ok(iteration), Ok(_meta), Ok(_free), Ok(blocks), Ok(block_size)) = (f.u64(), f.u64(), f.u64(), f.u64(), f.u64()) else { continue };
        if best.is_none_or(|b| iteration > b.0) {
            best = Some((iteration, blocks, block_size));
        }
    }
    if let Some((_, blocks, size)) = best.filter(|b| b.2 > 0 && b.2 <= 1 << 30) {
        facts.push((t("Blocks"), format!("{} × {}", fmt_count(blocks), human_size(size))));
    }
    Ok(facts)
}

pub fn duckdb_from(path: &Path, cancel: &crate::media::Cancel) -> Res<TableData> {
    let header = duckdb_header(path)?;
    let tables = duckdb_query(
        path,
        "SELECT schema_name, table_name, 'table' AS kind, estimated_size, column_count FROM duckdb_tables() WHERE database_name = 'db' AND NOT internal \
         UNION ALL SELECT schema_name, view_name, 'view', NULL, column_count FROM duckdb_views() WHERE database_name = 'db' AND NOT internal ORDER BY 1, 2 LIMIT 10000",
        cancel,
    );
    let Some(tables) = tables else {
        let note = if crate::media::find_tool("duckdb").is_some() { t("The DuckDB tool could not open this database.") } else { t("Install DuckDB to see the tables of this database.") };
        let sheets = vec![key_values(t("Metadata"), header.into_iter().chain([(t("Note"), note)]).collect())];
        return Ok(TableData { format: "DuckDB", info: vec![], sheets });
    };
    let columns = duckdb_query(
        path,
        "SELECT schema_name, table_name, column_name, data_type, is_nullable FROM duckdb_columns() WHERE database_name = 'db' AND NOT internal ORDER BY schema_name, table_name, column_index LIMIT 100000",
        cancel,
    )
    .unwrap_or_default();
    let qualified = |m: &serde_json::Map<String, serde_json::Value>, key: &str| {
        let schema = json_text(m.get("schema_name"));
        let name = json_text(m.get(key));
        if schema == "main" || schema.is_empty() { name } else { format!("{schema}.{name}") }
    };
    let table_rows: Vec<Vec<String>> = tables
        .iter()
        .map(|m| {
            let kind = json_text(m.get("kind"));
            vec![qualified(m, "table_name"), if kind == "view" { t("view") } else { t("table") }, json_text(m.get("column_count")), json_text(m.get("estimated_size")).parse::<u64>().map(fmt_count).unwrap_or_default()]
        })
        .collect();
    let column_rows: Vec<Vec<String>> = columns
        .iter()
        .map(|m| vec![qualified(m, "table_name"), json_text(m.get("column_name")), json_text(m.get("data_type")), if m.get("is_nullable") == Some(&serde_json::Value::Bool(false)) { "NOT NULL".into() } else { String::new() }])
        .collect();
    let info = vec![format!("{} {}", fmt_count(table_rows.len() as u64), t("tables"))];
    let sheets = vec![
        text_sheet(t("Tables"), vec![t("Name"), t("Kind"), t("Columns"), t("Rows (estimated)")], table_rows),
        text_sheet(t("Columns"), vec![t("Table"), t("Name"), t("Type"), t("Note")], column_rows),
        key_values(t("Metadata"), header),
    ];
    Ok(TableData { format: "DuckDB", info, sheets })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn cell(s: &Sheet, r: usize, c: usize) -> Option<&str> {
        s.rows[r][c].as_deref()
    }

    /// The database is attached read-only after extensions are switched off, and a quote
    /// in its name cannot end the SQL string.
    #[test]
    fn duckdb_opens_databases_read_only() {
        let s = duckdb_script(Path::new("/data/it's.duckdb"), "SELECT 1").unwrap();
        let attach = s.find("ATTACH '/data/it''s.duckdb' AS db (READ_ONLY)").unwrap();
        assert!(s.find("SET autoload_known_extensions = false").unwrap() < attach);
        assert!(s.find("SET autoinstall_known_extensions = false").unwrap() < attach);
        let locked = s.find("SET lock_configuration = true").unwrap();
        assert!(attach < s.find("SET enable_external_access = false").unwrap());
        assert!(locked < s.find("SELECT 1").unwrap());
        assert!(duckdb_script(Path::new("/a\0b"), "SELECT 1").is_none());
    }

    #[test]
    fn netcdf_classic_header() {
        let mut v = b"CDF\x01".to_vec();
        let be = |n: u32| n.to_be_bytes();
        let name = |s: &str| {
            let mut b = be(s.len() as u32).to_vec();
            b.extend(s.as_bytes());
            while b.len() % 4 != 0 {
                b.push(0);
            }
            b
        };
        v.extend(be(3)); // records
        v.extend(be(0x0A));
        v.extend(be(2));
        v.extend(name("time"));
        v.extend(be(0));
        v.extend(name("x"));
        v.extend(be(10));
        v.extend(be(0x0C));
        v.extend(be(1));
        v.extend(name("title"));
        v.extend(be(2));
        v.extend(be(4));
        v.extend(b"Demo");
        v.extend(be(0x0B));
        v.extend(be(1));
        v.extend(name("temp"));
        v.extend(be(2));
        v.extend(be(0));
        v.extend(be(1));
        v.extend(be(0x0C));
        v.extend(be(1));
        v.extend(name("units"));
        v.extend(be(2));
        v.extend(be(1));
        v.extend(b"K\0\0\0");
        v.extend(be(5));
        v.extend(be(40));
        v.extend(be(200));
        let d = netcdf_classic(Cursor::new(v)).unwrap();
        let vars = &d.sheets[0];
        assert_eq!(cell(vars, 0, 0), Some("temp"));
        assert_eq!(cell(vars, 0, 1), Some("3 × 10"));
        assert_eq!(cell(vars, 0, 2), Some("time, x"));
        assert_eq!(cell(vars, 0, 3), Some("float"));
        assert_eq!(cell(vars, 0, 4), Some("[K]"));
        assert_eq!(cell(&d.sheets[2], 0, 1), Some("Demo"));
    }

    #[test]
    fn orc_types_that_refer_to_themselves_stay_small() {
        // A struct of 20 fields, each of them that same struct again.
        let mut ty = vec![0x08, 12, 0x12, 20];
        ty.extend([0u8; 20]);
        for _ in 0..20 {
            ty.extend([0x1A, 1, b'a']);
        }
        let mut footer = vec![0x22, ty.len() as u8];
        footer.extend(ty);
        footer.extend([0x30, 5]);
        let ps = [0x08, footer.len() as u8, 0x10, 0];
        let mut file = b"ORC".to_vec();
        file.extend(&footer);
        file.extend(ps);
        file.push(ps.len() as u8);
        let d = orc_from(Cursor::new(file)).unwrap();
        assert_eq!(d.sheets[0].rows.len(), 20);
        let text = cell(&d.sheets[0], 0, 1).unwrap();
        assert!(text.contains('…') && text.len() < 64 * 1024, "{}", text.len());
    }

    #[test]
    fn mat_files_of_countless_elements_are_cut_short() {
        let mut v = b"MATLAB 5.0 MAT-file".to_vec();
        v.resize(126, b' ');
        v.extend(b"IM");
        // A million empty elements.
        v.extend(std::iter::repeat_n([1u8, 0, 0, 0, 0, 0, 0, 0], 1_000_000).flatten());
        let d = mat_v5(Cursor::new(v)).unwrap();
        assert!(d.info.contains(&t("(truncated)")), "{:?}", d.info);
    }

    #[test]
    fn damaged_headers_are_refused() {
        assert!(netcdf_classic(Cursor::new(b"CDF\x01\0\0\0\x03\0\0\0\x0A\x7f\xff\xff\xff".to_vec())).is_err());
        assert!(mat_v5(Cursor::new(vec![0u8; 200])).is_err());
        assert!(stata_from(Cursor::new(b"<stata_dta><header><release>999".to_vec())).is_err());
        assert!(spss_from(Cursor::new(b"$FL2".to_vec())).is_err());
        assert!(sas_from(Cursor::new(vec![0u8; 100])).is_err());
        assert!(orc_from(Cursor::new(b"ORC\xff".to_vec())).is_err());
        assert!(orc_unpack(&[0x10, 0, 0, 1, 2], 1).is_err(), "a block longer than the footer");
    }
}
