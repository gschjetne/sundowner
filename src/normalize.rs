//! Unicode canonical decomposition and composition, and the font-aware
//! normalization that text shaping needs.
//!
//! Text can spell `é` as one character (U+00E9) or as `e` followed by a
//! combining acute accent (U+0301). Fonts have a designed glyph for the
//! precomposed character but can position a combining mark only
//! approximately, and some combinations have no precomposed character at
//! all. [`for_font`] rewrites each cluster (a base character and the marks
//! that follow it) into the form a font renders best, the way HarfBuzz
//! does: decompose, put the marks in canonical order, and recompose
//! whatever the font has glyphs for. [`nfd`] and [`nfc`] are the standard
//! normalization forms (tested against the official conformance data).

use crate::normalize_table::{CLASSES, COMPOSITIONS, DECOMPOSITIONS};

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

fn props(c: char) -> u32 {
    let cp = c as u32;
    CLASSES[CLASSES.partition_point(|&r| r >> 11 <= cp) - 1]
}

/// Canonical combining class.
pub fn ccc(c: char) -> u8 {
    if (c as u32) < 0x300 {
        return 0;
    }
    props(c) as u8
}

/// Whether `c` is a combining mark (General_Category Mn, Mc or Me).
pub fn is_mark(c: char) -> bool {
    (c as u32) >= 0x300 && props(c) & 0x100 != 0
}

/// One level of canonical decomposition: `c` is equivalent to `a` followed
/// by `b` (if any).
pub fn decompose(c: char) -> Option<(char, Option<char>)> {
    let cp = c as u32;
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let s = cp - S_BASE;
        let t = s % T_COUNT;
        return if t == 0 {
            let l = char::from_u32(L_BASE + s / N_COUNT)?;
            Some((l, char::from_u32(V_BASE + (s % N_COUNT) / T_COUNT)))
        } else {
            Some((char::from_u32(cp - t)?, char::from_u32(T_BASE + t)))
        };
    }
    if cp < 0xC0 {
        return None;
    }
    let i = DECOMPOSITIONS.binary_search_by_key(&cp, |d| d.0).ok()?;
    let (_, a, b) = DECOMPOSITIONS[i];
    Some((char::from_u32(a)?, if b == 0 { None } else { char::from_u32(b) }))
}

/// The primary composite of `a` and `b`, if there is one.
pub fn compose(a: char, b: char) -> Option<char> {
    let (a, b) = (a as u32, b as u32);
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return char::from_u32(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE).is_multiple_of(T_COUNT)
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&b)
    {
        return char::from_u32(a + b - T_BASE);
    }
    let i = COMPOSITIONS.binary_search_by_key(&(a, b), |c| (c.0, c.1)).ok()?;
    char::from_u32(COMPOSITIONS[i].2)
}

/// Append the full canonical decomposition of `c`.
fn decompose_full(c: char, out: &mut Vec<char>) {
    match decompose(c) {
        Some((a, b)) => {
            decompose_full(a, out);
            if let Some(b) = b {
                decompose_full(b, out);
            }
        }
        None => out.push(c),
    }
}

