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
//! `age` (e.g. `3d`, `12h`, `2w`), `hidden` (`true`/`false`).
//! Operators: `= != <> < <= > >= LIKE NOT LIKE ILIKE GLOB IN NOT IN`, `AND OR NOT`, parentheses.
//! `LIKE` is case-insensitive: `%` = any run of characters, `_` = exactly one.
use crate::vfs::Entry;
use std::time::{Duration, SystemTime};

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
            let tokens = tokenize(s)?;
            let mut p = Parser { t: tokens, i: 0 };
            let e = p.or()?;
            if p.i < p.t.len() {
                return Err(format!("Unexpected '{}'", p.t[p.i].text()));
            }
            return Ok(Filter::Expr(e));
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
        Ok(Filter::Substring(s.to_lowercase()))
    }

    pub fn matches(&self, e: &Entry) -> bool {
        match self {
            Filter::All => true,
            Filter::Substring(s) => e.name.to_lowercase().contains(s),
            Filter::Exts(exts) => !e.is_dir && exts.iter().any(|x| *x == e.ext),
            Filter::Expr(x) => eval(x, e),
        }
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Filter::All)
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
        .any(|w| matches!(*w, "LIKE" | "ILIKE" | "GLOB" | "IN" | "AND" | "OR" | "NOT"));
    let starts_field = words.first().is_some_and(|w| {
        matches!(*w, "NAME" | "EXT" | "SIZE" | "TYPE" | "MODIFIED" | "AGE" | "HIDDEN" | "NOT")
    });
    starts_field && (has_kw || s.contains(['=', '<', '>']))
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
            while i < c.len() && !c[i].is_whitespace() && !"()',=<>!\"".contains(c[i]) {
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
                other => return Err(format!("Unknown field '{other}'")),
            },
            Some(t) => return Err(format!("Expected a field, found '{}'", t.text())),
            None => return Err("Expected a field".into()),
        };
        let negate = self.eat_kw("NOT");
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
            _ => Value::Str(raw.to_lowercase()),
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

fn eval(x: &Expr, e: &Entry) -> bool {
    match x {
        Expr::And(a, b) => eval(a, e) && eval(b, e),
        Expr::Or(a, b) => eval(a, e) || eval(b, e),
        Expr::Not(a) => !eval(a, e),
        Expr::Cmp { field, op, values } => {
            let text: Option<String> = match field {
                Field::Name => Some(e.name.to_lowercase()),
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
}
