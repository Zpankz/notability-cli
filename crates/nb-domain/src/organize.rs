//! Recently Deleted, folder appearance/nesting/order and legacy subjects/dividers
//! (`/metadata` mutations, same payloads as the web app).

use nb_api::{NbError, Result};
use serde_json::{Map, Value, json};

use crate::client::{Client, merge, resolve_folder};
use crate::models::{Folder, Library, Note, ms_to_iso};

pub const RETENTION_DAYS: f64 = 30.0;
pub const DEFAULT_COLOR: &str = "#8E8E93";
pub const FOLDER_COLORS: [(&str, &str); 15] = [
    ("crimson", "#A40848"),
    ("red", "#BF1818"),
    ("orange", "#ED5E00"),
    ("yellow", "#F89E15"),
    ("green", "#1E7022"),
    ("teal", "#005B4D"),
    ("cyan", "#007783"),
    ("blue", "#0957B9"),
    ("navy", "#182588"),
    ("indigo", "#351796"),
    ("purple", "#5C0D8F"),
    ("slate", "#273840"),
    ("brown", "#3F241E"),
    ("black", "#080808"),
    ("gray", "#525252"),
];

/// Glob match when the pattern has wildcards, else case-sensitive prefix match.
pub fn title_match(title: &str, pat: &str) -> bool {
    if pat.contains(['*', '?', '[']) {
        glob(
            title.chars().collect::<Vec<_>>().as_slice(),
            &pat.chars().collect::<Vec<_>>(),
        )
    } else {
        title.starts_with(pat)
    }
}

fn glob(t: &[char], p: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') => (0..=t.len()).any(|i| glob(&t[i..], &p[1..])),
        Some('?') => !t.is_empty() && glob(&t[1..], &p[1..]),
        Some('[') => {
            let Some(end) = p.iter().skip(2).position(|&c| c == ']').map(|i| i + 2) else {
                return t.first() == Some(&'[') && glob(&t[1..], &p[1..]);
            };
            let Some(&c) = t.first() else { return false };
            let (neg, set) = if p[1] == '!' {
                (true, &p[2..end])
            } else {
                (false, &p[1..end])
            };
            let mut hit = false;
            let mut i = 0;
            while i < set.len() {
                if i + 2 < set.len() && set[i + 1] == '-' {
                    hit |= set[i] <= c && c <= set[i + 2];
                    i += 3;
                } else {
                    hit |= set[i] == c;
                    i += 1;
                }
            }
            hit != neg && glob(&t[1..], &p[end + 1..])
        }
        Some(&c) => t.first() == Some(&c) && glob(&t[1..], &p[1..]),
    }
}

pub fn trash_rows(lib: &Library, pat: Option<&str>) -> Vec<Value> {
    let fm = lib.folder_map();
    let mut notes = lib.deleted_notes();
    notes.sort_by(|a, b| {
        b.deleted_at
            .unwrap_or(0.0)
            .total_cmp(&a.deleted_at.unwrap_or(0.0))
    });
    notes
        .into_iter()
        .filter(|n| pat.is_none_or(|p| title_match(&n.title, p)))
        .map(|n| {
            let mut s = n.summary(&fm);
            s["deleted_at"] = ms_to_iso(n.deleted_at);
            s["purge_after"] = ms_to_iso(Some(
                n.deleted_at.unwrap_or(0.0) + RETENTION_DAYS * 86_400_000.0,
            ));
            s["folder_path"] = json!(lib.folder_path(n.folder_id.as_deref()));
            s
        })
        .collect()
}

pub fn resolve_trashed(lib: &Library, r: &str) -> Result<Note> {
    let del = lib.deleted_notes();
    if let Some(n) = del.iter().find(|n| n.id == r) {
        return Ok((*n).clone());
    }
    let pref: Vec<_> = del
        .iter()
        .filter(|n| r.len() >= 6 && n.id.starts_with(r))
        .collect();
    let exact: Vec<_> = del.iter().filter(|n| n.title == r).collect();
    for c in [pref, exact] {
        match c.len() {
            1 => return Ok((**c[0]).clone()),
            0 => {}
            k => {
                return Err(NbError::usage(format!(
                    "'{r}' matches {k} notes in Recently Deleted; use the note id"
                )));
            }
        }
    }
    Err(NbError::not_found(format!(
        "no note in Recently Deleted matches: {r}"
    )))
}

pub fn trash_plan(lib: &Library, pat: Option<&str>, all: bool) -> Result<Vec<Note>> {
    match (pat, all) {
        (None, false) => Err(NbError::usage(
            "refusing to empty Recently Deleted without a scope: pass --match PATTERN or --all",
        )),
        (Some(_), true) => Err(NbError::usage("--match and --all are mutually exclusive")),
        _ => Ok(lib
            .deleted_notes()
            .into_iter()
            .filter(|n| all || title_match(&n.title, pat.unwrap_or_default()))
            .cloned()
            .collect()),
    }
}

