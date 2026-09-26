//! Pair kerning from the OpenType GPOS `kern` feature (pair adjustment
//! lookups, also inside extension lookups), with the legacy `kern` table as
//! a fallback for fonts without GPOS kerning.
//!
//! Only the horizontal advance between two adjacent glyphs is adjusted;
//! contextual and vertical positioning are out of scope. Everything is read
//! directly from the font bytes with bounds checks, so malformed tables just
//! mean no kerning.

use crate::otl::{self, class_of, coverage, i16_at, u16_at, u32_at, Budget, Gdef, Scripts};

/// Upper bound on the records read while collecting the kerning lookups.
const BUDGET: usize = 100_000;

/// Offsets of the pair-positioning subtables of the `kern` lookups, relative
/// to the start of the GPOS table.
#[derive(Default)]
pub struct Kerning {
    /// Flags, mark filtering set and subtables of each lookup.
    lookups: Vec<(u16, u16, Vec<usize>)>,
    gdef: Gdef,
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
            k.gdef = Gdef::parse(gdef);
        } else if let Some(t) = kern {
            k.legacy = parse_legacy(t);
        }
        k
    }

    pub fn is_empty(&self) -> bool {
        self.lookups.is_empty() && self.legacy.is_none()
    }

    /// Adjustment of the advance between `left` and `right`, in font units.
    ///
    /// As OpenType specifies, every lookup of the feature is applied in
    /// lookup-list order and their adjustments accumulate (HarfBuzz does the
    /// same); within one lookup only the first subtable that matches the
    /// pair applies. Fonts that split kerning across lookups therefore get
    /// the sum, and fonts with the same pair in several lookups get what the
    /// font designer specified, not a single value.
    pub fn pair(&self, gpos: &[u8], gdef: &[u8], kern: &[u8], left: u16, right: u16) -> i32 {
        if let Some((off, n)) = self.legacy {
            return legacy_pair(kern, off, n, left, right).unwrap_or(0) as i32;
        }
        let mut total = 0i32;
        for (flags, set, subtables) in &self.lookups {
            let skip = |g: u16| otl::ignores(&self.gdef, gdef, *flags, *set, g);
            if skip(left) || skip(right) {
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

fn parse_gpos(g: &[u8]) -> Option<Vec<(u16, u16, Vec<usize>)>> {
    let mut budget = Budget(BUDGET);
    let lookup_ids = otl::feature_lookups(g, Scripts::All, &[b"kern"], &mut budget)?;
    let lookup_list = u16_at(g, 8)? as usize;
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
            budget.take(u16_at(g, lookup + 4)? as usize)?,
        );
        let set = if flags & otl::USE_MARK_FILTERING_SET != 0 {
            u16_at(g, lookup + 6 + 2 * n).unwrap_or(0)
        } else {
            0
        };
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
            out.push((flags, set, subtables));
        }
    }
    Some(out)
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
