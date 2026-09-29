//! Page layout: turns the parsed document into positioned text runs, rules,
//! boxes and images on fixed-size pages.

use crate::arabic;
use crate::bidi;
use crate::chars;
use crate::fonts::{FaceId, Fonts};
use crate::front_matter::{self, Table};
use crate::gpos::Attachment;
use crate::gsub::{mask, Glyph, Script};
use crate::hyphenate::{self, Patterns};
use crate::image::{self, Image};
use crate::inline::{self, Inline, Style};
use crate::linebreak::{self, Break};
use crate::markdown::{Align, Block, Document};
use crate::normalize;
use crate::tags::{self, Tagger};
use crate::yaml;
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
/// Most entries kept in the shaping and hyphenation caches: enough for the
/// words of a novel and their syllables.
const SHAPE_CACHE_SIZE: usize = 50_000;
const TEXT: Color = (0.11, 0.11, 0.12);
const MUTED: Color = (0.4, 0.4, 0.43);
const LINK: Color = (0.02, 0.35, 0.75);
const CODE_BG: Color = (0.95, 0.95, 0.96);
const RULE: Color = (0.82, 0.82, 0.85);

// Page breaking. Breaks are chosen for the whole document at once, as the
// Knuth-Plass algorithm chooses line breaks for a paragraph: every place a
// page may break has a penalty, every page costs its unused space, and the
// breaks with the least total cost win (see `plan_pages`). The penalties
// are on the scale of plain TeX's.

/// Penalty that rules out a break unless nothing else fits.
const FORBID: f32 = 1e6;
/// A break after the first line of a paragraph, leaving it alone at the
/// bottom of a page (TeX's `\clubpenalty`).
const CLUB: f32 = 150.0;
/// A break before the last line of a paragraph, leaving it alone at the top
/// of a page (TeX's `\widowpenalty`).
const WIDOW: f32 = 150.0;
/// Breaks inside a block quote.
const IN_QUOTE: f32 = 100.0;
/// Breaks inside a list item, and between the items of a list.
const IN_ITEM: f32 = 60.0;
const BETWEEN_ITEMS: f32 = 20.0;
/// Breaks inside a code block, and near its ends (fewer than
/// `CODE_EDGE_LINES` lines on one side).
const IN_CODE: f32 = 100.0;
const CODE_EDGE: f32 = 200.0;
const CODE_EDGE_LINES: usize = 3;
/// Breaks between the rows of a table, and inside a row. A row is kept
/// together only while it is at most `TALL_ROW` of the text area high; a
/// taller one is split like a quote, rather than moved whole and leaving a
/// hole of up to a page above it.
const TABLE_ROW: f32 = 50.0;
const IN_ROW: f32 = 1000.0;
const IN_TALL_ROW: f32 = 100.0;
const TALL_ROW: f32 = 0.5;
/// Breaks in front matter cost this more than [`TABLE_ROW`] for each key
/// that spans the rows on both sides, so they fall between the largest
/// groups of rows that can be kept together.
const IN_SPAN: f32 = 50.0;
/// Breaks before a heading of level 1 to 3 are encouraged, so that pages
/// start with a new section (plain TeX's `\beginsection` does the same).
const SECTION: [f32; 3] = [-100.0, -60.0, -30.0];
/// Cost of every page, so that pages are not added to avoid penalties.
const PAGE_COST: f32 = 10000.0;
/// Cost of a page left entirely empty; less empty pages cost this times the
/// cube of the empty fraction, so a line or two costs next to nothing.
const EMPTY_COST: f32 = 2000.0;

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
    /// Whether to show YAML front matter as a table. `None` leaves it out
    /// with a warning, as nobody has said whether they want it.
    pub front_matter: Option<bool>,
    /// Fonts and fallback chains used to set text.
    pub fonts: Arc<Fonts>,
    /// Whether paragraphs are justified (set flush on both sides).
    pub justify: bool,
    /// The language of the text (a BCP 47 tag such as `en-US`), from the
    /// command line: it overrides the front matter's `lang`.
    pub lang: Option<String>,
    /// The language to assume when neither the command line nor the front
    /// matter gives one (from `.sundowner`).
    pub default_lang: Option<String>,
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
            front_matter: None,
            fonts: Fonts::builtin(),
            justify: true,
            lang: None,
            default_lang: None,
        }
    }
}

/// A positioned glyph. Distances are in 1/1000 em: `adv` moves the pen to
/// the next glyph (including kerning; zero for marks), and the glyph is
/// drawn offset by `dx`, `dy` from the pen (marks on their base). `text`
/// is the text it stands for, in [`Layout::texts`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct G {
    id: u16,
    adv: f32,
    dx: f32,
    dy: f32,
    text: u32,
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
    /// soft hyphen or at a hyphenation point).
    hyphen: Option<Frag>,
    /// The kerning added to the last frag for the word that continues it
    /// without a space, in 1/1000 em; taken off again if the line breaks
    /// between them.
    join_kern: f32,
    /// Where the word may be lengthened, if the line it is on is justified.
    kashida: Option<Kashida>,
}

/// A place where an Arabic word may be lengthened by kashidas: tatweels
/// (U+0640) inserted between two of its joined letters, which is then
/// shaped again. The part of the word shaped with them is a segment of one
/// style, set in one run.
struct Kashida {
    /// The frag of the word the segment is in, and its glyphs there.
    frag: usize,
    glyphs: std::ops::Range<usize>,
    /// The segment, and where in it the tatweels go.
    text: Vec<char>,
    at: usize,
    bold: bool,
    italic: bool,
    rtl: bool,
    joins: Joins,
    /// Its priority, from [`arabic::kashida`]: lower is better.
    priority: u8,
    /// The advance of the segment's glyphs as shaped, in 1/1000 em.
    /// Kerning added to its last glyph later (with text that continues it)
    /// is the difference.
    adv: f32,
    /// How much wider each tatweel makes the word, and how many it may
    /// take (see [`KASHIDA_MAX`]).
    step: f32,
    most: usize,
}

impl Word {
    fn width(&self) -> f32 {
        self.frags.iter().map(|f| f.width).sum()
    }
}

/// How the lines of a paragraph are broken and set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// Each line takes as many words as fit; set flush with the start.
    Greedy,
    /// The breaks are chosen for the whole paragraph (see [`total_fit`]),
    /// for an even ragged edge; set flush with the start.
    Ragged,
    /// The breaks are chosen for the whole paragraph, and the spaces of
    /// each line but the last widened or narrowed to fill it.
    Justify,
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
    /// Its `Link` structure element.
    pub elem: usize,
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
    /// The structure tree: its elements, the root first.
    pub structure: Vec<tags::Elem>,
    /// For each page, the structure element of each marked-content
    /// sequence on it, by MCID.
    pub marked: Vec<Vec<usize>>,
    /// The language of the text (a BCP 47 tag), if it is known.
    pub lang: Option<String>,
}

enum MarkerKind {
    Text(String),
    Check(bool),
}

struct Marker {
    kind: MarkerKind,
    /// Its `Lbl` structure element.
    lbl: usize,
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

/// A place where a page may break, recorded in the first pass: where the
/// content before it ends and the content after it starts (they differ by
/// the gap between them, which is dropped at the top of a page), and the
/// penalty of breaking there.
#[derive(Clone, Copy, Debug)]
pub struct Breakpoint {
    pub above: f32,
    pub below: f32,
    pub penalty: f32,
}

/// Lines laid out in the first pass, for the second.
enum Laidout {
    Text(Vec<Laid>, bool),
    Code(Vec<LevelRuns>),
    Table(Vec<f32>, f32, Vec<Vec<(Vec<Line>, bool)>>),
}

enum Pass {
    /// Lay out on one endless page and record where pages may break.
    Measure(Vec<Breakpoint>),
    /// Lay out on pages, breaking at the chosen breakpoints.
    Set(Vec<bool>),
}

struct Layout<'a> {
    o: &'a Options,
    refs: &'a HashMap<String, String>,
    pages: Vec<Page>,
    /// Where the content on each full page ends.
    page_ends: Vec<f32>,
    y: f32,
    at_top: bool,
    pending_gap: f32,
    pass: Pass,
    /// Places the page may break so far in this pass.
    breakpoints: usize,
    /// Penalty of breaks in the current block, from the blocks it is in.
    keep: f32,
    /// Penalty of the next break instead of `keep`, if set.
    next_penalty: Option<f32>,
    /// Blocks laid out in the first pass, in order, and how many of them
    /// have been used in this pass.
    laidout: Vec<Option<Laidout>>,
    blocks_laid: usize,
    marker: Option<Marker>,
    images: Vec<Image>,
    image_cache: HashMap<String, Option<usize>>,
    image_bytes: usize,
    headings: Vec<Heading>,
    anchors: HashMap<String, (usize, f32)>,
    slug_counts: HashMap<String, usize>,
    warnings: Vec<String>,
    used: RefCell<Vec<BTreeMap<u16, String>>>,
    /// The same, as the index in [`Layout::texts`] of each glyph's text, by
    /// face and glyph ID, for drawing (see [`draw_glyphs`]).
    mapped: RefCell<Vec<Vec<u32>>>,
    missing: RefCell<BTreeSet<char>>,
    /// Kerning per glyph pair in 1/1000 em. Text repeats the same pairs
    /// constantly, so this avoids most GPOS lookups.
    kern_cache: RefCell<HashMap<(FaceId, u16, u16), f32>>,
    /// Shaped text by (text, style bits, size). Documents repeat the same
    /// words constantly, so this avoids most shaping work.
    shape_cache: RefCell<HashMap<(String, u8, u32), Runs>>,
    /// Hyphenation patterns for the document's language, if it is known
    /// and sundowner has them.
    hyph: Option<&'static Patterns>,
    /// Hyphenation points of words (lowercase).
    hyph_cache: RefCell<HashMap<String, Rc<[usize]>>>,
    /// Whether a word may be split between two letters, in a style (bold
    /// and italic bits), without changing its glyphs.
    split_cache: RefCell<HashMap<(char, char, u8), bool>>,
    /// The texts glyphs stand for ([`G::text`]), each once; the first is
    /// empty.
    texts: RefCell<(Vec<String>, HashMap<String, u32>)>,
    /// The structure tree, built in the second pass.
    tags: Tagger,
    /// The `Link` element that text of the last link drawn went to: its
    /// target, the element, and the element it is in.
    last_link: Option<(Rc<str>, usize, usize)>,
}

