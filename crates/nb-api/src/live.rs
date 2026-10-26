//! REST side channels of the web recorder and Learn: live transcription sessions
//! (`/live-transcription/session/<id>`) and syllabus parse jobs (`/learn/syllabusParseJob`).

use std::time::Duration;

use reqwest::Method;
use serde_json::{Value, json};

use crate::config::ENDPOINTS;
use crate::error::{NbError, Result};
use crate::http::{Http, Req};

/// One live-transcription reply: the running transcript and smart-notes text, and how many
/// chunks the server has consumed (`None` for a 204 "consumed, nothing new").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveReply {
    pub status: u16,
    pub transcription_text: Option<String>,
    pub smart_notes_text: Option<String>,
    pub consumed: Option<u64>,
}

pub fn live_url(session: &str) -> String {
    format!("{}/live-transcription/session/{session}", ENDPOINTS.api)
}

fn quota_or(e: NbError, what: &str) -> NbError {
    if e.status == Some(403) {
        NbError::quota(format!(
            "{what} refused (HTTP 403: not entitled or out of transcription quota)"
        ))
        .with_status(403)
    } else {
        e
    }
}

/// Parse a live-transcription PUT reply body.
pub fn parse_live(status: u16, body: &[u8]) -> LiveReply {
    if status == 204 || body.is_empty() {
        return LiveReply {
            status,
            ..LiveReply::default()
        };
    }
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    LiveReply {
        status,
        transcription_text: v["transcriptionText"].as_str().map(str::to_string),
        smart_notes_text: v["smartNotesText"].as_str().map(str::to_string),
        consumed: v["lastAudioChunkIndexConsumed"].as_u64(),
    }
}

impl Http {
    /// PUT every not-yet-acknowledged recorder chunk (concatenated); `last_index` is the
    /// running count of chunks appended so far (`nb-last-audio-chunk-index`).
    pub async fn live_put(
        &self,
        session: &str,
        note_id: &str,
        language: &str,
        data: Vec<u8>,
        last_index: u64,
    ) -> Result<LiveReply> {
        let r = self
            .send(
                Req::new(Method::PUT, live_url(session))
                    .header("content-type", "audio/mp4")
                    .header("accept-language", language)
                    .header("nb-collab-note-id", note_id)
                    .header("nb-last-audio-chunk-index", last_index.to_string())
                    .header("nb-request-id", uuid::Uuid::new_v4().to_string())
                    .bytes(data)
                    .timeout(Duration::from_secs(60)),
            )
            .await
            .map_err(|e| quota_or(e, "live transcription"))?;
        if r.status >= 400 {
            return Err(NbError::server(format!(
                "live transcription PUT failed: HTTP {}",
                r.status
            ))
            .with_status(r.status));
        }
        Ok(parse_live(r.status, &r.body))
    }

    /// Close a live-transcription session; returns the server's final JSON (or `{}`).
    pub async fn live_finalize(&self, session: &str, language: &str) -> Result<Value> {
        let r = self
            .send(
                Req::new(Method::POST, format!("{}/finalize", live_url(session)))
                    .header("accept-language", language)
                    .timeout(Duration::from_secs(120)),
            )
            .await
            .map_err(|e| quota_or(e, "live transcription finalize"))?;
        if r.status >= 400 {
            return Err(NbError::server(format!(
                "live transcription finalize failed: HTTP {}",
                r.status
            ))
            .with_status(r.status));
        }
        Ok(if r.body.is_empty() {
            json!({})
        } else {
            r.json().unwrap_or_else(|_| json!({"text": r.text()}))
        })
    }

    /// Create a syllabus parse job: returns `{jobId, uploadUrl, requiredHeaders}`.
    pub async fn syllabus_job(&self, params: Value) -> Result<Value> {
        let r = self
            .send(
                Req::new(
                    Method::POST,
                    format!("{}/learn/syllabusParseJob", ENDPOINTS.api),
                )
                .json(params)
                .once(),
            )
            .await
            .map_err(|e| quota_or(e, "syllabus import"))?;
        if r.status >= 400 {
            return Err(NbError::server(format!(
                "syllabus job failed: HTTP {}: {}",
                r.status,
                r.text().chars().take(200).collect::<String>()
            ))
            .with_status(r.status));
        }
        r.json()
    }

    /// Wait for a syllabus parse job: returns `{course, events}`.
    pub async fn syllabus_await(&self, job_id: &str) -> Result<Value> {
        let r = self
            .send(
                Req::new(
                    Method::POST,
                    format!("{}/learn/syllabusParseJob/{job_id}/await", ENDPOINTS.api),
                )
                .json(json!({}))
                .timeout(Duration::from_secs(300)),
            )
            .await?;
        if r.status >= 400 {
            return Err(
                NbError::server(format!("syllabus parse failed: HTTP {}", r.status))
                    .with_status(r.status),
            );
        }
        r.json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_replies() {
        let r = parse_live(
            200,
            br#"{"transcriptionText":"hello","smartNotesText":"- hi","lastAudioChunkIndexConsumed":3}"#,
        );
        assert_eq!(r.transcription_text.as_deref(), Some("hello"));
        assert_eq!(r.smart_notes_text.as_deref(), Some("- hi"));
        assert_eq!(r.consumed, Some(3));
        assert_eq!(parse_live(204, b"").consumed, None);
    }

    #[test]
    fn maps_403_to_quota() {
        let e = quota_or(NbError::auth("x").with_status(403), "live");
        assert_eq!(e.kind, crate::Kind::Quota);
    }
}
