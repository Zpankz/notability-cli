use std::collections::HashMap;
use std::path::PathBuf;

use incurs::schema::{FieldMeta, FieldType};
use nb_api::{NbError, Result};
use nb_domain::export::{fmt_ts, note_details, safe_name, summary_text, to_markdown, to_text};
use nb_domain::state::NoteState;
use nb_render::compose;
use serde_json::{Value, json};

use crate::handler;
use crate::spec::*;

const FORMATS: [&str; 8] = [
    "md",
    "txt",
    "json",
    "transcript",
    "pdf",
    "ntb",
    "source-pdf",
    "assets",
];

pub fn specs() -> Vec<Spec> {
    vec![
        Spec::new(
            "files export",
            Effect::R,
            "Export notes: md, txt, json, transcript, pdf (local render), ntb, source-pdf, assets, thumbnail",
            export,
        )
        .arg(req_multi(
            "notes",
            "Note refs (id, id prefix, title or folder/path/title).",
        ))
        .opt(cli_name(
            FieldMeta {
                field_type: FieldType::Array(Box::new(FieldType::Enum(
                    FORMATS.iter().map(|f| f.to_string()).collect(),
                ))),
                default: Some(json!(["md"])),
                ..text(
                    "format",
                    "Repeatable. pdf = local approximation of Export as PDF; ntb = Notability archive; source-pdf = the imported PDFs merged; assets = every attachment.",
                )
            },
            "to",
        ))
        .opt(alias(text_def("out", ".", "Output directory (CLI)."), 'o'))
        .opt(flag("thumbnail", "Also save the PNG thumbnail (and embed it in .ntb)."))
        .opt(flag_on(
            "label",
            "pdf: print the small 'Local render' footer on each page (--no-label to omit).",
        ))
        .opt(flag(
            "stdout",
            "Return md/txt/json/transcript inline instead of writing files (always on over MCP). pdf/ntb/source-pdf/assets are always written to --out.",
        )),
        Spec::new(
            "files render",
            Effect::R,
            "Render one page of a note to PNG (local render: source PDF or paper, ink, shapes, text, images)",
            render,
        )
        .image_result()
        .arg(req("note", "Note ref."))
        .opt(num_def("page", 1.0, "Page number (1-based)."))
        .opt(num_def("scale", 1.5, "Pixels per point (1 = 72 dpi)."))
        .opt(flag_on("label", "Print the 'Local render' footer (--no-label to omit)."))
        .opt(alias(
            text("out", "PNG path (CLI; default <title>.p<N>.png). Over MCP the image is returned inline."),
            'o',
        )),
        Spec::new(
            "files toc",
            Effect::R,
            "Table of contents: pages (with their source PDF page) and each imported PDF's outline",
            toc,
        )
        .arg(req("note", "Note ref.")),
    ]
}

async fn note_state(cl: &nb_domain::client::Client, id: &str) -> Result<NoteState> {
    let b = cl.http.note_bundle(id).await?;
    Ok(NoteState::new(&b))
}

/// The local PDF render of a note, fetching the assets it needs (missing ones are reported).
async fn local_pdf(
    cl: &nb_domain::client::Client,
    id: &str,
    st: &NoteState,
    label: bool,
) -> Result<(Vec<u8>, Value)> {
    let mut assets = HashMap::new();
    for sha in compose::needed_assets(st) {
        if let Ok((data, _)) = cl.http.download_asset(id, &sha).await {
            assets.insert(sha, data);
        }
    }
    compose::render(st, &assets, label).map_err(|e| NbError::generic(format!("render: {e}")))
}

/// Transcript text, one segment per line with `[mm:ss]` stamps.
pub fn transcript_text(trs: &[Value], timestamps: bool) -> String {
    let mut out = Vec::new();
    for (i, t) in trs.iter().enumerate() {
        out.push(format!(
            "— recording {} ({})",
            i + 1,
            t["status"].as_str().unwrap_or("unknown")
        ));
        for s in t["segments"].as_array().into_iter().flatten() {
            let pre = if timestamps {
                format!("[{}] ", fmt_ts(s["startTime"].as_f64()))
            } else {
                String::new()
            };
            out.push(format!(
                "{pre}{}",
                s["text"].as_str().unwrap_or_default().trim()
            ));
        }
    }
    out.join("\n")
}

