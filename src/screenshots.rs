//! README screenshots, rendered headlessly with egui_kittest (wgpu):
//! `cargo test export_screenshots -- --ignored` (or `make screenshots`) writes every
//! scene to `docs/light/` and `docs/dark/`, then combines four of them into
//! `docs/showcase.png`.
//!
//! Every scene starts a fresh `App` with `HOME` pointing at a generated demo tree
//! (`/tmp/matrix_finder`), so the user's own settings and files never show up.
//! Set `MATRIX_FINDER_SHOTS=name,name` to render only some scenes (`showcase` is
//! one of the names; it reads the scenes it combines from `docs/`).
use super::*;
use egui::{pos2, Event, PointerButton, Pos2};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use image::RgbaImage;
use std::time::SystemTime;

const W: f32 = 1120.0;
const H: f32 = 700.0;

/// One rendering of all scenes: the folder it goes to and the themes it uses.
#[derive(Clone, Copy)]
struct Mode {
    dir: &'static str,
    theme: &'static str,
    /// The tiles of the `native-themes` scene: (theme, window chrome).
    native: [(&'static str, &'static str); 4],
}

const MODES: [Mode; 2] = [
    Mode {
        dir: "light",
        theme: "Breeze Light",
        native: [("Adwaita Light", "gnome"), ("Breeze Light", "kde"), ("macOS Light", "mac"), ("Windows 11 Light", "windows")],
    },
    Mode {
        dir: "dark",
        theme: "Breeze Dark",
        native: [("Adwaita Dark", "gnome"), ("Breeze Dark", "kde"), ("macOS Dark", "mac"), ("Windows 11 Dark", "windows")],
    },
];

thread_local! {
    /// The folder under `docs/` that `save` writes to.
    static MODE_DIR: std::cell::Cell<&'static str> = const { std::cell::Cell::new("light") };
}

struct Demo {
    home: PathBuf,
    demo: PathBuf,
    downloads: PathBuf,
}

// ------------------------------------------------------------------ fixture

/// Small deterministic PRNG (xorshift), so the demo data is the same on every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const CITIES: [&str; 5] = ["Hồ Chí Minh", "Hà Nội", "Đà Nẵng", "Huế", "Cần Thơ"];
const PRODUCTS: [(&str, f64); 7] = [
    ("Phở bò", 55000.0),
    ("Bánh mì", 25000.0),
    ("Cà phê", 30000.0),
    ("Trà sen", 35000.0),
    ("Nước mía", 15000.0),
    ("Chè ba màu", 28000.0),
    ("Bún chả", 50000.0),
];

struct Order {
    id: i64,
    /// Days since 1970-01-01.
    day: i32,
    city: &'static str,
    product: &'static str,
    qty: i64,
    price: f64,
    paid: bool,
}

fn orders(n: usize) -> Vec<Order> {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    (0..n)
        .map(|i| {
            let (product, price) = PRODUCTS[r.below(PRODUCTS.len() as u64) as usize];
            Order {
                id: 10001 + i as i64,
                day: 19723 + r.below(366) as i32,
                city: CITIES[r.below(CITIES.len() as u64) as usize],
                product,
                qty: 1 + r.below(20) as i64,
                price,
                paid: r.below(10) < 8,
            }
        })
        .collect()
}

fn ymd(day: i32) -> String {
    let d = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap() + chrono::Duration::days(day as i64);
    d.format("%Y-%m-%d").to_string()
}

fn write_csv(path: &Path, rows: &[Order]) {
    let mut s = String::from("order_id,date,city,product,qty,price,total,paid\n");
    for o in rows {
        s.push_str(&format!("{},{},{},{},{},{},{},{}\n", o.id, ymd(o.day), o.city, o.product, o.qty, o.price, o.qty as f64 * o.price, o.paid));
    }
    std::fs::write(path, s).unwrap();
}

fn write_parquet(path: &Path, rows: &[Order]) {
    use parquet::basic::{Compression, ZstdLevel};
    use parquet::data_type::{BoolType, ByteArray, ByteArrayType, DoubleType, Int32Type, Int64Type};
    use parquet::file::properties::WriterProperties;
    use parquet::file::writer::SerializedFileWriter;
    use parquet::schema::parser::parse_message_type;
    let schema = parse_message_type(
        "message orders { required int64 order_id; required int32 date (DATE); required binary city (UTF8);
         required binary product (UTF8); required int64 qty; required double price; required double total; required boolean paid; }",
    )
    .unwrap();
    let props = WriterProperties::builder().set_compression(Compression::ZSTD(ZstdLevel::default())).build();
    let mut w = SerializedFileWriter::new(std::fs::File::create(path).unwrap(), Arc::new(schema), Arc::new(props)).unwrap();
    let mut rg = w.next_row_group().unwrap();
    let ids: Vec<i64> = rows.iter().map(|o| o.id).collect();
    let days: Vec<i32> = rows.iter().map(|o| o.day).collect();
    let cities: Vec<ByteArray> = rows.iter().map(|o| ByteArray::from(o.city)).collect();
    let products: Vec<ByteArray> = rows.iter().map(|o| ByteArray::from(o.product)).collect();
    let qty: Vec<i64> = rows.iter().map(|o| o.qty).collect();
    let price: Vec<f64> = rows.iter().map(|o| o.price).collect();
    let total: Vec<f64> = rows.iter().map(|o| o.qty as f64 * o.price).collect();
    let paid: Vec<bool> = rows.iter().map(|o| o.paid).collect();
    macro_rules! col {
        ($t:ty, $v:expr) => {{
            let mut c = rg.next_column().unwrap().unwrap();
            c.typed::<$t>().write_batch($v, None, None).unwrap();
            c.close().unwrap();
        }};
    }
    col!(Int64Type, &ids);
    col!(Int32Type, &days);
    col!(ByteArrayType, &cities);
    col!(ByteArrayType, &products);
    col!(Int64Type, &qty);
    col!(DoubleType, &price);
    col!(DoubleType, &total);
    col!(BoolType, &paid);
    rg.close().unwrap();
    w.close().unwrap();
}

fn write_avro(path: &Path, rows: &[Order]) {
    use apache_avro::{types::Record, types::Value, Codec, Schema, Writer};
    let schema = Schema::parse_str(
        r#"{"type":"record","name":"Order","fields":[
            {"name":"order_id","type":"long"},{"name":"date","type":{"type":"int","logicalType":"date"}},
            {"name":"city","type":"string"},{"name":"product","type":"string"},{"name":"qty","type":"long"},
            {"name":"price","type":"double"},{"name":"total","type":"double"},{"name":"paid","type":"boolean"}]}"#,
    )
    .unwrap();
    let mut w = Writer::with_codec(&schema, Vec::new(), Codec::Snappy);
    for o in rows {
        let mut r = Record::new(&schema).unwrap();
        r.put("order_id", o.id);
        r.put("date", Value::Date(o.day));
        r.put("city", o.city);
        r.put("product", o.product);
        r.put("qty", o.qty);
        r.put("price", o.price);
        r.put("total", o.qty as f64 * o.price);
        r.put("paid", o.paid);
        w.append(r).unwrap();
    }
    std::fs::write(path, w.into_inner().unwrap()).unwrap();
}

fn write_sqlite(path: &Path, rows: &[Order]) {
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch("CREATE TABLE orders(id INTEGER PRIMARY KEY, date TEXT, city TEXT, product TEXT, qty INTEGER, price REAL); CREATE VIEW paid_by_city AS SELECT city, count(*) AS orders, sum(qty * price) AS total FROM orders GROUP BY city;").unwrap();
    for o in rows {
        c.execute("INSERT INTO orders VALUES(?1, ?2, ?3, ?4, ?5, ?6)", rusqlite::params![o.id, ymd(o.day), o.city, o.product, o.qty, o.price]).unwrap();
    }
}

/// A smooth ring, to show off the 3D view.
fn write_torus_stl(path: &Path) {
    let (n, m) = (96, 48);
    let point = |i: usize, j: usize| {
        let (a, b) = (std::f32::consts::TAU * i as f32 / n as f32, std::f32::consts::TAU * j as f32 / m as f32);
        let r = 20.0 + 8.0 * b.cos();
        [r * a.cos(), r * a.sin(), 8.0 * b.sin()]
    };
    let mut out = vec![0u8; 80];
    out.extend(((n * m * 2) as u32).to_le_bytes());
    for i in 0..n {
        for j in 0..m {
            let (a, b, c, d) = (point(i, j), point(i + 1, j), point(i + 1, j + 1), point(i, j + 1));
            for tri in [[a, b, c], [a, c, d]] {
                out.extend([0u8; 12]);
                out.extend(tri.iter().flatten().flat_map(|f| f.to_le_bytes()));
                out.extend([0u8; 2]);
            }
        }
    }
    touch(path, &out);
}

fn write_epub(path: &Path) {
    use std::io::Write;
    let chapter = |title: &str, body: &str| format!("<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{title}</title></head><body><h1>{title}</h1>{body}</body></html>");
    let one = chapter(
        "The Long Road South",
        "<p>The train left <b>Hà Nội</b> before dawn, and by noon the rice fields had given way to <i>limestone hills</i> that rose straight out of the green.</p><p>She kept three things in her bag:</p><ul><li>a notebook</li><li>her grandmother's letters</li><li>a ticket with no return date</li></ul><blockquote><p>Every road is long until you have walked it.</p></blockquote><p>By evening the air smelled of salt, and the conductor called out <b>Đà Nẵng</b> as if it were a question.</p>",
    );
    let two = chapter("Arrival", "<p>Nobody was waiting at the station, which was exactly what she had hoped for.</p>");
    let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let o = zip::write::SimpleFileOptions::default();
    let opf = r#"<package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>A Short Story</dc:title><dc:creator>Matrix Finder demo</dc:creator></metadata><manifest><item id="a" href="one.xhtml"/><item id="b" href="two.xhtml"/></manifest><spine><itemref idref="a"/><itemref idref="b"/></spine></package>"#;
    for (name, text) in [("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>"#), ("OPS/book.opf", opf), ("OPS/one.xhtml", one.as_str()), ("OPS/two.xhtml", two.as_str())] {
        z.start_file(name, o).unwrap();
        z.write_all(text.as_bytes()).unwrap();
    }
    z.finish().unwrap();
}

fn write_xlsx(path: &Path, rows: &[Order]) {
    use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};
    let mut wb = Workbook::new();
    let date_fmt = Format::new().set_num_format("yyyy-mm-dd");
    let bold = Format::new().set_bold();
    let ws = wb.add_worksheet();
    ws.set_name("Orders").unwrap();
    for (c, h) in ["order_id", "date", "city", "product", "qty", "price", "total", "paid"].iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *h, &bold).unwrap();
    }
    for (i, o) in rows.iter().enumerate() {
        let r = i as u32 + 1;
        ws.write_number(r, 0, o.id as f64).unwrap();
        ws.write_datetime_with_format(r, 1, &ExcelDateTime::parse_from_str(&ymd(o.day)).unwrap(), &date_fmt).unwrap();
        ws.write_string(r, 2, o.city).unwrap();
        ws.write_string(r, 3, o.product).unwrap();
        ws.write_number(r, 4, o.qty as f64).unwrap();
        ws.write_number(r, 5, o.price).unwrap();
        ws.write_number(r, 6, o.qty as f64 * o.price).unwrap();
        ws.write_boolean(r, 7, o.paid).unwrap();
    }
    let ws = wb.add_worksheet();
    ws.set_name("Summary").unwrap();
    ws.write_string_with_format(0, 0, "city", &bold).unwrap();
    ws.write_string_with_format(0, 1, "orders", &bold).unwrap();
    ws.write_string_with_format(0, 2, "revenue", &bold).unwrap();
    for (i, city) in CITIES.iter().enumerate() {
        let of: Vec<&Order> = rows.iter().filter(|o| o.city == *city).collect();
        ws.write_string(i as u32 + 1, 0, *city).unwrap();
        ws.write_number(i as u32 + 1, 1, of.len() as f64).unwrap();
        ws.write_number(i as u32 + 1, 2, of.iter().map(|o| o.qty as f64 * o.price).sum::<f64>()).unwrap();
    }
    wb.save(path).unwrap();
}

