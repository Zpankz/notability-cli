use std::time::Duration;

use incurs::schema::FieldMeta;
use nb_api::{NbError, Result};
use nb_domain::export::note_details;
use nb_domain::services::{MODES, STATUSES, completion, parse_cards};
use serde_json::{Value, json};

use super::files::summaries_text;
use crate::handler;
use crate::spec::*;

fn modes() -> Vec<String> {
    MODES.iter().map(|m| m.to_lowercase()).collect()
}

fn mode_opt() -> incurs::schema::FieldMeta {
    let m = modes();
    let refs: Vec<&str> = m.iter().map(String::as_str).collect();
    choice(
        "mode",
        &refs,
        Some("practice_multiple_choice"),
        "Quiz mode.",
    )
}

pub fn specs() -> Vec<Spec> {
    let statuses: Vec<String> = STATUSES.iter().map(|s| s.to_lowercase()).collect();
    let statuses: Vec<&str> = statuses.iter().map(String::as_str).collect();
    vec![
        Spec::new(
            "learn materials",
            Effect::R,
            "Study-material status and contents",
            materials,
        )
        .arg(note_arg()),
        Spec::new(
            "learn summary",
            Effect::R,
            "AI summary already generated for a note",
            summary,
        )
        .arg(note_arg()),
        Spec::new(
            "learn wait",
            Effect::R,
            "Wait for generation to finish (--timeout)",
            wait,
        )
        .arg(note_arg())
        .opt(num_def("timeout", 180.0, "Seconds to wait.")),
        Spec::new(
            "learn generate",
            Effect::Aiw,
            "Generate summary/quizzes/flashcards (spends quota)",
            generate,
        )
        .arg(note_arg())
        .opt(text(
            "text",
            "Study text to send (default: the note's typed text).",
        ))
        .opt(text("text_file", "Read the study text from this file."))
        .opt(flag_on(
            "quizzes",
            "Include quizzes (--no-quizzes to skip).",
        ))
        .opt(flag_on(
            "flashcards",
            "Include flashcards (--no-flashcards to skip).",
        ))
        .opt(text("language", "Output language."))
        .opt(flag("wait", "Wait until generation finishes.")),
        Spec::new(
            "learn explain",
            Effect::Ai,
            "Explain a quiz question",
            explain,
        )
        .arg(req("question_id", "Quiz question id.")),
        Spec::new(
            "learn quiz list",
            Effect::R,
            "Quiz sessions (--note --status)",
            quiz_list,
        )
        .opt(text("note", "Only sessions for this note."))
        .opt(choice(
            "status",
            &["all", "active", "completed"],
            Some("all"),
            "Session status.",
        ))
        .opt(alias(num_def("limit", 20.0, "Max sessions."), 'n')),
        Spec::new(
            "learn quiz start",
            Effect::W,
            "Start a quiz session",
            quiz_start,
        )
        .arg(note_arg())
        .opt(mode_opt()),
        Spec::new(
            "learn quiz answer",
            Effect::W,
            "Answer a question",
            quiz_answer,
        )
        .arg(note_arg())
        .arg(req("session_id", "Quiz session id."))
        .arg(req_num("question_index", "Question index."))
        .opt(mode_opt())
        .opt(FieldMeta {
            required: true,
            ..choice("status", &statuses, None, "Answer status.")
        })
        .opt(text("choice", "A-D for multiple choice."))
        .opt(text("fill", "Answer text for fill-in-the-blank."))
        .opt(flag("finish", "Also mark the session completed.")),
        Spec::new(
            "learn quiz finish",
            Effect::W,
            "Finish a session",
            quiz_finish,
        )
        .arg(note_arg())
        .arg(req("session_id", "Quiz session id."))
        .opt(mode_opt()),
        Spec::new(
            "learn cards due",
            Effect::R,
            "Flashcards due for review",
            cards_due,
        )
        .opt(multi("note", "Restrict to notes (repeatable).")),
        Spec::new(
            "learn cards rate",
            Effect::W,
            "Rate a flashcard",
            cards_rate,
        )
        .arg(note_arg())
        .arg(req("session_id", "Spaced-repetition session id."))
        .arg(req_num("question_index", "Card index."))
        .arg(req_choice(
            "rating",
            &["again", "hard", "good", "easy"],
            "again, hard, good or easy.",
        )),
        Spec::new(
            "learn cards import",
            Effect::W,
            "Import flashcards from a delimited file",
            cards_import,
        )
        .arg(note_arg())
        .arg(req("file", "term<sep>definition lines."))
        .opt(text_def(
            "sep",
            "\t",
            "Separator between term and definition.",
        )),
    ]
}

