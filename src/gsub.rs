//! Glyph substitution from the OpenType GSUB table: ligatures, contextual
//! alternates and the other substitutions a font applies by default.
//!
//! The features applied are those HarfBuzz enables by default for the
//! scripts that need no script-specific shaping (Latin, Greek, Cyrillic,
//! Armenian, Hebrew, Chinese, Japanese and Korean): `rvrn`, `ccmp`, `locl`, `rlig`, `liga`, `clig`,
//! `calt` and `rclt`, taken from the default language system of the text's
//! script. Their lookups are applied in lookup-list order, each one over the
//! whole glyph sequence, as the OpenType specification prescribes. All
//! lookup types are implemented: single, multiple, alternate (the first
//! alternate), ligature, contextual and chained contextual (all three
//! formats), extension and reverse chained contextual, honouring the lookup
//! flags (ignored glyph classes, mark attachment types and mark filtering
//! sets).
//!
//! Every glyph carries the index of the first character it represents (its
//! *cluster*), so text can be recovered for copy and paste and line breaks
//! can be mapped to glyphs. Work is bounded: malformed or hostile fonts can
//! neither loop forever nor grow the glyph sequence without limit.

use crate::otl::{self, coverage, u16_at, u32_at, Budget, Gdef, Scripts};

/// Features applied by default, in no particular order (lookup order decides).
const FEATURES: [&[u8; 4]; 8] = [
    b"rvrn", b"ccmp", b"locl", b"rlig", b"liga", b"clig", b"calt", b"rclt",
];

/// Nested lookups deeper than this are not applied.
const MAX_NESTING: usize = 8;

/// Longest input sequence a contextual rule may match.
const MAX_CONTEXT: usize = 64;

/// The script whose language system supplies the features.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Script {
    Latin,
    Greek,
    Cyrillic,
    Armenian,
    Hebrew,
    Han,
    Kana,
    Hangul,
    /// Anything else, including text without letters.
    Other,
}

impl Script {
    /// The script of a character, or `None` for characters shared by all
    /// scripts (digits, punctuation, symbols, combining marks).
    pub fn of(c: char) -> Option<Script> {
        Some(match c as u32 {
            0x41..=0x5A | 0x61..=0x7A | 0xAA | 0xBA | 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2AF => {
                Script::Latin
            }
            0x1D00..=0x1D25 | 0x1E00..=0x1EFF | 0x2C60..=0x2C7F | 0xA720..=0xA7FF | 0xAB30..=0xAB6F => {
                Script::Latin
            }
            0xFB00..=0xFB06 | 0xFF21..=0xFF3A | 0xFF41..=0xFF5A => Script::Latin,
            0x370..=0x373 | 0x376..=0x377 | 0x37B..=0x37D | 0x37F | 0x386 | 0x388..=0x3E1 | 0x3F0..=0x3FF => {
                Script::Greek
            }
            0x1F00..=0x1FFF => Script::Greek,
            0x400..=0x52F | 0x1C80..=0x1C8F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F => Script::Cyrillic,
            0x531..=0x556 | 0x559..=0x58A | 0x58D..=0x58F | 0xFB13..=0xFB17 => Script::Armenian,
            0x591..=0x5C7 | 0x5D0..=0x5F4 | 0xFB1D..=0xFB4F => Script::Hebrew,
            0x2E80..=0x2FDF | 0x3005 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303B | 0x3400..=0x4DBF => {
                Script::Han
            }
            0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF => Script::Han,
            0x3041..=0x3096 | 0x309D..=0x309F | 0x30A1..=0x30FA | 0x30FD..=0x30FF | 0x31F0..=0x31FF => {
                Script::Kana
            }
            0xFF66..=0xFF6F | 0xFF71..=0xFF9D | 0x1B000..=0x1B16F => Script::Kana,
            0x1100..=0x11FF | 0x3131..=0x318E | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xFFA0..=0xFFDC => {
                Script::Hangul
            }
            _ => return None,
        })
    }

    /// OpenType script tags to look for, best first.
    pub(crate) fn tags(self) -> &'static [&'static [u8; 4]] {
        match self {
            Script::Latin => &[b"latn", b"DFLT", b"dflt"],
            Script::Greek => &[b"grek", b"DFLT", b"dflt", b"latn"],
            Script::Cyrillic => &[b"cyrl", b"DFLT", b"dflt", b"latn"],
            Script::Armenian => &[b"armn", b"DFLT", b"dflt", b"latn"],
            Script::Hebrew => &[b"hebr", b"DFLT", b"dflt", b"latn"],
            Script::Han => &[b"hani", b"DFLT", b"dflt", b"latn"],
            Script::Kana => &[b"kana", b"DFLT", b"dflt", b"latn"],
            Script::Hangul => &[b"hang", b"DFLT", b"dflt", b"latn"],
            Script::Other => &[b"DFLT", b"dflt", b"latn"],
        }
    }

    /// Every script, Latin first.
    pub const ALL: [Script; 9] = [
        Script::Latin,
        Script::Greek,
        Script::Cyrillic,
        Script::Armenian,
        Script::Hebrew,
        Script::Han,
        Script::Kana,
        Script::Hangul,
        Script::Other,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// A glyph and the index of the first character it stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub id: u16,
    pub cluster: u32,
}

