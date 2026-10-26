//! Server-side ASR (speech recognition) jobs and the GCS POST-policy upload they need.
//!
//! Mirrors the web client's `registerAsrJobs`: `getASRJob{sha512Hash}`; on `JOB_NOT_FOUND`,
//! `createASRJob{sha512Hash, audioCreationDateMs, extension}` returns an `uploadPolicy`
//! (`{url, fields}`); the audio is POSTed as a multipart form (gzip-encoded for m4a), then
//! `getASRJob` is polled until `COMPLETED`.

use std::io::Write;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::{NbError, Result};
use crate::http::Http;

const CREATE_ASR: &str = "mutation createASRJob($p: CreateASRJobParams!) {
  createASRJob(createASRJobParams: $p) { success errorCode job { uploadPolicy status } } }";

/// `createASRJob.extension` for a recording mime type (client map: m4a, mp4, webm).
pub fn asr_extension(mime: &str) -> Option<&'static str> {
    match mime {
        "audio/x-m4a" => Some("m4a"),
        "audio/mp4" => Some("mp4"),
        "audio/webm" => Some("webm"),
        _ => None,
    }
}

fn content_type(ext: &str) -> &'static str {
    match ext {
        "m4a" => "audio/m4a",
        "mp4" => "audio/mp4",
        "webm" | "weba" => "audio/webm",
        _ => "application/octet-stream",
    }
}

fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data)
        .and_then(|()| e.finish())
        .map_err(|err| NbError::generic(format!("gzip: {err}")))
}

fn job_status(job: &Value) -> Option<String> {
    if job["success"].as_bool() == Some(true) {
        job["job"]["status"].as_str().map(str::to_string)
    } else {
        job["errorCode"].as_str().map(str::to_string)
    }
}

impl Http {
    /// POST `body` to a GCS POST-policy (`{url, fields}`) without cookies.
    pub async fn post_policy(&self, policy: &Value, body: Vec<u8>, ext: &str) -> Result<u16> {
        let url = policy["url"]
            .as_str()
            .ok_or_else(|| NbError::server("ASR upload policy has no url"))?;
        let mut fields: Vec<(String, String)> = policy["fields"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_str().map_or_else(|| v.to_string(), str::to_string),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let body = if ext == "m4a" {
            fields.push(("Content-Encoding".into(), "gzip".into()));
            gzip(&body)?
        } else {
            body
        };
        fields.push(("Content-Type".into(), content_type(ext).into()));
        let key = fields
            .iter()
            .find(|(k, _)| k == "key")
            .map_or_else(|| "audio".to_string(), |(_, v)| v.clone());
        let mut form = reqwest::multipart::Form::new();
        for (k, v) in fields {
            form = form.text(k, v);
        }
        form = form.part("file", reqwest::multipart::Part::bytes(body).file_name(key));
        self.gate().await;
        let r = self
            .external_client()
            .post(url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| NbError::server(format!("ASR upload: {e}")))?;
        let status = r.status().as_u16();
        if !matches!(status, 200 | 201 | 204) {
            let text: String = r
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            return Err(
                NbError::server(format!("ASR upload failed: HTTP {status} {text}"))
                    .with_status(status),
            );
        }
        Ok(status)
    }

    /// Reuse an existing ASR job for `sha512`, else create one and upload the audio once; then
    /// poll until it finishes or `wait` elapses. Returns `{created_job, status, text, segments}`.
    pub async fn transcribe(
        &self,
        sha512: &str,
        data: &[u8],
        mime: &str,
        start_ms: i64,
        wait: Duration,
    ) -> Result<Value> {
        let ext = asr_extension(mime).ok_or_else(|| {
            NbError::usage(format!("ASR does not accept {mime} (m4a, mp4 or webm)"))
        })?;
        let mut job = self.transcript(sha512).await?;
        let mut created = false;
        if job["success"].as_bool() != Some(true) && job["errorCode"] == "JOB_NOT_FOUND" {
            let res = self
                .gql_field(
                    CREATE_ASR,
                    json!({"p": {"sha512Hash": sha512, "audioCreationDateMs": start_ms, "extension": ext}}),
                    "createASRJob",
                )
                .await?;
            if res["success"].as_bool() != Some(true) {
                let code = res["errorCode"].as_str().unwrap_or("UNKNOWN").to_string();
                let e = if matches!(code.as_str(), "QUOTA_EXCEEDED" | "NOT_SUBSCRIBED") {
                    NbError::quota(format!("createASRJob: {code}"))
                } else {
                    NbError::server(format!("createASRJob: {code}"))
                };
                return Err(e.with_key(Some(code)));
            }
            let pol = &res["job"]["uploadPolicy"];
            let pol = match pol {
                Value::String(s) => serde_json::from_str(s)
                    .map_err(|e| NbError::server(format!("bad ASR upload policy: {e}")))?,
                v => v.clone(),
            };
            if pol.is_object() {
                self.post_policy(&pol, data.to_vec(), ext).await?;
            }
            created = true;
        }
        let deadline = Instant::now() + wait;
        loop {
            let st = job_status(&job);
            if matches!(st.as_deref(), Some("COMPLETED" | "PERMANENT_ERROR"))
                || Instant::now() > deadline
            {
                break;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            job = self.transcript(sha512).await?;
        }
        let segs = job["job"]["results"]["segments"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let text = segs
            .iter()
            .filter_map(|s| s["text"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(
            json!({"created_job": created, "status": job_status(&job), "text": text, "segments": segs}),
        )
    }
}
