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

/// Every word of `tests/data/arabic-words.txt`, in every style, shaped the
/// way layout shapes text, against HarfBuzz (`tools/gen_harfbuzz.py`).
#[test]
fn every_word_matches_harfbuzz_in_every_style() {
    let options = sundowner::Options::default();
    let data = include_str!("data/arabic-harfbuzz.txt");
    let (mut cases, mut failures) = (0, Vec::new());
    for line in data.lines().filter(|l| !l.starts_with('#')) {
        let mut fields = line.split('\t');
        let (style, word) = (fields.next().unwrap(), fields.next().unwrap());
        let expected: Vec<(u16, i32, i32, i32)> = fields
            .map(|g| {
                let v: Vec<i32> = g.split(':').map(|x| x.parse().unwrap()).collect();
                (v[0] as u16, v[1], v[2], v[3])
            })
            .collect();
        let (bold, italic) = (style.contains("Bold"), style.contains("Italic"));
        let rtl = !word.chars().all(|c| ('\u{660}'..='\u{669}').contains(&c));
        let runs = sundowner::layout::shape_text(word, bold, italic, rtl, &options);
        let faces: Vec<&str> = runs
            .iter()
            .map(|r| options.fonts.faces[r.0].postscript_name.as_str())
            .collect();
        // Amiri's units per em are 1000, so 1/1000 em are font units.
        let got: Vec<(u16, i32, i32, i32)> = runs
            .iter()
            .flat_map(|r| &r.1)
            .map(|&(id, adv, dx, dy)| (id, adv.round() as i32, dx.round() as i32, dy.round() as i32))
            .collect();
        cases += 1;
        if got != expected || faces.iter().any(|f| *f != format!("Amiri-{style}")) {
            failures.push(format!(
                "{style} {word}: {faces:?}\n  got      {got:?}\n  expected {expected:?}"
            ));
        }
    }
    assert!(cases >= 600, "only {cases} cases");
    assert!(
        failures.is_empty(),
        "{} of {cases} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
