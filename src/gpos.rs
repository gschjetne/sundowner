//! Mark positioning from the OpenType GPOS `mark` and `mkmk` features:
//! attaching combining marks to base glyphs (mark-to-base), to ligatures
//! (mark-to-ligature) and to other marks (mark-to-mark) by their anchors.
//!
//! The result is, for each attached mark, the glyph it hangs on and the
//! offset of the mark's origin from that glyph's origin. Turning that into
//! positions (with zero-width marks) is up to the caller. As with GSUB,
//! features come from the default language system of the text's script,
//! and every read is bounds-checked.

use crate::gsub::Script;
use crate::otl::{self, coverage, i16_at, u16_at, u32_at, Budget, Gdef, Scripts};

/// A mark's attachment: the index of the glyph it attaches to and the
/// offset of the mark's origin from that glyph's origin, in font units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub parent: usize,
    pub dx: i32,
    pub dy: i32,
}

struct Lookup {
    kind: u16,
    flags: u16,
    mark_set: u16,
    subtables: Vec<usize>,
}

#[derive(Default)]
pub struct MarkPositioning {
    lookups: Vec<Option<Lookup>>,
    plans: [Vec<u16>; Script::ALL.len()],
}

const BUDGET: usize = 100_000;

impl MarkPositioning {
    pub fn parse(gpos: Option<&[u8]>) -> MarkPositioning {
        gpos.and_then(try_parse).unwrap_or_default()
    }

    /// Whether the font positions marks for text in `script`.
    pub fn has(&self, script: Script) -> bool {
        !self.plans[script as usize].is_empty()
    }

    /// Attach the marks among `glyphs`. `is_mark` says which glyphs are
    /// marks; `gpos` and `gdef` are the table data.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        gpos: &[u8],
        gdef_data: &[u8],
        gdef: &Gdef,
        script: Script,
        glyphs: &[u16],
        components: &[u8],
        is_mark: &dyn Fn(usize) -> bool,
    ) -> Vec<Option<Attachment>> {
        let mut out = vec![None; glyphs.len()];
        for &li in &self.plans[script as usize] {
            let Some(Some(l)) = self.lookups.get(li as usize) else {
                continue;
            };
            let skips = |g: u16| otl::ignores(gdef, gdef_data, l.flags, l.mark_set, g);
            for i in 0..glyphs.len() {
                if skips(glyphs[i]) {
                    continue;
                }
                for &st in &l.subtables {
                    if let Some(a) = attach(gpos, l.kind, st, glyphs, components, i, is_mark, &skips) {
                        out[i] = Some(a);
                        break;
                    }
                }
            }
        }
        out
    }
}

fn try_parse(t: &[u8]) -> Option<MarkPositioning> {
    let mut budget = Budget(BUDGET);
    let mut plans: [Vec<u16>; Script::ALL.len()] = Default::default();
    // A malformed or budget-exhausting script section only loses that
    // script; Latin comes first so it survives problems in later ones.
    for s in Script::ALL {
        plans[s as usize] =
            otl::feature_lookups(t, Scripts::First(s.tags()), &[b"mark", b"mkmk"], &mut budget)
                .unwrap_or_default();
    }
    let list = u16_at(t, 8)? as usize;
    let n = budget.take(u16_at(t, list)? as usize)?;
    let mut lookups: Vec<Option<Lookup>> = (0..n).map(|_| None).collect();
    let mut wanted: Vec<u16> = plans.iter().flatten().copied().collect();
    wanted.sort_unstable();
    wanted.dedup();
    for i in wanted {
        if let Some(slot) = lookups.get_mut(i as usize) {
            *slot = parse_lookup(t, list, i as usize, &mut budget);
        }
    }
    for plan in &mut plans {
        plan.retain(|&i| lookups.get(i as usize).is_some_and(Option::is_some));
    }
    Some(MarkPositioning { lookups, plans })
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
    let mut subtables = Vec::new();
    let mut ext_kind = None;
    for k in 0..n {
        let st = at + u16_at(t, at + 6 + 2 * k)? as usize;
        if kind == 9 {
            let (Some(1), Some(inner), Some(off)) = (u16_at(t, st), u16_at(t, st + 2), u32_at(t, st + 4))
            else {
                continue;
            };
            if *ext_kind.get_or_insert(inner) != inner {
                continue;
            }
            subtables.push(st + off as usize);
        } else {
            subtables.push(st);
        }
    }
    if kind == 9 {
        kind = ext_kind?;
    }
    if !(4..=6).contains(&kind) || subtables.is_empty() {
        return None;
    }
    Some(Lookup {
        kind,
        flags,
        mark_set,
        subtables,
    })
}