/// A small multi-page PDF (standard fonts, a coloured title band and a bar chart).
fn write_pdf(path: &Path, title: &str, pages: usize) {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        String::new(), // pages, filled in below
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".into(),
    ];
    let mut kids = vec![];
    let body = [
        "Revenue grew in every region this quarter, led by Hue and Da Nang.",
        "Coffee and pho remain the best sellers; sugarcane juice doubled.",
        "Online orders now make up a third of all sales.",
        "Average basket size rose from 6.1 to 7.4 items.",
        "Two new stores opened in Can Tho and Hai Phong.",
        "Delivery times fell by 18 % after the warehouse move.",
        "Customer satisfaction reached 4.6 out of 5.",
        "Next quarter: loyalty programme and a seasonal menu.",
    ];
    let colors = ["0.85 0.55 0.15", "0.20 0.50 0.80", "0.60 0.30 0.70", "0.25 0.60 0.40"];
    for p in 0..pages {
        let mut s = String::new();
        s.push_str("0.11 0.36 0.30 rg 50 700 512 44 re f\n");
        s.push_str(&format!("BT /F2 20 Tf 1 1 1 rg 62 715 Td ({title} - Page {}) Tj ET\n", p + 1));
        s.push_str("BT /F1 12 Tf 0 0 0 rg 62 670 Td 18 TL\n");
        for i in 0..body.len() {
            s.push_str(&format!("({}) Tj T*\n", body[(i + p) % body.len()]));
        }
        s.push_str("ET\n");
        for (i, c) in colors.iter().enumerate() {
            let h = 60 + ((i * 47 + p * 31) % 140);
            s.push_str(&format!("{c} rg {} 220 56 {h} re f\n", 80 + i * 110));
        }
        s.push_str("0.3 0.3 0.3 RG 1 w 62 218 m 540 218 l S\n");
        let content = objs.len() + 1;
        objs.push(format!("<< /Length {} >>\nstream\n{s}endstream", s.len()));
        kids.push(objs.len() + 1);
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {content} 0 R >>"
        ));
    }
    objs[1] = format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.iter().map(|k| format!("{k} 0 R")).collect::<Vec<_>>().join(" "));
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = vec![];
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
    }
    let xref = out.len();
    out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1));
    for o in offsets {
        out.push_str(&format!("{o:010} 00000 n \n"));
    }
    out.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1));
    touch(path, out.as_bytes());
}

fn bar_chart_png(w: u32, h: u32) -> Vec<u8> {
    let vals = [0.42f32, 0.55, 0.61, 0.48, 0.72, 0.80, 0.66, 0.90, 0.77, 0.85, 0.70, 0.95];
    let img = RgbaImage::from_fn(w, h, |x, y| {
        let slot = w as f32 / vals.len() as f32;
        let i = ((x as f32 / slot) as usize).min(vals.len() - 1);
        let inside = (x as f32 - i as f32 * slot) > slot * 0.18 && (x as f32 - i as f32 * slot) < slot * 0.82;
        let top = h as f32 * (1.0 - vals[i] * 0.9);
        if inside && (y as f32) >= top {
            image::Rgba([0x1f, 0x77, 0xb4, 255])
        } else if y == h - 1 {
            image::Rgba([120, 120, 120, 255])
        } else {
            image::Rgba([255, 255, 255, 255])
        }
    });
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

fn photo(w: u32, h: u32, seed: u32, fmt: image::ImageFormat) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        let sky = fy < 0.55 + 0.08 * (fx * 6.0 + seed as f32).sin();
        if sky {
            image::Rgb([(90.0 + 120.0 * fy) as u8, (150.0 + 70.0 * fy) as u8, 235])
        } else {
            image::Rgb([(40.0 + 60.0 * fx) as u8, (120.0 + 50.0 * (1.0 - fy)) as u8, (60.0 + 20.0 * fx) as u8])
        }
    });
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut buf, fmt).unwrap();
    buf.into_inner()
}

fn write_notebook(path: &Path) {
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD.encode(bar_chart_png(520, 180));
    let nb = serde_json::json!({
        "nbformat": 4, "nbformat_minor": 5,
        "metadata": {"kernelspec": {"name": "python3", "display_name": "Python 3", "language": "python"},
                     "language_info": {"name": "python"}},
        "cells": [
            {"cell_type": "markdown", "metadata": {}, "source": [
                "# Sales analysis\n", "Load the **orders** and plot revenue by month.\n", "\n", "- clean data\n", "- aggregate\n"]},
            {"cell_type": "code", "execution_count": 1, "metadata": {}, "source": [
                "import pandas as pd\n", "df = pd.read_parquet('orders.parquet')\n", "print('rows:', len(df))\n", "print('cols:', len(df.columns))"],
             "outputs": [{"output_type": "stream", "name": "stdout", "text": ["rows: 2500\n", "cols: 8\n"]}]},
            {"cell_type": "code", "execution_count": 2, "metadata": {}, "source": ["df.groupby('city')['total'].sum()"],
             "outputs": [{"output_type": "execute_result", "execution_count": 2, "metadata": {}, "data": {"text/plain": [
                "city\n", "Cần Thơ        6512000.0\n", "Huế           6987000.0\n", "Hà Nội        7120000.0\n", "Name: total, dtype: float64"]}}]},
            {"cell_type": "code", "execution_count": 3, "metadata": {}, "source": ["df.resample('M', on='date')['total'].sum().plot(kind='bar')"],
             "outputs": [{"output_type": "display_data", "metadata": {}, "data": {"image/png": png, "text/plain": ["<Axes>"]}}]},
            {"cell_type": "code", "execution_count": 4, "metadata": {}, "source": ["df['price'].describe()"], "outputs": []}
        ]
    });
    touch(path, serde_json::to_string_pretty(&nb).unwrap().as_bytes());
}

const README_MD: &str = "# Matrix Finder demo\n\nSome **bold**, *italic* and `inline code`. A [link](https://example.com).\n\n\
- Dual panels\n- Archives: zip, 7z, rar\n  1. nested item\n- [x] Task done\n\n```rust\nfn main() { println!(\"hi\"); }\n```\n\n\
| Format | Read | Write |\n|---|:-:|:-:|\n| zip | ✓ | ✓ |\n| rar | ✓ | ✓ |\n\n> Quote block\n";

fn touch(path: &Path, bytes: &[u8]) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

fn zip_of(src: &[PathBuf], dest: &Path, fmt: ArchiveFormat) {
    archive::create(src, None, dest, fmt, 6, &[], &ops::Progress::default()).unwrap();
}

/// Short, readable root for the breadcrumbs (`/ › tmp › matrix_finder › home › user`).
fn demo_root() -> PathBuf {
    // Stress runs happen side by side, each with its own demo folder.
    if let Some(root) = std::env::var_os("MATRIX_FINDER_DEMO_ROOT") {
        return root.into();
    }
    if cfg!(unix) { PathBuf::from("/tmp/matrix_finder") } else { std::env::temp_dir().join("matrix_finder") }
}

fn build_demo() -> Demo {
    let root = demo_root();
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home").join("user");
    let demo = home.join("demo");
    let downloads = home.join("Downloads");
    // The system's own name for the video folder (bookmarks find it through `dirs`).
    let movies = if cfg!(target_os = "macos") { "Movies" } else { "Videos" };
    for d in ["Desktop", "Documents", "Downloads", "Music", "Pictures", "Projects", movies] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    touch(&home.join(".zshrc"), b"PROMPT='%F{blue}%1~%f %# '\nexport CLICOLOR=1\nunsetopt PROMPT_SP\n");
    touch(&home.join("Documents/notes.md"), b"# Notes\n");
    touch(&home.join("Pictures/hoi-an.jpg"), &photo(800, 533, 1, image::ImageFormat::Jpeg));

    // ~/Downloads: checksum demo.
    touch(&downloads.join("photo.jpg"), &photo(900, 600, 2, image::ImageFormat::Jpeg));
    write_pdf(&downloads.join("release-notes.pdf"), "Release Notes", 2);
    touch(&downloads.join("ubuntu-26.04-desktop.iso"), b"hello world");
    let sums: Vec<(String, String)> = ["ubuntu-26.04-desktop.iso", "release-notes.pdf"]
        .iter()
        .map(|n| {
            let f = std::fs::File::open(downloads.join(n)).unwrap();
            (n.to_string(), crate::checksum::hash_reader(f, crate::checksum::Algo::Sha256, |_| true).unwrap())
        })
        .collect();
    touch(&downloads.join("SHA256SUMS"), sums.iter().map(|(n, h)| format!("{h}  {n}\n")).collect::<String>().as_bytes());

    // ~/Projects/matrix_finder: something to run in the terminal.
    let proj = home.join("Projects/matrix_finder");
    touch(&proj.join("Cargo.toml"), b"[package]\nname = \"matrix_finder\"\nversion = \"0.1.0\"\n");
    touch(&proj.join("README.md"), README_MD.as_bytes());
    touch(&proj.join("src/main.rs"), b"fn main() {}\n");
    touch(&proj.join("LICENSE"), b"MIT\n");
    touch(
        &proj.join("build.log"),
        b"Docs: https://github.com/alacritty/alacritty\n\x1b[32m[ok]\x1b[0m build  \x1b[33m[!]\x1b[0m 2 warnings  \x1b[31m[x]\x1b[0m 0 errors\n",
    );
    std::fs::create_dir_all(proj.join("assets")).unwrap();

    // ~/demo: the general demo tree.
    let rows = orders(2500);
    for d in ["big", "images", "out", "rename", "Thư mục tiếng Việt"] {
        std::fs::create_dir_all(demo.join(d)).unwrap();
    }
    let mut r = Rng(42);
    let noise: Vec<u8> = (0..300_000).map(|_| r.next() as u8).collect();
    touch(&demo.join("data.bin"), &noise);
    touch(&demo.join("file1.log"), b"1\n");
    touch(&demo.join("file2.log"), b"2\n");
    touch(&demo.join("file10.log"), b"10\n");
    touch(&demo.join("notes.txt"), b"todo\n");
    write_csv(&demo.join("orders.csv"), &rows);
    write_parquet(&demo.join("orders.parquet"), &rows);
    write_avro(&demo.join("orders.avro"), &rows);
    std::fs::create_dir_all(demo.join("data")).unwrap();
    write_csv(&demo.join("data/orders.csv"), &rows);
    write_parquet(&demo.join("data/orders.parquet"), &rows);
    write_avro(&demo.join("data/orders.avro"), &rows);
    write_xlsx(&demo.join("data/orders.xlsx"), &rows[..300]);
    touch(&demo.join("src/filter.rs"), include_str!("filter.rs").as_bytes());
    touch(&demo.join("src/theme.rs"), include_str!("theme.rs").as_bytes());
    for i in 1..=3 {
        touch(&demo.join(format!("images/photo-{i}.jpg")), &photo(640, 427, i, image::ImageFormat::Jpeg));
        touch(&demo.join(format!("big/video_{i}.mkv")), &noise[..4096]);
    }
    touch(&demo.join("images/chart.png"), &bar_chart_png(600, 300));
    for (i, n) in ["IMG_0001.JPG", "IMG_0002.JPG", "IMG_0003.JPG", "scan 1.pdf", "scan 2.pdf"].iter().enumerate() {
        touch(&demo.join("rename").join(n), &[b'0' + i as u8]);
    }
    for n in [
        "Báo cáo THÁNG 1.docx",
        "Báo cáo tháng 2 - bản nháp.docx",
        "Ghi chú họp.txt",
        "ảnh  hội an   (bản nháp).jpg",
        "Ảnh Đà Nẵng 01.JPG",
        "Ảnh Đà Nẵng 02.JPG",
    ] {
        touch(&demo.join("rename2").join(n), b"x\n");
    }
    touch(&demo.join("Thư mục tiếng Việt/ghi chú.txt"), "Xin chào".as_bytes());
    let docs = demo.join("docs");
    touch(&docs.join("README.md"), README_MD.as_bytes());
    touch(&docs.join("data.bin"), &noise);
    zip_of(&[demo.join("src")], &docs.join("src.zip"), ArchiveFormat::Zip);
    zip_of(&[demo.join("images")], &docs.join("bundle.zip"), ArchiveFormat::Zip);
    zip_of(&[demo.join("images")], &demo.join("bundle.zip"), ArchiveFormat::Zip);
    zip_of(&[demo.join("src")], &demo.join("archive.tar.gz"), ArchiveFormat::TarGz);
    write_notebook(&demo.join("papers/analysis.ipynb"));
    write_pdf(&demo.join("papers/report.pdf"), "Quarterly Report", 3);
    // ~/demo/formats: one file for the newer Quick View formats.
    let formats = demo.join("formats");
    std::fs::create_dir_all(&formats).unwrap();
    touch(&formats.join("Source Sans 3.otf"), include_bytes!("../assets/fonts/SourceSans3-Regular.otf"));
    touch(&formats.join("server.pem"), include_bytes!("../assets/testdata/test-cert.pem"));
    write_torus_stl(&formats.join("torus.stl"));
    write_sqlite(&formats.join("shop.sqlite"), &rows[..600]);
    write_epub(&formats.join("A Short Story.epub"));
    let jsonl: String = rows[..200].iter().map(|o| format!("{}\n", serde_json::json!({"order": o.id, "city": o.city, "product": o.product, "qty": o.qty, "paid": o.paid}))).collect();
    touch(&formats.join("orders.jsonl"), jsonl.as_bytes());
    // A home-like folder with one file of each kind (icons).
    let h2 = demo.join("home");
    for d in ["Desktop", "Documents", "Downloads", "Music", "Pictures", "Projects", "Videos"] {
        std::fs::create_dir_all(h2.join(d)).unwrap();
    }
    for f in [
        "app.py", "backup.zip", "data.json", "index.html", "letter.docx", "logo.svg", "main.rs", "movie.mp4", "notes.md", "photo.jpg",
        "readme.txt", "report.pdf", "setup.sh", "sheet.xlsx", "slides.pptx",
    ] {
        touch(&h2.join(f), b"{}");
    }

    // Fixed, plausible timestamps (newest first in walk order).
    let base = chrono::NaiveDate::from_ymd_opt(2026, 11, 4).unwrap().and_hms_opt(16, 30, 0).unwrap();
    let base = base.and_local_timezone(chrono::Local).single().map(SystemTime::from).unwrap_or_else(SystemTime::now);
    for (i, e) in walkdir::WalkDir::new(&root).contents_first(true).sort_by_file_name().into_iter().flatten().enumerate() {
        let t = base - Duration::from_secs((i as u64 * 7919) % (3 * 86400));
        if let Ok(f) = std::fs::File::open(e.path()) {
            let _ = f.set_modified(t);
        }
    }
    Demo { home, demo, downloads }
}

