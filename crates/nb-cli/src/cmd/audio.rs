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
        Spec::new(
            "audio live start",
            Effect::Aiw,
            "Live transcription: stream the mic (or a file) and get the transcript and smart notes",
            live_start,
        )
        .local_device()
        .arg(note_arg())
        .opt(text(
            "input_file",
            "Stream this audio file instead of a device.",
        ))
        .opt(text(
            "device",
            "ffmpeg FORMAT:INPUT (default pulse:default on Linux, avfoundation::0 on macOS).",
        ))
        .opt(num(
            "seconds",
            "Stop after this many seconds (required for a device).",
        ))
        .opt(text_def("language", "en-US", "Spoken language (BCP 47)."))
        .opt(flag(
            "realtime",
            "Pace a file at real-time speed, like a live recording.",
        ))
        .opt(flag(
            "attach",
            "Also attach the streamed audio to the note as a recording.",
        ))
        .opt(flag_on(
            "finalize",
            "Close the session when the audio ends (--no-finalize to keep it open).",
        )),
        Spec::new(
            "audio live finalize",
            Effect::Ai,
            "Close a live-transcription session left open with --no-finalize",
            live_finalize,
        )
        .arg(req("session_id", "Session id from `audio live start`."))
        .opt(text_def("language", "en-US", "Spoken language (BCP 47).")),
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

handler!(live_start, |c| {
    use nb_domain::live::{LiveOptions, LiveSource, default_device};
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let src = match (c.os("input_file"), c.os("device")) {
        (Some(_), Some(_)) => {
            return Err(nb_api::NbError::usage(
                "pass --input-file or --device, not both",
            ));
        }
        (Some(f), None) => LiveSource::File {
            path: f.into(),
            realtime: c.b("realtime"),
        },
        (None, d) => LiveSource::Device(d.unwrap_or_else(default_device)),
    };
    let attach = c.b("attach");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("notability-live-{}-{stamp}", std::process::id()));
    if attach {
        std::fs::create_dir_all(&tmp)
            .map_err(|e| nb_api::NbError::generic(format!("temp dir: {e}")))?;
    }
    let opts = LiveOptions {
        language: c.os("language").unwrap_or_else(|| "en-US".into()),
        seconds: c.of("seconds"),
        chunks_per_put: 2,
        save: attach.then(|| tmp.join("live.mp4")),
        session: None,
        finalize: c.b("finalize"),
    };
    let res = cl
        .live_transcribe(&n.id, src, opts, |t| {
            if !t.is_empty() {
                eprintln!("{t}");
            }
        })
        .await;
    if res.is_err() && attach {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    let mut out = res?;
    out["title"] = json!(n.title);
    if attach {
        let raw = tmp.join("live.mp4");
        let m4a = tmp.join("Live transcription.m4a");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-loglevel", "error", "-y", "-i"])
            .arg(&raw)
            .args(["-c", "copy", "-movflags", "+faststart"])
            .arg(&m4a)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(nb_api::NbError::generic(
                "could not remux the streamed audio for --attach",
            ));
        }
        let audio = nb_domain::media::prepare_audio(&m4a.display().to_string());
        let _ = std::fs::remove_dir_all(&tmp);
        let rec = nb_domain::media::add_recording(
            &cl,
            &n.id,
            audio?,
            None,
            Some("Live transcription"),
            false,
            false,
        )
        .await?;
        out["recording"] = rec;
        out.as_object_mut().map(|o| o.remove("saved"));
    }
    Ok(out)
});

handler!(live_finalize, |c| {
    let cl = c.client()?;
    let lang = c.os("language").unwrap_or_else(|| "en-US".into());
    let sid = c.s("session_id")?;
    let r = cl.http.live_finalize(&sid, &lang).await?;
    Ok(json!({"session_id": sid, "finalized": r}))
});
