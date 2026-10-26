//! Materialised view of a decoded `NoteBundle`: what reads and writes need (metadata, pages,
//! text, paragraph styles, blocks, math, ink, shapes, comments).

use std::collections::{BTreeMap, HashMap, HashSet};

use nb_api::{NbError, Result};
use serde_json::{Map, Value, json};

use crate::ids::{Id, SeqId, fmt_id, fmt_seq, id_of, linearize, seq_of};

pub const LAYOUT_MODES: [(&str, u64); 2] = [("paged", 0), ("pageless", 1)];
pub const BLOCK_TEXT: u64 = 0;
pub const BLOCK_IMAGE: u64 = 1;
pub const BLOCK_MATH: u64 = 2;
pub const DECORATORS: [(&str, u64); 5] = [
    ("bullet", 1),
    ("number", 2),
    ("check", 3),
    ("quote", 4),
    ("code", 5),
];

pub fn default_background() -> Value {
    json!({
        "paper": {"backgroundColor": {"bitsR": 255, "bitsG": 255, "bitsB": 255, "bitsA": 255}},
        "size": {"width": 612.0, "height": 792.0},
        "margins": {"top": 36.0, "bottom": 36.0, "left": 36.0, "right": 36.0},
    })
}

fn lamport(o: &Value) -> (u64, u64) {
    (
        o["id"]["timestamp"].as_u64().unwrap_or(0),
        o["id"]["site"].as_u64().unwrap_or(0),
    )
}

fn ptype(o: &Value) -> &str {
    o["payload"]["type"].as_str().unwrap_or_default()
}

fn pval(o: &Value) -> &Value {
    &o["payload"]["value"]
}

#[derive(Debug, Clone)]
pub struct Page {
    pub index: usize,
    pub id: SeqId,
    pub node: SeqId,
    pub background: Option<Value>,
}

impl Page {
    pub fn to_json(&self) -> Value {
        json!({"index": self.index, "id": fmt_seq(self.id), "node": fmt_seq(self.node), "background": self.background})
    }
}

/// Paragraph attributes after applying every `ModifyParagraphStyle` / `UpdateCheckbox`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParaStyle {
    pub decorator: Option<u64>,
    pub language: Option<String>,
    pub checked: Option<bool>,
    pub indent: Option<u64>,
    pub alignment: Option<u64>,
    pub line_spacing: Option<f64>,
}

impl ParaStyle {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        if let Some(d) = self.decorator {
            let name = DECORATORS
                .iter()
                .find(|(_, v)| *v == d)
                .map_or("unknown", |(n, _)| n);
            m.insert("list".into(), json!(name));
        }
        if self.decorator == Some(5) {
            m.insert(
                "language".into(),
                json!(self.language.clone().unwrap_or_else(|| "plaintext".into())),
            );
        }
        if let Some(c) = self.checked {
            m.insert("checked".into(), json!(c));
        }
        if let Some(i) = self.indent {
            m.insert("indent".into(), json!(i));
        }
        if let Some(a) = self.alignment {
            m.insert(
                "align".into(),
                json!(
                    ["natural", "left", "center", "right"]
                        .get(a as usize)
                        .copied()
                        .unwrap_or("unknown")
                ),
            );
        }
        if let Some(s) = self.line_spacing {
            m.insert("spacing".into(), json!(s));
        }
        Value::Object(m)
    }
}

/// One paragraph of a text field: its key (first char / preceding newline), text and style.
#[derive(Debug, Clone)]
pub struct Paragraph {
    pub key: SeqId,
    pub text: String,
    pub style: ParaStyle,
}

pub struct NoteState {
    pub site: u64,
    pub ops: Vec<Value>,
}

impl NoteState {
    pub fn new(bundle: &Value) -> Self {
        let ops = bundle["ops"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|o| o.get("payload").is_some())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Self {
            site: bundle["editorSite"].as_u64().unwrap_or(0),
            ops,
        }
    }

    pub fn add_ops(&mut self, ops: impl IntoIterator<Item = Value>) {
        self.ops
            .extend(ops.into_iter().filter(|o| o.get("payload").is_some()));
    }