// ------------------------------------------------------------------ harness

fn settings(theme: &str, demo: &Demo) -> Settings {
    let mut cfg = Settings::default();
    cfg.theme = theme.into();
    set_terminal_theme(&mut cfg, theme);
    cfg.start_in_home = false;
    cfg.session = None;
    cfg.left_paths = vec![demo.demo.to_string_lossy().to_string()];
    cfg.bookmarks = vec![
        Bookmark::new("Demo", demo.demo.to_string_lossy().to_string()),
        Bookmark::new("Docs", demo.demo.join("docs").to_string_lossy().to_string()),
    ];
    cfg
}

/// Terminal profiles have their own colour theme (Breeze Light by default).
fn set_terminal_theme(cfg: &mut Settings, theme: &str) {
    for p in &mut cfg.terminal_profiles {
        p.theme = theme.into();
    }
}

fn app_harness(cfg: &Settings, chrome: &str) -> Harness<'static, App> {
    std::fs::create_dir_all(crate::config::config_dir()).unwrap();
    std::fs::write(crate::config::config_file(), serde_json::to_string_pretty(cfg).unwrap()).unwrap();
    std::env::set_var("MATRIX_FINDER_CHROME", chrome);
    let mut h = Harness::builder()
        .with_size(egui::vec2(W, H))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| App::new(cc, crate::chrome::Chrome::detect(true), None));
    let app = h.state_mut();
    // Only the root file system: the sidebar must not show this machine's volumes.
    app.drives = crate::vfs::drives().into_iter().filter(|d| d.1 == Path::new("/")).map(|mut d| {
        d.0 = "/".into();
        d
    }).collect();
    app.drives_at = Instant::now() + Duration::from_secs(3600);
    // macOS keeps the settings in ~/Library, which would show up in the demo home folder.
    if let Some(home) = dirs::home_dir() {
        let _ = std::fs::remove_dir_all(home.join("Library"));
    }
    settle(&mut h, 4);
    h
}

/// Run frames while background work (listings, previews, terminals) finishes.
fn settle(h: &mut Harness<'static, App>, frames: usize) {
    for _ in 0..frames {
        h.step();
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn wait_until(h: &mut Harness<'static, App>, secs: f32, mut done: impl FnMut(&App) -> bool) {
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f32() < secs {
        h.step();
        if done(h.state()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    settle(h, 4);
}

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs")
}

fn save(img: &RgbaImage, name: &str) {
    let dir = out_dir().join(MODE_DIR.get());
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{name}.png"));
    img.save(&p).unwrap();
    eprintln!("wrote {}", p.display());
}

fn shot(h: &mut Harness<'static, App>, name: &str) {
    settle(h, 3);
    let img = h.render().unwrap();
    save(&img, name);
}

fn push(h: &mut Harness<'static, App>, e: Event) {
    h.input_mut().events.push(e);
}

fn move_to(h: &mut Harness<'static, App>, p: Pos2) {
    push(h, Event::PointerMoved(p));
    settle(h, 2);
}

fn button(h: &mut Harness<'static, App>, p: Pos2, b: PointerButton, pressed: bool, m: Modifiers) {
    push(h, Event::PointerButton { pos: p, button: b, pressed, modifiers: m });
    h.input_mut().modifiers = m;
    settle(h, 2);
}

fn right_click(h: &mut Harness<'static, App>, p: Pos2) {
    move_to(h, p);
    button(h, p, PointerButton::Secondary, true, Modifiers::NONE);
    button(h, p, PointerButton::Secondary, false, Modifiers::NONE);
}

/// Click the text input whose centre is inside `area`, then type `text` into it.
fn type_into(h: &mut Harness<'static, App>, area: egui::Rect, text: &str) -> bool {
    let at = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .filter_map(|n| n.bounding_box())
        .map(|r| pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32))
        .find(|c| area.contains(*c));
    let Some(at) = at else { return false };
    move_to(h, at);
    button(h, at, PointerButton::Primary, true, Modifiers::NONE);
    button(h, at, PointerButton::Primary, false, Modifiers::NONE);
    push(h, Event::Text(text.into()));
    settle(h, 4);
    true
}

/// Type into a built-in terminal once its shell has started.
fn term_type(h: &mut Harness<'static, App>, side: usize, uid: u64, text: &str) {
    wait_until(h, 2.0, |_| false);
    let app = h.state_mut();
    let _ = side;
    let mut all = app.panes.iter().chain(app.strip.tabs.iter().flat_map(|w| w.panes.iter()));
    if let Some(t) = all.find(|p| p.tab().uid == uid).and_then(|p| p.tab().term.as_ref()) {
        t.write(text.as_bytes().to_vec());
    }
    std::thread::sleep(Duration::from_millis(800));
    wait_until(h, 1.5, |_| false);
}

/// Centre of the first accessibility node with this label (and an optional x range).
fn node_center(h: &Harness<'static, App>, label: &str, x_min: f32) -> Option<Pos2> {
    h.query_all_by_label(label).find_map(|n| {
        let r = n.bounding_box()?;
        ((r.x0 as f32) >= x_min).then(|| pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32))
    })
}

impl App {
    /// Show `dir` in pane `side` (creating panes on the right as needed), cursor on `cursor`.
    fn shot_pane(&mut self, side: usize, dir: &Path, cursor: Option<&str>) {
        while self.panes.len() <= side {
            let last = self.panes.len() - 1;
            self.split_pane(last, SplitDir::Right, false);
        }
        self.panes[side] = self.files_pane(Location::Local(dir.to_path_buf()));
        self.shot_wait_listing(side);
        if let Some(c) = cursor {
            self.panes[side].tab_mut().select_name(c);
        }
        self.quick = None;
    }

    fn shot_wait_listing(&mut self, side: usize) {
        let t0 = Instant::now();
        while self.panes[side].tab().is_loading() && t0.elapsed() < Duration::from_secs(5) {
            let ctx = lc!(self);
            self.panes[side].tab_mut().poll(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn shot_mark(&mut self, side: usize, names: &[&str]) {
        let tab = self.panes[side].tab_mut();
        tab.marked = names.iter().map(|s| s.to_string()).collect();
    }

    fn shot_focus(&mut self, side: usize) {
        self.active = side;
        self.prev_active = if side == 0 { 1.min(self.panes.len() - 1) } else { 0 };
        self.last_active = side;
    }

    /// Open a terminal tab at the end of the tab bar (shown); returns its window's uid.
    fn shot_terminal(&mut self, dir: &Path, title: Option<&str>, group: Option<u32>) -> u64 {
        let at = self.strip.tabs.len();
        self.new_terminal_tab(at, group, None, Some(dir.to_path_buf()));
        self.strip.tabs[self.strip.active].custom_title = title.map(str::to_string);
        self.panes[0].tab().uid
    }

    /// Open a file tab at the end of the tab bar (shown).
    fn shot_files_tab(&mut self, dir: &Path, group: Option<u32>) {
        let at = self.strip.tabs.len();
        self.new_files_tab(at, group, Some(Location::Local(dir.to_path_buf())));
        self.shot_wait_listing(0);
    }
}

/// Draw the macOS traffic lights where the system would put them (headless renders have none).
fn paint_traffic_lights(img: &mut RgbaImage) {
    for (cx, col) in [(20.0f32, [0xff, 0x5f, 0x57]), (40.0, [0xfe, 0xbc, 0x2e]), (60.0, [0x28, 0xc8, 0x40])] {
        let cy = 14.0f32;
        for y in 6..23u32 {
            for x in (cx as u32 - 8)..(cx as u32 + 8) {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let a = (6.5 - d).clamp(0.0, 1.0);
                if a > 0.0 {
                    let p = img.get_pixel_mut(x, y);
                    for c in 0..3 {
                        p.0[c] = (p.0[c] as f32 * (1.0 - a) + col[c] as f32 * a) as u8;
                    }
                }
            }
        }
    }
}

/// Point HOME (settings, bookmarks, the shell) at the demo folder and build it.
fn demo_env() -> Demo {
    let home = demo_root().join("home").join("user");
    std::fs::create_dir_all(&home).unwrap();
    // Before anything reads the environment: settings, bookmarks and the terminal use HOME.
    std::env::set_var("HOME", &home);
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::set_var("SHELL", if Path::new("/bin/zsh").exists() { "/bin/zsh" } else { "/bin/bash" });
    std::env::set_var("ZDOTDIR", &home);
    build_demo()
}

/// Press at `from`, move to `to` in small steps, release there.
fn drag(h: &mut Harness<'static, App>, from: Pos2, to: Pos2) {
    move_to(h, from);
    button(h, from, PointerButton::Primary, true, Modifiers::NONE);
    for k in 1..=12 {
        move_to(h, from + (to - from) * (k as f32 / 12.0));
    }
    settle(h, 2);
    button(h, to, PointerButton::Primary, false, Modifiers::NONE);
    settle(h, 3);
}

/// Drag a dialog (`id` is its title, unless it sets its own) to the middle of the
/// window, as a user would: egui opens dialogs towards the top left.
fn center_dialog(h: &mut Harness<'static, App>, id: &str) {
    let r = h.ctx.memory(|m| m.area_rect(egui::Id::new(id))).unwrap_or_else(|| panic!("no dialog {id}"));
    let grab = r.left_top() + egui::vec2(40.0, 30.0);
    drag(h, grab, grab + (pos2(W / 2.0, H / 2.0) - r.center()));
}

/// Scroll the Settings page down, leaving the pointer on nothing that reacts to it.
fn scroll_settings(h: &mut Harness<'static, App>, by: f32) {
    move_to(h, pos2(W * 0.7, H * 0.6));
    push(h, Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -by), modifiers: Modifiers::NONE });
    settle(h, 4);
    move_to(h, pos2(330.0, H - 12.0));
    settle(h, 2);
}

/// The grip of window `side` (left end of its title row).
fn grip(h: &Harness<'static, App>, side: usize) -> Pos2 {
    let r = h.state().pane_rects.iter().find(|(i, _)| *i == side).unwrap().1;
    pos2(r.left() + 13.0, r.top() + 20.0)
}

fn window_rect(h: &Harness<'static, App>, side: usize) -> egui::Rect {
    h.state().pane_rects.iter().find(|(i, _)| *i == side).unwrap().1
}

fn view_uids(app: &App) -> Vec<u64> {
    app.panes.iter().map(|p| p.tab().uid).collect()
}

/// Windows dragged by their grips: swap, dock at an edge, out to the tab bar
/// and into another tab; a tab dragged onto a window edge merges into it.
#[test]
#[ignore]
fn window_drag_and_drop() {
    let d = demo_env();
    let mut h = app_harness(&settings("Elegant Dark", &d), "gnome");
    let app = h.state_mut();
    app.shot_pane(0, &d.demo, None);
    app.shot_pane(1, &d.demo.join("docs"), None); // [demo | docs]
    settle(&mut h, 4);
    let before = view_uids(h.state());

    // Middle of the other window: the two swap places.
    let to = window_rect(&h, 1).center();
    let g = grip(&h, 0);
    drag(&mut h, g, to);
    assert_eq!(view_uids(h.state()), vec![before[1], before[0]], "swapped");
    assert_eq!(h.state().layout.leaves(), vec![0, 1]);

    // Bottom edge of the other window: stacked under it, one column left.
    let r = window_rect(&h, 1);
    let g = grip(&h, 0);
    drag(&mut h, g, pos2(r.center().x, r.bottom() - 20.0));
    assert_eq!(h.state().layout.dims(), (1, 2), "docked below");
    assert_eq!(h.state().panes.len(), 2);

    // Onto the tab bar, right of the only tab: it becomes a second tab.
    let moved = h.state().panes[h.state().layout.leaves()[1]].tab().uid;
    let side = h.state().layout.leaves()[1];
    let g = grip(&h, side);
    drag(&mut h, g, pos2(560.0, 61.0));
    let app = h.state();
    assert_eq!(app.strip.tabs.len(), 2, "a new tab");
    assert_eq!(app.panes.len(), 1);
    assert_eq!(app.panes[0].tab().uid, moved, "the new tab is shown with the moved window");

    // Drag the first tab onto the right edge of the shown window: the tabs merge.
    let first = h.state().strip.tabs[0].uid;
    let tab_x = 230.0;
    let r = window_rect(&h, 0);
    move_to(&mut h, pos2(tab_x, 61.0));
    drag(&mut h, pos2(tab_x, 61.0), pos2(r.right() - 30.0, r.center().y));
    let app = h.state();
    assert_eq!(app.strip.tabs.len(), 1, "merged into one tab");
    assert!(app.strip.index_of(first).is_none() || app.live == first);
    assert_eq!(app.panes.len(), 2);
    assert_eq!(app.layout.dims(), (2, 1), "side by side");

    // A window dropped on the middle of another tab moves into that tab.
    let (at, group) = h.state().next_slot();
    h.state_mut().new_files_tab(at, group, Some(Location::Local(d.demo.join("src"))));
    let first_tab = h.state().strip.tabs[0].uid;
    let tab1 = h.state().strip.tabs[1].uid;
    h.state_mut().show_tab(0);
    settle(&mut h, 3);
    let moving = h.state().panes[1].tab().uid;
    // Two tabs at their full 210 px width; the second one's middle.
    let bar_left = window_rect(&h, 0).left();
    let g = grip(&h, 1);
    drag(&mut h, g, pos2(bar_left + 210.0 + 2.0 + 105.0, 61.0));
    let app = h.state();
    assert_eq!(app.live, tab1, "the target tab is shown");
    assert!(app.panes.iter().any(|p| p.tab().uid == moving), "with the moved window");
    assert_eq!(app.panes.len(), 2);
    let w0 = app.strip.tabs.iter().find(|w| w.uid == first_tab).unwrap();
    assert_eq!(w0.panes.len(), 1, "the old tab keeps one window");
}

/// File types opening with a chosen app, and going back to the system's default. A
/// small script stands in for the apps; no test here starts a real one.
#[test]
#[ignore]
fn default_apps_choose_and_reset() {
    use std::os::unix::fs::PermissionsExt;
    let d = demo_env();
    let log = demo_root().join("opened.log");
    let script = demo_root().join("handler.sh");
    std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> {}\n", log.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let handler = script.to_string_lossy().to_string();
    let mut cfg = settings("Breeze Light", &d);
    cfg.default_apps.insert("pdf".into(), handler.clone());
    cfg.default_apps.insert("zip".into(), handler.clone());
    let mut h = app_harness(&cfg, "gnome");
    let opened = |log: &Path, name: &str| std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l| l.ends_with(name)).count();

    // Enter and F4 both use the chosen app.
    h.state_mut().shot_pane(0, &d.demo.join("papers"), Some("report.pdf"));
    h.state_mut().run_cmd(Cmd::Open);
    h.state_mut().run_cmd(Cmd::OpenDefault);
    wait_until(&mut h, 3.0, |_| opened(&log, "report.pdf") == 2);
    assert_eq!(opened(&log, "report.pdf"), 2);

    // A chosen app takes over from browsing inside an archive; without one it browses again.
    h.state_mut().shot_pane(0, &d.demo, Some("bundle.zip"));
    h.state_mut().run_cmd(Cmd::Open);
    wait_until(&mut h, 3.0, |_| opened(&log, "bundle.zip") == 1);
    assert_eq!(opened(&log, "bundle.zip"), 1);
    assert!(matches!(h.state().panes[0].tab().loc, Location::Local(_)));
    h.state_mut().cfg.default_apps.remove("zip");
    h.state_mut().run_cmd(Cmd::Open);
    assert!(matches!(h.state().panes[0].tab().loc, Location::Archive { .. }));
    assert_eq!(opened(&log, "bundle.zip"), 1);

    // "Always" in the Open with dialog stores the choice for that type, and later opens use it.
    h.state_mut().shot_pane(0, &d.demo, Some("notes.txt"));
    h.state_mut().run_cmd(Cmd::OpenWith);
    settle(&mut h, 3);
    let app = h.state_mut();
    let Some(Dialog::OpenWith { program, always, .. }) = app.dialog.as_mut() else { panic!("no dialog") };
    assert!(program.is_empty(), "no app chosen yet");
    *program = handler.clone();
    *always = true;
    let dialog = app.dialog.take().unwrap();
    app.confirm_dialog(dialog);
    assert_eq!(app.cfg.default_apps.get("txt"), Some(&handler));
    app.run_cmd(Cmd::OpenDefault);
    wait_until(&mut h, 3.0, |_| opened(&log, "notes.txt") == 2);
    assert_eq!(opened(&log, "notes.txt"), 2);
    assert_eq!(Settings::load().default_apps.get("txt"), Some(&handler));

    // The dialog starts from the chosen app.
    h.state_mut().run_cmd(Cmd::OpenWith);
    let Some(Dialog::OpenWith { program, .. }) = h.state().dialog.as_ref() else { panic!("no dialog") };
    assert_eq!(program, &handler);
    h.state_mut().dialog = None;

    // Settings: the reset button on a row, then "reset all".
    h.state_mut().open_settings(SettingsPage::DefaultApps);
    settle(&mut h, 4);
    assert_eq!(h.state().cfg.default_apps.len(), 2);
    h.get_all_by_label("↺").next().unwrap().click();
    settle(&mut h, 3);
    assert_eq!(h.state().cfg.default_apps.len(), 1);
    scroll_settings(&mut h, 500.0);
    h.get_by_label(&t("Reset all to system defaults")).click();
    settle(&mut h, 3);
    assert_eq!(h.state().cfg.default_apps.len(), 1, "asks first");
    h.get_by_label(&t("Reset")).click();
    settle(&mut h, 3);
    assert!(h.state().cfg.default_apps.is_empty());
    assert!(Settings::load().default_apps.is_empty());
}

/// Colour themes and fonts through the Settings tab, as a user would pick them.
#[test]
#[ignore]
fn terminal_close_asks_while_a_program_runs() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let term = h.state_mut().shot_terminal(&d.home, None, None);
    let tabs = h.state().strip.tabs.len();
    let running = |app: &App| app.panes[0].tab().term.as_ref().and_then(|t| t.running_program());
    term_type(&mut h, 0, term, "sleep 30\n");
    wait_until(&mut h, 3.0, |app| running(app).is_some());
    assert_eq!(running(h.state()).as_deref(), Some("sleep"));

    // Closing the tab asks; Cancel keeps it.
    let live = h.state().live;
    h.state_mut().request_close_tabs(&[live]);
    assert!(matches!(&h.state().dialog, Some(Dialog::CloseTerminals { programs, .. }) if programs == &["sleep"]));
    settle(&mut h, 2);
    h.get_by_label(&t("Cancel")).click();
    settle(&mut h, 2);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().strip.tabs.len(), tabs);

    // Quitting asks too, naming the program.
    assert!(matches!(h.state().quit, Quit::No));
    h.state_mut().quit = Quit::Asking;
    settle(&mut h, 2);
    h.get_by_label("sleep");
    h.get_by_label(&t("Cancel")).click();
    settle(&mut h, 2);
    assert!(matches!(h.state().quit, Quit::No));

    // Back at the prompt: closing goes ahead without asking.
    term_type(&mut h, 0, term, "\x03");
    wait_until(&mut h, 3.0, |app| running(app).is_none());
    assert_eq!(running(h.state()), None);
    let live = h.state().live;
    h.state_mut().request_close_tabs(&[live]);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().strip.tabs.len(), tabs - 1);

    // With the setting off, a running program does not hold closing back.
    let term = h.state_mut().shot_terminal(&d.home, None, None);
    term_type(&mut h, 0, term, "sleep 30\n");
    wait_until(&mut h, 3.0, |app| running(app).is_some());
    h.state_mut().cfg.term_confirm_close = false;
    let live = h.state().live;
    h.state_mut().request_close_tabs(&[live]);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().strip.tabs.len(), tabs - 1);
}