pub(crate) fn anchor(t: &[u8], at: usize) -> Option<(i32, i32)> {
    if !(1..=3).contains(&u16_at(t, at)?) {
        return None;
    }
    Some((i16_at(t, at + 2)? as i32, i16_at(t, at + 4)? as i32))
}

/// Try one mark attachment subtable for the mark at `i`.
///
/// `components` numbers the glyphs that a multiple substitution produced
/// (1, 2, ...; 0 for other glyphs): as in HarfBuzz, a mark goes on the
/// first glyph of such a sequence rather than on a later one that the
/// lookup does not cover.
#[allow(clippy::too_many_arguments)]
pub(crate) fn attach(
    t: &[u8],
    kind: u16,
    st: usize,
    glyphs: &[u16],
    components: &[u8],
    i: usize,
    is_mark: &dyn Fn(usize) -> bool,
    skips: &dyn Fn(u16) -> bool,
) -> Option<Attachment> {
    if u16_at(t, st)? != 1 {
        return None;
    }
    let mark_index = coverage(t, st + u16_at(t, st + 2)? as usize, glyphs[i])?;
    let classes = u16_at(t, st + 6)? as usize;
    let mark_array = st + u16_at(t, st + 8)? as usize;
    if mark_index >= u16_at(t, mark_array)? as usize {
        return None;
    }
    let record = mark_array + 2 + 4 * mark_index;
    let class = u16_at(t, record)? as usize;
    let mark_anchor = anchor(t, mark_array + u16_at(t, record + 2)? as usize)?;
    if class >= classes {
        return None;
    }
    let second_cov = st + u16_at(t, st + 4)? as usize;
    let second_array = st + u16_at(t, st + 10)? as usize;
    let (parent, anchor_at) = match kind {
        // Mark-to-base and mark-to-ligature: the nearest preceding glyph
        // that is not a mark (for mark-to-base, skipping later glyphs of a
        // multiple substitution that the lookup does not cover).
        4 | 5 => {
            let later_component = |j: usize| {
                let c = components.get(j).copied().unwrap_or(0);
                c > 1 && j > 0 && !is_mark(j - 1) && components.get(j - 1).is_some_and(|&p| p + 1 == c)
            };
            let j = (0..i).rev().find(|&j| {
                !is_mark(j)
                    && (kind != 4 || !later_component(j) || coverage(t, second_cov, glyphs[j]).is_some())
            })?;
            let idx = coverage(t, second_cov, glyphs[j])?;
            if idx >= u16_at(t, second_array)? as usize {
                return None;
            }
            if kind == 4 {
                let off = u16_at(t, second_array + 2 + 2 * (idx * classes + class))?;
                (j, (off != 0).then_some(second_array + off as usize)?)
            } else {
                // The mark goes on the last component of the ligature.
                // Which component a mark belonged to before the ligature
                // formed is not tracked, so a mark on an earlier component
                // (an accent on the f of an fi ligature) also lands on the
                // last one.
                let attach = second_array + u16_at(t, second_array + 2 + 2 * idx)? as usize;
                let components = u16_at(t, attach)? as usize;
                let comp = components.checked_sub(1)?;
                let off = u16_at(t, attach + 2 + 2 * (comp * classes + class))?;
                (j, (off != 0).then_some(attach + off as usize)?)
            }
        }
        // Mark-to-mark: the preceding glyph (skipping what the lookup
        // ignores) must be a mark.
        6 => {
            let j = (0..i).rev().find(|&j| !skips(glyphs[j]))?;
            if !is_mark(j) {
                return None;
            }
            let idx = coverage(t, second_cov, glyphs[j])?;
            if idx >= u16_at(t, second_array)? as usize {
                return None;
            }
            let off = u16_at(t, second_array + 2 + 2 * (idx * classes + class))?;
            (j, (off != 0).then_some(second_array + off as usize)?)
        }
        _ => return None,
    };
    let (bx, by) = anchor(t, anchor_at)?;
    Some(Attachment {
        parent,
        dx: bx - mark_anchor.0,
        dy: by - mark_anchor.1,
    })
}
