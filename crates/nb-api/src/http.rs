//! HTTP policy: a 250 ms rate gate, retries (transport errors, 429, 5xx with Retry-After),
//! status → error mapping, and credentials scoped to the Notability origin. Redirects are
//! followed by hand; a hop to another origin is fetched cookie-less by the external client.

use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::Value;
use tokio::sync::Mutex;
use url::Url;

use crate::config::{AUTH_COOKIE, CLIENT_HEADERS, ENDPOINTS, USER_AGENT};
use crate::error::{NbError, Result};

pub const MIN_INTERVAL: Duration = Duration::from_millis(250);

/// A finished response with its body read.
#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub url: Url,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Result<Value> {
        serde_json::from_slice(&self.body).map_err(|_| {
            NbError::server(format!(
                "non-JSON response (HTTP {}) from {}",
                self.status, self.url
            ))
        })
    }
}

#[derive(Debug, Clone, Default)]
pub enum Body {
    #[default]
    None,
    Json(Value),
    Bytes(Vec<u8>),
}

/// One request description. `auth` adds the `nb-auth-token` header; the cookie is sent only
/// to the Notability origin.
#[derive(Debug, Clone)]
pub struct Req {
    pub method: Method,
    pub url: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Body,
    pub auth: bool,
    pub timeout: Option<Duration>,
    /// Retry transport errors, 429 and 5xx (off for non-replayable calls such as AI turns).
    pub retry: bool,
}

impl Req {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            query: vec![],
            headers: vec![],
            body: Body::None,
            auth: true,
            timeout: None,
            retry: true,
        }
    }
    pub fn query(mut self, k: &str, v: impl ToString) -> Self {
        self.query.push((k.into(), v.to_string()));
        self
    }
    pub fn header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn json(mut self, v: Value) -> Self {
        self.body = Body::Json(v);
        self
    }
    pub fn bytes(mut self, b: Vec<u8>) -> Self {
        self.body = Body::Bytes(b);
        self
    }
    pub fn no_auth(mut self) -> Self {
        self.auth = false;
        self
    }
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }
    pub fn once(mut self) -> Self {
        self.retry = false;
        self
    }
}

#[derive(Clone)]
pub struct Http {
    token: SecretString,
    api: Url,
    inner: reqwest::Client,
    external: reqwest::Client,
    last: Arc<Mutex<Option<Instant>>>,
    pub max_retries: u32,
}

fn origin(u: &Url) -> (String, Option<String>, Option<u16>) {
    (
        u.scheme().to_string(),
        u.host_str().map(str::to_string),
        u.port_or_known_default(),
    )
}

fn transport(e: &reqwest::Error, url: &str) -> NbError {
    NbError::server(format!("network error calling {url}: {e}"))
}

