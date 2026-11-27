//! README screenshots, rendered headlessly with egui_kittest (wgpu):
//! `cargo test export_screenshots -- --ignored` (or `make screenshots`) writes every
//! scene to `docs/light/` and `docs/dark/`, then combines four of them into
//! `docs/showcase.png` and, with the logo, name, slogan and link above, `docs/banner.png`.
//!
//! Every scene starts a fresh `App` with `HOME` pointing at a generated demo tree
//! (`/tmp/matrix_finder`), so the user's own settings and files never show up.
//! Set `MATRIX_FINDER_SHOTS=name,name` to render only some scenes (`showcase` is
//! one of the names; it reads the scenes it combines from `docs/`).
use super::*;
use alacritty_terminal::term::TermMode;
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

/// A typical API answer, to explore as a tree (written out, to keep its keys in order).
/// Made-up tasks: no people, places or businesses.
fn write_api_json(path: &Path) {
    let tasks = [
        (1, "Write the release notes", false, 2, r#""docs", "release""#, 4, 1),
        (2, "Fix the flaky archive test", true, 1, r#""tests""#, 2, 2),
        (3, "Update the screenshots", false, 3, r#""docs", "images""#, 6, 0),
    ];
    let data: Vec<String> = tasks
        .iter()
        .map(|(id, title, done, priority, labels, total, checked)| {
            format!("    {{\n      \"id\": {id},\n      \"title\": \"{title}\",\n      \"done\": {done},\n      \"priority\": {priority},\n      \"labels\": [{labels}],\n      \"checklist\": {{ \"total\": {total}, \"done\": {checked} }}\n    }}")
        })
        .collect();
    let doc = format!(
        "{{\n  \"status\": \"ok\",\n  \"page\": {{ \"number\": 1, \"size\": 3, \"total\": 42 }},\n  \"data\": [\n{}\n  ],\n  \"next\": \"https://api.example.com/tasks?page=2\",\n  \"error\": null\n}}\n",
        data.join(",\n")
    );
    std::fs::write(path, doc).unwrap();
}

/// Hourly readings of a few weather stations.
fn write_weather_h5(path: &Path) {
    let (hours, stations) = (24usize, 5usize);
    let mut r = Rng(7);
    let temp: Vec<f32> = (0..hours * stations)
        .map(|i| {
            let (h, s) = (i / stations, i % stations);
            let day = (std::f32::consts::TAU * (h as f32 - 9.0) / 24.0).sin();
            ((24.0 + s as f32 * 1.5 + 5.0 * day + (r.next() % 100) as f32 / 100.0) * 10.0).round() / 10.0
        })
        .collect();
    let humidity: Vec<f32> = temp.iter().map(|t| ((120.0 - t * 1.6) * 10.0).round() / 10.0).collect();
    let mut b = hdf5_pure::FileBuilder::new();
    b.create_dataset("temperature_c").with_f32_data(&temp).with_shape(&[hours as u64, stations as u64]);
    b.create_dataset("humidity_pct").with_f32_data(&humidity).with_shape(&[hours as u64, stations as u64]);
    b.create_dataset("station_id").with_i32_data(&[48820, 48855, 48900, 48845, 48877]).with_shape(&[stations as u64]);
    b.create_dataset("hour").with_i32_data(&(0..hours as i32).collect::<Vec<_>>()).with_shape(&[hours as u64]);
    std::fs::write(path, b.finish().unwrap()).unwrap();
}

/// The header of a small transformer's weights, with zeros for the numbers.
fn write_safetensors(path: &Path) {
    let (vocab, dim, ffn, layers) = (4096u64, 64u64, 172u64, 4);
    let mut tensors: Vec<(String, &str, Vec<u64>)> = vec![("model.embed_tokens.weight".into(), "BF16", vec![vocab, dim])];
    for l in 0..layers {
        for p in ["q_proj", "k_proj", "v_proj", "o_proj"] {
            tensors.push((format!("model.layers.{l}.self_attn.{p}.weight"), "BF16", vec![dim, dim]));
        }
        tensors.push((format!("model.layers.{l}.mlp.gate_proj.weight"), "BF16", vec![ffn, dim]));
        tensors.push((format!("model.layers.{l}.mlp.up_proj.weight"), "BF16", vec![ffn, dim]));
        tensors.push((format!("model.layers.{l}.mlp.down_proj.weight"), "BF16", vec![dim, ffn]));
        tensors.push((format!("model.layers.{l}.input_layernorm.weight"), "F32", vec![dim]));
        tensors.push((format!("model.layers.{l}.post_attention_layernorm.weight"), "F32", vec![dim]));
    }
    tensors.push(("model.norm.weight".into(), "F32", vec![dim]));
    tensors.push(("lm_head.weight".into(), "BF16", vec![vocab, dim]));
    let mut header = serde_json::Map::new();
    header.insert("__metadata__".into(), serde_json::json!({"format": "pt", "model": "tiny-llama-demo"}));
    let mut at = 0u64;
    for (name, dtype, shape) in &tensors {
        let size = shape.iter().product::<u64>() * if *dtype == "F32" { 4 } else { 2 };
        header.insert(name.clone(), serde_json::json!({"dtype": dtype, "shape": shape, "data_offsets": [at, at + size]}));
        at += size;
    }
    let header = serde_json::to_string(&header).unwrap();
    let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
    bytes.extend(header.as_bytes());
    bytes.resize(bytes.len() + at as usize, 0);
    std::fs::write(path, bytes).unwrap();
}

/// An odd polygon for the geometry view: a Koch snowflake with a star-shaped hole,
/// in longitude and latitude (around Hoan Kiem Lake, Hanoi).
fn write_snowflake_geojson(path: &Path) {
    let (cx, cy, r) = (105.8524f64, 21.0287f64, 0.01f64);
    // A triangle, counter-clockwise, each side then cut in a Koch bump four times.
    let mut ring: Vec<[f64; 2]> = (0..3)
        .map(|i| {
            let a = std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * i as f64 / 3.0;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect();
    for _ in 0..4 {
        let mut next = Vec::with_capacity(ring.len() * 4);
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let d = [(b[0] - a[0]) / 3.0, (b[1] - a[1]) / 3.0];
            let p1 = [a[0] + d[0], a[1] + d[1]];
            // The bump points outwards: to the right of a counter-clockwise edge.
            let (c, s) = ((-60f64).to_radians().cos(), (-60f64).to_radians().sin());
            let peak = [p1[0] + d[0] * c - d[1] * s, p1[1] + d[0] * s + d[1] * c];
            next.extend([a, p1, peak, [a[0] + 2.0 * d[0], a[1] + 2.0 * d[1]]]);
        }
        ring = next;
    }
    ring.push(ring[0]);
    let mut star: Vec<[f64; 2]> = (0..10)
        .map(|i| {
            let a = std::f64::consts::FRAC_PI_2 - std::f64::consts::TAU * i as f64 / 10.0;
            let k = if i % 2 == 0 { 0.42 } else { 0.17 };
            [cx + r * k * a.cos(), cy + r * k * a.sin()]
        })
        .collect();
    star.push(star[0]);
    let round = |v: &[[f64; 2]]| v.iter().map(|p| [(p[0] * 1e6).round() / 1e6, (p[1] * 1e6).round() / 1e6]).collect::<Vec<_>>();
    let doc = serde_json::json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "properties": {"name": "Koch snowflake with a star-shaped hole"},
            "geometry": {"type": "Polygon", "coordinates": [round(&ring), round(&star)]}
        }]
    });
    std::fs::write(path, serde_json::to_string(&doc).unwrap()).unwrap();
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
    let chapter = |title: &str, body: &str| {
        format!("<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{title}</title></head><body><h1>{title}</h1>{body}</body></html>")
    };
    let one = chapter(
        "The Long Road South",
        "<p>The train left <b>Hà Nội</b> before dawn, and by noon the rice fields had given way to <i>limestone hills</i> that rose straight out of the green.</p><p>She kept three things in her bag:</p><ul><li>a notebook</li><li>her grandmother's letters</li><li>a ticket with no return date</li></ul><blockquote><p>Every road is long until you have walked it.</p></blockquote><p>By evening the air smelled of salt, and the conductor called out <b>Đà Nẵng</b> as if it were a question.</p>",
    );
    let two = chapter("Arrival", "<p>Nobody was waiting at the station, which was exactly what she had hoped for.</p>");
    let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let o = zip::write::SimpleFileOptions::default();
    let opf = r#"<package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>A Short Story</dc:title><dc:creator>Matrix Finder demo</dc:creator></metadata><manifest><item id="a" href="one.xhtml"/><item id="b" href="two.xhtml"/></manifest><spine><itemref idref="a"/><itemref idref="b"/></spine></package>"#;
    for (name, text) in [
        ("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>"#),
        ("OPS/book.opf", opf),
        ("OPS/one.xhtml", one.as_str()),
        ("OPS/two.xhtml", two.as_str()),
    ] {
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
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {content} 0 R >>"));
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
    if cfg!(unix) {
        PathBuf::from("/tmp/matrix_finder")
    } else {
        std::env::temp_dir().join("matrix_finder")
    }
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
            (n.to_string(), crate::tools::checksum::hash_reader(f, crate::tools::checksum::Algo::Sha256, |_| true).unwrap())
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
    touch(&demo.join("src/filter.rs"), include_str!("../ui/filter.rs").as_bytes());
    touch(&demo.join("src/theme.rs"), include_str!("../style/theme.rs").as_bytes());
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
    touch(&formats.join("Source Sans 3.otf"), include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/SourceSans3-Regular.otf")));
    touch(&formats.join("server.pem"), include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/testdata/test-cert.pem")));
    write_torus_stl(&formats.join("torus.stl"));
    write_snowflake_geojson(&formats.join("snowflake.geojson"));
    write_api_json(&formats.join("api-response.json"));
    write_weather_h5(&formats.join("weather.h5"));
    write_safetensors(&formats.join("tiny-llm.safetensors"));
    write_sqlite(&formats.join("shop.sqlite"), &rows[..600]);
    write_epub(&formats.join("A Short Story.epub"));
    let jsonl: String = rows[..200]
        .iter()
        .map(|o| format!("{}\n", serde_json::json!({"order": o.id, "city": o.city, "product": o.product, "qty": o.qty, "paid": o.paid})))
        .collect();
    touch(&formats.join("orders.jsonl"), jsonl.as_bytes());
    // Git repositories among the demo folders, so the list shows their branches (made
    // after the archives above, which must not hold a `.git`). Skipped without git.
    for (dir, branch, dirty) in [("docs", "main", false), ("papers", "main", false), ("src", "feature/sql-filter", true)] {
        let dir = demo.join(dir);
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .env("GIT_AUTHOR_DATE", "2026-11-04T16:30:00")
                .env("GIT_COMMITTER_DATE", "2026-11-04T16:30:00")
                .output()
                .is_ok_and(|o| o.status.success())
        };
        if git(&["init", "-q", "-b", branch]) && git(&["add", "-A"]) {
            git(&["-c", "user.email=demo@example.com", "-c", "user.name=Demo", "commit", "-qm", "Start"]);
            if dirty {
                touch(&dir.join("sql.rs"), b"// work in progress\n");
            }
        }
    }
    // A home-like folder with one file of each kind (icons).
    let h2 = demo.join("home");
    for d in ["Desktop", "Documents", "Downloads", "Music", "Pictures", "Projects", "Videos"] {
        std::fs::create_dir_all(h2.join(d)).unwrap();
    }
    for f in [
        "app.py",
        "backup.zip",
        "data.json",
        "index.html",
        "letter.docx",
        "logo.svg",
        "main.rs",
        "movie.mp4",
        "notes.md",
        "photo.jpg",
        "readme.txt",
        "report.pdf",
        "setup.sh",
        "sheet.xlsx",
        "slides.pptx",
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
        Bookmark::new("Data", demo.demo.join("data").to_string_lossy()),
        // A group, so the screenshots show that bookmarks can be kept together.
        github_group(demo),
    ];
    cfg
}

/// A bookmark group "Github" holding two projects (code icons), for the screenshots.
fn github_group(d: &Demo) -> Bookmark {
    let code = |name: &str, path: PathBuf| Bookmark { icon: crate::config::BookmarkIcon::Code, ..Bookmark::new(name, path.to_string_lossy()) };
    let mut g = Bookmark::new_folder("Github");
    g.children = vec![code("Project 1", d.home.join("Projects/matrix_finder")), code("Project 2", d.demo.join("src"))];
    g
}

/// Terminal profiles have their own colour theme (Breeze Dark by default).
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
        .build_eframe(|cc| App::new(cc, crate::ui::chrome::Chrome::detect(true), None));
    let app = h.state_mut();
    // Only the root file system: the sidebar must not show this machine's volumes.
    fn root() -> Vec<crate::files::drives::Drive> {
        crate::files::drives::list()
            .into_iter()
            .filter(|d| d.mount == Path::new("/"))
            .map(|mut d| {
                d.label = "/".into();
                d
            })
            .collect()
    }
    app.drives = root();
    app.drive_watch = crate::files::drives::Watcher::spawn_with(root, Duration::from_secs(3600), Duration::from_secs(3600), None);
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

/// Drag a dialog (`id` is its egui id: the app's windows set their own, e.g. "about")
/// to the middle of the window, as a user would: egui opens dialogs towards the top left.
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
#[cfg(unix)]
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

/// The AI button in a terminal's info bar: it starts the chosen agent at the prompt,
/// is greyed out while a program runs, and Settings chooses the agent or turns it off.
#[cfg(unix)]
#[test]
#[ignore]
fn terminal_ai_button() {
    use crate::system::agents::{AgentAct, Found};
    use egui::accesskit::Role;
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    // Stand-in agents: each notes the folder it started in, then keeps running.
    let bin = d.home.join("agent-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = |name: &str| {
        let p = bin.join(name);
        std::fs::write(&p, "#!/bin/sh\npwd > \"$0.ran\"\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&p, <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_file(bin.join(format!("{name}.ran")));
        Found { id: crate::system::agents::get(name).unwrap().id, run: p.to_string_lossy().to_string() }
    };
    let found = vec![fake("claude"), fake("codex"), fake("gemini")];
    let term = h.state_mut().shot_terminal(&d.demo, None, None);
    h.state_mut().agents = Some(found);
    wait_until(&mut h, 2.0, |_| false);
    let idle = |app: &App| app.panes[0].tab().term.as_ref().is_some_and(|t| t.is_idle());
    wait_until(&mut h, 5.0, idle);

    // One button, for the first agent installed.
    assert!(h.query_by_role_and_label(Role::Button, "Claude Code").is_some());
    assert!(h.query_by_label("Codex").is_none());
    assert!(h.query_by_label("Gemini CLI").is_none());

    // At the prompt, a click starts it in this terminal and folder.
    h.get_by_role_and_label(Role::Button, "Claude Code").click();
    wait_until(&mut h, 5.0, |app| !idle(app) && bin.join("claude.ran").exists());
    let ran = std::fs::read_to_string(bin.join("claude.ran")).expect("the agent ran");
    assert_eq!(Path::new(ran.trim()).canonicalize().unwrap(), d.demo.canonicalize().unwrap());
    assert_eq!(h.state().panes[0].tab().uid, term);

    // While it runs, the button is greyed out and a click does nothing.
    let tabs = h.state().strip.tabs.len();
    assert!(h.get_by_role_and_label(Role::Button, "Claude Code").is_disabled());
    std::fs::remove_file(bin.join("claude.ran")).unwrap();
    h.get_by_role_and_label(Role::Button, "Claude Code").click();
    wait_until(&mut h, 1.0, |_| false);
    assert!(!bin.join("claude.ran").exists());
    assert_eq!(h.state().strip.tabs.len(), tabs);

    // Back at the prompt it works again.
    term_type(&mut h, 0, term, "\x03");
    wait_until(&mut h, 5.0, idle);
    assert!(!h.get_by_role_and_label(Role::Button, "Claude Code").is_disabled());

    // Its menu switches to another installed agent; that is saved.
    h.state_mut().agent_act(0, AgentAct::Choose("codex"));
    settle(&mut h, 3);
    assert!(h.query_by_label("Claude Code").is_none());
    assert!(h.query_by_role_and_label(Role::Button, "Codex").is_some());
    assert_eq!(Settings::load().term_agent, "codex");

    // Settings → Terminal: pick Gemini CLI; an agent that is not installed cannot be
    // picked; Automatic goes back to the first one installed.
    h.state_mut().open_settings(SettingsPage::Terminal);
    settle(&mut h, 4);
    scroll_settings(&mut h, 2000.0);
    h.get_by_role_and_label(Role::RadioButton, "Gemini CLI").click();
    settle(&mut h, 3);
    assert_eq!(h.state().cfg.term_agent, "gemini");
    assert!(h.get_by_role_and_label(Role::RadioButton, "Qwen Code").is_disabled());
    h.get_by_role_and_label(Role::RadioButton, "Qwen Code").click();
    settle(&mut h, 3);
    assert_eq!(h.state().cfg.term_agent, "gemini");
    let settings_tab = h.state().strip.active;
    h.state_mut().show_tab(settings_tab - 1);
    settle(&mut h, 3);
    assert!(h.query_by_role_and_label(Role::Button, "Gemini CLI").is_some());
    h.state_mut().show_tab(settings_tab);
    settle(&mut h, 3);
    h.get_by_role_and_label(Role::RadioButton, &t("Automatic: the first one installed")).click();
    settle(&mut h, 3);
    assert_eq!(h.state().cfg.term_agent, "");

    // The switch turns the button off.
    h.get_by_role_and_label(Role::CheckBox, &t("Show the AI button")).click();
    settle(&mut h, 3);
    assert!(!h.state().cfg.term_agents);
    h.state_mut().show_tab(settings_tab - 1);
    settle(&mut h, 3);
    assert!(h.query_by_label("Claude Code").is_none());
}

/// A profile's startup script runs at the prompt of a started shell: what the login
/// and startup files set up (variables, functions) is there, even when they are slow.
#[test]
#[ignore]
fn terminal_startup_script_after_shell_start() {
    let d = demo_env();
    if !Path::new("/bin/zsh").exists() {
        eprintln!("skipped: no zsh");
        return;
    }
    touch(&d.home.join(".zprofile"), b"export MF_LOGIN=from-zprofile\n");
    let rc = std::fs::read_to_string(d.home.join(".zshrc")).unwrap_or_default();
    touch(&d.home.join(".zshrc"), format!("{rc}sleep 1\nmf_rc_fn() {{ echo from-zshrc; }}\n").as_bytes());
    let out = d.home.join("startup-out.txt");
    let _ = std::fs::remove_file(&out);
    let mut cfg = settings("Breeze Light", &d);
    cfg.terminal_profiles[0].shell = "/bin/zsh".into();
    cfg.terminal_profiles[0].startup = "mf_rc_fn > ~/startup-out.txt\n\necho \"$MF_LOGIN\" >> ~/startup-out.txt".into();
    let mut h = app_harness(&cfg, "gnome");
    let home = d.home.clone();
    h.state_mut().shot_terminal(&home, None, None);
    wait_until(&mut h, 10.0, |_| std::fs::read_to_string(&out).is_ok_and(|s| s.lines().count() >= 2));
    assert_eq!(std::fs::read_to_string(&out).unwrap_or_default(), "from-zshrc\nfrom-zprofile\n");
}

/// Git folders in the list: `(git) [branch]` after the name, cut with "…" when long, in
/// the name's colour, or (if asked for) green when everything is committed and orange
/// with changes. Off in Settings, they are plain folders again.
#[test]
#[ignore]
fn git_folders_show_badges_and_branches() {
    let d = demo_env();
    let root = d.home.join("Projects");
    let _ = std::fs::remove_dir_all(&root);
    let git = |dir: &Path, args: &[&str]| {
        let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().is_ok_and(|o| o.status.success());
        assert!(ok, "git {args:?}");
    };
    let repo = |name: &str, branch: &str| {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", branch]);
        touch(&dir.join("README.md"), b"# demo\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["-c", "user.email=demo@example.com", "-c", "user.name=Demo", "commit", "-qm", "init"]);
        dir
    };
    repo("matrix_finder", "main");
    let web = repo("website", "feature/redesign-landing-page-with-new-hero");
    touch(&web.join("index.html"), b"<h1>new</h1>");
    let notes = repo("notes", "main");
    git(&notes, &["checkout", "-q", "--detach"]);
    std::fs::create_dir_all(root.join("photos")).unwrap();
    let ctx = egui::Context::default();
    let look = |name: &str| crate::files::gitinfo::wait_for(&root.join(name), true, &ctx);
    use crate::files::gitinfo::State;
    assert!(matches!(look("matrix_finder"), Some(r) if r.branch == "main" && r.state == State::Clean && !r.detached));
    assert!(matches!(look("website"), Some(r) if r.branch.starts_with("feature/") && r.state == State::Dirty && r.untracked == 1));
    assert!(matches!(look("notes"), Some(r) if r.detached && r.branch.len() == 7));
    assert_eq!(look("photos"), None);
    use crate::config::GitStyle;
    for (theme, colored, style, file) in [
        ("Breeze Light", false, GitStyle::Text, "git-folders-light"),
        ("Breeze Light", true, GitStyle::Text, "git-folders-color"),
        ("Breeze Light", false, GitStyle::Filled, "git-folders-filled"),
        ("Breeze Light", true, GitStyle::Filled, "git-folders-filled-color"),
        ("Breeze Dark", false, GitStyle::Filled, "git-folders-dark-filled"),
        ("Breeze Dark", true, GitStyle::Text, "git-folders-dark"),
    ] {
        let mut cfg = settings(theme, &d);
        cfg.git_status = colored;
        cfg.git_style = style;
        let mut h = app_harness(&cfg, "gnome");
        h.state_mut().shot_pane(0, &root, Some("photos"));
        wait_until(&mut h, 8.0, |_| false);
        shot(&mut h, file);
    }
    // The switches in Settings → Files & layout → Git.
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    assert!(h.state().cfg.git_branches && h.state().cfg.git_label && !h.state().cfg.git_status, "default: branches, (git), one colour");
    h.state_mut().open_settings(SettingsPage::Files);
    settle(&mut h, 4);
    scroll_settings(&mut h, 220.0);
    h.query_all_by_label("Colour by commit state").last().expect("the switch").click();
    settle(&mut h, 3);
    assert!(h.state().cfg.git_status);
    shot(&mut h, "git-settings");
    let mut cfg = settings("Breeze Light", &d);
    cfg.git_branches = false;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &root, Some("photos"));
    wait_until(&mut h, 2.0, |_| false);
    shot(&mut h, "git-folders-off");
}

/// A full-screen program like Claude Code: it turns on the kitty keyboard protocol, mouse
/// reports (SGR, with motion) and focus reports, and gets the clicks, keys and focus
/// changes in the forms it asked for.
#[test]
#[ignore]
fn terminal_tui_input_like_claude_code() {
    let d = demo_env();
    let log = d.home.join("tui-input.bin");
    let _ = std::fs::remove_file(&log);
    // Raw input, logged byte for byte; it ends on Ctrl+Q.
    let script = r#"
import os, sys, termios, tty
fd = sys.stdin.fileno()
tty.setraw(fd)
out = open(os.path.expanduser("~/tui-input.bin"), "wb", buffering=0)
sys.stdout.write("[?1049h[?1000h[?1002h[?1003h[?1006h[?1004h[>5u[?u")
sys.stdout.flush()
while True:
    b = os.read(fd, 1024)
    out.write(b)
    if b"" in b or b"113;5u" in b:
        break
sys.stdout.write("[<u[?1049l")
"#;
    touch(&d.home.join("tui.py"), script.as_bytes());
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let home = d.home.clone();
    let uid = h.state_mut().shot_terminal(&home, None, None);
    term_type(&mut h, 0, uid, "python3 ~/tui.py\r");
    let mode = |app: &App| app.panes[0].tab().term.as_ref().map(|t| t.mode()).unwrap_or(TermMode::empty());
    wait_until(&mut h, 10.0, |app| mode(app).contains(TermMode::ALT_SCREEN | TermMode::MOUSE_MOTION | TermMode::DISAMBIGUATE_ESC_CODES));
    let m = mode(h.state());
    assert!(m.contains(TermMode::SGR_MOUSE | TermMode::MOUSE_MOTION | TermMode::FOCUS_IN_OUT), "{m:?}");
    assert!(m.contains(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALTERNATE_KEYS), "kitty flags 1 and 4: {m:?}");
    let read = || std::fs::read(&log).unwrap_or_default();
    let has = |needle: &[u8]| read().windows(needle.len()).any(|w| w == needle);

    // A click in the middle of the terminal: press and release, in SGR form.
    let at = pos2(W * 0.5, H * 0.5);
    move_to(&mut h, at);
    button(&mut h, at, PointerButton::Primary, true, Modifiers::NONE);
    button(&mut h, at, PointerButton::Primary, false, Modifiers::NONE);
    wait_until(&mut h, 2.0, |_| has(b"m"));
    let text = String::from_utf8_lossy(&read()).into_owned();
    assert!(regex::Regex::new(r"\x1b\[<0;\d+;\d+M").unwrap().is_match(&text), "press: {text:?}");
    assert!(regex::Regex::new(r"\x1b\[<0;\d+;\d+m").unwrap().is_match(&text), "release: {text:?}");
    assert!(regex::Regex::new(r"\x1b\[<35;\d+;\d+M").unwrap().is_match(&text), "motion: {text:?}");

    // A click whose pointer wobbles a few pixels (across a cell's edge, at some of these
    // places) stays a click: no drag, and released where it was pressed.
    let sgr = regex::Regex::new(r"\x1b\[<(\d+);(\d+);(\d+)([Mm])").unwrap();
    for dx in 0..8 {
        let p = pos2(W * 0.5 + dx as f32, H * 0.5 + 40.0);
        move_to(&mut h, p);
        let start = read().len();
        button(&mut h, p, PointerButton::Primary, true, Modifiers::NONE);
        move_to(&mut h, p + egui::vec2(3.0, 1.0));
        button(&mut h, p + egui::vec2(3.0, 1.0), PointerButton::Primary, false, Modifiers::NONE);
        wait_until(&mut h, 2.0, |_| read()[start..].ends_with(b"m"));
        let text = String::from_utf8_lossy(&read()[start..]).into_owned();
        let reports: Vec<_> = sgr.captures_iter(&text).map(|c| (c[1].to_string(), c[2].to_string(), c[3].to_string(), c[4].to_string())).collect();
        assert!(reports.iter().all(|r| r.0 == "0"), "a drag was reported: {text:?}");
        let (press, release) = (reports.iter().find(|r| r.3 == "M").unwrap(), reports.iter().find(|r| r.3 == "m").unwrap());
        assert_eq!((&press.1, &press.2), (&release.1, &release.2), "{text:?}");
    }
    // A real drag is still reported as one.
    let start = read().len();
    let p = pos2(W * 0.5, H * 0.5 + 40.0);
    button(&mut h, p, PointerButton::Primary, true, Modifiers::NONE);
    move_to(&mut h, p + egui::vec2(60.0, 0.0));
    button(&mut h, p + egui::vec2(60.0, 0.0), PointerButton::Primary, false, Modifiers::NONE);
    wait_until(&mut h, 2.0, |_| read()[start..].ends_with(b"m"));
    assert!(String::from_utf8_lossy(&read()[start..]).contains("\x1b[<32;"), "drag");

    // Shift+Enter is not Enter.
    let start = read().len();
    push(&mut h, Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::SHIFT });
    settle(&mut h, 3);
    wait_until(&mut h, 2.0, |_| read().len() > start);
    assert_eq!(&read()[start..], b"\x1b[13;2u");

    // Focus going and coming back.
    let start = read().len();
    for focused in [false, true] {
        h.input_mut().focused = focused;
        push(&mut h, Event::WindowFocused(focused));
        settle(&mut h, 2);
    }
    wait_until(&mut h, 2.0, |_| read().len() >= start + 6);
    assert_eq!(&read()[start..], b"\x1b[O\x1b[I");

    // A right click goes to the program, and opens no menu of the app's own.
    let start = read().len();
    right_click(&mut h, pos2(W * 0.5, H * 0.5));
    wait_until(&mut h, 2.0, |_| read()[start..].ends_with(b"m"));
    assert!(String::from_utf8_lossy(&read()[start..]).contains("\x1b[<2;"), "right click: {:?}", String::from_utf8_lossy(&read()[start..]));
    assert!(h.query_by_label(&t("Select all")).is_none(), "the app's menu opened");

    // An input method (Telex here): what it composes is only drawn, with the input
    // method's window placed at the cursor; what it commits is typed.
    let start = read().len();
    for e in [
        Event::Ime(egui::ImeEvent::Enabled),
        Event::Ime(egui::ImeEvent::Preedit("vie".into())),
        Event::Text("x".into()),
        Event::Ime(egui::ImeEvent::Preedit("việ".into())),
    ] {
        push(&mut h, e);
        settle(&mut h, 1);
    }
    let ime = h.output().platform_output.ime.expect("IME allowed in the terminal");
    assert!(ime.rect.width() > 0.0 && ime.rect.width() < W / 2.0, "{ime:?}");
    for e in [Event::Ime(egui::ImeEvent::Disabled), Event::Ime(egui::ImeEvent::Commit("việt".into())), Event::Ime(egui::ImeEvent::Disabled)] {
        push(&mut h, e);
    }
    settle(&mut h, 2);
    wait_until(&mut h, 2.0, |_| read().len() > start);
    assert_eq!(String::from_utf8_lossy(&read()[start..]), "việt");

    // Ctrl+Q, as a kitty key, ends it; then the shell gets plain keys again.
    push(&mut h, Event::Key { key: egui::Key::Q, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL });
    wait_until(&mut h, 3.0, |app| !mode(app).contains(TermMode::ALT_SCREEN));
    assert!(has(b"\x1b[113;5u"), "{:?}", String::from_utf8_lossy(&read()));
    assert!(!mode(h.state()).intersects(TermMode::KITTY_KEYBOARD_PROTOCOL));
}

/// Cmd/Ctrl+click on a link opens it even while the program in the terminal takes the
/// mouse (as Claude Code does): first a question that shows where it goes, then the
/// browser chosen in the settings; "Don't ask again" holds for that terminal.
#[cfg(unix)]
#[test]
#[ignore]
fn terminal_links_open_after_asking() {
    let d = demo_env();
    let (log, opened) = (d.home.join("link-input.bin"), d.home.join("opened.txt"));
    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_file(&opened);
    // A screen full of links, the mouse reported to the program, its input logged.
    let script = r#"
import os, sys, tty
fd = sys.stdin.fileno()
tty.setraw(fd)
out = open(os.path.expanduser("~/link-input.bin"), "wb", buffering=0)
cols, rows = os.get_terminal_size(fd)
sys.stdout.write("\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[H")
for r in range(rows - 1):
    sys.stdout.write(("https://example.com/docs/" + "x" * cols)[: cols - 1] + "\r\n")
sys.stdout.flush()
while True:
    b = os.read(fd, 1024)
    out.write(b)
    if b"\x11" in b:
        break
sys.stdout.write("\x1b[?1000l\x1b[?1006l\x1b[?1049l")
"#;
    touch(&d.home.join("links.py"), script.as_bytes());
    // The "browser" writes down the links it is given.
    let browser = d.home.join("browser.sh");
    touch(&browser, format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n", opened.display()).as_bytes());
    std::fs::set_permissions(&browser, <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755)).unwrap();
    let mut cfg = settings("Breeze Light", &d);
    cfg.browser = format!("\"{}\"", browser.display());
    assert!(cfg.term_confirm_links, "asks by default");
    let mut h = app_harness(&cfg, "gnome");
    let home = d.home.clone();
    let uid = h.state_mut().shot_terminal(&home, None, None);
    term_type(&mut h, 0, uid, "python3 ~/links.py\r");
    let mode = |app: &App| app.panes[0].tab().term.as_ref().map(|t| t.mode()).unwrap_or(TermMode::empty());
    wait_until(&mut h, 10.0, |app| mode(app).contains(TermMode::ALT_SCREEN | TermMode::MOUSE_REPORT_CLICK));
    wait_until(&mut h, 1.0, |_| false);
    let read = || String::from_utf8_lossy(&std::fs::read(&log).unwrap_or_default()).into_owned();
    let links = || std::fs::read_to_string(&opened).unwrap_or_default();
    let at = pos2(W * 0.5, H * 0.5);
    let cmd_click = |h: &mut Harness<'static, App>| cmd_click_at(h, at);

    // The question, with the site; the program does not get the click.
    cmd_click(&mut h);
    let url = match &h.state().dialog {
        Some(Dialog::OpenLink { url, .. }) => url.clone(),
        _ => panic!("no question before opening the link"),
    };
    assert!(url.starts_with("https://example.com/docs/x"), "{url}");
    assert!(h.query_by_label("example.com").is_some(), "the site is shown");
    assert!(!read().contains("\x1b[<0;"), "the program got the click: {:?}", read());
    // Cancel opens nothing.
    let p = node_center(&h, &t("Cancel"), 0.0).expect("Cancel");
    click_at(&mut h, p);
    assert!(h.state().dialog.is_none());
    wait_until(&mut h, 1.0, |_| false);
    assert_eq!(links(), "");

    // Open, and stop asking in this terminal.
    cmd_click(&mut h);
    let p = node_center(&h, &t("Don't ask again in this terminal"), 0.0).expect("checkbox");
    click_at(&mut h, p);
    assert!(matches!(&h.state().dialog, Some(Dialog::OpenLink { stop_asking: true, .. })));
    let p = node_center(&h, &t("Open link"), 0.0).expect("Open link");
    click_at(&mut h, p);
    wait_until(&mut h, 5.0, |_| !links().is_empty());
    assert_eq!(links(), format!("{url}\n"));
    assert!(h.state().dialog.is_none());

    // Again: no question this time.
    cmd_click(&mut h);
    assert!(h.state().dialog.is_none(), "asked again");
    wait_until(&mut h, 5.0, |_| links().lines().count() == 2);
    assert_eq!(links().lines().count(), 2);

    // A plain click still goes to the program.
    click_at(&mut h, at);
    wait_until(&mut h, 2.0, |_| read().contains("\x1b[<0;"));
    assert!(read().contains("\x1b[<0;"), "plain click: {:?}", read());
    if let Some(t) = h.state().panes[0].tab().term.as_ref() {
        t.write(vec![0x11]);
    }
    wait_until(&mut h, 3.0, |app| !mode(app).contains(TermMode::ALT_SCREEN));

    // At the shell's prompt (no mouse reporting) too.
    term_type(&mut h, 0, uid, "stty sane; clear; for i in $(seq 60); do echo https://example.org/shell/$i; done\r");
    cmd_click_at(&mut h, pos2(300.0, H * 0.5));
    wait_until(&mut h, 5.0, |_| links().lines().count() == 3);
    assert!(links().lines().nth(2).is_some_and(|l| l.starts_with("https://example.org/shell/")), "{}", links());
}

fn cmd_click_at(h: &mut Harness<'static, App>, at: Pos2) {
    move_to(h, at);
    button(h, at, PointerButton::Primary, true, Modifiers::COMMAND);
    button(h, at, PointerButton::Primary, false, Modifiers::COMMAND);
    h.input_mut().modifiers = Modifiers::NONE;
    settle(h, 3);
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
    let width = |h: &Harness<'static, App>, family: egui::FontFamily| {
        h.ctx.fonts(|f| f.layout_no_wrap("MMMMMMMMMM".into(), egui::FontId::new(14.0, family), egui::Color32::WHITE).size().x)
    };
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
    h.state_mut().cfg.ui_font_size = crate::style::fonts::DEFAULT_UI_SIZE;
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
        wait_for_git(&mut h, d);
        shot(&mut h, "panels");
    }

    // The same two panels with a file's menu open on Layout: how a window is split.
    if want("panels-layout") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some("bundle.zip"));
        app.shot_pane(1, &d.demo.join("home"), Some(".."));
        app.shot_focus(0);
        wait_for_git(&mut h, d);
        // The archive.tar row of the left window.
        right_click(&mut h, pos2(330.0, 524.0));
        if let Some(p) = node_center(&h, &t("Layout"), 300.0) {
            move_to(&mut h, p);
            settle(&mut h, 4);
        }
        shot(&mut h, "panels-layout");
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

    // The filter language: an SQL-like query filters the folder as it is typed.
    if want("sql-filter") {
        let mut cfg = settings(mode.theme, d);
        cfg.quick_view = true;
        cfg.quick_view_frac = 0.48;
        let mut h = app_harness(&cfg, gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, None);
        app.shot_focus(0);
        settle(&mut h, 3);
        // Typed straight into the list: the small search box takes the query.
        push(&mut h, Event::Text("name LIKE 'order%' AND size > 15KB".into()));
        settle(&mut h, 6);
        assert_eq!(h.state().panes[0].tab().view.iter().filter(|&&i| !h.state().panes[0].tab().entries[i].is_parent).count(), 3, "the three orders files");
        // The Parquet one, previewed.
        key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
        key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
        settle(&mut h, 10);
        std::thread::sleep(Duration::from_millis(600));
        settle(&mut h, 10);
        shot(&mut h, "sql-filter");
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

    // The sidebar's drives (a USB stick with its Eject button) and the address bar's
    // folder tree, open at the current folder.
    if want("folder-tree") {
        let mut cfg = settings(mode.theme, d);
        cfg.bookmarks = crate::config::system_bookmarks();
        let mut h = app_harness(&cfg, gnome);
        let drive = |name: &str, mount: &str, total: u64, avail: u64, ejectable: bool| crate::files::drives::Drive {
            label: mount.into(),
            name: name.into(),
            system: false,
            mount: PathBuf::from(mount),
            total,
            avail,
            fs: String::new(),
            device: String::new(),
            removable: ejectable,
            ejectable,
        };
        let drives = vec![drive(&t("Computer"), "/", 512 << 30, 180 << 30, false), drive("KINGSTON", "/media/user/KINGSTON", 32 << 30, 20 << 30, true)];
        let fixed = drives.clone();
        let app = h.state_mut();
        app.drives = drives;
        app.drive_watch = crate::files::drives::Watcher::spawn_with(move || fixed.clone(), Duration::from_secs(3600), Duration::from_secs(3600), None);
        app.shot_pane(0, &d.demo.join("home").join("Documents"), None);
        settle(&mut h, 3);
        let b = h.get_all_by_role_and_label(egui::accesskit::Role::Button, &t("Folder tree")).next().unwrap().bounding_box().unwrap();
        click_at(&mut h, pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32));
        for _ in 0..40 {
            settle(&mut h, 1);
            std::thread::sleep(Duration::from_millis(20));
        }
        shot(&mut h, "folder-tree");
    }

    // The Compress dialog for a folder, saving next to the other window.
    if want("compress") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some("images"));
        app.shot_pane(1, &d.demo.join("home"), None);
        app.shot_focus(0);
        settle(&mut h, 3);
        h.state_mut().run_cmd(Cmd::Pack);
        settle(&mut h, 4);
        shot(&mut h, "compress");
    }

    if want("about") {
        let mut h = app_harness(&settings(mode.theme, d), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.demo, Some("bundle.zip"));
        app.shot_pane(1, &d.demo.join("src"), Some(".."));
        app.shot_focus(0);
        app.show_about = true;
        settle(&mut h, 3);
        center_dialog(&mut h, "about");
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
        ("quickview-geometry", d.demo.join("formats"), "snowflake.geojson"),
        ("quickview-json", d.demo.join("formats"), "api-response.json"),
        ("quickview-hdf5", d.demo.join("formats"), "weather.h5"),
        ("quickview-safetensors", d.demo.join("formats"), "tiny-llm.safetensors"),
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
        if name == "quickview-safetensors" {
            // Room for every column, and for the file names.
            cfg.show_sidebar = false;
            cfg.quick_view_frac = 0.68;
        }
        let mut h = app_harness(&cfg, gnome);
        h.state_mut().shot_pane(0, &dir, Some(file));
        settle(&mut h, 10);
        std::thread::sleep(Duration::from_millis(800));
        settle(&mut h, 10);
        if name == "quickview-hdf5" {
            // A dataset, not just the list of them.
            h.get_all_by_label("temperature_c").next().expect("its tab").click();
            settle(&mut h, 3);
        }
        if name == "quickview-json" {
            // The title of the first task, as if clicked: its path shows above the tree.
            h.state_mut().quick.as_mut().and_then(|q| q.json()).expect("a JSON tree").select(&[2, 0, 1]);
            settle(&mut h, 3);
        }
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
            du.view = crate::tools::diskusage::DuView::Table;
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
            app.spawn_job(title, After::Refresh, Some(Location::Local(d.demo.join("out"))), vec![], move |p| {
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
        let row_y = |k: f32| top + 80.0 + crate::ui::pane::ROW_H * k;
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
        cfg.bookmarks = vec![
            Bookmark::new("Home", d.home.to_string_lossy().to_string()),
            Bookmark::new("Documents", d.home.join("Documents").to_string_lossy()),
            Bookmark::new("Downloads", d.home.join("Downloads").to_string_lossy()),
            github_group(d),
        ];
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
            TerminalProfile {
                name: "Python venv".into(),
                shell: "/bin/zsh".into(),
                startup: "source .venv/bin/activate\nclear".into(),
                ..TerminalProfile::default()
            },
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
            // Two kinds ticked.
            for name in ["Spreadsheets", "Data"] {
                let i = app.cfg.quick_filters.iter().position(|q| q.name == name).unwrap();
                app.set_quick_filter(Some(i));
            }
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
        let hash = crate::tools::checksum::hash_reader(iso, crate::tools::checksum::Algo::Sha256, |_| true).unwrap();
        type_into(&mut h, egui::Rect::from_min_max(pos2(W * 0.2, 80.0), pos2(W, H * 0.4)), &hash);
        settle(&mut h, 8);
        shot(&mut h, "checksum");
    }

    if want("checksum-verify") {
        let mut h = app_harness(&downloads_cfg(), gnome);
        let app = h.state_mut();
        app.shot_pane(0, &d.downloads, Some("SHA256SUMS"));
        app.open_checksum();
        settle(&mut h, 10);
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
        // A dark terminal in the light interface too, like most people run one.
        let mut cfg = downloads_cfg();
        set_terminal_theme(&mut cfg, "Breeze Dark");
        let mut h = app_harness(&cfg, gnome);
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
/// The four windows of `docs/showcase.png`: the Breeze Light interface, with a dark terminal.
const SHOWCASE: [(&str, &str); 4] = [("light", "panels-layout"), ("light", "terminal-tabs"), ("light", "quickview-hdf5"), ("light", "quickview-json")];

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

/// Wait until the demo's git folders show their branches.
fn wait_for_git(h: &mut Harness<'static, App>, d: &Demo) {
    let ctx = h.ctx.clone();
    for name in ["docs", "papers", "src"] {
        let _ = crate::files::gitinfo::wait_for(&d.demo.join(name), false, &ctx);
    }
    settle(h, 4);
}

/// The banner's slogan and link: `docs/banner.png` is the showcase under a header with
/// the logo and the name, for sharing the project.
const SLOGAN: &str = "Files, data and a terminal, side by side.";
const REPO: &str = "github.com/quangtiencs/matrix_finder";

/// Combine four scenes into `docs/showcase.png`, and the same with a header into `docs/banner.png`.
fn compose_showcase() {
    for (name, header) in [("showcase", false), ("banner", true)] {
        let p = out_dir().join(format!("{name}.png"));
        compose(header).save(&p).unwrap();
        eprintln!("wrote {}", p.display());
    }
}

/// Four scenes as windows with rounded corners and a soft shadow on a calm gradient
/// that suits both light and dark pages; `header` adds the logo, name, slogan and link above.
fn compose(header: bool) -> RgbaImage {
    const PAD: u32 = 56;
    const GAP: u32 = 36;
    const RADIUS: f32 = 12.0;
    const HEAD: u32 = 300;
    let tiles: Vec<RgbaImage> = SHOWCASE.iter().map(|(mode, name)| image::open(out_dir().join(mode).join(format!("{name}.png"))).unwrap().to_rgba8()).collect();
    let (tw, th) = tiles[0].dimensions();
    let head = if header { HEAD } else { 0 };
    let (cw, ch) = (PAD * 2 + GAP + tw * 2, head + PAD * 2 + GAP + th * 2);
    let top_left = |i: usize| (PAD + (i as u32 % 2) * (tw + GAP), head + PAD + (i as u32 / 2) * (th + GAP));

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
    if header {
        draw_header(&mut out, PAD + HEAD / 2);
    }
    out
}

/// The logo beside the name, the slogan and the link, centred as one group at height `mid`.
fn draw_header(out: &mut RgbaImage, mid: u32) {
    use resvg::{tiny_skia, usvg};
    const LOGO: u32 = 216;
    const SPACE: u32 = 44;
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1800" height="{LOGO}">
  <text x="0" y="100" font-family="'Source Sans 3'" font-weight="600" font-size="108" fill="#ffffff">Matrix Finder</text>
  <text x="3" y="158" font-family="'Source Sans 3'" font-size="44" fill="#ffffff" fill-opacity="0.9">{}</text>
  <text x="3" y="206" font-family="'Source Sans 3'" font-size="34" fill="#d4e2ff">{}</text>
</svg>"##,
        esc(SLOGAN),
        esc(REPO)
    );
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_font_data(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/SourceSans3-Regular.otf")).to_vec());
    opt.fontdb_mut().load_font_data(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/SourceSans3-Semibold.otf")).to_vec());
    let tree = usvg::Tree::from_str(&svg, &opt).unwrap();
    let mut text = tiny_skia::Pixmap::new(1800, LOGO).unwrap();
    resvg::render(&tree, tiny_skia::Transform::default(), &mut text.as_mut());
    // The text's real width, so the group is centred.
    let text_w = (0..1800u32).rev().find(|&x| (0..LOGO).any(|y| text.pixel(x, y).unwrap().alpha() > 0)).unwrap() + 1;
    let logo = crate::style::logo::render(crate::style::logo::LOGO_SVG, LOGO).unwrap();
    let left = (out.width() - (LOGO + SPACE + text_w)) / 2;
    let top = mid - LOGO / 2;
    for y in 0..LOGO {
        for x in 0..LOGO {
            let p = &logo[((y * LOGO + x) * 4) as usize..][..4];
            let below = *out.get_pixel(left + x, top + y);
            out.put_pixel(left + x, top + y, over(below, [p[0], p[1], p[2]], p[3] as f32 / 255.0));
        }
        for x in 0..text_w {
            // Premultiplied: the colour over the background is c + below × (1 − a).
            let p = text.pixel(x, y).unwrap();
            let a = p.alpha() as f32 / 255.0;
            if a > 0.0 {
                let (ox, oy) = (left + LOGO + SPACE + x, top + y);
                let b = out.get_pixel(ox, oy).0;
                let mix = |c: u8, b: u8| (c as f32 + b as f32 * (1.0 - a)).round().min(255.0) as u8;
                out.put_pixel(ox, oy, image::Rgba([mix(p.red(), b[0]), mix(p.green(), b[1]), mix(p.blue(), b[2]), 255]));
            }
        }
    }
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
    let c = crate::ui::shortcuts::Chord::parse(text).unwrap();
    let m = Modifiers { ctrl: c.ctrl, alt: c.alt, shift: c.shift, mac_cmd: c.cmd, command: c.cmd || (c.ctrl && !cfg!(target_os = "macos")) };
    key_press(h, c.key, None, m);
}

/// Typing in a file window opens the search box with the typed text, which filters the
/// folder; Esc puts the folder back, Enter keeps the filter. Space still marks files.
#[test]
#[ignore]
fn type_to_search_in_a_file_window() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let downloads = d.home.join("Downloads");
    h.state_mut().shot_pane(0, &downloads, None);
    h.state_mut().active = 0;
    settle(&mut h, 3);
    let names = |app: &App| -> Vec<String> {
        let tab = app.panes[0].tab();
        tab.view.iter().map(|&i| tab.entries[i].name.clone()).filter(|n| n != "..").collect()
    };
    let all = names(h.state());
    assert!(all.len() > 2, "{all:?}");

    let text = |app: &App| app.type_search.as_ref().map(|s| s.text.clone());
    // Two letters typed in one frame, then more in the small search box (not the palette).
    push(&mut h, Event::Text("ph".into()));
    settle(&mut h, 2);
    assert!(!h.state().palette.open, "no command palette");
    assert_eq!(text(h.state()).as_deref(), Some("ph"));
    push(&mut h, Event::Text("otox".into()));
    settle(&mut h, 2);
    key_press(&mut h, Key::Backspace, None, Modifiers::NONE);
    assert_eq!(text(h.state()).as_deref(), Some("photo"), "Backspace edits the search, not the folder");
    assert_eq!(names(h.state()), ["photo.jpg"], "filtered while typing");
    let tab = h.state().panes[0].tab();
    assert_eq!(tab.cursor_entry().map(|e| e.name.as_str()), Some("photo.jpg"), "the cursor is on the first match");

    // Esc: closed, the whole folder again.
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert!(h.state().type_search.is_none());
    assert_eq!(names(h.state()), all);

    // Backspace over the last letter closes it too.
    push(&mut h, Event::Text("s".into()));
    settle(&mut h, 2);
    key_press(&mut h, Key::Backspace, None, Modifiers::NONE);
    assert!(h.state().type_search.is_none());
    assert_eq!(names(h.state()), all);

    // Enter keeps the filter.
    push(&mut h, Event::Text("SHA".into()));
    settle(&mut h, 2);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    assert!(h.state().type_search.is_none() && !h.state().palette.open);
    assert_eq!(h.state().panes[0].tab().filter_text, "SHA");
    assert_eq!(names(h.state()), ["SHA256SUMS"]);
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert_eq!(names(h.state()), all, "Esc in the list turns the filter off");

    // Space marks the file under the cursor instead.
    h.state_mut().panes[0].tab_mut().cursor = 1;
    push(&mut h, Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    push(&mut h, Event::Text(" ".into()));
    settle(&mut h, 2);
    assert!(!h.state().palette.open);
    assert_eq!(h.state().panes[0].tab().marked.len(), 1);

    // Text that comes with a shortcut does not start a search.
    h.state_mut().panes[0].tab_mut().marked.clear();
    h.input_mut().modifiers = Modifiers::COMMAND;
    push(&mut h, Event::Text("a".into()));
    settle(&mut h, 1);
    h.input_mut().modifiers = Modifiers::NONE;
    settle(&mut h, 1);
    assert!(!h.state().palette.open && h.state().type_search.is_none());
}

/// `source LIKE` in the search box: the files are read in the background, in this
/// folder while typing and in every subfolder with ↓ Enter; and regular expressions on names.
#[test]
#[ignore]
fn search_file_contents_like_grep() {
    let d = demo_env();
    let dir = d.demo.join("grep");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "hello\nTODO: write tests\n").unwrap();
    std::fs::write(dir.join("b.md"), "nothing to do here\n").unwrap();
    std::fs::write(dir.join("sub/c.rs"), "fn main() {} // todo later\n").unwrap();
    std::fs::write(dir.join("blob.dat"), b"\x00\x00TODO\n").unwrap();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    h.state_mut().shot_pane(0, &dir, None);
    h.state_mut().active = 0;
    settle(&mut h, 3);
    let names = |app: &App| -> Vec<String> {
        let tab = app.panes[0].tab();
        let mut v: Vec<String> = tab.view.iter().map(|&i| tab.entries[i].name.clone()).filter(|n| n != "..").collect();
        v.sort();
        v
    };
    let done = |h: &mut Harness<'static, App>| wait_until(h, 5.0, |a| !a.panes[0].tab().is_scanning() && !a.panes[0].tab().is_loading());

    // While typing: this folder only, binary files left out.
    push(&mut h, Event::Text("source LIKE '%todo%'".into()));
    settle(&mut h, 2);
    done(&mut h);
    assert_eq!(names(h.state()), ["a.txt"]);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    done(&mut h);
    assert_eq!(names(h.state()), ["a.txt"], "Enter keeps it");
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert_eq!(names(h.state()), ["a.txt", "b.md", "blob.dat", "sub"]);

    // Every subfolder, with a regular expression.
    push(&mut h, Event::Text("source ~* '^(todo|fn)'".into()));
    settle(&mut h, 2);
    key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    done(&mut h);
    assert!(h.state().panes[0].tab().search.is_some());
    assert_eq!(names(h.state()), ["a.txt", Path::new("sub").join("c.rs").to_string_lossy().as_ref()]);
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    done(&mut h);

    // Names by regular expression, while typing.
    push(&mut h, Event::Text(r"name ~ '^[ab]\.(txt|md)$'".into()));
    settle(&mut h, 3);
    assert_eq!(names(h.state()), ["a.txt", "b.md"]);
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Windows and tabs by keyboard, numbered windows, and keys a terminal keeps. The keys
/// here are bound in the settings, as the defaults differ by system.
#[test]
#[ignore]
fn keyboard_tabs_and_windows() {
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    for (id, key) in [
        ("tab_1", "Ctrl+Shift+1"),
        ("tab_2", "Ctrl+Shift+2"),
        ("last_used_tab", "Ctrl+Shift+Backslash"),
        ("window_1", "Ctrl+Shift+F1"),
        ("window_2", "Ctrl+Shift+F2"),
        ("window_3", "Ctrl+Shift+F3"),
        ("next_window", "Ctrl+Shift+CloseBracket"),
        ("previous_window", "Ctrl+Shift+OpenBracket"),
        ("next_tab", "Ctrl+Tab"),
        ("close_window", "Ctrl+W"),
    ] {
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
    assert_eq!(app.panes[app.active].tab().title(), t("Tasks"));
    assert!(h.ctx.memory(|m| m.area_rect(egui::Id::new(t("Tasks")))).is_none());

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
    h.state_mut().history.push(FinishedJob {
        title: "Copied".into(),
        ok: true,
        cancelled: false,
        msg: String::new(),
        bytes: 1,
        items: false,
        secs: 1.0,
        dest: Some(docs.clone()),
        at: "12:00".into(),
    });
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
    app.spawn_job("Copying".into(), After::Refresh, None, vec![], |p| {
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
    h.get_by_label(&t("Keep running")).click();
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
    use crate::ui::shortcuts::{self, Action, Chord};
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
#[cfg(unix)]
#[test]
#[ignore]
fn too_large_files_are_not_opened() {
    use std::os::unix::fs::PermissionsExt;
    let d = demo_env();
    let dir = d.demo.join("too-large");
    std::fs::create_dir_all(&dir).unwrap();
    let names = [
        "a.png",
        "a.pdf",
        "a.csv",
        "a.parquet",
        "a.sqlite",
        "a.h5",
        "a.xlsx",
        "a.mp4",
        "a.stl",
        "a.geojson",
        "a.heic",
        "a.nef",
        "a.epub",
        "a.txt",
        "a.bin",
        "a.zip",
        "a.tar.gz",
    ];
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

/// Named pipes and devices are never opened by Quick View (a pipe with no writer would
/// hold a preview forever, one with a writer would lose its data): an info card instead,
/// and the regular file next to them still previews.
#[cfg(unix)]
#[test]
#[ignore]
fn special_files_are_not_opened() {
    let d = demo_env();
    let dir = d.demo.join("special");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["pipe.txt", "pipe.png", "pipe.zip"] {
        let c = std::ffi::CString::new(dir.join(name).to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    }
    std::os::unix::fs::symlink("/dev/zero", dir.join("zero.txt")).unwrap();
    std::fs::write(dir.join("small.txt"), "hello").unwrap();
    let mut cfg = settings("Breeze Light", &d);
    cfg.quick_view = true;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &dir, None);
    settle(&mut h, 3);
    for (name, kind) in [("pipe.txt", "Named pipe"), ("pipe.png", "Named pipe"), ("pipe.zip", "Named pipe"), ("zero.txt", "Device")] {
        h.state_mut().panes[0].tab_mut().select_name(name);
        settle(&mut h, 2);
        let q = h.state().quick.as_ref().expect("a preview");
        assert_eq!(q.name, name);
        assert!(q.describe().starts_with(&format!("info {}", t(kind))), "{name}: {}", q.describe());
    }
    h.state_mut().panes[0].tab_mut().select_name("small.txt");
    wait_until(&mut h, 3.0, |a| a.quick.as_ref().is_some_and(|q| q.name == "small.txt" && q.is_ready()));
    assert!(h.state().quick.as_ref().unwrap().describe().starts_with("code"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A settings file that cannot be read is set aside, not overwritten by the defaults when
/// quitting, and the user is told where it is.
#[test]
#[ignore]
fn broken_settings_are_kept_and_told() {
    let _ = demo_env();
    let file = crate::config::config_file();
    let dir = file.parent().unwrap().to_path_buf();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("MATRIX_FINDER_CHROME", "gnome");
    let broken = r#"{"connections":[{"name":"srv","protocol":"Sftp","host":"h","port":22,"user":"me","password":"pw"}"#;
    std::fs::write(&file, broken).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(W, H))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| App::new(cc, crate::ui::chrome::Chrome::detect(true), None));
    settle(&mut h, 3);
    assert!(matches!(&h.state().dialog, Some(Dialog::Message { title, .. }) if *title == t("The settings file could not be read")));
    let kept: Vec<PathBuf> =
        std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().contains("settings.json.broken-")).collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), broken);
    // Saving writes a good file, and leaves the one set aside alone.
    h.state_mut().save_cfg();
    assert!(crate::config::parse_lenient(&std::fs::read_to_string(&file).unwrap()).is_ok());
    assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), broken);
}

/// A password that an older version saved: at start it moves into memory, so the saved
/// connection still opens without asking this session, and neither the settings file
/// nor its `.bak` keeps it. A password typed later is not saved either.
#[test]
#[ignore]
fn saved_passwords_move_into_memory() {
    let _ = demo_env();
    let file = crate::config::config_file();
    let dir = file.parent().unwrap().to_path_buf();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("MATRIX_FINDER_CHROME", "gnome");
    let old = r#"{"theme":"Dracula","connections":[{"name":"srv","protocol":"Ftp","host":"old.example","port":21,"user":"me","password":"hunter2-secret"}]}"#;
    std::fs::write(&file, old).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(W, H))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| App::new(cc, crate::ui::chrome::Chrome::detect(true), None));
    settle(&mut h, 2);
    let on_disk = || [file.clone(), crate::config::backup_file(&file)].map(|f| std::fs::read_to_string(f).unwrap_or_default());
    for text in on_disk() {
        assert!(text.contains("old.example") && !text.contains("hunter2"), "{text}");
    }
    let saved = h.state().cfg.connections[0].clone();
    assert_eq!((saved.name.as_str(), saved.password.as_str()), ("srv", ""));
    assert_eq!(remote::with_known_password(saved).password, "hunter2-secret");

    // A new connection saved with a password: the dialog says it is not kept, and it isn't.
    let typed = ConnectionProfile {
        name: "new".into(),
        protocol: Protocol::Ftp,
        host: "127.0.0.1".into(),
        port: 1,
        user: "me".into(),
        password: "typed-secret".into(),
        ..Default::default()
    };
    h.state_mut().show_dialog(Dialog::Connect { profile: typed.clone(), save: true }, false);
    settle(&mut h, 2);
    let _ = h.get_by_label(&format!("🔒 {}", t("Kept in memory (RAM) only and never written to disk. Forgotten when Matrix Finder quits.")));
    let _ = h.get_by_label(&t("Saved to disk: the server, user, key file and start folder. Never the password."));
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("connect-dialog.png")).unwrap();
    }
    h.state_mut().dialog = None;
    h.state_mut().start_connect(typed, true);
    settle(&mut h, 2);
    assert!(h.state().cfg.connections.iter().any(|c| c.name == "new" && c.password.is_empty()));
    for text in on_disk() {
        assert!(!text.contains("typed-secret") && !text.contains("hunter2"), "{text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Checking for updates: GitHub's answer (made up here; tests never go online) shows a
/// newer version with its notes, can be skipped, and the check at start keeps quiet about
/// a skipped version. Nothing is installed.
#[test]
#[ignore]
fn update_check_offers_a_newer_version() {
    let d = demo_env();
    let release = |v: &str| {
        format!(
            r#"{{"tag_name": "v{v}", "name": "Matrix Finder {v}", "html_url": "https://github.com/quangtiencs/matrix_finder/releases/tag/v{v}", "published_at": "2026-12-01T08:00:00Z", "body": "- Faster previews\n- Fewer bugs", "assets": [{{"name": "MatrixFinder-{v}-macos-universal.dmg", "size": 39000000, "browser_download_url": "https://github.com/quangtiencs/matrix_finder/releases/download/v{v}/MatrixFinder-{v}-macos-universal.dmg"}}]}}"#
        )
    };
    let answer = |a: crate::util::Res<String>| *crate::system::update::FAKE_ANSWER.lock().unwrap() = Some(a);
    let cfg = settings("Breeze Light", &d);
    assert!(!cfg.check_updates, "off by default");
    let mut h = app_harness(&cfg, "gnome");
    settle(&mut h, 2);
    assert!(h.state().update_check.is_none(), "nothing is checked at start");

    // Asked for: a newer version, with its notes.
    answer(Ok(release("99.1.0")));
    h.state_mut().check_for_updates(true);
    wait_until(&mut h, 5.0, |a| a.update_found.is_some());
    settle(&mut h, 3);
    assert!(h.query_by_label("Matrix Finder 99.1.0").is_some());
    assert!(h.query_by_label("Faster previews").is_some());
    assert!(h.query_by_label(&t("Open release page")).is_some());
    if let Ok(out) = std::env::var("MF_SHOT_DIR") {
        h.render().unwrap().save(format!("{out}/update.png")).unwrap();
    }
    h.get_by_label(&t("Skip this version")).click();
    settle(&mut h, 3);
    assert!(h.state().update_found.is_none());
    assert_eq!(h.state().cfg.update_skip, "v99.1.0");
    assert!(h.state().cfg.update_checked > 0);

    // At start, a skipped version is not offered again; a newer one is.
    h.state_mut().check_for_updates(false);
    wait_until(&mut h, 5.0, |a| a.update_check.is_none());
    settle(&mut h, 2);
    assert!(h.state().update_found.is_none());
    answer(Ok(release("99.2.0")));
    h.state_mut().check_for_updates(false);
    wait_until(&mut h, 5.0, |a| a.update_found.is_some());
    h.get_by_label(&t("Later")).click();
    settle(&mut h, 2);
    assert!(h.state().update_found.is_none());
    assert_eq!(h.state().cfg.update_skip, "v99.1.0", "Later skips nothing");

    // Up to date, or offline: told only when asked.
    answer(Ok(release(env!("CARGO_PKG_VERSION"))));
    h.state_mut().check_for_updates(true);
    wait_until(&mut h, 5.0, |a| a.update_check.is_none());
    assert!(h.state().toasts.iter().any(|x| x.text.starts_with(&t("Matrix Finder is up to date"))));
    assert!(!h.state().toasts.iter().any(|x| x.text == t("Checking for updates…")), "replaced by the answer");
    answer(Err("Could not reach GitHub: offline".into()));
    h.state_mut().toasts.clear();
    h.state_mut().check_for_updates(false);
    wait_until(&mut h, 5.0, |a| a.update_check.is_none());
    assert!(h.state().toasts.is_empty(), "the check at start stays quiet");
    h.state_mut().check_for_updates(true);
    wait_until(&mut h, 5.0, |a| a.update_check.is_none());
    assert!(h.state().toasts.iter().any(|x| x.error && x.text.contains("offline")));
    *crate::system::update::FAKE_ANSWER.lock().unwrap() = None;
}

/// A tab under the pointer turns clearly darker, as in a browser, and fades back.
#[test]
#[ignore]
fn hovered_tab_turns_darker() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    h.state_mut().shot_files_tab(&d.demo, None);
    assert_eq!(h.state().strip.active, 1, "the first tab is not selected");
    // The first tab, past its title.
    let tab = egui::Rect::from_min_max(pos2(280.0, 52.0), pos2(400.0, 70.0));
    // Its background: the median brightness, which the title and close button leave alone.
    let background = |img: &RgbaImage| {
        let mut px: Vec<f32> = img
            .enumerate_pixels()
            .filter(|(x, y, _)| tab.contains(pos2(*x as f32, *y as f32)))
            .map(|(_, _, p)| (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0)
            .collect();
        px.sort_by(f32::total_cmp);
        px[px.len() / 2]
    };
    move_to(&mut h, pos2(W - 5.0, H - 5.0));
    settle(&mut h, 6);
    let away = background(&h.render().unwrap());
    move_to(&mut h, tab.center());
    settle(&mut h, 8);
    let over = background(&h.render().unwrap());
    assert!(away - over >= 12.0, "darker under the pointer: {away:.1} → {over:.1}");
    move_to(&mut h, pos2(W - 5.0, H - 5.0));
    settle(&mut h, 8);
    let back = background(&h.render().unwrap());
    assert!((back - away).abs() < 1.0, "back to the bar: {away:.1} → {back:.1}");
}

/// A long file name is cut to fit the title row of Quick View: it never runs under the
/// buttons at the end of the row.
#[test]
#[ignore]
fn long_names_leave_room_for_the_buttons() {
    let d = demo_env();
    let dir = d.demo.join("long-names");
    std::fs::create_dir_all(&dir).unwrap();
    let long = "Báo cáo tổng hợp số liệu khách hàng theo từng tỉnh thành và từng quý của năm tài chính 2026 - bản cuối cùng đã duyệt";
    let files: [(&str, Vec<u8>, &str); 5] = [
        ("json", br#"{"a": [1, 2, {"b": null}]}"#.to_vec(), "Expand all"),
        ("csv", b"a,b\n1,2\n".to_vec(), "Source"),
        ("rs", b"fn main() {}\n".to_vec(), "Wrap"),
        ("png", photo(64, 48, 3, image::ImageFormat::Png), "Fit"),
        ("stl", b"solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n".to_vec(), "Reset view"),
    ];
    for (ext, bytes, _) in &files {
        std::fs::write(dir.join(format!("{long}.{ext}")), bytes).unwrap();
    }
    for frac in [0.0, 0.6] {
        let mut cfg = settings("Breeze Light", &d);
        cfg.quick_view = true;
        if frac > 0.0 {
            cfg.quick_view_frac = frac;
        }
        let mut h = app_harness(&cfg, "gnome");
        for (ext, _, button) in &files {
            let name = format!("{long}.{ext}");
            h.state_mut().shot_pane(0, &dir, Some(&name));
            wait_until(&mut h, 5.0, |a| a.quick.as_ref().is_some_and(|q| q.name == name && q.is_ready()));
            settle(&mut h, 3);
            let button = h.get_by_label(&t(button)).bounding_box().unwrap();
            // The title, not the file's row in the list or the status bar.
            let title = h
                .get_all_by_label(&name)
                .filter_map(|n| n.bounding_box())
                .find(|r| r.y1 < 150.0 && r.x0 > f64::from(W) * 0.35)
                .unwrap_or_else(|| panic!("no title for {ext}"));
            let beside = title.y0 < button.y1 && title.y1 > button.y0;
            assert!(!beside || title.x1 <= button.x0, "{ext}: the name ({:.0}…{:.0}) runs under {:.0}", title.x0, title.x1, button.x0);
            assert!(title.x1 - title.x0 > 100.0, "{ext}: some of the name shows ({:.0}…{:.0})", title.x0, title.x1);
            if let Ok(out) = std::env::var("MF_SHOT_DIR") {
                h.render().unwrap().save(format!("{out}/long-{ext}-{frac}.png")).unwrap();
            }
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// JSON in Quick View: a tree whose rows give their path, in the style chosen.
#[test]
#[ignore]
fn json_tree_copies_paths() {
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    cfg.quick_view = true;
    cfg.quick_view_frac = 0.6;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &d.demo.join("formats"), Some("api-response.json"));
    wait_until(&mut h, 5.0, |a| a.quick.as_ref().is_some_and(|q| q.name == "api-response.json" && q.is_ready()));
    let status = h.state().quick.as_ref().unwrap().describe();
    assert!(status.starts_with("json values=41 depth=4"), "{status}");
    settle(&mut h, 3);
    let copied = |h: &mut Harness<'static, App>| {
        h.get_by_label(&t("Copy path")).click();
        h.step();
        h.output().platform_output.commands.iter().find_map(|c| match c {
            egui::OutputCommand::CopyText(s) => Some(s.clone()),
            _ => None,
        })
    };
    h.state_mut().quick.as_mut().and_then(|q| q.json()).unwrap().select(&[2, 0, 1]);
    settle(&mut h, 2);
    assert_eq!(copied(&mut h).as_deref(), Some("j['data'][0]['title']"));
    // Another style, kept for the next file.
    h.get_by_label("jq").click();
    settle(&mut h, 2);
    assert_eq!(copied(&mut h).as_deref(), Some(".data[0].title"));
    h.get_by_label("JSONPath").click();
    settle(&mut h, 2);
    assert_eq!(copied(&mut h).as_deref(), Some("$.data[0].title"));
    h.get_by_label("Python").click();
    settle(&mut h, 2);
    // Folders open and close; the source is there too.
    h.get_by_label(&t("Collapse all")).click();
    settle(&mut h, 2);
    assert!(h.state().quick.as_ref().unwrap().describe().ends_with("rows=6"), "{}", h.state().quick.as_ref().unwrap().describe());
    h.get_by_label(&t("Source")).click();
    settle(&mut h, 2);
    assert!(h.query_by_label(&t("Copy path")).is_none());
    // Not JSON after all: shown as text.
    std::fs::write(d.demo.join("formats/broken.json"), "{\"a\": 1, // a comment\n}").unwrap();
    h.state_mut().shot_pane(0, &d.demo.join("formats"), Some("broken.json"));
    wait_until(&mut h, 5.0, |a| a.quick.as_ref().is_some_and(|q| q.name == "broken.json" && q.is_ready()));
    assert!(h.state().quick.as_ref().unwrap().describe().starts_with("code"));
}

/// GeoJSON in Quick View: the shapes are drawn, moved and zoomed, and the source shown.
#[test]
#[ignore]
fn geometry_preview_moves_and_shows_its_source() {
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    cfg.quick_view = true;
    cfg.quick_view_frac = 0.6;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &d.demo.join("formats"), Some("snowflake.geojson"));
    wait_until(&mut h, 5.0, |a| a.quick.as_ref().is_some_and(|q| q.name == "snowflake.geojson" && q.is_ready()));
    let status = h.state().quick.as_ref().unwrap().describe();
    assert!(status.starts_with(r#"geo GeoJSON [("Polygon", 1)] features=Some(1) vertices=780"#), "{status}");
    settle(&mut h, 3);
    let before = h.render().unwrap();
    let middle = pos2(W * 0.7, H * 0.55);
    drag(&mut h, middle, middle + egui::vec2(120.0, 40.0));
    move_to(&mut h, middle);
    push(&mut h, Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, 240.0), modifiers: Modifiers::NONE });
    settle(&mut h, 4);
    let after = h.render().unwrap();
    assert!(before.as_raw() != after.as_raw(), "dragging and zooming changed the view");
    h.get_by_label(&t("Reset view")).click();
    settle(&mut h, 4);
    // The source, then back.
    h.get_by_label(&t("Source")).click();
    settle(&mut h, 3);
    assert!(h.query_by_label(&t("Reset view")).is_none());
    h.get_by_label(&t("Source")).click();
    settle(&mut h, 3);
    assert!(h.query_by_label(&t("Reset view")).is_some());
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
    let mut names: Vec<String> =
        std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
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

/// Typing in the address bar suggests the folders that match: arrows pick one, Enter
/// opens it, Tab completes it, a click opens it, Esc puts the address back.
#[test]
#[ignore]
fn address_bar_suggestions() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.home, None);
    h.state_mut().active = 0;
    settle(&mut h, 3);
    // HOME is the demo home, so "~" works as typed.
    let home = "~";
    let type_address = |h: &mut Harness<'static, App>, text: &str| {
        h.state_mut().panes[0].tab_mut().editing_path = true;
        h.state_mut().panes[0].tab_mut().focus_path = true;
        settle(h, 2);
        key_press(h, Key::A, None, Modifiers::COMMAND);
        push(h, Event::Text(text.into()));
        settle(h, 2);
        for _ in 0..50 {
            if !h.state().panes[0].tab().complete.items.is_empty() {
                break;
            }
            settle(h, 1);
        }
    };
    let names = |h: &Harness<'static, App>| -> Vec<String> { h.state().panes[0].tab().complete.items.iter().map(|s| s.name.clone()).collect() };

    type_address(&mut h, &format!("{home}/Do"));
    assert_eq!(names(&h), ["Documents", "Downloads"]);
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        settle(&mut h, 2);
        h.render().unwrap().save(Path::new(&out).join("address-suggestions.png")).unwrap();
    }
    key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
    key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
    assert_eq!(h.state().panes[0].tab().complete.sel, Some(1));
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 3);
    assert!(!h.state().panes[0].tab().editing_path);
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(d.home.join("Downloads")));

    // Tab completes the only match and goes on with the folders inside it.
    type_address(&mut h, &format!("{home}/Pro"));
    assert_eq!(names(&h), ["Projects"]);
    key_press(&mut h, Key::Tab, None, Modifiers::NONE);
    assert_eq!(h.state().panes[0].tab().path_text, format!("{home}/Projects/"));
    assert!(h.state().panes[0].tab().editing_path, "Tab keeps the keyboard in the address bar");
    for _ in 0..50 {
        if names(&h) == ["matrix_finder"] {
            break;
        }
        settle(&mut h, 1);
    }
    assert_eq!(names(&h), ["matrix_finder"]);
    // A click on a suggestion opens it.
    let row = h.state().panes[0].tab().complete.rows[0];
    move_to(&mut h, row.center());
    button(&mut h, row.center(), PointerButton::Primary, true, Modifiers::NONE);
    button(&mut h, row.center(), PointerButton::Primary, false, Modifiers::NONE);
    settle(&mut h, 3);
    assert!(!h.state().panes[0].tab().editing_path);
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(d.home.join("Projects/matrix_finder")));

    // Esc: no list, the address is put back.
    let before = h.state().panes[0].tab().path_text.clone();
    type_address(&mut h, &format!("{home}/D"));
    assert!(!names(&h).is_empty());
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert!(!h.state().panes[0].tab().editing_path);
    assert!(h.state().panes[0].tab().complete.items.is_empty());
    assert_eq!(h.state().panes[0].tab().path_text, before);
}

