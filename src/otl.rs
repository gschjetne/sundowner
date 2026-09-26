//! Reading the OpenType layout tables shared by GSUB and GPOS: coverage
//! and class definition tables, the script/feature/lookup lists and the
//! glyph classes in GDEF. Every read is bounds-checked; malformed data
//! reads as "not covered" or "no features" instead of panicking.

use std::cmp::Ordering;

pub fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(i)?, *d.get(i + 1)?]))
}

pub fn i16_at(d: &[u8], i: usize) -> Option<i16> {
    u16_at(d, i).map(|v| v as i16)
}

pub fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

/// Upper bound on the records read while collecting features and lookups.
/// Real fonts need a few thousand; corrupt counts in a malformed font could
/// otherwise nest into billions of iterations.
pub struct Budget(pub usize);

impl Budget {
    /// Charge `n` records; `None` once the budget is spent.
    pub fn take(&mut self, n: usize) -> Option<usize> {
        self.0 = self.0.checked_sub(n)?;
        Some(n)
    }
}

/// Index of `g` in a coverage table, if covered.
pub fn coverage(d: &[u8], at: usize, g: u16) -> Option<usize> {
    let n = u16_at(d, at + 2)? as usize;
    match u16_at(d, at)? {
        1 => {
            let (mut lo, mut hi) = (0, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                match u16_at(d, at + 4 + 2 * mid)?.cmp(&g) {
                    Ordering::Equal => return Some(mid),
                    Ordering::Less => lo = mid + 1,
                    Ordering::Greater => hi = mid,
                }
            }
            None
        }
        2 => {
            let (mut lo, mut hi) = (0, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let r = at + 4 + 6 * mid;
                let (start, end) = (u16_at(d, r)?, u16_at(d, r + 2)?);
                if g < start {
                    hi = mid;
                } else if g > end {
                    lo = mid + 1;
                } else {
                    return Some(u16_at(d, r + 4)? as usize + (g - start) as usize);
                }
            }
            None
        }
        _ => None,
    }
}

/// Call `f` for every glyph a coverage table lists (below `num_glyphs`).
/// Returns false if the table's format is unknown.
pub fn coverage_glyphs(
    d: &[u8],
    at: usize,
    num_glyphs: u16,
    budget: &mut Budget,
    mut f: impl FnMut(u16),
) -> bool {
    let Some(n) = u16_at(d, at + 2) else { return false };
    let n = n as usize;
    match u16_at(d, at) {
        Some(1) => {
            for i in 0..n {
                match u16_at(d, at + 4 + 2 * i) {
                    Some(g) if g < num_glyphs => f(g),
                    Some(_) => {}
                    None => break,
                }
            }
            budget.take(n).is_some()
        }
        Some(2) => {
            for i in 0..n {
                let r = at + 4 + 6 * i;
                let (Some(start), Some(end)) = (u16_at(d, r), u16_at(d, r + 2)) else {
                    break;
                };
                let end = end.min(num_glyphs.saturating_sub(1));
                if start > end {
                    continue;
                }
                if budget.take(1 + (end - start) as usize).is_none() {
                    return false;
                }
                (start..=end).for_each(&mut f);
            }
            true
        }
        _ => false,
    }
}

/// Class of `g` in a class definition table (0 when not listed).
pub fn class_of(d: &[u8], at: usize, g: u16) -> u16 {
    let class = || -> Option<u16> {
        match u16_at(d, at)? {
            1 => {
                let start = u16_at(d, at + 2)?;
                let n = u16_at(d, at + 4)?;
                if g < start || g - start >= n {
                    return Some(0);
                }
                u16_at(d, at + 6 + 2 * (g - start) as usize)
            }
            2 => {
                let n = u16_at(d, at + 2)? as usize;
                let (mut lo, mut hi) = (0, n);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let r = at + 4 + 6 * mid;
                    let (start, end) = (u16_at(d, r)?, u16_at(d, r + 2)?);
                    if g < start {
                        hi = mid;
                    } else if g > end {
                        lo = mid + 1;
                    } else {
                        return u16_at(d, r + 4);
                    }
                }
                Some(0)
            }
            _ => Some(0),
        }
    };
    class().unwrap_or(0)
}

/// Which language systems to take features from.
pub enum Scripts<'a> {
    /// Every language system of every script (used for kerning, which is
    /// the same in all of them in practice).
    All,
    /// The default language system of the first of these scripts that the
    /// font has.
    First(&'a [&'a [u8; 4]]),
}

