//! Minimal Socket.IO v4 (EIO=4) client for the collab `/metadata` namespace: the library arrives
//! as `partial-update`; writes are `change{mutations}` answered by `change-ack`.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::config::{AUTH_COOKIE, CLIENT_HEADERS, ENDPOINTS, USER_AGENT};
use crate::error::{NbError, Result};

const NS: &str = "/metadata";

/// Decode `42/metadata,[ackId]["ev", payload]`; `None` for any other packet.
pub fn parse_event_packet(m: &str) -> Option<(String, Value)> {
    let body = m.strip_prefix("42/metadata,")?;
    let body = body.trim_start_matches(|c: char| c.is_ascii_digit());
    let arr: Vec<Value> = serde_json::from_str(body).ok()?;
    let mut it = arr.into_iter();
    let ev = it.next()?.as_str()?.to_string();
    Some((ev, it.next().unwrap_or(Value::Null)))
}

/// The envelope the web app's metadata manager puts around each mutation.
pub fn mutation(kind: &str, payload: Value) -> Value {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    json!({"idempotencyKey": uuid::Uuid::new_v4().to_string(), "timestamp": ts, kind: payload})
}

pub(crate) fn tls_connector() -> Connector {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map(|b| b.with_root_certificates(roots).with_no_client_auth());
    match cfg {
        Ok(c) => Connector::Rustls(Arc::new(c)),
        Err(_) => Connector::Plain,
    }
}

pub struct MetadataSocket {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
}

