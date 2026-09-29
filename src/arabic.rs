//! Arabic joining: which positional form (isolated, initial, medial or
//! final) each letter takes, from the joining types of it and its
//! neighbours (the cursive joining model of the Unicode Standard, chapter
//! 9.2, as HarfBuzz implements it). Transparent characters such as vowel
//! marks are skipped; a zero width joiner (join causing) joins and a zero
//! width non-joiner (non-joining) separates.

use crate::bidi_table::{self as table, JoiningType};
use crate::gsub::mask;

/// The joining type of a character. (`JOINING` starts at U+0000, so every
/// code point is in a range.)
pub fn joining_type(c: char) -> JoiningType {
    let cp = c as u32;
    let i = table::JOINING.partition_point(|&r| r >> 3 <= cp) - 1;
    table::JOINING_TYPES[(table::JOINING[i] & 7) as usize]
}

/// Whether a character, found before some text, joins towards it.
pub fn joins_forward(c: char) -> bool {
    matches!(joining_type(c), JoiningType::D | JoiningType::L | JoiningType::C)
}

/// Whether a character, found after some text, joins towards it.
pub fn joins_backward(c: char) -> bool {
    matches!(joining_type(c), JoiningType::D | JoiningType::R | JoiningType::C)
}

/// Whether any character of `text` joins, so its forms need working out.
pub fn needs_joining(text: &[char]) -> bool {
    text.iter()
        .any(|&c| matches!(joining_type(c), JoiningType::D | JoiningType::R | JoiningType::L))
}

/// The nearest character of `text` before `i` (or from `i` on, forward)
/// that is not transparent: the neighbour that decides the joining.
pub fn neighbour(text: &[char], i: usize, forward: bool) -> Option<char> {
    let joins = |c: &&char| joining_type(**c) != JoiningType::T;
    if forward {
        text.get(i..)?.iter().find(joins).copied()
    } else {
        text[..i].iter().rev().find(joins).copied()
    }
}

/// The feature masks ([`mask::ISOL`] and so on, or 0) for the characters
/// of `text`. `before` and `after` say whether the text around it joins
/// towards it ([`joins_forward`] of the character before, [`joins_backward`]
/// of the one after).
pub fn forms(text: &[char], before: bool, after: bool) -> Vec<u16> {
    // HarfBuzz's state machine without the Syriac states. States: 0 the
    // previous character does not join; 1 it is right-joining (or isolated
    // right-joining); 2 it can join forward and is isolated so far; 3 it
    // can join forward and is final so far. Columns: U, L, R, D. Entries:
    // (form for the previous character, form for this one, next state).
    const N: u16 = 0;
    const I: u16 = mask::ISOL;
    const F: u16 = mask::FINA;
    const M: u16 = mask::MEDI;
    const S: u16 = mask::INIT;
    const TABLE: [[(u16, u16, u8); 4]; 4] = [
        [(N, N, 0), (N, I, 2), (N, I, 1), (N, I, 2)],
        [(N, N, 0), (N, I, 2), (N, I, 1), (N, I, 2)],
        [(N, N, 0), (N, I, 2), (S, F, 1), (S, F, 3)],
        [(N, N, 0), (N, I, 2), (M, F, 1), (M, F, 3)],
    ];
    let column = |c: char| match joining_type(c) {
        JoiningType::U => Some(0),
        JoiningType::L => Some(1),
        JoiningType::R => Some(2),
        JoiningType::D | JoiningType::C => Some(3),
        JoiningType::T => None,
    };
    let mut out = vec![0; text.len()];
    let mut state = if before { 2 } else { 0 };
    let mut prev: Option<usize> = None;
    for (i, &c) in text.iter().enumerate() {
        let Some(col) = column(c) else { continue };
        let (p, this, next) = TABLE[state][col];
        if let (Some(k), true) = (prev, p != N) {
            out[k] = p;
        }
        out[i] = this;
        prev = Some(i);
        state = next as usize;
    }
    if after {
        if let (Some(k), (p, _, _)) = (prev, TABLE[state][3]) {
            if p != N {
                out[k] = p;
            }
        }
    }
    out
}

