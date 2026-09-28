//! PDF/A-2 (ISO 19005-2), the archival form of PDF: everything needed to
//! show the document is inside it (fonts are embedded anyway), colours are
//! tied to a colour space by an output intent, and the document describes
//! itself in XMP metadata.
//!
//! A document conforms at level A (accessible) when it is tagged and every
//! glyph maps to Unicode, and at level B (basic, visual) when some glyph
//! stands for no text of its own. It cannot conform when it holds a CMYK
//! image, as the output intent is RGB.

use crate::layout::Output;

/// The PDF/A conformance level a document reaches (`'A'` or `'B'`), or why
/// it is not PDF/A.
///
/// Every document is tagged, with all its content marked, so level A turns
/// on whether every glyph maps to Unicode. It is not PDF/UA, which asks
/// more (a language, headings that do not skip levels), and is not
/// declared.
pub fn conformance(doc: &Output) -> Result<char, String> {
    if doc.images.iter().any(|img| img.dict.contains("/DeviceCMYK")) {
        return Err("it has a CMYK image, and its colours are RGB".into());
    }
    // Glyphs that stand for no text: the second and later glyphs a
    // character is set with.
    let unmapped = doc
        .used
        .iter()
        .any(|used| used.iter().any(|(&g, text)| g != 0 && text.is_empty()));
    Ok(if unmapped { 'B' } else { 'A' })
}

/// Escape text for XML, leaving out characters XML does not allow.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {}
            c => out.push(c),
        }
    }
    out
}

/// The document's XMP metadata packet: its title, language and producer,
/// which match the document information dictionary, and its PDF/A part and
/// conformance level, if it conforms.
pub fn xmp(title: Option<&str>, lang: Option<&str>, producer: &str, level: Option<char>) -> String {
    let mut x = String::from(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         <rdf:Description rdf:about=\"\"\n  \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n  \
         xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n  \
         xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\">\n\
         <dc:format>application/pdf</dc:format>\n",
    );
    if let Some(t) = title {
        x.push_str(&format!(
            "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n",
            xml_escape(t)
        ));
    }
    if let Some(l) = lang {
        x.push_str(&format!(
            "<dc:language><rdf:Bag><rdf:li>{}</rdf:li></rdf:Bag></dc:language>\n",
            xml_escape(l)
        ));
    }
    x.push_str(&format!(
        "<pdf:Producer>{}</pdf:Producer>\n",
        xml_escape(producer)
    ));
    if let Some(level) = level {
        x.push_str(&format!(
            "<pdfaid:part>2</pdfaid:part>\n<pdfaid:conformance>{level}</pdfaid:conformance>\n"
        ));
    }
    x.push_str("</rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>");
    x
}