/// Mount a small disk image (macOS) and make it the only other drive in the sidebar.
fn mount_test_image(dir: &Path, name: &str) -> PathBuf {
    let img = dir.join(format!("{name}.dmg"));
    if !img.exists() {
        let ok = std::process::Command::new("hdiutil").args(["create", "-size", "4m", "-fs", "HFS+", "-volname", name]).arg(&img).output().unwrap();
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    }
    let ok = std::process::Command::new("hdiutil").arg("attach").arg(&img).output().unwrap();
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    let mount = PathBuf::from("/Volumes").join(name);
    std::fs::create_dir_all(mount.join("photos")).unwrap();
    mount
}

fn wait_for(h: &mut Harness<'static, App>, what: &str, done: impl Fn(&App) -> bool) {
    for _ in 0..300 {
        if done(h.state()) {
            return;
        }
        settle(h, 1);
    }
    panic!("timed out: {what}");
}

/// The eject button of the sidebar: refused while a transfer uses the drive, explains
/// which program holds it, and ejects it, sending the windows on it home. A drive
/// unmounted elsewhere leaves the sidebar by itself. macOS only: it mounts a disk image.
#[test]
#[ignore]
fn eject_drives_from_the_sidebar() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let name = format!("MFDrive{}", std::process::id());
    let mount = PathBuf::from("/Volumes").join(&name);
    let lister = {
        let mount = mount.clone();
        move || -> Vec<crate::files::drives::Drive> {
            crate::files::drives::list()
                .into_iter()
                .filter(|x| x.mount == Path::new("/") || x.mount == mount)
                .map(|mut x| {
                    if x.mount == Path::new("/") {
                        x.label = "/".into();
                    }
                    x
                })
                .collect()
        }
    };
    h.state_mut().drive_watch = crate::files::drives::Watcher::spawn_with(lister, Duration::from_millis(200), Duration::from_secs(3600), None);
    let images = crate::util::TempDir::new().unwrap();
    assert_eq!(mount_test_image(images.path(), &name), mount);
    let home = d.home.clone();
    let has = |app: &App, m: &Path| app.drives.iter().any(|x| x.mount == m);
    wait_for(&mut h, "the drive shows up", |app| has(app, &mount));
    h.state_mut().shot_pane(0, &mount.join("photos"), None);
    h.state_mut().active = 0;
    settle(&mut h, 3);
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        let row = h.state().drive_rows.iter().find_map(|r| r.2).unwrap();
        move_to(&mut h, row.center());
        h.render().unwrap().save(Path::new(&out).join("eject-button.png")).unwrap();
    }
    let click_eject = |h: &mut Harness<'static, App>| {
        let r = h.state().drive_rows.iter().find(|(m, _, _)| *m == mount).and_then(|r| r.2).expect("an eject button");
        move_to(h, r.center());
        button(h, r.center(), PointerButton::Primary, true, Modifiers::NONE);
        button(h, r.center(), PointerButton::Primary, false, Modifiers::NONE);
    };
    let last_toast = |app: &App| app.toasts.last().map(|t| (t.text.clone(), t.error));

    // 1. A transfer that uses the drive: refused, nothing is run.
    let app = h.state_mut();
    app.spawn_job("Copying".into(), After::Refresh, None, vec![mount.join("photos")], |p| {
        while !p.cancelled() {
            std::thread::sleep(Duration::from_millis(20));
        }
        Err("Cancelled".into())
    });
    app.jobs[0].job.progress.release();
    click_eject(&mut h);
    let (text, error) = last_toast(h.state()).unwrap();
    assert!(error && text.contains(&t("A task is using this drive. Wait for it to finish, or cancel it.")), "{text}");
    assert!(h.state().ejecting.is_empty() && has(h.state(), &mount));
    for j in &h.state().jobs {
        j.job.progress.cancel();
    }
    wait_for(&mut h, "the transfer stops", |app| app.jobs.is_empty());

    // 2. Another program has a folder open there: it is named, and the drive stays.
    let mut busy = std::process::Command::new("sleep").arg("60").current_dir(&mount).spawn().unwrap();
    click_eject(&mut h);
    assert_eq!(h.state().ejecting.len(), 1, "ejecting in the background");
    wait_for(&mut h, "the eject fails", |app| app.ejecting.is_empty());
    let (text, error) = last_toast(h.state()).unwrap();
    assert!(error && text.contains("sleep"), "{text}");
    assert!(has(h.state(), &mount));
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(mount.join("photos")), "the window stays on the drive");
    busy.kill().unwrap();
    busy.wait().unwrap();

    // 3. Ejected: gone from the sidebar and the system, the window goes home.
    click_eject(&mut h);
    wait_for(&mut h, "the eject finishes", |app| app.ejecting.is_empty());
    let (text, error) = last_toast(h.state()).unwrap();
    assert!(!error && text.contains(&name), "{text}");
    assert!(!has(h.state(), &mount));
    assert!(!mount.exists());
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(home.clone()));
    settle(&mut h, 15);
    assert!(!has(h.state(), &mount), "the watcher agrees");

    // 4. Unmounted elsewhere: the sidebar follows, and the window on it goes home.
    mount_test_image(images.path(), &name);
    wait_for(&mut h, "the drive is back", |app| has(app, &mount));
    h.state_mut().shot_pane(0, &mount.join("photos"), None);
    settle(&mut h, 3);
    let ok = std::process::Command::new("hdiutil").arg("detach").arg(&mount).output().unwrap();
    assert!(ok.status.success());
    wait_for(&mut h, "the drive leaves the sidebar", |app| !has(app, &mount));
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(home));
}

