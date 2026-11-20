//! Pictures from PDF files: the first page of a PDF, drawn as a form
//! XObject, so that diagrams stay vector graphics with searchable text.
//!
//! Nothing on the page is interpreted. Its content stream and the objects
//! its resources reach (fonts, images, patterns, other forms) are copied
//! into the document, renumbered. What is changed on the way:
//!
//! - Marked content (`BDC`, `BMC`, `EMC`, `MP`, `DP`) is taken out of the
//!   page's content and of the forms and patterns it uses, as it belongs
//!   to the other file's structure tree (Chromium tags the PDFs it
//!   writes); the picture is a `Figure` in sundowner's structure.
//! - Keys that tie objects to the other file (`StructParent`, `Metadata`,
//!   optional content, `PieceInfo`), and ones PDF/A forbids (`Interpolate`
//!   on images, `Alternates`, `OPI`, transfer functions), are left out.
//!
//! Some files are refused: encrypted ones, those that use fonts they do
//! not embed (viewers would substitute them, and sundowner promises that
//! they never do), PostScript XObjects, and filters that PDF/A forbids
//! (LZW) or that sundowner cannot decode where it needs to read a stream.
//!
//! The file comes from a filter, whose input is the document, or from the
//! document's directory, so it is treated as hostile: every read is
//! bounds-checked, nesting and reference chains are capped, cycles are
//! harmless, and decompression is limited.

use crate::flate;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

/// Deepest nesting of arrays and dictionaries, and longest chain of
/// references followed to read one object.
const MAX_DEPTH: usize = 32;
/// Most objects copied from one file.
const MAX_OBJECTS: usize = 100_000;
/// Largest decoded stream.
const MAX_DECODED: usize = 64 << 20;
/// Most decoded bytes per file, in all.
const MAX_TOTAL_DECODED: usize = 256 << 20;
/// Most cross-reference sections followed (incremental updates).
const MAX_SECTIONS: usize = 64;

/// A PDF object. In a [`Form`], streams hold their data as it is written
/// (encoded as their `Filter` says), and references are to other objects
/// of the form, by index.
#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Str(Vec<u8>),
    Name(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Dict),
    Stream(Dict, Vec<u8>),
    Ref(u32, u16),
}

/// A dictionary's entries, in order.
pub type Dict = Vec<(Vec<u8>, Obj)>;

fn get<'d>(d: &'d Dict, key: &[u8]) -> Option<&'d Obj> {
    d.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn set(d: &mut Dict, key: &[u8], v: Obj) {
    match d.iter_mut().find(|(k, _)| k == key) {
        Some(e) => e.1 = v,
        None => d.push((key.to_vec(), v)),
    }
}

impl Obj {
    fn num(&self) -> Option<f64> {
        match *self {
            Obj::Int(i) => Some(i as f64),
            Obj::Real(r) => Some(r),
            _ => None,
        }
    }

    fn int(&self) -> Option<i64> {
        match *self {
            Obj::Int(i) => Some(i),
            _ => None,
        }
    }

    fn name(&self) -> Option<&[u8]> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }

    fn dict(&self) -> Option<&Dict> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }
}

/// The first page of a PDF file, ready to be written as a form XObject.
#[derive(Clone, Debug)]
pub struct Form {
    /// The objects to write; the first is the form XObject.
    pub objects: Vec<Obj>,
    /// The page's size as shown (turned as its `Rotate` says), in points.
    pub width: f32,
    pub height: f32,
    /// How many pages the file has.
    pub pages: usize,
    /// Whether it uses CMYK colours (which PDF/A does not allow next to
    /// sundowner's RGB output intent).
    pub cmyk: bool,
    /// Whether it uses a font with no Unicode map, so that its text may not
    /// map to Unicode (PDF/A level B rather than A).
    pub unmapped: bool,
}

impl Form {
    /// About how many bytes it adds to a PDF.
    pub fn size(&self) -> usize {
        self.objects
            .iter()
            .map(|o| match o {
                Obj::Stream(_, data) => data.len() + 64,
                _ => 64,
            })
            .sum()
    }
}

/// Whether a file is a PDF file. The header may come after up to 1 KB of
/// something else, as readers allow.
pub fn is_pdf(data: &[u8]) -> bool {
    let head = &data[..data.len().min(1024)];
    head.windows(5).any(|w| w == b"%PDF-")
}

/// Read the first page of a PDF file.
pub fn load(data: &[u8]) -> Result<Form, String> {
    if data.len() as u64 > crate::image::MAX_FILE_BYTES {
        return Err(format!(
            "file is larger than {} MiB",
            crate::image::MAX_FILE_BYTES >> 20
        ));
    }
    let mut r = Reader::new(data);
    let trailer = r.trailer().map_err(|e| format!("malformed PDF: {e}"))?;
    if get(&trailer, b"Encrypt").is_some() {
        return Err("the PDF is encrypted".into());
    }
    page(&mut r, &trailer)
}

// ------------------------------------------------------------------ lexer

