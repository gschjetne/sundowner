//! PDF 1.4 file serialization.

use crate::flate;
use crate::layout::{Font, Output, Target};
use std::io::Write;

struct Writer {
    out: Vec<u8>,
    offsets: Vec<usize>,
}

impl Writer {
    fn alloc(&mut self) -> usize {
        self.offsets.push(0);
        self.offsets.len() - 1
    }

    fn begin(&mut self, id: usize) {
        self.offsets[id] = self.out.len();
        let _ = writeln!(self.out, "{id} 0 obj");
    }

    fn obj(&mut self, id: usize, body: &str) {
        self.begin(id);
        self.out.extend_from_slice(body.as_bytes());
        self.out.extend_from_slice(b"\nendobj\n");
    }

    fn stream(&mut self, id: usize, dict: &str, data: &[u8]) {
        self.begin(id);
        let _ = write!(self.out, "<< {dict} /Length {} >>\nstream\n", data.len());
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }
}

/// Encode a text string as UTF-16BE hex, which PDF accepts for any Unicode text.
fn text_string(s: &str) -> String {
    let mut out = String::from("<FEFF");
    for u in s.encode_utf16() {
        out.push_str(&format!("{u:04X}"));
    }
    out.push('>');
    out
}

/// Escape a URI as a PDF literal string (7-bit ASCII only).
fn uri_string(s: &str) -> String {
    let mut out = String::from("(");
    for b in s.bytes() {
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            0x21..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out.push(')');
    out
}

fn n(v: f32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn write(doc: &Output, page_w: f32, page_h: f32, serif: bool) -> Vec<u8> {
    let mut w = Writer {
        out: Vec::new(),
        offsets: vec![0],
    };
    w.out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    let catalog = w.alloc();
    let pages_id = w.alloc();
    let info = w.alloc();
    let resources = w.alloc();
    let fonts: Vec<usize> = Font::ALL.iter().map(|_| w.alloc()).collect();
    let images: Vec<(usize, Option<usize>)> = doc
        .images
        .iter()
        .map(|img| (w.alloc(), img.smask.as_ref().map(|_| w.alloc())))
        .collect();
    let page_ids: Vec<(usize, usize)> = doc.pages.iter().map(|_| (w.alloc(), w.alloc())).collect();
    let outline_root = if doc.headings.is_empty() {
        None
    } else {
        Some(w.alloc())
    };
    let outline_ids: Vec<usize> = doc.headings.iter().map(|_| w.alloc()).collect();

    // Fonts and resources.
    for (font, &id) in Font::ALL.iter().zip(&fonts) {
        let enc = "/Encoding /WinAnsiEncoding";
        w.obj(
            id,
            &format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{} {enc} >>",
                font.base_name(serif)
            ),
        );
    }
    let mut res = String::from("<< /ProcSet [/PDF /Text /ImageB /ImageC /ImageI] /Font <<");
    for (font, id) in Font::ALL.iter().zip(&fonts) {
        res.push_str(&format!(" /F{} {id} 0 R", font.id()));
    }
    res.push_str(" >>");
    if !images.is_empty() {
        res.push_str(" /XObject <<");
        for (i, (id, _)) in images.iter().enumerate() {
            res.push_str(&format!(" /Im{i} {id} 0 R"));
        }
        res.push_str(" >>");
    }
    res.push_str(" >>");
    w.obj(resources, &res);

    for (img, &(id, smask)) in doc.images.iter().zip(&images) {
        let mut dict = format!(
            "/Type /XObject /Subtype /Image /Width {} /Height {} {}",
            img.width, img.height, img.dict
        );
        if let (Some(mask_id), Some(mask)) = (smask, &img.smask) {
            dict.push_str(&format!(" /SMask {mask_id} 0 R"));
            w.stream(
                mask_id,
                &format!(
                    "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray \
                     /BitsPerComponent 8 /Filter /FlateDecode",
                    img.width, img.height
                ),
                mask,
            );
        }
        w.stream(id, &dict, &img.data);
    }

    let dest = |page: usize, y: f32| -> String {
        let pid = page_ids.get(page).map(|p| p.0).unwrap_or(page_ids[0].0);
        format!("[{pid} 0 R /XYZ null {} null]", n(y))
    };

    // Pages, content streams and link annotations.
    for (page, &(pid, cid)) in doc.pages.iter().zip(&page_ids) {
        let mut annots = String::new();
        for link in &page.links {
            let action = match &link.target {
                Target::Uri(u) => format!("/A << /S /URI /URI {} >>", uri_string(u)),
                Target::Anchor(a) => {
                    let found = doc
                        .anchors
                        .get(a.as_str())
                        .or_else(|| doc.anchors.get(&a.to_lowercase()));
                    match found {
                        Some(&(p, y)) => format!("/Dest {}", dest(p, y)),
                        None => continue,
                    }
                }
            };
            let r = link.rect;
            annots.push_str(&format!(
                "<< /Type /Annot /Subtype /Link /Rect [{} {} {} {}] /Border [0 0 0] {action} >> ",
                n(r[0]),
                n(r[1]),
                n(r[2]),
                n(r[3])
            ));
        }
        let annots = if annots.is_empty() {
            String::new()
        } else {
            format!(" /Annots [{annots}]")
        };
        w.obj(
            pid,
            &format!(
                "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}] /Resources {resources} 0 R \
                 /Contents {cid} 0 R{annots} >>",
                n(page_w),
                n(page_h)
            ),
        );
        let compressed = flate::zlib_compress(&page.ops);
        w.stream(cid, "/Filter /FlateDecode", &compressed);
    }
    let kids: Vec<String> = page_ids.iter().map(|(p, _)| format!("{p} 0 R")).collect();
    w.obj(
        pages_id,
        &format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            page_ids.len()
        ),
    );

    // Document outline (bookmarks) built from the heading hierarchy.
    if let Some(root) = outline_root {
        let count = doc.headings.len();
        let mut parent: Vec<Option<usize>> = vec![None; count];
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
        let mut top: Vec<usize> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for (i, h) in doc.headings.iter().enumerate() {
            while stack.last().is_some_and(|&s| doc.headings[s].level >= h.level) {
                stack.pop();
            }
            match stack.last() {
                Some(&p) => {
                    parent[i] = Some(p);
                    children[p].push(i);
                }
                None => top.push(i),
            }
            stack.push(i);
        }
        let siblings = |i: usize| -> &Vec<usize> {
            match parent[i] {
                Some(p) => &children[p],
                None => &top,
            }
        };
        for (i, h) in doc.headings.iter().enumerate() {
            let mut d = format!(
                "<< /Title {} /Parent {} 0 R /Dest {}",
                text_string(&h.title),
                parent[i].map(|p| outline_ids[p]).unwrap_or(root),
                dest(h.page, h.y)
            );
            let sib = siblings(i);
            if let Some(pos) = sib.iter().position(|&s| s == i) {
                if pos > 0 {
                    d.push_str(&format!(" /Prev {} 0 R", outline_ids[sib[pos - 1]]));
                }
                if let Some(&next) = sib.get(pos + 1) {
                    d.push_str(&format!(" /Next {} 0 R", outline_ids[next]));
                }
            }
            if let (Some(&f), Some(&l)) = (children[i].first(), children[i].last()) {
                // Negative count: sub-headings start collapsed.
                d.push_str(&format!(
                    " /First {} 0 R /Last {} 0 R /Count -{}",
                    outline_ids[f],
                    outline_ids[l],
                    children[i].len()
                ));
            }
            d.push_str(" >>");
            w.obj(outline_ids[i], &d);
        }
        let (f, l) = (top[0], top[top.len() - 1]);
        w.obj(
            root,
            &format!(
                "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
                outline_ids[f],
                outline_ids[l],
                top.len()
            ),
        );
        w.obj(
            catalog,
            &format!(
                "<< /Type /Catalog /Pages {pages_id} 0 R /Outlines {root} 0 R /PageMode /UseOutlines >>"
            ),
        );
    } else {
        w.obj(catalog, &format!("<< /Type /Catalog /Pages {pages_id} 0 R >>"));
    }

    let mut info_dict = format!(
        "<< /Producer {}",
        text_string(concat!("sundowner ", env!("CARGO_PKG_VERSION")))
    );
    if let Some(t) = &doc.title {
        info_dict.push_str(&format!(" /Title {}", text_string(t)));
    }
    info_dict.push_str(" >>");
    w.obj(info, &info_dict);

    // Cross-reference table and trailer.
    let xref = w.out.len();
    let size = w.offsets.len();
    let _ = write!(w.out, "xref\n0 {size}\n0000000000 65535 f \n");
    for off in &w.offsets[1..] {
        let _ = writeln!(w.out, "{off:010} 00000 n ");
    }
    let _ = write!(
        w.out,
        "trailer\n<< /Size {size} /Root {catalog} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    );
    w.out
}
