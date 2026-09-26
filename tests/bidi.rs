//! The official UAX #9 conformance tests (`BidiTest.txt` and
//! `BidiCharacterTest.txt` of the Unicode version the tables were generated
//! from, stored compressed).

use sundowner::bidi::{self, Bracket};
use sundowner::bidi_table::{BidiClass, BIDI_CLASSES};
use sundowner::linebreak_table::UNICODE_VERSION;

fn load(data: &[u8], name: &str) -> String {
    let text = sundowner::flate::zlib_decompress(data, 64 << 20).expect("test data decompresses");
    let text = String::from_utf8(text).unwrap();
    assert!(
        text.starts_with(&format!("# {name}-{UNICODE_VERSION}.txt")),
        "test data and tables are from different Unicode versions"
    );
    text
}

/// Levels after L1, with `None` for characters removed by X9, and the
/// visual order of the others.
fn line(classes: &[BidiClass], brackets: &[Bracket], level: Option<u8>) -> (u8, Vec<Option<u8>>, Vec<usize>) {
    let mut l = bidi::resolve_classes(classes, brackets, level);
    bidi::reset_whitespace(classes, &mut l.levels, l.paragraph);
    let kept: Vec<usize> = (0..classes.len())
        .filter(|&i| !bidi::removed(classes[i]))
        .collect();
    let kept_levels: Vec<u8> = kept.iter().map(|&i| l.levels[i]).collect();
    let order = bidi::visual_order(&kept_levels)
        .into_iter()
        .map(|k| kept[k])
        .collect();
    let levels = (0..classes.len())
        .map(|i| (!bidi::removed(classes[i])).then_some(l.levels[i]))
        .collect();
    (l.paragraph, levels, order)
}

fn parse_levels(s: &str) -> Vec<Option<u8>> {
    s.split_whitespace()
        .map(|x| if x == "x" { None } else { Some(x.parse().unwrap()) })
        .collect()
}

fn parse_order(s: &str) -> Vec<usize> {
    s.split_whitespace().map(|x| x.parse().unwrap()).collect()
}

fn report(cases: usize, failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{} of {cases} cases fail, for example:\n{}",
        failures.len(),
        failures.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn unicode_bidi_test() {
    let text = load(include_bytes!("data/BidiTest.txt.zlib"), "BidiTest");
    let names = [
        "L", "R", "AL", "EN", "ES", "ET", "AN", "CS", "NSM", "BN", "B", "S", "WS", "ON", "LRE", "LRO", "RLE",
        "RLO", "PDF", "LRI", "RLI", "FSI", "PDI",
    ];
    let (mut levels, mut order) = (Vec::new(), Vec::new());
    let (mut cases, mut failures) = (0, Vec::new());
    for l in text.lines().skip(1) {
        if let Some(v) = l.strip_prefix("@Levels:") {
            levels = parse_levels(v);
            continue;
        }
        if let Some(v) = l.strip_prefix("@Reorder:") {
            order = parse_order(v);
            continue;
        }
        let Some((input, bits)) = l.split_once(';') else {
            continue;
        };
        let classes: Vec<BidiClass> = input
            .split_whitespace()
            .map(|n| BIDI_CLASSES[names.iter().position(|&x| x == n).expect("known class")])
            .collect();
        let brackets = vec![Bracket::None; classes.len()];
        let bits: u8 = bits.trim().parse().unwrap();
        for (bit, level) in [(1, None), (2, Some(0)), (4, Some(1))] {
            if bits & bit == 0 {
                continue;
            }
            cases += 1;
            let (_, got_levels, got_order) = line(&classes, &brackets, level);
            if got_levels != levels || got_order != order {
                failures.push(format!(
                    "{input} (direction {bit}): levels {got_levels:?}, order {got_order:?}"
                ));
            }
        }
    }
    assert!(cases > 400_000, "only {cases} test cases found");
    report(cases, &failures);
}

#[test]
fn unicode_bidi_character_test() {
    let text = load(
        include_bytes!("data/BidiCharacterTest.txt.zlib"),
        "BidiCharacterTest",
    );
    let (mut cases, mut failures) = (0, Vec::new());
    for l in text.lines().skip(1) {
        let f: Vec<&str> = l.split(';').collect();
        if f.len() < 5 {
            continue;
        }
        let chars: Vec<char> = f[0]
            .split_whitespace()
            .map(|h| char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap())
            .collect();
        let level = match f[1] {
            "0" => Some(0),
            "1" => Some(1),
            _ => None,
        };
        let classes: Vec<BidiClass> = chars.iter().map(|&c| bidi::class(c)).collect();
        let brackets: Vec<Bracket> = chars.iter().map(|&c| Bracket::of(c)).collect();
        cases += 1;
        let (para, levels, order) = line(&classes, &brackets, level);
        if para.to_string() != f[2] || levels != parse_levels(f[3]) || order != parse_order(f[4]) {
            failures.push(format!("{l}\n  got {para}; {levels:?}; {order:?}"));
        }
    }
    assert!(cases > 90_000, "only {cases} test cases found");
    report(cases, &failures);
}
