//! sundowner: a dependency-free Markdown to PDF converter.
#![forbid(unsafe_code)]

pub mod encoding;
pub mod flate;
pub mod image;
pub mod inline;
pub mod layout;
pub mod markdown;
mod metrics;
pub mod pdf;

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
    let pdf = pdf::write(&out, options.page_width, options.page_height, options.serif);
    Converted {
        pdf,
        warnings: out.warnings,
    }
}
