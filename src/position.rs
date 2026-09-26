//! Glyph positioning from the OpenType GPOS table with every lookup type:
//! single and pair adjustment, cursive attachment, mark-to-base,
//! mark-to-ligature and mark-to-mark attachment, and contextual and
//! chained contextual positioning, as HarfBuzz applies them.
//!
//! This is what right-to-left text and fonts with cursive attachment or
//! contextual positioning (such as Arabic Naskh fonts) need. It works on a
//! whole run at once, in logical order, and produces HarfBuzz's positions:
//! for left-to-right text, glyphs are drawn at the pen plus their offset
//! and the pen then moves by their advance; for right-to-left text the
//! same holds with the glyph order reversed. Positions are in font units.
//! Simpler text keeps the pairwise kerning of [`crate::kern`], which is
//! the same for fonts that only kern and attach marks.

use crate::gpos::{anchor, attach, Attachment};
use crate::gsub::Script;
use crate::otl::{self, coverage, i16_at, u16_at, u32_at, Budget, Gdef, Rule, Scripts, Step};

/// The position of a glyph, in font units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub x_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
    /// Relative index of the glyph this one is attached to, or 0.
    chain: i32,
    cursive: bool,
}

impl Pos {
    /// A glyph's starting position: just its advance.
    pub fn advance(x_advance: i32) -> Pos {
        Pos {
            x_advance,
            ..Pos::default()
        }
    }
}

/// GPOS features applied by default, as in HarfBuzz for horizontal text.
const FEATURES: [&[u8; 4]; 7] = [b"abvm", b"blwm", b"mark", b"mkmk", b"curs", b"dist", b"kern"];

/// Nested lookups deeper than this are not applied.
const MAX_NESTING: usize = 8;

/// Longest input sequence a contextual rule may match.
const MAX_CONTEXT: usize = 64;

/// Upper bound on the records read while parsing.
const BUDGET: usize = 200_000;

/// Lookup flag: the last glyph of a cursive chain stays on the baseline.
const RIGHT_TO_LEFT: u16 = 0x0001;

struct Lookup {
    kind: u16,
    flags: u16,
    mark_set: u16,
    subtables: Vec<usize>,
}

/// The GPOS lookups of a face and which apply to each script.
#[derive(Default)]
pub struct Positioning {
    lookups: Vec<Option<Lookup>>,
    plans: [Vec<u16>; Script::ALL.len()],
    /// Per script: whether a plan has lookups that pairwise kerning and
    /// mark attachment cannot reproduce (cursive or contextual ones).
    complex: [bool; Script::ALL.len()],
}

impl Positioning {
    pub fn parse(gpos: Option<&[u8]>) -> Positioning {
        gpos.and_then(try_parse).unwrap_or_default()
    }

    /// Whether text in `script` needs this module rather than pairwise
    /// kerning and mark attachment.
    pub fn is_complex(&self, script: Script) -> bool {
        self.complex[script as usize]
    }

    /// Whether the plan for `script` has any lookups.
    pub fn has(&self, script: Script) -> bool {
        !self.plans[script as usize].is_empty()
    }

    /// Apply the lookups for `script` to `glyphs`, adjusting `pos` (which
    /// starts with the glyphs' advances).
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
        rtl: bool,
        pos: &mut [Pos],
    ) {
        let mut cx = Ctx {
            t: gpos,
            gdef_data,
            gdef,
            p: self,
            glyphs,
            components,
            is_mark,
            rtl,
            pos,
            ops: 0,
            max_ops: glyphs.len().saturating_mul(256).saturating_add(4096),
        };
        for &li in &self.plans[script as usize] {
            cx.apply_lookup(li);
        }
    }
}

fn try_parse(t: &[u8]) -> Option<Positioning> {
    let mut budget = Budget(BUDGET);
    let mut plans: [Vec<u16>; Script::ALL.len()] = Default::default();
    for s in Script::ALL {
        plans[s as usize] =
            otl::feature_lookups(t, Scripts::First(s.tags()), &FEATURES, &mut budget).unwrap_or_default();
    }
    let list = u16_at(t, 8)? as usize;
    let n = budget.take(u16_at(t, list)? as usize)?;
    let mut lookups: Vec<Option<Lookup>> = (0..n).map(|_| None).collect();
    // The lookups of the plans, and those their contextual rules call.
    let mut wanted: Vec<u16> = plans.iter().flatten().copied().collect();
    let mut k = 0;
    while k < wanted.len() {
        let i = wanted[k] as usize;
        k += 1;
        if lookups.get(i).is_none_or(|l| l.is_some()) {
            continue;
        }
        let Some(l) = parse_lookup(t, list, i, &mut budget) else {
            continue;
        };
        if matches!(l.kind, 7 | 8) {
            for &st in &l.subtables {
                nested_lookups(t, st, l.kind == 8, &mut wanted, &mut budget);
            }
        }
        lookups[i] = Some(l);
    }
    for plan in &mut plans {
        plan.retain(|&i| lookups.get(i as usize).is_some_and(Option::is_some));
    }
    let mut complex = [false; Script::ALL.len()];
    for s in Script::ALL {
        complex[s as usize] = plans[s as usize].iter().any(|&i| {
            let l = lookups[i as usize].as_ref().expect("kept lookups are parsed");
            matches!(l.kind, 1 | 3 | 7 | 8)
        });
    }
    Some(Positioning {
        lookups,
        plans,
        complex,
    })
}