pub fn summaries_text(sums: &[Value]) -> String {
    sums.iter()
        .map(|s| {
            format!(
                "— {}\n{}",
                s["kind"].as_str().unwrap_or_default(),
                summary_text(&s["content"])
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn write(path: PathBuf, data: &[u8], written: &mut Vec<String>) -> Result<()> {
    std::fs::write(&path, data)
        .map_err(|e| NbError::usage(format!("cannot write {}: {e}", path.display())))?;
    written.push(path.display().to_string());
    Ok(())
}

handler!(export, |c| {
    let cl = c.client()?;
    let formats = c.list("format");
    let formats: Vec<&str> = if formats.is_empty() {
        vec!["md"]
    } else {
        formats.iter().map(String::as_str).collect()
    };
    let inline = c.mcp || c.b("stdout");
    let out_dir = PathBuf::from(c.os("out").unwrap_or_else(|| ".".into()));
    let mut results = Vec::new();
    for r in c.list("notes") {
        let want_tr = formats
            .iter()
            .any(|f| matches!(*f, "transcript" | "json" | "md" | "txt"));
        let d = note_details(&cl, &r, want_tr, true).await?;
        let id = d["id"].as_str().unwrap_or_default().to_string();
        let title = d["title"].as_str().unwrap_or_default().to_string();
        let base = safe_name(&title, &id[..id.len().min(8)]);
        let mut written = Vec::new();
        let mut inline_out = serde_json::Map::new();
        if !inline {
            std::fs::create_dir_all(&out_dir)
                .map_err(|e| NbError::usage(format!("cannot create {}: {e}", out_dir.display())))?;
        }
        for f in &formats {
            let (ext, body) = match *f {
                "md" => ("md", to_markdown(&d)),
                "txt" => ("txt", to_text(&d)),
                "json" => ("json", serde_json::to_string_pretty(&d).unwrap_or_default()),
                "transcript" => (
                    "transcript.txt",
                    transcript_text(
                        d["transcripts"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                        true,
                    ),
                ),
                _ => continue,
            };
            if inline {
                inline_out.insert(
                    (*f).to_string(),
                    if *f == "json" { d.clone() } else { json!(body) },
                );
            } else {
                write(
                    out_dir.join(format!("{base}.{ext}")),
                    body.as_bytes(),
                    &mut written,
                )?;
            }
        }
        let mut row = json!({"id": id, "title": title});
        let mk = |d: &PathBuf| {
            std::fs::create_dir_all(d)
                .map_err(|e| NbError::usage(format!("cannot create {}: {e}", d.display())))
        };
        if formats.contains(&"pdf") {
            mk(&out_dir)?;
            let st = note_state(&cl, &id).await?;
            let (pdf, stats) = local_pdf(&cl, &id, &st, c.b("label")).await?;
            write(out_dir.join(format!("{base}.pdf")), &pdf, &mut written)?;
            row["pdf"] = stats;
        }
        if formats.contains(&"ntb") {
            mk(&out_dir)?;
            let (data, info) = nb_domain::media::export_ntb(&cl, &id, c.b("thumbnail")).await?;
            write(out_dir.join(format!("{base}.ntb")), &data, &mut written)?;
            row["ntb"] = json!({"bytes": data.len(), "ops": info["ops"], "assets": info["assets"], "thumbnail": info["thumbnail"]});
        }
        if formats.contains(&"source-pdf") {
            let raw = cl.http.note_bundle(&id).await?;
            let ops = raw["ops"].as_array().cloned().unwrap_or_default();
            let mut pdfs = Vec::new();
            for h in nb_domain::media::pdf_hashes(&ops) {
                pdfs.push(cl.http.download_asset(&id, &h).await?.0);
            }
            if pdfs.is_empty() {
                row["source_pdf"] = json!({"pdfs": 0, "note": "the note has no imported PDF"});
            } else {
                let merged = nb_render::merge::concat(&pdfs)
                    .map_err(|e| NbError::generic(format!("merge: {e}")))?;
                mk(&out_dir)?;
                write(
                    out_dir.join(format!("{base}.source.pdf")),
                    &merged,
                    &mut written,
                )?;
                let pages = nb_render::info::page_sizes(&merged).map_or(0, |p| p.len());
                row["source_pdf"] =
                    json!({"pdfs": pdfs.len(), "pages": pages, "bytes": merged.len()});
            }
        }
        let all_assets = formats.contains(&"assets");
        let mut assets = Vec::new();
        if all_assets {
            let adir = out_dir.join(format!("{base}_assets"));
            for h in d["assets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let short = &h[..h.len().min(16)];
                let (data, ext) = match cl.http.download_asset(&id, h).await {
                    Ok(x) => x,
                    Err(e) => {
                        assets.push(json!({"hash": short, "error": e.message}));
                        continue;
                    }
                };
                let mut a = json!({"hash": short, "type": ext, "bytes": data.len()});
                mk(&adir)?;
                let p = adir.join(format!("{short}.{ext}"));
                write(p.clone(), &data, &mut written)?;
                a["path"] = json!(p.display().to_string());
                assets.push(a);
            }
        }
        if all_assets {
            row["assets"] = json!(assets);
        }
        if c.b("thumbnail") {
            let n = cl.resolve_note(&id).await?;
            let png = cl.http.thumbnail(&id, n.thumbnail_url.as_deref()).await?;
            if inline {
                row["thumbnail_bytes"] = json!(png.len());
            } else {
                write(
                    out_dir.join(format!("{base}.thumbnail.png")),
                    &png,
                    &mut written,
                )?;
            }
        }
        if inline {
            row["content"] = Value::Object(inline_out);
        }
        if !written.is_empty() || !inline {
            row["files"] = json!(written);
        }
        results.push(row);
    }
    Ok(json!({"exports": results}))
});

handler!(render, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let st = note_state(&cl, &n.id).await?;
    let page = c.u("page")?.max(1);
    let scale = c.f("scale")?.clamp(0.1, 8.0) as f32;
    let (pdf, stats) = local_pdf(&cl, &n.id, &st, c.b("label")).await?;
    let total = stats["pages"].as_u64().unwrap_or(0) as usize;
    if page > total {
        return Err(NbError::usage(format!(
            "page {page} out of range ({total} pages)"
        )));
    }
    let (png, w, h) = nb_render::raster::page_png(&pdf, page - 1, scale)
        .map_err(|e| NbError::generic(format!("raster: {e}")))?;
    let mut out = json!({"id": n.id, "title": n.title, "page": page, "pages": total,
                         "width": w, "height": h, "bytes": png.len(), "render": stats});
    if c.mcp {
        use base64::Engine;
        out["image"] = json!({"data": base64::engine::general_purpose::STANDARD.encode(&png),
                              "mimeType": "image/png"});
    } else {
        let base = safe_name(&n.title, &n.id[..n.id.len().min(8)]);
        let path = PathBuf::from(c.os("out").unwrap_or_else(|| format!("{base}.p{page}.png")));
        if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(d)
                .map_err(|e| NbError::usage(format!("cannot create {}: {e}", d.display())))?;
        }
        let mut written = Vec::new();
        write(path, &png, &mut written)?;
        out["path"] = json!(written[0]);
    }
    Ok(out)
});

handler!(toc, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let st = note_state(&cl, &n.id).await?;
    let note_bg = st.note_background();
    let mut docs: Vec<String> = Vec::new();
    let mut pages = Vec::new();
    for p in st.pages() {
        let bg = p.background.clone().unwrap_or_else(|| note_bg.clone());
        let pdf = &bg["pdf"];
        let (w, h) = st.page_size(&p);
        let mut row = json!({"page": p.index, "id": nb_domain::ids::fmt_seq(p.id),
                             "width": w, "height": h, "kind": "paper"});
        if let Some(sha) = pdf["metadata"]["assetHash"].as_str() {
            if !docs.iter().any(|d| d == sha) {
                docs.push(sha.to_string());
            }
            let doc = docs.iter().position(|d| d == sha).unwrap_or(0) + 1;
            row["kind"] = json!("pdf");
            row["pdf"] = json!({"doc": doc, "file": pdf["metadata"]["fileName"],
                                "page": pdf["pageOffset"].as_u64().unwrap_or(0) + p.id.2 + 1});
        }
        pages.push(row);
    }
    let mut outlines = Vec::new();
    for (i, sha) in docs.iter().enumerate() {
        let mut d = json!({"doc": i + 1, "hash": &sha[..sha.len().min(16)]});
        match cl.http.download_asset(&n.id, sha).await {
            Ok((data, _)) => match nb_render::info::outline(&data) {
                Ok(items) => {
                    let first_note_page = |src: usize| {
                        pages
                            .iter()
                            .find(|p| {
                                p["pdf"]["doc"] == json!(i + 1) && p["pdf"]["page"] == json!(src)
                            })
                            .map(|p| p["page"].clone())
                    };
                    d["outline"] = json!(
                        items
                            .iter()
                            .map(|o| json!({
                        "level": o.level, "title": o.title, "pdf_page": o.page,
                        "note_page": o.page.and_then(first_note_page)}))
                            .collect::<Vec<_>>()
                    );
                }
                Err(e) => d["error"] = json!(e.0),
            },
            Err(e) => d["error"] = json!(e.message),
        }
        outlines.push(d);
    }
    Ok(json!({"id": n.id, "title": n.title, "pages": pages, "pdfs": outlines}))
});
