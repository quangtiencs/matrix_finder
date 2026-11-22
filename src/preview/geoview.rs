//! Geometry previews (GeoJSON, WKT, WKB, GPX, KML, shapefiles, GeoPackage): the shapes a
//! file holds, drawn on a plain grid with x and y at the same scale. There is no map
//! under them and nothing is fetched.
use crate::i18n::t;
use crate::preview::Styled;
use crate::style::theme::Theme;
use crate::util::{ext_of, Res};
use egui::Color32;

/// Files beyond this many vertices are shown incomplete, with a note.
const MAX_VERTICES: usize = 5_000_000;
/// Collections inside collections: deeper than this a file is refused (parsing recurses).
const MAX_DEPTH: usize = 64;
/// How far in and out the view goes, relative to the whole drawing.
const ZOOM: std::ops::RangeInclusive<f64> = 0.1..=10_000.0;

pub fn is_geo(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "geojson" | "geojsonl" | "geojsons" | "wkt" | "wkb" | "gpx" | "kml" | "kmz" | "shp" | "gpkg")
}

/// Formats read in place from a file on disk (a database of any size).
pub fn reads_path(name: &str) -> bool {
    ext_of(name) == "gpkg"
}

/// Whether the file is text (its source can be shown) and in which language.
pub fn source_language(name: &str) -> &'static str {
    match ext_of(name).as_str() {
        "geojson" | "geojsonl" | "geojsons" => "json",
        "gpx" | "kml" => "xml",
        _ => "txt",
    }
}

type Pt = [f64; 2];

enum Part {
    Point(Pt),
    Line(Vec<Pt>),
    Polygon(Vec<Vec<Pt>>),
}

/// The shapes of a file, flattened, each with its bounding box `[x0, y0, x1, y1]`.
pub struct GeoData {
    pub format: &'static str,
    parts: Vec<(Part, [f64; 4])>,
    /// Geometry types and how many of each, in the order first seen.
    pub kinds: Vec<(&'static str, usize)>,
    /// How many GeoJSON features (None for formats without features).
    pub features: Option<usize>,
    pub vertices: usize,
    pub bbox: [f64; 4],
    pub srid: Option<u32>,
    /// Some vertices were left out.
    pub truncated: bool,
}

#[derive(Default)]
struct Collect {
    parts: Vec<(Part, [f64; 4])>,
    kinds: Vec<(&'static str, usize)>,
    features: usize,
    vertices: usize,
    truncated: bool,
    srid: Option<u32>,
}

fn bbox_of<'a>(pts: impl Iterator<Item = &'a Pt>) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in pts {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    b
}

fn finite(p: &Pt) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

impl Collect {
    fn count(&mut self, kind: &'static str) {
        match self.kinds.iter_mut().find(|k| k.0 == kind) {
            Some(k) => k.1 += 1,
            None => self.kinds.push((kind, 1)),
        }
    }

    /// Whether `n` more vertices fit.
    fn room(&mut self, n: usize) -> bool {
        if self.vertices + n > MAX_VERTICES {
            self.truncated = true;
            return false;
        }
        self.vertices += n;
        true
    }

    fn point(&mut self, p: Pt) {
        if finite(&p) && self.room(1) {
            self.parts.push((Part::Point(p), [p[0], p[1], p[0], p[1]]));
        }
    }

    fn line(&mut self, mut v: Vec<Pt>) {
        v.retain(finite);
        if !v.is_empty() && self.room(v.len()) {
            let b = bbox_of(v.iter());
            self.parts.push((if v.len() == 1 { Part::Point(v[0]) } else { Part::Line(v) }, b));
        }
    }

    fn polygon(&mut self, mut rings: Vec<Vec<Pt>>) {
        for r in &mut rings {
            r.retain(finite);
        }
        rings.retain(|r| !r.is_empty());
        let n = rings.iter().map(Vec::len).sum();
        if n > 0 && self.room(n) {
            let b = bbox_of(rings.iter().flatten());
            self.parts.push((Part::Polygon(rings), b));
        }
    }

    fn finish(self, format: &'static str, features: bool) -> Res<GeoData> {
        if self.parts.is_empty() {
            return Err(t("This file has no geometry to show."));
        }
        let mut bbox = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for (_, b) in &self.parts {
            bbox = [bbox[0].min(b[0]), bbox[1].min(b[1]), bbox[2].max(b[2]), bbox[3].max(b[3])];
        }
        Ok(GeoData {
            format,
            parts: self.parts,
            kinds: self.kinds,
            features: features.then_some(self.features),
            vertices: self.vertices,
            bbox,
            srid: self.srid,
            truncated: self.truncated,
        })
    }
}

/// Read the geometry of a file, chosen by its extension.
pub fn load(name: &str, bytes: &[u8]) -> Res<GeoData> {
    match ext_of(name).as_str() {
        "geojson" => geojson(bytes, false),
        "geojsonl" | "geojsons" => geojson(bytes, true),
        "wkt" => wkt(bytes),
        "gpx" => gpx(bytes),
        "kml" => kml(bytes, "KML"),
        "kmz" => kmz(bytes),
        "shp" => shapefile(bytes),
        "gpkg" => {
            // SQLite only reads real files.
            let dir = crate::util::TempDir::new()?;
            let path = dir.path().join("preview.gpkg");
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            geopackage(&path)
        }
        _ => wkb_file(bytes),
    }
}

// ---------------------------------------------------------------- GeoJSON

/// A coordinates array: a position, or a list of them (or of lists).
/// Positions are read straight into two numbers, extra ones (height) are skipped.
enum Elem {
    Num(f64),
    Pos(Pt),
    List(Vec<Elem>),
}

impl<'de> serde::Deserialize<'de> for Elem {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Elem;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a number or an array of coordinates")
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Elem, E> {
                Ok(Elem::Num(v))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Elem, E> {
                Ok(Elem::Num(v as f64))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Elem, E> {
                Ok(Elem::Num(v as f64))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Elem, A::Error> {
                let Some(first) = seq.next_element::<Elem>()? else { return Ok(Elem::List(Vec::new())) };
                if let Elem::Num(x) = first {
                    let y = seq.next_element::<f64>()?.ok_or_else(|| serde::de::Error::custom("a position needs two numbers"))?;
                    while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                    return Ok(Elem::Pos([x, y]));
                }
                // The length it claims is not trusted for the allocation.
                let mut v = vec![first];
                while let Some(e) = seq.next_element::<Elem>()? {
                    v.push(e);
                }
                Ok(Elem::List(v))
            }
        }
        d.deserialize_any(V)
    }
}

/// Any GeoJSON object. Properties and other members are skipped unread.
#[derive(serde::Deserialize)]
struct Obj {
    #[serde(rename = "type")]
    kind: String,
    coordinates: Option<Elem>,
    geometries: Option<Vec<Obj>>,
    geometry: Option<Box<Obj>>,
    features: Option<Vec<Obj>>,
}

fn pos(e: Elem) -> Option<Pt> {
    match e {
        Elem::Pos(p) => Some(p),
        _ => None,
    }
}

fn list(e: Elem) -> Option<Vec<Elem>> {
    match e {
        Elem::List(v) => Some(v),
        _ => None,
    }
}

fn path(e: Elem) -> Option<Vec<Pt>> {
    list(e)?.into_iter().map(pos).collect()
}

fn rings(e: Elem) -> Option<Vec<Vec<Pt>>> {
    list(e)?.into_iter().map(path).collect()
}

fn geojson(bytes: &[u8], sequence: bool) -> Res<GeoData> {
    let bad = |e: String| format!("{} {e}", t("Not valid GeoJSON:"));
    let mut c = Collect::default();
    if sequence {
        // One object per line (GeoJSON Text Sequences may start each with a record separator).
        for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
            let mut line = line.trim_ascii();
            while let [0x1e, rest @ ..] = line {
                line = rest.trim_ascii();
            }
            if line.is_empty() {
                continue;
            }
            let o: Obj = serde_json::from_slice(line).map_err(|e| bad(format!("line {}: {e}", i + 1)))?;
            geojson_obj(&mut c, o, true, 0).map_err(bad)?;
        }
    } else {
        let o: Obj = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        geojson_obj(&mut c, o, true, 0).map_err(bad)?;
    }
    c.finish("GeoJSON", true)
}

/// `top`: a geometry of its own (counted by type), not a member of a collection.
fn geojson_obj(c: &mut Collect, o: Obj, top: bool, depth: usize) -> Res<()> {
    if depth > MAX_DEPTH {
        return Err("nested too deeply".into());
    }
    let shape = |kind: &str| format!("{kind} has coordinates of the wrong shape");
    match o.kind.as_str() {
        "FeatureCollection" => {
            for f in o.features.unwrap_or_default() {
                geojson_obj(c, f, true, depth + 1)?;
            }
            return Ok(());
        }
        "Feature" => {
            c.features += 1;
            if let Some(g) = o.geometry {
                geojson_obj(c, *g, true, depth + 1)?;
            }
            return Ok(());
        }
        "GeometryCollection" => {
            for g in o.geometries.unwrap_or_default() {
                geojson_obj(c, g, false, depth + 1)?;
            }
            if top {
                c.count("GeometryCollection");
            }
            return Ok(());
        }
        _ => {}
    }
    let kind: &'static str = match o.kind.as_str() {
        "Point" => "Point",
        "MultiPoint" => "MultiPoint",
        "LineString" => "LineString",
        "MultiLineString" => "MultiLineString",
        "Polygon" => "Polygon",
        "MultiPolygon" => "MultiPolygon",
        other => return Err(format!("unknown type {other:?}")),
    };
    let Some(coords) = o.coordinates else { return Err(format!("{kind} has no coordinates")) };
    let ok = match kind {
        // An empty point.
        "Point" if matches!(&coords, Elem::List(v) if v.is_empty()) => Some(()),
        "Point" => pos(coords).map(|p| c.point(p)),
        "MultiPoint" => path(coords).map(|v| v.into_iter().for_each(|p| c.point(p))),
        "LineString" => path(coords).map(|v| c.line(v)),
        "MultiLineString" => rings(coords).map(|v| v.into_iter().for_each(|l| c.line(l))),
        "Polygon" => rings(coords).map(|r| c.polygon(r)),
        _ => list(coords).and_then(|v| v.into_iter().map(rings).collect::<Option<Vec<_>>>()).map(|v| v.into_iter().for_each(|r| c.polygon(r))),
    };
    ok.ok_or_else(|| shape(kind))?;
    if top {
        c.count(kind);
    }
    Ok(())
}