impl Http {
    pub fn new(token: SecretString) -> Result<Self> {
        let api = Url::parse(&ENDPOINTS.api)
            .map_err(|e| NbError::usage(format!("bad NOTABILITY_API_BASE: {e}")))?;
        let mut h = HeaderMap::new();
        let mut put = |k: &str, v: &str| {
            if let (Ok(k), Ok(v)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                h.insert(k, v);
            }
        };
        put("origin", &ENDPOINTS.api);
        put("referer", &format!("{}/app/", ENDPOINTS.api));
        for (k, v) in CLIENT_HEADERS.iter() {
            put(k, v);
        }
        let build = |b: reqwest::ClientBuilder| {
            b.build()
                .map_err(|e| NbError::generic(format!("http client: {e}")))
        };
        let inner = build(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .default_headers(h)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(60)),
        )?;
        let external = build(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(120)),
        )?;
        Ok(Self {
            token,
            api,
            inner,
            external,
            last: Arc::new(Mutex::new(None)),
            max_retries: 3,
        })
    }

    pub fn token(&self) -> &str {
        self.token.expose_secret()
    }

    pub fn is_api_origin(&self, url: &Url) -> bool {
        origin(url) == origin(&self.api)
    }

    /// Wait until at least [`MIN_INTERVAL`] has passed since the previous request.
    pub async fn gate(&self) {
        let mut last = self.last.lock().await;
        if let Some(t) = *last {
            let d = t.elapsed();
            if d < MIN_INTERVAL {
                tokio::time::sleep(MIN_INTERVAL - d).await;
            }
        }
        *last = Some(Instant::now());
    }

    pub async fn send(&self, req: Req) -> Result<Resp> {
        let base = Url::parse(&req.url)
            .map_err(|e| NbError::usage(format!("bad url {}: {e}", req.url)))?;
        let mut attempt = 0u32;
        loop {
            self.gate().await;
            let r = match self.send_following(&req, base.clone()).await {
                Ok(r) => r,
                Err(_) if req.retry && attempt < self.max_retries => {
                    attempt += 1;
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
                Err(e) => return Err(transport(&e, &req.url)),
            };
            let s = r.status;
            if s == 429 || s >= 500 {
                if req.retry && attempt < self.max_retries {
                    attempt += 1;
                    let ra = r
                        .headers
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| {
                            v.bytes()
                                .all(|b| b.is_ascii_digit())
                                .then(|| v.parse::<u64>().ok())
                                .flatten()
                        });
                    tokio::time::sleep(ra.map_or_else(|| backoff(attempt), Duration::from_secs))
                        .await;
                    continue;
                }
                if s == 429 {
                    return Err(
                        NbError::rate_limited("rate limited by Notability").with_status(429)
                    );
                }
                return Err(NbError::server(format!("HTTP {s} from {}", req.url)).with_status(s));
            }
            if s == 401 || s == 403 {
                return Err(NbError::auth(format!(
                    "HTTP {s}: session rejected (run `notability account login`)"
                ))
                .with_status(s));
            }
            if s == 404 {
                return Err(
                    NbError::not_found(format!("HTTP 404 for {}", req.url)).with_status(404)
                );
            }
            return Ok(r);
        }
    }

    async fn send_following(
        &self,
        req: &Req,
        mut url: Url,
    ) -> std::result::Result<Resp, reqwest::Error> {
        if !req.query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(req.query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }
        let mut method = req.method.clone();
        let mut with_body = true;
        for hop in 0..=5 {
            if hop > 0 && !self.is_api_origin(&url) {
                let m = if method == Method::HEAD {
                    Method::HEAD
                } else {
                    Method::GET
                };
                return collect(self.external.request(m, url).send().await?).await;
            }
            let api = self.is_api_origin(&url);
            let mut b = self.inner.request(method.clone(), url.clone());
            if api {
                b = b.header("cookie", format!("{AUTH_COOKIE}={}", self.token()));
                if req.auth {
                    b = b.header(AUTH_COOKIE, self.token());
                }
            }
            for (k, v) in &req.headers {
                b = b.header(k.as_str(), v.as_str());
            }
            if let Some(t) = req.timeout {
                b = b.timeout(t);
            }
            if with_body {
                b = match &req.body {
                    Body::None => b,
                    Body::Json(v) => b.json(v),
                    Body::Bytes(x) => b.body(x.clone()),
                };
            }
            let r = b.send().await?;
            let st = r.status();
            if st.is_redirection()
                && st != StatusCode::NOT_MODIFIED
                && let Some(next) = r
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|l| url.join(l).ok())
            {
                if matches!(st.as_u16(), 301..=303) && method != Method::HEAD {
                    method = Method::GET;
                    with_body = false;
                }
                url = next;
                continue;
            }
            return collect(r).await;
        }
        collect(self.inner.get(url).send().await?).await
    }

    /// Fetch a third-party URL (e.g. a signed storage URL) with no Notability credentials.
    /// The cookie- and token-free client used for signed storage URLs.
    pub(crate) fn external_client(&self) -> &reqwest::Client {
        &self.external
    }

    pub async fn fetch_external(&self, url: &str) -> Result<Resp> {
        self.fetch_external_with(url, &[]).await
    }

    /// Cookie-less GET of a non-Notability URL with extra headers (e.g. a Drive bearer token).
    pub async fn fetch_external_with(&self, url: &str, headers: &[(&str, String)]) -> Result<Resp> {
        let u = Url::parse(url).map_err(|e| NbError::usage(format!("bad url {url}: {e}")))?;
        if self.is_api_origin(&u) {
            return Err(NbError::usage(
                "fetch_external is only for non-Notability hosts",
            ));
        }
        self.gate().await;
        let mut b = self.external.get(u);
        for (k, v) in headers {
            b = b.header(*k, v.as_str());
        }
        let r = b.send().await.map_err(|e| transport(&e, url))?;
        collect(r).await.map_err(|e| transport(&e, url))
    }

    /// Cookie-less PUT of bytes to a signed upload URL.
    pub async fn put_external(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Result<Resp> {
        self.gate().await;
        let mut b = self.external.put(url);
        for (k, v) in headers {
            b = b.header(*k, v.as_str());
        }
        let r = b
            .body(body)
            .send()
            .await
            .map_err(|e| transport(&e, "signed upload URL"))?;
        collect(r)
            .await
            .map_err(|e| transport(&e, "signed upload URL"))
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(2u64.saturating_pow(attempt).min(10))
}

async fn collect(r: reqwest::Response) -> std::result::Result<Resp, reqwest::Error> {
    let status = r.status().as_u16();
    let headers = r.headers().clone();
    let url = r.url().clone();
    let body = r.bytes().await?.to_vec();
    Ok(Resp {
        status,
        headers,
        body,
        url,
    })
}
