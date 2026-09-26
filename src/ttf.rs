//! TrueType (`glyf` outline) font parsing and subsetting for PDF embedding.
//!
//! Only what PDF embedding needs is parsed: metrics, the character map,
//! glyph locations, the PostScript name and the embedding permissions.
//! Every read is bounds-checked, so malformed fonts produce an error rather
//! than a panic.

use crate::gpos::{Attachment, MarkPositioning};
use crate::gsub::{Glyph, Gsub, Script};
use crate::kern::Kerning;
use crate::otl::Gdef;
use crate::position::{self, Pos, Positioning};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub struct Face {
    data: Cow<'static, [u8]>,
    tables: HashMap<[u8; 4], (usize, usize)>,
    pub units_per_em: u16,
    num_glyphs: u16,
    loca_long: bool,
    pub ascent: i16,
    pub descent: i16,
    pub cap_height: i16,
    pub bbox: [i16; 4],
    pub italic_angle: f32,
    pub fixed_pitch: bool,
    pub italic: bool,
    /// The license forbids subsetting (OS/2 fsType bit 8): embed every glyph.
    pub no_subset: bool,
    pub postscript_name: String,
    /// For variable fonts, the default `wght` of the instance that is used.
    pub variable_default_weight: Option<f32>,
    advances: Vec<u16>,
    lsbs: Vec<i16>,
    cmap: HashMap<u32, u16>,
    kerning: Kerning,
    gsub: Gsub,
    marks: MarkPositioning,
    positioning: Positioning,
    gdef: Gdef,
}

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(i)?, *d.get(i + 1)?]))
}

fn i16_at(d: &[u8], i: usize) -> Option<i16> {
    u16_at(d, i).map(|v| v as i16)
}

fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

type R<T> = Result<T, String>;

fn bad(what: &str) -> String {
    format!("malformed font: {what}")
}

