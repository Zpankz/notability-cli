//! Imports and uploads: PDFs, images, audio recordings, `.ntb` archives and YouTube
//! transcripts, plus the `.ntb` / asset side of exports.
//!
//! Every asset goes through `prepareMediaUpload` → a cookie-free signed GCS `PUT` (skipped
//! when the server already has the bytes) and is then referenced by its sha512 in ops; the
//! server appends `AssetCloudPersisted` (site 65535) once storage has it.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::time::Duration;

use nb_api::{NbError, Result};
use serde_json::{Value, json};

use crate::client::{Client, now_ms};
use crate::ids::{fmt_id, id_of, seqd};
use crate::session::{EditSession, SERVER_ONLY_OPS, color_value, initial_ops, rebase_ops};
use crate::state::{BLOCK_IMAGE, NoteState};

pub const PDF_MIME: &str = "application/pdf";
pub const IMAGE_EXTS: [(&str, &str); 8] = [
    (".png", "image/png"),
    (".jpg", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".gif", "image/gif"),
    (".webp", "image/webp"),
    (".avif", "image/avif"),
    (".heic", "image/heic"),
    (".heif", "image/heif"),
];
/// Recording formats Notability stores natively (anything else is converted with ffmpeg).
pub const AUDIO_NATIVE: [(&str, &str); 4] = [
    (".m4a", "audio/x-m4a"),
    (".mp4", "audio/mp4"),
    (".webm", "audio/webm"),
    (".weba", "audio/webm"),
];
/// Extensions `files import media` treats as audio.
pub const AUDIO_EXTS: [&str; 10] = [
    ".mp3", ".m4a", ".wav", ".aac", ".webm", ".weba", ".ogg", ".flac", ".opus", ".mp4",
];
/// Video containers the web importer rejects.
pub const VIDEO_EXTS: [&str; 5] = [".mp4v", ".mov", ".avi", ".mkv", ".m4v"];
/// How long uploads wait for the server's `AssetCloudPersisted`.
pub const PERSIST_WAIT: Duration = Duration::from_secs(30);

pub fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

pub fn stem_of(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map_or_else(|| name.to_string(), |s| s.to_string_lossy().into_owned())
}

/// File bytes and base name.
pub fn read_file(path: &str) -> Result<(Vec<u8>, String)> {
    let p = Path::new(path);
    if !p.is_file() {
        return Err(NbError::usage(format!("no such file: {path}")));
    }
    let data = std::fs::read(p).map_err(|e| NbError::usage(format!("cannot read {path}: {e}")))?;
    let name = p
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
    Ok((data, name))
}

pub fn image_mime(name: &str) -> &'static str {
    let e = ext_of(name);
    IMAGE_EXTS
        .iter()
        .find(|(x, _)| *x == e)
        .map_or("application/octet-stream", |(_, m)| m)
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?])))
}

fn le_n(b: &[u8], i: usize, n: usize) -> Option<u32> {
    let s = b.get(i..i + n)?;
    Some(s.iter().rev().fold(0u32, |a, &x| (a << 8) | u32::from(x)))
}

/// Pixel size from the file header (PNG, GIF, JPEG, WebP).
pub fn image_size(d: &[u8]) -> Result<(u32, u32)> {
    let bad = || NbError::usage("unsupported image format (png, jpeg, gif, webp)");
    if d.starts_with(b"\x89PNG\r\n\x1a\n") && d.len() >= 24 {
        let w = u32::from_be_bytes([d[16], d[17], d[18], d[19]]);
        let h = u32::from_be_bytes([d[20], d[21], d[22], d[23]]);
        return Ok((w, h));
    }
    if (d.starts_with(b"GIF87a") || d.starts_with(b"GIF89a")) && d.len() >= 10 {
        return Ok((
            le_n(d, 6, 2).ok_or_else(bad)?,
            le_n(d, 8, 2).ok_or_else(bad)?,
        ));
    }
    if d.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 1 < d.len() {
            if d[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = d[i + 1];
            if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                let h = be16(d, i + 5).ok_or_else(bad)?;
                let w = be16(d, i + 7).ok_or_else(bad)?;
                return Ok((w, h));
            }
            if m == 0xFF {
                i += 1;
                continue;
            }
            i += 2 + be16(d, i + 2).ok_or_else(bad)? as usize;
        }
        return Err(bad());
    }
    if d.len() >= 30 && &d[0..4] == b"RIFF" && &d[8..12] == b"WEBP" {
        match &d[12..16] {
            b"VP8X" => {
                return Ok((
                    1 + le_n(d, 24, 3).ok_or_else(bad)?,
                    1 + le_n(d, 27, 3).ok_or_else(bad)?,
                ));
            }
            b"VP8 " => {
                return Ok((
                    le_n(d, 26, 2).ok_or_else(bad)? & 0x3FFF,
                    le_n(d, 28, 2).ok_or_else(bad)? & 0x3FFF,
                ));
            }
            b"VP8L" => {
                let b = le_n(d, 21, 4).ok_or_else(bad)?;
                return Ok(((b & 0x3FFF) + 1, ((b >> 14) & 0x3FFF) + 1));
            }
            _ => {}
        }
    }
    Err(bad())
}

