//! Block-level Markdown parser (CommonMark core plus GFM tables, task lists
//! and strikethrough). Inline content is kept as raw text and parsed lazily by
//! [`crate::inline`] once all link reference definitions are known.

use std::collections::HashMap;

/// Deeper nesting than this is rendered as plain text. It bounds recursion so
/// hostile input cannot overflow the stack.
pub const MAX_DEPTH: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(String),
    Code(Vec<String>),
    Quote(Vec<Block>),
    List {
        ordered: bool,
        start: u64,
        tight: bool,
        items: Vec<Item>,
    },
    Rule,
    Table {
        aligns: Vec<Align>,
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

#[derive(Debug)]
pub struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

pub struct Document {
    pub blocks: Vec<Block>,
    /// Link reference definitions, keyed by normalized label.
    pub refs: HashMap<String, String>,
}

pub fn parse(src: &str) -> Document {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let lines: Vec<String> = src
        .split('\n')
        .map(|l| expand_tabs(l.strip_suffix('\r').unwrap_or(l)))
        .collect();
    let mut refs = HashMap::new();
    let blocks = parse_blocks(&lines, 0, &mut refs);
    Document { blocks, refs }
}

/// Normalize a link label for case-insensitive lookup.
pub fn normalize_label(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') && !line.contains('\r') && !line.contains('\0') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0;
    for c in line.chars() {
        match c {
            '\t' => {
                let n = 4 - col % 4;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            }
            '\r' => {}
            '\0' => {
                out.push('\u{FFFD}');
                col += 1;
            }
            _ => {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

fn indent(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

fn is_blank(s: &str) -> bool {
    s.bytes().all(|b| b == b' ')
}

/// Remove up to `n` leading spaces.
fn strip_indent(s: &str, n: usize) -> &str {
    &s[indent(s).min(n)..]
}

struct Fence {
    indent: usize,
    ch: u8,
    len: usize,
}

fn fence_start(s: &str) -> Option<Fence> {
    let ind = indent(s);
    if ind > 3 {
        return None;
    }
    let rest = &s[ind..];
    let ch = *rest.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = rest.bytes().take_while(|&b| b == ch).count();
    if len < 3 {
        return None;
    }
    if ch == b'`' && rest[len..].contains('`') {
        return None;
    }
    Some(Fence { indent: ind, ch, len })
}

fn is_fence_end(s: &str, f: &Fence) -> bool {
    let ind = indent(s);
    if ind > 3 {
        return false;
    }
    let rest = &s[ind..];
    let n = rest.bytes().take_while(|&b| b == f.ch).count();
    n >= f.len && is_blank(&rest[n..])
}

fn atx_heading(s: &str) -> Option<(u8, String)> {
    let ind = indent(s);
    if ind > 3 {
        return None;
    }
    let rest = &s[ind..];
    let level = rest.bytes().take_while(|&b| b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let body = &rest[level..];
    if !body.is_empty() && !body.starts_with(' ') {
        return None;
    }
    let mut text = body.trim();
    // Optional closing sequence of #s, preceded by a space.
    let stripped = text.trim_end_matches('#');
    if stripped.is_empty() {
        text = "";
    } else if stripped.len() != text.len() && stripped.ends_with(' ') {
        text = stripped.trim_end();
    }
    Some((level as u8, text.to_string()))
}

fn is_rule(s: &str) -> bool {
    if indent(s) > 3 {
        return false;
    }
    let mut ch = 0u8;
    let mut count = 0;
    for b in s.bytes() {
        match b {
            b' ' => {}
            b'-' | b'*' | b'_' if ch == 0 || ch == b => {
                ch = b;
                count += 1;
            }
            _ => return false,
        }
    }
    count >= 3
}

fn setext_level(s: &str) -> Option<u8> {
    if indent(s) > 3 {
        return None;
    }
    let t = s.trim();
    if !t.is_empty() && t.bytes().all(|b| b == b'=') {
        Some(1)
    } else if !t.is_empty() && t.bytes().all(|b| b == b'-') {
        Some(2)
    } else {
        None
    }
}

fn quote_strip(s: &str) -> Option<&str> {
    let ind = indent(s);
    if ind > 3 {
        return None;
    }
    let rest = s[ind..].strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

#[derive(Clone, Copy)]
struct Marker {
    ordered: bool,
    /// Bullet character, or the delimiter (`.` / `)`) for ordered lists.
    ch: u8,
    start: u64,
    /// Column where item content begins.
    content: usize,
    /// True when nothing follows the marker on its line.
    empty: bool,
}

fn list_marker(s: &str) -> Option<Marker> {
    let ind = indent(s);
    if ind > 3 || is_rule(s) {
        return None;
    }
    let b = s.as_bytes();
    let (ordered, ch, start, end) = match b.get(ind)? {
        c @ (b'-' | b'*' | b'+') => (false, *c, 0, ind + 1),
        b'0'..=b'9' => {
            let digits = b[ind..].iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 9 {
                return None;
            }
            let d = *b.get(ind + digits)?;
            if d != b'.' && d != b')' {
                return None;
            }
            let start = s[ind..ind + digits].parse().ok()?;
            (true, d, start, ind + digits + 1)
        }
        _ => return None,
    };
    let after = &s[end..];
    if after.is_empty() || is_blank(after) {
        return Some(Marker {
            ordered,
            ch,
            start,
            content: end + 1,
            empty: true,
        });
    }
    let sp = indent(after);
    if sp == 0 {
        return None;
    }
    let content = if sp > 4 { end + 1 } else { end + sp };
    Some(Marker {
        ordered,
        ch,
        start,
        content,
        empty: false,
    })
}

/// Can this line interrupt a paragraph?
fn interrupts_paragraph(s: &str) -> bool {
    if fence_start(s).is_some()
        || atx_heading(s).is_some()
        || is_rule(s)
        || quote_strip(s).is_some()
        || s.trim_start().starts_with("<!--")
    {
        return true;
    }
    match list_marker(s) {
        Some(m) => !m.empty && (!m.ordered || m.start == 1),
        None => false,
    }
}

/// Split a table row into trimmed cells.
fn split_row(s: &str) -> Vec<String> {
    let mut t = s.trim();
    if let Some(r) = t.strip_prefix('|') {
        t = r;
    }
    if t.ends_with('|') && !t.ends_with("\\|") {
        t = &t[..t.len() - 1];
    }
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('\\');
                cur.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

fn delimiter_row(s: &str) -> Option<Vec<Align>> {
    if !s.contains('|') && !s.contains(':') {
        return None;
    }
    if indent(s) > 3 {
        return None;
    }
    split_row(s)
        .iter()
        .map(|c| {
            let left = c.starts_with(':');
            let right = c.len() > 1 && c.ends_with(':');
            let inner = c.trim_start_matches(':').trim_end_matches(':');
            if inner.is_empty() || !inner.bytes().all(|b| b == b'-') {
                return None;
            }
            Some(match (left, right) {
                (true, true) => Align::Center,
                (true, false) => Align::Left,
                (false, true) => Align::Right,
                _ => Align::None,
            })
        })
        .collect()
}

/// Parse `[label]: destination "optional title"` on a single line.
fn link_ref_def(s: &str) -> Option<(String, String)> {
    if indent(s) > 3 {
        return None;
    }
    let rest = s.trim_start().strip_prefix('[')?;
    let close = rest.find("]:")?;
    let label = &rest[..close];
    if label.trim().is_empty() || label.contains('[') || label.contains(']') || label.starts_with('^') {
        return None;
    }
    let after = rest[close + 2..].trim();
    let (dest, tail) = if let Some(a) = after.strip_prefix('<') {
        let end = a.find('>')?;
        (&a[..end], &a[end + 1..])
    } else {
        let end = after.find(' ').unwrap_or(after.len());
        (&after[..end], &after[end..])
    };
    if dest.is_empty() {
        return None;
    }
    let tail = tail.trim();
    let title_ok = tail.is_empty()
        || (tail.len() >= 2
            && ((tail.starts_with('"') && tail.ends_with('"'))
                || (tail.starts_with('\'') && tail.ends_with('\''))
                || (tail.starts_with('(') && tail.ends_with(')'))));
    if !title_ok {
        return None;
    }
    Some((normalize_label(label), dest.to_string()))
}

fn parse_blocks(lines: &[String], depth: usize, refs: &mut HashMap<String, String>) -> Vec<Block> {
    let mut out = Vec::new();
    if depth >= MAX_DEPTH {
        let text: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        if !text.is_empty() {
            out.push(Block::Paragraph(text.join("\n")));
        }
        return out;
    }
    let n = lines.len();
    let mut i = 0;
    while i < n {
        let line = lines[i].as_str();
        if is_blank(line) {
            i += 1;
            continue;
        }

        // Indented code block.
        if indent(line) >= 4 {
            let mut code = Vec::new();
            while i < n && (is_blank(&lines[i]) || indent(&lines[i]) >= 4) {
                code.push(strip_indent(&lines[i], 4).to_string());
                i += 1;
            }
            while code.last().is_some_and(|l| is_blank(l)) {
                code.pop();
            }
            out.push(Block::Code(code));
            continue;
        }

        if let Some(f) = fence_start(line) {
            i += 1;
            let mut code = Vec::new();
            while i < n && !is_fence_end(&lines[i], &f) {
                code.push(strip_indent(&lines[i], f.indent).to_string());
                i += 1;
            }
            i += 1; // closing fence (or end of input)
            out.push(Block::Code(code));
            continue;
        }

        if let Some((level, text)) = atx_heading(line) {
            out.push(Block::Heading { level, text });
            i += 1;
            continue;
        }

        if is_rule(line) {
            out.push(Block::Rule);
            i += 1;
            continue;
        }

        if quote_strip(line).is_some() {
            let mut inner: Vec<String> = Vec::new();
            let mut lazy_ok = false;
            while i < n {
                let l = lines[i].as_str();
                if let Some(s) = quote_strip(l) {
                    lazy_ok = !is_blank(s) && fence_start(s).is_none() && indent(s) < 4;
                    inner.push(s.to_string());
                } else if lazy_ok && !is_blank(l) && !interrupts_paragraph(l) {
                    inner.push(l.trim_start().to_string());
                } else {
                    break;
                }
                i += 1;
            }
            out.push(Block::Quote(parse_blocks(&inner, depth + 1, refs)));
            continue;
        }

        if let Some(first) = list_marker(line) {
            out.push(parse_list(lines, &mut i, first, depth, refs));
            continue;
        }

        if line.trim_start().starts_with("<!--") {
            while i < n && !lines[i].contains("-->") {
                i += 1;
            }
            i += 1;
            continue;
        }

        if line.contains('|') && i + 1 < n {
            if let Some(aligns) = delimiter_row(&lines[i + 1]) {
                let header = split_row(line);
                if header.len() == aligns.len() {
                    let cols = aligns.len();
                    i += 2;
                    let mut rows = Vec::new();
                    while i < n && !is_blank(&lines[i]) && !interrupts_paragraph(&lines[i]) {
                        let mut row = split_row(&lines[i]);
                        row.resize(cols, String::new());
                        rows.push(row);
                        i += 1;
                    }
                    out.push(Block::Table { aligns, header, rows });
                    continue;
                }
            }
        }

        // Paragraph, possibly turned into a setext heading.
        let mut para: Vec<&str> = vec![line.trim()];
        let mut heading = None;
        i += 1;
        while i < n {
            let l = lines[i].as_str();
            if is_blank(l) {
                break;
            }
            if let Some(level) = setext_level(l) {
                heading = Some(level);
                i += 1;
                break;
            }
            if interrupts_paragraph(l) {
                break;
            }
            para.push(l.trim_start());
            i += 1;
        }
        // Link reference definitions at the start of a paragraph.
        let mut skip = 0;
        while skip < para.len() {
            match link_ref_def(para[skip]) {
                Some((label, dest)) => {
                    refs.entry(label).or_insert(dest);
                    skip += 1;
                }
                None => break,
            }
        }
        let para = &para[skip..];
        if para.is_empty() {
            continue;
        }
        // Keep trailing double spaces (hard breaks) except on the last line.
        let mut text = para.join("\n");
        text.truncate(text.trim_end().len());
        match heading {
            Some(level) => out.push(Block::Heading { level, text }),
            None => out.push(Block::Paragraph(text)),
        }
    }
    out
}

fn parse_list(
    lines: &[String],
    i: &mut usize,
    first: Marker,
    depth: usize,
    refs: &mut HashMap<String, String>,
) -> Block {
    let n = lines.len();
    let mut items = Vec::new();
    let mut tight = true;
    let mut marker = first;
    loop {
        let w = marker.content;
        let line = lines[*i].as_str();
        let mut item: Vec<String> = vec![line.get(w..).unwrap_or("").to_string()];
        *i += 1;
        let mut prev_blank = marker.empty;
        if !(marker.empty && *i < n && is_blank(&lines[*i])) {
            while *i < n {
                let l = lines[*i].as_str();
                if is_blank(l) {
                    item.push(String::new());
                    prev_blank = true;
                } else if indent(l) >= w {
                    item.push(l[w..].to_string());
                    prev_blank = false;
                } else if !prev_blank && list_marker(l).is_none() && !interrupts_paragraph(l) {
                    // Lazy paragraph continuation.
                    item.push(l.trim_start().to_string());
                } else {
                    break;
                }
                *i += 1;
            }
        }
        let mut trailing = 0;
        while item.last().is_some_and(|l| is_blank(l)) {
            item.pop();
            trailing += 1;
        }
        // A blank line between an item's blocks makes the list loose.
        if item.iter().any(|l| is_blank(l)) && !item.iter().any(|l| fence_start(l).is_some()) {
            tight = false;
        }

        let mut task = None;
        if let Some(first) = item.first_mut() {
            for (pat, checked) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
                if let Some(rest) = first.strip_prefix(pat) {
                    if rest.is_empty() || rest.starts_with(' ') {
                        task = Some(checked);
                        *first = rest.trim_start().to_string();
                        break;
                    }
                }
            }
        }
        items.push(Item {
            task,
            blocks: parse_blocks(&item, depth + 1, refs),
        });

        match lines.get(*i).and_then(|l| list_marker(l)) {
            Some(m) if m.ordered == first.ordered && m.ch == first.ch => {
                if trailing > 0 {
                    tight = false;
                }
                marker = m;
            }
            _ => break,
        }
    }
    Block::List {
        ordered: first.ordered,
        start: first.start,
        tight,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_and_paragraphs() {
        let d = parse("# Title #\n\nSome *text*\nmore\n\nSub\n---\n");
        assert!(matches!(&d.blocks[0], Block::Heading { level: 1, text } if text == "Title"));
        assert!(matches!(&d.blocks[1], Block::Paragraph(t) if t == "Some *text*\nmore"));
        assert!(matches!(&d.blocks[2], Block::Heading { level: 2, text } if text == "Sub"));
    }

    #[test]
    fn nested_lists() {
        let d = parse("- a\n- b\n  - c\n  - d\n- [x] e\n\n1. one\n2. two\n");
        match &d.blocks[0] {
            Block::List {
                ordered: false,
                items,
                tight: true,
                ..
            } => {
                assert_eq!(items.len(), 3);
                assert!(matches!(&items[1].blocks[1], Block::List { items, .. } if items.len() == 2));
                assert_eq!(items[2].task, Some(true));
            }
            b => panic!("{b:?}"),
        }
        assert!(matches!(
            &d.blocks[1],
            Block::List {
                ordered: true,
                start: 1,
                ..
            }
        ));
    }

    #[test]
    fn code_quote_table() {
        let d = parse("```rust\nfn x() {}\n\n```\n> quote\nlazy\n\n| a | b |\n|:--|--:|\n| 1 | 2 |\n");
        assert!(matches!(&d.blocks[0], Block::Code(l) if l.len() == 2));
        assert!(
            matches!(&d.blocks[1], Block::Quote(b) if matches!(&b[0], Block::Paragraph(t) if t == "quote\nlazy"))
        );
        match &d.blocks[2] {
            Block::Table { aligns, header, rows } => {
                assert_eq!(aligns, &[Align::Left, Align::Right]);
                assert_eq!(header, &["a", "b"]);
                assert_eq!(rows[0], ["1", "2"]);
            }
            b => panic!("{b:?}"),
        }
    }

    #[test]
    fn link_refs_and_rules() {
        let d = parse("[x]: http://example.com \"t\"\n\n***\n");
        assert_eq!(d.refs.get("x").map(String::as_str), Some("http://example.com"));
        assert!(matches!(d.blocks[0], Block::Rule));
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let src = ">".repeat(100_000) + " deep";
        let d = parse(&src);
        assert_eq!(d.blocks.len(), 1);
        let src = (0..500)
            .map(|i| format!("{}- x", "  ".repeat(i)))
            .collect::<Vec<_>>()
            .join("\n");
        let _ = parse(&src);
    }
}
