//! The Unicode Line Breaking Algorithm (UAX #14), without tailoring.
//!
//! [`opportunities`] implements every rule of the algorithm, LB1 to LB31,
//! including the regular-expression rules for quotation marks (LB15a–d,
//! LB19a), numbers (LB25), Brahmic orthographic syllables (LB28a),
//! regional indicator pairs (LB30a) and emoji modifiers (LB30b). The
//! properties come from the Unicode Character Database, see
//! [`linebreak_table`](crate::linebreak_table); the implementation passes
//! the official conformance test (`tests/linebreak.rs`).

use crate::linebreak_table::{self as table, Class, Class::*};

/// Whether a line may or must break between two characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Break {
    /// The characters stay on the same line.
    No,
    /// A line break is allowed.
    Allowed,
    /// A line break is required (after a hard line break character).
    Mandatory,
}

/// Line breaking properties of a character: its class (after LB1) and the
/// `table::FLAG_*` bits.
pub fn properties(c: char) -> (Class, u32) {
    let cp = c as u32;
    let i = table::RANGES.partition_point(|&r| r >> 11 <= cp) - 1;
    let v = table::RANGES[i];
    (table::CLASSES[(v & 0x3F) as usize], (v >> 6) & 0x1F)
}

/// A character after LB9 and LB10: a base character together with the
/// combining marks and joiners attached to it.
#[derive(Clone, Copy)]
struct Unit {
    class: Class,
    flags: u32,
    /// The base character.
    ch: char,
    /// Whether the unit ends with a ZERO WIDTH JOINER (for LB8a).
    zwj: bool,
}

impl Unit {
    fn ea(&self) -> bool {
        self.flags & table::FLAG_EA != 0
    }

    fn is(&self, classes: &[Class]) -> bool {
        classes.contains(&self.class)
    }

    /// AK, AS or DOTTED CIRCLE, the bases of LB28a.
    fn aksara(&self) -> bool {
        matches!(self.class, AK | AS) || self.ch == '\u{25CC}'
    }
}

/// Break opportunities of `text`: element `i` says whether a line may break
/// before character `i`. The result has `text.len() + 1` elements; the
/// first is always [`Break::No`] (LB2) and the last [`Break::Mandatory`]
/// (LB3).
pub fn opportunities(text: &[char]) -> Vec<Break> {
    let mut out = vec![Break::No; text.len() + 1];
    // LB9 and LB10: fold combining marks and ZWJ into the preceding
    // character, unless that is a space or a line break.
    let mut units: Vec<Unit> = Vec::with_capacity(text.len());
    let mut starts: Vec<usize> = Vec::with_capacity(text.len());
    for (i, &c) in text.iter().enumerate() {
        let (class, flags) = properties(c);
        if matches!(class, CM | ZWJ) {
            if let Some(u) = units.last_mut().filter(|u| !u.is(&[BK, CR, LF, NL, SP, ZW])) {
                u.zwj = class == ZWJ;
                continue;
            }
            units.push(Unit {
                class: AL,
                flags,
                ch: c,
                zwj: class == ZWJ,
            });
        } else {
            units.push(Unit {
                class,
                flags,
                ch: c,
                zwj: false,
            });
        }
        starts.push(i);
    }
    for k in 1..units.len() {
        out[starts[k]] = rule(&units, k);
    }
    out[text.len()] = Break::Mandatory;
    out
}

