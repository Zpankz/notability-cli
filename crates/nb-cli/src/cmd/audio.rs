use nb_domain::EditSession;
use nb_domain::export::note_details;
use nb_domain::recordings::recordings;
use serde_json::json;

use super::files::transcript_text;
use crate::handler;
use crate::spec::*;

pub fn specs() -> Vec<Spec> {
    vec![
        Spec::new("audio list", Effect::R, "Recordings on a note", list).arg(note_arg()),
        Spec::new(
            "audio transcript",
            Effect::R,
            "Transcript text (--timestamps)",
            transcript,
        )
        .arg(note_arg())
        .opt(flag_on(
            "timestamps",
            "Prefix segments with [mm:ss] (--no-timestamps to drop).",
        )),
        super::imports::audio_upload_spec(),
        Spec::new(
            "audio record start",
            Effect::W,
            "Start recording audio for a note on this machine (ffmpeg)",
            rec_start,
        )
        .local_device()
        .arg(note_arg())
        .opt(text(
            "input_file",
            "Record from this audio file in real time instead of a device (tests).",
        ))
        .opt(text(
            "device",
            "ffmpeg FORMAT:INPUT (default pulse:default on Linux, avfoundation::0 on macOS).",
        )),
        Spec::new(
            "audio record stop",
            Effect::W,
            "Stop the local recording and attach it to its note",
            rec_stop,
        )
        .local_device()
        .opt(flag(
            "transcribe",
            "Run one server transcription (ASR) job after the upload.",
        ))
        .opt(flag("keep_file", "Keep the local m4a after uploading."))
        .opt(flag(
            "discard",
            "Stop and throw the recording away (no upload).",
        )),
        Spec::new(
            "audio record status",
            Effect::R,
            "Whether a local recording is running",
            rec_status,
        )
        .local_device(),
        Spec::new("audio delete", Effect::D, "Delete a recording", delete)
            .arg(note_arg())
            .arg(req(
                "recording_id",
                "Recording id SITE:TS (from audio list).",
            )),
    ]
}

handler!(list, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let s = EditSession::open(&cl, &n.id).await?;
    let items = recordings(&s);
    Ok(json!({"note_id": n.id, "title": n.title, "count": items.len(), "items": items}))
});

handler!(transcript, |c| {
    let cl = c.client()?;
    let d = note_details(&cl, &c.s("note")?, true, false).await?;
    let trs = d["transcripts"].as_array().cloned().unwrap_or_default();
    let text = transcript_text(&trs, c.b("timestamps"));
    Ok(json!({"note_id": d["id"], "title": d["title"], "transcripts": trs, "text": text}))
});

handler!(delete, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let mut s = EditSession::open(&cl, &n.id).await?;
    s.delete_recording(&c.s("recording_id")?).await
});

handler!(rec_start, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let mut r = nb_domain::recorder::start(
        &n.id,
        c.os("input_file").as_deref(),
        c.os("device").as_deref(),
    )?;
    r["title"] = json!(n.title);
    Ok(r)
});

handler!(rec_stop, |c| {
    if c.b("discard") && c.b("transcribe") {
        return Err(nb_api::NbError::usage(
            "--discard and --transcribe conflict",
        ));
    }
    let cl = c.client()?;
    nb_domain::recorder::stop(&cl, c.b("transcribe"), c.b("keep_file"), c.b("discard")).await
});

handler!(rec_status, |_c| { Ok(nb_domain::recorder::status()) });