impl Face {
    /// Parse a TrueType font, or font `index` of a TrueType collection.
    pub fn parse(data: Cow<'static, [u8]>, index: u32) -> R<Face> {
        let d: &[u8] = &data;
        let mut off = 0usize;
        if d.get(0..4) == Some(b"ttcf") {
            let n = u32_at(d, 8).ok_or_else(|| bad("collection header"))?;
            if index >= n {
                return Err(format!(
                    "font collection has {n} fonts; index {index} does not exist"
                ));
            }
            off = u32_at(d, 12 + 4 * index as usize).ok_or_else(|| bad("collection header"))? as usize;
        } else if index != 0 {
            return Err("font index given, but the file is not a font collection".into());
        }
        match d.get(off..off + 4) {
            Some([0, 1, 0, 0]) | Some(b"true") => {}
            Some(b"OTTO") => {
                return Err(
                    "OpenType fonts with PostScript (CFF) outlines are not supported; \
                            use a TrueType (.ttf) version of the font"
                        .into(),
                )
            }
            _ => return Err("not a TrueType font".into()),
        }
        let ntables = u16_at(d, off + 4).ok_or_else(|| bad("header"))? as usize;
        let mut tables = HashMap::new();
        for k in 0..ntables {
            let r = off + 12 + 16 * k;
            let tag: [u8; 4] = d
                .get(r..r + 4)
                .ok_or_else(|| bad("table directory"))?
                .try_into()
                .unwrap_or_default();
            let t_off = u32_at(d, r + 8).ok_or_else(|| bad("table directory"))? as usize;
            let t_len = u32_at(d, r + 12).ok_or_else(|| bad("table directory"))? as usize;
            if t_off.checked_add(t_len).is_none_or(|end| end > d.len()) {
                return Err(bad("table outside file"));
            }
            tables.insert(tag, (t_off, t_len));
        }
        let table = |tag: &[u8; 4]| -> Option<&[u8]> {
            let &(o, l) = tables.get(tag)?;
            d.get(o..o + l)
        };
        let req = |tag: &[u8; 4]| {
            table(tag).ok_or_else(|| bad(&format!("missing {} table", String::from_utf8_lossy(tag))))
        };

        if table(b"glyf").is_none() || table(b"loca").is_none() {
            return Err(
                "font has no TrueType outlines (glyf table); bitmap, color and CFF fonts \
                        are not supported"
                    .into(),
            );
        }
        let head = req(b"head")?;
        let units_per_em = u16_at(head, 18).ok_or_else(|| bad("head"))?;
        if !(16..=16384).contains(&units_per_em) {
            return Err(bad("unitsPerEm out of range"));
        }
        let bbox = [
            i16_at(head, 36).unwrap_or(0),
            i16_at(head, 38).unwrap_or(0),
            i16_at(head, 40).unwrap_or(0),
            i16_at(head, 42).unwrap_or(0),
        ];
        let mac_style = u16_at(head, 44).unwrap_or(0);
        let loca_long = i16_at(head, 50).ok_or_else(|| bad("head"))? == 1;
        let num_glyphs = u16_at(req(b"maxp")?, 4).ok_or_else(|| bad("maxp"))?;
        if num_glyphs == 0 {
            return Err(bad("no glyphs"));
        }
        let loca_needed = (num_glyphs as usize + 1) * if loca_long { 4 } else { 2 };
        if req(b"loca")?.len() < loca_needed {
            return Err(bad("loca table too short"));
        }

        let hhea = req(b"hhea")?;
        let mut ascent = i16_at(hhea, 4).ok_or_else(|| bad("hhea"))?;
        let mut descent = i16_at(hhea, 6).ok_or_else(|| bad("hhea"))?;
        let n_hmetrics =
            (u16_at(hhea, 34).ok_or_else(|| bad("hhea"))? as usize).clamp(1, num_glyphs as usize);
        let hmtx = req(b"hmtx")?;
        let mut advances = Vec::with_capacity(num_glyphs as usize);
        let mut lsbs = Vec::with_capacity(num_glyphs as usize);
        for g in 0..num_glyphs as usize {
            if g < n_hmetrics {
                advances.push(u16_at(hmtx, 4 * g).ok_or_else(|| bad("hmtx"))?);
                lsbs.push(i16_at(hmtx, 4 * g + 2).unwrap_or(0));
            } else {
                advances.push(advances[n_hmetrics - 1]);
                lsbs.push(i16_at(hmtx, 4 * n_hmetrics + 2 * (g - n_hmetrics)).unwrap_or(0));
            }
        }

        let mut cap_height = (ascent as f32 * 0.7) as i16;
        let mut no_subset = false;
        let mut italic = mac_style & 2 != 0;
        if let Some(os2) = table(b"OS/2") {
            let fs_type = u16_at(os2, 8).unwrap_or(0);
            if fs_type & 0x000F == 0x0002 {
                return Err("the font's license does not allow embedding (OS/2 fsType: restricted)".into());
            }
            if fs_type & 0x0200 != 0 {
                return Err("the font's license only allows embedding bitmaps (OS/2 fsType)".into());
            }
            no_subset = fs_type & 0x0100 != 0;
            italic |= u16_at(os2, 62).unwrap_or(0) & 1 != 0;
            if let (Some(a), Some(dsc)) = (i16_at(os2, 68), i16_at(os2, 70)) {
                if a > 0 && dsc < 0 {
                    ascent = a;
                    descent = dsc;
                }
            }
            if u16_at(os2, 0).unwrap_or(0) >= 2 {
                if let Some(c) = i16_at(os2, 88).filter(|&c| c > 0) {
                    cap_height = c;
                }
            }
        }
        let (italic_angle, fixed_pitch) = match table(b"post") {
            Some(p) => (
                u32_at(p, 4).map(|v| v as i32 as f32 / 65536.0).unwrap_or(0.0),
                u32_at(p, 12).unwrap_or(0) != 0,
            ),
            None => (0.0, false),
        };
        let postscript_name = table(b"name")
            .and_then(postscript_name)
            .unwrap_or_else(|| "Embedded".into());
        let variable_default_weight = table(b"fvar").map(fvar_default_weight);
        let kerning = Kerning::parse(table(b"GPOS"), table(b"GDEF"), table(b"kern"));
        let gsub = Gsub::parse(table(b"GSUB"), table(b"GDEF"), num_glyphs);
        let marks = MarkPositioning::parse(table(b"GPOS"));
        let positioning = Positioning::parse(table(b"GPOS"));
        let gdef = Gdef::parse(table(b"GDEF"));
        let cmap = parse_cmap(req(b"cmap")?, num_glyphs)?;
        if cmap.is_empty() {
            return Err("font has no usable Unicode character map".into());
        }

        Ok(Face {
            data,
            tables,
            units_per_em,
            num_glyphs,
            loca_long,
            ascent,
            descent,
            cap_height,
            bbox,
            italic_angle,
            fixed_pitch,
            italic,
            no_subset,
            postscript_name,
            variable_default_weight,
            kerning,
            positioning,
            gsub,
            marks,
            gdef,
            advances,
            lsbs,
            cmap,
        })
    }

