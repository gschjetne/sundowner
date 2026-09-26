//! End-to-end robustness tests: random and adversarial documents must always
//! convert into a structurally valid PDF, without panicking or hanging.
//!
//! The default tests are small smoke runs. The full-size stress runs are
//! `#[ignore]`d; run them with `cargo test --release -- --include-ignored`
//! (CI does).

use std::time::{Duration, Instant};
use sundowner::{convert, Options};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const PIECES: &[&str] = &[
    "#",
    "## ",
    "###### ",
    "\n",
    "\n\n",
    "  \n",
    "\t",
    " ",
    "    ",
    "-",
    "- ",
    "* ",
    "+ ",
    "1. ",
    "2) ",
    "> ",
    ">",
    "```",
    "~~~",
    "`",
    "``",
    "*",
    "**",
    "_",
    "__",
    "~~",
    "[",
    "]",
    "(",
    ")",
    "![",
    "](",
    "<",
    ">",
    "<br>",
    "<!--",
    "-->",
    "&amp;",
    "&#x",
    ";",
    "\\",
    "|",
    "|---|",
    ":-:",
    "---",
    "===",
    "[ ] ",
    "[x] ",
    "http://x.y/z",
    "<http://a.b>",
    "[ref]: /url",
    "[ref]",
    "#anchor",
    "word",
    "longwordlongwordlongwordlongwordlongwordlongwordlongwordlongwordlongwordlongword",
    "é",
    "€",
    "中文",
    "😀",
    "\u{0}",
    "\r",
    "\u{FEFF}",
    "\u{301}",
    "שָׁלוֹם",
    "עברית",
    "\u{5B8}",
    "Հայ",
    "123",
    "\u{202B}",
    "\u{202E}",
    "\u{202C}",
    "\u{2067}",
    "\u{2068}",
    "\u{2069}",
    "\u{200F}",
    "بِسْمِ",
    "لا",
    "\u{64E}",
    "\u{651}",
    "\u{640}",
    "\u{200D}",
    "\u{200C}",
    "١٢",
    "=",
    "![a](missing.png)",
];

fn random_doc(rng: &mut Rng, len: usize) -> String {
    (0..len).map(|_| PIECES[rng.below(PIECES.len())]).collect()
}

/// Check the xref table: every offset must point at the matching object.
fn assert_valid_pdf(pdf: &[u8]) {
    assert!(pdf.starts_with(b"%PDF-1.4\n"));
    assert!(pdf.ends_with(b"%%EOF\n"));
    let tail = std::str::from_utf8(&pdf[pdf.len().saturating_sub(64)..]).unwrap_or("");
    let sx = tail.rsplit("startxref\n").next().unwrap();
    let xref: usize = sx.lines().next().unwrap().trim().parse().unwrap();
    let table = std::str::from_utf8(&pdf[xref..]).unwrap();
    let mut lines = table.lines();
    assert_eq!(lines.next(), Some("xref"));
    let count: usize = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    lines.next();
    for id in 1..count {
        let entry = lines.next().unwrap();
        let off: usize = entry[..10].parse().unwrap();
        let expect = format!("{id} 0 obj");
        assert!(pdf[off..].starts_with(expect.as_bytes()), "object {id} misplaced");
    }
}

fn random_documents(rounds: usize) {
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let opts = Options::default();
    for round in 0..rounds {
        let len = 1 + rng.below(if round % 10 == 0 { 2000 } else { 120 });
        let doc = random_doc(&mut rng, len);
        let out = convert(&doc, &opts);
        assert_valid_pdf(&out.pdf);
    }
}

fn random_bytes(rounds: usize) {
    let mut rng = Rng(99);
    for _ in 0..rounds {
        let bytes: Vec<u8> = (0..rng.below(3000)).map(|_| rng.next() as u8).collect();
        let text = String::from_utf8_lossy(&bytes);
        assert_valid_pdf(&convert(&text, &Options::default()).pdf);
    }
}

#[test]
fn extreme_options() {
    let doc = "# Title\n\nSome text with `code` and a | table |\n|---|\n| x |\n\n```\ncode\n```\n";
    for (w, h, m, fs) in [
        (144.0, 144.0, 0.0, 72.0),
        (144.0, 144.0, 60.0, 4.0),
        (5000.0, 5000.0, 0.0, 4.0),
    ] {
        let o = Options {
            page_width: w,
            page_height: h,
            margin: m,
            font_size: fs,
            ..Options::default()
        };
        assert_valid_pdf(&convert(doc, &o).pdf);
    }
}

fn adversarial(scale: usize) {
    let cases = [
        "> ".repeat(100_000 / scale),
        "- ".repeat(50_000 / scale),
        "  - x\n".repeat(20_000 / scale),
        "*".repeat(200_000 / scale),
        "[".repeat(100_000 / scale) + &"](".repeat(50_000 / scale),
        "`".repeat(100_000 / scale),
        "|a".repeat(5000 / scale) + "\n" + &"|-".repeat(5000 / scale) + "\n" + &"|b".repeat(5000 / scale),
        "a\n".repeat(200_000 / scale),
        "x".repeat(1_000_000 / scale),
        "```\n".to_string() + &"line\n".repeat(100_000 / scale),
        "<!--".repeat(50_000 / scale),
        "&#".repeat(100_000 / scale),
        "1. ".repeat(50_000 / scale),
        "# ".repeat(100_000 / scale),
        "![".repeat(50_000 / scale) + &"](a)".repeat(10),
        "א".repeat(1_000_000 / scale),
        "א b ".repeat(100_000 / scale),
        "\u{202B}".repeat(100_000 / scale) + "x",
        "\u{2067}א(".repeat(50_000 / scale) + &")\u{2069}".repeat(50_000 / scale),
        "(א".repeat(100_000 / scale) + &")".repeat(100_000 / scale),
        "```\n".to_string() + &"שלום (x) עולם\n".repeat(20_000 / scale),
        "ب".repeat(1_000_000 / scale),
        "بِّ".repeat(200_000 / scale),
        "ل\u{200D}ا".repeat(100_000 / scale),
        "\u{640}".repeat(500_000 / scale),
        "&#".repeat(100_000 / scale) + "ب",
    ];
    for case in &cases {
        let start = Instant::now();
        let out = convert(case, &Options::default());
        assert_valid_pdf(&out.pdf);
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "slow case: {:?}",
            &case[..20.min(case.len())]
        );
    }
}

#[test]
fn random_documents_smoke() {
    random_documents(150);
}

#[test]
fn random_bytes_smoke() {
    random_bytes(40);
}

#[test]
fn adversarial_smoke() {
    adversarial(20);
}

#[test]
#[ignore = "stress test; run with --release -- --include-ignored"]
fn random_documents_stress() {
    random_documents(1500);
}

#[test]
#[ignore = "stress test; run with --release -- --include-ignored"]
fn random_bytes_stress() {
    random_bytes(300);
}

#[test]
#[ignore = "stress test; run with --release -- --include-ignored"]
fn adversarial_stress() {
    adversarial(1);
}
