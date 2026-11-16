//! A reader of Python pickles that only rebuilds data: lists, dicts, numbers, text, and
//! a placeholder for any other object (its class name, arguments and state). Nothing
//! is ever imported or called, so a pickle cannot run code here. PyTorch's tensors
//! (`_rebuild_tensor_v2` over a storage named by a persistent id) become `Tensor`s.
//! Limits on the work, the stack and the nesting keep a hostile file from taking long.
use crate::i18n::t;
use crate::util::Res;
use std::cell::RefCell;
use std::rc::Rc;

/// Most opcodes run for one pickle.
const MAX_OPS: usize = 20_000_000;
/// Most values made, kept on the stack or the memo, or added to lists and dicts:
/// what a large checkpoint needs many times over, and a few hundred megabytes at most.
const MAX_ITEMS: usize = 2_000_000;
/// Text kept of a string (keys are short; long texts are cut).
const MAX_TEXT: usize = 4096;

#[derive(Clone, Debug)]
pub enum V {
    None,
    Bool(bool),
    Int(i64),
    /// An integer too large for 64 bits.
    BigInt,
    Float(f64),
    Str(Rc<str>),
    /// Only the length of bytes is kept.
    Bytes(usize),
    List(Rc<RefCell<Vec<V>>>),
    Tuple(Rc<Vec<V>>),
    Dict(Rc<RefCell<Vec<(V, V)>>>),
    /// A class or function, as `module.name`.
    Global(Rc<str>),
    /// An object of a class that is not rebuilt.
    Obj(Rc<Obj>),
    Storage(Rc<Storage>),
    Tensor(Rc<Tensor>),
}

/// Values nest as deeply as a file makes them, and dropping them one inside another
/// would take a stack frame per level: they are taken apart in a loop instead.
impl Drop for V {
    fn drop(&mut self) {
        let mut work = vec![];
        take_children(self, &mut work);
        while let Some(mut v) = work.pop() {
            take_children(&mut v, &mut work);
        }
    }
}

/// Move out what this value alone holds (shared values are left to their other owners).
fn take_children(v: &mut V, out: &mut Vec<V>) {
    match v {
        V::List(l) if Rc::strong_count(l) == 1 => out.append(&mut l.borrow_mut()),
        V::Tuple(t) => {
            if let Some(items) = Rc::get_mut(t) {
                out.append(items);
            }
        }
        V::Dict(d) if Rc::strong_count(d) == 1 => {
            for (k, v) in d.borrow_mut().drain(..) {
                out.push(k);
                out.push(v);
            }
        }
        V::Obj(o) => {
            if let Some(o) = Rc::get_mut(o) {
                out.push(std::mem::replace(&mut o.args, V::None));
                out.push(o.state.replace(V::None));
            }
        }
        _ => {}
    }
}

#[derive(Debug)]
pub struct Obj {
    pub class: Rc<str>,
    pub args: V,
    pub state: RefCell<V>,
}

#[derive(Debug)]
pub struct Storage {
    /// `F32`, `BF16`, … from the storage's class.
    pub dtype: String,
    pub item_size: Option<u64>,
}

#[derive(Debug)]
pub struct Tensor {
    pub dtype: String,
    pub item_size: Option<u64>,
    pub shape: Option<Vec<u64>>,
    pub note: &'static str,
}

impl V {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            V::Str(s) | V::Global(s) => Some(s),
            _ => None,
        }
    }

    fn items(&self) -> Option<Vec<V>> {
        match self {
            V::Tuple(t) => Some(t.to_vec()),
            V::List(l) => Some(l.borrow().clone()),
            _ => None,
        }
    }

    /// A short text for a value shown as metadata.
    pub fn short_text(&self) -> Option<String> {
        Some(match self {
            V::None => "None".into(),
            V::Bool(b) => if *b { "True" } else { "False" }.into(),
            V::Int(i) => i.to_string(),
            V::Float(f) => f.to_string(),
            V::Str(s) => s.to_string(),
            V::Global(g) => g.to_string(),
            V::Bytes(n) => format!("<{n} bytes>"),
            _ => return None,
        })
    }
}

