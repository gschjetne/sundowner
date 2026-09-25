//! `.sundowner` configuration files.
//!
//! ```text
//! # Page settings (same meaning as the command-line options)
//! paper = a4
//! font-size = 11
//! margin = 20
//! page-numbers = true
//!
//! # Replace the bundled Alegreya for body text and headings
//! [body]
//! regular = fonts/SourceSans3-Regular.ttf
//! bold = fonts/SourceSans3-Bold.ttf
//! italic = fonts/SourceSans3-Italic.ttf
//! bold-italic = fonts/SourceSans3-BoldItalic.ttf
//!
//! # Replace the bundled IBM Plex Mono for code
//! [mono]
//! regular = fonts/JetBrainsMono-Regular.ttf
//!
//! # Extra fonts for characters the fonts above lack; repeat the section
//! # for more. They are tried in order.
//! [fallback]
//! regular = fonts/NotoSansJP-Regular.ttf
//! bold = fonts/NotoSansJP-Bold.ttf
//!
//! [fallback]
//! regular = fonts/NotoEmoji-Regular.ttf
//! ```
//!
//! Font paths are relative to the configuration file. A font inside a
//! TrueType collection is selected with `#index`, e.g. `fonts/x.ttc#2`.

use crate::fonts::{FamilySpec, FontSpec};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = ".sundowner";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub paper: Option<(f32, f32)>,
    pub font_size: Option<f32>,
    /// Margin in millimetres.
    pub margin: Option<f32>,
    pub page_numbers: Option<bool>,
    pub fonts: FontSpec,
}

const MM: f32 = 72.0 / 25.4;

pub fn parse_number(what: &str, v: &str, lo: f32, hi: f32) -> Result<f32, String> {
    match v.trim().parse::<f32>() {
        Ok(x) if x.is_finite() && x >= lo && x <= hi => Ok(x),
        _ => Err(format!(
            "{what}: expected a number between {lo} and {hi}, got '{v}'"
        )),
    }
}

/// Paper size name or `WIDTHxHEIGHT` in millimetres, returned in points.
pub fn parse_paper(v: &str) -> Result<(f32, f32), String> {
    let (w, h) = match v.trim().to_ascii_lowercase().as_str() {
        "a4" => (210.0, 297.0),
        "a5" => (148.0, 210.0),
        "a3" => (297.0, 420.0),
        "letter" => (215.9, 279.4),
        "legal" => (215.9, 355.6),
        other => {
            let (w, h) = other
                .split_once('x')
                .ok_or_else(|| format!("paper: unknown size '{v}'"))?;
            (
                parse_number("paper", w, 50.0, 5000.0)?,
                parse_number("paper", h, 50.0, 5000.0)?,
            )
        }
    };
    Ok((w * MM, h * MM))
}

/// Find the configuration for a document: the nearest `.sundowner` file in
/// `dir` or any of its parents.
pub fn find(dir: &Path) -> Option<PathBuf> {
    let dir = dir.canonicalize().ok()?;
    dir.ancestors().map(|d| d.join(FILE_NAME)).find(|p| p.is_file())
}

pub fn load(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    parse(&text, base).map_err(|e| format!("{}: {e}", path.display()))
}

enum Section {
    Top,
    Body,
    Mono,
    Fallback,
}

