//! Page layout: turns the parsed document into positioned text runs, rules,
//! boxes and images on fixed-size pages.

use crate::encoding;
use crate::image::{self, Image};
use crate::inline::{self, Inline, Style};
use crate::markdown::{Align, Block, Document};
use crate::metrics;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

const LINE_SPACING: f32 = 1.4;
const TEXT: Color = (0.11, 0.11, 0.12);
const MUTED: Color = (0.4, 0.4, 0.43);
const LINK: Color = (0.02, 0.35, 0.75);
const CODE_BG: Color = (0.95, 0.95, 0.96);
const RULE: Color = (0.82, 0.82, 0.85);

type Color = (f32, f32, f32);

#[derive(Clone, Debug)]
pub struct Options {
    pub page_width: f32,
    pub page_height: f32,
    pub margin: f32,
    pub font_size: f32,
    pub serif: bool,
    pub page_numbers: bool,
    /// Directory that relative image paths are resolved against. `None`
    /// disables loading local images.
    pub base_dir: Option<PathBuf>,
    pub title: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            page_width: 595.28,
            page_height: 841.89,
            margin: 56.7,
            font_size: 11.0,
            serif: false,
            page_numbers: true,
            base_dir: None,
            title: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Font {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
    MonoBold,
    MonoItalic,
    MonoBoldItalic,
}

impl Font {
    pub const ALL: [Font; 8] = [
        Font::Regular,
        Font::Bold,
        Font::Italic,
        Font::BoldItalic,
        Font::Mono,
        Font::MonoBold,
        Font::MonoItalic,
        Font::MonoBoldItalic,
    ];

    fn for_style(s: Style) -> Font {
        let i = (s.bold as usize) | (s.italic as usize) << 1 | (s.code as usize) << 2;
        [
            Font::Regular,
            Font::Bold,
            Font::Italic,
            Font::BoldItalic,
            Font::Mono,
            Font::MonoBold,
            Font::MonoItalic,
            Font::MonoBoldItalic,
        ][i]
    }

    pub fn id(self) -> usize {
        self as usize + 1
    }

    pub fn base_name(self, serif: bool) -> &'static str {
        match (self, serif) {
            (Font::Regular, false) => "Helvetica",
            (Font::Bold, false) => "Helvetica-Bold",
            (Font::Italic, false) => "Helvetica-Oblique",
            (Font::BoldItalic, false) => "Helvetica-BoldOblique",
            (Font::Regular, true) => "Times-Roman",
            (Font::Bold, true) => "Times-Bold",
            (Font::Italic, true) => "Times-Italic",
            (Font::BoldItalic, true) => "Times-BoldItalic",
            (Font::Mono, _) => "Courier",
            (Font::MonoBold, _) => "Courier-Bold",
            (Font::MonoItalic, _) => "Courier-Oblique",
            (Font::MonoBoldItalic, _) => "Courier-BoldOblique",
        }
    }

    fn widths(self, serif: bool) -> Option<&'static [u16; 256]> {
        Some(match (self, serif) {
            (Font::Regular, false) => &metrics::HELVETICA,
            (Font::Bold, false) => &metrics::HELVETICA_BOLD,
            (Font::Italic, false) => &metrics::HELVETICA_OBLIQUE,
            (Font::BoldItalic, false) => &metrics::HELVETICA_BOLD_OBLIQUE,
            (Font::Regular, true) => &metrics::TIMES_ROMAN,
            (Font::Bold, true) => &metrics::TIMES_BOLD,
            (Font::Italic, true) => &metrics::TIMES_ITALIC,
            (Font::BoldItalic, true) => &metrics::TIMES_BOLD_ITALIC,
            _ => return None,
        })
    }
}

/// A run of text in a single font, size, color and link.
#[derive(Clone)]
struct Frag {
    font: Font,
    size: f32,
    text: Vec<u8>,
    width: f32,
    color: Color,
    code: bool,
    strike: bool,
    link: Option<Rc<str>>,
}

enum Tok {
    Word(Vec<Frag>),
    Space(f32),
    Break,
    Image { alt: String, src: String },
}

#[derive(Default)]
struct Line {
    frags: Vec<(f32, Frag)>,
    width: f32,
    size: f32,
}

enum Laid {
    Line(Line),
    Image { alt: String, src: String },
}

pub enum Target {
    Uri(String),
    Anchor(String),
}

pub struct LinkArea {
    pub rect: [f32; 4],
    pub target: Target,
}

#[derive(Default)]
pub struct Page {
    pub ops: Vec<u8>,
    pub links: Vec<LinkArea>,
}