pub fn normalize_color(c: &str) -> Result<String> {
    let lc = c.to_lowercase();
    let v = FOLDER_COLORS
        .iter()
        .find(|(k, _)| *k == lc)
        .map_or(c, |(_, v)| v);
    let ok = v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !ok {
        let names: Vec<_> = FOLDER_COLORS.iter().map(|(k, _)| *k).collect();
        return Err(NbError::usage(format!(
            "invalid colour '{c}': use #RRGGBB or one of {}",
            names.join(", ")
        )));
    }
    Ok(v.to_uppercase())
}

fn siblings<'a>(lib: &'a Library, parent: Option<&str>, exclude: Option<&str>) -> Vec<&'a Folder> {
    let ws = &lib.private_space.id;
    let mut v: Vec<_> = lib
        .all_folders()
        .into_iter()
        .filter(|f| f.parent_id.as_deref() == parent && Some(f.id.as_str()) != exclude)
        .filter(|f| f.workspace_id.as_ref().is_none_or(|w| w == ws))
        .collect();
    v.sort_by(|a, b| {
        a.sibling_order
            .unwrap_or(0.0)
            .total_cmp(&b.sibling_order.unwrap_or(0.0))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    v
}

pub fn order_between(lo: Option<f64>, hi: Option<f64>) -> f64 {
    match (lo, hi) {
        (None, None) => 0.0,
        (None, Some(h)) => h - 1.0,
        (Some(l), None) => l + 1.0,
        (Some(l), Some(h)) => (l + h) / 2.0,
    }
}

pub fn plan_order(sibs: &[&Folder], position: &str, anchor: Option<&str>) -> Result<f64> {
    let o: Vec<Option<f64>> = sibs.iter().map(|f| f.sibling_order).collect();
    match position {
        "first" => Ok(order_between(None, o.first().copied().flatten())),
        "last" => Ok(order_between(o.last().copied().flatten(), None)),
        "before" | "after" => {
            let i = sibs
                .iter()
                .position(|f| Some(f.id.as_str()) == anchor)
                .ok_or_else(|| {
                    NbError::usage("the anchor folder must be a sibling in the destination parent")
                })?;
            Ok(if position == "before" {
                order_between(if i > 0 { o[i - 1] } else { None }, o[i])
            } else {
                order_between(o[i], o.get(i + 1).copied().flatten())
            })
        }
        p => Err(NbError::usage(format!("unknown position '{p}'"))),
    }
}

impl Client {
    pub async fn trash_restore(&self, r: &str) -> Result<Value> {
        let n = resolve_trashed(&self.library().await?, r)?;
        let res = self.mutate("undeleteNote", json!({"noteId": n.id})).await?;
        Ok(merge(
            json!({"id": n.id, "title": n.title, "restored": true}),
            res,
        ))
    }

    pub async fn trash_purge(&self, r: &str) -> Result<Value> {
        let n = resolve_trashed(&self.library().await?, r)?;
        let res = self
            .mutate("permanentlyDeleteNote", json!({"noteId": n.id}))
            .await?;
        Ok(merge(
            json!({"id": n.id, "title": n.title, "purged": true}),
            res,
        ))
    }

    pub async fn trash_empty(&self, pat: Option<&str>, all: bool) -> Result<Value> {
        let plan = trash_plan(&self.library().await?, pat, all)?;
        let (mut done, mut failed) = (Vec::new(), Vec::new());
        for n in plan {
            match self
                .mutate("permanentlyDeleteNote", json!({"noteId": n.id}))
                .await
            {
                Ok(_) => done.push(json!({"id": n.id, "title": n.title})),
                Err(e) => failed.push(json!({"id": n.id, "title": n.title, "error": e.message})),
            }
        }
        let scope = if all { json!("all") } else { json!(pat) };
        Ok(json!({"scope": scope, "count": done.len(), "purged": done, "failed": failed}))
    }

    pub async fn folder_edit(
        &self,
        r: &str,
        title: Option<&str>,
        color: Option<&str>,
        emoji: Option<&str>,
        clear_emoji: bool,
        locked: Option<bool>,
    ) -> Result<Value> {
        let f = self.resolve_folder(r).await?;
        let mut p = Map::new();
        p.insert("folderId".into(), json!(f.id));
        p.insert("title".into(), json!(title.unwrap_or(&f.title)));
        if let Some(c) = color {
            p.insert("hexColor".into(), json!(normalize_color(c)?));
        }
        if clear_emoji {
            p.insert("emoji".into(), Value::Null);
        } else if let Some(e) = emoji {
            p.insert("emoji".into(), json!(e));
        }
        if let Some(l) = locked {
            p.insert("isLocked".into(), json!(l));
        }
        if p.len() == 2 && title.is_none() {
            return Err(NbError::usage(
                "nothing to change: pass --title, --color, --emoji/--no-emoji or --lock/--unlock",
            ));
        }
        let change: Map<String, Value> = p
            .iter()
            .filter(|(k, _)| *k != "folderId")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let res = self.mutate("modifyFolder", Value::Object(p)).await?;
        Ok(merge(
            json!({"id": f.id, "before": {"title": f.title, "color": f.color, "emoji": f.emoji, "locked": f.locked},
                   "change": change}),
            res,
        ))
    }

    pub async fn folder_move(
        &self,
        r: &str,
        parent: Option<&str>,
        position: &str,
        anchor: Option<&str>,
    ) -> Result<Value> {
        let lib = self.library().await?;
        let f = resolve_folder(&lib, r)?;
        let pid = parent
            .map(|p| resolve_folder(&lib, p))
            .transpose()?
            .map(|f| f.id);
        let (desc, _, _) = Client::subtree(&lib, &f.id);
        if let Some(p) = &pid
            && (p == &f.id || desc.contains(p))
        {
            return Err(NbError::usage(
                "cannot move a folder into itself or one of its subfolders",
            ));
        }
        let anchor_id = anchor
            .map(|a| resolve_folder(&lib, a))
            .transpose()?
            .map(|f| f.id);
        let order = plan_order(
            &siblings(&lib, pid.as_deref(), Some(&f.id)),
            position,
            anchor_id.as_deref(),
        )?;
        let ws = f
            .workspace_id
            .clone()
            .unwrap_or_else(|| lib.private_space.id.clone());
        let res = self
            .mutate(
                "modifyFolder",
                json!({"folderId": f.id, "siblingOrder": order, "moveToParent": {"parentId": pid, "workspaceId": ws}}),
            )
            .await?;
        let to_path = pid.as_deref().and_then(|p| lib.folder_path(Some(p)));
        Ok(merge(
            json!({"id": f.id, "title": f.title, "from_parent": f.parent_id, "to_parent": pid, "to_path": to_path,
                   "sibling_order": order}),
            res,
        ))
    }

    pub async fn folder_reorder(
        &self,
        r: &str,
        position: &str,
        anchor: Option<&str>,
    ) -> Result<Value> {
        let lib = self.library().await?;
        let f = resolve_folder(&lib, r)?;
        let anchor_id = anchor
            .map(|a| resolve_folder(&lib, a))
            .transpose()?
            .map(|f| f.id);
        let order = plan_order(
            &siblings(&lib, f.parent_id.as_deref(), Some(&f.id)),
            position,
            anchor_id.as_deref(),
        )?;
        let res = self
            .mutate(
                "modifyFolder",
                json!({"folderId": f.id, "siblingOrder": order}),
            )
            .await?;
        Ok(merge(
            json!({"id": f.id, "title": f.title, "old_order": f.sibling_order, "sibling_order": order}),
            res,
        ))
    }

    pub async fn ordered_children(&self, parent: Option<&str>) -> Result<Vec<Value>> {
        let lib = self.library().await?;
        let pid = parent
            .map(|p| resolve_folder(&lib, p))
            .transpose()?
            .map(|f| f.id);
        Ok(siblings(&lib, pid.as_deref(), None)
            .into_iter()
            .map(|f| json!({"id": f.id, "title": f.title, "sibling_order": f.sibling_order, "color": f.color,
                            "emoji": f.emoji, "locked": f.locked}))
            .collect())
    }

    pub async fn organizers(&self, kind: Option<&str>) -> Result<Vec<Value>> {
        Ok(organizers(&self.library().await?, kind))
    }

    pub async fn organizer_create(
        &self,
        kind: &str,
        title: &str,
        parent: Option<&str>,
        color: &str,
    ) -> Result<Value> {
        let lib = self.library().await?;
        let pid = parent
            .map(|p| resolve_folder(&lib, p))
            .transpose()?
            .map(|f| f.id);
        let oid = uuid::Uuid::new_v4().to_string();
        let order = siblings(&lib, pid.as_deref(), None)
            .iter()
            .filter_map(|f| f.sibling_order)
            .reduce(f64::min)
            .map_or(0.0, |m| m - 1.0);
        let res = match kind {
            "subject" => {
                self.mutate(
                    "createSubject",
                    json!({"id": oid, "hexColor": normalize_color(color)?, "isLocked": false, "title": title,
                           "siblingOrder": order, "workspaceId": lib.private_space.id, "parentId": pid}),
                )
                .await?
            }
            "divider" => {
                self.mutate("createDivider", json!({"id": oid, "title": title, "siblingOrder": order, "parentId": pid}))
                    .await?
            }
            k => return Err(NbError::usage(format!("kind must be subject or divider, not '{k}'"))),
        };
        Ok(merge(
            json!({"id": oid, "type": kind, "title": title, "parent_id": pid}),
            res,
        ))
    }

    pub async fn organizer_edit(
        &self,
        r: &str,
        title: Option<&str>,
        color: Option<&str>,
        locked: Option<bool>,
        emoji: Option<&str>,
    ) -> Result<Value> {
        let o = resolve_organizer(&self.library().await?, r)?;
        let mut p = Map::new();
        p.insert("organizerId".into(), o["id"].clone());
        if let Some(t) = title {
            p.insert("title".into(), json!(t));
        }
        if let Some(c) = color {
            p.insert("subjectHexColor".into(), json!(normalize_color(c)?));
        }
        if let Some(l) = locked {
            p.insert("subjectIsLocked".into(), json!(l));
        }
        if let Some(e) = emoji {
            p.insert("subjectEmoji".into(), json!(e));
        }
        if p.len() == 1 {
            return Err(NbError::usage("nothing to change"));
        }
        if o["type"] == "divider" && p.keys().any(|k| k != "organizerId" && k != "title") {
            return Err(NbError::usage("dividers only have a title"));
        }
        let res = self
            .mutate("modifyOrganizer", Value::Object(p.clone()))
            .await?;
        let mut out = json!({"id": o["id"], "type": o["type"], "old_title": o["title"]});
        for (k, v) in p {
            out[k] = v;
        }
        Ok(merge(out, res))
    }

    pub async fn organizer_delete(&self, r: &str) -> Result<Value> {
        let lib = self.library().await?;
        let o = resolve_organizer(&lib, r)?;
        let id = o["id"].as_str().unwrap_or_default();
        let kids = lib
            .all_folders()
            .iter()
            .filter(|f| f.parent_id.as_deref() == Some(id))
            .count();
        let notes = lib
            .all_notes(true)
            .iter()
            .filter(|n| n.folder_id.as_deref() == Some(id))
            .count();
        if kids + notes > 0 {
            return Err(NbError::usage(format!(
                "{} '{}' is not empty ({kids} organizer(s), {notes} note(s))",
                o["type"].as_str().unwrap_or_default(),
                o["title"].as_str().unwrap_or_default()
            )));
        }
        let res = self
            .mutate("deleteOrganizer", json!({"organizerId": id}))
            .await?;
        Ok(merge(
            json!({"id": id, "type": o["type"], "title": o["title"]}),
            res,
        ))
    }
}

pub fn organizers(lib: &Library, kind: Option<&str>) -> Vec<Value> {
    let mut rows: Vec<(f64, Value)> = lib
        .all_folders()
        .into_iter()
        .filter_map(|f| {
            let t = f.organizer_type.as_deref().unwrap_or("Folder").to_lowercase();
            ((t == "subject" || t == "divider") && kind.is_none_or(|k| k == t)).then(|| {
                (
                    f.sibling_order.unwrap_or(0.0),
                    json!({"id": f.id, "type": t, "title": f.title, "parent_id": f.parent_id, "sibling_order": f.sibling_order}),
                )
            })
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|r| r.1).collect()
}

pub fn resolve_organizer(lib: &Library, r: &str) -> Result<Value> {
    let rows = organizers(lib, None);
    let mut hit: Vec<&Value> = rows.iter().filter(|o| o["id"] == r).collect();
    if hit.is_empty() {
        hit = rows.iter().filter(|o| o["title"] == r).collect();
    }
    match hit.len() {
        0 => Err(NbError::not_found(format!(
            "no subject/divider matching '{r}' (plain folders: use `folders`)"
        ))),
        1 => Ok(hit[0].clone()),
        k => Err(NbError::usage(format!(
            "{k} organizers are titled '{r}'; pass the id"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_like_fnmatchcase() {
        assert!(title_match("Lecture 3", "Lecture*"));
        assert!(title_match("sample-RS a", "sample-RS"));
        assert!(!title_match("lecture", "Lecture*"));
        assert!(title_match("a1", "a[0-9]"));
        assert!(title_match("ab", "a?"));
    }

    #[test]
    fn orders() {
        assert_eq!(order_between(Some(1.0), Some(2.0)), 1.5);
        assert_eq!(order_between(None, Some(2.0)), 1.0);
        assert_eq!(normalize_color("red").unwrap(), "#BF1818");
        assert!(normalize_color("#12345").is_err());
    }
}