/// `torch.FloatStorage` gives `F32` and 4 bytes per item.
fn storage_dtype(class: &str) -> (String, Option<u64>) {
    let name = class.rsplit('.').next().unwrap_or(class).trim_end_matches("Storage");
    let (dtype, size) = match name {
        "Float" => ("F32", 4),
        "Double" => ("F64", 8),
        "Half" => ("F16", 2),
        "BFloat16" => ("BF16", 2),
        "Long" => ("I64", 8),
        "Int" => ("I32", 4),
        "Short" => ("I16", 2),
        "Char" => ("I8", 1),
        "Byte" => ("U8", 1),
        "Bool" => ("BOOL", 1),
        "ComplexFloat" => ("C64", 8),
        "ComplexDouble" => ("C128", 16),
        "QInt8" => ("QI8", 1),
        "QUInt8" => ("QU8", 1),
        "QInt32" => ("QI32", 4),
        "Float8_e4m3fn" => ("F8_E4M3", 1),
        "Float8_e5m2" => ("F8_E5M2", 1),
        "UInt16" => ("U16", 2),
        "UInt32" => ("U32", 4),
        "UInt64" => ("U64", 8),
        other => return (other.to_string(), None),
    };
    (dtype.to_string(), Some(size))
}

fn bad() -> String {
    t("This file is too complex to preview.")
}

struct Machine<'a> {
    b: &'a [u8],
    i: usize,
    /// Values added to containers so far.
    added: usize,
    stack: Vec<V>,
    marks: Vec<usize>,
    memo: std::collections::HashMap<u32, V>,
}

