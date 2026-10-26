//! Imports that need a conversion or a download first, as the web app does them client-side:
//! Office files are converted to PDF (the web app uses an in-browser converter; here LibreOffice),
//! PDF links are fetched without Notability credentials, and Google Drive files are read with the
//! user's Drive OAuth token (`files/{id}?alt=media`, or `/export?mimeType=application/pdf` for
//! Google Docs, Sheets and Slides).

use std::path::{Path, PathBuf};
use std::process::Command;

use nb_api::{NbError, Result};
use serde_json::Value;

use crate::client::Client;
use crate::media::{ext_of, stem_of};

pub const OFFICE_EXTS: [&str; 9] = [
    ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".odt", ".ods", ".odp",
];
const PDF: &str = "application/pdf";
const GOOGLE_APPS: &str = "application/vnd.google-apps.";
const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";

pub fn is_office(name: &str) -> bool {
    OFFICE_EXTS.contains(&ext_of(name).as_str())
}

fn office_mime(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "application/msword" => ".doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => ".docx",
        "application/vnd.ms-excel" => ".xls",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => ".xlsx",
        "application/vnd.ms-powerpoint" => ".ppt",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => ".pptx",
        "application/vnd.oasis.opendocument.text" => ".odt",
        "application/vnd.oasis.opendocument.spreadsheet" => ".ods",
        "application/vnd.oasis.opendocument.presentation" => ".odp",
        _ => return None,
    })
}

fn soffice() -> Result<String> {
    if let Ok(p) = std::env::var("NOTABILITY_SOFFICE")
        && !p.is_empty()
    {
        return Ok(p);
    }
    let mut cands = vec!["soffice".to_string(), "libreoffice".to_string()];
    if cfg!(target_os = "macos") {
        cands.push("/Applications/LibreOffice.app/Contents/MacOS/soffice".into());
    }
    cands
        .into_iter()
        .find(|c| {
            Command::new(c)
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
        })
        .ok_or_else(|| {
            NbError::usage(
                "Office import needs LibreOffice (soffice) on PATH, or NOTABILITY_SOFFICE=/path/to/soffice",
            )
        })
}

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Result<Self> {
        let p = std::env::temp_dir().join(format!("notability-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).map_err(|e| NbError::generic(format!("temp dir: {e}")))?;
        Ok(Self(p))
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Convert an Office document to PDF; returns the PDF bytes and `<stem>.pdf`.
pub fn office_to_pdf(path: &Path) -> Result<(Vec<u8>, String)> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document")
        .to_string();
    if !is_office(&name) {
        return Err(NbError::usage(format!(
            "{name}: not an Office file ({})",
            OFFICE_EXTS.join(" ")
        )));
    }
    if !path.is_file() {
        return Err(NbError::usage(format!("no such file: {}", path.display())));
    }
    let bin = soffice()?;
    let out = TempDir::new()?;
    let profile = TempDir::new()?;
    let r = Command::new(&bin)
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.0.display()
        ))
        .args([
            "--headless",
            "--norestore",
            "--convert-to",
            "pdf",
            "--outdir",
        ])
        .arg(&out.0)
        .arg(path)
        .output()
        .map_err(|e| NbError::usage(format!("cannot run {bin}: {e}")))?;
    let pdf = out.0.join(format!("{}.pdf", stem_of(&name)));
    if !r.status.success() || !pdf.is_file() {
        return Err(NbError::usage(format!(
            "{name}: conversion to PDF failed ({})",
            String::from_utf8_lossy(&r.stderr)
                .trim()
                .chars()
                .take(200)
                .collect::<String>()
        )));
    }
    let data = std::fs::read(&pdf).map_err(|e| NbError::generic(format!("{e}")))?;
    Ok((data, format!("{}.pdf", stem_of(&name))))
}

/// Convert in-memory Office bytes (e.g. downloaded from Drive).
pub fn office_bytes_to_pdf(data: &[u8], name: &str) -> Result<(Vec<u8>, String)> {
    let dir = TempDir::new()?;
    let p = dir.0.join(name.replace(['/', '\\'], "_"));
    std::fs::write(&p, data).map_err(|e| NbError::generic(format!("{e}")))?;
    office_to_pdf(&p)
}

fn is_pdf(data: &[u8]) -> bool {
    data.starts_with(b"%PDF")
}

/// A file name from `Content-Disposition`, else the URL's last path segment, ending in `.pdf`.
pub fn url_file_name(url: &url::Url, disposition: Option<&str>) -> String {
    let from_cd = disposition.and_then(|d| {
        d.split(';').map(str::trim).find_map(|p| {
            p.strip_prefix("filename=")
                .map(|v| v.trim_matches('"').to_string())
        })
    });
    let seg = url
        .path_segments()
        .and_then(|mut s| s.next_back().map(str::to_string))
        .filter(|s| !s.is_empty());
    let raw = from_cd.or(seg).unwrap_or_else(|| "document.pdf".into());
    let raw = percent_decode(&raw);
    if ext_of(&raw) == ".pdf" {
        raw
    } else {
        format!("{raw}.pdf")
    }
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_string())
}

/// The Drive file id in a share link (`/d/<id>/`, `?id=<id>`) or a bare id.
pub fn drive_id(s: &str) -> Result<String> {
    let s = s.trim();
    let ok = |id: &str| {
        id.len() >= 10
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    if let Ok(u) = url::Url::parse(s) {
        if let Some(id) = u
            .query_pairs()
            .find(|(k, _)| k == "id")
            .map(|(_, v)| v.into_owned())
            && ok(&id)
        {
            return Ok(id);
        }
        let segs: Vec<&str> = u.path_segments().map(|p| p.collect()).unwrap_or_default();
        if let Some(i) = segs.iter().position(|p| *p == "d")
            && let Some(id) = segs.get(i + 1)
            && ok(id)
        {
            return Ok((*id).to_string());
        }
        return Err(NbError::usage("no Drive file id in that link"));
    }
    if ok(s) {
        Ok(s.to_string())
    } else {
        Err(NbError::usage("not a Drive file id or link"))
    }
}

fn drive_base() -> String {
    std::env::var("NOTABILITY_DRIVE_API")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DRIVE_API.to_string())
}