/// TATWEEL (U+0640), the kashida: a join-causing stroke that lengthens
/// the connection between two joined letters.
pub const TATWEEL: char = '\u{640}';

/// The letters of the joining groups that decide where a kashida goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    /// Seen and sad, and the letters built on them.
    Seen,
    /// Teh marbuta and heh.
    Heh,
    /// Reh and dal.
    Reh,
    /// Alef, tah, lam and kaf (with gaf).
    Alef,
    Lam,
    /// Beh, teh, noon, yeh and the letters built on them.
    Beh,
    /// Waw, ain, qaf and feh.
    Waw,
    Other,
}

fn group(c: char) -> Group {
    match c {
        '\u{633}'..='\u{636}' | '\u{69A}'..='\u{69E}' | '\u{6FA}' | '\u{6FB}' => Group::Seen,
        '\u{629}' | '\u{647}' | '\u{6C0}' | '\u{6C1}' | '\u{6C3}' | '\u{6D5}' => Group::Heh,
        '\u{62F}'..='\u{632}' | '\u{688}'..='\u{699}' | '\u{6EE}' | '\u{6EF}' => Group::Reh,
        '\u{644}' | '\u{6B5}'..='\u{6B8}' => Group::Lam,
        '\u{622}' | '\u{623}' | '\u{625}' | '\u{627}' | '\u{671}'..='\u{673}' | '\u{675}' => Group::Alef,
        '\u{637}' | '\u{638}' | '\u{69F}' | '\u{643}' | '\u{6A9}'..='\u{6B4}' => Group::Alef,
        '\u{626}' | '\u{628}' | '\u{62A}' | '\u{62B}' | '\u{646}' | '\u{649}' | '\u{64A}' => Group::Beh,
        '\u{66E}'
        | '\u{679}'..='\u{680}'
        | '\u{6B9}'..='\u{6BD}'
        | '\u{6CC}'
        | '\u{6CE}'
        | '\u{6D0}'
        | '\u{6D1}' => Group::Beh,
        '\u{624}' | '\u{648}' | '\u{6C4}'..='\u{6CB}' | '\u{6CF}' => Group::Waw,
        '\u{639}' | '\u{63A}' | '\u{6A0}' | '\u{641}' | '\u{642}' | '\u{66F}' | '\u{6A1}'..='\u{6A8}' => {
            Group::Waw
        }
        _ => Group::Other,
    }
}