// ---------------------------------------------------------------- WKT

struct Wkt<'a> {
    s: &'a [u8],
    i: usize,
}

impl Wkt<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|b| b.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            return true;
        }
        false
    }

    fn expect(&mut self, c: u8) -> Res<()> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", c as char, self.i))
        }
    }

    /// A word, upper-cased (empty if there is none here).
    fn word(&mut self) -> String {
        self.ws();
        let start = self.i;
        while self.s.get(self.i).is_some_and(|b| b.is_ascii_alphabetic()) {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).to_uppercase()
    }

    /// `EMPTY` is next (and taken).
    fn empty(&mut self) -> bool {
        let at = self.i;
        if self.word() == "EMPTY" {
            return true;
        }
        self.i = at;
        false
    }

    fn number(&mut self) -> Res<f64> {
        self.ws();
        let start = self.i;
        while self.s.get(self.i).is_some_and(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E')) {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i]).ok().and_then(|n| n.parse().ok()).ok_or_else(|| format!("expected a number at byte {start}"))
    }

    /// `x y [z [m]]`.
    fn coord(&mut self) -> Res<Pt> {
        let p = [self.number()?, self.number()?];
        while !matches!(self.peek(), Some(b',' | b')') | None) {
            self.number()?;
        }
        Ok(p)
    }

    /// `( a, b, … )` or `EMPTY`, each item read by `item`.
    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Res<T>) -> Res<Vec<T>> {
        if self.empty() {
            return Ok(Vec::new());
        }
        self.expect(b'(')?;
        let mut v = vec![item(self)?];
        while self.eat(b',') {
            v.push(item(self)?);
        }
        self.expect(b')')?;
        Ok(v)
    }

    fn path(&mut self) -> Res<Vec<Pt>> {
        self.list(|w| w.coord())
    }

    fn rings(&mut self) -> Res<Vec<Vec<Pt>>> {
        self.list(|w| w.path())
    }

    /// A point of a MULTIPOINT, with or without its own brackets.
    fn multi_point_item(&mut self) -> Res<Option<Pt>> {
        if self.empty() {
            return Ok(None);
        }
        if self.eat(b'(') {
            let p = self.coord()?;
            self.expect(b')')?;
            return Ok(Some(p));
        }
        self.coord().map(Some)
    }

    fn geometry(&mut self, c: &mut Collect, top: bool, depth: usize) -> Res<()> {
        if depth > MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        let mut kind = self.word();
        if kind == "SRID" {
            self.expect(b'=')?;
            c.srid = u32::try_from(self.number()? as i64).ok();
            self.expect(b';')?;
            kind = self.word();
        }
        // Dimensions: Z, M or ZM, also written as POINTZ.
        let at = self.i;
        if !matches!(self.word().as_str(), "Z" | "M" | "ZM") {
            self.i = at;
        }
        let base = kind.trim_end_matches("ZM").trim_end_matches(['Z', 'M']);
        let base = if matches!(base, "POINT" | "LINESTRING" | "POLYGON" | "MULTIPOINT" | "MULTILINESTRING" | "MULTIPOLYGON" | "GEOMETRYCOLLECTION" | "TRIANGLE" | "TIN" | "POLYHEDRALSURFACE") { base } else { kind.as_str() };
        let name: &'static str = match base {
            "POINT" => {
                if !self.empty() {
                    self.expect(b'(')?;
                    let p = self.coord()?;
                    self.expect(b')')?;
                    c.point(p);
                }
                "Point"
            }
            "LINESTRING" => {
                let v = self.path()?;
                c.line(v);
                "LineString"
            }
            "POLYGON" | "TRIANGLE" => {
                let r = self.rings()?;
                c.polygon(r);
                if base == "TRIANGLE" { "Triangle" } else { "Polygon" }
            }
            "MULTIPOINT" => {
                for p in self.list(|w| w.multi_point_item())?.into_iter().flatten() {
                    c.point(p);
                }
                "MultiPoint"
            }
            "MULTILINESTRING" => {
                for l in self.rings()? {
                    c.line(l);
                }
                "MultiLineString"
            }
            "MULTIPOLYGON" | "TIN" | "POLYHEDRALSURFACE" => {
                for r in self.list(|w| w.rings())? {
                    c.polygon(r);
                }
                match base {
                    "TIN" => "TIN",
                    "POLYHEDRALSURFACE" => "PolyhedralSurface",
                    _ => "MultiPolygon",
                }
            }
            "GEOMETRYCOLLECTION" => {
                self.list(|w| w.geometry(c, false, depth + 1))?;
                "GeometryCollection"
            }
            "" => return Err(format!("expected a geometry type at byte {}", self.i)),
            other => return Err(format!("{other} is not supported")),
        };
        if top {
            c.count(name);
        }
        Ok(())
    }
}

