//! The Unicode Bidirectional Algorithm (UAX #9), without tailoring.
//!
//! [`resolve`] computes the embedding level of every character of a
//! paragraph: the paragraph level (rules P2 and P3, or given), explicit
//! embeddings, overrides and isolates (X1–X10), weak types (W1–W7), paired
//! brackets (N0), neutrals (N1, N2) and implicit levels (I1, I2).
//! [`reset_whitespace`] applies rule L1 to a line and [`visual_order`]
//! rule L2. Mirroring (L4) is left to the caller through [`mirror`]. The
//! properties come from the Unicode Character Database, see
//! [`bidi_table`](crate::bidi_table); the implementation passes the
//! official conformance tests (`tests/bidi.rs`).

use crate::bidi_table::{self as table, BidiClass, BidiClass::*};

/// Deepest explicit embedding level (BD2).
const MAX_DEPTH: u8 = 125;

/// Most brackets open at once while pairing them (BD16).
const MAX_BRACKETS: usize = 63;

/// The bidirectional class of a character.
pub fn class(c: char) -> BidiClass {
    let cp = c as u32;
    let i = table::RANGES.partition_point(|&r| r >> 5 <= cp) - 1;
    table::BIDI_CLASSES[(table::RANGES[i] & 0x1F) as usize]
}

/// The character that mirrors `c` in right-to-left text, such as `)` for
/// `(` (Bidi_Mirroring_Glyph).
pub fn mirror(c: char) -> Option<char> {
    let i = table::MIRRORS.binary_search_by_key(&(c as u32), |m| m.0).ok()?;
    char::from_u32(table::MIRRORS[i].1)
}

/// The paired-bracket property of a character, for rule N0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bracket {
    None,
    /// An opening bracket; the value identifies the pair.
    Open(u32),
    /// A closing bracket; the value is that of its opening bracket.
    Close(u32),
}

impl Bracket {
    pub fn of(c: char) -> Bracket {
        // Brackets that are canonically equivalent pair with each other.
        let canonical = |cp: u32| match cp {
            0x2329 => 0x3008,
            0x232A => 0x3009,
            _ => cp,
        };
        match table::BRACKETS.binary_search_by_key(&(c as u32), |b| b.0) {
            Ok(i) => {
                let (cp, pair, open) = table::BRACKETS[i];
                if open {
                    Bracket::Open(canonical(cp))
                } else {
                    Bracket::Close(canonical(pair))
                }
            }
            Err(_) => Bracket::None,
        }
    }
}

/// The resolved levels of a paragraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Levels {
    /// The paragraph embedding level: 0 for left-to-right, 1 for
    /// right-to-left.
    pub paragraph: u8,
    /// The level of each character. Characters that rule X9 removes
    /// (explicit embeddings and boundary neutrals) get the level of the
    /// character before them, so they never split a run.
    pub levels: Vec<u8>,
}

impl Levels {
    pub fn rtl(&self) -> bool {
        self.paragraph % 2 == 1
    }
}

/// Resolve the levels of a paragraph. `rtl` gives its direction, or `None`
/// to take it from its first strong character (P2, P3), left-to-right if
/// there is none.
pub fn resolve(text: &[char], rtl: Option<bool>) -> Levels {
    let classes: Vec<BidiClass> = text.iter().map(|&c| class(c)).collect();
    let brackets: Vec<Bracket> = text.iter().map(|&c| Bracket::of(c)).collect();
    resolve_classes(&classes, &brackets, rtl.map(u8::from))
}

/// The direction of the first strong character of `text` (P2, P3), if any:
/// `Some(true)` for right-to-left.
pub fn first_strong(text: &[char]) -> Option<bool> {
    let classes: Vec<BidiClass> = text.iter().map(|&c| class(c)).collect();
    let matching = matching_pdis(&classes);
    first_strong_in(&classes, 0, &matching)
}

/// Whether `text` can have right-to-left parts: whether it has a
/// right-to-left character or an explicit formatting character that can
/// start right-to-left text. Without one, every level is 0.
pub fn needs_resolving(text: &[char]) -> bool {
    text.iter()
        .any(|&c| matches!(class(c), R | AL | RLE | RLO | RLI | FSI))
}

/// Whether rule X9 removes characters of this class.
pub fn removed(c: BidiClass) -> bool {
    matches!(c, RLE | LRE | RLO | LRO | PDF | BN)
}

fn isolate_initiator(c: BidiClass) -> bool {
    matches!(c, LRI | RLI | FSI)
}

