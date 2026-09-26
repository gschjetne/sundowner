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
    pub faces: Vec<Face>,
    /// Families tried in order for body text and headings.
    pub body: Vec<Family>,
    /// Families tried in order for code.
    pub mono: Vec<Family>,
    /// Problems worth telling the user about, such as variable fonts.
    pub warnings: Vec<String>,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.faces.iter().map(|x| x.postscript_name.as_str()).collect();
        f.debug_struct("Fonts").field("faces", &names).finish()
    }
}

/// A bundled font file and its license notice.
pub struct Bundled {
    pub file: &'static str,
    pub data: &'static [u8],
}

pub const BUNDLED: [Bundled; 8] = [
    Bundled {
        file: "Alegreya-Regular.ttf",
        data: include_bytes!("../fonts/Alegreya-Regular.ttf"),
    },
    Bundled {
        file: "Alegreya-Bold.ttf",
        data: include_bytes!("../fonts/Alegreya-Bold.ttf"),
    },
    Bundled {
        file: "Alegreya-Italic.ttf",
        data: include_bytes!("../fonts/Alegreya-Italic.ttf"),
    },
    Bundled {
        file: "Alegreya-BoldItalic.ttf",
        data: include_bytes!("../fonts/Alegreya-BoldItalic.ttf"),
    },
    Bundled {
        file: "Cousine-Regular.ttf",
        data: include_bytes!("../fonts/Cousine-Regular.ttf"),
    },
    Bundled {
        file: "Cousine-Bold.ttf",
        data: include_bytes!("../fonts/Cousine-Bold.ttf"),
    },
    Bundled {
        file: "Cousine-Italic.ttf",
        data: include_bytes!("../fonts/Cousine-Italic.ttf"),
    },
    Bundled {
        file: "Cousine-BoldItalic.ttf",
        data: include_bytes!("../fonts/Cousine-BoldItalic.ttf"),
    },
];

/// License notices for the bundled fonts, as required by the SIL Open Font
/// License (shown by `sundowner --licenses`).
pub const FONT_LICENSES: [(&str, &str); 2] = [
    (
        "Alegreya (Regular, Bold, Italic, Bold Italic; static instances generated from the variable \
         fonts, see fonts/build.py)",
        include_str!("../fonts/OFL-Alegreya.txt"),
    ),
    (
        "Cousine (Regular, Bold, Italic, Bold Italic; unmodified)",
        include_str!("../fonts/OFL-Cousine.txt"),
    ),
];

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
    /// The bundled fonts are always the final fallback.
    pub fn load(spec: &FontSpec) -> Result<Fonts, String> {
        let mut faces: Vec<Face> = Vec::new();
        let bundled = |i: usize, faces: &mut Vec<Face>| -> Result<FaceId, String> {
            faces.push(
                Face::parse(Cow::Borrowed(BUNDLED[i].data), 0)
                    .map_err(|e| format!("{}: {e}", BUNDLED[i].file))?,
            );
            Ok(faces.len() - 1)
        };
        let serif = Family {
            regular: bundled(0, &mut faces)?,
            bold: Some(bundled(1, &mut faces)?),
            italic: Some(bundled(2, &mut faces)?),
            bold_italic: Some(bundled(3, &mut faces)?),
        };
        let mono = Family {
            regular: bundled(4, &mut faces)?,
            bold: Some(bundled(5, &mut faces)?),
            italic: Some(bundled(6, &mut faces)?),
            bold_italic: Some(bundled(7, &mut faces)?),
        };

        let mut warnings = Vec::new();
        let mut load_family = |f: &FamilySpec, what: &str, faces: &mut Vec<Face>| -> Result<Family, String> {
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
                    faces.push(face);
                    Ok(Some(faces.len() - 1))
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
            for f in [first].iter().chain(&fallbacks).chain(&[main_body, serif, mono]) {
                if !out.contains(f) {
                    out.push(*f);
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
        assert_eq!(f.faces.len(), 9);
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
        assert_eq!(f.body[1].regular, 8, "the fallback is next in line");
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
            face, 8,
            "no bold italic in the user family: its regular face, never a fake"
        );
        assert_eq!(
            f.body.last().unwrap().regular,
            4,
            "bundled fonts remain the last fallback"
        );
    }

    /// Every bundled face must cover Latin (Basic, Latin-1 and Extended-A),
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
        for (face, b) in f.faces.iter().zip(&BUNDLED) {
            let missing: String = wanted.iter().filter(|&&c| face.glyph(c).is_none()).collect();
            assert!(missing.is_empty(), "{} lacks {missing:?}", b.file);
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
