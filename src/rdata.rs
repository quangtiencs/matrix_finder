//! R data files: `.RData`/`.rda` (the named objects of `save()`) and `.rds` (the one
//! object of `saveRDS()`), plain or packed with gzip, bzip2, xz or zstd. The R
//! serialization format is read as far as needed to list each object (class, type,
//! size) and to show data frames, matrices and vectors as tables. Nothing in the file
//! is ever run: functions, environments and byte code are only stepped over.
use crate::i18n::t;
use crate::table::{clip_cell, finish_sheet, fmt_count, Cell, Sheet, TableData, MAX_COLS, MAX_ROWS};
use crate::util::{es, Res};
use std::io::{BufRead, BufReader, Read};
use std::time::{Duration, Instant};

/// Most bytes of the unpacked stream read, and the longest time spent: past either, a
/// workspace lists the objects before that point.
const MAX_STREAM: u64 = 1024 * 1024 * 1024;
const TIME_LIMIT: Duration = Duration::from_secs(10);
/// Memory for kept values and objects, for the whole file.
const MAX_KEPT: usize = 128 * 1024 * 1024;
const MAX_DEPTH: usize = 1000;
/// Bytes kept of one string.
const MAX_STRING: u64 = 1024;
/// Values kept of an object at the top (a matrix needs more than its first rows), and of an attribute.
const TOP_VALUES: usize = MAX_ROWS * MAX_COLS;
const ATTR_VALUES: usize = 100_000;
/// Objects of a workspace with a tab of their own.
const MAX_SHEETS: usize = 40;

// R's types (`SEXPTYPE`) and the serialization's own codes.
const NILSXP: u8 = 0;
const SYMSXP: u8 = 1;
const LISTSXP: u8 = 2;
const CLOSXP: u8 = 3;
const ENVSXP: u8 = 4;
const PROMSXP: u8 = 5;
const LANGSXP: u8 = 6;
const SPECIALSXP: u8 = 7;
const BUILTINSXP: u8 = 8;
const CHARSXP: u8 = 9;
const LGLSXP: u8 = 10;
const INTSXP: u8 = 13;
const REALSXP: u8 = 14;
const CPLXSXP: u8 = 15;
const STRSXP: u8 = 16;
const DOTSXP: u8 = 17;
const VECSXP: u8 = 19;
const EXPRSXP: u8 = 20;
const BCODESXP: u8 = 21;
const EXTPTRSXP: u8 = 22;
const WEAKREFSXP: u8 = 23;
const RAWSXP: u8 = 24;
const S4SXP: u8 = 25;
const ALTREP_SXP: u8 = 238;
const ATTRLISTSXP: u8 = 239;
const ATTRLANGSXP: u8 = 240;
const BASEENV_SXP: u8 = 241;
const EMPTYENV_SXP: u8 = 242;
const BCREPREF: u8 = 243;
const BCREPDEF: u8 = 244;
const PERSISTSXP: u8 = 247;
const PACKAGESXP: u8 = 248;
const NAMESPACESXP: u8 = 249;
const BASENAMESPACE_SXP: u8 = 250;
const MISSINGARG_SXP: u8 = 251;
const UNBOUNDVALUE_SXP: u8 = 252;
const GLOBALENV_SXP: u8 = 253;
const NILVALUE_SXP: u8 = 254;
const REFSXP: u8 = 255;

const HAS_ATTR: i32 = 1 << 9;
const HAS_TAG: i32 = 1 << 10;
const LATIN1: i32 = 1 << 2;
const NA_INT: i32 = i32::MIN;

fn is_pairlist(ty: u8) -> bool {
    matches!(ty, LISTSXP | LANGSXP | CLOSXP | PROMSXP | DOTSXP)
}

/// R's `NA` for doubles is a NaN with 1954 in its low word.
fn is_na_real(x: f64) -> bool {
    x.is_nan() && x.to_bits() & 0xFFFF_FFFF == 1954
}

fn bad() -> Stop {
    Stop::Bad(t("Not a valid R data file."))
}

/// Why reading stopped: a limit (what was read so far can be shown), or a bad file.
enum Stop {
    Limit,
    Bad(String),
}

impl From<std::io::Error> for Stop {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof { Stop::Bad(t("This file is cut short.")) } else { Stop::Bad(e.to_string()) }
    }
}

type R<T> = Result<T, Stop>;

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Xdr,
    Native,
    Ascii,
}

#[derive(Default)]
enum Vals {
    #[default]
    None,
    /// Integers and logicals.
    Int(Vec<i32>),
    Real(Vec<f64>),
    Cplx(Vec<(f64, f64)>),
    Str(Vec<Option<String>>),
    Raw(Vec<u8>),
}

/// What is kept of an R object: its type, length, the first values, the elements of a
/// list and the attributes (`names`, `dim`, `class`, `levels`…), as far as needed.
#[derive(Default)]
struct Obj {
    ty: u8,
    len: u64,
    vals: Vals,
    items: Vec<(Option<String>, Obj)>,
    attrs: Vec<(String, Obj)>,
    /// A symbol's or string's text.
    name: Option<String>,
    /// The class of an ALTREP object that is not read.
    alt: Option<String>,
}

/// How much of an object to keep, by how deep it is: the objects themselves (level 0)
/// with many values, their elements (level 1, data frame columns) with a page of them,
/// and nothing deeper. Attributes of the first two levels keep theirs.
#[derive(Clone, Copy)]
struct Keep {
    level: i8,
    attr: bool,
}

impl Keep {
    const TOP: Keep = Keep { level: 0, attr: false };
    const ELEMENT: Keep = Keep { level: 1, attr: false };
    const NOTHING: Keep = Keep { level: 9, attr: false };

    fn values(self) -> usize {
        match (self.level, self.attr) {
            (..=1, true) => ATTR_VALUES,
            (..=0, false) => TOP_VALUES,
            (1, false) => MAX_ROWS,
            _ => 0,
        }
    }

    fn child(self) -> Keep {
        if self.attr { self } else { Keep { level: (self.level + 1).min(9), attr: false } }
    }

    fn attrs(self) -> Keep {
        if self.attr { Keep { level: 9, attr: true } } else { Keep { level: self.level, attr: true } }
    }

    fn items(self) -> usize {
        if self.level <= 1 { MAX_ROWS } else { 0 }
    }
}

enum Ref {
    Sym(String),
    Other(u8),
}

struct In<B> {
    r: B,
    format: Format,
    read: u64,
    started: Instant,
    refs: Vec<Ref>,
    kept: usize,
}