fn is_space(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn is_regular(b: u8) -> bool {
    !is_space(b) && !is_delim(b)
}

/// What the lexer finds next: an object, a keyword (an operator, in a
/// content stream), the end of an array or dictionary, or the end.
#[derive(Debug, PartialEq)]
enum Item<'a> {
    Obj(Obj),
    Kw(&'a [u8]),
    EndArray,
    EndDict,
    End,
}

struct Lexer<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn at(b: &'a [u8], pos: usize) -> Lexer<'a> {
        Lexer { b, pos }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while let Some(c) = self.peek() {
            if is_space(c) {
                self.pos += 1;
            } else if c == b'%' {
                while self.peek().is_some_and(|c| c != b'\n' && c != b'\r') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn regular_run(&mut self) -> &'a [u8] {
        let start = self.pos;
        while self.peek().is_some_and(is_regular) {
            self.pos += 1;
        }
        &self.b[start..self.pos]
    }

    /// The next item; references (`1 0 R`) are objects.
    fn next(&mut self, depth: usize) -> Result<Item<'a>, String> {
        if depth > MAX_DEPTH {
            return Err("objects nest too deeply".into());
        }
        self.skip_space();
        let Some(c) = self.peek() else {
            return Ok(Item::End);
        };
        let obj = match c {
            b'/' => {
                self.pos += 1;
                Obj::Name(decode_name(self.regular_run()))
            }
            b'(' => Obj::Str(self.literal()?),
            b'<' if self.b.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                let mut d = Dict::new();
                loop {
                    match self.next(depth + 1)? {
                        Item::EndDict => break,
                        Item::Obj(Obj::Name(k)) => match self.next(depth + 1)? {
                            Item::Obj(v) => set(&mut d, &k, v),
                            // A key without a value: its value is null.
                            Item::EndDict => break,
                            _ => return Err("bad dictionary value".into()),
                        },
                        _ => return Err("bad dictionary key".into()),
                    }
                }
                Obj::Dict(d)
            }
            b'<' => Obj::Str(self.hex()?),
            b'>' if self.b.get(self.pos + 1) == Some(&b'>') => {
                self.pos += 2;
                return Ok(Item::EndDict);
            }
            b'[' => {
                self.pos += 1;
                let mut a = Vec::new();
                loop {
                    match self.next(depth + 1)? {
                        Item::EndArray => break,
                        Item::Obj(o) => a.push(o),
                        _ => return Err("bad array element".into()),
                    }
                }
                Obj::Array(a)
            }
            b']' => {
                self.pos += 1;
                return Ok(Item::EndArray);
            }
            b'0'..=b'9' | b'+' | b'-' | b'.' => self.number(),
            c if is_regular(c) => {
                let kw = self.regular_run();
                match kw {
                    b"true" => Obj::Bool(true),
                    b"false" => Obj::Bool(false),
                    b"null" => Obj::Null,
                    _ => return Ok(Item::Kw(kw)),
                }
            }
            _ => return Err(format!("unexpected '{}'", c as char)),
        };
        Ok(Item::Obj(obj))
    }

    /// An object, where one must be.
    fn object(&mut self, depth: usize) -> Result<Obj, String> {
        match self.next(depth)? {
            Item::Obj(o) => Ok(o),
            _ => Err("expected an object".into()),
        }
    }

    /// A number, or a reference if two integers and `R` follow.
    fn number(&mut self) -> Obj {
        let run = self.regular_run();
        let text = std::str::from_utf8(run).unwrap_or("0");
        if !text.contains('.') {
            if let Ok(i) = text.parse::<i64>() {
                if (0..=u32::MAX as i64).contains(&i) {
                    let save = self.pos;
                    self.skip_space();
                    let gen = self.regular_run();
                    let gen_ok = !gen.is_empty() && gen.iter().all(u8::is_ascii_digit) && gen.len() <= 5;
                    self.skip_space();
                    if gen_ok
                        && self.peek() == Some(b'R')
                        && self.b.get(self.pos + 1).is_none_or(|&c| !is_regular(c))
                    {
                        self.pos += 1;
                        let g = std::str::from_utf8(gen)
                            .ok()
                            .and_then(|g| g.parse().ok())
                            .unwrap_or(0);
                        return Obj::Ref(i as u32, g);
                    }
                    self.pos = save;
                }
                return Obj::Int(i);
            }
        }
        // Readers take what they cannot read as a number as 0.
        match text.parse::<f64>() {
            Ok(r) if r.is_finite() && text.bytes().all(|c| c.is_ascii_digit() || b"+-.".contains(&c)) => {
                Obj::Real(r)
            }
            _ => Obj::Int(0),
        }
    }

    fn literal(&mut self) -> Result<Vec<u8>, String> {
        self.pos += 1;
        let mut out = Vec::new();
        let mut nest = 0usize;
        loop {
            let c = self.peek().ok_or("unterminated string")?;
            self.pos += 1;
            match c {
                b'(' => {
                    nest += 1;
                    out.push(c);
                }
                b')' if nest == 0 => return Ok(out),
                b')' => {
                    nest -= 1;
                    out.push(c);
                }
                b'\\' => {
                    let e = self.peek().ok_or("unterminated string")?;
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        // A line break after a backslash continues the line.
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        _ => out.push(e),
                    }
                }
                _ => out.push(c),
            }
        }
    }

    fn hex(&mut self) -> Result<Vec<u8>, String> {
        self.pos += 1;
        let mut out = Vec::new();
        let mut half: Option<u8> = None;
        loop {
            let c = self.peek().ok_or("unterminated hex string")?;
            self.pos += 1;
            let v = match c {
                b'>' => break,
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                c if is_space(c) => continue,
                _ => return Err("bad hex string".into()),
            };
            match half.take() {
                Some(h) => out.push(h << 4 | v),
                None => half = Some(v),
            }
        }
        if let Some(h) = half {
            out.push(h << 4);
        }
        Ok(out)
    }
}