struct Lookup {
    kind: u16,
    flags: u16,
    mark_set: u16,
    /// Subtable offsets relative to GSUB, extension subtables resolved.
    subtables: Vec<usize>,
    /// Glyphs that can start a match, one bit per glyph; `None` if unknown
    /// (every glyph is tried).
    first: Option<Vec<u64>>,
}

/// The GSUB lookups of a face and which of them apply to each script.
#[derive(Default)]
pub struct Gsub {
    lookups: Vec<Option<Lookup>>,
    /// Lookup indices to apply, in order, per [`Script`].
    plans: [Vec<u16>; Script::ALL.len()],
    gdef: Gdef,
}

/// Upper bound on the records read while parsing.
const BUDGET: usize = 1_000_000;

impl Gsub {
    pub fn parse(gsub: Option<&[u8]>, gdef: Option<&[u8]>, num_glyphs: u16) -> Gsub {
        let Some(t) = gsub else { return Gsub::default() };
        Gsub::try_parse(t, num_glyphs)
            .map(|mut g| {
                g.gdef = Gdef::parse(gdef);
                g
            })
            .unwrap_or_default()
    }

    fn try_parse(t: &[u8], num_glyphs: u16) -> Option<Gsub> {
        let mut budget = Budget(BUDGET);
        let mut plans: [Vec<u16>; Script::ALL.len()] = Default::default();
        // A malformed or budget-exhausting script section only loses that
        // script; Latin comes first so it survives problems in later ones.
        for s in Script::ALL {
            plans[s.index()] =
                otl::feature_lookups(t, Scripts::First(s.tags()), &FEATURES, &mut budget).unwrap_or_default();
        }
        let list = u16_at(t, 8)? as usize;
        let n = budget.take(u16_at(t, list)? as usize)?;
        let mut lookups: Vec<Option<Lookup>> =
            (0..n).map(|i| parse_lookup(t, list, i, &mut budget)).collect();
        for plan in &mut plans {
            plan.retain(|&i| lookups.get(i as usize).is_some_and(Option::is_some));
        }
        let mut wanted: Vec<u16> = plans.iter().flatten().copied().collect();
        wanted.sort_unstable();
        wanted.dedup();
        for i in wanted {
            if let Some(Some(l)) = lookups.get_mut(i as usize) {
                l.first = first_glyphs(t, l, num_glyphs, &mut budget);
            }
        }
        Some(Gsub {
            lookups,
            plans,
            gdef: Gdef::default(),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.plans.iter().all(Vec::is_empty)
    }

    /// Apply the default features for `script` to `glyphs`. `gsub` and
    /// `gdef` are the table data the lookups were parsed from.
    pub fn apply(&self, gsub: &[u8], gdef: &[u8], script: Script, glyphs: &mut Vec<Glyph>) {
        let plan = &self.plans[script.index()];
        if plan.is_empty() || glyphs.is_empty() {
            return;
        }
        let mut cx = Ctx {
            t: gsub,
            gdef_data: gdef,
            g: self,
            buf: std::mem::take(glyphs),
            ops: 0,
            max_ops: 0,
            max_len: 0,
        };
        let len = cx.buf.len();
        cx.max_ops = len.saturating_mul(256).saturating_add(4096);
        cx.max_len = len.saturating_mul(16).saturating_add(64);
        for &li in plan {
            cx.apply_lookup(li);
        }
        *glyphs = cx.buf;
    }
}

fn parse_lookup(t: &[u8], list: usize, i: usize, budget: &mut Budget) -> Option<Lookup> {
    let at = list + u16_at(t, list + 2 + 2 * i)? as usize;
    let (mut kind, flags) = (u16_at(t, at)?, u16_at(t, at + 2)?);
    let n = budget.take(u16_at(t, at + 4)? as usize)?;
    let mark_set = if flags & otl::USE_MARK_FILTERING_SET != 0 {
        u16_at(t, at + 6 + 2 * n).unwrap_or(0)
    } else {
        0
    };
    let mut subtables = Vec::with_capacity(n);
    let mut ext_kind = None;
    for k in 0..n {
        let st = at + u16_at(t, at + 6 + 2 * k)? as usize;
        if kind == 7 {
            // Extension: all subtables must wrap the same lookup type.
            let (Some(1), Some(inner), Some(off)) = (u16_at(t, st), u16_at(t, st + 2), u32_at(t, st + 4))
            else {
                continue;
            };
            if inner == 7 || *ext_kind.get_or_insert(inner) != inner {
                continue;
            }
            subtables.push(st + off as usize);
        } else {
            subtables.push(st);
        }
    }
    if kind == 7 {
        kind = ext_kind?;
    }
    if !(1..=8).contains(&kind) {
        return None;
    }
    Some(Lookup {
        kind,
        flags,
        mark_set,
        subtables,
        first: None,
    })
}

/// The set of glyphs that can start a match of lookup `l`.
fn first_glyphs(t: &[u8], l: &Lookup, num_glyphs: u16, budget: &mut Budget) -> Option<Vec<u64>> {
    let mut bits = vec![0u64; (num_glyphs as usize).div_ceil(64)];
    for &st in &l.subtables {
        let format = u16_at(t, st)?;
        let cov = match (l.kind, format) {
            (5, 3) => st + u16_at(t, st + 6)? as usize,
            (6, 3) => {
                let nb = u16_at(t, st + 2)? as usize;
                st + u16_at(t, st + 6 + 2 * nb)? as usize
            }
            _ => st + u16_at(t, st + 2)? as usize,
        };
        let known = otl::coverage_glyphs(t, cov, num_glyphs, budget, |g| {
            bits[g as usize / 64] |= 1 << (g % 64);
        });
        if !known {
            return None;
        }
    }
    Some(bits)
}

/// A sequence a contextual rule matches against: glyph IDs, classes of a
/// class definition, or coverage tables (offsets relative to `base`).
#[derive(Clone, Copy)]
enum Seq {
    Glyphs { at: usize, n: usize },
    Classes { at: usize, n: usize, classes: usize },
    Coverages { at: usize, n: usize, base: usize },
}

impl Seq {
    fn len(&self) -> usize {
        match *self {
            Seq::Glyphs { n, .. } | Seq::Classes { n, .. } | Seq::Coverages { n, .. } => n,
        }
    }

    fn matches(&self, t: &[u8], k: usize, g: u16) -> bool {
        match *self {
            Seq::Glyphs { at, .. } => u16_at(t, at + 2 * k) == Some(g),
            Seq::Classes { at, classes, .. } => u16_at(t, at + 2 * k) == Some(otl::class_of(t, classes, g)),
            Seq::Coverages { at, base, .. } => {
                u16_at(t, at + 2 * k).is_some_and(|o| coverage(t, base + o as usize, g).is_some())
            }
        }
    }
}

/// A contextual rule: what must precede, the rest of the input after the
/// first glyph, what must follow, and the nested lookups to apply.
struct Rule {
    backtrack: Option<Seq>,
    input: Seq,
    lookahead: Option<Seq>,
    records: usize,
    nrecords: usize,
}

struct Ctx<'a> {
    t: &'a [u8],
    gdef_data: &'a [u8],
    g: &'a Gsub,
    buf: Vec<Glyph>,
    ops: usize,
    max_ops: usize,
    max_len: usize,
}

impl<'a> Ctx<'a> {
    fn lookup(&self, li: u16) -> Option<&'a Lookup> {
        let g: &'a Gsub = self.g;
        g.lookups.get(li as usize)?.as_ref()
    }

