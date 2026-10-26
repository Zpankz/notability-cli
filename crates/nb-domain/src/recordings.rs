//! Audio recordings on a note: list (with transcription state) and delete.

use std::collections::HashSet;

use nb_api::{NbError, Result};
use serde_json::{Value, json};

use crate::ids::{Id, fmt_id, id_of, idd, parse_id};
use crate::session::EditSession;
use crate::state::NoteState;

/// Live `CreateRecording` entities (renames from `ModifyRecording` applied), oldest first.
pub fn recordings(s: &EditSession<'_>) -> Vec<Value> {
    recordings_of(&s.state)
}

/// [`recordings`] over a bare note state.
pub fn recordings_of(st: &NoteState) -> Vec<Value> {
    let (deleted, _) = st.deleted();
    let mut recs: Vec<(Id, Value)> = Vec::new();
    let mut persisted: HashSet<String> = HashSet::new();
    for o in st.sorted_of(&["CreateRecording", "ModifyRecording", "AssetCloudPersisted"]) {
        let p = &o["payload"]["value"];
        match o["payload"]["type"].as_str() {
            Some("CreateRecording") => {
                let Some(k) = id_of(&o["id"]) else { continue };
                let m = &p["recording"]["metadata"];
                let (start, end) = (p["startTime"].as_f64(), p["endTime"].as_f64());
                recs.push((
                    k,
                    json!({"id": fmt_id(k), "name": p["name"], "file_name": m["fileName"],
                           "mime": m["mimeType"], "size": m["fileSize"], "sha512": m["assetHash"],
                           "start_ms": p["startTime"], "end_ms": p["endTime"],
                           "duration_ms": end.unwrap_or(0.0) - start.unwrap_or(0.0)}),
                ));
            }
            Some("ModifyRecording") => {
                let target = id_of(&p["recording"]);
                if let (Some(t), Some(name)) = (target, p.get("name").filter(|n| !n.is_null()))
                    && let Some((_, r)) = recs.iter_mut().find(|(k, _)| *k == t)
                {
                    r["name"] = name.clone();
                }
            }
            _ => {
                if let Some(h) = p["assetHash"].as_str() {
                    persisted.insert(h.to_string());
                }
            }
        }
    }
    recs.into_iter()
        .filter(|(k, _)| !deleted.contains(k))
        .map(|(_, mut r)| {
            let p = r["sha512"].as_str().is_some_and(|h| persisted.contains(h));
            r["cloud_persisted"] = json!(p);
            r
        })
        .collect()
}

impl EditSession<'_> {
    /// `DeleteEntities{entityDeletes: [<CreateRecording op id>]}`; `rec_id` is `SITE:TS`.
    pub async fn delete_recording(&mut self, rec_id: &str) -> Result<Value> {
        let target = parse_id(rec_id).map_err(|_| {
            NbError::usage("recording id is SITE:TIMESTAMP (see `audio list NOTE`)")
        })?;
        let known =
            self.state.ops.iter().any(|o| {
                o["payload"]["type"] == "CreateRecording" && id_of(&o["id"]) == Some(target)
            });
        if !known {
            return Err(NbError::not_found(format!(
                "no recording {} in note {}",
                fmt_id(target),
                self.note_id
            )));
        }
        let o = self.op("DeleteEntities", json!({"entityDeletes": [idd(target)]}));
        self.emit(vec![o], json!({"deleted": fmt_id(target)})).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_live_recordings_with_renames() {
        let rec = |ts: u64, ty: &str, v: Value| json!({"id": {"site": 1, "timestamp": ts}, "payload": {"type": ty, "value": v}});
        let bundle = json!({"ops": [
            rec(1, "CreateRecording", json!({"recording": {"metadata": {"assetHash": "h1", "fileName": "a.m4a"}}, "startTime": 0, "endTime": 1500})),
            rec(2, "CreateRecording", json!({"recording": {"metadata": {"assetHash": "h2"}}, "startTime": 0, "endTime": 10})),
            rec(3, "ModifyRecording", json!({"recording": {"site": 1, "timestamp": 1}, "name": "Lecture"})),
            rec(4, "DeleteEntities", json!({"entityDeletes": [{"site": 1, "timestamp": 2}]})),
            rec(5, "AssetCloudPersisted", json!({"assetHash": "h1"})),
        ]});
        let s = EditSession::with_bundle(None, "n", &bundle);
        let r = recordings(&s);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["name"], "Lecture");
        assert_eq!(r[0]["duration_ms"], 1500.0);
        assert_eq!(r[0]["cloud_persisted"], true);
    }
}
