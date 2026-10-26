use std::collections::HashMap;

use futures::future::BoxFuture;
use nb_api::Result;
use nb_domain::client::Client;
use nb_domain::state::NoteState;
use serde_json::{Value, json};

use crate::handler;
use crate::spec::*;

pub fn specs() -> Vec<Spec> {
    vec![
        paging(
            Spec::new("chat threads", Effect::R, "List chat threads", threads),
            20.0,
        ),
        Spec::new("chat read", Effect::R, "Read a thread", read)
            .arg(req("thread_id", "Thread id.")),
        Spec::new(
            "chat ask",
            Effect::Ai,
            "Ask Notability AI (optionally about a note)",
            ask,
        )
        .arg(req("question", "Your question."))
        .opt(cli_name(
            text(
                "thread_id",
                "Continue an existing thread (default: new thread).",
            ),
            "thread",
        ))
        .opt(text("note", "Ask in the context of a note."))
        .opt(flag("delete_after", "Delete the thread after answering.")),
        Spec::new(
            "chat context",
            Effect::R,
            "What the assistant's note_read tool receives for a note (typed, PDF and transcript text)",
            context,
        )
        .arg(req("note", "Note ref.")),
        Spec::new("chat delete", Effect::D, "Delete a thread", delete)
            .arg(req("thread_id", "Thread id.")),
        Spec::new(
            "chat backfill",
            Effect::W,
            "Index recently edited notes for AI chat",
            backfill,
        ),
    ]
}

handler!(threads, |c| {
    let cl = c.client()?;
    let p = Paging::from(&c, "chat")?;
    let limit = if p.limit == 0 { 100 } else { p.limit };
    let rows = cl.http.chat_threads(limit, p.offset).await?;
    Ok(Paging { limit, ..p }.server_page(rows, "chat"))
});

handler!(read, |c| {
    let cl = c.client()?;
    cl.http.chat_thread(&c.s("thread_id")?).await
});

/// Client tools this CLI answers (web: `note_read` is auto-approved, the rest ask the user).
const TOOLS: [&str; 1] = ["note_read"];
/// Per-source cap on text sent back to the assistant.
const SOURCE_CHARS: usize = 120_000;

fn cap(mut s: String) -> String {
    if let Some((i, _)) = s.char_indices().nth(SOURCE_CHARS) {
        s.truncate(i);
        s.push_str("\n[truncated]");
    }
    s
}

/// The note's machine-readable text as web `note_read` sources: typed text, PDF text (per note
/// page) and transcripts. Handwriting is not readable here, as on web.
async fn note_sources(cl: &Client, id: &str) -> Result<Vec<Value>> {
    let bundle = cl.http.note_bundle(id).await?;
    let st = NoteState::new(&bundle);
    let mut sources = Vec::new();
    let typed = st.text(None);
    if !typed.trim().is_empty() {
        sources.push(json!({"source": "note_text", "content": cap(typed)}));
    }
    let note_bg = st.note_background();
    let mut texts: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut pdf = String::new();
    for p in st.pages() {
        let bg = p.background.clone().unwrap_or_else(|| note_bg.clone());
        let Some(sha) = bg["pdf"]["metadata"]["assetHash"].as_str() else {
            continue;
        };
        if !texts.contains_key(sha) {
            let t = match cl.http.download_asset(id, sha).await {
                Ok((data, _)) => nb_render::text::page_texts(&data).ok(),
                Err(_) => None,
            };
            texts.insert(sha.to_string(), t);
        }
        let src = bg["pdf"]["pageOffset"].as_u64().unwrap_or(0) + p.id.2;
        if let Some(t) = texts[sha].as_ref().and_then(|t| t.get(src as usize))
            && !t.is_empty()
        {
            pdf.push_str(&format!("[Page {}]\n{t}\n\n", p.index));
        }
        if pdf.len() > SOURCE_CHARS {
            break;
        }
    }
    if !pdf.trim().is_empty() {
        sources.push(json!({"source": "note_pdf", "content": cap(pdf.trim_end().to_string())}));
    }
    let d = nb_domain::export::note_details(cl, id, true, false).await?;
    let tr = super::files::transcript_text(
        d["transcripts"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default(),
        true,
    );
    if tr.lines().any(|l| l.starts_with('[')) {
        sources.push(json!({"source": "asr_transcription", "content": cap(tr)}));
    }
    Ok(sources)
}

/// Web `note_read` handler semantics, but any note of the user's can be read (not just an open one).
async fn note_read(cl: &Client, context: Option<String>, args: Value) -> (String, Value) {
    let want = args["noteId"].as_str().map(str::to_string).or(context);
    let Some(id) = want else {
        return (
            "ERROR".into(),
            json!({"message": "No note is open, so nothing can be read. Answer from search results instead."}),
        );
    };
    let note = match cl.library().await {
        Ok(lib) => lib
            .all_notes(false)
            .into_iter()
            .find(|n| n.id == id)
            .cloned(),
        Err(e) => return ("ERROR".into(), json!({"message": e.message})),
    };
    if note.is_none() {
        return (
            "TOOL_NOT_AVAILABLE".into(),
            json!({"message": format!("No note exists with id {id}. Answer from search results instead.")}),
        );
    }
    match note_sources(cl, &id).await {
        Ok(s) if s.is_empty() => (
            "SUCCESS".into(),
            json!({"sources": [], "message": "No machine-readable text on this device: the note has no typed, PDF, or transcript text. Handwritten content cannot be read on web."}),
        ),
        Ok(s) => ("SUCCESS".into(), json!({"sources": s})),
        Err(e) => ("ERROR".into(), json!({"message": e.message})),
    }
}

handler!(context, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let (status, data) = note_read(&cl, Some(n.id.clone()), json!({})).await;
    let summary: Vec<Value> = data["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| json!({"source": s["source"], "chars": s["content"].as_str().map_or(0, |t| t.chars().count())}))
        .collect();
    Ok(
        json!({"note_id": n.id, "title": n.title, "status": status, "summary": summary, "data": data}),
    )
});

handler!(ask, |c| {
    let cl = c.client()?;
    let nid = match c.os("note") {
        Some(r) => Some(cl.resolve_note(&r).await?.id),
        None => None,
    };
    let clr = &cl;
    let ctx_note = nid.clone();
    let resolve = move |_tool: String, args: Value| -> BoxFuture<'_, (String, Value)> {
        Box::pin(note_read(clr, ctx_note.clone(), args))
    };
    let mut r = cl
        .http
        .chat_ask(
            &c.s("question")?,
            c.os("thread_id").as_deref(),
            nid.as_deref(),
            &TOOLS,
            Some(&resolve),
        )
        .await?;
    if c.b("delete_after") {
        let tid = r["thread_id"].as_str().unwrap_or_default().to_string();
        r["deleted"] = json!(cl.http.chat_delete(&tid).await?);
    }
    Ok(r)
});

handler!(delete, |c| {
    let cl = c.client()?;
    let tid = c.s("thread_id")?;
    let ok = cl.http.chat_delete(&tid).await?;
    Ok(json!({"thread_id": tid, "deleted": ok}))
});

handler!(backfill, |c| {
    let cl = c.client()?;
    cl.http.chat_backfill().await
});