/// Where a word may be lengthened by a kashida, if anywhere: the index in
/// `text` to insert tatweels at, and its priority (lower is better). `forms`
/// are the positional forms of the letters ([`forms`]).
///
/// A kashida goes only between two joined letters (so never after the
/// last letter), and never inside lam-alef; after any marks of the letter
/// before it. Of the places a word has, the best is taken by the classic
/// priorities (as in LibreOffice and Microsoft Word): after seen or sad;
/// before a final teh marbuta or heh; before a final reh or dal; before a
/// final alef, tah, lam or kaf; after beh and the letters like it; before
/// a final waw, ain, qaf or feh; before any other final letter. Of places
/// with the same priority the last is taken. A word that already has a
/// tatweel gets no more.
pub fn kashida(text: &[char], forms: &[u16]) -> Option<(usize, u8)> {
    use mask::{FINA, INIT, MEDI};
    if text.contains(&TATWEEL) {
        return None;
    }
    let letter = |c: char| matches!(joining_type(c), JoiningType::D | JoiningType::R | JoiningType::L);
    let mut best: Option<(usize, u8)> = None;
    let mut prev: Option<usize> = None;
    for (i, &c) in text.iter().enumerate() {
        let jt = joining_type(c);
        if jt == JoiningType::T {
            continue;
        }
        if let Some(p) = prev.filter(|_| letter(c)) {
            let (a, b) = (group(text[p]), group(c));
            let joined = forms[p] & (INIT | MEDI) != 0 && forms[i] & (MEDI | FINA) != 0;
            let fina = forms[i] & FINA != 0;
            let priority = match () {
                _ if !joined || (a == Group::Lam && b == Group::Alef) => None,
                _ if a == Group::Seen => Some(1),
                _ if fina && b == Group::Heh => Some(2),
                _ if fina && b == Group::Reh => Some(3),
                _ if fina && matches!(b, Group::Alef | Group::Lam) => Some(4),
                _ if a == Group::Beh => Some(5),
                _ if fina && b == Group::Waw => Some(6),
                _ if fina => Some(7),
                _ => None,
            };
            if let Some(q) = priority.filter(|&q| best.is_none_or(|b| q <= b.1)) {
                // Before the letter, so after the marks of the one before.
                best = Some((i, q));
            }
        }
        prev = letter(c).then_some(i);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms_of(s: &str, before: bool, after: bool) -> Vec<u16> {
        forms(&s.chars().collect::<Vec<_>>(), before, after)
    }

    #[test]
    fn joining_table_starts_at_zero() {
        assert_eq!(table::JOINING[0] >> 3, 0);
        assert_eq!(joining_type('\0'), JoiningType::U);
        assert_eq!(joining_type(char::MAX), JoiningType::U);
    }

    #[test]
    fn letters_join_by_type() {
        use mask::{FINA, INIT, ISOL, MEDI};
        // بيت: beh (D), yeh (D), teh (D).
        assert_eq!(forms_of("بيت", false, false), [INIT, MEDI, FINA]);
        // دار: dal, alef and reh are right-joining, so none joins forward.
        assert_eq!(forms_of("دار", false, false), [ISOL, ISOL, ISOL]);
        // A vowel mark is transparent: بَيت joins across it.
        assert_eq!(forms_of("بَيت", false, false), [INIT, 0, MEDI, FINA]);
        // A zero width non-joiner separates, a zero width joiner joins.
        assert_eq!(forms_of("ب\u{200C}ب", false, false), [ISOL, 0, ISOL]);
        assert_eq!(forms_of("ب\u{200D}", false, false), [INIT, FINA]);
        // Context: text joined on both sides.
        assert_eq!(forms_of("ب", true, true), [MEDI]);
        assert_eq!(forms_of("ب", true, false), [FINA]);
        assert_eq!(forms_of("ب", false, true), [INIT]);
        // Latin does not join.
        assert_eq!(forms_of("ab", false, false), [0, 0]);
    }

    fn kashida_in(s: &str) -> Option<(String, u8)> {
        let text: Vec<char> = s.chars().collect();
        let (at, q) = kashida(&text, &forms(&text, false, false))?;
        let mut out = text.clone();
        out.insert(at, TATWEEL);
        Some((out.into_iter().collect(), q))
    }

    #[test]
    fn kashidas_go_between_joined_letters_by_priority() {
        // After seen, rather than before the final beh-like letter.
        assert_eq!(kashida_in("سلام"), Some(("سـلام".into(), 1)));
        // Before a final teh marbuta or heh.
        assert_eq!(kashida_in("مدرسة"), Some(("مدرسـة".into(), 1)));
        assert_eq!(kashida_in("كلمة"), Some(("كلمـة".into(), 2)));
        // Before a final reh or dal.
        assert_eq!(kashida_in("كبير"), Some(("كبيـر".into(), 3)));
        // Before a final lam, and never inside lam-alef.
        assert_eq!(kashida_in("جميل"), Some(("جميـل".into(), 4)));
        assert_eq!(kashida_in("كلا"), None);
        // After beh-like letters: the last such place.
        assert_eq!(kashida_in("بيتي"), Some(("بيتـي".into(), 5)));
        // Before a final letter of another group.
        assert_eq!(kashida_in("حلم"), Some(("حلـم".into(), 7)));
        // After the marks of the letter before.
        assert_eq!(kashida_in("كَتَبَ"), Some(("كَتَـبَ".into(), 5)));
        // Only between joined letters: dal, alef and reh join to nothing
        // after them, and a word with a tatweel gets no other.
        assert_eq!(kashida_in("دار"), None);
        assert_eq!(kashida_in("كـتب"), None);
        assert_eq!(kashida_in("ب"), None);
        assert_eq!(kashida_in("abc"), None);
    }
}
