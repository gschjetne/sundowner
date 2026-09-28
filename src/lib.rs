//! sundowner: a dependency-free Markdown to PDF converter.
#![forbid(unsafe_code)]

pub mod arabic;
pub mod bidi;
pub mod bidi_table;
pub mod chars;
pub mod config;
pub mod flate;
pub mod fonts;
pub mod front_matter;
pub mod gpos;
pub mod gsub;
pub mod hyphenate;
pub mod image;
pub mod inline;
pub mod kern;
pub mod layout;
pub mod linebreak;
pub mod linebreak_table;
pub mod markdown;
pub mod normalize;
pub mod normalize_table;
pub mod otl;
pub mod pdf;
pub mod pdfa;
pub mod position;
pub mod tags;
pub mod ttf;
pub mod yaml;

pub use layout::Options;

/// Result of a conversion: the PDF bytes plus non-fatal warnings (for example
/// images that could not be loaded).
pub struct Converted {
    pub pdf: Vec<u8>,
    pub warnings: Vec<String>,
}

/// Convert Markdown source to a PDF document.
pub fn convert(markdown: &str, options: &Options) -> Converted {
    let doc = markdown::parse(markdown);
    let out = layout::layout(&doc, options);
    let pdf = pdf::write(&out, options.page_width, options.page_height);
    let archival = pdfa::conformance(&out);
    let mut warnings = out.warnings;
    if let Err(why) = archival {
        warnings.push(format!("the PDF is not PDF/A, as {why}"));
    }
    Converted { pdf, warnings }
}
