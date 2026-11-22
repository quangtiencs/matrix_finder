//! The table every tabular format is read into: sheets of columns and rows (at most
//! `MAX_ROWS` × `MAX_COLS`), with clipped cells and inferred column types.

pub const MAX_ROWS: usize = 1000;
pub const MAX_COLS: usize = 300;
const MAX_CELL: usize = 300;

pub type Cell = Option<String>;

pub struct Sheet {
    pub name: String,
    pub columns: Vec<String>,
    pub types: Vec<String>,
    pub numeric: Vec<bool>,
    pub rows: Vec<Vec<Cell>>,
    /// Total number of data rows when known cheaply.
    pub total_rows: Option<u64>,
}

pub struct TableData {
    pub format: &'static str,
    pub info: Vec<String>,
    pub sheets: Vec<Sheet>,
}

pub fn clip_cell(s: String) -> String {
    if s.len() > MAX_CELL {
        let mut end = MAX_CELL;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    } else {
        s
    }
}

/// Infer a column type from sample values (used for CSV and spreadsheets).
pub fn infer_type<'a>(values: impl Iterator<Item = &'a str>) -> &'static str {
    let (mut ints, mut floats, mut bools, mut dates, mut total) = (0, 0, 0, 0, 0);
    for v in values.map(str::trim).filter(|v| !v.is_empty()).take(200) {
        total += 1;
        if v.parse::<i64>().is_ok() {
            ints += 1;
        } else if v.parse::<f64>().is_ok() {
            floats += 1;
        } else if matches!(v.to_ascii_lowercase().as_str(), "true" | "false") {
            bools += 1;
        } else if chrono::NaiveDate::parse_from_str(v.get(..10).unwrap_or(v), "%Y-%m-%d").is_ok() {
            dates += 1;
        }
    }
    if total == 0 {
        "empty"
    } else if ints == total {
        "integer"
    } else if ints + floats == total {
        "number"
    } else if bools == total {
        "bool"
    } else if dates == total {
        "date"
    } else {
        "text"
    }
}

fn is_numeric_type(t: &str) -> bool {
    let t = t.to_ascii_lowercase();
    ["int", "long", "float", "double", "number", "decimal", "integer"].iter().any(|k| t.starts_with(k))
}

pub fn finish_sheet(name: String, columns: Vec<String>, types: Option<Vec<String>>, rows: Vec<Vec<Cell>>, total: Option<u64>) -> Sheet {
    let types = types.unwrap_or_else(|| {
        (0..columns.len())
            .map(|c| infer_type(rows.iter().filter_map(|r| r.get(c).and_then(|v| v.as_deref()))).to_string())
            .collect()
    });
    let numeric = types.iter().map(|t| is_numeric_type(t)).collect();
    Sheet { name, columns, types, numeric, rows, total_rows: total }
}