/// Sort each run of characters with a nonzero combining class by class,
/// keeping the order of equal classes (the Canonical Ordering Algorithm).
fn reorder(s: &mut [char]) {
    let mut i = 0;
    while i < s.len() {
        if ccc(s[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < s.len() && ccc(s[i]) != 0 {
            i += 1;
        }
        s[start..i].sort_by_key(|&c| ccc(c));
    }
}

/// Canonical composition of a decomposed, reordered sequence. With
/// `marks_only`, only combining marks join a preceding starter (as
/// HarfBuzz does); `has` says which composites are acceptable.
fn recompose(s: &mut Vec<char>, marks_only: bool, has: impl Fn(char) -> bool) {
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    let mut starter: Option<usize> = None;
    for &c in s.iter() {
        let cc = ccc(c);
        if let Some(st) = starter {
            let last = out.len() - 1;
            let unblocked = st == last || (ccc(out[last]) < cc && cc != 0);
            if unblocked && (!marks_only || is_mark(c)) {
                if let Some(composed) = compose(out[st], c).filter(|&x| has(x)) {
                    out[st] = composed;
                    continue;
                }
            }
        }
        out.push(c);
        if cc == 0 {
            starter = Some(out.len() - 1);
        }
    }
    *s = out;
}

/// Normalization Form D.
pub fn nfd(s: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    for &c in s {
        decompose_full(c, &mut out);
    }
    reorder(&mut out);
    out
}

/// Normalization Form C.
pub fn nfc(s: &[char]) -> Vec<char> {
    let mut out = nfd(s);
    recompose(&mut out, false, |_| true);
    out
}

/// Longest run of marks that is reordered (as in HarfBuzz; longer runs are
/// not meaningful text and are left as they are).
const MAX_REORDER: usize = 32;

/// Rewrite `cluster` (a character followed by combining marks, or a single
/// character) into the form that renders best with a font that has glyphs
/// for the characters `has` accepts, and append it to `out`. Returns false
/// if some character of the result has no glyph.
///
/// A lone character the font has is kept. Otherwise characters are
/// decomposed as far as the font supports, marks are put in canonical
/// order, and each mark is composed with its base if the font has the
/// composite. This is HarfBuzz's normalization in its default ("composed")
/// mode.
pub fn for_font(cluster: &[char], has: impl Fn(char) -> bool, out: &mut Vec<char>) -> bool {
    let start = out.len();
    if let [c] = cluster {
        // Keep what the font has; decompose what it lacks.
        if has(*c) || !decompose_for(*c, true, &has, out) {
            out.push(*c);
        }
    } else {
        for &c in cluster {
            if !decompose_for(c, false, &has, out) {
                out.push(c);
            }
        }
        let run = &mut out[start..];
        if run.len() <= MAX_REORDER + 1 {
            reorder(run);
        }
        let mut tail = out.split_off(start);
        recompose(&mut tail, true, &has);
        out.extend(tail);
    }
    out[start..].iter().all(|&c| has(c))
}

/// Decompose `c` into characters the font has (HarfBuzz's `decompose`):
/// with `shortest`, stop at the first level the font supports, otherwise
/// go as deep as it supports. Returns false (appending nothing) if no
/// decomposition works.
fn decompose_for(c: char, shortest: bool, has: &impl Fn(char) -> bool, out: &mut Vec<char>) -> bool {
    let Some((a, b)) = decompose(c) else { return false };
    if b.is_some_and(|b| !has(b)) {
        return false;
    }
    let has_a = has(a);
    if !(shortest && has_a) && decompose_for(a, shortest, has, out) {
        out.extend(b);
        return true;
    }
    if has_a {
        out.push(a);
        out.extend(b);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Vec<char> {
        x.chars().collect()
    }

    #[test]
    fn primitives() {
        assert_eq!(ccc('a'), 0);
        assert_eq!(ccc('\u{301}'), 230);
        assert_eq!(ccc('\u{323}'), 220);
        assert!(is_mark('\u{301}') && !is_mark('a') && !is_mark('´'));
        assert_eq!(decompose('é'), Some(('e', Some('\u{301}'))));
        assert_eq!(decompose('Å'), Some(('A', Some('\u{30A}'))));
        assert_eq!(
            decompose('\u{212B}'),
            Some(('Å', None)),
            "ANGSTROM SIGN is a singleton"
        );
        assert_eq!(compose('e', '\u{301}'), Some('é'));
        assert_eq!(compose('A', '\u{30A}'), Some('Å'));
        assert_eq!(compose('\u{1100}', '\u{1161}'), Some('가'));
        assert_eq!(decompose('각'), Some(('가', Some('\u{11A8}'))));
    }

    #[test]
    fn font_normalization() {
        let run = |cluster: &str, has: &dyn Fn(char) -> bool| {
            let mut out = Vec::new();
            let ok = for_font(&s(cluster), has, &mut out);
            (out.into_iter().collect::<String>(), ok)
        };
        let all = |_: char| true;
        // Composes when the font has the composite.
        assert_eq!(run("e\u{301}", &all), ("é".into(), true));
        // Keeps the precomposed character, or decomposes it if the font
        // lacks it.
        assert_eq!(run("é", &all), ("é".into(), true));
        let no_precomposed = |c: char| c.is_ascii() || is_mark(c);
        assert_eq!(run("é", &no_precomposed), ("e\u{301}".into(), true));
        // Marks are reordered and composed with the best composite:
        // e + acute + dot below -> ẹ + acute -> ệ... only if available.
        assert_eq!(run("e\u{301}\u{323}", &all), ("ẹ\u{301}".into(), true));
        assert_eq!(run("é\u{323}", &all), ("ẹ\u{301}".into(), true));
        assert_eq!(run("ê\u{323}", &all), ("ệ".into(), true));
        // Nothing to compose with: mark stays separate.
        assert_eq!(run("q\u{301}", &all), ("q\u{301}".into(), true));
        // Missing glyphs are reported.
        assert_eq!(run("x\u{301}", &|c: char| c == 'x'), ("x\u{301}".into(), false));
    }
}
