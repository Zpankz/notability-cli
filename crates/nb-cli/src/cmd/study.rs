//! `learn toc|finish|session|discuss|syllabus`: TOC quizzes, "Finish my notes", the guided
//! "Start studying" session, Brain Break discussions and syllabus import.

use std::time::Duration;

use nb_api::{NbError, Result};
use nb_domain::Client;
use nb_domain::media;
use nb_domain::organize::DEFAULT_COLOR;
use nb_domain::study::{self, Section};
use serde_json::{Value, json};

use crate::handler;
use crate::spec::*;

fn stage_opt() -> incurs::schema::FieldMeta {
    choice(
        "stage",
        &["memorize", "practice", "test"],
        Some("practice"),
        "Stage: memorize (flashcards), practice (multiple choice) or test (exam).",
    )
}

pub fn specs() -> Vec<Spec> {
    vec![
        Spec::new(
            "learn toc list",
            Effect::R,
            "Top-level sections of the note's PDF table of contents",
            toc_list,
        )
        .arg(note_arg()),
        Spec::new(
            "learn toc quiz",
            Effect::Aiw,
            "Generate study materials from chosen TOC sections (spends one AI job)",
            toc_quiz,
        )
        .arg(note_arg())
        .opt(multi(
            "section",
            "Section index (from `learn toc list`) or title text; repeatable (default: the first three).",
        ))
        .opt(flag_on("quizzes", "Include quizzes (--no-quizzes to skip)."))
        .opt(flag_on(
            "flashcards",
            "Include flashcards (--no-flashcards to skip).",
        ))
        .opt(text("language", "Output language."))
        .opt(flag("wait", "Wait until generation finishes.")),
        Spec::new(
            "learn finish",
            Effect::Aiw,
            "Finish my notes: AI write-up from the note's text and transcripts, added to the note",
            finish,
        )
        .confirm()
        .arg(note_arg())
        .opt(flag(
            "use_existing",
            "Reuse the note's finished summary instead of starting a new AI job.",
        ))
        .opt(flag("no_insert", "Only return the write-up; do not add it to the note."))
        .opt(flag("at_start", "Insert at the start of the note instead of the end."))
        .opt(flag("dry_run", "Show what would happen without spending quota or writing."))
        .opt(num_def("timeout", 240.0, "Seconds to wait for the AI job.")),
        Spec::new(
            "learn session start",
            Effect::W,
            "Start or resume a study-session stage; shows the first open question",
            session_start,
        )
        .arg(note_arg())
        .opt(stage_opt()),
        Spec::new(
            "learn session next",
            Effect::W,
            "Show the stage's next open question and progress",
            session_start,
        )
        .arg(note_arg())
        .opt(stage_opt()),
        Spec::new(
            "learn session answer",
            Effect::W,
            "Answer the current question; graded like the app, then shows the next one",
            session_answer,
        )
        .arg(note_arg())
        .opt(stage_opt())
        .opt(text(
            "answer",
            "A-D for multiple choice, true/false, or the missing term for fill-in-the-blank.",
        ))
        .opt(flag("knew", "Flashcard: I knew it."))
        .opt(flag("missed", "Flashcard: I did not know it.")),
        Spec::new(
            "learn session end",
            Effect::W,
            "Complete the stage's session and show the score and the next stage",
            session_end,
        )
        .arg(note_arg())
        .opt(stage_opt()),
        Spec::new(
            "learn discuss start",
            Effect::Ai,
            "Brain Break: start an AI discussion about the note (topics + opening line)",
            discuss_start,
        )
        .unverified()
        .arg(note_arg()),
        Spec::new(
            "learn discuss reply",
            Effect::Ai,
            "Brain Break: next turn (pick --topic, a --choice from the last reply, or --say)",
            discuss_reply,
        )
        .unverified()
        .arg(note_arg())
        .opt(text("topic", "Topic id from `discuss start` (switches topic)."))
        .opt(text("choice", "Choice id from the last reply's choices."))
        .opt(text("say", "Free-form reply.")),
        Spec::new(
            "learn discuss end",
            Effect::Local,
            "Brain Break: forget the local discussion state for a note",
            discuss_end,
        )
        .arg(note_arg()),
        Spec::new(
            "learn syllabus",
            Effect::Aiw,
            "Import a syllabus PDF: AI reads the course and dates; creates a course folder with the PDF",
            syllabus,
        )
        .arg(req("file", "Syllabus PDF (max 14 MB)."))
        .opt(text("parent", "Parent folder for the course folder (default: library root)."))
        .opt(text(
            "name",
            "Course folder name (default: the course name read from the syllabus).",
        ))
        .opt(flag(
            "no_import",
            "Only parse; do not create the folder or import the PDF.",
        )),
    ]
}

