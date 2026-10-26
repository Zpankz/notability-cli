//! Rasterise one PDF page to PNG (previews, `files render`, MCP image content).

use hayro::vello_cpu::color::palette::css::WHITE;

use crate::{PdfError, Result, open};

/// PNG bytes of page `index` (0-based) at `scale` × 72 dpi, plus its pixel size.
pub fn page_png(data: &[u8], index: usize, scale: f32) -> Result<(Vec<u8>, u32, u32)> {
    let pdf = open(data)?;
    let pages = pdf.pages();
    let page = pages.get(index).ok_or_else(|| {
        PdfError(format!(
            "page {} out of range ({} pages)",
            index + 1,
            pages.len()
        ))
    })?;
    let pix = hayro::render(
        page,
        &hayro::RenderCache::new(),
        &hayro::hayro_interpret::InterpreterSettings::default(),
        &hayro::RenderSettings::default(),
        &hayro::PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let (w, h) = (u32::from(pix.width()), u32::from(pix.height()));
    let png = pix
        .into_png()
        .map_err(|e| PdfError(format!("png encode: {e:?}")))?;
    Ok((png, w, h))
}