/// The Drives section of the sidebar: by default only with more than one drive (the
/// system's own mounts do not count), always or never by the setting, and each drive
/// shown by its name, not its path.
#[test]
#[ignore]
fn drives_section_follows_the_setting() {
    use egui::accesskit::Role;
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let root = crate::files::drives::list().into_iter().find(|x| x.mount == Path::new("/")).unwrap();
    let mut data = root.clone();
    data.mount = PathBuf::from("/System/Volumes/Data");
    data.system = true;
    let usb = crate::files::drives::Drive {
        label: "/Volumes/KINGSTON (KINGSTON)".into(),
        name: "KINGSTON".into(),
        system: false,
        mount: PathBuf::from("/Volumes/KINGSTON"),
        total: 32 << 30,
        avail: 20 << 30,
        fs: "msdos".into(),
        device: "KINGSTON".into(),
        removable: true,
        ejectable: true,
    };
    let now = std::sync::Arc::new(std::sync::Mutex::new(vec![root.clone(), data.clone()]));
    let n2 = now.clone();
    h.state_mut().drive_watch =
        crate::files::drives::Watcher::spawn_with(move || n2.lock().unwrap().clone(), Duration::from_millis(50), Duration::from_secs(3600), None);
    let rows = |h: &Harness<'static, App>| h.state().drive_rows.iter().map(|r| r.1.clone()).collect::<Vec<_>>();
    let wait_rows = |h: &mut Harness<'static, App>, want: &[&str]| {
        for _ in 0..100 {
            if rows(h) == want {
                return;
            }
            settle(h, 1);
        }
        assert_eq!(rows(h), want);
    };
    let root_name = root.name.clone();
    assert!(!root_name.is_empty());

    // One drive (the Data volume is the system's): no Drives section.
    assert_eq!(h.state().cfg.sidebar_drives, crate::config::SidebarDrives::WhenSeveral);
    wait_rows(&mut h, &[]);
    // A USB drive is plugged in: both, by name.
    now.lock().unwrap().push(usb.clone());
    wait_rows(&mut h, &[&root_name, "KINGSTON"]);
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("drives-by-name.png")).unwrap();
    }
    // Never, picked in Settings → Files & layout.
    h.state_mut().open_settings(SettingsPage::Files);
    settle(&mut h, 4);
    scroll_settings(&mut h, 600.0);
    h.get_by_role_and_label(Role::RadioButton, &t("Never")).click();
    settle(&mut h, 3);
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("drives-setting.png")).unwrap();
    }
    assert_eq!(h.state().cfg.sidebar_drives, crate::config::SidebarDrives::Never);
    assert_eq!(Settings::load().sidebar_drives, crate::config::SidebarDrives::Never, "saved");
    wait_rows(&mut h, &[]);
    // Always: also with one drive.
    h.get_by_role_and_label(Role::RadioButton, &t("Always")).click();
    settle(&mut h, 3);
    now.lock().unwrap().pop();
    wait_rows(&mut h, &[&root_name]);
    // Back to the default: hidden again with one drive.
    h.get_by_role_and_label(Role::RadioButton, &t("Only when there is more than one drive")).click();
    settle(&mut h, 3);
    wait_rows(&mut h, &[]);
}

