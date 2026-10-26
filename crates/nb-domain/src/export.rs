//! Note details, Markdown/plain-text export and structured
//! content rendering (`$$…$$` math, fenced code, lists, checkboxes).

use std::collections::HashMap;

use nb_api::{NbError, Result};
use serde_json::{Value, json};

use crate::client::Client;
use crate::state::NoteState;

pub const HANDWRITING_NOTE: &str =
    "Handwriting is stored as vector ink; recognised text is not exposed by the web API.";

pub fn safe_name(s: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut prev_bad = false;
    for c in s.chars() {
        let bad = "\\/:*?\"<>|".contains(c) || (c as u32) < 0x20;
        if !bad {
            out.push(c);
        } else if !prev_bad {
            out.push('_');
        }
        prev_bad = bad;
    }
    let t = out.trim_matches(|c| matches!(c, ' ' | '.' | '_'));
    let t = if t.is_empty() { fallback } else { t };
    t.chars().take(120).collect()
}

pub async fn note_details(c: &Client, r: &str, transcripts: bool, learn: bool) -> Result<Value> {
    let n = c.resolve_note(r).await?;
    let lib = c.library().await?;
    let raw = c.http.note_bundle_raw(&n.id).await?;
    let dn = if raw.starts_with(b"No ops found") {
        nb_protocol::bundle::DecodedNote::default()
    } else {
        nb_protocol::bundle::decode_bundle(&raw)
            .map_err(|e| NbError::server(format!("bad note bundle: {e}")))?
    };
    let fields = dn.text_fields();
    // CreateRecording ops include deleted recordings; count live ones from the full state.
    let live_recs = if dn.recording_count > 0 {
        nb_protocol::codec::decode("NoteBundle", &raw)
            .map(|b| {
                let ops = b["ops"].as_array().cloned().unwrap_or_default();
                crate::recordings::recordings_of(&crate::media::state_of(ops))
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut out = n.summary(&lib.folder_map());
    out["folder_path"] = json!(lib.folder_path(n.folder_id.as_deref()));
    out["op_count"] = json!(dn.op_count);
    out["op_types"] = json!(dn.op_types);
    out["pages"] = json!(dn.page_ops);
    out["ink_strokes"] = json!(dn.ink_strokes);
    out["recordings"] = json!(live_recs.len());
    out["assets"] = json!(dn.asset_hashes);
    out["text"] = json!(fields.get(&None).cloned().unwrap_or_default());
    out["text_boxes"] = json!(
        fields
            .iter()
            .filter(|(k, v)| k.is_some() && !v.trim().is_empty())
            .map(|(_, v)| v)
            .collect::<Vec<_>>()
    );
    out["handwriting_note"] = if dn.ink_strokes > 0 {
        json!(HANDWRITING_NOTE)
    } else {
        Value::Null
    };
    let sm = if learn || transcripts {
        c.http.study_materials(&n.id).await.ok().flatten()
    } else {
        None
    };
    if transcripts {
        // Live recordings first (in note order), then any other ASR job the note's study
        // materials list (e.g. a recording deleted after transcription is dropped).
        let mut hashes: Vec<(Option<Value>, String)> = Vec::new();
        for r in &live_recs {
            if let Some(h) = r["sha512"].as_str()
                && !hashes.iter().any(|(_, x)| x == h)
            {
                hashes.push((Some(r["id"].clone()), h.to_string()));
            }
        }
        if live_recs.is_empty() {
            for a in sm
                .iter()
                .flat_map(|s| s["asrJobs"].as_array().into_iter().flatten())
            {
                if let Some(h) = a["sha512Hash"].as_str()
                    && !hashes.iter().any(|(_, x)| x == h)
                {
                    hashes.push((None, h.to_string()));
                }
            }
        }
        let mut segs = Vec::new();
        for (rid, h) in hashes {
            match c.http.transcript(&h).await {
                Ok(t) if t["success"] == false && t["errorCode"] == "JOB_NOT_FOUND" => {}
                Ok(t) => {
                    let job = &t["job"];
                    segs.push(json!({"recording_id": rid, "hash": h, "status": job["status"],
                        "segments": job["results"]["segments"].as_array().cloned().unwrap_or_default()}));
                }
                Err(e) => segs.push(json!({"recording_id": rid, "hash": h, "error": e.message})),
            }
        }
        out["transcripts"] = json!(segs);
    }
    if learn {
        out["study_materials"] = sm.clone().unwrap_or(Value::Null);
        if let Some(sm) = sm {
            // summaryOfSummaries usually shares its id with a summaryJobs entry; fetch each once.
            let mut summaries = Vec::new();
            let mut seen: Vec<String> = Vec::new();
            let jobs = ["summaryOfSummaries", "takeawaysSummary"]
                .into_iter()
                .map(|k| (k, &sm[k]))
                .chain(
                    sm["summaryJobs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|j| ("summary", j)),
                );
            for (kind, j) in jobs {
                let Some(id) = j["id"].as_str() else { continue };
                if j["status"] != "COMPLETED" || seen.iter().any(|s| s == id) {
                    continue;
                }
                seen.push(id.to_string());
                match c.http.summary_content(id).await {
                    Ok(content) => summaries.push(json!({"kind": kind, "content": content})),
                    Err(e) => summaries.push(json!({"kind": kind, "error": e.message})),
                }
            }
            out["summaries"] = json!(summaries);
        }
    }
    Ok(out)
}

pub fn fmt_ts(sec: Option<f64>) -> String {
    let Some(s) = sec else { return "--:--".into() };
    let s = s as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub fn summary_text(content: &Value) -> String {
    let parsed;
    let c = match content {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => {
                parsed = v;
                &parsed
            }
            Err(_) => return s.clone(),
        },
        v => v,
    };
    if let Some(o) = c.as_object() {
        for k in ["markdown", "summary", "text", "content"] {
            if let Some(s) = o.get(k).and_then(Value::as_str) {
                return s.to_string();
            }
        }
    }
    if let Value::String(s) = c {
        return s.clone();
    }
    format!(
        "```json\n{}\n```",
        serde_json::to_string_pretty(c).unwrap_or_default()
    )
}

pub fn to_markdown(d: &Value) -> String {
    let title = d["title"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("Untitled");
    let mut lines = vec![format!("# {title}"), String::new()];
    let content = d["content"].as_array().map(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    });
    let meta = [
        ("Folder", d["folder_path"].as_str().map(str::to_string)),
        ("Created", d["created_at"].as_str().map(str::to_string)),
        ("Updated", d["updated_at"].as_str().map(str::to_string)),
        ("Content", content),
        ("Note id", d["id"].as_str().map(str::to_string)),
    ];
    for (k, v) in meta {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            lines.push(format!("- **{k}:** {v}"));
        }
    }
    lines.push(String::new());
    let text = d["text"].as_str().unwrap_or_default();
    if !text.trim().is_empty() {
        lines.extend([
            "## Typed text".into(),
            String::new(),
            text.trim().to_string(),
            String::new(),
        ]);
    }
    for (i, tb) in d["text_boxes"].as_array().into_iter().flatten().enumerate() {
        lines.extend([
            format!("### Text box {}", i + 1),
            String::new(),
            tb.as_str().unwrap_or_default().trim().to_string(),
            String::new(),
        ]);
    }
    for s in d["summaries"].as_array().into_iter().flatten() {
        lines.extend([
            format!("## AI {}", s["kind"].as_str().unwrap_or_default()),
            String::new(),
            summary_text(&s["content"]).trim().to_string(),
            String::new(),
        ]);
    }
    for (i, t) in d["transcripts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let segs = t["segments"].as_array().cloned().unwrap_or_default();
        if segs.is_empty() {
            continue;
        }
        lines.extend([format!("## Transcript {}", i + 1), String::new()]);
        for s in segs {
            let spk = match s["speaker"].as_str() {
                Some(sp) if !sp.is_empty() => format!("**{sp}:** "),
                _ => match &s["speaker"] {
                    Value::Null => String::new(),
                    Value::String(_) => String::new(),
                    v => format!("**{v}:** "),
                },
            };
            lines.push(format!(
                "[{}] {spk}{}",
                fmt_ts(s["startTime"].as_f64()),
                s["text"].as_str().unwrap_or_default().trim()
            ));
        }
        lines.push(String::new());
    }
    if let Some(h) = d["handwriting_note"].as_str() {
        lines.extend([format!("> {h}"), String::new()]);
    }
    format!("{}\n", lines.join("\n").trim_end())
}

/// Plain text: the Markdown export with `*`, `#`, `>` and backtick markers removed.
pub fn to_text(d: &Value) -> String {
    let md = to_markdown(d);
    let mut out = String::with_capacity(md.len());
    let mut it = md.chars().peekable();
    while let Some(c) = it.next() {
        if "*#>`".contains(c) {
            while it.peek().is_some_and(|n| "*#>`".contains(*n)) {
                it.next();
            }
            if it.peek() == Some(&' ') {
                it.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Markdown of one text field from the edit log: lists, checkboxes, quotes, fenced code.
pub fn field_markdown(st: &NoteState, field: Option<crate::ids::Id>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut fence: Option<String> = None;
    let mut number = 0;
    for p in st.paragraphs(field) {
        let text = p.text.trim_start_matches('\n');
        let deco = p.style.decorator;
        if deco == Some(5) {
            let lang = p
                .style
                .language
                .clone()
                .unwrap_or_else(|| "plaintext".into());
            if fence.as_deref() != Some(lang.as_str()) {
                if fence.is_some() {
                    out.push("```".into());
                }
                out.push(format!(
                    "```{}",
                    if lang == "plaintext" { "" } else { &lang }
                ));
                fence = Some(lang);
            }
            out.push(text.to_string());
            continue;
        }
        if fence.take().is_some() {
            out.push("```".into());
        }
        let indent = "  ".repeat(p.style.indent.unwrap_or(0) as usize);
        number = if deco == Some(2) { number + 1 } else { 0 };
        out.push(match deco {
            Some(1) => format!("{indent}- {text}"),
            Some(2) => format!("{indent}{number}. {text}"),
            Some(3) => format!(
                "{indent}- [{}] {text}",
                if p.style.checked == Some(true) {
                    "x"
                } else {
                    " "
                }
            ),
            Some(4) => format!("> {text}"),
            _ => text.to_string(),
        });
    }
    if fence.is_some() {
        out.push("```".into());
    }
    out.join("\n")
}

pub fn math_markdown(latex: &str) -> String {
    format!("$${latex}$$")
}

/// Structured content of a note: body, text boxes, math (as `$$…$$`), images, ink, shapes.
pub fn content(st: &NoteState) -> Value {
    let blocks = st.blocks();
    let by_kind = |k: &str| {
        blocks
            .iter()
            .filter(|b| b["kind"] == k)
            .cloned()
            .collect::<Vec<_>>()
    };
    let boxes: Vec<Value> = by_kind("text")
        .into_iter()
        .map(|b| {
            let id = b["id"].as_str().and_then(|s| crate::ids::parse_id(s).ok());
            json!({"id": b["id"], "page": b["page"], "origin": b["origin"], "markdown": field_markdown(st, id)})
        })
        .collect();
    let math: Vec<Value> = by_kind("math")
        .into_iter()
        .map(|b| {
            json!({"id": b["id"], "page": b["page"], "latex": b["latex"],
                         "markdown": math_markdown(b["latex"].as_str().unwrap_or_default())})
        })
        .collect();
    let mut md = vec![field_markdown(st, None)];
    md.extend(
        boxes
            .iter()
            .filter_map(|b| b["markdown"].as_str().map(str::to_string)),
    );
    md.extend(
        math.iter()
            .filter_map(|m| m["markdown"].as_str().map(str::to_string)),
    );
    let ink = st.entities("CreateInk");
    let mut tools: HashMap<String, usize> = HashMap::new();
    for i in &ink {
        *tools
            .entry(i["tool"].as_str().unwrap_or("other").to_string())
            .or_default() += 1;
    }
    json!({
        "title": st.title(), "layout": st.layout(), "pages": st.pages().len(),
        "paragraphs": st.paragraphs(None).iter().map(|p| json!({"text": p.text.trim_start_matches('\n'), "style": p.style.to_json()})).collect::<Vec<_>>(),
        "text_boxes": boxes, "math": math, "images": by_kind("image"),
        "ink": {"strokes": ink.len(), "by_tool": tools}, "shapes": st.entities("CreateShape"),
        "comments": st.comments().len(),
        "markdown": md.into_iter().filter(|s| !s.trim().is_empty()).collect::<Vec<_>>().join("\n\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_name_sanitises() {
        assert_eq!(safe_name("a/b:c", "x"), "a_b_c");
        assert_eq!(safe_name("a//b", "x"), "a_b");
        assert_eq!(safe_name(" ..", "fallback"), "fallback");
    }

    #[test]
    fn text_strips_markdown_marks() {
        let d = json!({"title": "T", "id": "n1", "text": "hello **world**"});
        assert_eq!(
            to_text(&d),
            "T\n\n- Note id:n1\n\nTyped text\n\nhello world\n"
        );
    }
}