impl MetadataSocket {
    pub async fn connect(token: &str, timeout: Duration, sequence_id: Option<u64>) -> Result<Self> {
        let url = format!("{}?EIO=4&transport=websocket", ENDPOINTS.socket);
        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| NbError::server(format!("metadata socket connect failed: {e}")))?;
        let h = req.headers_mut();
        let hv = |s: &str| {
            HeaderValue::from_str(s)
                .map_err(|e| NbError::server(format!("metadata socket connect failed: {e}")))
        };
        h.insert("Origin", hv(&ENDPOINTS.api)?);
        h.insert("Cookie", hv(&format!("{AUTH_COOKIE}={token}"))?);
        h.insert("User-Agent", hv(USER_AGENT)?);
        let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(1 << 28))
            .max_frame_size(Some(1 << 28));
        let fut = tokio_tungstenite::connect_async_tls_with_config(
            req,
            Some(cfg),
            false,
            Some(tls_connector()),
        );
        let (ws, _) = tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| NbError::server("metadata socket connect failed: timed out"))?
            .map_err(|e| NbError::server(format!("metadata socket connect failed: {e}")))?;
        let mut s = Self { ws, timeout };
        let deadline = Instant::now() + timeout;
        let first = s.recv_text(deadline).await?.unwrap_or_default();
        if !first.starts_with('0') {
            let head: String = first.chars().take(80).collect();
            return Err(NbError::server(format!(
                "unexpected engine.io handshake: {head}"
            )));
        }
        let mut query: Map<String, Value> = CLIENT_HEADERS
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
        if let Some(seq) = sequence_id {
            query.insert("sequenceId".into(), json!(seq));
        }
        s.send(format!(
            "40{NS},{}",
            json!({"token": token, "query": query})
        ))
        .await?;
        Ok(s)
    }

    async fn send(&mut self, text: String) -> Result<()> {
        self.ws
            .send(Message::text(text))
            .await
            .map_err(|e| NbError::server(format!("metadata socket send failed: {e}")))
    }

    async fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            match tokio::time::timeout(left, self.ws.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => return Err(NbError::server("metadata socket disconnected by server")),
                Ok(Some(Err(e))) => {
                    return Err(NbError::server(format!("metadata socket error: {e}")));
                }
                Ok(Some(Ok(Message::Text(t)))) => return Ok(Some(t.to_string())),
                Ok(Some(Ok(Message::Close(_)))) => {
                    return Err(NbError::server("metadata socket disconnected by server"));
                }
                Ok(Some(Ok(_))) => continue,
            }
        }
    }

    async fn recv_event(&mut self, deadline: Instant) -> Result<Option<(String, Value)>> {
        loop {
            let Some(m) = self.recv_text(deadline).await? else {
                return Ok(None);
            };
            if m == "2" {
                self.send("3".into()).await?;
                continue;
            }
            if m.starts_with(&format!("40{NS},")) {
                continue;
            }
            if let Some(rest) = m.strip_prefix(&format!("44{NS},")) {
                let err: Value = serde_json::from_str(rest).unwrap_or(Value::Null);
                let msg = err
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| err.to_string());
                return Err(NbError::auth(format!(
                    "metadata socket rejected connection: {msg}"
                )));
            }
            if m.starts_with(&format!("41{NS}")) {
                return Err(NbError::server("metadata socket disconnected by server"));
            }
            if let Some((ev, payload)) = parse_event_packet(&m) {
                if ev == "error" {
                    return Err(NbError::server(format!("metadata socket error: {payload}")));
                }
                return Ok(Some((ev, payload)));
            }
        }
    }

    pub async fn wait_for(&mut self, names: &[&str]) -> Result<(String, Value)> {
        let deadline = Instant::now() + self.timeout;
        loop {
            match self.recv_event(deadline).await? {
                None => {
                    let mut n: Vec<_> = names.iter().map(|s| format!("'{s}'")).collect();
                    n.sort();
                    return Err(NbError::server(format!(
                        "timed out waiting for [{}] on metadata socket",
                        n.join(", ")
                    )));
                }
                Some((ev, p)) if names.contains(&ev.as_str()) => return Ok((ev, p)),
                Some(_) => {}
            }
        }
    }

    pub async fn library(&mut self) -> Result<Value> {
        Ok(self.wait_for(&["partial-update"]).await?.1)
    }

    /// Emit mutations and block until each is acknowledged; any failure is `WriteRejected`.
    pub async fn mutate(&mut self, mutations: &[Value]) -> Result<Value> {
        let keys: Vec<String> = mutations
            .iter()
            .filter_map(|m| m["idempotencyKey"].as_str().map(str::to_string))
            .collect();
        self.send(format!(
            "42{NS},{}",
            json!(["change", {"mutations": mutations}])
        ))
        .await?;
        let (mut ok, mut failed): (Vec<String>, Vec<Value>) = (Vec::new(), Vec::new());
        let deadline = Instant::now() + self.timeout;
        let done = |ok: &Vec<String>, failed: &Vec<Value>| {
            keys.iter().all(|k| {
                ok.contains(k)
                    || failed
                        .iter()
                        .any(|f| f["idempotencyKey"].as_str() == Some(k))
            })
        };
        while !done(&ok, &failed) {
            let Some((ev, p)) = self.recv_event(deadline).await? else {
                return Err(NbError::server("timed out waiting for change-ack"));
            };
            if ev != "change-ack" {
                continue;
            }
            for k in p["successfulMutations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if keys.iter().any(|x| x == k) {
                    ok.push(k.to_string());
                }
            }
            for f in p["failedMutations"].as_array().into_iter().flatten() {
                if f["idempotencyKey"]
                    .as_str()
                    .is_some_and(|k| keys.iter().any(|x| x == k))
                {
                    failed.push(f.clone());
                }
            }
        }
        if !failed.is_empty() {
            return Err(NbError::write_rejected(format!(
                "server rejected {} mutation(s): {}",
                failed.len(),
                Value::from(failed)
            )));
        }
        Ok(json!({"successful": ok, "failed": failed}))
    }

    pub async fn close(mut self) {
        let _ = self.send(format!("41{NS},")).await;
        let _ = self.ws.close(None).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_packets_parse_with_and_without_ack_id() {
        assert_eq!(
            parse_event_packet(r#"42/metadata,17["change-ack",{"a":1}]"#),
            Some(("change-ack".into(), json!({"a": 1})))
        );
        assert_eq!(
            parse_event_packet(r#"42/metadata,["x"]"#),
            Some(("x".into(), Value::Null))
        );
        assert_eq!(parse_event_packet("40/metadata,{}"), None);
    }

    #[test]
    fn sample_session_frames_parse() {
        let cap = include_str!("../tests/fixtures/socket-metadata.txt");
        let evs: Vec<_> = cap
            .lines()
            .filter_map(|l| l.strip_prefix("← "))
            .filter_map(parse_event_packet)
            .map(|e| e.0)
            .collect();
        assert_eq!(evs, ["partial-update", "change-ack"]);
        let sent: Vec<_> = cap
            .lines()
            .filter_map(|l| l.strip_prefix("→ "))
            .filter_map(parse_event_packet)
            .map(|e| e.0)
            .collect();
        assert_eq!(sent, ["change"]);
    }

    #[test]
    fn mutation_envelope_shape() {
        let m = mutation("createFolder", json!({"id": "f"}));
        assert_eq!(m["createFolder"]["id"], "f");
        assert_eq!(m["idempotencyKey"].as_str().map(str::len), Some(36));
        assert!(m["timestamp"].as_u64().unwrap() > 1_700_000_000_000);
    }
}