impl<B: BufRead> In<B> {
    fn count(&mut self, n: u64) -> R<()> {
        let before = self.read;
        self.read = self.read.saturating_add(n);
        if self.read > MAX_STREAM {
            return Err(Stop::Limit);
        }
        if before >> 20 != self.read >> 20 && self.started.elapsed() > TIME_LIMIT {
            return Err(Stop::Limit);
        }
        Ok(())
    }

    fn fill(&mut self, buf: &mut [u8]) -> R<()> {
        self.r.read_exact(buf)?;
        self.count(buf.len() as u64)
    }

    fn byte(&mut self) -> R<u8> {
        let mut b = [0u8];
        self.fill(&mut b)?;
        Ok(b[0])
    }

    fn peek(&mut self) -> R<Option<u8>> {
        Ok(self.r.fill_buf()?.first().copied())
    }

    fn skip(&mut self, n: u64) -> R<()> {
        if self.read.saturating_add(n) > MAX_STREAM {
            return Err(Stop::Limit);
        }
        let copied = std::io::copy(&mut (&mut self.r).take(n), &mut std::io::sink())?;
        if copied < n {
            return Err(Stop::Bad(t("This file is cut short.")));
        }
        self.count(n)
    }

    /// An ASCII file's next word.
    fn word(&mut self) -> R<String> {
        let mut w = vec![];
        loop {
            let b = self.byte()?;
            if b.is_ascii_whitespace() {
                if w.is_empty() {
                    continue;
                }
                break;
            }
            w.push(b);
            if w.len() > 1024 {
                return Err(bad());
            }
        }
        Ok(String::from_utf8_lossy(&w).into_owned())
    }

    fn int(&mut self) -> R<i32> {
        let mut b = [0u8; 4];
        match self.format {
            Format::Xdr => {
                self.fill(&mut b)?;
                Ok(i32::from_be_bytes(b))
            }
            Format::Native => {
                self.fill(&mut b)?;
                Ok(i32::from_le_bytes(b))
            }
            Format::Ascii => match self.word()?.as_str() {
                "NA" => Ok(NA_INT),
                w => w.parse().map_err(|_| bad()),
            },
        }
    }

    fn double(&mut self) -> R<f64> {
        let mut b = [0u8; 8];
        match self.format {
            Format::Xdr => {
                self.fill(&mut b)?;
                Ok(f64::from_be_bytes(b))
            }
            Format::Native => {
                self.fill(&mut b)?;
                Ok(f64::from_le_bytes(b))
            }
            Format::Ascii => Ok(match self.word()?.as_str() {
                "NA" => f64::from_bits(0x7FF0_0000_0000_07A2),
                "NaN" => f64::NAN,
                "Inf" => f64::INFINITY,
                "-Inf" => f64::NEG_INFINITY,
                w => w.parse().map_err(|_| bad())?,
            }),
        }
    }

    /// A vector's length: an int, or for long vectors -1 and two more.
    fn length(&mut self) -> R<u64> {
        match self.int()? {
            -1 => {
                let hi = self.int()? as u32 as u64;
                let lo = self.int()? as u32 as u64;
                Ok(hi << 32 | lo)
            }
            n if n < 0 => Err(bad()),
            n => Ok(n as u64),
        }
    }

    /// How many of `want` values may still be kept, at `size` bytes each.
    fn allow(&self, want: usize, n: u64) -> usize {
        if self.kept >= MAX_KEPT { 0 } else { want.min(n as usize).min((MAX_KEPT - self.kept) / 8) }
    }

    /// Whether one more object may be kept in a list.
    fn retain(&mut self) -> bool {
        if self.kept >= MAX_KEPT {
            return false;
        }
        self.kept += 128;
        true
    }

    /// Step over `n` items of `size` bytes (`words` words each in an ASCII file).
    fn skip_items(&mut self, n: u64, size: u64, words: u64) -> R<()> {
        if self.format == Format::Ascii {
            for _ in 0..n.checked_mul(words).ok_or(Stop::Limit)? {
                self.word()?;
            }
            Ok(())
        } else {
            self.skip(n.checked_mul(size).ok_or(Stop::Limit)?)
        }
    }

    /// The bytes of a string of `len` bytes, of which the first `keep` are kept.
    fn string_bytes(&mut self, len: u64, keep: u64) -> R<Vec<u8>> {
        let keep = keep.min(len);
        let mut out = vec![0u8; keep as usize];
        if self.format != Format::Ascii {
            self.fill(&mut out)?;
            self.skip(len - keep)?;
            return Ok(out);
        }
        out.clear();
        while self.peek()?.is_some_and(|b| b.is_ascii_whitespace()) {
            self.byte()?;
        }
        for _ in 0..len {
            let mut c = self.byte()?;
            if c == b'\\' {
                c = match self.byte()? {
                    b'n' => b'\n',
                    b't' => b'\t',
                    b'v' => 0x0B,
                    b'b' => 0x08,
                    b'r' => b'\r',
                    b'f' => 0x0C,
                    b'a' => 0x07,
                    d @ b'0'..=b'7' => {
                        let mut v = u32::from(d - b'0');
                        for _ in 0..2 {
                            match self.peek()? {
                                Some(e @ b'0'..=b'7') => {
                                    self.byte()?;
                                    v = v * 8 + u32::from(e - b'0');
                                }
                                _ => break,
                            }
                        }
                        v as u8
                    }
                    other => other,
                };
            }
            if (out.len() as u64) < keep {
                out.push(c);
            }
        }
        Ok(out)
    }

    /// A `CHARSXP` after its flags: `None` for `NA`.
    fn chars(&mut self, levels: i32, keep: u64) -> R<Option<String>> {
        let len = self.int()?;
        if len == -1 {
            return Ok(None);
        }
        let len = u64::try_from(len).map_err(|_| bad())?;
        let b = self.string_bytes(len, keep)?;
        let mut s = if levels & LATIN1 != 0 { b.iter().map(|&c| char::from(c)).collect() } else { String::from_utf8_lossy(&b).into_owned() };
        if keep > 0 {
            if len > keep {
                s.push('…');
            }
            self.kept += s.len() + 24;
        }
        Ok(Some(s))
    }

    fn item(&mut self, depth: usize, keep: Keep) -> R<Obj> {
        let flags = self.int()?;
        self.item_with(flags, depth, keep)
    }

