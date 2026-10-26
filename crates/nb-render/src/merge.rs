//! Concatenate PDFs page by page (the `source-pdf` export), in pure Rust.

use hayro_write::pdf_writer::{Pdf, Ref};
use hayro_write::{ChunkSettings, ExtractionQuery, extract};

use crate::{PdfError, Result, open};

/// Every page of every input, in order, as one PDF.
pub fn concat(inputs: &[Vec<u8>]) -> Result<Vec<u8>> {
    let mut out = Pdf::new();
    let mut next = Ref::new(1);
    let catalog = next.bump();
    let root = next.bump();
    let mut subtrees = Vec::new();
    let mut total = 0i32;
    let mut chunks = Vec::new();
    for (i, data) in inputs.iter().enumerate() {
        let src = open(data).map_err(|e| PdfError(format!("input {}: {e}", i + 1)))?;
        let n = src.pages().len();
        if n == 0 {
            continue;
        }
        let queries: Vec<_> = (0..n).map(ExtractionQuery::new_page).collect();
        let ex = extract(
            &src,
            Box::new(|| next.bump()),
            ChunkSettings::default(),
            &queries,
        )
        .map_err(|e| PdfError(format!("input {}: {e:?}", i + 1)))?;
        let kids: Vec<Ref> = ex
            .root_refs
            .iter()
            .map(|r| r.map_err(|e| PdfError(format!("input {}: {e:?}", i + 1))))
            .collect::<Result<_>>()?;
        total += kids.len() as i32;
        subtrees.push((ex.page_tree_parent_ref, kids));
        chunks.push(ex.chunk);
    }
    if total == 0 {
        return Err(PdfError("no pages to concatenate".into()));
    }
    out.catalog(catalog).pages(root);
    out.pages(root)
        .kids(subtrees.iter().map(|(r, _)| *r))
        .count(total);
    for (r, kids) in &subtrees {
        out.pages(*r)
            .parent(root)
            .count(kids.len() as i32)
            .kids(kids.iter().copied());
    }
    for c in &chunks {
        out.extend(c);
    }
    Ok(out.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::info::page_sizes;

    pub(crate) fn blank(sizes: &[(f32, f32)]) -> Vec<u8> {
        let mut pdf = Pdf::new();
        let mut next = Ref::new(1);
        let (cat, tree) = (next.bump(), next.bump());
        let pages: Vec<Ref> = sizes.iter().map(|_| next.bump()).collect();
        pdf.catalog(cat).pages(tree);
        pdf.pages(tree)
            .kids(pages.iter().copied())
            .count(pages.len() as i32);
        for (r, (w, h)) in pages.iter().zip(sizes) {
            pdf.page(*r)
                .media_box(hayro_write::pdf_writer::Rect::new(0.0, 0.0, *w, *h))
                .parent(tree);
        }
        pdf.finish()
    }

    #[test]
    fn concatenates_in_order() {
        let a = blank(&[(100.0, 200.0), (300.0, 400.0)]);
        let b = blank(&[(612.0, 792.0)]);
        let out = concat(&[a, b]).unwrap();
        assert_eq!(
            page_sizes(&out).unwrap(),
            vec![(100.0, 200.0), (300.0, 400.0), (612.0, 792.0)]
        );
    }
}
