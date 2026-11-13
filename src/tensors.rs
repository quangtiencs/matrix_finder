//! Model and tensor files: safetensors, NumPy archives (`.npz`) and ONNX. Each lists its
//! tensors (name, shape, type, size) through the table view. Only the headers are read,
//! so a model of many gigabytes opens at once; the small arrays of an `.npz` are shown too.
use crate::i18n::t;
use crate::table::{clip_cell, finish_sheet, fmt_count, Cell, Sheet, TableData, MAX_ROWS};
use crate::util::{es, human_size, Res};
use std::io::{Read, Seek, SeekFrom};

/// One row of a tensor list. `shape` is `None` when it is not known.
struct TensorRow {
    name: String,
    shape: Option<Vec<String>>,
    dtype: String,
    elements: Option<u64>,
    bytes: Option<u64>,
    note: String,
}

fn shape_text(shape: &Option<Vec<String>>) -> String {
    match shape {
        None => "?".to_string(),
        Some(s) if s.is_empty() => t("scalar"),
        Some(s) => s.join(" × "),
    }
}

/// The number of items of a shape; `None` if it does not fit (a damaged file).
fn element_count(shape: &[u64]) -> Option<u64> {
    shape.iter().try_fold(1u64, |n, d| n.checked_mul(*d))
}

/// `7,241,732,096 parameters (7.24 B)`.
fn parameters_info(total: u64) -> String {
    let short = match total {
        n if n >= 1_000_000_000 => format!(" ({:.2} B)", n as f64 / 1e9),
        n if n >= 1_000_000 => format!(" ({:.1} M)", n as f64 / 1e6),
        n if n >= 1_000 => format!(" ({:.1} K)", n as f64 / 1e3),
        _ => String::new(),
    };
    format!("{} {}{short}", fmt_count(total), t("parameters"))
}

/// The table of tensors. `count_label` names the item count ("Parameters", "Elements").
fn tensor_sheet(title: String, rows: &[TensorRow], count_label: String, with_note: bool) -> Sheet {
    let mut columns = vec![t("Name"), t("Shape"), t("Type"), count_label, t("Size")];
    let mut types = vec!["text".to_string(), "text".into(), "text".into(), "integer".into(), "integer".into()];
    if with_note {
        columns.push(t("Note"));
        types.push("text".into());
    }
    let cells = rows
        .iter()
        .take(MAX_ROWS)
        .map(|r| {
            let mut row: Vec<Cell> = vec![Some(clip_cell(r.name.clone())), Some(shape_text(&r.shape)), Some(r.dtype.clone()), r.elements.map(fmt_count), r.bytes.map(human_size)];
            if with_note {
                row.push(Some(clip_cell(r.note.clone())));
            }
            row
        })
        .collect();
    finish_sheet(title, columns, Some(types), cells, Some(rows.len() as u64))
}

fn key_value_sheet(title: String, pairs: Vec<(String, String)>) -> Sheet {
    let total = pairs.len() as u64;
    let rows = pairs.into_iter().take(MAX_ROWS).map(|(k, v)| vec![Some(clip_cell(k)), Some(clip_cell(v))]).collect();
    finish_sheet(title, vec![t("Key"), t("Value")], Some(vec!["text".into(), "text".into()]), rows, Some(total))
}

/// Distinct types in order of how many items they hold: `BF16, F32`.
fn dtype_summary(rows: &[TensorRow]) -> String {
    let mut seen: Vec<(String, u64)> = vec![];
    for r in rows {
        let n = r.elements.unwrap_or(0);
        match seen.iter_mut().find(|(d, _)| *d == r.dtype) {
            Some(s) => s.1 = s.1.saturating_add(n),
            None => seen.push((r.dtype.clone(), n)),
        }
    }
    seen.sort_by_key(|s| std::cmp::Reverse(s.1));
    seen.into_iter().map(|(d, _)| d).collect::<Vec<_>>().join(", ")
}

// ---------------------------------------------------------------- safetensors

/// The format's own limit on its JSON header.
const ST_MAX_HEADER: u64 = 100 * 1024 * 1024;

