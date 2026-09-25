//! Character classes used by text layout.

/// Characters that have no visible form and are dropped before layout.
pub fn is_invisible(c: char) -> bool {
    matches!(c as u32,
        0x00..=0x08 | 0x0B..=0x1F | 0x7F..=0x9F
        | 0xAD                      // soft hyphen
        | 0x200B..=0x200F           // zero-width space/joiners, direction marks
        | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFE00..=0xFE0F           // variation selectors
        | 0xFEFF
        | 0xE0100..=0xE01EF)
}

/// Whitespace other than the plain space that should lay out as a space.
pub fn is_space_like(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\r' | '\u{2000}'..='\u{200A}' | '\u{205F}' | '\u{3000}'
    )
}

/// A plain-text stand-in for a character no configured font covers.
pub fn substitute(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2212}' => "-",
        '\u{2013}' => "-",
        '\u{2014}' | '\u{2015}' => "--",
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' => "'",
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => "\"",
        '\u{2022}' | '\u{25CF}' | '\u{25E6}' | '\u{2023}' | '\u{2043}' => "*",
        '\u{2026}' => "...",
        '\u{2192}' => "->",
        '\u{2190}' => "<-",
        '\u{2194}' => "<->",
        '\u{21D2}' => "=>",
        '\u{21D0}' => "<=",
        '\u{21D4}' => "<=>",
        '\u{2264}' => "<=",
        '\u{2265}' => ">=",
        '\u{2260}' => "!=",
        '\u{2248}' => "~=",
        '\u{2713}' | '\u{2714}' | '\u{2705}' => "[x]",
        '\u{2717}' | '\u{2718}' | '\u{274C}' => "[ ]",
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{00A0}' | '\u{202F}' => " ",
        _ => return None,
    })
}

/// Scripts written without spaces between words, where a line may break
/// between any two characters (CJK ideographs, kana, Hangul, fullwidth forms).
pub fn breaks_anywhere(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x2FDF | 0x3000..=0x303F | 0x3040..=0x30FF | 0x3100..=0x31FF
        | 0x3200..=0x4DBF | 0x4E00..=0x9FFF | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert!(is_invisible('\u{AD}') && is_invisible('\u{FEFF}') && !is_invisible('a'));
        assert!(breaks_anywhere('中') && breaks_anywhere('あ') && !breaks_anywhere('a'));
        assert_eq!(substitute('\u{2192}'), Some("->"));
        assert_eq!(substitute('x'), None);
    }
}
