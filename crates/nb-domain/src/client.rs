//! `Client`: an authenticated `Http` plus a cached library snapshot, note/folder resolution,
//! and the Socket.IO library mutations (folders, notes, favourites, trash).

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nb_api::socketio::{MetadataSocket, mutation};
use nb_api::{Http, NbError, Result};
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};

use crate::models::{Folder, Library, Note};

pub const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
/// Ops per `append` request when seeding a new note.
pub const APPEND_CHUNK: usize = 500;

pub struct Client {
    pub http: Http,
    library: Mutex<Option<Library>>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(m), Value::Object(o)) = (a.as_object_mut(), b) {
        m.extend(o);
    }
    a
}

impl Client {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            library: Mutex::new(None),
        }
    }

    fn cached(&self) -> Option<Library> {
        self.library.lock().ok().and_then(|g| g.clone())
    }

    pub fn invalidate(&self) {
        if let Ok(mut g) = self.library.lock() {
            *g = None;
        }
    }

    pub async fn library(&self) -> Result<Library> {
        if let Some(l) = self.cached() {
            return Ok(l);
        }
        self.http.gate().await;
        let mut s = MetadataSocket::connect(self.http.token(), SOCKET_TIMEOUT, None).await?;
        let raw = s.library().await;
        s.close().await;
        let lib = Library::from_value(raw?)
            .map_err(|e| NbError::server(format!("unexpected library payload: {e}")))?;
        if let Ok(mut g) = self.library.lock() {
            *g = Some(lib.clone());
        }
        Ok(lib)
    }

    pub async fn refresh(&self) -> Result<Library> {
        self.invalidate();
        self.library().await
    }

    pub async fn notes(&self, folder: Option<&str>, include_shared: bool) -> Result<Vec<Note>> {
        let lib = self.library().await?;
        let mut notes: Vec<Note> = if include_shared {
            lib.all_notes(false).into_iter().cloned().collect()
        } else {
            lib.private_space
                .notes
                .iter()
                .filter(|n| n.deleted_at.is_none())
                .cloned()
                .collect()
        };
        if let Some(f) = folder {
            let fid = if matches!(f, "" | "none" | "unfiled") {
                None
            } else {
                Some(resolve_folder(&lib, f)?.id)
            };
            notes.retain(|n| n.folder_id == fid);
        }
        Ok(notes)
    }

    pub async fn resolve_note(&self, r: &str) -> Result<Note> {
        resolve_note(&self.library().await?, r)
    }

    pub async fn resolve_folder(&self, r: &str) -> Result<Folder> {
        resolve_folder(&self.library().await?, r)
    }

    // ------------------------------------------------------------- mutations
    pub async fn mutate(&self, kind: &str, payload: Value) -> Result<Value> {
        let m = mutation(kind, payload);
        self.http.gate().await;
        let mut s = MetadataSocket::connect(self.http.token(), SOCKET_TIMEOUT, None).await?;
        let res = async {
            s.library().await?;
            s.mutate(std::slice::from_ref(&m)).await
        }
        .await;
        s.close().await;
        self.invalidate();
        Ok(merge(
            json!({"mutation": kind, "idempotency_key": m["idempotencyKey"]}),
            res?,
        ))
    }

    pub(crate) async fn workspace_id(&self) -> Result<String> {
        Ok(self.library().await?.private_space.id)
    }

    pub async fn create_folder(
        &self,
        title: &str,
        parent: Option<&str>,
        color: &str,
    ) -> Result<Value> {
        let lib = self.library().await?;
        let parent_id = parent
            .map(|p| resolve_folder(&lib, p))
            .transpose()?
            .map(|f| f.id);
        let sib: Vec<f64> = lib
            .all_folders()
            .iter()
            .filter(|f| f.parent_id == parent_id)
            .filter_map(|f| f.sibling_order)
            .collect();
        let order = sib
            .iter()
            .copied()
            .reduce(f64::min)
            .map_or(0.0, |m| m - 1.0);
        let fid = uuid::Uuid::new_v4().to_string();
        let res = self
            .mutate(
                "createFolder",
                json!({"id": fid, "hexColor": color, "isLocked": false, "title": title, "siblingOrder": order,
                       "workspaceId": lib.private_space.id, "parentId": parent_id}),
            )
            .await?;
        Ok(merge(
            json!({"id": fid, "title": title, "parent_id": parent_id}),
            res,
        ))
    }

    pub async fn rename_folder(&self, r: &str, title: &str) -> Result<Value> {
        let f = self.resolve_folder(r).await?;
        let res = self
            .mutate("modifyFolder", json!({"folderId": f.id, "title": title}))
            .await?;
        Ok(merge(
            json!({"id": f.id, "old_title": f.title, "title": title}),
            res,
        ))
    }

    /// The folder's subtree: `(descendant folder ids, every note id incl. trashed, live note ids)`.
    pub fn subtree(lib: &Library, root: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
        let folders = lib.all_folders();
        let mut desc = Vec::new();
        let mut frontier = vec![root.to_string()];
        while let Some(cur) = frontier.pop() {
            let kids: Vec<String> = folders
                .iter()
                .filter(|x| x.parent_id.as_deref() == Some(cur.as_str()))
                .map(|x| x.id.clone())
                .collect();
            desc.extend(kids.iter().cloned());
            frontier.extend(kids);
        }
        let in_scope = |f: &Option<String>| {
            f.as_deref()
                .is_some_and(|f| f == root || desc.iter().any(|d| d == f))
        };
        let all: Vec<String> = lib
            .all_notes(true)
            .into_iter()
            .filter(|n| in_scope(&n.folder_id))
            .map(|n| n.id.clone())
            .collect();
        let live: Vec<String> = lib
            .all_notes(false)
            .into_iter()
            .filter(|n| in_scope(&n.folder_id))
            .map(|n| n.id.clone())
            .collect();
        (desc, all, live)
    }

    pub fn children_hash(children: &[String]) -> String {
        let mut c = children.to_vec();
        c.sort();
        let d = Sha1::digest(c.join(" ").as_bytes());
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub async fn delete_folder(&self, r: &str, recursive: bool) -> Result<Value> {
        let lib = self.library().await?;
        let f = resolve_folder(&lib, r)?;
        let (desc, all_notes, live) = Self::subtree(&lib, &f.id);
        if (!desc.is_empty() || !live.is_empty()) && !recursive {
            return Err(NbError::usage(format!(
                "folder '{}' contains {} folder(s) and {} note(s); pass --recursive to delete them too",
                f.title,
                desc.len(),
                live.len()
            )));
        }
        let mut payload = json!({"folderId": f.id});
        let children: Vec<String> = desc.iter().chain(&all_notes).cloned().collect();
        if !children.is_empty() {
            payload["childrenHash"] = json!(Self::children_hash(&children));
        }
        let res = self.mutate("deleteFolder", payload).await?;
        let mut trashed = Vec::new();
        for nid in live {
            self.mutate("deleteNote", json!({"noteId": nid})).await?;
            trashed.push(nid);
        }
        Ok(merge(
            json!({"id": f.id, "title": f.title, "deleted_children": children.len(), "trashed_notes": trashed}),
            res,
        ))
    }

    /// `createNote`, then the note's first ops at site 0 (a note opens only once its log is non-empty).
    pub async fn create_note(
        &self,
        title: &str,
        folder: Option<&str>,
        ops: Vec<Value>,
    ) -> Result<Value> {
        let nid = uuid::Uuid::new_v4().to_string();
        let ws = self.workspace_id().await?;
        let res = self
            .mutate(
                "createNote",
                json!({"id": nid, "deviceId": nb_api::auth::device_id()?, "workspaceId": ws, "title": title,
                       "createdAt": now_ms(), "linkAccessLevel": "VIEWER", "linkPermissionScope": "RESTRICTED"}),
            )
            .await?;
        let mut acks = Vec::new();
        for chunk in ops.chunks(APPEND_CHUNK) {
            acks.extend(self.http.append_ops(&nid, 0, chunk).await?);
        }
        let dups = acks
            .iter()
            .filter(|a| a["duplicate"].as_bool().unwrap_or(false))
            .count();
        let mut out = merge(json!({"id": nid, "title": title}), res);
        out["append"] = json!({"acks": acks.len(), "duplicates": dups});
        if let Some(f) = folder {
            out["move"] = self.move_note(&nid, Some(f)).await?;
        }
        Ok(out)
    }

    pub async fn move_note(&self, note: &str, folder: Option<&str>) -> Result<Value> {
        let lib = self.library().await?;
        let n = resolve_note(&lib, note)?;
        let fid = folder
            .map(|f| resolve_folder(&lib, f))
            .transpose()?
            .map(|f| f.id);
        let res = self
            .mutate("modifyNote", json!({"noteId": n.id, "moveToFolder": {"parentId": fid, "workspaceId": lib.private_space.id}}))
            .await?;
        Ok(merge(
            json!({"id": n.id, "from_folder": n.folder_id, "to_folder": fid}),
            res,
        ))
    }

    pub async fn favorite_note(&self, note: &str, value: bool) -> Result<Value> {
        let n = self.resolve_note(note).await?;
        let res = self
            .mutate("modifyNote", json!({"noteId": n.id, "isFavorited": value}))
            .await?;
        Ok(merge(json!({"id": n.id, "favorite": value}), res))
    }

    pub async fn delete_note(&self, note: &str, permanent: bool) -> Result<Value> {
        let n = self.resolve_note(note).await?;
        let res = self.mutate("deleteNote", json!({"noteId": n.id})).await?;
        let mut out = json!({"id": n.id, "title": n.title, "trashed": res});
        if permanent {
            out["permanently_deleted"] = self
                .mutate("permanentlyDeleteNote", json!({"noteId": n.id}))
                .await?;
        }
        Ok(out)
    }

    pub async fn restore_note(&self, note_id: &str) -> Result<Value> {
        let res = self
            .mutate("undeleteNote", json!({"noteId": note_id}))
            .await?;
        Ok(merge(json!({"id": note_id}), res))
    }

    pub async fn modify_note_raw(
        &self,
        note_id: &str,
        fields: Map<String, Value>,
    ) -> Result<Value> {
        let mut p = json!({"noteId": note_id});
        if let Some(m) = p.as_object_mut() {
            m.extend(fields);
        }
        self.mutate("modifyNote", p).await
    }
}

