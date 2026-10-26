//! Learn features beyond plain study batches: TOC quizzes, "Finish my notes", guided study
//! sessions, Brain Break discussions and syllabus import.

use std::path::PathBuf;
use std::time::Duration;

use nb_api::{NbError, Result};
use serde_json::{Value, json};

use crate::client::Client;
use crate::export::summary_text;
use crate::services::shape;
use crate::state::NoteState;

// ------------------------------------------------------------------ TOC quiz

/// One top-level outline section with its 1-based inclusive PDF page range.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub index: usize,
    pub title: String,
    pub start: usize,
    pub end: usize,
}

impl Section {
    pub fn to_json(&self) -> Value {
        json!({"index": self.index, "title": self.title, "start_page": self.start, "end_page": self.end})
    }
}

/// Top-level outline items with resolved pages; each runs until the next item (the last one
/// to the end of the document), as the web app's TOC quiz picker does.
/// `items` are `(level, title, page)` outline entries, depth first, level 1 = top.
pub fn toc_sections(items: &[(usize, String, Option<usize>)], pages: usize) -> Vec<Section> {
    let mut top: Vec<(String, usize)> = items
        .iter()
        .filter(|o| o.0 == 1)
        .filter_map(|o| o.2.map(|p| (o.1.trim().to_string(), p)))
        .collect();
    top.sort_by_key(|(_, p)| *p);
    let mut out = Vec::new();
    for (i, (title, start)) in top.iter().enumerate() {
        let next = top.get(i + 1).map_or(pages + 1, |(_, p)| *p);
        let end = next.saturating_sub(1).max(*start).min(pages.max(*start));
        out.push(Section {
            index: i + 1,
            title: title.clone(),
            start: *start,
            end,
        });
    }
    out
}

/// Pick sections by 1-based index or case-insensitive title substring; none ⇒ the first three.
pub fn select_sections(secs: &[Section], sel: &[String]) -> Result<Vec<Section>> {
    if secs.is_empty() {
        return Err(NbError::usage(
            "the note's PDFs have no table of contents (outline) to quiz on",
        ));
    }
    if sel.is_empty() {
        return Ok(secs.iter().take(3).cloned().collect());
    }
    let mut out: Vec<Section> = Vec::new();
    for s in sel {
        let hit = match s.trim().parse::<usize>() {
            Ok(i) => secs.iter().find(|x| x.index == i),
            Err(_) => {
                let l = s.to_lowercase();
                secs.iter().find(|x| x.title.to_lowercase().contains(&l))
            }
        }
        .ok_or_else(|| NbError::usage(format!("no TOC section matches '{s}'")))?;
        if !out.iter().any(|x| x.index == hit.index) {
            out.push(hit.clone());
        }
    }
    out.sort_by_key(|x| x.index);
    Ok(out)
}

/// The text the TOC quiz sends: each chosen section's title and page text.
pub fn section_text(secs: &[Section], page_texts: &[String]) -> String {
    let mut out = String::new();
    for s in secs {
        out.push_str(&format!("# {}\n\n", s.title));
        for p in s.start..=s.end {
            if let Some(t) = page_texts.get(p - 1) {
                out.push_str(t.trim());
                out.push_str("\n\n");
            }
        }
    }
    out.trim().to_string()
}

/// The sha512 of every PDF a note's pages use, in page order.
pub fn pdf_hashes(st: &NoteState) -> Vec<(String, String)> {
    let note_bg = st.note_background();
    let mut out: Vec<(String, String)> = Vec::new();
    for p in st.pages() {
        let bg = p.background.clone().unwrap_or_else(|| note_bg.clone());
        let m = &bg["pdf"]["metadata"];
        if let Some(sha) = m["assetHash"].as_str()
            && !out.iter().any(|(s, _)| s == sha)
        {
            out.push((
                sha.to_string(),
                m["fileName"].as_str().unwrap_or("document.pdf").to_string(),
            ));
        }
    }
    out
}

// --------------------------------------------------------------- study session

