//! PDF 1.4 file serialization.

use crate::flate;
use crate::layout::{Output, Target};
use crate::ttf::Face;
use std::collections::{BTreeMap, BTreeSet};
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

/// Only web and mail links become clickable; other schemes (`javascript:`,
/// `file:`, custom handlers) and relative paths are left as plain text.
fn uri_allowed(u: &str) -> bool {
    let lower = u.trim_start().to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|p| lower.starts_with(p))
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

pub fn write(doc: &Output, page_w: f32, page_h: f32) -> Vec<u8> {
    let mut w = Writer {
        out: Vec::new(),
        offsets: vec![0],
    };
    w.out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    let catalog = w.alloc();
    let pages_id = w.alloc();
    let info = w.alloc();
    let resources = w.alloc();
    // Five objects per embedded face: Type0 font, CIDFont, descriptor,
    // font file and ToUnicode map.
    let fonts: Vec<(usize, [usize; 5])> = doc
        .used
        .iter()
        .enumerate()
        .filter(|(_, u)| !u.is_empty())
        .map(|(face, _)| (face, [w.alloc(), w.alloc(), w.alloc(), w.alloc(), w.alloc()]))
        .collect();
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
    for &(face, ids) in &fonts {
        write_font(&mut w, &doc.fonts.faces[face], &doc.used[face], ids);
    }
    let mut res = String::from("<< /ProcSet [/PDF /Text /ImageB /ImageC /ImageI] /Font <<");
    for (face, ids) in &fonts {
        res.push_str(&format!(" /F{face} {} 0 R", ids[0]));
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

    // None if the page does not exist; callers then omit the destination
    // rather than point at the wrong page.
    let dest = |page: usize, y: f32| -> Option<String> {
        let pid = page_ids.get(page)?.0;
        Some(format!("[{pid} 0 R /XYZ null {} null]", n(y)))
    };

    // Pages, content streams and link annotations.
    for (page, &(pid, cid)) in doc.pages.iter().zip(&page_ids) {
        let mut annots = String::new();
        for link in &page.links {
            let action = match &link.target {
                Target::Uri(u) if uri_allowed(u) => format!("/A << /S /URI /URI {} >>", uri_string(u)),
                Target::Uri(_) => continue,
                Target::Anchor(a) => {
                    let found = doc
                        .anchors
                        .get(a.as_str())
                        .or_else(|| doc.anchors.get(&a.to_lowercase()));
                    match found.and_then(|&(p, y)| dest(p, y)) {
                        Some(d) => format!("/Dest {d}"),
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
                "<< /Title {} /Parent {} 0 R",
                text_string(&h.title),
                parent[i].map(|p| outline_ids[p]).unwrap_or(root),
            );
            if let Some(dst) = dest(h.page, h.y) {
                d.push_str(&format!(" /Dest {dst}"));
            }
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

/// Embed one face as a subset Type0/CIDFontType2 font with Identity-H
/// encoding: content streams address glyphs by their 2-byte glyph ID.
fn write_font(w: &mut Writer, face: &Face, used: &BTreeMap<u16, String>, ids: [usize; 5]) {
    let [type0, cid, desc, file, tounicode] = ids;
    let gids: BTreeSet<u16> = used.keys().copied().collect();

    // Subset fonts are named with a tag derived from their contents, so the
    // same input always produces the same output.
    let name = if face.no_subset {
        face.postscript_name.clone()
    } else {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in face
            .postscript_name
            .bytes()
            .chain(gids.iter().flat_map(|g| g.to_be_bytes()))
        {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
        let tag: String = (0..6)
            .map(|i| (b'A' + ((h >> (i * 8)) % 26) as u8) as char)
            .collect();
        format!("{tag}+{}", face.postscript_name)
    };

    let mut widths = String::new();
    let mut run: Vec<u16> = Vec::new();
    let flush = |run: &mut Vec<u16>, widths: &mut String| {
        if let Some(&first) = run.first() {
            let ws: Vec<String> = run.iter().map(|&g| n(face.advance_1000(g))).collect();
            widths.push_str(&format!("{first} [{}] ", ws.join(" ")));
        }
        run.clear();
    };
    for &g in &gids {
        if run.last().is_some_and(|&l| l + 1 != g) {
            flush(&mut run, &mut widths);
        }
        run.push(g);
    }
    flush(&mut run, &mut widths);

    let mut flags = 4; // symbolic: glyphs are addressed by ID, not a standard encoding
    if face.fixed_pitch {
        flags |= 1;
    }
    if face.italic {
        flags |= 64;
    }
    let b = face.bbox;
    w.obj(
        type0,
        &format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H \
             /DescendantFonts [{cid} 0 R] /ToUnicode {tounicode} 0 R >>"
        ),
    );
    w.obj(
        cid,
        &format!(
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
             /FontDescriptor {desc} 0 R /CIDToGIDMap /Identity /DW 1000 /W [{widths}] >>"
        ),
    );
    w.obj(
        desc,
        &format!(
            "<< /Type /FontDescriptor /FontName /{name} /Flags {flags} /FontBBox [{} {} {} {}] \
             /ItalicAngle {} /Ascent {} /Descent {} /CapHeight {} /StemV 80 /FontFile2 {file} 0 R >>",
            n(face.scale_1000(b[0])),
            n(face.scale_1000(b[1])),
            n(face.scale_1000(b[2])),
            n(face.scale_1000(b[3])),
            n(face.italic_angle),
            n(face.scale_1000(face.ascent)),
            n(face.scale_1000(face.descent)),
            n(face.scale_1000(face.cap_height)),
        ),
    );
    let font_data = face.subset(&gids);
    let compressed = flate::zlib_compress(&font_data);
    w.stream(
        file,
        &format!("/Filter /FlateDecode /Length1 {}", font_data.len()),
        &compressed,
    );

    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    // A glyph can stand for several characters (a ligature). Glyphs that
    // stand for none (the second glyph of a decomposed character) are left
    // out.
    let entries: Vec<(&u16, &String)> = used.iter().filter(|(&g, t)| g != 0 && !t.is_empty()).collect();
    for chunk in entries.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (g, text) in chunk {
            // At most 512 bytes (256 UTF-16 code units) per destination.
            let hex: String = text
                .encode_utf16()
                .take(256)
                .map(|u| format!("{u:04X}"))
                .collect();
            cmap.push_str(&format!("<{g:04X}> <{hex}>\n"));
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    w.stream(
        tounicode,
        "/Filter /FlateDecode",
        &flate::zlib_compress(cmap.as_bytes()),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn uri_allowlist() {
        for ok in ["http://a.b", "HTTPS://a.b/c?d", "mailto:x@y.z"] {
            assert!(super::uri_allowed(ok), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "other.md",
            "vscode://x",
            "",
        ] {
            assert!(!super::uri_allowed(bad), "{bad}");
        }
    }
}