    fn of<'a>(&'a self, types: &'a [&str]) -> impl Iterator<Item = &'a Value> + 'a {
        self.ops.iter().filter(move |o| types.contains(&ptype(o)))
    }

    pub(crate) fn sorted_of(&self, types: &[&str]) -> Vec<&Value> {
        let mut v: Vec<&Value> = self
            .ops
            .iter()
            .filter(|o| types.contains(&ptype(o)))
            .collect();
        v.sort_by_key(|o| lamport(o));
        v
    }

    pub fn max_timestamp(&self) -> Option<u64> {
        self.ops
            .iter()
            .filter_map(|o| o["id"]["timestamp"].as_u64())
            .max()
    }

    // ------------------------------------------------------------- metadata (LWW)
    pub fn metadata(&self) -> Map<String, Value> {
        let mut out = Map::new();
        for o in self.sorted_of(&["SetMetadata"]) {
            if let Some(m) = pval(o).as_object() {
                for (k, v) in m {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
        out
    }

    pub fn title(&self) -> Option<String> {
        self.metadata()
            .get("title")
            .and_then(|t| t["value"].as_str())
            .map(str::to_string)
    }

    pub fn layout(&self) -> String {
        let m = self
            .metadata()
            .get("layoutMode")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        LAYOUT_MODES
            .iter()
            .find(|(_, v)| *v == m)
            .map_or_else(|| m.to_string(), |(k, _)| (*k).to_string())
    }

    pub fn note_background(&self) -> Value {
        match self.metadata().get("pageBackground").map(|b| &b["value"]) {
            Some(v) if v.is_object() => v.clone(),
            _ => default_background(),
        }
    }

    // ------------------------------------------------------------- deletes
    pub fn deleted(&self) -> (HashSet<Id>, HashSet<SeqId>) {
        let (mut ents, mut pages) = (HashMap::new(), HashMap::new());
        for o in self.sorted_of(&["DeleteEntities"]) {
            let v = pval(o);
            let each = |k: &str| v[k].as_array().cloned().unwrap_or_default();
            for e in each("entityDeletes") {
                ents.insert(id_of(&e), true);
            }
            for e in each("entityUndeletes") {
                ents.insert(id_of(&e), false);
            }
            for p in each("pageDeletes") {
                pages.insert(seq_of(&p), true);
            }
            for p in each("pageUndeletes") {
                pages.insert(seq_of(&p), false);
            }
        }
        (
            ents.into_iter()
                .filter_map(|(k, d)| d.then_some(k).flatten())
                .collect(),
            pages
                .into_iter()
                .filter_map(|(k, d)| d.then_some(k).flatten())
                .collect(),
        )
    }

    // ------------------------------------------------------------- pages
    pub fn pages(&self) -> Vec<Page> {
        let mut nodes = Vec::new();
        let mut position: HashMap<SeqId, SeqId> = HashMap::new();
        let mut stamp: HashMap<SeqId, (u64, u64)> = HashMap::new();
        let mut backgrounds: HashMap<SeqId, Value> = HashMap::new();
        let mut order_created = Vec::new();
        for o in self.of(&["CreatePage"]) {
            let (s, t) = (
                o["id"]["site"].as_u64().unwrap_or(0),
                o["id"]["timestamp"].as_u64().unwrap_or(0),
            );
            let v = pval(o);
            let mut prev = seq_of(&v["location"]);
            for i in 0..v["pageCount"].as_u64().filter(|c| *c > 0).unwrap_or(1) {
                let nid = (s, t, i);
                nodes.push((nid, prev));
                position.insert(nid, nid);
                order_created.push(nid);
                if v["background"].is_object() {
                    backgrounds.insert(nid, v["background"].clone());
                }
                prev = Some(nid);
            }
        }
        for o in self.sorted_of(&["ModifyPage"]) {
            let (t, s) = lamport(o);
            let v = pval(o);
            let pids: Vec<SeqId> = v["pages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(seq_of)
                .collect();
            if let Some(mv) = v.get("moveTo") {
                let mut prev = seq_of(&mv["toId"]);
                for (i, pid) in pids.iter().enumerate() {
                    let nid = (s, t, i as u64);
                    nodes.push((nid, prev));
                    if (t, s) > stamp.get(pid).copied().unwrap_or((0, 0))
                        || !stamp.contains_key(pid)
                    {
                        position.insert(*pid, nid);
                        stamp.insert(*pid, (t, s));
                    }
                    prev = Some(nid);
                }
            }
            if v["background"].is_object() {
                for pid in &pids {
                    backgrounds.insert(*pid, v["background"]["value"].clone());
                }
            }
        }
        let (_, page_del) = self.deleted();
        let node_to_page: HashMap<SeqId, SeqId> = position
            .iter()
            .filter(|(p, _)| !page_del.contains(p))
            .map(|(p, n)| (*n, *p))
            .collect();
        linearize(&nodes)
            .into_iter()
            .filter_map(|n| node_to_page.get(&n).copied())
            .enumerate()
            .map(|(i, pid)| Page {
                index: i + 1,
                id: pid,
                node: position[&pid],
                background: backgrounds.get(&pid).filter(|b| b.is_object()).cloned(),
            })
            .collect()
    }

    pub fn page(&self, number: usize) -> Result<Page> {
        let ps = self.pages();
        if number < 1 || number > ps.len() {
            return Err(NbError::usage(format!(
                "page {number} out of range (note has {} page(s))",
                ps.len()
            )));
        }
        Ok(ps[number - 1].clone())
    }

    /// `(width, height)` in points: the PDF crop box for PDF pages, else the paper size.
    pub fn page_size(&self, page: &Page) -> (f64, f64) {
        let note_bg = self.note_background();
        let bg = page.background.as_ref().unwrap_or(&note_bg);
        if let Some(c) = bg["pdf"]["cropBoxes"]
            .as_array()
            .and_then(|c| c.get(page.id.2 as usize))
        {
            return (
                c["width"].as_f64().unwrap_or(612.0),
                c["height"].as_f64().unwrap_or(792.0),
            );
        }
        let sz = if bg["size"].is_object() {
            &bg["size"]
        } else {
            &note_bg["size"]
        };
        (
            sz["width"].as_f64().unwrap_or(612.0),
            sz["height"].as_f64().unwrap_or(792.0),
        )
    }

    fn page_numbers(&self) -> HashMap<SeqId, usize> {
        self.pages().into_iter().map(|p| (p.id, p.index)).collect()
    }

    // ------------------------------------------------------------- text
    pub fn text_chars(&self, field: Option<Id>) -> Vec<(SeqId, char)> {
        let mut nodes = Vec::new();
        let mut chars = HashMap::new();
        let mut removed: HashSet<SeqId> = HashSet::new();
        for o in self.of(&[
            "InsertString",
            "InsertChar",
            "RemoveChars",
            "RemoveChar",
            "ReviveChars",
        ]) {
            let (v, ty) = (pval(o), ptype(o));
            if id_of(&v["textField"]) != field {
                continue;
            }
            match ty {
                "RemoveChars" | "ReviveChars" => {
                    let locs = v["locations"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(seq_of);
                    if ty == "RemoveChars" {
                        removed.extend(locs);
                    } else {
                        for l in locs {
                            removed.remove(&l);
                        }
                    }
                }
                "RemoveChar" => {
                    removed.extend(seq_of(&v["location"]));
                }
                _ => {
                    let s: String = if ty == "InsertString" {
                        v["string"].as_str().unwrap_or_default().to_string()
                    } else {
                        char::from_u32(v["unicodeScalar"].as_u64().unwrap_or(0xFFFD) as u32)
                            .unwrap_or('\u{FFFD}')
                            .to_string()
                    };
                    let mut prev = seq_of(&v["location"]);
                    let (site, ts) = (
                        o["id"]["site"].as_u64().unwrap_or(0),
                        o["id"]["timestamp"].as_u64().unwrap_or(0),
                    );
                    for (i, ch) in s.chars().enumerate() {
                        let nid = (site, ts, i as u64);
                        nodes.push((nid, prev));
                        chars.insert(nid, ch);
                        prev = Some(nid);
                    }
                }
            }
        }
        linearize(&nodes)
            .into_iter()
            .filter(|n| !removed.contains(n))
            .map(|n| (n, chars[&n]))
            .collect()
    }

    pub fn text(&self, field: Option<Id>) -> String {
        self.text_chars(field).into_iter().map(|(_, c)| c).collect()
    }

    /// Paragraphs of a text field with their styles (web ordering: LWW by Lamport stamp).
    pub fn paragraphs(&self, field: Option<Id>) -> Vec<Paragraph> {
        let chars = self.text_chars(field);
        let mut paras: Vec<Paragraph> = Vec::new();
        let mut key: Option<SeqId> = None;
        let mut buf = String::new();
        for (id, ch) in &chars {
            if key.is_none() {
                key = Some(*id);
            }
            if *ch == '\n' {
                paras.push(Paragraph {
                    key: key.unwrap_or(*id),
                    text: std::mem::take(&mut buf),
                    style: ParaStyle::default(),
                });
                key = Some(*id);
            } else {
                buf.push(*ch);
            }
        }
        if let Some(k) = key {
            paras.push(Paragraph {
                key: k,
                text: buf,
                style: ParaStyle::default(),
            });
        }
        let pos: HashMap<SeqId, usize> =
            paras.iter().enumerate().map(|(i, p)| (p.key, i)).collect();
        for o in self.sorted_of(&["ModifyParagraphStyle", "UpdateCheckbox"]) {
            let v = pval(o);
            if id_of(&v["textField"]) != field {
                continue;
            }
            if ptype(o) == "UpdateCheckbox" {
                if let Some(&i) = seq_of(&v["location"]).and_then(|k| pos.get(&k)) {
                    paras[i].style.checked = v["isChecked"].as_bool();
                }
                continue;
            }
            let Some(&a) = seq_of(&v["start"]).and_then(|k| pos.get(&k)) else {
                continue;
            };
            let b = seq_of(&v["end"])
                .and_then(|k| pos.get(&k).copied())
                .unwrap_or(paras.len().saturating_sub(1));
            for p in paras.iter_mut().take(b + 1).skip(a) {
                let st = &mut p.style;
                if let Some(s) = v.get("decoratorStyle") {
                    st.decorator = s["value"].as_u64().filter(|d| *d != 0);
                }
                if let Some(s) = v.get("programmingLanguage") {
                    st.language = s["value"].as_str().map(str::to_string);
                }
                if let Some(s) = v.get("isChecked") {
                    st.checked = s["value"].as_bool();
                }
                if let Some(s) = v.get("indentLevel") {
                    st.indent = s["value"].as_u64().filter(|d| *d != 0);
                }
                if let Some(s) = v.get("alignment") {
                    st.alignment = s["value"].as_u64();
                }
                if let Some(s) = v.get("lineSpacing") {
                    st.line_spacing = s["value"].as_f64();
                }
            }
        }
        paras
    }

    /// Character-style ops that touch a text field, in Lamport order (for `text list --styles`).
    pub fn style_ops(&self, field: Option<Id>) -> Vec<Value> {
        self.sorted_of(&["ModifyStyle", "ClearStyle"])
            .into_iter()
            .filter(|o| id_of(&pval(o)["textField"]) == field)
            .map(|o| json!({"op": fmt_id((o["id"]["site"].as_u64().unwrap_or(0), o["id"]["timestamp"].as_u64().unwrap_or(0))), "type": ptype(o), "value": pval(o)}))
            .collect()
    }

    // ------------------------------------------------------------- blocks
    /// Block field overrides from `ModifyBlock` (LWW), keyed by block id.
    fn block_mods(&self) -> HashMap<Id, Map<String, Value>> {
        let mut out: HashMap<Id, Map<String, Value>> = HashMap::new();
        for o in self.sorted_of(&["ModifyBlock"]) {
            let v = pval(o);
            for b in v["blocks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(id_of)
            {
                let m = out.entry(b).or_default();
                for (k, x) in v.as_object().into_iter().flatten() {
                    if k == "blocks" {
                        continue;
                    }
                    let set_table = matches!(
                        k.as_str(),
                        "mathLatex" | "mathColor" | "cropRect" | "rotation" | "scale" | "paper"
                    );
                    let val = if set_table {
                        x.get("value").cloned().unwrap_or(Value::Null)
                    } else {
                        x.clone()
                    };
                    m.insert(k.clone(), val);
                }
            }
        }
        out
    }

    fn positions(&self) -> HashMap<Id, (Value, Value)> {
        let mut out = HashMap::new();
        for o in self.sorted_of(&["ModifyPositions"]) {
            for m in pval(o)["modifications"].as_array().into_iter().flatten() {
                if let Some(t) = id_of(&m["target"]) {
                    let e: &mut (Value, Value) = out.entry(t).or_insert((Value::Null, Value::Null));
                    if m["page"].is_object() {
                        e.0 = m["page"].clone();
                    }
                    if m["origin"].is_object() {
                        e.1 = m["origin"].clone();
                    }
                }
            }
        }
        out
    }

    pub fn blocks(&self) -> Vec<Value> {
        let (dels, _) = self.deleted();
        let page_no = self.page_numbers();
        let mods = self.block_mods();
        let moved = self.positions();
        let mut out = Vec::new();
        for o in self.of(&["CreateBlock"]) {
            let Some(bid) = id_of(&o["id"]) else { continue };
            if dels.contains(&bid) {
                continue;
            }
            let mut v = pval(o).as_object().cloned().unwrap_or_default();
            if let Some(m) = mods.get(&bid) {
                for (k, x) in m {
                    v.insert(k.clone(), x.clone());
                }
            }
            if let Some((p, org)) = moved.get(&bid) {
                if p.is_object() {
                    v.insert("page".into(), p.clone());
                }
                if org.is_object() {
                    v.insert("origin".into(), org.clone());
                }
            }
            let ty = v.get("type").and_then(Value::as_u64).unwrap_or(BLOCK_TEXT);
            let mut b = json!({
                "id": fmt_id(bid), "type": ty,
                "page": v.get("page").and_then(seq_of).and_then(|p| page_no.get(&p)),
                "origin": v.get("origin"), "size": v.get("size"), "text": self.text(Some(bid)),
            });
            match ty {
                BLOCK_MATH => {
                    b["kind"] = json!("math");
                    b["latex"] = v.get("mathLatex").cloned().unwrap_or(Value::Null);
                    b["color"] = v
                        .get("mathColor")
                        .and_then(nb_protocol::codec::color_hex)
                        .map(Value::from)
                        .unwrap_or(Value::Null);
                }
                BLOCK_IMAGE => {
                    b["kind"] = json!("image");
                    let meta = v.get("image").map(|i| &i["metadata"]);
                    b["asset"] = meta.map(|m| m["assetHash"].clone()).unwrap_or(Value::Null);
                    b["file_name"] = meta.map(|m| m["fileName"].clone()).unwrap_or(Value::Null);
                    b["crop"] = v.get("cropRect").cloned().unwrap_or(Value::Null);
                    b["flipped_h"] = json!(
                        v.get("imageFlippedHorizontally")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    );
                    b["flipped_v"] = json!(
                        v.get("imageFlippedVertically")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    );
                }
                _ => b["kind"] = json!("text"),
            }
            out.push(b);
        }
        out
    }

    pub fn block(&self, id: Id) -> Option<Value> {
        let s = fmt_id(id);
        self.blocks().into_iter().find(|b| b["id"] == s.as_str())
    }

    // ------------------------------------------------------------- ink / shapes
    pub fn entities(&self, create: &str) -> Vec<Value> {
        let (dels, _) = self.deleted();
        let page_no = self.page_numbers();
        let moved = self.positions();
        let modify = if create == "CreateInk" {
            "ModifyInk"
        } else {
            "ModifyShape"
        };
        let list_key = if create == "CreateInk" {
            "inks"
        } else {
            "shapes"
        };
        let mut mods: HashMap<Id, Map<String, Value>> = HashMap::new();
        for o in self.sorted_of(&[modify]) {
            let v = pval(o);
            for e in v[list_key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(id_of)
            {
                let m = mods.entry(e).or_default();
                for (k, x) in v
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(k, _)| *k != list_key)
                {
                    m.insert(k.clone(), x.clone());
                }
            }
        }
        let mut out = Vec::new();
        for o in self.of(&[create]) {
            let Some(id) = id_of(&o["id"]) else { continue };
            if dels.contains(&id) {
                continue;
            }
            let mut v = pval(o).as_object().cloned().unwrap_or_default();
            for (k, x) in mods.get(&id).into_iter().flatten() {
                v.insert(k.clone(), x.clone());
            }
            if let Some((p, org)) = moved.get(&id) {
                if p.is_object() {
                    v.insert("page".into(), p.clone());
                }
                if org.is_object() {
                    v.insert("origin".into(), org.clone());
                }
            }
            let color = v.get("color").and_then(nb_protocol::codec::color_hex);
            let mut e = json!({"id": fmt_id(id), "page": v.get("page").and_then(seq_of).and_then(|p| page_no.get(&p)),
                "origin": v.get("origin"), "color": color});
            if create == "CreateInk" {
                let tool = v.get("tool").and_then(Value::as_u64).unwrap_or(0);
                e["tool"] = json!(match tool {
                    0 => "pen",
                    1 => "pencil",
                    2 => "highlighter",
                    3 => "tape",
                    _ => "other",
                });
                e["width"] = v.get("width").cloned().unwrap_or(Value::Null);
                let pts = v
                    .get("encodedCenterPath")
                    .and_then(bytes_of)
                    .and_then(|b| nb_protocol::inkpath::decode(&b).ok());
                e["points"] = json!(pts.map_or(0, |p| p.elements.len()));
            } else {
                let def = v.get("definition").cloned().unwrap_or(Value::Null);
                e["kind"] = json!(shape_kind(&def));
                e["border_width"] = v.get("borderWidth").cloned().unwrap_or(Value::Null);
                e["fill"] = v
                    .get("fillColor")
                    .and_then(nb_protocol::codec::color_hex)
                    .map(Value::from)
                    .unwrap_or(Value::Null);
                e["definition"] = def;
            }
            out.push(e);
        }
        out
    }

    // ------------------------------------------------------------- comments
    pub fn comments(&self) -> Vec<Value> {
        let (dels, _) = self.deleted();
        let page_no = self.page_numbers();
        let mut mods: HashMap<Id, Map<String, Value>> = HashMap::new();
        for o in self.sorted_of(&["ModifyComment"]) {
            let v = pval(o);
            let Some(cid) = id_of(&v["comment"]) else {
                continue;
            };
            let m = mods.entry(cid).or_default();
            if let Some(t) = v.get("text") {
                m.insert("text".into(), t["value"].clone());
            }
            if let Some(r) = v.get("resolved") {
                m.insert(
                    "resolved".into(),
                    json!(r["value"].as_bool().unwrap_or(false)),
                );
            }
            if v["anchor"].is_object() {
                m.insert(
                    "anchor".into(),
                    json!({"type": "CanvasAnchor", "value": v["anchor"]}),
                );
            }
        }
        let mut threads: BTreeMap<usize, (Id, Value)> = BTreeMap::new();
        let mut replies = Vec::new();
        for (n, o) in self.of(&["CreateComment"]).enumerate() {
            let Some(cid) = id_of(&o["id"]) else { continue };
            let mut v = pval(o).as_object().cloned().unwrap_or_default();
            for (k, x) in mods.get(&cid).into_iter().flatten() {
                v.insert(k.clone(), x.clone());
            }
            let anchor = v.get("anchor").cloned().unwrap_or(Value::Null);
            let at = anchor["type"].as_str().unwrap_or_default();
            let av = &anchor["value"];
            let mut c = json!({"id": fmt_id(cid), "text": v.get("text").cloned().unwrap_or(json!("")),
                "resolved": v.get("resolved").and_then(Value::as_bool).unwrap_or(false),
                "anchor_type": anchor.get("type"), "client_time": o.get("clientTime"), "server_time": o.get("serverTime")});
            match at {
                "ReplyAnchor" => {
                    c["reply_to"] = json!(id_of(&av["root"]).map(fmt_id));
                    replies.push((cid, id_of(&av["root"]), c));
                    continue;
                }
                "CanvasAnchor" => {
                    c["page"] = json!(seq_of(&av["page"]).and_then(|p| page_no.get(&p)));
                    c["origin"] = av["origin"].clone();
                }
                "TextAnchor" => c["text_field"] = json!(id_of(&av["textField"]).map(fmt_id)),
                _ => {}
            }
            c["replies"] = json!([]);
            threads.insert(n, (cid, c));
        }
        for (rid, root, r) in replies {
            if dels.contains(&rid) {
                continue;
            }
            if let Some((_, t)) = threads.values_mut().find(|(id, _)| Some(*id) == root)
                && let Some(a) = t["replies"].as_array_mut()
            {
                a.push(r);
            }
        }
        threads
            .into_values()
            .filter(|(id, _)| !dels.contains(id))
            .map(|(_, t)| t)
            .collect()
    }

    pub fn entities_on_page(&self, page: SeqId) -> Vec<Id> {
        let (dels, _) = self.deleted();
        let mut where_: Vec<(Id, SeqId)> = Vec::new();
        for o in self.sorted_of_all() {
            let ty = ptype(o);
            if !ty.starts_with("Create") || ty == "CreatePage" {
                continue;
            }
            let v = pval(o);
            let pg = if v["page"].is_object() {
                seq_of(&v["page"])
            } else if v["anchor"]["type"] == "CanvasAnchor" {
                seq_of(&v["anchor"]["value"]["page"])
            } else {
                None
            };
            if let (Some(pg), Some(id)) = (pg, id_of(&o["id"])) {
                where_.retain(|(e, _)| *e != id);
                where_.push((id, pg));
            }
        }
        where_
            .into_iter()
            .filter(|(e, p)| *p == page && !dels.contains(e))
            .map(|(e, _)| e)
            .collect()
    }

    fn sorted_of_all(&self) -> Vec<&Value> {
        let mut v: Vec<&Value> = self.ops.iter().collect();
        v.sort_by_key(|o| lamport(o));
        v
    }
}

/// `[ubyte]` vectors decode as int arrays (or `{"hex"}` in the wire fixtures).
pub fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_u64().and_then(|n| u8::try_from(n).ok()))
            .collect(),
        Value::Object(o) => o
            .get("hex")
            .and_then(Value::as_str)
            .and_then(nb_protocol::codec::unhex),
        _ => None,
    }
}

fn shape_kind(def: &Value) -> &'static str {
    match def["type"].as_str() {
        Some("NormalShape") => "ellipse",
        Some("Line") if def["value"]["arrowHead"].as_u64().unwrap_or(0) > 0 => "arrow",
        Some("Line") => "line",
        Some("Polygon") => match def["value"]["points"].as_array().map_or(0, Vec::len) {
            3 => "triangle",
            4 => "rectangle",
            _ => "polygon",
        },
        _ => "unknown",
    }
}

/// `[x, y]` pairs to `Point` objects.
pub fn point(x: f64, y: f64) -> Value {
    json!({"x": x, "y": y})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> NoteState {
        NoteState::new(
            &serde_json::from_str(include_str!("../tests/fixtures/editlog_bundle.json")).unwrap(),
        )
    }

    #[test]
    fn sample_bundle_reconstructs() {
        let s = fixture();
        assert!(s.title().is_some());
        assert!(!s.pages().is_empty());
        let _ = (s.blocks(), s.comments(), s.text(None), s.paragraphs(None));
    }

    #[test]
    fn web_editor_ops_reconstruct_styles_math_ink_shapes() {
        let raw: Value = serde_json::from_str(include_str!(
            "../../nb-protocol/tests/fixtures/wire/web_ui/web_editor_ops.json"
        ))
        .unwrap();
        let mut ops: Vec<Value> = raw["ops"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.get("op").cloned().unwrap_or_else(|| o.clone()))
            .collect();
        // seed pages so page numbers resolve
        ops.insert(0, json!({"id": {"site": 0, "timestamp": 0}, "payload": {"type": "CreatePage", "value": {"pageCount": 2}}}));
        let s = NoteState::new(&json!({"editorSite": 1, "ops": ops}));
        let paras = s.paragraphs(None);
        let decos: Vec<Option<u64>> = paras.iter().map(|p| p.style.decorator).collect();
        assert!(decos.contains(&Some(5)), "{decos:?}");
        let math: Vec<Value> = s
            .blocks()
            .into_iter()
            .filter(|b| b["kind"] == "math")
            .collect();
        assert_eq!(math[0]["latex"], "\\frac{a}{b}+x^2");
        let shapes = s.entities("CreateShape");
        let kinds: HashSet<&str> = shapes.iter().filter_map(|e| e["kind"].as_str()).collect();
        for k in ["rectangle", "ellipse", "triangle", "arrow", "line"] {
            assert!(kinds.contains(k), "{kinds:?}");
        }
        assert!(!s.entities("CreateInk").is_empty());
        assert!(!s.comments().is_empty());
    }
}