pub fn resolve_note(lib: &Library, r: &str) -> Result<Note> {
    let notes = lib.all_notes(false);
    if let Some(n) = notes.iter().find(|n| n.id == r) {
        return Ok((*n).clone());
    }
    let pref: Vec<&&Note> = if r.len() >= 6 {
        notes.iter().filter(|n| n.id.starts_with(r)).collect()
    } else {
        vec![]
    };
    if pref.len() == 1 {
        return Ok((*pref[0]).clone());
    }
    let exact: Vec<&&Note> = notes.iter().filter(|n| n.title == r).collect();
    if exact.len() == 1 {
        return Ok((*exact[0]).clone());
    }
    if exact.is_empty()
        && let Some((path, title)) = r.rsplit_once('/')
    {
        let path = path.trim_matches('/');
        let hits: Vec<&&Note> = notes
            .iter()
            .filter(|n| {
                n.title == title && lib.folder_path(n.folder_id.as_deref()).as_deref() == Some(path)
            })
            .collect();
        match hits.len() {
            1 => return Ok((*hits[0]).clone()),
            0 => {}
            k => {
                return Err(NbError::usage(format!(
                    "'{r}' is ambiguous ({k} notes match); use the note id"
                )));
            }
        }
    }
    if exact.len() > 1 || pref.len() > 1 {
        let k = if exact.is_empty() {
            pref.len()
        } else {
            exact.len()
        };
        return Err(NbError::usage(format!(
            "'{r}' is ambiguous ({k} notes match); use the note id"
        )));
    }
    Err(NbError::not_found(format!("note not found: {r}")))
}

