//! sundowner: a dependency-free Markdown to PDF converter.
#![forbid(unsafe_code)]

pub mod chars;
pub mod config;
pub mod flate;
pub mod fonts;
pub mod image;
pub mod inline;
pub mod kern;
pub mod layout;
pub mod markdown;
pub mod pdf;
pub mod ttf;

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
    Converted {
        pdf,
        warnings: out.warnings,
    }
}