    /// Glyph for a character, if the font has one.
    pub fn glyph(&self, c: char) -> Option<u16> {
        self.cmap.get(&(c as u32)).copied()
    }

    /// Apply the font's default glyph substitutions (ligatures, contextual
    /// alternates, ...) for text in `script`.
    pub fn substitute(&self, script: Script, glyphs: &mut Vec<Glyph>) {
        if self.gsub.is_empty() {
            return;
        }
        let t = |tag: &[u8; 4]| self.table(tag).unwrap_or(&[]);
        self.gsub.apply(t(b"GSUB"), t(b"GDEF"), script, glyphs);
    }

    /// The GDEF class of a glyph (1 base, 2 ligature, 3 mark, 0 unknown),
    /// or `None` if the font does not classify glyphs.
    pub fn glyph_class(&self, g: u16) -> Option<u16> {
        self.gdef
            .has_classes()
            .then(|| self.gdef.glyph_class(self.table(b"GDEF").unwrap_or(&[]), g))
    }

    /// Whether the font has mark positioning (GPOS anchors) for `script`.
    pub fn positions_marks(&self, script: Script) -> bool {
        self.marks.has(script)
    }

    /// Attach the marks among `glyphs` to their bases by the font's anchors.
    pub fn attach_marks(
        &self,
        script: Script,
        glyphs: &[u16],
        components: &[u8],
        is_mark: &dyn Fn(usize) -> bool,
    ) -> Vec<Option<Attachment>> {
        let t = |tag: &[u8; 4]| self.table(tag).unwrap_or(&[]);
        self.marks.apply(
            t(b"GPOS"),
            t(b"GDEF"),
            &self.gdef,
            script,
            glyphs,
            components,
            is_mark,
        )
    }

    /// Whether text in `script` needs [`Face::position`]: the font has
    /// cursive, single or contextual positioning for it.
    pub fn needs_positioning(&self, script: Script) -> bool {
        self.positioning.is_complex(script)
    }

    /// Position a run of glyphs in logical order with all of the font's
    /// GPOS lookups for `script` (or its legacy `kern` table), placing
    /// marks by `fallback` attachments if the font does not position them.
    /// Returns HarfBuzz's positions in font units: see [`crate::position`].
    pub fn position(
        &self,
        script: Script,
        glyphs: &[u16],
        components: &[u8],
        is_mark: &dyn Fn(usize) -> bool,
        rtl: bool,
        fallback: Option<&[Option<Attachment>]>,
    ) -> Vec<Pos> {
        let t = |tag: &[u8; 4]| self.table(tag).unwrap_or(&[]);
        let mut pos: Vec<Pos> = glyphs
            .iter()
            .map(|&g| Pos::advance(self.advance(g) as i32))
            .collect();
        self.positioning.apply(
            t(b"GPOS"),
            t(b"GDEF"),
            &self.gdef,
            script,
            glyphs,
            components,
            is_mark,
            rtl,
            &mut pos,
        );
        // The legacy kern table, between glyphs that are not marks, split
        // between the two as HarfBuzz does.
        let bases: Vec<usize> = (0..glyphs.len()).filter(|&i| !is_mark(i)).collect();
        for w in bases.windows(2) {
            let (i, j) = (w[0], w[1]);
            let Some(k) = self.kerning.legacy(t(b"kern"), glyphs[i], glyphs[j]) else {
                break;
            };
            let (k1, k2) = (k >> 1, k - (k >> 1));
            pos[i].x_advance += k1;
            pos[j].x_advance += k2;
            pos[j].x_offset += k2;
        }
        if let Some(a) = fallback {
            position::attach_fallback(&mut pos, a);
        }
        position::finish(&mut pos, is_mark, rtl);
        pos
    }