pub fn resolve_folder(lib: &Library, r: &str) -> Result<Folder> {
    let folders = lib.all_folders();
    if let Some(f) = folders.iter().find(|f| f.id == r) {
        return Ok((*f).clone());
    }
    let by_path: Vec<&&Folder> = folders
        .iter()
        .filter(|f| lib.folder_path(Some(&f.id)).as_deref() == Some(r))
        .collect();
    if by_path.len() == 1 {
        return Ok((*by_path[0]).clone());
    }
    let by_title: Vec<&&Folder> = folders.iter().filter(|f| f.title == r).collect();
    if by_title.len() == 1 {
        return Ok((*by_title[0]).clone());
    }
    if by_title.len() > 1 {
        return Err(NbError::usage(format!(
            "folder title '{r}' is ambiguous; use the full path or id"
        )));
    }
    Err(NbError::not_found(format!("folder not found: {r}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> Library {
        Library::from_value(
            serde_json::from_str(include_str!("../tests/fixtures/library.json")).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn resolves_ids_and_titles() {
        let l = lib();
        let first = l.all_notes(false)[0].clone();
        assert_eq!(resolve_note(&l, &first.id).unwrap().id, first.id);
        assert_eq!(resolve_note(&l, &first.id[..8]).unwrap().id, first.id);
        assert_eq!(
            resolve_note(&l, "nope-nope").unwrap_err().message,
            "note not found: nope-nope"
        );
        assert_eq!(resolve_folder(&l, "Work/Projects").unwrap().id, "f-child");
        assert_eq!(resolve_folder(&l, "Planning").unwrap().id, "f-other");
    }

    #[test]
    fn children_hash_is_sha1_of_sorted_ids() {
        // sha1("a b") as hex
        assert_eq!(
            Client::children_hash(&["b".into(), "a".into()]),
            "7dbde93504122a707f849f2c12bdd9de71b41929"
        );
    }
}
