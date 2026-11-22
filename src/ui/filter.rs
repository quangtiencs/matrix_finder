//! Quick filter for the file list.
//!
//! Accepted inputs:
//! * plain text          → case-insensitive substring of the name (`report`)
//! * glob                → `*.rs`, `img_??.png`
//! * extension list      → `.rs .toml`  or  `.jpg,.png`
//! * SQL-like expression → `name LIKE '%test%' AND ext IN ('rs','toml')`,
//!                         `size > 10MB OR type = 'dir'`, `NOT name LIKE '.%'`,
//!                         `age < 7d`, `modified >= '2024-01-01'`
//!
//! Fields: `name`, `ext`, `size`, `type` (`'dir'`/`'file'`), `modified` (date),
//! `age` (e.g. `3d`, `12h`, `2w`), `hidden` (`true`/`false`), and `source` (alias
//! `content`): the lines of a file, read like grep does (binary files never match), as in
//! `source LIKE '%todo%'`.
//! Operators: `= != <> < <= > >= LIKE NOT LIKE ILIKE GLOB IN NOT IN`, `AND OR NOT`, parentheses,
//! and regular expressions: `REGEXP` / `RLIKE` (ignoring case), `~` (case-sensitive), `~*`
//! (ignoring case), `!~` and `!~*` (no match), as in `name REGEXP '^img_[0-9]+'`.
//! `LIKE` is case-insensitive: `%` = any run of characters, `_` = exactly one.
use crate::files::vfs::Entry;
use std::io::{BufRead, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::{Duration, SystemTime};

/// Longer lines are read in pieces of this size (a match across two pieces is missed).
const MAX_LINE: u64 = 1024 * 1024;
/// A compiled regular expression may take this much memory.
const MAX_REGEX: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub enum Filter {
    All,
    Substring(String),
    Exts(Vec<String>),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
    NotLike,
    In,
    NotIn,
}

#[derive(Debug, Clone)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Cmp { field: Field, op: Op, values: Vec<Value> },
    /// `field REGEXP 'pattern'`; `negate` for `NOT REGEXP` and `!~`.
    Regex { field: Field, re: regex::Regex, negate: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Field {
    Name,
    Ext,
    Size,
    Type,
    Modified,
    Age,
    Hidden,
    /// The lines of the file.
    Source,
}

impl Filter {
    pub fn parse(input: &str) -> Result<Filter, String> {
        let s = input.trim();
        if s.is_empty() {
            return Ok(Filter::All);
        }
        let s = s
            .strip_prefix("where ")
            .or_else(|| s.strip_prefix("WHERE "))
            .unwrap_or(s);
        if looks_like_sql(s) {
            match parse_sql(s) {
                Ok(e) => return Ok(Filter::Expr(e)),
                // Only words so far ("not" on the way to "notebook", "name in" for "name index"):
                // plain text, not an error.
                Err(_) if plain_words(s) => {}
                Err(e) => return Err(e),
            }
        }
        // Extension list: every token starts with '.'
        let parts: Vec<&str> = s
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if !parts.is_empty() && parts.iter().all(|p| p.starts_with('.') && p.len() > 1 && !p.contains('*')) {
            return Ok(Filter::Exts(parts.iter().map(|p| p[1..].to_lowercase()).collect()));
        }
        if s.contains('*') || s.contains('?') {
            // Glob(s) → LIKE patterns, OR'ed.
            let mut expr: Option<Expr> = None;
            for g in parts {
                let pat = glob_to_like(g);
                let cmp = Expr::Cmp { field: Field::Name, op: Op::Like, values: vec![Value::Str(pat)] };
                expr = Some(match expr {
                    None => cmp,
                    Some(e) => Expr::Or(Box::new(e), Box::new(cmp)),
                });
            }
            return Ok(expr.map(Filter::Expr).unwrap_or(Filter::All));
        }
        Ok(Filter::Substring(fold(s)))
    }

    /// Whether `e` passes, from what the listing knows: `source` never matches here.
    pub fn matches(&self, e: &Entry) -> bool {
        self.matches_at(e, None, &AtomicBool::new(false))
    }

    /// Whether `e`, the file at `path`, passes. Reading its lines (for `source`) stops
    /// early, without a match, once `stop` is set. This reads files: not on the UI thread.
    pub fn matches_at(&self, e: &Entry, path: Option<&Path>, stop: &AtomicBool) -> bool {
        match self {
            Filter::All => true,
            Filter::Substring(s) => fold(&e.name).contains(s),
            Filter::Exts(exts) => !e.is_dir && exts.iter().any(|x| *x == e.ext),
            Filter::Expr(x) => eval(x, e, path, stop),
        }
    }

    /// Whether it looks into files (`source`), which has to be done in the background.
    pub fn reads_content(&self) -> bool {
        fn walk(x: &Expr) -> bool {
            match x {
                Expr::And(a, b) | Expr::Or(a, b) => walk(a) || walk(b),
                Expr::Not(a) => walk(a),
                Expr::Cmp { field, .. } | Expr::Regex { field, .. } => *field == Field::Source,
            }
        }
        matches!(self, Filter::Expr(x) if walk(x))
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Filter::All)
    }
}

/// Text that reads as words being typed rather than SQL: no operators or quotes, and no
/// keyword written in capitals (`name LIKE` is SQL in the making, `name like` may be a name).
fn plain_words(s: &str) -> bool {
    s.chars().all(|c| c.is_alphanumeric() || c.is_whitespace() || matches!(c, '-' | '_' | '.'))
        && !s.split_whitespace().any(|w| matches!(w, "LIKE" | "ILIKE" | "GLOB" | "IN" | "AND" | "OR" | "NOT" | "REGEXP" | "RLIKE"))
}

fn parse_sql(s: &str) -> Result<Expr, String> {
    let tokens = tokenize(s)?;
    let mut p = Parser { t: tokens, i: 0 };
    let e = p.or()?;
    if p.i < p.t.len() {
        return Err(format!("Unexpected '{}'", p.t[p.i].text()));
    }
    Ok(e)
}

/// Lower case in one Unicode form (NFC), so that a name stored decomposed (macOS,
/// zip files: "e" + combining accents) matches what is typed.
pub fn fold(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_lowercase()
    } else {
        nfc(s).to_lowercase()
    }
}

