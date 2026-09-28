//! PDF file serialization: tagged PDF 1.7, and PDF/A-2 where the document
//! allows it (see [`crate::pdfa`]).

use crate::flate;
use crate::layout::{Output, Target};
use crate::pdfa;
use crate::tags::{text_string, Kid};
use crate::ttf::Face;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

/// Where an object is in the file.
#[derive(Clone, Copy)]
enum Place {
    /// At a byte offset.
    At(usize),
    /// In an object stream, at an index.
    In(usize, usize),
}

/// Objects per object stream.
const PER_STREAM: usize = 200;

/// Writes the objects of a PDF file. Streams are written as they come;
/// other objects (dictionaries: pages, fonts, structure elements...) are
/// kept and packed into compressed object streams at the end, which makes
/// them several times smaller, and the cross-reference table is a
/// compressed stream too (PDF 1.5).
struct Writer {
    out: Vec<u8>,
    places: Vec<Place>,
    /// Objects waiting for an object stream.
    packed: Vec<(usize, String)>,
}

impl Writer {
    fn alloc(&mut self) -> usize {
        self.places.push(Place::At(0));
        self.places.len() - 1
    }

    fn obj(&mut self, id: usize, body: &str) {
        self.packed.push((id, body.to_string()));
    }

    fn stream(&mut self, id: usize, dict: &str, data: &[u8]) {
        self.places[id] = Place::At(self.out.len());
        let _ = write!(
            self.out,
            "{id} 0 obj\n<< {dict} /Length {} >>\nstream\n",
            data.len()
        );
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    /// Write the object streams, and the cross-reference stream with the
    /// trailer's entries (`/Root`, `/Info`), and end the file. The file
    /// identifier is a hash of the file up to the cross-reference stream,
    /// which holds it (so it cannot hash itself): the same input still
    /// gives the same file.
    fn finish(mut self, trailer: &str) -> Vec<u8> {
        let packed = std::mem::take(&mut self.packed);
        for chunk in packed.chunks(PER_STREAM) {
            let stm = self.alloc();
            let (mut index, mut body) = (String::new(), String::new());
            for (k, (id, obj)) in chunk.iter().enumerate() {
                index.push_str(&format!("{id} {} ", body.len()));
                body.push_str(obj);
                body.push('\n');
                self.places[*id] = Place::In(stm, k);
            }
            let data = flate::zlib_compress(format!("{index}\n{body}").as_bytes());
            self.stream(
                stm,
                &format!(
                    "/Type /ObjStm /N {} /First {} /Filter /FlateDecode",
                    chunk.len(),
                    index.len() + 1
                ),
                &data,
            );
        }
        let id = format!("{:016X}{:016X}", fnv1a(&self.out, 0), fnv1a(&self.out, 1));
        let xref = self.alloc();
        let at = self.out.len();
        self.places[xref] = Place::At(at);
        // Rows of a type byte, 4 bytes of offset (or object stream) and 2
        // of generation (or index).
        let mut rows = vec![0, 0, 0, 0, 0, 0xFF, 0xFF];
        for place in &self.places[1..] {
            let (kind, a, b) = match *place {
                Place::At(off) => (1u8, off as u32, 0u16),
                Place::In(stm, k) => (2, stm as u32, k as u16),
            };
            rows.push(kind);
            rows.extend_from_slice(&a.to_be_bytes());
            rows.extend_from_slice(&b.to_be_bytes());
        }
        let size = self.places.len();
        self.stream(
            xref,
            &format!(
                "/Type /XRef /Size {size} /W [1 4 2] {trailer} /ID [<{id}> <{id}>] /Filter /FlateDecode"
            ),
            &flate::zlib_compress(&rows),
        );
        let _ = write!(self.out, "startxref\n{at}\n%%EOF\n");
        self.out
    }
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
        places: vec![Place::At(0)],
        packed: Vec::new(),
    };
    w.out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");

    let catalog = w.alloc();
    let pages_id = w.alloc();
    let info = w.alloc();
    let resources = w.alloc();
    let struct_root = w.alloc();
    let parent_tree = w.alloc();
    let metadata = w.alloc();
    let output_intent = w.alloc();
    let profile = w.alloc();
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
    let elem_ids: Vec<usize> = doc.structure.iter().map(|_| w.alloc()).collect();

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

