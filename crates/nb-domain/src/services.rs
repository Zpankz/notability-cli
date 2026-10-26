//! Sharing, settings, account panels and Learn (GraphQL).

use std::collections::BTreeMap;
use std::time::Duration;

use nb_api::{NbError, Result};
use serde_json::{Map, Value, json};

use crate::client::{Client, merge, now_ms};
use crate::models::ms_to_iso;

// ----------------------------------------------------------------- sharing
const Q_PERMS: &str = "query getAllPermissionedUsersForNote($allPermissionedUsersForNoteRequest: AllPermissionedUsersForNoteRequest!) {
  getAllPermissionedUsersForNote(allPermissionedUsersForNoteRequest: $allPermissionedUsersForNoteRequest) {
    permissionedUsers { accessLevel email name userId }
    permissionRequests { requestId userId email name accessLevel state }
  }
}";
const M_INVITE: &str = "mutation addNotePermissionsByEmail($addNotePermissionsByEmailRequest: AddNotePermissionsByEmailRequest!) {
  addNotePermissionsByEmail(addNotePermissionsByEmailRequest: $addNotePermissionsByEmailRequest) {
    created { email permissionLevel userId name }
    failed { email reason }
  }
}";
const M_GRANT: &str = "mutation grantNoteAccess($grantNoteAccessInput: GrantNoteAccessInput!) {
  grantNoteAccess(grantNoteAccessInput: $grantNoteAccessInput)
}";
const M_DENY: &str = "mutation denyNoteAccess($denyNoteAccessInput: DenyNoteAccessInput!) {
  denyNoteAccess(denyNoteAccessInput: $denyNoteAccessInput)
}";

pub fn norm(v: &str, allowed: &[&str], what: &str) -> Result<String> {
    let u = match v.to_uppercase().as_str() {
        "VIEW" => "VIEWER".to_string(),
        "EDIT" => "EDITOR".to_string(),
        s => s.to_string(),
    };
    if allowed.contains(&u.as_str()) {
        return Ok(u);
    }
    let names: Vec<String> = allowed.iter().map(|a| a.to_lowercase()).collect();
    Err(NbError::usage(format!(
        "invalid {what} '{v}': choose one of {}",
        names.join(", ")
    )))
}

const ACCESS: [&str; 2] = ["VIEWER", "EDITOR"];
const SCOPES: [&str; 2] = ["RESTRICTED", "PUBLIC"];

// ---------------------------------------------------------------- settings
const TOGGLES: [(&str, &str); 2] = [
    ("learn", "learnEnabled"),
    ("transcription", "audioTranscriptionEnabled"),
];
const Q_SETTINGS: &str = "query getMe { me { id isAiEnabled settings { appSettings } } }";
const M_SET: &str = "mutation setAppSetting($setAppSettingParams: SetAppSettingParams!) {
  setAppSetting(setAppSettingParams: $setAppSettingParams) { success errorCode appSettings }
}";
const M_AI: &str =
    "mutation updateLearnSettings($updateLearnSettingsParams: UpdateLearnSettingsParams!) {
  updateLearnSettings(updateLearnSettingsParams: $updateLearnSettingsParams)
}";
const M_NAME: &str = "mutation updateFullName($fullNameParams: FullNameParams!) { updateFullName(fullNameParams: $fullNameParams) }";

fn parse_json_str(v: Value) -> Value {
    match v {
        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        v => v,
    }
}

fn as_map(v: Value) -> Value {
    match parse_json_str(v) {
        Value::Object(m) => Value::Object(m),
        _ => json!({}),
    }
}

// ----------------------------------------------------------------- account
const QF: &str = "isUsageExceeded usagePercentage quotaWindowType";
const Q_STICKERS: &str = "query getMyStickers($userCustomAssetRequest: UserCustomAssetRequest!) {
  me { customAssets(userCustomAssetRequest: $userCustomAssetRequest) { stickers { id name } } } }";
const Q_SURVEY: &str =
    "query getUserSurveyResponse { getUserSurveyResponse { userId surveyFields { key value } } }";
const Q_FLAGS: &str = "query getFeatureFlags { featureFlags: getFeatureFlagsForUser }";
const Q_SITES: &str = "query getSiteUserMappings($noteId: GqlUUID!) {
  getSiteUserMappings(siteUserMappingQueryParams: {noteId: $noteId}) { siteIds userId userName avatarUrl } }";