/// Indices of the lookups of the features named `tags`, sorted and without
/// duplicates, from a GSUB or GPOS table `t`.
pub fn feature_lookups(
    t: &[u8],
    scripts: Scripts,
    tags: &[&[u8; 4]],
    budget: &mut Budget,
) -> Option<Vec<u16>> {
    let script_list = u16_at(t, 4)? as usize;
    let feature_list = u16_at(t, 6)? as usize;
    let nscripts = budget.take(u16_at(t, script_list)? as usize)?;
    let script_table =
        |s: usize| -> Option<usize> { Some(script_list + u16_at(t, script_list + 2 + 6 * s + 4)? as usize) };

    // Language system tables to read.
    let mut langsys: Vec<usize> = Vec::new();
    match scripts {
        Scripts::All => {
            for s in 0..nscripts {
                let script = script_table(s)?;
                if let Some(d) = u16_at(t, script).filter(|&d| d != 0) {
                    langsys.push(script + d as usize);
                }
                let nlang = budget.take(u16_at(t, script + 2).unwrap_or(0) as usize)?;
                for l in 0..nlang {
                    if let Some(o) = u16_at(t, script + 4 + 6 * l + 4) {
                        langsys.push(script + o as usize);
                    }
                }
            }
        }
        Scripts::First(wanted) => {
            let found = wanted.iter().find_map(|tag| {
                (0..nscripts)
                    .find(|&s| t.get(script_list + 2 + 6 * s..script_list + 6 + 6 * s) == Some(&tag[..]))
            });
            if let Some(script) = found.and_then(script_table) {
                if let Some(d) = u16_at(t, script).filter(|&d| d != 0) {
                    langsys.push(script + d as usize);
                }
            }
        }
    }

    // Feature indices of those language systems, including the required
    // feature.
    let mut feature_ids: Vec<u16> = Vec::new();
    for ls in langsys {
        if let Some(req) = u16_at(t, ls + 2).filter(|&r| r != 0xFFFF) {
            feature_ids.push(req);
        }
        let n = budget.take(u16_at(t, ls + 4).unwrap_or(0) as usize)?;
        feature_ids.extend((0..n).filter_map(|i| u16_at(t, ls + 6 + 2 * i)));
    }
    feature_ids.sort_unstable();
    feature_ids.dedup();

    let nfeatures = u16_at(t, feature_list)? as usize;
    let mut lookups: Vec<u16> = Vec::new();
    for f in feature_ids {
        let f = f as usize;
        if f >= nfeatures {
            continue;
        }
        let rec = feature_list + 2 + 6 * f;
        if !tags.iter().any(|tag| t.get(rec..rec + 4) == Some(&tag[..])) {
            continue;
        }
        let feature = feature_list + u16_at(t, rec + 4)? as usize;
        let n = budget.take(u16_at(t, feature + 2).unwrap_or(0) as usize)?;
        lookups.extend((0..n).filter_map(|i| u16_at(t, feature + 4 + 2 * i)));
    }
    lookups.sort_unstable();
    lookups.dedup();
    Some(lookups)
}

/// A sequence a contextual rule matches against: glyph IDs, classes of a
/// class definition, or coverage tables (offsets relative to `base`).
#[derive(Clone, Copy)]
pub enum Seq {
    Glyphs { at: usize, n: usize },
    Classes { at: usize, n: usize, classes: usize },
    Coverages { at: usize, n: usize, base: usize },
}

impl Seq {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        match *self {
            Seq::Glyphs { n, .. } | Seq::Classes { n, .. } | Seq::Coverages { n, .. } => n,
        }
    }

    pub fn matches(&self, t: &[u8], k: usize, g: u16) -> bool {
        match *self {
            Seq::Glyphs { at, .. } => u16_at(t, at + 2 * k) == Some(g),
            Seq::Classes { at, classes, .. } => u16_at(t, at + 2 * k) == Some(class_of(t, classes, g)),
            Seq::Coverages { at, base, .. } => {
                u16_at(t, at + 2 * k).is_some_and(|o| coverage(t, base + o as usize, g).is_some())
            }
        }
    }
}

/// A contextual rule: what must precede, the rest of the input after the
/// first glyph, what must follow, and the nested lookups to apply.
pub struct Rule {
    pub backtrack: Option<Seq>,
    pub input: Seq,
    pub lookahead: Option<Seq>,
    /// Offset and number of the SequenceLookupRecords.
    pub records: usize,
    pub nrecords: usize,
}