/// The three "Start studying" stages in order, with the quiz mode each one runs.
pub const STAGES: [(&str, &str); 3] = [
    ("memorize", "PRACTICE_FLASHCARDS"),
    ("practice", "PRACTICE_MULTIPLE_CHOICE"),
    ("test", "EXAM"),
];

pub fn stage_mode(stage: &str) -> Result<&'static str> {
    STAGES
        .iter()
        .find(|(s, _)| s.eq_ignore_ascii_case(stage))
        .map(|(_, m)| *m)
        .ok_or_else(|| NbError::usage("--stage must be memorize, practice or test"))
}

pub fn next_stage(stage: &str) -> Option<&'static str> {
    let i = STAGES
        .iter()
        .position(|(s, _)| s.eq_ignore_ascii_case(stage))?;
    STAGES.get(i + 1).map(|(s, _)| *s)
}

fn questions(active: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for q in active["quizQuestions"].as_array().into_iter().flatten() {
        if q["__typename"] == "MixAndMatch" {
            out.extend(q["flashcards"].as_array().cloned().unwrap_or_default());
        } else {
            out.push(q.clone());
        }
    }
    out
}

fn unanswered(q: &Value) -> bool {
    q["__typename"] != "QuestionPromise"
        && matches!(q["status"].as_str(), None | Some("UNANSWERED"))
}

/// A question as shown to the learner: the prompt and options, never the answer.
pub fn public_question(q: &Value) -> Value {
    let mut o = json!({"index": q["questionIndex"], "id": q["questionId"],
                       "type": q["__typename"]});
    match q["__typename"].as_str() {
        Some("MultipleChoice") => {
            o["question"] = q["question"].clone();
            o["choices"] = q["answers"].clone();
        }
        Some("TrueOrFalse") => {
            o["statement"] = q["statement"].clone();
            o["choices"] = json!(["true", "false"]);
        }
        Some("FillInTheBlank") => o["prompt"] = q["definition"].clone(),
        _ => o["term"] = q["term"].clone(),
    }
    o
}

/// The session view: progress plus the next unanswered question (without its answer).
pub fn session_view(note_id: &str, stage: &str, res: &Value) -> Value {
    let act = &res["active"];
    let qs = questions(act);
    let next = qs.iter().find(|q| unanswered(q));
    let answered = qs
        .iter()
        .filter(|q| q["__typename"] != "QuestionPromise" && !unanswered(q))
        .count();
    json!({"note_id": note_id, "stage": stage, "session_id": act["id"], "mode": act["mode"],
           "total": qs.len(), "answered": answered,
           "pending_questions": res["pending_questions"],
           "question": next.map(public_question),
           "done": next.is_none() && res["pending_questions"].as_u64().unwrap_or(0) == 0})
}

/// Grade like the web app: multiple choice against `correctAnswer`, true/false against the
/// statement's answer, fill-in-the-blank against the term, flashcards by self-assessment.
/// Returns `(status, correct_answer, choice, fill)`.
pub fn grade(
    q: &Value,
    answer: Option<&str>,
    known: Option<bool>,
) -> Result<(String, Value, Option<String>, Option<String>)> {
    let need = || NbError::usage("pass --answer for this question");
    match q["__typename"].as_str() {
        Some("MultipleChoice") => {
            let a = answer.ok_or_else(need)?.trim().to_uppercase();
            if a.len() != 1 || !"ABCD".contains(&a) {
                return Err(NbError::usage("--answer must be A, B, C or D"));
            }
            let correct = q["correctAnswer"]
                .as_str()
                .unwrap_or_default()
                .to_uppercase();
            let st = if a == correct { "CORRECT" } else { "INCORRECT" };
            Ok((st.into(), json!(correct), Some(a), None))
        }
        Some("TrueOrFalse") => {
            let a = answer.ok_or_else(need)?.trim().to_lowercase();
            let v = match a.as_str() {
                "true" | "t" | "yes" => true,
                "false" | "f" | "no" => false,
                _ => return Err(NbError::usage("--answer must be true or false")),
            };
            let c = &q["trueOrFalseCorrectAnswer"];
            let correct = c
                .as_bool()
                .or_else(|| c.as_str().map(|s| s.eq_ignore_ascii_case("true")))
                .unwrap_or(false);
            let st = if v == correct { "CORRECT" } else { "INCORRECT" };
            Ok((st.into(), json!(correct), None, None))
        }
        Some("FillInTheBlank") => {
            let a = answer.ok_or_else(need)?.trim().to_string();
            let term = q["term"].as_str().unwrap_or_default();
            let st = if a.eq_ignore_ascii_case(term.trim()) {
                "CORRECT"
            } else {
                "INCORRECT"
            };
            Ok((st.into(), json!(term), None, Some(a)))
        }
        _ => {
            let k =
                known.ok_or_else(|| NbError::usage("pass --known or --again for a flashcard"))?;
            let st = if k { "CORRECT" } else { "INCORRECT" };
            Ok((st.into(), q["definition"].clone(), None, None))
        }
    }
}