/// Add the lookups that the rules of a contextual subtable call to `out`.
fn nested_lookups(t: &[u8], st: usize, chain: bool, out: &mut Vec<u16>, budget: &mut Budget) {
    // Read the lookup records of every rule.
    fn add(t: &[u8], records: usize, n: usize, out: &mut Vec<u16>, budget: &mut Budget) {
        if budget.take(n).is_none() {
            return;
        }
        for r in 0..n {
            if let Some(li) = u16_at(t, records + 4 * r + 2) {
                if !out.contains(&li) {
                    out.push(li);
                }
            }
        }
    }
    let Some(format) = u16_at(t, st) else { return };
    let rule = |r: usize| -> Option<(usize, usize)> {
        if chain {
            let nb = u16_at(t, r)? as usize;
            let ni = u16_at(t, r + 2 + 2 * nb)? as usize;
            let at_la = r + 4 + 2 * nb + 2 * ni.checked_sub(1)?;
            let nla = u16_at(t, at_la)? as usize;
            let at_rec = at_la + 2 + 2 * nla;
            Some((at_rec + 2, u16_at(t, at_rec)? as usize))
        } else {
            let ni = u16_at(t, r)? as usize;
            Some((r + 4 + 2 * ni.checked_sub(1)?, u16_at(t, r + 2)? as usize))
        }
    };
    match format {
        1 | 2 => {
            let sets_at = match (format, chain) {
                (1, _) => st + 4,
                (_, true) => st + 10,
                (_, false) => st + 6,
            };
            let nsets = u16_at(t, sets_at).unwrap_or(0) as usize;
            if budget.take(nsets).is_none() {
                return;
            }
            for k in 0..nsets {
                let Some(o) = u16_at(t, sets_at + 2 + 2 * k).filter(|&o| o != 0) else {
                    continue;
                };
                let set = st + o as usize;
                let n = u16_at(t, set).unwrap_or(0) as usize;
                if budget.take(n).is_none() {
                    return;
                }
                for r in 0..n {
                    if let Some((rec, nrec)) = u16_at(t, set + 2 + 2 * r).and_then(|o| rule(set + o as usize))
                    {
                        add(t, rec, nrec, out, budget);
                    }
                }
            }
        }
        3 => {
            let found = if chain {
                (|| {
                    let nb = u16_at(t, st + 2)? as usize;
                    let at_i = st + 4 + 2 * nb;
                    let ni = u16_at(t, at_i)? as usize;
                    let at_la = at_i + 2 + 2 * ni;
                    let nla = u16_at(t, at_la)? as usize;
                    let at_rec = at_la + 2 + 2 * nla;
                    Some((at_rec + 2, u16_at(t, at_rec)? as usize))
                })()
            } else {
                (|| {
                    let ni = u16_at(t, st + 2)? as usize;
                    Some((st + 6 + 2 * ni, u16_at(t, st + 4)? as usize))
                })()
            };
            if let Some((rec, n)) = found {
                add(t, rec, n, out, budget);
            }
        }
        _ => {}
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
        if kind == 9 {
            let (Some(1), Some(inner), Some(off)) = (u16_at(t, st), u16_at(t, st + 2), u32_at(t, st + 4))
            else {
                continue;
            };
            if inner == 9 || *ext_kind.get_or_insert(inner) != inner {
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
    if !(1..=8).contains(&kind) || subtables.is_empty() {
        return None;
    }
    Some(Lookup {
        kind,
        flags,
        mark_set,
        subtables,
    })
}

/// Size in bytes of a ValueRecord with this format.
fn value_size(format: u16) -> usize {
    2 * (format & 0xFF).count_ones() as usize
}

/// Apply a ValueRecord (placement and horizontal advance; device tables
/// are ignored) to a position.
fn apply_value(t: &[u8], at: usize, format: u16, p: &mut Pos) {
    let field = |bit: u16| -> i32 {
        if format & bit == 0 {
            return 0;
        }
        i16_at(t, at + 2 * (format & (bit - 1)).count_ones() as usize).unwrap_or(0) as i32
    };
    p.x_offset += field(0x0001);
    p.y_offset += field(0x0002);
    p.x_advance += field(0x0004);
}

struct Ctx<'a> {
    t: &'a [u8],
    gdef_data: &'a [u8],
    gdef: &'a Gdef,
    p: &'a Positioning,
    glyphs: &'a [u16],
    components: &'a [u8],
    is_mark: &'a dyn Fn(usize) -> bool,
    rtl: bool,
    pos: &'a mut [Pos],
    ops: usize,
    max_ops: usize,
}

impl<'a> Ctx<'a> {
    fn lookup(&self, li: u16) -> Option<&'a Lookup> {
        let p: &'a Positioning = self.p;
        p.lookups.get(li as usize)?.as_ref()
    }

    fn skips(&self, l: &Lookup, g: u16) -> bool {
        otl::ignores(self.gdef, self.gdef_data, l.flags, l.mark_set, g)
    }

    fn next(&self, l: &Lookup, mut i: usize) -> Option<usize> {
        loop {
            i += 1;
            if !self.skips(l, *self.glyphs.get(i)?) {
                return Some(i);
            }
        }
    }

    fn prev(&self, l: &Lookup, mut i: usize) -> Option<usize> {
        loop {
            i = i.checked_sub(1)?;
            if !self.skips(l, self.glyphs[i]) {
                return Some(i);
            }
        }
    }

    fn spend(&mut self) -> bool {
        self.ops += 1;
        self.ops <= self.max_ops
    }

    fn apply_lookup(&mut self, li: u16) {
        let Some(l) = self.lookup(li) else { return };
        let mut i = 0;
        while i < self.glyphs.len() {
            if !self.skips(l, self.glyphs[i]) {
                if !self.spend() {
                    return;
                }
                if let Some(next) = self.apply_at(li, i, 0) {
                    i = next.max(i + 1);
                    continue;
                }
            }
            i += 1;
        }
    }

    /// Apply lookup `li` at glyph `i` (the first subtable that applies).
    /// Returns where processing continues, or `None` if nothing applied.
    fn apply_at(&mut self, li: u16, i: usize, depth: usize) -> Option<usize> {
        let l = self.lookup(li)?;
        for &st in &l.subtables {
            let applied = match l.kind {
                1 => self.single(st, i),
                2 => self.pair(l, st, i),
                3 => self.cursive(l, st, i),
                4..=6 => self.mark(l, st, i),
                7 | 8 => {
                    let g = self.glyphs[i];
                    otl::context_rules(self.t, st, l.kind == 8, g, |rule| {
                        if let Some(end) = self.try_rule(l, rule, i, depth) {
                            Step::Done(end)
                        } else if self.spend() {
                            Step::Next
                        } else {
                            Step::Stop
                        }
                    })
                }
                _ => None,
            };
            if applied.is_some() {
                return applied;
            }
        }
        None
    }

    fn single(&mut self, st: usize, i: usize) -> Option<usize> {
        let t = self.t;
        let cov = coverage(t, st + u16_at(t, st + 2)? as usize, self.glyphs[i])?;
        let format = u16_at(t, st + 4)?;
        let at = match u16_at(t, st)? {
            1 => st + 6,
            2 => {
                if cov >= u16_at(t, st + 6)? as usize {
                    return None;
                }
                st + 8 + cov * value_size(format)
            }
            _ => return None,
        };
        apply_value(t, at, format, &mut self.pos[i]);
        Some(i + 1)
    }

    fn pair(&mut self, l: &Lookup, st: usize, i: usize) -> Option<usize> {
        let t = self.t;
        let cov = coverage(t, st + u16_at(t, st + 2)? as usize, self.glyphs[i])?;
        let (f1, f2) = (u16_at(t, st + 4)?, u16_at(t, st + 6)?);
        let j = self.next(l, i)?;
        let second = self.glyphs[j];
        let at = match u16_at(t, st)? {
            1 => {
                if cov >= u16_at(t, st + 8)? as usize {
                    return None;
                }
                let set = st + u16_at(t, st + 10 + 2 * cov)? as usize;
                let n = u16_at(t, set)? as usize;
                let rec = 2 + value_size(f1) + value_size(f2);
                let (mut lo, mut hi) = (0, n);
                loop {
                    if lo >= hi {
                        return None;
                    }
                    let mid = (lo + hi) / 2;
                    let r = set + 2 + rec * mid;
                    match u16_at(t, r)?.cmp(&second) {
                        std::cmp::Ordering::Equal => break r + 2,
                        std::cmp::Ordering::Less => lo = mid + 1,
                        std::cmp::Ordering::Greater => hi = mid,
                    }
                }
            }
            2 => {
                let (cd1, cd2) = (
                    st + u16_at(t, st + 8)? as usize,
                    st + u16_at(t, st + 10)? as usize,
                );
                let (n1, n2) = (u16_at(t, st + 12)?, u16_at(t, st + 14)?);
                let (c1, c2) = (
                    otl::class_of(t, cd1, self.glyphs[i]),
                    otl::class_of(t, cd2, second),
                );
                if c1 >= n1 || c2 >= n2 {
                    return None;
                }
                st + 16 + (c1 as usize * n2 as usize + c2 as usize) * (value_size(f1) + value_size(f2))
            }
            _ => return None,
        };
        apply_value(t, at, f1, &mut self.pos[i]);
        apply_value(t, at + value_size(f1), f2, &mut self.pos[j]);
        Some(if f2 != 0 { j + 1 } else { j })
    }

    /// Cursive attachment: the exit anchor of the previous glyph meets the
    /// entry anchor of this one (HarfBuzz's CursivePosFormat1).
    fn cursive(&mut self, l: &Lookup, st: usize, j: usize) -> Option<usize> {
        let t = self.t;
        if u16_at(t, st)? != 1 {
            return None;
        }
        let cov_at = st + u16_at(t, st + 2)? as usize;
        let n = u16_at(t, st + 4)? as usize;
        let record = |g: u16, which: usize| -> Option<(i32, i32)> {
            let k = coverage(t, cov_at, g)?;
            if k >= n {
                return None;
            }
            let off = u16_at(t, st + 6 + 4 * k + which)?;
            if off == 0 {
                return None;
            }
            anchor(t, st + off as usize)
        };
        let (entry_x, entry_y) = record(self.glyphs[j], 0)?;
        let i = self.prev(l, j)?;
        let (exit_x, exit_y) = record(self.glyphs[i], 2)?;
        let pos = &mut *self.pos;
        if self.rtl {
            let d = exit_x + pos[i].x_offset;
            pos[i].x_advance -= d;
            pos[i].x_offset -= d;
            pos[j].x_advance = entry_x + pos[j].x_offset;
        } else {
            pos[i].x_advance = exit_x + pos[i].x_offset;
            let d = entry_x + pos[j].x_offset;
            pos[j].x_advance -= d;
            pos[j].x_offset -= d;
        }
        // The child hangs off the parent: the later glyph off the earlier
        // one, unless the lookup says right-to-left.
        let (mut child, mut parent) = (i, j);
        let mut y_offset = entry_y - exit_y;
        if l.flags & RIGHT_TO_LEFT == 0 {
            std::mem::swap(&mut child, &mut parent);
            y_offset = -y_offset;
        }
        reverse_cursive_chain(pos, child, parent, 0);
        pos[child].cursive = true;
        pos[child].chain = parent as i32 - child as i32;
        pos[child].y_offset = y_offset;
        if pos[parent].chain == -pos[child].chain {
            pos[parent].chain = 0;
            pos[parent].y_offset = 0;
        }
        Some(j + 1)
    }

    fn mark(&mut self, l: &Lookup, st: usize, i: usize) -> Option<usize> {
        let skips = |g: u16| self.skips(l, g);
        let a: Attachment = attach(
            self.t,
            l.kind,
            st,
            self.glyphs,
            self.components,
            i,
            self.is_mark,
            &skips,
        )?;
        let p = &mut self.pos[i];
        p.x_offset = a.dx;
        p.y_offset = a.dy;
        p.chain = a.parent as i32 - i as i32;
        p.cursive = false;
        Some(i + 1)
    }

    fn try_rule(&mut self, l: &Lookup, rule: &Rule, i: usize, depth: usize) -> Option<usize> {
        let t = self.t;
        if rule.input.len() + 1 > MAX_CONTEXT {
            return None;
        }
        let mut pos = Vec::with_capacity(rule.input.len() + 1);
        pos.push(i);
        for k in 0..rule.input.len() {
            let j = self.next(l, pos[k])?;
            if !rule.input.matches(t, k, self.glyphs[j]) {
                return None;
            }
            pos.push(j);
        }
        if let Some(b) = rule.backtrack {
            let mut j = i;
            for k in 0..b.len() {
                j = self.prev(l, j)?;
                if !b.matches(t, k, self.glyphs[j]) {
                    return None;
                }
            }
        }
        if let Some(la) = rule.lookahead {
            let mut j = pos[pos.len() - 1];
            for k in 0..la.len() {
                j = self.next(l, j)?;
                if !la.matches(t, k, self.glyphs[j]) {
                    return None;
                }
            }
        }
        if depth < MAX_NESTING {
            for r in 0..rule.nrecords {
                let rec = rule.records + 4 * r;
                let (Some(idx), Some(li)) = (u16_at(t, rec), u16_at(t, rec + 2)) else {
                    break;
                };
                let Some(&at) = pos.get(idx as usize) else {
                    continue;
                };
                if !self.spend() {
                    break;
                }
                let _ = self.apply_at(li, at, depth + 1);
            }
        }
        Some(pos[pos.len() - 1] + 1)
    }
}

/// When a glyph already in a cursive chain gets a new parent, turn its old
/// chain around so the whole connected tree hangs off the new parent.
fn reverse_cursive_chain(pos: &mut [Pos], i: usize, new_parent: usize, depth: usize) {
    let chain = pos[i].chain;
    if chain == 0 || !pos[i].cursive || depth > MAX_CONTEXT {
        return;
    }
    pos[i].chain = 0;
    let Some(j) = i.checked_add_signed(chain as isize).filter(|&j| j < pos.len()) else {
        return;
    };
    if j == new_parent {
        return;
    }
    reverse_cursive_chain(pos, j, new_parent, depth + 1);
    pos[j].y_offset = -pos[i].y_offset;
    pos[j].chain = -chain;
    pos[j].cursive = true;
}

/// Attach marks by `attachments` (from the outlines, for fonts without
/// mark positioning), as mark attachment lookups would.
pub fn attach_fallback(pos: &mut [Pos], attachments: &[Option<Attachment>]) {
    for (i, a) in attachments.iter().enumerate() {
        if let Some(a) = a.filter(|a| a.parent < i) {
            pos[i].x_offset = a.dx;
            pos[i].y_offset = a.dy;
            pos[i].chain = a.parent as i32 - i as i32;
            pos[i].cursive = false;
        }
    }
}

/// Finish positioning, as HarfBuzz does after GPOS: marks lose their
/// advance, and attached glyphs get their offsets from the glyphs they
/// hang on (marks relative to their base, cursive chains accumulating
/// their vertical offsets).
pub fn finish(pos: &mut [Pos], is_mark: &dyn Fn(usize) -> bool, rtl: bool) {
    for (i, p) in pos.iter_mut().enumerate() {
        if is_mark(i) {
            p.x_advance = 0;
        }
    }
    for i in 0..pos.len() {
        propagate(pos, i, rtl, 0);
    }
}

fn propagate(pos: &mut [Pos], i: usize, rtl: bool, depth: usize) {
    let chain = pos[i].chain;
    if chain == 0 || depth > MAX_CONTEXT {
        return;
    }
    pos[i].chain = 0;
    let Some(j) = i.checked_add_signed(chain as isize).filter(|&j| j < pos.len()) else {
        return;
    };
    propagate(pos, j, rtl, depth + 1);
    if pos[i].cursive {
        pos[i].y_offset += pos[j].y_offset;
        return;
    }
    if j >= i {
        return;
    }
    pos[i].x_offset += pos[j].x_offset;
    pos[i].y_offset += pos[j].y_offset;
    if rtl {
        for k in j + 1..=i {
            pos[i].x_offset += pos[k].x_advance;
        }
    } else {
        for k in j..i {
            pos[i].x_offset -= pos[k].x_advance;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_hang_on_their_base_in_both_directions() {
        // A base 500 wide and a mark attached 100 right of its origin.
        let mut pos = [
            Pos {
                x_advance: 500,
                ..Pos::default()
            },
            Pos {
                x_advance: 300,
                ..Pos::default()
            },
        ];
        let a = [
            None,
            Some(Attachment {
                parent: 0,
                dx: 100,
                dy: 50,
            }),
        ];
        let mark = |i: usize| i == 1;
        let mut ltr = pos;
        attach_fallback(&mut ltr, &a);
        finish(&mut ltr, &mark, false);
        // Left to right, the pen is past the base when the mark is drawn.
        assert_eq!(
            (ltr[1].x_advance, ltr[1].x_offset, ltr[1].y_offset),
            (0, -400, 50)
        );
        attach_fallback(&mut pos, &a);
        finish(&mut pos, &mark, true);
        // Right to left, the mark is drawn first, at the base's pen.
        assert_eq!((pos[1].x_advance, pos[1].x_offset, pos[1].y_offset), (0, 100, 50));
    }
}