/// An ICC (version 2) profile of the sRGB colour space (IEC 61966-2-1),
/// for the output intent: the sRGB primaries adapted to D50, and its tone
/// curve sampled at each 8-bit value.
pub fn srgb_profile() -> Vec<u8> {
    fn s15(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32).to_be_bytes()
    }
    fn xyz(v: [f64; 3]) -> Vec<u8> {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        for c in v {
            t.extend_from_slice(&s15(c));
        }
        t
    }
    let name = b"sRGB IEC61966-2.1";
    let mut desc = b"desc\0\0\0\0".to_vec();
    desc.extend_from_slice(&(name.len() as u32 + 1).to_be_bytes());
    desc.extend_from_slice(name);
    desc.push(0);
    // No Unicode or ScriptCode description.
    desc.extend_from_slice(&[0; 4 + 4 + 2 + 1 + 67]);
    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend_from_slice(b"No copyright, use freely\0");
    let mut curv = b"curv\0\0\0\0".to_vec();
    let n = 256;
    curv.extend_from_slice(&(n as u32).to_be_bytes());
    for k in 0..n {
        let v = k as f64 / (n - 1) as f64;
        let linear = if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        };
        curv.extend_from_slice(&((linear * 65535.0).round() as u16).to_be_bytes());
    }
    // The three tone curves share one tag.
    let tags: [(&[u8; 4], Vec<u8>); 9] = [
        (b"desc", desc),
        (b"cprt", cprt),
        (b"wtpt", xyz([0.9642, 1.0, 0.8249])),
        (b"rXYZ", xyz([0.4360747, 0.2225045, 0.0139322])),
        (b"gXYZ", xyz([0.3850649, 0.7168786, 0.0971045])),
        (b"bXYZ", xyz([0.1430804, 0.0606169, 0.7141733])),
        (b"rTRC", curv),
        (b"gTRC", Vec::new()),
        (b"bTRC", Vec::new()),
    ];
    let mut table = Vec::new();
    let mut data = Vec::new();
    let start = 128 + 4 + 12 * tags.len();
    let (mut curve_at, mut curve_len) = (0, 0);
    for (sig, body) in &tags {
        let (at, len) = if body.is_empty() {
            (curve_at, curve_len)
        } else {
            let at = start + data.len();
            data.extend_from_slice(body);
            while data.len() % 4 != 0 {
                data.push(0);
            }
            (at, body.len())
        };
        if &sig[1..] == b"TRC" {
            (curve_at, curve_len) = (at, len);
        }
        table.extend_from_slice(*sig);
        table.extend_from_slice(&(at as u32).to_be_bytes());
        table.extend_from_slice(&(len as u32).to_be_bytes());
    }
    let size = start + data.len();
    let mut p = Vec::with_capacity(size);
    p.extend_from_slice(&(size as u32).to_be_bytes());
    p.extend_from_slice(&[0; 4]); // preferred CMM
    p.extend_from_slice(&[2, 0x10, 0, 0]); // version 2.1
    p.extend_from_slice(b"mntrRGB XYZ ");
    // Created 2000-01-01.
    for v in [2000u16, 1, 1, 0, 0, 0] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(b"acsp");
    p.extend_from_slice(&[0; 4 + 4 + 4 + 4 + 8 + 4]); // platform to rendering intent
    for c in [0.9642, 1.0, 0.8249] {
        p.extend_from_slice(&s15(c));
    }
    p.resize(128, 0);
    p.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    p.extend_from_slice(&table);
    p.extend_from_slice(&data);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_profile_is_well_formed() {
        let p = srgb_profile();
        assert_eq!(u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize, p.len());
        assert_eq!(&p[12..24], b"mntrRGB XYZ ");
        assert_eq!(&p[36..40], b"acsp");
        let count = u32::from_be_bytes(p[128..132].try_into().unwrap()) as usize;
        assert_eq!(count, 9);
        for k in 0..count {
            let e = &p[132 + 12 * k..144 + 12 * k];
            let at = u32::from_be_bytes(e[4..8].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(e[8..12].try_into().unwrap()) as usize;
            assert!(at.is_multiple_of(4) && at + len <= p.len());
            let kind = &p[at..at + 4];
            match &e[..4] {
                b"rTRC" | b"gTRC" | b"bTRC" => assert_eq!(kind, b"curv"),
                b"desc" => assert_eq!(kind, b"desc"),
                b"cprt" => assert_eq!(kind, b"text"),
                _ => assert_eq!(kind, b"XYZ "),
            }
        }
    }

    #[test]
    fn xmp_escapes_and_declares_the_level() {
        let x = xmp(Some("A <b> & \"c\"\u{1}"), Some("en"), "sundowner", Some('A'));
        assert!(x.contains("A &lt;b&gt; &amp; &quot;c&quot;</rdf:li>"));
        assert!(x.contains("<pdfaid:part>2</pdfaid:part>"));
        assert!(x.contains("<pdfaid:conformance>A</pdfaid:conformance>"));
        assert!(!xmp(None, None, "sundowner", None).contains("pdfaid:part"));
    }
}
