//! Font families and per-character fallback.
//!
//! Text is set with a *chain* of families. For every character the first
//! family whose font has a glyph for it is used, so a document can mix
//! scripts as long as some configured font covers each character. Styles
//! are never synthesized: if a family lacks an italic or bold face, its
//! regular face is used as-is. Fonts are only ever loaded from the bundled
//! set or from files the user configures explicitly; installed system fonts
//! are never consulted, so output is identical on every machine.

use crate::ttf::Face;
use std::borrow::Cow;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Index into [`Fonts::faces`].
pub type FaceId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Family {
    pub regular: FaceId,
    pub bold: Option<FaceId>,
    pub italic: Option<FaceId>,
    pub bold_italic: Option<FaceId>,
}

impl Family {
    /// The face for a style, best match first, then the family's regular face.
    fn faces(&self, bold: bool, italic: bool) -> [FaceId; 2] {
        let styled = match (bold, italic) {
            (true, true) => self.bold_italic.or(self.bold).or(self.italic),
            (true, false) => self.bold,
            (false, true) => self.italic,
            (false, false) => None,
        };
        [styled.unwrap_or(self.regular), self.regular]
    }
}

pub struct Fonts {
    pub faces: Faces,
    /// Families tried in order for body text and headings.
    pub body: Vec<Family>,
    /// Families tried in order for code.
    pub mono: Vec<Family>,
    /// Problems worth telling the user about, such as variable fonts.
    pub warnings: Vec<String>,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fonts").field("faces", &self.faces.len()).finish()
    }
}

/// A bundled font file.
pub struct Bundled {
    /// Path below `fonts/`.
    pub file: &'static str,
    pub data: &'static [u8],
    /// `data` is zlib-compressed and inflated when the font is first used.
    pub compressed: bool,
}

macro_rules! base {
    ($file:literal) => {
        Bundled {
            file: $file,
            data: include_bytes!(concat!("../fonts/", $file)),
            compressed: false,
        }
    };
}

#[cfg(feature = "silk")]
macro_rules! silk {
    ($file:literal) => {
        Bundled {
            file: concat!("silk/", $file),
            data: include_bytes!(concat!(env!("OUT_DIR"), "/", $file, ".z")),
            compressed: true,
        }
    };
}

macro_rules! bundled {
    ($($extra:expr),*) => {
        &[
            base!("Alegreya-Regular.ttf"),
            base!("Alegreya-Bold.ttf"),
            base!("Alegreya-Italic.ttf"),
            base!("Alegreya-BoldItalic.ttf"),
            base!("Cousine-Regular.ttf"),
            base!("Cousine-Bold.ttf"),
            base!("Cousine-Italic.ttf"),
            base!("Cousine-BoldItalic.ttf"),
            $($extra),*
        ]
    };
}

/// The name of the set of bundled fonts this binary was built with.
#[cfg(not(feature = "silk"))]
pub const TIER: &str = "europa";
#[cfg(feature = "silk")]
pub const TIER: &str = "silk";

/// The bundled fonts: the base set, then the regular and bold faces of each
/// family of the tier.
#[cfg(not(feature = "silk"))]
pub const BUNDLED: &[Bundled] = bundled!();
#[cfg(feature = "silk")]
pub const BUNDLED: &[Bundled] = bundled!(
    silk!("FrankRuhlLibre-Regular.ttf"),
    silk!("FrankRuhlLibre-Bold.ttf"),
    silk!("NotoSerifHebrew-Regular.ttf"),
    silk!("NotoSerifHebrew-Bold.ttf"),
    silk!("NotoSerifArmenian-Regular.ttf"),
    silk!("NotoSerifArmenian-Bold.ttf"),
    silk!("NotoSerifTC-Regular.ttf"),
    silk!("NotoSerifTC-Bold.ttf"),
    silk!("NotoSerifJP-Regular.ttf"),
    silk!("NotoSerifJP-Bold.ttf"),
    silk!("GowunBatang-Regular.ttf"),
    silk!("GowunBatang-Bold.ttf"),
    silk!("Amiri-Regular.ttf"),
    silk!("Amiri-Bold.ttf"),
    silk!("Amiri-Italic.ttf"),
    silk!("Amiri-BoldItalic.ttf")
);

/// Number of bundled fonts in the base set (Alegreya and Cousine).
#[cfg(test)]
const BASE_FONTS: usize = 8;