    fn item_with(&mut self, flags: i32, depth: usize, keep: Keep) -> R<Obj> {
        if depth > MAX_DEPTH {
            return Err(Stop::Bad(t("This R file is nested too deeply to preview.")));
        }
        let ty = (flags & 0xFF) as u8;
        let d = depth + 1;
        let mut obj = Obj { ty, ..Default::default() };
        match ty {
            NILVALUE_SXP => {
                obj.ty = NILSXP;
                return Ok(obj);
            }
            EMPTYENV_SXP | BASEENV_SXP | GLOBALENV_SXP | BASENAMESPACE_SXP => {
                obj.ty = ENVSXP;
                return Ok(obj);
            }
            UNBOUNDVALUE_SXP | MISSINGARG_SXP => return Ok(obj),
            REFSXP => {
                let i = if flags >> 8 == 0 { self.int()? } else { flags >> 8 };
                match usize::try_from(i).ok().and_then(|i| self.refs.get(i.checked_sub(1)?)) {
                    Some(Ref::Sym(name)) => {
                        obj.ty = SYMSXP;
                        obj.name = Some(name.clone());
                    }
                    Some(Ref::Other(t)) => obj.ty = *t,
                    None => return Err(bad()),
                }
                return Ok(obj);
            }
            PERSISTSXP | PACKAGESXP | NAMESPACESXP => {
                self.string_vec(d)?;
                self.refs.push(Ref::Other(ENVSXP));
                obj.ty = ENVSXP;
                return Ok(obj);
            }
            SYMSXP => {
                let name = self.item(d, Keep::NOTHING)?.name.unwrap_or_default();
                self.refs.push(Ref::Sym(name.clone()));
                obj.name = Some(name);
                return Ok(obj);
            }
            ENVSXP => {
                self.int()?; // locked
                self.refs.push(Ref::Other(ENVSXP));
                // Enclosure, frame, hash table and attributes.
                for _ in 0..4 {
                    self.item(d, Keep::NOTHING)?;
                }
                return Ok(obj);
            }
            ALTREP_SXP => return self.altrep(d, keep),
            t if is_pairlist(t) => return self.pairlist(flags, d, keep),
            _ => {}
        }
        match ty {
            EXTPTRSXP => {
                self.refs.push(Ref::Other(ty));
                self.item(d, Keep::NOTHING)?;
                self.item(d, Keep::NOTHING)?;
            }
            WEAKREFSXP => self.refs.push(Ref::Other(ty)),
            SPECIALSXP | BUILTINSXP => {
                let n = u64::try_from(self.int()?).map_err(|_| bad())?;
                obj.name = Some(String::from_utf8_lossy(&self.string_bytes(n, 64)?).into_owned());
            }
            // Strings are kept whatever the level: they are symbols' names.
            CHARSXP => obj.name = self.chars(flags >> 12, MAX_STRING)?,
            LGLSXP | INTSXP => {
                obj.len = self.length()?;
                let k = self.allow(keep.values(), obj.len);
                let mut v = Vec::with_capacity(k.min(1 << 16));
                for _ in 0..k {
                    v.push(self.int()?);
                }
                self.kept += k * 4;
                self.skip_items(obj.len - k as u64, 4, 1)?;
                obj.vals = Vals::Int(v);
            }
            REALSXP => {
                obj.len = self.length()?;
                let k = self.allow(keep.values(), obj.len);
                let mut v = Vec::with_capacity(k.min(1 << 16));
                for _ in 0..k {
                    v.push(self.double()?);
                }
                self.kept += k * 8;
                self.skip_items(obj.len - k as u64, 8, 1)?;
                obj.vals = Vals::Real(v);
            }
            CPLXSXP => {
                obj.len = self.length()?;
                let k = self.allow(keep.values(), obj.len);
                let mut v = Vec::with_capacity(k.min(1 << 16));
                for _ in 0..k {
                    v.push((self.double()?, self.double()?));
                }
                self.kept += k * 16;
                self.skip_items(obj.len - k as u64, 16, 2)?;
                obj.vals = Vals::Cplx(v);
            }
            RAWSXP => {
                obj.len = self.length()?;
                let k = self.allow(keep.values(), obj.len);
                let v = if self.format == Format::Ascii {
                    let mut v = Vec::with_capacity(k.min(1 << 16));
                    for _ in 0..k {
                        v.push(u8::from_str_radix(&self.word()?, 16).map_err(|_| bad())?);
                    }
                    v
                } else {
                    let mut v = vec![0u8; k];
                    self.fill(&mut v)?;
                    v
                };
                self.kept += k;
                self.skip_items(obj.len - k as u64, 1, 1)?;
                obj.vals = Vals::Raw(v);
            }
            STRSXP => {
                obj.len = self.length()?;
                let k = self.allow(keep.values(), obj.len);
                let mut v = Vec::with_capacity(k.min(1 << 16));
                for i in 0..obj.len {
                    let f = self.int()?;
                    let kept = (i as usize) < k;
                    if (f & 0xFF) as u8 == CHARSXP {
                        let s = self.chars(f >> 12, if kept { MAX_STRING } else { 0 })?;
                        if kept {
                            v.push(s);
                        }
                    } else {
                        let s = self.item_with(f, d, Keep::NOTHING)?;
                        if kept {
                            v.push(s.name);
                        }
                    }
                }
                obj.vals = Vals::Str(v);
            }
            VECSXP | EXPRSXP => {
                obj.len = self.length()?;
                for i in 0..obj.len {
                    let child = self.item(d, keep.child())?;
                    if (i as usize) < keep.items() && self.retain() {
                        obj.items.push((None, child));
                    }
                }
            }
            S4SXP => {}
            BCODESXP => self.bytecode(d)?,
            _ => return Err(bad()),
        }
        if flags & HAS_ATTR != 0 {
            obj.attrs = attributes(self.item(d, keep.attrs())?);
        }
        Ok(obj)
    }

    /// A pairlist, read along its tail without going deeper for each node.
    fn pairlist(&mut self, flags: i32, depth: usize, keep: Keep) -> R<Obj> {
        let mut obj = Obj { ty: (flags & 0xFF) as u8, ..Default::default() };
        let mut flags = flags;
        loop {
            if flags & HAS_ATTR != 0 {
                let a = self.item(depth, keep.attrs())?;
                if obj.len == 0 {
                    obj.attrs = attributes(a);
                }
            }
            let tag = if flags & HAS_TAG != 0 { self.item(depth, Keep::NOTHING)?.name } else { None };
            let car = self.item(depth, keep.child())?;
            if (obj.len as usize) < keep.items() && self.retain() {
                obj.items.push((tag, car));
            }
            obj.len += 1;
            flags = self.int()?;
            if !is_pairlist((flags & 0xFF) as u8) {
                self.item_with(flags, depth, Keep::NOTHING)?;
                return Ok(obj);
            }
        }
    }

