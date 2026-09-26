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
    let out = render("กข 🎉");
    assert_eq!(out.warnings.len(), 1);
    assert!(out.warnings[0].contains("U+0E01") && out.warnings[0].contains("U+1F389"));
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

/// Chinese characters are set in traditional forms by default, with
/// simplified characters that the traditional font lacks from the
/// simplified one, and all in simplified forms on request.
#[cfg(feature = "silk")]
#[test]
fn chinese_is_traditional_unless_simplified_is_asked_for() {
    // 中 and 国 are in both fonts, 这 and 们 only in the simplified one.
    let src = "中国 这们";
    let fonts = |out: &Output, text: &str| -> Vec<String> {
        runs(out, 0)
            .iter()
            .filter(|x| x.text == text)
            .map(|x| name_of(out, x))
            .collect()
    };
    let out = render(src);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(fonts(&out, "中国"), ["NotoSerifTC-Regular"]);
    assert_eq!(fonts(&out, "这们"), ["NotoSerifSC-Regular"]);
    let out = layout::layout(
        &markdown::parse(src),
        &Options {
            page_numbers: false,
            fonts: Fonts::builtin_with(true),
            ..Options::default()
        },
    );
    assert_eq!(fonts(&out, "中国"), ["NotoSerifSC-Regular"]);
    assert_eq!(fonts(&out, "这们"), ["NotoSerifSC-Regular"]);
}
