//! Unicode to WinAnsiEncoding (Windows-1252) conversion for the base-14 fonts.

/// Append the WinAnsi encoding of `c` to `out`. Characters that the standard
/// fonts cannot show are transliterated where a close ASCII form exists and
/// replaced by `?` otherwise. Invisible characters are dropped.
pub fn push_char(out: &mut Vec<u8>, c: char) {
    let cp = c as u32;
    let b: u8 = match cp {
        0x00AD => return,
        0x20..=0x7E => cp as u8,
        0xA0..=0xFF => cp as u8,
        0x09 | 0x0A | 0x0D => b' ',
        0x2002..=0x200A | 0x202F | 0x205F | 0x3000 => b' ',
        0x200B..=0x200D | 0x2060 | 0xFEFF => return,
        0x0000..=0x001F | 0x007F..=0x009F => return,
        0x20AC => 0x80,
        0x201A => 0x82,
        0x0192 => 0x83,
        0x201E => 0x84,
        0x2026 => 0x85,
        0x2020 => 0x86,
        0x2021 => 0x87,
        0x02C6 => 0x88,
        0x2030 => 0x89,
        0x0160 => 0x8A,
        0x2039 => 0x8B,
        0x0152 => 0x8C,
        0x017D => 0x8E,
        0x2018 | 0x2032 => 0x91,
        0x2019 => 0x92,
        0x201C | 0x2033 => 0x93,
        0x201D => 0x94,
        0x2022 | 0x25CF | 0x25E6 | 0x2023 | 0x2043 => 0x95,
        0x2013 => 0x96,
        0x2014 | 0x2015 => 0x97,
        0x02DC => 0x98,
        0x2122 => 0x99,
        0x0161 => 0x9A,
        0x203A => 0x9B,
        0x0153 => 0x9C,
        0x017E => 0x9E,
        0x0178 => 0x9F,
        0x2010 | 0x2011 | 0x2012 | 0x2212 => b'-',
        0x2044 | 0x2215 => b'/',
        0x2217 => b'*',
        0x2236 => b':',
        0x223C => b'~',
        0x2192 => return out.extend_from_slice(b"->"),
        0x2190 => return out.extend_from_slice(b"<-"),
        0x2194 => return out.extend_from_slice(b"<->"),
        0x21D2 => return out.extend_from_slice(b"=>"),
        0x21D0 => return out.extend_from_slice(b"<="),
        0x21D4 => return out.extend_from_slice(b"<=>"),
        0x2264 => return out.extend_from_slice(b"<="),
        0x2265 => return out.extend_from_slice(b">="),
        0x2260 => return out.extend_from_slice(b"!="),
        0x2248 => return out.extend_from_slice(b"~="),
        0x221E => return out.extend_from_slice(b"inf"),
        0x2713 | 0x2714 | 0x2705 => return out.extend_from_slice(b"[x]"),
        0x2717 | 0x2718 | 0x274C => return out.extend_from_slice(b"[ ]"),
        0xFB00 => return out.extend_from_slice(b"ff"),
        0xFB01 => return out.extend_from_slice(b"fi"),
        0xFB02 => return out.extend_from_slice(b"fl"),
        0xFB03 => return out.extend_from_slice(b"ffi"),
        0xFB04 => return out.extend_from_slice(b"ffl"),
        // Combining marks cannot be composed with base-14 fonts; drop them so
        // decomposed text degrades to the base letter.
        0x0300..=0x036F => return,
        _ => latin_fallback(c).unwrap_or(b'?'),
    };
    out.push(b);
}

/// Encode a whole string.
pub fn encode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        push_char(&mut out, c);
    }
    out
}

/// Strip diacritics from common Latin Extended-A letters.
fn latin_fallback(c: char) -> Option<u8> {
    let s = match c {
        'Ā' | 'Ă' | 'Ą' => b'A',
        'ā' | 'ă' | 'ą' => b'a',
        'Ć' | 'Ĉ' | 'Ċ' | 'Č' => b'C',
        'ć' | 'ĉ' | 'ċ' | 'č' => b'c',
        'Ď' | 'Đ' => b'D',
        'ď' | 'đ' => b'd',
        'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => b'E',
        'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => b'e',
        'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => b'G',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => b'g',
        'Ĥ' | 'Ħ' => b'H',
        'ĥ' | 'ħ' => b'h',
        'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => b'I',
        'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => b'i',
        'Ĵ' => b'J',
        'ĵ' => b'j',
        'Ķ' => b'K',
        'ķ' => b'k',
        'Ĺ' | 'Ļ' | 'Ľ' | 'Ŀ' | 'Ł' => b'L',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => b'l',
        'Ń' | 'Ņ' | 'Ň' => b'N',
        'ń' | 'ņ' | 'ň' => b'n',
        'Ō' | 'Ŏ' | 'Ő' => b'O',
        'ō' | 'ŏ' | 'ő' => b'o',
        'Ŕ' | 'Ŗ' | 'Ř' => b'R',
        'ŕ' | 'ŗ' | 'ř' => b'r',
        'Ś' | 'Ŝ' | 'Ş' | 'Ș' => b'S',
        'ś' | 'ŝ' | 'ş' | 'ș' => b's',
        'Ţ' | 'Ť' | 'Ŧ' | 'Ț' => b'T',
        'ţ' | 'ť' | 'ŧ' | 'ț' => b't',
        'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => b'U',
        'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => b'u',
        'Ŵ' => b'W',
        'ŵ' => b'w',
        'Ŷ' => b'Y',
        'ŷ' => b'y',
        'Ź' | 'Ż' => b'Z',
        'ź' | 'ż' => b'z',
        _ => return None,
    };
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_latin1_pass_through() {
        assert_eq!(encode("Hello, wörld!"), b"Hello, w\xF6rld!".to_vec());
    }

    #[test]
    fn typographic_characters() {
        assert_eq!(
            encode("\u{201C}a\u{201D} \u{2014} \u{20AC}"),
            vec![0x93, b'a', 0x94, b' ', 0x97, b' ', 0x80]
        );
        assert_eq!(encode("a\u{2192}b"), b"a->b".to_vec());
        assert_eq!(encode("\u{4E2D}"), b"?".to_vec());
        assert_eq!(encode("a\u{00AD}b\u{200B}"), b"ab".to_vec());
    }
}