/// F3 shows and hides Quick View, Shift+F3 opens the viewer window, and F4 opens a
/// folder (or "..", or a folder inside an archive) in the same window instead of
/// handing it to the system's file manager.
#[test]
#[ignore]
fn f3_quick_view_and_f4_opens_folders_here() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.demo, None);
    h.state_mut().active = 0;
    settle(&mut h, 3);
    let put_cursor = |h: &mut Harness<'static, App>, name: &str| {
        let tab = h.state_mut().panes[0].tab_mut();
        let at = tab.view.iter().position(|&i| tab.entries[i].name == name).unwrap_or_else(|| panic!("no {name}"));
        tab.cursor = at;
        settle(h, 1);
    };
    let loc = |h: &Harness<'static, App>| h.state().panes[0].tab().loc.clone();

    // F3: Quick View on, and off again.
    let before = h.state().cfg.quick_view;
    put_cursor(&mut h, "notes.txt");
    key_press(&mut h, Key::F3, None, Modifiers::NONE);
    assert_eq!(h.state().cfg.quick_view, !before);
    key_press(&mut h, Key::F3, None, Modifiers::NONE);
    assert_eq!(h.state().cfg.quick_view, before);
    assert!(h.state().viewer.is_none(), "F3 alone opens no viewer window");
    // Shift+F3: the viewer window.
    key_press(&mut h, Key::F3, None, Modifiers::SHIFT);
    assert_eq!(h.state().viewer.as_ref().map(|v| v.name.clone()).as_deref(), Some("notes.txt"));
    h.state_mut().viewer = None;
    settle(&mut h, 1);

    // F4 on a folder: opened here.
    put_cursor(&mut h, "docs");
    key_press(&mut h, Key::F4, None, Modifiers::NONE);
    settle(&mut h, 3);
    assert_eq!(loc(&h), Location::Local(d.demo.join("docs")));
    // F4 on "..": back up.
    put_cursor(&mut h, "..");
    key_press(&mut h, Key::F4, None, Modifiers::NONE);
    settle(&mut h, 3);
    assert_eq!(loc(&h), Location::Local(d.demo.clone()));
    // F4 on a folder inside an archive: opened here too.
    h.state_mut().shot_pane(0, &d.demo, None);
    settle(&mut h, 2);
    let zip = d.demo.join("bundle.zip");
    h.state_mut().navigate_active(Location::Archive { file: zip.clone(), inner: String::new() });
    settle(&mut h, 5);
    put_cursor(&mut h, "images");
    key_press(&mut h, Key::F4, None, Modifiers::NONE);
    settle(&mut h, 5);
    assert_eq!(loc(&h), Location::Archive { file: zip, inner: "images".into() });
}