/// A safetensors file is an 8-byte header length, a JSON header naming each tensor's
/// type, shape and place in the data, then the data, which is never read here.
pub fn safetensors_from<R: Read>(mut r: R) -> Res<TableData> {
    use serde_json::Value;
    let bad = || t("Not a valid safetensors file.");
    let mut len = [0u8; 8];
    r.read_exact(&mut len).map_err(|_| bad())?;
    let len = u64::from_le_bytes(len);
    if !(2..=ST_MAX_HEADER).contains(&len) {
        return Err(bad());
    }
    let mut header = Vec::new();
    r.take(len).read_to_end(&mut header).map_err(es)?;
    if header.len() as u64 != len {
        return Err(t("This file is cut short."));
    }
    let json: serde_json::Map<String, Value> = serde_json::from_slice(&header).map_err(|_| bad())?;
    let mut metadata = vec![];
    let mut rows = vec![];
    for (name, v) in json {
        if name == "__metadata__" {
            for (k, v) in v.as_object().into_iter().flatten() {
                metadata.push((k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string)));
            }
            continue;
        }
        let dtype = v.get("dtype").and_then(Value::as_str).unwrap_or("?").to_string();
        let shape: Option<Vec<u64>> = v.get("shape").and_then(Value::as_array).and_then(|a| a.iter().map(Value::as_u64).collect());
        let offsets = v.get("data_offsets").and_then(Value::as_array).and_then(|a| Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?)));
        rows.push(TensorRow {
            elements: shape.as_deref().and_then(element_count),
            shape: shape.map(|s| s.iter().map(u64::to_string).collect()),
            dtype,
            bytes: offsets.and_then(|(start, end)| end.checked_sub(start)),
            name,
            note: String::new(),
        });
    }
    rows.sort_by(|a, b| crate::pane::natural_cmp(&a.name, &b.name));
    let total: u64 = rows.iter().filter_map(|r| r.elements).fold(0, u64::saturating_add);
    let bytes: u64 = rows.iter().filter_map(|r| r.bytes).fold(0, u64::saturating_add);
    let mut info = vec![format!("{} {}", fmt_count(rows.len() as u64), t("tensors")), parameters_info(total), human_size(bytes)];
    if !rows.is_empty() {
        info.push(dtype_summary(&rows));
    }
    let mut sheets = vec![tensor_sheet(t("Tensors"), &rows, t("Parameters"), false)];
    if !metadata.is_empty() {
        sheets.push(key_value_sheet(t("Metadata"), metadata));
    }
    Ok(TableData { format: "safetensors", info, sheets })
}

// ---------------------------------------------------------------- NumPy archives

/// Arrays up to this size are read whole and shown on a tab of their own.
const NPZ_MAX_ARRAY: u64 = 64 * 1024 * 1024;
const NPZ_MAX_TOTAL: u64 = 256 * 1024 * 1024;
const NPZ_MAX_SHOWN: usize = 40;

/// `np.savez` writes a ZIP file with one `.npy` file per array.
pub fn npz_from<R: Read + Seek>(r: R) -> Res<TableData> {
    let mut zip = zip::ZipArchive::new(r).map_err(|_| t("Not a valid NumPy archive."))?;
    let mut rows = vec![];
    let mut sheets = vec![];
    let mut budget = NPZ_MAX_TOTAL;
    for i in 0..zip.len() {
        let entry_name = zip.name_for_index(i).unwrap_or("?").to_string();
        let mut f = match zip.by_index(i) {
            Ok(f) => f,
            Err(e) => {
                let name = entry_name;
                rows.push(TensorRow { name, shape: None, dtype: String::new(), elements: None, bytes: None, note: format!("{}: {e}", t("Cannot be read")) });
                continue;
            }
        };
        if f.is_dir() {
            continue;
        }
        let name = f.name().strip_suffix(".npy").unwrap_or(f.name()).to_string();
        let size = f.size();
        let small = sheets.len() < NPZ_MAX_SHOWN && size <= NPZ_MAX_ARRAY && size <= budget;
        let whole = if small {
            let mut bytes = Vec::with_capacity(size as usize);
            match (&mut f).take(size).read_to_end(&mut bytes) {
                Ok(_) => Some(bytes),
                Err(e) => {
                    rows.push(TensorRow { name, shape: None, dtype: String::new(), elements: None, bytes: Some(size), note: format!("{}: {e}", t("Cannot be read")) });
                    continue;
                }
            }
        } else {
            None
        };
        let header = match &whole {
            Some(bytes) => crate::scidata::npy_header(&mut std::io::Cursor::new(bytes)),
            None => crate::scidata::npy_header(&mut f),
        };
        let header = match header {
            Ok(h) => h,
            Err(e) => {
                rows.push(TensorRow { name, shape: None, dtype: String::new(), elements: None, bytes: Some(size), note: e });
                continue;
            }
        };
        let note = if !header.readable() {
            t("Python objects are not read")
        } else if let Some(bytes) = whole {
            budget -= size;
            match crate::scidata::npy_from(std::io::Cursor::new(bytes)) {
                Ok(mut data) if !data.sheets.is_empty() => {
                    let mut sheet = data.sheets.remove(0);
                    sheet.name = name.clone();
                    sheets.push(sheet);
                    format!("{} {name}", t("Tab:"))
                }
                Ok(_) => String::new(),
                Err(e) => e,
            }
        } else {
            t("Too large to preview")
        };
        rows.push(TensorRow {
            elements: element_count(&header.shape),
            shape: Some(header.shape.iter().map(u64::to_string).collect()),
            dtype: header.dtype_text(),
            bytes: Some(size),
            name,
            note,
        });
    }
    let total: u64 = rows.iter().filter_map(|r| r.elements).fold(0, u64::saturating_add);
    let info = vec![format!("{} {}", fmt_count(rows.len() as u64), t("arrays")), format!("{} {}", fmt_count(total), t("elements"))];
    sheets.insert(0, tensor_sheet(t("Contents"), &rows, t("Elements"), true));
    Ok(TableData { format: "NumPy archive", info, sheets })
}