pub struct Heading {
    pub level: u8,
    pub title: String,
    pub page: usize,
    pub y: f32,
}

pub struct Output {
    pub pages: Vec<Page>,
    pub images: Vec<Image>,
    pub headings: Vec<Heading>,
    pub anchors: HashMap<String, (usize, f32)>,
    pub title: Option<String>,
    pub warnings: Vec<String>,
}

enum MarkerKind {
    Text(Vec<u8>),
    Check(bool),
}

struct Marker {
    kind: MarkerKind,
    right: f32,
    size: f32,
    color: Color,
}

#[derive(Clone, Copy)]
struct Ctx {
    x: f32,
    w: f32,
    color: Color,
    tight: bool,
    list_depth: usize,
}

struct Layout<'a> {
    o: &'a Options,
    refs: &'a HashMap<String, String>,
    pages: Vec<Page>,
    y: f32,
    at_top: bool,
    pending_gap: f32,
    marker: Option<Marker>,
    images: Vec<Image>,
    image_cache: HashMap<String, Option<usize>>,
    headings: Vec<Heading>,
    anchors: HashMap<String, (usize, f32)>,
    slug_counts: HashMap<String, usize>,
    warnings: Vec<String>,
}

pub fn layout(doc: &Document, o: &Options) -> Output {
    let mut l = Layout {
        o,
        refs: &doc.refs,
        pages: vec![Page::default()],
        y: o.page_height - o.margin,
        at_top: true,
        pending_gap: 0.0,
        marker: None,
        images: Vec::new(),
        image_cache: HashMap::new(),
        headings: Vec::new(),
        anchors: HashMap::new(),
        slug_counts: HashMap::new(),
        warnings: Vec::new(),
    };
    let ctx = Ctx {
        x: o.margin,
        w: o.page_width - 2.0 * o.margin,
        color: TEXT,
        tight: false,
        list_depth: 0,
    };
    l.blocks(&doc.blocks, ctx);
    if o.page_numbers {
        l.number_pages();
    }
    let title = o
        .title
        .clone()
        .or_else(|| l.headings.iter().find(|h| h.level == 1).map(|h| h.title.clone()));
    Output {
        pages: l.pages,
        images: l.images,
        headings: l.headings,
        anchors: l.anchors,
        title,
        warnings: l.warnings,
    }
}

/// Append a number in PDF syntax.
fn num(out: &mut Vec<u8>, v: f32) {
    let v = if v.is_finite() { v } else { 0.0 };
    let r = (v * 100.0).round() as i64;
    if r < 0 {
        out.push(b'-');
    }
    let r = r.unsigned_abs();
    let _ = write!(out, "{}", r / 100);
    let frac = r % 100;
    if frac != 0 {
        if frac.is_multiple_of(10) {
            let _ = write!(out, ".{}", frac / 10);
        } else {
            let _ = write!(out, ".{frac:02}");
        }
    }
}

fn nums(out: &mut Vec<u8>, vs: &[f32]) {
    for v in vs {
        num(out, *v);
        out.push(b' ');
    }
}

fn pdf_text(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'(');
    for &b in bytes {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c == ' ' {
            out.push('-');
        }
    }
    out
}