fn q_quotas() -> String {
    format!(
        "query getQuotas {{ me {{ subscriptionOverview {{ tier quotas {{
  learnQuestions {{ {QF} }} learnQuestionUploads {{ {QF} }} learnSummaries {{ {QF} }}
  liveTranscription {{ {QF} }} youtubeTranscripts {{ {QF} }} }} }} }} }}"
    )
}

pub fn author_for(names: &BTreeMap<i64, String>, comment_id: &str) -> String {
    let site: i64 = comment_id
        .split(':')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    names
        .get(&site)
        .cloned()
        .unwrap_or_else(|| format!("Anonymous {site}"))
}

// ------------------------------------------------------------------- learn
pub const MODES: [&str; 8] = [
    "EXAM",
    "EXAM_MULTIPLE_CHOICE",
    "PRACTICE_FILL_IN_THE_BLANK",
    "PRACTICE_FLASHCARDS",
    "PRACTICE_MIX_AND_MATCH",
    "PRACTICE_MULTIPLE_CHOICE",
    "PRACTICE_TRUE_OR_FALSE",
    "SPACED_REPETITION_FLASHCARDS",
];
pub const STATUSES: [&str; 5] = ["CORRECT", "INCORRECT", "SKIPPED", "FLAGGED", "UNANSWERED"];
pub const RATINGS: [(&str, &str); 4] = [
    ("again", "SKIP"),
    ("hard", "HARD"),
    ("good", "MEDIUM"),
    ("easy", "EASY"),
];

const SESSION: &str = "id noteId mode numQuestions numAnswered numCorrect numIncorrect numSkipped createdAt updatedAt completedAt";
const M_SYNC_BODY: &str = "
    activeQuizSession { id noteId mode numQuestions numAnswered createdAt updatedAt completedAt
      quizQuestions {
        ... on MultipleChoice { __typename questionId questionIndex type status question correctAnswer
                                userResponseMultipleChoice answers { A B C D } }
        ... on Flashcard { __typename questionId questionIndex type status term definition }
        ... on FillInTheBlank { __typename questionId questionIndex type status term definition userResponseFillInTheBlank }
        ... on MixAndMatch { __typename type flashcards { __typename questionId questionIndex type status term definition } }
        ... on TrueOrFalse { __typename questionId questionIndex type status statement trueOrFalseCorrectAnswer: correctAnswer }
        ... on QuestionPromise { __typename questionId quizJobId type }
      }
    }
  }
}";
const M_GENERATE: &str = "mutation generateStudyMaterialsBatch($generateStudyMaterialsBatchParams: GenerateStudyMaterialsBatchParams!) {
  generateStudyMaterialsBatch(generateStudyMaterialsBatchParams: $generateStudyMaterialsBatchParams) {
    id promptGroup language isServerManaged
    quizJobs { id type promptKey status errorReason }
    summaryOfSummaries { id status promptKey errorReason }
    takeawaysSummary { id status promptKey errorReason }
  }
}";
const Q_EXPLAIN: &str = "query getQuizQuestionExplanation($getQuizQuestionExplanationParams: GetQuizQuestionExplanationParams!) {
  getQuizQuestionExplanation(getQuizQuestionExplanationParams: $getQuizQuestionExplanationParams)
}";
const M_UPLOAD: &str =
    "mutation uploadUserGeneratedQuiz($uploadUserGeneratedQuiz: UploadUserGeneratedQuizParams!) {
  uploadUserGeneratedQuiz(uploadUserGeneratedQuiz: $uploadUserGeneratedQuiz)
}";

