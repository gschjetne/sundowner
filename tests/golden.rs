//! Golden layout tests: check what text is drawn, in which font, where, and
//! which links and bookmarks are produced for small known documents.

use sundowner::fonts::Fonts;
use sundowner::layout::{self, Options, Output, Target};
use sundowner::markdown;
use sundowner::tags::Kid;

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
        let end = find(ops, p, b" ET\n").expect("unterminated text object");
        let obj = std::str::from_utf8(&ops[p..end]).unwrap();
        let f = obj.find("/F").unwrap() + 2;
        let tokens: Vec<&str> = obj[f..].split(' ').collect();
        let face: usize = tokens[0].parse().unwrap();
        let (x, y) = (tokens[3].parse().unwrap(), tokens[4].parse().unwrap());
        // `<hex> Tj`, or `[<hex> kern <hex> ...] TJ` with kerning.
        let shown = &obj[obj.find('<').unwrap()..=obj.rfind('>').unwrap()];
        let hex: String = shown
            .split('<')
            .filter_map(|part| part.split('>').next())
            .collect();
        let text = (0..hex.len() / 4)
            .map(|k| u16::from_str_radix(&hex[4 * k..4 * k + 4], 16).unwrap())
            .map(|g| out.used[face].get(&g).map_or("\u{FFFD}", String::as_str))
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

