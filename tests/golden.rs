//! Golden layout tests: check what text is drawn, in which font, where, and
//! which links and bookmarks are produced for small known documents.

use sundowner::fonts::Fonts;
use sundowner::layout::{self, Options, Output, Target};
use sundowner::markdown;

/// One text-showing operation: face, position and the text it draws.
#[derive(Debug)]
struct Run {
    face: usize,
    x: f32,
    y: f32,
    text: String,
}

fn render(src: &str) -> Output {
    let doc = markdown::parse(src);
    layout::layout(
        &doc,
        &Options {
            page_numbers: false,
            ..Options::default()
        },
    )
}

/// Parse the `BT r g b rg /Fn size Tf x y Td <glyphs> Tj ET` runs of a page,
/// mapping glyph IDs back to characters.
fn runs(out: &Output, page: usize) -> Vec<Run> {
    let ops = &out.pages[page].ops;
    let mut result = Vec::new();
    let mut i = 0;
    while let Some(p) = find(ops, i, b"BT ") {
        let end = find(ops, p, b" Tj ET\n").expect("unterminated text object");
        let obj = std::str::from_utf8(&ops[p..end]).unwrap();
        let f = obj.find("/F").unwrap() + 2;
        let tokens: Vec<&str> = obj[f..].split(' ').collect();
        let face: usize = tokens[0].parse().unwrap();
        let (x, y) = (tokens[3].parse().unwrap(), tokens[4].parse().unwrap());
        let hex = &obj[obj.find('<').unwrap() + 1..obj.find('>').unwrap()];
        let text = (0..hex.len() / 4)
            .map(|k| u16::from_str_radix(&hex[4 * k..4 * k + 4], 16).unwrap())
            .map(|g| out.used[face].get(&g).copied().unwrap_or('\u{FFFD}'))
            .collect();
        result.push(Run { face, x, y, text });
        i = end;
    }
    result
}

/// Face IDs of the bundled fonts, in load order.
const REGULAR: usize = 0;
const BOLD: usize = 1;
const ITALIC: usize = 2;
const BOLD_ITALIC: usize = 3;
const MONO: usize = 4;
const MONO_ITALIC: usize = 6;
const MONO_BOLD_ITALIC: usize = 7;

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn run<'a>(runs: &'a [Run], text: &str) -> &'a Run {
    runs.iter()
        .find(|r| r.text == text)
        .unwrap_or_else(|| panic!("no run {text:?} in {runs:?}"))
}

#[test]
fn headings_make_outline_and_anchors() {
    let out = render("# Alpha\n\ntext\n\n## Beta Two\n\n### Gamma\n\n## Beta Two\n");
    let h: Vec<(u8, &str)> = out.headings.iter().map(|h| (h.level, h.title.as_str())).collect();
    assert_eq!(h, [(1, "Alpha"), (2, "Beta Two"), (3, "Gamma"), (2, "Beta Two")]);
    for a in ["alpha", "beta-two", "gamma", "beta-two-1"] {
        assert!(out.anchors.contains_key(a), "missing anchor {a}");
    }
    assert_eq!(out.title.as_deref(), Some("Alpha"));
    let r = runs(&out, 0);
    // Headings are bold, body text regular, and they flow downwards.
    assert_eq!(run(&r, "Alpha").face, BOLD);
    assert_eq!(run(&r, "text").face, REGULAR);
    assert!(run(&r, "Alpha").y > run(&r, "text").y);
    assert!(run(&r, "text").y > run(&r, "Gamma").y);
}

#[test]
fn inline_styles_use_the_right_fonts() {
    let r = runs(&render("plain **bold** *italic* ***both*** `code`"), 0);
    assert_eq!(run(&r, "plain").face, REGULAR);
    assert_eq!(run(&r, "bold").face, BOLD);
    assert_eq!(run(&r, "italic").face, ITALIC);
    assert_eq!(run(&r, "both").face, BOLD_ITALIC);
    assert_eq!(run(&r, "code").face, MONO);
    // Words on one line share a baseline and advance left to right.
    assert_eq!(run(&r, "plain").y, run(&r, "code").y);
    assert!(run(&r, "plain").x < run(&r, "bold").x && run(&r, "bold").x < run(&r, "code").x);
}

#[test]
fn nested_lists_are_indented_with_markers() {
    let r = runs(&render("- one\n  - two\n    1. three\n- four\n"), 0);
    let (one, two, three, four) = (run(&r, "one"), run(&r, "two"), run(&r, "three"), run(&r, "four"));
    assert!(one.x < two.x && two.x < three.x);
    assert_eq!(one.x, four.x);
    assert!(one.y > two.y && two.y > three.y && three.y > four.y);
    // Bullet for level one, en dash for level two, "1." for the ordered list.
    let bullet = run(&r, "\u{2022}");
    assert!(bullet.x < one.x && bullet.y == one.y);
    let dash = run(&r, "\u{2013}");
    assert!(dash.x < two.x && dash.y == two.y);
    let n = run(&r, "1.");
    assert!(n.x < three.x && n.y == three.y);
}