    /// The bounding box of a glyph's outline, `[x_min, y_min, x_max, y_max]`
    /// in font units, or `None` for an empty glyph.
    pub fn glyph_bbox(&self, gid: u16) -> Option<[i16; 4]> {
        let g = self.glyph_data(gid);
        Some([i16_at(g, 2)?, i16_at(g, 4)?, i16_at(g, 6)?, i16_at(g, 8)?])
    }

    /// Kerning between two adjacent glyphs, in font units (negative moves
    /// the right glyph closer).
    pub fn kern(&self, left: u16, right: u16) -> i32 {
        if self.kerning.is_empty() {
            return 0;
        }
        let t = |tag: &[u8; 4]| self.table(tag).unwrap_or(&[]);
        self.kerning.pair(t(b"GPOS"), t(b"GDEF"), t(b"kern"), left, right)
    }

    /// Kerning scaled to 1/1000 em.
    pub fn kern_1000(&self, left: u16, right: u16) -> f32 {
        self.kern(left, right) as f32 * 1000.0 / self.units_per_em as f32
    }

    /// Advance width in font units.
    pub fn advance(&self, gid: u16) -> u16 {
        self.advances.get(gid as usize).copied().unwrap_or(0)
    }

    /// Advance width scaled to 1/1000 em, as PDF widths are expressed.
    pub fn advance_1000(&self, gid: u16) -> f32 {
        self.advance(gid) as f32 * 1000.0 / self.units_per_em as f32
    }

    pub fn scale_1000(&self, v: i16) -> f32 {
        v as f32 * 1000.0 / self.units_per_em as f32
    }

    fn table(&self, tag: &[u8; 4]) -> Option<&[u8]> {
        let &(o, l) = self.tables.get(tag)?;
        self.data.get(o..o + l)
    }

    fn glyph_data(&self, gid: u16) -> &[u8] {
        let (Some(loca), Some(glyf)) = (self.table(b"loca"), self.table(b"glyf")) else {
            return &[];
        };
        let g = gid as usize;
        let (a, b) = if self.loca_long {
            (
                u32_at(loca, 4 * g).map(|v| v as usize),
                u32_at(loca, 4 * g + 4).map(|v| v as usize),
            )
        } else {
            (
                u16_at(loca, 2 * g).map(|v| v as usize * 2),
                u16_at(loca, 2 * g + 2).map(|v| v as usize * 2),
            )
        };
        match (a, b) {
            (Some(a), Some(b)) if a <= b => glyf.get(a..b).unwrap_or(&[]),
            _ => &[],
        }
    }

    /// Glyphs referenced by a composite glyph.
    fn components(&self, gid: u16) -> Vec<u16> {
        let g = self.glyph_data(gid);
        let mut out = Vec::new();
        if i16_at(g, 0).unwrap_or(0) >= 0 {
            return out;
        }
        let mut p = 10;
        while let (Some(flags), Some(child)) = (u16_at(g, p), u16_at(g, p + 2)) {
            out.push(child);
            p += 4 + if flags & 0x0001 != 0 { 4 } else { 2 };
            p += if flags & 0x0008 != 0 {
                2
            } else if flags & 0x0040 != 0 {
                4
            } else if flags & 0x0080 != 0 {
                8
            } else {
                0
            };
            if flags & 0x0020 == 0 || out.len() > 256 {
                break;
            }
        }
        out
    }

