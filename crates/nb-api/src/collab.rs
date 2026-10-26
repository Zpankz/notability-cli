//! collab-api: note bundles, op appends and asset downloads.

use nb_protocol::bundle::sniff_extension;
use nb_protocol::codec;
use reqwest::Method;
use serde_json::{Value, json};

use crate::auth::device_id;
use crate::config::ENDPOINTS;
use crate::error::{NbError, Result};
use crate::http::{Http, Req};

/// `"site:timestamp[:index]"`.
pub fn fmt_id(id: &Value) -> String {
    let base = format!(
        "{}:{}",
        id["site"].as_u64().unwrap_or(0),
        id["timestamp"].as_u64().unwrap_or(0)
    );
    match id.get("index").and_then(Value::as_u64) {
        Some(i) => format!("{base}:{i}"),
        None => base,
    }
}

impl Http {
    pub async fn note_bundle_raw(&self, note_id: &str) -> Result<Vec<u8>> {
        let url = format!("{}/note/{note_id}/bundle", ENDPOINTS.collab);
        let r = self
            .send(
                Req::new(Method::POST, url)
                    .query("deviceId", device_id()?)
                    .header("content-type", "application/octet-stream")
                    .bytes(Vec::new()),
            )
            .await?;
        Ok(r.body)
    }

    /// The decoded `NoteBundle`, or an empty log for notes created without one.
    pub async fn note_bundle(&self, note_id: &str) -> Result<Value> {
        let raw = self.note_bundle_raw(note_id).await?;
        if raw.starts_with(b"No ops found") {
            return Ok(json!({"editorSite": 0, "ops": []}));
        }
        if raw.len() < 8 {
            return Err(NbError::not_found(format!(
                "note {note_id} has no edit log"
            )));
        }
        Ok(codec::decode("NoteBundle", &raw)?)
    }

    /// `PUT /note/{id}/append?siteId=` with an `AppendOpsEvent`; every op must be acked.
    pub async fn append_ops(&self, note_id: &str, site: u64, ops: &[Value]) -> Result<Vec<Value>> {
        let body = codec::encode("AppendOpsEvent", &json!({"ops": ops}))?;
        let url = format!("{}/note/{note_id}/append", ENDPOINTS.collab);
        let r = self
            .send(
                Req::new(Method::PUT, url)
                    .query("siteId", site)
                    .header("content-type", "application/octet-stream")
                    .bytes(body),
            )
            .await?;
        if r.status >= 400 {
            return Err(
                NbError::write_rejected(format!("append rejected: HTTP {}", r.status))
                    .with_status(r.status),
            );
        }
        let resp = if r.body.is_empty() {
            json!({"acks": []})
        } else {
            codec::decode("NoteMutationResponse", &r.body)?
        };
        let acks = resp["acks"].as_array().cloned().unwrap_or_default();
        let bad: Vec<String> = acks
            .iter()
            .filter(|a| a.get("nakError").is_some_and(|e| !e.is_null()))
            .map(|a| format!("{}: {}", fmt_id(&a["id"]), a["nakError"]))
            .collect();
        if !bad.is_empty() {
            return Err(NbError::write_rejected(format!(
                "append rejected: {}",
                bad.join("; ")
            )));
        }
        if acks.len() != ops.len() {
            return Err(NbError::write_rejected(format!(
                "append acknowledged {} of {} ops",
                acks.len(),
                ops.len()
            )));
        }
        Ok(acks)
    }

    pub async fn media_url(&self, note_id: &str, asset_hash: &str) -> Result<String> {
        let r = self
            .send(
                Req::new(Method::GET, format!("{}/media/{asset_hash}", ENDPOINTS.api))
                    .query("noteId", note_id),
            )
            .await?;
        Ok(r.text().trim().trim_matches('"').to_string())
    }

    /// Asset bytes plus a sniffed file extension; signed storage URLs are fetched cookie-less.
    pub async fn download_asset(
        &self,
        note_id: &str,
        asset_hash: &str,
    ) -> Result<(Vec<u8>, &'static str)> {
        let url = self.media_url(note_id, asset_hash).await?;
        let external = url::Url::parse(&url)
            .map(|u| !self.is_api_origin(&u))
            .unwrap_or(false);
        let r = if external {
            self.fetch_external(&url).await?
        } else {
            self.send(Req::new(Method::GET, &url)).await?
        };
        if r.status != 200 {
            let short: String = asset_hash.chars().take(12).collect();
            let msg = format!("asset {short}… download failed: HTTP {}", r.status);
            let e = if r.status == 404 {
                NbError::not_found(msg)
            } else {
                NbError::server(msg)
            };
            return Err(e.with_status(r.status));
        }
        let ext = sniff_extension(&r.body[..r.body.len().min(16)]);
        Ok((r.body, ext))
    }

    pub async fn thumbnail(&self, note_id: &str, thumbnail_url: Option<&str>) -> Result<Vec<u8>> {
        let default = format!("/images/thumbnail/{note_id}");
        let path = thumbnail_url.unwrap_or(&default);
        let url = if path.starts_with('/') {
            format!("{}{path}", ENDPOINTS.api)
        } else {
            path.to_string()
        };
        Ok(self.send(Req::new(Method::GET, url)).await?.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_format_canonically() {
        assert_eq!(fmt_id(&json!({"site": 3, "timestamp": 9})), "3:9");
        assert_eq!(
            fmt_id(&json!({"site": 0, "timestamp": 2, "index": 4})),
            "0:2:4"
        );
    }
}