/// The sidebar button sits just before the "Matrix Finder" menu: at the top of the
/// sidebar while it is shown, at the left of the top bar while it is hidden. The
/// menu's View submenu turns the sidebar on and off too.
#[test]
#[ignore]
fn sidebar_button_next_to_the_app_menu() {
    use egui::accesskit::Role;
    let d = demo_env();
    for (theme, chrome) in [("Adwaita Light", "gnome"), ("macOS Light", "mac"), ("Windows 11 Light", "windows")] {
        let mut h = app_harness(&settings(theme, &d), chrome);
        let rect = |h: &Harness<'static, App>, label: &str| {
            let r = h.get_by_role_and_label(Role::Button, label).bounding_box().unwrap_or_else(|| panic!("{chrome}: no box for {label:?}"));
            egui::Rect::from_min_max(pos2(r.x0 as f32, r.y0 as f32), pos2(r.x1 as f32, r.y1 as f32))
        };
        let brand = |h: &Harness<'static, App>| {
            let r = h.get_by_label_contains("Matrix Finder").bounding_box().unwrap();
            egui::Rect::from_min_max(pos2(r.x0 as f32, r.y0 as f32), pos2(r.x1 as f32, r.y1 as f32))
        };
        let next_to = |b: egui::Rect, s: egui::Rect, what: &str| {
            assert!(s.right() <= b.left() + 1.0 && b.left() - s.right() <= 10.0, "{chrome} {what}: button at {s:?}, menu at {b:?}");
            assert!((s.center().y - b.center().y).abs() <= 2.0, "{chrome} {what}: same row ({s:?} / {b:?})");
        };

        // Shown: in the sidebar, after the menu.
        assert!(h.state().cfg.show_sidebar);
        let (b, s) = (brand(&h), rect(&h, &t("Hide sidebar")));
        next_to(b, s, "shown");
        assert!(s.right() <= h.state().cfg.sidebar_width + 1.0, "{chrome}: inside the sidebar ({s:?})");
        assert!(h.query_by_role_and_label(Role::Button, &t("Show sidebar")).is_none());
        h.get_by_role_and_label(Role::Button, &t("Hide sidebar")).click();
        settle(&mut h, 3);
        assert!(!h.state().cfg.show_sidebar, "{chrome}: hidden");
        assert!(!Settings::load().show_sidebar, "{chrome}: saved");
        if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
            h.render().unwrap().save(Path::new(&out).join(format!("sidebar-hidden-{chrome}.png"))).unwrap();
        }

        // Hidden: in the top bar, after the menu, and it brings the sidebar back.
        let (b, s) = (brand(&h), rect(&h, &t("Show sidebar")));
        next_to(b, s, "hidden");
        assert!(h.query_by_role_and_label(Role::Button, &t("Hide sidebar")).is_none());
        h.get_by_role_and_label(Role::Button, &t("Show sidebar")).click();
        settle(&mut h, 3);
        assert!(h.state().cfg.show_sidebar, "{chrome}: shown again");
        assert!(Settings::load().show_sidebar, "{chrome}: saved");
        if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
            h.render().unwrap().save(Path::new(&out).join(format!("sidebar-shown-{chrome}.png"))).unwrap();
        }
        // Only one such button: none left at the right end of the top bar.
        assert_eq!(h.query_all_by_label_contains(&t("Sidebar")).count() + h.query_all_by_label_contains("sidebar").count(), 1, "{chrome}");

        // The app menu: View → Sidebar, checked while shown; it hides and shows the sidebar.
        for want in [false, true] {
            h.get_by_label_contains("Matrix Finder").click();
            settle(&mut h, 2);
            // Submenus open on hover.
            h.get_by_role_and_label(Role::Button, &t("View")).hover();
            settle(&mut h, 3);
            let item = h.get_by(|n| n.role() == Role::CheckBox && n.label().is_some_and(|l| l.starts_with(&format!("{}\t", t("Sidebar")))));
            assert_eq!(item.toggled() == Some(egui::accesskit::Toggled::True), !want, "{chrome}: the item shows the state");
            if !want {
                if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
                    h.render().unwrap().save(Path::new(&out).join(format!("sidebar-menu-{chrome}.png"))).unwrap();
                }
            }
            h.get_by(|n| n.role() == Role::CheckBox && n.label().is_some_and(|l| l.starts_with(&format!("{}\t", t("Sidebar"))))).click();
            settle(&mut h, 3);
            assert_eq!(h.state().cfg.show_sidebar, want, "{chrome}: from the menu");
            assert_eq!(Settings::load().show_sidebar, want, "{chrome}: saved");
            assert!(
                h.query_by(|n| n.role() == Role::CheckBox && n.label().is_some_and(|l| l.starts_with(&format!("{}\t", t("Sidebar"))))).is_none(),
                "{chrome}: the menu closes"
            );
        }
    }
}

/// A press and release of the left button at `p`.
fn click_at(h: &mut Harness<'static, App>, p: Pos2) {
    move_to(h, p);
    button(h, p, PointerButton::Primary, true, Modifiers::NONE);
    button(h, p, PointerButton::Primary, false, Modifiers::NONE);
    settle(h, 2);
}

/// Checksum and Batch rename open in a tab of their own (no floating window), do their
/// work there, and leave with their tab.
#[test]
#[ignore]
fn checksum_and_batch_rename_open_in_tabs() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let dir = d.demo.join("out").join("tool-tabs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Báo cáo A.TXT"), "alpha").unwrap();
    std::fs::write(dir.join("Báo cáo B.TXT"), "beta").unwrap();
    h.state_mut().shot_pane(0, &dir, Some("Báo cáo A.TXT"));
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let tabs = h.state().strip.tabs.len();
    let files_tab = h.state().live;
    // A button is in sight: above the bottom of the window's card (the status bar is below).
    let in_sight = |h: &Harness<'static, App>, label: &str| {
        let b = h.get_by_role_and_label(egui::accesskit::Role::Button, label).bounding_box().unwrap();
        assert!(b.y1 as f32 <= H - 36.0, "{label:?} is cut off at the bottom ({b:?})");
    };
    let names = || {
        let mut v: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    };

    // Checksum: a new tab with the hash in it.
    h.state_mut().run_cmd(Cmd::Checksum);
    settle(&mut h, 3);
    assert_eq!(h.state().strip.tabs.len(), tabs + 1, "a tab of its own");
    assert_ne!(h.state().live, files_tab, "and it is shown");
    assert_eq!(h.state().panes.len(), 1);
    assert!(h.state().panes[0].tab().checksum.is_some());
    assert!(!h.state().panes[0].tab().is_files());
    assert_eq!(h.state().panes[0].tab().title(), t("Checksum"));
    assert!(h.ctx.memory(|m| m.area_rect(egui::Id::new("checksum-window"))).is_none(), "no floating window");
    let want = crate::tools::checksum::hash_reader(&b"alpha"[..], crate::tools::checksum::Algo::Sha256, |_| true).unwrap();
    for _ in 0..100 {
        if h.query_by_label(&want).is_some() {
            break;
        }
        settle(&mut h, 1);
    }
    assert!(h.query_by_label(&want).is_some(), "the hash is shown in the tab");
    in_sight(&h, &t("Save checksum file…"));
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("checksum-tab.png")).unwrap();
    }
    let checksum_tab = h.state().live;
    h.state_mut().request_close_tabs(&[checksum_tab]);
    settle(&mut h, 3);
    assert_eq!(h.state().strip.tabs.len(), tabs, "closed with its tab");
    assert_eq!(h.state().live, files_tab);

    // Batch rename: a new tab; renaming there changes the folder, undo puts it back.
    h.state_mut().shot_mark(0, &["Báo cáo A.TXT", "Báo cáo B.TXT"]);
    h.state_mut().run_cmd(Cmd::BatchRename);
    settle(&mut h, 3);
    assert_eq!(h.state().strip.tabs.len(), tabs + 1);
    assert!(h.state().panes[0].tab().renamer.is_some());
    assert_eq!(h.state().panes[0].tab().title(), t("Batch rename"));
    h.get_by_label("Web-safe (slug)").click();
    settle(&mut h, 3);
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("rename-tab.png")).unwrap();
    }
    in_sight(&h, &t("Rename"));
    h.get_by_role_and_label(egui::accesskit::Role::Button, &t("Rename")).click();
    settle(&mut h, 3);
    assert_eq!(names(), ["bao-cao-a.txt", "bao-cao-b.txt"]);
    in_sight(&h, &t("Undo last rename"));
    h.get_by_label(&t("Undo last rename")).click();
    settle(&mut h, 3);
    assert_eq!(names(), ["Báo cáo A.TXT", "Báo cáo B.TXT"]);
    let rename_tab = h.state().live;
    h.state_mut().request_close_tabs(&[rename_tab]);
    settle(&mut h, 3);
    assert_eq!(h.state().strip.tabs.len(), tabs);
    assert_eq!(h.state().live, files_tab);
}

/// The Copy dialog: the "To" field suggests folders while a path is typed (archives
/// cannot take files, so they are left out), and its folder tree (with OK and Cancel)
/// picks one.
#[test]
#[ignore]
fn copy_dialog_suggests_folders_and_has_a_tree() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let dir = d.demo.join("out").join("copy-to");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("alpha")).unwrap();
    std::fs::create_dir_all(dir.join("beta").join("inner")).unwrap();
    std::fs::write(dir.join("src.txt"), "copy me").unwrap();
    std::fs::copy(d.demo.join("bundle.zip"), dir.join("bundle.zip")).unwrap();
    let docs = d.home.join("Documents");
    let _ = std::fs::remove_file(docs.join("src.txt"));
    h.state_mut().shot_pane(0, &dir, Some("src.txt"));
    h.state_mut().shot_pane(1, &d.home, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let dst = |h: &Harness<'static, App>| match &h.state().dialog {
        Some(Dialog::Transfer { dst, .. }) => Some(dst.clone()),
        _ => None,
    };
    let names = |h: &Harness<'static, App>| -> Vec<String> { h.state().dialog_path.complete.items.iter().map(|s| s.name.clone()).collect() };
    let wait_names = |h: &mut Harness<'static, App>, want: &[&str]| {
        for _ in 0..60 {
            if names(h) == want {
                return;
            }
            settle(h, 1);
        }
        assert_eq!(names(h), want);
    };

    // The other window is chosen; "Another folder" shows the address, with it filled in.
    let other_folder = |h: &mut Harness<'static, App>| {
        h.get_by_label(&t("Another folder")).click();
        settle(h, 3);
    };
    // Typing a path: its folders, not the zip.
    key_press(&mut h, Key::F5, None, Modifiers::NONE);
    settle(&mut h, 2);
    other_folder(&mut h);
    assert_eq!(dst(&h), Some(d.home.display().to_string()));
    key_press(&mut h, Key::A, None, Modifiers::COMMAND);
    push(&mut h, Event::Text(format!("{}/", dir.display())));
    settle(&mut h, 2);
    wait_names(&mut h, &["alpha", "beta"]);
    settle(&mut h, 2);
    assert_eq!(h.state().dialog_path.complete.rows.len(), 2, "the list is shown");
    let width = |h: &Harness<'static, App>| h.ctx.memory(|m| m.area_rect(egui::Id::new("dialog"))).map(|r| r.width());
    let w0 = width(&h);
    settle(&mut h, 10);
    assert_eq!(width(&h), w0, "the dialog keeps its width");
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("copy-suggestions.png")).unwrap();
    }
    // ↓ ↓ Enter takes "beta" and goes on with the folders in it; the dialog stays.
    key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
    key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
    assert_eq!(h.state().dialog_path.complete.sel, Some(1));
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert_eq!(dst(&h), Some(format!("{}/beta/", dir.display())), "Enter on a suggestion fills it in");
    wait_names(&mut h, &["inner"]);
    // A click on a suggestion fills it in too.
    let row = h.state().dialog_path.complete.rows[0];
    click_at(&mut h, row.center());
    assert_eq!(dst(&h), Some(format!("{}/beta/inner/", dir.display())));
    // Esc closes the list only.
    wait_names(&mut h, &[]);
    key_press(&mut h, Key::Backspace, None, Modifiers::NONE);
    key_press(&mut h, Key::Backspace, None, Modifiers::NONE);
    settle(&mut h, 2);
    wait_names(&mut h, &["inner"]);
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(names(&h).is_empty(), "Esc closes the list");
    assert!(dst(&h).is_some(), "and leaves the dialog open");
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("dialog-dst")), "typing goes on in the field");
    // Enter with no suggestion chosen copies.
    key_press(&mut h, Key::A, None, Modifiers::COMMAND);
    push(&mut h, Event::Text(format!("{}/beta", dir.display())));
    settle(&mut h, 2);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(dst(&h).is_none(), "copying");
    wait_until(&mut h, 5.0, |_| dir.join("beta").join("src.txt").exists());
    assert!(dir.join("beta").join("src.txt").exists());

    // The folder tree: opens at the folder in the field. A click chooses a folder, OK
    // takes it; Cancel and Esc leave the field as it was.
    let tree = dialog_tree;
    key_press(&mut h, Key::F5, None, Modifiers::NONE);
    settle(&mut h, 2);
    other_folder(&mut h);
    let r = open_dialog_tree(&mut h, &docs);
    assert_eq!(tree(&h).unwrap().1, Some(d.home.clone()), "the folder in the field is chosen");
    assert_eq!(h.state().dialog_path.tree_of(egui::Id::new("dialog-dst")).unwrap().current(), Some(d.home.as_path()));
    // Small: well inside the window.
    let (ok, cancel) = tree(&h).unwrap().2.expect("OK and Cancel");
    assert!(ok.bottom() < H - 40.0 && cancel.right() < ok.left(), "Cancel, then OK: {cancel:?} {ok:?}");
    click_at(&mut h, r.center());
    assert_eq!(tree(&h).map(|t| t.1), Some(Some(docs.clone())), "a click chooses a folder and leaves the tree open");
    assert_eq!(dst(&h), Some(d.home.display().to_string()), "the field is not changed yet");
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("copy-tree.png")).unwrap();
    }
    // Cancel: nothing changes.
    click_at(&mut h, cancel.center());
    assert!(tree(&h).is_none(), "Cancel closes the tree");
    assert_eq!(dst(&h), Some(d.home.display().to_string()));
    assert!(dst(&h).is_some(), "the dialog stays");
    // Esc: the same.
    open_dialog_tree(&mut h, &docs);
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(tree(&h).is_none(), "Esc closes the tree");
    assert_eq!(dst(&h), Some(d.home.display().to_string()), "and only the tree");
    // ↓ chooses the next folder, Enter takes it (and does not copy yet).
    open_dialog_tree(&mut h, &docs);
    let rows: Vec<PathBuf> = tree(&h).unwrap().0.into_iter().map(|r| r.0).collect();
    let below = rows[rows.iter().position(|p| *p == d.home).unwrap() + 1].clone();
    key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
    settle(&mut h, 1);
    assert_eq!(tree(&h).unwrap().1, Some(below.clone()));
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(tree(&h).is_none());
    assert_eq!(dst(&h), Some(below.display().to_string()), "Enter takes the folder chosen");
    // OK takes the folder clicked.
    key_press(&mut h, Key::A, None, Modifiers::COMMAND);
    push(&mut h, Event::Text(d.home.display().to_string()));
    settle(&mut h, 2);
    let r = open_dialog_tree(&mut h, &docs);
    click_at(&mut h, r.center());
    let ok = tree(&h).unwrap().2.unwrap().0;
    click_at(&mut h, ok.center());
    assert!(tree(&h).is_none(), "OK closes the tree");
    assert_eq!(dst(&h), Some(docs.display().to_string()));
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("dialog-dst")), "the field has the keyboard back");
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 2);
    wait_until(&mut h, 5.0, |_| docs.join("src.txt").exists());
    assert!(docs.join("src.txt").exists());
    let _ = std::fs::remove_file(docs.join("src.txt"));
}