    // Link annotations: the action and description of each link area that
    // becomes a link; the others (links to other schemes, or to anchors
    // that do not exist) are left as plain text.
    let links: Vec<Vec<Option<(String, String)>>> = doc
        .pages
        .iter()
        .map(|page| {
            page.links
                .iter()
                .map(|link| match &link.target {
                    Target::Uri(u) if uri_allowed(u) => {
                        Some((format!("/A << /S /URI /URI {} >>", uri_string(u)), u.clone()))
                    }
                    Target::Uri(_) => None,
                    Target::Anchor(a) => {
                        let found = doc
                            .anchors
                            .get(a.as_str())
                            .or_else(|| doc.anchors.get(&a.to_lowercase()));
                        let &(p, y) = found?;
                        let d = dest(p, y)?;
                        // Described by the heading it leads to.
                        let title = doc
                            .headings
                            .iter()
                            .find(|h| h.page == p && h.y == y)
                            .map_or_else(|| format!("#{a}"), |h| h.title.clone());
                        Some((format!("/Dest {d}"), title))
                    }
                })
                .collect()
        })
        .collect();
    let annot_ids: Vec<Vec<Option<usize>>> = links
        .iter()
        .map(|page| page.iter().map(|l| l.as_ref().map(|_| w.alloc())).collect())
        .collect();

    // Keys of the parent tree: each page's marked content has the page's
    // number, and each link annotation a number after those.
    let mut next_key = doc.pages.len();
    let mut annot_parents: Vec<(usize, usize)> = Vec::new();