fn decode_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if raw[i] == b'#' && i + 2 < raw.len() {
            if let (Some(h), Some(l)) = (hex(raw[i + 1]), hex(raw[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

// ----------------------------------------------------------------- reader

/// Where an object is: at a byte offset, or in an object stream at an
/// index; or free.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Entry {
    At(usize),
    In(u32, usize),
    Free,
}

/// A decoded object stream: its data, and where each object in it starts.
struct ObjStm {
    data: Vec<u8>,
    offsets: Vec<(u32, usize)>,
}

struct Reader<'a> {
    b: &'a [u8],
    xref: HashMap<u32, Entry>,
    objstms: HashMap<u32, std::rc::Rc<ObjStm>>,
    /// Decoded bytes still allowed.
    budget: usize,
    /// References being followed now, to cap chains and break cycles.
    depth: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        Reader {
            b,
            xref: HashMap::new(),
            objstms: HashMap::new(),
            budget: MAX_TOTAL_DECODED,
            depth: 0,
        }
    }

    /// Read the cross-reference sections and return the trailer. If they
    /// cannot be read, find the objects by scanning the file.
    fn trailer(&mut self) -> Result<Dict, String> {
        match self.read_xref() {
            Ok(t) if get(&t, b"Root").is_some() => Ok(t),
            _ => {
                self.xref.clear();
                self.scan()
            }
        }
    }

    fn read_xref(&mut self) -> Result<Dict, String> {
        let tail_start = self.b.len().saturating_sub(1024);
        let tail = &self.b[tail_start..];
        let at = tail
            .windows(9)
            .rposition(|w| w == b"startxref")
            .ok_or("no startxref")?;
        let mut lx = Lexer::at(self.b, tail_start + at + 9);
        let mut offset = match lx.object(0)? {
            Obj::Int(o) if o >= 0 => Some(o as usize),
            _ => return Err("bad startxref".into()),
        };
        let mut trailer: Option<Dict> = None;
        let mut seen = HashSet::new();
        while let Some(off) = offset.take() {
            if !seen.insert(off) || seen.len() > MAX_SECTIONS {
                break;
            }
            let section = self.xref_section(off)?;
            offset = get(&section, b"Prev").and_then(Obj::int).map(|p| p as usize);
            // A hybrid file's table has its streamed part elsewhere.
            if let Some(stm) = get(&section, b"XRefStm").and_then(Obj::int) {
                if seen.insert(stm as usize) {
                    let _ = self.xref_section(stm as usize);
                }
            }
            trailer.get_or_insert(section);
        }
        trailer.ok_or_else(|| "no trailer".into())
    }

    /// Read a cross-reference table or stream at `off`, adding the entries
    /// not already known (newer sections come first), and return its
    /// trailer dictionary.
    fn xref_section(&mut self, off: usize) -> Result<Dict, String> {
        let mut lx = Lexer::at(self.b, off);
        lx.skip_space();
        if self.b.get(lx.pos..lx.pos + 4) == Some(b"xref") {
            lx.pos += 4;
            loop {
                match lx.next(0)? {
                    Item::Kw(b"trailer") => {
                        return match lx.object(0)? {
                            Obj::Dict(d) => Ok(d),
                            _ => Err("bad trailer".into()),
                        };
                    }
                    Item::Obj(Obj::Int(first)) => {
                        let count = lx.object(0)?.int().ok_or("bad xref subsection")?;
                        if first < 0 || count < 0 || count as usize > self.b.len() / 18 {
                            return Err("bad xref subsection".into());
                        }
                        for k in 0..count {
                            let o = lx.object(0)?.int().ok_or("bad xref entry")?;
                            let _gen = lx.object(0)?;
                            let kind = match lx.next(0)? {
                                Item::Kw(k) => k,
                                _ => return Err("bad xref entry".into()),
                            };
                            let num = first.saturating_add(k) as u32;
                            let entry = match kind {
                                b"n" if o > 0 => Entry::At(o as usize),
                                _ => Entry::Free,
                            };
                            self.xref.entry(num).or_insert(entry);
                        }
                    }
                    _ => return Err("bad xref table".into()),
                }
            }
        }
        // A cross-reference stream.
        let (_, obj) = self.parse_at(off)?;
        let Obj::Stream(dict, raw) = obj else {
            return Err("no xref at startxref".into());
        };
        if get(&dict, b"Type").and_then(Obj::name) != Some(b"XRef") {
            return Err("no xref at startxref".into());
        }
        let data = self.decode(&dict, &raw)?;
        let w: Vec<usize> = match get(&dict, b"W") {
            Some(Obj::Array(a)) if a.len() == 3 => a
                .iter()
                .map(|x| x.int().filter(|&v| (0..=8).contains(&v)).map(|v| v as usize))
                .collect::<Option<_>>()
                .ok_or("bad xref /W")?,
            _ => return Err("bad xref /W".into()),
        };
        let row = w.iter().sum::<usize>().max(1);
        let size = get(&dict, b"Size").and_then(Obj::int).unwrap_or(0).max(0);
        let index: Vec<i64> = match get(&dict, b"Index") {
            Some(Obj::Array(a)) => a.iter().filter_map(Obj::int).collect(),
            _ => vec![0, size],
        };
        let field = |bytes: &[u8]| bytes.iter().fold(0u64, |v, &b| v << 8 | b as u64);
        let mut rows = data.chunks_exact(row);
        for pair in index.chunks_exact(2) {
            let (first, count) = (pair[0].max(0), pair[1].max(0));
            for k in 0..count {
                let Some(r) = rows.next() else { break };
                let (a, rest) = r.split_at(w[0]);
                let (b, c) = rest.split_at(w[1]);
                let kind = if w[0] == 0 { 1 } else { field(a) };
                let entry = match kind {
                    1 => Entry::At(field(b) as usize),
                    2 => Entry::In(field(b) as u32, field(c) as usize),
                    _ => Entry::Free,
                };
                self.xref.entry(first.saturating_add(k) as u32).or_insert(entry);
            }
        }
        Ok(dict)
    }

    /// Find the objects of a file whose cross-reference sections are
    /// broken, by scanning it for `n g obj`; later ones replace earlier
    /// ones, as incremental updates do. The trailer is the last `trailer`
    /// dictionary, or the dictionary of a cross-reference stream, or one
    /// made up for the catalog.
    fn scan(&mut self) -> Result<Dict, String> {
        let b = self.b;
        let mut found: Vec<(u32, usize)> = Vec::new();
        let mut trailer = None;
        let mut i = 0;
        while i < b.len() {
            let start_ok = i == 0 || !is_regular(b[i - 1]);
            if start_ok && b[i].is_ascii_digit() {
                let mut lx = Lexer::at(b, i);
                if let (Ok(Item::Obj(Obj::Int(n))), Ok(Item::Obj(Obj::Int(_))), Ok(Item::Kw(b"obj"))) =
                    (lx.next(0), lx.next(0), lx.next(0))
                {
                    if (0..=u32::MAX as i64).contains(&n) {
                        found.push((n as u32, i));
                        i = lx.pos;
                        continue;
                    }
                }
            } else if start_ok && b[i..].starts_with(b"trailer") {
                let mut lx = Lexer::at(b, i + 7);
                if let Ok(Obj::Dict(d)) = lx.object(0) {
                    trailer = Some(d);
                }
            }
            i += 1;
        }
        for &(n, off) in &found {
            self.xref.insert(n, Entry::At(off));
        }
        // Objects in object streams, unless also found on their own.
        let mut catalog = None;
        let mut xref_dict = None;
        for &(n, off) in &found {
            let Ok((_, obj)) = self.parse_at(off) else {
                continue;
            };
            let Some(d) = obj.dict() else { continue };
            match get(d, b"Type").and_then(Obj::name) {
                Some(b"ObjStm") => {
                    if let Ok(stm) = self.objstm(n) {
                        for (k, &(m, _)) in stm.offsets.iter().enumerate() {
                            self.xref.entry(m).or_insert(Entry::In(n, k));
                        }
                    }
                }
                Some(b"XRef") if get(d, b"Root").is_some() => xref_dict = Some(d.clone()),
                Some(b"Catalog") => catalog = Some(n),
                _ => {}
            }
        }
        let trailer = trailer
            .filter(|t| get(t, b"Root").is_some())
            .or(xref_dict)
            .or_else(|| catalog.map(|c| vec![(b"Root".to_vec(), Obj::Ref(c, 0))]));
        trailer.ok_or_else(|| "no catalog".into())
    }

    /// Parse the object at a byte offset: `n g obj ... endobj`, with its
    /// stream's data if it has one.
    fn parse_at(&mut self, off: usize) -> Result<(u32, Obj), String> {
        let b = self.b;
        let mut lx = Lexer::at(b, off);
        let num = match (lx.next(0)?, lx.next(0)?, lx.next(0)?) {
            (Item::Obj(Obj::Int(n)), Item::Obj(Obj::Int(_)), Item::Kw(b"obj")) => n as u32,
            _ => return Err("bad object header".into()),
        };
        let obj = lx.object(0)?;
        let Obj::Dict(dict) = obj else {
            return Ok((num, obj));
        };
        lx.skip_space();
        if !b[lx.pos..].starts_with(b"stream") {
            return Ok((num, Obj::Dict(dict)));
        }
        let mut start = lx.pos + 6;
        if b.get(start) == Some(&b'\r') {
            start += 1;
        }
        if b.get(start) == Some(&b'\n') {
            start += 1;
        }
        let length = match get(&dict, b"Length") {
            Some(Obj::Int(n)) => Some(*n),
            Some(r @ Obj::Ref(..)) => self.resolve(r).ok().and_then(|o| o.int()),
            _ => None,
        };
        let declared = length
            .filter(|&n| n >= 0)
            .map(|n| start.saturating_add(n as usize))
            .filter(|&end| end <= b.len())
            .filter(|&end| {
                let mut l = Lexer::at(b, end);
                l.skip_space();
                b[l.pos..].starts_with(b"endstream")
            });
        let end = match declared {
            Some(end) => end,
            None => {
                // Find `endstream`, and leave out the line break before it.
                let at = b[start..]
                    .windows(9)
                    .position(|w| w == b"endstream")
                    .ok_or("stream without endstream")?;
                let mut end = start + at;
                if end > start && b[end - 1] == b'\n' {
                    end -= 1;
                }
                if end > start && b[end - 1] == b'\r' {
                    end -= 1;
                }
                end
            }
        };
        Ok((num, Obj::Stream(dict, b[start..end].to_vec())))
    }

    /// Object number `num`; null if there is none.
    fn object(&mut self, num: u32) -> Result<Obj, String> {
        if self.depth >= MAX_DEPTH {
            return Err("references nest too deeply".into());
        }
        self.depth += 1;
        let r = self.object_inner(num);
        self.depth -= 1;
        r
    }

    fn object_inner(&mut self, num: u32) -> Result<Obj, String> {
        match self.xref.get(&num).copied() {
            Some(Entry::At(off)) => {
                let (n, obj) = self.parse_at(off)?;
                if n != num {
                    return Err(format!("object {num} is not where the xref says"));
                }
                Ok(obj)
            }
            Some(Entry::In(stm, k)) => {
                let s = self.objstm(stm)?;
                let &(n, off) = s.offsets.get(k).ok_or("bad object stream index")?;
                if n != num {
                    return Err(format!("object {num} is not where the xref says"));
                }
                Lexer::at(&s.data, off).object(0)
            }
            Some(Entry::Free) | None => Ok(Obj::Null),
        }
    }

    fn objstm(&mut self, num: u32) -> Result<std::rc::Rc<ObjStm>, String> {
        if let Some(s) = self.objstms.get(&num) {
            return Ok(s.clone());
        }
        let Obj::Stream(dict, raw) = self.object(num)? else {
            return Err("bad object stream".into());
        };
        let data = self.decode(&dict, &raw)?;
        let n = get(&dict, b"N").and_then(Obj::int).unwrap_or(0).max(0) as usize;
        let first = get(&dict, b"First").and_then(Obj::int).unwrap_or(0).max(0) as usize;
        if n > data.len() / 2 + 1 {
            return Err("bad object stream".into());
        }
        let mut lx = Lexer::at(&data, 0);
        let mut offsets = Vec::with_capacity(n);
        for _ in 0..n {
            let (Some(m), Some(o)) = (lx.object(0)?.int(), lx.object(0)?.int()) else {
                return Err("bad object stream".into());
            };
            if m < 0 || o < 0 {
                return Err("bad object stream".into());
            }
            offsets.push((m as u32, first.saturating_add(o as usize)));
        }
        let s = std::rc::Rc::new(ObjStm { data, offsets });
        self.objstms.insert(num, s.clone());
        Ok(s)
    }

    /// Follow a reference (or references to references).
    fn resolve(&mut self, o: &Obj) -> Result<Obj, String> {
        let mut o = o.clone();
        for _ in 0..MAX_DEPTH {
            match o {
                Obj::Ref(n, _) => o = self.object(n)?,
                other => return Ok(other),
            }
        }
        Err("references nest too deeply".into())
    }

    fn resolve_dict(&mut self, o: Option<&Obj>) -> Result<Option<Dict>, String> {
        match o {
            None => Ok(None),
            Some(o) => match self.resolve(o)? {
                Obj::Dict(d) => Ok(Some(d)),
                _ => Ok(None),
            },
        }
    }

    /// Decode a stream's data through its filters.
    fn decode(&mut self, dict: &Dict, raw: &[u8]) -> Result<Vec<u8>, String> {
        let filters = match get(dict, b"Filter").map(|f| self.resolve(f)).transpose()? {
            None | Some(Obj::Null) => Vec::new(),
            Some(Obj::Name(n)) => vec![n],
            Some(Obj::Array(a)) => a
                .into_iter()
                .filter_map(|f| f.name().map(<[u8]>::to_vec))
                .collect(),
            _ => return Err("bad stream filter".into()),
        };
        let parms = match get(dict, b"DecodeParms").map(|p| self.resolve(p)).transpose()? {
            Some(Obj::Array(a)) => a,
            Some(d @ Obj::Dict(_)) => vec![d],
            _ => Vec::new(),
        };
        let mut data = raw.to_vec();
        for (k, f) in filters.iter().enumerate() {
            let limit = MAX_DECODED.min(self.budget);
            data = match f.as_slice() {
                b"FlateDecode" | b"Fl" => {
                    let out =
                        flate::zlib_decompress(&data, limit).map_err(|e| format!("bad Flate data: {e}"))?;
                    match parms.get(k) {
                        Some(Obj::Dict(p)) => predict(p, out)?,
                        _ => out,
                    }
                }
                b"ASCIIHexDecode" | b"AHx" => ascii_hex(&data),
                b"ASCII85Decode" | b"A85" => ascii85(&data)?,
                other => {
                    return Err(format!(
                        "it has a stream compressed with {}, which sundowner cannot read",
                        String::from_utf8_lossy(other)
                    ))
                }
            };
            if data.len() > limit {
                return Err("a stream decompresses to too much data".into());
            }
        }
        self.budget = self.budget.saturating_sub(data.len());
        Ok(data)
    }
}

/// Undo a PNG predictor (`/Predictor` 10 to 15), as cross-reference
/// streams use.
fn predict(p: &Dict, data: Vec<u8>) -> Result<Vec<u8>, String> {
    let int = |k: &[u8], d: i64| get(p, k).and_then(Obj::int).unwrap_or(d);
    let predictor = int(b"Predictor", 1);
    if predictor < 10 {
        return if predictor <= 1 {
            Ok(data)
        } else {
            Err("TIFF predictors are not supported".into())
        };
    }
    let colors = int(b"Colors", 1).clamp(1, 32) as usize;
    let bpc = int(b"BitsPerComponent", 8).clamp(1, 16) as usize;
    let columns = int(b"Columns", 1).clamp(1, 1 << 24) as usize;
    let bpp = (colors * bpc).div_ceil(8);
    let row = (colors * bpc * columns).div_ceil(8);
    let height = data.len() / (row + 1);
    crate::image::unfilter(&data, row, height, bpp).ok_or_else(|| "bad predictor data".into())
}

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut half: Option<u8> = None;
    for &c in data {
        let v = match c {
            b'>' => break,
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => continue,
        };
        match half.take() {
            Some(h) => out.push(h << 4 | v),
            None => half = Some(v),
        }
    }
    if let Some(h) = half {
        out.push(h << 4);
    }
    out
}

