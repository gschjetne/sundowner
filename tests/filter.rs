//! Filters: code blocks piped through commands that turn them into images.
//! The commands here are POSIX shell commands, so these tests are for Unix.
#![cfg(unix)]

use std::time::{Duration, Instant};
use sundowner::filter::{self, Filters};
use sundowner::markdown::{self, Block};
use sundowner::Options;

/// A command that writes a PNG, whatever its input (tests run in the
/// package's directory).
const PNG: &str = "cat examples/sunset.png";

fn filters(text: &str) -> Filters {
    filter::parse(text).unwrap()
}

/// Run the filters on a document, and return the warnings and, for each
/// top-level block, whether it became a figure.
fn apply(src: &str, f: &Filters) -> (Vec<String>, Vec<bool>, markdown::Document) {
    let mut doc = markdown::parse(src);
    let warnings = filter::apply(&mut doc, f);
    let figures = doc
        .blocks
        .iter()
        .map(|b| matches!(b, Block::Figure { .. }))
        .collect();
    (warnings, figures, doc)
}

#[test]
fn parses_filters_files() {
    let f = filters(
        "# comment\n[mermaid]\ncommand = mmdc -i - -o -\ndpi = 192\ntimeout = 60\n\n\
         [dot, Graphviz]\ncommand = dot -Tpng | cat\n",
    );
    let m = f.for_info("mermaid").unwrap();
    assert_eq!(m.command, "mmdc -i - -o -");
    assert_eq!(m.dpi, 192.0);
    assert_eq!(m.timeout, Duration::from_secs(60));
    // The language is the info string's first word, in any case.
    assert_eq!(f.for_info("Mermaid {theme=dark}"), Some(m));
    let dot = f.for_info("graphviz").unwrap();
    assert_eq!(dot.command, "dot -Tpng | cat");
    assert_eq!(dot.dpi, 96.0);
    assert_eq!(dot.timeout, Duration::from_secs(30));
    assert_eq!(f.for_info("dot"), Some(dot));
    assert_eq!(f.for_info("rust"), None);
    assert_eq!(f.for_info(""), None);
}

#[test]
fn reports_errors_in_filters_files() {
    for (src, msg) in [
        ("command = x", "line 1: 'command' outside a [language] section"),
        ("[mermaid]\ndpi = 100", "line 1: [mermaid] has no command"),
        ("[mermaid]\ncommand =", "line 2: command: missing command"),
        (
            "[a]\ncommand = x\n[b a]\ncommand = y",
            "line 3: 'a' has a filter already",
        ),
        ("[a]\ncommand = x\ndpi = 0", "line 3: dpi"),
        ("[a]\ncommand = x\ntimeout = forever", "line 3: timeout"),
        ("[a]\nshell = bash", "line 2: unknown setting 'shell'"),
        ("[ ]", "line 1: a section names the languages"),
        ("[a]\njust text", "line 2: expected 'key = value'"),
    ] {
        let e = filter::parse(src).unwrap_err();
        assert!(e.contains(msg), "{src:?}: {e}");
    }
}

#[test]
fn sundowner_files_cannot_hold_filters() {
    let e = sundowner::config::parse("[mermaid]\ncommand = mmdc", std::path::Path::new(".")).unwrap_err();
    assert!(e.contains("--filters"), "{e}");
}

#[test]
fn commands_read_the_code_and_write_the_image() {
    let out = filter::run("tr a-z A-Z", b"graph LR\n", Duration::from_secs(10)).unwrap();
    assert_eq!(out, b"GRAPH LR\n");
    // Pipes and other shell syntax work.
    let out = filter::run("tr a-z A-Z | rev", b"abc\n", Duration::from_secs(10)).unwrap();
    assert_eq!(out, b"CBA\n");
}