    /// Build a TrueType font containing only the glyphs in `used` (plus
    /// `.notdef` and composite-glyph components). Glyph IDs are preserved,
    /// so the PDF can address glyphs directly (CIDToGIDMap /Identity).
    pub fn subset(&self, used: &BTreeSet<u16>) -> Vec<u8> {
        let mut keep: BTreeSet<u16> = BTreeSet::new();
        let mut work: Vec<u16> = if self.no_subset {
            (0..self.num_glyphs).collect()
        } else {
            used.iter()
                .copied()
                .chain([0])
                .filter(|&g| g < self.num_glyphs)
                .collect()
        };
        while let Some(g) = work.pop() {
            if g < self.num_glyphs && keep.insert(g) {
                work.extend(self.components(g));
            }
        }
        let n = keep.last().map_or(1, |&m| m as usize + 1);

        let mut glyf = Vec::new();
        let mut loca = Vec::with_capacity(4 * (n + 1));
        for g in 0..n as u16 {
            loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
            if keep.contains(&g) {
                glyf.extend_from_slice(self.glyph_data(g));
                while glyf.len() % 4 != 0 {
                    glyf.push(0);
                }
            }
        }
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());

        let mut hmtx = Vec::with_capacity(4 * n);
        for g in 0..n {
            // Unused slots get zero metrics, which compress to almost nothing.
            let (adv, lsb) = if keep.contains(&(g as u16)) {
                (self.advances[g], self.lsbs[g])
            } else {
                (0, 0)
            };
            hmtx.extend_from_slice(&adv.to_be_bytes());
            hmtx.extend_from_slice(&lsb.to_be_bytes());
        }
        let patch = |tag: &[u8; 4], f: &dyn Fn(&mut Vec<u8>)| -> Option<Vec<u8>> {
            let mut t = self.table(tag)?.to_vec();
            f(&mut t);
            Some(t)
        };
        let set16 = |t: &mut Vec<u8>, at: usize, v: u16| {
            if let Some(s) = t.get_mut(at..at + 2) {
                s.copy_from_slice(&v.to_be_bytes());
            }
        };
        let mut out_tables: BTreeMap<[u8; 4], Vec<u8>> = BTreeMap::new();
        if let Some(h) = patch(b"head", &|t| {
            set16(t, 50, 1); // long loca
            if let Some(s) = t.get_mut(8..12) {
                s.fill(0); // checkSumAdjustment, fixed below
            }
        }) {
            out_tables.insert(*b"head", h);
        }
        if let Some(t) = patch(b"hhea", &|t| set16(t, 34, n as u16)) {
            out_tables.insert(*b"hhea", t);
        }
        if let Some(t) = patch(b"maxp", &|t| set16(t, 4, n as u16)) {
            out_tables.insert(*b"maxp", t);
        }
        if let Some(t) = patch(b"post", &|t| {
            t.truncate(32);
            t.resize(32, 0);
            t[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
        }) {
            out_tables.insert(*b"post", t);
        }
        for tag in [b"cvt ", b"fpgm", b"prep", b"OS/2"] {
            if let Some(t) = self.table(tag) {
                out_tables.insert(*tag, t.to_vec());
            }
        }
        out_tables.insert(*b"glyf", glyf);
        out_tables.insert(*b"loca", loca);
        out_tables.insert(*b"hmtx", hmtx);
        // No cmap: PDF content addresses glyphs by ID (Identity-H with
        // CIDToGIDMap /Identity), and the ToUnicode map provides text
        // extraction, so the character map is not needed in the subset.
        write_sfnt(&out_tables)
    }
}

fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(b));
    }
    sum
}

fn write_sfnt(tables: &BTreeMap<[u8; 4], Vec<u8>>) -> Vec<u8> {
    let n = tables.len() as u16;
    let mut pow = 1u16;
    let mut log = 0u16;
    while pow * 2 <= n {
        pow *= 2;
        log += 1;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    for v in [n, pow * 16, log, n * 16 - pow * 16] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut offset = 12 + 16 * tables.len();
    let mut head_at = None;
    for (tag, data) in tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_at = Some(offset);
        }
        offset += data.len().div_ceil(4) * 4;
    }
    for data in tables.values() {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    if let Some(h) = head_at {
        let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        if let Some(s) = out.get_mut(h + 8..h + 12) {
            s.copy_from_slice(&adj.to_be_bytes());
        }
    }
    out
}