fn asset_meta(up: &Value, name: &str, mime: &str) -> Value {
    json!({"assetHash": up["sha512"], "fileName": name, "mimeType": mime, "fileSize": up["size"]})
}

/// Poll the note until the server has appended `AssetCloudPersisted` for `sha`.
pub async fn wait_persisted(
    client: &Client,
    note_id: &str,
    sha: &str,
    wait: Duration,
) -> Result<bool> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        let b = client.http.note_bundle(note_id).await?;
        let hit = b["ops"].as_array().into_iter().flatten().any(|o| {
            o["payload"]["type"] == "AssetCloudPersisted"
                && o["payload"]["value"]["assetHash"] == sha
        });
        if hit {
            return Ok(true);
        }
        if std::time::Instant::now() > deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn persisted(client: &Client, note_id: &str, up: &Value, wait: bool) -> Result<Value> {
    if !wait {
        return Ok(Value::Null);
    }
    let sha = up["sha512"].as_str().unwrap_or_default();
    Ok(json!(
        wait_persisted(client, note_id, sha, PERSIST_WAIT).await?
    ))
}

fn summary(acks: &[Value]) -> Value {
    let dups = acks
        .iter()
        .filter(|a| a["duplicate"].as_bool().unwrap_or(false))
        .count();
    json!({"acks": acks.len(), "duplicates": dups})
}

// ------------------------------------------------------------------ PDFs
fn pdf_page_value(up: &Value, name: &str, sizes: &[(f64, f64)]) -> Value {
    let crops: Vec<Value> = sizes
        .iter()
        .map(|(w, h)| json!({"width": w, "height": h}))
        .collect();
    let (w, h) = sizes.first().copied().unwrap_or((612.0, 792.0));
    json!({"background": {"pdf": {"metadata": asset_meta(up, name, PDF_MIME),
                                  "totalPageCount": sizes.len(), "pagesConsumed": sizes.len(),
                                  "cropBoxes": crops},
                          "size": {"width": w, "height": h}},
           "pageCount": sizes.len()})
}

fn check_pdf(data: &[u8], name: &str, sizes: &[(f64, f64)]) -> Result<()> {
    if !data.starts_with(b"%PDF") {
        return Err(NbError::usage(format!("{name} is not a PDF")));
    }
    if sizes.is_empty() {
        return Err(NbError::usage(format!("{name} has no pages")));
    }
    Ok(())
}

/// Home → Import a PDF: `createNote` + `SetMetadata` and one `CreatePage` holding every page
/// (the web importer writes no blank-note seed). `sizes` come from `nb_render::info::page_sizes`.
pub async fn import_pdf_new(
    client: &Client,
    data: &[u8],
    name: &str,
    sizes: &[(f64, f64)],
    title: Option<&str>,
    folder: Option<&str>,
    wait: bool,
) -> Result<Value> {
    check_pdf(data, name, sizes)?;
    let up = client.http.upload_bytes(data).await?;
    let title = title.map_or_else(|| name.to_string(), str::to_string);
    let now = now_ms();
    let ops = vec![
        json!({"id": {"site": 0, "timestamp": 0}, "clientTime": now, "payload": {"type": "SetMetadata", "value": {
            "title": {"value": title},
            "pageBackground": {"value": {"margins": {"top": 36.0, "bottom": 36.0, "left": 36.0, "right": 36.0}}},
            "layoutMode": 1}}}),
        json!({"id": {"site": 0, "timestamp": 1}, "clientTime": now,
               "payload": {"type": "CreatePage", "value": pdf_page_value(&up, name, sizes)}}),
    ];
    let note = client.create_note(&title, folder, ops).await?;
    let id = note["id"].as_str().unwrap_or_default().to_string();
    Ok(
        json!({"note_id": id, "title": title, "created_note": true, "pages": sizes.len(),
              "asset": up, "append": note["append"], "move": note.get("move"),
              "cloud_persisted": persisted(client, &id, &up, wait).await?}),
    )
}

/// Insert a PDF's pages after page `after` (1-based; 0 = at the start; default: at the end).
pub async fn import_pdf_into(
    client: &Client,
    note_id: &str,
    data: &[u8],
    name: &str,
    sizes: &[(f64, f64)],
    after: Option<usize>,
    wait: bool,
) -> Result<Value> {
    check_pdf(data, name, sizes)?;
    let mut s = EditSession::open(client, note_id).await?;
    let pages = s.state.pages();
    let idx = after.map_or(pages.len(), |a| a.min(pages.len()));
    let up = client.http.upload_bytes(data).await?;
    let mut value = pdf_page_value(&up, name, sizes);
    if idx > 0 {
        value["location"] = seqd(pages[idx - 1].node);
    }
    let o = s.op("CreatePage", value);
    let acks = s.append(vec![o]).await?;
    Ok(
        json!({"note_id": note_id, "created_note": false, "after_page": idx, "pages_added": sizes.len(),
              "page_count": s.state.pages().len(), "asset": up, "append": summary(&acks),
              "cloud_persisted": persisted(client, note_id, &up, wait).await?}),
    )
}