#[test]
fn tables_align_columns() {
    let out = render("| Left | Right |\n|:-----|------:|\n| a | 1 |\n| bb | 22222 |\n");
    let r = runs(&out, 0);
    assert_eq!(run(&r, "Left").face, BOLD, "header is bold");
    assert_eq!(run(&r, "a").face, REGULAR);
    // Left column shares its left edge; right column shares its right edge.
    assert_eq!(run(&r, "a").x, run(&r, "bb").x);
    assert_eq!(run(&r, "Left").x, run(&r, "a").x);
    // Right-aligned cells end at the same x. Measure with the font's own
    // advance widths.
    let fonts = Fonts::builtin();
    let w = |s: &str| -> f32 {
        s.chars()
            .map(|c| fonts.width(REGULAR, fonts.faces[REGULAR].glyph(c).unwrap(), 11.0 * 0.92))
            .sum()
    };
    let right_edge = |s: &str| run(&r, s).x + w(s);
    assert!((right_edge("1") - right_edge("22222")).abs() < 0.05);
    // Rows go down the page.
    assert!(run(&r, "Left").y > run(&r, "a").y && run(&r, "a").y > run(&r, "bb").y);
}

#[test]
fn internal_and_external_links() {
    let out = render("[go](#target) [web](https://example.com) [js](javascript:alert(1))\n\n# Target\n");
    let links = &out.pages[0].links;
    assert!(matches!(&links[0].target, Target::Anchor(a) if a == "target"));
    assert!(matches!(&links[1].target, Target::Uri(u) if u == "https://example.com"));
    assert!(out.anchors.contains_key("target"));

    let pdf = sundowner::convert(
        "[go](#target) [web](https://example.com) [js](javascript:alert(1)) [bad](#nowhere)\n\n# Target\n",
        &Options::default(),
    )
    .pdf;
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(
        text.matches("/Subtype /Link").count(),
        2,
        "only #target and https links are clickable"
    );
    assert!(text.contains("/URI (https://example.com)"));
    assert!(!text.contains("javascript"));
    assert!(text.contains("/Dest ["));
}

#[test]
fn long_documents_break_pages() {
    let src = "paragraph line\n\n".repeat(200);
    let out = render(&src);
    assert!(out.pages.len() > 3);
    for p in 0..out.pages.len() {
        for r in runs(&out, p) {
            assert!(r.y > 50.0 && r.y < 842.0 - 50.0, "text outside margins: {r:?}");
        }
    }
}

#[test]
fn every_script_in_the_bundled_fonts_uses_real_glyphs() {
    let out = render("Καλημέρα Съешь Łódź “q” € `код κώδικας Łódź ἀρχὴ` *`курсив`* **_`ᾠδή`_**");
    let r = runs(&out, 0);
    assert_eq!(run(&r, "Καλημέρα").face, REGULAR);
    assert_eq!(run(&r, "Съешь").face, REGULAR);
    assert_eq!(run(&r, "Łódź").face, REGULAR);
    // Code in every script and style stays in Cousine; nothing falls back.
    assert_eq!(run(&r, "код").face, MONO);
    assert_eq!(run(&r, "κώδικας").face, MONO);
    assert_eq!(run(&r, "курсив").face, MONO_ITALIC);
    assert_eq!(run(&r, "ᾠδή").face, MONO_BOLD_ITALIC);
    assert_eq!(run(&r, "ἀρχὴ").face, MONO);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[test]
fn uncovered_characters_warn_and_use_notdef() {
    let out = render("日本 🎉");
    assert_eq!(out.warnings.len(), 1);
    assert!(out.warnings[0].contains("U+65E5") && out.warnings[0].contains("U+1F389"));
    let r = runs(&out, 0);
    assert!(r.iter().all(|x| x.face == REGULAR));
    // The glyph is .notdef (ID 0), which maps back to the first missing character.
    assert!(out.used[REGULAR].contains_key(&0));
}

#[test]
fn fonts_are_embedded_subsets_with_unicode_maps() {
    let pdf = sundowner::convert("Hello **bold** `code`", &Options::default()).pdf;
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(
        text.matches("/Subtype /Type0").count(),
        3,
        "regular, bold and mono"
    );
    assert_eq!(text.matches("/FontFile2").count(), 3);
    assert_eq!(text.matches("/ToUnicode").count(), 3);
    assert!(!text.contains("/Type1"), "no base-14 fonts");
    assert!(text.contains("+AlegreyaRoman-Regular") && text.contains("+Cousine-Regular"));
    // Same input, same bytes.
    assert_eq!(
        pdf,
        sundowner::convert("Hello **bold** `code`", &Options::default()).pdf
    );
}