    /// An object stored in a compact form: a sequence `1:n`, numbers to be turned into
    /// text, or a wrapper around a plain vector. Others are only named.
    fn altrep(&mut self, depth: usize, keep: Keep) -> R<Obj> {
        let info = self.item(depth, Keep { level: 0, attr: true })?;
        let class = info.items.first().and_then(|(_, o)| o.name.clone()).unwrap_or_default();
        let ty = match info.items.get(2).map(|(_, o)| &o.vals) {
            Some(Vals::Int(v)) => v.first().map_or(0, |t| *t as u8),
            _ => 0,
        };
        // A level up, so a wrapper's object is read as this one would be.
        let state = self.item(depth, Keep { level: keep.level - 1, attr: keep.attr })?;
        let attrs = attributes(self.item(depth, keep.attrs())?);
        let mut obj = match class.as_str() {
            "compact_intseq" | "compact_realseq" => {
                let info: Vec<f64> = match &state.vals {
                    Vals::Real(v) => v.clone(),
                    Vals::Int(v) => v.iter().map(|i| f64::from(*i)).collect(),
                    _ => vec![],
                };
                let (n, first, step) = match info[..] {
                    [n, first, step, ..] if n.is_finite() && n >= 0.0 => (n as u64, first, step),
                    _ => (0, 0.0, 0.0),
                };
                let k = self.allow(keep.values(), n) as u64;
                let seq = (0..k).map(|i| first + i as f64 * step);
                let vals = if class == "compact_intseq" { Vals::Int(seq.map(|x| x as i32).collect()) } else { Vals::Real(seq.collect()) };
                Obj { ty: if class == "compact_intseq" { INTSXP } else { REALSXP }, len: n, vals, ..Default::default() }
            }
            "deferred_string" => match state.items.into_iter().next() {
                Some((_, arg)) => Obj { ty: STRSXP, len: arg.len, vals: Vals::Str((0..arg.len.min(keep.values() as u64) as usize).map(|i| value_text(&arg, i)).collect()), ..Default::default() },
                None => Obj { ty: STRSXP, ..Default::default() },
            },
            c if c.starts_with("wrap_") => state.items.into_iter().next().map(|(_, o)| o).unwrap_or_default(),
            _ => Obj { ty, alt: Some(class), ..Default::default() },
        };
        if !attrs.is_empty() {
            obj.attrs = attrs;
        }
        Ok(obj)
    }

    fn string_vec(&mut self, depth: usize) -> R<()> {
        if self.int()? != 0 {
            return Err(bad());
        }
        for _ in 0..self.int()?.max(0) {
            self.item(depth, Keep::NOTHING)?;
        }
        Ok(())
    }

    /// Compiled code of a function: its instructions and constants.
    fn bytecode(&mut self, depth: usize) -> R<()> {
        self.int()?; // number of shared cells
        self.bytecode_body(depth)
    }

    fn bytecode_body(&mut self, depth: usize) -> R<()> {
        if depth > MAX_DEPTH {
            return Err(Stop::Bad(t("This R file is nested too deeply to preview.")));
        }
        self.item(depth + 1, Keep::NOTHING)?;
        for _ in 0..self.int()?.max(0) {
            let ty = self.int()?;
            match u8::try_from(ty).unwrap_or(0) {
                BCODESXP => self.bytecode_body(depth + 1)?,
                LANGSXP | LISTSXP | BCREPDEF | BCREPREF | ATTRLANGSXP | ATTRLISTSXP => self.bytecode_lang(ty, depth + 1)?,
                _ => drop(self.item(depth + 1, Keep::NOTHING)?),
            }
        }
        Ok(())
    }

    /// A call kept with byte code, where shared parts are written once.
    fn bytecode_lang(&mut self, ty: i32, depth: usize) -> R<()> {
        if depth > MAX_DEPTH {
            return Err(Stop::Bad(t("This R file is nested too deeply to preview.")));
        }
        let mut ty = ty;
        loop {
            match u8::try_from(ty).unwrap_or(0) {
                BCREPREF => {
                    self.int()?;
                    return Ok(());
                }
                code @ (BCREPDEF | LANGSXP | LISTSXP | ATTRLANGSXP | ATTRLISTSXP) => {
                    let mut code = code;
                    if code == BCREPDEF {
                        self.int()?; // position
                        code = u8::try_from(self.int()?).unwrap_or(0);
                    }
                    if matches!(code, ATTRLANGSXP | ATTRLISTSXP) {
                        self.item(depth, Keep::NOTHING)?;
                    }
                    self.item(depth, Keep::NOTHING)?; // tag
                    let car = self.int()?;
                    self.bytecode_lang(car, depth + 1)?;
                    ty = self.int()?; // the tail, in this loop
                }
                _ => {
                    self.item(depth, Keep::NOTHING)?;
                    return Ok(());
                }
            }
        }
    }
}

/// A pairlist of attributes as `(name, value)`.
fn attributes(o: Obj) -> Vec<(String, Obj)> {
    o.items.into_iter().filter_map(|(tag, v)| Some((tag?, v))).collect()
}

// ---------------------------------------------------------------- showing objects

impl Obj {
    fn attr(&self, name: &str) -> Option<&Obj> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    fn strings(&self) -> &[Option<String>] {
        match &self.vals {
            Vals::Str(v) => v,
            _ => &[],
        }
    }

    fn numbers(&self) -> Vec<f64> {
        match &self.vals {
            Vals::Int(v) => v.iter().map(|i| if *i == NA_INT { f64::NAN } else { f64::from(*i) }).collect(),
            Vals::Real(v) => v.clone(),
            _ => vec![],
        }
    }

    fn classes(&self) -> Vec<String> {
        self.attr("class").map(|c| c.strings().iter().flatten().cloned().collect()).unwrap_or_default()
    }

    fn inherits(&self, class: &str) -> bool {
        self.classes().iter().any(|c| c == class)
    }

    fn dims(&self) -> Option<Vec<u64>> {
        let d = self.attr("dim")?.numbers();
        (!d.is_empty() && d.iter().all(|x| x.is_finite() && *x >= 0.0)).then(|| d.iter().map(|x| *x as u64).collect())
    }

    fn is_data_frame(&self) -> bool {
        self.ty == VECSXP && self.inherits("data.frame")
    }

