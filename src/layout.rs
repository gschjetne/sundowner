//! Page layout: turns the parsed document into positioned text runs, rules,
//! boxes and images on fixed-size pages.

use crate::arabic;
use crate::bidi;
use crate::chars;
use crate::fonts::{FaceId, Fonts};
use crate::gpos::Attachment;
use crate::gsub::{mask, Glyph, Script};
use crate::image::{self, Image};
use crate::inline::{self, Inline, Style};
use crate::linebreak::{self, Break};
use crate::markdown::{Align, Block, Document};
use crate::normalize;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

const LINE_SPACING: f32 = 1.4;
/// Longest run of characters shaped as one unit.
const MAX_RUN: usize = 1024;
/// Most combining marks kept with one base character.
const MAX_CLUSTER: usize = 32;
/// Most entries kept in the shaping cache.
const SHAPE_CACHE_SIZE: usize = 20_000;
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
    pub page_numbers: bool,
    /// Directory that relative image paths are resolved against. `None`
    /// disables loading local images.
    pub base_dir: Option<PathBuf>,
    pub title: Option<String>,
    /// Fonts and fallback chains used to set text.
    pub fonts: Arc<Fonts>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            page_width: 595.28,
            page_height: 841.89,
            margin: 56.7,
            font_size: 11.0,
            page_numbers: true,
            base_dir: None,
            title: None,
            fonts: Fonts::builtin(),
        }
    }
}

/// A positioned glyph. Distances are in 1/1000 em: `adv` moves the pen to
/// the next glyph (including kerning; zero for marks), and the glyph is
/// drawn offset by `dx`, `dy` from the pen (marks on their base).
#[derive(Clone, Copy, Debug, PartialEq)]
struct G {
    id: u16,
    adv: f32,
    dx: f32,
    dy: f32,
}

/// A run of text in a single font, size, color, link and bidirectional
/// embedding level. Glyphs are in logical order; right-to-left runs are
/// turned around when their line is drawn.
#[derive(Clone)]
struct Frag {
    face: FaceId,
    size: f32,
    glyphs: Vec<G>,
    /// Embedding level (UAX #9); odd levels are right-to-left.
    level: u8,
    width: f32,
    color: Color,
    code: bool,
    strike: bool,
    link: Option<Rc<str>>,
}

impl Frag {
    /// Whether glyphs in `face`, style `st` and embedding level `level` can
    /// be appended to this frag.
    fn same_run(&self, face: FaceId, st: &TextStyle, level: u8) -> bool {
        !self.glyphs.is_empty()
            && self.face == face
            && self.size == st.size
            && self.code == st.style.code
            && self.strike == st.style.strike
            && self.link == st.link
            && self.color == st.color
            && self.level == level
    }

    /// A copy with only some of the glyphs.
    fn piece(&self, range: std::ops::Range<usize>, width: f32) -> Frag {
        Frag {
            face: self.face,
            size: self.size,
            glyphs: self.glyphs[range].to_vec(),
            level: self.level,
            width,
            color: self.color,
            code: self.code,
            strike: self.strike,
            link: self.link.clone(),
        }
    }

    /// Whether `next` can be drawn as a continuation of this frag.
    fn joins(&self, next: &Frag) -> bool {
        !self.glyphs.is_empty()
            && !next.glyphs.is_empty()
            && self.face == next.face
            && self.size == next.size
            && self.code == next.code
            && self.strike == next.strike
            && self.link == next.link
            && self.color == next.color
            && self.level == next.level
    }
}

/// How a piece of inline text is set.
struct TextStyle {
    style: Style,
    size: f32,
    color: Color,
    link: Option<Rc<str>>,
    /// Width of a space.
    space: f32,
}

impl TextStyle {
    fn frag(&self, glyphs: Vec<G>, width: f32, face: FaceId, level: u8) -> Frag {
        Frag {
            face,
            size: self.size,
            glyphs,
            level,
            width,
            color: self.color,
            code: self.style.code,
            strike: self.style.strike,
            link: self.link.clone(),
        }
    }
}

/// What a character of a paragraph is (the index is its [`TextStyle`] or
/// image).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Text(usize),
    Space(usize),
    Break,
    Image(usize),
}

/// Text between two line break opportunities. It may contain spaces
/// where the line must not break (as gaps: frags without glyphs).
#[derive(Default)]
struct Word {
    frags: Vec<Frag>,
    /// A hyphen to show if the line breaks after this word (it ends with a
    /// soft hyphen).
    hyphen: Option<Frag>,
}

impl Word {
    fn width(&self) -> f32 {
        self.frags.iter().map(|f| f.width).sum()
    }
}

enum Tok {
    /// A line may break between two consecutive words, and after a space.
    Word(Word),
    /// Spaces: their width and embedding level.
    Space(f32, u8),
    Break,
    Image {
        alt: String,
        src: String,
    },
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
    pub fonts: Arc<Fonts>,
    /// Glyphs used per face (index = [`FaceId`]), with the text each glyph
    /// represents (several characters for a ligature, empty for the second
    /// and later glyphs of a character), for font subsetting and the
    /// ToUnicode map.
    pub used: Vec<BTreeMap<u16, String>>,
}

enum MarkerKind {
    Text(String),
    Check(bool),
}

struct Marker {
    kind: MarkerKind,
    /// Where the marker ends (left-to-right items) or starts (right-to-left
    /// items).
    edge: f32,
    rtl: bool,
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
    image_bytes: usize,
    headings: Vec<Heading>,
    anchors: HashMap<String, (usize, f32)>,
    slug_counts: HashMap<String, usize>,
    warnings: Vec<String>,
    used: RefCell<Vec<BTreeMap<u16, String>>>,
    missing: RefCell<BTreeSet<char>>,
    /// Kerning per glyph pair in 1/1000 em. Text repeats the same pairs
    /// constantly, so this avoids most GPOS lookups.
    kern_cache: RefCell<HashMap<(FaceId, u16, u16), f32>>,
    /// Shaped text by (text, style bits, size). Documents repeat the same
    /// words constantly, so this avoids most shaping work.
    shape_cache: RefCell<HashMap<(String, u8, u32), Runs>>,
}

impl<'a> Layout<'a> {
    fn new(o: &'a Options, refs: &'a HashMap<String, String>) -> Layout<'a> {
        Layout {
            o,
            refs,
            pages: vec![Page::default()],
            y: o.page_height - o.margin,
            at_top: true,
            pending_gap: 0.0,
            marker: None,
            images: Vec::new(),
            image_cache: HashMap::new(),
            image_bytes: 0,
            headings: Vec::new(),
            anchors: HashMap::new(),
            slug_counts: HashMap::new(),
            warnings: Vec::new(),
            used: RefCell::new(vec![BTreeMap::new(); o.fonts.faces.len()]),
            missing: RefCell::new(BTreeSet::new()),
            kern_cache: RefCell::new(HashMap::new()),
            shape_cache: RefCell::new(HashMap::new()),
        }
    }
}

/// A glyph from [`shape_text`]: its ID, advance, and x and y offsets.
pub type ShapedGlyph = (u16, f32, f32, f32);