// ------------------------------------------------------------------ discussion

fn discuss_dir() -> PathBuf {
    crate::recorder::state_dir().join("discussions")
}

fn discuss_path(note_id: &str) -> PathBuf {
    discuss_dir().join(format!("{note_id}.json"))
}

pub fn discuss_load(note_id: &str) -> Result<Value> {
    let p = discuss_path(note_id);
    let raw = std::fs::read(&p).map_err(|_| {
        NbError::usage("no Brain Break discussion for this note: run `learn discuss start` first")
    })?;
    serde_json::from_slice(&raw).map_err(|e| NbError::generic(format!("{}: {e}", p.display())))
}

pub fn discuss_save(note_id: &str, v: &Value) -> Result<()> {
    let d = discuss_dir();
    std::fs::create_dir_all(&d).map_err(|e| NbError::generic(format!("{}: {e}", d.display())))?;
    std::fs::write(
        discuss_path(note_id),
        serde_json::to_vec_pretty(v).unwrap_or_default(),
    )
    .map_err(|e| NbError::generic(format!("{}: {e}", d.display())))
}

pub fn discuss_clear(note_id: &str) -> bool {
    std::fs::remove_file(discuss_path(note_id)).is_ok()
}

/// Apply a learner turn to the local discussion state; returns the dialogue request params.
/// `choice` is a choice id from the last dialogue node, `say` a free-form reply.
pub fn discuss_turn(
    state: &mut Value,
    note_text: &str,
    topic: Option<&str>,
    choice: Option<&str>,
    say: Option<&str>,
) -> Result<Value> {
    if let Some(t) = topic {
        if !state["topics"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|x| x["id"] == t)
        {
            return Err(NbError::usage(format!("no discussion topic with id '{t}'")));
        }
        state["active_topic"] = json!(t);
    }
    let mut new_concepts: Vec<Value> = Vec::new();
    let mut just_completed = false;
    if let Some(cid) = choice {
        let ch = state["node"]["choices"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c["id"] == cid)
            .cloned()
            .ok_or_else(|| {
                NbError::usage(format!("no choice with id '{cid}' in the last reply"))
            })?;
        push_history(state, "student", ch["text"].as_str().unwrap_or_default());
        if let Some(t) = ch["topicId"].as_str() {
            state["active_topic"] = json!(t);
        }
        new_concepts = ch["conceptsCovered"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if let Some(active) = state["active_topic"].as_str().map(str::to_string)
            && !new_concepts.is_empty()
            && let Some(top) = state["topics"]
                .as_array_mut()
                .and_then(|a| a.iter_mut().find(|x| x["id"] == active.as_str()))
        {
            let mut covered: Vec<Value> = top["conceptsCovered"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for c in &new_concepts {
                if !covered.contains(c) {
                    covered.push(c.clone());
                }
            }
            just_completed = covered.len() >= top["concepts"].as_array().map_or(0, Vec::len);
            top["conceptsCovered"] = json!(covered);
        }
    } else if let Some(s) = say {
        push_history(state, "student", s);
    }
    let topics: Vec<Value> = state["topics"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| json!({"id": t["id"], "title": t["title"], "concepts": t["concepts"],
                        "conceptsCovered": t["conceptsCovered"].as_array().cloned().unwrap_or_default()}))
        .collect();
    let hist: Vec<Value> = state["history"].as_array().cloned().unwrap_or_default();
    let tail = hist[hist.len().saturating_sub(100)..].to_vec();
    let mut p = json!({"noteContent": note_text, "conversationHistory": tail, "topics": topics});
    if let Some(t) = state["active_topic"].as_str() {
        p["activeTopicId"] = json!(t);
    }
    if choice.is_some() {
        p["justCompletedTopic"] = json!(just_completed);
        if !new_concepts.is_empty() {
            p["newConceptsBeingCovered"] = json!(new_concepts);
        }
    }
    Ok(p)
}

pub fn push_history(state: &mut Value, speaker: &str, text: &str) {
    if !state["history"].is_array() {
        state["history"] = json!([]);
    }
    if let Some(h) = state["history"].as_array_mut() {
        h.push(json!({"speaker": speaker, "text": text}));
        if h.len() > 100 {
            let cut = h.len() - 100;
            h.drain(..cut);
        }
    }
}

const M_START_DISCUSSION: &str = "mutation startDiscussion($params: StartDiscussionParams!) {
  startDiscussion(params: $params) { title introText topics { id title concepts } } }";
const M_DIALOGUE: &str =
    "mutation generateDiscussionDialogue($params: GenerateDiscussionDialogueParams!) {
  generateDiscussionDialogue(params: $params) { expressionMood text writeInPrompt
    choices { id text preResponse preResponseMood scriptAction topicId conceptsCovered } } }";