fn ascii85(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut group = [0u8; 5];
    let mut n = 0;
    let data = data.strip_prefix(b"<~").unwrap_or(data);
    for &c in data {
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[n] = c - b'!';
                n += 1;
                if n == 5 {
                    let v = group.iter().fold(0u64, |v, &d| v * 85 + d as u64);
                    if v > u32::MAX as u64 {
                        return Err("bad ASCII85 data".into());
                    }
                    out.extend_from_slice(&(v as u32).to_be_bytes());
                    n = 0;
                }
            }
            c if is_space(c) => {}
            _ => return Err("bad ASCII85 data".into()),
        }
    }
    if n == 1 {
        return Err("bad ASCII85 data".into());
    }
    if n > 1 {
        for d in group.iter_mut().skip(n) {
            *d = 84;
        }
        let v = group.iter().fold(0u64, |v, &d| v * 85 + d as u64);
        out.extend_from_slice(&(v.min(u32::MAX as u64) as u32).to_be_bytes()[..n - 1]);
    }
    Ok(out)
}

// ------------------------------------------------------------------- page

/// The first page, and what it inherits from the page tree.
fn page(r: &mut Reader, trailer: &Dict) -> Result<Form, String> {
    let catalog = r
        .resolve_dict(get(trailer, b"Root"))?
        .ok_or("malformed PDF: no catalog")?;
    let pages_ref = get(&catalog, b"Pages")
        .cloned()
        .ok_or("malformed PDF: no pages")?;
    let pages = r
        .resolve_dict(Some(&pages_ref))?
        .ok_or("malformed PDF: no pages")?;
    let count = get(&pages, b"Count").and_then(Obj::int).unwrap_or(1).max(0) as usize;

    // Depth-first to the first leaf, carrying inheritable attributes.
    let mut inherited: Dict = Vec::new();
    let mut node = pages;
    let mut seen = HashSet::new();
    let leaf = 'walk: {
        for _ in 0..MAX_DEPTH {
            for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
                if let Some(v) = get(&node, key) {
                    set(&mut inherited, key, v.clone());
                }
            }
            let Some(kids) = get(&node, b"Kids") else {
                break 'walk node;
            };
            let kids = match r.resolve(kids)? {
                Obj::Array(a) => a,
                _ => return Err("malformed PDF: bad page tree".into()),
            };
            let mut next = None;
            for kid in kids.iter().take(1024) {
                if let Obj::Ref(n, _) = kid {
                    if !seen.insert(*n) {
                        continue;
                    }
                }
                if let Some(d) = r.resolve_dict(Some(kid))? {
                    // Skip empty subtrees.
                    if get(&d, b"Kids").is_none() || get(&d, b"Count").and_then(Obj::int) != Some(0) {
                        next = Some(d);
                        break;
                    }
                }
            }
            node = next.ok_or("the PDF has no pages")?;
        }
        return Err("malformed PDF: page tree too deep".into());
    };
    for (k, v) in leaf {
        set(&mut inherited, &k, v);
    }
    let page = inherited;

    // The page's box, as shown.
    let rect = |r: &mut Reader, key: &[u8]| -> Result<Option<[f64; 4]>, String> {
        let Some(v) = get(&page, key) else { return Ok(None) };
        let Obj::Array(a) = r.resolve(v)? else {
            return Ok(None);
        };
        let mut n = Vec::new();
        for x in &a {
            n.push(r.resolve(x)?.num());
        }
        Ok(match n.as_slice() {
            [Some(a), Some(b), Some(c), Some(d)] => Some([a.min(*c), b.min(*d), a.max(*c), b.max(*d)]),
            _ => None,
        })
    };
    let media = rect(r, b"MediaBox")?.unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let bbox = match rect(r, b"CropBox")? {
        Some(c) => [
            c[0].max(media[0]),
            c[1].max(media[1]),
            c[2].min(media[2]),
            c[3].min(media[3]),
        ],
        None => media,
    };
    let (w, h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    if !(w > 0.0 && h > 0.0 && w < 1e6 && h < 1e6 && bbox.iter().all(|v| v.abs() < 1e7)) {
        return Err("the PDF's page has no size".into());
    }
    let rotate = match get(&page, b"Rotate").map(|v| r.resolve(v)).transpose()? {
        Some(Obj::Int(d)) => d.rem_euclid(360) / 90 * 90,
        _ => 0,
    };
    let unit = match get(&page, b"UserUnit").map(|v| r.resolve(v)).transpose()? {
        Some(o) => o.num().filter(|u| *u > 0.0 && *u < 100.0).unwrap_or(1.0),
        None => 1.0,
    };
    // Map the box to the first quadrant, turned clockwise by `rotate`.
    let [x0, y0, ..] = bbox;
    let (matrix, shown) = match rotate {
        90 => ([0.0, -1.0, 1.0, 0.0, -y0, w + x0], (h, w)),
        180 => ([-1.0, 0.0, 0.0, -1.0, w + x0, h + y0], (w, h)),
        270 => ([0.0, 1.0, -1.0, 0.0, h + y0, -x0], (h, w)),
        _ => ([1.0, 0.0, 0.0, 1.0, -x0, -y0], (w, h)),
    };

    let mut c = Copier {
        r,
        map: HashMap::new(),
        objects: vec![Obj::Null],
        queue: Vec::new(),
        cmyk: false,
        unmapped: false,
    };
    let content = c.page_content(get(&page, b"Contents"))?;
    let resources = match get(&page, b"Resources") {
        Some(res) => c.copy(res, 0)?,
        None => Obj::Dict(Vec::new()),
    };
    let mut dict: Dict = vec![
        (b"Type".to_vec(), Obj::Name(b"XObject".to_vec())),
        (b"Subtype".to_vec(), Obj::Name(b"Form".to_vec())),
        (
            b"BBox".to_vec(),
            Obj::Array(bbox.iter().map(|&v| Obj::Real(v)).collect()),
        ),
        (
            b"Matrix".to_vec(),
            Obj::Array(matrix.iter().map(|&v| Obj::Real(v * unit)).collect()),
        ),
        (b"Resources".to_vec(), resources),
    ];
    if let Some(group) = get(&page, b"Group") {
        let group = c.copy(group, 0)?;
        dict.push((b"Group".to_vec(), group));
    }
    dict.push((b"Filter".to_vec(), Obj::Name(b"FlateDecode".to_vec())));
    c.objects[0] = Obj::Stream(dict, flate::zlib_compress(&content));
    c.run()?;
    Ok(Form {
        objects: c.objects,
        width: (shown.0 * unit) as f32,
        height: (shown.1 * unit) as f32,
        pages: count.max(1),
        cmyk: c.cmyk,
        unmapped: c.unmapped,
    })
}