// ------------------------------------------------------------------ images
#[allow(clippy::too_many_arguments)]
fn image_value(
    up: &Value,
    name: &str,
    iw: u32,
    ih: u32,
    page: Value,
    frame: (f64, f64, f64, f64),
) -> Value {
    let (x, y, w, h) = frame;
    json!({"type": BLOCK_IMAGE, "page": page, "origin": {"x": x, "y": y},
           "size": {"width": w, "height": h}, "textWrap": 1,
           "image": {"metadata": asset_meta(up, name, image_mime(name)),
                     "size": {"width": f64::from(iw), "height": f64::from(ih)}}})
}

/// Insert → Image on page `page` (1-based) of an existing note: one `CreateBlock{type: Image}`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_image(
    client: &Client,
    note_id: &str,
    data: &[u8],
    name: &str,
    page: usize,
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    wait: bool,
) -> Result<Value> {
    let (iw, ih) = image_size(data)?;
    if iw == 0 || ih == 0 {
        return Err(NbError::usage("image has no pixels"));
    }
    let mut s = EditSession::open(client, note_id).await?;
    let pg = s.state.page(page)?;
    let (pw, ph) = s.state.page_size(&pg);
    let w = width.unwrap_or_else(|| f64::from(iw).min(pw - 72.0));
    if w <= 0.0 {
        return Err(NbError::usage("--width must be > 0"));
    }
    let h = w * f64::from(ih) / f64::from(iw);
    let x = x.unwrap_or((pw - w) / 2.0);
    let y = y.unwrap_or_else(|| 36f64.max((ph - h) / 3.0));
    let up = client.http.upload_bytes(data).await?;
    let o = s.op(
        "CreateBlock",
        image_value(&up, name, iw, ih, seqd(pg.id), (x, y, w, h)),
    );
    let bid = id_of(&o["id"]).map(fmt_id);
    let acks = s.append(vec![o]).await?;
    Ok(
        json!({"note_id": note_id, "created_note": false, "block_id": bid, "page": page,
              "frame": {"x": x, "y": y, "width": w, "height": h}, "asset": up, "append": summary(&acks),
              "cloud_persisted": persisted(client, note_id, &up, wait).await?}),
    )
}

/// Home → Import an image (`createNewNoteFromImages`): the blank-note seed plus the image
/// centred on the first page.
pub async fn import_image_new(
    client: &Client,
    data: &[u8],
    name: &str,
    title: Option<&str>,
    folder: Option<&str>,
    wait: bool,
) -> Result<Value> {
    let (iw, ih) = image_size(data)?;
    if iw == 0 || ih == 0 {
        return Err(NbError::usage("image has no pixels"));
    }
    let up = client.http.upload_bytes(data).await?;
    let title = title.map_or_else(|| stem_of(name), str::to_string);
    let mut ops = initial_ops(&title);
    let (pw, ph, m) = (612.0, 792.0, 36.0);
    let scale = 1f64
        .min((pw - 2.0 * m) / f64::from(iw))
        .min((ph - 2.0 * m) / f64::from(ih));
    let (w, h) = (f64::from(iw) * scale, f64::from(ih) * scale);
    let first = json!({"site": 0, "timestamp": 0, "index": 0});
    let blk = image_value(
        &up,
        name,
        iw,
        ih,
        first,
        ((pw - w) / 2.0, (ph - h) / 2.0, w, h),
    );
    ops.push(
        json!({"id": {"site": 0, "timestamp": ops.len()}, "clientTime": now_ms(),
                    "payload": {"type": "CreateBlock", "value": blk}}),
    );
    let note = client.create_note(&title, folder, ops).await?;
    let id = note["id"].as_str().unwrap_or_default().to_string();
    Ok(
        json!({"note_id": id, "title": title, "created_note": true, "block_id": "0:2",
              "frame": {"x": (pw - w) / 2.0, "y": (ph - h) / 2.0, "width": w, "height": h},
              "asset": up, "append": note["append"], "move": note.get("move"),
              "cloud_persisted": persisted(client, &id, &up, wait).await?}),
    )
}

// ------------------------------------------------------------------ assets
/// sha512 → `{fileName, mimeType, fileSize, kind}` for every `AssetMetadata` the ops reference.
pub fn asset_refs(ops: &[Value]) -> BTreeMap<String, Value> {
    fn walk(x: &Value, kind: &str, out: &mut BTreeMap<String, Value>, order: &mut Vec<String>) {
        match x {
            Value::Object(o) => {
                if let Some(h) = o.get("assetHash").and_then(Value::as_str)
                    && o.contains_key("mimeType")
                    && !out.contains_key(h)
                {
                    order.push(h.to_string());
                    out.insert(
                        h.to_string(),
                        json!({"fileName": o.get("fileName"), "mimeType": o.get("mimeType"),
                               "fileSize": o.get("fileSize"), "kind": kind}),
                    );
                }
                for v in o.values() {
                    walk(v, kind, out, order);
                }
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, kind, out, order)),
            _ => {}
        }
    }
    let mut out = BTreeMap::new();
    let mut order = Vec::new();
    for o in ops {
        let t = o["payload"]["type"].as_str().unwrap_or_default();
        if SERVER_ONLY_OPS.contains(&t) {
            continue;
        }
        walk(&o["payload"]["value"], t, &mut out, &mut order);
    }
    out
}

