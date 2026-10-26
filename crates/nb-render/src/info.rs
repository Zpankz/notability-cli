//! Facts about an imported PDF: page sizes (what `CreatePage.cropBoxes` records) and its outline.

use std::collections::HashMap;

use hayro_write::hayro_syntax::object::{Array, Dict, MaybeRef, Object, ObjectIdentifier};

use crate::{Result, open};

/// `(width, height)` of every page in points: the crop box (clipped to the media box), swapped
/// for 90°/270° rotation, as the web importer records it.
pub fn page_sizes(data: &[u8]) -> Result<Vec<(f64, f64)>> {
    let pdf = open(data)?;
    Ok(pdf
        .pages()
        .iter()
        .map(|p| {
            let (w, h) = p.render_dimensions();
            (round2(w), round2(h))
        })
        .collect())
}

fn round2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

/// One outline (bookmark) entry; `page` is 1-based, `None` when the destination is unresolved.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub level: usize,
    pub title: String,
    pub page: Option<usize>,
}

/// The document outline, depth first.
pub fn outline(data: &[u8]) -> Result<Vec<OutlineItem>> {
    let pdf = open(data)?;
    let xref = pdf.xref();
    let mut page_of: HashMap<ObjectIdentifier, usize> = HashMap::new();
    for (i, p) in pdf.pages().iter().enumerate() {
        if let Some(id) = p.raw().obj_id() {
            page_of.insert(id, i + 1);
        }
    }
    let Some(catalog) = xref.get::<Dict<'_>>(xref.root_id()) else {
        return Ok(Vec::new());
    };
    let names = catalog
        .get::<Dict<'_>>("Names")
        .and_then(|n| n.get::<Dict<'_>>("Dests"));
    let old_dests = catalog.get::<Dict<'_>>("Dests");
    let resolve = |d: Object<'_>| -> Option<usize> {
        dest_page(d, &page_of, names.as_ref(), old_dests.as_ref(), 0)
    };
    let mut out = Vec::new();
    let Some(root) = catalog.get::<Dict<'_>>("Outlines") else {
        return Ok(out);
    };
    let mut stack: Vec<(Dict<'_>, usize)> = Vec::new();
    if let Some(first) = root.get::<Dict<'_>>("First") {
        stack.push((first, 1));
    }
    let mut guard = 0usize;
    while let Some((item, level)) = stack.pop() {
        guard += 1;
        if guard > 100_000 {
            break;
        }
        let title = item
            .get::<hayro_write::hayro_syntax::object::String<'_>>("Title")
            .map(|s| decode_text(s.as_bytes()))
            .unwrap_or_default();
        let page = item
            .get::<Object<'_>>("Dest")
            .and_then(resolve)
            .or_else(|| {
                item.get::<Dict<'_>>("A")
                    .and_then(|a| a.get::<Object<'_>>("D"))
                    .and_then(resolve)
            });
        out.push(OutlineItem {
            level,
            title: title.trim().to_string(),
            page,
        });
        // Visit order: children before the next sibling (push sibling first).
        if let Some(next) = item.get::<Dict<'_>>("Next") {
            stack.push((next, level));
        }
        if let Some(first) = item.get::<Dict<'_>>("First") {
            stack.push((first, level + 1));
        }
    }
    Ok(out)
}

fn dest_page(
    d: Object<'_>,
    page_of: &HashMap<ObjectIdentifier, usize>,
    names: Option<&Dict<'_>>,
    old: Option<&Dict<'_>>,
    depth: usize,
) -> Option<usize> {
    if depth > 4 {
        return None;
    }
    match d {
        Object::Array(a) => first_page_ref(&a, page_of),
        Object::Dict(dict) => dict
            .get::<Object<'_>>("D")
            .and_then(|x| dest_page(x, page_of, names, old, depth + 1)),
        Object::String(s) => {
            let key = s.as_bytes().to_vec();
            names
                .and_then(|n| name_tree_lookup(n, &key, 0))
                .and_then(|x| dest_page(x, page_of, names, old, depth + 1))
        }
        Object::Name(n) => old
            .and_then(|o| o.get::<Object<'_>>(n.as_ref()))
            .and_then(|x| dest_page(x, page_of, names, old, depth + 1)),
        _ => None,
    }
}

fn first_page_ref(a: &Array<'_>, page_of: &HashMap<ObjectIdentifier, usize>) -> Option<usize> {
    match a.raw_iter().next()? {
        MaybeRef::Ref(r) => page_of.get(&ObjectIdentifier::from(r)).copied(),
        // Some writers store a 0-based page number for remote destinations.
        MaybeRef::NotRef(Object::Number(n)) => usize::try_from(n.as_i64()).ok().map(|i| i + 1),
        MaybeRef::NotRef(_) => None,
    }
}

fn name_tree_lookup<'a>(node: &Dict<'a>, key: &[u8], depth: usize) -> Option<Object<'a>> {
    if depth > 32 {
        return None;
    }
    if let Some(names) = node.get::<Array<'a>>("Names") {
        let mut it = names.iter::<Object<'a>>();
        while let (Some(k), Some(v)) = (it.next(), it.next()) {
            if let Object::String(s) = k
                && s.as_bytes() == key
            {
                return Some(v);
            }
        }
    }
    if let Some(kids) = node.get::<Array<'a>>("Kids") {
        for kid in kids.iter::<Dict<'a>>() {
            if let Some(v) = name_tree_lookup(&kid, key, depth + 1) {
                return Some(v);
            }
        }
    }
    None
}

/// PDF text strings: UTF-16BE with a BOM, UTF-8 with a BOM, else PDFDocEncoding (≈ Latin-1).
pub fn decode_text(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = b[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    b.iter().map(|&c| char::from(c)).collect()
}
