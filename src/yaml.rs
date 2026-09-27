//! A reader for the YAML that front matter is written in, to show it, not
//! to interpret it: scalars are kept as the text they are written as
//! (`2024-01-01` and `true` stay text), and only their quoting and line
//! folding are undone.
//!
//! Supported: block mappings and sequences (including a sequence at the
//! same indentation as its key), flow collections (`[a, b]`, `{a: 1}`),
//! plain, single- and double-quoted scalars, literal (`|`) and folded (`>`)
//! block scalars, comments and tags (which are dropped). Anchors, aliases
//! and complex (`?`) keys are rejected. Input comes from documents, so
//! nesting depth and size are bounded.

/// Deepest nesting accepted. It bounds recursion, so hostile input cannot
/// overflow the stack; front matter this deep could not be shown anyway.
pub const MAX_DEPTH: usize = 32;
/// Most nodes accepted.
const MAX_NODES: usize = 20_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A scalar's text; null (`~`, `null` or nothing) is empty.
    Scalar(String),
    Map(Vec<(String, Node)>),
    Seq(Vec<Node>),
}

/// Parse `src`, whose first line is line `first_line` of the file (for
/// error messages).
pub fn parse(src: &str, first_line: usize) -> Result<Node, String> {
    let mut p = Parser {
        lines: src.lines().map(str::to_string).collect(),
        i: 0,
        nodes: 0,
        first_line,
    };
    let node = match p.skip() {
        Some(n) => p.node(n, -1, 0)?,
        None => Node::Scalar(String::new()),
    };
    if p.skip().is_some() {
        return Err(p.err("unexpected content"));
    }
    Ok(node)
}

/// Whether `line` starts a block mapping at the left margin (`key: ...`).
pub fn starts_mapping(line: &str) -> bool {
    !line.starts_with(' ') && split_key(line).is_some()
}