/// Asset hashes of the note's PDF page backgrounds, in op order, without repeats.
pub fn pdf_hashes(ops: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for o in ops {
        let t = o["payload"]["type"].as_str().unwrap_or_default();
        let v = &o["payload"]["value"];
        let bg = match t {
            "CreatePage" => &v["background"],
            "ModifyPage" => &v["background"]["value"],
            _ => continue,
        };
        if let Some(h) = bg["pdf"]["metadata"]["assetHash"].as_str()
            && !out.iter().any(|x| x == h)
        {
            out.push(h.to_string());
        }
    }
    out
}

// ------------------------------------------------------------------ .ntb
pub struct Ntb {
    pub version: String,
    pub manifest: Value,
    pub bundle: Vec<u8>,
    pub assets: BTreeMap<String, Vec<u8>>,
    pub thumbnail: Option<Vec<u8>>,
}

fn zip_err(e: impl std::fmt::Display) -> NbError {
    NbError::generic(format!("zip: {e}"))
}

/// `noteManager.exportNoteToNtb`: `version`, `manifest.json`, `noteBundle`, `assets/<sha512>`,
/// optional `thumbnail.png` (only when it is a PNG).
pub fn build_ntb(
    bundle: &[u8],
    assets: &BTreeMap<String, Vec<u8>>,
    thumbnail: Option<&[u8]>,
    app_version: &str,
) -> Result<Vec<u8>> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, data: &[u8]| -> Result<()> {
        z.start_file(name, opt).map_err(zip_err)?;
        z.write_all(data).map_err(zip_err)
    };
    put("version", b"1")?;
    put(
        "manifest.json",
        json!({"appVersion": app_version}).to_string().as_bytes(),
    )?;
    put("noteBundle", bundle)?;
    for (sha, data) in assets {
        put(&format!("assets/{sha}"), data)?;
    }
    if let Some(t) = thumbnail.filter(|t| t.starts_with(b"\x89PNG\r\n\x1a\n")) {
        put("thumbnail.png", t)?;
    }
    Ok(z.finish().map_err(zip_err)?.into_inner())
}

pub fn parse_ntb(data: &[u8]) -> Result<Ntb> {
    let mut z = zip::ZipArchive::new(Cursor::new(data))
        .map_err(|_| NbError::usage("not an .ntb archive"))?;
    let names: Vec<String> = z.file_names().map(str::to_string).collect();
    let mut read = |n: &str| -> Result<Vec<u8>> {
        let mut f = z.by_name(n).map_err(zip_err)?;
        let mut b = Vec::new();
        f.read_to_end(&mut b).map_err(zip_err)?;
        Ok(b)
    };
    for req in ["version", "manifest.json", "noteBundle"] {
        if !names.iter().any(|n| n == req) {
            return Err(NbError::usage(format!(".ntb is missing '{req}'")));
        }
    }
    let mut assets = BTreeMap::new();
    for n in names
        .iter()
        .filter(|n| n.starts_with("assets/") && n.len() > 7)
    {
        assets.insert(n[7..].to_string(), read(n)?);
    }
    let manifest = serde_json::from_slice(&read("manifest.json")?).unwrap_or(Value::Null);
    Ok(Ntb {
        version: String::from_utf8_lossy(&read("version")?).into_owned(),
        manifest,
        bundle: read("noteBundle")?,
        assets,
        thumbnail: if names.iter().any(|n| n == "thumbnail.png") {
            Some(read("thumbnail.png")?)
        } else {
            None
        },
    })
}

