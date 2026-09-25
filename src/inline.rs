//! Inline Markdown parser: emphasis (CommonMark delimiter algorithm), code
//! spans, links, images, autolinks, entities, hard breaks and GFM
//! strikethrough. Every scan is bounded so the parser stays linear on
//! pathological input.

use crate::markdown::normalize_label;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text {
        text: String,
        style: Style,
        link: Option<String>,
    },
    Break,
    Image {
        alt: String,
        src: String,
    },
}

/// Longest link label or destination we are willing to scan for.
const MAX_LABEL: usize = 999;
const MAX_DEST: usize = 4096;
const MAX_NEST: usize = 3;

pub fn parse(src: &str, refs: &HashMap<String, String>) -> Vec<Inline> {
    let chars: Vec<char> = src.chars().collect();
    parse_chars(&chars, refs, false, 0)
}

/// Concatenate the visible text of parsed inlines.
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut s = String::new();
    for i in inlines {
        match i {
            Inline::Text { text, .. } => s.push_str(text),
            Inline::Break => s.push(' '),
            Inline::Image { alt, .. } => s.push_str(alt),
        }
    }
    s
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Em,
    Strong,
    Strike,
}

struct Delim {
    ch: char,
    count: usize,
    orig: usize,
    can_open: bool,
    can_close: bool,
    opens: Vec<Kind>,
    closes: Vec<Kind>,
}

enum Node {
    Text(String),
    Delim(usize),
    Code(String),
    Break,
    Group(Vec<Inline>),
    Image { alt: String, src: String },
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace())
}

fn starts_with(c: &[char], i: usize, pat: &str) -> bool {
    let mut k = i;
    for p in pat.chars() {
        match c.get(k) {
            Some(&x) if x.eq_ignore_ascii_case(&p) => k += 1,
            _ => return false,
        }
    }
    true
}

struct Parser<'a> {
    c: &'a [char],
    refs: &'a HashMap<String, String>,
    in_link: bool,
    depth: usize,
    /// Code span opener position -> (content start, content end, span end).
    spans: HashMap<usize, (usize, usize, usize)>,
    /// `[` position -> matching `]` position.
    brackets: HashMap<usize, usize>,
    nodes: Vec<Node>,
    delims: Vec<Delim>,
    text: String,
    no_comment_close: bool,
}

fn parse_chars(c: &[char], refs: &HashMap<String, String>, in_link: bool, depth: usize) -> Vec<Inline> {
    let mut p = Parser {
        c,
        refs,
        in_link,
        depth,
        spans: HashMap::new(),
        brackets: HashMap::new(),
        nodes: Vec::new(),
        delims: Vec::new(),
        text: String::new(),
        no_comment_close: false,
    };
    p.find_code_spans();
    p.find_brackets();
    p.scan();
    p.process_emphasis();
    let mut out = Vec::new();
    p.flatten(&mut out);
    out
}