#[test]
fn code_blocks_become_figures() {
    let f = filters(&format!("[diagram]\ncommand = {PNG}\ndpi = 192\n"));
    let src = "```diagram\na -> b\n```\n\n```rust\nfn main() {}\n```\n\n> ```diagram\n> b -> c\n> ```\n\n    indented\n";
    let (warnings, figures, doc) = apply(src, &f);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(figures, [true, false, false, false]);
    // In the quote too.
    assert!(matches!(&doc.blocks[2], Block::Quote(b) if matches!(b[0], Block::Figure { .. })));
    assert_eq!(doc.figures.len(), 2);
    assert_eq!(doc.figures[0].dpi, 192.0);

    // The images are embedded, with the code as their alt text.
    let out = sundowner::layout::layout(&doc, &Options::default());
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(out.images.len(), 2);
    let alts: Vec<_> = out
        .structure
        .iter()
        .filter(|e| e.tag == "Figure")
        .map(|e| e.alt.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(alts, ["a -> b", "b -> c"]);
    // At 192 dpi, the 320 x 120 pixel image is 120 x 45 points.
    let ops = String::from_utf8_lossy(&out.pages[0].ops);
    assert!(ops.contains("q 120 0 0 45 "), "{ops}");
}

#[test]
fn identical_code_blocks_are_run_once() {
    let f = filters(&format!("[diagram]\ncommand = {PNG}\n"));
    let block = "```diagram\na -> b\n```\n\n";
    let (warnings, figures, doc) = apply(&format!("{block}{block}```diagram\nc\n```\n"), &f);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(figures, [true, true, true]);
    assert_eq!(doc.figures.len(), 2);
}

#[test]
fn failed_filters_leave_the_code_with_a_warning() {
    for (command, msg) in [
        (
            "echo 'Parse error on line 2' >&2; exit 3",
            "exited with status 3: Parse error on line 2",
        ),
        ("true", "it wrote nothing"),
        ("echo not an image", "did not write a PDF, PNG or JPEG"),
        ("/nonexistent/command", "exited with status 127"),
    ] {
        let f = filters(&format!("[d]\ncommand = {command}\n"));
        let (warnings, figures, doc) = apply("```d\nx\n```\n\n```d\ny\n```\n", &f);
        assert_eq!(figures, [false, false], "{command}");
        assert!(doc.figures.is_empty());
        assert_eq!(warnings.len(), 2, "{command}: {warnings:?}");
        assert!(
            warnings[1].starts_with("d code block 2: the filter failed, so it is shown as code: "),
            "{warnings:?}"
        );
        assert!(warnings[0].contains(msg), "{command}: {warnings:?}");
    }
}

#[test]
fn slow_filters_are_stopped() {
    let start = Instant::now();
    let e = filter::run("sleep 30", b"", Duration::from_millis(300)).unwrap_err();
    assert!(e.contains("took longer than 0.3 s"), "{e}");
    // So is what they started, even when it holds their output open.
    let e = filter::run("sleep 30 & cat", b"x", Duration::from_millis(300)).unwrap_err();
    assert!(e.contains("took longer"), "{e}");
    let e = filter::run("(sleep 30; echo late) | cat", b"", Duration::from_millis(300)).unwrap_err();
    assert!(e.contains("took longer"), "{e}");
    assert!(start.elapsed() < Duration::from_secs(10), "{:?}", start.elapsed());
}

#[test]
fn output_is_limited() {
    let e = filter::run("head -c 70000000 /dev/zero", b"", Duration::from_secs(20)).unwrap_err();
    assert!(e.contains("more than 64 MiB"), "{e}");
}

#[test]
fn commands_need_not_read_their_input() {
    // A large code block, written to a command that never reads it.
    let big = "x".repeat(4 << 20);
    let out = filter::run("echo done", big.as_bytes(), Duration::from_secs(10)).unwrap();
    assert_eq!(out, b"done\n");
}

#[test]
fn runs_per_document_are_capped() {
    let f = filters(&format!("[d]\ncommand = {PNG}\n"));
    let n = filter::MAX_RUNS + 3;
    let src: String = (0..n).map(|k| format!("```d\n{k}\n```\n\n")).collect();
    let (warnings, figures, _) = apply(&src, &f);
    assert_eq!(figures.iter().filter(|&&f| f).count(), filter::MAX_RUNS);
    assert!(!figures[n - 1]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].starts_with("3 code block(s) are shown as code"),
        "{warnings:?}"
    );
}

#[test]
fn without_filters_nothing_runs() {
    let (warnings, figures, _) = apply("```mermaid\ngraph LR\n```\n", &Filters::default());
    assert!(warnings.is_empty());
    assert_eq!(figures, [false]);
}