/// Copies objects from the file into a [`Form`], renumbering them.
struct Copier<'r, 'a> {
    r: &'r mut Reader<'a>,
    /// The index in `objects` of each object copied, by object number.
    map: HashMap<u32, usize>,
    objects: Vec<Obj>,
    /// Objects to copy: object number and index.
    queue: Vec<(u32, usize)>,
    cmyk: bool,
    unmapped: bool,
}

/// Keys left out of copied dictionaries: they tie an object to the other
/// file (its structure tree, metadata, optional content, private data), or
/// serve only marked content, which is taken out, or PDF/A forbids them.
const DROP: [&[u8]; 13] = [
    b"StructParent",
    b"StructParents",
    b"Metadata",
    b"OC",
    b"Properties",
    b"PieceInfo",
    b"LastModified",
    b"Interpolate",
    b"Alternates",
    b"OPI",
    b"TR",
    b"TR2",
    b"Ref",
];

impl Copier<'_, '_> {
    /// The page's content: its streams, decoded, joined, and cleaned.
    fn page_content(&mut self, contents: Option<&Obj>) -> Result<Vec<u8>, String> {
        let streams = match contents.map(|c| self.r.resolve(c)).transpose()? {
            None | Some(Obj::Null) => Vec::new(),
            Some(s @ Obj::Stream(..)) => vec![s],
            Some(Obj::Array(a)) => {
                let mut v = Vec::new();
                for s in a.iter().take(10_000) {
                    v.push(self.r.resolve(s)?);
                }
                v
            }
            _ => return Err("malformed PDF: bad page contents".into()),
        };
        let mut data = Vec::new();
        for s in streams {
            if let Obj::Stream(d, raw) = s {
                data.extend_from_slice(&self.r.decode(&d, &raw)?);
                data.push(b'\n');
            }
        }
        clean_content(&data, &mut self.cmyk)
    }