impl<'a> Machine<'a> {
    fn take(&mut self, n: usize) -> Res<&'a [u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n).ok_or_else(bad)?).ok_or_else(|| t("This file is cut short."))?;
        self.i += n;
        Ok(s)
    }

    fn u8(&mut self) -> Res<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Res<usize> {
        let b = self.take(2)?;
        Ok(usize::from(u16::from_le_bytes([b[0], b[1]])))
    }

    fn u32(&mut self) -> Res<usize> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }

    fn u64(&mut self) -> Res<usize> {
        let b = self.take(8)?;
        usize::try_from(u64::from_le_bytes(b.try_into().unwrap_or_default())).map_err(|_| bad())
    }

    fn line(&mut self) -> Res<&'a str> {
        let rest = &self.b[self.i..];
        let end = rest.iter().position(|c| *c == b'\n').ok_or_else(|| t("This file is cut short."))?;
        self.i += end + 1;
        std::str::from_utf8(&rest[..end]).map_err(|_| bad())
    }

    fn text(&mut self, n: usize) -> Res<V> {
        let raw = self.take(n)?;
        let mut s = String::from_utf8_lossy(&raw[..raw.len().min(MAX_TEXT)]).into_owned();
        if raw.len() > MAX_TEXT {
            s.push('…');
        }
        Ok(V::Str(s.into()))
    }

    /// Count `n` more values kept in containers.
    fn add(&mut self, n: usize) -> Res<()> {
        self.added += n;
        if self.added > MAX_ITEMS {
            return Err(bad());
        }
        Ok(())
    }

    fn push(&mut self, v: V) -> Res<()> {
        if self.stack.len() >= MAX_ITEMS {
            return Err(bad());
        }
        self.stack.push(v);
        Ok(())
    }

    fn pop(&mut self) -> Res<V> {
        // A mark protects what is below it.
        if self.marks.last().is_some_and(|m| *m >= self.stack.len()) {
            return Err(bad());
        }
        self.stack.pop().ok_or_else(bad)
    }

    fn top(&mut self) -> Res<&mut V> {
        self.stack.last_mut().ok_or_else(bad)
    }

    /// What was pushed since the last mark.
    fn since_mark(&mut self) -> Res<Vec<V>> {
        let at = self.marks.pop().ok_or_else(bad)?;
        if at > self.stack.len() {
            return Err(bad());
        }
        Ok(self.stack.split_off(at))
    }

    fn remember(&mut self, key: u32) -> Res<()> {
        if self.memo.len() >= MAX_ITEMS {
            return Err(bad());
        }
        let v = self.stack.last().cloned().ok_or_else(bad)?;
        self.memo.insert(key, v);
        Ok(())
    }

    fn recall(&mut self, key: u32) -> Res<()> {
        let v = self.memo.get(&key).cloned().ok_or_else(bad)?;
        self.push(v)
    }

    fn int(&mut self, bytes: &[u8]) -> V {
        if bytes.len() <= 8 {
            let fill = if bytes.last().is_some_and(|b| b & 0x80 != 0) { 0xFF } else { 0 };
            let mut b = [fill; 8];
            b[..bytes.len()].copy_from_slice(bytes);
            V::Int(i64::from_le_bytes(b))
        } else {
            V::BigInt
        }
    }

    fn set_items(dict: &V, pairs: Vec<V>) -> Res<()> {
        // Items set on an object that is not rebuilt are dropped.
        if let V::Dict(d) = dict {
            let mut d = d.borrow_mut();
            let mut it = pairs.into_iter();
            while let (Some(k), Some(v)) = (it.next(), it.next()) {
                d.push((k, v));
            }
        }
        Ok(())
    }

    fn run(&mut self) -> Res<V> {
        for _ in 0..MAX_OPS {
            let op = self.u8()?;
            match op {
                0x80 => {
                    self.u8()?;
                }
                0x95 => {
                    self.u64()?;
                }
                b'.' => return self.pop(),
                b'(' => {
                    if self.marks.len() >= MAX_ITEMS {
                        return Err(bad());
                    }
                    self.marks.push(self.stack.len());
                }
                b'0' => {
                    self.pop()?;
                }
                b'1' => {
                    self.since_mark()?;
                }
                b'2' => {
                    let v = self.top()?.clone();
                    self.push(v)?;
                }
                b'J' => {
                    let b = self.take(4)?;
                    self.push(V::Int(i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))))?;
                }
                b'K' => {
                    let v = self.u8()?;
                    self.push(V::Int(i64::from(v)))?;
                }
                b'M' => {
                    let v = self.u16()?;
                    self.push(V::Int(v as i64))?;
                }
                0x8a | 0x8b => {
                    let n = if op == 0x8a { usize::from(self.u8()?) } else { self.u32()? };
                    let bytes = self.take(n)?;
                    let v = self.int(bytes);
                    self.push(v)?;
                }
                b'I' | b'L' => {
                    let line = self.line()?.trim_end_matches('L');
                    let v = match line {
                        "00" => V::Bool(false),
                        "01" => V::Bool(true),
                        n => n.parse::<i64>().map(V::Int).unwrap_or_else(|_| V::Str(n.into())),
                    };
                    self.push(v)?;
                }
                b'G' => {
                    let b = self.take(8)?;
                    self.push(V::Float(f64::from_be_bytes(b.try_into().unwrap_or_default())))?;
                }
                b'F' => {
                    let v = self.line()?.parse::<f64>().map_err(|_| bad())?;
                    self.push(V::Float(v))?;
                }
                b'N' => self.push(V::None)?,
                0x88 => self.push(V::Bool(true))?,
                0x89 => self.push(V::Bool(false))?,
                b'X' | b'T' | b'B' => {
                    let n = self.u32()?;
                    let v = if op == b'B' { self.take(n).map(|b| V::Bytes(b.len()))? } else { self.text(n)? };
                    self.push(v)?;
                }
                0x8c | b'U' | b'C' => {
                    let n = usize::from(self.u8()?);
                    let v = if op == b'C' { self.take(n).map(|b| V::Bytes(b.len()))? } else { self.text(n)? };
                    self.push(v)?;
                }
                0x8d | 0x8e | 0x96 => {
                    let n = self.u64()?;
                    let v = if op == 0x8d { self.text(n)? } else { self.take(n).map(|b| V::Bytes(b.len()))? };
                    self.push(v)?;
                }
                b'V' | b'S' => {
                    let line = self.line()?;
                    let s = line.trim_matches(['\'', '"']);
                    self.push(V::Str(s.chars().take(MAX_TEXT).collect::<String>().into()))?;
                }
                b']' => self.push(V::List(Rc::default()))?,
                b'l' => {
                    let items = self.since_mark()?;
                    self.push(V::List(Rc::new(RefCell::new(items))))?;
                }
                b'a' => {
                    self.add(1)?;
                    let v = self.pop()?;
                    if let V::List(l) = self.top()? {
                        l.borrow_mut().push(v);
                    }
                }
                b'e' => {
                    let items = self.since_mark()?;
                    self.add(items.len())?;
                    if let V::List(l) = self.top()? {
                        l.borrow_mut().extend(items);
                    }
                }
                b')' => self.push(V::Tuple(Rc::default()))?,
                b't' => {
                    let items = self.since_mark()?;
                    self.push(V::Tuple(Rc::new(items)))?;
                }
                0x85..=0x87 => {
                    let n = usize::from(op - 0x84);
                    if self.stack.len() < n || self.marks.last().is_some_and(|m| *m > self.stack.len() - n) {
                        return Err(bad());
                    }
                    let items = self.stack.split_off(self.stack.len() - n);
                    self.push(V::Tuple(Rc::new(items)))?;
                }
                b'}' => self.push(V::Dict(Rc::default()))?,
                b'd' => {
                    let items = self.since_mark()?;
                    let dict = V::Dict(Rc::default());
                    Self::set_items(&dict, items)?;
                    self.push(dict)?;
                }
                b's' => {
                    self.add(2)?;
                    let v = self.pop()?;
                    let k = self.pop()?;
                    let dict = self.top()?.clone();
                    Self::set_items(&dict, vec![k, v])?;
                }
                b'u' => {
                    let items = self.since_mark()?;
                    self.add(items.len())?;
                    let dict = self.top()?.clone();
                    Self::set_items(&dict, items)?;
                }
                0x8f => self.push(V::List(Rc::default()))?,
                0x90 => {
                    let items = self.since_mark()?;
                    self.add(items.len())?;
                    if let V::List(l) = self.top()? {
                        l.borrow_mut().extend(items);
                    }
                }
                0x91 => {
                    let items = self.since_mark()?;
                    self.push(V::List(Rc::new(RefCell::new(items))))?;
                }
                b'c' => {
                    let module = self.line()?;
                    let name = self.line()?;
                    self.push(V::Global(format!("{module}.{name}").into()))?;
                }
                0x93 => {
                    let name = self.pop()?;
                    let module = self.pop()?;
                    let g = format!("{}.{}", module.as_str().unwrap_or("?"), name.as_str().unwrap_or("?"));
                    self.push(V::Global(g.into()))?;
                }
                b'R' => {
                    let args = self.pop()?;
                    let callable = self.pop()?;
                    let v = reduce(&callable, args, 0);
                    self.push(v)?;
                }
                0x81 => {
                    let args = self.pop()?;
                    let class = self.pop()?;
                    let v = reduce(&class, args, 0);
                    self.push(v)?;
                }
                0x92 => {
                    let _kwargs = self.pop()?;
                    let args = self.pop()?;
                    let class = self.pop()?;
                    let v = reduce(&class, args, 0);
                    self.push(v)?;
                }
                b'o' => {
                    let mut items = self.since_mark()?;
                    if items.is_empty() {
                        return Err(bad());
                    }
                    let class = items.remove(0);
                    self.push(reduce(&class, V::Tuple(Rc::new(items)), 0))?;
                }
                b'i' => {
                    let module = self.line()?;
                    let name = self.line()?;
                    let items = self.since_mark()?;
                    let class = V::Global(format!("{module}.{name}").into());
                    self.push(reduce(&class, V::Tuple(Rc::new(items)), 0))?;
                }
                b'b' => {
                    let state = self.pop()?;
                    // A dict's state is its attributes (`_metadata`), not items: dropped.
                    if let V::Obj(o) = self.top()? {
                        *o.state.borrow_mut() = state;
                    }
                }
                b'p' => {
                    let key = self.line()?.parse::<u32>().map_err(|_| bad())?;
                    self.remember(key)?;
                }
                b'q' => {
                    let key = u32::from(self.u8()?);
                    self.remember(key)?;
                }
                b'r' => {
                    let key = self.u32()? as u32;
                    self.remember(key)?;
                }
                0x94 => {
                    let key = self.memo.len() as u32;
                    self.remember(key)?;
                }
                b'g' => {
                    let key = self.line()?.parse::<u32>().map_err(|_| bad())?;
                    self.recall(key)?;
                }
                b'h' => {
                    let key = u32::from(self.u8()?);
                    self.recall(key)?;
                }
                b'j' => {
                    let key = self.u32()? as u32;
                    self.recall(key)?;
                }
                b'Q' => {
                    let pid = self.pop()?;
                    self.push(persistent(&pid))?;
                }
                b'P' => {
                    let _ = self.line()?;
                    self.push(V::None)?;
                }
                // Extension registry codes, out-of-band buffers: not used by the files read here.
                _ => return Err(bad()),
            }
        }
        Err(bad())
    }
}