#[test]
#[ignore]
fn settings_fonts_and_terminal_themes() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let app = h.state_mut();
    let term = app.shot_terminal(&d.home, None, None);
    let term_theme = |app: &App| {
        let all = app.panes.iter().chain(app.strip.tabs.iter().flat_map(|w| w.panes.iter()));
        all.into_iter().find(|p| p.tab().uid == term).and_then(|p| p.tab().term.as_ref()).and_then(|t| t.theme).map(|t| t.name)
    };
    assert_eq!(term_theme(h.state()), Some("Breeze Light"));

    // A theme clicked in the gallery reaches the profile and the open terminal.
    h.state_mut().open_settings(SettingsPage::Terminal);
    settle(&mut h, 4);
    scroll_settings(&mut h, 300.0);
    h.get_by_label("One Dark").click();
    settle(&mut h, 3);
    assert_eq!(h.state().cfg.terminal_profiles[0].theme, "One Dark");
    assert_eq!(term_theme(h.state()), Some("One Dark"));

    // A real shell draws the scheme's own ANSI colours: green and blue backgrounds here.
    let settings_tab = h.state().strip.active;
    h.state_mut().show_tab(settings_tab - 1);
    term_type(&mut h, 0, term, "printf '\\e[42m      \\e[44m      \\e[0m\\n'\n");
    let img = h.render().unwrap();
    for (name, rgb) in [("green", [0x98, 0xc3, 0x79]), ("blue", [0x61, 0xaf, 0xef])] {
        let n = img.pixels().filter(|p| p.0[..3] == rgb).count();
        assert!(n > 100, "One Dark {name} in the terminal: {n} pixels");
    }
    h.state_mut().show_tab(settings_tab);

    // A font from the dropdown is applied and saved.
    h.state_mut().settings_page = SettingsPage::Fonts;
    settle(&mut h, 3);
    h.get_by_label("JetBrains Mono").click();
    settle(&mut h, 3);
    h.get_by_label("DejaVu Sans Mono").click();
    settle(&mut h, 3);
    assert_eq!(h.state().cfg.term_font, "builtin:dejavu");
    assert_eq!(h.state().fonts_applied.term_font, "builtin:dejavu");
    assert_eq!(Settings::load().term_font, "builtin:dejavu");

    // The interface size scales interface text, and only that.
    let width = |h: &Harness<'static, App>, family: egui::FontFamily| h.ctx.fonts(|f| f.layout_no_wrap("MMMMMMMMMM".into(), egui::FontId::new(14.0, family), egui::Color32::WHITE).size().x);
    let term_family = || egui::FontFamily::Name("term".into());
    let (ui_before, term_before) = (width(&h, egui::FontFamily::Proportional), width(&h, term_family()));
    let app = h.state_mut();
    app.cfg.ui_font_size = 20.0;
    let ctx = h.ctx.clone();
    h.state_mut().apply_fonts(&ctx);
    settle(&mut h, 3);
    let ratio = width(&h, egui::FontFamily::Proportional) / ui_before;
    assert!((1.3..1.6).contains(&ratio), "interface text at 20 of 14 px: {ratio}");
    assert_eq!(width(&h, term_family()), term_before);

    // A font that cannot draw text goes back to the default, with a message.
    let app = h.state_mut();
    app.cfg.ui_font = "No Such Font 123".into();
    let ctx = h.ctx.clone();
    h.state_mut().apply_fonts(&ctx);
    assert_eq!(h.state().cfg.ui_font, "");
    h.state_mut().cfg.ui_font_size = crate::fonts::DEFAULT_UI_SIZE;
    i18n::set_language("en");
}

// ------------------------------------------------------------------ scenes

#[test]
#[ignore]
fn export_screenshots() {
    let only: Vec<String> = std::env::var("MATRIX_FINDER_SHOTS").unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(String::from).collect();
    let want = |n: &str| only.is_empty() || only.iter().any(|o| o == n);
    for mode in MODES {
        MODE_DIR.set(mode.dir);
        // A fresh demo tree, so both modes show exactly the same files.
        render_scenes(&demo_env(), mode, &want);
    }
    if want("showcase") {
        compose_showcase();
    }
}

