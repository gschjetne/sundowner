//! The official UAX #14 conformance test (`LineBreakTest.txt` of the Unicode
//! version the tables were generated from, stored compressed).

use sundowner::linebreak::{opportunities, Break};
use sundowner::linebreak_table::UNICODE_VERSION;

#[test]
fn unicode_line_break_test() {
    let data = include_bytes!("data/LineBreakTest.txt.zlib");
    let text = sundowner::flate::zlib_decompress(data, 64 << 20).expect("test data decompresses");
    let text = String::from_utf8(text).unwrap();
    assert!(
        text.starts_with(&format!("# LineBreakTest-{UNICODE_VERSION}.txt")),
        "test data and tables are from different Unicode versions"
    );
    let (mut cases, mut failures) = (0, Vec::new());
    for line in text.lines() {
        let spec = line.split('#').next().unwrap().trim();
        if spec.is_empty() {
            continue;
        }
        let mut chars = Vec::new();
        let mut expected = Vec::new();
        for tok in spec.split_whitespace() {
            match tok {
                "÷" => expected.push(true),
                "×" => expected.push(false),
                hex => chars.push(char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap()),
            }
        }
        assert_eq!(expected.len(), chars.len() + 1, "{line}");
        let got: Vec<bool> = opportunities(&chars).iter().map(|b| *b != Break::No).collect();
        // The boundaries at the start (never a break) and end (always one)
        // are checked too.
        cases += 1;
        if got != expected {
            failures.push(line.to_string());
        }
    }
    assert!(cases > 10_000, "only {cases} test cases found");
    assert!(
        failures.is_empty(),
        "{} of {cases} cases fail, for example:\n{}",
        failures.len(),
        failures.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );
}