/// A family of the tier: the start of its file names in [`BUNDLED`] (the
/// files are `<name>-Regular.ttf`, `-Bold`, `-Italic` and `-BoldItalic`,
/// those that exist), and whether it comes before Cousine in the fallback
/// chains.
type TierFamily = (&'static str, bool);

/// The families of the tier, in fallback order. The Hebrew and Armenian
/// ones come before Cousine, so Hebrew is not set in Cousine. The others
/// come after it, so the symbols, box drawing characters and Latin letters
/// that Cousine has keep coming from it (and do not unpack a large font).
///
/// Chinese characters are set in their traditional forms (Noto Serif TC);
/// Noto Serif JP, cut down to the Japanese kanji TC lacks, completes
/// Japanese. Characters only simplified Chinese uses are not bundled: a
/// simplified Chinese font can be added as a fallback.
#[cfg(not(feature = "silk"))]
const TIER_FAMILIES: &[TierFamily] = &[];
#[cfg(feature = "silk")]
const TIER_FAMILIES: &[TierFamily] = &[
    ("silk/FrankRuhlLibre", true),
    ("silk/NotoSerifHebrew", true),
    ("silk/NotoSerifArmenian", true),
    ("silk/Amiri", false),
    ("silk/NotoSerifTC", false),
    ("silk/NotoSerifJP", false),
    ("silk/GowunBatang", false),
];

/// The bytes of a bundled font, inflated on first use and kept for the
/// rest of the run.
fn bundled_data(i: usize) -> &'static [u8] {
    static INFLATED: [OnceLock<Vec<u8>>; BUNDLED.len()] = [const { OnceLock::new() }; BUNDLED.len()];
    let b = &BUNDLED[i];
    if !b.compressed {
        return b.data;
    }
    INFLATED[i].get_or_init(|| {
        crate::flate::zlib_decompress(b.data, MAX_FONT_FILE as usize).expect("bundled fonts are valid")
    })
}

/// License notices for the bundled fonts, as required by the SIL Open Font
/// License (shown by `sundowner --licenses`).
pub const FONT_LICENSES: &[(&str, &str)] = &[
    (
        "Alegreya (Regular, Bold, Italic, Bold Italic; static instances generated from the variable \
         fonts, see fonts/build.py)",
        include_str!("../fonts/OFL-Alegreya.txt"),
    ),
    (
        "Cousine (Regular, Bold, Italic, Bold Italic; unmodified)",
        include_str!("../fonts/OFL-Cousine.txt"),
    ),
    #[cfg(feature = "silk")]
    (
        "Frank Ruhl Libre (Regular, Bold; static instances generated from the variable font)",
        include_str!("../fonts/silk/OFL-FrankRuhlLibre.txt"),
    ),
    #[cfg(feature = "silk")]
    (
        "Noto Serif Hebrew (Regular, Bold; static instances generated from the variable font)",
        include_str!("../fonts/silk/OFL-NotoSerifHebrew.txt"),
    ),
    #[cfg(feature = "silk")]
    (
        "Noto Serif Armenian (Regular, Bold; static instances generated from the variable font)",
        include_str!("../fonts/silk/OFL-NotoSerifArmenian.txt"),
    ),
    #[cfg(feature = "silk")]
    (
        "Noto Serif TC (Regular, Bold; static instances generated from the variable font)",
        include_str!("../fonts/silk/OFL-NotoSerifTC.txt"),
    ),
    #[cfg(feature = "silk")]
    (
        "Noto Serif JP (Regular, Bold; static instances generated from the variable font, cut down \
         to the Japanese kanji that Noto Serif TC lacks)",
        include_str!("../fonts/silk/OFL-NotoSerifJP.txt"),
    ),
    #[cfg(feature = "silk")]
    (
        "Gowun Batang (Regular, Bold; unmodified)",
        include_str!("../fonts/silk/OFL-GowunBatang.txt"),
    ),
    #[cfg(feature = "silk")]
    (
        "Amiri (Regular, Bold, Italic, Bold Italic; unmodified)",
        include_str!("../fonts/silk/OFL-Amiri.txt"),
    ),
];

/// The loaded faces, indexed by [`FaceId`]. Compressed bundled fonts are
/// only inflated and parsed when a face is first looked at, so a document
/// that never reaches them in a fallback chain does not pay for them.
pub struct Faces(Vec<Slot>);

