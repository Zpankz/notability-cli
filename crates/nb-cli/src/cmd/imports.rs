//! `files import …` and `audio upload`: uploads go prepareMediaUpload → cookie-free GCS PUT,
//! then the op that references the asset; the server confirms with `AssetCloudPersisted`.

use incurs::schema::FieldMeta;
use nb_api::{NbError, Result};
use nb_domain::Client;
use nb_domain::media::{self, AUDIO_EXTS, IMAGE_EXTS, VIDEO_EXTS};
use nb_domain::session::initial_ops;
use serde_json::{Value, json};

use crate::handler;
use crate::spec::*;

fn into_opt() -> FieldMeta {
    text(
        "into",
        "Add to this existing note instead of creating one (id, prefix, title or path).",
    )
}
fn title_opt() -> FieldMeta {
    text("title", "Title of the new note (default: the file name).")
}
fn folder_opt() -> FieldMeta {
    text(
        "folder",
        "Folder for the new note (id, title or path; default: library root).",
    )
}
fn wait_opt() -> FieldMeta {
    flag_on(
        "wait",
        "Wait up to 30 s for the server's AssetCloudPersisted (--no-wait to skip).",
    )
}
fn image_opts(s: Spec) -> Spec {
    s.opt(num_def(
        "page",
        1.0,
        "Page for the image with --into (1-based).",
    ))
    .opt(num("x", "Left edge in points (default: centred)."))
    .opt(num("y", "Top edge in points (default: upper third)."))
    .opt(num(
        "width",
        "Width in points (default: natural size, fitted to the page).",
    ))
}

pub fn specs() -> Vec<Spec> {
    vec![
        Spec::new(
            "files import pdf",
            Effect::W,
            "Import a PDF as a new note, or insert its pages into a note",
            pdf,
        )
        .arg(req("file", "Path to the PDF."))
        .opt(into_opt())
        .opt(num(
            "after_page",
            "With --into: insert after this page (0 = before the first; default: at the end).",
        ))
        .opt(title_opt())
        .opt(folder_opt())
        .opt(wait_opt()),
        image_opts(
            Spec::new(
                "files import image",
                Effect::W,
                "Import an image as a new note, or place it on a note's page",
                image,
            )
            .arg(req(
                "file",
                "Path to a PNG, JPEG, GIF, WebP, HEIC, BMP or TIFF image.",
            ))
            .opt(into_opt()),
        )
        .opt(title_opt())
        .opt(folder_opt())
        .opt(wait_opt()),
        image_opts(
            Spec::new(
                "files import media",
                Effect::W,
                "Import any supported file: PDF pages, an image, or audio as a recording",
                media_cmd,
            )
            .arg(req(
                "file",
                "Path to a PDF, image or audio file (audio other than m4a/mp4/webm needs ffmpeg).",
            ))
            .opt(into_opt()),
        )
        .opt(num(
            "after_page",
            "PDF with --into: insert after this page.",
        ))
        .opt(text("name", "Audio: recording name."))
        .opt(flag(
            "transcribe",
            "Audio: run one server transcription (ASR) job after the upload.",
        ))
        .opt(title_opt())
        .opt(folder_opt())
        .opt(wait_opt()),
        Spec::new(
            "files import ntb",
            Effect::W,
            "Import a Notability .ntb bundle as a new note",
            ntb,
        )
        .arg(req(
            "file",
            "Path to the .ntb file (from `files export --to ntb` or the apps).",
        ))
        .opt(title_opt())
        .opt(folder_opt()),
        Spec::new(
            "files import youtube",
            Effect::W,
            "Import a YouTube video's link and transcript as a new note, or into a note",
            youtube,
        )
        .arg(req("url", "YouTube URL or 11-character video id."))
        .opt(into_opt())
        .opt(text(
            "title",
            "Title of the new note (default: the video title).",
        ))
        .opt(folder_opt())
        .opt(multi(
            "lang",
            "Preferred caption languages, in order (e.g. --lang en --lang de).",
        )),
    ]
}

pub fn audio_upload_spec() -> Spec {
    Spec::new(
        "audio upload",
        Effect::W,
        "Attach an audio file to a note as a recording",
        audio_upload,
    )
    .arg(note_arg())
    .arg(req(
        "file",
        "Audio file (m4a/mp4/webm upload as-is; mp3, wav, … are converted with ffmpeg).",
    ))
    .opt(text("name", "Recording name."))
    .opt(num(
        "start_ms",
        "Recording start time, epoch ms (default: now).",
    ))
    .opt(flag(
        "transcribe",
        "Run one server transcription (ASR) job after the upload.",
    ))
    .opt(wait_opt())
}

fn pdf_sizes(data: &[u8], name: &str) -> Result<Vec<(f64, f64)>> {
    nb_render::info::page_sizes(data)
        .map_err(|e| NbError::usage(format!("{name}: not a readable PDF ({e})")))
}

