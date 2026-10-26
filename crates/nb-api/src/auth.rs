//! Credential storage (`credentials.json`), device id and CDP session import.
//!
//! One long-lived HS256 JWT in the `nb-auth-token` cookie authenticates every API surface. It is
//! never asked for as an argv value: it comes from `NOTABILITY_TOKEN`, stdin, a browser over CDP
//! (`Storage.getCookies` only; no page is attached or navigated) or the stored file (0600).

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use futures_util::{SinkExt as _, StreamExt as _};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::config::AUTH_COOKIE;
use crate::error::{NbError, Result};

pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NOTABILITY_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(x).join("notability-cli");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("notability-cli")
}

pub fn credentials_path() -> PathBuf {
    config_dir().join("credentials.json")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

fn ensure_dir() -> Result<PathBuf> {
    let d = config_dir();
    fs::create_dir_all(&d)?;
    set_mode(&d, 0o700)?;
    Ok(d)
}

#[cfg(unix)]
fn set_mode(p: &std::path::Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(p, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_: &std::path::Path, _: u32) -> Result<()> {
    Ok(())
}

/// Write pretty JSON to a 0600 file inside the 0700 config dir.
pub fn write_private(path: &std::path::Path, data: &Value) -> Result<()> {
    ensure_dir()?;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(
        serde_json::to_string_pretty(data)
            .unwrap_or_default()
            .as_bytes(),
    )?;
    set_mode(path, 0o600)
}

/// Decode (without verifying) the JWT payload; display only.
pub fn jwt_claims(token: &str) -> Value {
    token
        .split('.')
        .nth(1)
        .and_then(|p| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(p.trim_end_matches('='))
                .ok()
        })
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}))
}

#[derive(Clone)]
pub struct Credentials {
    pub token: SecretString,
    pub source: String,
    pub imported_at: f64,
    pub cookie_expires: Option<f64>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Credentials {
    pub fn new(token: String, source: impl Into<String>) -> Self {
        Self {
            token: SecretString::from(token),
            source: source.into(),
            imported_at: now_secs(),
            cookie_expires: None,
        }
    }

    pub fn claims(&self) -> Value {
        jwt_claims(self.token.expose_secret())
    }

    pub fn user_id(&self) -> Option<String> {
        self.claims()
            .get("userId")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Safe-to-print summary.
    pub fn redacted(&self) -> Value {
        let c = self.claims();
        let tok = self.token.expose_secret();
        json!({
            "source": self.source,
            "user_id": c.get("userId"),
            "token_version": c.get("version"),
            "issued_at": c.get("iat"),
            "imported_at": (self.imported_at > 0.0).then_some(self.imported_at as i64),
            "cookie_expires": self.cookie_expires.filter(|v| *v != 0.0).map(|v| v as i64),
            "token_preview": (!tok.is_empty()).then(|| format!("{}…", tok.chars().take(6).collect::<String>())),
        })
    }
}

/// `NOTABILITY_TOKEN`, then the credentials file (either `{"token": ...}` or a raw CDP cookie list).
pub fn load_credentials() -> Result<Option<Credentials>> {
    if let Ok(env) = std::env::var("NOTABILITY_TOKEN") {
        let t = env.trim();
        if !t.is_empty() {
            return Ok(Some(Credentials {
                token: SecretString::from(t.to_string()),
                source: "env:NOTABILITY_TOKEN".into(),
                imported_at: 0.0,
                cookie_expires: None,
            }));
        }
    }
    let p = credentials_path();
    let Ok(raw) = fs::read(&p) else {
        return Ok(None);
    };
    let data: Value = serde_json::from_slice(&raw)
        .map_err(|e| NbError::auth(format!("{} is not valid JSON: {e}", p.display())))?;
    let mut tok = data
        .get("token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut expires = data.get("cookie_expires").and_then(Value::as_f64);
    if tok.is_none() {
        for c in data
            .get("cookies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if c.get("name").and_then(Value::as_str) == Some(AUTH_COOKIE) {
                tok = c.get("value").and_then(Value::as_str).map(str::to_string);
                expires = expires.or_else(|| c.get("expires").and_then(Value::as_f64));
            }
        }
    }
    let Some(tok) = tok else {
        return Err(NbError::auth(format!(
            "{} has no {AUTH_COOKIE}; re-run `notability account login`.",
            p.display()
        )));
    };
    Ok(Some(Credentials {
        token: SecretString::from(tok),
        source: data
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("file")
            .to_string(),
        imported_at: data
            .get("imported_at")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        cookie_expires: expires,
    }))
}

pub fn require_credentials() -> Result<Credentials> {
    load_credentials()?.ok_or_else(|| {
        NbError::auth("Not signed in. Run `notability account login --cdp` (copies the session from a signed-in browser).")
    })
}

pub fn save_credentials(c: &Credentials) -> Result<PathBuf> {
    let p = credentials_path();
    let imported = if c.imported_at > 0.0 {
        c.imported_at
    } else {
        now_secs()
    };
    write_private(
        &p,
        &json!({
            "token": c.token.expose_secret(),
            "source": c.source,
            "imported_at": imported,
            "cookie_expires": c.cookie_expires,
        }),
    )?;
    Ok(p)
}

pub fn delete_credentials() -> Result<bool> {
    let p = credentials_path();
    match fs::remove_file(&p) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// A stable per-install device id (the web app keeps one in IndexedDB).
pub fn device_id() -> Result<String> {
    let p = settings_path();
    let mut data: Value = fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}));
    if let Some(id) = data
        .get("device_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Ok(id.to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    if !data.is_object() {
        data = json!({});
    }
    data["device_id"] = json!(id);
    write_private(&p, &data)?;
    Ok(id)
}