impl Layout<'_> {
    fn text_width(&self, font: Font, bytes: &[u8], size: f32) -> f32 {
        let units: u32 = match font.widths(self.o.serif) {
            Some(w) => bytes.iter().map(|&b| w[b as usize] as u32).sum(),
            None => bytes.len() as u32 * 600,
        };
        units as f32 * size / 1000.0
    }

    fn top(&self) -> f32 {
        self.o.page_height - self.o.margin
    }

    fn bottom(&self) -> f32 {
        self.o.margin
    }

    fn page(&mut self) -> &mut Page {
        if self.pages.is_empty() {
            self.pages.push(Page::default());
        }
        let last = self.pages.len() - 1;
        &mut self.pages[last]
    }

    fn new_page(&mut self) {
        self.pages.push(Page::default());
        self.y = self.top();
        self.at_top = true;
    }

    fn gap(&mut self, h: f32) {
        self.pending_gap = self.pending_gap.max(h);
    }

    /// Reserve `h` points of vertical space, applying any pending gap and
    /// starting a new page when the current one is full.
    fn ensure(&mut self, h: f32) {
        if !self.at_top {
            self.y -= self.pending_gap;
        }
        self.pending_gap = 0.0;
        if self.y - h < self.bottom() && !self.at_top {
            self.new_page();
        }
        self.at_top = false;
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Color) {
        let ops = &mut self.page().ops;
        nums(ops, &[c.0, c.1, c.2]);
        ops.extend_from_slice(b"rg ");
        nums(ops, &[x, y, w, h]);
        ops.extend_from_slice(b"re f\n");
    }

    fn stroke_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        let ops = &mut self.page().ops;
        nums(ops, &[c.0, c.1, c.2]);
        ops.extend_from_slice(b"RG ");
        num(ops, width);
        ops.extend_from_slice(b" w ");
        nums(ops, &[x0, y0]);
        ops.extend_from_slice(b"m ");
        nums(ops, &[x1, y1]);
        ops.extend_from_slice(b"l S\n");
    }

    fn show_text(&mut self, font: Font, size: f32, x: f32, y: f32, c: Color, text: &[u8]) {
        let ops = &mut self.page().ops;
        ops.extend_from_slice(b"BT ");
        nums(ops, &[c.0, c.1, c.2]);
        let _ = write!(ops, "rg /F{} ", font.id());
        num(ops, size);
        ops.extend_from_slice(b" Tf ");
        nums(ops, &[x, y]);
        ops.extend_from_slice(b"Td ");
        pdf_text(ops, text);
        ops.extend_from_slice(b" Tj ET\n");
    }

    // ------------------------------------------------------------ inline text

    fn tokenize(&self, inlines: &[Inline], size: f32, color: Color, force_bold: bool) -> Vec<Tok> {
        let mut toks = Vec::new();
        let mut word: Vec<Frag> = Vec::new();
        for item in inlines {
            match item {
                Inline::Text { text, style, link } => {
                    let mut style = *style;
                    style.bold |= force_bold;
                    let font = Font::for_style(style);
                    let fsize = if style.code { size * 0.9 } else { size };
                    let fcolor = if link.is_some() { LINK } else { color };
                    let link: Option<Rc<str>> = link.as_deref().map(Rc::from);
                    let space = self.text_width(font, b" ", fsize);
                    let bytes = encoding::encode(text);
                    for (k, part) in bytes.split(|&b| b == b' ').enumerate() {
                        if k > 0 {
                            if !word.is_empty() {
                                toks.push(Tok::Word(std::mem::take(&mut word)));
                            }
                            if style.code || !matches!(toks.last(), Some(Tok::Space(_))) {
                                toks.push(Tok::Space(space));
                            }
                        }
                        if part.is_empty() {
                            continue;
                        }
                        let w = self.text_width(font, part, fsize);
                        match word.last_mut() {
                            Some(f)
                                if f.font == font
                                    && f.code == style.code
                                    && f.strike == style.strike
                                    && f.link == link =>
                            {
                                f.text.extend_from_slice(part);
                                f.width += w;
                            }
                            _ => word.push(Frag {
                                font,
                                size: fsize,
                                text: part.to_vec(),
                                width: w,
                                color: fcolor,
                                code: style.code,
                                strike: style.strike,
                                link: link.clone(),
                            }),
                        }
                    }
                }
                Inline::Break => {
                    if !word.is_empty() {
                        toks.push(Tok::Word(std::mem::take(&mut word)));
                    }
                    toks.push(Tok::Break);
                }
                Inline::Image { alt, src } => {
                    if !word.is_empty() {
                        toks.push(Tok::Word(std::mem::take(&mut word)));
                    }
                    toks.push(Tok::Image {
                        alt: alt.clone(),
                        src: src.clone(),
                    });
                }
            }
        }
        if !word.is_empty() {
            toks.push(Tok::Word(word));
        }
        toks
    }

    fn wrap(&self, toks: Vec<Tok>, max_w: f32, base_size: f32) -> Vec<Laid> {
        let max_w = max_w.max(1.0);
        let mut out = Vec::new();
        let mut line = Line {
            size: base_size,
            ..Line::default()
        };
        let mut space = 0.0f32;
        let push_frag = |line: &mut Line, x: f32, f: Frag| {
            line.size = line.size.max(f.size);
            line.width = x + f.width;
            line.frags.push((x, f));
        };
        for tok in toks {
            match tok {
                Tok::Space(w) => {
                    if !line.frags.is_empty() {
                        space = if space > 0.0 { space + w } else { w };
                    }
                }
                Tok::Break => {
                    out.push(Laid::Line(std::mem::replace(
                        &mut line,
                        Line {
                            size: base_size,
                            ..Line::default()
                        },
                    )));
                    space = 0.0;
                }
                Tok::Image { alt, src } => {
                    if !line.frags.is_empty() {
                        out.push(Laid::Line(std::mem::replace(
                            &mut line,
                            Line {
                                size: base_size,
                                ..Line::default()
                            },
                        )));
                    }
                    out.push(Laid::Image { alt, src });
                    space = 0.0;
                }
                Tok::Word(frags) => {
                    let w: f32 = frags.iter().map(|f| f.width).sum();
                    if !line.frags.is_empty() && line.width + space + w > max_w {
                        out.push(Laid::Line(std::mem::replace(
                            &mut line,
                            Line {
                                size: base_size,
                                ..Line::default()
                            },
                        )));
                        space = 0.0;
                    }
                    if line.frags.is_empty() && w > max_w {
                        // Break an overlong word at character boundaries.
                        for f in frags {
                            let mut start = 0;
                            let mut x = line.width;
                            let mut acc = 0.0;
                            for (k, &b) in f.text.iter().enumerate() {
                                let cw = self.text_width(f.font, &[b], f.size);
                                if x + acc + cw > max_w && (x + acc) > 0.0 {
                                    if k > start {
                                        let piece = Frag {
                                            text: f.text[start..k].to_vec(),
                                            width: acc,
                                            ..f.clone()
                                        };
                                        push_frag(&mut line, x, piece);
                                    }
                                    out.push(Laid::Line(std::mem::replace(
                                        &mut line,
                                        Line {
                                            size: base_size,
                                            ..Line::default()
                                        },
                                    )));
                                    start = k;
                                    x = 0.0;
                                    acc = 0.0;
                                }
                                acc += cw;
                            }
                            if start < f.text.len() {
                                let piece = Frag {
                                    text: f.text[start..].to_vec(),
                                    width: acc,
                                    ..f
                                };
                                push_frag(&mut line, x, piece);
                            }
                        }
                    } else {
                        let mut x = if line.frags.is_empty() {
                            0.0
                        } else {
                            line.width + space
                        };
                        for f in frags {
                            let fw = f.width;
                            push_frag(&mut line, x, f);
                            x += fw;
                        }
                    }
                    space = 0.0;
                }
            }
        }
        if !line.frags.is_empty() {
            out.push(Laid::Line(line));
        }
        out
    }

    fn line_height(&self, line: &Line) -> f32 {
        line.size * LINE_SPACING
    }

    /// Draw a wrapped line with its top at the current `y`.
    fn draw_line(&mut self, line: &Line, x: f32, lh: f32) {
        let baseline = self.y - lh / 2.0 - line.size * 0.26;
        self.draw_marker(baseline);
        for (fx, f) in &line.frags {
            if f.code {
                self.fill_rect(
                    x + fx - 1.5,
                    baseline - f.size * 0.28,
                    f.width + 3.0,
                    f.size * 1.2,
                    CODE_BG,
                );
            }
        }
        for (fx, f) in &line.frags {
            let fx = x + fx;
            self.show_text(f.font, f.size, fx, baseline, f.color, &f.text);
            if f.strike {
                let sy = baseline + f.size * 0.3;
                self.stroke_line(fx, sy, fx + f.width, sy, f.size * 0.06, f.color);
            }
            if let Some(url) = &f.link {
                let rect = [
                    fx,
                    baseline - f.size * 0.25,
                    fx + f.width,
                    baseline + f.size * 0.85,
                ];
                let target = match url.strip_prefix('#') {
                    Some(a) => Target::Anchor(a.to_string()),
                    None => Target::Uri(url.to_string()),
                };
                let page = self.page();
                // Merge with the previous area when it continues the same link.
                if let Some(prev) = page.links.last_mut() {
                    let same = match (&prev.target, &target) {
                        (Target::Uri(a), Target::Uri(b)) | (Target::Anchor(a), Target::Anchor(b)) => a == b,
                        _ => false,
                    };
                    if same && (prev.rect[1] - rect[1]).abs() < 0.01 && rect[0] - prev.rect[2] < f.size {
                        prev.rect[2] = rect[2];
                        continue;
                    }
                }
                page.links.push(LinkArea { rect, target });
            }
        }
    }

    fn draw_marker(&mut self, baseline: f32) {
        let Some(m) = self.marker.take() else { return };
        match m.kind {
            MarkerKind::Text(t) => {
                let w = self.text_width(Font::Regular, &t, m.size);
                self.show_text(Font::Regular, m.size, m.right - w, baseline, m.color, &t);
            }
            MarkerKind::Check(checked) => {
                let s = m.size * 0.72;
                let (x, y) = (m.right - s, baseline - m.size * 0.05);
                let ops = &mut self.page().ops;
                nums(ops, &[m.color.0, m.color.1, m.color.2]);
                ops.extend_from_slice(b"RG 0.8 w ");
                nums(ops, &[x, y, s, s]);
                ops.extend_from_slice(b"re S\n");
                if checked {
                    let ops = &mut self.page().ops;
                    ops.extend_from_slice(b"1.2 w ");
                    nums(ops, &[x + s * 0.2, y + s * 0.5]);
                    ops.extend_from_slice(b"m ");
                    nums(ops, &[x + s * 0.42, y + s * 0.2]);
                    ops.extend_from_slice(b"l ");
                    nums(ops, &[x + s * 0.82, y + s * 0.85]);
                    ops.extend_from_slice(b"l S\n");
                }
            }
        }
    }

    /// Lay out a run of inline content as wrapped lines.
    fn text_block(&mut self, inlines: &[Inline], size: f32, ctx: Ctx, bold: bool) {
        // Images that cannot be loaded become inline placeholder text.
        let mut owned = Vec::new();
        let inlines = if inlines.iter().any(|i| matches!(i, Inline::Image { .. })) {
            for i in inlines {
                match i {
                    Inline::Image { alt, src } if self.load_image(src).is_none() => {
                        owned.push(placeholder(alt, src));
                    }
                    other => owned.push(other.clone()),
                }
            }
            &owned[..]
        } else {
            inlines
        };
        let toks = self.tokenize(inlines, size, ctx.color, bold);
        for item in self.wrap(toks, ctx.w, size) {
            match item {
                Laid::Line(line) => {
                    let lh = self.line_height(&line);
                    self.ensure(lh);
                    self.draw_line(&line, ctx.x, lh);
                    self.y -= lh;
                }
                Laid::Image { alt, src } => self.image(&alt, &src, ctx),
            }
        }
    }

    // ------------------------------------------------------------ images

    fn load_image(&mut self, src: &str) -> Option<usize> {
        if let Some(r) = self.image_cache.get(src) {
            return *r;
        }
        let result = self.read_image(src);
        let idx = match result {
            Ok(img) => {
                self.images.push(img);
                Some(self.images.len() - 1)
            }
            Err(e) => {
                self.warnings.push(format!("image '{src}': {e}"));
                None
            }
        };
        self.image_cache.insert(src.to_string(), idx);
        idx
    }

    fn read_image(&self, src: &str) -> Result<Image, String> {
        let lower = src.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("data:") {
            return Err("remote images are not supported".into());
        }
        let base = self.o.base_dir.as_ref().ok_or("image loading is disabled")?;
        let path = percent_decode(src.strip_prefix("file://").unwrap_or(src));
        let path = base.join(path);
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        image::load(&bytes)
    }

    fn image(&mut self, alt: &str, src: &str, ctx: Ctx) {
        let Some(idx) = self.load_image(src) else {
            return self.text_block(&[placeholder(alt, src)], self.o.font_size, ctx, false);
        };
        let (pw, ph) = (self.images[idx].width as f32, self.images[idx].height as f32);
        // Treat pixels as 96 dpi, then shrink to fit the column and page.
        let mut w = pw * 0.75;
        let mut h = ph * 0.75;
        let max_h = (self.top() - self.bottom()) * 0.95;
        if w > ctx.w {
            h *= ctx.w / w;
            w = ctx.w;
        }
        if h > max_h {
            w *= max_h / h;
            h = max_h;
        }
        let (w, h) = (w.max(1.0), h.max(1.0));
        self.ensure(h + 4.0);
        self.draw_marker(self.y - self.o.font_size);
        let y = self.y - h - 2.0;
        let ops = &mut self.page().ops;
        ops.extend_from_slice(b"q ");
        nums(ops, &[w, 0.0, 0.0, h, ctx.x, y]);
        let _ = writeln!(ops, "cm /Im{idx} Do Q");
        self.y -= h + 4.0;
    }

    // ------------------------------------------------------------ blocks

    fn blocks(&mut self, blocks: &[Block], ctx: Ctx) {
        let fs = self.o.font_size;
        let para_gap = if ctx.tight { fs * 0.25 } else { fs * 0.75 };
        for b in blocks {
            match b {
                Block::Heading { level, text } => self.heading(*level, text, ctx),
                Block::Paragraph(raw) => {
                    let inl = inline::parse(raw, self.refs);
                    self.text_block(&inl, fs, ctx, false);
                    self.gap(para_gap);
                }
                Block::Code(lines) => {
                    self.code_block(lines, ctx);
                    self.gap(para_gap.max(fs * 0.5));
                }
                Block::Quote(inner) => {
                    self.quote(inner, ctx);
                    self.gap(para_gap);
                }
                Block::List {
                    ordered,
                    start,
                    tight,
                    items,
                } => {
                    self.list(*ordered, *start, *tight, items, ctx);
                    self.gap(para_gap);
                }
                Block::Rule => {
                    self.gap(fs * 0.6);
                    self.ensure(fs * 0.6);
                    self.draw_marker(self.y - fs);
                    let y = self.y - fs * 0.3;
                    self.stroke_line(ctx.x, y, ctx.x + ctx.w, y, 0.75, RULE);
                    self.y -= fs * 0.6;
                    self.gap(fs * 0.6);
                }
                Block::Table { aligns, header, rows } => {
                    self.table(aligns, header, rows, ctx);
                    self.gap(fs * 0.75);
                }
            }
        }
        if self.marker.is_some() {
            // Empty list item: still show its bullet.
            let lh = fs * LINE_SPACING;
            self.ensure(lh);
            self.draw_marker(self.y - lh / 2.0 - fs * 0.26);
            self.y -= lh;
        }
    }

    fn heading(&mut self, level: u8, raw: &str, ctx: Ctx) {
        let fs = self.o.font_size;
        let scale = [2.0, 1.6, 1.3, 1.15, 1.0, 0.9][(level.clamp(1, 6) - 1) as usize];
        let size = fs * scale;
        self.gap(if level <= 2 { fs * 1.3 } else { fs * 1.0 });
        // Keep the heading together with the start of the following text.
        self.ensure(size * LINE_SPACING + fs * LINE_SPACING * 2.0);

        let inl = inline::parse(raw, self.refs);
        let title = inline::plain_text(&inl);
        let page = self.pages.len() - 1;
        let top = self.y + 2.0;
        let mut slug = slugify(&title);
        let n = self.slug_counts.entry(slug.clone()).or_insert(0);
        if *n > 0 {
            slug = format!("{slug}-{n}");
        }
        *n += 1;
        self.anchors.entry(slug).or_insert((page, top));
        self.headings.push(Heading {
            level,
            title,
            page,
            y: top,
        });

        let color = if level >= 6 { MUTED } else { ctx.color };
        self.text_block(&inl, size, Ctx { color, ..ctx }, true);
        if level <= 2 {
            let y = self.y - fs * 0.15;
            self.stroke_line(
                ctx.x,
                y,
                ctx.x + ctx.w,
                y,
                if level == 1 { 1.0 } else { 0.6 },
                RULE,
            );
            self.y -= fs * 0.3;
        }
        self.gap(fs * 0.55);
    }

    fn code_block(&mut self, lines: &[String], ctx: Ctx) {
        let fs = self.o.font_size;
        let size = fs * 0.85;
        let lh = size * 1.35;
        let pad = fs * 0.6;
        let char_w = size * 0.6;
        let max_chars = (((ctx.w - 2.0 * pad) / char_w).floor() as usize).max(1);

        self.ensure(pad + lh);
        let top_y = self.y;
        self.fill_rect(ctx.x, top_y - pad, ctx.w, pad, CODE_BG);
        self.draw_marker(top_y - pad - lh / 2.0 - size * 0.26);
        self.y -= pad;
        let empty: [String; 1] = [String::new()];
        let lines = if lines.is_empty() { &empty[..] } else { lines };
        for line in lines {
            let bytes = encoding::encode(line);
            let chunks: Vec<&[u8]> = if bytes.is_empty() {
                vec![&[][..]]
            } else {
                bytes.chunks(max_chars).collect()
            };
            for chunk in chunks {
                if self.y - lh < self.bottom() {
                    self.new_page();
                }
                self.at_top = false;
                self.fill_rect(ctx.x, self.y - lh, ctx.w, lh + 0.3, CODE_BG);
                let baseline = self.y - lh / 2.0 - size * 0.26;
                if !chunk.is_empty() {
                    self.show_text(Font::Mono, size, ctx.x + pad, baseline, ctx.color, chunk);
                }
                self.y -= lh;
            }
        }
        if self.y - pad < self.bottom() {
            self.new_page();
            self.at_top = false;
        }
        self.fill_rect(ctx.x, self.y - pad, ctx.w, pad + 0.3, CODE_BG);
        self.y -= pad;
    }

    fn quote(&mut self, inner: &[Block], ctx: Ctx) {
        let fs = self.o.font_size;
        let indent = fs * 1.2;
        self.ensure(fs * LINE_SPACING);
        let start = (self.pages.len() - 1, self.y);
        let child = Ctx {
            x: ctx.x + indent,
            w: (ctx.w - indent).max(fs),
            color: MUTED,
            ..ctx
        };
        self.blocks(inner, child);
        self.pending_gap = 0.0;
        let end = (self.pages.len() - 1, self.y);
        let bar_x = ctx.x + fs * 0.3;
        for p in start.0..=end.0 {
            let top = if p == start.0 { start.1 } else { self.top() };
            let bot = if p == end.0 { end.1 } else { self.bottom() };
            if top - bot > 0.5 {
                let ops = &mut self.pages[p].ops;
                nums(ops, &[RULE.0, RULE.1, RULE.2]);
                ops.extend_from_slice(b"rg ");
                nums(ops, &[bar_x, bot, 2.5, top - bot]);
                ops.extend_from_slice(b"re f\n");
            }
        }
    }

    fn list(&mut self, ordered: bool, start: u64, tight: bool, items: &[crate::markdown::Item], ctx: Ctx) {
        let fs = self.o.font_size;
        let last = start.saturating_add(items.len() as u64);
        let indent = if ordered {
            let widest = format!("{last}.");
            self.text_width(Font::Regular, widest.as_bytes(), fs) + fs * 0.6
        } else {
            fs * 1.5
        }
        .max(fs * 1.5);
        let bullet: &[u8] = match ctx.list_depth % 3 {
            0 => &[0x95],
            1 => &[0x96],
            _ => &[0xB7],
        };
        let child = Ctx {
            x: ctx.x + indent,
            w: (ctx.w - indent).max(fs * 2.0),
            tight,
            list_depth: ctx.list_depth + 1,
            ..ctx
        };
        for (k, item) in items.iter().enumerate() {
            let kind = match item.task {
                Some(c) => MarkerKind::Check(c),
                None if ordered => {
                    MarkerKind::Text(format!("{}.", start.saturating_add(k as u64)).into_bytes())
                }
                None => MarkerKind::Text(bullet.to_vec()),
            };
            self.marker = Some(Marker {
                kind,
                right: ctx.x + indent - fs * 0.45,
                size: fs,
                color: ctx.color,
            });
            self.blocks(&item.blocks, child);
            if !tight {
                self.gap(fs * 0.75);
            }
        }
    }

    fn table(&mut self, aligns: &[Align], header: &[String], rows: &[Vec<String>], ctx: Ctx) {
        let cols = aligns.len();
        if cols == 0 {
            return;
        }
        let fs = self.o.font_size;
        let size = fs * 0.92;
        let pad = fs * 0.45;
        let cell_toks = |l: &Self, raw: &str, bold: bool| -> Vec<Tok> {
            let mut inl = inline::parse(raw, l.refs);
            for i in inl.iter_mut() {
                if let Inline::Image { alt, .. } = i {
                    *i = Inline::Text {
                        text: format!("[{alt}]"),
                        style: Style::default(),
                        link: None,
                    };
                }
            }
            l.tokenize(&inl, size, ctx.color, bold)
        };
        let mut grid: Vec<Vec<Vec<Tok>>> = Vec::with_capacity(rows.len() + 1);
        grid.push(header.iter().map(|c| cell_toks(self, c, true)).collect());
        for r in rows {
            grid.push(r.iter().take(cols).map(|c| cell_toks(self, c, false)).collect());
        }

        // Natural (single-line) and minimum (longest word) column widths.
        let mut nat = vec![0.0f32; cols];
        let mut min = vec![0.0f32; cols];
        for row in &grid {
            for (c, toks) in row.iter().enumerate() {
                let mut total = 0.0;
                for t in toks {
                    match t {
                        Tok::Word(f) => {
                            let w: f32 = f.iter().map(|f| f.width).sum();
                            total += w;
                            min[c] = min[c].max(w.min(ctx.w / cols as f32));
                        }
                        Tok::Space(w) => total += w,
                        _ => {}
                    }
                }
                nat[c] = nat[c].max(total);
            }
        }
        let avail = ctx.w - 2.0 * pad * cols as f32;
        let (sum_nat, sum_min): (f32, f32) = (nat.iter().sum(), min.iter().sum());
        let widths: Vec<f32> = if sum_nat <= avail {
            nat.clone()
        } else if sum_min < avail && sum_nat > sum_min {
            let k = (avail - sum_min) / (sum_nat - sum_min);
            (0..cols).map(|c| min[c] + (nat[c] - min[c]) * k).collect()
        } else {
            let k = avail / sum_min.max(1.0);
            min.iter().map(|m| m * k).collect()
        };
        let widths: Vec<f32> = widths.iter().map(|w| w.max(size)).collect();
        let table_w: f32 = widths.iter().map(|w| w + 2.0 * pad).sum();

        self.ensure(size * LINE_SPACING * 2.0 + pad * 2.0);
        self.draw_marker(self.y - fs);
        self.stroke_line(ctx.x, self.y, ctx.x + table_w, self.y, 0.8, RULE);
        for (r, row) in grid.into_iter().enumerate() {
            let cells: Vec<Vec<Line>> = row
                .into_iter()
                .enumerate()
                .map(|(c, toks)| {
                    self.wrap(toks, widths[c], size)
                        .into_iter()
                        .filter_map(|l| if let Laid::Line(l) = l { Some(l) } else { None })
                        .collect()
                })
                .collect();
            let nlines = cells.iter().map(Vec::len).max().unwrap_or(0).max(1);
            let bg = r == 0;
            self.ensure(pad);
            if bg {
                self.fill_rect(ctx.x, self.y - pad, table_w, pad, CODE_BG);
            }
            self.y -= pad;
            for k in 0..nlines {
                let lh = cells
                    .iter()
                    .filter_map(|c| c.get(k))
                    .map(|l| self.line_height(l))
                    .fold(size * LINE_SPACING, f32::max);
                if self.y - lh < self.bottom() {
                    self.new_page();
                }
                self.at_top = false;
                if bg {
                    self.fill_rect(ctx.x, self.y - lh, table_w, lh + 0.3, CODE_BG);
                }
                let mut x = ctx.x;
                for (c, lines) in cells.iter().enumerate() {
                    if let Some(line) = lines.get(k) {
                        let slack = (widths[c] - line.width).max(0.0);
                        let off = match aligns[c] {
                            Align::Right => slack,
                            Align::Center => slack / 2.0,
                            _ => 0.0,
                        };
                        self.draw_line(line, x + pad + off, lh);
                    }
                    x += widths[c] + 2.0 * pad;
                }
                self.y -= lh;
            }
            if self.y - pad < self.bottom() {
                self.new_page();
            }
            if bg {
                self.fill_rect(ctx.x, self.y - pad, table_w, pad + 0.3, CODE_BG);
            }
            self.y -= pad;
            let (lw, color) = if bg { (0.8, MUTED) } else { (0.5, RULE) };
            self.stroke_line(ctx.x, self.y, ctx.x + table_w, self.y, lw, color);
        }
    }

    fn number_pages(&mut self) {
        let total = self.pages.len();
        let size = (self.o.font_size * 0.8).max(6.0);
        let y = (self.o.margin * 0.5 - size * 0.3).max(size * 0.5);
        for i in 0..total {
            let label = format!("{}", i + 1);
            let w = self.text_width(Font::Regular, label.as_bytes(), size);
            let x = (self.o.page_width - w) / 2.0;
            let mut ops = std::mem::take(&mut self.pages[i].ops);
            ops.extend_from_slice(b"BT ");
            nums(&mut ops, &[MUTED.0, MUTED.1, MUTED.2]);
            let _ = write!(ops, "rg /F{} ", Font::Regular.id());
            num(&mut ops, size);
            ops.extend_from_slice(b" Tf ");
            nums(&mut ops, &[x, y]);
            ops.extend_from_slice(b"Td ");
            pdf_text(&mut ops, label.as_bytes());
            ops.extend_from_slice(b" Tj ET\n");
            self.pages[i].ops = ops;
        }
    }
}

fn placeholder(alt: &str, src: &str) -> Inline {
    Inline::Text {
        text: format!("[image: {}]", if alt.is_empty() { src } else { alt }),
        style: Style {
            italic: true,
            ..Style::default()
        },
        link: None,
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formatting() {
        let mut v = Vec::new();
        for x in [0.0, 1.0, -2.5, 3.14159, 0.05, f32::NAN, 100.999] {
            num(&mut v, x);
            v.push(b' ');
        }
        assert_eq!(String::from_utf8(v).unwrap(), "0 1 -2.5 3.14 0.05 0 101 ");
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Hello, World! 2"), "hello-world-2");
        assert_eq!(percent_decode("a%20b%zz%2"), "a b%zz%2");
    }
}