pub fn parse(text: &str, base: &Path) -> Result<Config, String> {
    let mut cfg = Config::default();
    let mut section = Section::Top;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let err = |m: String| format!("line {}: {m}", n + 1);
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = match name.trim() {
                "body" => {
                    if cfg.fonts.body.is_some() {
                        return Err(err("[body] given twice".into()));
                    }
                    cfg.fonts.body = Some(FamilySpec::default());
                    Section::Body
                }
                "mono" => {
                    if cfg.fonts.mono.is_some() {
                        return Err(err("[mono] given twice".into()));
                    }
                    cfg.fonts.mono = Some(FamilySpec::default());
                    Section::Mono
                }
                "fallback" => {
                    cfg.fonts.fallbacks.push(FamilySpec::default());
                    Section::Fallback
                }
                other => return Err(err(format!("unknown section [{other}]"))),
            };
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err("expected 'key = value'".into()))?;
        let key = key.trim();
        let mut value = value.trim();
        if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            value = &value[1..value.len() - 1];
        }
        match section {
            Section::Top => match key {
                "paper" => cfg.paper = Some(parse_paper(value).map_err(err)?),
                "font-size" => {
                    cfg.font_size = Some(parse_number("font-size", value, 4.0, 72.0).map_err(err)?)
                }
                "margin" => cfg.margin = Some(parse_number("margin", value, 0.0, 100.0).map_err(err)?),
                "page-numbers" => {
                    cfg.page_numbers = Some(match value {
                        "true" | "yes" | "on" => true,
                        "false" | "no" | "off" => false,
                        _ => {
                            return Err(err(format!(
                                "page-numbers: expected true or false, got '{value}'"
                            )))
                        }
                    })
                }
                _ => return Err(err(format!("unknown setting '{key}'"))),
            },
            Section::Body | Section::Mono | Section::Fallback => {
                let fam = match section {
                    Section::Body => cfg.fonts.body.as_mut(),
                    Section::Mono => cfg.fonts.mono.as_mut(),
                    _ => cfg.fonts.fallbacks.last_mut(),
                }
                .ok_or_else(|| err("font outside a section".into()))?;
                let slot = match key {
                    "regular" => &mut fam.regular,
                    "bold" => &mut fam.bold,
                    "italic" => &mut fam.italic,
                    "bold-italic" => &mut fam.bold_italic,
                    _ => {
                        return Err(err(format!(
                            "unknown font style '{key}' (use regular, bold, italic, bold-italic)"
                        )))
                    }
                };
                if value.is_empty() {
                    return Err(err(format!("{key}: missing font path")));
                }
                *slot = Some(font_path(base, value).map_err(err)?);
            }
        }
    }
    for (i, f) in cfg
        .fonts
        .body
        .iter()
        .chain(&cfg.fonts.mono)
        .chain(&cfg.fonts.fallbacks)
        .enumerate()
    {
        if f.regular.is_none() {
            return Err(format!("font section {} has no 'regular' font", i + 1));
        }
    }
    Ok(cfg)
}

fn font_path(base: &Path, value: &str) -> Result<(PathBuf, u32), String> {
    let (path, index) = match value.rsplit_once('#') {
        Some((p, i)) if !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit()) => {
            (p, i.parse().map_err(|_| format!("bad font index '{i}'"))?)
        }
        _ => (value, 0),
    };
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    Ok((path, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let c = parse(
            "# comment\npaper = letter\nfont-size = 12\nmargin = 25\npage-numbers = off\n\n\
             [body]\nregular = \"a b.ttf\"\nbold = b.ttf\n[mono]\nregular = /abs/m.ttf\n\
             [fallback]\nregular = cjk.ttc#2\n[fallback]\nregular = emoji.ttf\n",
            Path::new("/cfg"),
        )
        .unwrap();
        assert_eq!(c.font_size, Some(12.0));
        assert_eq!(c.margin, Some(25.0));
        assert_eq!(c.page_numbers, Some(false));
        assert!(c.paper.is_some());
        let body = c.fonts.body.unwrap();
        assert_eq!(body.regular, Some((PathBuf::from("/cfg/a b.ttf"), 0)));
        assert_eq!(body.bold, Some((PathBuf::from("/cfg/b.ttf"), 0)));
        assert_eq!(
            c.fonts.mono.unwrap().regular,
            Some((PathBuf::from("/abs/m.ttf"), 0))
        );
        assert_eq!(c.fonts.fallbacks.len(), 2);
        assert_eq!(
            c.fonts.fallbacks[0].regular,
            Some((PathBuf::from("/cfg/cjk.ttc"), 2))
        );
    }

    #[test]
    fn reports_errors_with_line_numbers() {
        for (src, msg) in [
            ("bogus = 1", "line 1: unknown setting"),
            ("\n[weird]", "line 2: unknown section"),
            ("[body]\nfancy = x.ttf", "line 2: unknown font style"),
            ("font-size = 1000", "line 1: font-size"),
            ("[fallback]\nbold = x.ttf", "no 'regular'"),
            ("just text", "expected 'key = value'"),
        ] {
            let e = parse(src, Path::new(".")).unwrap_err();
            assert!(e.contains(msg), "{src:?}: {e}");
        }
    }
}