/// The text of a PDF file, followed by that of its object streams
/// (inflated), where most of its objects are.
fn pdf_text(pdf: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(pdf).into_owned();
    let mut from = 0;
    while let Some(p) = find(pdf, from, b"/Type /ObjStm") {
        let at = find(pdf, p, b"/Length ").expect("a length") + 8;
        let end = find(pdf, at, b" ").expect("a number");
        let len: usize = std::str::from_utf8(&pdf[at..end]).unwrap().parse().unwrap();
        let start = find(pdf, end, b"stream\n").expect("a stream") + 7;
        let data = sundowner::flate::zlib_decompress(&pdf[start..start + len], 1 << 30).unwrap();
        text.push_str(&String::from_utf8_lossy(&data));
        from = start + len;
    }
    text
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
    let text = pdf_text(&pdf);
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

/// The page each run of `text` is on.
fn pages_of(out: &Output, text: &str) -> Vec<usize> {
    (0..out.pages.len())
        .flat_map(|p| {
            runs(out, p)
                .into_iter()
                .filter(|r| r.text == text)
                .map(move |_| p)
        })
        .collect()
}

/// Paragraphs that fill most of the first page.
fn filler(n: usize) -> String {
    (0..n).map(|k| format!("filler{k}\n\n")).collect()
}

#[test]
fn code_blocks_that_fit_on_the_next_page_are_not_split() {
    // With 28 paragraphs above it, the code block does not fit on the first
    // page; it moves to the second page whole instead of being split.
    let code: String = (0..10).map(|k| format!("line{k}\n")).collect();
    let out = render(&format!("{}```\n{code}```\n\nafter\n", filler(28)));
    assert_eq!(out.pages.len(), 2);
    assert_eq!(pages_of(&out, "filler27"), [0]);
    for k in 0..10 {
        assert_eq!(pages_of(&out, &format!("line{k}")), [1], "line{k}");
    }
}

#[test]
fn headings_stay_with_the_text_after_them() {
    for n in 26..32 {
        let out = render(&format!(
            "{}## Heading\n\nSectionstart and more words.\n",
            filler(n)
        ));
        let heading = pages_of(&out, "Heading");
        assert_eq!(heading.len(), 1);
        assert_eq!(heading, pages_of(&out, "Sectionstart"), "{n} paragraphs");
    }
}

#[test]
fn page_breaks_use_space_left_at_the_end() {
    // A long code block is split only where it has to be.
    let code: String = (0..80).map(|k| format!("line{k}\n")).collect();
    let out = render(&format!("{}```\n{code}```\n", filler(20)));
    // How many lines of code a page holds.
    let full = render(&format!("```\n{code}```\n"));
    let per_page = (0..80)
        .filter(|k| pages_of(&full, &format!("line{k}")) == [0])
        .count();
    assert!(per_page < 80);
    // The block starts at the top of the second page, fills it, and the
    // rest goes on the third: it is split once, where it has to be.
    let page: Vec<usize> = (0..80)
        .map(|k| pages_of(&out, &format!("line{k}"))[..][0])
        .collect();
    let expected: Vec<usize> = (0..80).map(|k| if k < per_page { 1 } else { 2 }).collect();
    assert_eq!(page, expected);
    assert_eq!(out.pages.len(), 3);
}

#[test]
fn table_rows_taller_than_half_a_page_are_split() {
    // A row of a few lines moves to the next page whole rather than being
    // split; a row taller than half the text area is split like a quote,
    // so it starts right after a few paragraphs above it rather than
    // leaving most of their page empty.
    let row = |words: usize| -> String {
        let text: Vec<String> = (0..words).map(|k| format!("w{k}")).collect();
        format!("| a | b |\n|---|---|\n| start | {} |\n", text.join(" "))
    };
    let mut moved = false;
    for n in 20..34 {
        let out = render(&format!("{}{}", filler(n), row(60)));
        let start = pages_of(&out, "start");
        assert_eq!(start, pages_of(&out, "w59"), "{n} paragraphs");
        moved |= start != pages_of(&out, &format!("filler{}", n - 1));
    }
    assert!(moved);
    let out = render(&format!("{}{}", filler(12), row(800)));
    assert_eq!(pages_of(&out, "filler11"), [0]);
    assert_eq!(pages_of(&out, "start"), [0]);
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
fn uncovered_characters_warn_and_show_as_boxes() {
    let out = render("กข 🎉");
    assert_eq!(out.warnings.len(), 1);
    assert!(out.warnings[0].contains("U+0E01") && out.warnings[0].contains("U+1F389"));
    // They are set in the regular face as .notdef (ID 0), but drawn as the
    // outline of a box, as PDF/A does not allow .notdef to be shown...
    assert!(out.used[REGULAR].contains_key(&0));
    let ops = String::from_utf8_lossy(&out.pages[0].ops).to_string();
    assert!(!ops.contains("<0000>"), "{ops}");
    assert_eq!(ops.matches(" re S\n").count(), 3, "{ops}");
    // ...each with an invisible space that carries its text.
    let space = format!("<{:04X}>", Fonts::builtin().faces[REGULAR].glyph(' ').unwrap());
    assert_eq!(ops.matches(&format!("{space} Tj 0 Tr")).count(), 3, "{ops}");
    for text in ["<FEFF0E01>", "<FEFF0E02>", "<FEFFD83CDF89>"] {
        assert!(ops.contains(&format!("/ActualText {text}")), "{text} in {ops}");
    }
}

#[test]
fn fonts_are_embedded_subsets_with_unicode_maps() {
    let pdf = sundowner::convert("Hello **bold** `code`", &Options::default()).pdf;
    let text = pdf_text(&pdf);
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

#[test]
fn kerning_is_applied_and_measured() {
    let out = render("AVATAR Tolstoy");
    let ops = String::from_utf8_lossy(&out.pages[0].ops);
    // AVATAR kerns A-V (-42), V-A, A-T, T-A: shown with a TJ array.
    assert!(ops.contains("[<"), "{ops}");
    assert!(ops.contains(">42<"), "A-V kerning of -42/1000 em: {ops}");
    let r = runs(&out, 0);
    // The next word starts where the kerned word ends plus a space, so the
    // measured width includes the kerning.
    let fonts = Fonts::builtin();
    let face = &fonts.faces[REGULAR];
    let advance = |s: &str| -> f32 {
        s.chars()
            .map(|c| fonts.width(REGULAR, face.glyph(c).unwrap(), 11.0))
            .sum()
    };
    let gap = run(&r, "Tolstoy").x - run(&r, "AVATAR").x;
    let unkerned = advance("AVATAR ");
    assert!(gap < unkerned - 1.0, "kerned {gap} vs unkerned {unkerned}");
}

#[test]
fn monospace_code_is_never_kerned() {
    let out = render("`AVATAR To`");
    let ops = String::from_utf8_lossy(&out.pages[0].ops);
    assert!(!ops.contains("TJ"), "{ops}");
}

fn render_narrow(src: &str, width: f32) -> Output {
    let doc = markdown::parse(src);
    layout::layout(
        &doc,
        &Options {
            page_numbers: false,
            page_width: width,
            margin: 10.0,
            ..Options::default()
        },
    )
}

/// The runs of a page grouped into lines (by baseline), top to bottom, each
/// line's runs joined left to right.
fn lines(out: &Output) -> Vec<String> {
    let mut r = runs(out, 0);
    r.sort_by(|a, b| b.y.total_cmp(&a.y).then(a.x.total_cmp(&b.x)));
    let mut lines: Vec<(f32, String)> = Vec::new();
    for run in r {
        match lines.last_mut() {
            // A hyphen shown where a line breaks inside a word is drawn on
            // its own (with the soft hyphen as its text).
            Some((y, text)) if *y == run.y && run.text == "-" => text.push('-'),
            Some((y, text)) if *y == run.y => {
                text.push(' ');
                text.push_str(&run.text);
            }
            _ => lines.push((run.y, run.text)),
        }
    }
    lines.into_iter().map(|l| l.1).collect()
}

#[test]
fn ligatures_are_formed_and_map_back_to_text() {
    let out = render("office fjord *fifty*");
    let r = runs(&out, 0);
    // Runs map back to the original text through the ligature glyphs...
    assert_eq!(run(&r, "office").face, REGULAR);
    assert_eq!(run(&r, "fjord").face, REGULAR);
    assert_eq!(run(&r, "fifty").face, ITALIC);
    // ...which each stand for several characters.
    for (face, lig) in [(REGULAR, "fi"), (REGULAR, "fj"), (ITALIC, "fi")] {
        assert!(
            out.used[face].values().any(|t| t == lig),
            "no {lig} ligature in {:?}",
            out.used[face]
        );
    }
    // A ZERO WIDTH NON-JOINER prevents the ligature.
    let out = render("shelf\u{200C}ish");
    assert!(!out.used[REGULAR].values().any(|t| t.chars().count() > 1));
    assert_eq!(run(&runs(&out, 0), "shelfish").face, REGULAR);
    // Code has no ligatures to begin with.
    let out = render("`office`");
    assert!(!out.used[MONO].values().any(|t| t.chars().count() > 1));
}

#[test]
fn lines_break_by_the_unicode_rules() {
    // Hyphens and dashes are break opportunities even without spaces.
    let l = lines(&render_narrow("state-of-the-art-technology well—known", 70.0));
    assert!(l.len() >= 3, "{l:?}");
    assert!(l[0].ends_with('-'), "{l:?}");
    assert_eq!(
        l.concat().replace(' ', ""),
        "state-of-the-art-technologywell—known"
    );
    // No break before closing punctuation, even after a space, nor after
    // an opening bracket.
    for width in [40.0, 50.0, 60.0, 70.0, 80.0] {
        let l = lines(&render_narrow("aaa bbb ccc ! ( ddd ) eee", width));
        for line in &l {
            assert!(!line.starts_with('!') && !line.starts_with(')'), "{l:?}");
            assert!(!line.ends_with('('), "{l:?}");
        }
    }
    // No-break spaces keep words together.
    let l = lines(&render_narrow("aaaaaa bbbbbb\u{A0}cccccc", 120.0));
    assert!(l.iter().any(|x| x.contains("bbbbbb\u{A0}cccccc")), "{l:?}");
}

#[test]
fn soft_hyphens_show_only_where_the_line_breaks() {
    let src = "extra\u{AD}ordinarily incompre\u{AD}hensibilities";
    let l = lines(&render(src));
    assert_eq!(l, ["extraordinarily incomprehensibilities"]);
    let l = lines(&render_narrow(src, 110.0));
    assert!(l.len() >= 2, "{l:?}");
    let hyphenated: Vec<&String> = l.iter().filter(|x| x.contains('-')).collect();
    assert!(
        !hyphenated.is_empty() && hyphenated.iter().all(|x| x.ends_with('-')),
        "{l:?}"
    );
    assert_eq!(
        l.concat().replace('-', ""),
        "extraordinarily incomprehensibilities".replace(' ', "")
    );
}

#[test]
fn combining_marks_compose_or_attach() {
    // e + combining acute is set as the precomposed é, and copies as é.
    let out = render("cafe\u{301}");
    let r = runs(&out, 0);
    assert_eq!(run(&r, "café").face, REGULAR);
    let e_acute = Fonts::builtin().faces[REGULAR].glyph('é').unwrap();
    assert_eq!(out.used[REGULAR].get(&e_acute).map(String::as_str), Some("é"));
    // Marks in non-canonical order compose the same way: ệ.
    let out = render("e\u{302}\u{323} e\u{323}\u{302}");
    let r = runs(&out, 0);
    assert!(r.iter().all(|x| x.text == "ệ"), "{r:?}");
    // Without a precomposed character, the mark is positioned on its base
    // by the font's anchors: above a capital with text rise.
    let out = render("Q\u{303}");
    let ops = String::from_utf8_lossy(&out.pages[0].ops).to_string();
    assert!(ops.contains(" Ts [") && ops.contains("] TJ 0 Ts ET"), "{ops}");
    assert_eq!(run(&runs(&out, 0), "Q\u{303}").face, REGULAR);
    // Over a capital, the font's contextual substitutions use a flatter
    // tilde, which still copies as the combining tilde.
    let tilde = Fonts::builtin().faces[REGULAR].glyph('\u{303}').unwrap();
    assert!(!out.used[REGULAR].contains_key(&tilde));
    assert!(out.used[REGULAR].values().any(|t| t == "\u{303}"));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[test]
fn joined_words_kern_past_trailing_marks() {
    // A soft hyphen that does not break joins "A̱" and "VATAR" on one line.
    // The A-V pair must still be kerned although "A̱" ends with a combining
    // mark (U+0331 has no precomposed form with A), so the next word lands
    // exactly where it does without the mark.
    let x_after = |src: &str| run(&runs(&render(src), 0), "x").x;
    let with_mark = x_after("A\u{331}\u{AD}VATAR x");
    let without = x_after("A\u{AD}VATAR x");
    assert!((with_mark - without).abs() < 0.01, "{with_mark} vs {without}");
}

/// The text of a run as read in logical order, for right-to-left runs
/// (whose glyphs are drawn, and so mapped back, in visual order).
fn logical(s: &str) -> String {
    s.chars().rev().collect()
}

fn rtl_run<'a>(runs: &'a [Run], text: &str) -> &'a Run {
    run(runs, &logical(text))
}

#[test]
fn right_to_left_paragraphs_are_reordered_and_flush_right() {
    let out = render("שלום עולם\n\nabc");
    let r = runs(&out, 0);
    let (first, second) = (rtl_run(&r, "שלום"), rtl_run(&r, "עולם"));
    // The first word is on the right, and the paragraph ends at the right
    // margin while the Latin one starts at the left.
    assert!(first.x > second.x, "{r:?}");
    assert!(second.x > run(&r, "abc").x + 300.0, "{r:?}");
    let margin = Options::default().margin;
    let right = Options::default().page_width - margin;
    assert_eq!(run(&r, "abc").x, margin);
    assert!(first.x < right && first.x > right - 50.0, "{r:?}");
}

#[test]
fn mixed_directions_follow_the_bidi_algorithm() {
    // Hebrew inside an English paragraph: the Hebrew words run right to
    // left between the English ones.
    let l = lines(&render("abc שלום עולם def"));
    assert_eq!(l, [format!("abc {} {} def", logical("עולם"), logical("שלום"))]);
    // Numbers and Latin inside Hebrew stay left to right, and brackets are
    // mirrored so they still enclose their text.
    // (Runs split where the direction changes, so compare without spaces.)
    let l = lines(&render("שלום 123 (abc) עולם"))[0].replace(' ', "");
    assert_eq!(l, format!("{}(abc)123{}", logical("עולם"), logical("שלום")));
    let l = lines(&render("א (ב) ג"))[0].replace(' ', "");
    assert_eq!(l, "ג(ב)א");
}

#[test]
fn right_to_left_lists_and_quotes_are_mirrored() {
    let out = render("- שלום\n- abc\n\n> עולם\n");
    let r = runs(&out, 0);
    let bullets: Vec<&Run> = r.iter().filter(|x| x.text == "\u{2022}").collect();
    assert_eq!(bullets.len(), 2);
    // The Hebrew item has its bullet on the right, the Latin one on the left.
    assert!(bullets[0].x > rtl_run(&r, "שלום").x);
    assert!(bullets[1].x < run(&r, "abc").x);
    // The quote is indented from the right: it ends left of the bar's side.
    let margin = Options::default().margin;
    let right = Options::default().page_width - margin;
    assert!(rtl_run(&r, "עולם").x < right - 15.0);
    // Its bar is on the right.
    let ops = String::from_utf8_lossy(&out.pages[0].ops).to_string();
    let bar = ops.lines().find(|l| l.ends_with("re f")).expect("quote bar");
    let bar_x: f32 = bar.split(' ').nth_back(5).unwrap().parse().unwrap();
    assert!(bar_x > right - 10.0, "{bar}");
}

#[test]
fn code_stays_left_to_right_with_right_to_left_parts_reordered() {
    let l = lines(&render("```\nx = \"שלום עולם\"\n```\n"))[0].replace(' ', "");
    assert_eq!(l, format!("x=\"{}{}\"", logical("עולם"), logical("שלום")));
}

/// The silk tier sets each of its scripts in its own family, in regular and
/// bold.
#[cfg(feature = "silk")]
#[test]
fn silk_sets_each_script_in_its_family() {
    let out = render("שלום **שלום** Հայերեն 中文 ひらがな 한국어 **中文**");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let r = runs(&out, 0);
    let name = |run: &Run| out.fonts.faces[run.face].postscript_name.clone();
    let mut hebrew: Vec<String> = r.iter().filter(|x| x.text == logical("שלום")).map(name).collect();
    hebrew.sort();
    assert_eq!(hebrew, ["FrankRuhlLibre-Bold", "FrankRuhlLibre-Regular"]);
    assert_eq!(name(run(&r, "Հայերեն")), "NotoSerifArmenian-Regular");
    assert_eq!(name(run(&r, "ひらがな")), "NotoSerifTC-Regular");
    assert_eq!(name(run(&r, "한국어")), "GowunBatang-Regular");
    let mut han: Vec<String> = r.iter().filter(|x| x.text == "中文").map(name).collect();
    han.sort();
    assert_eq!(han, ["NotoSerifTC-Bold", "NotoSerifTC-Regular"]);
    // Cantillation marks, which Frank Ruhl Libre lacks, come from Noto
    // Serif Hebrew together with their letter.
    let out = render("בָּרָ֣א");
    let r = runs(&out, 0);
    assert!(r
        .iter()
        .any(|x| name_of(&out, x) == "NotoSerifHebrew-Regular" && x.text.contains('\u{5A3}')));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[cfg(feature = "silk")]
fn name_of(out: &Output, run: &Run) -> String {
    out.fonts.faces[run.face].postscript_name.clone()
}

/// Arabic is set in Amiri, in all four styles, with its letters joined,
/// also across a change of style, and with the punctuation around it.
#[cfg(feature = "silk")]
#[test]
fn arabic_is_joined_and_set_in_amiri() {
    let out = render("كتب **كتب** *كتب* ***كتب***");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let r = runs(&out, 0);
    let mut names: Vec<String> = r
        .iter()
        .filter(|x| x.text == logical("كتب"))
        .map(|x| name_of(&out, x))
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["Amiri-Bold", "Amiri-BoldItalic", "Amiri-Italic", "Amiri-Regular"]
    );
    // Joined: none of the letters is drawn with its isolated glyph.
    let word = r.iter().find(|x| name_of(&out, x) == "Amiri-Regular").unwrap();
    let face = &out.fonts.faces[word.face];
    for c in "كتب".chars() {
        let nominal = face.glyph(c).unwrap();
        assert!(!out.used[word.face].contains_key(&nominal), "{c} is not joined");
    }
    // A bold last letter still joins to the regular ones before it: it
    // is drawn in its final form, not the isolated one.
    let out = render("كت**ب** ب");
    let r = runs(&out, 0);
    let bold = r.iter().find(|x| name_of(&out, x) == "Amiri-Bold").unwrap();
    let isolated = out.fonts.faces[bold.face].glyph('ب').unwrap();
    assert!(!out.used[bold.face].contains_key(&isolated));
    // Punctuation next to Arabic comes from Amiri, not Alegreya.
    let out = render("مرحبا.");
    let r = runs(&out, 0);
    assert!(r.iter().all(|x| name_of(&out, x) == "Amiri-Regular"), "{r:?}");
    // Right to left and flush right, with Arabic-Indic digits left to right.
    let l = lines(&render("العدد ١٢٣"))[0].replace(' ', "");
    assert_eq!(l, format!("١٢٣{}", logical("العدد")));
}

/// Chinese characters are set in their traditional forms, Japanese kanji
/// that Noto Serif TC lacks in Noto Serif JP, and characters only
/// simplified Chinese uses are missing (a simplified Chinese font is added
/// as a fallback).
#[cfg(feature = "silk")]
#[test]
fn chinese_is_traditional_and_japanese_is_complete() {
    // 天 is in Noto Serif TC; 気 (Japanese for 氣) only in Noto Serif JP.
    let out = render("天気 這們");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let names: Vec<(String, String)> = runs(&out, 0)
        .iter()
        .map(|x| (x.text.clone(), name_of(&out, x)))
        .collect();
    let set_in = |text: &str| names.iter().find(|n| n.0 == text).map(|n| n.1.as_str());
    assert_eq!(set_in("天"), Some("NotoSerifTC-Regular"));
    assert_eq!(set_in("気"), Some("NotoSerifJP-Regular"));
    assert_eq!(set_in("這們"), Some("NotoSerifTC-Regular"));
    let out = render("这们");
    assert_eq!(out.warnings.len(), 1);
    assert!(out.warnings[0].contains("'这' (U+8FD9)"), "{}", out.warnings[0]);
}

fn render_front(src: &str, front_matter: Option<bool>) -> Output {
    let doc = markdown::parse(src);
    layout::layout(
        &doc,
        &Options {
            page_numbers: false,
            front_matter,
            ..Options::default()
        },
    )
}

const FRONT: &str = "---\ntitle: Notes\nauthor:\n  name: Ada\n  role: Writer\n---\n\nBody\n";

#[test]
fn front_matter_is_left_out_with_a_warning_unless_asked_for() {
    let out = render_front(FRONT, None);
    assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    assert!(out.warnings[0].contains("--front-matter"));
    let texts: Vec<String> = runs(&out, 0).into_iter().map(|r| r.text).collect();
    assert_eq!(texts, ["Body"]);

    let out = render_front(FRONT, Some(false));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let texts: Vec<String> = runs(&out, 0).into_iter().map(|r| r.text).collect();
    assert_eq!(texts, ["Body"]);

    // Empty front matter needs no warning.
    assert!(render_front("---\n---\nBody\n", None).warnings.is_empty());
}

#[test]
fn front_matter_is_a_table_with_spanning_keys() {
    let out = render_front(FRONT, Some(true));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let r = runs(&out, 0);
    let (title, notes, author, name, ada, role, writer, body) = (
        run(&r, "title"),
        run(&r, "Notes"),
        run(&r, "author"),
        run(&r, "name"),
        run(&r, "Ada"),
        run(&r, "role"),
        run(&r, "Writer"),
        run(&r, "Body"),
    );
    // Keys are bold, values regular.
    assert_eq!((title.face, author.face, name.face), (BOLD, BOLD, BOLD));
    assert_eq!((notes.face, ada.face, writer.face), (REGULAR, REGULAR, REGULAR));
    // "author" spans the rows of its mapping, whose keys are one column in.
    assert_eq!(title.x, author.x);
    assert_eq!(author.y, name.y);
    assert_eq!(name.x, role.x);
    assert!(name.x > author.x && ada.x > name.x && ada.x == writer.x);
    assert!(name.y > role.y);
    // The top-level value spans the columns to the right edge.
    assert_eq!(notes.x, name.x);
    assert!(title.y > author.y && role.y > body.y);
}

#[test]
fn front_matter_too_deep_or_unreadable_is_left_out_with_a_warning() {
    // Nine columns; seven fit on A4.
    let deep: String = (0..8).map(|k| format!("{}k{k}:\n", "  ".repeat(k))).collect();
    let deep = format!("---\n{deep}{}x: y\n---\nBody\n", "  ".repeat(8));
    let out = render_front(&deep, Some(true));
    assert!(
        out.warnings[0].contains("nested too deeply"),
        "{:?}",
        out.warnings
    );
    assert_eq!(runs(&out, 0).len(), 1);

    let bad = "---\na: &anchor x\n---\nBody\n";
    let out = render_front(bad, Some(true));
    assert!(out.warnings[0].contains("line 2: anchors"), "{:?}", out.warnings);
    assert_eq!(runs(&out, 0).len(), 1);
}

#[test]
fn front_matter_that_needs_several_pages_breaks_between_rows() {
    let mut src = String::from("---\nlist:\n");
    for k in 0..200 {
        src.push_str(&format!("  - name: item{k}\n    value: v{k}\n"));
    }
    src.push_str("---\nBody\n");
    let out = render_front(&src, Some(true));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert!(out.pages.len() > 2);
    // Each item's two rows stay together.
    for k in 0..200 {
        assert_eq!(
            pages_of(&out, &format!("item{k}")),
            pages_of(&out, &format!("v{k}")),
            "item {k}"
        );
    }
}

/// The width of `text` set in the regular body font at the default size.
fn width(text: &str) -> f32 {
    let o = Options::default();
    let em: f32 = layout::shape_text(text, false, false, false, &o)
        .iter()
        .flat_map(|(_, g)| g)
        .map(|g| g.1)
        .sum();
    em * o.font_size / 1000.0
}

/// Each line of the first page, top to bottom: where its first run starts
/// and its last one (by position) ends, and its text.
fn line_edges(out: &Output) -> Vec<(f32, f32, String)> {
    let mut r = runs(out, 0);
    r.sort_by(|a, b| b.y.total_cmp(&a.y).then(a.x.total_cmp(&b.x)));
    let mut lines: Vec<(f32, Vec<&Run>)> = Vec::new();
    for run in &r {
        match lines.last_mut() {
            Some((y, l)) if *y == run.y => l.push(run),
            _ => lines.push((run.y, vec![run])),
        }
    }
    lines
        .into_iter()
        .map(|(_, l)| {
            let last = l[l.len() - 1];
            let text: Vec<&str> = l.iter().map(|r| r.text.as_str()).collect();
            (l[0].x, last.x + width(&last.text), text.join(" "))
        })
        .collect()
}

const PROSE: &str = "Typographers soon discovered that the appearance of a page depends on \
    countless small decisions. The width of the column, the size of the type, the spacing \
    between words and lines, and the treatment of the margins all influence readability. \
    Justified setting, in which every line except the last extends to both margins, gives a \
    formal and orderly impression, but it requires careful management of the spaces between \
    words.";

fn render_with(src: &str, o: Options) -> Output {
    layout::layout(
        &markdown::parse(src),
        &Options {
            page_numbers: false,
            ..o
        },
    )
}

#[test]
fn paragraphs_are_justified_except_their_last_lines() {
    let o = Options::default();
    let (left, right) = (o.margin, o.page_width - o.margin);
    let src = format!("{PROSE}\n\n{PROSE}  \nAfter a hard break.");
    let lines = line_edges(&render(&src));
    // Two paragraphs of several lines, the second with a hard break.
    let n = lines.len();
    assert!(n >= 8, "{lines:?}");
    let ends = |k: usize| (lines[k].1 - right).abs() < 0.05;
    for (k, (start, _, text)) in lines.iter().enumerate() {
        assert_eq!(*start, left, "{text}");
        let last = text.ends_with("words.") || k + 1 == n;
        assert_eq!(ends(k), !last, "line {k}: {text:?} ends at {}", lines[k].1);
    }

    // Unless justification is turned off.
    let lines = line_edges(&render_with(&src, Options { justify: false, ..o }));
    assert!(
        lines.iter().all(|l| l.0 == left && l.1 < right - 0.05),
        "{lines:?}"
    );
}

#[test]
fn headings_and_table_cells_are_not_justified() {
    let o = Options::default();
    let right = o.page_width - o.margin;
    let heading = format!("## {}", &PROSE[..200]);
    let lines = line_edges(&render(&heading));
    assert!(
        lines.len() >= 2 && lines.iter().all(|l| l.1 < right - 0.05),
        "{lines:?}"
    );
    let table = format!("| a | b |\n|---|---|\n| {PROSE} | x |\n");
    let lines = line_edges(&render(&table));
    assert!(lines.len() >= 3, "{lines:?}");
    let ends: Vec<f32> = lines[1..lines.len() - 1].iter().map(|l| l.1).collect();
    assert!(ends.windows(2).any(|w| (w[0] - w[1]).abs() > 1.0), "{lines:?}");
}

#[test]
fn right_to_left_paragraphs_are_justified_too() {
    let o = Options::default();
    let (left, right) = (o.margin, o.page_width - o.margin);
    let words = ["שלום", "עולם", "ספר", "מילים", "ארוכות", "קצת"];
    let src: Vec<&str> = (0..120).map(|k| words[k % words.len()]).collect();
    let lines = line_edges(&render(&src.join(" ")));
    assert!(lines.len() >= 3, "{lines:?}");
    for (k, (start, end, text)) in lines.iter().enumerate() {
        // Flush right, and (but for the last line) flush left. (The width
        // measured here is only close for right-to-left text.)
        assert!((end - right).abs() < 0.5, "{text}: {end}");
        assert_eq!(
            (start - left).abs() < 0.05,
            k + 1 < lines.len(),
            "{text}: {start}"
        );
    }
}

/// The lines of `src` set `width` wide, in language `lang`.
fn hyphenated(src: &str, width: f32, lang: Option<&str>) -> Vec<String> {
    lines(&render_with(
        src,
        Options {
            page_width: width,
            margin: 10.0,
            lang: lang.map(String::from),
            ..Options::default()
        },
    ))
}

#[test]
fn english_is_hyphenated_when_the_language_is_given() {
    let src = "Characteristically, internationalization notwithstanding considerable \
               standardization remains extraordinarily complicated and unpredictable.";
    let plain = src.replace(' ', "");
    for width in [120.0, 150.0, 200.0] {
        let l = hyphenated(src, width, None);
        assert!(l.iter().all(|x| !x.ends_with('-')), "{l:?}");
        let l = hyphenated(src, width, Some("en-US"));
        assert!(l.iter().any(|x| x.ends_with('-')), "{l:?}");
        // Only the breaks show hyphens, and the text stays the same.
        assert_eq!(l.concat().replace([' ', '-'], ""), plain, "{l:?}");
        // At syllables, with at least two letters before the hyphen and
        // three after it.
        for (a, b) in l.iter().zip(&l[1..]) {
            if let Some(a) = a.strip_suffix('-') {
                let before = a.rsplit(' ').next().unwrap();
                let after = b.split([' ', '.', ',']).next().unwrap();
                assert!(before.chars().count() >= 2 && after.chars().count() >= 3, "{l:?}");
            }
        }
    }
    // The front matter's lang does the same, and the command line's wins.
    let front = format!("---\nlang: en\n---\n\n{src}");
    let l = lines(&render_front_narrow(&front, None));
    assert!(l.iter().any(|x| x.ends_with('-')), "{l:?}");
    let out = render_front_narrow(&front, Some("sv"));
    assert!(lines(&out).iter().all(|x| !x.ends_with('-')));
    assert!(
        out.warnings.iter().any(|w| w.contains("'sv'")),
        "{:?}",
        out.warnings
    );
}

fn render_front_narrow(src: &str, lang: Option<&str>) -> Output {
    render_with(
        src,
        Options {
            page_width: 150.0,
            margin: 10.0,
            front_matter: Some(false),
            lang: lang.map(String::from),
            ..Options::default()
        },
    )
}

#[test]
fn only_plain_words_are_hyphenated() {
    // Not code, acronyms, camel case, words joined by a hyphen or slash,
    // or URLs; and never inside a ligature ("of-fice" would lose its
    // "ffi").
    for src in [
        "`internationalization` `internationalization`",
        "INTERNATIONALIZATION INTERNATIONALIZATION",
        "InternationalizationRules InternationalizationRules",
        "state-of-the-internationalization and/internationalization",
        "https://example.com/internationalization/standardization",
        "office office office office office office office",
    ] {
        for width in [60.0, 80.0, 100.0, 120.0] {
            // No hyphens are added.
            let l = hyphenated(src, width, Some("en"));
            assert_eq!(l.concat().replace(' ', ""), src.replace(['`', ' '], ""), "{l:?}");
        }
    }
    // Capitalized words are hyphenated.
    let l = hyphenated("Internationalization Internationalization", 80.0, Some("en"));
    assert!(l.iter().any(|x| x.ends_with('-')), "{l:?}");
}

#[test]
fn emphasized_and_linked_words_are_hyphenated() {
    for src in [
        "*Internationalization*, *internationalization*.",
        "[Internationalization](https://example.com), \"internationalization\"",
    ] {
        let l = hyphenated(src, 80.0, Some("en"));
        assert!(l.iter().any(|x| x.ends_with('-')), "{l:?}");
    }
}

// ---------------------------------------------------------------- tagged PDF

/// The structure tree from element `id` down, without its content, as
/// `Document(H1 P(Link))`.
fn tree(out: &Output, id: usize) -> String {
    let e = &out.structure[id];
    let kids: Vec<String> = e
        .kids
        .iter()
        .filter_map(|k| match k {
            Kid::Elem(c) => Some(tree(out, *c)),
            _ => None,
        })
        .collect();
    if kids.is_empty() {
        e.tag.to_string()
    } else {
        format!("{}({})", e.tag, kids.join(" "))
    }
}

#[test]
fn the_structure_tree_follows_the_blocks() {
    let out = render(
        "# Title\n\nSome [linked text](https://example.com) here.\n\n1. one\n2. two\n   - nested\n\n\
         > quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```\ncode\n```\n\n---\n\n###### Last\n",
    );
    assert_eq!(
        tree(&out, 0),
        "Document(H1 P(Link) L(LI(Lbl LBody(P)) LI(Lbl LBody(P L(LI(Lbl LBody(P)))))) \
         BlockQuote(P) Table(TR(TH TH) TR(TD TD)) Code H6)"
    );
    let attrs = |tag: &str| -> Vec<&str> {
        out.structure
            .iter()
            .filter(|e| e.tag == tag)
            .map(|e| e.attrs.as_str())
            .collect()
    };
    assert_eq!(attrs("TH"), ["/O /Table /Scope /Column"; 2]);
    assert_eq!(
        attrs("L"),
        [
            "/O /List /ListNumbering /Decimal",
            "/O /List /ListNumbering /Disc"
        ]
    );
    // The link's annotation is in its element.
    let link = out.structure.iter().find(|e| e.tag == "Link").unwrap();
    assert!(link.kids.contains(&Kid::Link { page: 0, index: 0 }));
    assert_eq!(out.structure[out.pages[0].links[0].elem].tag, "Link");
}

#[test]
fn content_is_marked_as_its_element_or_as_an_artifact() {
    let out = layout::layout(
        &markdown::parse("# Title\n\n`code` and ~~struck~~ text\n\n> quote\n\n- [x] done\n\n---\n"),
        &Options::default(),
    );
    let ops = String::from_utf8_lossy(&out.pages[0].ops).to_string();
    // Every marked-content sequence is closed, and belongs to the element
    // that refers to it.
    assert_eq!(
        ops.matches(" BDC\n").count() + ops.matches(" BMC\n").count(),
        ops.matches("EMC\n").count()
    );
    for (mcid, &elem) in out.marked[0].iter().enumerate() {
        let tag = out.structure[elem].tag;
        assert!(
            ops.contains(&format!("/{tag} <</MCID {mcid}>> BDC")),
            "{tag} {mcid}"
        );
        assert!(out.structure[elem].kids.contains(&Kid::Content { page: 0, mcid }));
    }
    // Backgrounds, rules, the quote's bar and the page number are artifacts;
    // the check box is the list item's label, with its text.
    assert!(ops.contains("/Artifact BMC"));
    assert!(ops.contains("/Artifact <</Type /Pagination /Subtype /Footer>> BDC"));
    assert!(ops.contains("/Span <</ActualText <FEFF2611>>> BDC"));
    // Nothing is drawn outside marked content.
    let mut depth = 0i32;
    for line in ops.lines() {
        if line.ends_with(" BDC") || line.ends_with(" BMC") {
            depth += 1;
        } else if line == "EMC" {
            depth -= 1;
        } else {
            assert!(depth > 0, "unmarked: {line}");
        }
    }
}

#[test]
fn right_to_left_text_is_placed_in_reading_order() {
    // Drawn right to left, the text before the link is drawn last; it is
    // still read first.
    let out = render("שלום [עולם](https://example.com) ומה שלומך");
    let p = out.structure.iter().find(|e| e.tag == "P").unwrap();
    let mcid = |k: &Kid| match k {
        Kid::Content { mcid, .. } => *mcid,
        other => panic!("{other:?}"),
    };
    assert_eq!(p.kids.len(), 3, "{:?}", p.kids);
    assert!(matches!(p.kids[1], Kid::Elem(e) if out.structure[e].tag == "Link"));
    assert!(mcid(&p.kids[0]) > mcid(&p.kids[2]), "{:?}", p.kids);
}

#[test]
fn hyphens_at_breaks_stand_for_soft_hyphens() {
    let l = hyphenated(PROSE, 120.0, Some("en"));
    assert!(l.iter().any(|x| x.ends_with('-')));
    let out = render_with(
        PROSE,
        Options {
            page_width: 120.0,
            margin: 10.0,
            lang: Some("en".into()),
            ..Options::default()
        },
    );
    let ops = String::from_utf8_lossy(&out.pages[0].ops).to_string();
    assert!(ops.contains("/Span <</ActualText <FEFF00AD>>> BDC"), "{ops}");
    assert_eq!(out.lang.as_deref(), Some("en"));
}

#[test]
fn documents_are_pdfa_and_tagged() {
    let src = "---\nlang: en-GB\n---\n# A \"Title\" & (more)\n\nText.\n";
    let pdf = sundowner::convert(src, &Options::default()).pdf;
    let text = pdf_text(&pdf);
    for part in [
        "/MarkInfo << /Marked true >>",
        "/StructTreeRoot",
        "/ParentTree",
        "/StructParents 0",
        "/Lang (en-GB)",
        "/DisplayDocTitle true",
        "/OutputIntents",
        "/S /GTS_PDFA1",
        "/ID [<",
        "<pdfaid:part>2</pdfaid:part>",
        "<pdfaid:conformance>A</pdfaid:conformance>",
        "<rdf:li xml:lang=\"x-default\">A &quot;Title&quot; &amp; (more)</rdf:li>",
        "<rdf:li>en-GB</rdf:li>",
        "/Title (A \"Title\" & \\(more\\))",
    ] {
        assert!(text.contains(part), "no {part}");
    }
}

#[test]
fn images_with_alt_text_are_figures_and_others_artifacts() {
    let o = Options {
        base_dir: Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")),
        page_numbers: false,
        ..Options::default()
    };
    let out = layout::layout(
        &markdown::parse("![A sunset](sunset.png)\n\n![](sunset.png)\n"),
        &o,
    );
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let figures: Vec<_> = out.structure.iter().filter(|e| e.tag == "Figure").collect();
    assert_eq!(figures.len(), 1);
    assert_eq!(figures[0].alt.as_deref(), Some("A sunset"));
    // The second image is drawn as an artifact.
    let ops = String::from_utf8_lossy(&out.pages[0].ops).to_string();
    let second = ops.rfind("/Im0 Do").unwrap();
    let before = &ops[..second];
    assert!(
        before.rfind("/Artifact BMC").unwrap() > before.rfind("/Figure <<").unwrap(),
        "{ops}"
    );
}

#[test]
fn cmyk_images_are_not_pdfa() {
    let dir = std::env::temp_dir().join(format!("sundowner-cmyk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A JPEG's header is enough: SOI, and SOF0 with four components.
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 20, 8, 0, 4, 0, 4, 4];
    jpeg.extend([1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 4, 0x11, 0]);
    std::fs::write(dir.join("c.jpg"), jpeg).unwrap();
    let o = Options {
        base_dir: Some(dir.clone()),
        ..Options::default()
    };
    let c = sundowner::convert("![CMYK](c.jpg)", &o);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        c.warnings.iter().any(|w| w.contains("not PDF/A")),
        "{:?}",
        c.warnings
    );
    let text = pdf_text(&c.pdf);
    assert!(text.contains("/DeviceCMYK") && !text.contains("pdfaid:part"));
    // It is still tagged, as a figure.
    assert!(text.contains("/S /Figure") && text.contains("/Alt (CMYK)"));
}
