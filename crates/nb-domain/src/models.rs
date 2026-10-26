//! Library payload from the `/metadata` socket. Unknown fields are kept in `extra`.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Epoch milliseconds to RFC 3339 UTC with microsecond precision.
pub fn ms_to_iso(ms: Option<f64>) -> Value {
    let Some(ms) = ms else { return Value::Null };
    let total_us = (ms * 1000.0).round() as i64;
    let secs = total_us.div_euclid(1_000_000);
    let us = total_us.rem_euclid(1_000_000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let base = format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        tod / 3600,
        tod % 3600 / 60,
        tod % 60
    );
    Value::String(if us == 0 {
        format!("{base}Z")
    } else {
        format!("{base}.{us:06}Z")
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserPermission {
    pub owner_id: Option<String>,
    pub shared_with_me: bool,
    pub user_access_level: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexTimestamp {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub created_at: Option<f64>,
    pub updated_at: Option<f64>,
    pub most_recent_op_time: Option<f64>,
    pub last_opened: Option<f64>,
    pub folder_id: Option<String>,
    pub workspace_id: Option<String>,
    pub is_favorited: bool,
    pub has_recordings: bool,
    pub thumbnail_url: Option<String>,
    pub user_permission: Option<UserPermission>,
    pub index_timestamps: Vec<IndexTimestamp>,
    pub has_user_shares: bool,
    pub deleted_at: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Note {
    pub fn content_types(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .index_timestamps
            .iter()
            .map(|i| i.kind.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn shared(&self) -> bool {
        self.has_user_shares
            || self
                .user_permission
                .as_ref()
                .is_some_and(|p| p.shared_with_me)
    }

    pub fn summary(&self, folders: &HashMap<String, Folder>) -> Value {
        let folder = self
            .folder_id
            .as_ref()
            .and_then(|f| folders.get(f))
            .map(|f| f.title.clone());
        json!({
            "id": self.id, "title": self.title, "folder_id": self.folder_id, "folder": folder,
            "created_at": ms_to_iso(self.created_at), "updated_at": ms_to_iso(self.updated_at),
            "last_opened": ms_to_iso(self.last_opened), "favorite": self.is_favorited,
            "has_recordings": self.has_recordings, "content": self.content_types(), "shared": self.shared(),
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Folder {
    pub id: String,
    pub title: String,
    pub parent_id: Option<String>,
    pub sibling_order: Option<f64>,
    pub workspace_id: Option<String>,
    pub color: Option<String>,
    pub emoji: Option<String>,
    pub locked: bool,
    pub created_at: Option<f64>,
    pub updated_at: Option<f64>,
    pub organizer_type: Option<String>,
}

impl Folder {
    pub fn summary(&self, notes: Option<usize>, path: Option<String>) -> Value {
        let mut d = json!({
            "id": self.id, "title": self.title, "parent_id": self.parent_id, "path": path,
            "color": self.color, "emoji": self.emoji, "locked": self.locked,
            "updated_at": ms_to_iso(self.updated_at),
        });
        if let Some(n) = notes {
            d["notes"] = json!(n);
        }
        d
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Workspace {
    pub id: String,
    pub title: String,
    pub folders: Vec<Folder>,
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Library {
    pub sequence_id: Option<u64>,
    pub private_space: Workspace,
    pub shared_spaces: Vec<Workspace>,
    pub shared_notes: Vec<Note>,
}

impl Library {
    pub fn from_value(v: Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(v)
    }

    fn every_note(&self) -> impl Iterator<Item = &Note> {
        self.private_space
            .notes
            .iter()
            .chain(self.shared_spaces.iter().flat_map(|w| w.notes.iter()))
            .chain(self.shared_notes.iter())
    }

    /// Live notes (Recently Deleted excluded unless `include_deleted`).
    pub fn all_notes(&self, include_deleted: bool) -> Vec<&Note> {
        self.every_note()
            .filter(|n| include_deleted || n.deleted_at.is_none_or(|d| d == 0.0))
            .collect()
    }

    pub fn deleted_notes(&self) -> Vec<&Note> {
        self.every_note()
            .filter(|n| n.deleted_at.is_some_and(|d| d != 0.0))
            .collect()
    }

    pub fn all_folders(&self) -> Vec<&Folder> {
        self.private_space
            .folders
            .iter()
            .chain(self.shared_spaces.iter().flat_map(|w| w.folders.iter()))
            .collect()
    }

    pub fn folder_map(&self) -> HashMap<String, Folder> {
        self.all_folders()
            .into_iter()
            .map(|f| (f.id.clone(), f.clone()))
            .collect()
    }

    pub fn folder_path(&self, folder_id: Option<&str>) -> Option<String> {
        let fm = self.folder_map();
        let (mut parts, mut seen) = (Vec::new(), std::collections::HashSet::new());
        let mut cur = folder_id.map(str::to_string);
        while let Some(c) = cur {
            let Some(f) = fm.get(&c) else { break };
            if !seen.insert(c.clone()) {
                break;
            }
            parts.push(f.title.clone());
            cur = f.parent_id.clone();
        }
        parts.reverse();
        (!parts.is_empty()).then(|| parts.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_microsecond_format() {
        assert_eq!(ms_to_iso(Some(0.0)), "1970-01-01T00:00:00Z");
        assert_eq!(
            ms_to_iso(Some(1_761_000_000_123.0)),
            "2025-10-20T22:40:00.123000Z"
        );
        assert_eq!(ms_to_iso(None), Value::Null);
    }

    #[test]
    fn fixture_library_parses() {
        let raw = include_str!("../tests/fixtures/library.json");
        let lib = Library::from_value(serde_json::from_str(raw).unwrap()).unwrap();
        assert!(!lib.all_notes(true).is_empty());
        for f in lib.all_folders() {
            let _ = lib.folder_path(Some(&f.id));
        }
    }
}