impl Parser<'_> {
    fn find_code_spans(&mut self) {
        let c = self.c;
        let mut runs = Vec::new();
        let mut i = 0;
        while i < c.len() {
            if c[i] == '`' {
                let s = i;
                while i < c.len() && c[i] == '`' {
                    i += 1;
                }
                runs.push((s, i - s));
            } else {
                i += 1;
            }
        }
        let mut by_len: HashMap<usize, VecDeque<usize>> = HashMap::new();
        for (k, &(_, len)) in runs.iter().enumerate() {
            by_len.entry(len).or_default().push_back(k);
        }
        let mut k = 0;
        while k < runs.len() {
            let (s, len) = runs[k];
            let escaped = c[..s].iter().rev().take_while(|&&x| x == '\\').count() % 2 == 1;
            let q = by_len.entry(len).or_default();
            while q.front().is_some_and(|&f| f <= k) {
                q.pop_front();
            }
            match q.front() {
                Some(&m) if !escaped => {
                    let e = runs[m].0;
                    self.spans.insert(s, (s + len, e, e + len));
                    k = m + 1;
                }
                _ => k += 1,
            }
        }
    }

    fn find_brackets(&mut self) {
        let c = self.c;
        let mut stack = Vec::new();
        let mut i = 0;
        while i < c.len() {
            match c[i] {
                '\\' => i += 1,
                '`' => {
                    if let Some(&(_, _, end)) = self.spans.get(&i) {
                        i = end;
                        continue;
                    }
                }
                '[' => stack.push(i),
                ']' => {
                    if let Some(o) = stack.pop() {
                        self.brackets.insert(o, i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn flush(&mut self) {
        if !self.text.is_empty() {
            self.nodes.push(Node::Text(std::mem::take(&mut self.text)));
        }
    }

    fn scan(&mut self) {
        let c = self.c;
        let n = c.len();
        let mut i = 0;
        while i < n {
            let ch = c[i];
            match ch {
                '\\' => match c.get(i + 1) {
                    Some(&x) if x.is_ascii_punctuation() => {
                        self.text.push(x);
                        i += 2;
                    }
                    Some('\n') => {
                        self.flush();
                        self.nodes.push(Node::Break);
                        i += 2;
                    }
                    _ => {
                        self.text.push('\\');
                        i += 1;
                    }
                },
                '`' => {
                    if let Some(&(cs, ce, end)) = self.spans.get(&i) {
                        self.flush();
                        let mut s: String = c[cs..ce]
                            .iter()
                            .map(|&x| if x == '\n' { ' ' } else { x })
                            .collect();
                        if s.len() >= 2 && s.starts_with(' ') && s.ends_with(' ') && !s.trim().is_empty() {
                            s = s[1..s.len() - 1].to_string();
                        }
                        self.nodes.push(Node::Code(s));
                        i = end;
                    } else {
                        while i < n && c[i] == '`' {
                            self.text.push('`');
                            i += 1;
                        }
                    }
                }
                '*' | '_' | '~' => {
                    let s = i;
                    while i < n && c[i] == ch {
                        i += 1;
                    }
                    let count = i - s;
                    if ch == '~' && count > 2 {
                        self.text.extend(std::iter::repeat_n('~', count));
                        continue;
                    }
                    let before = if s == 0 { ' ' } else { c[s - 1] };
                    let after = c.get(i).copied().unwrap_or(' ');
                    let left = !after.is_whitespace()
                        && (!is_punct(after) || before.is_whitespace() || is_punct(before));
                    let right = !before.is_whitespace()
                        && (!is_punct(before) || after.is_whitespace() || is_punct(after));
                    let (can_open, can_close) = if ch == '_' {
                        (
                            left && (!right || is_punct(before)),
                            right && (!left || is_punct(after)),
                        )
                    } else {
                        (left, right)
                    };
                    self.flush();
                    self.nodes.push(Node::Delim(self.delims.len()));
                    self.delims.push(Delim {
                        ch,
                        count,
                        orig: count,
                        can_open,
                        can_close,
                        opens: Vec::new(),
                        closes: Vec::new(),
                    });
                }
                '!' if c.get(i + 1) == Some(&'[') => match self.link_at(i + 1) {
                    Some((ts, te, src, end)) => {
                        let alt = if self.depth < MAX_NEST {
                            plain_text(&parse_chars(&c[ts..te], self.refs, true, self.depth + 1))
                        } else {
                            c[ts..te].iter().collect()
                        };
                        self.flush();
                        self.nodes.push(Node::Image { alt, src });
                        i = end;
                    }
                    None => {
                        self.text.push('!');
                        i += 1;
                    }
                },
                '[' if !self.in_link && self.depth < MAX_NEST => match self.link_at(i) {
                    Some((ts, te, url, end)) => {
                        let mut inner = parse_chars(&c[ts..te], self.refs, true, self.depth + 1);
                        for x in inner.iter_mut() {
                            if let Inline::Text { link, .. } = x {
                                *link = Some(url.clone());
                            }
                        }
                        self.flush();
                        self.nodes.push(Node::Group(inner));
                        i = end;
                    }
                    None => {
                        self.text.push('[');
                        i += 1;
                    }
                },
                '<' => i = self.angle(i),
                '\n' => {
                    let hard = self.text.ends_with("  ");
                    let trimmed = self.text.trim_end_matches(' ').len();
                    self.text.truncate(trimmed);
                    if hard {
                        self.flush();
                        self.nodes.push(Node::Break);
                    } else {
                        self.text.push(' ');
                    }
                    i += 1;
                    while i < n && c[i] == ' ' {
                        i += 1;
                    }
                }
                '&' => i = self.entity(i),
                'h' | 'H'
                    if !self.in_link
                        && (i == 0 || !c[i - 1].is_alphanumeric())
                        && (starts_with(c, i, "http://") || starts_with(c, i, "https://")) =>
                {
                    i = self.bare_url(i);
                }
                _ => {
                    self.text.push(ch);
                    i += 1;
                }
            }
        }
        self.flush();
    }

    /// Try to parse a link starting at the `[` at `open`. Returns the text
    /// range, destination and the position after the whole construct.
    fn link_at(&self, open: usize) -> Option<(usize, usize, String, usize)> {
        let c = self.c;
        let close = *self.brackets.get(&open)?;
        if c.get(close + 1) == Some(&'(') {
            if let Some((dest, end)) = self.inline_dest(close + 2) {
                return Some((open + 1, close, dest, end));
            }
        }
        if self.refs.is_empty() {
            return None;
        }
        let lookup = |a: usize, b: usize| -> Option<String> {
            if b - a > MAX_LABEL {
                return None;
            }
            let label: String = c[a..b].iter().collect();
            self.refs.get(&normalize_label(&label)).cloned()
        };
        if c.get(close + 1) == Some(&'[') {
            if let Some(&c2) = self.brackets.get(&(close + 1)) {
                let found = if c2 == close + 2 {
                    lookup(open + 1, close)
                } else {
                    lookup(close + 2, c2)
                };
                return found.map(|d| (open + 1, close, d, c2 + 1));
            }
        }
        if c.get(open + 1) == Some(&'^') {
            return None;
        }
        lookup(open + 1, close).map(|d| (open + 1, close, d, close + 1))
    }

    /// Parse `destination "title")` starting just after `(`.
    fn inline_dest(&self, start: usize) -> Option<(String, usize)> {
        let c = self.c;
        let n = c.len();
        let skip_ws = |mut i: usize| {
            while i < n && c[i].is_whitespace() {
                i += 1;
            }
            i
        };
        let mut i = skip_ws(start);
        let mut dest = String::new();
        if c.get(i) == Some(&'<') {
            i += 1;
            while i < n && c[i] != '>' {
                if c[i] == '\n' || c[i] == '<' || i - start > MAX_DEST {
                    return None;
                }
                dest.push(c[i]);
                i += 1;
            }
            i += 1;
        } else {
            let mut depth = 0usize;
            while i < n && !c[i].is_whitespace() {
                if i - start > MAX_DEST {
                    return None;
                }
                match c[i] {
                    '\\' if c.get(i + 1).is_some_and(|x| x.is_ascii_punctuation()) => {
                        dest.push(c[i + 1]);
                        i += 2;
                        continue;
                    }
                    '(' => depth += 1,
                    ')' if depth == 0 => break,
                    ')' => depth -= 1,
                    _ => {}
                }
                dest.push(c[i]);
                i += 1;
            }
        }
        i = skip_ws(i);
        if let Some(&q) = c.get(i) {
            let close = match q {
                '"' => Some('"'),
                '\'' => Some('\''),
                '(' => Some(')'),
                _ => None,
            };
            if let Some(close) = close {
                let t0 = i;
                i += 1;
                while i < n && c[i] != close {
                    if c[i] == '\\' {
                        i += 1;
                    }
                    if i - t0 > MAX_DEST {
                        return None;
                    }
                    i += 1;
                }
                i = skip_ws(i + 1);
            }
        }
        if c.get(i) == Some(&')') {
            Some((dest, i + 1))
        } else {
            None
        }
    }

    /// Handle `<`: autolinks, HTML comments, `<br>` and other inline HTML tags.
    fn angle(&mut self, i: usize) -> usize {
        let c = self.c;
        let n = c.len();
        if starts_with(c, i, "<!--") {
            if !self.no_comment_close {
                let mut j = i + 4;
                while j + 2 < n {
                    if c[j] == '-' && c[j + 1] == '-' && c[j + 2] == '>' {
                        return j + 3;
                    }
                    j += 1;
                }
                self.no_comment_close = true;
            }
            self.text.push('<');
            return i + 1;
        }
        let mut j = i + 1;
        while j < n && j - i < 2048 && !matches!(c[j], '<' | '>' | ' ' | '\n') {
            j += 1;
        }
        if j < n && c[j] == '>' && j > i + 1 {
            let inner: String = c[i + 1..j].iter().collect();
            let scheme_len = inner
                .find(':')
                .filter(|&k| (2..=32).contains(&k))
                .filter(|_| inner.starts_with(|c: char| c.is_ascii_alphabetic()))
                .filter(|&k| {
                    inner[..k]
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
                });
            let url = if scheme_len.is_some() {
                Some(inner.clone())
            } else if inner.contains('@')
                && !inner.starts_with('@')
                && !inner.ends_with('@')
                && !inner.contains('/')
            {
                Some(format!("mailto:{inner}"))
            } else {
                None
            };
            if let Some(url) = url {
                self.flush();
                self.nodes.push(Node::Group(vec![Inline::Text {
                    text: inner,
                    style: Style::default(),
                    link: if self.in_link { None } else { Some(url) },
                }]));
                return j + 1;
            }
        }
        // Inline HTML tag: strip it, turning <br> into a line break.
        let first = c.get(i + 1).copied().unwrap_or(' ');
        let second = c.get(i + 2).copied().unwrap_or(' ');
        if first.is_ascii_alphabetic() || (first == '/' && second.is_ascii_alphabetic()) {
            let mut j = i + 1;
            while j < n && j - i < 1024 && c[j] != '>' && c[j] != '<' {
                j += 1;
            }
            if j < n && c[j] == '>' {
                let name: String = c[i + 1..j]
                    .iter()
                    .skip_while(|&&x| x == '/')
                    .take_while(|x| x.is_ascii_alphanumeric())
                    .collect();
                if name.eq_ignore_ascii_case("br") {
                    self.flush();
                    self.nodes.push(Node::Break);
                }
                return j + 1;
            }
        }
        self.text.push('<');
        i + 1
    }

    fn entity(&mut self, i: usize) -> usize {
        let c = self.c;
        let mut j = i + 1;
        while j < c.len() && j - i <= 32 && (c[j].is_ascii_alphanumeric() || c[j] == '#') {
            j += 1;
        }
        if c.get(j) == Some(&';') && j > i + 1 {
            let name: String = c[i + 1..j].iter().collect();
            if let Some(ch) = decode_entity(&name) {
                self.text.push(ch);
                return j + 1;
            }
        }
        self.text.push('&');
        i + 1
    }

    fn bare_url(&mut self, i: usize) -> usize {
        let c = self.c;
        let mut j = i;
        while j < c.len() && !c[j].is_whitespace() && c[j] != '<' && j - i < MAX_DEST {
            j += 1;
        }
        // Trailing punctuation is not part of the URL.
        loop {
            match c[j - 1] {
                '.' | ',' | ':' | ';' | '!' | '?' | '"' | '\'' | '*' | '_' | '~' => j -= 1,
                ')' => {
                    let open = c[i..j].iter().filter(|&&x| x == '(').count();
                    let close = c[i..j].iter().filter(|&&x| x == ')').count();
                    if close > open {
                        j -= 1;
                    } else {
                        break;
                    }
                }
                _ => break,
            }
            if j <= i {
                break;
            }
        }
        let url: String = c[i..j].iter().collect();
        if !url.contains("://") || url.ends_with("://") {
            self.text.push_str(&url);
            return j.max(i + 1);
        }
        self.flush();
        self.nodes.push(Node::Group(vec![Inline::Text {
            text: url.clone(),
            style: Style::default(),
            link: Some(url),
        }]));
        j
    }

    /// The CommonMark "process emphasis" procedure over the delimiter list.
    fn process_emphasis(&mut self) {
        let m = self.delims.len();
        let mut prev: Vec<Option<usize>> = (0..m).map(|k| k.checked_sub(1)).collect();
        let mut next: Vec<Option<usize>> = (0..m).map(|k| Some(k + 1).filter(|&x| x < m)).collect();
        fn unlink(k: usize, prev: &mut [Option<usize>], next: &mut [Option<usize>]) {
            let (p, nx) = (prev[k], next[k]);
            if let Some(p) = p {
                next[p] = nx;
            }
            if let Some(nx) = nx {
                prev[nx] = p;
            }
        }
        // openers_bottom[char][can_open][orig % 3]: openers at or below are excluded.
        let mut bottom = [[[-1i64; 3]; 2]; 3];
        let mut cur = if m > 0 { Some(0) } else { None };
        while let Some(k) = cur {
            let d = &self.delims[k];
            if !d.can_close || d.count == 0 {
                cur = next[k];
                continue;
            }
            let ci = match d.ch {
                '*' => 0,
                '_' => 1,
                _ => 2,
            };
            let key = (ci, d.can_open as usize, d.orig % 3);
            let lower = bottom[key.0][key.1][key.2];
            let mut found = None;
            let mut o = prev[k];
            while let Some(oi) = o {
                if (oi as i64) <= lower {
                    break;
                }
                let od = &self.delims[oi];
                if od.ch == d.ch && od.can_open && od.count > 0 {
                    let ok = if d.ch == '~' {
                        od.count == d.count
                    } else {
                        !((d.can_open || od.can_close)
                            && (od.orig + d.orig).is_multiple_of(3)
                            && !(od.orig.is_multiple_of(3) && d.orig.is_multiple_of(3)))
                    };
                    if ok {
                        found = Some(oi);
                        break;
                    }
                }
                o = prev[oi];
            }
            match found {
                Some(oi) => {
                    let oc = self.delims[oi].count;
                    let dc = self.delims[k].count;
                    let (used, kind) = if d.ch == '~' {
                        (dc, Kind::Strike)
                    } else if dc >= 2 && oc >= 2 {
                        (2, Kind::Strong)
                    } else {
                        (1, Kind::Em)
                    };
                    self.delims[oi].count -= used;
                    self.delims[oi].opens.push(kind);
                    self.delims[k].count -= used;
                    self.delims[k].closes.push(kind);
                    let mut b = next[oi];
                    while let Some(bi) = b {
                        if bi == k {
                            break;
                        }
                        b = next[bi];
                        unlink(bi, &mut prev, &mut next);
                    }
                    if self.delims[oi].count == 0 {
                        unlink(oi, &mut prev, &mut next);
                    }
                    if self.delims[k].count == 0 {
                        cur = next[k];
                        unlink(k, &mut prev, &mut next);
                    }
                }
                None => {
                    bottom[key.0][key.1][key.2] = k as i64 - 1;
                    cur = next[k];
                    if !d.can_open {
                        unlink(k, &mut prev, &mut next);
                    }
                }
            }
        }
    }

    fn flatten(&self, out: &mut Vec<Inline>) {
        let (mut em, mut strong, mut strike) = (0i32, 0i32, 0i32);
        let style = |em: i32, strong: i32, strike: i32| Style {
            bold: strong > 0,
            italic: em > 0,
            strike: strike > 0,
            code: false,
        };
        let bump = |k: Kind, d: i32, em: &mut i32, strong: &mut i32, strike: &mut i32| match k {
            Kind::Em => *em += d,
            Kind::Strong => *strong += d,
            Kind::Strike => *strike += d,
        };
        for node in &self.nodes {
            match node {
                Node::Text(s) => push_text(out, s, style(em, strong, strike), None),
                Node::Delim(k) => {
                    let d = &self.delims[*k];
                    for &kind in &d.closes {
                        bump(kind, -1, &mut em, &mut strong, &mut strike);
                    }
                    if d.count > 0 {
                        let s: String = std::iter::repeat_n(d.ch, d.count).collect();
                        push_text(out, &s, style(em, strong, strike), None);
                    }
                    for &kind in d.opens.iter().rev() {
                        bump(kind, 1, &mut em, &mut strong, &mut strike);
                    }
                }
                Node::Code(s) => {
                    let mut st = style(em, strong, strike);
                    st.code = true;
                    push_text(out, s, st, None);
                }
                Node::Break => out.push(Inline::Break),
                Node::Image { alt, src } => out.push(Inline::Image {
                    alt: alt.clone(),
                    src: src.clone(),
                }),
                Node::Group(items) => {
                    let outer = style(em, strong, strike);
                    for item in items {
                        match item {
                            Inline::Text { text, style: s, link } => {
                                let st = Style {
                                    bold: s.bold || outer.bold,
                                    italic: s.italic || outer.italic,
                                    strike: s.strike || outer.strike,
                                    code: s.code,
                                };
                                push_text(out, text, st, link.as_deref());
                            }
                            other => out.push(other.clone()),
                        }
                    }
                }
            }
        }
    }
}

fn push_text(out: &mut Vec<Inline>, s: &str, style: Style, link: Option<&str>) {
    if s.is_empty() {
        return;
    }
    if let Some(Inline::Text {
        text,
        style: ps,
        link: pl,
    }) = out.last_mut()
    {
        if *ps == style && pl.as_deref() == link {
            text.push_str(s);
            return;
        }
    }
    out.push(Inline::Text {
        text: s.to_string(),
        style,
        link: link.map(str::to_string),
    });
}

fn decode_entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let v = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            num.parse().ok()?
        };
        return Some(char::from_u32(v).filter(|&c| c != '\0').unwrap_or('\u{FFFD}'));
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{A0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "sect" => '§',
        "para" => '¶',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "larr" => '←',
        "rarr" => '→',
        "harr" => '↔',
        "le" => '≤',
        "ge" => '≥',
        "ne" => '≠',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<Inline> {
        parse(s, &HashMap::new())
    }

    fn t(text: &str, bold: bool, italic: bool) -> Inline {
        Inline::Text {
            text: text.into(),
            style: Style {
                bold,
                italic,
                ..Style::default()
            },
            link: None,
        }
    }

    #[test]
    fn emphasis() {
        assert_eq!(
            p("a *b* **c**"),
            vec![
                t("a ", false, false),
                t("b", false, true),
                t(" ", false, false),
                t("c", true, false)
            ]
        );
        assert_eq!(p("***x***"), vec![t("x", true, true)]);
        assert_eq!(p("snake_case_name"), vec![t("snake_case_name", false, false)]);
        assert_eq!(p("*unclosed"), vec![t("*unclosed", false, false)]);
        assert_eq!(p("**a*"), vec![t("*", false, false), t("a", false, true)]);
    }

    #[test]
    fn code_and_escapes() {
        let r = p("`a*b*` \\*x\\*");
        assert!(matches!(&r[0], Inline::Text { text, style, .. } if text == "a*b*" && style.code));
        assert!(matches!(&r[1], Inline::Text { text, .. } if text == " *x*"));
        let r = p("`` a ` b ``");
        assert!(matches!(&r[0], Inline::Text { text, .. } if text == "a ` b"));
    }

    #[test]
    fn links() {
        let r = p("see [the *site*](http://x.org \"t\") and <https://y.org>");
        assert!(
            matches!(&r[1], Inline::Text { text, link: Some(l), .. } if text == "the " && l == "http://x.org")
        );
        assert!(
            matches!(&r[2], Inline::Text { text, link: Some(_), style, .. } if text == "site" && style.italic)
        );
        assert!(matches!(&r[4], Inline::Text { link: Some(l), .. } if l == "https://y.org"));
        let mut refs = HashMap::new();
        refs.insert("foo".to_string(), "/url".to_string());
        let r = parse("[Foo] and [x][foo]", &refs);
        assert!(matches!(&r[0], Inline::Text { link: Some(l), .. } if l == "/url"));
        assert!(matches!(&r[2], Inline::Text { text, link: Some(_), .. } if text == "x"));
        let r = p("![alt *text*](img.png)");
        assert_eq!(
            r,
            vec![Inline::Image {
                alt: "alt text".into(),
                src: "img.png".into()
            }]
        );
        let r = p("go to https://example.com/a_(b). now");
        assert!(matches!(&r[1], Inline::Text { link: Some(l), .. } if l == "https://example.com/a_(b)"));
    }

    #[test]
    fn breaks_entities_html() {
        let r = p("a  \nb\\\nc<br/>d &amp; &#x41; <span>e</span><!-- hidden -->");
        assert_eq!(r[1], Inline::Break);
        assert_eq!(r[3], Inline::Break);
        assert_eq!(r[5], Inline::Break);
        assert_eq!(plain_text(&r), "a b c d & A e");
        assert_eq!(plain_text(&p("~~del~~")), "del");
        assert!(matches!(&p("~~del~~")[0], Inline::Text { style, .. } if style.strike));
    }

    #[test]
    fn pathological_inputs_are_fast() {
        let start = std::time::Instant::now();
        for s in [
            "*a ".repeat(50_000),
            "[".repeat(100_000),
            "[a](".repeat(30_000),
            "`".to_string() + &"``` ".repeat(30_000),
            "<!--".repeat(30_000),
            "**_".repeat(40_000),
            "[".repeat(50_000) + &"]".repeat(50_000),
            "![[](".repeat(30_000),
        ] {
            let mut refs = HashMap::new();
            refs.insert("a".to_string(), "b".to_string());
            let _ = parse(&s, &refs);
        }
        assert!(start.elapsed().as_secs() < 10, "took {:?}", start.elapsed());
    }
}