/// `s` in composed form (NFC).
pub fn nfc(s: &str) -> std::borrow::Cow<'_, str> {
    use unicode_normalization::UnicodeNormalization;
    if s.is_ascii() || unicode_normalization::is_nfc(s) {
        std::borrow::Cow::Borrowed(s)
    } else {
        std::borrow::Cow::Owned(s.nfc().collect())
    }
}

fn looks_like_sql(s: &str) -> bool {
    let up = s.to_uppercase();
    let words: Vec<&str> = up
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
        .collect();
    let has_kw = words
        .iter()
        .any(|w| matches!(*w, "LIKE" | "ILIKE" | "GLOB" | "IN" | "AND" | "OR" | "NOT" | "REGEXP" | "RLIKE"));
    let starts_field = words.first().is_some_and(|w| {
        matches!(*w, "NAME" | "EXT" | "SIZE" | "TYPE" | "MODIFIED" | "AGE" | "HIDDEN" | "SOURCE" | "CONTENT" | "NOT")
    });
    starts_field && (has_kw || s.contains(['=', '<', '>', '~']))
}

fn glob_to_like(g: &str) -> String {
    let mut out = String::new();
    for c in g.chars() {
        match c {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// SQL LIKE (case-insensitive). `\` escapes `%` and `_`.
pub fn like(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut p: Vec<(char, bool)> = Vec::new(); // (char, is_literal)
    let mut it = pattern.to_lowercase().chars().collect::<Vec<_>>().into_iter();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(n) = it.next() {
                p.push((n, true));
            }
        } else {
            p.push((c, false));
        }
    }
    // Iterative wildcard matching with backtracking on '%'.
    let (mut ti, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && !p[pi].1 && p[pi].0 == '%' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && ((!p[pi].1 && p[pi].0 == '_') || p[pi].0 == t[ti]) {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && !p[pi].1 && p[pi].0 == '%' {
        pi += 1;
    }
    pi == p.len()
}

// ---------------------------------------------------------------- tokenizer

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Sym(String),
    LParen,
    RParen,
    Comma,
}

impl Tok {
    fn text(&self) -> String {
        match self {
            Tok::Word(s) | Tok::Sym(s) => s.clone(),
            Tok::Str(s) => format!("'{s}'"),
            Tok::LParen => "(".into(),
            Tok::RParen => ")".into(),
            Tok::Comma => ",".into(),
        }
    }
    fn kw(&self, k: &str) -> bool {
        matches!(self, Tok::Word(w) if w.eq_ignore_ascii_case(k))
    }
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '\'' || ch == '"' {
            let q = ch;
            i += 1;
            let mut v = String::new();
            loop {
                if i >= c.len() {
                    return Err("Unterminated string".into());
                }
                if c[i] == q {
                    if i + 1 < c.len() && c[i + 1] == q {
                        v.push(q);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                v.push(c[i]);
                i += 1;
            }
            out.push(Tok::Str(v));
        } else if ch == '(' {
            out.push(Tok::LParen);
            i += 1;
        } else if ch == ')' {
            out.push(Tok::RParen);
            i += 1;
        } else if ch == ',' {
            out.push(Tok::Comma);
            i += 1;
        } else if ch == '~' || (ch == '!' && c.get(i + 1) == Some(&'~')) {
            // Regular expressions: ~ ~* !~ !~*
            let mut sym = ch.to_string();
            i += 1;
            if ch == '!' {
                sym.push('~');
                i += 1;
            }
            if c.get(i) == Some(&'*') {
                sym.push('*');
                i += 1;
            }
            out.push(Tok::Sym(sym));
        } else if "=<>!".contains(ch) {
            let mut sym = ch.to_string();
            if i + 1 < c.len() && "=<>".contains(c[i + 1]) {
                sym.push(c[i + 1]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Sym(sym));
        } else {
            let mut w = String::new();
            while i < c.len() && !c[i].is_whitespace() && !"()',=<>!~\"".contains(c[i]) {
                w.push(c[i]);
                i += 1;
            }
            out.push(Tok::Word(w));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- parser

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn eat_kw(&mut self, k: &str) -> bool {
        if self.peek().is_some_and(|t| t.kw(k)) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while self.eat_kw("OR") {
            let r = self.and()?;
            l = Expr::Or(Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.not()?;
        while self.eat_kw("AND") {
            let r = self.not()?;
            l = Expr::And(Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.eat_kw("NOT") {
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Tok::LParen) {
            self.i += 1;
            let e = self.or()?;
            if self.next() != Some(Tok::RParen) {
                return Err("Expected ')'".into());
            }
            return Ok(e);
        }
        let field = match self.next() {
            Some(Tok::Word(w)) => match w.to_lowercase().as_str() {
                "name" => Field::Name,
                "ext" | "extension" => Field::Ext,
                "size" => Field::Size,
                "type" | "kind" => Field::Type,
                "modified" | "date" | "mtime" => Field::Modified,
                "age" => Field::Age,
                "hidden" => Field::Hidden,
                "source" | "content" | "contents" => Field::Source,
                other => return Err(format!("Unknown field '{other}'")),
            },
            Some(t) => return Err(format!("Expected a field, found '{}'", t.text())),
            None => return Err("Expected a field".into()),
        };
        let negate = self.eat_kw("NOT");
        // Regular expressions: (case-sensitive, no match).
        let regex = match self.peek() {
            Some(Tok::Sym(s)) if !negate && s.contains('~') => Some((s == "~" || s == "!~", s.starts_with('!'))),
            Some(t) if t.kw("REGEXP") || t.kw("RLIKE") => Some((false, negate)),
            _ => None,
        };
        if let Some((case_sensitive, no_match)) = regex {
            self.i += 1;
            if !matches!(field, Field::Name | Field::Ext | Field::Source) {
                return Err("REGEXP works on name, ext and source".into());
            }
            let pattern = match self.next() {
                Some(Tok::Str(s)) | Some(Tok::Word(s)) => s,
                Some(t) => return Err(format!("Expected a pattern, found '{}'", t.text())),
                None => return Err("Expected a pattern".into()),
            };
            let re = regex::RegexBuilder::new(&nfc(&pattern))
                .case_insensitive(!case_sensitive)
                .size_limit(MAX_REGEX)
                .build()
                .map_err(|e| format!("Bad regular expression: {}", e.to_string().lines().last().unwrap_or_default().trim()))?;
            return Ok(Expr::Regex { field, re, negate: no_match });
        }
        let op = match self.next() {
            Some(Tok::Sym(s)) if !negate => match s.as_str() {
                "=" | "==" => Op::Eq,
                "!=" | "<>" => Op::Ne,
                "<" => Op::Lt,
                "<=" => Op::Le,
                ">" => Op::Gt,
                ">=" => Op::Ge,
                _ => return Err(format!("Unknown operator '{s}'")),
            },
            Some(t) if t.kw("LIKE") || t.kw("ILIKE") => if negate { Op::NotLike } else { Op::Like },
            Some(t) if t.kw("GLOB") => {
                let v = self.value(field)?;
                let pat = match v {
                    Value::Str(s) => glob_to_like(&s),
                    Value::Num(n) => n.to_string(),
                };
                return Ok(Expr::Cmp {
                    field,
                    op: if negate { Op::NotLike } else { Op::Like },
                    values: vec![Value::Str(pat)],
                });
            }
            Some(t) if t.kw("IN") => if negate { Op::NotIn } else { Op::In },
            Some(t) => return Err(format!("Expected an operator, found '{}'", t.text())),
            None => return Err("Expected an operator".into()),
        };
        let values = if matches!(op, Op::In | Op::NotIn) {
            if self.next() != Some(Tok::LParen) {
                return Err("Expected '(' after IN".into());
            }
            let mut vs = vec![self.value(field)?];
            loop {
                match self.next() {
                    Some(Tok::Comma) => vs.push(self.value(field)?),
                    Some(Tok::RParen) => break,
                    _ => return Err("Expected ',' or ')'".into()),
                }
            }
            vs
        } else {
            vec![self.value(field)?]
        };
        if field == Field::Source && !matches!(op, Op::Like | Op::NotLike) {
            return Err("source works with LIKE and REGEXP, e.g. source LIKE '%todo%'".into());
        }
        Ok(Expr::Cmp { field, op, values })
    }

    fn value(&mut self, field: Field) -> Result<Value, String> {
        let raw = match self.next() {
            Some(Tok::Str(s)) => s,
            Some(Tok::Word(w)) => w,
            Some(t) => return Err(format!("Expected a value, found '{}'", t.text())),
            None => return Err("Expected a value".into()),
        };
        Ok(match field {
            Field::Size => Value::Num(parse_size(&raw).ok_or(format!("Bad size '{raw}' (e.g. 10MB)"))?),
            Field::Age => Value::Num(parse_age(&raw).ok_or(format!("Bad age '{raw}' (e.g. 7d, 12h)"))?),
            Field::Modified => Value::Num(parse_date(&raw).ok_or(format!("Bad date '{raw}' (YYYY-MM-DD)"))?),
            _ => Value::Str(fold(&raw)),
        })
    }
}

fn parse_size(s: &str) -> Option<f64> {
    let s = s.trim().to_lowercase();
    let idx = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
    let (n, unit) = s.split_at(idx);
    let n: f64 = n.parse().ok()?;
    let mul = match unit.trim() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024f64.powi(4),
        _ => return None,
    };
    Some(n * mul)
}

/// Age in seconds.
fn parse_age(s: &str) -> Option<f64> {
    let s = s.trim().to_lowercase();
    let idx = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
    let (n, unit) = s.split_at(idx);
    let n: f64 = n.parse().ok()?;
    let mul = match unit {
        "s" => 1.0,
        "m" | "min" => 60.0,
        "h" => 3600.0,
        "" | "d" => 86400.0,
        "w" => 7.0 * 86400.0,
        "mo" => 30.0 * 86400.0,
        "y" => 365.0 * 86400.0,
        _ => return None,
    };
    Some(n * mul)
}

/// Date → unix seconds (local midnight).
fn parse_date(s: &str) -> Option<f64> {
    let d = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()?;
    let dt = d.and_hms_opt(0, 0, 0)?.and_local_timezone(chrono::Local).single()?;
    Some(dt.timestamp() as f64)
}

fn cmp_num(a: f64, op: Op, b: f64) -> bool {
    match op {
        Op::Eq => a == b,
        Op::Ne => a != b,
        Op::Lt => a < b,
        Op::Le => a <= b,
        Op::Gt => a > b,
        Op::Ge => a >= b,
        _ => false,
    }
}

/// Whether some line of the file at `path` passes `test`, like grep. `None` for a binary
/// file (a NUL byte in its first 64 KB), one that cannot be read, or once `stop` is set.
pub fn grep(path: &Path, stop: &AtomicBool, mut test: impl FnMut(&str) -> bool) -> Option<bool> {
    let f = crate::util::open_regular(path).ok()?;
    let mut r = std::io::BufReader::with_capacity(64 * 1024, f);
    if r.fill_buf().ok()?.contains(&0) {
        return None;
    }
    let mut line = Vec::new();
    for n in 0u64.. {
        if n % 4096 == 0 && stop.load(AtomicOrdering::Relaxed) {
            return None;
        }
        line.clear();
        if (&mut r).take(MAX_LINE).read_until(b'\n', &mut line).ok()? == 0 {
            break;
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if test(&String::from_utf8_lossy(&line)) {
            return Some(true);
        }
    }
    Some(false)
}

/// A LIKE pattern for many lines: `%text%`, the usual one, is a plain search.
fn line_matcher(pattern: &str) -> impl Fn(&str) -> bool + '_ {
    let inner = pattern.strip_prefix('%').and_then(|p| p.strip_suffix('%')).filter(|p| !p.contains(['%', '_', '\\']));
    let lower = inner.map(str::to_lowercase);
    move |line: &str| match &lower {
        Some(text) => line.to_lowercase().contains(text.as_str()),
        None => like(line, pattern),
    }
}

/// `source …` for a file: whether some line passes. Folders, binary files and what
/// cannot be read never match, with or without NOT.
fn source_matches(e: &Entry, path: Option<&Path>, stop: &AtomicBool, test: impl FnMut(&str) -> bool, negate: bool) -> bool {
    let Some(path) = path.filter(|_| !e.is_dir && !e.is_parent) else { return false };
    grep(path, stop, test).is_some_and(|found| found != negate)
}

fn eval(x: &Expr, e: &Entry, path: Option<&Path>, stop: &AtomicBool) -> bool {
    match x {
        Expr::And(a, b) => eval(a, e, path, stop) && eval(b, e, path, stop),
        Expr::Or(a, b) => eval(a, e, path, stop) || eval(b, e, path, stop),
        Expr::Not(a) => !eval(a, e, path, stop),
        Expr::Regex { field, re, negate } => match field {
            Field::Source => source_matches(e, path, stop, |l| re.is_match(l), *negate),
            Field::Ext => re.is_match(&e.ext) != *negate,
            _ => re.is_match(&nfc(&e.name)) != *negate,
        },
        Expr::Cmp { field: Field::Source, op, values } => {
            let Some(Value::Str(pattern)) = values.first() else { return false };
            source_matches(e, path, stop, line_matcher(pattern), *op == Op::NotLike)
        }
        Expr::Cmp { field, op, values } => {
            let text: Option<String> = match field {
                Field::Name => Some(fold(&e.name)),
                Field::Ext => Some(e.ext.clone()),
                Field::Type => Some(if e.is_dir { "dir".into() } else { "file".into() }),
                Field::Hidden => Some(e.hidden.to_string()),
                _ => None,
            };
            if let Some(t) = text {
                let strs = values.iter().map(|v| match v {
                    Value::Str(s) => s.trim_start_matches('.').to_string(),
                    Value::Num(n) => n.to_string(),
                });
                let norm = |s: String| {
                    if *field == Field::Type {
                        match s.as_str() {
                            "d" | "folder" | "directory" => "dir".to_string(),
                            "f" => "file".to_string(),
                            _ => s,
                        }
                    } else if *field == Field::Name {
                        // names are matched raw (keep leading dots)
                        s
                    } else {
                        s
                    }
                };
                let raw: Vec<String> = if *field == Field::Name {
                    values
                        .iter()
                        .map(|v| match v {
                            Value::Str(s) => s.clone(),
                            Value::Num(n) => n.to_string(),
                        })
                        .collect()
                } else {
                    strs.map(norm).collect()
                };
                return match op {
                    Op::Eq => t == raw[0],
                    Op::Ne => t != raw[0],
                    Op::Like => like(&t, &raw[0]),
                    Op::NotLike => !like(&t, &raw[0]),
                    Op::In => raw.iter().any(|v| *v == t),
                    Op::NotIn => !raw.iter().any(|v| *v == t),
                    Op::Lt => t < raw[0],
                    Op::Le => t <= raw[0],
                    Op::Gt => t > raw[0],
                    Op::Ge => t >= raw[0],
                };
            }
            let n = match field {
                Field::Size => {
                    if e.is_dir && !e.is_parent && e.size == 0 {
                        return false;
                    }
                    e.size as f64
                }
                Field::Modified => match e.modified.and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok()) {
                    Some(d) => d.as_secs_f64(),
                    None => return false,
                },
                Field::Age => match e.modified {
                    Some(m) => SystemTime::now()
                        .duration_since(m)
                        .unwrap_or(Duration::ZERO)
                        .as_secs_f64(),
                    None => return false,
                },
                _ => return false,
            };
            let nums: Vec<f64> = values
                .iter()
                .filter_map(|v| if let Value::Num(n) = v { Some(*n) } else { None })
                .collect();
            match op {
                Op::In => nums.contains(&n),
                Op::NotIn => !nums.contains(&n),
                Op::Like | Op::NotLike => false,
                _ => nums.first().is_some_and(|b| cmp_num(n, *op, *b)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> Entry {
        Entry::new(name.into(), false, size, Some(SystemTime::now()))
    }
    fn dir(name: &str) -> Entry {
        Entry::new(name.into(), true, 0, Some(SystemTime::now()))
    }

    #[test]
    fn like_semantics() {
        assert!(like("main.rs", "%.rs"));
        assert!(like("Main.RS", "%.rs"));
        assert!(!like("main.rsx", "%.rs"));
        assert!(like("a1.png", "a_.png"));
        assert!(!like("a12.png", "a_.png"));
        assert!(like("50%off", "50\\%%"));
        assert!(like("anything", "%"));
        assert!(like("abcabd", "%ab_"));
    }

    #[test]
    fn typing_words_is_plain_text() {
        // On the way to "notebook" and "name index": no error, a plain search.
        for s in ["not", "Not ", "name in", "name and"] {
            assert!(matches!(Filter::parse(s), Ok(Filter::Substring(_))), "{s}");
        }
        let f = Filter::parse("not").unwrap();
        assert!(f.matches(&file("notebook.ipynb", 1)) && !f.matches(&file("main.rs", 1)));
        // Real SQL mistakes still show.
        assert!(Filter::parse("size > ").is_err());
        assert!(Filter::parse("name = ").is_err() && Filter::parse("NOT").is_err());
        assert!(matches!(Filter::parse("NOT name LIKE '.%'"), Ok(Filter::Expr(_))));
    }

    #[test]
    fn decomposed_names_match() {
        use unicode_normalization::UnicodeNormalization;
        // A name as macOS and zip files often store it: letters + combining accents.
        let nfd: String = "Báo cáo Đà Nẵng.pdf".nfd().collect();
        let e = file(&nfd, 1);
        assert!(Filter::parse("nẵng").unwrap().matches(&e));
        assert!(Filter::parse("báo cáo").unwrap().matches(&e));
        assert!(Filter::parse("name LIKE '%đà%'").unwrap().matches(&e));
        assert!(Filter::parse("name REGEXP 'nẵng'").unwrap().matches(&e));
    }

    #[test]
    fn modes() {
        let rs = file("main.rs", 2000);
        let toml = file("Cargo.toml", 10);
        let d = dir("src");
        let f = Filter::parse("main").unwrap();
        assert!(f.matches(&rs) && !f.matches(&toml));
        let f = Filter::parse(".rs .toml").unwrap();
        assert!(f.matches(&rs) && f.matches(&toml) && !f.matches(&d));
        let f = Filter::parse("*.toml").unwrap();
        assert!(!f.matches(&rs) && f.matches(&toml));
    }

    #[test]
    fn sql() {
        let rs = file("main.rs", 2 * 1024 * 1024);
        let toml = file("Cargo.toml", 10);
        let d = dir("src");
        let f = Filter::parse("name LIKE '%a%' AND ext IN ('rs', 'toml')").unwrap();
        assert!(f.matches(&rs) && f.matches(&toml) && !f.matches(&d));
        let f = Filter::parse("size > 1MB").unwrap();
        assert!(f.matches(&rs) && !f.matches(&toml));
        let f = Filter::parse("type = 'dir' OR ext = 'toml'").unwrap();
        assert!(f.matches(&d) && f.matches(&toml) && !f.matches(&rs));
        let f = Filter::parse("NOT ext = rs").unwrap();
        assert!(!f.matches(&rs) && f.matches(&toml));
        let f = Filter::parse("name NOT LIKE 'c%'").unwrap();
        assert!(f.matches(&rs) && !f.matches(&toml));
        let f = Filter::parse("ext NOT IN ('.rs')").unwrap();
        assert!(!f.matches(&rs));
        let f = Filter::parse("where (ext = 'rs' or ext = 'toml') and size < 100").unwrap();
        assert!(!f.matches(&rs) && f.matches(&toml));
        let f = Filter::parse("age < 1d").unwrap();
        assert!(f.matches(&rs));
        let f = Filter::parse("modified > '2000-01-01'").unwrap();
        assert!(f.matches(&rs));
        let f = Filter::parse("name GLOB '*.RS'").unwrap();
        assert!(f.matches(&rs));
        assert!(Filter::parse("name LIKE").is_err());
        assert!(Filter::parse("size > lots").is_err());
        assert!(Filter::parse("bogus = 1").is_ok()); // not SQL-like → substring
    }

    #[test]
    fn regular_expressions_on_names() {
        let img = file("IMG_0042.JPG", 10);
        let other = file("img-cat.png", 10);
        let f = Filter::parse(r"name REGEXP '^img_\d+\.jpe?g$'").unwrap();
        assert!(f.matches(&img) && !f.matches(&other), "REGEXP ignores case");
        let f = Filter::parse(r"name ~ '^IMG_\d'").unwrap();
        assert!(f.matches(&img) && !f.matches(&other));
        let f = Filter::parse(r"name ~ '^img_\d'").unwrap();
        assert!(!f.matches(&img), "~ minds case");
        let f = Filter::parse(r"name ~* '^img_\d'").unwrap();
        assert!(f.matches(&img));
        let f = Filter::parse(r"name !~ '^IMG' AND name !~* 'zzz'").unwrap();
        assert!(!f.matches(&img) && f.matches(&other));
        let f = Filter::parse("name NOT REGEXP 'cat|dog' AND ext RLIKE '^(jpg|png)$'").unwrap();
        assert!(f.matches(&img) && !f.matches(&other));
        // Upper case stays: \D is not \d.
        let f = Filter::parse(r"name REGEXP '^\D+$'").unwrap();
        assert!(!f.matches(&img) && f.matches(&other));
        assert!(!Filter::parse("name REGEXP 'x'").unwrap().reads_content());
        for bad in ["name REGEXP '('", "size REGEXP '1'", "name ~"] {
            assert!(Filter::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn source_reads_lines_like_grep() {
        let dir = std::env::temp_dir().join(format!("mf-grep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, bytes: &[u8]| {
            std::fs::write(dir.join(name), bytes).unwrap();
            Entry::new(name.into(), false, bytes.len() as u64, Some(SystemTime::now()))
        };
        let notes = write("notes.md", b"# Plan\r\n- TODO: ship it\r\n- done\r\n");
        let code = write("main.rs", b"fn main() {\n    let x = 42;\n}\n");
        let blob = write("blob.bin", b"\x00\x01TODO\n");
        let sub = Entry::new("sub".into(), true, 0, None);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let stop = AtomicBool::new(false);
        let at = |f: &Filter, e: &Entry| f.matches_at(e, Some(&dir.join(&e.name)), &stop);

        let f = Filter::parse("source LIKE '%todo%'").unwrap();
        assert!(f.reads_content());
        assert!(at(&f, &notes) && !at(&f, &code) && !at(&f, &blob) && !at(&f, &sub));
        // Without a path (the listing alone) it never matches.
        assert!(!f.matches(&notes));
        // A pattern is held against each line, as grep does: whole lines, \r\n trimmed.
        assert!(at(&Filter::parse("source LIKE '- done'").unwrap(), &notes));
        assert!(at(&Filter::parse("content LIKE 'fn %'").unwrap(), &code));
        assert!(!at(&Filter::parse("source LIKE 'let%'").unwrap(), &code));
        let f = Filter::parse("source NOT LIKE '%todo%'").unwrap();
        assert!(!at(&f, &notes) && at(&f, &code) && !at(&f, &blob), "binary files never match");
        let f = Filter::parse(r"source ~ 'let \w+ = \d+;' AND ext = 'rs'").unwrap();
        assert!(at(&f, &code) && !at(&f, &notes));
        let f = Filter::parse("source REGEXP '^- (todo|fixme)' OR name LIKE 'main%'").unwrap();
        assert!(at(&f, &notes) && at(&f, &code));
        assert!(Filter::parse("source = 'x'").is_err());
        assert!(Filter::parse("source > 3").is_err());
        stop.store(true, AtomicOrdering::Relaxed);
        assert!(!at(&Filter::parse("source LIKE '%todo%'").unwrap(), &notes), "stopped");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