fn app_version() -> String {
    nb_api::config::CLIENT_HEADERS
        .iter()
        .find(|(k, _)| *k == "client-git-hash")
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// Build the `.ntb` for a note from server data.
pub async fn export_ntb(
    client: &Client,
    note_id: &str,
    thumbnail: bool,
) -> Result<(Vec<u8>, Value)> {
    let raw = client.http.note_bundle_raw(note_id).await?;
    if raw.starts_with(b"No ops found") || raw.len() < 8 {
        return Err(NbError::usage("the note has no content yet"));
    }
    let b = nb_protocol::codec::decode("NoteBundle", &raw)
        .map_err(|e| NbError::server(format!("bundle: {e}")))?;
    let ops = b["ops"].as_array().cloned().unwrap_or_default();
    let mut assets = BTreeMap::new();
    for sha in asset_refs(&ops).keys() {
        let (data, _) = client.http.download_asset(note_id, sha).await?;
        assets.insert(sha.clone(), data);
    }
    let thumb = if thumbnail {
        client.http.thumbnail(note_id, None).await.ok()
    } else {
        None
    };
    let has_thumb = thumb
        .as_deref()
        .is_some_and(|t| t.starts_with(b"\x89PNG\r\n\x1a\n"));
    let data = build_ntb(&raw, &assets, thumb.as_deref(), &app_version())?;
    Ok((
        data,
        json!({"ops": ops.len(), "assets": assets.len(), "thumbnail": has_thumb}),
    ))
}

/// `createNoteFromNtb`: a new note with the bundle's ops rebased onto site 0, every asset uploaded.
pub async fn import_ntb(
    client: &Client,
    data: &[u8],
    name: &str,
    title: Option<&str>,
    folder: Option<&str>,
) -> Result<Value> {
    let nt = parse_ntb(data)?;
    let b = nb_protocol::codec::decode("NoteBundle", &nt.bundle)
        .map_err(|e| NbError::usage(format!(".ntb noteBundle is not a NoteBundle: {e}")))?;
    let src: Vec<Value> = b["ops"].as_array().cloned().unwrap_or_default();
    let mut ops = rebase_ops(&src, 0);
    let refs = asset_refs(&ops);
    let missing = refs.keys().filter(|s| !nt.assets.contains_key(*s)).count();
    if missing > 0 {
        return Err(NbError::usage(format!(
            ".ntb lacks {missing} referenced asset(s)"
        )));
    }
    let mut uploaded = 0;
    for sha in refs.keys() {
        let up = client.http.upload_bytes(&nt.assets[sha]).await?;
        if up["sha512"] != sha.as_str() {
            return Err(NbError::usage(format!(
                "asset {sha:.12}… does not match its hash"
            )));
        }
        if up["uploaded"].as_bool() == Some(true) {
            uploaded += 1;
        }
    }
    let title = title.map_or_else(
        || {
            name.strip_suffix(".ntb")
                .or_else(|| name.strip_suffix(".NTB"))
                .unwrap_or(name)
                .to_string()
        },
        str::to_string,
    );
    ops.push(
        json!({"id": {"site": 0, "timestamp": ops.len()}, "clientTime": now_ms(),
                    "payload": {"type": "SetMetadata", "value": {"title": {"value": title}}}}),
    );
    let n = ops.len();
    let note = client.create_note(&title, folder, ops).await?;
    Ok(
        json!({"note_id": note["id"], "title": title, "created_note": true, "ops": n,
              "acks": note["append"]["acks"], "assets": refs.len(), "assets_uploaded": uploaded,
              "ntb_version": nt.version, "app_version": nt.manifest["appVersion"], "move": note.get("move")}),
    )
}

// ------------------------------------------------------------------ YouTube
const YT_QUERY: &str =
    "query getYoutubeTranscript($youtubeTranscriptParams: YoutubeTranscriptParams!) {
  getYoutubeTranscript(youtubeTranscriptParams: $youtubeTranscriptParams) {
    videoId track { languageCode isGenerated segments { text start duration } } } }";
pub const YT_LABEL: &str = "\u{1f5d2}\u{fe0f} Transcript: ";