fn m_sync() -> String {
    format!(
        "mutation syncQuizSession($syncQuizSessionParams: SyncQuizSessionParams!) {{
  syncQuizSession(syncQuizSessionParams: $syncQuizSessionParams) {{
    lastCompletedQuizSession {{ {SESSION} }}{M_SYNC_BODY}"
    )
}

pub fn quiz_mode(m: &str) -> Result<String> {
    let u = m.to_uppercase().replace('-', "_");
    if MODES.contains(&u.as_str()) {
        return Ok(u);
    }
    let names: Vec<String> = MODES.iter().map(|x| x.to_lowercase()).collect();
    Err(NbError::usage(format!(
        "invalid quiz mode '{m}': choose one of {}",
        names.join(", ")
    )))
}

/// One question completion, validated before it is sent.
pub fn completion(
    index: i64,
    status: &str,
    choice: Option<&str>,
    fill: Option<&str>,
    rating: Option<&str>,
) -> Result<Value> {
    let st = status.to_uppercase();
    if !STATUSES.contains(&st.as_str()) {
        let names: Vec<String> = STATUSES.iter().map(|x| x.to_lowercase()).collect();
        return Err(NbError::usage(format!(
            "invalid status '{status}': choose one of {}",
            names.join(", ")
        )));
    }
    let mut c = json!({"questionIndex": index, "status": st});
    if let Some(ch) = choice {
        let u = ch.to_uppercase();
        if u.len() != 1 || !"ABCD".contains(&u) {
            return Err(NbError::usage("--choice must be A, B, C or D"));
        }
        c["userResponseMultipleChoice"] = json!(u);
    }
    if let Some(f) = fill {
        c["userResponseFillInTheBlank"] = json!(f);
    }
    if let Some(r) = rating {
        let lr = r.to_lowercase();
        let v = RATINGS
            .iter()
            .find(|(k, v)| *k == lr || *v == r.to_uppercase())
            .map(|(_, v)| *v)
            .ok_or_else(|| NbError::usage("--rating must be again, hard, good or easy"))?;
        c["userResponseFlashcard"] = json!(v);
        c["reviewDate"] = json!(iso_millis(now_ms()));
    }
    Ok(c)
}

fn iso_millis(ms: u64) -> String {
    let s = ms_to_iso(Some(ms as f64))
        .as_str()
        .unwrap_or_default()
        .to_string();
    match s.split_once('.') {
        Some((base, frac)) => format!("{base}.{}Z", &frac[..3]),
        None => s.replace('Z', ".000Z"),
    }
}

/// `term<sep>definition` lines (the web app's paste format).
pub fn parse_cards(text: &str, sep: &str) -> Result<Vec<Value>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (t, d) = l.split_once(sep).ok_or_else(|| {
                NbError::usage(format!(
                    "line without separator {sep:?}: {}",
                    l.chars().take(60).collect::<String>()
                ))
            })?;
            Ok(json!({"term": t.trim(), "definition": d.trim()}))
        })
        .collect()
}

