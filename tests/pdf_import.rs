//! Pictures from PDF files: the first page of a PDF, embedded as a form
//! XObject, from image links and from filters.

use std::path::{Path, PathBuf};
use sundowner::import::{self, Obj};
use sundowner::layout::{self, Options};
use sundowner::{flate, markdown};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name),
    )
    .unwrap()
}

/// The decoded content stream of an imported page.
fn content(f: &import::Form) -> String {
    let Obj::Stream(_, data) = &f.objects[0] else {
        panic!("not a stream")
    };
    String::from_utf8_lossy(&flate::zlib_decompress(data, 1 << 24).unwrap()).into_owned()
}

/// A fresh directory for a test's files.
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sundowner-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn reads_what_chromium_and_cairo_write() {
    // Mermaid's mmdc prints with Chromium (Skia): a classic cross-reference
    // table, Type 3 fonts, and a tagged page with marked content.
    let raw = data("chromium-mermaid.pdf");
    assert!(String::from_utf8_lossy(&raw).contains("/StructTreeRoot"));
    let marked = raw
        .windows(7)
        .enumerate()
        .filter(|(_, w)| w == b"stream\n")
        .filter_map(|(k, _)| flate::zlib_decompress(&raw[k + 7..], 1 << 24).ok())
        .any(|s| s.windows(3).any(|w| w == b"BDC"));
    assert!(marked, "the page has marked content to take out");
    let f = import::load(&raw).unwrap();
    assert!(f.width > 200.0 && f.height > 40.0, "{} x {}", f.width, f.height);
    assert_eq!(f.pages, 1);
    assert!(!f.cmyk && !f.unmapped);
    let c = content(&f);
    assert!(!c.contains("BDC") && !c.contains("EMC"), "{c}");
    assert!(c.contains(" Tj") || c.contains(" TJ"), "text is kept");

    // Graphviz writes with cairo: object streams, a cross-reference
    // stream, and an embedded TrueType font.
    let f = import::load(&data("cairo-graphviz.pdf")).unwrap();
    assert!(f.width > 100.0 && f.height > 20.0, "{} x {}", f.width, f.height);
    assert!(!f.cmyk && !f.unmapped);
    let mut all = String::new();
    for o in &f.objects {
        if let Obj::Stream(d, _) = o {
            import::write_entries(&mut all, d, &[]);
        }
    }
    assert!(all.contains("/Length1"), "the font file is copied: {all}");
}