// -------------------------------------------------------------------- syllabus

/// `YYYY-MM-DD` today in local time and the IANA zone name (`TZ`, `/etc/timezone` or the
/// `/etc/localtime` link; `UTC` when none is set).
pub fn local_date_zone() -> (String, String) {
    let date = std::process::Command::new("date")
        .arg("+%F")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| s.len() == 10)
        .unwrap_or_else(|| "1970-01-01".into());
    let zone = std::env::var("TZ")
        .ok()
        .map(|z| z.trim_start_matches(':').to_string())
        .filter(|z| z.contains('/'))
        .or_else(|| {
            std::fs::read_to_string("/etc/timezone")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            std::fs::read_link("/etc/localtime").ok().and_then(|p| {
                let s = p.display().to_string();
                s.split_once("zoneinfo/").map(|(_, z)| z.to_string())
            })
        })
        .unwrap_or_else(|| "UTC".into());
    (date, zone)
}

pub const SYLLABUS_MAX: usize = 14 * 1024 * 1024;

// ---------------------------------------------------------------------- client

impl Client {
    /// The PDFs a note uses: `(sha512, file name)` in page order.
    pub async fn note_pdfs(&self, note_id: &str) -> Result<Vec<(String, String)>> {
        let b = self.http.note_bundle(note_id).await?;
        Ok(pdf_hashes(&NoteState::new(&b)))
    }

    /// The study batch's summary-of-summaries markdown when it is COMPLETED.
    pub async fn ready_summary(&self, note_id: &str) -> Result<Option<(String, String)>> {
        let Some(sm) = self.http.study_materials(note_id).await? else {
            return Ok(None);
        };
        let s = &sm["summaryOfSummaries"];
        if s["status"].as_str().map(str::to_uppercase).as_deref() != Some("COMPLETED") {
            return Ok(None);
        }
        let id = s["id"].as_str().unwrap_or_default().to_string();
        let content = self.http.summary_content(&id).await?;
        Ok(Some((id, summary_text(&content))))
    }

