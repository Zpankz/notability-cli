//! Pure-Rust PDF work (no native libraries): reading imported PDFs (page sizes, outline),
//! concatenating source PDFs, the local "Export as PDF" approximation and page rasterising.

pub mod compose;
pub mod info;
pub mod merge;
pub mod raster;
pub mod text;

pub use hayro_write::hayro_syntax;
pub use hayro_write::pdf_writer;

/// A PDF this crate could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfError(pub String);

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PdfError {}

pub type Result<T> = std::result::Result<T, PdfError>;

pub(crate) fn open(data: &[u8]) -> Result<hayro_syntax::Pdf> {
    hayro_syntax::Pdf::new(data.to_vec())
        .map_err(|e| PdfError(format!("not a readable PDF: {e:?}")))
}