/// Read the `nb-auth-token` cookie from a running Chromium via CDP `Storage.getCookies`.
/// Does not save; the caller validates the token first.
pub async fn import_from_cdp(cdp: &str, timeout: Duration) -> Result<Credentials> {
    let mut base = cdp.trim_end_matches('/').to_string();
    if !base.starts_with("http") {
        base = format!("http://{base}");
    }
    let http = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| NbError::generic(e.to_string()))?;
    let ver: Value = async {
        http.get(format!("{base}/json/version"))
            .send()
            .await?
            .json()
            .await
    }
    .await
    .map_err(|e: reqwest::Error| NbError::auth(format!("Cannot reach CDP endpoint {base}: {e}")))?;
    let ws_url = ver
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            NbError::auth(format!(
                "CDP endpoint {base} did not expose webSocketDebuggerUrl"
            ))
        })?;
    let msg = tokio::time::timeout(timeout, async {
        let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| NbError::auth(format!("CDP websocket connect failed: {e}")))?;
        let req = json!({"id": 1, "method": "Storage.getCookies", "params": {}}).to_string();
        ws.send(Message::text(req))
            .await
            .map_err(|e| NbError::auth(format!("CDP send failed: {e}")))?;
        while let Some(m) = ws.next().await {
            let m = m.map_err(|e| NbError::auth(format!("CDP read failed: {e}")))?;
            if let Message::Text(t) = m {
                let v: Value = serde_json::from_str(&t).unwrap_or_default();
                if v.get("id") == Some(&json!(1)) {
                    let _ = ws.close(None).await;
                    return Ok(v);
                }
            }
        }
        Err(NbError::auth(
            "CDP connection closed before Storage.getCookies answered",
        ))
    })
    .await
    .map_err(|_| NbError::auth("CDP Storage.getCookies timed out"))??;
    if let Some(err) = msg.get("error") {
        return Err(NbError::auth(format!("CDP error: {err}")));
    }
    let cookies = msg
        .pointer("/result/cookies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tok = cookies
        .iter()
        .filter(|c| c.get("domain").and_then(Value::as_str).is_some_and(|d| d.ends_with("notability.com")))
        .find(|c| c.get("name").and_then(Value::as_str) == Some(AUTH_COOKIE))
        .ok_or_else(|| NbError::auth("No nb-auth-token cookie in that browser. Sign in at https://notability.com/app first."))?;
    let host = base.split_once("//").map_or(base.as_str(), |(_, h)| h);
    let mut creds = Credentials::new(
        tok.get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        format!("cdp:{host}"),
    );
    creds.cookie_expires = tok
        .get("expires")
        .and_then(Value::as_f64)
        .filter(|e| *e > 0.0);
    Ok(creds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_decode_without_padding() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"userId":"u1","version":2}"#);
        let c = Credentials::new(format!("h.{payload}.s"), "test");
        assert_eq!(c.user_id().as_deref(), Some("u1"));
        assert_eq!(
            c.redacted()["token_preview"],
            json!(format!("h.{}…", &payload[..4]))
        );
        assert_eq!(jwt_claims("garbage"), json!({}));
    }
}