    /// Rows of a data frame: `row.names` is often stored as `c(NA, -n)`.
    fn nrow(&self) -> u64 {
        match self.attr("row.names") {
            Some(r) => match &r.vals {
                Vals::Int(v) if v.len() == 2 && v[0] == NA_INT => v[1].unsigned_abs().into(),
                _ => r.len,
            },
            None => self.items.first().map_or(0, |(_, c)| c.len),
        }
    }

    /// Element names: the `names` attribute, or a pairlist's tags.
    fn names(&self) -> Vec<Option<String>> {
        match self.attr("names") {
            Some(n) => n.strings().to_vec(),
            None => self.items.iter().map(|(tag, _)| tag.clone()).collect(),
        }
    }

    fn is_atomic(&self) -> bool {
        matches!(self.ty, LGLSXP | INTSXP | REALSXP | CPLXSXP | STRSXP | RAWSXP)
    }
}

/// `typeof()` in R.
fn type_name(o: &Obj) -> &'static str {
    match o.ty {
        NILSXP => "NULL",
        SYMSXP => "symbol",
        LISTSXP => "pairlist",
        CLOSXP => "closure",
        ENVSXP => "environment",
        PROMSXP => "promise",
        LANGSXP => "language",
        SPECIALSXP => "special",
        BUILTINSXP => "builtin",
        CHARSXP => "char",
        LGLSXP => "logical",
        INTSXP => "integer",
        REALSXP => "double",
        CPLXSXP => "complex",
        STRSXP => "character",
        DOTSXP => "...",
        VECSXP => "list",
        EXPRSXP => "expression",
        BCODESXP => "bytecode",
        EXTPTRSXP => "externalptr",
        WEAKREFSXP => "weakref",
        RAWSXP => "raw",
        S4SXP => "S4",
        _ => "?",
    }
}

/// `class()` in R, joined.
fn class_text(o: &Obj) -> String {
    let classes = o.classes();
    if !classes.is_empty() {
        return classes.join(", ");
    }
    match (o.dims().map(|d| d.len()), o.ty) {
        (Some(2), _) => "matrix".into(),
        (Some(_), _) => "array".into(),
        (_, CLOSXP | SPECIALSXP | BUILTINSXP) => "function".into(),
        (_, REALSXP) => "numeric".into(),
        (_, LANGSXP) => "call".into(),
        _ => type_name(o).into(),
    }
}

/// Rows × columns of a data frame, the dimensions of an array, a vector's length.
fn size_text(o: &Obj) -> String {
    if o.is_data_frame() {
        return format!("{} × {}", fmt_count(o.nrow()), fmt_count(o.len));
    }
    if let Some(d) = o.dims() {
        return d.iter().map(|n| fmt_count(*n)).collect::<Vec<_>>().join(" × ");
    }
    if o.alt.is_some() {
        return "?".into();
    }
    match o.ty {
        LGLSXP | INTSXP | REALSXP | CPLXSXP | STRSXP | RAWSXP | VECSXP | EXPRSXP | LISTSXP => fmt_count(o.len),
        _ => String::new(),
    }
}

fn real_text(x: f64) -> String {
    if is_na_real(x) {
        "NA".into()
    } else if x.is_nan() {
        "NaN".into()
    } else if x.is_infinite() {
        if x > 0.0 { "Inf".into() } else { "-Inf".into() }
    } else if x != 0.0 && (x.abs() >= 1e15 || x.abs() < 1e-5) {
        format!("{x:e}")
    } else {
        format!("{x}")
    }
}

/// One value of a vector as R would print it; `None` past what was kept.
fn value_text(o: &Obj, i: usize) -> Option<String> {
    let classes = o.classes();
    let has = |c: &str| classes.iter().any(|x| x == c);
    match &o.vals {
        Vals::Int(v) => {
            let x = *v.get(i)?;
            Some(if x == NA_INT {
                "NA".into()
            } else if o.ty == LGLSXP {
                if x != 0 { "TRUE".into() } else { "FALSE".into() }
            } else if has("factor") {
                let levels = o.attr("levels").map(|l| l.strings()).unwrap_or_default();
                match usize::try_from(x - 1).ok().and_then(|l| levels.get(l)) {
                    Some(Some(s)) => s.clone(),
                    _ => x.to_string(),
                }
            } else if has("Date") {
                date_text(f64::from(x))
            } else {
                x.to_string()
            })
        }
        Vals::Real(v) => {
            let x = *v.get(i)?;
            Some(if !x.is_finite() {
                real_text(x)
            } else if has("Date") {
                date_text(x)
            } else if has("POSIXct") {
                chrono::DateTime::from_timestamp(x.floor() as i64, 0).map_or_else(|| real_text(x), |d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            } else {
                real_text(x)
            })
        }
        Vals::Cplx(v) => v.get(i).map(|(re, im)| format!("{}{}{}i", real_text(*re), if *im < 0.0 { "-" } else { "+" }, real_text(im.abs()))),
        Vals::Str(v) => v.get(i).map(|s| s.clone().unwrap_or_else(|| "NA".into())),
        Vals::Raw(v) => v.get(i).map(|b| format!("{b:02x}")),
        Vals::None => None,
    }
}

fn date_text(days: f64) -> String {
    chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|d| d.checked_add_signed(chrono::Duration::try_days(days.floor() as i64)?))
        .map_or_else(|| real_text(days), |d| d.format("%Y-%m-%d").to_string())
}

/// The type shown above a column: R's names, which the table knows to align as numbers.
fn column_type(o: &Obj) -> String {
    let classes = o.classes();
    if let Some(c) = classes.iter().find(|c| matches!(c.as_str(), "factor" | "Date" | "POSIXct")) {
        return c.clone();
    }
    type_name(o).to_string()
}

/// A cell for something that is not a single value: `numeric [3]`.
fn summary_text(o: &Obj) -> String {
    let size = size_text(o);
    if size.is_empty() { class_text(o) } else { format!("{} [{size}]", class_text(o)) }
}

fn cell_of(o: &Obj, i: usize) -> Cell {
    if o.is_atomic() && o.dims().is_none() {
        value_text(o, i).map(clip_cell)
    } else {
        o.items.get(i).map(|(_, e)| summary_text(e))
    }
}