/// Shape a piece of text in one style and direction the way layout does
/// (font fallback, normalization, Arabic joining, GSUB and GPOS), and
/// return its runs: the face, and for each glyph its ID, advance and x and
/// y offsets in 1/1000 em. Right-to-left runs are in logical order with
/// HarfBuzz's right-to-left positions. For tests and tools.
pub fn shape_text(
    text: &str,
    bold: bool,
    italic: bool,
    rtl: bool,
    o: &Options,
) -> Vec<(FaceId, Vec<ShapedGlyph>)> {
    let refs = HashMap::new();
    let l = Layout::new(o, &refs);
    l.shape_dir(text, false, bold, italic, 1000.0, rtl)
        .into_iter()
        .map(|(face, glyphs, _)| (face, glyphs.iter().map(|g| (g.id, g.adv, g.dx, g.dy)).collect()))
        .collect()
}

pub fn layout(doc: &Document, o: &Options) -> Output {
    let mut l = Layout::new(o, &doc.refs);
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
    let missing = l.missing.take();
    if !missing.is_empty() {
        let shown: Vec<String> = missing
            .iter()
            .take(12)
            .map(|c| format!("'{c}' (U+{:04X})", *c as u32))
            .collect();
        l.warnings.push(format!(
            "{} character(s) have no glyph in any configured font and are shown as boxes: {}{} \
             (add a [fallback] font that covers them to .sundowner)",
            missing.len(),
            shown.join(", "),
            if missing.len() > 12 { ", ..." } else { "" }
        ));
    }
    Output {
        pages: l.pages,
        images: l.images,
        headings: l.headings,
        anchors: l.anchors,
        title,
        warnings: l.warnings,
        fonts: o.fonts.clone(),
        used: l.used.into_inner(),
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

/// Append a text object drawing `glyphs` (2-byte glyph IDs, Identity-H).
/// `width` gives each glyph's advance as the PDF font declares it; where a
/// glyph's advance differs (kerning, zero-width marks) or it is offset (a
/// positioned mark), `TJ` adjustments and text rise (`Ts`) move it, in
/// thousandths of an em.
#[allow(clippy::too_many_arguments)]
fn glyph_ops(
    ops: &mut Vec<u8>,
    face: FaceId,
    size: f32,
    x: f32,
    y: f32,
    c: Color,
    glyphs: &[G],
    width: impl Fn(u16) -> f32,
) {
    ops.extend_from_slice(b"BT ");
    nums(ops, &[c.0, c.1, c.2]);
    let _ = write!(ops, "rg /F{face} ");
    num(ops, size);
    ops.extend_from_slice(b" Tf ");
    nums(ops, &[x, y]);
    let plain = |g: &G| g.dx == 0.0 && g.dy == 0.0 && (g.adv - width(g.id)).abs() < 0.005;
    if glyphs.iter().all(plain) {
        ops.extend_from_slice(b"Td <");
        for g in glyphs {
            let _ = write!(ops, "{:04X}", g.id);
        }
        ops.extend_from_slice(b"> Tj ET\n");
        return;
    }
    ops.extend_from_slice(b"Td [");
    // Pending pen movement to the right, and whether a hex string is open.
    let (mut shift, mut rise, mut open) = (0.0f32, 0.0f32, false);
    for g in glyphs {
        shift += g.dx;
        if g.dy != rise {
            // Text rise cannot change inside a TJ array.
            if open {
                ops.push(b'>');
                open = false;
            }
            if shift.abs() >= 0.005 {
                num(ops, -shift);
                shift = 0.0;
            }
            ops.extend_from_slice(b"] TJ ");
            num(ops, g.dy * size / 1000.0);
            ops.extend_from_slice(b" Ts [");
            rise = g.dy;
        }
        if shift.abs() >= 0.005 {
            if open {
                ops.push(b'>');
                open = false;
            }
            num(ops, -shift);
        }
        if !open {
            ops.push(b'<');
            open = true;
        }
        let _ = write!(ops, "{:04X}", g.id);
        shift = g.adv - width(g.id) - g.dx;
    }
    if open {
        ops.push(b'>');
    }
    ops.extend_from_slice(b"] TJ");
    if rise != 0.0 {
        ops.extend_from_slice(b" 0 Ts");
    }
    ops.extend_from_slice(b" ET\n");
}

/// Whether the text before and after a piece of text joins to it
/// cursively (Arabic letters joined across a style change).
#[derive(Clone, Copy, Default)]
struct Joins {
    before: bool,
    after: bool,
}

impl Joins {
    /// How `text[range]` joins to the rest of `text`.
    fn of(text: &[char], range: std::ops::Range<usize>) -> Joins {
        if !arabic::needs_joining(&text[range.clone()]) {
            return Joins::default();
        }
        Joins {
            before: arabic::neighbour(text, range.start, false).is_some_and(arabic::joins_forward),
            after: arabic::neighbour(text, range.end, true).is_some_and(arabic::joins_backward),
        }
    }
}

/// Glyph runs for a piece of text: `(face, glyphs, width)`.
type Runs = Vec<(FaceId, Vec<G>, f32)>;

/// Glyph runs in logical order with their embedding levels:
/// `(level, face, glyphs, width)`.
type LevelRuns = Vec<(u8, FaceId, Vec<G>, f32)>;

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
    fn fonts(&self) -> &Fonts {
        &self.o.fonts
    }

    /// Map text to glyphs, choosing a font per character from the fallback
    /// chain, then applying each font's glyph substitutions (ligatures,
    /// contextual alternates) to the runs of consecutive characters set in
    /// the same font and script.
    fn shape(&self, text: &str, mono: bool, bold: bool, italic: bool, size: f32) -> Runs {
        self.shape_dir(text, mono, bold, italic, size, false)
    }

    /// [`Layout::shape`] for text of one direction. Right-to-left text
    /// gets mirrored characters (UAX #9 rule L4), such as `)` for `(`; its
    /// glyphs stay in logical order.
    fn shape_dir(&self, text: &str, mono: bool, bold: bool, italic: bool, size: f32, rtl: bool) -> Runs {
        self.shape_joined(text, mono, bold, italic, size, rtl, Joins::default())
    }

    /// [`Layout::shape_dir`] for text that may be part of a longer run of
    /// cursively joined (Arabic) text: `joins` says whether the text around
    /// it joins to it.
    #[allow(clippy::too_many_arguments)]
    fn shape_joined(
        &self,
        text: &str,
        mono: bool,
        bold: bool,
        italic: bool,
        size: f32,
        rtl: bool,
        joins: Joins,
    ) -> Runs {
        let key = (
            text.to_string(),
            mono as u8
                | (bold as u8) << 1
                | (italic as u8) << 2
                | (rtl as u8) << 3
                | (joins.before as u8) << 4
                | (joins.after as u8) << 5,
            size.to_bits(),
        );
        if let Some(runs) = self.shape_cache.borrow().get(&key) {
            return runs.clone();
        }
        let runs = self.shape_uncached(text, mono, bold, italic, size, rtl, joins);
        let mut cache = self.shape_cache.borrow_mut();
        if cache.len() >= SHAPE_CACHE_SIZE {
            cache.clear();
        }
        cache.insert(key, runs.clone());
        runs
    }

    #[allow(clippy::too_many_arguments)]
    fn shape_uncached(
        &self,
        text: &str,
        mono: bool,
        bold: bool,
        italic: bool,
        size: f32,
        rtl: bool,
        joins: Joins,
    ) -> Runs {
        // The features each character gets: Arabic positional forms (from
        // the text with its joiners and non-joiners, before they are
        // dropped), and `rtlm` for right-to-left characters that were not
        // mirrored.
        let raw: Vec<char> = text.chars().collect();
        let forms = if arabic::needs_joining(&raw) {
            arabic::forms(&raw, joins.before, joins.after)
        } else {
            vec![0; raw.len()]
        };
        let mut chars = Vec::with_capacity(raw.len());
        let mut masks = Vec::with_capacity(raw.len());
        for (&c, form) in raw.iter().zip(forms) {
            if chars::is_invisible(c) {
                continue;
            }
            let c = if chars::is_space_like(c) { ' ' } else { c };
            let mirrored = if rtl { bidi::mirror(c) } else { None };
            chars.push(mirrored.unwrap_or(c));
            let rtlm = if rtl && mirrored.is_none() { mask::RTLM } else { 0 };
            masks.push(mask::GLOBAL | form | rtlm);
        }
        // Each base character is resolved together with the combining marks
        // that follow it, so they end up in one font. The first item of a
        // cluster gets its character's features.
        //
        // Punctuation, digits and other characters shared by all scripts
        // take the font of the Hebrew, Arabic, Armenian or CJK text around
        // them, if it has them (the text before them, or else after them),
        // rather than the first font of the chain.
        let own_font = |s: Option<Script>| {
            matches!(
                s,
                Some(
                    Script::Hebrew
                        | Script::Arabic
                        | Script::Armenian
                        | Script::Han
                        | Script::Kana
                        | Script::Hangul
                )
            )
        };
        let mut context: Option<FaceId> = None;
        // The next character with a script, from each position on.
        let mut next_letter: Vec<Option<char>> = vec![None; chars.len() + 1];
        for k in (0..chars.len()).rev() {
            next_letter[k] = if Script::of(chars[k]).is_some() {
                Some(chars[k])
            } else {
                next_letter[k + 1]
            };
        }
        let mut items: Vec<(FaceId, u16, char)> = Vec::new();
        let mut item_masks: Vec<u16> = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let mut j = i + 1;
            while j < chars.len() && j - i <= MAX_CLUSTER && normalize::is_mark(chars[j]) {
                j += 1;
            }
            let script = Script::of(chars[i]);
            let preferred = match script {
                _ if mono => None,
                Some(_) => None,
                None => context.or_else(|| {
                    let next = next_letter[j]?;
                    if !own_font(Script::of(next)) {
                        return None;
                    }
                    self.fonts().resolve(next, mono, bold, italic).map(|r| r.0)
                }),
            };
            let first = items.len();
            self.resolve_cluster(&chars[i..j], mono, bold, italic, preferred, &mut items);
            if script.is_some() {
                context = own_font(script).then(|| items.get(first).map(|x| x.0)).flatten();
            }
            let rest = masks[i] & (mask::GLOBAL | mask::RTLM);
            item_masks.extend((first..items.len()).map(|k| if k == first { masks[i] } else { rest }));
            i = j;
        }
        let mut runs = Vec::new();
        let mut start = 0;
        while start < items.len() {
            let face = items[start].0;
            let mut script = None;
            let mut end = start;
            while let Some(&(f, _, c)) = items.get(end) {
                let s = Script::of(c);
                // Very long runs (a word thousands of letters long) are
                // shaped in pieces to keep the work linear.
                if f != face || (script.is_some() && s.is_some() && s != script) || end - start == MAX_RUN {
                    break;
                }
                script = script.or(s);
                end += 1;
            }
            runs.push(self.shape_run(
                face,
                &items[start..end],
                &item_masks[start..end],
                script.unwrap_or(Script::Other),
                size,
                rtl,
            ));
            start = end;
        }
        runs
    }

    /// Resolve a character and the combining marks after it. The first font
    /// that covers the whole cluster, after normalizing it for that font
    /// (composing marks into precomposed characters the font has, or
    /// decomposing characters it lacks), sets it; if none does, each
    /// character is resolved on its own.
    ///
    /// A `preferred` face is tried first.
    fn resolve_cluster(
        &self,
        cluster: &[char],
        mono: bool,
        bold: bool,
        italic: bool,
        preferred: Option<FaceId>,
        items: &mut Vec<(FaceId, u16, char)>,
    ) {
        let hebrew = cluster.iter().any(|&c| Script::of(c) == Some(Script::Hebrew));
        let mut norm = Vec::new();
        if let Some(face) = preferred {
            let f = &self.fonts().faces[face];
            let forms = hebrew && !f.positions_marks(Script::Hebrew);
            if normalize::for_font(cluster, |c| f.glyph(c).is_some(), forms, &mut norm) {
                items.extend(norm.iter().map(|&c| (face, f.glyph(c).unwrap_or(0), c)));
                return;
            }
        }
        if let [c] = cluster {
            if let Some((face, gid)) = self.fonts().resolve(*c, mono, bold, italic) {
                items.push((face, gid, *c));
                return;
            }
        }
        for face in self.fonts().candidates(mono, bold, italic) {
            let f = &self.fonts().faces[face];
            norm.clear();
            let forms = hebrew && !f.positions_marks(Script::Hebrew);
            if normalize::for_font(cluster, |c| f.glyph(c).is_some(), forms, &mut norm) {
                items.extend(norm.iter().map(|&c| (face, f.glyph(c).unwrap_or(0), c)));
                return;
            }
        }
        for &c in cluster {
            self.resolve_char(c, mono, bold, italic, items, true);
        }
    }

    /// Resolve a character to a face and glyph, falling back to a plain-text
    /// substitute (for example `->` for an arrow) when no font covers it.
    fn resolve_char(
        &self,
        c: char,
        mono: bool,
        bold: bool,
        italic: bool,
        items: &mut Vec<(FaceId, u16, char)>,
        subst: bool,
    ) {
        if chars::is_invisible(c) {
            return;
        }
        let c = if chars::is_space_like(c) { ' ' } else { c };
        match self.fonts().resolve(c, mono, bold, italic) {
            Some((face, gid)) => items.push((face, gid, c)),
            None => {
                if let Some(s) = chars::substitute(c).filter(|_| subst) {
                    for sc in s.chars() {
                        self.resolve_char(sc, mono, bold, italic, items, false);
                    }
                    return;
                }
                if c != ' ' {
                    self.missing.borrow_mut().insert(c);
                }
                items.push((self.fonts().primary(mono, bold, italic), 0, c));
            }
        }
    }

    /// Substitute and position the glyphs of characters set in one face,
    /// and record the text each resulting glyph stands for. Right-to-left
    /// runs keep their glyphs in logical order, positioned as HarfBuzz
    /// positions right-to-left text (see [`right_to_left`]).
    fn shape_run(
        &self,
        face: FaceId,
        items: &[(FaceId, u16, char)],
        masks: &[u16],
        script: Script,
        size: f32,
        rtl: bool,
    ) -> (FaceId, Vec<G>, f32) {
        let font = &self.fonts().faces[face];
        let mut glyphs: Vec<Glyph> = items
            .iter()
            .zip(masks)
            .enumerate()
            .map(|(k, (&(_, id, _), &mask))| Glyph {
                id,
                cluster: k as u32,
                mask,
                component: 0,
            })
            .collect();
        font.substitute(script, &mut glyphs);
        let mut used = self.used.borrow_mut();
        for (k, g) in glyphs.iter().enumerate() {
            // The first glyph of a cluster represents all of its characters.
            let first = k == 0 || glyphs[k - 1].cluster != g.cluster;
            let text: String = if first {
                let end = glyphs[k + 1..]
                    .iter()
                    .map(|n| n.cluster)
                    .find(|&c| c != g.cluster)
                    .unwrap_or(items.len() as u32);
                items
                    .get(g.cluster as usize..end as usize)
                    .unwrap_or_default()
                    .iter()
                    .map(|i| i.2)
                    .collect()
            } else {
                String::new()
            };
            let entry = used[face].entry(g.id).or_default();
            if entry.is_empty() {
                *entry = text;
            }
        }
        drop(used);

        // Marks by the font's glyph classes, or else by their characters.
        let source = |g: &Glyph| items.get(g.cluster as usize).map_or(' ', |i| i.2);
        let marks: Vec<bool> = glyphs
            .iter()
            .map(|g| match font.glyph_class(g.id) {
                Some(class) => class == 3,
                None => normalize::is_mark(source(g)),
            })
            .collect();
        let fallback = || -> Vec<Option<Attachment>> {
            let ids: Vec<u16> = glyphs.iter().map(|g| g.id).collect();
            let classes: Vec<u8> = glyphs
                .iter()
                .map(|g| match source(g) {
                    c if normalize::is_mark(c) => normalize::ccc(c),
                    _ => 230,
                })
                .collect();
            fallback_marks(font, &ids, &marks, &classes)
        };
        // Every right-to-left run goes this way: `right_to_left` expects
        // HarfBuzz's right-to-left positions.
        if rtl || font.needs_positioning(script) {
            // Everything the font's GPOS table does, as HarfBuzz does it.
            let ids: Vec<u16> = glyphs.iter().map(|g| g.id).collect();
            let fallback = (marks.contains(&true) && !font.positions_marks(script)).then(fallback);
            let components: Vec<u8> = glyphs.iter().map(|g| g.component).collect();
            let pos = font.position(script, &ids, &components, &|k| marks[k], rtl, fallback.as_deref());
            let scale = 1000.0 / font.units_per_em as f32;
            let out: Vec<G> = ids
                .iter()
                .zip(pos)
                .map(|(&id, p)| G {
                    id,
                    adv: p.x_advance as f32 * scale,
                    dx: p.x_offset as f32 * scale,
                    dy: p.y_offset as f32 * scale,
                })
                .collect();
            let width = out.iter().map(|g| g.adv).sum::<f32>() * size / 1000.0;
            return (face, out, width);
        }
        let mut out: Vec<G> = glyphs
            .iter()
            .zip(&marks)
            .map(|(g, &mark)| G {
                id: g.id,
                adv: if mark { 0.0 } else { font.advance_1000(g.id) },
                dx: 0.0,
                dy: 0.0,
            })
            .collect();
        // Kerning between neighbouring glyphs, looking past marks.
        let mut prev: Option<usize> = None;
        for k in 0..out.len() {
            if marks[k] {
                continue;
            }
            if let Some(p) = prev {
                out[p].adv += self.kern_1000(face, out[p].id, out[k].id);
            }
            prev = Some(k);
        }
        if marks.iter().any(|&m| m) {
            let ids: Vec<u16> = out.iter().map(|g| g.id).collect();
            let attachments = if font.positions_marks(script) {
                let components: Vec<u8> = glyphs.iter().map(|g| g.component).collect();
                font.attach_marks(script, &ids, &components, &|k| marks[k])
            } else {
                fallback()
            };
            let scale = 1000.0 / font.units_per_em as f32;
            let mut pens = Vec::with_capacity(out.len());
            let mut pen = 0.0;
            for g in &out {
                pens.push(pen);
                pen += g.adv;
            }
            for k in 0..out.len() {
                if let Some(a) = attachments[k].filter(|a| a.parent < k) {
                    let p = out[a.parent];
                    out[k].dx = pens[a.parent] + p.dx + a.dx as f32 * scale - pens[k];
                    out[k].dy = p.dy + a.dy as f32 * scale;
                }
            }
        }
        let width = out.iter().map(|g| g.adv).sum::<f32>() * size / 1000.0;
        (face, out, width)
    }

    /// Add the kerning between the last glyph of `f` and `next` to `f`, for
    /// text that continues `f` on the same line.
    ///
    /// Like `shape_run`, this looks past marks: the pair is the last base
    /// glyph (non-zero advance) and `next`, not a trailing combining mark.
    fn kern_join(&self, f: &mut Frag, next: &G) {
        if next.adv == 0.0 {
            return;
        }
        let Some(b) = f.glyphs.iter().rposition(|g| g.adv != 0.0) else {
            return;
        };
        let k = self.kern_1000(f.face, f.glyphs[b].id, next.id);
        if k == 0.0 {
            return;
        }
        f.glyphs[b].adv += k;
        if f.level % 2 == 1 {
            // Right to left (see `right_to_left`), the glyph keeps its
            // place and the next one moves; so do the marks after it.
            f.glyphs[b].dx += k;
            for g in &mut f.glyphs[b + 1..] {
                g.dx += k;
            }
        } else {
            // Marks after the base are placed relative to the pen, which
            // the kerning just moved; keep them where they were.
            for g in &mut f.glyphs[b + 1..] {
                g.dx -= k;
            }
        }
        f.width += k * f.size / 1000.0;
    }

    /// Kerning between two glyphs of a face, in 1/1000 em.
    fn kern_1000(&self, face: FaceId, left: u16, right: u16) -> f32 {
        *self
            .kern_cache
            .borrow_mut()
            .entry((face, left, right))
            .or_insert_with(|| self.o.fonts.faces[face].kern_1000(left, right))
    }

    /// Shape a line of text that stands alone (a line of code, a list
    /// marker): resolve its embedding levels, with the direction `rtl` or
    /// else that of its first strong character, and shape each run of one
    /// level. The runs are in logical order; see [`Layout::visual_runs`].
    fn shape_levels(
        &self,
        text: &str,
        mono: bool,
        bold: bool,
        italic: bool,
        size: f32,
        rtl: Option<bool>,
    ) -> LevelRuns {
        let chars: Vec<char> = text.chars().collect();
        if rtl != Some(true) && !bidi::needs_resolving(&chars) {
            let runs = self.shape(text, mono, bold, italic, size);
            return runs.into_iter().map(|(f, g, w)| (0, f, g, w)).collect();
        }
        let mut l = bidi::resolve(&chars, rtl);
        let classes: Vec<_> = chars.iter().map(|&c| bidi::class(c)).collect();
        bidi::reset_whitespace(&classes, &mut l.levels, l.paragraph);
        let mut out = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let level = l.levels[i];
            let mut j = i + 1;
            while j < chars.len() && l.levels[j] == level {
                j += 1;
            }
            let seg: String = chars[i..j].iter().collect();
            let joins = Joins::of(&chars, i..j);
            for (f, g, w) in self.shape_joined(&seg, mono, bold, italic, size, level % 2 == 1, joins) {
                out.push((level, f, g, w));
            }
            i = j;
        }
        out
    }

    /// Put runs from [`Layout::shape_levels`] into visual order (UAX #9
    /// rule L2), turning right-to-left runs around.
    fn visual_runs(&self, runs: LevelRuns) -> Runs {
        let levels: Vec<u8> = runs.iter().map(|r| r.0).collect();
        let mut slots: Vec<Option<_>> = runs.into_iter().map(Some).collect();
        bidi::visual_order(&levels)
            .into_iter()
            .filter_map(|k| slots[k].take())
            .map(|(level, face, glyphs, w)| {
                if level % 2 == 0 {
                    return (face, glyphs, w);
                }
                (face, right_to_left(&glyphs), w)
            })
            .collect()
    }

    fn measure(&self, text: &str, mono: bool, bold: bool, italic: bool, size: f32) -> f32 {
        self.shape(text, mono, bold, italic, size)
            .iter()
            .map(|r| r.2)
            .sum()
    }

    fn show_runs(&mut self, runs: &Runs, size: f32, x: f32, y: f32, c: Color) {
        let mut x = x;
        for (face, glyphs, w) in runs {
            let fonts = self.o.fonts.clone();
            let font = &fonts.faces[*face];
            glyph_ops(&mut self.page().ops, *face, size, x, y, c, glyphs, |g| {
                font.advance_1000(g)
            });
            x += w;
        }
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

    // ------------------------------------------------------------ inline text

    /// Split inline content into words, spaces and hard breaks. Words end at
    /// the line break opportunities of the Unicode line breaking algorithm
    /// (UAX #14), computed over the whole paragraph so that they are right
    /// across style changes. Each piece of text between opportunities is
    /// shaped as a unit.
    ///
    /// Embedding levels come from the Unicode Bidirectional Algorithm (UAX
    /// #9), also over the whole paragraph; text changes frags where the
    /// level changes. The second result says whether the paragraph is
    /// right-to-left (its first strong character is).
    fn tokenize(&self, inlines: &[Inline], size: f32, color: Color, force_bold: bool) -> (Vec<Tok>, bool) {
        // The paragraph as one character sequence. Hard breaks and images
        // stand in as LINE SEPARATOR and OBJECT REPLACEMENT CHARACTER, which
        // have the right line breaking classes.
        let mut text: Vec<char> = Vec::new();
        let mut slots: Vec<Slot> = Vec::new();
        let mut styles: Vec<TextStyle> = Vec::new();
        let mut images: Vec<(&str, &str)> = Vec::new();
        for item in inlines {
            match item {
                Inline::Text { text: t, style, link } => {
                    let mut style = *style;
                    style.bold |= force_bold;
                    let (mono, bold, italic) = (style.code, style.bold, style.italic);
                    let fsize = if mono { size * 0.9 } else { size };
                    let space = match self.measure(" ", mono, bold, italic, fsize) {
                        w if w > 0.0 => w,
                        _ => fsize * 0.25,
                    };
                    styles.push(TextStyle {
                        style,
                        size: fsize,
                        color: if link.is_some() { LINK } else { color },
                        link: link.as_deref().map(Rc::from),
                        space,
                    });
                    let k = styles.len() - 1;
                    for c in t.chars() {
                        if chars::is_space_like(c) || c == ' ' {
                            // Runs of spaces collapse, except in code.
                            if !mono && slots.last().is_some_and(|s| matches!(s, Slot::Space(_))) {
                                continue;
                            }
                            // Tabs and newlines are ordinary spaces here.
                            text.push(if (c as u32) < 0x20 { ' ' } else { c });
                            slots.push(Slot::Space(k));
                        } else if chars::is_control(c) {
                            continue;
                        } else {
                            text.push(c);
                            slots.push(Slot::Text(k));
                        }
                    }
                }
                Inline::Break => {
                    text.push('\u{2028}');
                    slots.push(Slot::Break);
                }
                Inline::Image { alt, src } => {
                    text.push('\u{FFFC}');
                    slots.push(Slot::Image(images.len()));
                    images.push((alt, src));
                }
            }
        }
        let breaks = linebreak::opportunities(&text);
        let (levels, rtl) = if bidi::needs_resolving(&text) {
            let l = bidi::resolve(&text, None);
            (l.levels.clone(), l.rtl())
        } else {
            (vec![0; text.len()], false)
        };

        let mut toks = Vec::new();
        let mut word = Word::default();
        // Width and level of the spaces since the last text.
        let mut pending: Option<(f32, u8)> = None;
        // Whether the last word ended at an opportunity without a space, so
        // the next one continues it on the same line if both fit.
        let mut joined = false;
        let end_word = |word: &mut Word, toks: &mut Vec<Tok>| {
            if !word.frags.is_empty() {
                toks.push(Tok::Word(std::mem::take(word)));
            }
        };
        let mut i = 0;
        while i < text.len() {
            match slots[i] {
                Slot::Break => {
                    end_word(&mut word, &mut toks);
                    toks.push(Tok::Break);
                    pending = None;
                    joined = false;
                    i += 1;
                }
                Slot::Image(k) => {
                    end_word(&mut word, &mut toks);
                    let (alt, src) = images[k];
                    toks.push(Tok::Image {
                        alt: alt.to_string(),
                        src: src.to_string(),
                    });
                    pending = None;
                    joined = false;
                    i += 1;
                }
                Slot::Space(k) => {
                    pending.get_or_insert((0.0, levels[i])).0 += styles[k].space;
                    i += 1;
                }
                Slot::Text(k) => {
                    // A segment: text in one style up to the next space,
                    // opportunity or ZERO WIDTH NON-JOINER (which prevents
                    // ligatures).
                    let mut j = i + 1;
                    while j < text.len()
                        && slots[j] == Slot::Text(k)
                        && breaks[j] == Break::No
                        && text[j - 1] != '\u{200C}'
                        && levels[j] == levels[i]
                    {
                        j += 1;
                    }
                    let st = &styles[k];
                    if breaks[i] != Break::No {
                        // A soft hyphen right before the opportunity shows as
                        // a hyphen if the line breaks there.
                        if pending.is_none() && i > 0 && text[i - 1] == '\u{AD}' {
                            if let Slot::Text(h) = slots[i - 1] {
                                word.hyphen = self.hyphen(&styles[h], levels[i - 1]);
                            }
                        }
                        end_word(&mut word, &mut toks);
                        match pending.take() {
                            Some((w, level)) => {
                                toks.push(Tok::Space(w, level));
                                joined = false;
                            }
                            None => joined = matches!(toks.last(), Some(Tok::Word(_))),
                        }
                    } else if let Some((w, level)) = pending.take() {
                        // Spaces where the line must not break stay inside the word.
                        let face = self
                            .fonts()
                            .primary(st.style.code, st.style.bold, st.style.italic);
                        word.frags.push(st.frag(Vec::new(), w, face, level));
                    }
                    let seg: String = text[i..j].iter().collect();
                    let (mono, bold, italic) = (st.style.code, st.style.bold, st.style.italic);
                    let level = levels[i];
                    let joins = Joins::of(&text, i..j);
                    for (face, glyphs, w) in
                        self.shape_joined(&seg, mono, bold, italic, st.size, level % 2 == 1, joins)
                    {
                        if glyphs.is_empty() {
                            continue;
                        }
                        if word.frags.is_empty() && joined {
                            // Kerning with the end of the previous word, for
                            // when both end up on one line.
                            if let Some(Tok::Word(prev)) = toks.last_mut() {
                                if let Some(f) = prev.frags.last_mut().filter(|f| f.same_run(face, st, level))
                                {
                                    self.kern_join(f, &glyphs[0]);
                                }
                            }
                        }
                        joined = false;
                        match word.frags.last_mut() {
                            Some(f) if f.same_run(face, st, level) => {
                                self.kern_join(f, &glyphs[0]);
                                f.glyphs.extend_from_slice(&glyphs);
                                f.width += w;
                            }
                            _ => word.frags.push(st.frag(glyphs, w, face, level)),
                        }
                    }
                    i = j;
                }
            }
        }
        end_word(&mut word, &mut toks);
        (toks, rtl)
    }

    /// The hyphen shown where a line breaks at a soft hyphen.
    fn hyphen(&self, st: &TextStyle, level: u8) -> Option<Frag> {
        let (mono, bold, italic) = (st.style.code, st.style.bold, st.style.italic);
        let runs = self.shape("-", mono, bold, italic, st.size);
        let (face, glyphs, w) = runs.into_iter().next()?;
        Some(st.frag(glyphs, w, face, level))
    }

    fn wrap(&self, toks: &[Tok], max_w: f32, base_size: f32) -> Vec<Laid> {
        let max_w = max_w.max(1.0);
        let mut out = Vec::new();
        let new_line = || Line {
            size: base_size,
            ..Line::default()
        };
        let mut line = new_line();
        // Width and embedding level of the spaces before the next word.
        let mut space = 0.0f32;
        let mut space_level = 0u8;
        // The words on the current line: token index, and the number of
        // frags and the width of the line before the word.
        let mut placed: Vec<(usize, usize, f32)> = Vec::new();
        let push_frag = |line: &mut Line, x: f32, f: Frag| {
            line.size = line.size.max(f.size);
            line.width = x + f.width;
            line.frags.push((x, f));
        };
        let mut i = 0;
        while i < toks.len() {
            match &toks[i] {
                Tok::Space(w, level) => {
                    if !line.frags.is_empty() {
                        space += w;
                        space_level = *level;
                    }
                }
                Tok::Break => {
                    out.push(Laid::Line(std::mem::replace(&mut line, new_line())));
                    placed.clear();
                    space = 0.0;
                }
                Tok::Image { alt, src } => {
                    if !line.frags.is_empty() {
                        out.push(Laid::Line(std::mem::replace(&mut line, new_line())));
                    }
                    out.push(Laid::Image {
                        alt: alt.clone(),
                        src: src.clone(),
                    });
                    placed.clear();
                    space = 0.0;
                }
                Tok::Word(word) => {
                    let w = word.width();
                    if !line.frags.is_empty() && line.width + space + w > max_w {
                        // Break before this word. If the last word on the line
                        // needs a hyphen that does not fit, it moves to the
                        // next line too.
                        let mut restart = i;
                        while placed.len() > 1 {
                            let &(ti, nfrags, before) = placed.last().expect("placed is not empty");
                            let hyphen = match &toks[ti] {
                                Tok::Word(prev) => prev.hyphen.as_ref(),
                                _ => None,
                            };
                            if hyphen.is_none_or(|h| line.width + h.width <= max_w) {
                                break;
                            }
                            line.frags.truncate(nfrags);
                            line.width = before;
                            line.size = line.frags.iter().map(|f| f.1.size).fold(base_size, f32::max);
                            placed.pop();
                            restart = ti;
                        }
                        if let Some(&(ti, ..)) = placed.last() {
                            if let Tok::Word(Word { hyphen: Some(h), .. }) = &toks[ti] {
                                if let Some((_, last)) = line.frags.last_mut().filter(|(_, l)| l.joins(h)) {
                                    let before = last.width;
                                    self.kern_join(last, &h.glyphs[0]);
                                    line.width += last.width - before;
                                }
                                let x = line.width;
                                push_frag(&mut line, x, h.clone());
                            }
                        }
                        out.push(Laid::Line(std::mem::replace(&mut line, new_line())));
                        placed.clear();
                        space = 0.0;
                        if restart != i {
                            i = restart;
                            continue;
                        }
                    }
                    placed.push((i, line.frags.len(), line.width));
                    if line.frags.is_empty() && w > max_w {
                        // Break an overlong word at glyph boundaries.
                        for f in &word.frags {
                            let mut start = 0;
                            let mut x = line.width;
                            if f.glyphs.is_empty() {
                                push_frag(&mut line, x, f.clone());
                                continue;
                            }
                            let mut acc = 0.0;
                            for (k, g) in f.glyphs.iter().enumerate() {
                                let cw = g.adv * f.size / 1000.0;
                                // Never split a mark from its base.
                                if x + acc + cw > max_w && (x + acc) > 0.0 && g.adv != 0.0 {
                                    if k > start {
                                        let piece = f.piece(start..k, acc);
                                        push_frag(&mut line, x, piece);
                                    }
                                    out.push(Laid::Line(std::mem::replace(&mut line, new_line())));
                                    placed.clear();
                                    start = k;
                                    x = 0.0;
                                    acc = 0.0;
                                }
                                acc += cw;
                            }
                            if start < f.glyphs.len() {
                                let piece = f.piece(start..f.glyphs.len(), acc);
                                push_frag(&mut line, x, piece);
                            }
                        }
                    } else {
                        let mut x = line.width;
                        if !line.frags.is_empty() && space > 0.0 {
                            // The spaces become a frag without glyphs, so
                            // bidirectional reordering can place them.
                            let first = &word.frags[0];
                            let gap = Frag {
                                face: first.face,
                                size: first.size,
                                glyphs: Vec::new(),
                                level: space_level,
                                width: space,
                                color: first.color,
                                code: false,
                                strike: false,
                                link: None,
                            };
                            push_frag(&mut line, x, gap);
                            x += space;
                        }
                        for f in &word.frags {
                            push_frag(&mut line, x, f.clone());
                            x += f.width;
                        }
                    }
                    space = 0.0;
                }
            }
            i += 1;
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
        // Words that continue each other without a space are drawn as one
        // run; their widths already include the kerning between them.
        let mut frags: Vec<(f32, Frag)> = Vec::with_capacity(line.frags.len());
        for (x, f) in &line.frags {
            if let Some((px, p)) = frags.last_mut() {
                if p.joins(f) && (*px + p.width - x).abs() < 0.01 {
                    p.glyphs.extend_from_slice(&f.glyphs);
                    p.width = x + f.width - *px;
                    continue;
                }
            }
            frags.push((*x, f.clone()));
        }
        if frags.iter().any(|(_, f)| f.level > 0) {
            frags = self.reorder(frags);
        }
        for (fx, f) in &frags {
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
        for (fx, f) in &frags {
            let fx = x + fx;
            if !f.glyphs.is_empty() {
                let fonts = self.o.fonts.clone();
                let font = &fonts.faces[f.face];
                glyph_ops(
                    &mut self.page().ops,
                    f.face,
                    f.size,
                    fx,
                    baseline,
                    f.color,
                    &f.glyphs,
                    |g| font.advance_1000(g),
                );
            }
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

    /// Put the frags of a line, in logical order, into visual order (UAX #9
    /// rule L2), turning right-to-left frags around.
    fn reorder(&self, frags: Vec<(f32, Frag)>) -> Vec<(f32, Frag)> {
        let levels: Vec<u8> = frags.iter().map(|(_, f)| f.level).collect();
        let mut x = frags.first().map_or(0.0, |f| f.0);
        let mut slots: Vec<Option<Frag>> = frags.into_iter().map(|(_, f)| Some(f)).collect();
        let mut out = Vec::with_capacity(slots.len());
        for k in bidi::visual_order(&levels) {
            let Some(mut f) = slots[k].take() else { continue };
            if f.level % 2 == 1 {
                f.glyphs = right_to_left(&f.glyphs);
            }
            let w = f.width;
            out.push((x, f));
            x += w;
        }
        out
    }

    fn draw_marker(&mut self, baseline: f32) {
        let Some(m) = self.marker.take() else { return };
        match m.kind {
            MarkerKind::Text(t) => {
                let runs = self.visual_runs(self.shape_levels(&t, false, false, false, m.size, Some(m.rtl)));
                let w: f32 = runs.iter().map(|r| r.2).sum();
                let x = if m.rtl { m.edge } else { m.edge - w };
                self.show_runs(&runs, m.size, x, baseline, m.color);
            }
            MarkerKind::Check(checked) => {
                let s = m.size * 0.72;
                let x = if m.rtl { m.edge } else { m.edge - s };
                let y = baseline - m.size * 0.05;
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
        let (toks, rtl) = self.tokenize(inlines, size, ctx.color, bold);
        for item in self.wrap(&toks, ctx.w, size) {
            match item {
                Laid::Line(line) => {
                    let lh = self.line_height(&line);
                    self.ensure(lh);
                    // Right-to-left paragraphs are set flush right.
                    let x = if rtl {
                        ctx.x + (ctx.w - line.width).max(0.0)
                    } else {
                        ctx.x
                    };
                    self.draw_line(&line, x, lh);
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
        let result = self.read_image(src).and_then(|img| {
            let size = img.data.len() + img.smask.as_ref().map_or(0, Vec::len);
            if self.image_bytes + size > image::MAX_TOTAL_BYTES {
                return Err("total size of embedded images exceeds the limit".into());
            }
            self.image_bytes += size;
            Ok(img)
        });
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
        let base = self.o.base_dir.as_ref().ok_or("image loading is disabled")?;
        let path = confine(base, src)?;
        image::load(&image::read_file(&path)?)
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
        let avail = (ctx.w - 2.0 * pad).max(size);

        self.ensure(pad + lh);
        let top_y = self.y;
        self.fill_rect(ctx.x, top_y - pad, ctx.w, pad, CODE_BG);
        self.draw_marker(top_y - pad - lh / 2.0 - size * 0.26);
        self.y -= pad;
        let empty: [String; 1] = [String::new()];
        let lines = if lines.is_empty() { &empty[..] } else { lines };
        for line in lines {
            // Wrap long lines at the glyph that would overflow the box.
            // Lines of code are left-to-right paragraphs; right-to-left text
            // in them is reordered.
            let mut chunks: Vec<LevelRuns> = vec![Vec::new()];
            let mut x = 0.0;
            for (level, face, glyphs, _) in self.shape_levels(line, true, false, false, size, Some(false)) {
                for g in glyphs {
                    let w = g.adv * size / 1000.0;
                    // Never split a mark from its base.
                    if x + w > avail && x > 0.0 && g.adv != 0.0 {
                        chunks.push(Vec::new());
                        x = 0.0;
                    }
                    let chunk = chunks.last_mut().expect("chunks is never empty");
                    match chunk.last_mut() {
                        Some((l, f, gs, cw)) if *f == face && *l == level => {
                            gs.push(g);
                            *cw += w;
                        }
                        _ => chunk.push((level, face, vec![g], w)),
                    }
                    x += w;
                }
            }
            for chunk in chunks {
                if self.y - lh < self.bottom() {
                    self.new_page();
                }
                self.at_top = false;
                self.fill_rect(ctx.x, self.y - lh, ctx.w, lh + 0.3, CODE_BG);
                let baseline = self.y - lh / 2.0 - size * 0.26;
                let runs = self.visual_runs(chunk);
                self.show_runs(&runs, size, ctx.x + pad, baseline, ctx.color);
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
        // Right-to-left quotes have their bar on the right.
        let rtl = blocks_rtl(inner);
        let child = Ctx {
            x: if rtl { ctx.x } else { ctx.x + indent },
            w: (ctx.w - indent).max(fs),
            color: MUTED,
            ..ctx
        };
        self.blocks(inner, child);
        self.pending_gap = 0.0;
        let end = (self.pages.len() - 1, self.y);
        let bar_x = if rtl {
            ctx.x + ctx.w - fs * 0.3 - 2.5
        } else {
            ctx.x + fs * 0.3
        };
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
            self.measure(&format!("{last}."), false, false, false, fs) + fs * 0.6
        } else {
            fs * 1.5
        }
        .max(fs * 1.5);
        let bullet = match ctx.list_depth % 3 {
            0 => "\u{2022}",
            1 => "\u{2013}",
            _ => "\u{00B7}",
        };
        let child = Ctx {
            x: ctx.x + indent,
            w: (ctx.w - indent).max(fs * 2.0),
            tight,
            list_depth: ctx.list_depth + 1,
            ..ctx
        };
        for (k, item) in items.iter().enumerate() {
            // Right-to-left items are indented from the right, with the
            // marker on that side.
            let rtl = blocks_rtl(&item.blocks);
            let child = if rtl { Ctx { x: ctx.x, ..child } } else { child };
            let kind = match item.task {
                Some(c) => MarkerKind::Check(c),
                None if ordered => MarkerKind::Text(format!("{}.", start.saturating_add(k as u64))),
                None => MarkerKind::Text(bullet.to_string()),
            };
            self.marker = Some(Marker {
                kind,
                edge: if rtl {
                    ctx.x + ctx.w - indent + fs * 0.45
                } else {
                    ctx.x + indent - fs * 0.45
                },
                rtl,
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
        let cell_toks = |l: &Self, raw: &str, bold: bool| -> (Vec<Tok>, bool) {
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
        let mut grid: Vec<Vec<(Vec<Tok>, bool)>> = Vec::with_capacity(rows.len() + 1);
        grid.push(header.iter().map(|c| cell_toks(self, c, true)).collect());
        for r in rows {
            grid.push(r.iter().take(cols).map(|c| cell_toks(self, c, false)).collect());
        }

        // Natural (single-line) and minimum (longest word) column widths.
        let mut nat = vec![0.0f32; cols];
        let mut min = vec![0.0f32; cols];
        for row in &grid {
            for (c, (toks, _)) in row.iter().enumerate() {
                let mut total = 0.0;
                for t in toks {
                    match t {
                        Tok::Word(word) => {
                            let w = word.width();
                            total += w;
                            min[c] = min[c].max(w.min(ctx.w / cols as f32));
                        }
                        Tok::Space(w, _) => total += w,
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
            let cells: Vec<(Vec<Line>, bool)> = row
                .into_iter()
                .enumerate()
                .map(|(c, (toks, rtl))| {
                    let lines = self
                        .wrap(&toks, widths[c], size)
                        .into_iter()
                        .filter_map(|l| if let Laid::Line(l) = l { Some(l) } else { None })
                        .collect();
                    (lines, rtl)
                })
                .collect();
            let nlines = cells.iter().map(|c| c.0.len()).max().unwrap_or(0).max(1);
            let bg = r == 0;
            self.ensure(pad);
            if bg {
                self.fill_rect(ctx.x, self.y - pad, table_w, pad, CODE_BG);
            }
            self.y -= pad;
            for k in 0..nlines {
                let lh = cells
                    .iter()
                    .filter_map(|c| c.0.get(k))
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
                for (c, (lines, rtl)) in cells.iter().enumerate() {
                    if let Some(line) = lines.get(k) {
                        let slack = (widths[c] - line.width).max(0.0);
                        // Cells without an alignment follow their text's
                        // direction.
                        let off = match aligns[c] {
                            Align::Right => slack,
                            Align::Center => slack / 2.0,
                            Align::None if *rtl => slack,
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
            let runs = self.shape(&(i + 1).to_string(), false, false, false, size);
            let w: f32 = runs.iter().map(|r| r.2).sum();
            let mut x = (self.o.page_width - w) / 2.0;
            for (face, glyphs, gw) in &runs {
                let font = &self.o.fonts.faces[*face];
                glyph_ops(&mut self.pages[i].ops, *face, size, x, y, MUTED, glyphs, |g| {
                    font.advance_1000(g)
                });
                x += gw;
            }
        }
    }
}

/// Whether blocks read right-to-left: whether the first strong character
/// of their first text (UAX #9 rules P2 and P3) is right-to-left. Code
/// blocks and rules have no direction and are skipped.
fn blocks_rtl(blocks: &[Block]) -> bool {
    let text_rtl = |t: &str| bidi::first_strong(&t.chars().collect::<Vec<_>>());
    fn first(blocks: &[Block], text_rtl: &dyn Fn(&str) -> Option<bool>) -> Option<bool> {
        blocks.iter().find_map(|b| match b {
            Block::Heading { text, .. } | Block::Paragraph(text) => text_rtl(text),
            Block::Quote(inner) => first(inner, text_rtl),
            Block::List { items, .. } => items.iter().find_map(|i| first(&i.blocks, text_rtl)),
            Block::Table { header, .. } => header.iter().find_map(|h| text_rtl(h)),
            Block::Code(_) | Block::Rule => None,
        })
    }
    first(blocks, &text_rtl) == Some(true)
}

/// Put the glyphs of a right-to-left run, which are in logical order and
/// positioned as HarfBuzz positions right-to-left text, into visual order.
/// HarfBuzz's positions are made for exactly this: reversed, the glyphs
/// are drawn left to right like any others.
///
/// This relies on every right-to-left run having been positioned by
/// [`crate::position`] (see `shape_run`, which sends every right-to-left
/// run there, Hebrew as much as Arabic): the pairwise kerning and mark
/// attachment of left-to-right runs position glyphs differently.
fn right_to_left(glyphs: &[G]) -> Vec<G> {
    glyphs.iter().rev().copied().collect()
}

/// Place marks on their bases for a font without mark positioning data,
/// from the glyphs' outlines: centred horizontally, and stacked above the
/// base (or below it, by the mark's combining class) with a small gap.
fn fallback_marks(
    font: &crate::ttf::Face,
    ids: &[u16],
    marks: &[bool],
    classes: &[u8],
) -> Vec<Option<Attachment>> {
    let gap = font.units_per_em as i32 / 16;
    let mut out = vec![None; ids.len()];
    // The current base, its box, and the top and bottom of the stack.
    let mut base: Option<(usize, [i32; 4])> = None;
    let (mut top, mut bottom) = (0, 0);
    for k in 0..ids.len() {
        let bbox = font.glyph_bbox(ids[k]).map(|b| b.map(i32::from));
        if !marks[k] {
            base = bbox.map(|b| (k, b));
            if let Some(b) = bbox {
                (top, bottom) = (b[3], b[1]);
            }
            continue;
        }
        let (Some((parent, b)), Some(m)) = (base, bbox) else {
            continue;
        };
        let dx = (b[0] + b[2]) / 2 - (m[0] + m[2]) / 2;
        let dy = match classes[k] {
            // Overlays stay where they are designed.
            1 => 0,
            // Below the base.
            200..=204 | 218 | 220 | 222 | 233 => {
                let dy = bottom - gap - m[3];
                bottom = dy + m[1];
                dy
            }
            _ => {
                let dy = top + gap - m[1];
                top = dy + m[3];
                dy
            }
        };
        out[k] = Some(Attachment { parent, dx, dy });
    }
    out
}

/// Resolve an image reference to a file inside `base`. Only relative paths
/// are accepted, and the resolved file (after following symlinks) must stay
/// under the document directory, so a Markdown file cannot pull arbitrary
/// files such as `/etc/passwd` or `../../secret.png` into the PDF.
fn confine(base: &Path, src: &str) -> Result<PathBuf, String> {
    let lower = src.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Err("remote images are not supported".into());
    }
    if lower.contains("://") || lower.starts_with("file:") || lower.starts_with("data:") {
        return Err("only relative file paths are supported".into());
    }
    let rel = percent_decode(src);
    let rel = Path::new(&rel);
    let escapes = rel
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
    if rel.as_os_str().is_empty() || escapes {
        return Err("image path must be relative and inside the document directory".into());
    }
    let base = base
        .canonicalize()
        .map_err(|e| format!("document directory: {e}"))?;
    let full = base.join(rel).canonicalize().map_err(|e| e.to_string())?;
    if !full.starts_with(&base) {
        return Err("image path resolves outside the document directory".into());
    }
    Ok(full)
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
        for x in [0.0, 1.0, -2.5, 1.23456, 0.05, f32::NAN, 100.999] {
            num(&mut v, x);
            v.push(b' ');
        }
        assert_eq!(String::from_utf8(v).unwrap(), "0 1 -2.5 1.23 0.05 0 101 ");
    }

    #[test]
    fn fallback_marks_stack_above_and_below() {
        let fonts = Fonts::builtin();
        let f = &fonts.faces[0];
        let g = |c: char| f.glyph(c).unwrap();
        let ids = [g('A'), g('\u{301}'), g('\u{308}'), g('\u{323}')];
        let bbox = |c: char| f.glyph_bbox(g(c)).unwrap().map(i32::from);
        let a = fallback_marks(f, &ids, &[false, true, true, true], &[0, 230, 230, 220]);
        let (base, acute, dia, dot) = (bbox('A'), bbox('\u{301}'), bbox('\u{308}'), bbox('\u{323}'));
        let [Some(a1), Some(a2), Some(a3)] = [a[1], a[2], a[3]] else {
            panic!("{a:?}")
        };
        assert!(a.iter().skip(1).all(|x| x.is_some_and(|x| x.parent == 0)));
        // Centred on the base.
        let centre = |b: [i32; 4], dx: i32| (b[0] + b[2]) / 2 + dx;
        assert!((centre(acute, a1.dx) - (base[0] + base[2]) / 2).abs() <= 1);
        // The acute sits above the A, the diaeresis above the acute, the dot
        // below the A.
        assert!(acute[1] + a1.dy > base[3]);
        assert!(dia[1] + a2.dy > acute[3] + a1.dy);
        assert!(dot[3] + a3.dy < base[1]);
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Hello, World! 2"), "hello-world-2");
        assert_eq!(percent_decode("a%20b%zz%2"), "a b%zz%2");
    }

    #[test]
    fn image_paths_are_confined() {
        let dir = std::env::temp_dir().join(format!("sundowner-confine-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("img"));
        std::fs::write(dir.join("img/a.png"), b"x").unwrap();
        assert!(confine(&dir, "img/a.png").is_ok());
        assert!(confine(&dir, "./img/a.png").is_ok());
        assert!(confine(&dir, "img%2Fa.png").is_ok());
        for bad in [
            "/etc/passwd",
            "file:///etc/passwd",
            "file:img/a.png",
            "../x.png",
            "img/../../x.png",
            "%2e%2e/x.png",
            "http://example.com/a.png",
            "data:image/png;base64,AA==",
            "",
        ] {
            assert!(confine(&dir, bad).is_err(), "{bad} was accepted");
        }
        #[cfg(unix)]
        {
            let link = dir.join("img/link.png");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink("/etc/hostname", &link).unwrap();
            assert!(confine(&dir, "img/link.png").is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
