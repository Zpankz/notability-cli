//! `EditSession`: append ops to one note as this CLI device (its own per-note site), plus the
//! op builders for text, styling, paragraphs, math, ink, shapes, comments, pages and images.

use std::collections::{HashMap, HashSet};
use std::f64::consts::FRAC_PI_2;

use nb_api::{NbError, Result};
use nb_protocol::inkpath::{self, PathElement};
use serde_json::{Map, Value, json};

use crate::client::{Client, now_ms};
use crate::ids::{Id, SeqId, fmt_id, fmt_seq, id_of, idd, parse_id, seq_of, seqd};
use crate::state::{
    BLOCK_IMAGE, BLOCK_MATH, DECORATORS, LAYOUT_MODES, NoteState, Page, default_background, point,
};

pub const FLAIRS: [(&str, Option<u64>); 4] = [
    ("plain", None),
    ("lines", Some(0)),
    ("dots", Some(1)),
    ("grid", Some(2)),
];
pub const SERVER_ONLY_OPS: [&str; 1] = ["AssetCloudPersisted"];
/// The web editor's default ink/shape colour (#171A20).
pub const INK_COLOR: (u8, u8, u8, u8) = (0x17, 0x1a, 0x20, 255);
pub const HIGHLIGHT_COLOR: (u8, u8, u8, u8) = (0xff, 0xf6, 0x00, 89);
/// Font-size presets of the web editor's style menu.
pub const FONT_PRESETS: [(&str, f64); 6] = [
    ("title", 36.0),
    ("h1", 30.0),
    ("h2", 24.0),
    ("h3", 18.0),
    ("body", 14.0),
    ("caption", 12.0),
];
pub const ALIGNMENTS: [(&str, u64); 4] = [("natural", 0), ("left", 1), ("center", 2), ("right", 3)];
/// Prism language ids accepted by the web editor's code-block picker.
pub const CODE_LANGUAGES: [&str; 27] = [
    "plaintext",
    "bash",
    "c",
    "cpp",
    "csharp",
    "css",
    "diff",
    "go",
    "graphql",
    "html",
    "java",
    "javascript",
    "json",
    "kotlin",
    "latex",
    "lua",
    "markdown",
    "matlab",
    "objectivec",
    "php",
    "python",
    "r",
    "ruby",
    "rust",
    "sql",
    "swift",
    "typescript",
];
/// Character attributes settable with `ModifyStyle` (CLI name, op field, value kind).
pub const STYLE_ATTRS: [(&str, &str, AttrKind); 11] = [
    ("bold", "bold", AttrKind::Bool),
    ("italic", "italic", AttrKind::Bool),
    ("underline", "underline", AttrKind::Bool),
    ("strike", "strikethrough", AttrKind::Bool),
    ("superscript", "superscript", AttrKind::Bool),
    ("subscript", "subscript", AttrKind::Bool),
    ("code", "code", AttrKind::Bool),
    ("highlight", "highlight", AttrKind::Color),
    ("color", "foregroundColor", AttrKind::Color),
    ("font", "familyName", AttrKind::String),
    ("size", "size", AttrKind::Float),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrKind {
    Bool,
    Color,
    String,
    Float,
}

const TEXTBOX_MARGINS: [(&str, f64); 4] =
    [("top", 3.0), ("bottom", 3.0), ("left", 5.0), ("right", 5.0)];

pub fn color_value(c: (u8, u8, u8, u8)) -> Value {
    nb_protocol::codec::color(c.0, c.1, c.2, c.3)
}

/// `#RRGGBB` or `#RRGGBBAA` to a `Color`.
pub fn parse_color(s: &str) -> Result<Value> {
    let h = s.trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    let parsed = match h.len() {
        6 if h.is_ascii() => byte(0)
            .zip(byte(2))
            .zip(byte(4))
            .map(|((r, g), b)| (r, g, b, 255)),
        8 if h.is_ascii() => byte(0)
            .zip(byte(2))
            .zip(byte(4))
            .zip(byte(6))
            .map(|(((r, g), b), a)| (r, g, b, a)),
        _ => None,
    };
    parsed
        .map(color_value)
        .ok_or_else(|| NbError::usage(format!("bad colour '{s}' (expected #RRGGBB or #RRGGBBAA)")))
}

/// Which characters of a text field an edit applies to.
#[derive(Clone, Debug, PartialEq)]
pub enum Selection {
    All,
    /// The `nth` (1-based) occurrence of a substring.
    Match(String, usize),
    /// Character offsets, end exclusive.
    Range(usize, usize),
    /// 1-based paragraph numbers, inclusive.
    Paragraphs(usize, usize),
}

/// Where a canvas entity sits: `(page seq, origin, kind)`.
type Placement = (Option<SeqId>, (f64, f64), &'static str);

/// A resolved selection: char span `[start, end)` over `chars`.
struct Span {
    chars: Vec<(SeqId, char)>,
    start: usize,
    end: usize,
    all: bool,
}

impl Span {
    fn boundaries(&self) -> (Value, Value) {
        let s = json!({"location": seqd(self.chars[self.start].0), "type": 0});
        let e = if self.all && self.end == self.chars.len() {
            json!({"type": 3})
        } else {
            json!({"location": seqd(self.chars[self.end - 1].0), "type": 1})
        };
        (s, e)
    }
    fn text(&self) -> String {
        self.chars[self.start..self.end]
            .iter()
            .map(|c| c.1)
            .collect()
    }
    /// 0-based paragraph index of each char (a newline opens the paragraph it keys).
    fn para_of(&self, i: usize) -> usize {
        self.chars[..=i].iter().filter(|c| c.1 == '\n').count()
    }
}

pub struct EditSession<'a> {
    client: Option<&'a Client>,
    pub note_id: String,
    pub state: NoteState,
    ts: u64,
    /// Ops built for a note that does not exist yet (see [`EditSession::new_note`]).
    pub staged: Vec<Value>,
}

fn op_ids(acks: &[Value]) -> Value {
    json!(
        acks.iter()
            .filter_map(|a| id_of(&a["id"]).map(fmt_id))
            .collect::<Vec<_>>()
    )
}

impl<'a> EditSession<'a> {
    pub async fn open(client: &'a Client, note_id: &str) -> Result<Self> {
        let bundle = client.http.note_bundle(note_id).await?;
        Ok(Self::with_bundle(Some(client), note_id, &bundle))
    }

    pub fn with_bundle(client: Option<&'a Client>, note_id: &str, bundle: &Value) -> Self {
        let state = NoteState::new(bundle);
        let ts = state.max_timestamp().map_or(0, |t| t + 1);
        Self {
            client,
            note_id: note_id.to_string(),
            state,
            ts,
            staged: Vec::new(),
        }
    }

    /// The first ops of a note that does not exist yet (site 0, timestamps from 0); nothing is
    /// sent — pass `staged` to `Client::create_note`.
    pub fn new_note(title: &str) -> Self {
        let mut s = Self::with_bundle(None, "", &json!({"editorSite": 0, "ops": []}));
        let ops = initial_ops(title);
        s.ts = ops.len() as u64;
        s.state.add_ops(ops.clone());
        s.staged = ops;
        s
    }

    pub fn site(&self) -> u64 {
        self.state.site
    }

    pub fn op(&mut self, ptype: &str, value: Value) -> Value {
        let o = json!({"id": {"site": self.site(), "timestamp": self.ts}, "clientTime": now_ms(),
                       "payload": {"type": ptype, "value": value}});
        self.ts += 1;
        o
    }

    pub async fn append(&mut self, ops: Vec<Value>) -> Result<Vec<Value>> {
        if ops.is_empty() {
            return Ok(Vec::new());
        }
        let Some(client) = self.client else {
            let acks: Vec<Value> = ops.iter().map(|o| json!({"id": o["id"]})).collect();
            self.state.add_ops(ops.iter().cloned());
            self.staged.extend(ops);
            return Ok(acks);
        };
        let acks = client
            .http
            .append_ops(&self.note_id, self.site(), &ops)
            .await?;
        let applied: Vec<Value> = acks
            .iter()
            .zip(&ops)
            .map(|(a, o)| {
                if a["timestampedOp"].is_object() {
                    a["timestampedOp"].clone()
                } else {
                    o.clone()
                }
            })
            .collect();
        self.state.add_ops(applied);
        Ok(acks)
    }

    pub(crate) async fn emit(&mut self, ops: Vec<Value>, mut out: Value) -> Result<Value> {
        let acks = self.append(ops).await?;
        out["op_ids"] = op_ids(&acks);
        Ok(out)
    }

    // ------------------------------------------------------------- metadata
    pub async fn rename(&mut self, title: &str) -> Result<Value> {
        let old = self.state.title();
        let o = self.op("SetMetadata", json!({"title": {"value": title}}));
        self.emit(vec![o], json!({"old_title": old, "title": title}))
            .await
    }

    pub async fn set_layout(&mut self, mode: &str) -> Result<Value> {
        let Some((_, m)) = LAYOUT_MODES.iter().find(|(k, _)| *k == mode) else {
            return Err(NbError::usage(
                "layout must be one of ['paged', 'pageless']",
            ));
        };
        let o = self.op("SetMetadata", json!({"layoutMode": m}));
        self.emit(vec![o], json!({"layout": mode})).await
    }

    // ------------------------------------------------------------- text
    pub async fn add_text(&mut self, text: &str, at_start: bool) -> Result<Value> {
        if text.is_empty() {
            return Err(NbError::usage("text is empty"));
        }
        let chars = self.state.text_chars(None);
        let mut v = json!({"string": text});
        let after = match chars.last() {
            Some(last) if !at_start => {
                v["location"] = seqd(last.0);
                Some(fmt_seq(last.0))
            }
            _ => None,
        };
        let o = self.op("InsertString", v);
        self.emit(vec![o], json!({"inserted": text, "after": after}))
            .await
    }

    pub async fn add_text_box(
        &mut self,
        text: &str,
        page: usize,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: f64,
    ) -> Result<Value> {
        if text.is_empty() {
            return Err(NbError::usage("text is empty"));
        }
        let pg = self.state.page(page)?;
        let longest = text
            .lines()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(text.chars().count());
        let w = width.unwrap_or_else(|| (8.0 * longest as f64 + 10.0).max(20.0));
        let margins: Map<String, Value> = TEXTBOX_MARGINS
            .iter()
            .map(|(k, v)| ((*k).to_string(), json!(v)))
            .collect();
        let block = self.op(
            "CreateBlock",
            json!({"page": seqd(pg.id), "origin": point(x, y), "size": {"width": w, "height": height},
                   "resizesWidthToFitText": width.is_none(), "margins": margins}),
        );
        let ins = self.op(
            "InsertString",
            json!({"string": text, "textField": block["id"].clone()}),
        );
        let box_id = id_of(&block["id"]).map(fmt_id);
        self.emit(
            vec![block, ins],
            json!({"box_id": box_id, "page": page, "text": text}),
        )
        .await
    }

    fn text_field(&self, field: Option<&str>) -> Result<Option<Id>> {
        let Some(f) = field else { return Ok(None) };
        let id = parse_id(f)?;
        match self.state.block(id) {
            Some(b) if b["kind"] == "text" => Ok(Some(id)),
            _ => Err(NbError::not_found(format!("no text box {f}"))),
        }
    }

    fn span(&self, field: Option<Id>, sel: &Selection) -> Result<Span> {
        let chars = self.state.text_chars(field);
        if chars.is_empty() {
            return Err(NbError::usage("the text is empty; nothing to select"));
        }
        let n = chars.len();
        let (start, end, all) = match sel {
            Selection::All => (0, n, true),
            Selection::Range(a, b) => {
                if !(a < b && *b <= n) {
                    return Err(NbError::usage(format!(
                        "range must satisfy 0 <= start < end <= {n}"
                    )));
                }
                (*a, *b, false)
            }
            Selection::Match(needle, nth) => {
                let hay: Vec<char> = chars.iter().map(|c| c.1).collect();
                let pat: Vec<char> = needle.chars().collect();
                if pat.is_empty() {
                    return Err(NbError::usage("--match text is empty"));
                }
                let hits: Vec<usize> = (0..=n.saturating_sub(pat.len()))
                    .filter(|&i| hay[i..].starts_with(&pat))
                    .collect();
                let Some(&i) = hits.get(nth.saturating_sub(1)) else {
                    return Err(NbError::not_found(format!(
                        "'{needle}' occurs {} time(s); occurrence {nth} not found",
                        hits.len()
                    )));
                };
                (i, i + pat.len(), false)
            }
            Selection::Paragraphs(a, b) => {
                let starts: Vec<usize> = std::iter::once(0)
                    .chain(
                        chars
                            .iter()
                            .enumerate()
                            .filter(|c| c.1.1 == '\n')
                            .map(|c| c.0),
                    )
                    .collect();
                let k = starts.len();
                if !(1 <= *a && a <= b && *b <= k) {
                    return Err(NbError::usage(format!("paragraphs are 1..{k}")));
                }
                let end = starts.get(*b).copied().unwrap_or(n);
                (starts[a - 1], end, false)
            }
        };
        Ok(Span {
            chars,
            start,
            end,
            all,
        })
    }

    fn with_field(v: &mut Value, field: Option<Id>) {
        if let Some(f) = field {
            v["textField"] = idd(f);
        }
    }

    /// One `ModifyStyle` per attribute (as the web editor writes them). `Value::Null` resets.
    pub async fn style(
        &mut self,
        field: Option<&str>,
        sel: &Selection,
        attrs: &[(String, Value)],
    ) -> Result<Value> {
        if attrs.is_empty() {
            return Err(NbError::usage("no style attribute given"));
        }
        let f = self.text_field(field)?;
        let sp = self.span(f, sel)?;
        let (s, e) = sp.boundaries();
        let mut ops = Vec::new();
        for (field_name, v) in attrs {
            let mut val = json!({"start": s, "end": e});
            Self::with_field(&mut val, f);
            val[field_name.as_str()] = if v.is_null() {
                json!({})
            } else {
                json!({"value": v})
            };
            ops.push(self.op("ModifyStyle", val));
        }
        let applied: Vec<&str> = attrs.iter().map(|a| a.0.as_str()).collect();
        self.emit(
            ops,
            json!({"text": sp.text(), "start": sp.start, "end": sp.end, "applied": applied}),
        )
        .await
    }

    /// The web editor's link: reset underline/colour, then set `link`.
    pub async fn link(&mut self, field: Option<&str>, sel: &Selection, url: &str) -> Result<Value> {
        let f = self.text_field(field)?;
        let sp = self.span(f, sel)?;
        let (s, e) = sp.boundaries();
        let mut a = json!({"start": s, "end": e, "underline": {}, "foregroundColor": {}});
        Self::with_field(&mut a, f);
        let mut b = json!({"start": s, "end": e, "link": {"value": url}});
        Self::with_field(&mut b, f);
        let ops = vec![self.op("ModifyStyle", a), self.op("ModifyStyle", b)];
        self.emit(ops, json!({"text": sp.text(), "link": url}))
            .await
    }

    pub async fn clear_style(
        &mut self,
        field: Option<&str>,
        sel: &Selection,
        paragraph: bool,
    ) -> Result<Value> {
        let f = self.text_field(field)?;
        let sp = self.span(f, sel)?;
        let (s, e) = sp.boundaries();
        let mut v = json!({"start": s, "end": e});
        if paragraph {
            v["paragraph"] = json!(true);
        }
        Self::with_field(&mut v, f);
        let o = self.op("ClearStyle", v);
        self.emit(
            vec![o],
            json!({"text": sp.text(), "cleared": true, "paragraph": paragraph}),
        )
        .await
    }

    /// `ModifyParagraphStyle` over every paragraph the selection touches. Keys of `attrs` are op
    /// fields (`decoratorStyle`, `programmingLanguage`, `isChecked`, `indentLevel`, `alignment`,
    /// `lineSpacing`); `Value::Null` resets.
    pub async fn paragraph_style(
        &mut self,
        field: Option<&str>,
        sel: &Selection,
        attrs: &[(String, Value)],
    ) -> Result<Value> {
        if attrs.is_empty() {
            return Err(NbError::usage("no paragraph attribute given"));
        }
        let f = self.text_field(field)?;
        let sp = self.span(f, sel)?;
        let paras = self.state.paragraphs(f);
        let (a, b) = (sp.para_of(sp.start), sp.para_of(sp.end - 1));
        let (Some(first), Some(last)) = (paras.get(a), paras.get(b)) else {
            return Err(NbError::usage("selection is outside the text"));
        };
        let mut v = json!({"start": seqd(first.key)});
        if b != a {
            v["end"] = seqd(last.key);
        }
        Self::with_field(&mut v, f);
        for (k, x) in attrs {
            v[k.as_str()] = if x.is_null() {
                json!({})
            } else {
                json!({"value": x})
            };
        }
        let o = self.op("ModifyParagraphStyle", v);
        let applied: Vec<&str> = attrs.iter().map(|a| a.0.as_str()).collect();
        self.emit(
            vec![o],
            json!({"paragraphs": [a + 1, b + 1], "applied": applied}),
        )
        .await
    }

    pub async fn set_checked(
        &mut self,
        field: Option<&str>,
        paragraph: usize,
        checked: bool,
    ) -> Result<Value> {
        let f = self.text_field(field)?;
        let paras = self.state.paragraphs(f);
        let Some(p) = paragraph.checked_sub(1).and_then(|i| paras.get(i)) else {
            return Err(NbError::usage(format!("paragraphs are 1..{}", paras.len())));
        };
        if p.style.decorator != Some(3) {
            return Err(NbError::usage(format!(
                "paragraph {paragraph} is not a checklist item"
            )));
        }
        let mut v = json!({"location": seqd(p.key), "isChecked": checked});
        Self::with_field(&mut v, f);
        let text = p.text.clone();
        let o = self.op("UpdateCheckbox", v);
        self.emit(
            vec![o],
            json!({"paragraph": paragraph, "text": text, "checked": checked}),
        )
        .await
    }

    /// Append `code` as new paragraph(s) styled as a code block.
    pub async fn add_code(&mut self, code: &str, language: &str) -> Result<Value> {
        if code.is_empty() {
            return Err(NbError::usage("code is empty"));
        }
        if !CODE_LANGUAGES.contains(&language) {
            return Err(NbError::usage(format!(
                "language must be one of {}",
                CODE_LANGUAGES.join(", ")
            )));
        }
        let chars = self.state.text_chars(None);
        let s: String = if chars.is_empty() {
            code.to_string()
        } else {
            format!("\n{code}")
        };
        let mut ins = json!({"string": s});
        if let Some(last) = chars.last() {
            ins["location"] = seqd(last.0);
        }
        let ins = self.op("InsertString", ins);
        let (site, ts) = id_of(&ins["id"]).unwrap_or_default();
        let last_nl = s
            .chars()
            .enumerate()
            .filter(|c| c.1 == '\n')
            .map(|c| c.0 as u64)
            .last()
            .unwrap_or(0);
        let mut style = json!({"start": seqd((site, ts, 0)), "decoratorStyle": {"value": 5},
                               "programmingLanguage": {"value": language}});
        if last_nl > 0 {
            style["end"] = seqd((site, ts, last_nl));
        }
        let st = self.op("ModifyParagraphStyle", style);
        self.emit(vec![ins, st], json!({"code": code, "language": language}))
            .await
    }

    // ------------------------------------------------------------- math
    pub async fn add_math(
        &mut self,
        latex: &str,
        page: usize,
        x: f64,
        y: f64,
        color: Option<Value>,
    ) -> Result<Value> {
        if latex.trim().is_empty() {
            return Err(NbError::usage("latex is empty"));
        }
        let pg = self.state.page(page)?;
        let w = (9.0 * latex.chars().count() as f64).clamp(40.0, 480.0);
        let o = self.op(
            "CreateBlock",
            json!({"type": BLOCK_MATH, "page": seqd(pg.id), "origin": point(x, y), "size": {"width": w, "height": 40.0},
                   "mathLatex": latex, "mathColor": color.unwrap_or_else(|| color_value((0, 0, 0, 255)))}),
        );
        let id = id_of(&o["id"]).map(fmt_id);
        self.emit(
            vec![o],
            json!({"math_id": id, "page": page, "latex": latex}),
        )
        .await
    }

    fn block_of_kind(&self, id: &str, kind: u64, what: &str) -> Result<(Id, Value)> {
        let bid = parse_id(id)?;
        match self.state.block(bid) {
            Some(b) if b["type"] == kind => Ok((bid, b)),
            _ => Err(NbError::not_found(format!("no {what} {id}"))),
        }
    }

    pub async fn edit_math(
        &mut self,
        id: &str,
        latex: Option<&str>,
        color: Option<Value>,
    ) -> Result<Value> {
        let (bid, old) = self.block_of_kind(id, BLOCK_MATH, "math block")?;
        if latex.is_none() && color.is_none() {
            return Err(NbError::usage(
                "nothing to change: pass --latex and/or --color",
            ));
        }
        let mut v = json!({"blocks": [idd(bid)]});
        if let Some(l) = latex {
            v["mathLatex"] = json!({"value": l});
        }
        if let Some(c) = color {
            v["mathColor"] = json!({"value": c});
        }
        let o = self.op("ModifyBlock", v);
        self.emit(
            vec![o],
            json!({"math_id": id, "old_latex": old["latex"], "latex": latex}),
        )
        .await
    }

    // ------------------------------------------------------------- images
    pub async fn crop_image(&mut self, id: &str, x: f64, y: f64, w: f64, h: f64) -> Result<Value> {
        let (bid, _) = self.block_of_kind(id, BLOCK_IMAGE, "image")?;
        if w <= 0.0 || h <= 0.0 {
            return Err(NbError::usage("crop width and height must be > 0"));
        }
        let rect = json!({"origin": point(x, y), "size": {"width": w, "height": h}});
        let o = self.op(
            "ModifyBlock",
            json!({"blocks": [idd(bid)], "cropRect": {"value": rect}}),
        );
        self.emit(vec![o], json!({"image_id": id, "crop": rect}))
            .await
    }

    /// Toggle the image's horizontal and/or vertical flip.
    pub async fn flip_image(
        &mut self,
        id: &str,
        horizontal: bool,
        vertical: bool,
    ) -> Result<Value> {
        let (bid, b) = self.block_of_kind(id, BLOCK_IMAGE, "image")?;
        if !horizontal && !vertical {
            return Err(NbError::usage("pass --horizontal and/or --vertical"));
        }
        let mut v = json!({"blocks": [idd(bid)]});
        let fh = b["flipped_h"].as_bool().unwrap_or(false) ^ horizontal;
        let fv = b["flipped_v"].as_bool().unwrap_or(false) ^ vertical;
        if horizontal {
            v["imageFlippedHorizontally"] = json!(fh);
        }
        if vertical {
            v["imageFlippedVertically"] = json!(fv);
        }
        let o = self.op("ModifyBlock", v);
        self.emit(
            vec![o],
            json!({"image_id": id, "flipped_h": fh, "flipped_v": fv}),
        )
        .await
    }

    // ------------------------------------------------------------- ink & shapes
    /// A stroke through `points` (page coordinates), as straight cubic segments.
    pub async fn draw_ink(
        &mut self,
        page: usize,
        points: &[(f64, f64)],
        highlighter: bool,
        color: Option<Value>,
        width: Option<f64>,
    ) -> Result<Value> {
        if points.len() < 2 {
            return Err(NbError::usage("a stroke needs at least two points"));
        }
        let pg = self.state.page(page)?;
        let (ox, oy) = points[0];
        let w = width.unwrap_or(if highlighter { 12.0 } else { 2.0 });
        let el = |kind: u8, pts: Vec<(f64, f64)>| PathElement {
            kind,
            points: pts,
            width: Some(w * 0.645),
            force: Some(1.0),
            altitude: Some(FRAC_PI_2),
            azimuth: Some(0.0),
        };
        let mut elements = vec![el(3, vec![(0.0, 0.0)])];
        for pair in points.windows(2) {
            let (a, b) = (
                (pair[0].0 - ox, pair[0].1 - oy),
                (pair[1].0 - ox, pair[1].1 - oy),
            );
            let lerp = |t: f64| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            elements.push(el(0, vec![lerp(1.0 / 3.0), lerp(2.0 / 3.0), b]));
        }
        let path = inkpath::encode(&elements, None)
            .map_err(|e| NbError::usage(format!("stroke: {}", e.0)))?;
        let default = if highlighter {
            HIGHLIGHT_COLOR
        } else {
            INK_COLOR
        };
        let mut v = json!({
            "page": seqd(pg.id), "origin": point(ox, oy), "style": 1,
            "color": color.unwrap_or_else(|| color_value(default)), "width": w,
            "encodedCenterPath": path,
            "styleMap": [{"backingPencilSeed": 0, "backingPencilReferencePoint": point(0.0, 0.0),
                          "backingDashPhase": 0.0, "backingDashPeriod": 0.0}],
        });
        if highlighter {
            v["tool"] = json!(2);
            v["zIndex"] = json!(999_999);
        }
        let o = self.op("CreateInk", v);
        let id = id_of(&o["id"]).map(fmt_id);
        let tool = if highlighter { "highlighter" } else { "pen" };
        self.emit(
            vec![o],
            json!({"ink_id": id, "page": page, "tool": tool, "points": points.len()}),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_shape(
        &mut self,
        kind: &str,
        page: usize,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        color: Option<Value>,
        border: f64,
    ) -> Result<Value> {
        let pg = self.state.page(page)?;
        let poly = |pts: &[(f64, f64)]| json!({"type": "Polygon", "value": {"points": pts.iter().map(|p| point(p.0, p.1)).collect::<Vec<_>>()}});
        let def = match kind {
            "rectangle" => poly(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]),
            "triangle" => poly(&[(w / 2.0, 0.0), (w, h), (0.0, h)]),
            "ellipse" => {
                json!({"type": "NormalShape", "value": {"size": {"width": w, "height": h}}})
            }
            "line" => {
                json!({"type": "Line", "value": {"start": point(0.0, 0.0), "end": point(w, h)}})
            }
            "arrow" => {
                json!({"type": "Line", "value": {"start": point(0.0, 0.0), "end": point(w, h), "arrowHead": 1}})
            }
            _ => {
                return Err(NbError::usage(
                    "shape must be one of rectangle, ellipse, triangle, line, arrow",
                ));
            }
        };
        let o = self.op(
            "CreateShape",
            json!({"page": seqd(pg.id), "origin": point(x, y), "color": color.unwrap_or_else(|| color_value(INK_COLOR)),
                   "borderWidth": border, "definition": def}),
        );
        let id = id_of(&o["id"]).map(fmt_id);
        self.emit(vec![o], json!({"shape_id": id, "page": page, "kind": kind}))
            .await
    }

    /// Current `(page seq, origin)` of every movable canvas entity (blocks, ink, shapes).
    fn placements(&self) -> HashMap<String, Placement> {
        let pages: Vec<Page> = self.state.pages();
        let seq = |n: &Value| {
            n.as_u64()
                .and_then(|n| pages.get((n as usize).wrapping_sub(1)))
                .map(|p| p.id)
        };
        let org = |v: &Value| {
            (
                v["origin"]["x"].as_f64().unwrap_or(0.0),
                v["origin"]["y"].as_f64().unwrap_or(0.0),
            )
        };
        let mut out = HashMap::new();
        for (list, kind) in [
            (self.state.blocks(), "block"),
            (self.state.entities("CreateInk"), "ink"),
            (self.state.entities("CreateShape"), "shape"),
        ] {
            for v in list {
                if let Some(id) = v["id"].as_str() {
                    out.insert(id.to_string(), (seq(&v["page"]), org(&v), kind));
                }
            }
        }
        out
    }

    /// Move entities by `(dx, dy)`, optionally onto another page (`ModifyPositions`).
    pub async fn move_entities(
        &mut self,
        ids: &[String],
        dx: f64,
        dy: f64,
        to_page: Option<usize>,
    ) -> Result<Value> {
        if ids.is_empty() {
            return Err(NbError::usage("no entity ids given"));
        }
        let place = self.placements();
        let target_page = to_page.map(|p| self.state.page(p)).transpose()?;
        let mut mods = Vec::new();
        let mut seen = HashSet::new();
        for id in ids {
            let Some((pg, (x, y), _)) = place.get(id) else {
                return Err(NbError::not_found(format!(
                    "no movable entity {id} (text boxes, math, images, ink, shapes)"
                )));
            };
            if !seen.insert(id) {
                continue;
            }
            let pg = target_page.as_ref().map(|p| p.id).or(*pg);
            let mut m = json!({"target": idd(parse_id(id)?), "origin": point(x + dx, y + dy)});
            if let Some(p) = pg {
                m["page"] = seqd(p);
            }
            mods.push(m);
        }
        let o = self.op("ModifyPositions", json!({"modifications": mods}));
        self.emit(
            vec![o],
            json!({"moved": ids, "dx": dx, "dy": dy, "to_page": to_page}),
        )
        .await
    }

    /// Erase ink strokes (or any canvas entity) by id.
    pub async fn erase(&mut self, ids: &[String], kinds: &[&str]) -> Result<Value> {
        let place = self.placements();
        let mut targets = Vec::new();
        for id in ids {
            match place.get(id) {
                Some((_, _, k)) if kinds.contains(k) => targets.push(idd(parse_id(id)?)),
                _ => {
                    return Err(NbError::not_found(format!(
                        "no live {} {id}",
                        kinds.join("/")
                    )));
                }
            }
        }
        if targets.is_empty() {
            return Err(NbError::usage("no ids given"));
        }
        let o = self.op("DeleteEntities", json!({"entityDeletes": targets}));
        self.emit(vec![o], json!({"deleted": ids})).await
    }

    // ------------------------------------------------------------- comments
    fn known_comments(&self) -> HashMap<String, Value> {
        let mut known = HashMap::new();
        for t in self.state.comments() {
            for r in t["replies"].as_array().into_iter().flatten() {
                if let Some(id) = r["id"].as_str() {
                    known.insert(id.to_string(), r.clone());
                }
            }
            if let Some(id) = t["id"].as_str() {
                known.insert(id.to_string(), t);
            }
        }
        known
    }

    pub async fn add_comment(
        &mut self,
        text: &str,
        page: usize,
        x: f64,
        y: f64,
        reply_to: Option<&str>,
    ) -> Result<Value> {
        if text.is_empty() {
            return Err(NbError::usage("comment text is empty"));
        }
        let anchor = if let Some(r) = reply_to {
            let root = parse_id(r)?;
            if !self
                .state
                .comments()
                .iter()
                .any(|c| c["id"] == fmt_id(root).as_str())
            {
                return Err(NbError::not_found(format!("no comment thread {r}")));
            }
            json!({"type": "ReplyAnchor", "value": {"root": idd(root)}})
        } else {
            let pg = self.state.page(page)?;
            json!({"type": "CanvasAnchor", "value": {"page": seqd(pg.id), "origin": point(x, y)}})
        };
        let o = self.op("CreateComment", json!({"text": text, "anchor": anchor}));
        let id = id_of(&o["id"]).map(fmt_id);
        self.emit(
            vec![o],
            json!({"comment_id": id, "reply_to": reply_to, "text": text}),
        )
        .await
    }

    pub async fn delete_comment(&mut self, cid: &str) -> Result<Value> {
        let target = parse_id(cid)?;
        if !self.known_comments().contains_key(&fmt_id(target)) {
            return Err(NbError::not_found(format!("no live comment {cid}")));
        }
        let o = self.op("DeleteEntities", json!({"entityDeletes": [idd(target)]}));
        self.emit(vec![o], json!({"deleted": fmt_id(target)})).await
    }

    pub async fn edit_comment(&mut self, cid: &str, text: &str) -> Result<Value> {
        let target = parse_id(cid)?;
        let Some(old) = self.known_comments().remove(&fmt_id(target)) else {
            return Err(NbError::not_found(format!("no live comment {cid}")));
        };
        if text.is_empty() {
            return Err(NbError::usage("comment text is empty"));
        }
        let o = self.op(
            "ModifyComment",
            json!({"comment": idd(target), "text": {"value": text}}),
        );
        self.emit(
            vec![o],
            json!({"comment_id": fmt_id(target), "old_text": old["text"], "text": text}),
        )
        .await
    }

    /// Re-pin a canvas comment thread (`ModifyComment.anchor` is a plain `CanvasAnchor`).
    pub async fn move_comment(&mut self, cid: &str, page: usize, x: f64, y: f64) -> Result<Value> {
        let target = parse_id(cid)?;
        let Some(c) = self.known_comments().remove(&fmt_id(target)) else {
            return Err(NbError::not_found(format!("no live comment {cid}")));
        };
        if c["anchor_type"] != "CanvasAnchor" {
            return Err(NbError::usage(format!(
                "comment {cid} is not pinned to the canvas (only threads on a page can move)"
            )));
        }
        let pg = self.state.page(page)?;
        let o = self.op(
            "ModifyComment",
            json!({"comment": idd(target), "anchor": {"page": seqd(pg.id), "origin": point(x, y)}}),
        );
        self.emit(
            vec![o],
            json!({"comment_id": fmt_id(target), "page": page, "origin": point(x, y)}),
        )
        .await
    }

    // ------------------------------------------------------------- pages
    pub async fn add_pages(
        &mut self,
        after: Option<usize>,
        count: usize,
        template: Option<Value>,
    ) -> Result<Value> {
        let ps = self.state.pages();
        if count < 1 {
            return Err(NbError::usage("count must be >= 1"));
        }
        let after = after.unwrap_or(ps.len());
        if after > ps.len() {
            return Err(NbError::usage(format!("--after must be 0..{}", ps.len())));
        }
        let mut v = json!({"pageCount": count});
        if after > 0 {
            v["location"] = seqd(ps[after - 1].node);
        }
        if let Some(t) = template {
            v["background"] = t;
        }
        let o = self.op("CreatePage", v);
        let (s, t) = id_of(&o["id"]).unwrap_or_default();
        let new: Vec<String> = (0..count as u64).map(|i| fmt_seq((s, t, i))).collect();
        let mut out = self
            .emit(vec![o], json!({"added": new, "after_page": after}))
            .await?;
        out["page_count"] = json!(self.state.pages().len());
        Ok(out)
    }

    pub async fn move_page(&mut self, page: usize, to: usize) -> Result<Value> {
        let ps = self.state.pages();
        let n = ps.len();
        if !((1..=n).contains(&page) && (1..=n).contains(&to)) {
            return Err(NbError::usage(format!("pages are 1..{n}")));
        }
        let moved = fmt_seq(ps[page - 1].id);
        if page == to {
            return Ok(json!({"moved": moved, "from": page, "to": to, "op_ids": []}));
        }
        let rest: Vec<&Page> = ps.iter().filter(|p| p.index != page).collect();
        let move_to = if to > 1 {
            json!({"toId": seqd(rest[to - 2].node)})
        } else {
            json!({})
        };
        let o = self.op(
            "ModifyPage",
            json!({"pages": [seqd(ps[page - 1].id)], "moveTo": move_to}),
        );
        let mut out = self
            .emit(vec![o], json!({"moved": moved, "from": page, "to": to}))
            .await?;
        out["order"] = json!(
            self.state
                .pages()
                .iter()
                .map(|p| fmt_seq(p.id))
                .collect::<Vec<_>>()
        );
        Ok(out)
    }

    pub async fn delete_page(&mut self, page: usize) -> Result<Value> {
        let pg = self.state.page(page)?;
        let ents = self.state.entities_on_page(pg.id);
        let mut ops = vec![self.op(
            "DeleteEntities",
            json!({"entityDeletes": ents.iter().map(|e| idd(*e)).collect::<Vec<_>>(), "pageDeletes": [seqd(pg.id)]}),
        )];
        if self.state.pages().len() == 1 {
            ops.push(self.op("CreatePage", json!({"pageCount": 1})));
        }
        let ents: Vec<String> = ents.into_iter().map(fmt_id).collect();
        let mut out = self
            .emit(
                ops,
                json!({"deleted_page": fmt_seq(pg.id), "deleted_entities": ents}),
            )
            .await?;
        out["page_count"] = json!(self.state.pages().len());
        Ok(out)
    }

    pub async fn set_template(&mut self, paper: Value, page: Option<usize>) -> Result<Value> {
        let o = match page {
            None => {
                let mut bg = self.state.note_background();
                bg["paper"] = paper.clone();
                self.op("SetMetadata", json!({"pageBackground": {"value": bg}}))
            }
            Some(n) => {
                let pg = self.state.page(n)?;
                let mut bg = pg
                    .background
                    .clone()
                    .unwrap_or_else(|| self.state.note_background());
                bg["paper"] = paper.clone();
                self.op(
                    "ModifyPage",
                    json!({"pages": [seqd(pg.id)], "background": {"value": bg}}),
                )
            }
        };
        let scope = page.map_or_else(|| "note".to_string(), |n| format!("page {n}"));
        self.emit(vec![o], json!({"scope": scope, "paper": paper}))
            .await
    }
}

pub fn make_paper(style: &str, spacing: Option<f64>, color: Option<&str>) -> Result<Value> {
    let Some((_, flair)) = FLAIRS.iter().find(|(k, _)| *k == style) else {
        return Err(NbError::usage(
            "template must be one of plain, lines, dots, grid",
        ));
    };
    let mut p = json!({});
    if let Some(f) = flair {
        p["flair"] = json!(f);
    }
    if let Some(s) = spacing {
        p["flairSpacing"] = json!(s);
    }
    let c = color.unwrap_or("#FFFFFF").trim_start_matches('#');
    if c.len() != 6 || !c.is_ascii() {
        return Err(NbError::usage("color must be #RRGGBB"));
    }
    let b = |i: usize| {
        u8::from_str_radix(&c[i..i + 2], 16).map_err(|_| NbError::usage("color must be #RRGGBB"))
    };
    p["backgroundColor"] = json!({"bitsR": b(0)?, "bitsG": b(2)?, "bitsB": b(4)?, "bitsA": 255});
    Ok(p)
}

pub fn describe_paper(bg: Option<&Value>) -> Value {
    let p = bg.map(|b| &b["paper"]).filter(|p| p.is_object());
    let flair = p.and_then(|p| p["flair"].as_u64());
    let name = FLAIRS
        .iter()
        .find(|(_, f)| *f == flair)
        .map_or("plain", |(k, _)| k);
    let color = p
        .and_then(|p| p.get("backgroundColor"))
        .and_then(nb_protocol::codec::color_hex);
    json!({"template": name, "spacing": p.and_then(|p| p.get("flairSpacing")), "color": color})
}

/// The two ops the web client writes for a brand-new note (site 0).
pub fn initial_ops(title: &str) -> Vec<Value> {
    let now = now_ms();
    vec![
        json!({"id": {"site": 0, "timestamp": 0}, "clientTime": now, "payload": {"type": "CreatePage", "value": {"pageCount": 2}}}),
        json!({"id": {"site": 0, "timestamp": 1}, "clientTime": now, "payload": {"type": "SetMetadata", "value": {
            "title": {"value": title}, "pageBackground": {"value": default_background()},
            "defaultFontFamily": "Inter", "defaultFontSize": 14.0, "layoutMode": 1}}}),
    ]
}

fn is_id(v: &Value) -> bool {
    v.as_object().is_some_and(|o| {
        let keys: HashSet<&str> = o.keys().map(String::as_str).collect();
        keys == HashSet::from(["site", "timestamp"])
            || keys == HashSet::from(["site", "timestamp", "index"])
    })
}

/// Copy `ops` into a fresh log at site 0 (timestamps from `start`), keeping causal order and
/// references; transient and server-only ops are dropped.
pub fn rebase_ops(ops: &[Value], start: u64) -> Vec<Value> {
    let mut live: Vec<&Value> = ops
        .iter()
        .filter(|o| {
            o.get("payload").is_some()
                && !o["transientInteraction"].as_bool().unwrap_or(false)
                && !SERVER_ONLY_OPS.contains(&o["payload"]["type"].as_str().unwrap_or_default())
        })
        .collect();
    live.sort_by_key(|o| (o["id"]["timestamp"].as_u64(), o["id"]["site"].as_u64()));
    let remap: HashMap<(u64, u64), u64> = live
        .iter()
        .enumerate()
        .filter_map(|(i, o)| id_of(&o["id"]).map(|id| (id, start + i as u64)))
        .collect();
    fn walk(x: &Value, remap: &HashMap<(u64, u64), u64>) -> Value {
        if is_id(x) {
            let k = (
                x["site"].as_u64().unwrap_or(0),
                x["timestamp"].as_u64().unwrap_or(0),
            );
            if let Some(t) = remap.get(&k) {
                let mut y = json!({"site": 0, "timestamp": t});
                if let Some(i) = x.get("index") {
                    y["index"] = i.clone();
                }
                return y;
            }
            return x.clone();
        }
        match x {
            Value::Object(o) => {
                Value::Object(o.iter().map(|(k, v)| (k.clone(), walk(v, remap))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(|v| walk(v, remap)).collect()),
            _ => x.clone(),
        }
    }
    let now = now_ms();
    live.iter()
        .enumerate()
        .map(|(i, o)| {
            let mut n = json!({"id": {"site": 0, "timestamp": start + i as u64}, "clientTime": now,
                               "payload": walk(&o["payload"], &remap)});
            if o["audioTime"].as_u64().unwrap_or(0) > 0 {
                n["audioTime"] = o["audioTime"].clone();
            }
            n
        })
        .collect()
}

/// The web client's Duplicate: `createNote`, then append the copied log.
pub async fn duplicate_note(client: &Client, note_ref: &str, title: Option<&str>) -> Result<Value> {
    let src = client.resolve_note(note_ref).await?;
    let st = NoteState::new(&client.http.note_bundle(&src.id).await?);
    let new_title = title.map_or_else(
        || format!("{} copy", st.title().unwrap_or_else(|| src.title.clone())),
        str::to_string,
    );
    let mut ops = rebase_ops(&st.ops, 0);
    ops.push(
        json!({"id": {"site": 0, "timestamp": ops.len()}, "clientTime": now_ms(),
                    "payload": {"type": "SetMetadata", "value": {"title": {"value": new_title}}}}),
    );
    let folder = src.folder_id.clone();
    let mut res = client
        .create_note(&new_title, folder.as_deref(), ops)
        .await?;
    res["source_id"] = json!(src.id);
    res["ops_copied"] = res["append"]["acks"].clone();
    Ok(res)
}

/// Decorator name to its `decoratorStyle` value (`none` is `Value::Null`: remove the list).
pub fn decorator(name: &str) -> Result<Value> {
    if name == "none" {
        return Ok(Value::Null);
    }
    DECORATORS
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| json!(v))
        .ok_or_else(|| {
            NbError::usage("list must be one of bullet, number, check, quote, code, none")
        })
}

pub fn heading_attrs(level: &str) -> Result<Vec<(String, Value)>> {
    let Some((_, size)) = FONT_PRESETS.iter().find(|(k, _)| *k == level) else {
        return Err(NbError::usage(
            "heading must be one of title, h1, h2, h3, body, caption",
        ));
    };
    let bold = !matches!(level, "body" | "caption");
    let mut a = vec![
        ("size".to_string(), json!(size)),
        ("bold".to_string(), json!(bold)),
    ];
    for f in [
        "italic",
        "underline",
        "superscript",
        "subscript",
        "strikethrough",
    ] {
        a.push((f.to_string(), json!(false)));
    }
    Ok(a)
}

pub fn alignment(name: &str) -> Result<u64> {
    ALIGNMENTS
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| *v)
        .ok_or_else(|| NbError::usage("align must be one of natural, left, center, right"))
}

/// `x,y;x,y;...` (or space-separated pairs) to points.
pub fn parse_points(s: &str) -> Result<Vec<(f64, f64)>> {
    s.split([';', ' '])
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (x, y) = p
                .split_once(',')
                .ok_or_else(|| NbError::usage(format!("bad point '{p}' (expected X,Y)")))?;
            let f = |v: &str| {
                v.trim()
                    .parse::<f64>()
                    .map_err(|_| NbError::usage(format!("bad point '{p}'")))
            };
            Ok((f(x)?, f(y)?))
        })
        .collect()
}

