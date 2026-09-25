//! End-to-end robustness tests: random and adversarial documents must always
//! convert into a structurally valid PDF, without panicking or hanging.

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

#[test]
fn random_documents_convert() {
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let opts = Options::default();
    for round in 0..1500 {
        let len = 1 + rng.below(if round % 10 == 0 { 2000 } else { 120 });
        let doc = random_doc(&mut rng, len);
        let out = convert(&doc, &opts);
        assert_valid_pdf(&out.pdf);
    }
}

#[test]
fn random_bytes_convert() {
    let mut rng = Rng(99);
    for _ in 0..300 {
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

#[test]
fn adversarial_inputs_are_fast() {
    let cases = [
        "> ".repeat(100_000),
        "- ".repeat(50_000),
        "  - x\n".repeat(20_000),
        "*".repeat(200_000),
        "[".repeat(100_000) + &"](".repeat(50_000),
        "`".repeat(100_000),
        "|a".repeat(5000) + "\n" + &"|-".repeat(5000) + "\n" + &"|b".repeat(5000),
        "a\n".repeat(200_000),
        "x".repeat(1_000_000),
        "```\n".to_string() + &"line\n".repeat(100_000),
        "<!--".repeat(50_000),
        "&#".repeat(100_000),
        "1. ".repeat(50_000),
        "# ".repeat(100_000),
        "![".repeat(50_000) + &"](a)".repeat(10),
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