    /// Copy a value, queueing the objects it refers to.
    fn copy(&mut self, o: &Obj, depth: usize) -> Result<Obj, String> {
        if depth > MAX_DEPTH {
            return Err("objects nest too deeply".into());
        }
        Ok(match o {
            Obj::Ref(n, _) => {
                let idx = match self.map.get(n) {
                    Some(&i) => i,
                    None => {
                        if self.objects.len() >= MAX_OBJECTS {
                            return Err("the PDF has too many objects".into());
                        }
                        let i = self.objects.len();
                        self.objects.push(Obj::Null);
                        self.map.insert(*n, i);
                        self.queue.push((*n, i));
                        i
                    }
                };
                Obj::Ref(idx as u32, 0)
            }
            Obj::Array(a) => Obj::Array(
                a.iter()
                    .map(|v| self.copy(v, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            Obj::Dict(d) => Obj::Dict(self.copy_dict(d, depth)?),
            Obj::Name(n) => {
                if n == b"DeviceCMYK" {
                    self.cmyk = true;
                }
                o.clone()
            }
            // Streams are only ever objects of their own.
            Obj::Stream(..) => Obj::Null,
            other => other.clone(),
        })
    }

    fn copy_dict(&mut self, d: &Dict, depth: usize) -> Result<Dict, String> {
        self.check(d)?;
        let mut out = Dict::with_capacity(d.len());
        for (k, v) in d {
            if DROP.contains(&k.as_slice()) || k == b"Length" {
                continue;
            }
            out.push((k.clone(), self.copy(v, depth + 1)?));
        }
        Ok(out)
    }

    /// Refuse what cannot be embedded, and note what affects PDF/A.
    fn check(&mut self, d: &Dict) -> Result<(), String> {
        let name = |k: &[u8]| get(d, k).and_then(Obj::name);
        if name(b"Subtype") == Some(b"PS") || get(d, b"PS").is_some() {
            return Err("it has PostScript in it".into());
        }
        if name(b"Type") == Some(b"Font") {
            let sub = name(b"Subtype").unwrap_or(b"");
            // Fonts with outlines of their own must embed them. (Type 3
            // fonts draw their glyphs themselves, and a Type 0 font's
            // descendant is checked as a font of its own.)
            if matches!(
                sub,
                b"Type1" | b"MMType1" | b"TrueType" | b"CIDFontType0" | b"CIDFontType2"
            ) {
                let descriptor = self.r.resolve_dict(get(d, b"FontDescriptor"))?;
                let embedded = descriptor.is_some_and(|fd| {
                    [&b"FontFile"[..], b"FontFile2", b"FontFile3"]
                        .iter()
                        .any(|k| get(&fd, k).is_some())
                });
                if !embedded {
                    return Err(not_embedded(d, b"BaseFont"));
                }
            }
            if matches!(sub, b"Type1" | b"TrueType" | b"MMType1" | b"Type3" | b"Type0")
                && get(d, b"ToUnicode").is_none()
            {
                self.unmapped = true;
            }
        }
        Ok(())
    }

    /// Copy the queued objects, and those they refer to.
    fn run(&mut self) -> Result<(), String> {
        while let Some((num, idx)) = self.queue.pop() {
            let obj = self.r.object(num)?;
            self.objects[idx] = match obj {
                Obj::Stream(d, raw) => self.copy_stream(&d, raw)?,
                other => self.copy(&other, 0)?,
            };
        }
        Ok(())
    }

    fn copy_stream(&mut self, d: &Dict, raw: Vec<u8>) -> Result<Obj, String> {
        let filters = match get(d, b"Filter") {
            Some(f) => match self.r.resolve(f)? {
                Obj::Name(n) => vec![n],
                Obj::Array(a) => a
                    .into_iter()
                    .filter_map(|f| f.name().map(<[u8]>::to_vec))
                    .collect(),
                _ => Vec::new(),
            },
            None => Vec::new(),
        };
        if filters.iter().any(|f| f == b"LZWDecode" || f == b"LZW") {
            return Err("it has LZW-compressed data, which PDF/A does not allow".into());
        }
        let name = |k: &[u8]| get(d, k).and_then(Obj::name);
        let content =
            name(b"Subtype") == Some(b"Form") || get(d, b"PatternType").and_then(Obj::int) == Some(1);
        let mut dict = self.copy_dict(d, 0)?;
        if !content {
            return Ok(Obj::Stream(dict, raw));
        }
        // Forms and tiling patterns are content: clean them like the page.
        let data = self.r.decode(d, &raw)?;
        let data = clean_content(&data, &mut self.cmyk)?;
        dict.retain(|(k, _)| k != b"Filter" && k != b"DecodeParms" && k != b"DL");
        dict.push((b"Filter".to_vec(), Obj::Name(b"FlateDecode".to_vec())));
        Ok(Obj::Stream(dict, flate::zlib_compress(&data)))
    }
}

fn not_embedded(d: &Dict, key: &[u8]) -> String {
    let font = get(d, key)
        .and_then(Obj::name)
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_else(|| "a font".into());
    format!("it uses {font} without embedding it")
}

/// Take marked content out of a content stream, and note whether it sets
/// CMYK colours. Everything else is kept byte for byte.
fn clean_content(data: &[u8], cmyk: &mut bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len());
    let mut lx = Lexer::at(data, 0);
    // Copied up to here; and where the current operator's operands start.
    let mut copied = 0;
    let mut operands: Option<usize> = None;
    loop {
        lx.skip_space();
        let start = lx.pos;
        match lx.next(0)? {
            Item::End => break,
            Item::Obj(o) => {
                if o.name() == Some(b"DeviceCMYK") {
                    *cmyk = true;
                }
                operands.get_or_insert(start);
            }
            Item::Kw(op) => {
                match op {
                    b"BDC" | b"BMC" | b"EMC" | b"MP" | b"DP" => {
                        out.extend_from_slice(&data[copied..operands.unwrap_or(start)]);
                        out.push(b' ');
                        copied = lx.pos;
                    }
                    b"k" | b"K" => *cmyk = true,
                    b"BI" => skip_inline_image(&mut lx)?,
                    _ => {}
                }
                operands = None;
            }
            // Stray closing brackets: leave them be.
            Item::EndArray | Item::EndDict => {}
        }
    }
    out.extend_from_slice(&data[copied..]);
    Ok(out)
}

/// Skip an inline image, `BI <dict> ID <data> EI`, after `BI`.
fn skip_inline_image(lx: &mut Lexer) -> Result<(), String> {
    let mut length = None;
    loop {
        match lx.next(1)? {
            Item::Kw(b"ID") => break,
            Item::Obj(Obj::Name(k)) => {
                let v = lx.object(1)?;
                if k == b"L" || k == b"Length" {
                    length = v.int().filter(|&n| n >= 0).map(|n| n as usize);
                }
            }
            Item::End => return Err("unterminated inline image".into()),
            _ => return Err("bad inline image".into()),
        }
    }
    // One white-space character, then the data.
    let start = lx.pos + 1;
    let b = lx.b;
    if let Some(len) = length {
        let mut l = Lexer::at(b, start.saturating_add(len).min(b.len()));
        l.skip_space();
        if b[l.pos..].starts_with(b"EI") {
            lx.pos = l.pos + 2;
            return Ok(());
        }
    }
    // `EI` between white space (or at the end).
    let mut i = start;
    while i + 2 <= b.len() {
        if &b[i..i + 2] == b"EI" && i > 0 && is_space(b[i - 1]) && b.get(i + 2).is_none_or(|&c| is_space(c)) {
            lx.pos = i + 2;
            return Ok(());
        }
        i += 1;
    }
    Err("unterminated inline image".into())
}

// ------------------------------------------------------------------ write

/// Write an object, its references mapped to object numbers by `ids`.
pub fn write(out: &mut String, o: &Obj, ids: &[usize]) {
    match o {
        Obj::Null => out.push_str("null"),
        Obj::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Obj::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Obj::Real(r) => {
            // Rust prints floats without exponents, and as short as
            // round-trips; keep it to a sensible precision.
            let r = if r.is_finite() {
                (r * 1e6).round() / 1e6
            } else {
                0.0
            };
            let _ = write!(out, "{}", if r == 0.0 { 0.0 } else { r });
        }
        Obj::Str(s) => {
            out.push('<');
            for b in s {
                let _ = write!(out, "{b:02X}");
            }
            out.push('>');
        }
        Obj::Name(n) => write_name(out, n),
        Obj::Array(a) => {
            out.push('[');
            for (k, v) in a.iter().enumerate() {
                if k > 0 {
                    out.push(' ');
                }
                write(out, v, ids);
            }
            out.push(']');
        }
        Obj::Dict(d) => {
            out.push_str("<<");
            write_entries(out, d, ids);
            out.push_str(" >>");
        }
        // Streams are written by the caller, with `write_entries`.
        Obj::Stream(..) => out.push_str("null"),
        Obj::Ref(i, _) => match ids.get(*i as usize) {
            Some(id) => {
                let _ = write!(out, "{id} 0 R");
            }
            None => out.push_str("null"),
        },
    }
}

/// Write a dictionary's entries, without the brackets.
pub fn write_entries(out: &mut String, d: &Dict, ids: &[usize]) {
    for (k, v) in d {
        out.push(' ');
        write_name(out, k);
        out.push(' ');
        write(out, v, ids);
    }
}

fn write_name(out: &mut String, n: &[u8]) {
    out.push('/');
    for &b in n {
        if (0x21..=0x7E).contains(&b) && !is_delim(b) && b != b'#' {
            out.push(b as char);
        } else {
            let _ = write!(out, "#{b:02X}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PDF file with a classic cross-reference table, from objects
    /// numbered from 1 (`stream` bodies are written as given).
    pub(crate) fn pdf(objects: &[&str], trailer_extra: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (k, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", k + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R{trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    fn stream(dict: &str, data: &str) -> String {
        format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
    }

    /// A one-page PDF with the given content and resources.
    fn simple(content: &str, resources: &str, page_extra: &str) -> Vec<u8> {
        pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>",
                &format!(
                    "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources {resources}{page_extra} >>"
                ),
                &stream("", content),
            ],
            "",
        )
    }

    fn content(f: &Form) -> String {
        let Obj::Stream(_, data) = &f.objects[0] else {
            panic!()
        };
        String::from_utf8(flate::zlib_decompress(data, 1 << 20).unwrap()).unwrap()
    }

    fn entry<'f>(f: &'f Form, key: &[u8]) -> &'f Obj {
        get(f.objects[0].dict().unwrap(), key).unwrap()
    }

    #[test]
    fn lexes_objects() {
        let src = b"<< /A 1 /B -2.5 /C (a (b) \\) \\101\\\nc) /D <48 65 6c6> /E [true false null /N#20x] /F 12 0 R /G .5 >>";
        let o = Lexer::at(src, 0).object(0).unwrap();
        let d = o.dict().unwrap();
        assert_eq!(get(d, b"A"), Some(&Obj::Int(1)));
        assert_eq!(get(d, b"B"), Some(&Obj::Real(-2.5)));
        assert_eq!(get(d, b"C"), Some(&Obj::Str(b"a (b) ) Ac".to_vec())));
        assert_eq!(get(d, b"D"), Some(&Obj::Str(b"Hel`".to_vec())));
        assert_eq!(
            get(d, b"E"),
            Some(&Obj::Array(vec![
                Obj::Bool(true),
                Obj::Bool(false),
                Obj::Null,
                Obj::Name(b"N x".to_vec())
            ]))
        );
        assert_eq!(get(d, b"F"), Some(&Obj::Ref(12, 0)));
        assert_eq!(get(d, b"G"), Some(&Obj::Real(0.5)));
        // Two integers not followed by R are two integers.
        let mut lx = Lexer::at(b"[1 2 3 R 4 5]", 0);
        assert_eq!(
            lx.object(0).unwrap(),
            Obj::Array(vec![Obj::Int(1), Obj::Ref(2, 3), Obj::Int(4), Obj::Int(5)])
        );
    }

    #[test]
    fn reads_a_simple_page() {
        let f = load(&simple("0 0 1 rg 10 10 50 50 re f", "<< >>", "")).unwrap();
        assert_eq!((f.width, f.height), (200.0, 100.0));
        assert_eq!(f.pages, 1);
        assert!(!f.cmyk && !f.unmapped);
        assert_eq!(content(&f).trim(), "0 0 1 rg 10 10 50 50 re f");
        let mut s = String::new();
        write(&mut s, entry(&f, b"Matrix"), &[]);
        assert_eq!(s, "[1 0 0 1 0 0]");
    }

    #[test]
    fn copies_resources_and_renumbers_them() {
        let mut objs = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 4 0 R \
             /Resources << /XObject << /X 5 0 R >> /ExtGState << /G 6 0 R >> >> /StructParents 0 >>"
                .to_string(),
            stream("", "/X Do"),
        ];
        // A form with marked content of its own, and a graphics state that
        // refers back to it (a cycle).
        objs.push(stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /StructParents 3 /Resources << /ExtGState << /G 6 0 R >> >>",
            "/P <</MCID 0>> BDC 0 0 1 1 re f EMC",
        ));
        objs.push(
            "<< /Type /ExtGState /CA 0.5 /SMask << /S /Luminosity /G 5 0 R >> /TR /Identity >>".to_string(),
        );
        let refs: Vec<&str> = objs.iter().map(String::as_str).collect();
        let f = load(&pdf(&refs, "")).unwrap();
        assert_eq!(f.objects.len(), 3, "{:?}", f.objects);
        let mut all = String::new();
        let ids: Vec<usize> = (100..103).collect();
        for o in &f.objects {
            match o {
                Obj::Stream(d, data) => {
                    write_entries(&mut all, d, &ids);
                    all.push_str(&String::from_utf8_lossy(
                        &flate::zlib_decompress(data, 1 << 20).unwrap(),
                    ));
                }
                o => write(&mut all, o, &ids),
            }
            all.push('\n');
        }
        assert!(!all.contains("StructParents") && !all.contains("/TR"), "{all}");
        assert!(
            !all.contains("BDC") && !all.contains("EMC") && !all.contains("MCID"),
            "{all}"
        );
        assert!(all.contains("0 0 1 1 re f"), "{all}");
        assert!(all.contains("/G 102 0 R") && all.contains("/G 101 0 R"), "{all}");
    }