#[test]
fn pdf_images_are_drawn_as_forms() {
    // sundowner's own output, as an image of another document.
    let d = dir("pdf-image");
    let note = sundowner::convert(
        "**Bold** and *italic*, with a [link](https://example.com).\n",
        &Options {
            page_width: 200.0,
            page_height: 100.0,
            margin: 10.0,
            page_numbers: false,
            ..Options::default()
        },
    );
    std::fs::write(d.join("note.pdf"), &note.pdf).unwrap();
    let doc = markdown::parse("Before.\n\n![A note](note.pdf)\n\nAfter.\n");
    let o = Options {
        base_dir: Some(d.clone()),
        ..Options::default()
    };
    let out = layout::layout(&doc, &o);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(out.forms.len(), 1);
    assert!(out.images.is_empty());
    assert_eq!((out.forms[0].width, out.forms[0].height), (200.0, 100.0));
    let ops = String::from_utf8_lossy(&out.pages[0].ops);
    assert!(
        ops.contains("q 1 0 0 1 ") && ops.contains(" cm /Fm0 Do Q"),
        "{ops}"
    );
    let alts: Vec<_> = out
        .structure
        .iter()
        .filter(|e| e.tag == "Figure")
        .filter_map(|e| e.alt.as_deref())
        .collect();
    assert_eq!(alts, ["A note"]);

    // The whole document: still PDF/A, and the note's text is in it.
    let pdf = sundowner::convert("![A note](note.pdf)\n", &o);
    assert!(pdf.warnings.is_empty(), "{:?}", pdf.warnings);
    let text = String::from_utf8_lossy(&pdf.pdf);
    assert!(text.contains("pdfaid:conformance>A<"), "level A");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn wide_pages_are_scaled_to_the_column() {
    let d = dir("pdf-wide");
    let wide = sundowner::convert(
        "Wide.\n",
        &Options {
            page_width: 2000.0,
            page_height: 500.0,
            ..Options::default()
        },
    );
    std::fs::write(d.join("wide.pdf"), &wide.pdf).unwrap();
    let out = layout::layout(
        &markdown::parse("![](wide.pdf)\n"),
        &Options {
            base_dir: Some(d.clone()),
            ..Options::default()
        },
    );
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    // The column of A4 with 20 mm margins is 481.89 points wide.
    let ops = String::from_utf8_lossy(&out.pages[0].ops);
    assert!(ops.contains("q 0.24094 0 0 0.24094 "), "{ops}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn only_the_first_page_is_shown() {
    let d = dir("pdf-pages");
    let long: String = (0..200).map(|k| format!("Paragraph {k}.\n\n")).collect();
    let many = sundowner::convert(&long, &Options::default());
    std::fs::write(d.join("many.pdf"), &many.pdf).unwrap();
    let out = layout::layout(
        &markdown::parse("![](many.pdf)\n"),
        &Options {
            base_dir: Some(d.clone()),
            ..Options::default()
        },
    );
    assert_eq!(out.forms.len(), 1);
    assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    assert!(
        out.warnings[0].starts_with("image 'many.pdf': only the first of its ")
            && out.warnings[0].ends_with(" pages is shown"),
        "{:?}",
        out.warnings
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn unusable_pdfs_become_placeholders() {
    let d = dir("pdf-bad");
    // A font that is not embedded.
    let content = "BT /F1 12 Tf (x) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 50 50] /Contents 4 0 R \
         /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>"
            .to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (k, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", k + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    std::fs::write(d.join("helvetica.pdf"), &pdf).unwrap();
    std::fs::write(d.join("broken.pdf"), b"%PDF-1.7\nnothing to see").unwrap();
    let out = layout::layout(
        &markdown::parse("![one](helvetica.pdf) ![two](broken.pdf)\n"),
        &Options {
            base_dir: Some(d.clone()),
            ..Options::default()
        },
    );
    assert!(out.forms.is_empty());
    assert_eq!(
        out.warnings[0],
        "image 'helvetica.pdf': it uses Helvetica without embedding it"
    );
    assert!(
        out.warnings[1].starts_with("image 'broken.pdf': malformed PDF"),
        "{:?}",
        out.warnings
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn filters_may_write_pdf() {
    use sundowner::filter;
    let f = filter::parse("[dot]\ncommand = cat tests/data/cairo-graphviz.pdf\n").unwrap();
    let src = "```dot\ndigraph { a -> b }\n```\n";
    let mut doc = markdown::parse(src);
    let warnings = filter::apply(&mut doc, &f);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(matches!(doc.blocks[0], markdown::Block::Figure { .. }));
    let out = layout::layout(&doc, &Options::default());
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(out.forms.len(), 1);
    let pdf = sundowner::convert(
        src,
        &Options {
            filters: f,
            ..Options::default()
        },
    );
    assert!(pdf.warnings.is_empty(), "{:?}", pdf.warnings);
}

/// Mutations of real files (bytes changed, ranges cut out or repeated)
/// never panic or hang the reader.
#[test]
#[ignore = "stress test; run with --release -- --include-ignored"]
fn mutated_pdfs_fail_cleanly() {
    let mut seed = 0x5EED_u64;
    let mut rng = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for name in ["chromium-mermaid.pdf", "cairo-graphviz.pdf"] {
        let base = data(name);
        let start = std::time::Instant::now();
        for _ in 0..3000 {
            let mut f = base.clone();
            match rng() % 3 {
                0 => {
                    for _ in 0..rng() % 8 + 1 {
                        let at = rng() as usize % f.len();
                        f[at] = rng() as u8;
                    }
                }
                1 => {
                    let at = rng() as usize % f.len();
                    let len = (rng() as usize % 200).min(f.len() - at);
                    f.drain(at..at + len);
                }
                _ => {
                    let at = rng() as usize % f.len();
                    let len = (rng() as usize % 200).min(f.len() - at);
                    let piece = f[at..at + len].to_vec();
                    f.splice(at..at, piece);
                }
            }
            let _ = import::load(&f);
        }
        assert!(start.elapsed().as_secs() < 60, "{name}: {:?}", start.elapsed());
    }
}
