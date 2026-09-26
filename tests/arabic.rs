//! Arabic shaping in the silk build: joining, GSUB stages and GPOS
//! (cursive attachment, contextual positioning, marks), checked glyph for
//! glyph against HarfBuzz 14.5 with Amiri.
#![cfg(feature = "silk")]

use sundowner::arabic;
use sundowner::fonts::Fonts;
use sundowner::gsub::{mask, Glyph, Script};

/// Shape `text` (which must need no normalization) with Amiri Regular as a
/// right-to-left run, and return `(glyph, x_advance, x_offset, y_offset)`
/// in logical order, as HarfBuzz reports them.
fn shape(text: &str) -> Vec<(u16, i32, i32, i32)> {
    let fonts = Fonts::builtin();
    let face = (0..fonts.faces.len())
        .find(|&i| fonts.faces[i].postscript_name == "Amiri-Regular")
        .expect("Amiri is bundled");
    let f = &fonts.faces[face];
    let chars: Vec<char> = text.chars().collect();
    let forms = arabic::forms(&chars, false, false);
    let mut glyphs: Vec<Glyph> = chars
        .iter()
        .zip(&forms)
        .enumerate()
        .map(|(k, (&c, &form))| Glyph {
            id: f.glyph(c).expect("Amiri covers the text"),
            cluster: k as u32,
            mask: mask::GLOBAL | mask::RTLM | form,
            component: 0,
        })
        .collect();
    f.substitute(Script::Arabic, &mut glyphs);
    let ids: Vec<u16> = glyphs.iter().map(|g| g.id).collect();
    let components: Vec<u8> = glyphs.iter().map(|g| g.component).collect();
    let marks: Vec<bool> = ids.iter().map(|&g| f.glyph_class(g) == Some(3)).collect();
    let pos = f.position(Script::Arabic, &ids, &components, &|k| marks[k], true, None);
    ids.iter()
        .zip(pos)
        .map(|(&g, p)| (g, p.x_advance, p.x_offset, p.y_offset))
        .collect()
}

#[test]
fn matches_harfbuzz() {
    // Positional forms, a lam that becomes two glyphs (the sukun goes on
    // the first), stacked marks and cursive attachment.
    assert_eq!(
        shape("ٱلْكِتَٰبُ"),
        [
            (129, 217, 0, 0),
            (1843, 175, 0, 0),
            (5477, 185, 0, 0),
            (98, 0, 42, 0),
            (1821, 588, 0, 0),
            (96, 0, 36, 0),
            (1658, 244, 0, 0),
            (94, 0, -188, 0),
            (128, 0, -238, 88),
            (1589, 883, 0, 0),
            (95, 0, 183, 0),
        ]
    );
    assert_eq!(
        shape("بِسْمِ"),
        [
            (3639, 219, 0, 0),
            (96, 0, -133, 0),
            (3662, 291, 0, 0),
            (98, 0, -175, 0),
            (4030, 565, 0, 0),
            (96, 0, 76, 0),
        ]
    );
    // Lam-alef.
    assert_eq!(shape("لا"), [(2513, 302, 0, 0), (2518, 340, 0, 0)]);
}