fn render_scenes(d: &Demo, mode: Mode, want: &dyn Fn(&str) -> bool) {
    let gnome = "gnome";

    if want("overview") {
        let mut cfg = settings(mode.theme, d);
        cfg.bookmarks = crate::config::system_bookmarks();
        let mut h = app_harness(&cfg, gnome);
        h.state_mut().shot_pane(0, &d.demo.join("home"), Some("Documents"));
        shot(&mut h, "overview");
    }

    if want("panels") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some("bundle.zip"));
        app.shot_pane(1, &d.demo.join("home"), Some(".."));
        app.shot_focus(0);
        shot(&mut h, "panels");
    }

    if want("native-themes") {
        let mut tiles = vec![];
        for (theme, chrome) in mode.native {
            let mut h = app_harness(&settings(theme, d), chrome);
            let app = h.state_mut();
            app.shot_pane(0, &d.demo.join("home"), Some("setup.sh"));
            app.shot_pane(1, &d.demo, Some(".."));
            app.shot_focus(0);
            settle(&mut h, 3);
            let mut img = h.render().unwrap();
            if chrome == "mac" {
                paint_traffic_lights(&mut img);
            }
            tiles.push(img);
        }
        let mut out = RgbaImage::new(W as u32, H as u32);
        for (i, t) in tiles.iter().enumerate() {
            let small = image::imageops::resize(t, W as u32 / 2, H as u32 / 2, image::imageops::FilterType::Lanczos3);
            image::imageops::overlay(&mut out, &small, (i as i64 % 2) * W as i64 / 2, (i as i64 / 2) * H as i64 / 2);
        }
        save(&out, "native-themes");
    }

    if want("palette") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some("bundle.zip"));
        app.shot_pane(1, &d.demo.join("src"), Some(".."));
        app.shot_focus(0);
        app.palette.open_with("cop");
        shot(&mut h, "palette");
    }

    if want("about") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some("bundle.zip"));
        app.shot_pane(1, &d.demo.join("src"), Some(".."));
        app.shot_focus(0);
        app.show_about = true;
        settle(&mut h, 3);
        center_dialog(&mut h, &t("About"));
        shot(&mut h, "about");
    }

    // Quick View scenes: one pane plus the preview panel.
    for (name, dir, file) in [
        ("markdown", d.demo.join("docs"), "README.md"),
        ("table-parquet", d.demo.join("data"), "orders.parquet"),
        ("table-excel", d.demo.join("data"), "orders.xlsx"),
        ("quickview-pdf", d.demo.join("papers"), "report.pdf"),
        ("quickview-notebook", d.demo.join("papers"), "analysis.ipynb"),
        ("quickview-font", d.demo.join("formats"), "Source Sans 3.otf"),
        ("quickview-model", d.demo.join("formats"), "torus.stl"),
        ("quickview-sqlite", d.demo.join("formats"), "shop.sqlite"),
        ("quickview-certificate", d.demo.join("formats"), "server.pem"),
        ("quickview-epub", d.demo.join("formats"), "A Short Story.epub"),
    ] {
        if !want(name) {
            continue;
        }
        let mut cfg = settings(mode.theme, d);
        cfg.quick_view = true;
        if name.starts_with("table") || name.starts_with("quickview-") && name != "quickview-pdf" && name != "quickview-notebook" {
            cfg.quick_view_frac = 0.6;
        }
        let mut h = app_harness(&cfg, gnome);
        h.state_mut().shot_pane(0, &dir, Some(file));
        settle(&mut h, 10);
        std::thread::sleep(Duration::from_millis(800));
        settle(&mut h, 10);
        if name == "table-parquet" {
            type_into(&mut h, egui::Rect::from_min_max(pos2(W * 0.6, 60.0), pos2(W, 200.0)), "Huế");
            settle(&mut h, 6);
        }
        shot(&mut h, name);
    }

    if want("disk-usage") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some(".."));
        app.shot_pane(1, &d.demo.join("docs"), Some(".."));
        app.shot_focus(0);
        app.open_disk_usage(d.demo.clone());
        wait_until(&mut h, 5.0, |a| !a.panes[0].tab().du.as_ref().is_some_and(|d| d.is_scanning()));
        shot(&mut h, "disk-usage");
    }

    if want("disk-usage-table") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some(".."));
        app.open_disk_usage(d.demo.clone());
        if let Some(du) = app.panes[0].tab_mut().du.as_mut() {
            du.view = crate::diskusage::DuView::Table;
            du.set_depth(2);
        }
        wait_until(&mut h, 5.0, |a| !a.panes[0].tab().du.as_ref().is_some_and(|d| d.is_scanning()));
        shot(&mut h, "disk-usage-table");
    }

    if want("transfers") {
        let mut cfg = settings(mode.theme, d);
        cfg.max_parallel = 1;
        let mut h = app_harness(&cfg, gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo.join("big"), Some("video_1.mkv"));
        app.shot_pane(1, &d.demo.join("out"), Some(".."));
        app.shot_focus(0);
        for i in 1..=3 {
            let src = d.demo.join(format!("big/video_{i}.mkv")).to_string_lossy().to_string();
            let title = format!("{} 1 {}", t("Copying"), t("item(s)"));
            app.spawn_job(title, After::Refresh, Some(Location::Local(d.demo.join("out"))), move |p| {
                let total: u64 = 1_500_000_000;
                p.set_total(total);
                p.set_current(&src);
                while !p.cancelled() && p.done() < total {
                    p.add(5_300_000);
                    std::thread::sleep(Duration::from_millis(40));
                }
                Ok(String::new())
            });
        }
        app.open_transfers();
        settle(&mut h, 3);
        // ~61 % at ~133 MB/s.
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(7) {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
        shot(&mut h, "transfers");
        for j in &h.state().jobs {
            j.job.progress.cancel();
        }
    }

    if want("batch-rename") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo.join("rename2"), Some("Báo cáo THÁNG 1.docx"));
        app.shot_pane(1, &d.demo, Some(".."));
        app.shot_focus(0);
        app.open_batch_rename();
        settle(&mut h, 4);
        h.get_by_label("Web-safe (slug)").click();
        settle(&mut h, 4);
        center_dialog(&mut h, &t("Batch rename"));
        move_to(&mut h, pos2(W / 2.0, H - 20.0));
        shot(&mut h, "batch-rename");
    }

    if want("drag-drop") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo.join("data"), Some("orders.parquet"));
        app.shot_pane(1, &d.demo, Some(".."));
        app.shot_mark(0, &["orders.avro", "orders.parquet"]);
        app.shot_focus(0);
        settle(&mut h, 3);
        // Rows are ROW_H apart below the column header: orders.parquet is row 3 on the
        // left, "images" row 5 on the right (row 0 is "..", 80 px below the window's top).
        let top = window_rect(&h, 0).top();
        let row_y = |k: f32| top + 80.0 + crate::pane::ROW_H * k;
        let from = pos2(330.0, row_y(3.0));
        let to = pos2(880.0, row_y(5.0));
        move_to(&mut h, from);
        button(&mut h, from, PointerButton::Primary, true, Modifiers::NONE);
        for i in 1..=8 {
            let p = from + (to - from) * (i as f32 / 8.0);
            push(&mut h, Event::PointerMoved(p));
            h.input_mut().modifiers = Modifiers::SHIFT;
            h.step();
        }
        // Capture before the spring-loaded folder (≈ 1 s of hovering) opens it.
        h.step();
        let img = h.render().unwrap();
        save(&img, "drag-drop");
        button(&mut h, to, PointerButton::Primary, false, Modifiers::NONE);
        h.state_mut().dialog = None;
    }

    if want("terminal-tabs") {
        // Light files beside a dark terminal in both modes, so the two windows are easy to tell apart.
        let mut cfg = settings("Breeze Light", d);
        set_terminal_theme(&mut cfg, "Breeze Dark");
        cfg.bookmarks = vec![Bookmark::new("Home", d.home.to_string_lossy().to_string())];
        let mut h = app_harness(&cfg, gnome);
        let proj = d.home.join("Projects/matrix_finder");
        let app = h.state_mut();
        app.shot_pane(0, &d.home, None);
        app.strip.toggle_pin(0);
        // "Rust": a split tab with the project's files beside a terminal, and a "cargo" terminal tab.
        app.shot_files_tab(&proj, None);
        let rust = app.strip.new_group(1);
        app.split_pane(0, SplitDir::Right, true);
        let term = app.panes[1].tab().uid;
        app.layout.set_ratio(&[], 0.45);
        let split_tab = app.strip.active;
        app.shot_terminal(&proj, Some("cargo"), Some(rust));
        if let Some(g) = app.strip.group_mut(rust) {
            g.name = "Rust".into();
        }
        app.shot_files_tab(&d.home.join("Documents"), None);
        let last = app.strip.tabs.len() - 1;
        let web = app.strip.new_group(last);
        let at = app.strip.tabs.len();
        app.new_terminal_tab(at, Some(web), None, Some(d.home.clone()));
        if let Some(g) = app.strip.group_mut(web) {
            g.name = "Web".into();
        }
        app.shot_files_tab(&d.home.join("Documents"), None);
        app.strip.toggle_collapsed(web);
        app.show_tab(split_tab);
        app.shot_focus(1);
        term_type(&mut h, 0, term, "ls -lgo && cat build.log\n");
        shot(&mut h, "terminal-tabs");
    }

    if want("terminal-profiles") || want("terminal-themes") || want("settings") {
        use crate::terminal::TerminalProfile;
        let mut cfg = settings(mode.theme, d);
        cfg.terminal_profiles = vec![
            TerminalProfile { name: "Default".into(), ..TerminalProfile::default() },
            TerminalProfile { name: "Python venv".into(), shell: "/bin/zsh".into(), startup: "source .venv/bin/activate\nclear".into(), ..TerminalProfile::default() },
            TerminalProfile { name: "Night shift".into(), theme: "Tokyo Night".into(), ..TerminalProfile::default() },
            TerminalProfile { name: "Ghostty".into(), terminal: TerminalKind::Ghostty, ..TerminalProfile::default() },
        ];
        set_terminal_theme(&mut cfg, mode.theme);
        cfg.terminal_profiles[2].theme = "Tokyo Night".into();
        if want("terminal-profiles") || want("terminal-themes") {
            let mut h = app_harness(&cfg, gnome);
            let app = h.state_mut();
            app.shot_pane(0, &d.demo.join("papers"), Some(".."));
            app.profile_sel = 2;
            app.open_settings(SettingsPage::Terminal);
            settle(&mut h, 6);
            shot(&mut h, "terminal-profiles");
            // Down to the sample and the colour themes.
            scroll_settings(&mut h, 300.0);
            shot(&mut h, "terminal-themes");
        }
        if want("settings") {
            let mut h = app_harness(&cfg, gnome);
            let app = h.state_mut();
            app.shot_pane(0, &d.demo.join("home"), Some("setup.sh"));
            app.shot_pane(1, &d.demo, Some(".."));
            app.shot_focus(0);
            app.open_settings(SettingsPage::Appearance);
            shot(&mut h, "settings");
        }
    }

    if want("bookmarks-sidebar") || want("bookmarks-bar") || want("settings-bookmarks") {
        // The standard folders, plus a folder of project bookmarks.
        let mut cfg = settings(mode.theme, d);
        cfg.bookmarks = crate::config::system_bookmarks();
        let mut work = Bookmark::new_folder("Work");
        work.children = vec![
            Bookmark { icon: crate::config::BookmarkIcon::Code, ..Bookmark::new("matrix_finder", d.home.join("Projects/matrix_finder").to_string_lossy()) },
            Bookmark::new("Demo data", d.demo.join("data").to_string_lossy()),
            Bookmark { icon: crate::config::BookmarkIcon::Star, ..Bookmark::new("Papers", d.demo.join("papers").to_string_lossy()) },
        ];
        cfg.bookmarks.push(work);
        let folder = cfg.bookmarks.len() - 1;
        if want("bookmarks-sidebar") {
            let mut h = app_harness(&cfg, gnome);
            let app = h.state_mut();
            app.shot_pane(0, &d.home, Some("Documents"));
            shot(&mut h, "bookmarks-sidebar");
        }
        if want("bookmarks-bar") {
            let mut cfg = cfg.clone();
            cfg.bookmark_bar = crate::config::BookmarkBar::Horizontal;
            let mut h = app_harness(&cfg, gnome);
            let app = h.state_mut();
            app.shot_pane(0, &d.home.join("Downloads"), Some("photo.jpg"));
            app.shot_pane(1, &d.home.join("Projects/matrix_finder"), Some(".."));
            settle(&mut h, 2);
            h.ctx.memory_mut(|m| m.open_popup(egui::Id::new(("bookmark-folder", folder))));
            shot(&mut h, "bookmarks-bar");
        }
        if want("settings-bookmarks") {
            let mut h = app_harness(&cfg, gnome);
            let app = h.state_mut();
            app.shot_pane(0, &d.home, None);
            app.open_settings(SettingsPage::Bookmarks);
            shot(&mut h, "settings-bookmarks");
        }
    }

    if want("quick-filter") || want("settings-quick-filters") {
        let cfg = settings(mode.theme, d);
        if want("quick-filter") {
            let mut h = app_harness(&cfg, gnome);
            let app = h.state_mut();
            app.shot_pane(0, &d.demo, Some(".."));
            app.shot_pane(1, &d.demo.join("data"), Some(".."));
            app.shot_focus(0);
            let i = app.cfg.quick_filters.iter().position(|q| q.name == "Spreadsheets").unwrap();
            app.set_quick_filter(Some(i));
            settle(&mut h, 3);
            let uid = h.state().panes[0].tab().uid;
            h.ctx.memory_mut(|m| m.open_popup(egui::Id::new(("quick-filter", uid))));
            shot(&mut h, "quick-filter");
        }
        if want("settings-quick-filters") {
            let mut h = app_harness(&cfg, gnome);
            let app = h.state_mut();
            app.shot_pane(0, &d.demo, None);
            app.open_settings(SettingsPage::QuickFilters);
            shot(&mut h, "settings-quick-filters");
        }
    }

    if want("settings-default-apps") {
        let mut cfg = settings(mode.theme, d);
        cfg.default_apps.insert("md".into(), "TextEdit.app".into());
        cfg.default_apps.insert("mp4".into(), "\"QuickTime Player.app\"".into());
        let mut h = app_harness(&cfg, gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, None);
        app.open_settings(SettingsPage::DefaultApps);
        // The system's answers arrive in the background.
        wait_until(&mut h, 2.0, |_| false);
        shot(&mut h, "settings-default-apps");
    }

    if want("open-with") {
        let mut cfg = settings(mode.theme, d);
        cfg.default_apps.insert("pdf".into(), "Preview.app".into());
        let mut h = app_harness(&cfg, gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo.join("papers"), Some("report.pdf"));
        app.run_cmd(Cmd::OpenWith);
        wait_until(&mut h, 1.0, |_| false);
        shot(&mut h, "open-with");
    }

    if want("settings-shortcuts") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, None);
        app.open_settings(SettingsPage::Shortcuts);
        shot(&mut h, "settings-shortcuts");
    }

    if want("split-numbered") {
        let mut cfg = settings(mode.theme, d);
        cfg.quick_view = false;
        let mut h = app_harness(&cfg, gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some("bundle.zip"));
        app.shot_pane(1, &d.demo.join("home"), Some(".."));
        app.split_pane(1, SplitDir::Down, true);
        let term = app.panes[2].tab().uid;
        app.shot_focus(0);
        term_type(&mut h, 2, term, "ls\n");
        shot(&mut h, "split-numbered");
    }

    if want("settings-fonts") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, None);
        app.open_settings(SettingsPage::Fonts);
        shot(&mut h, "settings-fonts");
    }

    if want("split") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo.join("papers"), Some("report.pdf"));
        settle(&mut h, 2);
        right_click(&mut h, pos2(W * 0.7, H * 0.4));
        if let Some(p) = node_center(&h, &t("Layout"), W / 2.0) {
            move_to(&mut h, p);
            settle(&mut h, 4);
        }
        shot(&mut h, "split");
    }

    if want("split-narrow") {
        let mut cfg = settings(mode.theme, d);
        cfg.quick_view = true;
        let mut h = app_harness(&cfg, gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo.join("papers"), Some(".."));
        app.shot_pane(1, &d.demo.join("papers"), Some(".."));
        app.split_pane(1, SplitDir::Down, false);
        app.shot_pane(2, &d.demo, Some("papers"));
        app.shot_focus(2);
        settle(&mut h, 10);
        shot(&mut h, "split-narrow");
    }

    if want("vietnamese") {
        let mut cfg = settings(mode.theme, d);
        cfg.language = "vi".into();
        let mut h = app_harness(&cfg, gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some("Thư mục tiếng Việt"));
        app.shot_pane(1, &d.demo.join("docs"), Some(".."));
        app.shot_mark(0, &["images", "src"]);
        app.shot_focus(0);
        app.run_cmd(Cmd::Delete);
        shot(&mut h, "vietnamese");
        h.state_mut().dialog = None;
        i18n::set_language("en");
    }

    // Scenes in ~/Downloads.
    let downloads_cfg = || {
        let mut cfg = settings(mode.theme, d);
        cfg.bookmarks = vec![Bookmark::new("Home", d.home.to_string_lossy().to_string())];
        cfg
    };

    if want("checksum") {
        let mut h = app_harness(&downloads_cfg(), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.downloads, Some("ubuntu-26.04-desktop.iso"));
        app.shot_mark(0, &["release-notes.pdf", "ubuntu-26.04-desktop.iso"]);
        app.open_checksum();
        settle(&mut h, 4);
        if h.query_by_label("SHA-256").is_some() {
            h.get_by_label("SHA-256").click();
            settle(&mut h, 4);
        }
        let iso = std::fs::File::open(d.downloads.join("ubuntu-26.04-desktop.iso")).unwrap();
        let hash = crate::checksum::hash_reader(iso, crate::checksum::Algo::Sha256, |_| true).unwrap();
        type_into(&mut h, egui::Rect::from_min_max(pos2(W * 0.2, 120.0), pos2(W * 0.85, H * 0.35)), &hash);
        settle(&mut h, 8);
        center_dialog(&mut h, "checksum-window");
        shot(&mut h, "checksum");
    }

    if want("checksum-verify") {
        let mut h = app_harness(&downloads_cfg(), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.downloads, Some("SHA256SUMS"));
        app.open_checksum();
        settle(&mut h, 10);
        center_dialog(&mut h, "checksum-window");
        shot(&mut h, "checksum-verify");
    }

    if want("new-menu") {
        let mut h = app_harness(&downloads_cfg(), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.downloads, Some("ubuntu-26.04-desktop.iso"));
        app.shot_mark(0, &["release-notes.pdf", "ubuntu-26.04-desktop.iso"]);
        settle(&mut h, 2);
        right_click(&mut h, pos2(W * 0.55, H * 0.6));
        if let Some(p) = node_center(&h, &t("New"), 0.0) {
            move_to(&mut h, p);
            settle(&mut h, 4);
        }
        shot(&mut h, "new-menu");
    }

    if want("terminal") {
        let mut h = app_harness(&downloads_cfg(), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.downloads, Some(".."));
        let term = app.shot_terminal(&d.downloads, None, None);
        let input = "ls -lgo; shasum -a 256 -c SHA256SUMS; for c in {31..36}; do printf \"\\e[${c}m## \\e[0m\"; done; echo\n";
        term_type(&mut h, 0, term, input);
        shot(&mut h, "terminal");
    }
}