/// A persistent id: PyTorch names its storages `('storage', class, key, location, size, …)`.
fn persistent(pid: &V) -> V {
    let Some(items) = pid.items() else { return V::None };
    if items.first().and_then(V::as_str) != Some("storage") {
        return V::None;
    }
    let class = items.get(1).and_then(V::as_str).unwrap_or("?");
    let (dtype, item_size) = storage_dtype(class);
    V::Storage(Rc::new(Storage { dtype, item_size }))
}

fn int_list(v: &V) -> Option<Vec<u64>> {
    v.items()?.iter().map(|d| if let V::Int(n) = d { u64::try_from(*n).ok() } else { None }).collect()
}

/// What calling a class or function would give, for the few that matter; any other
/// call becomes an `Obj` naming what would have been called.
fn reduce(callable: &V, args: V, depth: usize) -> V {
    let name = callable.as_str().unwrap_or("?");
    let items = args.items().unwrap_or_default();
    let tensor = |note: &'static str| {
        let (dtype, item_size) = match items.first() {
            Some(V::Storage(s)) => (s.dtype.clone(), s.item_size),
            _ => ("?".to_string(), None),
        };
        V::Tensor(Rc::new(Tensor { dtype, item_size, shape: items.get(2).and_then(int_list), note }))
    };
    match name {
        "torch._utils._rebuild_tensor_v2" | "torch._utils._rebuild_tensor" | "torch._utils._rebuild_tensor_v3" => tensor(""),
        "torch._utils._rebuild_qtensor" => tensor("quantized"),
        "torch._utils._rebuild_parameter" | "torch._utils._rebuild_parameter_with_state" => items.first().cloned().unwrap_or(V::None),
        // A tensor subclass: the tensor is rebuilt by the function it names.
        "torch._tensor._rebuild_from_type_v2" | "torch._tensor._rebuild_from_type" if depth < 4 => match (items.first(), items.get(2)) {
            (Some(f), Some(a)) => reduce(f, a.clone(), depth + 1),
            _ => V::None,
        },
        "collections.OrderedDict" | "builtins.dict" | "__builtin__.dict" => V::Dict(Rc::default()),
        "builtins.set" | "builtins.frozenset" | "builtins.list" | "__builtin__.set" | "__builtin__.frozenset" | "__builtin__.list" => V::List(Rc::new(RefCell::new(items.first().and_then(V::items).unwrap_or_default()))),
        "torch.device" => V::Str(items.first().and_then(V::as_str).unwrap_or("device").into()),
        _ => V::Obj(Rc::new(Obj { class: name.into(), args, state: RefCell::new(V::None) })),
    }
}

