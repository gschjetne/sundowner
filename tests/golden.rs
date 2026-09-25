//! Golden layout tests: check what text is drawn, in which font, where, and
//! which links and bookmarks are produced for small known documents.

use sundowner::layout::{self, Options, Output, Target};
use sundowner::markdown;

/// One `show_text` call: font number, position and decoded (WinAnsi) text.
#[derive(Debug)]
struct Run {
    font: usize,
    x: f32,
    y: f32,
    text: Vec<u8>,
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

/// Parse the `BT r g b rg /Fn size Tf x y Td (text) Tj ET` runs of a page.
fn runs(out: &Output, page: usize) -> Vec<Run> {
    let ops = &out.pages[page].ops;
    let mut result = Vec::new();
    let mut i = 0;
    while let Some(p) = find(ops, i, b"BT ") {
        let end = find(ops, p, b" Tj ET\n").expect("unterminated text object");
        let obj = &ops[p..end];
        let f = find(obj, 0, b"/F").unwrap() + 2;
        let tokens: Vec<&[u8]> = obj[f..].splitn(6, |&b| b == b' ').collect();
        let num = |t: &[u8]| std::str::from_utf8(t).unwrap().parse::<f32>().unwrap();
        let font = num(tokens[0]) as usize;
        let (x, y) = (num(tokens[3]), num(tokens[4]));
        let lit = &obj[find(obj, 0, b"(").unwrap() + 1..obj.len() - 1];
        let mut text = Vec::new();
        let mut k = 0;
        while k < lit.len() {
            if lit[k] == b'\\' {
                k += 1;
            }
            text.push(lit[k]);
            k += 1;
        }
        result.push(Run { font, x, y, text });
        i = end;
    }
    result
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn run<'a>(runs: &'a [Run], text: &str) -> &'a Run {
    runs.iter()
        .find(|r| r.text == text.as_bytes())
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
    // Headings are bold (F2), body text regular (F1), and flow downwards.
    assert_eq!(run(&r, "Alpha").font, 2);
    assert_eq!(run(&r, "text").font, 1);
    assert!(run(&r, "Alpha").y > run(&r, "text").y);
    assert!(run(&r, "text").y > run(&r, "Gamma").y);
}

#[test]
fn inline_styles_use_the_right_fonts() {
    let r = runs(&render("plain **bold** *italic* ***both*** `code`"), 0);
    assert_eq!(run(&r, "plain").font, 1);
    assert_eq!(run(&r, "bold").font, 2);
    assert_eq!(run(&r, "italic").font, 3);
    assert_eq!(run(&r, "both").font, 4);
    assert_eq!(run(&r, "code").font, 5);
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
    // Bullet (0x95) for level one, en dash for level two, "1." for the ordered list.
    let bullet = r.iter().find(|x| x.text == [0x95]).unwrap();
    assert!(bullet.x < one.x && bullet.y == one.y);
    let dash = r.iter().find(|x| x.text == [0x96]).unwrap();
    assert!(dash.x < two.x && dash.y == two.y);
    let n = run(&r, "1.");
    assert!(n.x < three.x && n.y == three.y);
}

#[test]
fn tables_align_columns() {
    let out = render("| Left | Right |\n|:-----|------:|\n| a | 1 |\n| bb | 22222 |\n");
    let r = runs(&out, 0);
    assert_eq!(run(&r, "Left").font, 2, "header is bold");
    assert_eq!(run(&r, "a").font, 1);
    // Left column shares its left edge; right column shares its right edge.
    assert_eq!(run(&r, "a").x, run(&r, "bb").x);
    assert_eq!(run(&r, "Left").x, run(&r, "a").x);
    let w = |s: &str| s.len() as f32 * 556.0 * 11.0 * 0.92 / 1000.0; // Helvetica digits
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