/// The first PDF on the note with a usable outline: `(sha, file, sections, bytes)`.
async fn toc_of(cl: &Client, note_id: &str) -> Result<(String, String, Vec<Section>, Vec<u8>)> {
    let pdfs = cl.note_pdfs(note_id).await?;
    if pdfs.is_empty() {
        return Err(NbError::usage("the note has no PDF pages"));
    }
    for (sha, name) in pdfs {
        let (data, _) = cl.http.download_asset(note_id, &sha).await?;
        let Ok(items) = nb_render::info::outline(&data) else {
            continue;
        };
        let pages = nb_render::info::page_sizes(&data)
            .map(|p| p.len())
            .unwrap_or(0);
        let items: Vec<(usize, String, Option<usize>)> = items
            .into_iter()
            .map(|o| (o.level, o.title, o.page))
            .collect();
        let secs = study::toc_sections(&items, pages);
        if secs.len() > 1 {
            return Ok((sha, name, secs, data));
        }
    }
    Err(NbError::usage(
        "no PDF on this note has a table of contents (outline) with two or more sections",
    ))
}

/// Extracted text of every PDF on a note (pages in order), `None` when there is none.
async fn pdf_text_of(cl: &Client, note_id: &str) -> Result<Option<String>> {
    let mut out = String::new();
    for (sha, _) in cl.note_pdfs(note_id).await? {
        let (data, _) = cl.http.download_asset(note_id, &sha).await?;
        if let Ok(pages) = nb_render::text::page_texts(&data) {
            for p in pages {
                out.push_str(p.trim());
                out.push_str("\n\n");
            }
        }
    }
    Ok((!out.trim().is_empty()).then_some(out))
}

handler!(toc_list, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let (_, file, secs, _) = toc_of(&cl, &n.id).await?;
    let items: Vec<Value> = secs.iter().map(Section::to_json).collect();
    Ok(
        json!({"note_id": n.id, "title": n.title, "pdf": file, "count": items.len(), "items": items}),
    )
});

handler!(toc_quiz, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let (_, file, secs, data) = toc_of(&cl, &n.id).await?;
    let pick = study::select_sections(&secs, &c.list("section"))?;
    let texts = nb_render::text::page_texts(&data)
        .map_err(|e| NbError::usage(format!("{file}: cannot read PDF text ({e})")))?;
    let pdf_text = study::section_text(&pick, &texts);
    if pdf_text.trim().chars().count() < 20 {
        return Err(NbError::usage(
            "the chosen sections have no extractable text",
        ));
    }
    let mut materials = Vec::new();
    let typed = nb_domain::search::note_text(&cl, &n.id)
        .await
        .unwrap_or_default();
    if !typed.trim().is_empty() {
        materials.push(json!({"content": typed, "type": "NOTE_TEXT"}));
    }
    materials.push(json!({"content": pdf_text, "type": "NOTE_PDF"}));
    let batch = cl
        .learn_generate_with(
            &n.id,
            materials,
            Vec::new(),
            c.b("quizzes"),
            c.b("flashcards"),
            c.os("language").as_deref(),
        )
        .await?;
    let mut out = json!({"note_id": n.id, "title": n.title, "pdf": file,
                         "sections": pick.iter().map(Section::to_json).collect::<Vec<_>>(),
                         "batch": batch});
    if c.b("wait") {
        let sm = json!(cl.learn_wait(&n.id, Duration::from_secs(180)).await?);
        let failed: Vec<Value> = sm["quizJobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|j| j["status"] == "FAILED")
            .map(|j| json!({"type": j["type"], "reason": j["errorReason"]}))
            .collect();
        if failed.iter().any(|f| f["reason"] == "INPUT_TOO_SHORT") {
            out["hint"] = json!(
                "the chosen sections are too short for the generator; pick more or longer sections"
            );
        }
        if !failed.is_empty() {
            out["failed_jobs"] = json!(failed);
        }
        out["study_materials"] = sm;
    }
    Ok(out)
});