/// A list's elements: name, class, type and size of each.
fn listing_sheet(name: String, o: &Obj) -> Sheet {
    let names = o.names();
    let rows = o
        .items
        .iter()
        .enumerate()
        .take(MAX_ROWS)
        .map(|(i, (_, e))| vec![Some(names.get(i).cloned().flatten().unwrap_or_else(|| format!("[[{}]]", i + 1))), Some(class_text(e)), Some(type_name(e).to_string()), Some(size_text(e))])
        .collect();
    finish_sheet(name, vec![t("Name"), t("Class"), t("Type"), t("Dimensions")], Some(vec!["text".into(); 4]), rows, Some(o.len))
}

/// A tab for an object: a data frame's rows, a matrix, a vector, or a list's elements.
fn object_sheet(name: String, o: &Obj) -> Option<Sheet> {
    if o.is_data_frame() {
        let nrow = o.nrow();
        let names = o.names();
        let mut columns = vec![];
        let mut types = vec![];
        let row_names = o.attr("row.names").filter(|r| matches!(r.vals, Vals::Str(_)));
        if row_names.is_some() {
            columns.push(String::new());
            types.push("character".to_string());
        }
        let cols: Vec<&Obj> = o.items.iter().map(|(_, c)| c).take(MAX_COLS).collect();
        for (i, c) in cols.iter().enumerate() {
            columns.push(names.get(i).cloned().flatten().unwrap_or_else(|| format!("V{}", i + 1)));
            types.push(column_type(c));
        }
        let rows = (0..nrow.min(MAX_ROWS as u64) as usize)
            .map(|r| {
                let mut row: Vec<Cell> = vec![];
                if let Some(rn) = row_names {
                    row.push(value_text(rn, r));
                }
                row.extend(cols.iter().map(|c| cell_of(c, r)));
                row
            })
            .collect();
        return Some(finish_sheet(name, columns, Some(types), rows, Some(nrow)));
    }
    if o.is_atomic() {
        if let Some(dims) = o.dims() {
            // Column-major, as R stores it: rows are the first dimension.
            let nrow = dims.first().copied().unwrap_or(0);
            let per_row: u64 = dims.iter().skip(1).product();
            let shown_cols = per_row.min(MAX_COLS as u64) as usize;
            let dimnames = o.attr("dimnames").map(|d| &d.items);
            let names_of = |axis: usize| dimnames.and_then(|d| d.get(axis)).map(|(_, n)| n.strings()).filter(|n| !n.is_empty());
            let (row_names, col_names) = (names_of(0), if dims.len() == 2 { names_of(1) } else { None });
            let mut columns = vec![];
            let mut types = vec![];
            if row_names.is_some() {
                columns.push(String::new());
                types.push("character".to_string());
            }
            for c in 0..shown_cols {
                columns.push(col_names.and_then(|n| n.get(c).cloned().flatten()).unwrap_or_else(|| format!("[,{}]", c + 1)));
                types.push(column_type(o));
            }
            let rows = (0..nrow.min(MAX_ROWS as u64))
                .map(|r| {
                    let mut row: Vec<Cell> = vec![];
                    if let Some(n) = row_names {
                        row.push(n.get(r as usize).cloned().flatten());
                    }
                    for c in 0..shown_cols as u64 {
                        row.push(usize::try_from(r + c * nrow).ok().and_then(|i| value_text(o, i)).map(clip_cell));
                    }
                    row
                })
                .collect();
            return Some(finish_sheet(name, columns, Some(types), rows, Some(nrow)));
        }
        let names = o.attr("names").map(|n| n.strings());
        let mut columns = vec![];
        let mut types = vec![];
        if names.is_some() {
            columns.push(t("Name"));
            types.push("character".to_string());
        }
        columns.push(t("Value"));
        types.push(column_type(o));
        let rows = (0..o.len.min(MAX_ROWS as u64) as usize)
            .map(|i| {
                let mut row: Vec<Cell> = vec![];
                if let Some(n) = names {
                    row.push(n.get(i).cloned().flatten());
                }
                row.push(value_text(o, i).map(clip_cell));
                row
            })
            .collect();
        return Some(finish_sheet(name, columns, Some(types), rows, Some(o.len)));
    }
    if matches!(o.ty, VECSXP | EXPRSXP | LISTSXP) {
        return Some(listing_sheet(name, o));
    }
    None
}

/// The packed stream, unpacked.
fn unpacked<T: Read + 'static>(r: T) -> Res<BufReader<Box<dyn Read>>> {
    let mut r = BufReader::new(r);
    let head: Vec<u8> = r.fill_buf().map_err(es)?.iter().take(6).copied().collect();
    let inner: Box<dyn Read> = if head.starts_with(&[0x1F, 0x8B]) {
        Box::new(flate2::read::MultiGzDecoder::new(r))
    } else if head.starts_with(b"BZh") {
        Box::new(bzip2::read::MultiBzDecoder::new(r))
    } else if head.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0]) {
        Box::new(xz2::read::XzDecoder::new_multi_decoder(r))
    } else if head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        Box::new(zstd::stream::read::Decoder::with_buffer(r).map_err(es)?)
    } else {
        Box::new(r)
    };
    Ok(BufReader::with_capacity(256 * 1024, inner))
}

/// The objects of a workspace (a pairlist tagged with their names), as far as they can
/// be read. Past `MAX_ROWS` objects, only their number is kept.
fn read_workspace<B: BufRead>(input: &mut In<B>, objects: &mut Vec<(String, Obj)>, count: &mut u64) -> R<()> {
    let mut flags = input.int()?;
    while (flags & 0xFF) as u8 == LISTSXP {
        if flags & HAS_ATTR != 0 {
            input.item(1, Keep::NOTHING)?;
        }
        let name = if flags & HAS_TAG != 0 { input.item(1, Keep::NOTHING)?.name.unwrap_or_default() } else { String::new() };
        let keep = match objects.len() {
            n if n < MAX_SHEETS => Keep::TOP,
            n if n < MAX_ROWS => Keep::ELEMENT,
            _ => Keep::NOTHING,
        };
        let value = input.item(1, keep)?;
        if objects.len() < MAX_ROWS {
            objects.push((name, value));
        }
        *count += 1;
        flags = input.int()?;
    }
    Ok(())
}

