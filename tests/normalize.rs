//! The official Unicode normalization conformance test
//! (`NormalizationTest.txt`, comments stripped, stored compressed) for NFC
//! and NFD, which the font-aware normalization is built from.

use sundowner::linebreak_table::UNICODE_VERSION;
use sundowner::normalize::{nfc, nfd};

#[test]
fn unicode_normalization_test() {
    let data = include_bytes!("data/NormalizationTest.txt.zlib");
    let text = sundowner::flate::zlib_decompress(data, 64 << 20).expect("test data decompresses");
    let text = String::from_utf8(text).unwrap();
    assert!(
        text.starts_with(&format!("# NormalizationTest-{UNICODE_VERSION}.txt")),
        "test data and tables are from different Unicode versions"
    );
    let (mut cases, mut failures) = (0, Vec::new());
    for line in text.lines().skip(1) {
        if line.starts_with('@') {
            continue;
        }
        let cols: Vec<Vec<char>> = line
            .split(';')
            .take(5)
            .map(|col| {
                col.split_whitespace()
                    .map(|h| char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap())
                    .collect()
            })
            .collect();
        assert_eq!(cols.len(), 5, "{line}");
        cases += 1;
        // c2 == NFC(c1) == NFC(c2) == NFC(c3), c4 == NFC(c4) == NFC(c5),
        // c3 == NFD(c1) == NFD(c2) == NFD(c3), c5 == NFD(c4) == NFD(c5).
        let ok = (0..3).all(|i| nfc(&cols[i]) == cols[1] && nfd(&cols[i]) == cols[2])
            && (3..5).all(|i| nfc(&cols[i]) == cols[3] && nfd(&cols[i]) == cols[4]);
        if !ok {
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