/// The address bar's folder tree: opens at the folder shown, a chevron opens a folder,
/// a click chooses one and OK (or Enter) goes there, Esc or a click elsewhere closes it.
#[test]
#[ignore]
fn address_bar_folder_tree() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.home, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let rows = |h: &Harness<'static, App>| -> Vec<(PathBuf, egui::Rect)> { h.state().panes[0].tab().tree.as_ref().map(|t| t.rows.clone()).unwrap_or_default() };
    let wait_row = |h: &mut Harness<'static, App>, p: &Path| -> egui::Rect {
        for _ in 0..60 {
            if let Some(r) = rows(h).iter().find(|r| r.0 == p) {
                return r.1;
            }
            settle(h, 1);
        }
        if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
            h.render().unwrap().save(Path::new(&out).join("address-tree-fail.png")).unwrap();
        }
        panic!("no row for {}: {:?}", p.display(), rows(h).iter().map(|r| (r.0.clone(), r.1)).collect::<Vec<_>>());
    };
    // The address bar of the window on the left.
    let open = |h: &mut Harness<'static, App>| {
        h.query_all_by_role_and_label(egui::accesskit::Role::Button, &t("Folder tree"))
            .min_by(|a, b| a.bounding_box().unwrap().x0.total_cmp(&b.bounding_box().unwrap().x0))
            .unwrap()
            .click();
        settle(h, 3);
        assert!(h.state().panes[0].tab().tree.is_some(), "the tree opens");
    };

    open(&mut h);
    let projects = d.home.join("Projects");
    wait_row(&mut h, &projects);
    let mf = projects.join("matrix_finder");
    assert!(!rows(&h).iter().any(|r| r.0 == mf), "closed folders do not show their subfolders");
    // The chevron opens a folder.
    let chev = h.state().panes[0].tab().tree.as_ref().unwrap().toggles.iter().find(|r| r.0 == projects).expect("a chevron").1;
    click_at(&mut h, chev.center());
    assert!(h.state().panes[0].tab().tree.is_some(), "the chevron leaves the tree open");
    let r = wait_row(&mut h, &mf);
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("address-tree.png")).unwrap();
    }
    // A click on a folder chooses it, OK goes there.
    click_at(&mut h, r.center());
    assert!(h.state().panes[0].tab().tree.is_some(), "a click leaves the tree open");
    assert_eq!(h.state().panes[0].tab().tree.as_ref().unwrap().selected(), Some(mf.as_path()));
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(d.home.clone()), "and goes nowhere yet");
    let ok = h.state().panes[0].tab().tree.as_ref().unwrap().buttons.expect("OK and Cancel").0;
    click_at(&mut h, ok.center());
    settle(&mut h, 2);
    assert!(h.state().panes[0].tab().tree.is_none(), "OK closes the tree");
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(mf.clone()));

    // Opened again: at the new folder, its parents open.
    open(&mut h);
    wait_row(&mut h, &mf);
    assert_eq!(h.state().panes[0].tab().tree.as_ref().unwrap().current(), Some(mf.as_path()));
    // ← goes to the folder holding it, Enter goes there; the file list's keys are left alone.
    let cursor = h.state().panes[0].tab().cursor;
    key_press(&mut h, Key::ArrowLeft, None, Modifiers::NONE);
    settle(&mut h, 1);
    if h.state().panes[0].tab().tree.as_ref().unwrap().selected() == Some(mf.as_path()) {
        // It was open (it has subfolders): the first ← closed it.
        key_press(&mut h, Key::ArrowLeft, None, Modifiers::NONE);
        settle(&mut h, 1);
    }
    assert_eq!(h.state().panes[0].tab().tree.as_ref().unwrap().selected(), Some(projects.as_path()));
    key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
    key_press(&mut h, Key::ArrowUp, None, Modifiers::NONE);
    settle(&mut h, 1);
    assert_eq!(h.state().panes[0].tab().cursor, cursor, "the arrows move in the tree, not in the list");
    assert_eq!(h.state().panes[0].tab().tree.as_ref().unwrap().selected(), Some(projects.as_path()));
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(h.state().panes[0].tab().tree.is_none());
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(projects.clone()), "Enter goes to the folder chosen");
    // Esc closes it.
    open(&mut h);
    wait_row(&mut h, &projects);
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(h.state().panes[0].tab().tree.is_none(), "Esc closes the tree");
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(projects.clone()));
    // So does a click elsewhere.
    open(&mut h);
    click_at(&mut h, pos2(W * 0.6, H - 60.0));
    assert!(h.state().panes[0].tab().tree.is_none(), "a click elsewhere closes the tree");
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(projects));
}

/// An open folder tree as a test sees it: its rows, the folder chosen, and its OK and
/// Cancel buttons.
type TreeSeen = (Vec<(PathBuf, egui::Rect)>, Option<PathBuf>, Option<(egui::Rect, egui::Rect)>);

/// The folder tree of the dialog's folder field, while it is open.
fn dialog_tree(h: &Harness<'static, App>) -> Option<TreeSeen> {
    h.state().dialog_path.tree_of(egui::Id::new("dialog-dst")).map(|t| (t.rows.clone(), t.selected().map(Path::to_path_buf), t.buttons))
}

/// Open the dialog's folder tree (the lowest "Folder tree" button: the address bars have
/// one too) and wait for the row of `p`.
fn open_dialog_tree(h: &mut Harness<'static, App>, p: &Path) -> egui::Rect {
    h.query_all_by_role_and_label(egui::accesskit::Role::Button, &t("Folder tree"))
        .max_by(|a, b| a.bounding_box().unwrap().y0.total_cmp(&b.bounding_box().unwrap().y0))
        .unwrap()
        .click();
    settle(h, 3);
    for _ in 0..60 {
        if let Some(r) = dialog_tree(h).and_then(|t| t.0.iter().find(|r| r.0 == p).map(|r| r.1)) {
            return r;
        }
        settle(h, 1);
    }
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("dialog-tree-fail.png")).unwrap();
    }
    panic!("no tree row for {}", p.display());
}

fn suggestion_names(h: &Harness<'static, App>) -> Vec<String> {
    h.state().dialog_path.complete.items.iter().map(|s| s.name.clone()).collect()
}

/// The Compress dialog: the formats to choose from (the single-file ones only for one
/// file), a format typed at the end of the name is chosen, "Save to" suggests folders and
/// has the folder tree, and the file it will make is shown (with a warning when one is
/// there already). Plain .tar has no level.
#[test]
#[ignore]
fn compress_dialog() {
    use egui::accesskit::Role;
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let dir = d.demo.join("out").join("pack");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("photos").join("2024")).unwrap();
    std::fs::write(dir.join("photos").join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("notes.txt"), "notes").unwrap();
    let dest = dir.join("dest");
    std::fs::create_dir_all(dest.join("inner")).unwrap();
    std::fs::write(dest.join("photos.zip"), "taken").unwrap();
    h.state_mut().shot_pane(0, &dir, Some("photos"));
    h.state_mut().shot_pane(1, &dest, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let pack = |h: &Harness<'static, App>| match &h.state().dialog {
        Some(Dialog::Pack { name, dest_dir, format, .. }) => Some((name.clone(), dest_dir.clone(), *format)),
        _ => None,
    };
    let chip = |h: &Harness<'static, App>, ext: &str| h.query_all_by_role_and_label(Role::Button, &format!(".{ext}")).next().map(|n| n.bounding_box().unwrap());
    let has_text = |h: &Harness<'static, App>, s: &str| h.query_by_label(s).is_some();
    let taken = t("A file with this name is already there: choose another name.");

    // A folder: no single-file formats; the other window is where it goes.
    h.state_mut().run_cmd(Cmd::Pack);
    settle(&mut h, 3);
    assert_eq!(pack(&h), Some(("photos".into(), dest.display().to_string(), ArchiveFormat::Zip)));
    assert!(chip(&h, "zip").is_some() && chip(&h, "7z").is_some() && chip(&h, "tar.zst").is_some());
    assert!(chip(&h, "gz").is_none() && chip(&h, "xz").is_none(), "single-file formats are for one file");
    assert!(has_text(&h, &ArchiveFormat::Zip.blurb()), "what the format is good for");
    let makes =
        |h: &Harness<'static, App>, name: &str, dir: &Path| h.query_all_by_label(name).count() >= 1 && h.query_by_label(&dir.display().to_string()).is_some();
    assert!(makes(&h, "photos.zip", &dest), "the file it makes");
    assert!(has_text(&h, &taken), "a warning: photos.zip is there already");
    assert_eq!(h.query_all_by_role(Role::Slider).count(), 1, "the level");
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("compress-dialog.png")).unwrap();
    }
    // Enter while the name is taken keeps the dialog.
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(pack(&h).is_some(), "not made over the file that is there");
    // A click on a format chooses it.
    let r = chip(&h, "7z").unwrap();
    click_at(&mut h, pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32));
    assert_eq!(pack(&h).unwrap().2, ArchiveFormat::SevenZ);
    assert!(!has_text(&h, &taken));
    assert!(makes(&h, "photos.7z", &dest));
    // .tar has no level.
    let r = chip(&h, "tar").unwrap();
    click_at(&mut h, pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32));
    assert_eq!(pack(&h).unwrap().2, ArchiveFormat::Tar);
    assert_eq!(h.query_all_by_role(Role::Slider).count(), 0, "no level for .tar");
    // A format typed in the name is chosen, and not added twice.
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::new("pack-name")));
    settle(&mut h, 1);
    key_press(&mut h, Key::A, None, Modifiers::COMMAND);
    push(&mut h, Event::Text("album.tar.gz".into()));
    settle(&mut h, 2);
    assert_eq!(pack(&h).unwrap().2, ArchiveFormat::TarGz);
    assert!(makes(&h, "album.tar.gz", &dest));

    // Save to: folders that match what is typed.
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::new("dialog-dst")));
    settle(&mut h, 1);
    key_press(&mut h, Key::A, None, Modifiers::COMMAND);
    push(&mut h, Event::Text(format!("{}/", dir.display())));
    for _ in 0..60 {
        if suggestion_names(&h) == ["dest", "photos"] {
            break;
        }
        settle(&mut h, 1);
    }
    assert_eq!(suggestion_names(&h), ["dest", "photos"], "folders only");
    // Enter on a suggestion fills it in, and does not compress yet.
    key_press(&mut h, Key::ArrowDown, None, Modifiers::NONE);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert_eq!(pack(&h).map(|p| p.1), Some(format!("{}/dest/", dir.display())));
    // And the folder tree, at the folder typed: choose "inner", OK.
    let r = open_dialog_tree(&mut h, &dest.join("inner"));
    assert_eq!(dialog_tree(&h).unwrap().1, Some(dest.clone()), "open at the folder typed");
    click_at(&mut h, r.center());
    let ok = dialog_tree(&h).unwrap().2.unwrap().0;
    click_at(&mut h, ok.center());
    assert!(dialog_tree(&h).is_none());
    assert_eq!(pack(&h).map(|p| p.1), Some(dest.join("inner").display().to_string()));
    // Enter compresses.
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(pack(&h).is_none());
    let made = dest.join("inner").join("album.tar.gz");
    wait_until(&mut h, 10.0, |a| a.jobs.is_empty() && made.exists());
    let listed = archive::list(&made).unwrap();
    assert!(listed.iter().any(|e| e.path == "photos/a.txt"), "{:?}", listed.iter().map(|e| &e.path).collect::<Vec<_>>());

    // One file: the single-file formats too.
    h.state_mut().shot_pane(0, &dir, Some("notes.txt"));
    settle(&mut h, 2);
    h.state_mut().run_cmd(Cmd::Pack);
    settle(&mut h, 3);
    let r = chip(&h, "gz").expect(".gz for one file");
    click_at(&mut h, pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32));
    assert_eq!(pack(&h).unwrap().2, ArchiveFormat::Gz);
    assert!(makes(&h, "notes.gz", &dest));
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(pack(&h).is_none(), "Esc closes the dialog");
}

/// A large compression shows its progress: in the status bar, the toolbar's gauge and
/// the Tasks tab, and it can be paused there.
#[test]
#[ignore]
fn compress_progress_shows_in_tasks() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let dir = d.demo.join("out").join("pack-progress");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 64 MB that does not compress much.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let bytes: Vec<u8> = (0..64 << 20)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    std::fs::write(dir.join("big.bin"), &bytes).unwrap();
    h.state_mut().shot_pane(0, &dir, Some("big.bin"));
    h.state_mut().shot_pane(1, &dir, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);

    h.state_mut().run_cmd(Cmd::Pack);
    settle(&mut h, 3);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    h.step();
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().jobs.len(), 1);
    let job = h.state().jobs[0].job.progress.clone();
    // Paused once it has begun, so that it is caught part way.
    let t0 = Instant::now();
    while job.done() == 0 && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(2));
    }
    job.pause();
    settle(&mut h, 2);
    let f = job.fraction().expect("the size is known");
    assert!(f > 0.0 && f < 1.0, "part way: {f}");
    let title = format!("{} big.zip", t("Compressing"));
    assert!(h.query_all_by_label(&title).count() >= 1, "the status bar shows it");
    assert_eq!(h.state().transfers_gauge().map(|g| g.1), Some(true), "the gauge shows it paused");
    // The Tasks tab: the job with its progress bar, paused.
    h.state_mut().open_transfers();
    settle(&mut h, 3);
    assert_eq!(h.state().panes[h.state().active].tab().title(), t("Tasks"));
    assert!(h.query_all_by_label(&title).count() >= 2, "listed in Tasks");
    assert!(h.query_all_by_label_contains(&t("Paused")).count() >= 2, "paused, in the status bar and in Tasks");
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("compress-progress.png")).unwrap();
    }
    job.resume();
    wait_until(&mut h, 60.0, |a| a.jobs.is_empty());
    assert!(h.state().history.iter().any(|j| j.ok && j.title == title), "finished");
    assert!(dir.join("big.zip").exists());
}

/// The Extract dialog's "Extract to" field suggests folders and has the folder tree.
#[test]
#[ignore]
fn extract_dialog_suggests_folders_and_has_a_tree() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.demo, Some("bundle.zip"));
    h.state_mut().shot_pane(1, &d.home, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let dest = |h: &Harness<'static, App>| match &h.state().dialog {
        Some(Dialog::Unpack { dest, .. }) => Some(dest.clone()),
        _ => None,
    };
    h.state_mut().run_cmd(Cmd::ExtractOther);
    settle(&mut h, 3);
    assert_eq!(dest(&h), Some(d.home.join("bundle").display().to_string()));
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("dialog-dst")));
    key_press(&mut h, Key::A, None, Modifiers::COMMAND);
    push(&mut h, Event::Text(format!("{}/Doc", d.home.display())));
    for _ in 0..60 {
        if suggestion_names(&h) == ["Documents"] {
            break;
        }
        settle(&mut h, 1);
    }
    assert_eq!(suggestion_names(&h), ["Documents"]);
    // The tree, at the home folder (the folder typed is not there).
    let docs = d.home.join("Documents");
    let r = open_dialog_tree(&mut h, &docs);
    click_at(&mut h, r.center());
    let ok = dialog_tree(&h).unwrap().2.unwrap().0;
    click_at(&mut h, ok.center());
    assert_eq!(dest(&h), Some(docs.display().to_string()));
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(dest(&h).is_none());
}

/// Extract to a folder typed as `~/…` (as the suggestions fill it in) goes to the home
/// folder, and a relative one to the archive's folder, never to a folder named "~".
#[test]
#[ignore]
fn extract_to_home_and_relative_folders() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let (home_out, rel_out) = (d.home.join("extract-tilde"), d.demo.join("extract-rel"));
    for p in [&home_out, &rel_out] {
        let _ = std::fs::remove_dir_all(p);
    }
    h.state_mut().shot_pane(0, &d.demo, Some("bundle.zip"));
    h.state_mut().shot_pane(1, &d.home, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    for (typed, want) in [("~/extract-tilde/", &home_out), ("extract-rel", &rel_out)] {
        h.state_mut().run_cmd(Cmd::ExtractOther);
        settle(&mut h, 3);
        match &mut h.state_mut().dialog {
            Some(Dialog::Unpack { dest, .. }) => *dest = typed.to_string(),
            _ => panic!("no Extract dialog"),
        }
        settle(&mut h, 2);
        key_press(&mut h, Key::Enter, None, Modifiers::NONE);
        settle(&mut h, 2);
        assert!(h.state().dialog.is_none(), "{typed}: extracting");
        wait_until(&mut h, 10.0, |a| a.jobs.is_empty() && want.join("images").is_dir());
        assert!(want.join("images").is_dir(), "{typed} → {}", want.display());
    }
    assert!(!Path::new("~").exists() && !Path::new("extract-rel").exists(), "nothing in the working folder");
}

/// Relative paths are taken from the window's folder (address bar, Copy to), and a
/// bookmark kept as `~/…` is the folder in the home folder.
#[test]
#[ignore]
fn relative_and_home_paths() {
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    cfg.bookmarks = vec![Bookmark::new("Docs", "~/Documents")];
    let mut h = app_harness(&cfg, "gnome");
    let dir = d.demo.join("out").join("relative");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    h.state_mut().shot_pane(0, &dir, Some("a.txt"));
    h.state_mut().shot_pane(1, &d.home, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    // Copy to "sub": the folder copied from holds it.
    h.state_mut().run_cmd(Cmd::Copy);
    settle(&mut h, 2);
    match &mut h.state_mut().dialog {
        Some(Dialog::Transfer { dst, .. }) => *dst = "sub".into(),
        _ => panic!("no Copy dialog"),
    }
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 2);
    assert!(h.state().dialog.is_none(), "copying");
    wait_until(&mut h, 10.0, |a| a.jobs.is_empty() && dir.join("sub/a.txt").exists());
    assert!(dir.join("sub/a.txt").exists());
    // The address bar: "sub" goes into this window's folder.
    h.state_mut().handle_pane_event(0, PaneEvent::NavigateText("sub".into()));
    settle(&mut h, 2);
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(dir.join("sub")));
    // Nothing relative is read from where the app was started.
    h.state_mut().handle_pane_event(0, PaneEvent::NavigateText("src".into()));
    settle(&mut h, 2);
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(dir.join("sub")), "no folder src in sub");
    // The bookmark "~/Documents": opens the home folder's, and counts as that folder's
    // bookmark (the star takes it away instead of adding another).
    let b = h.state().cfg.bookmarks[0].clone();
    h.state_mut().open_bookmark(&b);
    settle(&mut h, 2);
    let docs = d.home.join("Documents");
    assert_eq!(h.state().panes[0].tab().loc, Location::Local(docs.clone()));
    h.state_mut().toggle_bookmark(docs.display().to_string(), "Documents".into());
    assert!(h.state().cfg.bookmarks.is_empty(), "{:?}", h.state().cfg.bookmarks.iter().map(|b| &b.path).collect::<Vec<_>>());
}

/// The quick filter menu ticks several categories: a click ticks or unticks one and the
/// menu stays open; the file list shows the kinds ticked, and the chip names them.
#[test]
#[ignore]
fn quick_filter_ticks_several_kinds() {
    use egui::accesskit::Role;
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let dir = d.demo.join("out").join("quick-kinds");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    for f in ["a.jpg", "b.mp4", "c.mp3", "d.txt"] {
        std::fs::write(dir.join(f), b"x").unwrap();
    }
    h.state_mut().shot_pane(0, &dir, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let shown = |h: &Harness<'static, App>| -> Vec<String> {
        let tab = h.state().panes[0].tab();
        tab.view.iter().map(|&i| &tab.entries[i]).filter(|e| !e.is_parent).map(|e| e.name.clone()).collect()
    };
    let menu_open = |h: &Harness<'static, App>| h.ctx.memory(|m| m.is_popup_open(egui::Id::new(("quick-filter", h.state().panes[0].tab().uid))));
    let funnel = |h: &mut Harness<'static, App>| {
        h.query_all_by_role_and_label(Role::Button, &t("Find and filter"))
            .min_by(|a, b| a.bounding_box().unwrap().x0.total_cmp(&b.bounding_box().unwrap().x0))
            .unwrap()
            .click();
        settle(h, 2);
    };
    let tick = |h: &mut Harness<'static, App>, name: &str| {
        let b = h.get_by_role_and_label(Role::CheckBox, &t(name)).bounding_box().unwrap();
        click_at(h, pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32));
    };

    funnel(&mut h);
    assert!(menu_open(&h));
    tick(&mut h, "Images");
    assert!(menu_open(&h), "the menu stays open for more");
    assert_eq!(shown(&h), ["sub", "a.jpg"]);
    tick(&mut h, "Videos");
    assert_eq!(shown(&h), ["sub", "a.jpg", "b.mp4"], "both kinds");
    assert_eq!(h.state().panes[0].tab().quick.as_ref().unwrap().names, ["Images", "Videos"]);
    assert!(h.query_by_label(&format!("{}, {}", t("Images"), t("Videos"))).is_some(), "the chip names them");
    if let Ok(out) = std::env::var("MATRIX_FINDER_SAMPLES_OUT") {
        h.render().unwrap().save(Path::new(&out).join("quick-filter-menu.png")).unwrap();
    }
    // Unticking one leaves the other.
    tick(&mut h, "Images");
    assert_eq!(shown(&h), ["sub", "b.mp4"]);
    // A click outside closes the menu; the filter stays.
    click_at(&mut h, pos2(W * 0.6, H - 60.0));
    assert!(!menu_open(&h));
    assert_eq!(shown(&h), ["sub", "b.mp4"]);
    // The palette ticks too.
    let i = h.state().cfg.quick_filters.iter().position(|q| q.name == "Music").unwrap();
    h.state_mut().set_quick_filter(Some(i));
    settle(&mut h, 2);
    assert_eq!(shown(&h), ["sub", "b.mp4", "c.mp3"]);
    // Show all files turns it off and closes the menu.
    funnel(&mut h);
    h.get_by_label(&t("Show all files")).click();
    settle(&mut h, 2);
    assert!(h.state().panes[0].tab().quick.is_none());
    assert!(!menu_open(&h));
    assert_eq!(shown(&h), ["sub", "a.jpg", "b.mp4", "c.mp3", "d.txt"]);
}