fn indent(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

fn is_blank(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t.starts_with('#')
}

/// Whether block content starts a sequence entry (`-` then space or end).
fn is_dash(content: &str) -> bool {
    content == "-" || content.starts_with("- ")
}

/// Drop a trailing comment (` #...`) from plain text.
fn strip_comment(s: &str) -> &str {
    if s.starts_with('#') {
        return "";
    }
    match s.find(" #") {
        Some(k) => s[..k].trim_end(),
        None => s.trim_end(),
    }
}

fn plain(s: &str) -> Node {
    match s {
        "~" | "null" | "Null" | "NULL" => Node::Scalar(String::new()),
        _ => Node::Scalar(s.to_string()),
    }
}

enum Quoted {
    /// Closed at this byte offset (just past the closing quote).
    Closed(usize),
    /// The line ended inside the string; `true` if it ended in an escaped
    /// line break (a backslash), which joins the lines without a space.
    Open(bool),
}

/// Scan a quoted string's text on one line, starting just after the
/// opening quote (or at the start of a continuation line), appending the
/// characters it stands for to `out`.
fn scan_quoted(s: &str, q: char, out: &mut String) -> Result<Quoted, String> {
    let mut chars = s.char_indices().peekable();
    while let Some((k, c)) = chars.next() {
        if c == q {
            if q == '\'' && chars.peek().map(|&(_, c)| c) == Some('\'') {
                out.push('\'');
                chars.next();
                continue;
            }
            return Ok(Quoted::Closed(k + 1));
        }
        if c != '\\' || q != '"' {
            out.push(c);
            continue;
        }
        let Some((_, e)) = chars.next() else {
            return Ok(Quoted::Open(true));
        };
        let hex = |chars: &mut std::iter::Peekable<std::str::CharIndices>, n: usize| {
            let digits: String = (0..n).filter_map(|_| chars.next().map(|(_, c)| c)).collect();
            u32::from_str_radix(&digits, 16)
                .ok()
                .filter(|_| digits.len() == n)
                .and_then(char::from_u32)
                .ok_or_else(|| format!("bad escape '\\{e}{digits}'"))
        };
        out.push(match e {
            'n' => '\n',
            't' | '\t' => '\t',
            'r' => '\r',
            '0' => '\0',
            'a' => '\u{7}',
            'b' => '\u{8}',
            'e' => '\u{1B}',
            'f' => '\u{C}',
            'v' => '\u{B}',
            ' ' => ' ',
            '"' => '"',
            '/' => '/',
            '\\' => '\\',
            'N' => '\u{85}',
            '_' => '\u{A0}',
            'L' => '\u{2028}',
            'P' => '\u{2029}',
            'x' => hex(&mut chars, 2)?,
            'u' => hex(&mut chars, 4)?,
            'U' => hex(&mut chars, 8)?,
            _ => return Err(format!("unknown escape '\\{e}'")),
        });
    }
    Ok(Quoted::Open(false))
}

/// Split `key: rest` (a block mapping entry) into its key and the rest,
/// trimmed. `None` if the line is not a mapping entry.
fn split_key(content: &str) -> Option<(String, &str)> {
    let first = content.chars().next()?;
    if first == '"' || first == '\'' {
        let mut key = String::new();
        let Ok(Quoted::Closed(end)) = scan_quoted(&content[1..], first, &mut key) else {
            return None;
        };
        let rest = content[1 + end..].trim_start();
        let rest = rest.strip_prefix(':')?;
        if !(rest.is_empty() || rest.starts_with(' ')) {
            return None;
        }
        return Some((key, rest.trim()));
    }
    if "[]{},#&*!|>%@`".contains(first) || is_dash(content) || content == "?" || content.starts_with("? ") {
        return None;
    }
    let b = content.as_bytes();
    for k in 0..b.len() {
        match b[k] {
            b'#' if k > 0 && b[k - 1] == b' ' => return None,
            b':' if k + 1 == b.len() || b[k + 1] == b' ' => {
                let key = content[..k].trim_end();
                return (!key.is_empty()).then(|| (key.to_string(), content[k + 1..].trim()));
            }
            _ => {}
        }
    }
    None
}

struct Parser {
    lines: Vec<String>,
    /// The current line.
    i: usize,
    nodes: usize,
    first_line: usize,
}

impl Parser {
    fn err(&self, m: &str) -> String {
        format!("line {}: {m}", self.i + self.first_line)
    }

    /// Move to the next line with content, returning its indentation.
    fn skip(&mut self) -> Option<usize> {
        while self.i < self.lines.len() && is_blank(&self.lines[self.i]) {
            self.i += 1;
        }
        self.lines.get(self.i).map(|l| indent(l))
    }

    fn count(&mut self, depth: usize) -> Result<(), String> {
        self.nodes += 1;
        if depth > MAX_DEPTH {
            Err(self.err("nested too deeply"))
        } else if self.nodes > MAX_NODES {
            Err(self.err("too large"))
        } else {
            Ok(())
        }
    }

    /// The block node whose first line is the current one, indented by `n`,
    /// inside a node indented by `parent` (-1 at the top).
    fn node(&mut self, n: usize, parent: isize, depth: usize) -> Result<Node, String> {
        self.count(depth)?;
        let content = &self.lines[self.i][n..];
        if is_dash(content) {
            self.seq(n, depth)
        } else if split_key(content).is_some() {
            self.map(n, depth)
        } else {
            let content = content.to_string();
            self.value(&content, parent, depth)
        }
    }

    fn seq(&mut self, n: usize, depth: usize) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(ind) = self.skip() {
            if ind < n || !is_dash(&self.lines[self.i][n..]) {
                if ind > n {
                    return Err(self.err("unexpected indentation"));
                }
                break;
            }
            let rest = &self.lines[self.i][n + 1..];
            if is_blank(rest) {
                self.i += 1;
                items.push(match self.skip() {
                    Some(next) if next > n => self.node(next, n as isize, depth + 1)?,
                    _ => Node::Scalar(String::new()),
                });
            } else {
                // Blank out the dash: the item is then a block node indented
                // to where its content starts, and so are its following
                // lines (`- a: 1` / `  b: 2`).
                let col = n + 1 + indent(rest);
                self.lines[self.i].replace_range(n..n + 1, " ");
                items.push(self.node(col, n as isize, depth + 1)?);
            }
        }
        Ok(Node::Seq(items))
    }

    fn map(&mut self, n: usize, depth: usize) -> Result<Node, String> {
        let mut entries = Vec::new();
        while let Some(ind) = self.skip() {
            if ind < n {
                break;
            }
            if ind > n {
                return Err(self.err("unexpected indentation"));
            }
            let line = self.lines[self.i][n..].to_string();
            if is_dash(&line) {
                return Err(self.err("sequence entry inside a mapping"));
            }
            let (key, rest) = split_key(&line).ok_or_else(|| {
                if line.starts_with('?') {
                    self.err("complex keys are not supported")
                } else {
                    self.err("expected 'key: value'")
                }
            })?;
            let value = self.value(rest, n as isize, depth + 1)?;
            entries.push((key, value));
        }
        Ok(Node::Map(entries))
    }

    /// The value `rest` found on the current line, after a key or dash, in
    /// a node indented by `owner`. Moves past the lines it takes.
    fn value(&mut self, rest: &str, owner: isize, depth: usize) -> Result<Node, String> {
        self.count(depth)?;
        let rest = rest.trim();
        let first = rest.chars().next().unwrap_or('#');
        match first {
            '#' => {
                self.i += 1;
                match self.skip() {
                    Some(next) if next as isize > owner => self.node(next, owner, depth + 1),
                    // YAML allows a mapping's sequence value at the key's
                    // own indentation.
                    Some(next) if next as isize == owner && is_dash(&self.lines[self.i][next..]) => {
                        self.seq(next, depth + 1)
                    }
                    _ => Ok(Node::Scalar(String::new())),
                }
            }
            '!' => {
                // A tag: drop it.
                let after = rest.find(' ').map_or("", |k| &rest[k..]);
                self.value(after, owner, depth)
            }
            '&' | '*' => Err(self.err("anchors and aliases are not supported")),
            '|' | '>' => self.block_scalar(rest, owner),
            '"' | '\'' => {
                let (s, after) = self.quoted(&rest[1..], first)?;
                if !is_blank(&after) {
                    return Err(self.err("unexpected text after a quoted string"));
                }
                self.i += 1;
                Ok(Node::Scalar(s))
            }
            '[' | '{' => {
                let text = self.flow_text(rest)?;
                let chars: Vec<char> = text.chars().collect();
                let mut pos = 0;
                let node = flow(&chars, &mut pos, false, depth, &mut self.nodes).map_err(|e| self.err(&e))?;
                skip_ws(&chars, &mut pos);
                if pos < chars.len() && chars[pos] != '#' {
                    return Err(self.err("unexpected text after a flow collection"));
                }
                self.i += 1;
                Ok(node)
            }
            '@' | '`' | '%' => Err(self.err(&format!("'{first}' cannot start a value"))),
            _ => {
                // A plain scalar, continued on more indented lines.
                let mut text = strip_comment(rest).to_string();
                let mut breaks = 0;
                let mut j = self.i + 1;
                self.i = j;
                while j < self.lines.len() {
                    let l = &self.lines[j];
                    if l.trim().is_empty() {
                        breaks += 1;
                    } else if indent(l) as isize > owner && !l.trim_start().starts_with('#') {
                        if split_key(l.trim_start()).is_some() {
                            self.i = j;
                            return Err(self.err("unexpected indentation"));
                        }
                        let piece = strip_comment(l.trim());
                        if breaks > 0 {
                            text.extend(std::iter::repeat_n('\n', breaks));
                        } else {
                            text.push(' ');
                        }
                        text.push_str(piece);
                        breaks = 0;
                        self.i = j + 1;
                    } else {
                        break;
                    }
                    j += 1;
                }
                Ok(plain(&text))
            }
        }
    }

    /// A quoted scalar starting at `s` (just past the opening quote) on the
    /// current line and possibly continuing on the next ones. Returns the
    /// string and what follows it on its last line, which becomes the
    /// current line.
    fn quoted(&mut self, s: &str, q: char) -> Result<(String, String), String> {
        let mut out = String::new();
        let mut line = s.to_string();
        loop {
            match scan_quoted(&line, q, &mut out).map_err(|e| self.err(&e))? {
                Quoted::Closed(end) => return Ok((out, line[end..].to_string())),
                Quoted::Open(escaped) => {
                    // Fold the line break: a space, or a line break for
                    // each blank line.
                    if !escaped {
                        out.truncate(out.trim_end_matches([' ', '\t']).len());
                    }
                    let mut blank = false;
                    loop {
                        self.i += 1;
                        let Some(next) = self.lines.get(self.i) else {
                            return Err(self.err("unterminated quoted string"));
                        };
                        if !next.trim().is_empty() {
                            line = next.trim_start().to_string();
                            break;
                        }
                        out.push('\n');
                        blank = true;
                    }
                    if !escaped && !blank {
                        out.push(' ');
                    }
                }
            }
        }
    }

    /// A literal (`|`) or folded (`>`) block scalar whose header `head` is
    /// on the current line.
    fn block_scalar(&mut self, head: &str, owner: isize) -> Result<Node, String> {
        let folded = head.starts_with('>');
        let mut explicit = None;
        for c in strip_comment(&head[1..]).chars() {
            match c {
                '-' | '+' => {}
                '1'..='9' if explicit.is_none() => explicit = Some(c as usize - '0' as usize),
                _ => return Err(self.err("bad block scalar header")),
            }
        }
        self.i += 1;
        let ind = match explicit {
            Some(k) => owner.max(0) as usize + k,
            None => match self.lines[self.i..].iter().find(|l| !l.trim().is_empty()) {
                Some(l) if indent(l) as isize > owner => indent(l),
                _ => return Ok(Node::Scalar(String::new())),
            },
        };
        let mut lines: Vec<&str> = Vec::new();
        while let Some(l) = self.lines.get(self.i) {
            if l.trim().is_empty() {
                lines.push("");
            } else if indent(l) >= ind {
                lines.push(&l[ind..]);
            } else {
                break;
            }
            self.i += 1;
        }
        while lines.last() == Some(&"") {
            lines.pop();
        }
        if !folded {
            return Ok(Node::Scalar(lines.join("\n")));
        }
        // Folded: single line breaks between lines of text become spaces;
        // blank lines and more indented lines keep theirs.
        let mut out = String::new();
        let mut prev_text = false;
        for l in lines {
            let text = !l.is_empty() && !l.starts_with(' ');
            if l.is_empty() {
                out.push('\n');
            } else if prev_text && text {
                out.push(' ');
            } else if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(l);
            prev_text = text;
        }
        Ok(Node::Scalar(out))
    }

    /// The text of a flow collection starting at `rest` on the current
    /// line, joined with the lines it continues on until its brackets
    /// balance. The last of them becomes the current line.
    fn flow_text(&mut self, rest: &str) -> Result<String, String> {
        let mut text = String::new();
        let mut line = rest.to_string();
        let mut open = 0usize;
        let mut quote = None;
        loop {
            let mut prev = ' ';
            for c in line.chars() {
                match quote {
                    Some(q) => {
                        if c == q {
                            quote = None;
                        }
                    }
                    None => match c {
                        '"' | '\'' => quote = Some(c),
                        '[' | '{' => open += 1,
                        ']' | '}' => open = open.saturating_sub(1),
                        '#' if prev == ' ' => break,
                        _ => {}
                    },
                }
                text.push(c);
                prev = c;
                if open == 0 && quote.is_none() {
                    return Ok(text);
                }
            }
            text.push(' ');
            self.i += 1;
            line = match self.lines.get(self.i) {
                Some(l) => l.trim().to_string(),
                None => return Err(self.err("unterminated flow collection")),
            };
        }
    }
}