/// Default value of the `wght` axis of a variable font (400 if it has none).
///
/// `fvar` header: majorVersion, minorVersion, axesArrayOffset (offset 4),
/// reserved, axisCount (8), axisSize (10), instanceCount, instanceSize.
/// Each VariationAxisRecord: axisTag, minValue, defaultValue (+8), maxValue
/// as 16.16 fixed-point numbers, then flags and axisNameID.
fn fvar_default_weight(fvar: &[u8]) -> f32 {
    let axes = u16_at(fvar, 4).unwrap_or(0) as usize;
    let count = u16_at(fvar, 8).unwrap_or(0) as usize;
    let size = u16_at(fvar, 10).unwrap_or(20) as usize;
    (0..count)
        .map(|k| axes + k * size)
        .find(|&a| fvar.get(a..a + 4) == Some(b"wght"))
        .and_then(|a| u32_at(fvar, a + 8))
        .map_or(400.0, |v| (v as i32 as f32 / 65536.0).round())
}

fn postscript_name(name: &[u8]) -> Option<String> {
    let count = u16_at(name, 2)? as usize;
    let storage = u16_at(name, 4)? as usize;
    let mut best: Option<String> = None;
    for k in 0..count {
        let r = 6 + 12 * k;
        let (platform, name_id) = (u16_at(name, r)?, u16_at(name, r + 6)?);
        if name_id != 6 {
            continue;
        }
        let len = u16_at(name, r + 8)? as usize;
        let off = storage + u16_at(name, r + 10)? as usize;
        let raw = name.get(off..off + len)?;
        let s: String = match platform {
            0 | 3 => char::decode_utf16(raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])))
                .map(|c| c.unwrap_or('?'))
                .collect(),
            _ => raw.iter().map(|&b| b as char).collect(),
        };
        best = Some(s);
        if platform == 3 {
            break;
        }
    }
    // Keep only characters that are safe in a PDF name.
    let clean: String = best?
        .chars()
        .filter(|c| c.is_ascii_graphic() && !"[](){}<>/%#".contains(*c))
        .take(63)
        .collect();
    (!clean.is_empty()).then_some(clean)
}