/// Enter in a dialog confirms only from its text field or with nothing focused: on Cancel
/// it cancels, on a checkbox it only ticks. Esc closes; a click outside throws away a
/// confirmation, never a form. A command from the palette acts on the file the cursor was on.
#[test]
#[ignore]
fn dialog_keys_follow_the_focused_widget() {
    let d = demo_env();
    let dir = d.home.join("dialog-keys");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("keep.txt"), b"keep").unwrap();
    std::fs::write(dir.join("other.md"), b"other").unwrap();
    let mut cfg = settings("Breeze Light", &d);
    cfg.confirm_delete = true;
    cfg.use_trash = true;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &dir, Some("keep.txt"));
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let cursor = |h: &Harness<'static, App>| h.state().panes[0].tab().cursor_entry().map(|e| e.name.clone());
    assert_eq!(cursor(&h).as_deref(), Some("keep.txt"));

    // Delete, then Enter on Cancel: nothing is deleted.
    h.state_mut().run_cmd(Cmd::Delete);
    settle(&mut h, 2);
    assert!(matches!(h.state().dialog, Some(Dialog::Delete { .. })));
    h.get_by_label(&t("Cancel")).focus();
    settle(&mut h, 2);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    assert!(h.state().dialog.is_none(), "Enter on Cancel cancels");
    settle(&mut h, 4);
    assert!(h.state().jobs.is_empty() && dir.join("keep.txt").exists());

    // Enter and Space on the trash checkbox tick it, and confirm nothing.
    h.state_mut().run_cmd(Cmd::Delete);
    settle(&mut h, 2);
    h.get_by_label(&t("Move to trash (recycle bin)")).focus();
    settle(&mut h, 2);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    assert!(matches!(h.state().dialog, Some(Dialog::Delete { trash: false, .. })), "unticked, still asking");
    key_press(&mut h, Key::Space, None, Modifiers::NONE);
    assert!(matches!(h.state().dialog, Some(Dialog::Delete { trash: true, .. })));
    // A click outside closes a confirmation.
    let corner = pos2(6.0, H - 6.0);
    move_to(&mut h, corner);
    button(&mut h, corner, PointerButton::Primary, true, Modifiers::NONE);
    button(&mut h, corner, PointerButton::Primary, false, Modifiers::NONE);
    assert!(h.state().dialog.is_none());
    settle(&mut h, 4);
    assert!(h.state().jobs.is_empty() && dir.join("keep.txt").exists());

    // Deleting for good takes a click on the red button: Enter does not confirm it, even
    // with nothing focused, while it confirms other dialogs then.
    h.state_mut().cfg.use_trash = false;
    h.state_mut().run_cmd(Cmd::Delete);
    settle(&mut h, 2);
    assert!(matches!(h.state().dialog, Some(Dialog::Delete { trash: false, .. })));
    assert!(h.state().egui_ctx.memory(|m| m.focused()).is_none(), "nothing focused");
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    settle(&mut h, 3);
    assert!(matches!(h.state().dialog, Some(Dialog::Delete { trash: false, .. })), "Enter alone does not delete for good");
    assert!(h.state().jobs.is_empty() && dir.join("keep.txt").exists());
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert!(h.state().dialog.is_none());
    // With confirmations off, deleting for good (Shift+Delete) still asks.
    h.state_mut().cfg.confirm_delete = false;
    h.state_mut().cfg.use_trash = true;
    let ctx = h.ctx.clone();
    h.state_mut().run_shortcut(&ctx, Action::DeletePermanently);
    settle(&mut h, 2);
    assert!(matches!(h.state().dialog, Some(Dialog::Delete { trash: false, .. })), "Shift+Delete asks even with confirmations off");
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    settle(&mut h, 3);
    assert!(h.state().jobs.is_empty() && dir.join("keep.txt").exists());
    h.state_mut().cfg.confirm_delete = true;

    // A click outside keeps a typed name; Enter in the field creates the folder where
    // it was asked for, even when the window went to another folder meanwhile.
    h.state_mut().run_cmd(Cmd::NewFolder);
    settle(&mut h, 3);
    push(&mut h, Event::Text("made".into()));
    settle(&mut h, 2);
    button(&mut h, corner, PointerButton::Primary, true, Modifiers::NONE);
    button(&mut h, corner, PointerButton::Primary, false, Modifiers::NONE);
    assert!(matches!(&h.state().dialog, Some(Dialog::NewItem { name, .. }) if name == "made"), "the form stays");
    let app = h.state_mut();
    app.panes[0].tab_mut().navigate(Location::Local(d.demo.clone()), &lc!(app));
    settle(&mut h, 2);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    assert!(h.state().dialog.is_none());
    assert!(dir.join("made").is_dir() && !d.demo.join("made").exists());
    h.state_mut().shot_pane(0, &dir, Some("keep.txt"));
    settle(&mut h, 3);

    // Esc closes any dialog.
    h.state_mut().run_cmd(Cmd::Rename);
    settle(&mut h, 2);
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert!(h.state().dialog.is_none());

    // The palette's search filters the folder while it is open; Esc, or a command chosen
    // there, puts the cursor back on its file.
    assert_eq!(cursor(&h).as_deref(), Some("keep.txt"));
    h.state_mut().palette.open_with("other");
    settle(&mut h, 3);
    assert_ne!(cursor(&h).as_deref(), Some("keep.txt"), "hidden by the filter");
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert!(!h.state().palette.open);
    assert_eq!(cursor(&h).as_deref(), Some("keep.txt"));
    h.state_mut().palette.open_with("other");
    settle(&mut h, 3);
    h.state_mut().restore_pal_filter();
    h.state_mut().palette.open = false;
    h.state_mut().run_cmd(Cmd::Rename);
    assert!(matches!(&h.state().dialog, Some(Dialog::Rename { old, .. }) if old == "keep.txt"));
    h.state_mut().dialog = None;
    settle(&mut h, 2);

    // Shift+Tab goes back to the window before.
    let app = h.state_mut();
    app.shot_pane(1, &dir, None);
    app.split_pane(1, SplitDir::Down, false);
    app.shot_focus(0);
    settle(&mut h, 3);
    let leaves = h.state().layout.leaves();
    key_press(&mut h, Key::Tab, None, Modifiers::SHIFT);
    assert_eq!(h.state().active, *leaves.last().unwrap());
    key_press(&mut h, Key::Tab, None, Modifiers::NONE);
    assert_eq!(h.state().active, leaves[0]);

    // Another window closing leaves the focus where it is.
    let focused = h.state().panes[leaves[0]].tab().uid;
    let last = *leaves.last().unwrap();
    h.state_mut().close_pane(last);
    settle(&mut h, 2);
    assert_eq!(h.state().panes[h.state().active].tab().uid, focused);

    // A file window's keys are not offered (nor run) in Settings.
    h.state_mut().open_settings(SettingsPage::Appearance);
    settle(&mut h, 3);
    assert!(h.query_by_label(&t("New folder")).is_none());
    h.state_mut().run_cmd(Cmd::NewFolder);
    assert!(h.state().dialog.is_none());
}

/// A narrow sidebar (dragged to its minimum, or a quarter of a small window) is never
/// narrower than its app menu row: content spilling past its edge left an unpainted
/// band between the sidebar and the files.
#[test]
#[ignore]
fn narrow_sidebar_leaves_no_band() {
    let d = demo_env();
    for chrome in ["mac", "gnome", "windows"] {
        for (sidebar, w) in [(160.0, W), (210.0, 640.0), (210.0, 560.0)] {
            let mut cfg = settings("macOS Light", &d);
            cfg.sidebar_width = sidebar;
            let mut h = app_harness(&cfg, chrome);
            h.set_size(egui::vec2(w, H));
            settle(&mut h, 4);
            let img = h.render().unwrap();
            let r = egui::containers::panel::PanelState::load(&h.ctx, egui::Id::new("sidebar")).unwrap().rect;
            let panel = h.state().theme.panel;
            let px = img.get_pixel(r.right() as u32 - 2, (H - 120.0) as u32).0;
            assert_eq!([px[0], px[1], px[2]], [panel.r(), panel.g(), panel.b()], "{chrome} sidebar {sidebar} window {w}: painted to its edge ({r:?})");
        }
    }
}

/// The Terminal submenu also offers the selected folders: one opens at once, several
/// only after a confirmation that names them (a stray click opens nothing).
#[test]
#[ignore]
fn terminal_in_selected_folders() {
    let d = demo_env();
    let row = |h: &Harness<'static, App>, k: f32| {
        let r = window_rect(h, 0);
        pos2(r.left() + 120.0, r.top() + 80.0 + crate::ui::pane::ROW_H * k)
    };
    let menu_item = |h: &mut Harness<'static, App>, k: f32, label: &str| {
        right_click(h, row(h, k));
        settle(h, 2);
        let p = node_center(h, &t("Terminal"), 0.0).expect("Terminal submenu");
        move_to(h, p);
        settle(h, 4);
        let p = node_center(h, label, 0.0).unwrap_or_else(|| panic!("no menu item {label:?}"));
        click_at(h, p);
        settle(h, 4);
    };
    // The shell's folder, as the shell has it (/private/tmp for /tmp on macOS).
    let cwd = |h: &Harness<'static, App>| h.state().panes[h.state().active].tab().term.as_ref().and_then(|t| t.cwd.canonicalize().ok());

    // Several marked folders (rows 1 and 2: big, data): asks first, names them.
    let mut h = app_harness(&settings("Adwaita Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.demo, Some(".."));
    h.state_mut().shot_mark(0, &["big", "data", "archive.tar"]);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let tabs = h.state().strip.tabs.len();
    menu_item(&mut h, 1.0, &t("Open terminals in {n} folders…").replace("{n}", "2"));
    match &h.state().dialog {
        Some(Dialog::OpenTerminals { dirs }) => assert_eq!(dirs, &vec![d.demo.join("big"), d.demo.join("data")]),
        _ => panic!("asks before opening several terminals"),
    }
    assert_eq!(h.state().strip.tabs.len(), tabs, "nothing opened yet");
    assert!(h.query_by_label(&t("Open {n} terminals?").replace("{n}", "2")).is_some());
    h.get_by_label(&t("Open {n} terminals").replace("{n}", "2")).click();
    settle(&mut h, 4);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().strip.tabs.len(), tabs + 2, "a terminal tab for each folder");
    assert_eq!(cwd(&h), d.demo.join("data").canonicalize().ok());

    // Cancel opens nothing.
    let mut h = app_harness(&settings("Adwaita Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.demo, Some(".."));
    h.state_mut().shot_mark(0, &["big", "data"]);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let tabs = h.state().strip.tabs.len();
    menu_item(&mut h, 2.0, &t("Open terminals in {n} folders…").replace("{n}", "2"));
    h.get_by_label(&t("Cancel")).click();
    settle(&mut h, 3);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().strip.tabs.len(), tabs, "cancelled");

    // A right-click on a folder outside the marked ones is about that folder alone: the
    // marks go, and the terminal opens where the menu says (not in the marked folders).
    let tabs = h.state().strip.tabs.len();
    h.state_mut().shot_mark(0, &["big", "data"]);
    settle(&mut h, 2);
    menu_item(&mut h, 3.0, &t("Open terminal in “{name}”").replace("{name}", "docs"));
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().strip.tabs.len(), tabs + 1);
    assert_eq!(cwd(&h), d.demo.join("docs").canonicalize().ok());
    let mut h = app_harness(&settings("Adwaita Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.demo, Some(".."));
    h.state_mut().shot_mark(0, &["big", "data"]);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let p = row(&h, 3.0);
    // On "..", which has no commands for items, the marks stay.
    let up = row(&h, 0.0);
    right_click(&mut h, up);
    settle(&mut h, 2);
    assert_eq!(h.state().panes[0].tab().marked.len(), 2, "a right-click on .. keeps the marks");
    // Esc closes the menu, and only the menu (it used to clear the marks too).
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert_eq!(h.state().panes[0].tab().marked.len(), 2, "Esc on the menu keeps the marks");
    right_click(&mut h, p);
    settle(&mut h, 2);
    assert!(h.state().panes[0].tab().marked.is_empty(), "the marks are gone");
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    let tabs = h.state().strip.tabs.len();

    // One folder, right-clicked without a selection: named in the menu, opens at once.
    h.state_mut().shot_mark(0, &[]);
    menu_item(&mut h, 1.0, &t("Open terminal in “{name}”").replace("{name}", "big"));
    assert!(h.state().dialog.is_none(), "no question for one folder");
    assert_eq!(h.state().strip.tabs.len(), tabs + 1);
    assert_eq!(cwd(&h), d.demo.join("big").canonicalize().ok());
}

/// Copy / Move in a split tab offers the other windows (with a picture of where each
/// one is); with one window, the folder tree shows under the address and follows it.
#[test]
#[ignore]
fn transfer_destinations() {
    let d = demo_env();
    let shots = std::env::var("MATRIX_FINDER_SHOT_DIR").ok();
    let dst = |h: &Harness<'static, App>| match &h.state().dialog {
        Some(Dialog::Transfer { dst, .. }) => dst.clone(),
        _ => panic!("no Copy dialog"),
    };

    // Split: windows 2 and 3 are offered (not window 1, the source), and "Another folder";
    // exactly one is chosen, and the button names it.
    let mut h = app_harness(&settings("Adwaita Light", &d), "gnome");
    let app = h.state_mut();
    app.shot_pane(0, &d.demo, Some("big"));
    app.shot_pane(1, &d.demo.join("docs"), Some(".."));
    app.split_pane(1, SplitDir::Down, false);
    app.shot_pane(2, &d.demo.join("images"), Some(".."));
    app.shot_focus(0);
    settle(&mut h, 3);
    h.state_mut().run_cmd(Cmd::Copy);
    settle(&mut h, 3);
    let win = |n: usize, dir: &Path| {
        format!("{} · {}: {}", t("Window {n}").replace("{n}", &n.to_string()), dir.file_name().unwrap().to_string_lossy(), dir.display())
    };
    let selected = |h: &Harness<'static, App>, label: &str| h.get_by_label(label).toggled() == Some(egui::accesskit::Toggled::True);
    let other = t("Another folder");
    let fields = |h: &Harness<'static, App>| h.query_all_by_role(egui::accesskit::Role::TextInput).count();
    let docs = d.demo.join("docs");
    let images = d.demo.join("images");
    assert!(h.query_by_label(&win(1, &d.demo)).is_none(), "not the source");
    assert!(selected(&h, &win(2, &docs)), "the other window, as before");
    assert!(!selected(&h, &win(3, &images)) && !selected(&h, &other));
    assert_eq!(fields(&h), 0, "no address while a window is chosen");
    assert!(h.query_by_label(&t("Copy to “{name}”").replace("{name}", "docs")).is_some(), "the button names the place");
    if let Some(dir) = &shots {
        h.render().unwrap().save(format!("{dir}/transfer-split.png")).unwrap();
    }
    h.get_by_label(&win(3, &images)).click();
    settle(&mut h, 2);
    assert_eq!(dst(&h), images.display().to_string());
    assert!(selected(&h, &win(3, &images)) && !selected(&h, &win(2, &docs)));
    // Another folder: the address shows, has the keyboard, and the windows are not chosen.
    h.get_by_label(&other).click();
    settle(&mut h, 3);
    assert!(selected(&h, &other) && !selected(&h, &win(3, &images)), "only Another folder is chosen");
    assert_eq!(fields(&h), 1);
    assert!(h.state().dialog_tree.is_some(), "with the folder tree");
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("dialog-dst")), "typing goes to the address");
    key_press(&mut h, Key::A, None, Modifiers::COMMAND);
    let formats = d.demo.join("formats");
    push(&mut h, Event::Text(formats.display().to_string()));
    settle(&mut h, 3);
    assert_eq!(dst(&h), formats.display().to_string());
    assert!(h.query_by_label(&t("Copy to “{name}”").replace("{name}", "formats")).is_some());
    if let Some(dir) = &shots {
        h.render().unwrap().save(format!("{dir}/transfer-split-other.png")).unwrap();
    }
    // Back to a window: the address goes away.
    h.get_by_label(&win(2, &docs)).click();
    settle(&mut h, 2);
    assert_eq!(dst(&h), docs.display().to_string());
    assert_eq!(fields(&h), 0);
    assert!(h.state().dialog_tree.is_none(), "no tree while a window is chosen");
    // Window 2 shows another folder while the dialog is open: no window is the
    // destination any more, so the address shows the one that will be used.
    h.state_mut().shot_pane(1, &d.demo.join("out"), Some(".."));
    // The choices are read again twice a second.
    std::thread::sleep(Duration::from_millis(600));
    settle(&mut h, 3);
    assert_eq!(dst(&h), docs.display().to_string(), "unchanged");
    assert!(selected(&h, &other), "Another folder is chosen");
    assert_eq!(fields(&h), 1, "with its address shown");

    // One window: the tree is there; a click on a folder fills in the address, and
    // typing a folder moves the tree to it.
    let mut h = app_harness(&settings("Adwaita Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.demo, Some("big"));
    while h.state().panes.len() > 1 {
        h.state_mut().close_pane(1);
    }
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    h.state_mut().run_cmd(Cmd::Copy);
    settle(&mut h, 6);
    let tree = h.state().dialog_tree.as_ref().expect("the folder tree");
    // Nothing is filled in (never the folder copied from): the tree opens there for
    // bearings, nothing chosen, and Copy waits for a destination.
    assert_eq!(dst(&h), "");
    assert_eq!((tree.current(), tree.selected()), (Some(d.demo.as_path()), None));
    assert!(h.query_all_by_label(&t("Copy")).any(|n| n.is_disabled()), "Copy waits for a destination");
    let formats = d.demo.join("formats");
    let row = tree.rows.iter().find(|(p, _)| *p == formats).map(|(_, r)| r.center()).expect("formats in the tree");
    if let Some(dir) = &shots {
        h.render().unwrap().save(format!("{dir}/transfer-tree.png")).unwrap();
    }
    click_at(&mut h, row);
    settle(&mut h, 2);
    assert_eq!(dst(&h), formats.display().to_string());
    // Type another folder into the address.
    if let Some(Dialog::Transfer { dst, .. }) = h.state_mut().dialog.as_mut() {
        dst.clear();
    }
    let docs = d.demo.join("docs");
    assert!(type_into(&mut h, egui::Rect::EVERYTHING, &docs.display().to_string()));
    assert_eq!(dst(&h), docs.display().to_string(), "typed");
    assert_eq!(h.state().dialog_tree.as_ref().unwrap().selected(), Some(docs.as_path()), "the tree follows the address");

    // A terminal beside, in the folder copied from: not offered, and nothing filled in.
    let mut h = app_harness(&settings("Adwaita Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.demo, Some("big"));
    while h.state().panes.len() > 1 {
        h.state_mut().close_pane(1);
    }
    let _ = h.state_mut().split_pane(0, SplitDir::Right, true);
    h.state_mut().shot_focus(0);
    settle(&mut h, 10);
    h.state_mut().run_cmd(Cmd::Copy);
    settle(&mut h, 3);
    assert_eq!(dst(&h), "", "not the source, though a window shows it");
    assert!(h.state().dialog_tree.is_some(), "the tree to pick from");
}

/// The app's windows (About, Keyboard shortcuts…) have the platform's title bar: its
/// close button closes them, and so does Esc, the one in front first.
#[test]
#[ignore]
fn dialogs_close_by_their_button_and_esc() {
    let d = demo_env();
    let shots = std::env::var("MATRIX_FINDER_SHOT_DIR").ok();
    for chrome in ["gnome", "kde", "mac", "windows"] {
        let mut h = app_harness(&settings("Adwaita Light", &d), chrome);
        h.state_mut().show_about = true;
        settle(&mut h, 3);
        if let Some(dir) = &shots {
            h.render().unwrap().save(format!("{dir}/about-{chrome}.png")).unwrap();
        }
        // The close button in About's title bar.
        let about = h.ctx.memory(|m| m.area_rect(egui::Id::new("about"))).expect("About is shown");
        let close = h
            .query_all_by_label(&t("Close"))
            .filter_map(|n| n.bounding_box())
            .map(|r| pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32))
            .find(|p| about.contains(*p))
            .unwrap_or_else(|| panic!("{chrome}: a close button in About"));
        assert!(close.y < about.top() + 44.0, "{chrome}: in the title bar");
        click_at(&mut h, close);
        settle(&mut h, 2);
        assert!(!h.state().show_about, "{chrome}: the button closes About");

        // Esc: the window in front first (About opens after Keyboard shortcuts).
        h.state_mut().show_help = true;
        settle(&mut h, 2);
        h.state_mut().show_about = true;
        settle(&mut h, 3);
        key_press(&mut h, Key::Escape, None, Modifiers::NONE);
        assert!(!h.state().show_about && h.state().show_help, "{chrome}: Esc closes the front window");
        key_press(&mut h, Key::Escape, None, Modifiers::NONE);
        assert!(!h.state().show_help, "{chrome}: and then the next");

        // After a click in the files, Esc is theirs: the window stays.
        h.state_mut().show_about = true;
        settle(&mut h, 3);
        click_at(&mut h, pos2(W * 0.9, H * 0.85));
        key_press(&mut h, Key::Escape, None, Modifiers::NONE);
        assert!(h.state().show_about, "{chrome}: Esc after a click in the files is not the window's");
        // A click on the window gives it the keyboard back.
        let about = h.ctx.memory(|m| m.area_rect(egui::Id::new("about"))).unwrap();
        click_at(&mut h, about.center());
        key_press(&mut h, Key::Escape, None, Modifiers::NONE);
        assert!(!h.state().show_about, "{chrome}: Esc after a click on the window closes it");

        // Going to another window by keyboard (or a command) gives the files the keys back.
        if chrome == "gnome" {
            if h.state().panes.len() < 2 {
                h.state_mut().split_pane(0, SplitDir::Right, false);
                settle(&mut h, 3);
            }
            h.state_mut().show_about = true;
            settle(&mut h, 3);
            assert!(h.state().keys_to_window);
            let other = h.state().layout.next_after(h.state().active);
            h.state_mut().shot_focus(other);
            settle(&mut h, 2);
            assert!(!h.state().keys_to_window, "the keyboard follows the window switched to");
            key_press(&mut h, Key::Escape, None, Modifiers::NONE);
            assert!(h.state().show_about, "Esc is the files' then");
        }
    }
}

/// On Linux the main window has rounded corners: they are see-through, the rest is not.
#[test]
#[ignore]
fn linux_window_has_round_corners() {
    let d = demo_env();
    for (chrome, radius) in [("gnome", 12.0), ("kde", 8.0)] {
        let mut h = app_harness(&settings("Adwaita Light", &d), chrome);
        settle(&mut h, 2);
        assert_eq!(h.state().window_radius, radius, "{chrome}");
        let img = h.render().unwrap();
        let alpha = |x: f32, y: f32| img.get_pixel(x as u32, y as u32).0[3];
        for (x, y) in [(0.0, 0.0), (W - 1.0, 0.0), (0.0, H - 1.0), (W - 1.0, H - 1.0)] {
            assert_eq!(alpha(x, y), 0, "{chrome}: corner ({x}, {y}) shows the desktop");
        }
        for (x, y) in [(radius, 1.0), (W / 2.0, H - 2.0), (2.0, H / 2.0), (W - 2.0, H / 2.0)] {
            assert_eq!(alpha(x, y), 255, "{chrome}: ({x}, {y}) is the window");
        }
    }
}

#[test]
fn transfer_button_names_short() {
    assert_eq!(place_name("/home/me/docs/"), Some("docs".into()));
    assert_eq!(place_name("  "), None);
    assert_eq!(place_name("/data/a-very-long-folder-name-from-2024"), Some("a-very-long-folde…".into()));
    assert_eq!(place_name("sftp://host/srv/www"), Some("www".into()));
    // A server's top folder is named by its host, not its port.
    assert_eq!(place_name("sftp://me@db:22/"), Some("db".into()));
    assert_eq!(place_name("ftp://[::1]:2121"), Some("::1".into()));
    assert_eq!(place_name("sftp://me@::1:22/"), Some("::1".into()), "IPv6 written without brackets");
    assert_eq!(place_name("C:\\Users\\me\\docs"), Some("docs".into()));
    assert_eq!(place_name("/tmp/b.zip::inner/x"), Some("x".into()));
}

/// Settings → Appearance → Window: the size at start, set from the window as it is
/// or back to the default, and saved.
#[test]
#[ignore]
fn window_size_at_start_setting() {
    let d = demo_env();
    let mut h = app_harness(&settings("Adwaita Light", &d), "gnome");
    h.state_mut().open_settings(SettingsPage::Appearance);
    settle(&mut h, 3);
    assert!(h.query_by_label(&t("Window size at start")).is_some());
    h.get_by_label(&t("Use current size")).click();
    settle(&mut h, 2);
    assert_eq!((h.state().cfg.window_width, h.state().cfg.window_height), (W, H), "the window as it is");
    assert_eq!((Settings::load().window_width, Settings::load().window_height), (W, H), "saved");
    if let Ok(dir) = std::env::var("MATRIX_FINDER_SHOT_DIR") {
        // Scroll down to the Window section.
        move_to(&mut h, pos2(W * 0.6, H * 0.6));
        for _ in 0..6 {
            push(&mut h, Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -120.0), modifiers: Modifiers::NONE });
            h.step();
        }
        settle(&mut h, 3);
        h.render().unwrap().save(format!("{dir}/window-size.png")).unwrap();
    }
    h.get_by_label(&format!("{} (1000 × 600)", t("Default"))).click();
    settle(&mut h, 2);
    assert_eq!((h.state().cfg.window_width, h.state().cfg.window_height), (1000.0, 600.0));
}