enum Slot {
    Ready(Face),
    Bundled(usize, OnceLock<Face>),
}

impl Faces {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn push(&mut self, face: Face) -> FaceId {
        self.0.push(Slot::Ready(face));
        self.0.len() - 1
    }
}

impl std::ops::Index<FaceId> for Faces {
    type Output = Face;

    /// The first look at a compressed bundled face inflates and parses it:
    /// for a CJK font, about 0.15 s and tens of MB. That is what keeps
    /// documents that never need it fast, so it must stay lazy.
    fn index(&self, id: FaceId) -> &Face {
        match &self.0[id] {
            Slot::Ready(face) => face,
            Slot::Bundled(i, face) => face.get_or_init(|| {
                Face::parse(Cow::Borrowed(bundled_data(*i)), 0).expect("bundled fonts are valid")
            }),
        }
    }
}

/// A family whose faces are given as font files: `(path, collection index)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FamilySpec {
    pub regular: Option<(std::path::PathBuf, u32)>,
    pub bold: Option<(std::path::PathBuf, u32)>,
    pub italic: Option<(std::path::PathBuf, u32)>,
    pub bold_italic: Option<(std::path::PathBuf, u32)>,
}

/// User font configuration: replacements for the bundled body and code
/// families, and extra fallback families tried after them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontSpec {
    pub body: Option<FamilySpec>,
    pub mono: Option<FamilySpec>,
    pub fallbacks: Vec<FamilySpec>,
}

/// Largest font file that will be loaded.
const MAX_FONT_FILE: u64 = 512 << 20;

impl Fonts {
    /// The bundled fonts only: Alegreya for text, Cousine for code.
    pub fn builtin() -> Arc<Fonts> {
        static BUILTIN: OnceLock<Arc<Fonts>> = OnceLock::new();
        BUILTIN
            .get_or_init(|| Arc::new(Fonts::load(&FontSpec::default()).expect("bundled fonts are valid")))
            .clone()
    }

