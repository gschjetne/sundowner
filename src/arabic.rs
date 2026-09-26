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
}