fn shape(note_id: &str, r: &Value) -> Value {
    let act = r.get("activeQuizSession").cloned().unwrap_or(Value::Null);
    let qs = act
        .get("quizQuestions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let pending = qs
        .iter()
        .filter(|q| q["__typename"] == "QuestionPromise")
        .count();
    json!({"note_id": note_id, "active": act, "last_completed": r.get("lastCompletedQuizSession").cloned().unwrap_or(Value::Null),
           "questions": qs.len(), "pending_questions": pending})
}

impl Client {
    async fn gql(&self, q: &str, vars: Value, op: &str) -> Result<Value> {
        self.http.graphql(q, vars, Some(op)).await
    }

    fn link_settings(n: &crate::models::Note) -> Value {
        let s = n
            .extra
            .get("linkShareSettings")
            .cloned()
            .unwrap_or(Value::Null);
        json!({"scope": s.get("linkPermissionScope"), "access": s.get("linkAccessLevel")})
    }

    pub async fn share_status(&self, r: &str) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let d = self
            .gql(
                Q_PERMS,
                json!({"allPermissionedUsersForNoteRequest": {"noteId": n.id}}),
                "getAllPermissionedUsersForNote",
            )
            .await?;
        let p = &d["getAllPermissionedUsersForNote"];
        let or_empty = |v: &Value| if v.is_array() { v.clone() } else { json!([]) };
        Ok(
            json!({"id": n.id, "title": n.title, "link": Self::link_settings(&n),
                  "users": or_empty(&p["permissionedUsers"]), "requests": or_empty(&p["permissionRequests"])}),
        )
    }

    pub async fn share_link(&self, r: &str, scope: &str, access: &str) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let scope = norm(scope, &SCOPES, "scope")?;
        let access = norm(access, &ACCESS, "access level")?;
        let before = Self::link_settings(&n);
        let r1 = self
            .mutate(
                "modifyNote",
                json!({"noteId": n.id, "linkAccessLevel": access}),
            )
            .await?;
        let r2 = self
            .mutate(
                "modifyNote",
                json!({"noteId": n.id, "linkPermissionScope": scope}),
            )
            .await?;
        Ok(
            json!({"id": n.id, "title": n.title, "before": before, "link": {"scope": scope, "access": access},
                  "mutations": [r1, r2]}),
        )
    }

    pub async fn share_user(&self, r: &str, user_id: &str, access: Option<&str>) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let lvl = access
            .map(|a| norm(a, &ACCESS, "access level"))
            .transpose()?;
        let mut perm = json!({"userId": user_id});
        if let Some(l) = &lvl {
            perm["accessLevel"] = json!(l);
        }
        let res = self
            .mutate("modifyNote", json!({"noteId": n.id, "permissions": [perm]}))
            .await?;
        Ok(merge(
            json!({"id": n.id, "user_id": user_id, "access": lvl, "removed": lvl.is_none()}),
            res,
        ))
    }

    pub async fn share_invite(&self, r: &str, emails: &[String], access: &str) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let lvl = norm(access, &ACCESS, "access level")?;
        let perms: Vec<Value> = emails
            .iter()
            .map(|e| json!({"email": e, "permissionLevel": lvl}))
            .collect();
        let d = self
            .gql(
                M_INVITE,
                json!({"addNotePermissionsByEmailRequest": {"noteId": n.id, "permissions": perms}}),
                "addNotePermissionsByEmail",
            )
            .await?;
        Ok(merge(
            json!({"id": n.id}),
            d["addNotePermissionsByEmail"].clone(),
        ))
    }

    pub async fn share_answer(&self, request_id: &str, grant: bool, access: &str) -> Result<Value> {
        let r = if grant {
            let role = norm(access, &ACCESS, "access level")?;
            self.gql(M_GRANT, json!({"grantNoteAccessInput": {"notePermissionRequestId": request_id, "grantedRole": role}}), "grantNoteAccess")
                .await?
        } else {
            self.gql(
                M_DENY,
                json!({"denyNoteAccessInput": {"notePermissionRequestId": request_id}}),
                "denyNoteAccess",
            )
            .await?
        };
        Ok(json!({"request_id": request_id, "granted": grant, "result": r}))
    }

    // ---------------------------------------------------------- settings
    pub async fn settings(&self) -> Result<Value> {
        let me = self.gql(Q_SETTINGS, json!({}), "getMe").await?["me"].take();
        let raw = as_map(me["settings"]["appSettings"].clone());
        let mut toggles = Map::new();
        for (name, key) in TOGGLES {
            let v = parse_json_str(raw.get(key).cloned().unwrap_or(json!("true")));
            toggles.insert(name.into(), json!(v == json!(true) || v == json!("true")));
        }
        toggles.insert(
            "ai".into(),
            json!(me["isAiEnabled"].as_bool().unwrap_or(false)),
        );
        Ok(json!({"toggles": toggles, "app_settings": raw}))
    }

    pub async fn settings_set(&self, key: &str, value: &str) -> Result<Value> {
        let key = TOGGLES
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(key, |(_, v)| v);
        if key.is_empty() {
            return Err(NbError::usage("setting key is required"));
        }
        let before = self.settings().await?["app_settings"]
            .get(key)
            .cloned()
            .unwrap_or(Value::Null);
        let r = self
            .gql(
                M_SET,
                json!({"setAppSettingParams": {"key": key, "value": value}}),
                "setAppSetting",
            )
            .await?["setAppSetting"]
            .take();
        let after = as_map(r["appSettings"].clone());
        Ok(
            json!({"key": key, "before": before, "value": after.get(key).cloned().unwrap_or(json!(value)),
                  "success": r["success"], "error_code": r["errorCode"]}),
        )
    }

    pub async fn settings_toggle(&self, name: &str, on: bool) -> Result<Value> {
        if name == "ai" {
            let before = self.settings().await?["toggles"]["ai"].clone();
            let r = self
                .gql(
                    M_AI,
                    json!({"updateLearnSettingsParams": {"isAiEnabled": on}}),
                    "updateLearnSettings",
                )
                .await?;
            return Ok(
                json!({"key": "isAiEnabled", "before": before, "value": on, "success": r["updateLearnSettings"]}),
            );
        }
        let key = TOGGLES
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .ok_or_else(|| {
                NbError::usage(format!(
                    "unknown toggle '{name}': choose ai, learn, transcription"
                ))
            })?;
        self.settings_set(key, if on { "true" } else { "false" })
            .await
    }

    pub async fn settings_name(&self, name: &str) -> Result<Value> {
        let before = self.http.me().await?["name"].clone();
        let r = self
            .gql(
                M_NAME,
                json!({"fullNameParams": {"fullName": name}}),
                "updateFullName",
            )
            .await?;
        Ok(json!({"before": before, "name": name, "success": r["updateFullName"]}))
    }

    // ----------------------------------------------------------- account
    pub async fn quotas(&self) -> Result<Value> {
        Ok(
            self.gql(&q_quotas(), json!({}), "getQuotas").await?["me"]["subscriptionOverview"]
                .take(),
        )
    }

    pub async fn stickers(&self) -> Result<Value> {
        let r = self
            .gql(
                Q_STICKERS,
                json!({"userCustomAssetRequest": {"type": "STICKER"}}),
                "getMyStickers",
            )
            .await?;
        let s = &r["me"]["customAssets"]["stickers"];
        Ok(if s.is_array() { s.clone() } else { json!([]) })
    }

    pub async fn survey(&self) -> Result<Value> {
        let r = self
            .gql(Q_SURVEY, json!({}), "getUserSurveyResponse")
            .await?;
        Ok(match &r["getUserSurveyResponse"] {
            Value::Null => json!({}),
            v => v.clone(),
        })
    }

    pub async fn feature_flags(&self) -> Result<Value> {
        Ok(self.gql(Q_FLAGS, json!({}), "getFeatureFlags").await?["featureFlags"].take())
    }

    async fn site_mappings(&self, note_id: &str) -> Result<Vec<Value>> {
        let r = self
            .gql(Q_SITES, json!({"noteId": note_id}), "getSiteUserMappings")
            .await?;
        Ok(r["getSiteUserMappings"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// CRDT site id → display name (unknown sites render as "Anonymous <site>").
    pub async fn site_authors(&self, note_id: &str) -> BTreeMap<i64, String> {
        let mut out = BTreeMap::new();
        for m in self.site_mappings(note_id).await.unwrap_or_default() {
            let name = m["userName"]
                .as_str()
                .or(m["userId"].as_str())
                .unwrap_or_default()
                .to_string();
            for s in m["siteIds"].as_array().into_iter().flatten() {
                if let Some(i) = s
                    .as_i64()
                    .or_else(|| s.as_str().and_then(|x| x.parse().ok()))
                {
                    out.insert(i, name.clone());
                }
            }
        }
        out
    }

    pub async fn collaborators(&self, r: &str) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let sites = self.site_mappings(&n.id).await?;
        Ok(json!({"id": n.id, "title": n.title, "sites": sites}))
    }

    // ------------------------------------------------------------- learn
    async fn sync_quiz(&self, params: Value) -> Result<Value> {
        Ok(self
            .gql(
                &m_sync(),
                json!({"syncQuizSessionParams": params}),
                "syncQuizSession",
            )
            .await?["syncQuizSession"]
            .take())
    }

    pub async fn learn_generate(
        &self,
        r: &str,
        text: Option<String>,
        quizzes: bool,
        flashcards: bool,
        language: Option<&str>,
    ) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let text = match text {
            Some(t) => t,
            None => crate::search::note_text(self, &n.id)
                .await
                .unwrap_or_default(),
        };
        if text.trim().is_empty() {
            return Err(NbError::usage(
                "the note has no typed text to study; pass --text or --text-file",
            ));
        }
        let mut p = json!({"clientNoteId": n.id, "studyMaterials": [{"content": text, "type": "NOTE_TEXT"}],
            "asrJobSha512Hashes": [], "isServerManaged": true, "promptGroup": "USER_GUIDED",
            "promptOptions": ["latex"], "includeQuizzes": quizzes, "includeFlashcards": flashcards});
        if let Some(l) = language {
            p["language"] = json!(l);
        }
        let b = self
            .gql(
                M_GENERATE,
                json!({"generateStudyMaterialsBatchParams": p}),
                "generateStudyMaterialsBatch",
            )
            .await?["generateStudyMaterialsBatch"]
            .take();
        Ok(json!({"note_id": n.id, "title": n.title, "batch": b}))
    }

    /// Poll the study batch until every quiz/summary job leaves PENDING/RUNNING, or time out.
    pub async fn learn_wait(&self, note_id: &str, timeout: Duration) -> Result<Option<Value>> {
        let deadline = std::time::Instant::now() + timeout;
        let mut sm = None;
        while std::time::Instant::now() < deadline {
            sm = self.http.study_materials(note_id).await?;
            if let Some(s) = &sm {
                let mut jobs: Vec<&Value> = s["quizJobs"]
                    .as_array()
                    .map(|a| a.iter().collect())
                    .unwrap_or_default();
                if s["summaryOfSummaries"].is_object() {
                    jobs.push(&s["summaryOfSummaries"]);
                }
                let done = |j: &&Value| {
                    matches!(
                        j["status"]
                            .as_str()
                            .unwrap_or_default()
                            .to_uppercase()
                            .as_str(),
                        "COMPLETED" | "FAILED" | "ERROR"
                    )
                };
                if !jobs.is_empty() && jobs.iter().all(done) {
                    return Ok(sm);
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        Ok(sm)
    }

    pub async fn quiz_start(&self, r: &str, mode: &str) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let res = self
            .sync_quiz(json!({"quizTypeParams": {"noteId": n.id, "mode": quiz_mode(mode)?}}))
            .await?;
        Ok(shape(&n.id, &res))
    }

    pub async fn quiz_answer(
        &self,
        r: &str,
        session_id: &str,
        mode: &str,
        completions: Vec<Value>,
        finish: bool,
    ) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        let mut upd = json!({"quizSessionId": session_id, "quizQuestionCompletions": completions});
        if finish {
            upd["isCompleted"] = json!(true);
        }
        let res = self
            .sync_quiz(json!({"quizTypeParams": {"noteId": n.id, "mode": quiz_mode(mode)?}, "quizSessionUpdateParams": upd}))
            .await?;
        Ok(shape(&n.id, &res))
    }

    /// Spaced repetition: easy ⇒ CORRECT, anything else ⇒ INCORRECT (as the web app does).
    pub async fn card_rate(
        &self,
        r: &str,
        session_id: &str,
        index: i64,
        rating: &str,
    ) -> Result<Value> {
        let st = if rating.eq_ignore_ascii_case("easy") {
            "CORRECT"
        } else {
            "INCORRECT"
        };
        let c = completion(index, st, None, None, Some(rating))?;
        self.quiz_answer(
            r,
            session_id,
            "SPACED_REPETITION_FLASHCARDS",
            vec![c],
            false,
        )
        .await
    }

    pub async fn quiz_explain(&self, question_id: &str) -> Result<Value> {
        let r = self
            .gql(
                Q_EXPLAIN,
                json!({"getQuizQuestionExplanationParams": {"questionId": question_id}}),
                "getQuizQuestionExplanation",
            )
            .await?;
        Ok(json!({"question_id": question_id, "explanation": r["getQuizQuestionExplanation"]}))
    }

    pub async fn cards_import(&self, r: &str, cards: Vec<Value>) -> Result<Value> {
        let n = self.resolve_note(r).await?;
        if cards.is_empty() {
            return Err(NbError::usage("no flashcards to import"));
        }
        let mut clean = Vec::new();
        for c in cards {
            let t = c["term"].as_str().unwrap_or_default().trim();
            let d = c["definition"].as_str().unwrap_or_default().trim();
            if t.is_empty() || d.is_empty() {
                return Err(NbError::usage(
                    "every flashcard needs a non-empty term and definition",
                ));
            }
            clean.push(json!({"term": t, "definition": d}));
        }
        let k = clean.len();
        let r = self
            .gql(M_UPLOAD, json!({"uploadUserGeneratedQuiz": {"noteId": n.id, "uploadedFlashcard": {"flashcards": clean}}}), "uploadUserGeneratedQuiz")
            .await?;
        Ok(json!({"note_id": n.id, "imported": k, "success": r["uploadUserGeneratedQuiz"]}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_validation() {
        assert_eq!(norm("view", &ACCESS, "x").unwrap(), "VIEWER");
        assert!(norm("owner", &ACCESS, "x").is_err());
        assert_eq!(
            quiz_mode("practice-flashcards").unwrap(),
            "PRACTICE_FLASHCARDS"
        );
        let c = completion(2, "correct", Some("b"), None, Some("good")).unwrap();
        assert_eq!(c["userResponseMultipleChoice"], "B");
        assert_eq!(c["userResponseFlashcard"], "MEDIUM");
        assert!(c["reviewDate"].as_str().unwrap().ends_with('Z'));
        assert_eq!(c["reviewDate"].as_str().unwrap().len(), 24);
        assert!(completion(0, "maybe", None, None, None).is_err());
        let cards = parse_cards("a\tb\n\nc\td e\n", "\t").unwrap();
        assert_eq!(cards.len(), 2);
        assert!(parse_cards("nosep", "\t").is_err());
        let mut names = BTreeMap::new();
        names.insert(3, "Alex".to_string());
        assert_eq!(author_for(&names, "3:17"), "Alex");
        assert_eq!(author_for(&names, "9:1"), "Anonymous 9");
        assert!(q_quotas().contains("youtubeTranscripts { isUsageExceeded"));
        assert!(m_sync().contains("lastCompletedQuizSession { id noteId"));
    }
}