pub fn rdata_from<T: Read + 'static>(r: T) -> Res<TableData> {
    let mut r = unpacked(r)?;
    // A workspace starts with a line of its own; a single object does not.
    let workspace = r.fill_buf().map_err(es)?.starts_with(b"RD");
    if workspace {
        let mut magic = [0u8; 5];
        r.read_exact(&mut magic).map_err(|_| t("Not a valid R data file."))?;
        if !matches!(&magic, b"RDX2\n" | b"RDX3\n" | b"RDA2\n" | b"RDA3\n" | b"RDB2\n" | b"RDB3\n") {
            return Err(t("This R file is in an old format that the preview does not read."));
        }
    }
    let mut line = [0u8; 2];
    r.read_exact(&mut line).map_err(|_| t("Not a valid R data file."))?;
    let format = match &line {
        b"X\n" => Format::Xdr,
        b"A\n" => Format::Ascii,
        b"B\n" => Format::Native,
        _ => return Err(t("Not a valid R data file.")),
    };
    let mut input = In { r, format, read: 0, started: Instant::now(), refs: vec![], kept: 0 };
    let message = |e: Stop| match e {
        Stop::Bad(e) => e,
        Stop::Limit => t("This R file is too large to preview."),
    };
    let version = input.int().map_err(message)?;
    let writer = input.int().map_err(message)?;
    input.int().map_err(message)?;
    match version {
        2 => {}
        3 => {
            let n = u64::try_from(input.int().map_err(message)?).map_err(|_| t("Not a valid R data file."))?;
            input.string_bytes(n, 0).map_err(message)?; // native encoding
        }
        _ => return Err(t("Not a valid R data file.")),
    }
    let r_version = format!("R {}.{}.{}", writer >> 16, (writer >> 8) & 0xFF, writer & 0xFF);
    if !workspace {
        let o = input.item(0, Keep::TOP).map_err(message)?;
        let mut info = vec![class_text(&o)];
        let size = size_text(&o);
        if !size.is_empty() {
            info.push(size);
        }
        info.push(r_version);
        let sheet = object_sheet(class_text(&o), &o).unwrap_or_else(|| contents_sheet(&[(String::new(), o)], 1, &[]));
        return Ok(TableData { format: "RDS", info, sheets: vec![sheet] });
    }
    let mut objects = vec![];
    let mut count = 0u64;
    let mut info = vec![];
    match read_workspace(&mut input, &mut objects, &mut count) {
        Ok(()) => {}
        Err(Stop::Bad(e)) if objects.is_empty() => return Err(e),
        Err(Stop::Limit) => info.push(t("Read in part: the file is too large")),
        Err(Stop::Bad(e)) => info.push(e),
    }
    info.insert(0, format!("{} {}", fmt_count(count), t("objects")));
    info.push(r_version);
    let mut sheets = vec![];
    let mut tabs = vec![];
    for (name, o) in objects.iter().take(MAX_SHEETS) {
        let sheet = object_sheet(name.clone(), o);
        tabs.push(sheet.is_some());
        sheets.extend(sheet);
    }
    sheets.insert(0, contents_sheet(&objects, count, &tabs));
    Ok(TableData { format: "RData", info, sheets })
}