impl<'a> Layout<'a> {
    fn new(o: &'a Options, refs: &'a HashMap<String, String>) -> Layout<'a> {
        Layout {
            o,
            refs,
            pages: vec![Page::default()],
            page_ends: Vec::new(),
            y: o.page_height - o.margin,
            at_top: true,
            pending_gap: 0.0,
            pass: Pass::Measure(Vec::new()),
            breakpoints: 0,
            keep: 0.0,
            next_penalty: None,
            laidout: Vec::new(),
            blocks_laid: 0,
            marker: None,
            images: Vec::new(),
            image_cache: HashMap::new(),
            image_bytes: 0,
            headings: Vec::new(),
            anchors: HashMap::new(),
            slug_counts: HashMap::new(),
            warnings: Vec::new(),
            used: RefCell::new(vec![BTreeMap::new(); o.fonts.faces.len()]),
            mapped: RefCell::new(vec![Vec::new(); o.fonts.faces.len()]),
            missing: RefCell::new(BTreeSet::new()),
            kern_cache: RefCell::new(HashMap::new()),
            shape_cache: RefCell::new(HashMap::new()),
            hyph: None,
            hyph_cache: RefCell::new(HashMap::new()),
            split_cache: RefCell::new(HashMap::new()),
            texts: RefCell::new((vec![String::new()], HashMap::new())),
            tags: Tagger::default(),
            last_link: None,
        }
    }

    /// Record that glyph `gid` of `face` maps to text `text` (an index in
    /// [`Layout::texts`]) in the ToUnicode map.
    fn map_glyph(&self, face: FaceId, gid: u16, text: u32) {
        let m = &mut self.mapped.borrow_mut()[face];
        if m.len() <= gid as usize {
            m.resize(gid as usize + 1, 0);
        }
        m[gid as usize] = text;
    }

    /// The index of `text` in [`Layout::texts`].
    fn intern(&self, text: &str) -> u32 {
        if text.is_empty() {
            return 0;
        }
        let mut t = self.texts.borrow_mut();
        if let Some(&k) = t.1.get(text) {
            return k;
        }
        let k = t.0.len() as u32;
        t.0.push(text.to_string());
        t.1.insert(text.to_string(), k);
        k
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
    let front = l.front_matter(doc.front_matter.as_deref(), ctx);
    let lang = o
        .lang
        .clone()
        .or_else(|| doc.front_matter.as_deref().and_then(front_matter::language))
        .or_else(|| o.default_lang.clone());
    let mut known_lang = None;
    if let Some(lang) = lang {
        if !hyphenate::valid_tag(&lang) {
            l.warnings.push(format!(
                "the front matter's lang '{lang}' is not a language tag (such as en or en-US), \
                 so text is not hyphenated"
            ));
        } else if let Some(p) = hyphenate::for_language(&lang) {
            l.hyph = Some(p);
            known_lang = Some(lang);
        } else {
            l.warnings.push(format!(
                "there are no hyphenation patterns for the language '{lang}', so text is not \
                 hyphenated (only English is hyphenated so far)"
            ));
            known_lang = Some(lang);
        }
    }
    // Lay the document out twice: once to find where pages may break, and
    // again to break them where it is best. The second pass reuses the
    // first one's lines, shaping and images.
    let content = |l: &mut Layout| {
        if let Some(t) = &front {
            l.span_table(t, ctx);
            l.gap(o.font_size * 1.5);
        }
        l.blocks(&doc.blocks, ctx);
    };
    content(&mut l);
    if let Pass::Measure(breakpoints) = &l.pass {
        let plan = plan_pages(breakpoints, l.y, l.top() - l.bottom());
        l.restart(plan);
        content(&mut l);
    }
    l.unmark();
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
        marked: std::mem::take(&mut l.tags.marked),
        structure: l.tags.finish(),
        lang: known_lang,
    }
}