fn skip_ws(c: &[char], pos: &mut usize) {
    while *pos < c.len() && c[*pos] == ' ' {
        *pos += 1;
    }
}

/// A node in a flow collection at `pos`: a collection, or a scalar ending
/// at `,`, a closing bracket, or (for a mapping key) `: `.
fn flow(c: &[char], pos: &mut usize, key: bool, depth: usize, nodes: &mut usize) -> Result<Node, String> {
    *nodes += 1;
    if depth > MAX_DEPTH {
        return Err("nested too deeply".into());
    }
    if *nodes > MAX_NODES {
        return Err("too large".into());
    }
    skip_ws(c, pos);
    let Some(&first) = c.get(*pos) else {
        return Err("unterminated flow collection".into());
    };
    match first {
        '[' | '{' => {
            let map = first == '{';
            let close = if map { '}' } else { ']' };
            *pos += 1;
            let mut entries = Vec::new();
            let mut items = Vec::new();
            loop {
                skip_ws(c, pos);
                match c.get(*pos) {
                    None => return Err("unterminated flow collection".into()),
                    Some(&ch) if ch == close => {
                        *pos += 1;
                        break;
                    }
                    _ => {}
                }
                if map {
                    let k = match flow(c, pos, true, depth + 1, nodes)? {
                        Node::Scalar(s) => s,
                        _ => return Err("collections as keys are not supported".into()),
                    };
                    skip_ws(c, pos);
                    let v = if c.get(*pos) == Some(&':') {
                        *pos += 1;
                        skip_ws(c, pos);
                        if matches!(c.get(*pos), Some(',') | Some('}')) {
                            Node::Scalar(String::new())
                        } else {
                            flow(c, pos, false, depth + 1, nodes)?
                        }
                    } else {
                        Node::Scalar(String::new())
                    };
                    entries.push((k, v));
                } else {
                    items.push(flow(c, pos, false, depth + 1, nodes)?);
                }
                skip_ws(c, pos);
                match c.get(*pos) {
                    Some(',') => *pos += 1,
                    Some(&ch) if ch == close => {}
                    _ => return Err(format!("expected ',' or '{close}' in a flow collection")),
                }
            }
            Ok(if map { Node::Map(entries) } else { Node::Seq(items) })
        }
        '"' | '\'' => {
            let rest: String = c[*pos + 1..].iter().collect();
            let mut out = String::new();
            match scan_quoted(&rest, first, &mut out)? {
                Quoted::Closed(end) => {
                    *pos += 1 + rest[..end].chars().count();
                    Ok(Node::Scalar(out))
                }
                Quoted::Open(_) => Err("unterminated quoted string".into()),
            }
        }
        '&' | '*' => Err("anchors and aliases are not supported".into()),
        _ => {
            let start = *pos;
            while let Some(&ch) = c.get(*pos) {
                let colon = ch == ':'
                    && matches!(
                        c.get(*pos + 1),
                        None | Some(' ') | Some(',') | Some('}') | Some(']')
                    );
                if ch == ',' || ch == ']' || ch == '}' || (key && colon) {
                    break;
                }
                *pos += 1;
            }
            let s: String = c[start..*pos].iter().collect();
            Ok(plain(s.trim()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Node {
        Node::Scalar(x.into())
    }

    fn map(entries: &[(&str, Node)]) -> Node {
        Node::Map(entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    #[test]
    fn mappings_and_sequences() {
        let src = "title: Hello: world # comment\n\
                   date: 2024-01-01\n\
                   empty:\n\
                   author:\n  name: A. Person\n  email: a@example.com\n\
                   tags:\n- one\n- two\n\
                   people:\n  - name: X\n    role: y\n  -\n    name: Z\n";
        assert_eq!(
            parse(src, 1).unwrap(),
            map(&[
                ("title", s("Hello: world")),
                ("date", s("2024-01-01")),
                ("empty", s("")),
                (
                    "author",
                    map(&[("name", s("A. Person")), ("email", s("a@example.com"))])
                ),
                ("tags", Node::Seq(vec![s("one"), s("two")])),
                (
                    "people",
                    Node::Seq(vec![
                        map(&[("name", s("X")), ("role", s("y"))]),
                        map(&[("name", s("Z"))])
                    ])
                ),
            ])
        );
    }

    #[test]
    fn scalars() {
        let src = "a: 'it''s'\n\
                   b: \"tab\\there \\u00e9\"\n\
                   c: \"folded\n  over lines\"\n\
                   d: plain\n  continued\n\
                   e: |\n  line 1\n    line 2\n\n  line 3\n\
                   f: >-\n  folded\n  text\n\n  para\n\
                   g: !!str ~\n\
                   \"quoted key\": x\n";
        assert_eq!(
            parse(src, 1).unwrap(),
            map(&[
                ("a", s("it's")),
                ("b", s("tab\there é")),
                ("c", s("folded over lines")),
                ("d", s("plain continued")),
                ("e", s("line 1\n  line 2\n\nline 3")),
                ("f", s("folded text\npara")),
                ("g", s("")),
                ("quoted key", s("x")),
            ])
        );
    }

    #[test]
    fn flow_collections() {
        let src = "tags: [a, 'b, c', [d]]\nm: {x: 1, y: [2,\n  3], z}\n";
        assert_eq!(
            parse(src, 1).unwrap(),
            map(&[
                (
                    "tags",
                    Node::Seq(vec![s("a"), s("b, c"), Node::Seq(vec![s("d")])])
                ),
                (
                    "m",
                    map(&[
                        ("x", s("1")),
                        ("y", Node::Seq(vec![s("2"), s("3")])),
                        ("z", s(""))
                    ])
                ),
            ])
        );
    }

    #[test]
    fn errors() {
        for (src, msg) in [
            ("a: &x 1", "line 3: anchors"),
            ("a: 1\n  b: 2", "line 4: unexpected indentation"),
            ("a: \"open", "unterminated"),
            ("a: [1, 2", "unterminated"),
            ("a: 1\n? b", "complex keys"),
            ("a: \"\\q\"", "unknown escape"),
        ] {
            let e = parse(src, 3).unwrap_err();
            assert!(e.contains(msg), "{src:?}: {e}");
        }
    }

    #[test]
    fn hostile_nesting_is_bounded() {
        let deep = "a: ".to_string() + &"[".repeat(100_000) + &"]".repeat(100_000);
        assert!(parse(&deep, 1).unwrap_err().contains("nested too deeply"));
        let deep: String = (0..10_000).map(|i| format!("{}k:\n", " ".repeat(i))).collect();
        assert!(parse(&deep, 1).unwrap_err().contains("nested too deeply"));
        let deep: String = (0..10_000).map(|i| format!("{}- \n", " ".repeat(i))).collect();
        assert!(parse(&deep, 1).is_err());
        let wide = "a: [".to_string() + &"1,".repeat(100_000) + "]";
        assert!(parse(&wide, 1).unwrap_err().contains("too large"));
    }

    #[test]
    fn detects_mapping_start() {
        assert!(starts_mapping("title: x"));
        assert!(starts_mapping("\"a b\": x"));
        assert!(!starts_mapping("# Heading"));
        assert!(!starts_mapping("Some text"));
        assert!(!starts_mapping("- item"));
        assert!(!starts_mapping("  indented: x"));
        assert!(!starts_mapping("http://example.com"));
    }
}