fn contents_sheet(objects: &[(String, Obj)], count: u64, tabs: &[bool]) -> Sheet {
    let rows = objects
        .iter()
        .enumerate()
        .take(MAX_ROWS)
        .map(|(i, (name, o))| {
            let note = match (tabs.get(i), &o.alt) {
                (_, Some(class)) => format!("ALTREP {class}"),
                (Some(true), _) => format!("{} {name}", t("Tab:")),
                (Some(false), _) => t("This type is not shown"),
                (None, _) => String::new(),
            };
            vec![Some(clip_cell(name.clone())), Some(class_text(o)), Some(type_name(o).to_string()), Some(size_text(o)), Some(note)]
        })
        .collect();
    finish_sheet(t("Contents"), vec![t("Object"), t("Class"), t("Type"), t("Dimensions"), t("Note")], Some(vec!["text".into(); 5]), rows, Some(count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn cell(s: &Sheet, r: usize, c: usize) -> Option<&str> {
        s.rows[r][c].as_deref()
    }

    /// A small XDR writer for R objects.
    #[derive(Default)]
    struct W(Vec<u8>);

    impl W {
        fn int(&mut self, i: i32) -> &mut Self {
            self.0.extend_from_slice(&i.to_be_bytes());
            self
        }
        fn flags(&mut self, ty: u8, attr: bool, tag: bool) -> &mut Self {
            self.int(i32::from(ty) | if attr { HAS_ATTR } else { 0 } | if tag { HAS_TAG } else { 0 })
        }
        fn chars(&mut self, s: &str) -> &mut Self {
            self.int(i32::from(CHARSXP) | (1 << 3) << 12).int(s.len() as i32);
            self.0.extend_from_slice(s.as_bytes());
            self
        }
        /// A symbol: written in full the first time, then as a reference.
        fn sym(&mut self, syms: &mut Vec<String>, s: &str) -> &mut Self {
            match syms.iter().position(|x| x == s) {
                Some(i) => self.int(((i as i32 + 1) << 8) | i32::from(REFSXP)),
                None => {
                    syms.push(s.into());
                    self.flags(SYMSXP, false, false).chars(s)
                }
            }
        }
        fn strs(&mut self, v: &[Option<&str>], attr: bool) -> &mut Self {
            self.flags(STRSXP, attr, false).int(v.len() as i32);
            for s in v {
                match s {
                    Some(s) => self.chars(s),
                    None => self.int(i32::from(CHARSXP)).int(-1),
                };
            }
            self
        }
        fn reals(&mut self, v: &[f64], attr: bool) -> &mut Self {
            self.flags(REALSXP, attr, false).int(v.len() as i32);
            for x in v {
                self.0.extend_from_slice(&x.to_be_bytes());
            }
            self
        }
        fn ints(&mut self, ty: u8, v: &[i32], attr: bool) -> &mut Self {
            self.flags(ty, attr, false).int(v.len() as i32);
            for x in v {
                self.int(*x);
            }
            self
        }
        fn nil(&mut self) -> &mut Self {
            self.int(i32::from(NILVALUE_SXP))
        }
        /// A pairlist node with a tag; the caller writes the value, then the next node or `nil`.
        fn node(&mut self, syms: &mut Vec<String>, tag: &str) -> &mut Self {
            self.flags(LISTSXP, false, true).sym(syms, tag)
        }
        fn header(&mut self, workspace: bool) -> &mut Self {
            if workspace {
                self.0.extend_from_slice(b"RDX3\n");
            }
            self.0.extend_from_slice(b"X\n");
            self.int(3).int(0x040301).int(0x030500).int(5);
            self.0.extend_from_slice(b"UTF-8");
            self
        }
    }

    /// `save(df, m, f, file=…)` with `df <- data.frame(x=c(1.5,NA), g=factor(c("b","a")), s=c("u",NA))`,
    /// `m <- matrix(1:6, 2, dimnames=list(c("r1","r2"), NULL))` and a function.
    fn workspace() -> Vec<u8> {
        let mut syms = vec![];
        let mut w = W::default();
        w.header(true);
        w.node(&mut syms, "df");
        w.flags(VECSXP, true, false).int(3);
        w.reals(&[1.5, f64::from_bits(0x7FF0_0000_0000_07A2)], false);
        w.ints(INTSXP, &[2, 1], true);
        w.node(&mut syms, "levels").strs(&[Some("a"), Some("b")], false);
        w.node(&mut syms, "class").strs(&[Some("factor")], false).nil();
        w.strs(&[Some("u"), None], false);
        w.node(&mut syms, "names").strs(&[Some("x"), Some("g"), Some("s")], false);
        w.node(&mut syms, "class").strs(&[Some("data.frame")], false);
        w.node(&mut syms, "row.names").ints(INTSXP, &[NA_INT, -2], false).nil();
        w.node(&mut syms, "m");
        w.ints(INTSXP, &[1, 2, 3, 4, 5, 6], true);
        w.node(&mut syms, "dim").ints(INTSXP, &[2, 3], false);
        w.node(&mut syms, "dimnames").flags(VECSXP, false, false).int(2).strs(&[Some("r1"), Some("r2")], false).nil().nil();
        w.node(&mut syms, "f");
        // function(a) a: a closure with the global environment, formals and body.
        w.flags(CLOSXP, false, true).int(i32::from(GLOBALENV_SXP));
        w.node(&mut syms, "a").int(i32::from(MISSINGARG_SXP)).nil();
        w.sym(&mut syms, "a");
        w.nil();
        w.0
    }

    #[test]
    fn workspace_objects_and_tabs() {
        let d = rdata_from(Cursor::new(workspace())).unwrap();
        assert_eq!(d.format, "RData");
        assert_eq!(d.info, vec![format!("3 {}", t("objects")), "R 4.3.1".to_string()]);
        let c = &d.sheets[0];
        assert_eq!((cell(c, 0, 0), cell(c, 0, 1), cell(c, 0, 2), cell(c, 0, 3)), (Some("df"), Some("data.frame"), Some("list"), Some("2 × 3")));
        assert_eq!((cell(c, 1, 1), cell(c, 1, 3)), (Some("matrix"), Some("2 × 3")));
        assert_eq!((cell(c, 2, 1), cell(c, 2, 2), cell(c, 2, 4)), (Some("function"), Some("closure"), Some(t("This type is not shown").as_str())));
        assert_eq!(d.sheets.len(), 3);
        let df = &d.sheets[1];
        assert_eq!(df.columns, ["x", "g", "s"]);
        assert_eq!(df.types, ["double", "factor", "character"]);
        assert_eq!(df.rows, vec![vec![Some("1.5".to_string()), Some("b".into()), Some("u".into())], vec![Some("NA".into()), Some("a".into()), Some("NA".into())]]);
        let m = &d.sheets[2];
        assert_eq!(m.columns, ["", "[,1]", "[,2]", "[,3]"]);
        assert_eq!(m.rows[1], vec![Some("r2".to_string()), Some("2".into()), Some("4".into()), Some("6".into())]);
    }

    #[test]
    fn packed_and_single_objects() {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&workspace()).unwrap();
        let d = rdata_from(Cursor::new(gz.finish().unwrap())).unwrap();
        assert_eq!(d.sheets[0].total_rows, Some(3));
        // saveRDS(c(a=TRUE, b=NA)): no workspace line, one object.
        let mut syms = vec![];
        let mut w = W::default();
        w.header(false).ints(LGLSXP, &[1, NA_INT], true).node(&mut syms, "names").strs(&[Some("a"), Some("b")], false).nil();
        let mut xz = xz2::write::XzEncoder::new(Vec::new(), 1);
        xz.write_all(&w.0).unwrap();
        let d = rdata_from(Cursor::new(xz.finish().unwrap())).unwrap();
        assert_eq!(d.format, "RDS");
        assert_eq!(d.info, vec!["logical".to_string(), "2".into(), "R 4.3.1".into()]);
        assert_eq!(d.sheets[0].rows, vec![vec![Some("a".to_string()), Some("TRUE".into())], vec![Some("b".into()), Some("NA".into())]]);
    }

    #[test]
    fn compact_sequences_are_expanded() {
        // saveRDS(1:1e9): an ALTREP `compact_intseq` of a billion numbers in a few bytes.
        let mut syms = vec![];
        let mut w = W::default();
        w.header(false).int(i32::from(ALTREP_SXP));
        w.flags(LISTSXP, false, false).sym(&mut syms, "compact_intseq");
        w.flags(LISTSXP, false, false).sym(&mut syms, "base");
        w.flags(LISTSXP, false, false).ints(INTSXP, &[13], false).nil();
        w.reals(&[1e9, 1.0, 1.0], false).nil();
        let d = rdata_from(Cursor::new(w.0)).unwrap();
        assert_eq!(d.info[..2], ["integer".to_string(), "1,000,000,000".into()]);
        assert_eq!(d.sheets[0].total_rows, Some(1_000_000_000));
        assert_eq!(d.sheets[0].rows.len(), MAX_ROWS);
        assert_eq!(cell(&d.sheets[0], 2, 0), Some("3"));
    }

    #[test]
    fn bad_and_cut_files_are_refused() {
        assert!(rdata_from(Cursor::new(b"hello".to_vec())).is_err());
        assert!(rdata_from(Cursor::new(b"RDX1\nX\n".to_vec())).is_err());
        let ws = workspace();
        // Cut inside the first object: nothing to show.
        assert!(rdata_from(Cursor::new(ws[..40].to_vec())).is_err());
        // Cut later: the objects before are listed, with what went wrong.
        let d = rdata_from(Cursor::new(ws[..ws.len() - 8].to_vec())).unwrap();
        assert_eq!(d.sheets[0].total_rows, Some(2));
        assert!(d.info.contains(&t("This file is cut short.")), "{:?}", d.info);
        // A list nested beyond any sense is refused, not followed down the stack.
        let mut w = W::default();
        w.header(false);
        for _ in 0..5000 {
            w.flags(VECSXP, false, false).int(1);
        }
        w.nil();
        // On a stack the size of the preview thread's.
        let err = std::thread::Builder::new().stack_size(32 * 1024 * 1024).spawn(move || rdata_from(Cursor::new(w.0)).err()).unwrap().join().unwrap();
        assert_eq!(err, Some(t("This R file is nested too deeply to preview.")));
    }
}