/// For each isolate initiator, the index of its matching PDI (BD9).
fn matching_pdis(classes: &[BidiClass]) -> Vec<Option<usize>> {
    let mut out = vec![None; classes.len()];
    let mut open = Vec::new();
    for (i, &c) in classes.iter().enumerate() {
        match c {
            LRI | RLI | FSI => open.push(i),
            PDI => {
                if let Some(o) = open.pop() {
                    out[o] = Some(i);
                }
            }
            B => open.clear(),
            _ => {}
        }
    }
    out
}

/// P2 and P3 from `from` to the end of the paragraph, skipping isolates.
fn first_strong_in(classes: &[BidiClass], from: usize, matching: &[Option<usize>]) -> Option<bool> {
    let mut i = from;
    while i < classes.len() {
        match classes[i] {
            L => return Some(false),
            R | AL => return Some(true),
            B => return None,
            c if isolate_initiator(c) => i = matching[i]?,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The next embedding level above `level` with the given direction.
fn next_level(level: u8, rtl: bool) -> u8 {
    if rtl {
        (level + 1) | 1
    } else {
        (level + 2) & !1
    }
}

fn direction(level: u8) -> BidiClass {
    if level % 2 == 1 {
        R
    } else {
        L
    }
}

/// [`resolve`] on bidirectional classes and brackets; `level` is the
/// paragraph level, or `None` to find it.
pub fn resolve_classes(orig: &[BidiClass], brackets: &[Bracket], level: Option<u8>) -> Levels {
    let n = orig.len();
    let matching = matching_pdis(orig);
    let para = level.unwrap_or_else(|| u8::from(first_strong_in(orig, 0, &matching) == Some(true)));

    // X1–X8: explicit levels and directions.
    #[derive(Clone, Copy)]
    struct Entry {
        level: u8,
        over: Option<BidiClass>,
        isolate: bool,
    }
    let base = Entry {
        level: para,
        over: None,
        isolate: false,
    };
    let mut stack = vec![base];
    let (mut overflow_isolates, mut overflow_embeddings, mut valid_isolates) = (0usize, 0usize, 0usize);
    let mut levels = vec![para; n];
    let mut classes = orig.to_vec();
    for i in 0..n {
        let top = *stack.last().expect("the stack keeps its base entry");
        levels[i] = top.level;
        match orig[i] {
            RLE | LRE | RLO | LRO => {
                let new = next_level(top.level, matches!(orig[i], RLE | RLO));
                if new <= MAX_DEPTH && overflow_isolates == 0 && overflow_embeddings == 0 {
                    stack.push(Entry {
                        level: new,
                        over: match orig[i] {
                            RLO => Some(R),
                            LRO => Some(L),
                            _ => None,
                        },
                        isolate: false,
                    });
                } else if overflow_isolates == 0 {
                    overflow_embeddings += 1;
                }
            }
            RLI | LRI | FSI => {
                if let Some(o) = top.over {
                    classes[i] = o;
                }
                let rtl = match orig[i] {
                    RLI => true,
                    LRI => false,
                    _ => {
                        // Only the text up to the matching PDI counts.
                        let end = matching[i].unwrap_or(n);
                        first_strong_in(&orig[..end], i + 1, &matching) == Some(true)
                    }
                };
                let new = next_level(top.level, rtl);
                if new <= MAX_DEPTH && overflow_isolates == 0 && overflow_embeddings == 0 {
                    valid_isolates += 1;
                    stack.push(Entry {
                        level: new,
                        over: None,
                        isolate: true,
                    });
                } else {
                    overflow_isolates += 1;
                }
            }
            PDI => {
                if overflow_isolates > 0 {
                    overflow_isolates -= 1;
                } else if valid_isolates > 0 {
                    overflow_embeddings = 0;
                    while stack.last().is_some_and(|e| !e.isolate) {
                        stack.pop();
                    }
                    stack.pop();
                    valid_isolates -= 1;
                }
                let top = *stack.last().expect("the base entry is never an isolate");
                levels[i] = top.level;
                if let Some(o) = top.over {
                    classes[i] = o;
                }
            }
            PDF => {
                if overflow_isolates > 0 {
                } else if overflow_embeddings > 0 {
                    overflow_embeddings -= 1;
                } else if !top.isolate && stack.len() >= 2 {
                    stack.pop();
                }
            }
            B => {
                // X8: a paragraph separator ends every embedding.
                levels[i] = para;
                stack.truncate(1);
                (overflow_isolates, overflow_embeddings, valid_isolates) = (0, 0, 0);
            }
            BN => {}
            _ => {
                if let Some(o) = top.over {
                    classes[i] = o;
                }
            }
        }
    }

    // X9 and X10: level runs of the remaining characters, joined across
    // isolates into isolating run sequences.
    let kept: Vec<usize> = (0..n).filter(|&i| !removed(orig[i])).collect();
    let mut sequences: Vec<Vec<usize>> = Vec::new();
    // Sequence waiting for the PDI at an index to continue it.
    let mut waiting: Vec<Option<usize>> = vec![None; n];
    let mut k = 0;
    while k < kept.len() {
        let mut end = k + 1;
        while end < kept.len() && levels[kept[end]] == levels[kept[k]] {
            end += 1;
        }
        let run = &kept[k..end];
        let seq = match waiting[run[0]].take() {
            Some(s) if orig[run[0]] == PDI => s,
            _ => {
                sequences.push(Vec::new());
                sequences.len() - 1
            }
        };
        sequences[seq].extend_from_slice(run);
        let last = run[run.len() - 1];
        if isolate_initiator(orig[last]) {
            if let Some(m) = matching[last] {
                waiting[m] = Some(seq);
            }
        }
        k = end;
    }
    // Position of each kept character in `kept`, to find its neighbours.
    let mut pos = vec![usize::MAX; n];
    for (p, &i) in kept.iter().enumerate() {
        pos[i] = p;
    }
    let explicit = levels.clone();
    for seq in &sequences {
        let first = seq[0];
        let last = seq[seq.len() - 1];
        let level = explicit[first];
        let before = match pos[first] {
            0 => para,
            p => explicit[kept[p - 1]],
        };
        let after = match kept.get(pos[last] + 1) {
            Some(&j) if !isolate_initiator(orig[last]) => explicit[j],
            _ => para,
        };
        let sos = direction(level.max(before));
        let eos = direction(level.max(after));
        let mut t: Vec<BidiClass> = seq.iter().map(|&i| classes[i]).collect();
        resolve_weak(&mut t, sos);
        let e = direction(level);
        resolve_brackets(&mut t, seq, orig, brackets, sos, e);
        resolve_neutrals(&mut t, sos, eos, e);
        // I1, I2.
        for (&i, &c) in seq.iter().zip(&t) {
            levels[i] += match (levels[i] % 2, c) {
                (0, R) => 1,
                (0, AN | EN) => 2,
                (1, L | EN | AN) => 1,
                _ => 0,
            };
        }
    }
    // Removed characters take the level of the character before them.
    let mut prev = None;
    for i in 0..n {
        if removed(orig[i]) {
            levels[i] = prev.unwrap_or(para);
        } else {
            prev = Some(levels[i]);
        }
    }
    if let Some(i) = (0..n).find(|&i| !removed(orig[i])) {
        let first = levels[i];
        levels[..i].fill(first);
    }
    Levels {
        paragraph: para,
        levels,
    }
}

/// W1–W7 on the types of an isolating run sequence.
fn resolve_weak(t: &mut [BidiClass], sos: BidiClass) {
    let len = t.len();
    // W1.
    for k in 0..len {
        if t[k] == NSM {
            t[k] = match k {
                0 => sos,
                _ if matches!(t[k - 1], LRI | RLI | FSI | PDI) => ON,
                _ => t[k - 1],
            };
        }
    }
    // W2, W3.
    let mut strong = sos;
    for c in t.iter_mut() {
        match *c {
            L | R => strong = *c,
            AL => {
                strong = AL;
                *c = R;
            }
            EN if strong == AL => *c = AN,
            _ => {}
        }
    }
    // W4.
    for k in 1..len.saturating_sub(1) {
        match (t[k - 1], t[k], t[k + 1]) {
            (EN, ES | CS, EN) => t[k] = EN,
            (AN, CS, AN) => t[k] = AN,
            _ => {}
        }
    }
    // W5.
    let mut k = 0;
    while k < len {
        if t[k] != ET {
            k += 1;
            continue;
        }
        let start = k;
        while k < len && t[k] == ET {
            k += 1;
        }
        if (start > 0 && t[start - 1] == EN) || (k < len && t[k] == EN) {
            t[start..k].fill(EN);
        }
    }
    // W6, W7.
    let mut strong = sos;
    for c in t.iter_mut() {
        match *c {
            ES | ET | CS => *c = ON,
            L | R => strong = *c,
            EN if strong == L => *c = L,
            _ => {}
        }
    }
}

/// N0: paired brackets take the direction of what they enclose.
fn resolve_brackets(
    t: &mut [BidiClass],
    seq: &[usize],
    orig: &[BidiClass],
    brackets: &[Bracket],
    sos: BidiClass,
    e: BidiClass,
) {
    let mut open: Vec<(u32, usize)> = Vec::new();
    let mut pairs = Vec::new();
    for (k, &i) in seq.iter().enumerate() {
        if t[k] != ON {
            continue;
        }
        match brackets.get(i) {
            Some(&Bracket::Open(id)) => {
                if open.len() == MAX_BRACKETS {
                    break;
                }
                open.push((id, k));
            }
            Some(&Bracket::Close(id)) => {
                if let Some(p) = open.iter().rposition(|o| o.0 == id) {
                    pairs.push((open[p].1, k));
                    open.truncate(p);
                }
            }
            _ => {}
        }
    }
    pairs.sort_unstable();
    let strong = |c: BidiClass| match c {
        L => Some(L),
        R | AN | EN => Some(R),
        _ => None,
    };
    for (o, c) in pairs {
        let mut inside = None;
        for &x in &t[o + 1..c] {
            match strong(x) {
                Some(s) if s == e => {
                    inside = Some(e);
                    break;
                }
                Some(s) => inside = Some(s),
                None => {}
            }
        }
        let dir = match inside {
            None => continue,
            Some(d) if d == e => e,
            Some(opposite) => {
                let context = t[..o].iter().rev().find_map(|&x| strong(x)).unwrap_or(sos);
                if context == opposite {
                    opposite
                } else {
                    e
                }
            }
        };
        for b in [o, c] {
            t[b] = dir;
            for k in b + 1..t.len() {
                if orig[seq[k]] != NSM {
                    break;
                }
                t[k] = dir;
            }
        }
    }
}

/// N1, N2: neutrals between two strong types of the same direction take
/// it; others take the embedding direction.
fn resolve_neutrals(t: &mut [BidiClass], sos: BidiClass, eos: BidiClass, e: BidiClass) {
    let neutral = |c: BidiClass| matches!(c, B | S | WS | ON | LRI | RLI | FSI | PDI);
    let dir = |c: BidiClass| if c == L { L } else { R };
    let mut k = 0;
    while k < t.len() {
        if !neutral(t[k]) {
            k += 1;
            continue;
        }
        let start = k;
        while k < t.len() && neutral(t[k]) {
            k += 1;
        }
        let before = if start == 0 { sos } else { dir(t[start - 1]) };
        let after = if k == t.len() { eos } else { dir(t[k]) };
        t[start..k].fill(if before == after { before } else { e });
    }
}

/// L1 for one line: segment and paragraph separators, and whitespace (and
/// isolate formatting characters) before them or at the end of the line,
/// go back to the paragraph level. `classes` are the original classes of
/// the line's characters.
pub fn reset_whitespace(classes: &[BidiClass], levels: &mut [u8], paragraph: u8) {
    let mut trailing = true;
    for k in (0..classes.len()).rev() {
        match classes[k] {
            B | S => {
                levels[k] = paragraph;
                trailing = true;
            }
            WS | LRI | RLI | FSI | PDI => {
                if trailing {
                    levels[k] = paragraph;
                }
            }
            c if removed(c) => {
                if trailing {
                    levels[k] = paragraph;
                }
            }
            _ => trailing = false,
        }
    }
}

/// L2: the order in which to show items with these levels, left to right,
/// as indices into `levels`.
pub fn visual_order(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let Some(&max) = levels.iter().max() else {
        return order;
    };
    let Some(min_odd) = levels.iter().filter(|&&l| l % 2 == 1).min() else {
        return order;
    };
    for level in (*min_odd..=max).rev() {
        let mut k = 0;
        while k < order.len() {
            if levels[order[k]] < level {
                k += 1;
                continue;
            }
            let start = k;
            while k < order.len() && levels[order[k]] >= level {
                k += 1;
            }
            order[start..k].reverse();
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hebrew_with_numbers_and_latin() {
        // "שלום 123 abc": RTL paragraph, the number and Latin word stay LTR.
        let text: Vec<char> = "שלום 123 abc".chars().collect();
        let l = resolve(&text, None);
        assert!(l.rtl());
        assert_eq!(l.levels, [1, 1, 1, 1, 1, 2, 2, 2, 1, 2, 2, 2]);
        let order = visual_order(&l.levels);
        assert_eq!(order, [9, 10, 11, 8, 5, 6, 7, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn class_table_starts_at_zero() {
        // `class` relies on this: every code point is in some range.
        assert_eq!(table::RANGES[0] >> 5, 0);
        assert_eq!(class('\0'), BN);
        assert_eq!(class(char::MAX), BN);
    }

    #[test]
    fn brackets_mirror() {
        assert_eq!(mirror('('), Some(')'));
        assert_eq!(mirror('«'), Some('»'));
        assert_eq!(mirror('a'), None);
        assert_eq!(Bracket::of('\u{2329}'), Bracket::Open(0x3008));
        assert_eq!(Bracket::of('\u{3009}'), Bracket::Close(0x3008));
    }
}