    #[test]
    fn marked_content_is_removed_and_the_rest_kept() {
        let mut cmyk = false;
        let src = b"/Artifact BMC 1 g EMC/Span<</MCID 3/ActualText(x)>>BDC BT /F1 1 Tf (a)Tj ET EMC /P MP q BI /W 2 /H 1 /CS /G /BPC 8 ID \x00EI\xff EI Q";
        let out = clean_content(src, &mut cmyk).unwrap();
        let out = String::from_utf8_lossy(&out);
        assert!(
            !out.contains("BMC") && !out.contains("BDC") && !out.contains("EMC") && !out.contains("MP"),
            "{out}"
        );
        assert!(
            out.contains("1 g")
                && out.contains("BT /F1 1 Tf (a)Tj ET")
                && out.contains("ID \u{0}EI\u{FFFD} EI Q"),
            "{out}"
        );
        assert!(!cmyk);
        clean_content(b"0 0 0 1 k", &mut cmyk).unwrap();
        assert!(cmyk);
    }

    #[test]
    fn rotation_turns_the_box() {
        for (rot, matrix, size) in [
            (90, "[0 -1 1 0 -20 210]", (100.0, 200.0)),
            (180, "[-1 0 0 -1 210 120]", (200.0, 100.0)),
            (-90, "[0 1 -1 0 120 -10]", (100.0, 200.0)),
        ] {
            let f = load(&pdf(
                &[
                    "<< /Type /Catalog /Pages 2 0 R >>",
                    &format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /Rotate {rot} >>"),
                    "<< /Type /Page /Parent 2 0 R /MediaBox [10 20 210 120] >>",
                ],
                "",
            ))
            .unwrap();
            let mut s = String::new();
            write(&mut s, entry(&f, b"Matrix"), &[]);
            assert_eq!(s, matrix, "{rot}");
            assert_eq!((f.width, f.height), size, "{rot}");
        }
    }