fn is_vid(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// The 11-character video id of a watch / youtu.be / shorts / embed / live URL, or a bare id.
pub fn video_id(url: &str) -> Result<String> {
    if is_vid(url) {
        return Ok(url.to_string());
    }
    for marker in ["v=", "youtu.be/", "/shorts/", "/embed/", "/live/"] {
        let mut from = 0;
        while let Some(i) = url[from..].find(marker) {
            let start = from + i + marker.len();
            if let Some(cand) = url.get(start..start + 11)
                && is_vid(cand)
            {
                return Ok(cand.to_string());
            }
            from = start;
        }
    }
    Err(NbError::usage(format!("not a YouTube video URL: {url}")))
}

fn loc(o: &Value, index: usize) -> Value {
    json!({"site": o["id"]["site"], "timestamp": o["id"]["timestamp"], "index": index})
}

fn bound(o: &Value, index: usize, ty: u8) -> Value {
    json!({"location": loc(o, index), "type": ty})
}

/// The ops the web editor's "YouTube transcript" insert emits at the end of the body: link line,
/// grey 12 pt label + bold title, transcript, blank line. Indices count code points.
pub fn youtube_ops(s: &mut EditSession<'_>, url: &str, title: &str, text: &str) -> Vec<Value> {
    let chars = s.state.text_chars(None);
    let anchor = chars.last().map(|(id, _)| seqd(*id));
    let pre = if chars.last().is_some_and(|(_, c)| *c != '\n') {
        "\n"
    } else {
        ""
    };
    let n = |x: &str| x.chars().count();
    let mut v = json!({"string": format!("{pre}{url}\n\n")});
    if let Some(a) = anchor {
        v["location"] = a;
    }
    let u = s.op("InsertString", v);
    let o = n(pre);
    let link = s.op(
        "ModifyStyle",
        json!({"start": bound(&u, o, 0), "end": bound(&u, o + n(url) - 1, 1), "link": {"value": url}}),
    );
    let lab = s.op(
        "InsertString",
        json!({"location": loc(&u, o + n(url) + 1), "string": YT_LABEL}),
    );
    let ti = s.op(
        "InsertString",
        json!({"location": loc(&lab, n(YT_LABEL) - 1), "string": format!("{title}\n\n")}),
    );
    let grey = s.op(
        "ModifyStyle",
        json!({"start": bound(&lab, 0, 0), "end": bound(&ti, n(title), 0),
               "size": {"value": 12.0}, "foregroundColor": {"value": color_value((134, 134, 134, 255))}}),
    );
    let bold = s.op(
        "ModifyStyle",
        json!({"start": bound(&ti, 0, 0), "end": bound(&ti, n(title), 0), "bold": {"value": true}}),
    );
    let tx = s.op(
        "InsertString",
        json!({"location": loc(&ti, n(title) + 1), "string": text}),
    );
    let tail = s.op(
        "InsertString",
        json!({"location": loc(&tx, n(text).saturating_sub(1)), "string": "\n\n"}),
    );
    vec![u, link, lab, ti, grey, bold, tx, tail]
}

async fn oembed_title(client: &Client, watch: &str) -> Option<String> {
    let mut u = url::Url::parse("https://www.youtube.com/oembed").ok()?;
    u.query_pairs_mut()
        .append_pair("url", watch)
        .append_pair("format", "json");
    let r = client.http.fetch_external(u.as_str()).await.ok()?;
    if r.status != 200 {
        return None;
    }
    let v: Value = serde_json::from_slice(&r.body).ok()?;
    v["title"].as_str().map(str::to_string)
}

/// Home → YouTube (new note) or Insert → YouTube transcript (existing note).
pub async fn import_youtube(
    client: &Client,
    url: &str,
    note_id: Option<&str>,
    title: Option<&str>,
    folder: Option<&str>,
    languages: &[String],
) -> Result<Value> {
    let vid = video_id(url)?;
    let watch = if url.starts_with("http") {
        url.to_string()
    } else {
        format!("https://www.youtube.com/watch?v={vid}")
    };
    let vtitle = oembed_title(client, &watch)
        .await
        .unwrap_or_else(|| vid.clone());
    let langs: Vec<String> = if languages.is_empty() {
        vec!["en".into()]
    } else {
        languages.to_vec()
    };
    let tr = client
        .http
        .gql_field(
            YT_QUERY,
            json!({"youtubeTranscriptParams": {"videoId": vid, "preferredLanguages": langs}}),
            "getYoutubeTranscript",
        )
        .await?;
    let track = &tr["track"];
    if !track.is_object() {
        return Err(NbError::not_found(
            "no transcript is available for this video",
        ));
    }
    let segs = track["segments"].as_array().cloned().unwrap_or_default();
    let text = segs
        .iter()
        .filter_map(|s| s["text"].as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return Err(NbError::not_found("the transcript is empty"));
    }
    let mut out = json!({"video_id": vid, "title": vtitle, "language": track["languageCode"],
                         "generated": track["isGenerated"], "segments": segs.len(),
                         "chars": text.chars().count()});
    match note_id {
        None => {
            let ntitle = title.map_or_else(|| vtitle.clone(), str::to_string);
            let mut s = EditSession::new_note(&ntitle);
            let yt = youtube_ops(&mut s, &watch, &vtitle, &text);
            let mut ops = s.staged.clone();
            ops.extend(yt);
            let note = client.create_note(&ntitle, folder, ops).await?;
            out["note_id"] = note["id"].clone();
            out["created_note"] = json!(true);
            out["append"] = note["append"].clone();
        }
        Some(nid) => {
            let mut s = EditSession::open(client, nid).await?;
            let ops = youtube_ops(&mut s, &watch, &vtitle, &text);
            let acks = s.append(ops).await?;
            out["note_id"] = json!(nid);
            out["created_note"] = json!(false);
            out["append"] = summary(&acks);
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ audio
/// Duration from the `moov/mvhd` box of a (non-fragmented) mp4/m4a.
pub fn mp4_duration_ms(d: &[u8]) -> Option<u64> {
    fn boxes(d: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
        let mut out = Vec::new();
        let mut i = start;
        while i + 8 <= end {
            let mut size = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
            let typ = [d[i + 4], d[i + 5], d[i + 6], d[i + 7]];
            let mut hdr = 8;
            if size == 1 {
                let Some(b) = d.get(i + 8..i + 16) else { break };
                size = u64::from_be_bytes(b.try_into().unwrap_or_default()) as usize;
                hdr = 16;
            } else if size == 0 {
                size = end - i;
            }
            if size < hdr || i + size > end {
                break;
            }
            out.push((typ, i + hdr, i + size));
            i += size;
        }
        out
    }
    for (t, s, e) in boxes(d, 0, d.len()) {
        if &t != b"moov" {
            continue;
        }
        for (t2, s2, _) in boxes(d, s, e) {
            if &t2 != b"mvhd" {
                continue;
            }
            let rd32 = |i: usize| {
                d.get(i..i + 4)
                    .map(|b| u64::from(u32::from_be_bytes(b.try_into().unwrap_or_default())))
            };
            let (ts, dur) = if d.get(s2) == Some(&1) {
                (
                    rd32(s2 + 20)?,
                    d.get(s2 + 24..s2 + 32)
                        .map(|b| u64::from_be_bytes(b.try_into().unwrap_or_default()))?,
                )
            } else {
                (rd32(s2 + 12)?, rd32(s2 + 16)?)
            };
            if ts > 0 && dur > 0 && dur != 0xFFFF_FFFF {
                return Some(dur * 1000 / ts);
            }
        }
    }
    None
}

/// Duration of a PCM WAV (`fmt ` byte rate and `data` size).
pub fn wav_duration_ms(d: &[u8]) -> Option<u64> {
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return None;
    }
    let mut i = 12;
    let mut rate = None;
    while i + 8 <= d.len() {
        let id = &d[i..i + 4];
        let sz = u32::from_le_bytes(d[i + 4..i + 8].try_into().ok()?) as usize;
        if id == b"fmt " && sz >= 12 {
            rate = Some(u64::from(u32::from_le_bytes(
                d.get(i + 16..i + 20)?.try_into().ok()?,
            )));
        } else if id == b"data" {
            let r = rate.filter(|r| *r > 0)?;
            let len = sz.min(d.len() - i - 8) as u64;
            return Some(len * 1000 / r);
        }
        i += 8 + sz + (sz & 1);
    }
    None
}

fn which(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

fn ffprobe_duration(path: &Path) -> Option<u64> {
    let o = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .ok()?;
    let s: f64 = String::from_utf8_lossy(&o.stdout).trim().parse().ok()?;
    Some((s * 1000.0) as u64)
}

fn has_video(path: &Path) -> bool {
    if !which("ffprobe") {
        return false;
    }
    std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .is_ok_and(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .any(|t| t == "video")
        })
}

/// A recording ready to upload: bytes, file name, mime, duration and whether ffmpeg converted it.
pub struct PreparedAudio {
    pub data: Vec<u8>,
    pub file_name: String,
    pub mime: &'static str,
    pub duration_ms: u64,
    pub converted: bool,
}

/// Native formats (m4a, mp4, webm) are used as-is; anything else (wav, mp3, ogg, flac, video
/// tracks…) is converted to AAC m4a with ffmpeg. There is no pure-Rust
/// AAC encoder, and Notability stores and transcribes recordings only as m4a/mp4/webm.
pub fn prepare_audio(path: &str) -> Result<PreparedAudio> {
    let (data, name) = read_file(path)?;
    let p = Path::new(path);
    let ext = ext_of(&name);
    if let Some((_, mime)) = AUDIO_NATIVE.iter().find(|(e, _)| *e == ext)
        && !has_video(p)
    {
        let duration_ms = mp4_duration_ms(&data)
            .or_else(|| ffprobe_duration(p))
            .ok_or_else(|| NbError::usage(format!("could not determine the duration of {name}")))?;
        return Ok(PreparedAudio {
            data,
            file_name: name,
            mime,
            duration_ms,
            converted: false,
        });
    }
    if !which("ffmpeg") {
        return Err(NbError::usage(format!(
            "{ext} is not a Notability recording format (m4a, mp4, webm); install ffmpeg to convert"
        )));
    }
    let out = std::env::temp_dir().join(format!(
        "nb-rec-{}.m4a",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    let r = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(p)
        .args(["-vn", "-c:a", "aac", "-b:a", "96k"])
        .arg(&out)
        .output()
        .map_err(|e| NbError::usage(format!("ffmpeg: {e}")))?;
    if !r.status.success() {
        let _ = std::fs::remove_file(&out);
        let msg: String = String::from_utf8_lossy(&r.stderr)
            .trim()
            .chars()
            .take(200)
            .collect();
        return Err(NbError::usage(format!(
            "ffmpeg could not convert {name}: {msg}"
        )));
    }
    let conv = std::fs::read(&out).map_err(|e| NbError::generic(format!("{e}")));
    let dur = conv
        .as_ref()
        .ok()
        .and_then(|d| mp4_duration_ms(d))
        .or_else(|| ffprobe_duration(&out));
    let _ = std::fs::remove_file(&out);
    let conv = conv?;
    let duration_ms = dur
        .or_else(|| wav_duration_ms(&data))
        .ok_or_else(|| NbError::usage(format!("could not determine the duration of {name}")))?;
    Ok(PreparedAudio {
        data: conv,
        file_name: format!("{}.m4a", stem_of(&name)),
        mime: "audio/x-m4a",
        duration_ms,
        converted: true,
    })
}

/// Upload audio and attach it as a recording: `CreateRecording{recording: {metadata},
/// startTime, endTime}`; `transcribe` runs (or reuses) one ASR job.
pub async fn add_recording(
    client: &Client,
    note_id: &str,
    audio: PreparedAudio,
    start_ms: Option<u64>,
    name: Option<&str>,
    transcribe: bool,
    wait: bool,
) -> Result<Value> {
    if transcribe && nb_api::asr::asr_extension(audio.mime).is_none() {
        return Err(NbError::usage(format!(
            "ASR does not accept {}",
            audio.mime
        )));
    }
    let mut s = EditSession::open(client, note_id).await?;
    let up = client.http.upload_bytes(&audio.data).await?;
    let start = start_ms.unwrap_or_else(now_ms);
    let mut v = json!({"recording": {"metadata": asset_meta(&up, &audio.file_name, audio.mime)},
                       "startTime": start, "endTime": start + audio.duration_ms});
    if let Some(n) = name {
        v["name"] = json!(n);
    }
    let o = s.op("CreateRecording", v);
    let rid = id_of(&o["id"]).map(fmt_id);
    let acks = s.append(vec![o]).await?;
    let mut out = json!({"note_id": note_id, "recording_id": rid, "duration_ms": audio.duration_ms,
                         "mime": audio.mime, "file_name": audio.file_name, "converted": audio.converted,
                         "asset": up, "append": summary(&acks),
                         "cloud_persisted": persisted(client, note_id, &up, wait).await?});
    if transcribe {
        let sha = up["sha512"].as_str().unwrap_or_default();
        out["transcript"] = client
            .http
            .transcribe(
                sha,
                &audio.data,
                audio.mime,
                start as i64,
                Duration::from_secs(180),
            )
            .await?;
    }
    Ok(out)
}

/// The note state of a freshly decoded bundle (for callers holding raw ops).
pub fn state_of(ops: Vec<Value>) -> NoteState {
    NoteState::new(&json!({"editorSite": 0, "ops": ops}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(300u32.to_be_bytes());
        png.extend(200u32.to_be_bytes());
        assert_eq!(image_size(&png).unwrap(), (300, 200));
        let gif = b"GIF89a\x0a\x00\x14\x00".to_vec();
        assert_eq!(image_size(&gif).unwrap(), (10, 20));
        // JPEG: SOI, APP0 (len 4), SOF0 with h=0x0102 w=0x0304
        let jpg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 17, 8, 1, 2, 3, 4,
        ];
        assert_eq!(image_size(&jpg).unwrap(), (0x0304, 0x0102));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        let bits: u32 = (99) | (49 << 14);
        webp.extend(bits.to_le_bytes());
        webp.resize(30, 0);
        assert_eq!(image_size(&webp).unwrap(), (100, 50));
        assert!(image_size(b"nope").is_err());
    }

    #[test]
    fn video_ids() {
        for u in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ?si=x",
            "dQw4w9WgXcQ",
        ] {
            assert_eq!(video_id(u).unwrap(), "dQw4w9WgXcQ", "{u}");
        }
        assert!(video_id("https://example.com/").is_err());
    }

    #[test]
    fn ntb_round_trip() {
        let mut assets = BTreeMap::new();
        assets.insert("ab".repeat(64), b"asset".to_vec());
        let png = b"\x89PNG\r\n\x1a\nrest";
        let z = build_ntb(b"bundle", &assets, Some(png), "deadbeef").unwrap();
        let n = parse_ntb(&z).unwrap();
        assert_eq!(n.version, "1");
        assert_eq!(n.manifest["appVersion"], "deadbeef");
        assert_eq!(n.bundle, b"bundle");
        assert_eq!(n.assets, assets);
        assert_eq!(n.thumbnail.as_deref(), Some(&png[..]));
        let z2 = build_ntb(b"b", &BTreeMap::new(), Some(b"GIF89a"), "x").unwrap();
        assert!(parse_ntb(&z2).unwrap().thumbnail.is_none());
        assert!(parse_ntb(b"not a zip").is_err());
    }

    #[test]
    fn youtube_ops_shape() {
        let mut s = EditSession::new_note("t");
        let ops = youtube_ops(
            &mut s,
            "https://youtu.be/dQw4w9WgXcQ",
            "Title",
            "hello world",
        );
        assert_eq!(ops.len(), 8);
        assert!(ops[0]["payload"]["value"].get("location").is_none());
        assert_eq!(
            ops[0]["payload"]["value"]["string"],
            "https://youtu.be/dQw4w9WgXcQ\n\n"
        );
        assert_eq!(ops[1]["payload"]["value"]["end"]["location"]["index"], 27);
        assert_eq!(ops[2]["payload"]["value"]["location"]["index"], 29);
        assert_eq!(
            ops[3]["payload"]["value"]["location"]["index"],
            YT_LABEL.chars().count() - 1
        );
        assert_eq!(ops[6]["payload"]["value"]["location"]["index"], 6);
        assert_eq!(ops[7]["payload"]["value"]["location"]["index"], 10);
        // Applying them yields the expected body text.
        s.state.add_ops(ops);
        assert_eq!(
            s.state.text(None),
            format!("https://youtu.be/dQw4w9WgXcQ\n\n{YT_LABEL}Title\n\nhello world\n\n")
        );
    }

    #[test]
    fn refs_and_pdf_hashes() {
        let ops = vec![
            json!({"id": {"site": 0, "timestamp": 1}, "payload": {"type": "CreatePage", "value": {"background": {"pdf": {"metadata": {"assetHash": "p1", "mimeType": "application/pdf"}}}}}}),
            json!({"id": {"site": 0, "timestamp": 2}, "payload": {"type": "ModifyPage", "value": {"background": {"value": {"pdf": {"metadata": {"assetHash": "p2", "mimeType": "application/pdf"}}}}}}}),
            json!({"id": {"site": 0, "timestamp": 3}, "payload": {"type": "AssetCloudPersisted", "value": {"assetHash": "p3", "mimeType": "x"}}}),
        ];
        assert_eq!(pdf_hashes(&ops), vec!["p1", "p2"]);
        assert_eq!(
            asset_refs(&ops).keys().cloned().collect::<Vec<_>>(),
            vec!["p1", "p2"]
        );
    }

    #[test]
    fn wav_duration() {
        let mut w = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
        w.extend(8000u32.to_le_bytes());
        w.extend(16000u32.to_le_bytes());
        w.extend([2, 0, 16, 0]);
        w.extend(b"data");
        w.extend(32000u32.to_le_bytes());
        w.resize(w.len() + 32000, 0);
        assert_eq!(wav_duration_ms(&w), Some(2000));
    }
}