fn parse_cmap(cmap: &[u8], num_glyphs: u16) -> R<HashMap<u32, u16>> {
    let n = u16_at(cmap, 2).ok_or_else(|| bad("cmap"))? as usize;
    // Prefer full-Unicode format 12 subtables, then BMP format 4.
    let mut best: Option<(u8, usize)> = None;
    for k in 0..n {
        let r = 4 + 8 * k;
        let (Some(platform), Some(encoding), Some(off)) =
            (u16_at(cmap, r), u16_at(cmap, r + 2), u32_at(cmap, r + 4))
        else {
            break;
        };
        let off = off as usize;
        let format = u16_at(cmap, off).unwrap_or(0);
        let rank = match (platform, encoding, format) {
            (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => 3,
            (3, 1, 4) | (0, 3, 4) => 2,
            (0, _, 4) => 1,
            _ => 0,
        };
        if rank > 0 && best.is_none_or(|(r, _)| rank > r) {
            best = Some((rank, off));
        }
    }
    let Some((_, off)) = best else {
        return Err("font has no Unicode character map".into());
    };
    let t = cmap.get(off..).ok_or_else(|| bad("cmap"))?;
    let mut map = HashMap::new();
    let mut add = |cp: u32, gid: u32| {
        if gid != 0 && gid < num_glyphs as u32 && char::from_u32(cp).is_some() {
            map.entry(cp).or_insert(gid as u16);
        }
    };
    match u16_at(t, 0) {
        Some(4) => {
            let seg2 = u16_at(t, 6).ok_or_else(|| bad("cmap format 4"))? as usize;
            let (ends, starts, deltas, ranges) = (14, 16 + seg2, 16 + 2 * seg2, 16 + 3 * seg2);
            for s in 0..seg2 / 2 {
                let (Some(end), Some(start), Some(delta), Some(range)) = (
                    u16_at(t, ends + 2 * s),
                    u16_at(t, starts + 2 * s),
                    u16_at(t, deltas + 2 * s),
                    u16_at(t, ranges + 2 * s),
                ) else {
                    return Err(bad("cmap format 4"));
                };
                if start > end {
                    continue;
                }
                for c in start..=end {
                    if c == 0xFFFF {
                        break;
                    }
                    let gid = if range == 0 {
                        c.wrapping_add(delta)
                    } else {
                        let at = ranges + 2 * s + range as usize + 2 * (c - start) as usize;
                        match u16_at(t, at) {
                            Some(0) | None => 0,
                            Some(g) => g.wrapping_add(delta),
                        }
                    };
                    add(c as u32, gid as u32);
                }
            }
        }
        Some(12) => {
            let groups = u32_at(t, 12).ok_or_else(|| bad("cmap format 12"))? as usize;
            let mut total = 0u64;
            for k in 0..groups {
                let g = 16 + 12 * k;
                let (Some(start), Some(end), Some(gid)) = (u32_at(t, g), u32_at(t, g + 4), u32_at(t, g + 8))
                else {
                    return Err(bad("cmap format 12"));
                };
                if start > end || end > 0x10FFFF {
                    continue;
                }
                total += (end - start) as u64 + 1;
                if total > 0x110000 {
                    return Err(bad("cmap format 12 too large"));
                }
                for c in start..=end {
                    add(c, gid.saturating_add(c - start));
                }
            }
        }
        _ => return Err(bad("unsupported cmap format")),
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALEGREYA: &[u8] = include_bytes!("../fonts/Alegreya-Regular.ttf");

    #[test]
    fn parses_bundled_font() {
        let f = Face::parse(Cow::Borrowed(ALEGREYA), 0).unwrap();
        assert!(f.glyph('A').is_some());
        assert!(f.glyph('Ж').is_some());
        assert!(f.glyph('中').is_none());
        assert!(f.advance(f.glyph('M').unwrap()) > f.advance(f.glyph('i').unwrap()));
        assert!(f.postscript_name.starts_with("Alegreya"));
        assert!(!f.italic && !f.no_subset);
    }

    #[test]
    fn subset_is_a_valid_smaller_font() {
        let f = Face::parse(Cow::Borrowed(ALEGREYA), 0).unwrap();
        let used: BTreeSet<u16> = "Héllo".chars().filter_map(|c| f.glyph(c)).collect();
        let sub = f.subset(&used);
        assert!(sub.len() < ALEGREYA.len() / 4, "{} bytes", sub.len());
        assert_eq!(checksum(&sub), 0xB1B0_AFBA, "whole-font checksum must match");
        // Every used glyph's outline is carried over unchanged.
        for g in used {
            let outline = f.glyph_data(g);
            assert!(!outline.is_empty());
            assert!(
                sub.windows(outline.len()).any(|w| w == outline),
                "glyph {g} missing"
            );
        }
    }

    #[test]
    fn composite_components_are_kept() {
        let f = Face::parse(Cow::Borrowed(ALEGREYA), 0).unwrap();
        let composite = "éÅñÖ"
            .chars()
            .filter_map(|c| f.glyph(c))
            .find(|&g| !f.components(g).is_empty());
        let g = composite.expect("Alegreya has composite accented glyphs");
        let sub = f.subset(&BTreeSet::from([g]));
        for part in f.components(g) {
            let outline = f.glyph_data(part);
            assert!(outline.is_empty() || sub.windows(outline.len()).any(|w| w == outline));
        }
    }

    #[test]
    fn gpos_pair_kerning() {
        let f = Face::parse(Cow::Borrowed(ALEGREYA), 0).unwrap();
        let k = |a: char, b: char| f.kern(f.glyph(a).unwrap(), f.glyph(b).unwrap());
        // Reference values from fontTools for the class-based (format 2) subtable.
        assert_eq!(k('A', 'V'), -42);
        assert_eq!(k('T', 'o'), -60);
        assert_eq!(k('L', 'T'), -40);
        assert_eq!(k('Γ', 'α'), -60);
        assert_eq!(k('Г', 'А'), -55);
        assert_eq!(k('o', 'o'), 0);
        // A monospace font has no kerning.
        let mono = Face::parse(Cow::Borrowed(include_bytes!("../fonts/Cousine-Regular.ttf")), 0).unwrap();
        assert_eq!(mono.kern(mono.glyph('A').unwrap(), mono.glyph('V').unwrap()), 0);
    }

    #[test]
    fn gsub_ligatures() {
        let f = Face::parse(Cow::Borrowed(ALEGREYA), 0).unwrap();
        let shape = |s: &str| {
            let mut g: Vec<Glyph> = s
                .chars()
                .enumerate()
                .map(|(k, c)| Glyph {
                    id: f.glyph(c).unwrap(),
                    cluster: k as u32,
                    mask: crate::gsub::mask::GLOBAL,
                    component: 0,
                })
                .collect();
            f.substitute(Script::Latin, &mut g);
            g
        };
        // Glyph IDs and clusters as HarfBuzz produces them.
        let fi = shape("fi");
        assert_eq!(fi.len(), 1);
        assert_ne!(fi[0].id, f.glyph('f').unwrap());
        let office = shape("office");
        assert_eq!(
            office.iter().map(|g| g.cluster).collect::<Vec<_>>(),
            [0, 1, 2, 4, 5]
        );
        assert_eq!(shape("xyz").len(), 3);
    }

    #[test]
    fn corrupt_gsub_never_panics() {
        let f = Face::parse(Cow::Borrowed(ALEGREYA), 0).unwrap();
        let (gsub, _) = f.tables[b"GSUB"];
        let (gdef, gdef_len) = f.tables[b"GDEF"];
        let mut seed = 11u64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..200 {
            let mut d = ALEGREYA.to_vec();
            for _ in 0..8 {
                let r = rand();
                let i = if r % 4 == 0 {
                    gdef + (r as usize >> 8) % gdef_len
                } else {
                    // Headers, lists and the first subtables.
                    gsub + (r as usize >> 8) % 4096
                };
                d[i] = (r >> 3) as u8;
            }
            let Ok(face) = Face::parse(Cow::Owned(d), 0) else {
                continue;
            };
            let mut g: Vec<Glyph> = (0..24)
                .map(|k| Glyph {
                    id: (rand() % 1200) as u16 | if round % 2 == 0 { 0 } else { 300 },
                    cluster: k,
                    mask: crate::gsub::mask::GLOBAL,
                    component: 0,
                })
                .collect();
            for script in Script::ALL {
                face.substitute(script, &mut g);
            }
        }
    }

    #[test]
    fn fvar_default_weight_follows_the_spec_layout() {
        // Header with the axis array at offset 16 and 20-byte axis records;
        // `wght` is the second axis to exercise the walk.
        let fixed = |v: f32| ((v * 65536.0) as i32).to_be_bytes();
        let mut t = Vec::new();
        for v in [1u16, 0, 16, 2, 2, 20, 0, 8] {
            t.extend_from_slice(&v.to_be_bytes());
        }
        for (tag, min, def, max) in [(b"wdth", 75.0, 100.0, 100.0), (b"wght", 100.0, 350.0, 900.0)] {
            t.extend_from_slice(tag);
            for v in [min, def, max] {
                t.extend_from_slice(&fixed(v));
            }
            t.extend_from_slice(&[0, 0, 1, 0]); // flags, axisNameID
        }
        assert_eq!(fvar_default_weight(&t), 350.0);
        // Only a width axis: treated as regular weight.
        let mut w = t[..36].to_vec();
        w[9] = 1;
        assert_eq!(fvar_default_weight(&w), 400.0);
        // Truncated tables never panic.
        for n in 0..t.len() {
            let _ = fvar_default_weight(&t[..n]);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(Face::parse(Cow::Borrowed(b"not a font"), 0).is_err());
        assert!(Face::parse(Cow::Borrowed(b"OTTO\0\0\0\0"), 0)
            .err()
            .unwrap()
            .contains("CFF"));
        // Corrupt the table directory and the start of the tables; parsing
        // and subsetting must fail cleanly or succeed, never panic.
        let mut seed = 3u64;
        for _ in 0..300 {
            let mut d = ALEGREYA.to_vec();
            for _ in 0..6 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let i = (seed as usize >> 8) % 2048;
                d[i] = (seed >> 3) as u8;
            }
            if let Ok(f) = Face::parse(Cow::Owned(d), 0) {
                let used: BTreeSet<u16> = "Ab".chars().filter_map(|c| f.glyph(c)).collect();
                let _ = f.subset(&used);
                for l in 0..40 {
                    let _ = f.kern(l, 40 - l);
                }
            }
        }
    }
}