impl Client {
    /// Fetch a PDF link without Notability credentials; rejects anything that is not a PDF.
    pub async fn fetch_pdf_url(&self, url: &str) -> Result<(Vec<u8>, String)> {
        let r = self.http.fetch_external(url).await?;
        if !(200..300).contains(&r.status) {
            return Err(NbError::usage(format!("{url}: HTTP {}", r.status)).with_status(r.status));
        }
        let ctype = r
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if !is_pdf(&r.body) {
            return Err(NbError::usage(format!(
                "{url}: not a PDF (content-type {})",
                if ctype.is_empty() { "unknown" } else { &ctype }
            )));
        }
        let cd = r
            .headers
            .get("content-disposition")
            .and_then(|v| v.to_str().ok());
        Ok((r.body.clone(), url_file_name(&r.url, cd)))
    }

    async fn drive_get(&self, url: &str, token: &str) -> Result<nb_api::http::Resp> {
        let r = self
            .http
            .fetch_external_with(url, &[("authorization", format!("Bearer {token}"))])
            .await?;
        match r.status {
            200..=299 => Ok(r),
            401 | 403 => Err(NbError::auth(
                "Google Drive rejected the token (needs the drive.readonly or drive.file scope)",
            )
            .with_status(r.status)),
            404 => Err(NbError::not_found(
                "Drive file not found (or not shared with this token)",
            )),
            s => Err(NbError::server(format!("Google Drive: HTTP {s}")).with_status(s)),
        }
    }

    /// Download a Drive file as PDF bytes: PDFs as-is, Google Docs/Sheets/Slides exported to PDF,
    /// Office files converted locally.
    pub async fn drive_pdf(
        &self,
        id_or_link: &str,
        token: &str,
    ) -> Result<(Vec<u8>, String, Value)> {
        if token.trim().is_empty() {
            return Err(NbError::usage(
                "a Google Drive OAuth access token is required",
            ));
        }
        let id = drive_id(id_or_link)?;
        let base = drive_base();
        let meta = self
            .drive_get(
                &format!("{base}/files/{id}?fields=id,name,mimeType&supportsAllDrives=true"),
                token,
            )
            .await?
            .json()?;
        let name = meta["name"].as_str().unwrap_or("Drive file").to_string();
        let mime = meta["mimeType"].as_str().unwrap_or_default().to_string();
        let (data, file) = if mime.starts_with(GOOGLE_APPS) {
            let r = self
                .drive_get(&format!("{base}/files/{id}/export?mimeType={PDF}"), token)
                .await?;
            let n = if ext_of(&name) == ".pdf" {
                name.clone()
            } else {
                format!("{name}.pdf")
            };
            (r.body, n)
        } else {
            let r = self
                .drive_get(
                    &format!("{base}/files/{id}?alt=media&supportsAllDrives=true"),
                    token,
                )
                .await?;
            if mime == PDF || is_pdf(&r.body) {
                let n = if ext_of(&name) == ".pdf" {
                    name.clone()
                } else {
                    format!("{name}.pdf")
                };
                (r.body, n)
            } else if let Some(ext) = office_mime(&mime).or_else(|| is_office(&name).then_some(""))
            {
                let n = if ext.is_empty() || ext_of(&name) == ext {
                    name.clone()
                } else {
                    format!("{name}{ext}")
                };
                office_bytes_to_pdf(&r.body, &n)?
            } else {
                return Err(NbError::usage(format!(
                    "{name}: Drive file type {mime} cannot be imported (PDF, Office or Google Docs/Sheets/Slides)"
                )));
            }
        };
        if !is_pdf(&data) {
            return Err(NbError::server("Google Drive did not return a PDF"));
        }
        Ok((
            data,
            file,
            serde_json::json!({"id": id, "name": name, "mime_type": mime}),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_ids() {
        let id = "1AbCdEfGhIjKlMnOp_q-rs";
        assert_eq!(
            drive_id(&format!(
                "https://drive.google.com/file/d/{id}/view?usp=sharing"
            ))
            .unwrap(),
            id
        );
        assert_eq!(
            drive_id(&format!("https://drive.google.com/open?id={id}")).unwrap(),
            id
        );
        assert_eq!(
            drive_id(&format!("https://docs.google.com/document/d/{id}/edit")).unwrap(),
            id
        );
        assert_eq!(drive_id(id).unwrap(), id);
        assert!(drive_id("https://example.com/nothing").is_err());
        assert!(drive_id("bad id!").is_err());
    }

    #[test]
    fn url_names() {
        let u = url::Url::parse("https://example.com/papers/My%20Paper.pdf?x=1").unwrap();
        assert_eq!(url_file_name(&u, None), "My Paper.pdf");
        assert_eq!(
            url_file_name(&u, Some("attachment; filename=\"a.pdf\"")),
            "a.pdf"
        );
        let u = url::Url::parse("https://example.com/get?id=3").unwrap();
        assert_eq!(url_file_name(&u, None), "get.pdf");
    }

    #[test]
    fn office_types() {
        assert!(is_office("Deck.PPTX"));
        assert!(!is_office("a.pdf"));
        assert_eq!(office_mime("application/vnd.ms-excel"), Some(".xls"));
        assert!(office_to_pdf(Path::new("x.pdf")).is_err());
    }
}