/// Unpickle one pickle from the start of `b`; also returns where it ended.
pub fn load(b: &[u8]) -> Res<(V, usize)> {
    let mut m = Machine { b, i: 0, added: 0, stack: vec![], marks: vec![], memo: Default::default() };
    let v = m.run()?;
    Ok((v, m.i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_data_and_names_the_rest() {
        // {'a': [1, 2.5, 'x'], 'b': (True, None), 'c': os.system('…') never called}
        let mut p = vec![0x80, 2, b'}', b'q', 0, b'(', 0x8c, 1, b'a', b']', b'q', 1, b'(', b'K', 1, b'G'];
        p.extend(2.5f64.to_be_bytes());
        p.extend([0x8c, 1, b'x', b'e', 0x8c, 1, b'b', 0x88, b'N', 0x86, 0x8c, 1, b'c']);
        p.extend(b"cos\nsystem\n");
        p.extend([0x8c, 2, b'r', b'm', 0x85, b'R', b'u', b'.']);
        let (v, end) = load(&p).unwrap();
        assert_eq!(end, p.len());
        let V::Dict(d) = &v else { panic!("a dict") };
        let d = d.borrow();
        assert_eq!(d.len(), 3);
        let V::List(l) = &d[0].1 else { panic!() };
        assert!(matches!(l.borrow().as_slice(), [V::Int(1), V::Float(f), V::Str(s)] if *f == 2.5 && &**s == "x"));
        let V::Obj(o) = &d[2].1 else { panic!("an object, not a call") };
        assert_eq!(&*o.class, "os.system");
    }

    #[test]
    fn deep_nesting_is_dropped_without_recursion() {
        // [[[[…]]]] a million deep: a million empty lists, each appended to the one below.
        let mut p = vec![b']'; 1_000_000];
        p.extend(vec![b'a'; 999_999]);
        p.push(b'.');
        let (v, _) = load(&p).unwrap();
        drop(v);
    }

    #[test]
    fn refuses_damaged_pickles() {
        assert!(load(b"").is_err());
        assert!(load(&[0x80, 2, b'a']).is_err(), "append to nothing");
        assert!(load(b"(.").is_err(), "nothing above the mark");
        assert!(load(&[b'h', 7, b'.']).is_err(), "no such memo");
        assert!(load(&[b'X', 0xFF, 0xFF, 0xFF, 0x7F]).is_err(), "longer than the file");
        // Endless marks stop at the limit instead of filling the memory.
        assert!(load(&vec![b'('; 1000]).is_err());
        // A list appended to itself over and over stops at the limit.
        let mut p = vec![b']'];
        for _ in 0..3_000_000 {
            p.extend(b"2a");
        }
        assert!(load(&p).is_err());
    }
}
