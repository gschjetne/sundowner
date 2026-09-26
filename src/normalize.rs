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

/// HarfBuzz's "modified combining class", by which it orders marks for
/// shaping: the canonical combining class, except that Hebrew points are
/// ordered the way fonts expect them (shin and sin dots first, then
/// dagesh, rafe and the vowels, meteg last).
pub fn shaping_class(c: char) -> u8 {
    match ccc(c) {
        10 => 22, // sheva
        11 => 15, // hataf segol
        12 => 16, // hataf patah
        13 => 17, // hataf qamats
        14 => 23, // hiriq
        15 => 18, // tsere
        16 => 19, // segol
        17 => 20, // patah
        18 => 21, // qamats
        19 => 14, // holam
        20 => 24, // qubuts
        21 => 12, // dagesh
        22 => 25, // meteg
        23 => 13, // rafe
        24 => 10, // shin dot
        25 => 11, // sin dot
        cc => cc,
    }
}

/// Sort each run of characters with a nonzero class by class, keeping the
/// order of equal classes (with [`ccc`], the Canonical Ordering Algorithm).
fn reorder(s: &mut [char], class: fn(char) -> u8) {
    let mut i = 0;
    while i < s.len() {
        if class(s[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < s.len() && class(s[i]) != 0 {
            i += 1;
        }
        s[start..i].sort_by_key(|&c| class(c));
    }
}

/// HarfBuzz's Hebrew mark reordering, after sorting by [`shaping_class`]:
/// in patah or qamats, sheva or hiriq, then meteg or a mark below, the
/// last two swap, so the meteg goes with the first vowel.
fn reorder_hebrew(s: &mut [char]) {
    for i in 2..s.len() {
        let (c0, c1, c2) = (
            shaping_class(s[i - 2]),
            shaping_class(s[i - 1]),
            shaping_class(s[i]),
        );
        if matches!(c0, 20 | 21) && matches!(c1, 22 | 23) && matches!(c2, 25 | 220) {
            s.swap(i - 1, i);
            break;
        }
    }
}

/// Hebrew presentation forms with points, which canonical composition
/// excludes. HarfBuzz composes them for fonts that cannot position marks.
fn compose_hebrew(a: char, b: char) -> Option<char> {
    const DAGESH: [u32; 27] = [
        0xFB30, 0xFB31, 0xFB32, 0xFB33, 0xFB34, 0xFB35, 0xFB36, 0, 0xFB38, 0xFB39, 0xFB3A, 0xFB3B, 0xFB3C, 0,
        0xFB3E, 0, 0xFB40, 0xFB41, 0, 0xFB43, 0xFB44, 0, 0xFB46, 0xFB47, 0xFB48, 0xFB49, 0xFB4A,
    ];
    let cp = match (b as u32, a as u32) {
        (0x5B4, 0x5D9) => 0xFB1D,
        (0x5B7, 0x5F2) => 0xFB1F,
        (0x5B7, 0x5D0) => 0xFB2E,
        (0x5B8, 0x5D0) => 0xFB2F,
        (0x5B9, 0x5D5) => 0xFB4B,
        (0x5BC, a @ 0x5D0..=0x5EA) => DAGESH[(a - 0x5D0) as usize],
        (0x5BC, 0xFB2A) => 0xFB2C,
        (0x5BC, 0xFB2B) => 0xFB2D,
        (0x5BF, 0x5D1) => 0xFB4C,
        (0x5BF, 0x5DB) => 0xFB4D,
        (0x5BF, 0x5E4) => 0xFB4E,
        (0x5C1, 0x5E9) => 0xFB2A,
        (0x5C1, 0xFB49) => 0xFB2C,
        (0x5C2, 0x5E9) => 0xFB2B,
        (0x5C2, 0xFB49) => 0xFB2D,
        _ => 0,
    };
    char::from_u32(cp).filter(|_| cp != 0)
}

/// Composition of a decomposed sequence ordered by `class`. With
/// `marks_only`, only combining marks join a preceding starter (as
/// HarfBuzz does), and with `hebrew`, Hebrew presentation forms are
/// composed too; `has` says which composites are acceptable.
fn recompose(
    s: &mut Vec<char>,
    marks_only: bool,
    hebrew: bool,
    class: fn(char) -> u8,
    has: impl Fn(char) -> bool,
) {
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    let mut starter: Option<usize> = None;
    for &c in s.iter() {
        let cc = class(c);
        if let Some(st) = starter {
            let last = out.len() - 1;
            let unblocked = st == last || (class(out[last]) < cc && cc != 0);
            if unblocked && (!marks_only || is_mark(c)) {
                let composed = compose(out[st], c).or_else(|| compose_hebrew(out[st], c).filter(|_| hebrew));
                if let Some(composed) = composed.filter(|&x| has(x)) {
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
    reorder(&mut out, ccc);
    out
}

/// Normalization Form C.
pub fn nfc(s: &[char]) -> Vec<char> {
    let mut out = nfd(s);
    recompose(&mut out, false, false, ccc, |_| true);
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
/// mode, with its mark order ([`shaping_class`]). `hebrew_forms` also
/// composes Hebrew presentation forms, as HarfBuzz does for fonts that do
/// not position Hebrew marks.
pub fn for_font(
    cluster: &[char],
    has: impl Fn(char) -> bool,
    hebrew_forms: bool,
    out: &mut Vec<char>,
) -> bool {
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
            reorder(run, shaping_class);
            reorder_hebrew(run);
        }
        let mut tail = out.split_off(start);
        recompose(&mut tail, true, hebrew_forms, shaping_class, &has);
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
            let ok = for_font(&s(cluster), has, false, &mut out);
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
        // Hebrew points go in the order fonts expect: shin dot, dagesh,
        // then the vowel; a meteg after patah and sheva moves before the
        // sheva.
        assert_eq!(
            run("\u{5E9}\u{5B8}\u{5BC}\u{5C1}", &all).0,
            "\u{5E9}\u{5C1}\u{5BC}\u{5B8}"
        );
        assert_eq!(
            run("\u{5D0}\u{5B7}\u{5B0}\u{5BD}", &all).0,
            "\u{5D0}\u{5B7}\u{5BD}\u{5B0}"
        );
        // Presentation forms only when asked for.
        let mut out = Vec::new();
        assert!(for_font(&s("\u{5E9}\u{5BC}\u{5C1}"), all, true, &mut out));
        assert_eq!(out, ['\u{FB2C}']);
    }
}