    /// "Finish my notes": a summary batch over the note's typed text and transcripts, waited
    /// for (up to `timeout`). `use_existing` reuses a COMPLETED summary without a new job.
    /// `pdf_text` is the note's extracted PDF text (the caller parses the PDFs), sent like the
    /// web app as a `NOTE_PDF` material. Recordings count only once their transcript is ready.
    pub async fn finish_notes(
        &self,
        r: &str,
        use_existing: bool,
        pdf_text: Option<String>,
        timeout: Duration,
    ) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let rec = crate::recorder::status();
        if rec["running"] == true && rec["note_id"] == n.id.as_str() {
            return Err(NbError::usage(
                "a recording is running on this note; stop it before finishing the notes",
            ));
        }
        if use_existing {
            return match self.ready_summary(&n.id).await? {
                Some((id, md)) => Ok(json!({"note_id": n.id, "title": n.title, "summary_job": id,
                                            "reused": true, "markdown": md})),
                None => Err(NbError::usage(
                    "no finished summary to reuse; run without --use-existing",
                )),
            };
        }
        let text = crate::search::note_text(self, &n.id)
            .await
            .unwrap_or_default();
        let b = self.http.note_bundle(&n.id).await?;
        let st = NoteState::new(&b);
        let mut hashes = Vec::new();
        let mut untranscribed = Vec::new();
        for r in crate::recordings::recordings_of(&st) {
            let Some(sha) = r["sha512"].as_str() else {
                continue;
            };
            let job = self.http.transcript(sha).await.unwrap_or_default();
            if job["job"]["status"]
                .as_str()
                .map(str::to_uppercase)
                .as_deref()
                == Some("COMPLETED")
            {
                hashes.push(sha.to_string());
            } else {
                untranscribed.push(r["id"].clone());
            }
        }
        let pdf_text = pdf_text.filter(|t| !t.trim().is_empty());
        let chars = text.trim().chars().count()
            + pdf_text.as_deref().map_or(0, |t| t.trim().chars().count());
        if chars < 20 && hashes.is_empty() {
            let hint = if untranscribed.is_empty() {
                "add typed notes, a PDF or a recording first"
            } else {
                "the recordings have no transcript yet (`audio upload --transcribe`)"
            };
            return Err(NbError::usage(format!(
                "not enough content to finish: {hint}"
            )));
        }
        let mut materials = Vec::new();
        if !text.trim().is_empty() {
            materials.push(json!({"content": text, "type": "NOTE_TEXT"}));
        }
        if let Some(t) = &pdf_text {
            materials.push(json!({"content": t, "type": "NOTE_PDF"}));
        }
        let batch = self
            .learn_generate_with(&n.id, materials, hashes.clone(), false, false, None)
            .await?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if let Some(sm) = self.http.study_materials(&n.id).await? {
                let s = &sm["summaryOfSummaries"];
                match s["status"].as_str().map(str::to_uppercase).as_deref() {
                    Some("COMPLETED") => {
                        let id = s["id"].as_str().unwrap_or_default().to_string();
                        let md = summary_text(&self.http.summary_content(&id).await?);
                        return Ok(json!({"note_id": n.id, "title": n.title, "summary_job": id,
                                         "batch": batch["id"], "transcripts": hashes.len(),
                                         "untranscribed_recordings": untranscribed,
                                         "reused": false, "markdown": md}));
                    }
                    Some("FAILED" | "ERROR") => {
                        let why = s["errorReason"].as_str().unwrap_or("unknown").to_string();
                        return Err(finish_error(&why));
                    }
                    _ => {}
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(NbError::server(
                    "finishing the notes timed out; rerun later with --use-existing",
                ));
            }
        }
    }

    /// Start or resume a guided study session stage.
    pub async fn session_start(&self, r: &str, stage: &str) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let mode = stage_mode(stage)?;
        if self.http.study_materials(&n.id).await?.is_none() {
            return Err(NbError::usage(
                "the note has no study materials yet: run `learn generate` (or `learn toc`) first",
            ));
        }
        let res = self
            .sync_quiz(json!({"quizTypeParams": {"noteId": n.id, "mode": mode}}))
            .await?;
        Ok(session_view(&n.id, stage, &shape(&n.id, &res)))
    }

    /// Grade one answer to the current question and send the completion.
    pub async fn session_answer(
        &self,
        r: &str,
        stage: &str,
        answer: Option<&str>,
        known: Option<bool>,
    ) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let mode = stage_mode(stage)?;
        let cur = shape(
            &n.id,
            &self
                .sync_quiz(json!({"quizTypeParams": {"noteId": n.id, "mode": mode}}))
                .await?,
        );
        let qs = questions(&cur["active"]);
        let q = qs.iter().find(|q| unanswered(q)).cloned().ok_or_else(|| {
            NbError::usage("no unanswered question left: run `learn session end`")
        })?;
        let (status, correct, choice, fill) = grade(&q, answer, known)?;
        let idx = q["questionIndex"].as_i64().unwrap_or(0);
        let comp =
            crate::services::completion(idx, &status, choice.as_deref(), fill.as_deref(), None)?;
        let sid = cur["active"]["id"].as_str().unwrap_or_default().to_string();
        let res = self
            .sync_quiz(json!({"quizTypeParams": {"noteId": n.id, "mode": mode},
                              "quizSessionUpdateParams": {"quizSessionId": sid, "quizQuestionCompletions": [comp]}}))
            .await?;
        let view = session_view(&n.id, stage, &shape(&n.id, &res));
        let mut out = json!({"correct": status == "CORRECT", "status": status,
                             "correct_answer": correct, "answered": public_question(&q), "next": view});
        if view["done"] == true {
            out["next_stage"] = json!(next_stage(stage));
        }
        Ok(out)
    }

    /// Mark the stage's session completed; returns the score.
    pub async fn session_end(&self, r: &str, stage: &str) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let mode = stage_mode(stage)?;
        let cur = shape(
            &n.id,
            &self
                .sync_quiz(json!({"quizTypeParams": {"noteId": n.id, "mode": mode}}))
                .await?,
        );
        let sid = cur["active"]["id"].as_str().unwrap_or_default().to_string();
        if sid.is_empty() {
            return Err(NbError::usage("no active session for this stage"));
        }
        let qs = questions(&cur["active"]);
        let correct = qs.iter().filter(|q| q["status"] == "CORRECT").count();
        let answered = qs
            .iter()
            .filter(|q| q["__typename"] != "QuestionPromise" && !unanswered(q))
            .count();
        let res = self
            .sync_quiz(json!({"quizTypeParams": {"noteId": n.id, "mode": mode},
                              "quizSessionUpdateParams": {"quizSessionId": sid, "quizQuestionCompletions": [], "isCompleted": true}}))
            .await?;
        Ok(
            json!({"note_id": n.id, "stage": stage, "session_id": sid, "completed": true,
                  "answered": answered, "correct": correct, "total": qs.len(),
                  "next_stage": next_stage(stage),
                  "last_completed": res.get("lastCompletedQuizSession").cloned().unwrap_or(Value::Null)}),
        )
    }

    /// Start a Brain Break discussion (AI): topics and the opening line, saved locally.
    pub async fn discuss_start(&self, r: &str) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let text = crate::search::note_text(self, &n.id)
            .await
            .unwrap_or_default();
        if text.trim().is_empty() {
            return Err(NbError::usage("the note has no typed text to discuss"));
        }
        let d = self
            .gql(
                M_START_DISCUSSION,
                json!({"params": {"noteText": text}}),
                "startDiscussion",
            )
            .await?["startDiscussion"]
            .take();
        if d.is_null() {
            return Err(NbError::server("no discussion returned for this note"));
        }
        let topics: Vec<Value> = d["topics"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| json!({"id": t["id"], "title": t["title"], "concepts": t["concepts"], "conceptsCovered": []}))
            .collect();
        let mut state =
            json!({"note_id": n.id, "title": d["title"], "topics": topics, "history": []});
        push_history(
            &mut state,
            "sage",
            d["introText"].as_str().unwrap_or_default(),
        );
        discuss_save(&n.id, &state)?;
        Ok(
            json!({"note_id": n.id, "title": d["title"], "intro": d["introText"], "topics": d["topics"]}),
        )
    }

    /// One Brain Break turn (AI): pick a topic, a choice from the last reply, or say something.
    pub async fn discuss_reply(
        &self,
        r: &str,
        topic: Option<&str>,
        choice: Option<&str>,
        say: Option<&str>,
    ) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let mut state = discuss_load(&n.id)?;
        let text = crate::search::note_text(self, &n.id)
            .await
            .unwrap_or_default();
        let params = discuss_turn(&mut state, &text, topic, choice, say)?;
        if params.get("activeTopicId").is_none() {
            return Err(NbError::usage("pick a topic first with --topic ID"));
        }
        let node = self
            .gql(
                M_DIALOGUE,
                json!({"params": params}),
                "generateDiscussionDialogue",
            )
            .await?["generateDiscussionDialogue"]
            .take();
        push_history(
            &mut state,
            "sage",
            node["text"].as_str().unwrap_or_default(),
        );
        state["node"] = node.clone();
        discuss_save(&n.id, &state)?;
        Ok(json!({"note_id": n.id, "topic": state["active_topic"], "reply": node}))
    }

    /// Parse a syllabus PDF (AI): course name, meeting times and dated events.
    pub async fn syllabus_parse(&self, data: &[u8]) -> Result<Value> {
        if data.len() > SYLLABUS_MAX {
            return Err(NbError::usage("syllabus PDFs are limited to 14 MB"));
        }
        if !data.starts_with(b"%PDF") {
            return Err(NbError::usage("the syllabus must be a PDF"));
        }
        let (date, zone) = local_date_zone();
        let d = nb_api::upload::digest(data);
        let job = self
            .http
            .syllabus_job(json!({"md5Hash": d.md5_b64, "sizeInBytes": d.size,
                                 "referenceDate": date, "timezone": zone, "locale": "en"}))
            .await?;
        let job_id = job["jobId"].as_str().unwrap_or_default().to_string();
        let url = job["uploadUrl"].as_str().unwrap_or_default().to_string();
        if job_id.is_empty() || url.is_empty() {
            return Err(NbError::server("syllabus job reply had no jobId/uploadUrl"));
        }
        let headers: Vec<(String, String)> = job["requiredHeaders"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let hs: Vec<(&str, String)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let up = self.http.put_external(&url, &hs, data.to_vec()).await?;
        if up.status >= 300 {
            return Err(NbError::server(format!(
                "syllabus upload rejected: HTTP {}",
                up.status
            )));
        }
        let mut res = self.http.syllabus_await(&job_id).await?;
        res["job_id"] = json!(job_id);
        Ok(res)
    }
}

