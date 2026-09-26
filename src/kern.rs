//! Pair kerning from the OpenType GPOS `kern` feature (pair adjustment
//! lookups, also inside extension lookups), with the legacy `kern` table as
//! a fallback for fonts without GPOS kerning.
//!
//! Only the horizontal advance between two adjacent glyphs is adjusted;
//! contextual and vertical positioning are out of scope. Everything is read
//! directly from the font bytes with bounds checks, so malformed tables just
//! mean no kerning.

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(i)?, *d.get(i + 1)?]))
}

fn i16_at(d: &[u8], i: usize) -> Option<i16> {
    u16_at(d, i).map(|v| v as i16)
}

fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

const IGNORE_MARKS: u16 = 0x0008;

/// Upper bound on the records read while collecting the kerning lookups.
/// Real fonts need a few hundred; corrupt counts in a malformed font could
/// otherwise nest into billions of iterations.
const BUDGET: usize = 100_000;

/// Offsets of the pair-positioning subtables of the `kern` lookups, relative
/// to the start of the GPOS table.
#[derive(Default)]
pub struct Kerning {
    lookups: Vec<(u16, Vec<usize>)>,
    /// Offset of the GDEF glyph class definition, relative to GDEF.
    gdef_classes: Option<usize>,
    /// Offset and pair count of a legacy `kern` format 0 subtable.
    legacy: Option<(usize, usize)>,
}

impl Kerning {
    pub fn parse(gpos: Option<&[u8]>, gdef: Option<&[u8]>, kern: Option<&[u8]>) -> Kerning {
        let mut k = Kerning::default();
        if let Some(g) = gpos {
            k.lookups = parse_gpos(g).unwrap_or_default();
        }
        if !k.lookups.is_empty() {
            k.gdef_classes = gdef
                .and_then(|d| u16_at(d, 4))
                .filter(|&o| o != 0)
                .map(|o| o as usize);
        } else if let Some(t) = kern {
            k.legacy = parse_legacy(t);
        }
        k
    }

    pub fn is_empty(&self) -> bool {
        self.lookups.is_empty() && self.legacy.is_none()
    }

    /// Adjustment of the advance between `left` and `right`, in font units.
    pub fn pair(&self, gpos: &[u8], gdef: &[u8], kern: &[u8], left: u16, right: u16) -> i32 {
        if let Some((off, n)) = self.legacy {
            return legacy_pair(kern, off, n, left, right).unwrap_or(0) as i32;
        }
        let is_mark = |g: u16| self.gdef_classes.is_some_and(|o| class_of(gdef, o, g) == 3);
        let mut total = 0i32;
        for (flags, subtables) in &self.lookups {
            if flags & IGNORE_MARKS != 0 && (is_mark(left) || is_mark(right)) {
                continue;
            }
            for &st in subtables {
                if let Some(v) = pair_subtable(gpos, st, left, right) {
                    total += v as i32;
                    break;
                }
            }
        }
        total
    }
}

fn parse_gpos(g: &[u8]) -> Option<Vec<(u16, Vec<usize>)>> {
    let mut budget = BUDGET;
    // Charge `n` records against the budget; stop parsing once it runs out.
    let mut take = |n: usize| -> Option<usize> {
        budget = budget.checked_sub(n)?;
        Some(n)
    };
    let scripts = u16_at(g, 4)? as usize;
    let features = u16_at(g, 6)? as usize;
    let lookup_list = u16_at(g, 8)? as usize;

    // Feature indices of every language system of every script.
    let mut feature_ids: Vec<u16> = Vec::new();
    let nscripts = take(u16_at(g, scripts)? as usize)?;
    for s in 0..nscripts {
        let script = scripts + u16_at(g, scripts + 2 + 6 * s + 4)? as usize;
        let mut langsys: Vec<usize> = Vec::new();
        if let Some(d) = u16_at(g, script).filter(|&d| d != 0) {
            langsys.push(script + d as usize);
        }
        let nlang = take(u16_at(g, script + 2).unwrap_or(0) as usize)?;
        for l in 0..nlang {
            if let Some(o) = u16_at(g, script + 4 + 6 * l + 4) {
                langsys.push(script + o as usize);
            }
        }
        for ls in langsys {
            let n = take(u16_at(g, ls + 4).unwrap_or(0) as usize)?;
            feature_ids.extend((0..n).filter_map(|i| u16_at(g, ls + 6 + 2 * i)));
        }
    }
    feature_ids.sort_unstable();
    feature_ids.dedup();

    // Lookups of the `kern` features.
    let nfeatures = u16_at(g, features)? as usize;
    let mut lookup_ids: Vec<u16> = Vec::new();
    for f in feature_ids {
        let f = f as usize;
        if f >= nfeatures {
            continue;
        }
        let rec = features + 2 + 6 * f;
        if g.get(rec..rec + 4) != Some(b"kern") {
            continue;
        }
        let feature = features + u16_at(g, rec + 4)? as usize;
        let n = take(u16_at(g, feature + 2).unwrap_or(0) as usize)?;
        lookup_ids.extend((0..n).filter_map(|i| u16_at(g, feature + 4 + 2 * i)));
    }
    lookup_ids.sort_unstable();
    lookup_ids.dedup();

    let nlookups = u16_at(g, lookup_list)? as usize;
    let mut out = Vec::new();
    for id in lookup_ids {
        let id = id as usize;
        if id >= nlookups {
            continue;
        }
        let lookup = lookup_list + u16_at(g, lookup_list + 2 + 2 * id)? as usize;
        let (kind, flags, n) = (
            u16_at(g, lookup)?,
            u16_at(g, lookup + 2)?,
            take(u16_at(g, lookup + 4)? as usize)?,
        );
        let mut subtables = Vec::new();
        for i in 0..n {
            let st = lookup + u16_at(g, lookup + 6 + 2 * i)? as usize;
            match kind {
                2 => subtables.push(st),
                // Extension lookup wrapping a pair adjustment subtable.
                9 if u16_at(g, st + 2) == Some(2) => subtables.push(st + u32_at(g, st + 4)? as usize),
                _ => {}
            }
        }
        if !subtables.is_empty() {
            out.push((flags, subtables));
        }
    }
    Some(out)
}