// ------------------------------------------------------------------ showcase

/// The four scenes of `docs/showcase.png`, left to right and top to bottom:
/// (mode, scene). Light and dark both appear; `terminal-tabs` is light files beside a dark terminal in either mode.
const SHOWCASE: [(&str, &str); 4] = [("light", "panels"), ("light", "terminal-tabs"), ("dark", "table-parquet"), ("light", "disk-usage")];

/// Coverage (0..=1) of a rectangle with rounded corners at pixel (`x`, `y`).
fn rounded_coverage(x: u32, y: u32, w: u32, h: u32, radius: f32) -> f32 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let dx = (radius - px).max(px - (w as f32 - radius)).max(0.0);
    let dy = (radius - py).max(py - (h as f32 - radius)).max(0.0);
    (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
}

/// Blend `top` over `bottom` with opacity `a`.
fn over(bottom: image::Rgba<u8>, top: [u8; 3], a: f32) -> image::Rgba<u8> {
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32 * a).round() as u8;
    image::Rgba([mix(bottom.0[0], top[0]), mix(bottom.0[1], top[1]), mix(bottom.0[2], top[2]), 255])
}

/// Combine four scenes into one image: windows with rounded corners and a soft
/// shadow on a calm gradient that suits both light and dark pages.
fn compose_showcase() {
    const PAD: u32 = 56;
    const GAP: u32 = 36;
    const RADIUS: f32 = 12.0;
    let tiles: Vec<RgbaImage> = SHOWCASE
        .iter()
        .map(|(mode, name)| image::open(out_dir().join(mode).join(format!("{name}.png"))).unwrap().to_rgba8())
        .collect();
    let (tw, th) = tiles[0].dimensions();
    let (cw, ch) = (PAD * 2 + GAP + tw * 2, PAD * 2 + GAP + th * 2);
    let top_left = |i: usize| (PAD + (i as u32 % 2) * (tw + GAP), PAD + (i as u32 / 2) * (th + GAP));

    let mut out = RgbaImage::from_fn(cw, ch, |x, y| {
        let k = (x as f32 / cw as f32 + y as f32 / ch as f32) / 2.0;
        let mix = |a: f32, b: f32| (a + (b - a) * k).round() as u8;
        image::Rgba([mix(58.0, 92.0), mix(86.0, 74.0), mix(122.0, 128.0), 255])
    });

    // The shadow of all four windows: their shapes, blurred, a little lower down.
    let mut mask = image::GrayImage::new(cw, ch);
    for i in 0..tiles.len() {
        let (ox, oy) = top_left(i);
        for y in 0..th {
            for x in 0..tw {
                mask.put_pixel(ox + x, oy + y + 10, image::Luma([(rounded_coverage(x, y, tw, th, RADIUS) * 255.0) as u8]));
            }
        }
    }
    let mask = image::imageops::blur(&mask, 16.0);
    for (x, y, p) in out.enumerate_pixels_mut() {
        *p = over(*p, [8, 12, 24], mask.get_pixel(x, y).0[0] as f32 / 255.0 * 0.55);
    }

    for (i, tile) in tiles.iter().enumerate() {
        let (ox, oy) = top_left(i);
        for (x, y, p) in tile.enumerate_pixels() {
            let a = rounded_coverage(x, y, tw, th, RADIUS);
            let below = *out.get_pixel(ox + x, oy + y);
            out.put_pixel(ox + x, oy + y, over(below, [p.0[0], p.0[1], p.0[2]], a));
        }
    }
    let p = out_dir().join("showcase.png");
    out.save(&p).unwrap();
    eprintln!("wrote {}", p.display());
}

/// An animation really changes on screen, and stops when paused.
#[test]
#[ignore]
fn animated_images_play_and_pause() {
    let d = demo_env();
    let dir = d.demo.join("anim");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let mut bytes = vec![];
        let mut enc = image::codecs::gif::GifEncoder::new(&mut bytes);
        for shade in [40u8, 200] {
            let img = RgbaImage::from_pixel(200, 200, image::Rgba([shade, shade, shade, 255]));
            enc.encode_frame(image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(150, 1))).unwrap();
        }
        drop(enc);
        std::fs::write(dir.join("a.gif"), bytes).unwrap();
    }
    let mut cfg = settings("Breeze Light", &d);
    cfg.quick_view = true;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &dir, Some("a.gif"));
    wait_until(&mut h, 3.0, |a| a.quick.as_ref().is_some_and(|q| q.is_ready()));
    assert!(h.state().quick.as_ref().unwrap().describe().contains("frames=2"));
    // Quick View is on the right; its picture is in the middle of the window's right part.
    let sample = |h: &mut Harness<'static, App>| {
        let img = h.render().unwrap();
        img.get_pixel((W * 0.82) as u32, (H * 0.5) as u32).0[0]
    };
    let mut seen = std::collections::HashSet::new();
    for _ in 0..16 {
        h.step();
        seen.insert(sample(&mut h));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(seen.contains(&40) && seen.contains(&200), "both frames came up: {seen:?}");
    h.get_by_label(&t("Pause")).click();
    settle(&mut h, 3);
    let held = sample(&mut h);
    for _ in 0..8 {
        h.step();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(sample(&mut h), held, "paused");
    }
    assert!(h.query_by_label(&t("Play")).is_some());
}

fn key_press(h: &mut Harness<'static, App>, key: Key, physical: Option<Key>, m: Modifiers) {
    h.input_mut().modifiers = m;
    push(h, Event::Key { key, physical_key: physical, pressed: true, repeat: false, modifiers: m });
    h.step();
    push(h, Event::Key { key, physical_key: physical, pressed: false, repeat: false, modifiers: m });
    h.input_mut().modifiers = Modifiers::NONE;
    settle(h, 2);
}

fn press_chord(h: &mut Harness<'static, App>, text: &str) {
    let c = crate::shortcuts::Chord::parse(text).unwrap();
    let m = Modifiers { ctrl: c.ctrl, alt: c.alt, shift: c.shift, mac_cmd: c.cmd, command: c.cmd || (c.ctrl && !cfg!(target_os = "macos")) };
    key_press(h, c.key, None, m);
}

/// Windows and tabs by keyboard, numbered windows, and keys a terminal keeps. The keys
/// here are bound in the settings, as the defaults differ by system.
#[test]
#[ignore]
fn keyboard_tabs_and_windows() {
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    for (id, key) in [("tab_1", "Ctrl+Shift+1"), ("tab_2", "Ctrl+Shift+2"), ("last_used_tab", "Ctrl+Shift+Backslash"), ("window_1", "Ctrl+Shift+F1"), ("window_2", "Ctrl+Shift+F2"), ("window_3", "Ctrl+Shift+F3"), ("next_window", "Ctrl+Shift+CloseBracket"), ("previous_window", "Ctrl+Shift+OpenBracket"), ("next_tab", "Ctrl+Tab"), ("close_window", "Ctrl+W")] {
        cfg.shortcuts.insert(id.to_string(), vec![key.to_string()]);
    }
    let mut h = app_harness(&cfg, "gnome");
    let app = h.state_mut();
    app.shot_pane(0, &d.demo, None);
    app.shot_pane(1, &d.demo.join("docs"), None);
    app.split_pane(1, SplitDir::Down, true); // [files | files / terminal]
    let leaves = app.layout.leaves();
    assert_eq!(leaves.len(), 3);
    wait_until(&mut h, 1.0, |_| false);

    // By number, in the order they are on screen; and around.
    press_chord(&mut h, "Ctrl+Shift+F2");
    assert_eq!(h.state().active, leaves[1]);
    press_chord(&mut h, "Ctrl+Shift+F3");
    assert_eq!(h.state().active, leaves[2]);
    assert!(h.state().panes[leaves[2]].tab().is_terminal());
    wait_until(&mut h, 1.0, |_| false);
    // From a terminal too, which does not get the key.
    press_chord(&mut h, "Ctrl+Shift+CloseBracket");
    assert_eq!(h.state().active, leaves[0], "next, around the end");
    press_chord(&mut h, "Ctrl+Shift+OpenBracket");
    assert_eq!(h.state().active, leaves[2], "previous, around the start");

    // A terminal keeps what terminal programs use, even when it is bound.
    wait_until(&mut h, 1.0, |_| false);
    press_chord(&mut h, "Ctrl+W");
    assert_eq!(h.state().panes.len(), 3, "Ctrl+W went to the terminal");
    assert_eq!(h.state().strip.tabs.len(), 1);

    // A shifted digit, which arrives as the sign it types, on the digit's key.
    let (at, group) = h.state().next_slot();
    h.state_mut().new_files_tab(at, group, Some(Location::Local(d.demo.join("src"))));
    settle(&mut h, 3);
    let ctrl_shift = Modifiers { ctrl: true, shift: true, command: !cfg!(target_os = "macos"), ..Modifiers::NONE };
    key_press(&mut h, Key::Exclamationmark, Some(Key::Num1), ctrl_shift);
    assert_eq!(h.state().strip.active, 0, "Ctrl+Shift+1");
    key_press(&mut h, Key::Num2, Some(Key::Num2), ctrl_shift);
    assert_eq!(h.state().strip.active, 1, "Ctrl+Shift+2");
    press_chord(&mut h, "Ctrl+Shift+Backslash");
    assert_eq!(h.state().strip.active, 0, "back to the tab before");
    press_chord(&mut h, "Ctrl+Shift+Backslash");
    assert_eq!(h.state().strip.active, 1, "and to the one before that");
    press_chord(&mut h, "Ctrl+Tab");
    assert_eq!(h.state().strip.active, 0, "next, around the end");
}