/// Settings → Bookmarks: "Add bookmark" offers the folder the user was in (not the
/// Settings tab's), named; "New group" adds a group with its name ready to type.
#[test]
#[ignore]
fn bookmark_toolbar_adds_folders_and_groups() {
    let d = demo_env();
    let shots = std::env::var("MATRIX_FINDER_SHOT_DIR").ok();
    let mut h = app_harness(&settings("Adwaita Light", &d), "gnome");
    let formats = d.demo.join("formats");
    h.state_mut().shot_pane(0, &formats, Some(".."));
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    h.state_mut().open_settings(SettingsPage::Bookmarks);
    settle(&mut h, 3);
    let before = h.state().cfg.bookmarks.len();
    h.get_by_label(&format!("+ {}", t("Add bookmark"))).click();
    settle(&mut h, 3);
    if let Some(dir) = &shots {
        h.render().unwrap().save(format!("{dir}/bookmarks-add.png")).unwrap();
    }
    let item = t("The folder you were in: {name}").replace("{name}", "formats");
    h.get_by_label(&item).click();
    settle(&mut h, 3);
    let b = h.state().cfg.bookmarks.last().unwrap().clone();
    assert_eq!(h.state().cfg.bookmarks.len(), before + 1);
    assert_eq!((b.name.as_str(), b.path.as_str()), ("formats", formats.to_string_lossy().as_ref()), "the folder the user was in");
    assert_eq!(Settings::load().bookmarks.len(), before + 1, "saved");
    // Once bookmarked, it is offered no more.
    h.get_by_label(&format!("+ {}", t("Add bookmark"))).click();
    settle(&mut h, 3);
    assert!(h.get_by_label(&item).is_disabled(), "already a bookmark");
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    // A new group: at the end, its name has the keyboard.
    h.get_by_label(&format!("+ {}", t("New group"))).click();
    settle(&mut h, 4);
    let g = h.state().cfg.bookmarks.last().unwrap().clone();
    assert!(g.folder && g.name == t("New group"));
    assert!(h.ctx.memory(|m| m.focused()).is_some(), "the group's name is ready to type");
    if let Some(dir) = &shots {
        h.render().unwrap().save(format!("{dir}/bookmarks-group.png")).unwrap();
    }
}

/// A compression that finishes while its folder's tab is hidden (the Tasks tab is shown)
/// is listed once that tab is shown again, without leaving the folder.
#[test]
#[ignore]
fn a_hidden_tab_shows_files_made_meanwhile() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let dir = d.demo.join("out").join("pack-hidden");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 32 MB that does not compress much: still running when it is paused below.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let bytes: Vec<u8> = (0..32 << 20)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    std::fs::write(dir.join("notes.txt"), &bytes).unwrap();
    h.state_mut().shot_pane(0, &dir, Some("notes.txt"));
    h.state_mut().shot_pane(1, &dir, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    h.state_mut().run_cmd(Cmd::Pack);
    settle(&mut h, 3);
    key_press(&mut h, Key::Enter, None, Modifiers::NONE);
    // Held until the Tasks tab is shown, so that it finishes while the folder is hidden.
    let job = h.state().jobs.first().map(|j| j.job.progress.clone()).expect("compressing");
    job.pause();
    let folder_tab = h.state().strip.active;
    h.state_mut().open_transfers();
    settle(&mut h, 2);
    assert_ne!(h.state().strip.active, folder_tab, "Tasks is another tab");
    job.resume();
    wait_until(&mut h, 30.0, |a| a.jobs.is_empty());
    assert!(dir.join("notes.zip").exists());
    h.state_mut().show_tab(folder_tab);
    settle(&mut h, 2);
    for i in 0..2 {
        assert!(h.state().panes[i].tab().entries.iter().any(|e| e.name == "notes.zip"), "window {i} lists the new file");
    }
}

/// A file another program adds to a shown folder appears without a refresh; with the
/// setting off it does not.
#[test]
#[ignore]
fn shown_folders_refresh_by_themselves() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let dir = d.demo.join("out").join("auto-refresh");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("old.txt"), "x").unwrap();
    h.state_mut().cfg.auto_refresh_secs = 1;
    h.state_mut().shot_pane(0, &dir, Some("old.txt"));
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let lists = |a: &App, name: &str| a.panes[0].tab().entries.iter().any(|e| e.name == name);
    std::fs::write(dir.join("new.txt"), "x").unwrap();
    wait_until(&mut h, 10.0, |a| lists(a, "new.txt"));
    assert_eq!(h.state().panes[0].tab().cursor_entry().map(|e| e.name.clone()).as_deref(), Some("old.txt"), "the cursor stays");
    h.state_mut().cfg.auto_refresh = false;
    settle(&mut h, 2);
    std::fs::write(dir.join("later.txt"), "x").unwrap();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(3) {
        h.step();
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!lists(h.state(), "later.txt"), "off: not until a refresh");
}

/// The toolbar's view options: icons (arrow keys move in the grid) and list, hidden
/// files, kept per window in the session; dimmed and inert in a terminal.
#[test]
#[ignore]
fn view_options_switch_icons_and_hidden_files() {
    use crate::config::ViewMode;
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    h.state_mut().shot_pane(0, &d.demo, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    assert_eq!(h.state().panes[0].tab().grid_cols, None, "the list by default");
    let click_button = |h: &Harness<'static, App>| h.query_by_label("View options").expect("the button").click();
    let choose = |h: &mut Harness<'static, App>, label: &str| {
        click_button(h);
        settle(h, 2);
        h.query_by_label_contains(label).expect("in the menu").click();
        settle(h, 2);
        key_press(h, Key::Escape, None, Modifiers::NONE);
        settle(h, 2);
    };
    choose(&mut h, &t("Icons"));
    let cols = h.state().panes[0].tab().grid_cols.expect("icons");
    assert!(cols > 1, "several icons per line: {cols}");
    assert_eq!(h.state().panes[0].tab().view_mode, Some(ViewMode::Icons));
    let has_parent = |a: &App| a.panes[0].tab().view.iter().any(|&i| a.panes[0].tab().entries[i].is_parent);
    assert!(!has_parent(h.state()), "no \"..\" among the icons");
    // Right goes one icon, Down a line of icons, Left back one.
    h.state_mut().panes[0].tab_mut().cursor = 0;
    for (key, want) in [(Key::ArrowRight, 1), (Key::ArrowDown, 1 + cols), (Key::ArrowLeft, cols)] {
        key_press(&mut h, key, None, Modifiers::NONE);
        settle(&mut h, 1);
        assert_eq!(h.state().panes[0].tab().cursor, want, "{key:?}");
    }
    // Kept in the session, for this window only.
    let saved = h.state().save_session();
    assert_eq!(saved.tabs[saved.active].view, Some(ViewMode::Icons));
    // Hidden files, from the same menu.
    let hidden = h.state().cfg.show_hidden;
    choose(&mut h, &t("Show hidden files"));
    assert_eq!(h.state().cfg.show_hidden, !hidden);
    choose(&mut h, &t("List"));
    assert_eq!(h.state().panes[0].tab().grid_cols, None, "back to the list");
    assert!(has_parent(h.state()), "\"..\" is back in the list");
    // In a terminal the button is dimmed and does nothing.
    h.state_mut().new_terminal_here(None, None);
    settle(&mut h, 4);
    assert!(h.state().panes[h.state().active].tab().is_terminal());
    click_button(&h);
    settle(&mut h, 2);
    assert!(h.query_by_label(&t("Icons")).is_none(), "no menu in a terminal");
}

/// The icon view shows pictures by their thumbnail (Settings: on by default); turned
/// off, no picture is read.
#[test]
#[ignore]
fn the_icon_view_shows_thumbnails_unless_turned_off() {
    let d = demo_env();
    let mut cfg = settings("Breeze Light", &d);
    assert!(cfg.thumbnails, "on by default");
    cfg.thumbnails = false;
    let mut h = app_harness(&cfg, "gnome");
    h.state_mut().shot_pane(0, &d.demo.join("images"), None);
    h.state_mut().shot_focus(0);
    h.state_mut().set_view_mode(crate::config::ViewMode::Icons);
    settle(&mut h, 4);
    assert_eq!(h.state().thumbs.asked(), 0, "off: nothing read");
    h.state_mut().cfg.thumbnails = true;
    wait_until(&mut h, 10.0, |a| a.thumbs.made() >= 4);
    // The list view asks for none.
    let asked = h.state().thumbs.asked();
    h.state_mut().set_view_mode(crate::config::ViewMode::List);
    h.state_mut().shot_pane(0, &d.demo.join("data"), None);
    settle(&mut h, 4);
    assert_eq!(h.state().thumbs.asked(), asked);
}

/// A right-click menu acts on the file it was opened on: the folder does not reload by
/// itself under the open menu, and if it reloads anyway (a job finished), the command
/// still finds that file by name rather than by its row.
#[test]
#[ignore]
fn a_right_click_menu_keeps_its_file() {
    let d = demo_env();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    let dir = d.demo.join("out").join("menu-target");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["mm.txt", "zz.txt"] {
        std::fs::write(dir.join(n), "x").unwrap();
    }
    h.state_mut().cfg.auto_refresh_secs = 1;
    h.state_mut().shot_pane(0, &dir, Some("zz.txt"));
    h.state_mut().shot_focus(0);
    settle(&mut h, 3);
    let names = |a: &App| a.panes[0].tab().view.iter().map(|&i| a.panes[0].tab().entries[i].name.clone()).collect::<Vec<_>>();
    assert_eq!(names(h.state()), ["..", "mm.txt", "zz.txt"]);
    // Rows are 28 points high below the header: "..", mm.txt, zz.txt.
    right_click(&mut h, pos2(420.0, 160.0 + 2.0 * 28.0));
    settle(&mut h, 2);
    assert!(h.state().panes[0].tab().menu_open, "the menu is open");
    assert_eq!(h.state().panes[0].tab().cursor_entry().map(|e| e.name.clone()).as_deref(), Some("zz.txt"));
    // A new file above it: no reload while the menu is open.
    std::fs::write(dir.join("aa.txt"), "x").unwrap();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(3500) {
        h.step();
        std::thread::sleep(Duration::from_millis(40));
    }
    assert!(!names(h.state()).contains(&"aa.txt".to_string()), "not under the open menu");
    // Reloaded anyway (as after a job): zz.txt's row now holds mm.txt.
    let remotes = std::collections::HashMap::new();
    let lc = ListCtx { remotes: &remotes, show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
    h.state_mut().panes[0].tab_mut().reload(&lc);
    settle(&mut h, 2);
    assert_eq!(names(h.state()), ["..", "aa.txt", "mm.txt", "zz.txt"]);
    h.query_by_label_contains(&t("Properties")).expect("the menu is still open").click();
    settle(&mut h, 2);
    match &h.state().dialog {
        Some(Dialog::Message { text, .. }) => assert!(text.contains("zz.txt") && !text.contains("mm.txt"), "{text}"),
        _ => panic!("no Properties"),
    }
    // The menu closed: the folder reloads by itself again.
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    std::fs::write(dir.join("ab.txt"), "x").unwrap();
    wait_until(&mut h, 10.0, |a| names(a).contains(&"ab.txt".to_string()));
}

/// The Find panel of the funnel button: it filters as you type, searches the folders
/// below with the Find button, and a running search shows its progress and stops with
/// Stop (its results stay) or Esc.
#[test]
#[ignore]
fn the_find_panel_filters_searches_and_stops() {
    use egui::accesskit::Role;
    let d = demo_env();
    let dir = d.demo.join("out").join("find-panel");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("deep")).unwrap();
    for f in ["My_Report.txt", "my-report.md", "notes.txt", "deep/Old my report.pdf"] {
        std::fs::write(dir.join(f), "x").unwrap();
    }
    // Large files to read, for a search slow enough to catch running.
    let chunk = vec![b'a'; 4 << 20];
    for i in 0..60 {
        let sub = dir.join("big").join(format!("part-{}", i / 10));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("log_{i}.txt")), &chunk).unwrap();
    }
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    h.state_mut().shot_pane(0, &dir, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 2);
    let names = |a: &App| {
        let t = a.panes[0].tab();
        let mut v: Vec<String> = t.view.iter().map(|&i| t.entries[i].name.clone()).filter(|n| n != "..").collect();
        v.sort();
        v
    };
    // Ctrl+F opens it with the keyboard in its field; it filters as you type.
    h.state_mut().panes[0].tab_mut().open_find = true;
    settle(&mut h, 3);
    push(&mut h, Event::Text("MY report".into()));
    settle(&mut h, 3);
    assert_eq!(names(h.state()), ["My_Report.txt", "my-report.md"]);
    // Subfolders too: it waits for Find.
    h.query_by_label(&t("Subfolders too")).expect("the choice").click();
    settle(&mut h, 2);
    assert!(h.state().panes[0].tab().search.is_none(), "not as you type");
    assert_eq!(names(h.state()).len(), 5, "the whole folder until Find");
    h.query_by_role_and_label(Role::Button, &t("Find")).expect("Find").click();
    wait_until(&mut h, 10.0, |a| a.panes[0].tab().search.as_ref().is_some_and(|s| !s.running));
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(names(h.state()), ["My_Report.txt", format!("deep{sep}Old my report.pdf").as_str(), "my-report.md"]);
    // Text inside the big files: Stop, in the footer, keeps what was found so far.
    let f = crate::ui::filter::Find { text: "needle".into(), mode: crate::ui::filter::FindMode::Content, subfolders: true, ..Default::default() };
    h.state_mut().panes[0].tab_mut().find_draft = f.clone();
    let lc = ListCtx { remotes: &std::collections::HashMap::new(), show_hidden: false, dirs_first: true, default_view: crate::config::ViewMode::List };
    h.state_mut().panes[0].tab_mut().set_find(f.clone(), &lc).unwrap();
    settle(&mut h, 2);
    assert!(h.state().panes[0].tab().search.as_ref().is_some_and(|s| s.running), "still reading");
    h.query_by_role_and_label(Role::Button, &t("Stop")).expect("Stop in the footer").click();
    wait_until(&mut h, 10.0, |a| a.panes[0].tab().search.as_ref().is_some_and(|s| !s.running));
    assert!(h.state().panes[0].tab().search.as_ref().unwrap().stopped);
    // Esc stops a running search first, then shows the folder again.
    h.state_mut().panes[0].tab_mut().set_find(f, &lc).unwrap();
    settle(&mut h, 2);
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    wait_until(&mut h, 10.0, |a| a.panes[0].tab().search.as_ref().is_some_and(|s| s.stopped && !s.running));
    key_press(&mut h, Key::Escape, None, Modifiers::NONE);
    assert!(h.state().panes[0].tab().search.is_none());
    assert_eq!(names(h.state()), ["My_Report.txt", "big", "deep", "my-report.md", "notes.txt"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// In the icon view the empty cells after the last icon are background: a right-click
/// there opens the folder's menu, as below the icons and in the list.
#[test]
#[ignore]
fn empty_icon_cells_take_the_folder_menu() {
    let d = demo_env();
    let dir = d.demo.join("out").join("icon-cells");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("only.txt"), "x").unwrap();
    let mut h = app_harness(&settings("Breeze Light", &d), "gnome");
    h.state_mut().shot_pane(0, &dir, None);
    h.state_mut().shot_focus(0);
    settle(&mut h, 2);
    h.state_mut().set_view_mode(crate::config::ViewMode::Icons);
    settle(&mut h, 3);
    assert!(h.state().panes[0].tab().grid_cols.is_some_and(|c| c >= 4));
    // The first line: the icon at its left, empty cells to its right.
    right_click(&mut h, pos2(640.0, 196.0));
    settle(&mut h, 2);
    assert!(h.state().panes[0].tab().menu_open, "the folder's menu");
    assert!(h.query_by_label_contains(&t("Select all")).is_some());
}
