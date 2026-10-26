//! Title/folder-path search plus a local full-text cache of typed note text (on-disk layout:
//! `$NOTABILITY_CACHE_DIR/text/{sha1(id)[:2]}/{id}.json`).

use std::fs;
use std::path::PathBuf;

use nb_api::Result;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::client::Client;
use crate::models::Note;

pub const CONTENT_FETCH_LIMIT: usize = 40;

pub fn cache_dir() -> PathBuf {
    if let Some(b) = std::env::var_os("NOTABILITY_CACHE_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(b).join("text");
    }
    let root = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".cache"));
    root.join("notability-cli").join("text")
}

fn version(n: &Note) -> i64 {
    n.most_recent_op_time.or(n.updated_at).unwrap_or(0.0) as i64
}

fn path(id: &str) -> PathBuf {
    let h = Sha1::digest(id.as_bytes());
    cache_dir()
        .join(format!("{:02x}", h[0]))
        .join(format!("{id}.json"))
}

pub fn get(n: &Note) -> Option<String> {
    let d: Value = serde_json::from_slice(&fs::read(path(&n.id)).ok()?).ok()?;
    (d["version"].as_i64() == Some(version(n)))
        .then(|| d["text"].as_str().map(str::to_string))
        .flatten()
}

pub fn put(n: &Note, text: &str) -> std::io::Result<()> {
    let p = path(&n.id);
    if let Some(d) = p.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("tmp");
    let body = json!({"id": n.id, "version": version(n), "title": n.title, "text": text});
    fs::write(&tmp, body.to_string())?;
    fs::rename(tmp, p)
}

/// Notes that can contain typed text (TEXT index, or no index info yet).
pub fn searchable<'a>(notes: impl IntoIterator<Item = &'a Note>) -> Vec<&'a Note> {
    notes
        .into_iter()
        .filter(|n| n.content_types().iter().any(|t| t == "TEXT") || n.index_timestamps.is_empty())
        .collect()
}

pub async fn note_text(c: &Client, id: &str) -> Result<String> {
    let raw = c.http.note_bundle_raw(id).await?;
    if raw.starts_with(b"No ops found") {
        return Ok(String::new());
    }
    nb_protocol::bundle::decode_bundle(&raw)
        .map(|d| d.text())
        .map_err(|e| nb_api::NbError::server(format!("bad note bundle: {e}")))
}

/// Fill the cache for every stale or missing note (newest first).
pub async fn index(c: &Client, limit: Option<usize>, refresh: bool) -> Result<Value> {
    let lib = c.library().await?;
    let mut notes = searchable(lib.all_notes(false));
    notes.sort_by_key(|n| std::cmp::Reverse(version(n)));
    let (mut fetched, mut cached, mut failed) = (0, 0, 0);
    for n in &notes {
        if !refresh && get(n).is_some() {
            cached += 1;
            continue;
        }
        if limit.is_some_and(|l| fetched >= l) {
            continue;
        }
        match note_text(c, &n.id).await {
            Ok(t) if put(n, &t).is_ok() => fetched += 1,
            _ => failed += 1,
        }
    }
    Ok(
        json!({"candidates": notes.len(), "fetched": fetched, "already_cached": cached, "failed": failed,
              "remaining": notes.len().saturating_sub(fetched + cached + failed),
              "cache_dir": cache_dir().display().to_string()}),
    )
}

pub fn clear() -> usize {
    fn walk(p: &std::path::Path, n: &mut usize) {
        for e in fs::read_dir(p).into_iter().flatten().flatten() {
            let q = e.path();
            if q.is_dir() {
                walk(&q, n);
            } else if q.extension().is_some_and(|x| x == "json") && fs::remove_file(&q).is_ok() {
                *n += 1;
            }
        }
    }
    let mut n = 0;
    walk(&cache_dir(), &mut n);
    n
}

pub async fn search(c: &Client, query: &str, content: bool, limit: usize) -> Result<Vec<Value>> {
    let ql = query.to_lowercase();
    let lib = c.library().await?;
    let fm = lib.folder_map();
    let mut all = lib.all_notes(false);
    all.sort_by(|a, b| {
        b.updated_at
            .unwrap_or(0.0)
            .total_cmp(&a.updated_at.unwrap_or(0.0))
    });
    let mut hits = Vec::new();
    for n in &all {
        let path = lib.folder_path(n.folder_id.as_deref()).unwrap_or_default();
        if n.title.to_lowercase().contains(&ql) || path.to_lowercase().contains(&ql) {
            let mut h = n.summary(&fm);
            h["folder_path"] = json!(path);
            h["match"] = json!("title");
            hits.push(h);
        }
    }
    if content {
        let seen: Vec<String> = hits
            .iter()
            .filter_map(|h| h["id"].as_str().map(str::to_string))
            .collect();
        let mut cands = searchable(lib.all_notes(false));
        cands.sort_by_key(|n| std::cmp::Reverse(version(n)));
        let mut fetched = 0;
        for n in cands {
            if seen.contains(&n.id) {
                continue;
            }
            let text = match get(n) {
                Some(t) => t,
                None if fetched >= CONTENT_FETCH_LIMIT => continue,
                None => {
                    fetched += 1;
                    let Ok(t) = note_text(c, &n.id).await else {
                        continue;
                    };
                    // A cache write failure only costs a refetch next time.
                    let _ = put(n, &t);
                    t
                }
            };
            if let Some(snippet) = snippet(&text, query) {
                let mut h = n.summary(&fm);
                h["folder_path"] = json!(lib.folder_path(n.folder_id.as_deref()));
                h["match"] = json!("content");
                h["snippet"] = json!(snippet);
                hits.push(h);
            }
        }
    }
    hits.truncate(limit);
    Ok(hits)
}

/// ±60 chars around the first case-insensitive hit, newlines flattened.
pub fn snippet(text: &str, query: &str) -> Option<String> {
    let t: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() || lower.len() != t.len() || q.len() > lower.len() {
        return None;
    }
    let i = (0..=lower.len() - q.len()).find(|&i| lower[i..].starts_with(&q))?;
    let end = (i + query.chars().count() + 60).min(t.len());
    Some(
        t[i.saturating_sub(60)..end]
            .iter()
            .collect::<String>()
            .replace('\n', " "),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn snippet_window() {
        let text = format!("{}needle{}", "a".repeat(100), "\nb".repeat(50));
        let s = super::snippet(&text, "NEEDLE").unwrap();
        assert_eq!(s.chars().count(), 60 + 6 + 60);
        assert!(!s.contains('\n'));
        assert!(super::snippet("abc", "zz").is_none());
    }
}