// ---------------------------------------------------------------- ONNX

/// A protobuf reader that steps over what it does not need (above all the weights'
/// data) without reading it. Every message has an end, checked against its parent's.
struct Pb<R> {
    r: R,
    pos: u64,
}

const WIRE_VARINT: u8 = 0;
const WIRE_I64: u8 = 1;
const WIRE_LEN: u8 = 2;
const WIRE_I32: u8 = 5;
/// Longest text kept from a name or a note.
const PB_MAX_TEXT: u64 = 64 * 1024;

fn onnx_bad() -> String {
    t("Not a valid ONNX file.")
}

impl<R: Read + Seek> Pb<R> {
    fn byte(&mut self) -> Res<u8> {
        let mut b = [0u8];
        self.r.read_exact(&mut b).map_err(|_| t("This file is cut short."))?;
        self.pos += 1;
        Ok(b[0])
    }

    fn varint(&mut self) -> Res<u64> {
        let mut v = 0u64;
        for shift in (0..70).step_by(7) {
            let b = self.byte()?;
            v |= u64::from(b & 0x7F).checked_shl(shift).unwrap_or(0);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(onnx_bad())
    }

    /// The next field number and wire type, or `None` at the end of the message.
    fn key(&mut self, end: u64) -> Res<Option<(u64, u8)>> {
        if self.pos >= end {
            return Ok(None);
        }
        let k = self.varint()?;
        Ok(Some((k >> 3, (k & 7) as u8)))
    }

    /// The end of a length-delimited field that starts here.
    fn sub_end(&mut self, end: u64) -> Res<u64> {
        let len = self.varint()?;
        self.pos.checked_add(len).filter(|e| *e <= end).ok_or_else(onnx_bad)
    }

    fn skip_to(&mut self, to: u64) -> Res<()> {
        let n = to - self.pos;
        // Small gaps are read through, so a buffered reader keeps its buffer.
        if n <= 64 * 1024 {
            let copied = std::io::copy(&mut (&mut self.r).take(n), &mut std::io::sink()).map_err(es)?;
            if copied != n {
                return Err(t("This file is cut short."));
            }
        } else {
            self.r.seek(SeekFrom::Start(to)).map_err(es)?;
        }
        self.pos = to;
        Ok(())
    }

    fn skip(&mut self, wire: u8, end: u64) -> Res<()> {
        match wire {
            WIRE_VARINT => self.varint().map(drop),
            WIRE_I64 | WIRE_I32 => {
                let to = self.pos + if wire == WIRE_I64 { 8 } else { 4 };
                if to > end {
                    return Err(onnx_bad());
                }
                self.skip_to(to)
            }
            WIRE_LEN => {
                let to = self.sub_end(end)?;
                self.skip_to(to)
            }
            _ => Err(onnx_bad()),
        }
    }

    fn string(&mut self, end: u64) -> Res<String> {
        let to = self.sub_end(end)?;
        let keep = (to - self.pos).min(PB_MAX_TEXT);
        let mut b = vec![0u8; keep as usize];
        self.r.read_exact(&mut b).map_err(|_| t("This file is cut short."))?;
        self.pos += keep;
        self.skip_to(to)?;
        Ok(String::from_utf8_lossy(&b).into_owned())
    }

    /// Repeated integers: one per field, or packed together in one.
    fn ints(&mut self, wire: u8, end: u64, out: &mut Vec<i64>) -> Res<()> {
        if wire == WIRE_LEN {
            let to = self.sub_end(end)?;
            while self.pos < to {
                out.push(self.varint()? as i64);
            }
            Ok(())
        } else if wire == WIRE_VARINT {
            out.push(self.varint()? as i64);
            Ok(())
        } else {
            Err(onnx_bad())
        }
    }

    /// A message of `(field 1, field 2)` strings: `StringStringEntryProto`, opsets.
    fn pair(&mut self, end: u64) -> Res<(String, String)> {
        let end = self.sub_end(end)?;
        let (mut a, mut b) = (String::new(), String::new());
        while let Some((field, wire)) = self.key(end)? {
            match (field, wire) {
                (1, WIRE_LEN) => a = self.string(end)?,
                (2, WIRE_LEN) => b = self.string(end)?,
                (2, WIRE_VARINT) => b = self.varint()?.to_string(),
                _ => self.skip(wire, end)?,
            }
        }
        Ok((a, b))
    }
}

/// ONNX `TensorProto.DataType`: name and bits per item.
fn onnx_type(code: u64) -> (&'static str, Option<u64>) {
    match code {
        1 => ("float32", Some(32)),
        2 => ("uint8", Some(8)),
        3 => ("int8", Some(8)),
        4 => ("uint16", Some(16)),
        5 => ("int16", Some(16)),
        6 => ("int32", Some(32)),
        7 => ("int64", Some(64)),
        8 => ("string", None),
        9 => ("bool", Some(8)),
        10 => ("float16", Some(16)),
        11 => ("float64", Some(64)),
        12 => ("uint32", Some(32)),
        13 => ("uint64", Some(64)),
        14 => ("complex64", Some(64)),
        15 => ("complex128", Some(128)),
        16 => ("bfloat16", Some(16)),
        17 => ("float8e4m3fn", Some(8)),
        18 => ("float8e4m3fnuz", Some(8)),
        19 => ("float8e5m2", Some(8)),
        20 => ("float8e5m2fnuz", Some(8)),
        21 => ("uint4", Some(4)),
        22 => ("int4", Some(4)),
        23 => ("float4e2m1", Some(4)),
        24 => ("float8e8m0", Some(8)),
        25 => ("uint2", Some(2)),
        26 => ("int2", Some(2)),
        _ => ("?", None),
    }
}

#[derive(Default)]
struct OnnxGraph {
    weights: Vec<TensorRow>,
    inputs: Vec<(String, String, String)>,
    outputs: Vec<(String, String, String)>,
    /// Operator and how many nodes use it, in order of first use.
    ops: Vec<(String, u64)>,
    nodes: u64,
}

#[derive(Default)]
struct OnnxModel {
    ir_version: Option<u64>,
    producer: String,
    producer_version: String,
    domain: String,
    model_version: Option<u64>,
    doc: String,
    opsets: Vec<(String, String)>,
    metadata: Vec<(String, String)>,
    graph: Option<OnnxGraph>,
}

/// `TensorProto`: its name, type and dimensions; the data is stepped over.
fn onnx_tensor<R: Read + Seek>(pb: &mut Pb<R>, end: u64, extra_dims: Option<Vec<i64>>) -> Res<TensorRow> {
    let (mut name, mut dtype, mut dims, mut raw, mut external, mut location) = (String::new(), 0, vec![], None, false, String::new());
    while let Some((field, wire)) = pb.key(end)? {
        match (field, wire) {
            (1, _) => pb.ints(wire, end, &mut dims)?,
            (2, WIRE_VARINT) => dtype = pb.varint()?,
            (8, WIRE_LEN) => name = pb.string(end)?,
            (9, WIRE_LEN) => {
                let to = pb.sub_end(end)?;
                raw = Some(to - pb.pos);
                pb.skip_to(to)?;
            }
            (13, WIRE_LEN) => {
                let (k, v) = pb.pair(end)?;
                if k == "location" {
                    location = v;
                }
            }
            (14, WIRE_VARINT) => external = pb.varint()? == 1,
            _ => pb.skip(wire, end)?,
        }
    }
    // A sparse tensor's shape is its own, not that of the values it stores.
    let dims = extra_dims.unwrap_or(dims);
    let (type_name, bits) = onnx_type(dtype);
    let shape: Option<Vec<u64>> = dims.iter().map(|d| u64::try_from(*d).ok()).collect();
    let elements = shape.as_deref().and_then(element_count);
    let bytes = raw.or_else(|| Some(elements?.checked_mul(bits?)?.div_ceil(8)));
    let note = if external { format!("{}: {location}", t("In a separate file")) } else { String::new() };
    Ok(TensorRow { name, shape: Some(dims.iter().map(i64::to_string).collect()), dtype: type_name.to_string(), elements, bytes, note })
}

/// `SparseTensorProto`: values (a tensor), indices, and the full shape.
fn onnx_sparse<R: Read + Seek>(pb: &mut Pb<R>, end: u64) -> Res<TensorRow> {
    let mut dims = vec![];
    let mut values: Option<(u64, u64)> = None;
    while let Some((field, wire)) = pb.key(end)? {
        match (field, wire) {
            (1, WIRE_LEN) => {
                let to = pb.sub_end(end)?;
                values = Some((pb.pos, to));
                pb.skip_to(to)?;
            }
            (3, _) => pb.ints(wire, end, &mut dims)?,
            _ => pb.skip(wire, end)?,
        }
    }
    let after = pb.pos;
    let mut row = match values {
        Some((start, to)) => {
            pb.r.seek(SeekFrom::Start(start)).map_err(es)?;
            pb.pos = start;
            let row = onnx_tensor(pb, to, Some(dims))?;
            pb.r.seek(SeekFrom::Start(after)).map_err(es)?;
            pb.pos = after;
            row
        }
        None => TensorRow { name: String::new(), shape: Some(dims.iter().map(i64::to_string).collect()), dtype: "?".into(), elements: None, bytes: None, note: String::new() },
    };
    row.note = t("sparse");
    Ok(row)
}

/// `ValueInfoProto`: a graph input or output, as name, shape and type.
fn onnx_value_info<R: Read + Seek>(pb: &mut Pb<R>, end: u64) -> Res<(String, String, String)> {
    let (mut name, mut shape, mut dtype) = (String::new(), "?".to_string(), String::new());
    while let Some((field, wire)) = pb.key(end)? {
        match (field, wire) {
            (1, WIRE_LEN) => name = pb.string(end)?,
            (2, WIRE_LEN) => {
                let type_end = pb.sub_end(end)?;
                while let Some((field, wire)) = pb.key(type_end)? {
                    match (field, wire) {
                        // Tensor and sparse tensor types: elem_type (1) and shape (2).
                        (1 | 8, WIRE_LEN) => {
                            let t_end = pb.sub_end(type_end)?;
                            while let Some((field, wire)) = pb.key(t_end)? {
                                match (field, wire) {
                                    (1, WIRE_VARINT) => dtype = onnx_type(pb.varint()?).0.to_string(),
                                    (2, WIRE_LEN) => shape = onnx_shape(pb, t_end)?,
                                    _ => pb.skip(wire, t_end)?,
                                }
                            }
                        }
                        (4, WIRE_LEN) => {
                            dtype = "sequence".into();
                            pb.skip(wire, type_end)?;
                        }
                        (5, WIRE_LEN) => {
                            dtype = "map".into();
                            pb.skip(wire, type_end)?;
                        }
                        (9, WIRE_LEN) => {
                            dtype = "optional".into();
                            pb.skip(wire, type_end)?;
                        }
                        _ => pb.skip(wire, type_end)?,
                    }
                }
            }
            _ => pb.skip(wire, end)?,
        }
    }
    Ok((name, shape, dtype))
}

/// `TensorShapeProto`: dimensions as numbers or names (`batch × 3 × 224 × 224`).
fn onnx_shape<R: Read + Seek>(pb: &mut Pb<R>, end: u64) -> Res<String> {
    let end = pb.sub_end(end)?;
    let mut dims = vec![];
    while let Some((field, wire)) = pb.key(end)? {
        if (field, wire) != (1, WIRE_LEN) {
            pb.skip(wire, end)?;
            continue;
        }
        let d_end = pb.sub_end(end)?;
        let mut dim = "?".to_string();
        while let Some((field, wire)) = pb.key(d_end)? {
            match (field, wire) {
                (1, WIRE_VARINT) => dim = (pb.varint()? as i64).to_string(),
                (2, WIRE_LEN) => dim = pb.string(d_end)?,
                _ => pb.skip(wire, d_end)?,
            }
        }
        dims.push(dim);
    }
    Ok(if dims.is_empty() { t("scalar") } else { dims.join(" × ") })
}

fn onnx_graph<R: Read + Seek>(pb: &mut Pb<R>, end: u64) -> Res<OnnxGraph> {
    let mut g = OnnxGraph::default();
    while let Some((field, wire)) = pb.key(end)? {
        match (field, wire) {
            (1, WIRE_LEN) => {
                let n_end = pb.sub_end(end)?;
                let (mut op, mut domain) = (String::new(), String::new());
                while let Some((field, wire)) = pb.key(n_end)? {
                    match (field, wire) {
                        (4, WIRE_LEN) => op = pb.string(n_end)?,
                        (7, WIRE_LEN) => domain = pb.string(n_end)?,
                        _ => pb.skip(wire, n_end)?,
                    }
                }
                if !domain.is_empty() && domain != "ai.onnx" {
                    op = format!("{domain}.{op}");
                }
                g.nodes += 1;
                match g.ops.iter_mut().find(|(o, _)| *o == op) {
                    Some(o) => o.1 += 1,
                    None => g.ops.push((op, 1)),
                }
            }
            (5, WIRE_LEN) => {
                let to = pb.sub_end(end)?;
                g.weights.push(onnx_tensor(pb, to, None)?);
            }
            (15, WIRE_LEN) => {
                let to = pb.sub_end(end)?;
                g.weights.push(onnx_sparse(pb, to)?);
            }
            (11, WIRE_LEN) => {
                let to = pb.sub_end(end)?;
                g.inputs.push(onnx_value_info(pb, to)?);
            }
            (12, WIRE_LEN) => {
                let to = pb.sub_end(end)?;
                g.outputs.push(onnx_value_info(pb, to)?);
            }
            _ => pb.skip(wire, end)?,
        }
    }
    Ok(g)
}

fn onnx_model<R: Read + Seek>(pb: &mut Pb<R>, end: u64) -> Res<OnnxModel> {
    let mut m = OnnxModel::default();
    while let Some((field, wire)) = pb.key(end)? {
        match (field, wire) {
            (1, WIRE_VARINT) => m.ir_version = Some(pb.varint()?),
            (2, WIRE_LEN) => m.producer = pb.string(end)?,
            (3, WIRE_LEN) => m.producer_version = pb.string(end)?,
            (4, WIRE_LEN) => m.domain = pb.string(end)?,
            (5, WIRE_VARINT) => m.model_version = Some(pb.varint()?),
            (6, WIRE_LEN) => m.doc = pb.string(end)?,
            (7, WIRE_LEN) => {
                let to = pb.sub_end(end)?;
                m.graph = Some(onnx_graph(pb, to)?);
            }
            (8, WIRE_LEN) => m.opsets.push(pb.pair(end)?),
            (14, WIRE_LEN) => m.metadata.push(pb.pair(end)?),
            _ => pb.skip(wire, end)?,
        }
    }
    Ok(m)
}

pub fn onnx_from<R: Read + Seek>(mut r: R) -> Res<TableData> {
    let len = r.seek(SeekFrom::End(0)).map_err(es)?;
    r.seek(SeekFrom::Start(0)).map_err(es)?;
    let mut pb = Pb { r, pos: 0 };
    let m = onnx_model(&mut pb, len).map_err(|e| if e == t("This file is cut short.") { e } else { onnx_bad() })?;
    // Any bytes parse as some protobuf; a model has a version and a graph.
    let (Some(ir_version), Some(g)) = (m.ir_version, m.graph) else {
        return Err(onnx_bad());
    };
    let weight_names: std::collections::HashSet<&str> = g.weights.iter().map(|w| w.name.as_str()).collect();
    // Older models also list their weights as inputs.
    let inputs: Vec<_> = g.inputs.iter().filter(|i| !weight_names.contains(i.0.as_str())).collect();
    let total: u64 = g.weights.iter().filter_map(|r| r.elements).fold(0, u64::saturating_add);
    let opset = m.opsets.iter().find(|(d, _)| d.is_empty() || d == "ai.onnx").map(|(_, v)| v.clone());
    let mut info = vec![format!("{} {}", fmt_count(g.weights.len() as u64), t("weights")), parameters_info(total), format!("{} {}", fmt_count(g.nodes), t("nodes"))];
    if let Some(v) = &opset {
        info.push(format!("opset {v}"));
    }
    if !m.producer.is_empty() {
        info.push(format!("{} {}", m.producer, m.producer_version).trim().to_string());
    }
    let mut sheets = vec![];
    if !g.weights.is_empty() {
        let with_note = g.weights.iter().any(|w| !w.note.is_empty());
        sheets.push(tensor_sheet(t("Weights"), &g.weights, t("Parameters"), with_note));
    }
    let io: Vec<Vec<Cell>> = inputs
        .iter()
        .map(|i| (t("input"), *i))
        .chain(g.outputs.iter().map(|o| (t("output"), o)))
        .take(MAX_ROWS)
        .map(|(kind, (name, shape, dtype))| vec![Some(kind), Some(clip_cell(name.clone())), Some(clip_cell(shape.clone())), Some(dtype.clone())])
        .collect();
    let io_total = (inputs.len() + g.outputs.len()) as u64;
    sheets.push(finish_sheet(t("Inputs and outputs"), vec![t("Kind"), t("Name"), t("Shape"), t("Type")], Some(vec!["text".into(); 4]), io, Some(io_total)));
    let mut ops = g.ops;
    ops.sort_by_key(|o| std::cmp::Reverse(o.1));
    let op_total = ops.len() as u64;
    let op_rows = ops.into_iter().take(MAX_ROWS).map(|(op, n)| vec![Some(op), Some(fmt_count(n))]).collect();
    sheets.push(finish_sheet(t("Operators"), vec![t("Operator"), t("Nodes")], Some(vec!["text".into(), "integer".into()]), op_rows, Some(op_total)));
    let mut about = vec![(t("IR version"), ir_version.to_string())];
    for (label, value) in [(t("Producer"), format!("{} {}", m.producer, m.producer_version).trim().to_string()), (t("Domain"), m.domain)] {
        if !value.is_empty() {
            about.push((label, value));
        }
    }
    if let Some(v) = m.model_version {
        about.push((t("Model version"), v.to_string()));
    }
    for (domain, version) in m.opsets {
        about.push((format!("opset {}", if domain.is_empty() { "ai.onnx" } else { &domain }), version));
    }
    if !m.doc.is_empty() {
        about.push((t("Description"), m.doc));
    }
    about.extend(m.metadata);
    sheets.push(key_value_sheet(t("Metadata"), about));
    Ok(TableData { format: "ONNX", info, sheets })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn cell(s: &Sheet, r: usize, c: usize) -> Option<&str> {
        s.rows[r][c].as_deref()
    }

    fn safetensors(header: &str, data: usize) -> Vec<u8> {
        let mut v = (header.len() as u64).to_le_bytes().to_vec();
        v.extend_from_slice(header.as_bytes());
        v.extend(std::iter::repeat_n(0u8, data));
        v
    }

    #[test]
    fn safetensors_lists_tensors_in_natural_order() {
        let header = r#"{"__metadata__":{"format":"pt"},"layers.10.w":{"dtype":"F32","shape":[2,3],"data_offsets":[0,24]},"layers.2.w":{"dtype":"BF16","shape":[4],"data_offsets":[24,32]},"bias":{"dtype":"F32","shape":[],"data_offsets":[32,36]}}"#;
        let d = safetensors_from(Cursor::new(safetensors(header, 36))).unwrap();
        let s = &d.sheets[0];
        let names: Vec<_> = (0..3).map(|r| cell(s, r, 0).unwrap()).collect();
        assert_eq!(names, ["bias", "layers.2.w", "layers.10.w"]);
        assert_eq!(cell(s, 0, 1), Some(t("scalar").as_str()));
        assert_eq!(cell(s, 2, 1), Some("2 × 3"));
        assert_eq!(cell(s, 2, 2), Some("F32"));
        assert_eq!(cell(s, 2, 3), Some("6"));
        assert_eq!(cell(s, 2, 4), Some("24 B"));
        assert!(d.info[1].starts_with("11 "), "{:?}", d.info);
        assert_eq!(d.info[3], "F32, BF16");
        let meta = &d.sheets[1];
        assert_eq!((cell(meta, 0, 0), cell(meta, 0, 1)), (Some("format"), Some("pt")));
    }

    #[test]
    fn safetensors_refuses_bad_headers() {
        assert!(safetensors_from(Cursor::new(b"abc".to_vec())).is_err());
        assert!(safetensors_from(Cursor::new(u64::MAX.to_le_bytes().to_vec())).is_err());
        assert!(safetensors_from(Cursor::new(safetensors("not json", 0))).is_err());
        let mut short = safetensors("{}", 0);
        short[0] = 50;
        assert_eq!(safetensors_from(Cursor::new(short)).err(), Some(t("This file is cut short.")));
        // Strange entries are listed with what is known of them.
        let d = safetensors_from(Cursor::new(safetensors(r#"{"x":{"shape":[-1]},"y":{"dtype":"F32","shape":[4294967296,4294967296,4294967296]}}"#, 0))).unwrap();
        assert_eq!(cell(&d.sheets[0], 0, 1), Some("?"));
        assert_eq!(cell(&d.sheets[0], 1, 3), None);
    }

    fn npy(descr: &str, shape: &str, data: &[u8]) -> Vec<u8> {
        let mut header = format!("{{'descr': {descr}, 'fortran_order': False, 'shape': {shape}, }}");
        while (10 + header.len() + 1) % 64 != 0 {
            header.push(' ');
        }
        header.push('\n');
        let mut v = b"\x93NUMPY\x01\x00".to_vec();
        v.extend_from_slice(&(header.len() as u16).to_le_bytes());
        v.extend_from_slice(header.as_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn npz_lists_arrays_and_shows_small_ones() {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("weights.npy", opts).unwrap();
        let data: Vec<u8> = [1.5f64, 2.0, 3.0, 4.0, 5.0, 6.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        zip.write_all(&npy("'<f8'", "(2, 3)", &data)).unwrap();
        zip.start_file("objects.npy", opts).unwrap();
        zip.write_all(&npy("'|O'", "(2,)", b"pickled")).unwrap();
        zip.start_file("broken.npy", opts).unwrap();
        zip.write_all(b"nonsense").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let d = npz_from(Cursor::new(bytes)).unwrap();
        let s = &d.sheets[0];
        assert_eq!(s.total_rows, Some(3));
        assert_eq!((cell(s, 0, 0), cell(s, 0, 1), cell(s, 0, 2), cell(s, 0, 3)), (Some("weights"), Some("2 × 3"), Some("f8"), Some("6")));
        assert_eq!(cell(s, 0, 5), Some(format!("{} weights", t("Tab:")).as_str()));
        assert_eq!((cell(s, 1, 2), cell(s, 1, 5)), (Some("O"), Some(t("Python objects are not read").as_str())));
        assert_eq!(cell(s, 2, 5), Some(t("Not a valid NumPy file.").as_str()));
        assert_eq!(d.sheets.len(), 2);
        assert_eq!(d.sheets[1].name, "weights");
        assert_eq!(cell(&d.sheets[1], 1, 0), Some("4"));
        assert!(npz_from(Cursor::new(b"not a zip".to_vec())).is_err());
    }

    // A tiny protobuf writer for test models.
    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    fn field_int(field: u64, v: u64, out: &mut Vec<u8>) {
        varint(field << 3, out);
        varint(v, out);
    }
    fn field_bytes(field: u64, b: &[u8], out: &mut Vec<u8>) {
        varint(field << 3 | 2, out);
        varint(b.len() as u64, out);
        out.extend_from_slice(b);
    }
    fn tensor(name: &str, dims: &[u64], dtype: u64, raw: usize) -> Vec<u8> {
        let mut t = vec![];
        let mut packed = vec![];
        for d in dims {
            varint(*d, &mut packed);
        }
        field_bytes(1, &packed, &mut t);
        field_int(2, dtype, &mut t);
        field_bytes(8, name.as_bytes(), &mut t);
        field_bytes(9, &vec![7u8; raw], &mut t);
        t
    }
    fn value_info(name: &str, dims: &[&str], elem: u64) -> Vec<u8> {
        let mut shape = vec![];
        for d in dims {
            let mut dim = vec![];
            match d.parse::<u64>() {
                Ok(n) => field_int(1, n, &mut dim),
                Err(_) => field_bytes(2, d.as_bytes(), &mut dim),
            }
            field_bytes(1, &dim, &mut shape);
        }
        let mut tensor_type = vec![];
        field_int(1, elem, &mut tensor_type);
        field_bytes(2, &shape, &mut tensor_type);
        let mut ty = vec![];
        field_bytes(1, &tensor_type, &mut ty);
        let mut v = vec![];
        field_bytes(1, name.as_bytes(), &mut v);
        field_bytes(2, &ty, &mut v);
        v
    }
    fn node(op: &str) -> Vec<u8> {
        let mut n = vec![];
        field_bytes(1, b"x", &mut n);
        field_bytes(4, op.as_bytes(), &mut n);
        n
    }

    fn model() -> Vec<u8> {
        let mut g = vec![];
        for op in ["Conv", "Relu", "Conv"] {
            field_bytes(1, &node(op), &mut g);
        }
        field_bytes(2, b"main", &mut g);
        field_bytes(5, &tensor("conv1.weight", &[8, 3, 3, 3], 1, 8 * 27 * 4), &mut g);
        field_bytes(5, &tensor("conv1.bias", &[8], 10, 16), &mut g);
        field_bytes(11, &value_info("pixels", &["batch", "3", "224", "224"], 1), &mut g);
        field_bytes(11, &value_info("conv1.bias", &["8"], 10), &mut g);
        field_bytes(12, &value_info("logits", &["batch", "1000"], 1), &mut g);
        let mut m = vec![];
        field_int(1, 8, &mut m);
        field_bytes(2, b"pytorch", &mut m);
        field_bytes(3, b"2.1", &mut m);
        let mut opset = vec![];
        field_int(2, 17, &mut opset);
        field_bytes(8, &opset, &mut m);
        field_bytes(7, &g, &mut m);
        let mut prop = vec![];
        field_bytes(1, b"author", &mut prop);
        field_bytes(2, b"me", &mut prop);
        field_bytes(14, &prop, &mut m);
        m
    }

    #[test]
    fn onnx_weights_inputs_and_operators() {
        let d = onnx_from(Cursor::new(model())).unwrap();
        assert_eq!(d.info, vec![format!("2 {}", t("weights")), format!("224 {}", t("parameters")), format!("3 {}", t("nodes")), "opset 17".into(), "pytorch 2.1".into()]);
        let w = &d.sheets[0];
        assert_eq!((cell(w, 0, 0), cell(w, 0, 1), cell(w, 0, 2), cell(w, 0, 3), cell(w, 0, 4)), (Some("conv1.weight"), Some("8 × 3 × 3 × 3"), Some("float32"), Some("216"), Some("864 B")));
        assert_eq!((cell(w, 1, 2), cell(w, 1, 4)), (Some("float16"), Some("16 B")));
        // The bias listed among the inputs by an older exporter is left out.
        let io = &d.sheets[1];
        assert_eq!(io.total_rows, Some(2));
        assert_eq!((cell(io, 0, 1), cell(io, 0, 2)), (Some("pixels"), Some("batch × 3 × 224 × 224")));
        assert_eq!((cell(io, 1, 0), cell(io, 1, 1)), (Some(t("output").as_str()), Some("logits")));
        let ops = &d.sheets[2];
        assert_eq!((cell(ops, 0, 0), cell(ops, 0, 1)), (Some("Conv"), Some("2")));
        let meta = &d.sheets[3];
        assert!(meta.rows.iter().any(|r| r[0].as_deref() == Some("author") && r[1].as_deref() == Some("me")));
    }

    #[test]
    fn onnx_refuses_what_is_not_a_model() {
        assert_eq!(onnx_from(Cursor::new(b"hello world, this is text".to_vec())).err(), Some(onnx_bad()));
        assert!(onnx_from(Cursor::new(Vec::new())).is_err());
        // Cut anywhere, a model is refused rather than misread.
        let m = model();
        for cut in [1, 10, m.len() / 2, m.len() - 1] {
            assert!(onnx_from(Cursor::new(m[..cut].to_vec())).is_err(), "{cut}");
        }
    }
}