/// Index of `g` in a coverage table, if covered.
fn coverage(d: &[u8], at: usize, g: u16) -> Option<usize> {
    let n = u16_at(d, at + 2)? as usize;
    match u16_at(d, at)? {
        1 => {
            let (mut lo, mut hi) = (0, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let v = u16_at(d, at + 4 + 2 * mid)?;
                match v.cmp(&g) {
                    std::cmp::Ordering::Equal => return Some(mid),
                    std::cmp::Ordering::Less => lo = mid + 1,
                    std::cmp::Ordering::Greater => hi = mid,
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

/// Class of `g` in a class definition table (0 when not listed).
fn class_of(d: &[u8], at: usize, g: u16) -> u16 {
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

/// Size in bytes of a ValueRecord with this format.
fn value_size(format: u16) -> usize {
    2 * (format & 0xFF).count_ones() as usize
}

/// Horizontal adjustment from a pair of ValueRecords: the first glyph's
/// XAdvance plus the second glyph's XPlacement.
fn value(d: &[u8], at: usize, f1: u16, f2: u16) -> i16 {
    let field = |base: usize, format: u16, bit: u16| -> i16 {
        if format & bit == 0 {
            return 0;
        }
        i16_at(d, base + 2 * (format & (bit - 1)).count_ones() as usize).unwrap_or(0)
    };
    let adv = field(at, f1, 0x0004);
    let place = field(at + value_size(f1), f2, 0x0001);
    adv.saturating_add(place)
}

/// Adjustment from one pair positioning subtable, or None if it does not
/// apply to this pair (so the next subtable is tried).
fn pair_subtable(d: &[u8], st: usize, left: u16, right: u16) -> Option<i16> {
    let format = u16_at(d, st)?;
    let cov = coverage(d, st + u16_at(d, st + 2)? as usize, left)?;
    let (f1, f2) = (u16_at(d, st + 4)?, u16_at(d, st + 6)?);
    match format {
        1 => {
            let sets = u16_at(d, st + 8)? as usize;
            if cov >= sets {
                return None;
            }
            let set = st + u16_at(d, st + 10 + 2 * cov)? as usize;
            let n = u16_at(d, set)? as usize;
            let rec = 2 + value_size(f1) + value_size(f2);
            let (mut lo, mut hi) = (0, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let r = set + 2 + rec * mid;
                let second = u16_at(d, r)?;
                match second.cmp(&right) {
                    std::cmp::Ordering::Equal => return Some(value(d, r + 2, f1, f2)),
                    std::cmp::Ordering::Less => lo = mid + 1,
                    std::cmp::Ordering::Greater => hi = mid,
                }
            }
            None
        }
        2 => {
            let (cd1, cd2) = (
                st + u16_at(d, st + 8)? as usize,
                st + u16_at(d, st + 10)? as usize,
            );
            let (n1, n2) = (u16_at(d, st + 12)?, u16_at(d, st + 14)?);
            let (c1, c2) = (class_of(d, cd1, left), class_of(d, cd2, right));
            if c1 >= n1 || c2 >= n2 {
                return None;
            }
            let rec = value_size(f1) + value_size(f2);
            let at = st + 16 + (c1 as usize * n2 as usize + c2 as usize) * rec;
            Some(value(d, at, f1, f2))
        }
        _ => None,
    }
}

fn parse_legacy(t: &[u8]) -> Option<(usize, usize)> {
    if u16_at(t, 0)? != 0 {
        return None; // Apple's 32-bit version header is not supported
    }
    let n = u16_at(t, 2)? as usize;
    let mut at = 4;
    for _ in 0..n {
        let len = u16_at(t, at + 2)? as usize;
        let coverage = u16_at(t, at + 4)?;
        // Format 0, horizontal, not cross-stream or minimum values.
        if coverage >> 8 == 0 && coverage & 0x0007 == 0x0001 {
            let pairs = u16_at(t, at + 6)? as usize;
            return Some((at + 14, pairs));
        }
        at += len.max(6);
    }
    None
}

fn legacy_pair(t: &[u8], at: usize, n: usize, left: u16, right: u16) -> Option<i16> {
    let key = (left as u32) << 16 | right as u32;
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let r = at + 6 * mid;
        match u32_at(t, r)?.cmp(&key) {
            std::cmp::Ordering::Equal => return i16_at(t, r + 4),
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
        }
    }
    None
}