/// One geometry, or several one after another (one per line, say).
fn wkt(bytes: &[u8]) -> Res<GeoData> {
    let mut w = Wkt { s: bytes, i: 0 };
    let mut c = Collect::default();
    while w.peek().is_some() {
        w.geometry(&mut c, true, 0).map_err(|e| format!("{} {e}", t("Not valid WKT:")))?;
        w.eat(b';');
    }
    c.finish("WKT", false)
}

// ---------------------------------------------------------------- WKB

struct Wkb<'a> {
    b: &'a [u8],
    i: usize,
}

impl Wkb<'_> {
    fn take(&mut self, n: usize) -> Res<&[u8]> {
        let s = self.b.get(self.i..self.i + n).ok_or_else(|| "the file ends too early".to_string())?;
        self.i += n;
        Ok(s)
    }

    fn u32(&mut self, le: bool) -> Res<u32> {
        let s: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    }

    fn f64(&mut self, le: bool) -> Res<f64> {
        let s: [u8; 8] = self.take(8)?.try_into().unwrap();
        Ok(if le { f64::from_le_bytes(s) } else { f64::from_be_bytes(s) })
    }

    /// A count of items at least `each` bytes long, refused if the rest of the file is shorter.
    fn count(&mut self, le: bool, each: usize) -> Res<usize> {
        let n = self.u32(le)? as usize;
        if n.saturating_mul(each) > self.b.len() - self.i {
            return Err(format!("a count of {n} is more than the file holds"));
        }
        Ok(n)
    }

    fn coords(&mut self, le: bool, dims: usize) -> Res<Vec<Pt>> {
        let n = self.count(le, dims * 8)?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let p = [self.f64(le)?, self.f64(le)?];
            self.take((dims - 2) * 8)?;
            v.push(p);
        }
        Ok(v)
    }

    fn rings(&mut self, le: bool, dims: usize) -> Res<Vec<Vec<Pt>>> {
        let n = self.count(le, 4)?;
        (0..n).map(|_| self.coords(le, dims)).collect()
    }

    /// Byte order and type of the geometry here: (little endian, base type, dimensions).
    fn header(&mut self, c: &mut Collect) -> Res<(bool, u32, usize)> {
        let le = match self.take(1)?[0] {
            0 => false,
            1 => true,
            b => return Err(format!("byte order {b} at byte {}", self.i - 1)),
        };
        let raw = self.u32(le)?;
        // PostGIS (EWKB) flags, or ISO codes: +1000 Z, +2000 M, +3000 ZM.
        if raw & 0x2000_0000 != 0 {
            c.srid = Some(self.u32(le)?);
        }
        let code = raw & 0x0fff_ffff;
        let iso = code / 1000;
        let z = raw & 0x8000_0000 != 0 || iso == 1 || iso == 3;
        let m = raw & 0x4000_0000 != 0 || iso == 2 || iso == 3;
        Ok((le, code % 1000, 2 + z as usize + m as usize))
    }

    fn geometry(&mut self, c: &mut Collect, top: bool, depth: usize) -> Res<()> {
        if depth > MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        let (le, base, dims) = self.header(c)?;
        // The smallest geometry is 9 bytes (an empty one: header and a zero count).
        let members = |w: &mut Self, c: &mut Collect| -> Res<()> {
            let n = w.count(le, 9)?;
            (0..n).try_for_each(|_| w.geometry(c, false, depth + 1))
        };
        let name = match base {
            1 => {
                let p = [self.f64(le)?, self.f64(le)?];
                self.take((dims - 2) * 8)?;
                // An empty point has NaN coordinates, which `point` leaves out.
                c.point(p);
                "Point"
            }
            2 => {
                let v = self.coords(le, dims)?;
                c.line(v);
                "LineString"
            }
            3 | 17 => {
                let r = self.rings(le, dims)?;
                c.polygon(r);
                if base == 17 { "Triangle" } else { "Polygon" }
            }
            4 => {
                members(self, c)?;
                "MultiPoint"
            }
            5 => {
                members(self, c)?;
                "MultiLineString"
            }
            6 | 15 | 16 => {
                members(self, c)?;
                match base {
                    15 => "PolyhedralSurface",
                    16 => "TIN",
                    _ => "MultiPolygon",
                }
            }
            7 => {
                members(self, c)?;
                "GeometryCollection"
            }
            other => return Err(format!("geometry type {other} is not supported")),
        };
        if top {
            c.count(name);
        }
        Ok(())
    }
}

/// Hex text (`0101…`, one geometry per line, as databases export it), if the file is that.
fn hex_geometries(bytes: &[u8]) -> Option<Vec<Vec<u8>>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut out = Vec::new();
    for token in text.split_ascii_whitespace() {
        let h = token.strip_prefix("\\x").or_else(|| token.strip_prefix("0x")).unwrap_or(token);
        if h.len() % 2 != 0 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        out.push((0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect());
    }
    (!out.is_empty()).then_some(out)
}

fn wkb_file(bytes: &[u8]) -> Res<GeoData> {
    let bad = |e: String| format!("{} {e}", t("Not valid WKB:"));
    let mut c = Collect::default();
    let (blobs, format) = match hex_geometries(bytes) {
        Some(h) => (h, "WKB (hex)"),
        None => (vec![bytes.to_vec()], "WKB"),
    };
    for blob in &blobs {
        // Several geometries may follow one another.
        let mut w = Wkb { b: blob, i: 0 };
        while w.i < blob.len() {
            w.geometry(&mut c, true, 0).map_err(bad)?;
        }
    }
    c.finish(format, false)
}

// ---------------------------------------------------------------- GPX and KML

/// `lon,lat[,alt]` tuples separated by white space (KML), or `lon lat alt` (gx:coord).
/// At most as many as the drawing can still take.
fn kml_coords(text: &str, one_per_line: bool) -> Vec<Pt> {
    let parse = |t: &str, sep: char| {
        let mut it = t.split(sep).filter(|s| !s.is_empty()).map(|s| s.trim().parse::<f64>());
        match (it.next(), it.next()) {
            (Some(Ok(x)), Some(Ok(y))) => Some([x, y]),
            _ => None,
        }
    };
    if one_per_line {
        return parse(text.trim(), ' ').into_iter().collect();
    }
    text.split_ascii_whitespace().filter_map(|t| parse(t, ',')).take(MAX_VERTICES + 1).collect()
}

fn xml_reader(bytes: &[u8]) -> quick_xml::Reader<&[u8]> {
    let mut r = quick_xml::Reader::from_reader(bytes);
    let c = r.config_mut();
    c.check_end_names = false;
    c.allow_unmatched_ends = true;
    r
}

fn xml_local(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}