fn finish_error(reason: &str) -> NbError {
    let r = reason.to_uppercase();
    if [
        "INPUT_TOO_SHORT",
        "INSUFFICIENT_CONTEXT",
        "TRANSCRIPT_DOES_NOT_EXIST",
    ]
    .iter()
    .any(|k| r.contains(k))
    {
        NbError::usage(format!("not enough content to finish the notes ({reason})"))
    } else if r.contains("QUOTA") {
        NbError::quota(format!("AI quota exceeded ({reason})"))
    } else if r.contains("SUBSCRIB") {
        NbError::quota(format!("a subscription is required ({reason})"))
    } else {
        NbError::server(format!("finishing the notes failed ({reason})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(level: usize, title: &str, page: Option<usize>) -> (usize, String, Option<usize>) {
        (level, title.into(), page)
    }

    #[test]
    fn sections_split_on_top_level() {
        let items = [
            item(1, "Intro", Some(1)),
            item(2, "Sub", Some(2)),
            item(1, "Methods", Some(4)),
            item(1, "Results", Some(7)),
            item(1, "Broken", None),
        ];
        let s = toc_sections(&items, 9);
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].start, s[0].end), (1, 3));
        assert_eq!((s[1].start, s[1].end), (4, 6));
        assert_eq!((s[2].start, s[2].end), (7, 9));
        let pick = select_sections(&s, &[]).unwrap();
        assert_eq!(pick.len(), 3);
        let pick = select_sections(&s, &["results".into(), "1".into()]).unwrap();
        assert_eq!(pick.iter().map(|x| x.index).collect::<Vec<_>>(), [1, 3]);
        assert!(select_sections(&s, &["nope".into()]).is_err());
        assert!(select_sections(&[], &[]).is_err());
        let t = section_text(
            &pick[..1],
            &["a".into(), "b".into(), "c".into(), "d".into()],
        );
        assert_eq!(t, "# Intro\n\na\n\nb\n\nc");
    }

    #[test]
    fn grades_like_the_web_app() {
        let mc = json!({"__typename": "MultipleChoice", "questionIndex": 0, "correctAnswer": "B",
                        "question": "q", "answers": {"A": "x", "B": "y"}});
        assert_eq!(grade(&mc, Some("b"), None).unwrap().0, "CORRECT");
        assert_eq!(grade(&mc, Some("A"), None).unwrap().0, "INCORRECT");
        assert!(grade(&mc, Some("E"), None).is_err());
        assert!(public_question(&mc).get("correctAnswer").is_none());
        let tf = json!({"__typename": "TrueOrFalse", "trueOrFalseCorrectAnswer": false});
        assert_eq!(grade(&tf, Some("false"), None).unwrap().0, "CORRECT");
        let fc = json!({"__typename": "Flashcard", "term": "t", "definition": "d"});
        assert_eq!(grade(&fc, None, Some(true)).unwrap().0, "CORRECT");
        assert!(grade(&fc, None, None).is_err());
        let fb = json!({"__typename": "FillInTheBlank", "term": "Mitosis", "definition": "___ is"});
        assert_eq!(grade(&fb, Some("mitosis"), None).unwrap().0, "CORRECT");
    }

    #[test]
    fn session_view_hides_answers_and_tracks_progress() {
        let res = json!({"pending_questions": 0, "active": {"id": "s1", "mode": "EXAM", "quizQuestions": [
            {"__typename": "MultipleChoice", "questionIndex": 0, "status": "CORRECT", "correctAnswer": "A"},
            {"__typename": "MultipleChoice", "questionIndex": 1, "status": "UNANSWERED", "correctAnswer": "C",
             "question": "q2", "answers": {"A": "1"}}]}});
        let v = session_view("n", "test", &res);
        assert_eq!(v["answered"], 1);
        assert_eq!(v["question"]["index"], 1);
        assert!(!v.to_string().contains("\"C\""));
        assert_eq!(v["done"], false);
        assert_eq!(stage_mode("Practice").unwrap(), "PRACTICE_MULTIPLE_CHOICE");
        assert_eq!(next_stage("memorize"), Some("practice"));
        assert_eq!(next_stage("test"), None);
    }

    #[test]
    fn discussion_turns_track_concepts() {
        let mut st = json!({"topics": [{"id": "t1", "title": "T", "concepts": ["a", "b"], "conceptsCovered": []}],
                            "history": [{"speaker": "sage", "text": "hi"}]});
        let p = discuss_turn(&mut st, "note", Some("t1"), None, None).unwrap();
        assert_eq!(p["activeTopicId"], "t1");
        assert!(p.get("justCompletedTopic").is_none());
        st["node"] = json!({"choices": [{"id": "c1", "text": "yes", "conceptsCovered": ["a", "b"], "topicId": "t1"}]});
        let p = discuss_turn(&mut st, "note", None, Some("c1"), None).unwrap();
        assert_eq!(p["justCompletedTopic"], true);
        assert_eq!(
            p["conversationHistory"][1],
            json!({"speaker": "student", "text": "yes"})
        );
        assert!(discuss_turn(&mut st, "note", Some("zz"), None, None).is_err());
        assert!(discuss_turn(&mut st, "note", None, Some("zz"), None).is_err());
    }

    #[test]
    fn finish_errors_map() {
        assert_eq!(finish_error("INPUT_TOO_SHORT").kind, nb_api::Kind::Usage);
        assert_eq!(finish_error("JOB_OBSOLETE").kind, nb_api::Kind::Server);
    }
}