    fn skips(&self, l: &Lookup, g: u16) -> bool {
        otl::ignores(&self.g.gdef, self.gdef_data, l.flags, l.mark_set, g)
    }

    fn next(&self, l: &Lookup, mut i: usize) -> Option<usize> {
        loop {
            i += 1;
            let g = self.buf.get(i)?;
            if !self.skips(l, g.id) {
                return Some(i);
            }
        }
    }

    fn prev(&self, l: &Lookup, mut i: usize) -> Option<usize> {
        loop {
            i = i.checked_sub(1)?;
            if !self.skips(l, self.buf[i].id) {
                return Some(i);
            }
        }
    }

    fn spend(&mut self) -> bool {
        self.ops += 1;
        self.ops <= self.max_ops
    }

    /// Apply lookup `li` over the whole glyph sequence.
    fn apply_lookup(&mut self, li: u16) {
        let Some(l) = self.lookup(li) else { return };
        let starts = |g: u16| {
            l.first
                .as_ref()
                .is_none_or(|b| b.get(g as usize / 64).is_some_and(|w| w >> (g % 64) & 1 != 0))
        };
        if l.kind == 8 {
            for i in (0..self.buf.len()).rev() {
                let g = self.buf[i].id;
                if starts(g) && !self.skips(l, g) && self.spend() {
                    self.reverse_chain(l, i);
                }
            }
            return;
        }
        let mut i = 0;
        while i < self.buf.len() {
            let g = self.buf[i].id;
            if starts(g) && !self.skips(l, g) {
                if !self.spend() {
                    return;
                }
                if let Some(next) = self.apply_at(li, i, 0) {
                    i = next;
                    continue;
                }
            }
            i += 1;
        }
    }

