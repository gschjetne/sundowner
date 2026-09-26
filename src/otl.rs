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