/// Splitting by keyboard: the same kind of window, in the right place, up to the limit.
#[test]
#[ignore]
fn transfers_open_in_a_tab() {
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    cfg.shortcuts.insert("transfers".to_string(), vec!["Ctrl+Shift+F9".to_string()]);
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &d.demo, None);
    settle(&mut h, 3);
    let before = h.state().strip.tabs.len();
    let docs = Location::Local(d.demo.join("docs"));

    // The shortcut opens a tab, and no floating window.
    press_chord(&mut h, "Ctrl+Shift+F9");
    settle(&mut h, 3);
    let app = h.state();
    assert_eq!(app.strip.tabs.len(), before + 1);
    assert!(app.panes[app.active].tab().transfers);
    assert_eq!(app.panes[app.active].tab().title(), t("Transfers"));
    assert!(h.ctx.memory(|m| m.area_rect(egui::Id::new(t("Transfers")))).is_none());

    // It has no file list for file commands to act on.
    press_chord(&mut h, "F5");
    settle(&mut h, 2);
    assert!(h.state().dialog.is_none());

    // From another tab, the one that is open comes back.
    h.state_mut().show_tab(0);
    settle(&mut h, 2);
    assert!(!h.state().panes[h.state().active].tab().transfers);
    press_chord(&mut h, "Ctrl+Shift+F9");
    settle(&mut h, 3);
    let app = h.state();
    assert_eq!(app.strip.tabs.len(), before + 1, "only one Transfers tab");
    assert!(app.panes[app.active].tab().transfers);

    // Show: the destination opens in a new files tab, and Transfers stays as it was.
    let home = h.state().panes[h.state().active].tab().loc.clone();
    h.state_mut().history.push(FinishedJob { title: "Copied".into(), ok: true, msg: String::new(), bytes: 1, secs: 1.0, dest: Some(docs.clone()), at: "12:00".into() });
    settle(&mut h, 2);
    h.get_by_label(&t("Show")).click();
    settle(&mut h, 3);
    let app = h.state();
    assert_eq!(app.strip.tabs.len(), before + 2);
    assert!(app.panes[app.active].tab().is_files());
    assert_eq!(app.panes[app.active].tab().loc, docs);

    // Back in it: with a file window beside it, that window goes there and no tab opens.
    let app = h.state_mut();
    app.open_transfers();
    assert_eq!(app.panes[app.active].tab().loc, home, "its own location did not change");
    let side = app.active;
    app.split_pane(side, SplitDir::Right, false);
    app.active = side;
    let tabs = app.strip.tabs.len();
    app.navigate_active(docs.clone());
    assert_eq!(app.strip.tabs.len(), tabs);
    assert!(app.panes[side].tab().transfers);
    assert!(app.panes[app.active].tab().is_files());
    assert_eq!(app.panes[app.active].tab().loc, docs);
    app.active = side;
    app.close_pane(1 - side);
    settle(&mut h, 3);

    // Clear empties the history.
    h.get_by_label(&t("Clear")).click();
    settle(&mut h, 2);
    assert!(h.state().history.is_empty());

    // It is closed like a tab, reopens as itself, and is not restored as a folder.
    let app = h.state_mut();
    assert!(app.panes[app.active].tab().transfers);
    let saved = app.save_session().tabs.len();
    assert_eq!(app.strip.tabs.len(), saved + 1, "every tab but Transfers is saved");
    app.close_current();
    assert!(!app.panes.iter().any(|p| p.tab().transfers));
    assert_eq!(app.save_session().tabs.len(), saved);
    app.reopen_closed_tab();
    assert!(app.panes[app.active].tab().transfers);
    settle(&mut h, 2);
}

/// A job that runs until it is cancelled.
fn endless_job(app: &mut App) -> std::sync::Arc<ops::Progress> {
    app.spawn_job("Copying".into(), After::Refresh, None, |p| {
        p.set_total(100);
        p.add(25);
        while !p.cancelled() {
            std::thread::sleep(Duration::from_millis(10));
        }
        Err("Cancelled".into())
    });
    app.jobs.last().unwrap().job.progress.clone()
}

fn request_close(h: &mut Harness<'static, App>) {
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    settle(h, 2);
}

#[test]
#[ignore]
fn quitting_with_transfers_asks_first() {
    let d = demo_env();
    let cfg = settings("Breeze Light", &d);
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &d.demo, None);
    settle(&mut h, 3);
    let tabs = h.state().strip.tabs.len();

    // Nothing running: no question.
    request_close(&mut h);
    assert!(h.state().quit == Quit::No);

    // A copy starting leaves the tabs alone.
    let job = endless_job(h.state_mut());
    settle(&mut h, 3);
    assert_eq!(h.state().strip.tabs.len(), tabs);
    assert!(h.state().panes.iter().all(|p| !p.tab().transfers));
    assert_eq!(h.state().transfers_gauge(), Some((Some(0.25), false)));
    job.pause();
    assert_eq!(h.state().transfers_gauge(), Some((Some(0.25), true)));
    job.resume();

    // Asked, and it can be turned down by button and by Escape.
    request_close(&mut h);
    assert!(h.state().quit == Quit::Asking);
    h.get_by_label(&t("Keep transferring")).click();
    settle(&mut h, 2);
    assert!(h.state().quit == Quit::No);
    assert!(!job.cancelled());
    request_close(&mut h);
    assert!(h.state().quit == Quit::Asking);
    press_chord(&mut h, "Escape");
    assert!(h.state().quit == Quit::No);

    // Keys do not reach what is behind the question.
    request_close(&mut h);
    press_chord(&mut h, "F7");
    settle(&mut h, 2);
    assert!(h.state().dialog.is_none(), "F7 must not open the New folder dialog");

    // Stop and quit: the jobs are cancelled and get to finish before it goes ahead.
    h.get_by_label(&t("Stop and quit")).click();
    settle(&mut h, 1);
    assert!(job.cancelled());
    wait_until(&mut h, 5.0, |a| a.quit == Quit::Go);
    assert!(h.state().jobs.is_empty());

    // A job that ends by itself while asking: the user's quit goes through.
    let mut h = app_harness(&cfg, "gnome");
    let job = endless_job(h.state_mut());
    settle(&mut h, 2);
    request_close(&mut h);
    assert!(h.state().quit == Quit::Asking);
    job.cancel();
    wait_until(&mut h, 5.0, |a| a.quit == Quit::Go);
}

#[test]
#[ignore]
fn keyboard_splits_windows() {
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    for (id, key) in [("split_right", "Ctrl+Shift+O"), ("split_down", "Ctrl+Shift+E"), ("split_left", "Ctrl+Shift+L"), ("split_up", "Ctrl+Shift+K")] {
        cfg.shortcuts.insert(id.to_string(), vec![key.to_string()]);
    }
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &d.demo, None);
    settle(&mut h, 3);

    // Files beside files, showing the same folder, and it gets the keyboard.
    press_chord(&mut h, "Ctrl+Shift+O");
    let app = h.state();
    assert_eq!(app.panes.len(), 2);
    assert_eq!(app.layout.dims(), (2, 1), "side by side");
    assert_eq!(app.layout.leaves(), vec![0, 1], "the new one on the right");
    assert_eq!(app.active, 1);
    assert!(app.panes[1].tab().is_files());
    assert_eq!(app.panes[1].tab().loc, app.panes[0].tab().loc);

    // On the left, the new window comes first.
    let mut h2 = app_harness(&cfg, "gnome");
    h2.state_mut().shot_pane(0, &d.demo, None);
    settle(&mut h2, 3);
    press_chord(&mut h2, "Ctrl+Shift+L");
    assert_eq!(h2.state().layout.leaves(), vec![1, 0]);
    press_chord(&mut h2, "Ctrl+Shift+K");
    assert_eq!(h2.state().layout.dims(), (2, 2), "a window above it, in the left column");
    assert_eq!(h2.state().layout.leaves()[0], 2, "above comes first");
    drop(h2);

    // A terminal splits into a terminal, and does not get the key itself.
    let app = h.state_mut();
    app.panes[1] = app.terminal_pane(None, &d.demo).unwrap();
    app.active = 1;
    wait_until(&mut h, 1.5, |_| false);
    press_chord(&mut h, "Ctrl+Shift+E");
    let app = h.state();
    assert_eq!(app.panes.len(), 3);
    assert_eq!(app.layout.dims(), (2, 2));
    assert_eq!(app.active, 2);
    assert!(app.panes[2].tab().is_terminal(), "a terminal under a terminal");

    // Full: nothing happens but a message.
    press_chord(&mut h, "Ctrl+Shift+O");
    assert_eq!(h.state().panes.len(), 3, "no third column");
    press_chord(&mut h, "Ctrl+Shift+E");
    assert_eq!(h.state().panes.len(), 3, "no third row");
}

/// Closing a mini window by keyboard, and a whole tab, including from a terminal.
#[test]
#[ignore]
fn keyboard_closes_windows_and_tabs() {
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    for (id, keys) in [("close_window", vec!["Ctrl+W", "Ctrl+Shift+W"]), ("close_tab", vec!["Ctrl+Shift+X"]), ("reopen_tab", vec!["Ctrl+Shift+T"])] {
        cfg.shortcuts.insert(id.to_string(), keys.into_iter().map(String::from).collect());
    }
    let mut h = app_harness(&cfg, "gnome");
    let app = h.state_mut();
    app.shot_pane(0, &d.demo, None);
    app.shot_pane(1, &d.demo.join("docs"), None);
    app.split_pane(1, SplitDir::Down, true); // [demo | docs / terminal], the terminal focused
    let first_tab = app.strip.tabs[0].uid;
    let at = app.strip.tabs.len();
    app.new_files_tab(at, None, Some(Location::Local(d.demo.join("src"))));
    let i = app.strip.index_of(first_tab).unwrap();
    app.show_tab(i);
    wait_until(&mut h, 1.5, |_| false);
    assert_eq!((h.state().panes.len(), h.state().strip.tabs.len()), (3, 2));
    assert!(h.state().panes[h.state().active].tab().is_terminal());
    let doc_uid = h.state().panes[1].tab().uid;
    let demo_uid = h.state().panes[0].tab().uid;

    // In a terminal, Ctrl+W stays with the program; the terminal's key closes just this window.
    press_chord(&mut h, "Ctrl+W");
    assert_eq!(h.state().panes.len(), 3, "Vim keeps Ctrl+W");
    press_chord(&mut h, "Ctrl+Shift+W");
    let app = h.state();
    assert_eq!((app.panes.len(), app.strip.tabs.len()), (2, 2), "one window less, the tab stays");
    assert!(app.panes.iter().all(|p| !p.tab().is_terminal()));

    // Among files the short key works, and another window has the keyboard after it.
    h.state_mut().active = h.state().panes.iter().position(|p| p.tab().uid == doc_uid).unwrap();
    press_chord(&mut h, "Ctrl+W");
    let app = h.state();
    assert_eq!(app.panes.len(), 1);
    assert_eq!(app.panes[0].tab().uid, demo_uid, "the other one is left");
    assert_eq!(app.active, 0);

    // The last window of a tab closes the tab.
    press_chord(&mut h, "Ctrl+W");
    assert_eq!(h.state().strip.tabs.len(), 1);

    // Close tab takes all its windows at once, and reopening brings them back.
    let app = h.state_mut();
    app.split_pane(0, SplitDir::Right, false);
    app.split_pane(1, SplitDir::Down, false);
    let at = app.strip.tabs.len();
    app.new_files_tab(at, None, Some(Location::Local(d.demo.join("src"))));
    app.show_tab(0);
    settle(&mut h, 3);
    assert_eq!(h.state().panes.len(), 3);
    press_chord(&mut h, "Ctrl+Shift+X");
    assert_eq!(h.state().strip.tabs.len(), 1, "the whole tab");
    assert_eq!(h.state().panes.len(), 1);
    press_chord(&mut h, "Ctrl+Shift+T");
    assert_eq!(h.state().strip.tabs.len(), 2);
    assert_eq!(h.state().panes.len(), 3, "with its three windows");
}