pub fn seq_label(v: &Value) -> Option<String> {
    seq_of(v).map(fmt_seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> EditSession<'static> {
        let mut s = EditSession::new_note("sample-RS unit");
        rt(s.add_text("Hello world\nsecond line\nthird", false)).unwrap();
        s
    }

    fn rt<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn new_note_seeds_two_pages_and_metadata() {
        let s = EditSession::new_note("T");
        assert_eq!(s.staged.len(), 2);
        assert_eq!(s.state.pages().len(), 2);
        assert_eq!(s.state.title().as_deref(), Some("T"));
        assert_eq!(s.state.layout(), "pageless");
    }

    #[test]
    fn style_match_writes_one_op_per_attr_with_boundaries() {
        let mut s = session();
        let attrs = vec![
            ("bold".to_string(), json!(true)),
            ("highlight".to_string(), color_value(HIGHLIGHT_COLOR)),
        ];
        let r = rt(s.style(None, &Selection::Match("world".into(), 1), &attrs)).unwrap();
        assert_eq!(r["text"], "world");
        let ops: Vec<&Value> = s
            .staged
            .iter()
            .filter(|o| o["payload"]["type"] == "ModifyStyle")
            .collect();
        assert_eq!(ops.len(), 2);
        let v = &ops[0]["payload"]["value"];
        assert_eq!(v["start"]["type"], 0);
        assert_eq!(v["end"]["type"], 1);
        assert_eq!(v["bold"]["value"], true);
        assert_eq!(
            ops[1]["payload"]["value"]["highlight"]["value"]["bitsA"],
            89
        );
    }

    #[test]
    fn all_selection_ends_at_end_of_doc() {
        let mut s = session();
        rt(s.style(None, &Selection::All, &[("italic".into(), Value::Null)])).unwrap();
        let v = &s.staged.last().unwrap()["payload"]["value"];
        assert_eq!(v["end"], json!({"type": 3}));
        assert_eq!(v["italic"], json!({}));
    }

    #[test]
    fn paragraph_style_and_checkbox_round_trip() {
        let mut s = session();
        rt(s.paragraph_style(
            None,
            &Selection::Paragraphs(2, 3),
            &[("decoratorStyle".into(), json!(3))],
        ))
        .unwrap();
        let paras = s.state.paragraphs(None);
        assert_eq!(paras[1].style.decorator, Some(3));
        assert_eq!(paras[2].style.decorator, Some(3));
        assert_eq!(paras[0].style.decorator, None);
        rt(s.set_checked(None, 2, true)).unwrap();
        assert_eq!(s.state.paragraphs(None)[1].style.checked, Some(true));
        assert!(rt(s.set_checked(None, 1, true)).is_err());
    }

    #[test]
    fn code_block_gets_language() {
        let mut s = session();
        rt(s.add_code("fn main() {}\nlet x = 1;", "rust")).unwrap();
        let paras = s.state.paragraphs(None);
        let code: Vec<_> = paras
            .iter()
            .filter(|p| p.style.decorator == Some(5))
            .collect();
        assert_eq!(code.len(), 2);
        assert_eq!(code[0].style.language.as_deref(), Some("rust"));
        assert!(rt(s.add_code("x", "klingon")).is_err());
    }

    #[test]
    fn math_ink_shapes_move_erase() {
        let mut s = session();
        rt(s.add_math("x^2", 1, 50.0, 60.0, None)).unwrap();
        let m = s
            .state
            .blocks()
            .into_iter()
            .find(|b| b["kind"] == "math")
            .unwrap();
        let mid = m["id"].as_str().unwrap().to_string();
        rt(s.edit_math(&mid, Some("y^3"), None)).unwrap();
        assert_eq!(
            s.state.block(parse_id(&mid).unwrap()).unwrap()["latex"],
            "y^3"
        );

        rt(s.draw_ink(
            1,
            &[(10.0, 10.0), (60.0, 40.0), (90.0, 10.0)],
            false,
            None,
            None,
        ))
        .unwrap();
        rt(s.draw_ink(1, &[(10.0, 100.0), (200.0, 100.0)], true, None, None)).unwrap();
        let inks = s.state.entities("CreateInk");
        assert_eq!(inks.len(), 2);
        assert_eq!(inks[0]["points"], 3);
        assert_eq!(inks[1]["tool"], "highlighter");

        for k in ["rectangle", "ellipse", "triangle", "line", "arrow"] {
            rt(s.add_shape(k, 1, 300.0, 300.0, 80.0, 64.0, None, 2.0)).unwrap();
        }
        let kinds: Vec<String> = s
            .state
            .entities("CreateShape")
            .iter()
            .map(|e| e["kind"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["rectangle", "ellipse", "triangle", "line", "arrow"]);

        let ink = inks[0]["id"].as_str().unwrap().to_string();
        rt(s.move_entities(&[ink.clone(), mid.clone()], 5.0, -5.0, Some(2))).unwrap();
        let moved = s
            .state
            .entities("CreateInk")
            .into_iter()
            .find(|e| e["id"] == ink.as_str())
            .unwrap();
        assert_eq!(moved["origin"], point(15.0, 5.0));
        assert_eq!(moved["page"], 2);
        assert!(rt(s.erase(std::slice::from_ref(&mid), &["ink"])).is_err());
        rt(s.erase(&[ink], &["ink"])).unwrap();
        assert_eq!(s.state.entities("CreateInk").len(), 1);
    }

    #[test]
    fn comments_edit_and_move() {
        let mut s = session();
        let r = rt(s.add_comment("first", 1, 10.0, 10.0, None)).unwrap();
        let cid = r["comment_id"].as_str().unwrap().to_string();
        rt(s.edit_comment(&cid, "edited")).unwrap();
        rt(s.move_comment(&cid, 2, 40.0, 50.0)).unwrap();
        let c = &s.state.comments()[0];
        assert_eq!(c["text"], "edited");
        assert_eq!(c["page"], 2);
        assert_eq!(c["origin"], point(40.0, 50.0));
        let reply = rt(s.add_comment("re", 1, 0.0, 0.0, Some(&cid))).unwrap();
        assert!(rt(s.move_comment(reply["comment_id"].as_str().unwrap(), 1, 0.0, 0.0)).is_err());
    }

    #[test]
    fn pages_add_move_delete_and_rebase() {
        let mut s = session();
        rt(s.add_pages(Some(1), 2, None)).unwrap();
        assert_eq!(s.state.pages().len(), 4);
        let r = rt(s.move_page(4, 1)).unwrap();
        assert_eq!(r["order"][0], r["moved"]);
        rt(s.delete_page(1)).unwrap();
        assert_eq!(s.state.pages().len(), 3);
        let rebased = rebase_ops(&s.staged, 0);
        assert!(rebased.iter().all(|o| o["id"]["site"] == 0));
        let st = NoteState::new(&json!({"editorSite": 0, "ops": rebased}));
        assert_eq!(st.pages().len(), 3);
        assert_eq!(st.text(None), s.state.text(None));
    }

    #[test]
    fn paper_round_trips() {
        let p = make_paper("grid", Some(20.0), Some("#FFEEDD")).unwrap();
        let d = describe_paper(Some(&json!({"paper": p})));
        assert_eq!(
            d,
            json!({"template": "grid", "spacing": 20.0, "color": "#FFEEDD"})
        );
        assert!(make_paper("hex", None, None).is_err());
        assert_eq!(
            parse_points("1,2;3,4 5,6").unwrap(),
            vec![(1.0, 2.0), (3.0, 4.0), (5.0, 6.0)]
        );
        assert!(parse_color("#12345").is_err());
        assert_eq!(parse_color("#FFF60059").unwrap()["bitsA"], 89);
    }
}