handler!(finish, |c| {
    let cl = c.client()?;
    let note = c.s("note")?;
    if c.b("dry_run") {
        let n = cl.resolve_note(&note).await?;
        let reuse = c.b("use_existing");
        return Ok(json!({"dry_run": true, "note_id": n.id, "title": n.title,
                         "ai_job": !reuse, "insert": !c.b("no_insert"),
                         "at": if c.b("at_start") { "start" } else { "end" }}));
    }
    let t = Duration::from_secs_f64(c.of("timeout").unwrap_or(240.0).max(10.0));
    let reuse = c.b("use_existing");
    let pdf_text = if reuse {
        None
    } else {
        let n = cl.resolve_note(&note).await?;
        pdf_text_of(&cl, &n.id).await?
    };
    let mut r = cl.finish_notes(&note, reuse, pdf_text, t).await?;
    if !c.b("no_insert") {
        let id = r["note_id"].as_str().unwrap_or_default().to_string();
        let md = r["markdown"].as_str().unwrap_or_default().to_string();
        if md.trim().is_empty() {
            return Err(NbError::server("the AI write-up was empty"));
        }
        let mut s = nb_domain::EditSession::open(&cl, &id).await?;
        r["inserted"] = s
            .add_text(&format!("{}\n", md.trim_end()), c.b("at_start"))
            .await?;
    }
    Ok(r)
});

fn stage(c: &Ctx) -> String {
    c.os("stage").unwrap_or_else(|| "practice".into())
}

handler!(session_start, |c| {
    let cl = c.client()?;
    cl.session_start(&c.s("note")?, &stage(&c)).await
});

handler!(session_answer, |c| {
    let cl = c.client()?;
    let known = match (c.b("knew"), c.b("missed")) {
        (true, true) => return Err(NbError::usage("pass --knew or --missed, not both")),
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    };
    cl.session_answer(&c.s("note")?, &stage(&c), c.os("answer").as_deref(), known)
        .await
});

handler!(session_end, |c| {
    let cl = c.client()?;
    cl.session_end(&c.s("note")?, &stage(&c)).await
});

handler!(discuss_start, |c| {
    let cl = c.client()?;
    cl.discuss_start(&c.s("note")?).await
});

handler!(discuss_reply, |c| {
    let cl = c.client()?;
    cl.discuss_reply(
        &c.s("note")?,
        c.os("topic").as_deref(),
        c.os("choice").as_deref(),
        c.os("say").as_deref(),
    )
    .await
});

handler!(discuss_end, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    Ok(json!({"note_id": n.id, "cleared": study::discuss_clear(&n.id)}))
});

handler!(syllabus, |c| {
    let cl = c.client()?;
    let (data, name) = media::read_file(&c.s("file")?)?;
    let sizes = super::imports::pdf_sizes(&data, &name)?;
    let mut parsed = cl.syllabus_parse(&data).await?;
    if c.b("no_import") {
        return Ok(parsed);
    }
    let course = c
        .os("name")
        .or_else(|| parsed["course"]["name"].as_str().map(String::from));
    let course = course
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| media::stem_of(&name), str::to_string);
    let folder = cl
        .create_folder(&course, c.os("parent").as_deref(), DEFAULT_COLOR)
        .await?;
    let fid = folder["id"].as_str().unwrap_or_default().to_string();
    let note = media::import_pdf_new(&cl, &data, &name, &sizes, None, Some(&fid), false).await?;
    parsed["folder"] = json!({"id": fid, "title": course});
    parsed["note"] = json!({"id": note["note_id"], "title": note["title"]});
    Ok(parsed)
});