/// Changing a shortcut in Settings: it takes effect, is saved, is taken from what had
/// it, and goes back.
#[test]
#[ignore]
fn shortcuts_can_be_changed_in_settings() {
    use crate::shortcuts::{self, Action, Chord};
    let d = demo_env();
    let cfg = settings("Breeze Light", &d);
    let mut h = app_harness(&cfg, "gnome");
    let app = h.state_mut();
    app.shot_pane(0, &d.demo, None);
    app.open_settings(SettingsPage::Shortcuts);
    settle(&mut h, 3);
    let os = shortcuts::Os::current();
    let first = |a| shortcuts::defaults(a, os)[0];
    let copy_key = first(Action::Copy);

    // Click its key, press a new one: a hint shows, and the old key is free.
    h.get_all_by_label(&copy_key.display(os)).next().expect("the key of Copy").click();
    settle(&mut h, 3);
    assert_eq!(h.state().shortcut_capture, Some((Action::Copy, Some(0))));
    press_chord(&mut h, "F12");
    assert_eq!(h.state().shortcut_capture, None);
    let new = Chord::parse("F12").unwrap();
    assert_eq!(shortcuts::active().keys(Action::Copy), [new]);
    assert_eq!(Settings::load().shortcuts["copy"], ["F12"], "saved");
    assert_eq!(shortcuts::hint(Action::Copy), "F12", "menus follow");

    // Keys that cannot work are refused, and it keeps waiting; Esc gives up.
    h.state_mut().shortcut_capture = Some((Action::Move, None));
    press_chord(&mut h, "X");
    assert!(h.state().shortcut_note.is_some());
    assert_eq!(h.state().shortcut_capture, Some((Action::Move, None)));
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert_eq!(h.state().shortcut_capture, None);
    assert_eq!(shortcuts::active().keys(Action::Move), shortcuts::defaults(Action::Move, os));

    // A key another action has moves here, and says so.
    h.state_mut().shortcut_capture = Some((Action::Move, Some(0)));
    press_chord(&mut h, "F12");
    assert_eq!(shortcuts::active().keys(Action::Move), [new]);
    assert_eq!(shortcuts::active().keys(Action::Copy), []);
    assert!(h.state().shortcut_note.as_deref().unwrap_or("").contains(&Action::Copy.label()));

    // Back to the defaults: nothing is left in the settings.
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("settings-note.png")).unwrap();
    }
    let label = format!("{} {}", t("Reset all to the defaults of"), os.name());
    h.get_by_label(&label).click();
    settle(&mut h, 3);
    h.get_by_label(&t("Reset")).click();
    settle(&mut h, 3);
    assert!(h.state().cfg.shortcuts.is_empty());
    assert!(Settings::load().shortcuts.is_empty());
    assert_eq!(shortcuts::active().keys(Action::Copy), shortcuts::defaults(Action::Copy, os));
}

/// A protected ZIP from the outside in: the message, the password (a wrong one first), the content.
#[test]
#[ignore]
fn password_protected_archives() {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let d = demo_env();
    let dir = d.demo.join("locked");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("secret.zip");
    let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let locked = SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "hunter2");
    z.start_file("note.txt", locked).unwrap();
    z.write_all(b"the treasure is under the tree").unwrap();
    z.start_file("open.txt", SimpleFileOptions::default()).unwrap();
    z.write_all(b"anyone can read this").unwrap();
    z.finish().unwrap();
    let mut cfg = settings("Breeze Light", &d);
    cfg.quick_view = true;
    let mut h = app_harness(&cfg, "gnome");
    let describe = |h: &Harness<'static, App>| h.state().quick.as_ref().map_or(String::new(), |q| q.describe());
    let wait_ready = |h: &mut Harness<'static, App>, name: &str| {
        let name = name.to_string();
        wait_until(h, 5.0, move |a| a.quick.as_ref().is_some_and(|q| q.name == name && q.is_ready()));
    };
    h.state_mut().shot_pane(0, &dir, Some("secret.zip"));
    wait_ready(&mut h, "secret.zip");
    assert!(describe(&h).contains("Password protected"), "{}", describe(&h));

    // Inside: its list is readable, and an unprotected file previews without a password.
    h.state_mut().run_cmd(Cmd::Open);
    h.state_mut().shot_wait_listing(0);
    settle(&mut h, 3);
    h.state_mut().panes[0].tab_mut().select_name("open.txt");
    wait_ready(&mut h, "open.txt");
    assert!(describe(&h).starts_with("code"), "{}", describe(&h));
    h.state_mut().panes[0].tab_mut().select_name("note.txt");
    wait_ready(&mut h, "note.txt");
    assert_eq!(describe(&h), "locked");

    // The button asks for it; a wrong password is told apart, and cleared.
    h.get_by_label(&t("Enter password…")).click();
    settle(&mut h, 3);
    assert!(matches!(h.state().dialog, Some(Dialog::ArchivePassword { .. })));
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("password-dialog.png")).unwrap();
    }
    let confirm = |h: &mut Harness<'static, App>, typed: &str| {
        if let Some(Dialog::ArchivePassword { password, .. }) = h.state_mut().dialog.as_mut() {
            *password = typed.to_string();
        }
        h.get_by_label(&t("Unlock")).click();
    };
    confirm(&mut h, "wrong");
    wait_until(&mut h, 5.0, |a| matches!(&a.dialog, Some(Dialog::ArchivePassword { wrong: true, .. })));
    let Some(Dialog::ArchivePassword { password, wrong, .. }) = &h.state().dialog else { panic!("still asking") };
    assert!(*wrong && password.is_empty());

    // The right one opens it, and the preview comes back with the content.
    confirm(&mut h, "hunter2");
    wait_until(&mut h, 5.0, |a| a.dialog.is_none());
    wait_ready(&mut h, "note.txt");
    assert!(describe(&h).starts_with("code"), "{}", describe(&h));

    // It is not written to the settings.
    h.state_mut().save_cfg();
    assert!(!std::fs::read_to_string(crate::config::config_file()).unwrap().contains("hunter2"));
}

/// Past the limit a file gets only the message. It is unreadable here, so any attempt
/// to open it would show an error instead; and a file under the limit still previews.
#[test]
#[ignore]
fn too_large_files_are_not_opened() {
    use std::os::unix::fs::PermissionsExt;
    let d = demo_env();
    let dir = d.demo.join("too-large");
    std::fs::create_dir_all(&dir).unwrap();
    let names = ["a.png", "a.pdf", "a.csv", "a.parquet", "a.sqlite", "a.h5", "a.xlsx", "a.mp4", "a.stl", "a.heic", "a.nef", "a.epub", "a.txt", "a.bin", "a.zip", "a.tar.gz"];
    let size = 20u64 * 1024 * 1024 * 1024;
    for name in names {
        let path = dir.join(name);
        std::fs::File::create(&path).unwrap().set_len(size).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    std::fs::write(dir.join("small.txt"), "hello").unwrap();
    let mut cfg = settings("Breeze Light", &d);
    cfg.quick_view = true;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &dir, None);
    settle(&mut h, 3);
    for name in names {
        h.state_mut().panes[0].tab_mut().select_name(name);
        settle(&mut h, 2);
        let q = h.state().quick.as_ref().expect("a preview");
        assert_eq!(q.name, name);
        assert_eq!(q.describe(), format!("too large {size}"), "{name}");
    }
    let _ = h.render().unwrap();
    h.state_mut().panes[0].tab_mut().select_name("small.txt");
    wait_until(&mut h, 3.0, |a| a.quick.as_ref().is_some_and(|q| q.name == "small.txt" && q.is_ready()));
    assert!(h.state().quick.as_ref().unwrap().describe().starts_with("code"));
    // So the demo folder can be removed next time.
    for name in names {
        std::fs::remove_file(dir.join(name)).unwrap();
    }
}

/// A huge image at full size: only the checkerboard squares in view get drawn.
#[test]
#[ignore]
fn huge_image_at_full_size_stays_responsive() {
    let d = demo_env();
    let dir = d.demo.join("huge-image");
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbaImage::from_pixel(8000, 8000, image::Rgba([40, 90, 200, 128])).save(dir.join("a.png")).unwrap();
    let mut cfg = settings("Breeze Light", &d);
    cfg.quick_view = true;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &dir, Some("a.png"));
    wait_until(&mut h, 40.0, |a| a.quick.as_ref().is_some_and(|q| q.is_ready()));
    h.get_by_label("1:1").click();
    settle(&mut h, 3);
    let t0 = Instant::now();
    for _ in 0..5 {
        h.step();
    }
    let per_frame = t0.elapsed() / 5;
    println!("frame at 1:1: {per_frame:?}");
    assert!(per_frame < Duration::from_millis(400), "{per_frame:?}");
}

/// One file through Quick View, as the app would show it, under a heap limit. A preview
/// that would exhaust memory aborts this process, so run it once per file:
/// `MATRIX_FINDER_STRESS_FILE=path` (`path//entry` for an entry in an archive).
#[test]
#[ignore]
fn quick_view_stress() {
    let Ok(target) = std::env::var("MATRIX_FINDER_STRESS_FILE") else { return };
    let scrolling = std::env::var("MATRIX_FINDER_STRESS_SCROLL").is_ok_and(|s| !s.is_empty());
    let limit_mb: usize = std::env::var("MATRIX_FINDER_STRESS_LIMIT_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(1536);
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(90));
        println!("STRESS timeout");
        std::process::exit(3);
    });
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    cfg.quick_view = true;
    let mut h = app_harness(&cfg, "gnome");
    crate::memcap::LIMIT.store(limit_mb * 1024 * 1024, std::sync::atomic::Ordering::Relaxed);
    let (file, inner) = match target.split_once("//") {
        Some((f, i)) => (PathBuf::from(f), Some(i.to_string())),
        None => (PathBuf::from(&target), None),
    };
    let name = inner.clone().unwrap_or_else(|| file.file_name().unwrap().to_string_lossy().into_owned());
    let app = h.state_mut();
    match inner {
        Some(_) => {
            let loc = Location::Archive { file: file.clone(), inner: String::new() };
            app.panes[0] = app.files_pane(loc);
            app.shot_wait_listing(0);
        }
        None => app.shot_pane(0, file.parent().unwrap(), None),
    }
    app.panes[0].tab_mut().select_name(&name);
    let t0 = Instant::now();
    // Scrolling through a folder quickly: the cursor moves on long before a preview has loaded.
    if let (true, Ok(dir)) = (scrolling, std::env::var("MATRIX_FINDER_STRESS_SCROLL")) {
        let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        h.state_mut().shot_pane(0, Path::new(&dir), None);
        for round in 0..3 {
            for n in &names {
                h.state_mut().panes[0].tab_mut().select_name(n);
                h.step();
                std::thread::sleep(Duration::from_millis(30 + round));
            }
        }
    }
    while t0.elapsed() < Duration::from_secs(80) {
        h.step();
        let wanted = h.state().panes[0].tab().cursor_entry().map(|e| e.name.clone());
        if h.state().quick.as_ref().is_some_and(|q| q.is_ready() && Some(&q.name) == wanted.as_ref()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    settle(&mut h, 4);
    let _ = h.render().unwrap();
    let status = h.state().quick.as_ref().map_or("no preview".to_string(), |q| q.describe());
    let peak = crate::memcap::PEAK.load(std::sync::atomic::Ordering::Relaxed) / (1024 * 1024);
    println!("STRESS ok {} ms, peak {peak} MB: {}", t0.elapsed().as_millis(), status.chars().take(110).collect::<String>());
}

/// Looks through a folder of sample files (`MATRIX_FINDER_SAMPLES`) as Quick View would,
/// writing each preview to `MATRIX_FINDER_SAMPLES_OUT` and printing how it loaded.
#[test]
#[ignore]
fn quick_view_samples() {
    let (Ok(dir), Ok(out)) = (std::env::var("MATRIX_FINDER_SAMPLES"), std::env::var("MATRIX_FINDER_SAMPLES_OUT")) else { return };
    let (dir, out) = (PathBuf::from(dir), PathBuf::from(out));
    std::fs::create_dir_all(&out).unwrap();
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    cfg.quick_view = true;
    cfg.quick_view_frac = 0.62;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &dir, None);
    settle(&mut h, 3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for name in names {
        h.state_mut().panes[0].tab_mut().select_name(&name);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(30) {
            h.step();
            if h.state().quick.as_ref().is_some_and(|q| q.is_ready() && q.name == name) {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        settle(&mut h, 3);
        let status = h.state().quick.as_ref().map_or("none".to_string(), |q| q.describe());
        println!("{name:<24} {:>6} ms  {status}", t0.elapsed().as_millis());
        let img = h.render().unwrap();
        img.save(out.join(format!("{name}.png"))).unwrap();
    }
}