fn xml_attr_f64(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<f64> {
    e.attributes().flatten().find(|a| a.key.as_ref() == key).and_then(|a| std::str::from_utf8(&a.value).ok()?.trim().parse().ok())
}

/// GPX: waypoints as points, and each route and track segment as a line.
fn gpx(bytes: &[u8]) -> Res<GeoData> {
    use quick_xml::events::Event;
    let mut r = xml_reader(bytes);
    let mut c = Collect::default();
    let mut line: Option<Vec<Pt>> = None;
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let point = || Some([xml_attr_f64(&e, b"lon")?, xml_attr_f64(&e, b"lat")?]);
                match xml_local(e.name().as_ref()) {
                    b"wpt" => {
                        if let Some(p) = point() {
                            c.point(p);
                            c.count("Waypoint");
                            c.features += 1;
                        }
                    }
                    b"rte" | b"trk" => {
                        c.count(if xml_local(e.name().as_ref()) == b"rte" { "Route" } else { "Track" });
                        c.features += 1;
                        line = Some(vec![]);
                    }
                    b"trkseg" => line = Some(vec![]),
                    b"rtept" | b"trkpt" => {
                        if let (Some(l), Some(p)) = (line.as_mut(), point()) {
                            if c.vertices + l.len() < MAX_VERTICES {
                                l.push(p);
                            } else {
                                c.truncated = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                if matches!(xml_local(e.name().as_ref()), b"rte" | b"trkseg") {
                    if let Some(l) = line.take() {
                        c.line(l);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("GPX: {e}")),
            _ => {}
        }
    }
    c.finish("GPX", true)
}

/// KML: the points, lines and polygons of its placemarks (and tracks), however nested.
fn kml(bytes: &[u8], format: &'static str) -> Res<GeoData> {
    use quick_xml::events::Event;
    let mut r = xml_reader(bytes);
    let mut c = Collect::default();
    // What the coordinates being read belong to.
    let (mut in_coords, mut in_gx_coord, mut kind): (bool, bool, Option<&'static str>) = (false, false, None);
    let mut rings: Option<Vec<Vec<Pt>>> = None;
    let mut track: Option<Vec<Pt>> = None;
    let mut text = String::new();
    let mut depth = 0usize;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > 256 {
                    return Err(format!("{format}: nested too deeply"));
                }
                match xml_local(e.name().as_ref()) {
                    b"Placemark" => c.features += 1,
                    b"Point" => kind = Some("Point"),
                    b"LineString" if rings.is_none() => kind = Some("LineString"),
                    b"Polygon" => {
                        kind = Some("Polygon");
                        rings = Some(vec![]);
                    }
                    b"Track" => track = Some(vec![]),
                    b"coordinates" => {
                        in_coords = true;
                        text.clear();
                    }
                    b"coord" if track.is_some() => {
                        in_gx_coord = true;
                        text.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) if in_coords || in_gx_coord => {
                if text.len() < 256 * 1024 * 1024 {
                    text.push_str(&String::from_utf8_lossy(e.as_ref()));
                }
            }
            Ok(Event::End(e)) => {
                depth = depth.saturating_sub(1);
                match xml_local(e.name().as_ref()) {
                    b"coordinates" if in_coords => {
                        in_coords = false;
                        let pts = kml_coords(&text, false);
                        match (&mut rings, kind) {
                            (Some(r), _) => r.push(pts),
                            (None, Some("Point")) => pts.into_iter().take(1).for_each(|p| c.point(p)),
                            (None, _) => c.line(pts),
                        }
                    }
                    b"coord" if in_gx_coord => {
                        in_gx_coord = false;
                        if let Some(t) = track.as_mut() {
                            t.extend(kml_coords(&text, true));
                        }
                    }
                    b"Polygon" => {
                        if let Some(r) = rings.take() {
                            c.polygon(r);
                        }
                        c.count("Polygon");
                    }
                    b"Point" | b"LineString" => {
                        if let Some(k) = kind.filter(|_| rings.is_none()) {
                            c.count(k);
                        }
                    }
                    b"Track" => {
                        if let Some(t) = track.take() {
                            c.line(t);
                            c.count("Track");
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("{format}: {e}")),
            _ => {}
        }
    }
    c.finish(format, true)
}

/// A KMZ file is a ZIP with the KML document in it (`doc.kml`, or the first `.kml`).
fn kmz(bytes: &[u8]) -> Res<GeoData> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("KMZ: {e}"))?;
    let name = zip.file_names().flatten().filter(|n| n.to_lowercase().ends_with(".kml")).min_by_key(|n| (*n != "doc.kml", n.len())).map(|n| n.into_owned()).ok_or("KMZ: no KML document inside")?;
    let mut kml_bytes = Vec::new();
    zip.by_name(&name).map_err(|e| e.to_string())?.take(crate::preview::MAX_IMAGE as u64).read_to_end(&mut kml_bytes).map_err(|e| e.to_string())?;
    kml(&kml_bytes, "KMZ")
}

// ---------------------------------------------------------------- Shapefile

/// An ESRI shapefile's `.shp`: its records' shapes (the attributes are in the `.dbf`).
fn shapefile(b: &[u8]) -> Res<GeoData> {
    let bad = || t("Not a valid shapefile.");
    if b.len() < 100 || b[..4] != [0, 0, 0x27, 0x0A] {
        return Err(bad());
    }
    let le_i32 = |at: usize| b.get(at..at + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()));
    let be_i32 = |at: usize| b.get(at..at + 4).map(|s| i32::from_be_bytes(s.try_into().unwrap()));
    let f64_at = |at: usize| b.get(at..at + 8).map(|s| f64::from_le_bytes(s.try_into().unwrap()));
    let pt = |at: usize| Some([f64_at(at)?, f64_at(at + 8)?]);
    let mut c = Collect::default();
    let mut at = 100usize;
    while at + 12 <= b.len() && !c.truncated {
        let len = be_i32(at + 4).ok_or_else(bad)?.max(0) as usize * 2;
        let body = at + 8;
        let end = body.checked_add(len).filter(|e| *e <= b.len()).ok_or_else(bad)?;
        at = end;
        c.features += 1;
        let kind = le_i32(body).ok_or_else(bad)?;
        match kind {
            0 => {}
            1 | 11 | 21 => {
                c.point(pt(body + 4).ok_or_else(bad)?);
                c.count("Point");
            }
            8 | 18 | 28 => {
                let n = le_i32(body + 36).ok_or_else(bad)?.max(0) as usize;
                if body + 40 + n * 16 > end {
                    return Err(bad());
                }
                (0..n).filter_map(|i| pt(body + 40 + i * 16)).for_each(|p| c.point(p));
                c.count("MultiPoint");
            }
            3 | 13 | 23 | 5 | 15 | 25 => {
                let parts = le_i32(body + 36).ok_or_else(bad)?.max(0) as usize;
                let n = le_i32(body + 40).ok_or_else(bad)?.max(0) as usize;
                // Both counts must fit in the record before anything is made of them.
                let points_at = body + 44 + parts * 4;
                if points_at > end || points_at.checked_add(n * 16).is_none_or(|e| e > end) {
                    return Err(bad());
                }
                let starts: Vec<usize> = (0..parts).map(|i| le_i32(body + 44 + i * 4).unwrap_or(0).clamp(0, n as i32) as usize).collect();
                let pieces: Vec<Vec<Pt>> = starts
                    .iter()
                    .enumerate()
                    .map(|(i, &s)| {
                        let e = starts.get(i + 1).copied().unwrap_or(n).max(s);
                        (s..e).filter_map(|k| pt(points_at + k * 16)).collect()
                    })
                    .collect();
                if matches!(kind, 5 | 15 | 25) {
                    c.polygon(pieces);
                    c.count("Polygon");
                } else {
                    pieces.into_iter().for_each(|p| c.line(p));
                    c.count("LineString");
                }
            }
            other => return Err(format!("{} ({other})", t("This shape type is not supported."))),
        }
    }
    c.finish("Shapefile", true)
}

// ---------------------------------------------------------------- GeoPackage

/// Rows read at most from one GeoPackage (the vertex limit usually stops sooner).
const GPKG_MAX_ROWS: usize = 2_000_000;

/// A GeoPackage is a SQLite database: each feature table's geometry column holds
/// WKB behind a small header. Read in place, from the first table on, until the
/// vertex limit; a query that takes too long is cut short.
pub fn geopackage(path: &std::path::Path) -> Res<GeoData> {
    use rusqlite::{Connection, OpenFlags};
    let mut conn = Connection::open_with_flags(crate::preview::scidata::sqlite_uri(path), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "trusted_schema", "OFF").map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    conn.progress_handler(10_000, Some(move || started.elapsed() > std::time::Duration::from_secs(5))).map_err(|e| e.to_string())?;
    let columns: Vec<(String, String)> = {
        let mut q = conn.prepare("SELECT table_name, column_name FROM gpkg_geometry_columns ORDER BY table_name").map_err(|_| t("Not a GeoPackage this preview can read."))?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    let mut srid: Option<u32> = None;
    let mut c = Collect::default();
    let mut read = 0usize;
    'tables: for (table, column) in columns {
        let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let Ok(mut q) = conn.prepare(&format!("SELECT {} FROM {}", quote(&column), quote(&table))) else { continue };
        let Ok(mut rows) = q.query([]) else { continue };
        while let Ok(Some(row)) = rows.next() {
            read += 1;
            if read > GPKG_MAX_ROWS || c.truncated {
                c.truncated = true;
                break 'tables;
            }
            let Ok(blob) = row.get_ref(0).and_then(|v| v.as_blob().map_err(Into::into)) else { continue };
            c.features += 1;
            // "GP", version, flags (byte order, envelope size, empty), SRS id, envelope, WKB.
            if blob.len() < 8 || &blob[..2] != b"GP" {
                continue;
            }
            let flags = blob[3];
            let id = if flags & 1 == 1 { i32::from_le_bytes(blob[4..8].try_into().unwrap()) } else { i32::from_be_bytes(blob[4..8].try_into().unwrap()) };
            srid = srid.or(u32::try_from(id).ok().filter(|i| *i > 0));
            let envelope = match (flags >> 1) & 7 {
                0 => 0,
                1 => 32,
                2 | 3 => 48,
                4 => 64,
                _ => continue,
            };
            if flags & 0x10 != 0 || blob.len() < 8 + envelope {
                continue;
            }
            let mut w = Wkb { b: &blob[8 + envelope..], i: 0 };
            let _ = w.geometry(&mut c, true, 0);
        }
    }
    if c.srid.is_none() {
        c.srid = srid;
    }
    c.finish("GeoPackage", true)
}

// ---------------------------------------------------------------- drawing

/// Where the view looks: the data point at the middle, and pixels per unit.
#[derive(Clone, Copy, PartialEq)]
struct View {
    center: Pt,
    scale: f64,
}

/// How much to scale the drawing so it fills `w` × `h` with a margin.
fn fit_scale(bbox: [f64; 4], w: f64, h: f64) -> f64 {
    let (bw, bh) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    let s = match (bw > 0.0, bh > 0.0) {
        (true, true) => (w * 0.9 / bw).min(h * 0.9 / bh),
        (true, false) => w * 0.9 / bw,
        (false, true) => h * 0.9 / bh,
        // A single point.
        _ => 1.0,
    };
    if s.is_finite() && s > 0.0 { s } else { 1.0 }
}

struct Colors {
    /// The picture is drawn opaque, on this: half-transparent texture pixels blend
    /// to a greyer colour than the same blend done here.
    bg: [u8; 4],
    grid: [u8; 4],
    line: [u8; 4],
    fill: [u8; 4],
    point: [u8; 4],
    halo: [u8; 4],
}

/// Draw the shapes into premultiplied RGBA pixels. `k` is pixels per point (line widths).
fn render(data: &GeoData, view: View, w: usize, h: usize, colors: &Colors, k: f32) -> Vec<u8> {
    use resvg::tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};
    let Some(mut pix) = Pixmap::new(w as u32, h as u32) else { return vec![0; w * h * 4] };
    pix.fill(resvg::tiny_skia::Color::from_rgba8(colors.bg[0], colors.bg[1], colors.bg[2], colors.bg[3]));
    let (half_w, half_h) = (w as f64 / 2.0, h as f64 / 2.0);
    let to_px = |p: &Pt| (((p[0] - view.center[0]) * view.scale + half_w) as f32, (half_h - (p[1] - view.center[1]) * view.scale) as f32);
    // What can be seen, in data units, with a margin for line widths.
    let margin = 8.0 * k as f64 / view.scale;
    let seen = [
        view.center[0] - half_w / view.scale - margin,
        view.center[1] - half_h / view.scale - margin,
        view.center[0] + half_w / view.scale + margin,
        view.center[1] + half_h / view.scale + margin,
    ];
    let paint = |c: [u8; 4]| {
        let mut p = Paint { anti_alias: true, ..Default::default() };
        p.set_color_rgba8(c[0], c[1], c[2], c[3]);
        p
    };
    let (fill, line, point, halo) = (paint(colors.fill), paint(colors.line), paint(colors.point), paint(colors.halo));
    // A light grid of round numbers, for a sense of scale. Counted, not added up: far
    // from zero a small step could leave the sum where it is.
    let step = grid_step(w.max(h) as f64 / view.scale);
    let grid_lines = |from: f64, to: f64| {
        let first = (from / step).ceil();
        (0..64).map(move |i| (first + i as f64) * step).take_while(move |v| *v <= to)
    };
    let mut pb = PathBuilder::new();
    for x in grid_lines(seen[0], seen[2]) {
        let (px, _) = to_px(&[x, 0.0]);
        pb.move_to(px.round() + 0.5, 0.0);
        pb.line_to(px.round() + 0.5, h as f32);
    }
    for y in grid_lines(seen[1], seen[3]) {
        let (_, py) = to_px(&[0.0, y]);
        pb.move_to(0.0, py.round() + 0.5);
        pb.line_to(w as f32, py.round() + 0.5);
    }
    if let Some(path) = pb.finish() {
        let mut p = paint(colors.grid);
        p.anti_alias = false;
        pix.stroke_path(&path, &p, &Stroke { width: k.round().max(1.0), ..Default::default() }, Transform::identity(), None);
    }
    let stroke = |width: f32| Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
    let (outline, path_stroke) = (stroke(1.25 * k), stroke(1.75 * k));
    // Points closer than this (pixels) to the last one drawn are skipped.
    let min_step = 0.35 * k;
    let add_path = |pb: &mut PathBuilder, pts: &[Pt], close: bool| {
        let mut last: Option<(f32, f32)> = None;
        for (i, p) in pts.iter().enumerate() {
            let (x, y) = to_px(p);
            match last {
                None => pb.move_to(x, y),
                Some((lx, ly)) if i + 1 < pts.len() && (x - lx).abs() < min_step && (y - ly).abs() < min_step => continue,
                Some(_) => pb.line_to(x, y),
            }
            last = Some((x, y));
        }
        if close {
            pb.close();
        }
    };
    let mut dots = Vec::new();
    // Polygons below, then lines, then points on top.
    for layer in 0..3 {
        for (part, b) in &data.parts {
            if b[2] < seen[0] || b[0] > seen[2] || b[3] < seen[1] || b[1] > seen[3] {
                continue;
            }
            let tiny = (b[2] - b[0]).max(b[3] - b[1]) * view.scale < 0.75 * k as f64;
            match (layer, part) {
                (0, Part::Polygon(_)) | (1, Part::Line(_)) if tiny => {
                    // Smaller than a pixel: a dot, so it does not vanish.
                    let (x, y) = to_px(&[b[0], b[1]]);
                    if let Some(r) = Rect::from_xywh(x - 0.5 * k, y - 0.5 * k, k, k) {
                        pix.fill_rect(r, &line, Transform::identity(), None);
                    }
                }
                (0, Part::Polygon(rings)) => {
                    let mut pb = PathBuilder::new();
                    for r in rings {
                        add_path(&mut pb, r, true);
                    }
                    if let Some(path) = pb.finish() {
                        pix.fill_path(&path, &fill, FillRule::EvenOdd, Transform::identity(), None);
                        pix.stroke_path(&path, &line, &outline, Transform::identity(), None);
                    }
                }
                (1, Part::Line(pts)) => {
                    let mut pb = PathBuilder::new();
                    add_path(&mut pb, pts, false);
                    if let Some(path) = pb.finish() {
                        pix.stroke_path(&path, &line, &path_stroke, Transform::identity(), None);
                    }
                }
                (2, Part::Point(p)) => dots.push(to_px(p)),
                _ => {}
            }
        }
    }
    // Many points: smaller dots.
    let r = if dots.len() > 20_000 { 1.5 * k } else { 3.5 * k };
    for (x, y) in dots {
        if let Some(c) = PathBuilder::from_circle(x, y, r + k) {
            pix.fill_path(&c, &halo, FillRule::Winding, Transform::identity(), None);
        }
        if let Some(c) = PathBuilder::from_circle(x, y, r) {
            pix.fill_path(&c, &point, FillRule::Winding, Transform::identity(), None);
        }
    }
    pix.take()
}

/// A round step for grid lines about `span / 6` apart (1, 2 or 5 times a power of ten).
fn grid_step(span: f64) -> f64 {
    let raw = span / 6.0;
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * p).find(|s| *s >= raw).unwrap_or(10.0 * p)
}

/// How many decimals show a coordinate to about one pixel.
fn decimals(units_per_px: f64) -> usize {
    if !(units_per_px.is_finite() && units_per_px > 0.0) {
        return 2;
    }
    (-units_per_px.log10()).ceil().clamp(0.0, 10.0) as usize
}

/// `a` moved `f` of the way towards `b`, opaque.
fn mix(a: Color32, b: Color32, f: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// An opaque colour as tiny-skia takes it.
fn rgba(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

pub struct GeoView {
    data: GeoData,
    source: Option<(Vec<Styled>, Color32)>,
    show_source: bool,
    /// 1 = the whole drawing fits.
    zoom: f64,
    center: Pt,
    texture: Option<egui::TextureHandle>,
    drawn: Option<Drawn>,
}

/// What the picture was drawn for: pixel size, centre and zoom, full quality, colours.
type Drawn = ([usize; 2], [f64; 3], bool, [[u8; 4]; 2]);

impl GeoView {
    pub fn new(data: GeoData, source: Option<(Vec<Styled>, Color32)>) -> Self {
        let center = Self::home(&data);
        Self { data, source, show_source: false, zoom: 1.0, center, texture: None, drawn: None }
    }

    fn home(data: &GeoData) -> Pt {
        [(data.bbox[0] + data.bbox[2]) / 2.0, (data.bbox[1] + data.bbox[3]) / 2.0]
    }

    fn reset(&mut self) {
        self.zoom = 1.0;
        self.center = Self::home(&self.data);
    }

    #[cfg(test)]
    pub fn describe(&self) -> String {
        let d = &self.data;
        format!("{} {:?} features={:?} vertices={} bbox={:?} srid={:?}", d.format, d.kinds, d.features, d.vertices, d.bbox, d.srid)
    }

    fn facts(&self) -> String {
        let d = &self.data;
        let mut parts = vec![d.format.to_string()];
        if let Some(n) = d.features {
            parts.push(format!("{} {}", crate::util::fmt_count(n as u64), if n == 1 { t("feature") } else { t("features") }));
        }
        let kinds: Vec<String> = d.kinds.iter().map(|(k, n)| format!("{} {k}", crate::util::fmt_count(*n as u64))).collect();
        if !kinds.is_empty() {
            parts.push(kinds.join(", "));
        }
        parts.push(format!("{} {}", crate::util::fmt_count(d.vertices as u64), t("vertices")));
        if let Some(s) = d.srid {
            parts.push(format!("SRID {s}"));
        }
        if d.truncated {
            parts.push(t("(truncated)"));
        }
        parts.join(" · ")
    }

    fn bbox_text(&self) -> String {
        let b = self.data.bbox;
        let d = decimals(((b[2] - b[0]).max(b[3] - b[1]) / 1000.0).max(f64::MIN_POSITIVE));
        format!("x {:.d$} … {:.d$}   y {:.d$} … {:.d$}", b[0], b[2], b[1], b[3])
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, name: &str, key: &str, th: &Theme) {
        // The extent goes with the facts: it is in their tooltip when they are cut.
        let facts = format!("{} · {}", self.facts(), self.bbox_text());
        crate::preview::header_row(ui, th, name, &[(facts, th.weak)], |ui| {
            if self.source.is_some() && ui.selectable_label(self.show_source, t("Source")).clicked() {
                self.show_source = !self.show_source;
            }
            if !self.show_source && ui.button(t("Reset view")).clicked() {
                self.reset();
            }
        });
        ui.separator();
        if let (true, Some((lines, bg))) = (self.show_source, &self.source) {
            // Its scroll position belongs to this file.
            ui.push_id(("source", key), |ui| crate::preview::code_view(ui, lines, *bg, false, th));
            return;
        }
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return;
        }
        let fit = fit_scale(self.data.bbox, rect.width() as f64, rect.height() as f64);
        let scale = fit * self.zoom;
        let to_data = |p: egui::Pos2, center: Pt, scale: f64| [center[0] + (p.x - rect.center().x) as f64 / scale, center[1] - (p.y - rect.center().y) as f64 / scale];
        let dragging = resp.dragged();
        if dragging {
            let d = resp.drag_delta();
            self.center[0] -= d.x as f64 / scale;
            self.center[1] += d.y as f64 / scale;
        }
        if resp.double_clicked() {
            self.reset();
        }
        let (wheel, pinch) = if resp.hovered() { ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta())) } else { (0.0, 1.0) };
        let factor = (wheel as f64 * 0.003).exp() * pinch as f64;
        if factor != 1.0 {
            let zoom = (self.zoom * factor).clamp(*ZOOM.start(), *ZOOM.end());
            // The point under the pointer stays where it is.
            if let Some(at) = resp.hover_pos() {
                let p = to_data(at, self.center, scale);
                let s = fit * zoom;
                self.center = [p[0] - (at.x - rect.center().x) as f64 / s, p[1] + (at.y - rect.center().y) as f64 / s];
            }
            self.zoom = zoom;
        }
        let scale = fit * self.zoom;
        let moving = dragging || factor != 1.0;
        if moving {
            // One more frame after it stops, at full quality.
            ui.ctx().request_repaint();
        }
        let painter = ui.painter_at(rect);
        // Full quality (twice the pixels) when still.
        let ppp = ui.ctx().pixels_per_point();
        let quality = if moving { 1.0 } else { 2.0 };
        let max_side = ui.input(|i| i.max_texture_side).clamp(1, 3200);
        let px = |v: f32| ((v * ppp * quality) as usize).clamp(1, max_side);
        let size = [px(rect.width()), px(rect.height())];
        let k = size[0] as f32 / rect.width();
        let colors = Colors {
            bg: rgba(th.bg),
            grid: rgba(mix(th.bg, th.weak, 0.2)),
            line: rgba(th.accent),
            fill: [th.accent.r(), th.accent.g(), th.accent.b(), if th.dark { 64 } else { 72 }],
            point: rgba(th.marked),
            halo: rgba(th.bg),
        };
        let state = (size, [self.center[0], self.center[1], self.zoom], !moving, [colors.line, colors.bg]);
        if self.drawn != Some(state) {
            let view = View { center: self.center, scale: scale * k as f64 };
            let pixels = render(&self.data, view, size[0], size[1], &colors, k);
            let image = egui::ColorImage::from_rgba_premultiplied(size, &pixels);
            match &mut self.texture {
                Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                None => self.texture = Some(crate::preview::upload_texture(ui.ctx(), format!("geo:{key}"), image)),
            }
            self.drawn = Some(state);
        }
        if let Some(tex) = &self.texture {
            painter.image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        let font = egui::FontId::proportional(11.5);
        if let Some(at) = resp.hover_pos() {
            let p = to_data(at, self.center, scale);
            let d = decimals(1.0 / scale);
            painter.text(rect.right_bottom() + egui::vec2(-6.0, -6.0), egui::Align2::RIGHT_BOTTOM, format!("{:.d$}, {:.d$}", p[0], p[1]), font.clone(), th.text);
        }
        let hint = resp.on_hover_cursor(if dragging { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        painter.text(hint.rect.left_bottom() + egui::vec2(6.0, -6.0), egui::Align2::LEFT_BOTTOM, t("Drag to move · scroll to zoom · double-click to reset"), font, th.weak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square with a square hole, ten wide.
    const SQUARE_WITH_HOLE: &str = "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (3 3, 7 3, 7 7, 3 7, 3 3))";

    fn wkb_le_polygon() -> Vec<u8> {
        let mut b = vec![1];
        b.extend(3u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(4u32.to_le_bytes());
        for (x, y) in [(0.0f64, 0.0f64), (4.0, 0.0), (0.0, 2.0), (0.0, 0.0)] {
            b.extend(x.to_le_bytes());
            b.extend(y.to_le_bytes());
        }
        b
    }

    #[test]
    fn shapefiles_that_claim_too_much_are_refused() {
        let mut v = vec![0u8; 100];
        v[..4].copy_from_slice(&[0, 0, 0x27, 0x0A]);
        let mut content = 5i32.to_le_bytes().to_vec();
        content.extend([0u8; 32]);
        content.extend(i32::MAX.to_le_bytes());
        content.extend(4i32.to_le_bytes());
        v.extend(1i32.to_be_bytes());
        v.extend(((content.len() / 2) as i32).to_be_bytes());
        v.extend(content);
        assert!(load("a.shp", &v).is_err());
        // And a good one: a point.
        let mut v = v[..100].to_vec();
        let mut content = 1i32.to_le_bytes().to_vec();
        content.extend(3.0f64.to_le_bytes());
        content.extend(4.0f64.to_le_bytes());
        v.extend(1i32.to_be_bytes());
        v.extend(((content.len() / 2) as i32).to_be_bytes());
        v.extend(content);
        let d = load("a.shp", &v).unwrap();
        assert_eq!((d.kinds.clone(), d.bbox), (vec![("Point", 1)], [3.0, 4.0, 3.0, 4.0]));
    }

    #[test]
    fn extensions() {
        for n in ["a.geojson", "a.GeoJSON", "a.wkt", "a.WKB", "a.geojsonl"] {
            assert!(is_geo(n), "{n}");
        }
        assert!(!is_geo("a.json"));
        assert_eq!(source_language("a.geojson"), "json");
        assert_eq!(source_language("a.wkt"), "txt");
    }

    #[test]
    fn geojson_feature_collection() {
        let text = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"name":"a","nested":{"x":[1,2,3]}},"geometry":{"type":"Polygon","coordinates":[[[0,0],[4,0],[4,3],[0,0]]]}},
            {"type":"Feature","properties":null,"geometry":null},
            {"type":"Feature","geometry":{"type":"MultiLineString","coordinates":[[[1,1,9],[2,2,9]],[[5,5],[6,-1]]]}},
            {"type":"Feature","geometry":{"type":"GeometryCollection","geometries":[{"type":"Point","coordinates":[-2,1]},{"type":"MultiPoint","coordinates":[[1,7],[2,8]]}]}}
        ]}"#;
        let g = load("a.geojson", text.as_bytes()).unwrap();
        assert_eq!(g.format, "GeoJSON");
        assert_eq!(g.features, Some(4));
        assert_eq!(g.kinds, vec![("Polygon", 1), ("MultiLineString", 1), ("GeometryCollection", 1)]);
        assert_eq!(g.vertices, 4 + 4 + 3);
        assert_eq!(g.bbox, [-2.0, -1.0, 6.0, 8.0]);
        // Bare geometries, and positions with a height.
        let g = load("a.geojson", br#"{"type":"MultiPolygon","coordinates":[[[[0,0,1],[1,0,1],[1,1,1],[0,0,1]]],[[[5,5],[6,5],[6,6],[5,5]]]]}"#).unwrap();
        assert_eq!((g.features, g.kinds.clone(), g.vertices), (Some(0), vec![("MultiPolygon", 1)], 8));
    }

    #[test]
    fn geojson_errors_and_sequences() {
        for bad in [&br#"{"type":"Polygon","coordinates":[[0,0],[1,1]]}"#[..], br#"{"type":"Point","coordinates":[1]}"#, br#"{"type":"Circle","coordinates":[1,2]}"#, b"[1, 2", b"{}"] {
            assert!(load("a.geojson", bad).err().unwrap().starts_with("Not valid GeoJSON:"), "{}", String::from_utf8_lossy(bad));
        }
        assert_eq!(load("a.geojson", br#"{"type":"FeatureCollection","features":[]}"#).err().unwrap(), "This file has no geometry to show.");
        let seq = "\u{1e}{\"type\":\"Point\",\"coordinates\":[1,2]}\n\n{\"type\":\"Feature\",\"geometry\":{\"type\":\"LineString\",\"coordinates\":[[0,0],[3,4]]}}\n";
        let g = load("a.geojsonl", seq.as_bytes()).unwrap();
        assert_eq!((g.features, g.vertices, g.bbox), (Some(1), 3, [0.0, 0.0, 3.0, 4.0]));
        // Deep nesting is refused, not a crash.
        let deep = format!("{}{{\"type\":\"Point\",\"coordinates\":[0,0]}}{}", "{\"type\":\"GeometryCollection\",\"geometries\":[".repeat(100), "]}".repeat(100));
        assert!(load("a.geojson", deep.as_bytes()).is_err());
    }

    #[test]
    fn wkt_of_every_kind() {
        let g = load("a.wkt", SQUARE_WITH_HOLE.as_bytes()).unwrap();
        assert_eq!((g.format, g.kinds.clone(), g.vertices, g.bbox), ("WKT", vec![("Polygon", 1)], 10, [0.0, 0.0, 10.0, 10.0]));
        let text = "SRID=4326;MULTIPOINT ((1 2), 3 4)\npoint z (1 2 3)\nLINESTRING M (0 0 1, -5 2.5e1 2)\n\
            MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), EMPTY, ((5 5, 6 5, 6 6, 5 5)))\n\
            GEOMETRYCOLLECTION (POINT (9 9), LINESTRING (0 0, 1 1), GEOMETRYCOLLECTION EMPTY)\nPOINT EMPTY\nPOLYGONZM ((0 0 0 0, 1 0 0 0, 1 1 0 0, 0 0 0 0))";
        let g = load("a.wkt", text.as_bytes()).unwrap();
        assert_eq!(g.srid, Some(4326));
        assert_eq!(g.kinds, vec![("MultiPoint", 1), ("Point", 2), ("LineString", 1), ("MultiPolygon", 1), ("GeometryCollection", 1), ("Polygon", 1)]);
        assert_eq!(g.vertices, 2 + 1 + 2 + 8 + 3 + 4);
        assert_eq!(g.bbox, [-5.0, 0.0, 9.0, 25.0]);
        for bad in ["POLYGON ((0 0, 1 1)", "CIRCULARSTRING (0 0, 1 1, 2 0)", "POINT (1)", "(1 2)"] {
            assert!(load("a.wkt", bad.as_bytes()).err().unwrap().starts_with("Not valid WKT:"), "{bad}");
        }
        assert_eq!(load("a.wkt", b"POINT EMPTY").err().unwrap(), "This file has no geometry to show.");
        let deep = format!("{}POINT (0 0){}", "GEOMETRYCOLLECTION (".repeat(200), ")".repeat(200));
        assert!(load("a.wkt", deep.as_bytes()).is_err());
    }

    #[test]
    fn wkb_binary_hex_and_ewkb() {
        let g = load("a.wkb", &wkb_le_polygon()).unwrap();
        assert_eq!((g.format, g.kinds.clone(), g.vertices, g.bbox), ("WKB", vec![("Polygon", 1)], 4, [0.0, 0.0, 4.0, 2.0]));
        // Big endian EWKB: a MultiPoint with Z and an SRID, then (concatenated) a 3D ISO line.
        let mut b = vec![0];
        b.extend((0x8000_0000u32 | 0x2000_0000 | 4).to_be_bytes());
        b.extend(3857u32.to_be_bytes());
        b.extend(2u32.to_be_bytes());
        for (x, y) in [(1.0f64, 2.0f64), (3.0, -4.0)] {
            b.push(0);
            b.extend(0x8000_0001u32.to_be_bytes());
            for v in [x, y, 7.0] {
                b.extend(v.to_be_bytes());
            }
        }
        b.push(1);
        b.extend(1002u32.to_le_bytes());
        b.extend(2u32.to_le_bytes());
        for v in [10.0f64, 10.0, 0.0, 11.0, 12.0, 0.0] {
            b.extend(v.to_le_bytes());
        }
        let g = load("a.wkb", &b).unwrap();
        assert_eq!((g.srid, g.kinds.clone(), g.vertices, g.bbox), (Some(3857), vec![("MultiPoint", 1), ("LineString", 1)], 4, [1.0, -4.0, 11.0, 12.0]));
        // Hex, one per line, as databases export it.
        let hex: String = wkb_le_polygon().iter().map(|x| format!("{x:02X}")).collect();
        let g = load("a.wkb", format!("{hex}\n\\x{}\n", hex.to_lowercase()).as_bytes()).unwrap();
        assert_eq!((g.format, g.kinds.clone(), g.vertices), ("WKB (hex)", vec![("Polygon", 2)], 8));
    }

    #[test]
    fn wkb_that_lies_is_refused() {
        // Claims four billion points: refused before allocating them.
        let mut b = vec![1];
        b.extend(2u32.to_le_bytes());
        b.extend(u32::MAX.to_le_bytes());
        assert!(load("a.wkb", &b).err().unwrap().starts_with("Not valid WKB:"));
        let mut cut = wkb_le_polygon();
        cut.truncate(cut.len() - 3);
        assert!(load("a.wkb", &cut).is_err());
        assert!(load("a.wkb", &[7, 1, 0, 0, 0]).is_err());
        let mut odd = vec![1];
        odd.extend(42u32.to_le_bytes());
        assert!(load("a.wkb", &odd).is_err());
        // Collections inside collections, very deep.
        let mut deep = Vec::new();
        for _ in 0..200 {
            deep.push(1);
            deep.extend(7u32.to_le_bytes());
            deep.extend(1u32.to_le_bytes());
        }
        assert!(load("a.wkb", &deep).is_err());
    }

    fn pixel(px: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    const COLORS: Colors = Colors { bg: [0; 4], grid: [0; 4], line: [255, 0, 0, 255], fill: [0, 0, 255, 128], point: [0, 255, 0, 255], halo: [0, 0, 0, 255] };

    #[test]
    fn polygons_are_filled_around_their_holes() {
        let g = load("a.wkt", SQUARE_WITH_HOLE.as_bytes()).unwrap();
        let (w, h) = (200, 100);
        let view = View { center: [5.0, 5.0], scale: fit_scale(g.bbox, w as f64, h as f64) };
        let px = render(&g, view, w, h, &COLORS, 1.0);
        // Filled between the outline and the hole, empty in the hole and outside.
        assert!(pixel(&px, w, 100, 50 - 35)[2] > 0, "filled near the top");
        assert_eq!(pixel(&px, w, 100, 50)[3], 0, "the hole is empty");
        assert_eq!(pixel(&px, w, 5, 50)[3], 0, "outside is empty");
        // y goes up: the bottom edge (y = 0) is drawn low in the picture.
        assert!(pixel(&px, w, 100, 50 + 45)[0] > 0, "outline at the bottom");
    }

    #[test]
    fn zoomed_far_in_and_points() {
        let g = load("a.wkt", SQUARE_WITH_HOLE.as_bytes()).unwrap();
        let (w, h) = (64, 64);
        // A ten-unit polygon some hundred thousand pixels across: its filled part still shows.
        let view = View { center: [1.5, 1.5], scale: fit_scale(g.bbox, w as f64, h as f64) * ZOOM.end() / 10.0 };
        let px = render(&g, view, w, h, &COLORS, 1.0);
        assert!(pixel(&px, w, 32, 32)[2] > 0);
        // Out of sight: nothing drawn.
        let view = View { center: [500.0, 500.0], scale: 10.0 };
        assert!(render(&g, view, w, h, &COLORS, 1.0).iter().all(|b| *b == 0));
        let g = load("a.wkt", b"POINT (3 4)").unwrap();
        let view = View { center: GeoView::home(&g), scale: fit_scale(g.bbox, 64.0, 64.0) };
        assert_eq!(pixel(&render(&g, view, w, h, &COLORS, 1.0), w, 32, 32)[1], 255);
    }

    #[test]
    fn drawn_opaque_on_a_grid() {
        let g = load("a.wkt", b"LINESTRING (0 0, 60 0)").unwrap();
        let colors = Colors { bg: [255, 255, 255, 255], grid: [200, 200, 200, 255], ..COLORS };
        // Ten pixels per unit, the middle at (0, 0): grid lines every 10 units, so every 100 pixels.
        let (w, h) = (300, 200);
        let px = render(&g, View { center: [0.0, 0.0], scale: 10.0 }, w, h, &colors, 1.0);
        assert!(px.chunks(4).all(|p| p[3] == 255), "opaque");
        assert_eq!(pixel(&px, w, 50, 20), [200, 200, 200, 255], "the line x = -10");
        assert_eq!(pixel(&px, w, 20, 20), [255, 255, 255, 255]);
        assert_eq!(pixel(&px, w, 200, 100)[0], 255, "the red line on the x axis");
    }

    #[test]
    fn grid_and_number_helpers() {
        assert_eq!(grid_step(60.0), 10.0);
        assert!((grid_step(0.09) - 0.02).abs() < 1e-12);
        assert_eq!(grid_step(0.0), 1.0);
        assert_eq!(decimals(0.001), 3);
        assert_eq!(decimals(250.0), 0);
        assert_eq!(fit_scale([1.0, 1.0, 1.0, 1.0], 100.0, 100.0), 1.0);
        assert_eq!(fit_scale([0.0, 0.0, 10.0, 0.0], 100.0, 50.0), 9.0);
    }
}
