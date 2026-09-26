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
    silk!("NotoSerifSC-Regular.ttf"),
    silk!("NotoSerifSC-Bold.ttf"),
    silk!("GowunBatang-Regular.ttf"),
    silk!("GowunBatang-Bold.ttf")
);

/// Number of bundled fonts in the base set (Alegreya and Cousine).
const BASE_FONTS: usize = 8;

/// Number of families of the tier that come before Cousine in the fallback
/// chains: the Hebrew and Armenian ones, so Hebrew is not set in Cousine.
/// The CJK fonts come after it, so the symbols and box drawing characters
/// that Cousine has keep coming from it (and do not unpack a CJK font).
const TIER_BEFORE_MONO: usize = if cfg!(feature = "silk") { 3 } else { 0 };

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
        "Noto Serif SC (Regular, Bold; static instances generated from the variable font)",
        include_str!("../fonts/silk/OFL-NotoSerifSC.txt"),
    ),
    #[cfg(feature = "silk")]
    (
        "Gowun Batang (Regular, Bold; unmodified)",
        include_str!("../fonts/silk/OFL-GowunBatang.txt"),
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
    /// families, Cousine, and the tier's CJK families.
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
        // The families of the tier: regular and bold, no italics.
        let tier: Vec<Family> = (BASE_FONTS..BUNDLED.len())
            .step_by(2)
            .map(|i| Family {
                regular: i,
                bold: Some(i + 1),
                italic: None,
                bold_italic: None,
            })
            .collect();

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
            let (alphabets, cjk) = tier.split_at(TIER_BEFORE_MONO);
            for f in families
                .chain([main_body, serif])
                .chain(alphabets.iter().copied())
                .chain([mono])
                .chain(cjk.iter().copied())
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
        let cases: [(&str, &[char], &str); 6] = [
            ("Hebrew", &hebrew, "FrankRuhlLibre"),
            ("cantillation", &chars(0x591..=0x5AF), "NotoSerifHebrew"),
            ("Armenian", &armenian, "NotoSerifArmenian"),
            ("Han", &han, "NotoSerifSC"),
            ("kana", &kana, "NotoSerifSC"),
            ("Hangul", &chars(0xAC00..=0xD7A3), "GowunBatang"),
        ];
        for (what, cs, family) in cases {
            for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
                for &c in cs {
                    let (face, _) = f
                        .resolve(c, false, bold, italic)
                        .unwrap_or_else(|| panic!("{what}: U+{:04X} not covered", c as u32));
                    let name = &f.faces[face].postscript_name;
                    assert!(
                        name.starts_with(family) && name.ends_with(if bold { "-Bold" } else { "-Regular" }),
                        "{what}: U+{:04X} set in {name}",
                        c as u32
                    );
                }
            }
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