    // Pages, content streams and link annotations.
    for (i, (page, &(pid, cid))) in doc.pages.iter().zip(&page_ids).enumerate() {
        let mut annots = Vec::new();
        for (k, link) in page.links.iter().enumerate() {
            let (Some((action, desc)), Some(id)) = (&links[i][k], annot_ids[i][k]) else {
                continue;
            };
            let r = link.rect;
            w.obj(
                id,
                &format!(
                    "<< /Type /Annot /Subtype /Link /Rect [{} {} {} {}] /Border [0 0 0] /F 4 \
                     /Contents {} /StructParent {next_key} {action} >>",
                    n(r[0]),
                    n(r[1]),
                    n(r[2]),
                    n(r[3]),
                    text_string(desc)
                ),
            );
            annot_parents.push((next_key, link.elem));
            next_key += 1;
            annots.push(format!("{id} 0 R"));
        }
        let annots = if annots.is_empty() {
            String::new()
        } else {
            format!(" /Annots [{}] /Tabs /S", annots.join(" "))
        };
        let parents = match doc.marked.get(i) {
            Some(m) if !m.is_empty() => format!(" /StructParents {i}"),
            _ => String::new(),
        };
        w.obj(
            pid,
            &format!(
                "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}] /Resources {resources} 0 R \
                 /Contents {cid} 0 R{annots}{parents} >>",
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
    }

    write_structure(
        &mut w,
        doc,
        &elem_ids,
        &page_ids,
        &annot_ids,
        struct_root,
        parent_tree,
    );
    let mut tree = String::from("<< /Nums [");
    for (i, m) in doc.marked.iter().enumerate().filter(|(_, m)| !m.is_empty()) {
        let refs: Vec<String> = m.iter().map(|&e| format!("{} 0 R", elem_ids[e])).collect();
        tree.push_str(&format!(" {i} [{}]", refs.join(" ")));
    }
    for &(key, elem) in &annot_parents {
        tree.push_str(&format!(" {key} {} 0 R", elem_ids[elem]));
    }
    tree.push_str(" ] >>");
    w.obj(parent_tree, &tree);

    let level = pdfa::conformance(doc).ok();
    let producer = concat!("sundowner ", env!("CARGO_PKG_VERSION"));
    let xmp = pdfa::xmp(doc.title.as_deref(), doc.lang.as_deref(), producer, level);
    // PDF/A wants the metadata unfiltered, so that it can be read without
    // PDF tools.
    w.stream(metadata, "/Type /Metadata /Subtype /XML", xmp.as_bytes());
    w.obj(
        output_intent,
        &format!(
            "<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB IEC61966-2.1) \
             /Info (sRGB IEC61966-2.1) /DestOutputProfile {profile} 0 R >>"
        ),
    );
    w.stream(
        profile,
        "/N 3 /Filter /FlateDecode",
        &flate::zlib_compress(&pdfa::srgb_profile()),
    );

    let mut cat = format!(
        "<< /Type /Catalog /Pages {pages_id} 0 R /StructTreeRoot {struct_root} 0 R \
         /MarkInfo << /Marked true >> /ViewerPreferences << /DisplayDocTitle true >> \
         /Metadata {metadata} 0 R /OutputIntents [{output_intent} 0 R]"
    );
    if let Some(lang) = &doc.lang {
        cat.push_str(&format!(" /Lang {}", text_string(lang)));
    }
    if let Some(root) = outline_root {
        cat.push_str(&format!(" /Outlines {root} 0 R /PageMode /UseOutlines"));
    }
    cat.push_str(" >>");
    w.obj(catalog, &cat);

    let mut info_dict = format!("<< /Producer {}", text_string(producer));
    if let Some(t) = &doc.title {
        info_dict.push_str(&format!(" /Title {}", text_string(t)));
    }
    info_dict.push_str(" >>");
    w.obj(info, &info_dict);

    w.finish(&format!("/Root {catalog} 0 R /Info {info} 0 R"))
}

/// A 64-bit FNV-1a hash of `data`, varied by `seed`.
fn fnv1a(data: &[u8], seed: u8) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed as u64;
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Write the structure tree root and elements. Marked content on an
/// element's own page (that of its first) is referred to by MCID alone.
#[allow(clippy::too_many_arguments)]
fn write_structure(
    w: &mut Writer,
    doc: &Output,
    elem_ids: &[usize],
    page_ids: &[(usize, usize)],
    annot_ids: &[Vec<Option<usize>>],
    struct_root: usize,
    parent_tree: usize,
) {
    let next_key = doc.pages.len() + annot_ids.iter().flatten().flatten().count();
    w.obj(
        struct_root,
        &format!(
            "<< /Type /StructTreeRoot /K [{} 0 R] /ParentTree {parent_tree} 0 R /ParentTreeNextKey {next_key} >>",
            elem_ids[0]
        ),
    );
    for (k, e) in doc.structure.iter().enumerate() {
        let parent = if k == 0 { struct_root } else { elem_ids[e.parent] };
        let mut d = format!("<< /Type /StructElem /S /{} /P {parent} 0 R", e.tag);
        let own_page = e.kids.iter().find_map(|kid| match kid {
            Kid::Content { page, .. } => Some(*page),
            _ => None,
        });
        if let Some(p) = own_page {
            d.push_str(&format!(" /Pg {} 0 R", page_ids[p].0));
        }
        let kids: Vec<String> = e
            .kids
            .iter()
            .filter_map(|kid| match *kid {
                Kid::Elem(c) => Some(format!("{} 0 R", elem_ids[c])),
                Kid::Content { page, mcid } if Some(page) == own_page => Some(mcid.to_string()),
                Kid::Content { page, mcid } => Some(format!(
                    "<< /Type /MCR /Pg {} 0 R /MCID {mcid} >>",
                    page_ids[page].0
                )),
                Kid::Link { page, index } => {
                    let annot = annot_ids.get(page)?.get(index).copied().flatten()?;
                    Some(format!(
                        "<< /Type /OBJR /Obj {annot} 0 R /Pg {} 0 R >>",
                        page_ids[page].0
                    ))
                }
            })
            .collect();
        if !kids.is_empty() {
            d.push_str(&format!(" /K [{}]", kids.join(" ")));
        }
        if let Some(alt) = &e.alt {
            d.push_str(&format!(" /Alt {}", text_string(alt)));
        }
        if !e.attrs.is_empty() {
            d.push_str(&format!(" /A << {} >>", e.attrs));
        }
        d.push_str(" >>");
        w.obj(elem_ids[k], &d);
    }
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