    /// Apply lookup `li` at glyph `i` (the first subtable that matches).
    /// Returns where processing continues, or `None` if nothing matched.
    fn apply_at(&mut self, li: u16, i: usize, depth: usize) -> Option<usize> {
        let l = self.lookup(li)?;
        let t = self.t;
        for &st in &l.subtables {
            let g = self.buf.get(i)?.id;
            let format = u16_at(t, st)?;
            let applied = match l.kind {
                1 => {
                    let Some(cov) = coverage(t, st + u16_at(t, st + 2)? as usize, g) else {
                        continue;
                    };
                    let new = match format {
                        1 => g.wrapping_add(u16_at(t, st + 4)?),
                        2 => u16_at(t, st + 6 + 2 * cov)?,
                        _ => continue,
                    };
                    self.buf[i].id = new;
                    Some(i + 1)
                }
                2 | 3 => {
                    let Some(cov) = coverage(t, st + u16_at(t, st + 2)? as usize, g) else {
                        continue;
                    };
                    if format != 1 || cov >= u16_at(t, st + 4)? as usize {
                        continue;
                    }
                    let seq = st + u16_at(t, st + 6 + 2 * cov)? as usize;
                    let n = u16_at(t, seq)? as usize;
                    if l.kind == 3 {
                        // Alternate: the default is the first one.
                        if n == 0 {
                            continue;
                        }
                        self.buf[i].id = u16_at(t, seq + 2)?;
                        Some(i + 1)
                    } else {
                        self.multiple(i, seq, n)
                    }
                }
                4 => self.ligature(l, st, i, g),
                5 | 6 => self.context(l, st, i, g, depth),
                _ => None,
            };
            if applied.is_some() {
                return applied;
            }
        }
        None
    }

    /// Replace glyph `i` with the `n` glyphs listed at `seq + 2`.
    fn multiple(&mut self, i: usize, seq: usize, n: usize) -> Option<usize> {
        if self.buf.len() - 1 + n > self.max_len {
            return None;
        }
        let ids: Option<Vec<u16>> = (0..n).map(|k| u16_at(self.t, seq + 2 + 2 * k)).collect();
        let cluster = self.buf[i].cluster;
        if n == 0 {
            // Deleted: the characters go with the next glyph if there is no
            // previous one to carry them.
            self.buf.remove(i);
            if i == 0 {
                if let Some(next) = self.buf.first_mut() {
                    next.cluster = cluster;
                }
            }
            return Some(i);
        }
        self.buf
            .splice(i..=i, ids?.into_iter().map(|id| Glyph { id, cluster }));
        Some(i + n)
    }