    /// Load the configured fonts. The chains are:
    ///
    /// - body: body family (default Alegreya), fallbacks, bundled families
    /// - code: code family (default Cousine), fallbacks, body family,
    ///   bundled families
    ///
    /// The bundled families are Alegreya, the tier's Hebrew and Armenian
    /// families, Cousine, and the tier's Arabic and CJK families.
    ///
    /// The bundled fonts are always the final fallback.
    pub fn load(spec: &FontSpec) -> Result<Fonts, String> {
        let mut faces = Faces(Vec::with_capacity(BUNDLED.len()));
        for (i, b) in BUNDLED.iter().enumerate() {
            let slot = if b.compressed {
                Slot::Bundled(i, OnceLock::new())
            } else {
                Slot::Ready(Face::parse(Cow::Borrowed(b.data), 0).map_err(|e| format!("{}: {e}", b.file))?)
            };
            faces.0.push(slot);
        }
        let serif = Family {
            regular: 0,
            bold: Some(1),
            italic: Some(2),
            bold_italic: Some(3),
        };
        let mono = Family {
            regular: 4,
            bold: Some(5),
            italic: Some(6),
            bold_italic: Some(7),
        };
        let tier = |before_mono: bool| {
            let face = |name: &str, style: &str| {
                let file = format!("{name}-{style}.ttf");
                BUNDLED.iter().position(|b| b.file == file)
            };
            TIER_FAMILIES
                .iter()
                .filter(move |f| f.1 == before_mono)
                .map(move |&(name, _)| Family {
                    regular: face(name, "Regular").expect("the regular face of a tier family is bundled"),
                    bold: face(name, "Bold"),
                    italic: face(name, "Italic"),
                    bold_italic: face(name, "BoldItalic"),
                })
        };

        let mut warnings = Vec::new();
        let mut load_family = |f: &FamilySpec, what: &str, faces: &mut Faces| -> Result<Family, String> {
            let mut one =
                |p: &Option<(std::path::PathBuf, u32)>, weight: f32| -> Result<Option<FaceId>, String> {
                    let Some((path, index)) = p else { return Ok(None) };
                    let face = load_face(path, *index)?;
                    if let Some(w) = face.variable_default_weight.filter(|&w| w != weight) {
                        warnings.push(format!(
                        "{}: variable font whose default instance has weight {w}, not {weight}; only the \
                         default instance can be embedded. Use a static font file for this style.",
                        path.display()
                    ));
                    }
                    Ok(Some(faces.push(face)))
                };
            let regular =
                one(&f.regular, 400.0)?.ok_or_else(|| format!("{what}: a 'regular' font is required"))?;
            Ok(Family {
                regular,
                bold: one(&f.bold, 700.0)?,
                italic: one(&f.italic, 400.0)?,
                bold_italic: one(&f.bold_italic, 700.0)?,
            })
        };
        let user_body = spec
            .body
            .as_ref()
            .map(|f| load_family(f, "[body]", &mut faces))
            .transpose()?;
        let user_mono = spec
            .mono
            .as_ref()
            .map(|f| load_family(f, "[mono]", &mut faces))
            .transpose()?;
        let fallbacks = spec
            .fallbacks
            .iter()
            .map(|f| load_family(f, "[fallback]", &mut faces))
            .collect::<Result<Vec<_>, _>>()?;

        let main_body = user_body.unwrap_or(serif);
        let main_mono = user_mono.unwrap_or(mono);
        let chain = |first: Family| -> Vec<Family> {
            let mut out: Vec<Family> = Vec::new();
            let families = [first].into_iter().chain(fallbacks.iter().copied());
            for f in families
                .chain([main_body, serif])
                .chain(tier(true))
                .chain([mono])
                .chain(tier(false))
            {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
            out
        };
        let body = chain(main_body);
        let code = chain(main_mono);
        Ok(Fonts {
            faces,
            body,
            mono: code,
            warnings,
        })
    }

    /// Find the face and glyph for `c`, trying each family of the chain.
    pub fn resolve(&self, c: char, mono: bool, bold: bool, italic: bool) -> Option<(FaceId, u16)> {
        let chain = if mono { &self.mono } else { &self.body };
        for fam in chain {
            for face in fam.faces(bold, italic) {
                if let Some(g) = self.faces[face].glyph(c) {
                    return Some((face, g));
                }
            }
        }
        None
    }

    /// The faces tried for text in this style, in the order [`Fonts::resolve`]
    /// tries them.
    pub fn candidates(&self, mono: bool, bold: bool, italic: bool) -> Vec<FaceId> {
        let chain = if mono { &self.mono } else { &self.body };
        let mut out = Vec::new();
        for fam in chain {
            for face in fam.faces(bold, italic) {
                if !out.contains(&face) {
                    out.push(face);
                }
            }
        }
        out
    }

    /// The face used for characters no font covers (its `.notdef` glyph).
    pub fn primary(&self, mono: bool, bold: bool, italic: bool) -> FaceId {
        let chain = if mono { &self.mono } else { &self.body };
        chain[0].faces(bold, italic)[0]
    }

    pub fn width(&self, face: FaceId, gid: u16, size: f32) -> f32 {
        self.faces[face].advance_1000(gid) * size / 1000.0
    }
}

fn load_face(path: &Path, index: u32) -> Result<Face, String> {
    let shown = path.display();
    let meta = std::fs::metadata(path).map_err(|e| format!("{shown}: {e}"))?;
    if !meta.is_file() || meta.len() > MAX_FONT_FILE {
        return Err(format!(
            "{shown}: not a regular file, or larger than {} MiB",
            MAX_FONT_FILE >> 20
        ));
    }
    let data = std::fs::read(path).map_err(|e| format!("{shown}: {e}"))?;
    Face::parse(Cow::Owned(data), index).map_err(|e| format!("{shown}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_chain() {
        let f = Fonts::builtin();
        let (face, _) = f.resolve('a', false, false, false).unwrap();
        assert!(f.faces[face].postscript_name.contains("Alegreya"));
        let (face, _) = f.resolve('a', true, false, false).unwrap();
        assert!(f.faces[face].postscript_name.contains("Cousine"));
        let (face, _) = f.resolve('a', true, false, true).unwrap();
        assert!(f.faces[face].italic && f.faces[face].postscript_name.contains("Cousine"));
        let (face, _) = f.resolve('a', false, true, true).unwrap();
        assert!(f.faces[face].italic);
        #[cfg(not(feature = "silk"))]
        assert!(f.resolve('中', false, false, false).is_none());
        assert!(f.warnings.is_empty());
    }

    fn bundled_path(name: &str) -> Option<(std::path::PathBuf, u32)> {
        Some((
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fonts")
                .join(name),
            0,
        ))
    }

    #[test]
    fn fallbacks_come_after_the_role_family() {
        // A fallback that covers Latin must not take over body text or code.
        let spec = FontSpec {
            fallbacks: vec![FamilySpec {
                regular: bundled_path("Alegreya-Bold.ttf"),
                ..Default::default()
            }],
            ..Default::default()
        };
        let f = Fonts::load(&spec).unwrap();
        let user = BUNDLED.len();
        assert_eq!(f.faces.len(), user + 1);
        assert_eq!(
            f.resolve('x', false, false, false).unwrap().0,
            0,
            "bundled Alegreya Regular"
        );
        assert_eq!(
            f.resolve('x', true, false, false).unwrap().0,
            4,
            "bundled Cousine"
        );
        assert_eq!(f.body[1].regular, user, "the fallback is next in line");
    }

    #[test]
    fn user_body_family_replaces_alegreya() {
        let spec = FontSpec {
            body: Some(FamilySpec {
                regular: bundled_path("Cousine-Regular.ttf"),
                ..Default::default()
            }),
            ..Default::default()
        };
        let f = Fonts::load(&spec).unwrap();
        let (face, _) = f.resolve('x', false, true, true).unwrap();
        assert_eq!(
            face,
            BUNDLED.len(),
            "no bold italic in the user family: its regular face, never a fake"
        );
        assert!(
            f.body.iter().any(|fam| fam.regular == 4) && f.body.last().unwrap().regular < BUNDLED.len(),
            "bundled fonts remain the last fallbacks"
        );
    }

    /// Every face of the base set must cover Latin (Basic, Latin-1 and Extended-A),
    /// modern and polytonic Greek and Cyrillic, so no style or code falls
    /// back to another font or to boxes.
    #[test]
    fn bundled_fonts_cover_latin_greek_cyrillic() {
        let f = Fonts::builtin();
        let latin = (0x20..=0x7E)
            .chain(0xA0..=0x17F)
            .filter(|&c| c != 0xAD && c != 0x149);
        let greek = (0x384..=0x3CE).filter(|c| ![0x38B, 0x38D, 0x3A2].contains(c));
        let cyrillic = 0x400..=0x45F;
        // Greek Extended (polytonic), minus its unassigned code points.
        const UNASSIGNED: [u32; 22] = [
            0x1F16, 0x1F17, 0x1F1E, 0x1F1F, 0x1F46, 0x1F47, 0x1F4E, 0x1F4F, 0x1F58, 0x1F5A, 0x1F5C, 0x1F5E,
            0x1F7E, 0x1F7F, 0x1FB5, 0x1FC5, 0x1FD4, 0x1FD5, 0x1FDC, 0x1FF0, 0x1FF1, 0x1FF5,
        ];
        let polytonic = (0x1F00..=0x1FFE).filter(|c| !UNASSIGNED.contains(c));
        let wanted: Vec<char> = latin
            .chain(greek)
            .chain(cyrillic)
            .chain(polytonic)
            .filter_map(char::from_u32)
            .collect();
        assert_eq!(
            wanted.len(),
            317 + 72 + 96 + 233,
            "Latin, Greek, Cyrillic, polytonic"
        );
        assert_eq!(f.faces.len(), BUNDLED.len());
        for (i, b) in BUNDLED[..BASE_FONTS].iter().enumerate() {
            let face = &f.faces[i];
            let missing: String = wanted.iter().filter(|&&c| face.glyph(c).is_none()).collect();
            assert!(missing.is_empty(), "{} lacks {missing:?}", b.file);
        }
    }

    /// Every script of the silk tier is set in its intended family, in
    /// every style, with no gaps.
    #[cfg(feature = "silk")]
    #[test]
    fn silk_covers_its_scripts() {
        let f = Fonts::builtin();
        let chars = |r: std::ops::RangeInclusive<u32>| r.filter_map(char::from_u32).collect::<Vec<_>>();
        let hebrew: Vec<char> = chars(0x5D0..=0x5EA)
            .into_iter()
            .chain(chars(0x5B0..=0x5BD))
            .chain([
                '\u{5BF}', '\u{5C1}', '\u{5C2}', '\u{5C7}', '\u{5BE}', '\u{5F3}', '\u{5F4}',
            ])
            .collect();
        let armenian: Vec<char> = chars(0x531..=0x556)
            .into_iter()
            .chain(chars(0x559..=0x55F))
            .chain(chars(0x561..=0x587))
            .chain(['\u{589}', '\u{58A}'])
            .collect();
        let han: Vec<char> = chars(0x3400..=0x4DBF)
            .into_iter()
            .chain(chars(0x4E00..=0x9FFF))
            .collect();
        let kana: Vec<char> = chars(0x3041..=0x3096)
            .into_iter()
            .chain(chars(0x30A1..=0x30FA))
            .chain(chars(0x3001..=0x303F))
            .collect();
        // Symbols that Cousine has come from it, not from a CJK font.
        for c in ['─', '█', '≡', '♠'] {
            let (face, _) = f.resolve(c, false, false, false).unwrap();
            assert_eq!(face, 4, "{c}");
        }
        let arabic: Vec<char> = chars(0x621..=0x63A)
            .into_iter()
            .chain(chars(0x641..=0x655))
            .chain(chars(0x660..=0x66C))
            .chain([
                '\u{60C}', '\u{61B}', '\u{61F}', '\u{67E}', '\u{686}', '\u{698}', '\u{6AF}', '\u{6CC}',
            ])
            .collect();
        let cases: [(&str, &[char], &str); 6] = [
            ("Hebrew", &hebrew, "FrankRuhlLibre"),
            ("cantillation", &chars(0x591..=0x5AF), "NotoSerifHebrew"),
            ("Armenian", &armenian, "NotoSerifArmenian"),
            ("kana", &kana, "NotoSerifTC"),
            ("Hangul", &chars(0xAC00..=0xD7A3), "GowunBatang"),
            ("Arabic", &arabic, "Amiri"),
        ];
        for (what, cs, family) in cases {
            for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
                for &c in cs {
                    let (face, _) = f
                        .resolve(c, false, bold, italic)
                        .unwrap_or_else(|| panic!("{what}: U+{:04X} not covered", c as u32));
                    let name = &f.faces[face].postscript_name;
                    // Only Amiri has italics.
                    let style = match (bold, italic && family == "Amiri") {
                        (true, true) => "-BoldItalic",
                        (true, false) => "-Bold",
                        (false, true) => "-Italic",
                        (false, false) => "-Regular",
                    };
                    assert!(
                        name.starts_with(family) && name.ends_with(style),
                        "{what}: U+{:04X} set in {name}",
                        c as u32
                    );
                }
            }
        }
        // Chinese characters: in Noto Serif TC where it has them, else in
        // the Japanese kanji of Noto Serif JP, else not at all.
        let in_family = |face: FaceId, family: &str| f.faces[face].postscript_name.starts_with(family);
        let (mut traditional, mut japanese) = (0, 0);
        for bold in [false, true] {
            for &c in &han {
                match f.resolve(c, false, bold, false) {
                    Some((face, _)) if in_family(face, "NotoSerifTC") => traditional += 1,
                    Some((face, _)) if in_family(face, "NotoSerifJP") => japanese += 1,
                    Some((face, _)) => panic!("U+{:04X} set in {}", c as u32, f.faces[face].postscript_name),
                    None => {}
                }
            }
        }
        assert!(
            traditional > 2 * 15_000 && japanese > 2 * 700,
            "{traditional} {japanese}"
        );
        // Kanji simplified in Japan come from Noto Serif JP; characters only
        // simplified Chinese uses are not bundled.
        for c in "気楽図読変帰対経済単歩黒戦".chars() {
            let (face, _) = f.resolve(c, false, false, false).unwrap();
            assert!(in_family(face, "NotoSerifJP"), "{c}");
        }
        for c in "這們氣樂".chars() {
            let (face, _) = f.resolve(c, false, false, false).unwrap();
            assert!(in_family(face, "NotoSerifTC"), "{c}");
        }
        for c in "这们语说".chars() {
            assert!(f.resolve(c, false, false, false).is_none(), "{c}");
        }
    }

    #[test]
    fn user_fonts_must_exist_and_be_valid() {
        let spec = FontSpec {
            fallbacks: vec![FamilySpec {
                regular: Some(("/nonexistent.ttf".into(), 0)),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(Fonts::load(&spec).unwrap_err().contains("nonexistent"));
        let spec = FontSpec {
            body: Some(FamilySpec::default()),
            ..Default::default()
        };
        assert!(Fonts::load(&spec).unwrap_err().contains("regular"));
    }
}