async fn import_pdf(c: &Ctx, cl: &Client, data: &[u8], name: &str) -> Result<Value> {
    let sizes = pdf_sizes(data, name)?;
    let wait = c.b("wait");
    match c.os("into") {
        Some(n) => {
            let note = cl.resolve_note(&n).await?;
            media::import_pdf_into(cl, &note.id, data, name, &sizes, c.ou("after_page")?, wait)
                .await
        }
        None => {
            if c.os("after_page").is_some() {
                return Err(NbError::usage("--after-page needs --into"));
            }
            media::import_pdf_new(
                cl,
                data,
                name,
                &sizes,
                c.os("title").as_deref(),
                c.os("folder").as_deref(),
                wait,
            )
            .await
        }
    }
}

async fn import_image(c: &Ctx, cl: &Client, data: &[u8], name: &str) -> Result<Value> {
    let wait = c.b("wait");
    match c.os("into") {
        Some(n) => {
            let note = cl.resolve_note(&n).await?;
            let page = c.ou("page")?.unwrap_or(1);
            media::insert_image(
                cl,
                &note.id,
                data,
                name,
                page,
                c.of("x"),
                c.of("y"),
                c.of("width"),
                wait,
            )
            .await
        }
        None => {
            media::import_image_new(
                cl,
                data,
                name,
                c.os("title").as_deref(),
                c.os("folder").as_deref(),
                wait,
            )
            .await
        }
    }
}

async fn import_audio(c: &Ctx, cl: &Client, path: &str, note: Option<String>) -> Result<Value> {
    let audio = media::prepare_audio(path)?;
    let (id, created) = match note {
        Some(n) => (cl.resolve_note(&n).await?.id, None),
        None => {
            let title = c
                .os("title")
                .unwrap_or_else(|| media::stem_of(&audio.file_name));
            let n = cl
                .create_note(&title, c.os("folder").as_deref(), initial_ops(&title))
                .await?;
            (n["id"].as_str().unwrap_or_default().to_string(), Some(n))
        }
    };
    let start = c.of("start_ms").map(|v| v as u64);
    let mut out = media::add_recording(
        cl,
        &id,
        audio,
        start,
        c.os("name").as_deref(),
        c.b("transcribe"),
        c.b("wait"),
    )
    .await?;
    out["created_note"] = json!(created.is_some());
    if let Some(n) = created {
        out["title"] = n["title"].clone();
    }
    Ok(out)
}

handler!(pdf, |c| {
    let cl = c.client()?;
    let (data, name) = media::read_file(&c.s("file")?)?;
    import_pdf(&c, &cl, &data, &name).await
});

handler!(image, |c| {
    let cl = c.client()?;
    let (data, name) = media::read_file(&c.s("file")?)?;
    if !IMAGE_EXTS.iter().any(|(e, _)| *e == media::ext_of(&name)) {
        return Err(NbError::usage(format!(
            "{name}: not a supported image type"
        )));
    }
    import_image(&c, &cl, &data, &name).await
});

handler!(media_cmd, |c| {
    let cl = c.client()?;
    let path = c.s("file")?;
    let ext = media::ext_of(&path);
    if ext == ".pdf" {
        let (data, name) = media::read_file(&path)?;
        return import_pdf(&c, &cl, &data, &name).await;
    }
    if IMAGE_EXTS.iter().any(|(e, _)| *e == ext) {
        let (data, name) = media::read_file(&path)?;
        return import_image(&c, &cl, &data, &name).await;
    }
    if VIDEO_EXTS.contains(&ext.as_str()) {
        return Err(NbError::usage(format!(
            "{ext}: video is not supported (extract the audio first, e.g. ffmpeg -i in{ext} -vn out.m4a)"
        )));
    }
    if AUDIO_EXTS.contains(&ext.as_str()) {
        return import_audio(&c, &cl, &path, c.os("into")).await;
    }
    Err(NbError::usage(format!(
        "{ext}: unsupported file type (PDF, image or audio: {})",
        AUDIO_EXTS.join(" ")
    )))
});

handler!(ntb, |c| {
    let cl = c.client()?;
    let (data, name) = media::read_file(&c.s("file")?)?;
    media::import_ntb(
        &cl,
        &data,
        &name,
        c.os("title").as_deref(),
        c.os("folder").as_deref(),
    )
    .await
});

handler!(youtube, |c| {
    let cl = c.client()?;
    let note = match c.os("into") {
        Some(n) => Some(cl.resolve_note(&n).await?.id),
        None => None,
    };
    media::import_youtube(
        &cl,
        &c.s("url")?,
        note.as_deref(),
        c.os("title").as_deref(),
        c.os("folder").as_deref(),
        &c.list("lang"),
    )
    .await
});

handler!(audio_upload, |c| {
    let cl = c.client()?;
    let path = c.s("file")?;
    if VIDEO_EXTS.contains(&media::ext_of(&path).as_str()) {
        return Err(NbError::usage(
            "video is not supported; extract the audio first",
        ));
    }
    import_audio(&c, &cl, &path, Some(c.s("note")?)).await
});