/// The break between `units[k - 1]` and `units[k]`, by the first rule
/// (LB4 onwards) that applies.
fn rule(u: &[Unit], k: usize) -> Break {
    use Break::{Allowed as Yes, Mandatory as Must, No};
    let (a, b) = (u[k - 1], u[k]);
    let at = |i: usize| u.get(i);
    let before = |i: usize| i.checked_sub(1).and_then(|j| u.get(j));
    // The unit before `a`, skipping spaces, and `a` itself if not a space.
    let skip_sp = || u[..k].iter().rev().find(|x| x.class != SP);

    // LB4, LB5: hard line breaks.
    match a.class {
        BK | LF | NL => return Must,
        CR => return if b.class == LF { No } else { Must },
        _ => {}
    }
    // LB6, LB7
    if b.is(&[BK, CR, LF, NL, SP, ZW]) {
        return No;
    }
    // LB8: ZW SP* ÷
    if skip_sp().is_some_and(|x| x.class == ZW) {
        return Yes;
    }
    // LB8a: ZWJ ×
    if a.zwj {
        return No;
    }
    // LB11
    if a.class == WJ || b.class == WJ {
        return No;
    }
    // LB12: GL ×
    if a.class == GL {
        return No;
    }
    // LB12a: [^SP BA HY HH] × GL
    if b.class == GL && !a.is(&[SP, BA, HY, HH]) {
        return No;
    }
    // LB13
    if b.is(&[CL, CP, EX, SY]) {
        return No;
    }
    // LB14: OP SP* ×
    if skip_sp().is_some_and(|x| x.class == OP) {
        return No;
    }
    // LB15a: (sot | BK | CR | LF | NL | OP | QU | GL | SP | ZW) [\p{Pi}&QU] SP* ×
    let sp_run = u[..k].iter().rev().take_while(|x| x.class == SP).count();
    if let Some(q) = (k - 1).checked_sub(sp_run) {
        if u[q].class == QU
            && u[q].flags & table::FLAG_PI != 0
            && before(q).is_none_or(|p| p.is(&[BK, CR, LF, NL, OP, QU, GL, SP, ZW]))
        {
            return No;
        }
    }
    // LB15b: × [\p{Pf}&QU] (SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF | NL | ZW | eot)
    if b.class == QU && b.flags & table::FLAG_PF != 0 {
        let next = at(k + 1);
        if next.is_none_or(|n| n.is(&[SP, GL, WJ, CL, QU, CP, EX, IS, SY, BK, CR, LF, NL, ZW])) {
            return No;
        }
    }
    // LB15c: SP ÷ IS NU
    if a.class == SP && b.class == IS && at(k + 1).is_some_and(|n| n.class == NU) {
        return Yes;
    }
    // LB15d: × IS
    if b.class == IS {
        return No;
    }
    // LB16: (CL | CP) SP* × NS
    if b.class == NS && skip_sp().is_some_and(|x| x.is(&[CL, CP])) {
        return No;
    }
    // LB17: B2 SP* × B2
    if b.class == B2 && skip_sp().is_some_and(|x| x.class == B2) {
        return No;
    }
    // LB18: SP ÷
    if a.class == SP {
        return Yes;
    }
    // LB19: × [QU - \p{Pi}], [QU - \p{Pf}] ×
    if b.class == QU && b.flags & table::FLAG_PI == 0 {
        return No;
    }
    if a.class == QU && a.flags & table::FLAG_PF == 0 {
        return No;
    }
    // LB19a: quotation marks next to non-East-Asian characters.
    if b.class == QU && (!a.ea() || at(k + 1).is_none_or(|n| !n.ea())) {
        return No;
    }
    if a.class == QU && (!b.ea() || before(k - 1).is_none_or(|p| !p.ea())) {
        return No;
    }
    // LB20: ÷ CB, CB ÷
    if a.class == CB || b.class == CB {
        return Yes;
    }
    // LB20a: (sot | BK | CR | LF | NL | SP | ZW | CB | GL) (HY | HH) × (AL | HL)
    if a.is(&[HY, HH])
        && b.is(&[AL, HL])
        && before(k - 1).is_none_or(|p| p.is(&[BK, CR, LF, NL, SP, ZW, CB, GL]))
    {
        return No;
    }
    // LB21: × BA, × HH, × HY, × NS, BB ×
    if b.is(&[BA, HH, HY, NS]) || a.class == BB {
        return No;
    }
    // LB21a: HL (HY | HH) × [^HL]
    if a.is(&[HY, HH]) && b.class != HL && before(k - 1).is_some_and(|p| p.class == HL) {
        return No;
    }
    // LB21b: SY × HL
    if a.class == SY && b.class == HL {
        return No;
    }
    // LB22: × IN
    if b.class == IN {
        return No;
    }
    // LB23: (AL | HL) × NU, NU × (AL | HL)
    if (a.is(&[AL, HL]) && b.class == NU) || (a.class == NU && b.is(&[AL, HL])) {
        return No;
    }
    // LB23a: PR × (ID | EB | EM), (ID | EB | EM) × PO
    if (a.class == PR && b.is(&[ID, EB, EM])) || (a.is(&[ID, EB, EM]) && b.class == PO) {
        return No;
    }
    // LB24: (PR | PO) × (AL | HL), (AL | HL) × (PR | PO)
    if (a.is(&[PR, PO]) && b.is(&[AL, HL])) || (a.is(&[AL, HL]) && b.is(&[PR, PO])) {
        return No;
    }
    // LB25: numbers, as in `$(12.35)`, `2,1%` or `-5`.
    // NU (SY | IS)*, ending at unit `end`.
    let number_before = |end: Option<usize>| -> bool {
        let Some(end) = end else { return false };
        u[..=end]
            .iter()
            .rev()
            .find(|x| !x.is(&[SY, IS]))
            .is_some_and(|x| x.class == NU)
    };
    if b.is(&[PO, PR]) {
        // NU (SY | IS)* (CL | CP)? × (PO | PR)
        if (a.is(&[CL, CP]) && number_before(k.checked_sub(2))) || number_before(Some(k - 1)) {
            return No;
        }
    }
    if a.is(&[PO, PR]) {
        // (PO | PR) × OP IS? NU, (PO | PR) × NU
        let next_nu = |i: usize| at(i).is_some_and(|n| n.class == NU);
        if b.class == NU
            || (b.class == OP
                && (next_nu(k + 1) || (at(k + 1).is_some_and(|n| n.class == IS) && next_nu(k + 2))))
        {
            return No;
        }
    }
    if b.class == NU && (a.is(&[HY, IS]) || number_before(Some(k - 1))) {
        return No;
    }
    // LB26: Korean syllable blocks.
    if (a.class == JL && b.is(&[JL, JV, H2, H3]))
        || (a.is(&[JV, H2]) && b.is(&[JV, JT]))
        || (a.is(&[JT, H3]) && b.class == JT)
    {
        return No;
    }
    // LB27
    if (a.is(&[JL, JV, JT, H2, H3]) && b.class == PO) || (a.class == PR && b.is(&[JL, JV, JT, H2, H3])) {
        return No;
    }
    // LB28: (AL | HL) × (AL | HL)
    if a.is(&[AL, HL]) && b.is(&[AL, HL]) {
        return No;
    }
    // LB28a: Brahmic orthographic syllables.
    if a.class == AP && b.aksara() {
        return No;
    }
    if a.aksara() && b.is(&[VF, VI]) {
        return No;
    }
    if a.class == VI && (b.class == AK || b.ch == '\u{25CC}') && before(k - 1).is_some_and(|p| p.aksara()) {
        return No;
    }
    if a.aksara() && b.aksara() && at(k + 1).is_some_and(|n| n.class == VF) {
        return No;
    }
    // LB29: IS × (AL | HL)
    if a.class == IS && b.is(&[AL, HL]) {
        return No;
    }
    // LB30: (AL | HL | NU) × [OP - EA], [CP - EA] × (AL | HL | NU)
    if (a.is(&[AL, HL, NU]) && b.class == OP && !b.ea()) || (a.class == CP && !a.ea() && b.is(&[AL, HL, NU]))
    {
        return No;
    }
    // LB30a: regional indicators pair up into flags.
    if a.class == RI && b.class == RI {
        let run = u[..k].iter().rev().take_while(|x| x.class == RI).count();
        if run % 2 == 1 {
            return No;
        }
    }
    // LB30b: EB × EM, [\p{Extended_Pictographic}&\p{Cn}] × EM
    if b.class == EM && (a.class == EB || a.flags & table::FLAG_EP_CN != 0) {
        return No;
    }
    // LB31
    Yes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaks(s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        let ops = opportunities(&chars);
        let mut out = String::new();
        for (i, c) in chars.iter().enumerate() {
            match ops[i] {
                Break::Allowed if i > 0 => out.push('|'),
                Break::Mandatory => out.push('!'),
                _ => {}
            }
            out.push(*c);
        }
        out
    }

    #[test]
    fn properties_lookup() {
        assert_eq!(properties('a').0, AL);
        assert_eq!(properties(' ').0, SP);
        assert_eq!(properties('中').0, ID);
        assert_eq!(properties('\u{10FFFF}').0, AL);
        assert!(properties('中').1 & table::FLAG_EA != 0);
    }

    #[test]
    fn common_cases() {
        assert_eq!(breaks("Hello, world!"), "Hello, |world!");
        assert_eq!(breaks("well-known"), "well-|known");
        assert_eq!(breaks("a - b"), "a |- |b");
        assert_eq!(breaks("(test)"), "(test)");
        assert_eq!(breaks("$12.50 and 3,5%"), "$12.50 |and |3,5%");
        assert_eq!(breaks("see: «this»"), "see: |«this»");
        assert_eq!(breaks("中文字"), "中|文|字");
        assert_eq!(breaks("a\u{2028}b"), "a\u{2028}!b");
        assert_eq!(breaks("x\u{200B}y"), "x\u{200B}|y");
        assert_eq!(breaks("a\u{A0}b"), "a\u{A0}b");
        assert_eq!(breaks("path/to"), "path/|to");
        assert_eq!(breaks("e\u{301}t\u{301}"), "e\u{301}t\u{301}");
        assert_eq!(breaks("🇳🇴🇸🇪"), "🇳🇴|🇸🇪");
    }
}