/// Choose where pages break: for each breakpoint, whether a new page starts
/// there. `end` is where the document ends and `page_h` the height of the
/// text area.
///
/// Each page costs [`PAGE_COST`] plus the penalty of the break that ends it
/// plus its badness: [`EMPTY_COST`] times the cube of the fraction of it
/// left empty. The last page's empty space is free, as a paragraph's last
/// line is in Knuth and Plass's line breaking, so space left over at the end
/// of the document goes to keeping blocks together earlier on. The cube
/// makes a line or two of space cost next to nothing, while half a page
/// costs as much as a bad break. Dynamic programming finds the breaks with
/// the least total cost. For each breakpoint it tries the page starts that
/// fit above it, so the time is the number of breakpoints times the number
/// that fit on a page: linear in the length of the document, as a page
/// holds a bounded number of them.
pub fn plan_pages(breakpoints: &[Breakpoint], end: f32, page_h: f32) -> Vec<bool> {
    let n = breakpoints.len();
    if n == 0 {
        return Vec::new();
    }
    let page_h = page_h.max(1.0) as f64;
    // Best total cost of the pages before breakpoint j, when a page starts
    // there, and where the page before it starts.
    let mut cost = vec![f64::INFINITY; n + 1];
    let mut from = vec![0usize; n + 1];
    cost[0] = 0.0;
    for j in 1..=n {
        let (above, penalty, last) = match breakpoints.get(j) {
            Some(b) => (b.above, b.penalty.max(-FORBID) as f64, false),
            None => (end, 0.0, true),
        };
        for i in (0..j).rev() {
            let h = (breakpoints[i].below - above) as f64;
            // A page must start at the previous breakpoint when even that
            // content does not fit.
            if h > page_h + 0.01 && i + 1 < j {
                break;
            }
            let empty = ((page_h - h) / page_h).clamp(0.0, 1.0);
            let badness = if h > page_h + 0.01 {
                FORBID as f64 * 10.0
            } else if last {
                0.0
            } else {
                EMPTY_COST as f64 * empty * empty * empty
            };
            let c = cost[i] + PAGE_COST as f64 + badness + penalty;
            if c < cost[j] {
                cost[j] = c;
                from[j] = i;
            }
        }
    }
    let mut plan = vec![false; n];
    let mut j = from[n];
    while j > 0 {
        plan[j] = true;
        j = from[j];
    }
    plan
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

/// Draw glyphs of `face` (in visual order), with the text each stands for
/// (in `texts`) wherever the font's Unicode map (`mapped`: each glyph's
/// text in the ToUnicode map) does not give it, as the `ActualText` of a
/// span holding just that glyph, so that readers still put the text in
/// order themselves. That is a hyphen shown where a line breaks at a
/// hyphenation point (a soft hyphen), a glyph that stands for other text
/// elsewhere, and a character no font has (glyph 0, `.notdef`), which is
/// drawn as the outline of a box, as PDF/A does not allow `.notdef` to be
/// shown, with an invisible space in it: readers take the text of glyphs,
/// not of paths.
#[allow(clippy::too_many_arguments)]
fn draw_glyphs(
    ops: &mut Vec<u8>,
    texts: &[String],
    mapped: &[u32],
    font: &crate::ttf::Face,
    face: FaceId,
    size: f32,
    (x, y): (f32, f32),
    c: Color,
    glyphs: &[G],
) {
    let width = |g: u16| font.advance_1000(g);
    let text = |g: &G| texts[g.text as usize].as_str();
    let special = |g: &G| g.id == 0 || mapped.get(g.id as usize).copied().unwrap_or(0) != g.text;
    if !glyphs.iter().any(special) {
        glyph_ops(ops, face, size, x, y, c, glyphs, width);
        return;
    }
    let at = |pen: f32| x + pen * size / 1000.0;
    // The first glyph not drawn yet, and the pen before it and now.
    let (mut start, mut start_pen, mut pen) = (0, 0.0, 0.0);
    for (k, g) in glyphs.iter().enumerate() {
        if special(g) {
            if k > start {
                glyph_ops(ops, face, size, at(start_pen), y, c, &glyphs[start..k], width);
            }
            tags::begin_actual_text(ops, text(g));
            if g.id == 0 {
                let w = if g.adv > 0.0 { g.adv } else { 500.0 } * size / 1000.0;
                let (bx, by) = (at(pen + g.dx), y + g.dy * size / 1000.0);
                nums(ops, &[c.0, c.1, c.2]);
                ops.extend_from_slice(b"RG ");
                num(ops, size * 0.05);
                ops.extend_from_slice(b" w ");
                nums(ops, &[bx + w * 0.12, by, w * 0.76, size * 0.68]);
                ops.extend_from_slice(b"re S\n");
                if let Some(space) = font.glyph(' ') {
                    let _ = write!(ops, "BT 3 Tr /F{face} ");
                    num(ops, size);
                    ops.extend_from_slice(b" Tf ");
                    nums(ops, &[bx, by]);
                    let _ = writeln!(ops, "Td <{space:04X}> Tj 0 Tr ET");
                }
            } else {
                glyph_ops(ops, face, size, at(pen), y, c, std::slice::from_ref(g), width);
            }
            ops.extend_from_slice(b"EMC\n");
            (start, start_pen) = (k + 1, pen + g.adv);
        }
        pen += g.adv;
    }
    if start < glyphs.len() {
        glyph_ops(ops, face, size, at(start_pen), y, c, &glyphs[start..], width);
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

/// How much wider (as a fraction of their width) the spaces of a justified
/// line may become, as far as the choice of breaks is concerned: at this
/// stretch a line is loose, with a badness of 100.
const STRETCH: f32 = 0.5;
/// How much narrower the spaces of a justified line may become: never
/// more than this.
const SHRINK: f32 = 0.25;
/// The most the spaces of a justified line are widened, as a fraction of
/// their width. A line that would need more, as when a long word or URL
/// follows it, is set flush with the start instead.
const MAX_GROWTH: f32 = 3.0;
/// The stretch of a ragged line, in ems: a line this much short of the
/// measure has a badness of 100.
const RAGGED_STRETCH: f32 = 3.0;
/// Demerits of every line, so that fewer lines are better (TeX's
/// `\linepenalty`).
const LINE_PENALTY: f64 = 10.0;
/// Penalty of a break at a hyphen that is not in the text (`\hyphenpenalty`).
const HYPHEN_PENALTY: f64 = 100.0;
/// Demerits of two hyphenated lines in a row (`\doublehyphendemerits`).
const DOUBLE_HYPHEN: f64 = 10_000.0;
/// Demerits of hyphenating the last full line (`\finalhyphendemerits`).
const FINAL_HYPHEN: f64 = 5_000.0;
/// Demerits of a tight or decent line next to a very loose one, or a
/// tight one next to a loose one (`\adjdemerits`).
const ADJACENT_FITNESS: f64 = 10_000.0;
/// How far (as a multiple of its stretch) a line may be short of the
/// measure to be considered at all.
const HOPELESS: f64 = 12.0;
/// Badness of a line that is too wide however it is set: a word longer
/// than the line, which will be split.
const OVERFULL: f64 = 1e6;
/// The most a word is lengthened by kashidas, in ems.
const KASHIDA_MAX: f32 = 1.0;

/// A word of a paragraph, for [`total_fit`].
struct FitWord {
    /// Its token.
    tok: usize,
    width: f32,
    /// The width of the spaces before it; zero if it continues the word
    /// before it (the line may break between them all the same).
    space: f32,
    /// The width of the hyphen shown if the line breaks after it.
    hyphen: f32,
    /// How much its kashidas can lengthen it.
    kashida: f32,
}

/// Choose where the lines of a paragraph break, by Knuth and Plass's
/// total-fit algorithm: of all the ways to break it, the one with the
/// least total demerits, which grow with the square of each line's
/// badness and the penalty of the break that ends it. A line's badness
/// grows with the cube of how far its spaces must stretch (or shrink) to
/// fill it: [`STRETCH`] and [`SHRINK`] when `justify` (and the kashidas of
/// its Arabic words, which stretch too), or else as if each line had
/// [`RAGGED_STRETCH`]. Hyphens, and very loose lines next to
/// tight ones, cost extra, as in TeX. The last line, and lines before a
/// hard line break, may be as short as they like.
///
/// Returns, for each token, whether a line breaks before it. Each hard
/// line break or image starts over. For each word, only the lines that
/// end with it and fit are tried, so the time is linear in the number of
/// words (times the words on a line).
fn total_fit(toks: &[Tok], max_w: f32, size: f32, justify: bool) -> Vec<bool> {
    let mut out = vec![false; toks.len()];
    let mut words: Vec<FitWord> = Vec::new();
    let mut space = 0.0;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Word(w) => {
                words.push(FitWord {
                    tok: i,
                    width: w.width(),
                    space: if words.is_empty() { 0.0 } else { space },
                    hyphen: w.hyphen.as_ref().map_or(0.0, |h| h.width),
                    kashida: w.kashida.as_ref().map_or(0.0, |k| k.step * k.most as f32),
                });
                space = 0.0;
            }
            Tok::Space(w, _) => space += w,
            Tok::Break | Tok::Image { .. } => {
                fit_lines(&words, max_w, size, justify, &mut out);
                words.clear();
                space = 0.0;
            }
        }
    }
    fit_lines(&words, max_w, size, justify, &mut out);
    out
}

/// [`total_fit`] for the words between hard line breaks.
fn fit_lines(words: &[FitWord], max_w: f32, size: f32, justify: bool, out: &mut [bool]) {
    let n = words.len();
    if n < 2 {
        return;
    }
    // Where each word starts on one endless line, the stretch and shrink
    // of the spaces before it, and the stretch of the kashidas before it.
    let mut pos = Vec::with_capacity(n);
    let mut stretch = Vec::with_capacity(n);
    let mut shrink = Vec::with_capacity(n);
    let mut kashidas = Vec::with_capacity(n + 1);
    let (mut x, mut st, mut sh, mut ka) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for (k, w) in words.iter().enumerate() {
        if k > 0 {
            x += words[k - 1].width + w.space;
            st += w.space * STRETCH;
            sh += w.space * SHRINK;
        }
        pos.push(x);
        stretch.push(st);
        shrink.push(sh);
        kashidas.push(ka);
        ka += w.kashida;
    }
    kashidas.push(ka);
    // Whether the line may break after word k, showing a hyphen.
    let hyphenated = |k: usize| k + 1 < n && words[k + 1].space == 0.0 && words[k].hyphen > 0.0;

    // best[k][c]: the least demerits of the lines up to a break after word
    // k whose last line has fitness class c (tight, decent, loose, very
    // loose), where that line starts, and the class of the line before.
    const NONE: (f64, usize, usize) = (f64::INFINITY, 0, 1);
    let mut best: Vec<[(f64, usize, usize); 4]> = vec![[NONE; 4]; n];
    for b in 0..n {
        let last = b + 1 == n;
        let hyph = hyphenated(b);
        for a in (0..=b).rev() {
            let natural = pos[b] + words[b].width - pos[a] + if hyph { words[b].hyphen } else { 0.0 };
            let (st, sh) = if justify {
                (
                    stretch[b] - stretch[a] + kashidas[b + 1] - kashidas[a],
                    shrink[b] - shrink[a],
                )
            } else {
                (RAGGED_STRETCH * size, 0.0)
            };
            let slack = max_w - natural;
            // The line's badness and fitness class.
            let (badness, class): (f64, usize) = if slack < -sh || (last && slack < 0.0) {
                if a < b {
                    break;
                }
                (OVERFULL, 1)
            } else if last {
                (0.0, 1)
            } else if slack < 0.0 {
                let r = f64::from(-slack / sh);
                (100.0 * r.powi(3), if r > 0.5 { 0 } else { 1 })
            } else {
                // A line without spaces cannot be stretched, but is best
                // nearly full.
                let r = f64::from(slack / if st > 0.0 { st } else { max_w * 0.02 });
                if r > HOPELESS && a < b {
                    // Lines this loose are never the best; skipping them
                    // saves most of the time. A line of one word is always
                    // tried, so there is always a way to break.
                    continue;
                }
                (
                    100.0 * r.powi(3),
                    if r <= 0.5 {
                        1
                    } else if r <= 1.0 {
                        2
                    } else {
                        3
                    },
                )
            };
            let mut line = (LINE_PENALTY + badness).powi(2);
            if hyph {
                line += HYPHEN_PENALTY.powi(2);
            }
            let before = a.checked_sub(1);
            let prev_hyph = before.is_some_and(hyphenated);
            if prev_hyph && hyph {
                line += DOUBLE_HYPHEN;
            }
            if prev_hyph && last {
                line += FINAL_HYPHEN;
            }
            for pc in 0..4 {
                let prev = match before {
                    Some(p) => best[p][pc].0,
                    None if pc == 1 => 0.0,
                    None => f64::INFINITY,
                };
                if prev == f64::INFINITY {
                    continue;
                }
                let mut d = prev + line;
                if justify && class.abs_diff(pc) > 1 {
                    d += ADJACENT_FITNESS;
                }
                if d < best[b][class].0 {
                    best[b][class] = (d, a, pc);
                }
            }
        }
    }
    // Follow the best breaks back from the end.
    let mut c = (0..4)
        .min_by(|&x, &y| best[n - 1][x].0.total_cmp(&best[n - 1][y].0))
        .unwrap_or(1);
    let mut b = n - 1;
    loop {
        let (_, a, pc) = best[b][c];
        if a == 0 {
            break;
        }
        out[words[a].tok] = true;
        b = a - 1;
        c = pc;
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Add `k` (in 1/1000 em) to the advance of the last base glyph of `f`, as
/// kerning with the glyph that follows it.
fn kern_last(f: &mut Frag, k: f32) {
    if k == 0.0 {
        return;
    }
    let Some(b) = f.glyphs.iter().rposition(|g| g.adv != 0.0) else {
        return;
    };
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
                let face = self.fonts().primary(mono, bold, italic);
                // The box drawn for it holds an invisible space, which
                // carries its text (see `draw_glyphs`).
                if let Some(space) = self.fonts().faces[face].glyph(' ') {
                    let entry = &mut self.used.borrow_mut()[face];
                    let entry = entry.entry(space).or_default();
                    if entry.is_empty() {
                        *entry = " ".into();
                        self.map_glyph(face, space, self.intern(" "));
                    }
                }
                items.push((face, 0, c));
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
        let mut texts = Vec::with_capacity(glyphs.len());
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
            let id = self.intern(&text);
            texts.push(id);
            let entry = used[face].entry(g.id).or_default();
            if entry.is_empty() && !text.is_empty() {
                *entry = text;
                self.map_glyph(face, g.id, id);
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
                .zip(&texts)
                .map(|((&id, p), &text)| G {
                    id,
                    adv: p.x_advance as f32 * scale,
                    dx: p.x_offset as f32 * scale,
                    dy: p.y_offset as f32 * scale,
                    text,
                })
                .collect();
            let width = out.iter().map(|g| g.adv).sum::<f32>() * size / 1000.0;
            return (face, out, width);
        }
        let mut out: Vec<G> = glyphs
            .iter()
            .zip(&marks)
            .zip(&texts)
            .map(|((g, &mark), &text)| G {
                id: g.id,
                adv: if mark { 0.0 } else { font.advance_1000(g.id) },
                dx: 0.0,
                dy: 0.0,
                text,
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
    /// Returns the kerning, in 1/1000 em.
    fn kern_join(&self, f: &mut Frag, next: &G) -> f32 {
        if next.adv == 0.0 {
            return 0.0;
        }
        let Some(b) = f.glyphs.iter().rposition(|g| g.adv != 0.0) else {
            return 0.0;
        };
        let k = self.kern_1000(f.face, f.glyphs[b].id, next.id);
        kern_last(f, k);
        k
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

    fn measure(&self, text: &str, mono: bool, bold: bool, italic: bool, size: f32) -> f32 {
        self.shape(text, mono, bold, italic, size)
            .iter()
            .map(|r| r.2)
            .sum()
    }

    /// Draw glyphs, given in visual order, as content of the structure
    /// element `elem` (see [`draw_glyphs`]).
    #[allow(clippy::too_many_arguments)]
    fn show(&mut self, elem: usize, face: FaceId, size: f32, x: f32, y: f32, c: Color, glyphs: &[G]) {
        self.mark(elem);
        let fonts = self.o.fonts.clone();
        let (texts, mapped) = (self.texts.borrow(), self.mapped.borrow());
        let page = self.pages.len() - 1;
        let ops = &mut self.pages[page].ops;
        draw_glyphs(
            ops,
            &texts.0,
            &mapped[face],
            &fonts.faces[face],
            face,
            size,
            (x, y),
            c,
            glyphs,
        );
    }

    /// Draw a line of runs from [`Layout::shape_levels`] from `x` on, as
    /// content of `elem`, in visual order (UAX #9 rule L2), with
    /// right-to-left runs turned around.
    #[allow(clippy::too_many_arguments)]
    fn show_levels(&mut self, elem: usize, runs: &LevelRuns, size: f32, x: f32, y: f32, c: Color) {
        if !self.drawing() {
            return;
        }
        let levels: Vec<u8> = runs.iter().map(|r| r.0).collect();
        let mut x = x;
        for k in bidi::visual_order(&levels) {
            let (level, face, glyphs, w) = &runs[k];
            if level % 2 == 1 {
                self.show(elem, *face, size, x, y, c, &right_to_left(glyphs));
            } else {
                self.show(elem, *face, size, x, y, c, glyphs);
            }
            x += w;
        }
    }

    /// Mark what is drawn next on the current page as content of the
    /// structure element `elem`.
    fn mark(&mut self, elem: usize) {
        let page = self.pages.len() - 1;
        self.tags.mark(&mut self.pages[page].ops, page, elem);
    }

    /// Mark what is drawn next on the current page as an artifact.
    fn mark_artifact(&mut self) {
        let page = self.pages.len() - 1;
        self.tags.artifact(&mut self.pages[page].ops);
    }

    /// End the marked content open on the current page.
    fn unmark(&mut self) {
        let page = self.pages.len() - 1;
        self.tags.close(&mut self.pages[page].ops);
    }

    /// The `Link` element for text linking to `url` in the element
    /// `parent`: that of the text before, if it links there too.
    fn link_elem(&mut self, url: &Rc<str>, parent: usize) -> usize {
        if let Some((u, e, p)) = &self.last_link {
            if u == url && *p == parent {
                return *e;
            }
        }
        let e = self.tags.add(parent, "Link");
        self.last_link = Some((url.clone(), e, parent));
        e
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
        self.unmark();
        self.page_ends.push(self.y);
        self.pages.push(Page::default());
        self.y = self.top();
        self.at_top = true;
    }

    /// Start the second pass, which breaks pages where `plan` says.
    fn restart(&mut self, plan: Vec<bool>) {
        self.pass = Pass::Set(plan);
        self.pages = vec![Page::default()];
        self.page_ends.clear();
        self.y = self.top();
        self.at_top = true;
        self.pending_gap = 0.0;
        self.breakpoints = 0;
        self.keep = 0.0;
        self.next_penalty = None;
        self.blocks_laid = 0;
        self.marker = None;
        self.headings.clear();
        self.anchors.clear();
        self.slug_counts.clear();
        self.tags = Tagger::default();
        self.last_link = None;
    }

    fn gap(&mut self, h: f32) {
        self.pending_gap = self.pending_gap.max(h);
    }

    /// Whether this pass draws: the first one only measures.
    fn drawing(&self) -> bool {
        matches!(self.pass, Pass::Set(_))
    }

    /// The lines of the next block from the first pass, if this is the
    /// second, and its place for [`Self::keep_laidout`]. Blocks are laid out
    /// in the same order in both passes.
    fn laidout(&mut self) -> (usize, Option<Laidout>) {
        let k = self.blocks_laid;
        self.blocks_laid += 1;
        match self.pass {
            Pass::Measure(_) => {
                self.laidout.push(None);
                (k, None)
            }
            Pass::Set(_) => (k, self.laidout.get_mut(k).and_then(Option::take)),
        }
    }

    /// Keep the lines of a block from the first pass for the second.
    fn keep_laidout(&mut self, k: usize, laid: Laidout) {
        if !self.drawing() {
            self.laidout[k] = Some(laid);
        }
    }

    /// Set the penalty of the next break, keeping a higher one already set
    /// (such as [`FORBID`]).
    fn penalize_next(&mut self, p: f32) {
        self.next_penalty = Some(self.next_penalty.map_or(p, |q| q.max(p)));
    }

    /// A place where the page may break, before content that is about to be
    /// drawn: `penalty` adds to that of the blocks it is in. Applies any
    /// pending gap, unless the page breaks here.
    fn may_break(&mut self, penalty: f32) {
        let penalty = self.next_penalty.take().unwrap_or(self.keep) + penalty;
        let k = self.breakpoints;
        self.breakpoints += 1;
        let above = self.y;
        match &mut self.pass {
            Pass::Measure(breakpoints) => {
                if !self.at_top {
                    self.y -= self.pending_gap;
                }
                breakpoints.push(Breakpoint {
                    above,
                    below: self.y,
                    penalty,
                });
            }
            Pass::Set(plan) => {
                if plan.get(k).copied().unwrap_or(false) && !self.at_top {
                    self.new_page();
                }
                if !self.at_top {
                    self.y -= self.pending_gap;
                }
            }
        }
        self.pending_gap = 0.0;
        self.at_top = false;
    }

    /// Fill a rectangle, as an artifact (a background).
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Color) {
        if !self.drawing() {
            return;
        }
        self.mark_artifact();
        let ops = &mut self.page().ops;
        nums(ops, &[c.0, c.1, c.2]);
        ops.extend_from_slice(b"rg ");
        nums(ops, &[x, y, w, h]);
        ops.extend_from_slice(b"re f\n");
    }

    /// Stroke a line, as an artifact (a rule).
    fn stroke_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        if !self.drawing() {
            return;
        }
        self.mark_artifact();
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
    fn tokenize(
        &self,
        inlines: &[Inline],
        size: f32,
        color: Color,
        force_bold: bool,
        hyphenate: bool,
        justify: bool,
    ) -> (Vec<Tok>, bool) {
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
        let mut breaks = linebreak::opportunities(&text);
        let (levels, rtl) = if bidi::needs_resolving(&text) {
            let l = bidi::resolve(&text, None);
            (l.levels.clone(), l.rtl())
        } else {
            (vec![0; text.len()], false)
        };
        // Hyphenation points: break opportunities that show a hyphen.
        let mut hyphens = vec![false; text.len()];
        if let Some(p) = self.hyph.filter(|_| hyphenate) {
            self.hyphenate(p, &text, &slots, &styles, &levels, &mut breaks, &mut hyphens);
        }

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
                        if pending.is_none() && i > 0 && (text[i - 1] == '\u{AD}' || hyphens[i]) {
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
                    let runs = self.shape_joined(&seg, mono, bold, italic, st.size, level % 2 == 1, joins);
                    // The frag the segment went into and where, if it is
                    // set in one run.
                    let one = runs.iter().filter(|r| !r.1.is_empty()).count() == 1;
                    let mut into: Option<(FaceId, usize, usize)> = None;
                    for (face, glyphs, w) in runs {
                        if glyphs.is_empty() {
                            continue;
                        }
                        if word.frags.is_empty() && joined {
                            // Kerning with the end of the previous word, for
                            // when both end up on one line.
                            if let Some(Tok::Word(prev)) = toks.last_mut() {
                                if let Some(f) = prev.frags.last_mut().filter(|f| f.same_run(face, st, level))
                                {
                                    prev.join_kern = self.kern_join(f, &glyphs[0]);
                                }
                            }
                        }
                        joined = false;
                        let n = word.frags.len();
                        match word.frags.last_mut() {
                            Some(f) if f.same_run(face, st, level) => {
                                self.kern_join(f, &glyphs[0]);
                                into = Some((face, n - 1, f.glyphs.len()));
                                f.glyphs.extend_from_slice(&glyphs);
                                f.width += w;
                            }
                            _ => {
                                into = Some((face, n, 0));
                                word.frags.push(st.frag(glyphs, w, face, level));
                            }
                        }
                    }
                    if let Some((face, k, start)) = into.filter(|_| justify && one) {
                        let shaped = &word.frags[k].glyphs;
                        let range = start..shaped.len();
                        let found = self.kashida(
                            &text[i..j],
                            st,
                            level % 2 == 1,
                            joins,
                            face,
                            k,
                            range,
                            &shaped[start..],
                        );
                        if let Some(found) = found {
                            if word.kashida.as_ref().is_none_or(|w| found.priority <= w.priority) {
                                word.kashida = Some(found);
                            }
                        }
                    }
                    i = j;
                }
            }
        }
        end_word(&mut word, &mut toks);
        (toks, rtl)
    }

    /// Find where the words of a paragraph may be hyphenated, and make
    /// those places break opportunities (`breaks`) that show a hyphen
    /// (`hyphens`).
    ///
    /// A word is text between spaces (or the ends of the paragraph) with no
    /// break opportunity in it, in one style, though punctuation around it
    /// may be in another: so not a word joined to another by a hyphen or
    /// slash, or part of a URL. Punctuation around it is left aside, and
    /// what remains must be letters the patterns know, with no capitals
    /// after the first (so not acronyms or camel case), and not code. A place where splitting the word would change its
    /// glyphs, such as in the "ffi" ligature of "office", is left out, as
    /// the word would then look different where it is not broken.
    #[allow(clippy::too_many_arguments)]
    fn hyphenate(
        &self,
        p: &Patterns,
        text: &[char],
        slots: &[Slot],
        styles: &[TextStyle],
        levels: &[u8],
        breaks: &mut [Break],
        hyphens: &mut [bool],
    ) {
        let n = text.len();
        let is_space = |k: usize| matches!(slots[k], Slot::Space(_) | Slot::Break | Slot::Image(_));
        let mut lowered = String::new();
        let mut i = 0;
        while i < n {
            let Slot::Text(k) = slots[i] else {
                i += 1;
                continue;
            };
            let mut j = i + 1;
            while j < n && slots[j] == Slot::Text(k) && breaks[j] == Break::No {
                j += 1;
            }
            let (start, end) = (i, j);
            i = j;
            // Only whole words: between spaces, or punctuation in another
            // style that the line may not break before or after.
            let open =
                |k: usize, c: usize| is_space(c) || (breaks[k] == Break::No && !text[c].is_alphanumeric());
            if (start > 0 && !open(start, start - 1)) || (end < n && !open(end, end)) {
                continue;
            }
            let st = &styles[k];
            if st.style.code || levels[start..end].iter().any(|&l| l != levels[start]) {
                continue;
            }
            let word = &text[start..end];
            let Some(a) = word.iter().position(|c| c.is_alphabetic()) else {
                continue;
            };
            let b = word
                .iter()
                .rposition(|c| c.is_alphabetic())
                .expect("a letter exists")
                + 1;
            let core = &word[a..b];
            if core.len() < p.left + p.right
                || core[1..].iter().any(|c| c.is_uppercase())
                || !core.iter().all(|&c| p.knows(lower(c)))
            {
                continue;
            }
            lowered.clear();
            lowered.extend(core.iter().map(|&c| lower(c)));
            let cached = self.hyph_cache.borrow().get(lowered.as_str()).cloned();
            let points = cached.unwrap_or_else(|| {
                let chars: Vec<char> = lowered.chars().collect();
                let points: Rc<[usize]> = p.points(&chars).into();
                let mut cache = self.hyph_cache.borrow_mut();
                if cache.len() >= SHAPE_CACHE_SIZE {
                    cache.clear();
                }
                cache.insert(lowered.clone(), points.clone());
                points
            });
            for &q in points.iter() {
                let q = a + q;
                if !self.splits(word[q - 1], word[q], st) {
                    continue;
                }
                breaks[start + q] = Break::Allowed;
                hyphens[start + q] = true;
            }
        }
    }

    /// Whether a word in style `st` may be split between the letters `x`
    /// and `y` without changing its glyphs: whether they are set the same
    /// together (as in a ligature such as "fi") as apart.
    fn splits(&self, x: char, y: char, st: &TextStyle) -> bool {
        let (bold, italic) = (st.style.bold, st.style.italic);
        let key = (x, y, (bold as u8) << 1 | italic as u8);
        if let Some(&ok) = self.split_cache.borrow().get(&key) {
            return ok;
        }
        let ids = |t: &str| -> Vec<u16> {
            self.shape(t, false, bold, italic, st.size)
                .into_iter()
                .flat_map(|(_, g, _)| g.into_iter().map(|g| g.id))
                .collect()
        };
        let mut apart = ids(x.encode_utf8(&mut [0; 4]));
        apart.extend(ids(y.encode_utf8(&mut [0; 4])));
        let ok = ids(&format!("{x}{y}")) == apart;
        self.split_cache.borrow_mut().insert(key, ok);
        ok
    }

    /// The hyphen shown where a line breaks at a soft hyphen.
    fn hyphen(&self, st: &TextStyle, level: u8) -> Option<Frag> {
        let (mono, bold, italic) = (st.style.code, st.style.bold, st.style.italic);
        let runs = self.shape("-", mono, bold, italic, st.size);
        let (face, mut glyphs, w) = runs.into_iter().next()?;
        // It stands for the soft hyphen of the text (or the hyphenation
        // point), as a hyphen shown only where a line breaks.
        let soft = self.intern("\u{AD}");
        for g in &mut glyphs {
            g.text = soft;
        }
        Some(st.frag(glyphs, w, face, level))
    }

    /// Fill a line to `max_w`: lengthen the words that have kashidas
    /// (`kashidas`: their frags in the line, and where) by as many tatweels
    /// as fit, the best places first and each by one before any by two;
    /// then widen (or narrow) the spaces between the words, `gaps`, by what
    /// is left, unless that would widen them by more than [`MAX_GROWTH`].
    fn justify(&self, line: &mut Line, gaps: &[usize], kashidas: &[(usize, &Kashida)], max_w: f32) {
        let spaces: f32 = gaps.iter().map(|&k| line.frags[k].1.width).sum();
        let slack = max_w - line.width;
        let mut counts = vec![0; kashidas.len()];
        let mut left = slack;
        let mut order: Vec<usize> = (0..kashidas.len()).collect();
        order.sort_by_key(|&k| kashidas[k].1.priority);
        // Round by round, until a round runs out of room: so no word gets
        // two more than another (their steps differ).
        let mut more = slack > 0.0;
        while more {
            let mut any = false;
            for &k in &order {
                let kd = kashidas[k].1;
                if counts[k] == kd.most {
                    continue;
                }
                if kd.step > left {
                    more = false;
                    continue;
                }
                counts[k] += 1;
                left -= kd.step;
                any = true;
            }
            more &= any;
        }
        if counts.iter().all(|&n| n == 0) && (spaces <= 0.0 || left / spaces > MAX_GROWTH) {
            return;
        }
        for (&(k, kd), &n) in kashidas.iter().zip(&counts) {
            if n > 0 {
                self.lengthen(&mut line.frags[k].1, kd, n);
            }
        }
        let place = |line: &mut Line| {
            let mut x = line.frags.first().map_or(0.0, |f| f.0);
            for (fx, f) in &mut line.frags {
                *fx = x;
                x += f.width;
            }
            line.width = x;
        };
        place(line);
        // The spaces fill the rest, unless that is too much for them: then
        // the line keeps its kashidas and is left short.
        if spaces > 0.0 && (max_w - line.width) / spaces <= MAX_GROWTH {
            let scale = 1.0 + (max_w - line.width) / spaces;
            for &k in gaps {
                line.frags[k].1.width *= scale;
            }
            place(line);
        }
    }

    /// Where a segment of an Arabic word, `text` in style `st` shaped as
    /// `shaped` (glyphs `glyphs` of frag `frag` in face `face`), may be
    /// lengthened by kashidas (see [`arabic::kashida`]).
    /// Not where a ligature would be broken, nor if the font has no tatweel
    /// (or it does not make the word wider).
    #[allow(clippy::too_many_arguments)]
    fn kashida(
        &self,
        text: &[char],
        st: &TextStyle,
        rtl: bool,
        joins: Joins,
        face: FaceId,
        frag: usize,
        glyphs: std::ops::Range<usize>,
        shaped: &[G],
    ) -> Option<Kashida> {
        if st.style.code || !arabic::needs_joining(text) {
            return None;
        }
        let forms = arabic::forms(text, joins.before, joins.after);
        let (at, priority) = arabic::kashida(text, &forms)?;
        // Whether a glyph stands for letters on both sides: count the
        // letters (which normalization keeps) each glyph stands for.
        let letter = |c: &char| {
            use crate::bidi_table::JoiningType::{D, L, R};
            matches!(arabic::joining_type(*c), D | L | R)
        };
        let before = text[..at].iter().filter(|c| letter(c)).count();
        let mut seen = 0;
        {
            let texts = self.texts.borrow();
            for g in shaped {
                let n = texts.0[g.text as usize].chars().filter(letter).count();
                if seen < before && before < seen + n {
                    return None;
                }
                seen += n;
            }
        }
        let (bold, italic) = (st.style.bold, st.style.italic);
        let mut kashida = Kashida {
            frag,
            glyphs,
            text: text.to_vec(),
            at,
            bold,
            italic,
            rtl,
            joins,
            priority,
            adv: shaped.iter().map(|g| g.adv).sum(),
            step: 0.0,
            most: 0,
        };
        let (_, lengthened) = self.shape_kashida(&kashida, st.size, 1).filter(|r| r.0 == face)?;
        let adv: f32 = lengthened.iter().map(|g| g.adv).sum();
        kashida.step = (adv - kashida.adv) * st.size / 1000.0;
        if kashida.step < 0.01 * st.size {
            return None;
        }
        kashida.most = (KASHIDA_MAX * st.size / kashida.step) as usize;
        (kashida.most > 0).then_some(kashida)
    }

    /// Shape the segment of `k` with `n` tatweels, if it is set in one run.
    ///
    /// The tatweels are text, as in PDFs from Word and LibreOffice: text
    /// read from the PDF has them. (Standing for no text, in an
    /// `ActualText` span, they would leave a gap that MuPDF reads as a
    /// space, splitting the word.)
    fn shape_kashida(&self, k: &Kashida, size: f32, n: usize) -> Option<(FaceId, Vec<G>)> {
        let mut text: String = k.text[..k.at].iter().collect();
        text.extend(std::iter::repeat_n(arabic::TATWEEL, n));
        text.extend(&k.text[k.at..]);
        let mut runs = self.shape_joined(&text, false, k.bold, k.italic, size, k.rtl, k.joins);
        if runs.len() != 1 {
            return None;
        }
        let (face, glyphs, _) = runs.pop()?;
        Some((face, glyphs))
    }

    /// Lengthen the word in `f` by `n` tatweels at its kashida `k`.
    fn lengthen(&self, f: &mut Frag, k: &Kashida, n: usize) {
        let Some((face, glyphs)) = self.shape_kashida(k, f.size, n) else {
            return;
        };
        if face != f.face {
            return;
        }
        let old: f32 = f.glyphs[k.glyphs.clone()].iter().map(|g| g.adv).sum();
        let mut piece = f.piece(0..0, 0.0);
        piece.glyphs = glyphs;
        // Keep the kerning with the text after it.
        kern_last(&mut piece, old - k.adv);
        let new: f32 = piece.glyphs.iter().map(|g| g.adv).sum();
        f.width += (new - old) * f.size / 1000.0;
        f.glyphs.splice(k.glyphs.clone(), piece.glyphs);
    }

    /// Break a paragraph into lines of at most `max_w`, and justify them
    /// if `fill` says so.
    fn wrap(&self, toks: &[Tok], max_w: f32, base_size: f32, fill: Fill) -> Vec<Laid> {
        let max_w = max_w.max(1.0);
        // Where the lines break: before the words marked here, or, filling
        // greedily, before each word that does not fit.
        let chosen = match fill {
            Fill::Greedy => None,
            Fill::Ragged => Some(total_fit(toks, max_w, base_size, false)),
            Fill::Justify => Some(total_fit(toks, max_w, base_size, true)),
        };
        // The spaces between words on the current line (frag indices),
        // and the kashidas of its words (the same, and where).
        let mut gaps: Vec<usize> = Vec::new();
        let mut kashidas: Vec<(usize, &Kashida)> = Vec::new();
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
                    gaps.clear();
                    kashidas.clear();
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
                    gaps.clear();
                    kashidas.clear();
                    space = 0.0;
                }
                Tok::Word(word) => {
                    let w = word.width();
                    let fits = match &chosen {
                        Some(breaks) => !breaks[i],
                        None => line.width + space + w <= max_w,
                    };
                    if !line.frags.is_empty() && !fits {
                        // Break before this word. If the last word on the line
                        // needs a hyphen that does not fit, it moves to the
                        // next line too.
                        let mut restart = i;
                        while chosen.is_none() && placed.len() > 1 {
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
                            if let Tok::Word(Word {
                                hyphen: Some(h),
                                join_kern,
                                ..
                            }) = &toks[ti]
                            {
                                if let Some((_, last)) = line.frags.last_mut() {
                                    // Kern with the hyphen, not the text after the break.
                                    let before = last.width;
                                    kern_last(last, -join_kern);
                                    if last.joins(h) {
                                        self.kern_join(last, &h.glyphs[0]);
                                    }
                                    line.width += last.width - before;
                                }
                                let x = line.width;
                                push_frag(&mut line, x, h.clone());
                            }
                        }
                        if fill == Fill::Justify {
                            self.justify(&mut line, &gaps, &kashidas, max_w);
                        }
                        out.push(Laid::Line(std::mem::replace(&mut line, new_line())));
                        placed.clear();
                        gaps.clear();
                        kashidas.clear();
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
                                    gaps.clear();
                                    kashidas.clear();
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
                            gaps.push(line.frags.len());
                            push_frag(&mut line, x, gap);
                            x += space;
                        }
                        if let Some(k) = word.kashida.as_ref().filter(|_| fill == Fill::Justify) {
                            kashidas.push((line.frags.len() + k.frag, k));
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
        if !self.drawing() {
            return;
        }
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
        // Where each frag goes: placed left to right in visual order (UAX
        // #9 rule L2), from the first one's place.
        let levels: Vec<u8> = frags.iter().map(|(_, f)| f.level).collect();
        let order = bidi::visual_order(&levels);
        let mut xs: Vec<f32> = frags.iter().map(|(fx, _)| *fx).collect();
        if levels.iter().any(|&l| l > 0) {
            let mut at = frags.first().map_or(0.0, |f| f.0);
            for &k in &order {
                xs[k] = at;
                at += frags[k].1.width;
            }
        }
        for &k in &order {
            let f = &frags[k].1;
            if f.code {
                self.fill_rect(
                    x + xs[k] - 1.5,
                    baseline - f.size * 0.28,
                    f.width + 3.0,
                    f.size * 1.2,
                    CODE_BG,
                );
            }
        }
        // Linked text goes to a `Link` element, found in logical order.
        // The text is drawn in visual order, which is what readers expect
        // (they reorder right-to-left text themselves), and placed in its
        // elements in logical order.
        self.tags.next_place();
        let parent = self.tags.current();
        let mut elems = vec![parent; frags.len()];
        for (k, (_, f)) in frags.iter().enumerate() {
            self.tags.place(k as u32);
            elems[k] = match &f.link {
                Some(url) => self.link_elem(url, parent),
                None => {
                    if !f.glyphs.is_empty() {
                        self.last_link = None;
                    }
                    parent
                }
            };
        }
        for &k in &order {
            let f = &frags[k].1;
            if !f.glyphs.is_empty() {
                self.tags.place(k as u32);
                let fx = x + xs[k];
                if f.level % 2 == 1 {
                    let turned = right_to_left(&f.glyphs);
                    self.show(elems[k], f.face, f.size, fx, baseline, f.color, &turned);
                } else {
                    self.show(elems[k], f.face, f.size, fx, baseline, f.color, &f.glyphs);
                }
            }
        }
        for &k in &order {
            self.tags.place(k as u32);
            let (f, fx) = (&frags[k].1, x + xs[k]);
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
                let elem = elems[k];
                let page_index = self.pages.len() - 1;
                let page = self.page();
                // Merge with the previous area when it continues the same link.
                if let Some(prev) = page.links.last_mut() {
                    let same = match (&prev.target, &target) {
                        (Target::Uri(a), Target::Uri(b)) | (Target::Anchor(a), Target::Anchor(b)) => a == b,
                        _ => false,
                    };
                    if same
                        && prev.elem == elem
                        && (prev.rect[1] - rect[1]).abs() < 0.01
                        && rect[0] - prev.rect[2] < f.size
                    {
                        prev.rect[2] = rect[2];
                        continue;
                    }
                }
                page.links.push(LinkArea { rect, target, elem });
                let index = page.links.len() - 1;
                self.tags.add_link(elem, page_index, index);
            }
        }
        self.tags.next_place();
    }

    fn draw_marker(&mut self, baseline: f32) {
        let Some(m) = self.marker.take() else { return };
        if !self.drawing() {
            return;
        }
        match m.kind {
            MarkerKind::Text(t) => {
                let runs = self.shape_levels(&t, false, false, false, m.size, Some(m.rtl));
                let w: f32 = runs.iter().map(|r| r.3).sum();
                let x = if m.rtl { m.edge } else { m.edge - w };
                self.show_levels(m.lbl, &runs, m.size, x, baseline, m.color);
            }
            MarkerKind::Check(checked) => {
                let s = m.size * 0.72;
                let x = if m.rtl { m.edge } else { m.edge - s };
                let y = baseline - m.size * 0.05;
                self.mark(m.lbl);
                let ops = &mut self.page().ops;
                tags::begin_actual_text(ops, if checked { "\u{2611}" } else { "\u{2610}" });
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
                self.page().ops.extend_from_slice(b"EMC\n");
            }
        }
    }

    /// Lay out a run of inline content as wrapped lines.
    ///
    /// Paragraphs (`body`) are justified, or else broken as ragged lines
    /// by total fit, and hyphenated if the language is known; other text,
    /// such as headings, is ragged and broken greedily.
    fn text_block(&mut self, inlines: &[Inline], size: f32, ctx: Ctx, bold: bool, body: bool) {
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
        let (slot, laidout) = self.laidout();
        let (laid, rtl) = match laidout {
            Some(Laidout::Text(laid, rtl)) => (laid, rtl),
            _ => {
                let justify = body && self.o.justify;
                let (toks, rtl) = self.tokenize(inlines, size, ctx.color, bold, body, justify);
                let fill = match (body, self.o.justify) {
                    (false, _) => Fill::Greedy,
                    (true, false) => Fill::Ragged,
                    (true, true) => Fill::Justify,
                };
                (self.wrap(&toks, ctx.w, size, fill), rtl)
            }
        };
        // Avoid leaving the first or last line of a paragraph alone. Only
        // text lines count, not images between them.
        let n = laid.iter().filter(|i| matches!(i, Laid::Line(_))).count();
        let mut k = 0;
        for item in &laid {
            let mut penalty = 0.0;
            if let Laid::Line(_) = item {
                if k == 1 {
                    penalty += CLUB;
                }
                if k > 0 && k + 1 == n {
                    penalty += WIDOW;
                }
                k += 1;
            }
            match item {
                Laid::Line(line) => {
                    let lh = self.line_height(line);
                    self.may_break(penalty);
                    // Right-to-left paragraphs are set flush right.
                    let x = if rtl {
                        ctx.x + (ctx.w - line.width).max(0.0)
                    } else {
                        ctx.x
                    };
                    self.draw_line(line, x, lh);
                    self.y -= lh;
                }
                Laid::Image { alt, src } => self.image(alt, src, ctx, penalty),
            }
        }
        self.keep_laidout(slot, Laidout::Text(laid, rtl));
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

    fn image(&mut self, alt: &str, src: &str, ctx: Ctx, penalty: f32) {
        let Some(idx) = self.load_image(src) else {
            self.penalize_next(self.keep + penalty);
            return self.text_block(&[placeholder(alt, src)], self.o.font_size, ctx, false, false);
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
        self.may_break(penalty);
        self.draw_marker(self.y - self.o.font_size);
        let y = self.y - h - 2.0;
        if self.drawing() {
            // An image without a description is decoration.
            let alt = alt.trim();
            if alt.is_empty() {
                self.mark_artifact();
            } else {
                let figure = self.tags.add(self.tags.current(), "Figure");
                self.tags.elems[figure].alt = Some(alt.to_string());
                self.mark(figure);
            }
            let ops = &mut self.page().ops;
            ops.extend_from_slice(b"q ");
            nums(ops, &[w, 0.0, 0.0, h, ctx.x, y]);
            let _ = writeln!(ops, "cm /Im{idx} Do Q");
        }
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
                    self.tags.begin("P");
                    self.text_block(&inl, fs, ctx, false, true);
                    self.tags.end();
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
                    self.may_break(0.0);
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
            self.may_break(0.0);
            self.draw_marker(self.y - lh / 2.0 - fs * 0.26);
            self.y -= lh;
        }
    }

    fn heading(&mut self, level: u8, raw: &str, ctx: Ctx) {
        let fs = self.o.font_size;
        let scale = [2.0, 1.6, 1.3, 1.15, 1.0, 0.9][(level.clamp(1, 6) - 1) as usize];
        let size = fs * scale;
        self.gap(if level <= 2 { fs * 1.3 } else { fs * 1.0 });
        if let Some(bonus) = SECTION.get(level.max(1) as usize - 1) {
            self.penalize_next(self.keep + bonus);
        }
        self.may_break(0.0);

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
        // Keep the heading's lines together, and with the text after it.
        let keep = std::mem::replace(&mut self.keep, FORBID);
        self.next_penalty = Some(FORBID);
        self.tags
            .begin(["H1", "H2", "H3", "H4", "H5", "H6"][(level.clamp(1, 6) - 1) as usize]);
        self.text_block(&inl, size, Ctx { color, ..ctx }, true, false);
        self.tags.end();
        self.keep = keep;
        self.next_penalty = Some(FORBID);
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

        self.may_break(0.0);
        let code = self.tags.begin("Code");
        let top_y = self.y;
        self.fill_rect(ctx.x, top_y - pad, ctx.w, pad, CODE_BG);
        self.draw_marker(top_y - pad - lh / 2.0 - size * 0.26);
        self.y -= pad;
        let (slot, laidout) = self.laidout();
        let mut chunks = match laidout {
            Some(Laidout::Code(chunks)) => chunks,
            _ => self.code_lines(lines, size, avail),
        };
        // Breaks near either end would leave a few lines alone; the first
        // line stays with the top padding.
        let n = chunks.len();
        let keep = self.keep;
        self.keep += IN_CODE;
        for (k, chunk) in chunks.iter_mut().enumerate() {
            let penalty = if k == 0 {
                FORBID
            } else if k < CODE_EDGE_LINES || n - k < CODE_EDGE_LINES {
                CODE_EDGE
            } else {
                0.0
            };
            self.may_break(penalty);
            if self.drawing() {
                self.fill_rect(ctx.x, self.y - lh, ctx.w, lh + 0.3, CODE_BG);
                let baseline = self.y - lh / 2.0 - size * 0.26;
                let runs = std::mem::take(chunk);
                self.show_levels(code, &runs, size, ctx.x + pad, baseline, ctx.color);
            }
            self.y -= lh;
        }
        self.keep = keep;
        self.keep_laidout(slot, Laidout::Code(chunks));
        self.fill_rect(ctx.x, self.y - pad, ctx.w, pad + 0.3, CODE_BG);
        self.y -= pad;
        self.tags.end();
    }

    /// Shape the lines of a code block, and wrap long lines at the glyph
    /// that would overflow the box.
    fn code_lines(&self, lines: &[String], size: f32, avail: f32) -> Vec<LevelRuns> {
        let empty: [String; 1] = [String::new()];
        let lines = if lines.is_empty() { &empty[..] } else { lines };
        let mut chunks: Vec<LevelRuns> = Vec::with_capacity(lines.len());
        for line in lines {
            // Lines of code are left-to-right paragraphs; right-to-left text
            // in them is reordered.
            chunks.push(Vec::new());
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
        }
        chunks
    }

    fn quote(&mut self, inner: &[Block], ctx: Ctx) {
        let fs = self.o.font_size;
        let indent = fs * 1.2;
        self.may_break(0.0);
        let start = (self.pages.len() - 1, self.y);
        // Right-to-left quotes have their bar on the right.
        let rtl = blocks_rtl(inner);
        let child = Ctx {
            x: if rtl { ctx.x } else { ctx.x + indent },
            w: (ctx.w - indent).max(fs),
            color: MUTED,
            ..ctx
        };
        let keep = self.keep;
        self.keep += IN_QUOTE;
        self.next_penalty = Some(FORBID);
        self.tags.begin("BlockQuote");
        self.blocks(inner, child);
        self.tags.end();
        self.keep = keep;
        self.pending_gap = 0.0;
        let end = (self.pages.len() - 1, self.y);
        let bar_x = if rtl {
            ctx.x + ctx.w - fs * 0.3 - 2.5
        } else {
            ctx.x + fs * 0.3
        };
        for p in start.0..=end.0 {
            if !self.drawing() {
                break;
            }
            let top = if p == start.0 { start.1 } else { self.top() };
            let bot = if p == end.0 { end.1 } else { self.page_ends[p] };
            if top - bot > 0.5 {
                // The bar is an artifact. Earlier pages are finished, with
                // nothing left open.
                let last = p + 1 == self.pages.len();
                if last {
                    self.mark_artifact();
                }
                let ops = &mut self.pages[p].ops;
                if !last {
                    ops.extend_from_slice(b"/Artifact BMC\n");
                }
                nums(ops, &[RULE.0, RULE.1, RULE.2]);
                ops.extend_from_slice(b"rg ");
                nums(ops, &[bar_x, bot, 2.5, top - bot]);
                ops.extend_from_slice(b"re f\n");
                if !last {
                    ops.extend_from_slice(b"EMC\n");
                }
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
        let list = self.tags.begin("L");
        self.tags.elems[list].attrs = format!(
            "/O /List /ListNumbering /{}",
            match (ordered, items.iter().any(|i| i.task.is_some())) {
                (true, _) => "Decimal",
                (false, true) => "None",
                (false, false) => "Disc",
            }
        );
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
            self.tags.begin("LI");
            let lbl = self.tags.add(self.tags.current(), "Lbl");
            self.marker = Some(Marker {
                kind,
                lbl,
                edge: if rtl {
                    ctx.x + ctx.w - indent + fs * 0.45
                } else {
                    ctx.x + indent - fs * 0.45
                },
                rtl,
                size: fs,
                color: ctx.color,
            });
            let keep = self.keep;
            if k > 0 {
                self.penalize_next(keep + BETWEEN_ITEMS);
            }
            self.keep += IN_ITEM;
            self.tags.begin("LBody");
            self.blocks(&item.blocks, child);
            self.tags.end();
            self.tags.end();
            self.keep = keep;
            if !tight {
                self.gap(fs * 0.75);
            }
        }
        self.tags.end();
    }

    fn table(&mut self, aligns: &[Align], header: &[String], rows: &[Vec<String>], ctx: Ctx) {
        let cols = aligns.len();
        if cols == 0 {
            return;
        }
        let fs = self.o.font_size;
        let size = fs * 0.92;
        let pad = fs * 0.45;
        let (slot, laidout) = self.laidout();
        let (widths, table_w, grid) = match laidout {
            Some(Laidout::Table(widths, table_w, grid)) => (widths, table_w, grid),
            _ => self.table_cells(cols, header, rows, size, pad, ctx),
        };

        self.may_break(0.0);
        self.draw_marker(self.y - fs);
        self.stroke_line(ctx.x, self.y, ctx.x + table_w, self.y, 0.8, RULE);
        // The first row is the header, whose cells head their columns.
        let table = self.tags.begin("Table");
        let cell_elems: Vec<Vec<usize>> = grid
            .iter()
            .enumerate()
            .map(|(r, cells)| {
                let tr = self.tags.add(table, "TR");
                (0..cells.len())
                    .map(|_| {
                        let cell = self.tags.add(tr, if r == 0 { "TH" } else { "TD" });
                        if r == 0 {
                            self.tags.elems[cell].attrs = "/O /Table /Scope /Column".into();
                        }
                        cell
                    })
                    .collect()
            })
            .collect();
        let text_h = self.top() - self.bottom();
        for (r, cells) in grid.iter().enumerate() {
            let nlines = cells.iter().map(|c| c.0.len()).max().unwrap_or(0).max(1);
            let heights: Vec<f32> = (0..nlines)
                .map(|k| {
                    cells
                        .iter()
                        .filter_map(|c| c.0.get(k))
                        .map(|l| self.line_height(l))
                        .fold(size * LINE_SPACING, f32::max)
                })
                .collect();
            let in_row = if 2.0 * pad + heights.iter().sum::<f32>() > TALL_ROW * text_h {
                IN_TALL_ROW
            } else {
                IN_ROW
            };
            let bg = r == 0;
            // The header stays with the first row.
            self.may_break(if r < 2 { FORBID } else { TABLE_ROW });
            if bg {
                self.fill_rect(ctx.x, self.y - pad, table_w, pad, CODE_BG);
            }
            self.y -= pad;
            for (k, &lh) in heights.iter().enumerate() {
                self.may_break(if k == 0 { FORBID } else { in_row });
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
                        self.tags.enter(cell_elems[r][c]);
                        self.draw_line(line, x + pad + off, lh);
                        self.tags.end();
                    }
                    x += widths[c] + 2.0 * pad;
                }
                self.y -= lh;
            }
            if bg {
                self.fill_rect(ctx.x, self.y - pad, table_w, pad + 0.3, CODE_BG);
            }
            self.y -= pad;
            let (lw, color) = if bg { (0.8, MUTED) } else { (0.5, RULE) };
            self.stroke_line(ctx.x, self.y, ctx.x + table_w, self.y, lw, color);
        }
        self.tags.end();
        self.keep_laidout(slot, Laidout::Table(widths, table_w, grid));
    }

    /// Lay out the cells of a table: the column widths, the table's width,
    /// and the lines of each cell with its direction, row by row.
    #[allow(clippy::type_complexity)]
    fn table_cells(
        &self,
        cols: usize,
        header: &[String],
        rows: &[Vec<String>],
        size: f32,
        pad: f32,
        ctx: Ctx,
    ) -> (Vec<f32>, f32, Vec<Vec<(Vec<Line>, bool)>>) {
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
            l.tokenize(&inl, size, ctx.color, bold, false, false)
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
        let widths = fit_columns(&nat, &min, ctx.w - 2.0 * pad * cols as f32, size);
        let table_w: f32 = widths.iter().map(|w| w + 2.0 * pad).sum();
        let grid = grid
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .enumerate()
                    .map(|(c, (toks, rtl))| {
                        let lines = self
                            .wrap(&toks, widths[c], size, Fill::Greedy)
                            .into_iter()
                            .filter_map(|l| if let Laid::Line(l) = l { Some(l) } else { None })
                            .collect();
                        (lines, rtl)
                    })
                    .collect()
            })
            .collect();
        (widths, table_w, grid)
    }

    /// What to show of the front matter, if there is any: its table, if
    /// it is wanted and can be shown. Warns otherwise, except when it was
    /// turned off.
    fn front_matter(&mut self, src: Option<&str>, ctx: Ctx) -> Option<Table> {
        let src = src?;
        if src.trim().is_empty() {
            return None;
        }
        match self.o.front_matter {
            Some(false) => return None,
            None => {
                self.warnings.push(
                    "the YAML front matter is left out; use --front-matter to show it as a table, \
                     or --no-front-matter to leave it out without this warning"
                        .into(),
                );
                return None;
            }
            Some(true) => {}
        }
        // It starts on the second line of the file.
        let node = match yaml::parse(src, 2) {
            Ok(node) => node,
            Err(e) => {
                self.warnings.push(format!(
                    "the YAML front matter is left out, as it could not be read: {e}"
                ));
                return None;
            }
        };
        let cols = front_matter::columns(&node);
        let fit = ((ctx.w / (front_matter::MIN_COLUMN_EM * self.o.font_size)) as usize).max(2);
        if cols > fit {
            self.warnings.push(format!(
                "the YAML front matter is left out, as it is nested too deeply to fit the page: \
                 it needs {cols} columns and {fit} fit"
            ));
            return None;
        }
        Some(front_matter::table(&node))
    }

    /// Draw a table whose cells may span rows and columns, as front matter
    /// is shown. A cell's lines fill the rows it spans from the top; the
    /// last of them grows if they do not fit. Rules between rows start at
    /// the first column not spanned across them, and the page may break
    /// between any two lines, preferably between rows that no key spans.
    fn span_table(&mut self, t: &Table, ctx: Ctx) {
        if t.cells.is_empty() {
            return;
        }
        let fs = self.o.font_size;
        let size = fs * 0.92;
        let pad = fs * 0.45;
        let toks: Vec<(Vec<Tok>, bool)> = t
            .cells
            .iter()
            .map(|c| {
                let mut inl = Vec::new();
                for (k, line) in c.text.split('\n').enumerate() {
                    if k > 0 {
                        inl.push(Inline::Break);
                    }
                    inl.push(Inline::Text {
                        text: line.to_string(),
                        style: Style::default(),
                        link: None,
                    });
                }
                self.tokenize(&inl, size, ctx.color, c.key, false, false)
            })
            .collect();

        // Natural and minimum widths as for tables, from the cells of one
        // column first; a wider cell spanning columns widens the last.
        let mut nat = vec![0.0f32; t.cols];
        let mut min = vec![0.0f32; t.cols];
        for spanning in [false, true] {
            for (c, (toks, _)) in t.cells.iter().zip(&toks) {
                if (c.cols > 1) != spanning {
                    continue;
                }
                let (mut line, mut widest, mut longest) = (0.0f32, 0.0f32, 0.0f32);
                for tok in toks {
                    match tok {
                        Tok::Word(word) => {
                            line += word.width();
                            longest = longest.max(word.width().min(ctx.w / t.cols as f32));
                        }
                        Tok::Space(w, _) => line += w,
                        Tok::Break => widest = widest.max(std::mem::take(&mut line)),
                        _ => {}
                    }
                }
                widest = widest.max(line);
                let last = c.col + c.cols - 1;
                let inner = 2.0 * pad * (c.cols - 1) as f32;
                let have_nat = nat[c.col..=last].iter().sum::<f32>() + inner;
                let have_min = min[c.col..=last].iter().sum::<f32>() + inner;
                nat[last] += (widest - have_nat).max(0.0);
                min[last] += (longest - have_min).max(0.0);
            }
        }
        let widths = fit_columns(&nat, &min, ctx.w - 2.0 * pad * t.cols as f32, size);
        let x_at: Vec<f32> = widths
            .iter()
            .scan(ctx.x, |x, w| {
                let at = *x;
                *x += w + 2.0 * pad;
                Some(at)
            })
            .collect();
        let table_w: f32 = widths.iter().map(|w| w + 2.0 * pad).sum();
        let lines: Vec<(Vec<Line>, bool)> = t
            .cells
            .iter()
            .zip(toks)
            .map(|(c, (toks, rtl))| {
                let w = widths[c.col..c.col + c.cols].iter().sum::<f32>() + 2.0 * pad * (c.cols - 1) as f32;
                let lines = self
                    .wrap(&toks, w, size, Fill::Greedy)
                    .into_iter()
                    .filter_map(|l| if let Laid::Line(l) = l { Some(l) } else { None })
                    .collect();
                (lines, rtl)
            })
            .collect();

        // Line slots of each row, numbered through the table: a cell's k-th
        // line is in slot `first[row] + k`.
        // A row has as many as the cells ending in it still need.
        let mut ending: Vec<Vec<usize>> = vec![Vec::new(); t.rows];
        for (i, c) in t.cells.iter().enumerate() {
            ending[c.row + c.rows - 1].push(i);
        }
        let mut first = vec![0usize; t.rows + 1];
        for r in 0..t.rows {
            let need = ending[r]
                .iter()
                .map(|&i| lines[i].0.len().saturating_sub(first[r] - first[t.cells[i].row]))
                .max()
                .unwrap_or(0);
            first[r + 1] = first[r] + need.max(1);
        }
        // The lines in each slot: (cell, line).
        let mut in_slot: Vec<Vec<(usize, usize)>> = vec![Vec::new(); first[t.rows]];
        let mut slot_h = vec![size * LINE_SPACING; first[t.rows]];
        for (i, (c, (ls, _))) in t.cells.iter().zip(&lines).enumerate() {
            for (k, l) in ls.iter().enumerate() {
                let s = first[c.row] + k;
                in_slot[s].push((i, k));
                slot_h[s] = slot_h[s].max(self.line_height(l));
            }
        }
        // The first column of each row not spanned from a row above.
        let mut start_col = vec![usize::MAX; t.rows + 1];
        for c in &t.cells {
            start_col[c.row] = start_col[c.row].min(c.col);
        }

        let text_h = self.top() - self.bottom();
        self.may_break(0.0);
        self.draw_marker(self.y - fs);
        self.stroke_line(ctx.x, self.y, ctx.x + table_w, self.y, 0.8, RULE);
        // Keys head the rows they span.
        let table = self.tags.begin("Table");
        let mut cell_elems = vec![0; t.cells.len()];
        for r in 0..t.rows {
            let tr = self.tags.add(table, "TR");
            for (i, c) in t.cells.iter().enumerate().filter(|(_, c)| c.row == r) {
                let cell = self.tags.add(tr, if c.key { "TH" } else { "TD" });
                let mut attrs = String::from("/O /Table");
                if c.key {
                    attrs.push_str(" /Scope /Row");
                }
                if c.rows > 1 {
                    attrs.push_str(&format!(" /RowSpan {}", c.rows));
                }
                if c.cols > 1 {
                    attrs.push_str(&format!(" /ColSpan {}", c.cols));
                }
                self.tags.elems[cell].attrs = attrs;
                cell_elems[i] = cell;
            }
        }
        for r in 0..t.rows {
            let slots = first[r]..first[r + 1];
            self.may_break(match r {
                0 => FORBID,
                _ => TABLE_ROW + IN_SPAN * start_col[r] as f32,
            });
            self.y -= pad;
            let tall = 2.0 * pad + slot_h[slots.clone()].iter().sum::<f32>() > TALL_ROW * text_h;
            for s in slots.clone() {
                if s > slots.start {
                    self.may_break(if tall { IN_TALL_ROW } else { IN_ROW });
                }
                let lh = slot_h[s];
                for &(i, k) in &in_slot[s] {
                    let (c, (ls, rtl)) = (&t.cells[i], &lines[i]);
                    let line = &ls[k];
                    let last = c.col + c.cols - 1;
                    let w = x_at[last] + widths[last] - x_at[c.col];
                    let off = if *rtl { (w - line.width).max(0.0) } else { 0.0 };
                    self.tags.enter(cell_elems[i]);
                    self.draw_line(line, x_at[c.col] + pad + off, lh);
                    self.tags.end();
                }
                self.y -= lh;
            }
            self.y -= pad;
            let (x0, lw) = if r + 1 == t.rows {
                (ctx.x, 0.8)
            } else {
                (x_at[start_col[r + 1].min(t.cols - 1)], 0.5)
            };
            self.stroke_line(x0, self.y, ctx.x + table_w, self.y, lw, RULE);
        }
        self.tags.end();
    }

    fn number_pages(&mut self) {
        let total = self.pages.len();
        let size = (self.o.font_size * 0.8).max(6.0);
        let y = (self.o.margin * 0.5 - size * 0.3).max(size * 0.5);
        for i in 0..total {
            let runs = self.shape(&(i + 1).to_string(), false, false, false, size);
            let w: f32 = runs.iter().map(|r| r.2).sum();
            let mut x = (self.o.page_width - w) / 2.0;
            let (texts, mapped) = (self.texts.borrow(), self.mapped.borrow());
            let ops = &mut self.pages[i].ops;
            ops.extend_from_slice(b"/Artifact <</Type /Pagination /Subtype /Footer>> BDC\n");
            for (face, glyphs, gw) in &runs {
                let font = &self.o.fonts.faces[*face];
                draw_glyphs(
                    ops,
                    &texts.0,
                    &mapped[*face],
                    font,
                    *face,
                    size,
                    (x, y),
                    MUTED,
                    glyphs,
                );
                x += gw;
            }
            ops.extend_from_slice(b"EMC\n");
        }
    }
}

/// Whether blocks read right-to-left: whether the first strong character
/// of their first text (UAX #9 rules P2 and P3) is right-to-left. Code
/// blocks and rules have no direction and are skipped.
/// Column widths for a table: their natural (single-line) widths if they
/// fit in `avail`, else shrunk toward their minimum (longest word) widths,
/// and at least `least` each.
fn fit_columns(nat: &[f32], min: &[f32], avail: f32, least: f32) -> Vec<f32> {
    let (sum_nat, sum_min): (f32, f32) = (nat.iter().sum(), min.iter().sum());
    let widths: Vec<f32> = if sum_nat <= avail {
        nat.to_vec()
    } else if sum_min < avail && sum_nat > sum_min {
        let k = (avail - sum_min) / (sum_nat - sum_min);
        nat.iter().zip(min).map(|(n, m)| m + (n - m) * k).collect()
    } else {
        let k = avail / sum_min.max(1.0);
        min.iter().map(|m| m * k).collect()
    };
    widths.iter().map(|w| w.max(least)).collect()
}

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

    /// Breakpoints between `n` lines of height 10, with the given penalties
    /// before each line.
    fn lines(penalties: &[f32]) -> (Vec<Breakpoint>, f32) {
        let bps = penalties
            .iter()
            .enumerate()
            .map(|(k, &penalty)| {
                let y = -10.0 * k as f32;
                Breakpoint {
                    above: y,
                    below: y,
                    penalty,
                }
            })
            .collect();
        (bps, -10.0 * penalties.len() as f32)
    }

    fn breaks(plan: &[bool]) -> Vec<usize> {
        (0..plan.len()).filter(|&k| plan[k]).collect()
    }

    #[test]
    fn pages_fill_up_when_nothing_is_penalized() {
        let (bps, end) = lines(&[0.0; 25]);
        assert_eq!(breaks(&plan_pages(&bps, end, 100.0)), [10, 20]);
    }

    #[test]
    fn pages_break_early_rather_than_at_a_penalty() {
        // Lines 8 to 14 are a block that the page should not break inside;
        // it goes to the second page whole, as there is room at the end.
        let mut p = [0.0; 25];
        p[9..15].fill(IN_CODE);
        let (bps, end) = lines(&p);
        assert_eq!(breaks(&plan_pages(&bps, end, 100.0)), [8, 18]);
        // Forbidden breaks are only taken when nothing else fits.
        let (bps, end) = lines(&[FORBID; 15]);
        assert_eq!(breaks(&plan_pages(&bps, end, 100.0)), [10]);
    }

    #[test]
    fn a_page_is_added_rather_than_breaking_where_it_is_forbidden() {
        // Two full pages, but the break between them is forbidden: the
        // first page ends a line early and a third page takes the rest.
        let mut p = [0.0; 20];
        p[10] = FORBID;
        let (bps, end) = lines(&p);
        assert_eq!(breaks(&plan_pages(&bps, end, 100.0)), [9, 19]);
    }

    #[test]
    fn blocks_are_split_when_there_is_no_room_at_the_end() {
        // The block of lines 8 to 14 would move to the second page whole
        // if there were room at the end (see above), but with 20 lines that
        // would take a third page, so it is split.
        let mut p = [0.0; 20];
        p[9..15].fill(IN_CODE);
        let (bps, end) = lines(&p);
        assert_eq!(breaks(&plan_pages(&bps, end, 100.0)), [10]);
    }

    #[test]
    fn content_taller_than_a_page_gets_a_page_of_its_own() {
        // Three lines, an image 150 high, and three more lines.
        let bps: Vec<Breakpoint> = [0.0, -10.0, -20.0, -30.0, -180.0, -190.0]
            .iter()
            .map(|&y| Breakpoint {
                above: y,
                below: y,
                penalty: 0.0,
            })
            .collect();
        let plan = plan_pages(&bps, -200.0, 100.0);
        assert_eq!(breaks(&plan), [3, 4]);
    }

    #[test]
    fn pages_are_not_added_to_avoid_penalties() {
        // Every break is bad, but three pages are still enough.
        let (bps, end) = lines(&[CLUB; 30]);
        assert_eq!(breaks(&plan_pages(&bps, end, 100.0)), [10, 20]);
        let (bps, end) = lines(&[]);
        assert!(plan_pages(&bps, end, 100.0).is_empty());
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