    fn ligature(&mut self, l: &Lookup, st: usize, i: usize, g: u16) -> Option<usize> {
        let t = self.t;
        let cov = coverage(t, st + u16_at(t, st + 2)? as usize, g)?;
        if u16_at(t, st)? != 1 || cov >= u16_at(t, st + 4)? as usize {
            return None;
        }
        let set = st + u16_at(t, st + 6 + 2 * cov)? as usize;
        let count = u16_at(t, set)? as usize;
        'ligs: for k in 0..count {
            let lig = set + u16_at(t, set + 2 + 2 * k)? as usize;
            let (id, n) = (u16_at(t, lig)?, u16_at(t, lig + 2)? as usize);
            if n == 0 || n > MAX_CONTEXT {
                continue;
            }
            let mut pos = Vec::with_capacity(n);
            pos.push(i);
            for c in 1..n {
                let Some(j) = self.next(l, pos[c - 1]) else {
                    continue 'ligs;
                };
                if u16_at(t, lig + 4 + 2 * (c - 1)) != Some(self.buf[j].id) {
                    continue 'ligs;
                }
                pos.push(j);
            }
            // The ligature takes the place of the first component; skipped
            // glyphs in between (usually marks) follow it, and everything
            // from the first to the last component becomes one cluster.
            let last = pos[n - 1];
            let cluster = self.buf[i].cluster;
            for gl in &mut self.buf[i..=last] {
                gl.cluster = cluster;
            }
            self.buf[i].id = id;
            for &j in pos[1..].iter().rev() {
                self.buf.remove(j);
            }
            return Some(i + 1);
        }
        None
    }

    fn context(&mut self, l: &Lookup, st: usize, i: usize, g: u16, depth: usize) -> Option<usize> {
        let t = self.t;
        let chain = l.kind == 6;
        let format = u16_at(t, st)?;
        let rule_at = |r: usize, classes: Option<[usize; 3]>| -> Option<Rule> {
            let seq = |at: usize, n: usize, which: usize| match classes {
                Some(c) => Seq::Classes {
                    at,
                    n,
                    classes: c[which],
                },
                None => Seq::Glyphs { at, n },
            };
            if chain {
                let nb = u16_at(t, r)? as usize;
                let ni = u16_at(t, r + 2 + 2 * nb)? as usize;
                let at_i = r + 4 + 2 * nb;
                let at_la = at_i + 2 * ni.checked_sub(1)?;
                let nla = u16_at(t, at_la)? as usize;
                let at_rec = at_la + 2 + 2 * nla;
                Some(Rule {
                    backtrack: Some(seq(r + 2, nb, 0)),
                    input: seq(at_i, ni - 1, 1),
                    lookahead: Some(seq(at_la + 2, nla, 2)),
                    records: at_rec + 2,
                    nrecords: u16_at(t, at_rec)? as usize,
                })
            } else {
                let ni = u16_at(t, r)? as usize;
                let nrec = u16_at(t, r + 2)? as usize;
                Some(Rule {
                    backtrack: None,
                    input: seq(r + 4, ni.checked_sub(1)?, 1),
                    lookahead: None,
                    records: r + 4 + 2 * (ni - 1),
                    nrecords: nrec,
                })
            }
        };
        match format {
            1 | 2 => {
                let cov = coverage(t, st + u16_at(t, st + 2)? as usize, g)?;
                let (classes, sets_at) = if format == 2 {
                    let c = |o: usize| -> Option<usize> { Some(st + u16_at(t, st + o)? as usize) };
                    if chain {
                        (Some([c(4)?, c(6)?, c(8)?]), st + 10)
                    } else {
                        let cd = c(4)?;
                        (Some([cd, cd, cd]), st + 6)
                    }
                } else {
                    (None, st + 4)
                };
                let index = match classes {
                    Some(c) => otl::class_of(t, c[1], g) as usize,
                    None => cov,
                };
                if index >= u16_at(t, sets_at)? as usize {
                    return None;
                }
                let set = match u16_at(t, sets_at + 2 + 2 * index)? {
                    0 => return None,
                    o => st + o as usize,
                };
                let n = u16_at(t, set)? as usize;
                for k in 0..n {
                    let Some(rule) = rule_at(set + u16_at(t, set + 2 + 2 * k)? as usize, classes) else {
                        continue;
                    };
                    if let Some(end) = self.try_rule(l, &rule, i, depth) {
                        return Some(end);
                    }
                    if !self.spend() {
                        return None;
                    }
                }
                None
            }
            3 => {
                let rule = if chain {
                    let nb = u16_at(t, st + 2)? as usize;
                    let at_i = st + 4 + 2 * nb;
                    let ni = u16_at(t, at_i)? as usize;
                    let at_la = at_i + 2 + 2 * ni;
                    let nla = u16_at(t, at_la)? as usize;
                    let at_rec = at_la + 2 + 2 * nla;
                    let first = u16_at(t, at_i + 2)? as usize;
                    coverage(t, st + first, g)?;
                    Rule {
                        backtrack: Some(Seq::Coverages {
                            at: st + 4,
                            n: nb,
                            base: st,
                        }),
                        input: Seq::Coverages {
                            at: at_i + 4,
                            n: ni.checked_sub(1)?,
                            base: st,
                        },
                        lookahead: Some(Seq::Coverages {
                            at: at_la + 2,
                            n: nla,
                            base: st,
                        }),
                        records: at_rec + 2,
                        nrecords: u16_at(t, at_rec)? as usize,
                    }
                } else {
                    let ni = u16_at(t, st + 2)? as usize;
                    let nrec = u16_at(t, st + 4)? as usize;
                    coverage(t, st + u16_at(t, st + 6)? as usize, g)?;
                    Rule {
                        backtrack: None,
                        input: Seq::Coverages {
                            at: st + 8,
                            n: ni.checked_sub(1)?,
                            base: st,
                        },
                        lookahead: None,
                        records: st + 6 + 2 * ni,
                        nrecords: nrec,
                    }
                };
                self.try_rule(l, &rule, i, depth)
            }
            _ => None,
        }
    }

    /// Match `rule` with its first input glyph at `i`; if it matches, apply
    /// its nested lookups and return where processing continues.
    fn try_rule(&mut self, l: &Lookup, rule: &Rule, i: usize, depth: usize) -> Option<usize> {
        let t = self.t;
        if rule.input.len() + 1 > MAX_CONTEXT {
            return None;
        }
        let mut pos = Vec::with_capacity(rule.input.len() + 1);
        pos.push(i);
        for k in 0..rule.input.len() {
            let j = self.next(l, pos[k])?;
            if !rule.input.matches(t, k, self.buf[j].id) {
                return None;
            }
            pos.push(j);
        }
        if let Some(b) = rule.backtrack {
            let mut j = i;
            for k in 0..b.len() {
                j = self.prev(l, j)?;
                if !b.matches(t, k, self.buf[j].id) {
                    return None;
                }
            }
        }
        if let Some(la) = rule.lookahead {
            let mut j = pos[pos.len() - 1];
            for k in 0..la.len() {
                j = self.next(l, j)?;
                if !la.matches(t, k, self.buf[j].id) {
                    return None;
                }
            }
        }
        Some(self.apply_records(rule, pos, depth))
    }

    /// Apply the nested lookups of a matched rule, keeping track of where
    /// the matched glyphs move as nested lookups add or remove glyphs (the
    /// same bookkeeping as HarfBuzz).
    fn apply_records(&mut self, rule: &Rule, mut pos: Vec<usize>, depth: usize) -> usize {
        let t = self.t;
        let mut end = pos[pos.len() - 1] as isize + 1;
        if depth >= MAX_NESTING {
            return end as usize;
        }
        for r in 0..rule.nrecords {
            let rec = rule.records + 4 * r;
            let (Some(idx), Some(li)) = (u16_at(t, rec), u16_at(t, rec + 2)) else {
                break;
            };
            let idx = idx as usize;
            if idx >= pos.len() || pos[idx] >= self.buf.len() {
                continue;
            }
            if !self.spend() {
                break;
            }
            let orig_len = self.buf.len() as isize;
            let _ = self.apply_at(li, pos[idx], depth + 1);
            let mut delta = self.buf.len() as isize - orig_len;
            if delta == 0 {
                continue;
            }
            end += delta;
            let here = pos[idx] as isize;
            if end < here {
                delta += here - end;
                end = here;
            }
            let next = idx + 1;
            let next_after = if delta > 0 {
                if pos.len() + delta as usize > MAX_CONTEXT {
                    break;
                }
                pos.splice(next..next, std::iter::repeat_n(0, delta as usize));
                next + delta as usize
            } else {
                let remove = (-delta as usize).min(pos.len() - next);
                pos.drain(next..next + remove);
                delta = -(remove as isize);
                next
            };
            for j in idx + 1..next_after {
                pos[j] = pos[j - 1] + 1;
            }
            for p in &mut pos[next_after..] {
                *p = (*p as isize + delta).max(0) as usize;
            }
        }
        end.max(0) as usize
    }

    fn reverse_chain(&mut self, l: &Lookup, i: usize) {
        let t = self.t;
        for &st in &l.subtables {
            let g = self.buf[i].id;
            let Some(sub) = (|| -> Option<u16> {
                if u16_at(t, st)? != 1 {
                    return None;
                }
                let cov = coverage(t, st + u16_at(t, st + 2)? as usize, g)?;
                let nb = u16_at(t, st + 4)? as usize;
                let at_la = st + 6 + 2 * nb;
                let nla = u16_at(t, at_la)? as usize;
                let at_sub = at_la + 2 + 2 * nla;
                let backtrack = Seq::Coverages {
                    at: st + 6,
                    n: nb,
                    base: st,
                };
                let lookahead = Seq::Coverages {
                    at: at_la + 2,
                    n: nla,
                    base: st,
                };
                let mut j = i;
                for k in 0..nb {
                    j = self.prev(l, j)?;
                    if !backtrack.matches(t, k, self.buf[j].id) {
                        return None;
                    }
                }
                let mut j = i;
                for k in 0..nla {
                    j = self.next(l, j)?;
                    if !lookahead.matches(t, k, self.buf[j].id) {
                        return None;
                    }
                }
                if cov >= u16_at(t, at_sub)? as usize {
                    return None;
                }
                u16_at(t, at_sub + 2 + 2 * cov)
            })() else {
                continue;
            };
            self.buf[i].id = sub;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    fn cov(glyphs: &[u16]) -> Vec<u8> {
        let mut v = vec![1, glyphs.len() as u16];
        v.extend_from_slice(glyphs);
        be(&v)
    }

    /// A class definition (format 2) from `(first, last, class)` ranges.
    fn classes(ranges: &[(u16, u16, u16)]) -> Vec<u8> {
        let mut v = vec![2, ranges.len() as u16];
        for &(a, b, c) in ranges {
            v.extend_from_slice(&[a, b, c]);
        }
        be(&v)
    }

    /// Concatenate a header of u16 fields with trailing blobs; each `None`
    /// field becomes the offset of the next blob.
    fn table(fields: &[Option<u16>], blobs: &[Vec<u8>]) -> Vec<u8> {
        let mut off = 2 * fields.len();
        let mut head = Vec::new();
        let mut blobs = blobs.iter();
        let mut tail = Vec::new();
        for f in fields {
            match f {
                Some(v) => head.push(*v),
                None => {
                    let b = blobs.next().expect("a blob per offset");
                    head.push(off as u16);
                    off += b.len();
                    tail.extend_from_slice(b);
                }
            }
        }
        let mut out = be(&head);
        out.extend(tail);
        out
    }

    /// A GSUB table with one `liga` feature (script DFLT) running the first
    /// `active` lookups, given as `(type, flags, subtables)`. The others are
    /// only reachable from contextual lookups.
    fn gsub_with(lookups: &[(u16, u16, Vec<Vec<u8>>)], active: u16) -> Vec<u8> {
        let langsys = be(&[0, 0xFFFF, 1, 0]);
        let script = table(&[None, Some(0)], &[langsys]);
        let mut script_list = be(&[1]);
        script_list.extend_from_slice(b"DFLT");
        script_list.extend(be(&[8]));
        script_list.extend(script);
        let mut feature = be(&[0, active]);
        feature.extend(be(&(0..active).collect::<Vec<_>>()));
        let mut feature_list = be(&[1]);
        feature_list.extend_from_slice(b"liga");
        feature_list.extend(be(&[8]));
        feature_list.extend(feature);
        let tables: Vec<Vec<u8>> = lookups
            .iter()
            .map(|(kind, flags, subs)| {
                let mut f = vec![Some(*kind), Some(*flags), Some(subs.len() as u16)];
                f.extend(subs.iter().map(|_| None));
                table(&f, subs)
            })
            .collect();
        let mut f = vec![Some(tables.len() as u16)];
        f.extend(tables.iter().map(|_| None));
        let lookup_list = table(&f, &tables);
        table(
            &[Some(1), Some(0), None, None, None],
            &[script_list, feature_list, lookup_list],
        )
    }

    fn gsub(lookups: &[(u16, u16, Vec<Vec<u8>>)]) -> Vec<u8> {
        gsub_with(lookups, lookups.len() as u16)
    }

    fn ids(t: &[u8], input: &[u16]) -> Vec<u16> {
        run(t, &[], input).iter().map(|g| g.0).collect()
    }

    fn run(t: &[u8], gdef: &[u8], ids: &[u16]) -> Vec<(u16, u32)> {
        let g = Gsub::parse(Some(t), Some(gdef).filter(|d| !d.is_empty()), 100);
        let mut buf: Vec<Glyph> = ids
            .iter()
            .enumerate()
            .map(|(k, &id)| Glyph {
                id,
                cluster: k as u32,
            })
            .collect();
        g.apply(t, gdef, Script::Latin, &mut buf);
        buf.iter().map(|g| (g.id, g.cluster)).collect()
    }

    #[test]
    fn single_multiple_and_ligature() {
        let single = table(&[Some(2), None, Some(1), Some(30)], &[cov(&[3])]);
        let multiple = table(&[Some(1), None, Some(1), None], &[cov(&[5]), be(&[2, 6, 7])]);
        let ligature = table(
            &[Some(1), None, Some(1), None],
            &[
                cov(&[1]),
                table(&[Some(1), None], &[[be(&[10, 3]), be(&[2, 30])].concat()]),
            ],
        );
        let t = gsub(&[
            (1, 0, vec![single]),
            (2, 0, vec![multiple]),
            (4, 0, vec![ligature]),
        ]);
        // 3 becomes 30, then 1 2 30 ligates into 10; 5 becomes 6 7.
        assert_eq!(run(&t, &[], &[1, 2, 3, 5]), [(10, 0), (6, 3), (7, 3)]);
        assert_eq!(run(&t, &[], &[1, 2, 4]), [(1, 0), (2, 1), (4, 2)]);
    }

    #[test]
    fn ligatures_skip_marks_when_asked() {
        // GDEF: glyph 9 is a mark.
        let gdef = table(
            &[Some(1), Some(0), None, Some(0), Some(0), Some(0)],
            &[classes(&[(9, 9, 3)])],
        );
        let ligature = table(
            &[Some(1), None, Some(1), None],
            &[cov(&[1]), table(&[Some(1), None], &[be(&[10, 2, 2])])],
        );
        let t = gsub(&[(4, otl::IGNORE_MARKS, vec![ligature.clone()])]);
        // The mark stays after the ligature, in its cluster.
        assert_eq!(run(&t, &gdef, &[1, 9, 2]), [(10, 0), (9, 0)]);
        let t = gsub(&[(4, 0, vec![ligature])]);
        assert_eq!(run(&t, &gdef, &[1, 9, 2]), [(1, 0), (9, 1), (2, 2)]);
    }

    #[test]
    fn chained_context_with_glyphs_and_classes() {
        // Lookup 1, only reachable from the context: 2 -> 20.
        let single = table(&[Some(1), None, Some(18)], &[cov(&[2])]);
        // Format 1: backtrack [1], input [2 3], lookahead [4]; apply lookup
        // 1 at input position 0.
        let rule = be(&[1, 1, 2, 3, 1, 4, 1, 0, 1]);
        let chain1 = table(
            &[Some(1), None, Some(1), None],
            &[cov(&[2]), table(&[Some(1), None], &[rule])],
        );
        let t = gsub_with(&[(6, 0, vec![chain1]), (1, 0, vec![single.clone()])], 1);
        assert_eq!(ids(&t, &[1, 2, 3, 4]), [1, 20, 3, 4]);
        assert_eq!(ids(&t, &[0, 2, 3, 4]), [0, 2, 3, 4], "backtrack must match");
        assert_eq!(ids(&t, &[1, 2, 3, 5]), [1, 2, 3, 5], "lookahead must match");
        assert_eq!(ids(&t, &[1, 2, 4, 4]), [1, 2, 4, 4], "input must match");
        assert_eq!(ids(&t, &[2, 3, 4]), [2, 3, 4], "backtrack needs a glyph");

        // Format 3: coverages for backtrack [1], input [2] [3], lookahead [4].
        let chain3 = table(
            &[
                Some(3),
                Some(1),
                None,
                Some(2),
                None,
                None,
                Some(1),
                None,
                Some(1),
                Some(0),
                Some(1),
            ],
            &[cov(&[1]), cov(&[2]), cov(&[3]), cov(&[4])],
        );
        let t = gsub_with(&[(6, 0, vec![chain3]), (1, 0, vec![single])], 1);
        assert_eq!(ids(&t, &[1, 2, 3, 4]), [1, 20, 3, 4]);
        assert_eq!(ids(&t, &[1, 2, 3, 5]), [1, 2, 3, 5]);
        assert_eq!(ids(&t, &[1, 2, 2, 4]), [1, 2, 2, 4]);

        // Format 2 (context, not chained): classes 1 = {2}, 2 = {3..5};
        // rule for class 1: input classes [1, 2], lookup 1 at position 1
        // (3..5 -> +10).
        let add = table(&[Some(1), None, Some(10)], &[cov(&[3, 4, 5])]);
        let class_rule = be(&[2, 1, 2, 1, 1]);
        let class_set = table(&[Some(1), None], &[class_rule]);
        let ctx2 = table(
            &[Some(2), None, None, Some(2), Some(0), None],
            &[cov(&[2]), classes(&[(2, 2, 1), (3, 5, 2)]), class_set],
        );
        let t = gsub_with(&[(5, 0, vec![ctx2]), (1, 0, vec![add])], 1);
        assert_eq!(ids(&t, &[2, 4, 4]), [2, 14, 4]);
        assert_eq!(ids(&t, &[2, 5, 2, 3]), [2, 15, 2, 13]);
        assert_eq!(ids(&t, &[2, 6]), [2, 6]);
    }

    #[test]
    fn nested_multiple_substitution_moves_the_context() {
        // Context (format 3) on input [1] [2]: lookup 1 at position 0 turns
        // 1 into 7 8. As in HarfBuzz, the new glyph joins the matched input
        // right after the current position, so sequence index 1 then means
        // the 8 and index 2 the original 2.
        let multiple = table(&[Some(1), None, Some(1), None], &[cov(&[1]), be(&[2, 7, 8])]);
        let single = table(&[Some(1), None, Some(100)], &[cov(&[2, 8])]);
        let ctx = |seq: u16| {
            table(
                &[
                    Some(3),
                    Some(2),
                    Some(2),
                    None,
                    None,
                    Some(0),
                    Some(1),
                    Some(seq),
                    Some(2),
                ],
                &[cov(&[1]), cov(&[2])],
            )
        };
        for (seq, want) in [
            (1, [(7, 0), (108, 0), (2, 1), (3, 2)]),
            (2, [(7, 0), (8, 0), (102, 1), (3, 2)]),
        ] {
            let lookups = [
                (5, 0, vec![ctx(seq)]),
                (2, 0, vec![multiple.clone()]),
                (1, 0, vec![single.clone()]),
            ];
            assert_eq!(run(&gsub_with(&lookups, 1), &[], &[1, 2, 3]), want);
        }
    }

    #[test]
    fn reverse_chaining_single() {
        // 5 -> 50 when followed by 6.
        let rev = table(
            &[Some(1), None, Some(0), Some(1), None, Some(1), Some(50)],
            &[cov(&[5]), cov(&[6])],
        );
        let t = gsub(&[(8, 0, vec![rev])]);
        let ids: Vec<u16> = run(&t, &[], &[5, 6, 5, 5]).iter().map(|g| g.0).collect();
        assert_eq!(ids, [50, 6, 5, 5]);
    }

    #[test]
    fn hostile_tables_stay_bounded() {
        // A multiple substitution that doubles glyph 1 forever, and a
        // context that recurses into itself.
        let multiple = table(&[Some(1), None, Some(1), None], &[cov(&[1]), be(&[2, 1, 1])]);
        let rule = be(&[1, 1, 0, 0]);
        let ctx = table(
            &[Some(1), None, Some(1), None],
            &[cov(&[1]), table(&[Some(1), None], &[rule])],
        );
        let t = gsub(&[(5, 0, vec![ctx]), (2, 0, vec![multiple])]);
        let out = run(&t, &[], &[1; 50]);
        assert!(out.len() <= 50 * 16 + 64, "{}", out.len());
        // Truncated and corrupted tables never panic.
        for n in 0..t.len() {
            let _ = run(&t[..n], &[], &[1, 2, 3]);
        }
        let mut seed = 7u64;
        for _ in 0..2000 {
            let mut d = t.clone();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let i = (seed as usize >> 8) % d.len();
            d[i] = (seed >> 3) as u8;
            let _ = run(&d, &[], &[1, 2, 1, 3, 1]);
        }
    }
}