fn read_file(p: &str) -> Result<String> {
    std::fs::read_to_string(p).map_err(|e| NbError::usage(format!("cannot read {p}: {e}")))
}

handler!(materials, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let sm = cl.http.study_materials(&n.id).await?;
    Ok(json!({"note_id": n.id, "title": n.title, "study_materials": sm}))
});

handler!(summary, |c| {
    let cl = c.client()?;
    let d = note_details(&cl, &c.s("note")?, false, true).await?;
    let sums = d["summaries"].as_array().cloned().unwrap_or_default();
    Ok(
        json!({"note_id": d["id"], "title": d["title"], "summaries": sums, "text": summaries_text(&sums)}),
    )
});

handler!(wait, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let t = Duration::from_secs_f64(c.of("timeout").unwrap_or(180.0).max(0.0));
    let sm = cl.learn_wait(&n.id, t).await?;
    Ok(json!({"note_id": n.id, "study_materials": sm}))
});

handler!(generate, |c| {
    let cl = c.client()?;
    let text = match (c.os("text"), c.os("text_file")) {
        (Some(_), Some(_)) => return Err(NbError::usage("pass --text or --text-file, not both")),
        (Some(t), None) => Some(t),
        (None, Some(p)) => Some(read_file(&p)?),
        (None, None) => None,
    };
    let note = c.s("note")?;
    let mut r = cl
        .learn_generate(
            &note,
            text,
            c.b("quizzes"),
            c.b("flashcards"),
            c.os("language").as_deref(),
        )
        .await?;
    if c.b("wait") {
        let id = r["note_id"].as_str().unwrap_or_default().to_string();
        r["study_materials"] = json!(cl.learn_wait(&id, Duration::from_secs(180)).await?);
    }
    Ok(r)
});

handler!(explain, |c| {
    let cl = c.client()?;
    cl.quiz_explain(&c.s("question_id")?).await
});

handler!(quiz_list, |c| {
    let cl = c.client()?;
    let nid = match c.os("note") {
        Some(r) => Some(cl.resolve_note(&r).await?.id),
        None => None,
    };
    let status = c
        .os("status")
        .unwrap_or_else(|| "all".into())
        .to_uppercase();
    let mut rows = cl
        .http
        .quiz_sessions(nid.as_deref(), c.ou("limit")?.unwrap_or(20), &status)
        .await?;
    if !rows.is_empty() {
        let lib = cl.library().await?;
        let titles: std::collections::HashMap<String, String> = lib
            .all_notes(true)
            .into_iter()
            .map(|n| (n.id.clone(), n.title.clone()))
            .collect();
        for r in &mut rows {
            let t = r["noteId"].as_str().and_then(|id| titles.get(id)).cloned();
            r["noteTitle"] = json!(t);
        }
    }
    Ok(json!({"count": rows.len(), "items": rows}))
});

fn mode(c: &Ctx) -> String {
    c.os("mode")
        .unwrap_or_else(|| "practice_multiple_choice".into())
}

handler!(quiz_start, |c| {
    let cl = c.client()?;
    cl.quiz_start(&c.s("note")?, &mode(&c)).await
});

handler!(quiz_answer, |c| {
    let cl = c.client()?;
    let comp = completion(
        c.u("question_index")? as i64,
        &c.s("status")?,
        c.os("choice").as_deref(),
        c.os("fill").as_deref(),
        None,
    )?;
    cl.quiz_answer(
        &c.s("note")?,
        &c.s("session_id")?,
        &mode(&c),
        vec![comp],
        c.b("finish"),
    )
    .await
});

handler!(quiz_finish, |c| {
    let cl = c.client()?;
    cl.quiz_answer(
        &c.s("note")?,
        &c.s("session_id")?,
        &mode(&c),
        Vec::new(),
        true,
    )
    .await
});

handler!(cards_due, |c| {
    let cl = c.client()?;
    let mut ids = Vec::new();
    for r in c.list("note") {
        ids.push(cl.resolve_note(&r).await?.id);
    }
    let rows: Vec<Value> = cl.http.flashcard_due(&ids).await?;
    Ok(json!({"count": rows.len(), "items": rows}))
});

handler!(cards_rate, |c| {
    let cl = c.client()?;
    cl.card_rate(
        &c.s("note")?,
        &c.s("session_id")?,
        c.u("question_index")? as i64,
        &c.s("rating")?,
    )
    .await
});

handler!(cards_import, |c| {
    let cl = c.client()?;
    let sep = c.os("sep").unwrap_or_else(|| "\t".into());
    let cards = parse_cards(&read_file(&c.s("file")?)?, &sep)?;
    cl.cards_import(&c.s("note")?, cards).await
});