    #[test]
    fn reads_xref_streams_and_object_streams() {
        // Objects 1 to 3 in an object stream (4), a content stream (5), and
        // a cross-reference stream (6) with a PNG predictor.
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 50 40] /Contents 5 0 R >>",
        ];
        let mut index = String::new();
        let mut body = String::new();
        for (k, o) in objs.iter().enumerate() {
            index.push_str(&format!("{} {} ", k + 1, body.len()));
            body.push_str(o);
            body.push('\n');
        }
        let objstm = flate::zlib_compress(format!("{index}\n{body}").as_bytes());
        let mut out = b"%PDF-1.7\n".to_vec();
        let at4 = out.len();
        out.extend_from_slice(
            format!(
                "4 0 obj\n<< /Type /ObjStm /N 3 /First {} /Filter /FlateDecode /Length {} >>\nstream\n",
                index.len() + 1,
                objstm.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&objstm);
        out.extend_from_slice(b"\nendstream\nendobj\n");
        let at5 = out.len();
        out.extend_from_slice(b"5 0 obj\n<< /Length 7 >>\nstream\n1 0 0 rg\nendstream\nendobj\n");
        let at6 = out.len();
        let rows: Vec<[u8; 4]> = vec![
            [0, 0, 0, 0],
            [2, 0, 4, 0],
            [2, 0, 4, 1],
            [2, 0, 4, 2],
            [1, (at4 >> 8) as u8, at4 as u8, 0],
            [1, (at5 >> 8) as u8, at5 as u8, 0],
            [1, (at6 >> 8) as u8, at6 as u8, 0],
        ];
        // PNG "Up" filter on each row.
        let mut raw = Vec::new();
        let mut prev = [0u8; 4];
        for r in &rows {
            raw.push(2);
            for k in 0..4 {
                raw.push(r[k].wrapping_sub(prev[k]));
            }
            prev = *r;
        }
        let xs = flate::zlib_compress(&raw);
        out.extend_from_slice(
            format!(
                "6 0 obj\n<< /Type /XRef /Size 7 /W [1 2 1] /Root 1 0 R /Filter /FlateDecode \
                 /DecodeParms << /Predictor 12 /Columns 4 >> /Length {} >>\nstream\n",
                xs.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&xs);
        out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{at6}\n%%EOF\n").as_bytes());
        let f = load(&out).unwrap();
        assert_eq!((f.width, f.height), (50.0, 40.0));
        assert_eq!(content(&f).trim(), "1 0 0 rg");
        // The same file with its cross-reference stream broken is found by
        // scanning.
        let mut broken = out.clone();
        let n = broken.len();
        broken[n - 12] = b'9';
        let f = load(&broken).unwrap();
        assert_eq!(content(&f).trim(), "1 0 0 rg");
    }

    #[test]
    fn incremental_updates_replace_objects() {
        let mut file = simple("1 g", "<< >>", "");
        let first_xref = {
            let s = String::from_utf8_lossy(&file).into_owned();
            let at = s.rfind("startxref\n").unwrap() + 10;
            s[at..].lines().next().unwrap().parse::<usize>().unwrap()
        };
        let at = file.len();
        file.extend_from_slice(b"4 0 obj\n<< /Length 3 >>\nstream\n0 g\nendstream\nendobj\n");
        let xref = file.len();
        file.extend_from_slice(
            format!(
                "xref\n4 1\n{at:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R /Prev {first_xref} >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        assert_eq!(content(&load(&file).unwrap()).trim(), "0 g");
    }

    #[test]
    fn refuses_what_cannot_be_embedded() {
        let e = load(&simple("", "<< >>", "")[..]).map(|_| ());
        assert!(e.is_ok());
        let enc = pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [] /Count 0 >>",
            ],
            " /Encrypt << /Filter /Standard >>",
        );
        assert!(load(&enc).unwrap_err().contains("encrypted"));
        let empty = pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [] /Count 0 >>",
            ],
            "",
        );
        assert!(load(&empty).unwrap_err().contains("no pages"));
        let helvetica = simple(
            "BT /F1 12 Tf (x) Tj ET",
            "<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>",
            "",
        );
        assert_eq!(
            load(&helvetica).unwrap_err(),
            "it uses Helvetica without embedding it"
        );
        let ps = simple(
            "",
            "<< /XObject << /P << /Type /XObject /Subtype /PS >> >> >>",
            "",
        );
        assert!(load(&ps).unwrap_err().contains("PostScript"));
        assert!(load(b"%PDF-1.4\ngarbage").is_err());
    }

    #[test]
    fn notes_cmyk_and_fonts_without_unicode_maps() {
        let f = load(&simple("0 0 0 1 k", "<< >>", "")).unwrap();
        assert!(f.cmyk);
        let f = load(&simple(
            "",
            "<< /ColorSpace << /C [/ICCBased 9 0 R] /D /DeviceCMYK >> >>",
            "",
        ))
        .unwrap();
        assert!(f.cmyk);
        let f = load(&simple(
            "",
            "<< /Font << /F << /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs << >> /Encoding << >> >> >> >>",
            "",
        ))
        .unwrap();
        assert!(f.unmapped && !f.cmyk);
    }

    #[test]
    fn hostile_files_fail_cleanly() {
        // Deep nesting, reference cycles, and lengths pointing nowhere.
        let deep = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
        assert!(load(&simple("", &deep, "")).is_err());
        let cyc = pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [2 0 R 3 0 R] /Count 1 >>",
                "<< /Type /Page /Parent 2 0 R /Resources 4 0 R /Contents 5 0 R >>",
                "5 0 R",
                "<< /Length 5 0 R >>\nstream\nq Q\nendstream",
            ],
            "",
        );
        let _ = load(&cyc);
        // Mutations of a valid file never panic.
        let base = simple(
            "0 0 1 rg 10 10 50 50 re f",
            "<< /ExtGState << /G << /CA 1 >> >> >>",
            " /Rotate 90",
        );
        let mut seed = 99u64;
        for _ in 0..3000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut f = base.clone();
            for k in 0..(seed % 6 + 1) {
                let at = ((seed >> (8 * k)) as usize) % f.len();
                f[at] = (seed >> (k + 3)) as u8;
            }
            let cut = (seed >> 40) as usize % (f.len() + 1);
            let _ = load(&f[..cut.max(f.len() / 2)]);
            let _ = load(&f);
        }
    }

    #[test]
    fn decodes_ascii_filters() {
        assert_eq!(ascii_hex(b"48 65 6C 6c 6F>"), b"Hello");
        assert_eq!(ascii85(b"<~87cURD]i,\"Ebo80~>").unwrap(), b"Hello World!");
        assert_eq!(ascii85(b"z~>").unwrap(), [0; 4]);
    }
}