/// What to do after trying a contextual rule.
pub enum Step<R> {
    /// The rule matched; this is the result.
    Done(R),
    /// Try the next rule.
    Next,
    /// Give up on the subtable.
    Stop,
}

/// Try the rules of a contextual (`chain` false) or chained contextual
/// subtable at `st` of GSUB or GPOS table `t` (all three formats) for a
/// first input glyph `g`, in order, until `f` is done with one.
pub fn context_rules<R>(
    t: &[u8],
    st: usize,
    chain: bool,
    g: u16,
    mut f: impl FnMut(&Rule) -> Step<R>,
) -> Option<R> {
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
                Some(c) => class_of(t, c[1], g) as usize,
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
                match f(&rule) {
                    Step::Done(r) => return Some(r),
                    Step::Next => {}
                    Step::Stop => return None,
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
            match f(&rule) {
                Step::Done(r) => Some(r),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Glyph properties from the GDEF table.
#[derive(Default)]
pub struct Gdef {
    /// Offsets, relative to GDEF, of the glyph class definition, the mark
    /// attachment class definition and the mark glyph sets.
    classes: Option<usize>,
    mark_attach: Option<usize>,
    mark_sets: Option<usize>,
}

/// GDEF glyph classes.
pub const BASE: u16 = 1;
pub const LIGATURE: u16 = 2;
pub const MARK: u16 = 3;

impl Gdef {
    pub fn parse(gdef: Option<&[u8]>) -> Gdef {
        let Some(d) = gdef else { return Gdef::default() };
        let off = |i: usize| u16_at(d, i).filter(|&o| o != 0).map(|o| o as usize);
        let minor = u16_at(d, 2).unwrap_or(0);
        Gdef {
            classes: off(4),
            mark_attach: off(10),
            mark_sets: if minor >= 2 { off(12) } else { None },
        }
    }

    /// Whether the font classifies glyphs (base, ligature, mark).
    pub fn has_classes(&self) -> bool {
        self.classes.is_some()
    }

    pub fn glyph_class(&self, d: &[u8], g: u16) -> u16 {
        self.classes.map_or(0, |o| class_of(d, o, g))
    }

    pub fn mark_attach_class(&self, d: &[u8], g: u16) -> u16 {
        self.mark_attach.map_or(0, |o| class_of(d, o, g))
    }

    pub fn in_mark_set(&self, d: &[u8], set: u16, g: u16) -> bool {
        let Some(sets) = self.mark_sets else { return false };
        if u16_at(d, sets) != Some(1) || set >= u16_at(d, sets + 2).unwrap_or(0) {
            return false;
        }
        u32_at(d, sets + 4 + 4 * set as usize).is_some_and(|o| coverage(d, sets + o as usize, g).is_some())
    }
}

/// Lookup flags.
pub const IGNORE_BASE_GLYPHS: u16 = 0x0002;
pub const IGNORE_LIGATURES: u16 = 0x0004;
pub const IGNORE_MARKS: u16 = 0x0008;
pub const USE_MARK_FILTERING_SET: u16 = 0x0010;
pub const MARK_ATTACHMENT_TYPE: u16 = 0xFF00;

/// Whether a lookup with `flags` (and mark filtering set `set`) skips `g`.
pub fn ignores(gdef: &Gdef, d: &[u8], flags: u16, set: u16, g: u16) -> bool {
    if flags
        & (IGNORE_BASE_GLYPHS
            | IGNORE_LIGATURES
            | IGNORE_MARKS
            | USE_MARK_FILTERING_SET
            | MARK_ATTACHMENT_TYPE)
        == 0
    {
        return false;
    }
    match gdef.glyph_class(d, g) {
        BASE => flags & IGNORE_BASE_GLYPHS != 0,
        LIGATURE => flags & IGNORE_LIGATURES != 0,
        MARK => {
            if flags & IGNORE_MARKS != 0 {
                true
            } else if flags & USE_MARK_FILTERING_SET != 0 {
                !gdef.in_mark_set(d, set, g)
            } else if flags & MARK_ATTACHMENT_TYPE != 0 {
                gdef.mark_attach_class(d, g) != flags >> 8
            } else {
                false
            }
        }
        _ => false,
    }
}
